//! Bounded, token-scoped Jellyfin-shaped HLS VOD HTTP subset.
//! This does not implement dynamic HLS scheduling or Smart TV parity.
use crate::{app::AppState, auth_api, hls, playback, transcode::TranscodeController};
use axum::{
    Router,
    body::Body,
    extract::{Path as RoutePath, Query, State},
    http::{
        HeaderMap, HeaderValue, StatusCode,
        header::{CACHE_CONTROL, CONTENT_LENGTH, CONTENT_TYPE},
    },
    response::Response,
    routing::get,
};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{
    collections::VecDeque,
    fs, io,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{io::AsyncReadExt, sync::Semaphore};
use tokio_util::io::ReaderStream;
use uuid::Uuid;

const MAX_PUBLISHED: usize = 3;
const MAX_MANIFEST: usize = 32 * 1024;
const MAX_SEGMENT_BYTES: u64 = 128 * 1024 * 1024;

#[derive(Debug)]
struct Published {
    id: String,
    item: String,
    kind: &'static str,
    digest: [u8; 32],
    source: PathBuf,
    private_root: PathBuf,
    media_playlist: String,
    segments: Vec<(PathBuf, u64)>,
}
impl Drop for Published {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.private_root);
    }
}

/// Small private cache; a segment request holds its own Arc during eviction.
#[derive(Debug)]
pub struct HlsRegistry {
    jobs: Semaphore,
    controller: TranscodeController,
    entries: Mutex<VecDeque<Arc<Published>>>,
}
impl HlsRegistry {
    pub fn new(executable: PathBuf) -> Self {
        Self {
            jobs: Semaphore::new(1),
            controller: TranscodeController::new(
                executable,
                1,
                Duration::from_secs(120),
                MAX_SEGMENT_BYTES,
            )
            .expect("constant HLS worker limits"),
            entries: Mutex::new(VecDeque::new()),
        }
    }

    fn lookup(&self, id: &str, item: &str, kind: &str, token: &str) -> Option<Arc<Published>> {
        let digest: [u8; 32] = Sha256::digest(token.as_bytes()).into();
        self.entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .find(|entry| entry.id == id && entry.item == item && entry.kind == kind && entry.digest == digest)
            .cloned()
    }

    fn publish(&self, entry: Arc<Published>) {
        let removed = {
            let mut entries = self
                .entries
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            entries.push_back(entry);
            (entries.len() > MAX_PUBLISHED)
                .then(|| entries.pop_front())
                .flatten()
        };
        drop(removed);
    }
}

