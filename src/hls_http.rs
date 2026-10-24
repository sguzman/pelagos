//! Bounded, token-scoped Jellyfin-shaped HLS VOD HTTP subset.
//! This does not implement dynamic HLS scheduling or Smart TV parity.
use crate::{
    app::AppState,
    auth_api, hls, playback, streaming,
    transcode::{MpegTsQuality, TranscodeController},
};
use axum::{
    Router,
    body::Body,
    extract::{Path as RoutePath, Query, State},
    http::{
        HeaderMap, HeaderValue, StatusCode,
        header::{
            ACCEPT_RANGES, CACHE_CONTROL, CONTENT_LENGTH, CONTENT_RANGE, CONTENT_TYPE, ETAG,
            IF_RANGE, RANGE,
        },
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
use tokio::{
    io::{AsyncReadExt, AsyncSeekExt},
    sync::Semaphore,
};
use tokio_util::io::ReaderStream;
use uuid::Uuid;

const MAX_PUBLISHED: usize = 3;
const MAX_MANIFEST: usize = 32 * 1024;
const MAX_SEGMENT_BYTES: u64 = 128 * 1024 * 1024;

#[derive(Debug)]
struct PublishedVariant {
    name: String,
    media_playlist: String,
    segments: Vec<(PathBuf, u64)>,
}

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
    start_time_ticks: u64,
    rendition_count: u8,
    private_root: PathBuf,
    main_playlist: String,
    master_playlist: String,
    media_playlist: String,
    segments: Vec<(PathBuf, u64)>,
    variants: Vec<PublishedVariant>,
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
    start_time_ticks: u64,
    rendition_count: u8,
}

/// Small private LRU cache; segment access promotes active publications and
/// an in-flight request holds its own Arc during eviction.
#[derive(Debug)]
pub struct HlsRegistry {
    jobs: Semaphore,
    waiters: Semaphore,
    controller: TranscodeController,
    entries: Mutex<VecDeque<Arc<Published>>>,
}
impl HlsRegistry {
    pub fn new(executable: PathBuf) -> Self {
        Self {
            jobs: Semaphore::new(1),
            waiters: Semaphore::new(4),
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

    /// Join at most four ongoing HLS publication requests. Once the first
    /// worker publishes, a waiter rechecks its exact cache key before
    /// considering any additional FFmpeg work. Cancellation releases both
    /// admission permits without a background worker or unbounded queue.
    async fn acquire_generation(&self) -> Result<tokio::sync::SemaphorePermit<'_>, StatusCode> {
        if let Ok(permit) = self.jobs.try_acquire() {
            return Ok(permit);
        }
        let queued = self
            .waiters
            .try_acquire()
            .map_err(|_| StatusCode::TOO_MANY_REQUESTS)?;
        let permit = tokio::time::timeout(Duration::from_secs(120), self.jobs.acquire())
            .await
            .map_err(|_| StatusCode::TOO_MANY_REQUESTS)?
            .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
        drop(queued);
        Ok(permit)
    }

    fn lookup(&self, id: &str, item: &str, kind: &str, token: &str) -> Option<Arc<Published>> {
        let digest: [u8; 32] = Sha256::digest(token.as_bytes()).into();
        let mut entries = self
            .entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let index = entries.iter().position(|entry| {
            entry.id == id && entry.item == item && entry.kind == kind && entry.digest == digest
        })?;
        let entry = entries.remove(index)?;
        entries.push_back(entry.clone());
        Some(entry)
    }

    /// Reuse an already completed rendition, never one owned by a sibling
    /// session or built for different source bytes/segment settings.
    fn cached(&self, query: &CacheLookup<'_>) -> Option<Arc<Published>> {
        let digest: [u8; 32] = Sha256::digest(query.token.as_bytes()).into();
        let mut entries = self
            .entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let index = entries.iter().rposition(|entry| {
            entry.item == query.item
                && entry.kind == query.kind
                && entry.digest == digest
                && entry.source == query.source
                && entry.source_size == query.size
                && entry.source_modified == query.modified
                && entry.target_seconds == query.target_seconds
                && entry.start_time_ticks == query.start_time_ticks
                && entry.rendition_count == query.rendition_count
        })?;
        let entry = entries.remove(index)?;
        entries.push_back(entry.clone());
        Some(entry)
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
    start_time_ticks: Option<i64>,
    renditions: Option<u8>,
    media_source_id: Option<String>,
    enable_adaptive_bitrate_streaming: Option<bool>,
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

/// Repeat admission checks after any encoder wait and immediately before
/// publication. User grants, bearer tokens, and source revisions can change
/// while the sole HLS worker is occupied; stale completed bytes must never
/// be advertised to a request that has since lost access.
async fn reauthorize_publication(
    state: &Arc<AppState>,
    headers: &HeaderMap,
    query: &CacheLookup<'_>,
) -> Result<(), StatusCode> {
    let (_, current_token) = auth_api::authenticated(state.clone(), headers.clone()).await?;
    if current_token != query.token {
        return Err(StatusCode::UNAUTHORIZED);
    }
    let (_, scope) = auth_api::library_scope(state.clone(), headers.clone()).await?;
    let (source, kind) = playback::media_file(state, query.item.to_owned(), scope).await?;
    if kind != query.kind || source != query.source {
        return Err(StatusCode::NOT_FOUND);
    }
    let metadata = tokio::fs::metadata(source)
        .await
        .map_err(|_| StatusCode::NOT_FOUND)?;
    if metadata.len() != query.size || metadata.modified().ok() != query.modified {
        return Err(StatusCode::NOT_FOUND);
    }
    Ok(())
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
    let start_time_ticks = query.start_time_ticks.unwrap_or(0);
    if !(0..=6_000_000_000).contains(&start_time_ticks) {
        return Err(StatusCode::BAD_REQUEST);
    }
    let start_time_ticks = start_time_ticks as u64;
    // The pinned v12.3 master routes carry MediaSourceId and the opt-in
    // adaptive switch. Pelagos PlaybackInfo currently exposes one source
    // whose ID exactly equals the item ID; never silently transcode a
    // different requested source or ignore an explicit adaptive choice.
    let rendition_count = query.renditions.unwrap_or(if query.enable_adaptive_bitrate_streaming
        == Some(true)
    {
        2
    } else {
        1
    });
    if !(1..=4).contains(&rendition_count)
        || (rendition_count > 1 && query.enable_adaptive_bitrate_streaming == Some(false))
    {
        return Err(StatusCode::BAD_REQUEST);
    }
    if rendition_count > 1 && !master {
        return Err(StatusCode::NOT_IMPLEMENTED);
    }
    let (_, token) = auth_api::authenticated(state.clone(), headers.clone()).await?;
    if query
        .media_source_id
        .as_deref()
        .is_some_and(|id| id != item)
    {
        return Err(StatusCode::NOT_FOUND);
    }
    let (_, scope) = auth_api::library_scope(state.clone(), headers.clone()).await?;
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
        start_time_ticks,
        rendition_count,
    };
    if let Some(existing) = registry.cached(&cache_key) {
        return Ok(playlist_response(if master {
            &existing.master_playlist
        } else {
            &existing.main_playlist
        }));
    }
    // Admission is reserved before touching any private output path.
    let _permit = registry.acquire_generation().await?;
    reauthorize_publication(&state, &headers, &cache_key).await?;
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
    if rendition_count > 1 {
        // Short, private audio-only and video VOD. Every variant is encoded
        // independently and measured; no nominal duplicate bitrates are
        // published as a ladder.
        const VIDEO_PROFILES: [MpegTsQuality; 4] = [
            MpegTsQuality {
                video_kbps: 400,
                audio_kbps: 96,
            },
            MpegTsQuality {
                video_kbps: 800,
                audio_kbps: 128,
            },
            MpegTsQuality {
                video_kbps: 1400,
                audio_kbps: 160,
            },
            MpegTsQuality {
                video_kbps: 2200,
                audio_kbps: 192,
            },
        ];
        const AUDIO_PROFILES: [MpegTsQuality; 4] = [
            MpegTsQuality {
                video_kbps: 400,
                audio_kbps: 48,
            },
            MpegTsQuality {
                video_kbps: 400,
                audio_kbps: 96,
            },
            MpegTsQuality {
                video_kbps: 400,
                audio_kbps: 160,
            },
            MpegTsQuality {
                video_kbps: 400,
                audio_kbps: 256,
            },
        ];
        let profiles = if expected_kind == "audio" {
            &AUDIO_PROFILES
        } else {
            &VIDEO_PROFILES
        };
        let adaptive = hls::generate_adaptive_segmented_vod_from(
            &registry.controller,
            state.probe_executable(),
            &source,
            &private_root.join("bundle"),
            target_seconds,
            start_time_ticks as f64 / 10_000_000.0,
            &profiles[..usize::from(rendition_count)],
        )
        .await
        .map_err(|err| match err.kind() {
            io::ErrorKind::WouldBlock => StatusCode::TOO_MANY_REQUESTS,
            io::ErrorKind::InvalidInput => StatusCode::BAD_REQUEST,
            io::ErrorKind::InvalidData => StatusCode::NOT_IMPLEMENTED,
            _ => StatusCode::SERVICE_UNAVAILABLE,
        })?;
        if adaptive.variants.len() < 2 {
            return Err(StatusCode::NOT_IMPLEMENTED);
        }
        let mut master_playlist = String::from("#EXTM3U\n#EXT-X-VERSION:3\n");
        let mut variants = Vec::new();
        let mut total_bytes = 0_u64;
        for (variant, rendition) in adaptive.variants.iter().zip(&adaptive.renditions) {
            if !variant.playlist_name.starts_with("variant")
                || rendition.segments.is_empty()
                || rendition.segments.len() > 64
            {
                return Err(StatusCode::INTERNAL_SERVER_ERROR);
            }
            let name = variant
                .playlist_name
                .strip_suffix("/index.m3u8")
                .ok_or(StatusCode::INTERNAL_SERVER_ERROR)?
                .to_owned();
            if !matches!(
                name.as_str(),
                "variant00" | "variant01" | "variant02" | "variant03"
            ) {
                return Err(StatusCode::INTERNAL_SERVER_ERROR);
            }
            if variant.bandwidth == 0 || variant.bandwidth > 1_000_000_000 {
                return Err(StatusCode::NOT_IMPLEMENTED);
            }
            master_playlist.push_str(&format!(
                "#EXT-X-STREAM-INF:BANDWIDTH={}",
                variant.bandwidth
            ));
            if let Some((width, height)) = variant.resolution {
                master_playlist.push_str(&format!(",RESOLUTION={width}x{height}"));
            }
            master_playlist.push_str(&format!("\nhls1/{id}/{name}/index.m3u8\n"));
            let mut media_playlist = tokio::fs::read_to_string(&rendition.playlist_path)
                .await
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            let mut segments = Vec::new();
            for (index, path) in rendition.segment_paths.iter().enumerate() {
                media_playlist = media_playlist
                    .replace(&format!("segment{index:05}.ts"), &format!("{index}.ts"));
                let size = rendition.segments[index].transport_bytes;
                total_bytes = total_bytes
                    .checked_add(size)
                    .ok_or(StatusCode::INTERNAL_SERVER_ERROR)?;
                if total_bytes > MAX_SEGMENT_BYTES {
                    return Err(StatusCode::NOT_IMPLEMENTED);
                }
                segments.push((path.clone(), size));
            }
            if media_playlist.len() > MAX_MANIFEST {
                return Err(StatusCode::INTERNAL_SERVER_ERROR);
            }
            variants.push(PublishedVariant {
                name,
                media_playlist,
                segments,
            });
        }
        if master_playlist.len() > MAX_MANIFEST {
            return Err(StatusCode::INTERNAL_SERVER_ERROR);
        }
        reauthorize_publication(&state, &headers, &cache_key).await?;
        let response = playlist_response(&master_playlist);
        let publication = Arc::new(Published {
            id,
            item,
            kind: expected_kind,
            digest: Sha256::digest(token.as_bytes()).into(),
            source,
            source_size,
            source_modified,
            target_seconds,
            start_time_ticks,
            rendition_count,
            private_root,
            main_playlist: String::new(),
            master_playlist,
            media_playlist: String::new(),
            segments: Vec::new(),
            variants,
        });
        cleanup.armed = false;
        registry.publish(publication);
        return Ok(response);
    }
    let artifact = hls::generate_segmented_vod_from(
        &registry.controller,
        state.probe_executable(),
        &source,
        &private_root.join("bundle"),
        target_seconds,
        start_time_ticks as f64 / 10_000_000.0,
    )
    .await
    .map_err(|err| match err.kind() {
        io::ErrorKind::WouldBlock => StatusCode::TOO_MANY_REQUESTS,
        io::ErrorKind::InvalidInput => StatusCode::BAD_REQUEST,
        io::ErrorKind::InvalidData => StatusCode::NOT_IMPLEMENTED,
        _ => StatusCode::SERVICE_UNAVAILABLE,
    })?;
    // FFmpeg and FFprobe may have run for most of the 120-second budget.
    // Recheck all session, library, and original media revision constraints
    // before publishing even a successfully verified transcode.
    reauthorize_publication(&state, &headers, &cache_key).await?;
    let mut playlist = tokio::fs::read_to_string(&artifact.playlist_path)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    if playlist.len() > MAX_MANIFEST || artifact.segments.len() > 64 {
        return Err(StatusCode::INTERNAL_SERVER_ERROR);
    }
    // Advertise the highest measured segment bandwidth. This is a single
    // verified VOD rendition, not a multi-bitrate ABR ladder.
    let peak_bandwidth = hls::measured_peak_transport_bandwidth(&artifact.segments)
        .map_err(|_| StatusCode::NOT_IMPLEMENTED)?;
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
        start_time_ticks,
        rendition_count,
        private_root,
        main_playlist: playlist,
        master_playlist,
        media_playlist,
        segments,
        variants: Vec::new(),
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
    let (_, scope) = auth_api::library_scope(state.clone(), headers.clone()).await?;
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

    let (name, media_playlist, segments, etag_variant) =
        if let Some((variant_name, leaf_name)) = name.split_once('/') {
            let variant = publication
                .variants
                .iter()
                .find(|variant| variant.name == variant_name)
                .ok_or(StatusCode::NOT_FOUND)?;
            (
                leaf_name,
                &variant.media_playlist,
                &variant.segments,
                variant_name,
            )
        } else {
            if !publication.variants.is_empty() {
                return Err(StatusCode::NOT_FOUND);
            }
            (
                name.as_str(),
                &publication.media_playlist,
                &publication.segments,
                "",
            )
        };
    if name == "index.m3u8" {
        let text = media_playlist;
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
    let (path, size) = segments.get(index).ok_or(StatusCode::NOT_FOUND)?;
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
    // A publication UUID plus its bounded segment index and fixed byte
    // count uniquely identifies this immutable, token-scoped MPEG-TS part.
    // A client may resume only when its If-Range strong validator matches.
    let etag = format!("\"hls-{playlist_id}-{etag_variant}-{index}-{size}\"");
    let apply_range = headers.get(IF_RANGE).is_none_or(|candidate| {
        candidate
            .to_str()
            .ok()
            .is_some_and(|candidate| candidate == etag)
    });
    let byte_range = if apply_range {
        headers.get(RANGE)
    } else {
        None
    };
    let byte_range = if let Some(value) = byte_range {
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
                response.headers_mut().insert(
                    ETAG,
                    HeaderValue::from_str(&etag).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?,
                );
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
    let response_length = usize::try_from(length).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    set_private_headers(&mut response, "video/mp2t", response_length);
    response.headers_mut().insert(
        ETAG,
        HeaderValue::from_str(&etag).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?,
    );
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

async fn video_variant_segment(
    State(state): State<Arc<AppState>>,
    RoutePath((item, playlist, variant, name)): RoutePath<(String, String, String, String)>,
    headers: HeaderMap,
) -> Result<Response, StatusCode> {
    serve_segment(
        state,
        item,
        playlist,
        format!("{variant}/{name}"),
        headers,
        "video",
        false,
    )
    .await
}

async fn head_video_variant_segment(
    State(state): State<Arc<AppState>>,
    RoutePath((item, playlist, variant, name)): RoutePath<(String, String, String, String)>,
    headers: HeaderMap,
) -> Result<Response, StatusCode> {
    serve_segment(
        state,
        item,
        playlist,
        format!("{variant}/{name}"),
        headers,
        "video",
        true,
    )
    .await
}

async fn audio_variant_segment(
    State(state): State<Arc<AppState>>,
    RoutePath((item, playlist, variant, name)): RoutePath<(String, String, String, String)>,
    headers: HeaderMap,
) -> Result<Response, StatusCode> {
    serve_segment(
        state,
        item,
        playlist,
        format!("{variant}/{name}"),
        headers,
        "audio",
        false,
    )
    .await
}

async fn head_audio_variant_segment(
    State(state): State<Arc<AppState>>,
    RoutePath((item, playlist, variant, name)): RoutePath<(String, String, String, String)>,
    headers: HeaderMap,
) -> Result<Response, StatusCode> {
    serve_segment(
        state,
        item,
        playlist,
        format!("{variant}/{name}"),
        headers,
        "audio",
        true,
    )
    .await
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
            "/Videos/{item_id}/hls1/{playlist_id}/{variant_id}/{segment_name}",
            get(video_variant_segment).head(head_video_variant_segment),
        )
        .route(
            "/Audio/{item_id}/hls1/{playlist_id}/{segment_name}",
            get(audio_segment).head(head_audio_segment),
        )
        .route(
            "/Audio/{item_id}/hls1/{playlist_id}/{variant_id}/{segment_name}",
            get(audio_variant_segment).head(head_audio_variant_segment),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn bounded_hls_generation_queue_coalesces_and_recovers_after_cancellation() {
        let registry = Arc::new(HlsRegistry::new(PathBuf::from("ffmpeg")));
        let active = registry.jobs.try_acquire().unwrap();
        let queued = (0..4)
            .map(|_| registry.waiters.try_acquire().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(
            registry.acquire_generation().await.err().unwrap(),
            StatusCode::TOO_MANY_REQUESTS
        );
        drop(queued);
        assert_eq!(registry.waiters.available_permits(), 4);

        let waiting_registry = registry.clone();
        let waiting = tokio::spawn(async move {
            let _permit = waiting_registry.acquire_generation().await.unwrap();
        });
        tokio::time::timeout(Duration::from_secs(1), async {
            while registry.waiters.available_permits() != 3 {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        assert!(!waiting.is_finished());
        waiting.abort();
        assert!(waiting.await.is_err());
        assert_eq!(registry.waiters.available_permits(), 4);

        let next_registry = registry.clone();
        let pending = tokio::spawn(async move {
            let _permit = next_registry.acquire_generation().await.unwrap();
        });
        tokio::time::timeout(Duration::from_secs(1), async {
            while registry.waiters.available_permits() != 3 {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        assert!(!pending.is_finished());
        drop(active);
        tokio::time::timeout(Duration::from_secs(2), pending)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(registry.waiters.available_permits(), 4);
        assert!(registry.jobs.try_acquire().is_ok());
    }

    #[tokio::test]
    async fn publication_waiter_rechecks_revoked_grants_tokens_and_source_revisions() {
        use crate::{auth, catalog, catalog_api};
        use axum::http::HeaderValue;

        let fixture = tempfile::tempdir().unwrap();
        let library = fixture.path().join("Movies");
        let state_dir = fixture.path().join("state");
        std::fs::create_dir_all(&library).unwrap();
        let path = library.join("Sample.mp4");
        std::fs::write(&path, b"synthetic catalogued movie").unwrap();
        catalog::index(&state_dir, &library).unwrap();
        auth::create_user(&state_dir, "admin", "hls-reauthorize-admin-password").unwrap();
        auth::create_user(&state_dir, "viewer", "hls-reauthorize-viewer-password").unwrap();
        auth::grant_library(&state_dir, "viewer", &library).unwrap();
        let viewer = auth::authenticate(
            &state_dir,
            "viewer",
            "hls-reauthorize-viewer-password",
            "server",
        )
        .unwrap()
        .unwrap();
        let token = viewer.access_token;
        let mut headers = HeaderMap::new();
        headers.insert("X-Emby-Token", HeaderValue::from_str(&token).unwrap());
        let original_source = std::fs::canonicalize(&path).unwrap();
        let original = std::fs::metadata(&original_source).unwrap();
        let id = catalog_api::stable_id(library.to_str().unwrap(), "Sample.mp4");
        let state = Arc::new(AppState::with_auth_dir(
            "server".to_owned(),
            state_dir.clone(),
        ));
        assert_eq!(
            reauthorize_publication(
                &state,
                &headers,
                &CacheLookup {
                    item: &id,
                    kind: "video",
                    token: &token,
                    source: &original_source,
                    size: original.len(),
                    modified: original.modified().ok(),
                    target_seconds: 2,
                    start_time_ticks: 0,
                    rendition_count: 1,
                },
            )
            .await,
            Ok(())
        );

        // Pause a request in the same one-worker admission gate as the HLS
        // encoder. Revoking its library grant while queued must survive the
        // gate transition as a denial, not a stale published manifest.
        let active = state.hls_registry().jobs.try_acquire().unwrap();
        let waiting_state = state.clone();
        let pending = tokio::spawn(async move {
            let _permit = waiting_state.hls_registry().acquire_generation().await?;
            reauthorize_publication(
                &waiting_state,
                &headers,
                &CacheLookup {
                    item: &id,
                    kind: "video",
                    token: &token,
                    source: &original_source,
                    size: original.len(),
                    modified: original.modified().ok(),
                    target_seconds: 2,
                    start_time_ticks: 0,
                    rendition_count: 1,
                },
            )
            .await
        });
        tokio::time::timeout(Duration::from_secs(2), async {
            while state.hls_registry().waiters.available_permits() == 4 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        auth::revoke_library(&state_dir, "viewer", &library).unwrap();
        drop(active);
        assert_eq!(pending.await.unwrap(), Err(StatusCode::NOT_FOUND));

        auth::grant_library(&state_dir, "viewer", &library).unwrap();
        // A separate valid token is not a substitute for the old one, and
        // restoring the grant never revalidates a revoked login.
        let fresh = auth::authenticate(
            &state_dir,
            "viewer",
            "hls-reauthorize-viewer-password",
            "server",
        )
        .unwrap()
        .unwrap();
        let mut fresh_headers = HeaderMap::new();
        fresh_headers.insert(
            "X-Emby-Token",
            HeaderValue::from_str(&fresh.access_token).unwrap(),
        );
        let source = std::fs::canonicalize(&path).unwrap();
        let expected = std::fs::metadata(&source).unwrap();
        let fresh_id = catalog_api::stable_id(library.to_str().unwrap(), "Sample.mp4");
        assert_eq!(
            reauthorize_publication(
                &state,
                &fresh_headers,
                &CacheLookup {
                    item: &fresh_id,
                    kind: "video",
                    token: &fresh.access_token,
                    source: &source,
                    size: expected.len(),
                    modified: expected.modified().ok(),
                    target_seconds: 2,
                    start_time_ticks: 0,
                    rendition_count: 1,
                },
            )
            .await,
            Ok(())
        );
        std::fs::write(&path, b"replaced after HLS encoding was admitted").unwrap();
        assert_eq!(
            reauthorize_publication(
                &state,
                &fresh_headers,
                &CacheLookup {
                    item: &fresh_id,
                    kind: "video",
                    token: &fresh.access_token,
                    source: &source,
                    size: expected.len(),
                    modified: expected.modified().ok(),
                    target_seconds: 2,
                    start_time_ticks: 0,
                    rendition_count: 1,
                },
            )
            .await,
            Err(StatusCode::NOT_FOUND)
        );
        auth::revoke(&state_dir, &fresh.access_token).unwrap();
        assert_eq!(
            reauthorize_publication(
                &state,
                &fresh_headers,
                &CacheLookup {
                    item: &fresh_id,
                    kind: "video",
                    token: &fresh.access_token,
                    source: &source,
                    size: expected.len(),
                    modified: expected.modified().ok(),
                    target_seconds: 2,
                    start_time_ticks: 0,
                    rendition_count: 1,
                },
            )
            .await,
            Err(StatusCode::UNAUTHORIZED)
        );
    }

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
                start_time_ticks: 0,
                rendition_count: 1,
                private_root: root,
                main_playlist: String::new(),
                master_playlist: String::new(),
                media_playlist: String::new(),
                segments: Vec::new(),
                variants: Vec::new(),
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
            start_time_ticks: 0,
            rendition_count: 1,
        };
        // Multiple synthetic entries use the same cache key; the most
        // recently used publication is now returned, rather than the one
        // with the highest original insertion position.
        assert_eq!(registry.cached(&key).unwrap().id, "playlist-0");
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
            start_time_ticks: 0,
            rendition_count: 1,
            private_root: fourth_root.clone(),
            main_playlist: String::new(),
            master_playlist: String::new(),
            media_playlist: String::new(),
            segments: Vec::new(),
            variants: Vec::new(),
        }));
        // Cached media and segment access make these publications recently
        // used. Evict the cold session-1 rather than active session-0.
        assert!(
            registry
                .lookup("playlist-0", "media-1", "video", owner)
                .is_some()
        );
        assert!(!temp.path().join("session-1").exists());
        assert!(first_root.exists());
        drop(in_flight);
        assert!(first_root.exists());
        assert!(
            registry
                .lookup("playlist-3", "media-1", "video", owner)
                .is_some()
        );
        drop(registry);
        for index in [0, 2, 3] {
            assert!(!temp.path().join(format!("session-{index}")).exists());
        }
    }
}
