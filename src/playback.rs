//! Conservative Jellyfin-shaped playback metadata, derived from FFprobe.
//! This is not playback negotiation parity: only static file sources are offered.
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    sync::Arc,
};

use axum::{
    Json, Router,
    extract::{Path as RoutePath, State},
    http::{HeaderMap, StatusCode},
    routing::get,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{app::AppState, auth_api, catalog, catalog_api, probe};

#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
pub struct PlaybackRequest {
    enable_direct_play: Option<bool>,
    enable_direct_stream: Option<bool>,
    enable_transcoding: Option<bool>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct PlaybackInfo {
    media_sources: Vec<MediaSource>,
    play_session_id: Option<String>,
    error_code: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct MediaSource {
    protocol: &'static str,
    id: String,
    #[serde(rename = "Type")]
    source_type: &'static str,
    container: String,
    size: u64,
    name: String,
    is_remote: bool,
    run_time_ticks: Option<i64>,
    supports_direct_play: bool,
    supports_direct_stream: bool,
    supports_transcoding: bool,
    media_streams: Vec<MediaStream>,
    required_http_headers: std::collections::BTreeMap<String, String>,
    direct_stream_url: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct MediaStream {
    index: i64,
    #[serde(rename = "Type")]
    stream_type: String,
    codec: Option<String>,
    language: Option<String>,
    width: Option<i64>,
    height: Option<i64>,
    bit_rate: Option<i64>,
    channels: Option<i64>,
    sample_rate: Option<i64>,
    profile: Option<String>,
    title: Option<String>,
    aspect_ratio: Option<String>,
    pixel_format: Option<String>,
    channel_layout: Option<String>,
    time_base: Option<String>,
    codec_time_base: Option<String>,
    color_range: Option<String>,
    color_space: Option<String>,
    color_transfer: Option<String>,
    color_primaries: Option<String>,
    bit_depth: Option<i64>,
    average_frame_rate: Option<f64>,
    real_frame_rate: Option<f64>,
    is_interlaced: bool,
    is_hearing_impaired: bool,
    is_original: bool,
    is_default: bool,
    is_forced: bool,
}

fn number(value: &Value) -> Option<i64> {
    value
        .as_i64()
        .or_else(|| value.as_str()?.parse::<i64>().ok())
}

fn text_field(value: &Value) -> Option<String> {
    value
        .as_str()
        .filter(|text| text.len() <= 256 && !text.chars().any(char::is_control))
        .map(str::to_owned)
}

fn frame_rate(value: &Value) -> Option<f64> {
    let raw = value.as_str()?;
    let (numerator, denominator) = raw.split_once('/')?;
    let numerator = numerator.parse::<f64>().ok()?;
    let denominator = denominator.parse::<f64>().ok()?;
    if !numerator.is_finite() || !denominator.is_finite() || denominator <= 0.0 {
        return None;
    }
    let fps = numerator / denominator;
    (fps.is_finite() && fps > 0.0 && fps <= 1_000.0).then_some(fps)
}

fn mapped_stream(value: &Value) -> Option<MediaStream> {
    let stream_type = match value["codec_type"].as_str()? {
        "video" => "Video",
        "audio" => "Audio",
        "subtitle" => "Subtitle",
        _ => return None,
    };
    let index = number(&value["index"])?;
    Some(MediaStream {
        index,
        stream_type: stream_type.to_owned(),
        codec: value["codec_name"].as_str().map(str::to_owned),
        language: value["tags"]["language"].as_str().map(str::to_owned),
        width: number(&value["width"]),
        height: number(&value["height"]),
        bit_rate: number(&value["bit_rate"]),
        channels: number(&value["channels"]),
        sample_rate: number(&value["sample_rate"]),
        profile: text_field(&value["profile"]),
        title: text_field(&value["tags"]["title"]),
        aspect_ratio: text_field(&value["display_aspect_ratio"]),
        pixel_format: text_field(&value["pix_fmt"]),
        channel_layout: text_field(&value["channel_layout"]),
        time_base: text_field(&value["time_base"]),
        codec_time_base: text_field(&value["codec_time_base"]),
        color_range: text_field(&value["color_range"]),
        color_space: text_field(&value["color_space"]),
        color_transfer: text_field(&value["color_transfer"]),
        color_primaries: text_field(&value["color_primaries"]),
        bit_depth: number(&value["bits_per_raw_sample"])
            .or_else(|| number(&value["bits_per_sample"])),
        average_frame_rate: frame_rate(&value["avg_frame_rate"]),
        real_frame_rate: frame_rate(&value["r_frame_rate"]),
        is_interlaced: matches!(
            value["field_order"].as_str(),
            Some("tt" | "bb" | "tb" | "bt")
        ),
        is_hearing_impaired: number(&value["disposition"]["hearing_impaired"]) == Some(1),
        is_original: number(&value["disposition"]["original"]) == Some(1),
        is_default: number(&value["disposition"]["default"]) == Some(1),
        is_forced: number(&value["disposition"]["forced"]) == Some(1),
    })
}

fn source_from_probe(
    item_id: &str,
    kind: &str,
    file: &Path,
    result: &probe::ProbeReport,
) -> MediaSource {
    let container = file
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let name = file
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "Video".to_owned());
    let run_time_ticks = result
        .format
        .as_ref()
        .and_then(|f| f["duration"].as_str())
        .and_then(|s| s.parse::<f64>().ok())
        .filter(|seconds| {
            seconds.is_finite() && *seconds >= 0.0 && *seconds <= i64::MAX as f64 / 10_000_000.0
        })
        .map(|seconds| (seconds * 10_000_000.0).round() as i64);
    MediaSource {
        protocol: "File",
        id: item_id.to_owned(),
        source_type: "Default",
        container,
        size: result.size_bytes,
        name,
        is_remote: false,
        run_time_ticks,
        supports_direct_play: true,
        supports_direct_stream: false,
        supports_transcoding: false,
        media_streams: result.streams.iter().filter_map(mapped_stream).collect(),
        required_http_headers: Default::default(),
        direct_stream_url: format!(
            "/{}/{item_id}/stream?static=true",
            if kind == "audio" { "Audio" } else { "Videos" }
        ),
    }
}

/// Lookup only authorized, indexed audio/video and resolve them within the root.
async fn media_file(
    state: &AppState,
    item_id: String,
    allowed: Option<HashSet<String>>,
) -> Result<(PathBuf, &'static str), StatusCode> {
    let dir = state.auth_dir().ok_or(StatusCode::SERVICE_UNAVAILABLE)?;
    let found = tokio::task::spawn_blocking(move || catalog::all_files(&dir))
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .into_iter()
        .find(|file| {
            let can_read = allowed
                .as_ref()
                .is_none_or(|roots| roots.contains(&file.root));
            can_read
                && matches!(file.kind.as_str(), "audio" | "video")
                && catalog_api::stable_id(&file.root, &file.relative_path) == item_id
        })
        .ok_or(StatusCode::NOT_FOUND)?;
    let root = tokio::fs::canonicalize(&found.root)
        .await
        .map_err(|_| StatusCode::NOT_FOUND)?;
    let path = tokio::fs::canonicalize(Path::new(&found.root).join(&found.relative_path))
        .await
        .map_err(|_| StatusCode::NOT_FOUND)?;
    if !path.starts_with(&root) {
        return Err(StatusCode::FORBIDDEN);
    }
    let kind = if found.kind == "audio" {
        "audio"
    } else {
        "video"
    };
    Ok((path, kind))
}

async fn playback_info(
    state: Arc<AppState>,
    item_id: String,
    headers: HeaderMap,
    req: PlaybackRequest,
) -> Result<Json<PlaybackInfo>, StatusCode> {
    let (_, allowed) = auth_api::library_scope(state.clone(), headers).await?;
    // Unknown negotiation criteria cannot be promised as implemented.
    // Jellyfin's fallback booleans describe allowed capabilities, not
    // requested conversions. Record the permissions but only advertise direct
    // play until remuxing and transcoding are actually implemented.
    tracing::debug!(
        direct_stream_allowed = req.enable_direct_stream.unwrap_or(true),
        transcoding_allowed = req.enable_transcoding.unwrap_or(true),
        "client fallback permissions do not imply source capability"
    );
    if req.enable_direct_play == Some(false) {
        return Err(StatusCode::NOT_IMPLEMENTED);
    }
    let (file, kind) = media_file(&state, item_id.clone(), allowed).await?;
    let probed = probe::probe(&file, state.probe_executable(), probe::DEFAULT_TIMEOUT)
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    Ok(Json(PlaybackInfo {
        media_sources: vec![source_from_probe(&item_id, kind, &file, &probed)],
        play_session_id: None,
        error_code: None,
    }))
}

async fn get_playback(
    State(state): State<Arc<AppState>>,
    RoutePath(item_id): RoutePath<String>,
    headers: HeaderMap,
) -> Result<Json<PlaybackInfo>, StatusCode> {
    playback_info(state, item_id, headers, PlaybackRequest::default()).await
}

async fn post_playback(
    State(state): State<Arc<AppState>>,
    RoutePath(item_id): RoutePath<String>,
    headers: HeaderMap,
    Json(query): Json<PlaybackRequest>,
) -> Result<Json<PlaybackInfo>, StatusCode> {
    playback_info(state, item_id, headers, query).await
}

pub fn router() -> Router<Arc<AppState>> {
    Router::new().route(
        "/Items/{item_id}/PlaybackInfo",
        get(get_playback).post(post_playback),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn maps_probe_to_conservative_jellyfin_stream_fields() {
        let source = probe::ProbeReport {
            path: PathBuf::from("synthetic.mp4"),
            size_bytes: 17,
            format: Some(json!({"duration":"60.123"})),
            streams: vec![
                json!({"index":0,"codec_type":"video","codec_name":"h264","width":1920,"height":1080,"bit_rate":"4000000","disposition":{"default":1}}),
                json!({"index":1,"codec_type":"audio","codec_name":"aac","channels":2,"sample_rate":"48000","tags":{"language":"eng"}}),
            ],
            chapters: vec![],
        };
        let media = source_from_probe("abc", "video", Path::new("synthetic.mp4"), &source);
        assert_eq!(media.run_time_ticks, Some(601_230_000));
        assert_eq!(media.media_streams.len(), 2);
        assert_eq!(media.media_streams[0].stream_type, "Video");
        assert_eq!(media.media_streams[1].sample_rate, Some(48000));
        assert!(!media.supports_transcoding);
        assert!(!media.supports_direct_stream);
        assert!(media.supports_direct_play);
        assert_eq!(media.direct_stream_url, "/Videos/abc/stream?static=true");
    }

    #[test]
    fn audio_sources_use_the_audio_stream_route() {
        let source = probe::ProbeReport {
            path: PathBuf::from("sound.flac"),
            size_bytes: 10,
            format: Some(json!({"duration":"12.5"})),
            streams: vec![
                json!({"index":0,"codec_type":"audio","codec_name":"flac","sample_rate":"96000","channels":2}),
            ],
            chapters: vec![],
        };
        let media = source_from_probe("track-id", "audio", Path::new("sound.flac"), &source);
        assert_eq!(
            media.direct_stream_url,
            "/Audio/track-id/stream?static=true"
        );
        assert_eq!(media.run_time_ticks, Some(125_000_000));
        assert!(media.supports_direct_play);
        assert!(!media.supports_transcoding);
        assert_eq!(media.media_streams[0].codec.as_deref(), Some("flac"));
    }


    #[test]
    fn media_stream_profiles_and_fps_only_report_probed_values() {
        let stream = json!({
            "index": 0,
            "codec_type": "video",
            "codec_name": "hevc",
            "profile": "Main 10",
            "width": 3840,
            "height": 2160,
            "pix_fmt": "yuv420p10le",
            "display_aspect_ratio": "16:9",
            "avg_frame_rate": "24000/1001",
            "r_frame_rate": "24/1",
            "color_primaries": "bt2020",
            "color_transfer": "smpte2084",
            "color_space": "bt2020nc",
            "color_range": "tv",
            "bits_per_raw_sample": "10",
            "time_base": "1/24000",
            "field_order": "progressive",
            "tags": {"title": "Main video"},
            "disposition": {"default": 1, "forced": 0, "original": 1}
        });
        let mapped = mapped_stream(&stream).unwrap();
        assert_eq!(mapped.profile.as_deref(), Some("Main 10"));
        assert_eq!(mapped.pixel_format.as_deref(), Some("yuv420p10le"));
        assert_eq!(mapped.bit_depth, Some(10));
        assert_eq!(mapped.aspect_ratio.as_deref(), Some("16:9"));
        assert_eq!(mapped.color_transfer.as_deref(), Some("smpte2084"));
        assert_eq!(mapped.title.as_deref(), Some("Main video"));
        assert_eq!(mapped.real_frame_rate, Some(24.0));
        assert!((mapped.average_frame_rate.unwrap() - 23.976_023_976).abs() < 1e-7);
        assert!(!mapped.is_interlaced);
        assert!(mapped.is_default);
        assert!(mapped.is_original);
        assert_eq!(frame_rate(&json!("0/0")), None);
        assert_eq!(frame_rate(&json!("30/0")), None);
        assert_eq!(frame_rate(&json!("NaN/1")), None);
        assert_eq!(frame_rate(&json!("30000/1")), None);
    }

    #[test]
    fn subtitle_dispositions_and_audio_layout_are_not_inferred() {
        let subtitle = mapped_stream(&json!({
            "index": 2,
            "codec_type": "subtitle",
            "codec_name": "subrip",
            "tags": {"language": "eng", "title": "SDH"},
            "disposition": {"hearing_impaired": 1, "forced": 1}
        }))
        .unwrap();
        assert!(subtitle.is_hearing_impaired);
        assert!(subtitle.is_forced);
        assert_eq!(subtitle.average_frame_rate, None);
        let audio = mapped_stream(&json!({
            "index": 1,
            "codec_type": "audio",
            "codec_name": "flac",
            "channel_layout": "5.1(side)",
            "sample_rate": "48000",
            "bits_per_sample": 24
        }))
        .unwrap();
        assert_eq!(audio.channel_layout.as_deref(), Some("5.1(side)"));
        assert_eq!(audio.bit_depth, Some(24));
        assert!(!audio.is_original);
    }

    #[test]
    fn invalid_duration_is_omitted_and_unknown_stream_ignored() {
        let source = probe::ProbeReport {
            path: PathBuf::from("x.mkv"),
            size_bytes: 0,
            format: Some(json!({"duration":"NaN"})),
            streams: vec![json!({"codec_type":"data","index":3})],
            chapters: vec![],
        };
        let result = source_from_probe("abc", "video", Path::new("x.mkv"), &source);
        assert_eq!(result.run_time_ticks, None);
        assert!(result.media_streams.is_empty());
    }
}