struct PrivateRoot {
    path: PathBuf,
    armed: bool,
}
impl Drop for PrivateRoot {
    fn drop(&mut self) {
        if self.armed {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}
#[cfg(unix)]
fn create_private_directory(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    fs::DirBuilder::new().mode(0o700).create(path)
}
#[cfg(not(unix))]
fn create_private_directory(path: &Path) -> io::Result<()> {
    fs::create_dir(path)
}

fn set_private_headers(response: &mut Response, mime: &'static str, size: usize) {
    response
        .headers_mut()
        .insert(CONTENT_TYPE, HeaderValue::from_static(mime));
    response
        .headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("private, no-store"));
    response.headers_mut().insert(
        CONTENT_LENGTH,
        HeaderValue::from_str(&size.to_string()).expect("length is numeric"),
    );
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PlaylistQuery {
    segment_length: Option<u32>,
}

async fn video_main(
    State(state): State<Arc<AppState>>,
    RoutePath(item): RoutePath<String>,
    Query(query): Query<PlaylistQuery>,
    headers: HeaderMap,
) -> Result<Response, StatusCode> {
    generate_hls_playlist(state, item, "video", query, headers, false).await
}

async fn video_master(
    State(state): State<Arc<AppState>>,
    RoutePath(item): RoutePath<String>,
    Query(query): Query<PlaylistQuery>,
    headers: HeaderMap,
) -> Result<Response, StatusCode> {
    generate_hls_playlist(state, item, "video", query, headers, true).await
}

async fn audio_main(
    State(state): State<Arc<AppState>>,
    RoutePath(item): RoutePath<String>,
    Query(query): Query<PlaylistQuery>,
    headers: HeaderMap,
) -> Result<Response, StatusCode> {
    generate_hls_playlist(state, item, "audio", query, headers, false).await
}

async fn audio_master(
    State(state): State<Arc<AppState>>,
    RoutePath(item): RoutePath<String>,
    Query(query): Query<PlaylistQuery>,
    headers: HeaderMap,
) -> Result<Response, StatusCode> {
    generate_hls_playlist(state, item, "audio", query, headers, true).await
}

async fn generate_hls_playlist(
    state: Arc<AppState>,
    item: String,
    expected_kind: &'static str,
    query: PlaylistQuery,
    headers: HeaderMap,
    master: bool,
) -> Result<Response, StatusCode> {
    let target_seconds = query.segment_length.unwrap_or(10);
    if !(1..=10).contains(&target_seconds) {
        return Err(StatusCode::BAD_REQUEST);
    }
    let (_, token) = auth_api::authenticated(state.clone(), headers.clone()).await?;
    let (_, scope) = auth_api::library_scope(state.clone(), headers).await?;
    let (source, kind) = playback::media_file(&state, item.clone(), scope).await?;
    if kind != expected_kind {
        return Err(StatusCode::NOT_FOUND);
    }
    // Admission is reserved before touching any private output path.
    let registry = state.hls_registry();
    let _permit = registry
        .jobs
        .try_acquire()
        .map_err(|_| StatusCode::TOO_MANY_REQUESTS)?;
    let id = Uuid::new_v4().simple().to_string();
    let private_root = std::env::temp_dir().join(format!("pelagos-hls-{id}"));
    create_private_directory(&private_root).map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    let mut cleanup = PrivateRoot {
        path: private_root.clone(),
        armed: true,
    };
    let artifact = hls::generate_segmented_vod(
        &registry.controller,
        state.probe_executable(),
        &source,
        &private_root.join("bundle"),
        target_seconds,
    )
    .await
    .map_err(|err| match err.kind() {
        io::ErrorKind::WouldBlock => StatusCode::TOO_MANY_REQUESTS,
        io::ErrorKind::InvalidData | io::ErrorKind::InvalidInput => StatusCode::NOT_IMPLEMENTED,
        _ => StatusCode::SERVICE_UNAVAILABLE,
    })?;
    let mut playlist = tokio::fs::read_to_string(&artifact.playlist_path)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    if playlist.len() > MAX_MANIFEST || artifact.segments.len() > 64 {
        return Err(StatusCode::INTERNAL_SERVER_ERROR);
    }
    // Advertise the highest measured segment bandwidth. This is a single
    // verified VOD rendition, not a multi-bitrate ABR ladder.
    let peak_bandwidth = artifact
        .segments
        .iter()
        .map(|segment| {
            (segment.transport_bytes as f64 * 8.0 / segment.duration_seconds).ceil() as u64
        })
        .max()
        .ok_or(StatusCode::INTERNAL_SERVER_ERROR)?;
    if !(1..=1_000_000_000).contains(&peak_bandwidth) {
        return Err(StatusCode::NOT_IMPLEMENTED);
    }
    let mut media_playlist = playlist.clone();
    let segments = artifact
        .segment_paths
        .iter()
        .enumerate()
        .map(|(index, path)| {
            playlist = playlist.replace(
                &format!("segment{index:05}.ts"),
                &format!("hls1/{id}/{index}.ts"),
            );
            media_playlist =
                media_playlist.replace(&format!("segment{index:05}.ts"), &format!("{index}.ts"));
            (path.clone(), artifact.segments[index].transport_bytes)
        })
        .collect();
    let reply = if master {
        format!(
            "#EXTM3U\n#EXT-X-VERSION:3\n#EXT-X-STREAM-INF:BANDWIDTH={peak_bandwidth}\nhls1/{id}/index.m3u8\n"
        )
    } else {
        playlist
    };
    let publication = Arc::new(Published {
        id,
        item,
        kind: expected_kind,
        digest: Sha256::digest(token.as_bytes()).into(),
        source,
        private_root,
        media_playlist,
        segments,
    });
    cleanup.armed = false;
    registry.publish(publication);
    let mut response = Response::new(Body::from(reply.clone()));
    set_private_headers(&mut response, "application/vnd.apple.mpegurl", reply.len());
    Ok(response)
}

async fn serve_segment(
    state: Arc<AppState>,
    item: String,
    playlist_id: String,
    name: String,
    headers: HeaderMap,
    expected_kind: &'static str,
    head_only: bool,
) -> Result<Response, StatusCode> {
    let (_, token) = auth_api::authenticated(state.clone(), headers.clone()).await?;
    let publication = state
        .hls_registry()
        .lookup(&playlist_id, &item, expected_kind, &token)
        .ok_or(StatusCode::NOT_FOUND)?;
    // Reauthorize every segment, including library grants and confined source.
    let (_, scope) = auth_api::library_scope(state.clone(), headers).await?;
    let (source, kind) = playback::media_file(&state, item, scope).await?;
    if kind != expected_kind || source != publication.source {
        return Err(StatusCode::NOT_FOUND);
    }

    if name == "index.m3u8" {
        let text = &publication.media_playlist;
        let mut response = Response::new(if head_only {
            Body::empty()
        } else {
            Body::from(text.clone())
        });
        set_private_headers(&mut response, "application/vnd.apple.mpegurl", text.len());
        return Ok(response);
    }
    let segment_number = name.strip_suffix(".ts").ok_or(StatusCode::NOT_FOUND)?;
    if segment_number.is_empty()
        || segment_number.len() > 2
        || (segment_number.starts_with('0') && segment_number.len() > 1)
    {
        return Err(StatusCode::NOT_FOUND);
    }
    let index = segment_number
        .parse::<usize>()
        .map_err(|_| StatusCode::NOT_FOUND)?;
    let (path, size) = publication
        .segments
        .get(index)
        .ok_or(StatusCode::NOT_FOUND)?;
    if *size == 0 || *size > MAX_SEGMENT_BYTES {
        return Err(StatusCode::NOT_FOUND);
    }
    let file = tokio::fs::File::open(path)
        .await
        .map_err(|_| StatusCode::NOT_FOUND)?;
    if file
        .metadata()
        .await
        .map_err(|_| StatusCode::NOT_FOUND)?
        .len()
        != *size
    {
        return Err(StatusCode::NOT_FOUND);
    }
    let length = usize::try_from(*size).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let body = if head_only {
        Body::empty()
    } else {
        Body::from_stream(ReaderStream::new(file.take(*size)))
    };
    let mut response = Response::new(body);
    set_private_headers(&mut response, "video/mp2t", length);
    Ok(response)
}

async fn video_segment(
    State(state): State<Arc<AppState>>,
    RoutePath((item, playlist, name)): RoutePath<(String, String, String)>,
    headers: HeaderMap,
) -> Result<Response, StatusCode> {
    serve_segment(state, item, playlist, name, headers, "video", false).await
}

async fn head_video_segment(
    State(state): State<Arc<AppState>>,
    RoutePath((item, playlist, name)): RoutePath<(String, String, String)>,
    headers: HeaderMap,
) -> Result<Response, StatusCode> {
    serve_segment(state, item, playlist, name, headers, "video", true).await
}

async fn audio_segment(
    State(state): State<Arc<AppState>>,
    RoutePath((item, playlist, name)): RoutePath<(String, String, String)>,
    headers: HeaderMap,
) -> Result<Response, StatusCode> {
    serve_segment(state, item, playlist, name, headers, "audio", false).await
}

async fn head_audio_segment(
    State(state): State<Arc<AppState>>,
    RoutePath((item, playlist, name)): RoutePath<(String, String, String)>,
    headers: HeaderMap,
) -> Result<Response, StatusCode> {
    serve_segment(state, item, playlist, name, headers, "audio", true).await
}

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/Videos/{item_id}/main.m3u8", get(video_main))
        .route("/Videos/{item_id}/master.m3u8", get(video_master))
        .route("/Audio/{item_id}/main.m3u8", get(audio_main))
        .route("/Audio/{item_id}/master.m3u8", get(audio_master))
        .route(
            "/Videos/{item_id}/hls1/{playlist_id}/{segment_name}",
            get(video_segment).head(head_video_segment),
        )
        .route(
            "/Audio/{item_id}/hls1/{playlist_id}/{segment_name}",
            get(audio_segment).head(head_audio_segment),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn published_bundles_are_token_scoped_bounded_and_cleaned_after_last_reader() {
        let temp = tempfile::tempdir().unwrap();
        let registry = HlsRegistry::new(PathBuf::from("ffmpeg"));
        let owner = "synthetic-private-token";
        let digest: [u8; 32] = Sha256::digest(owner.as_bytes()).into();
        let first_root = temp.path().join("session-0");
        for index in 0..3 {
            let root = temp.path().join(format!("session-{index}"));
            fs::create_dir(&root).unwrap();
            registry.publish(Arc::new(Published {
                id: format!("playlist-{index}"),
                item: "media-1".to_owned(),
                kind: "video",
                digest,
                source: PathBuf::from("/private/media-1"),
                private_root: root,
                media_playlist: String::new(),
                segments: Vec::new(),
            }));
        }
        assert!(registry.lookup("playlist-0", "media-1", "video", owner).is_some());
        assert!(registry.lookup("playlist-0", "media-2", "video", owner).is_none());
        assert!(
            registry
                .lookup("playlist-0", "media-1", "video", "other-token")
                .is_none()
        );
        let in_flight = registry.lookup("playlist-0", "media-1", "video", owner).unwrap();
        let fourth_root = temp.path().join("session-3");
        fs::create_dir(&fourth_root).unwrap();
        registry.publish(Arc::new(Published {
            id: "playlist-3".to_owned(),
            item: "media-1".to_owned(),
            kind: "video",
            digest,
            source: PathBuf::from("/private/media-1"),
            private_root: fourth_root.clone(),
            media_playlist: String::new(),
            segments: Vec::new(),
        }));
        assert!(registry.lookup("playlist-0", "media-1", "video", owner).is_none());
        assert!(first_root.exists());
        drop(in_flight);
        assert!(!first_root.exists());
        assert!(registry.lookup("playlist-3", "media-1", "video", owner).is_some());
        drop(registry);
        for index in 1..=3 {
            assert!(!temp.path().join(format!("session-{index}")).exists());
        }
    }
}
