//! Bounded, token-scoped Jellyfin-shaped HLS VOD HTTP subset.
//! This does not implement dynamic HLS scheduling or Smart TV parity.
use crate::{
    app::AppState,
    auth_api, hls, playback,
    transcode::{ExternalTextFormat, ExternalTextTrack, MpegTsQuality},
};
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
#[cfg(test)]
use std::time::Duration;
use std::{
    fs, io,
    path::{Path, PathBuf},
    sync::Arc,
};
use uuid::Uuid;

mod continuation;
mod registry;
mod segments;
mod tracks;
use continuation::video_next;
pub use registry::HlsRegistry;
use registry::{CacheLookup, Published, PublishedVariant, SidecarRevision};
use segments::serve_segment;
use tracks::{HlsSubtitle, read_selected_sidecar, validate_hls_track_selection};

const MAX_MANIFEST: usize = 32 * 1024;
const MAX_SEGMENT_BYTES: u64 = 128 * 1024 * 1024;
// Unwrapped 32-bit PGS timestamps use a 90 kHz clock. No rollover is supported.
const MAX_PGS_START_END_TICKS: u64 = u32::MAX as u64 * 10_000_000 / 90_000;

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

/// An explicit Pelagos excerpt is not the source's complete VOD timeline.
/// Mark every playlist response, including cache hits and variant playlists.
fn set_excerpt_headers(
    response: &mut Response,
    start_time_ticks: u64,
    excerpt_duration_ticks: Option<u64>,
) {
    if let Some(duration_ticks) = excerpt_duration_ticks {
        let headers = response.headers_mut();
        headers.insert("x-pelagos-excerpt", HeaderValue::from_static("true"));
        headers.insert(
            "x-pelagos-excerpt-start-ticks",
            HeaderValue::from_str(&start_time_ticks.to_string()).expect("numeric tick header"),
        );
        headers.insert(
            "x-pelagos-excerpt-duration-ticks",
            HeaderValue::from_str(&duration_ticks.to_string()).expect("numeric tick header"),
        );
    }
}

fn excerpt_playlist_response(
    playlist: &str,
    start_time_ticks: u64,
    excerpt_duration_ticks: Option<u64>,
) -> Response {
    let mut response = playlist_response(playlist);
    set_excerpt_headers(&mut response, start_time_ticks, excerpt_duration_ticks);
    response
}

/// This is an experimental opt-in navigation hint, not an HLS protocol tag.
/// The URL inherits token-scoped publication access and returns 204 at EOF.
fn set_continuation_path(
    response: &mut Response,
    item: &str,
    publication_id: &str,
    excerpt_duration_ticks: Option<u64>,
) {
    if excerpt_duration_ticks.is_some() {
        let path = format!("/Videos/{item}/hls1/{publication_id}/next.m3u8");
        if let Ok(value) = HeaderValue::from_str(&path) {
            response.headers_mut().insert("x-pelagos-next-path", value);
        }
    }
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
    // Experimental Pelagos-only explicit source-time excerpt, never a normal
    // Jellyfin full-VOD request. Requires an explicitly encoded PGS track.
    pelagos_excerpt_duration_ticks: Option<i64>,
    renditions: Option<u8>,
    media_source_id: Option<String>,
    enable_adaptive_bitrate_streaming: Option<bool>,
    // Pinned v12.3 DynamicHlsController negotiation parameters. Only the
    // current encoder's H.264/AAC MPEG-TS output can be honored.
    segment_container: Option<String>,
    audio_codec: Option<String>,
    video_codec: Option<String>,
    require_avc: Option<bool>,
    audio_stream_index: Option<i64>,
    video_stream_index: Option<i64>,
    subtitle_stream_index: Option<i64>,
    subtitle_method: Option<String>,
    // Only the internal continuation handler can set this. Never accepted
    // from an HTTP query, even if the caller guesses the field name.
    #[serde(skip)]
    expected_revision: Option<ExpectedRevision>,
}

/// An internal-only chain guard: continuation must not silently move from
/// the authorized original source/sidecar bytes to a newly selected revision.
#[derive(Debug, Clone)]
struct ExpectedRevision {
    source: PathBuf,
    size: u64,
    modified: Option<std::time::SystemTime>,
    sidecar: Option<SidecarRevision>,
}

impl ExpectedRevision {
    fn matches(
        &self,
        source: &Path,
        size: u64,
        modified: Option<std::time::SystemTime>,
        sidecar: Option<&SidecarRevision>,
    ) -> bool {
        self.source == source
            && self.size == size
            && self.modified == modified
            && self.sidecar.as_ref() == sidecar
    }
}

