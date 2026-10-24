//! Bounded, token-scoped Jellyfin-shaped HLS VOD HTTP subset.
//! This does not implement dynamic HLS scheduling or Smart TV parity.
use crate::{app::AppState, auth_api, hls, playback, streaming, transcode::TranscodeController};
use axum::{
    Router,
    body::Body,
    extract::{Path as RoutePath, Query, State},
    http::{
        HeaderMap, HeaderValue, StatusCode,
        header::{ACCEPT_RANGES, CACHE_CONTROL, CONTENT_LENGTH, CONTENT_RANGE, CONTENT_TYPE, RANGE},
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
use tokio::{io::{AsyncReadExt, AsyncSeekExt}, sync::Semaphore};
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
    source_size: u64,
    source_modified: Option<std::time::SystemTime>,
    target_seconds: u32,
    private_root: PathBuf,
    main_playlist: String,
    master_playlist: String,
    media_playlist: String,
    segments: Vec<(PathBuf, u64)>,
}
impl Drop for Published {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.private_root);
    }
}

struct CacheLookup<'a> {
    item: &'a str,
    kind: &'a str,
    token: &'a str,
    source: &'a Path,
    size: u64,
    modified: Option<std::time::SystemTime>,
    target_seconds: u32,
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
            .find(|entry| {
                entry.id == id && entry.item == item && entry.kind == kind && entry.digest == digest
            })
            .cloned()
    }

    /// Reuse an already completed rendition, never one owned by a sibling
    /// session or built for different source bytes/segment settings.
    fn cached(&self, query: &CacheLookup<'_>) -> Option<Arc<Published>> {
        let digest: [u8; 32] = Sha256::digest(query.token.as_bytes()).into();
        self.entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .rev()
            .find(|entry| {
                entry.item == query.item
                    && entry.kind == query.kind
                    && entry.digest == digest
                    && entry.source == query.source
                    && entry.source_size == query.size
                    && entry.source_modified == query.modified
                    && entry.target_seconds == query.target_seconds
            })
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

fn playlist_response(playlist: &str) -> Response {
    let mut response = Response::new(Body::from(playlist.to_owned()));
    set_private_headers(
        &mut response,
        "application/vnd.apple.mpegurl",
        playlist.len(),
    );
    response
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
    let metadata = tokio::fs::metadata(&source)
        .await
        .map_err(|_| StatusCode::NOT_FOUND)?;
    let source_size = metadata.len();
    let source_modified = metadata.modified().ok();
    let registry = state.hls_registry();
    let cache_key = CacheLookup {
        item: &item,
        kind: expected_kind,
        token: &token,
        source: &source,
        size: source_size,
        modified: source_modified,
        target_seconds,
    };
    if let Some(existing) = registry.cached(&cache_key) {
        return Ok(playlist_response(if master {
            &existing.master_playlist
        } else {
            &existing.main_playlist
        }));
    }
    // Admission is reserved before touching any private output path.
    let _permit = registry
        .jobs
        .try_acquire()
        .map_err(|_| StatusCode::TOO_MANY_REQUESTS)?;
    // Recheck after reserving the slot: the previous request may have
    // just published this rendition and no second encode is necessary.
    if let Some(existing) = registry.cached(&cache_key) {
        return Ok(playlist_response(if master {
            &existing.master_playlist
        } else {
            &existing.main_playlist
        }));
    }
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
    let master_playlist = format!(
        "#EXTM3U\n#EXT-X-VERSION:3\n#EXT-X-STREAM-INF:BANDWIDTH={peak_bandwidth}\nhls1/{id}/index.m3u8\n"
    );
    let response = playlist_response(if master { &master_playlist } else { &playlist });
    let publication = Arc::new(Published {
        id,
        item,
        kind: expected_kind,
        digest: Sha256::digest(token.as_bytes()).into(),
        source,
        source_size,
        source_modified,
        target_seconds,
        private_root,
        main_playlist: playlist,
        master_playlist,
        media_playlist,
        segments,
    });
    cleanup.armed = false;
    registry.publish(publication);
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
    // A published HLS segment must not survive replacement or modification
    // of the source file. Regeneration will create a new publication key.
    let metadata = tokio::fs::metadata(&source)
        .await
        .map_err(|_| StatusCode::NOT_FOUND)?;
    if metadata.len() != publication.source_size
        || metadata.modified().ok() != publication.source_modified
    {
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
    let mut file = tokio::fs::File::open(path)
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
    let byte_range = if let Some(value) = headers.get(RANGE) {
        match value
            .to_str()
            .ok()
            .and_then(|value| streaming::parse_range(value, *size))
        {
            Some(range) => Some(range),
            None => {
                let mut response = Response::new(Body::empty());
                *response.status_mut() = StatusCode::RANGE_NOT_SATISFIABLE;
                set_private_headers(&mut response, "video/mp2t", 0);
                response
                    .headers_mut()
                    .insert(ACCEPT_RANGES, HeaderValue::from_static("bytes"));
                response.headers_mut().insert(
                    CONTENT_RANGE,
                    HeaderValue::from_str(&format!("bytes */{size}"))
                        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?,
                );
                return Ok(response);
            }
        }
    } else {
        None
    };
    let start = byte_range.as_ref().map_or(0, |range| range.start);
    let end = byte_range.as_ref().map_or(*size - 1, |range| range.end);
    let length = end - start + 1;
    if start > 0 {
        file.seek(std::io::SeekFrom::Start(start))
            .await
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    }
    let body = if head_only {
        Body::empty()
    } else {
        Body::from_stream(ReaderStream::new(file.take(length)))
    };
    let mut response = Response::new(body);
    let response_length =
        usize::try_from(length).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    set_private_headers(&mut response, "video/mp2t", response_length);
    response
        .headers_mut()
        .insert(ACCEPT_RANGES, HeaderValue::from_static("bytes"));
    if byte_range.is_some() {
        *response.status_mut() = StatusCode::PARTIAL_CONTENT;
        response.headers_mut().insert(
            CONTENT_RANGE,
            HeaderValue::from_str(&format!("bytes {start}-{end}/{size}"))
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?,
        );
    }
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
        // Only one generation is admitted at a time; there is no FFmpeg
        // child or private output for excess concurrent HTTP requests.
        let admitted = registry.jobs.try_acquire().unwrap();
        assert!(registry.jobs.try_acquire().is_err());
        drop(admitted);
        assert!(registry.jobs.try_acquire().is_ok());
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
                source_size: 1,
                source_modified: None,
                target_seconds: 2,
                private_root: root,
                main_playlist: String::new(),
                master_playlist: String::new(),
                media_playlist: String::new(),
                segments: Vec::new(),
            }));
        }
        assert!(
            registry
                .lookup("playlist-0", "media-1", "video", owner)
                .is_some()
        );
        assert!(
            registry
                .lookup("playlist-0", "media-2", "video", owner)
                .is_none()
        );
        assert!(
            registry
                .lookup("playlist-0", "media-1", "video", "other-token")
                .is_none()
        );
        let source = Path::new("/private/media-1");
        let key = CacheLookup {
            item: "media-1",
            kind: "video",
            token: owner,
            source,
            size: 1,
            modified: None,
            target_seconds: 2,
        };
        assert_eq!(registry.cached(&key).unwrap().id, "playlist-2");
        assert!(
            registry
                .cached(&CacheLookup {
                    token: "other",
                    ..key
                })
                .is_none()
        );
        assert!(
            registry
                .cached(&CacheLookup {
                    kind: "audio",
                    ..key
                })
                .is_none()
        );
        assert!(
            registry
                .cached(&CacheLookup {
                    item: "media-2",
                    ..key
                })
                .is_none()
        );
        assert!(registry.cached(&CacheLookup { size: 2, ..key }).is_none());
        assert!(
            registry
                .cached(&CacheLookup {
                    target_seconds: 3,
                    ..key
                })
                .is_none()
        );
        assert!(
            registry
                .cached(&CacheLookup {
                    modified: Some(std::time::SystemTime::now()),
                    ..key
                })
                .is_none()
        );
        assert!(
            registry
                .cached(&CacheLookup {
                    source: Path::new("/private/other"),
                    ..key
                })
                .is_none()
        );
        let in_flight = registry
            .lookup("playlist-0", "media-1", "video", owner)
            .unwrap();
        let fourth_root = temp.path().join("session-3");
        fs::create_dir(&fourth_root).unwrap();
        registry.publish(Arc::new(Published {
            id: "playlist-3".to_owned(),
            item: "media-1".to_owned(),
            kind: "video",
            digest,
            source: PathBuf::from("/private/media-1"),
            source_size: 1,
            source_modified: None,
            target_seconds: 2,
            private_root: fourth_root.clone(),
            main_playlist: String::new(),
            master_playlist: String::new(),
            media_playlist: String::new(),
            segments: Vec::new(),
        }));
        assert!(
            registry
                .lookup("playlist-0", "media-1", "video", owner)
                .is_none()
        );
        assert!(first_root.exists());
        drop(in_flight);
        assert!(!first_root.exists());
        assert!(
            registry
                .lookup("playlist-3", "media-1", "video", owner)
                .is_some()
        );
        drop(registry);
        for index in 1..=3 {
            assert!(!temp.path().join(format!("session-{index}")).exists());
        }
    }
}
