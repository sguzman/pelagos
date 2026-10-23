//! Conservative Jellyfin-shaped playback metadata, derived from FFprobe.
//! This is not playback negotiation parity: only static file sources are offered.
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use axum::{
    Json, Router,
    extract::{Path as RoutePath, State},
    http::{HeaderMap, StatusCode},
    routing::{get, post},
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
    is_default: bool,
    is_forced: bool,
}

fn number(value: &Value) -> Option<i64> {
    value
        .as_i64()
        .or_else(|| value.as_str()?.parse::<i64>().ok())
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
        is_default: value["disposition"]["default"] == 1,
        is_forced: value["disposition"]["forced"] == 1,
    })
}

fn source_from_probe(item_id: &str, file: &Path, result: &probe::ProbeReport) -> MediaSource {
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
    }
}

/// Lookup only catalog-indexed video files and resolve them under the configured root.
async fn media_file(state: &AppState, item_id: String) -> Result<PathBuf, StatusCode> {
    let dir = state.auth_dir().ok_or(StatusCode::SERVICE_UNAVAILABLE)?;
    let found = tokio::task::spawn_blocking(move || catalog::all_files(&dir))
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .into_iter()
        .find(|file| {
            file.kind == "video"
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
    Ok(path)
}

async fn playback_info(
    state: Arc<AppState>,
    item_id: String,
    headers: HeaderMap,
    req: PlaybackRequest,
) -> Result<Json<PlaybackInfo>, StatusCode> {
    let (user, _) = auth_api::authenticated(state.clone(), headers).await?;
    if !user.policy.is_administrator {
        return Err(StatusCode::FORBIDDEN);
    }
    // Unknown negotiation criteria cannot be promised as implemented.
    if req.enable_direct_play == Some(false)
        || req.enable_direct_stream == Some(true)
        || req.enable_transcoding == Some(true)
    {
        return Err(StatusCode::NOT_IMPLEMENTED);
    }
    let file = media_file(&state, item_id.clone()).await?;
    let probed = probe::probe(&file, Path::new("ffprobe"), probe::DEFAULT_TIMEOUT)
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    Ok(Json(PlaybackInfo {
        media_sources: vec![source_from_probe(&item_id, &file, &probed)],
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
        let media = source_from_probe("abc", Path::new("synthetic.mp4"), &source);
        assert_eq!(media.run_time_ticks, Some(601_230_000));
        assert_eq!(media.media_streams.len(), 2);
        assert_eq!(media.media_streams[0].stream_type, "Video");
        assert_eq!(media.media_streams[1].sample_rate, Some(48000));
        assert!(!media.supports_transcoding);
        assert!(!media.supports_direct_stream);
        assert!(media.supports_direct_play);
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
        let result = source_from_probe("abc", Path::new("x.mkv"), &source);
        assert_eq!(result.run_time_ticks, None);
        assert!(result.media_streams.is_empty());
    }
}