/// Recognize explicit pinned v12.3 codec/container constraints without
/// silently accepting an output this bounded FFmpeg pipeline cannot produce.
fn validate_hls_output_request(query: &PlaylistQuery, kind: &str) -> Result<(), StatusCode> {
    if query
        .segment_container
        .as_deref()
        .is_some_and(|container| !matches!(container, "ts" | "mpegts"))
        || query
            .audio_codec
            .as_deref()
            .is_some_and(|codec| codec != "aac")
        || query
            .video_codec
            .as_deref()
            .is_some_and(|codec| kind != "video" || codec != "h264")
        || (query.require_avc == Some(true) && kind != "video")
    {
        return Err(StatusCode::NOT_IMPLEMENTED);
    }
    Ok(())
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
    let metadata = tokio::fs::metadata(&source)
        .await
        .map_err(|_| StatusCode::NOT_FOUND)?;
    if metadata.len() != query.size || metadata.modified().ok() != query.modified {
        return Err(StatusCode::NOT_FOUND);
    }
    if let Some(sidecar) = query.sidecar {
        let bytes = read_selected_sidecar(&source, &sidecar.filename)
            .await
            .map_err(|_| StatusCode::NOT_FOUND)?;
        let digest: [u8; 32] = Sha256::digest(&bytes).into();
        if digest != sidecar.digest {
            return Err(StatusCode::NOT_FOUND);
        }
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
    validate_hls_output_request(&query, expected_kind)?;
    let target_seconds = query.segment_length.unwrap_or(10);
    if !(1..=10).contains(&target_seconds) {
        return Err(StatusCode::BAD_REQUEST);
    }
    let excerpt_duration_ticks = query
        .pelagos_excerpt_duration_ticks
        .map(|duration| {
            u64::try_from(duration)
                .ok()
                .filter(|duration| (1_000_000..=6_000_000_000).contains(duration))
                .ok_or(StatusCode::BAD_REQUEST)
        })
        .transpose()?;
    let start_time_ticks = query.start_time_ticks.unwrap_or(0);
    let start_time_ticks = u64::try_from(start_time_ticks).map_err(|_| StatusCode::BAD_REQUEST)?;
    if let Some(duration) = excerpt_duration_ticks {
        let end = start_time_ticks
            .checked_add(duration)
            .ok_or(StatusCode::BAD_REQUEST)?;
        if end > MAX_PGS_START_END_TICKS {
            return Err(StatusCode::BAD_REQUEST);
        }
    } else if start_time_ticks > 6_000_000_000 {
        // Normal Jellyfin-compatible full-VOD requests retain the old limit.
        return Err(StatusCode::BAD_REQUEST);
    }
    // The pinned v12.3 master routes carry MediaSourceId and the opt-in
    // adaptive switch. Pelagos PlaybackInfo currently exposes one source
    // whose ID exactly equals the item ID; never silently transcode a
    // different requested source or ignore an explicit adaptive choice.
    let default_renditions = if query.enable_adaptive_bitrate_streaming == Some(true) {
        2
    } else {
        1
    };
    let rendition_count = query.renditions.unwrap_or(default_renditions);
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
        .is_some_and(|id| id != item.as_str())
    {
        return Err(StatusCode::NOT_FOUND);
    }
    let (_, scope) = auth_api::library_scope(state.clone(), headers.clone()).await?;
    let (source, kind) = playback::media_file(&state, item.clone(), scope).await?;
    if kind != expected_kind {
        return Err(StatusCode::NOT_FOUND);
    }
    let burn_in_subtitle =
        validate_hls_track_selection(&state, &source, &query, expected_kind).await?;
    if excerpt_duration_ticks.is_some()
        && (expected_kind != "video"
            || !matches!(
                burn_in_subtitle,
                Some(HlsSubtitle::ExternalPgs { .. } | HlsSubtitle::EmbeddedPgs(_))
            ))
    {
        return Err(StatusCode::NOT_IMPLEMENTED);
    }
    let selected_index = query.subtitle_stream_index.filter(|index| *index >= 0);
    let sidecar_revision = match burn_in_subtitle.as_ref() {
        Some(HlsSubtitle::ExternalSrt { revision, .. })
        | Some(HlsSubtitle::ExternalPgs { revision, .. }) => Some(revision.clone()),
        _ => None,
    };
    let metadata = tokio::fs::metadata(&source)
        .await
        .map_err(|_| StatusCode::NOT_FOUND)?;
    let source_size = metadata.len();
    let source_modified = metadata.modified().ok();
    if query.expected_revision.as_ref().is_some_and(|expected| {
        !expected.matches(
            &source,
            source_size,
            source_modified,
            sidecar_revision.as_ref(),
        )
    }) {
        return Err(StatusCode::NOT_FOUND);
    }
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
        excerpt_duration_ticks,
        rendition_count,
        subtitle_stream_index: selected_index,
        sidecar: sidecar_revision.as_ref(),
    };
    if let Some(existing) = registry.cached(&cache_key) {
        let mut response = excerpt_playlist_response(
            if master {
                &existing.master_playlist
            } else {
                &existing.main_playlist
            },
            existing.start_time_ticks,
            existing.excerpt_duration_ticks,
        );
        set_continuation_path(
            &mut response,
            &item,
            &existing.id,
            existing.excerpt_duration_ticks,
        );
        return Ok(response);
    }
    // Admission is reserved before touching any private output path.
    let _permit = registry.acquire_generation().await?;
    reauthorize_publication(&state, &headers, &cache_key).await?;
    // Recheck after reserving the slot: the previous request may have
    // just published this rendition and no second encode is necessary.
    if let Some(existing) = registry.cached(&cache_key) {
        let mut response = excerpt_playlist_response(
            if master {
                &existing.master_playlist
            } else {
                &existing.main_playlist
            },
            existing.start_time_ticks,
            existing.excerpt_duration_ticks,
        );
        set_continuation_path(
            &mut response,
            &item,
            &existing.id,
            existing.excerpt_duration_ticks,
        );
        return Ok(response);
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
        let subtitle = burn_in_subtitle.as_ref().map(|track| match track {
            HlsSubtitle::Embedded(index) => hls::AdaptiveSubtitle::Embedded(*index),
            HlsSubtitle::ExternalSrt { bytes, revision } => {
                let format = if revision.filename.to_ascii_lowercase().ends_with(".ass") {
                    ExternalTextFormat::Ass
                } else if revision.filename.to_ascii_lowercase().ends_with(".ssa") {
                    ExternalTextFormat::Ssa
                } else {
                    ExternalTextFormat::Srt
                };
                hls::AdaptiveSubtitle::External(ExternalTextTrack { bytes, format })
            }
            HlsSubtitle::ExternalPgs { bytes, .. } | HlsSubtitle::EmbeddedPgs(bytes) => {
                hls::AdaptiveSubtitle::Pgs(bytes)
            }
        });
        let adaptive = if let (Some(duration), Some(hls::AdaptiveSubtitle::Pgs(sup_bytes))) =
            (excerpt_duration_ticks, subtitle)
        {
            hls::generate_adaptive_segmented_pgs_window(
                &registry.controller,
                state.probe_executable(),
                &source,
                &private_root.join("bundle"),
                target_seconds,
                hls::AdaptivePgsPlaybackWindow {
                    start_seconds: start_time_ticks as f64 / 10_000_000.0,
                    duration_seconds: duration as f64 / 10_000_000.0,
                    qualities: &profiles[..usize::from(rendition_count)],
                    sup_bytes,
                },
            )
            .await
        } else {
            hls::generate_adaptive_segmented_vod_with_subtitle_from(
                &registry.controller,
                state.probe_executable(),
                &source,
                &private_root.join("bundle"),
                target_seconds,
                hls::AdaptiveWindow {
                    start_seconds: start_time_ticks as f64 / 10_000_000.0,
                    qualities: &profiles[..usize::from(rendition_count)],
                    subtitle,
                },
            )
            .await
        }
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
        let mut response =
            excerpt_playlist_response(&master_playlist, start_time_ticks, excerpt_duration_ticks);
        set_continuation_path(&mut response, &item, &id, excerpt_duration_ticks);
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
            excerpt_duration_ticks,
            rendition_count,
            subtitle_stream_index: selected_index,
            sidecar: sidecar_revision.clone(),
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
    let artifact = match burn_in_subtitle.as_ref() {
        Some(HlsSubtitle::Embedded(index)) => {
            hls::generate_segmented_vod_with_burn_in_from(
                &registry.controller,
                state.probe_executable(),
                &source,
                &private_root.join("bundle"),
                target_seconds,
                start_time_ticks as f64 / 10_000_000.0,
                *index,
            )
            .await
        }
        Some(HlsSubtitle::ExternalSrt { bytes, revision }) => {
            let format = if revision.filename.to_ascii_lowercase().ends_with(".ass") {
                ExternalTextFormat::Ass
            } else if revision.filename.to_ascii_lowercase().ends_with(".ssa") {
                ExternalTextFormat::Ssa
            } else {
                ExternalTextFormat::Srt
            };
            hls::generate_segmented_vod_with_external_text_from(
                &registry.controller,
                state.probe_executable(),
                &source,
                &private_root.join("bundle"),
                target_seconds,
                start_time_ticks as f64 / 10_000_000.0,
                ExternalTextTrack { bytes, format },
            )
            .await
        }
        Some(HlsSubtitle::EmbeddedPgs(bytes)) | Some(HlsSubtitle::ExternalPgs { bytes, .. }) => {
            if let Some(duration) = excerpt_duration_ticks {
                hls::generate_segmented_pgs_window(
                    &registry.controller,
                    state.probe_executable(),
                    &source,
                    &private_root.join("bundle"),
                    target_seconds,
                    hls::PgsPlaybackWindow {
                        start_seconds: start_time_ticks as f64 / 10_000_000.0,
                        duration_seconds: duration as f64 / 10_000_000.0,
                        sup_bytes: bytes,
                    },
                )
                .await
            } else {
                hls::generate_segmented_vod_with_pgs_from(
                    &registry.controller,
                    state.probe_executable(),
                    &source,
                    &private_root.join("bundle"),
                    target_seconds,
                    start_time_ticks as f64 / 10_000_000.0,
                    bytes,
                )
                .await
            }
        }
        None => {
            hls::generate_segmented_vod_from(
                &registry.controller,
                state.probe_executable(),
                &source,
                &private_root.join("bundle"),
                target_seconds,
                start_time_ticks as f64 / 10_000_000.0,
            )
            .await
        }
    }
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
    let mut response = excerpt_playlist_response(
        if master { &master_playlist } else { &playlist },
        start_time_ticks,
        excerpt_duration_ticks,
    );
    set_continuation_path(&mut response, &item, &id, excerpt_duration_ticks);
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
        excerpt_duration_ticks,
        rendition_count,
        subtitle_stream_index: selected_index,
        sidecar: sidecar_revision.clone(),
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
            "/Videos/{item_id}/hls1/{playlist_id}/next.m3u8",
            get(video_next).head(|| async { StatusCode::METHOD_NOT_ALLOWED }),
        )
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
    use super::tracks::read_confined_sup;
    use super::*;

    #[cfg(unix)]
    #[tokio::test]
    async fn graphical_sidecar_read_is_confined_bounded_and_rejects_symlinks() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().unwrap();
        let library = temp.path().join("library");
        std::fs::create_dir(&library).unwrap();
        let source = library.join("Feature.mkv");
        std::fs::write(&source, b"synthetic media").unwrap();
        let sidecar = library.join("Feature.es.sup");
        std::fs::write(&sidecar, b"PG synthetic bytes").unwrap();
        assert_eq!(
            read_confined_sup(&source, "Feature.es.sup").await,
            Ok(b"PG synthetic bytes".to_vec())
        );
        assert_eq!(
            read_confined_sup(&source, "Feature.es.srt").await,
            Err(StatusCode::NOT_IMPLEMENTED)
        );
        assert!(read_confined_sup(&source, "../outside.sup").await.is_err());

        let outside = temp.path().join("outside.sup");
        std::fs::write(&outside, b"private file").unwrap();
        symlink(&outside, library.join("Feature.fr.sup")).unwrap();
        assert!(read_confined_sup(&source, "Feature.fr.sup").await.is_err());

        std::fs::File::create(library.join("Feature.de.sup"))
            .unwrap()
            .set_len(crate::pgs::MAX_SUP_BYTES + 1)
            .unwrap();
        assert_eq!(
            read_confined_sup(&source, "Feature.de.sup").await,
            Err(StatusCode::NOT_IMPLEMENTED)
        );
        assert_eq!(std::fs::read(&source).unwrap(), b"synthetic media");
    }

    #[test]
    fn pinned_hls_codec_and_container_constraints_are_fail_closed() {
        let mut query = PlaylistQuery::default();
        assert_eq!(validate_hls_output_request(&query, "video"), Ok(()));
        assert_eq!(validate_hls_output_request(&query, "audio"), Ok(()));
        query.segment_container = Some("ts".to_owned());
        query.audio_codec = Some("aac".to_owned());
        query.video_codec = Some("h264".to_owned());
        query.require_avc = Some(true);
        assert_eq!(validate_hls_output_request(&query, "video"), Ok(()));
        assert_eq!(
            validate_hls_output_request(&query, "audio"),
            Err(StatusCode::NOT_IMPLEMENTED)
        );
        query.video_codec = None;
        query.require_avc = None;
        assert_eq!(validate_hls_output_request(&query, "audio"), Ok(()));
        for invalid_container in ["mp4", "fmp4", ""] {
            query.segment_container = Some(invalid_container.to_owned());
            assert_eq!(
                validate_hls_output_request(&query, "video"),
                Err(StatusCode::NOT_IMPLEMENTED)
            );
        }
        query.segment_container = Some("mpegts".to_owned());
        query.audio_codec = Some("mp3".to_owned());
        assert_eq!(
            validate_hls_output_request(&query, "video"),
            Err(StatusCode::NOT_IMPLEMENTED)
        );
    }

    #[test]
    fn private_continuation_revision_guard_rejects_source_and_sidecar_changes() {
        let sidecar = SidecarRevision {
            filename: "feature.es.sup".to_owned(),
            digest: [42; 32],
        };
        let source = Path::new("/synthetic/feature.mkv");
        let original = ExpectedRevision {
            source: source.to_path_buf(),
            size: 1234,
            modified: Some(std::time::SystemTime::UNIX_EPOCH),
            sidecar: Some(sidecar.clone()),
        };
        assert!(original.matches(
            source,
            1234,
            Some(std::time::SystemTime::UNIX_EPOCH),
            Some(&sidecar),
        ));
        assert!(!original.matches(
            Path::new("/synthetic/replacement.mkv"),
            1234,
            Some(std::time::SystemTime::UNIX_EPOCH),
            Some(&sidecar),
        ));
        assert!(!original.matches(
            source,
            1235,
            Some(std::time::SystemTime::UNIX_EPOCH),
            Some(&sidecar),
        ));
        assert!(!original.matches(source, 1234, None, Some(&sidecar)));
        assert!(!original.matches(
            source,
            1234,
            Some(std::time::SystemTime::UNIX_EPOCH),
            Some(&SidecarRevision {
                filename: sidecar.filename.clone(),
                digest: [99; 32],
            }),
        ));
        let unchanged_mtime = Some(std::time::SystemTime::UNIX_EPOCH);
        assert!(!original.matches(source, 1234, unchanged_mtime, None));
        let query: PlaylistQuery =
            serde_json::from_str(r#"{"pelagosExcerptDurationTicks":10000000}"#).unwrap();
        assert!(query.expected_revision.is_none());
        // The caller cannot inject or override the internal chain guard.
        let injected =
            serde_json::from_str::<PlaylistQuery>(r#"{"expectedRevision":{"size":1234}}"#);
        assert!(injected.is_err());
    }

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
                    excerpt_duration_ticks: None,
                    rendition_count: 1,
                    subtitle_stream_index: None,
                    sidecar: None,
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
                    excerpt_duration_ticks: None,
                    rendition_count: 1,
                    subtitle_stream_index: None,
                    sidecar: None,
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
                    excerpt_duration_ticks: None,
                    rendition_count: 1,
                    subtitle_stream_index: None,
                    sidecar: None,
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
                    excerpt_duration_ticks: None,
                    rendition_count: 1,
                    subtitle_stream_index: None,
                    sidecar: None,
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
                    excerpt_duration_ticks: None,
                    rendition_count: 1,
                    subtitle_stream_index: None,
                    sidecar: None,
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
                excerpt_duration_ticks: None,
                rendition_count: 1,
                subtitle_stream_index: None,
                sidecar: None,
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
            excerpt_duration_ticks: None,
            rendition_count: 1,
            subtitle_stream_index: None,
            sidecar: None,
        };
        // Multiple synthetic entries use the same cache key; the most
        // recently used publication is now returned, rather than the one
        // with the highest original insertion position.
        assert_eq!(registry.cached(&key).unwrap().id, "playlist-0");
        // An explicitly bounded excerpt cannot reuse the same session's
        // complete-VOD publication or another excerpt duration.
        assert!(
            registry
                .cached(&CacheLookup {
                    excerpt_duration_ticks: Some(40_000_000),
                    ..key
                })
                .is_none()
        );
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
        // Selected subtitle overlays must never reuse unburned or
        // differently selected output, even for the same user and source.
        for index in [0, 1, 31] {
            assert!(
                registry
                    .cached(&CacheLookup {
                        subtitle_stream_index: Some(index),
                        ..key
                    })
                    .is_none()
            );
        }
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
            excerpt_duration_ticks: None,
            rendition_count: 1,
            subtitle_stream_index: None,
            sidecar: None,
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
