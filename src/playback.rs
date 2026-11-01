//! Conservative Jellyfin-shaped playback metadata, derived from FFprobe.
//! This is not playback negotiation parity: only static file sources are offered.
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    process::Stdio,
    sync::{Arc, OnceLock},
    time::Duration,
};

use axum::{
    Json, Router,
    body::Body,
    extract::{Path as RoutePath, Query, State},
    http::{
        HeaderMap, HeaderValue, StatusCode,
        header::{CACHE_CONTROL, CONTENT_LENGTH, CONTENT_TYPE},
    },
    response::Response,
    routing::get,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{app::AppState, auth, auth_api, catalog, catalog_api, probe, streaming};
use tokio::{io::AsyncReadExt, process::Command, sync::Semaphore};
use tokio_util::io::ReaderStream;

#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
pub struct PlaybackRequest {
    user_id: Option<String>,
    media_source_id: Option<String>,
    start_time_ticks: Option<i64>,
    audio_stream_index: Option<i64>,
    subtitle_stream_index: Option<i64>,
    auto_open_live_stream: Option<bool>,
    enable_direct_play: Option<bool>,
    enable_direct_stream: Option<bool>,
    enable_transcoding: Option<bool>,
    max_streaming_bitrate: Option<i64>,
    max_audio_channels: Option<i64>,
    device_profile: Option<Value>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PlaybackQuery {
    user_id: Option<String>,
    media_source_id: Option<String>,
    start_time_ticks: Option<i64>,
    audio_stream_index: Option<i64>,
    subtitle_stream_index: Option<i64>,
    auto_open_live_stream: Option<bool>,
    enable_direct_play: Option<bool>,
    enable_direct_stream: Option<bool>,
    enable_transcoding: Option<bool>,
    max_streaming_bitrate: Option<i64>,
    max_audio_channels: Option<i64>,
}

impl PlaybackQuery {
    // Jellyfin v12.3 permits both POST body and deprecated query fields,
    // with the query taking precedence when explicitly provided.
    fn apply(self, mut request: PlaybackRequest) -> PlaybackRequest {
        if self.user_id.is_some() {
            request.user_id = self.user_id;
        }
        if self.media_source_id.is_some() {
            request.media_source_id = self.media_source_id;
        }
        if self.start_time_ticks.is_some() {
            request.start_time_ticks = self.start_time_ticks;
        }
        if self.audio_stream_index.is_some() {
            request.audio_stream_index = self.audio_stream_index;
        }
        if self.subtitle_stream_index.is_some() {
            request.subtitle_stream_index = self.subtitle_stream_index;
        }
        if self.auto_open_live_stream.is_some() {
            request.auto_open_live_stream = self.auto_open_live_stream;
        }
        if self.enable_direct_play.is_some() {
            request.enable_direct_play = self.enable_direct_play;
        }
        if self.enable_direct_stream.is_some() {
            request.enable_direct_stream = self.enable_direct_stream;
        }
        if self.enable_transcoding.is_some() {
            request.enable_transcoding = self.enable_transcoding;
        }
        if self.max_streaming_bitrate.is_some() {
            request.max_streaming_bitrate = self.max_streaming_bitrate;
        }
        if self.max_audio_channels.is_some() {
            request.max_audio_channels = self.max_audio_channels;
        }
        request
    }
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
    // Pinned MediaSourceInfo provides these fields to unmodified clients.
    // Never infer a bitrate or an ambiguous default track.
    bitrate: Option<i64>,
    video_type: Option<&'static str>,
    default_audio_stream_index: Option<i64>,
    default_subtitle_stream_index: Option<i64>,
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
    is_external: bool,
    is_text_subtitle_stream: bool,
    delivery_method: Option<&'static str>,
    delivery_url: Option<String>,
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

/// Conservative classification for probed embedded text subtitles,
/// consistent with the pinned MediaStream.IsTextSubtitleStream property.
fn is_text_subtitle(codec: Option<&str>) -> bool {
    let Some(codec) = codec else {
        return false;
    };
    let codec = codec.to_ascii_lowercase();
    ![
        "pgs",
        "dvdsub",
        "vobsub",
        "dvbsub",
        "dvd_subtitle",
        "dvb_subtitle",
    ]
    .iter()
    .any(|marker| codec.contains(marker))
        && !matches!(codec.as_str(), "sup" | "sub")
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
        is_external: false,
        is_text_subtitle_stream: stream_type == "Subtitle"
            && is_text_subtitle(value["codec_name"].as_str()),
        delivery_method: None,
        delivery_url: None,
    })
}

// Bounded, deterministic read-only local SRT discovery; reject symlinks
// and never enumerate beyond the already authorized video's directory.
#[derive(Debug)]
struct ExternalSubtitle {
    index: i64,
    filename: String,
    language: Option<String>,
    format: &'static str,
    forced: bool,
    hearing_impaired: bool,
    default: bool,
}

fn external_subtitles(source: &Path, report: &probe::ProbeReport) -> Vec<ExternalSubtitle> {
    let Some(dir) = source.parent() else {
        return Vec::new();
    };
    let Some(stem) = source.file_stem().and_then(|name| name.to_str()) else {
        return Vec::new();
    };
    let Some(prefix) = stem.len().checked_add(1) else {
        return Vec::new();
    };
    if prefix > 256 {
        return Vec::new();
    }
    let max_index = report
        .streams
        .iter()
        .filter_map(|stream| number(&stream["index"]))
        .filter(|index| *index >= 0)
        .max()
        .unwrap_or(-1);
    let Some(first_index) = max_index.checked_add(1) else {
        return Vec::new();
    };
    if first_index > i64::from(i32::MAX) - 16 {
        return Vec::new();
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut names = Vec::new();
    for (count, result) in entries.enumerate() {
        if count >= 512 {
            return Vec::new();
        }
        let Ok(entry) = result else {
            continue;
        };
        let Ok(filename) = entry.file_name().into_string() else {
            continue;
        };
        let Some(suffix) = filename
            .strip_prefix(stem)
            .and_then(|rest| rest.strip_prefix('.'))
        else {
            continue;
        };
        let suffix = suffix.to_ascii_lowercase();
        let tokens = suffix.split('.').collect::<Vec<_>>();
        if tokens.is_empty() || tokens.len() > 5 {
            continue;
        }
        let format = match tokens.last().copied() {
            Some("srt") => "srt",
            Some("vtt") => "vtt",
            _ => continue,
        };
        let mut language = None;
        let mut forced = false;
        let mut hearing_impaired = false;
        let mut default = false;
        let mut valid = true;
        for token in &tokens[..tokens.len() - 1] {
            match *token {
                "forced" if !forced => forced = true,
                "sdh" | "hi" if !hearing_impaired => hearing_impaired = true,
                "default" if !default => default = true,
                part if language.is_none()
                    && (2..=3).contains(&part.len())
                    && part.bytes().all(|byte| byte.is_ascii_alphabetic()) =>
                {
                    language = Some(part.to_owned());
                }
                _ => {
                    valid = false;
                    break;
                }
            }
        }
        if !valid {
            continue;
        }
        if entry.file_type().is_ok_and(|kind| kind.is_file())
            && entry
                .metadata()
                .is_ok_and(|meta| meta.len() <= 4 * 1024 * 1024)
        {
            names.push((
                filename,
                language,
                format,
                forced,
                hearing_impaired,
                default,
            ));
        }
    }
    names.sort_unstable_by(|a, b| a.0.cmp(&b.0));
    names
        .into_iter()
        .take(16)
        .enumerate()
        .map(
            |(offset, (filename, language, format, forced, hearing_impaired, default))| {
                ExternalSubtitle {
                    index: first_index + offset as i64,
                    filename,
                    language,
                    format,
                    forced,
                    hearing_impaired,
                    default,
                }
            },
        )
        .collect()
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
    let mut media_streams: Vec<MediaStream> =
        result.streams.iter().filter_map(mapped_stream).collect();
    if kind == "video" {
        // Only codecs supported by the bounded FFmpeg text extractor receive
        // actionable URLs. Do not imply that graphical or unknown subtitles
        // have a delivery path, and preserve their embedded source identity.
        for stream in &mut media_streams {
            if stream.stream_type == "Subtitle"
                && stream.is_text_subtitle_stream
                && matches!(
                    stream.codec.as_deref(),
                    Some("subrip" | "ass" | "ssa" | "mov_text" | "webvtt")
                )
                && (0..=i64::from(i32::MAX)).contains(&stream.index)
            {
                stream.delivery_method = Some("External");
                stream.delivery_url = Some(format!(
                    "/Videos/{item_id}/{item_id}/Subtitles/{}/Stream.srt",
                    stream.index
                ));
            }
        }
        for sidecar in external_subtitles(file, result) {
            let language = sidecar.language.as_deref();
            let codec = if sidecar.format == "vtt" {
                "webvtt"
            } else {
                "subrip"
            };
            let mut stream = mapped_stream(&serde_json::json!({
                "index": sidecar.index,
                "codec_type": "subtitle",
                "codec_name": codec,
                "tags": {"language": language},
                "disposition": {
                    "forced": i32::from(sidecar.forced),
                    "hearing_impaired": i32::from(sidecar.hearing_impaired),
                    "default": i32::from(sidecar.default)
                }
            }))
            .expect("local sidecar subtitle has a supported type and index");
            stream.is_external = true;
            stream.is_text_subtitle_stream = true;
            stream.delivery_method = Some("External");
            stream.delivery_url = Some(format!(
                "/Videos/{item_id}/{item_id}/Subtitles/{}/Stream.{}",
                sidecar.index, sidecar.format
            ));
            media_streams.push(stream);
        }
    }
    let mut audio_tracks = media_streams
        .iter()
        .filter(|stream| stream.stream_type == "Audio");
    let first_audio = audio_tracks.next();
    let default_audio_stream_index = media_streams
        .iter()
        .find(|stream| stream.stream_type == "Audio" && stream.is_default)
        .or_else(|| {
            audio_tracks
                .next()
                .is_none()
                .then_some(first_audio)
                .flatten()
        })
        .map(|stream| stream.index)
        .filter(|index| i32::try_from(*index).is_ok());
    MediaSource {
        protocol: "File",
        id: item_id.to_owned(),
        source_type: "Default",
        container,
        size: result.size_bytes,
        name,
        is_remote: false,
        run_time_ticks,
        bitrate: measured_bitrate(result).filter(|rate| i32::try_from(*rate).is_ok()),
        video_type: (kind == "video").then_some("VideoFile"),
        default_audio_stream_index,
        default_subtitle_stream_index: None,
        supports_direct_play: true,
        supports_direct_stream: false,
        supports_transcoding: false,
        media_streams,
        required_http_headers: Default::default(),
        direct_stream_url: format!(
            "/{}/{item_id}/stream?static=true",
            if kind == "audio" { "Audio" } else { "Videos" }
        ),
    }
}

/// Exact FFprobe report bitrate, preferring format total and otherwise
/// summing all positive audio/video stream bitrates only when each is known.
fn measured_bitrate(report: &probe::ProbeReport) -> Option<i64> {
    report
        .format
        .as_ref()
        .and_then(|format| number(&format["bit_rate"]))
        .filter(|rate| *rate > 0)
        .or_else(|| {
            let mut total = 0_i64;
            for stream in &report.streams {
                if !matches!(stream["codec_type"].as_str(), Some("audio" | "video")) {
                    continue;
                }
                total = total.checked_add(number(&stream["bit_rate"]).filter(|n| *n > 0)?)?;
            }
            (total > 0).then_some(total)
        })
}

/// Matches Jellyfin v12.3 DirectPlayProfile Container, AudioCodec, VideoCodec
/// and Type. Empty lists mean any codec; a leading "-" denies listed values.
fn profile_contains(list: &str, actual: &str) -> bool {
    if list.is_empty() {
        return true;
    }
    let (negative, allowed) = match list.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, list),
    };
    let found = allowed
        .split(',')
        .any(|candidate| !candidate.is_empty() && candidate.eq_ignore_ascii_case(actual));
    if negative { !found } else { found }
}

/// Only advertise an external sidecar through a requested subtitle
/// capability when the pinned SubtitleProfile explicitly permits External
/// delivery for the actual native file and language. No conversion is implied.
fn profile_allows_external_subtitles(
    profiles: &[Value],
    source: &MediaSource,
) -> Result<bool, StatusCode> {
    for rule in profiles {
        let properties = rule.as_object().ok_or(StatusCode::BAD_REQUEST)?;
        if properties.keys().any(|key| {
            !["Format", "Method", "Language", "Container", "DidlMode"].contains(&key.as_str())
        }) {
            return Err(StatusCode::BAD_REQUEST);
        }
        for field in ["Format", "Language", "Container", "DidlMode"] {
            if let Some(value) = properties.get(field).filter(|value| !value.is_null())
                && value
                    .as_str()
                    .is_none_or(|text| text.len() > 512 || text.chars().any(char::is_control))
            {
                return Err(StatusCode::BAD_REQUEST);
            }
        }
        if let Some(method) = properties.get("Method") {
            match method {
                Value::String(name)
                    if matches!(
                        name.as_str(),
                        "Encode" | "Embed" | "External" | "Hls" | "Drop"
                    ) => {}
                Value::Number(number) if (0..=4).contains(&number.as_i64().unwrap_or(-1)) => {}
                _ => return Err(StatusCode::BAD_REQUEST),
            }
        }
    }
    for stream in source
        .media_streams
        .iter()
        .filter(|stream| stream.stream_type == "Subtitle")
    {
        let Some(url) = stream
            .delivery_url
            .as_deref()
            .filter(|_| stream.delivery_method == Some("External"))
        else {
            return Ok(false);
        };
        let ext = if url.ends_with(".srt") {
            "srt"
        } else if url.ends_with(".vtt") {
            "vtt"
        } else {
            return Ok(false);
        };
        let matched = profiles.iter().any(|rule| {
            let method = &rule["Method"];
            let external = method == "External" || method == 2;
            if !external {
                return false;
            }
            let allowed_container = rule["Container"].as_str().unwrap_or("");
            let allowed_language = rule["Language"].as_str().unwrap_or("");
            let allowed_format = rule["Format"].as_str().unwrap_or("");
            let codec = stream.codec.as_deref().unwrap_or("");
            let compatible_format = if allowed_format.starts_with('-') {
                profile_contains(allowed_format, ext) && profile_contains(allowed_format, codec)
            } else {
                profile_contains(allowed_format, ext) || profile_contains(allowed_format, codec)
            };
            compatible_format
                && profile_contains(allowed_container, &source.container)
                && profile_contains(
                    allowed_language,
                    stream.language.as_deref().unwrap_or("und"),
                )
        });
        if !matched {
            return Ok(false);
        }
    }
    Ok(true)
}

/// This is a conservative, bounded direct-file subset, not full DLNA
/// negotiation. A client profile which requires unimplemented codec or
/// container constraints is refused rather than silently ignored.
fn profile_allows_direct_file(
    profile: &Value,
    source: &MediaSource,
    report: &probe::ProbeReport,
) -> Result<bool, StatusCode> {
    if serde_json::to_vec(profile)
        .map_err(|_| StatusCode::BAD_REQUEST)?
        .len()
        > 65_536
    {
        return Err(StatusCode::BAD_REQUEST);
    }
    let object = profile.as_object().ok_or(StatusCode::BAD_REQUEST)?;
    const ROOT_KEYS: &[&str] = &[
        "Name",
        "Id",
        "MaxStreamingBitrate",
        "MaxStaticBitrate",
        "MusicStreamingTranscodingBitrate",
        "MaxStaticMusicBitrate",
        "DirectPlayProfiles",
        "TranscodingProfiles",
        "ContainerProfiles",
        "CodecProfiles",
        "SubtitleProfiles",
    ];
    if object.keys().any(|key| !ROOT_KEYS.contains(&key.as_str())) {
        return Err(StatusCode::BAD_REQUEST);
    }
    for field in ["CodecProfiles", "ContainerProfiles"] {
        if let Some(value) = object.get(field).filter(|v| !v.is_null()) {
            let profiles = value.as_array().ok_or(StatusCode::BAD_REQUEST)?;
            if !profiles.is_empty() {
                return Err(StatusCode::NOT_IMPLEMENTED);
            }
        }
    }
    for field in ["TranscodingProfiles", "SubtitleProfiles"] {
        if let Some(value) = object.get(field).filter(|v| !v.is_null()) {
            let profiles = value.as_array().ok_or(StatusCode::BAD_REQUEST)?;
            if profiles.len() > 64 {
                return Err(StatusCode::BAD_REQUEST);
            }
            if field == "SubtitleProfiles"
                && !profiles.is_empty()
                && !profile_allows_external_subtitles(profiles, source)?
            {
                return Ok(false);
            }
        }
    }
    // A static direct file must fit an explicitly supplied static bitrate
    // ceiling. With no reliable measured bitrate, decline to claim support.
    let limit_field = if source
        .media_streams
        .iter()
        .any(|s| s.stream_type == "Video")
    {
        "MaxStaticBitrate"
    } else {
        "MaxStaticMusicBitrate"
    };
    if let Some(limit) = object.get(limit_field).filter(|value| !value.is_null()) {
        let limit = limit
            .as_i64()
            .filter(|value| *value > 0)
            .ok_or(StatusCode::BAD_REQUEST)?;
        if measured_bitrate(report).is_none_or(|bitrate| bitrate > limit) {
            return Ok(false);
        }
    }
    let Some(raw_rules) = object.get("DirectPlayProfiles") else {
        // Upstream DeviceProfile defaults this array to empty. A profile
        // without any direct-play rules is valid but cannot use static files.
        return Ok(false);
    };
    let rules = raw_rules.as_array().ok_or(StatusCode::BAD_REQUEST)?;
    if rules.len() > 64 {
        return Err(StatusCode::BAD_REQUEST);
    }
    let has_video = source
        .media_streams
        .iter()
        .any(|s| s.stream_type == "Video");
    let mut matched = false;
    for rule in rules {
        let properties = rule.as_object().ok_or(StatusCode::BAD_REQUEST)?;
        if properties
            .keys()
            .any(|key| !["Type", "Container", "AudioCodec", "VideoCodec"].contains(&key.as_str()))
        {
            return Err(StatusCode::BAD_REQUEST);
        }
        // DlnaProfileType defaults to Audio (0). JSON enum values can be
        // written as names or their pinned integer discriminants.
        let kind = match properties.get("Type") {
            None => "Audio",
            Some(Value::String(s)) => s.as_str(),
            Some(Value::Number(n)) => match n.as_i64() {
                Some(0) => "Audio",
                Some(1) => "Video",
                Some(2) => "Photo",
                Some(3) => "Subtitle",
                Some(4) => "Lyric",
                _ => return Err(StatusCode::BAD_REQUEST),
            },
            _ => return Err(StatusCode::BAD_REQUEST),
        };
        if !matches!(kind, "Video" | "Audio" | "Photo" | "Subtitle" | "Lyric") {
            return Err(StatusCode::BAD_REQUEST);
        }
        // DirectPlayProfile.Container is initialized to an empty wildcard.
        let container = properties
            .get("Container")
            .map(|value| value.as_str().ok_or(StatusCode::BAD_REQUEST))
            .transpose()?
            .unwrap_or("");
        let audio = properties
            .get("AudioCodec")
            .filter(|v| !v.is_null())
            .map(|v| v.as_str().ok_or(StatusCode::BAD_REQUEST))
            .transpose()?;
        let video = properties
            .get("VideoCodec")
            .filter(|v| !v.is_null())
            .map(|v| v.as_str().ok_or(StatusCode::BAD_REQUEST))
            .transpose()?;
        if [container, audio.unwrap_or(""), video.unwrap_or("")]
            .iter()
            .any(|value| value.len() > 512 || value.chars().any(char::is_control))
        {
            return Err(StatusCode::BAD_REQUEST);
        }
        if kind != if has_video { "Video" } else { "Audio" }
            || !profile_contains(container, &source.container)
        {
            continue;
        }
        let compatible = source
            .media_streams
            .iter()
            .filter(|stream| matches!(stream.stream_type.as_str(), "Audio" | "Video"))
            .all(|stream| {
                let matcher = if stream.stream_type == "Video" {
                    video.unwrap_or("")
                } else {
                    audio.unwrap_or("")
                };
                stream
                    .codec
                    .as_deref()
                    .is_some_and(|codec| profile_contains(matcher, codec))
            });
        if compatible {
            matched = true;
        }
    }
    Ok(matched)
}

/// A selected track can be honored without remuxing only when it is
/// already the sole track of its kind. A disabled subtitle selection is a
/// no-op only when no embedded subtitles are present. All other selections
/// require per-track playback negotiation that static file serving lacks.
fn stream_selection_is_noop(
    report: &probe::ProbeReport,
    audio_stream_index: Option<i64>,
    subtitle_stream_index: Option<i64>,
) -> bool {
    if let Some(index) = audio_stream_index {
        let mut audio_streams = report
            .streams
            .iter()
            .filter(|stream| stream["codec_type"] == "audio");
        if audio_streams
            .next()
            .and_then(|stream| number(&stream["index"]))
            != Some(index)
            || audio_streams.next().is_some()
        {
            return false;
        }
    }
    if let Some(index) = subtitle_stream_index
        && (index != -1
            || report
                .streams
                .iter()
                .any(|stream| stream["codec_type"] == "subtitle"))
    {
        return false;
    }
    true
}

/// Lookup only authorized, indexed audio/video and resolve them within the root.
pub(crate) async fn media_file(
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
    let (user, token) = auth_api::authenticated(state.clone(), headers.clone()).await?;
    if req.user_id.as_deref().is_some_and(|id| id != user.id) {
        return Err(StatusCode::FORBIDDEN);
    }
    if req.start_time_ticks.is_some_and(|ticks| ticks < 0) {
        return Err(StatusCode::BAD_REQUEST);
    }
    if req.start_time_ticks.is_some_and(|ticks| ticks > 0)
        || req.auto_open_live_stream == Some(true)
        || req
            .media_source_id
            .as_deref()
            .is_some_and(|id| id != item_id)
    {
        // No timestamp seeks or live media sessions yet.
        return Err(StatusCode::NOT_IMPLEMENTED);
    }
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
    if !stream_selection_is_noop(&probed, req.audio_stream_index, None) {
        return Err(StatusCode::NOT_IMPLEMENTED);
    }
    // Without an FFmpeg/remux fallback, never claim a static source can fit
    // requested stream bandwidth or output-channel limits when it cannot.
    if let Some(limit) = req.max_audio_channels {
        if limit <= 0 {
            return Err(StatusCode::BAD_REQUEST);
        }
        if probed
            .streams
            .iter()
            .filter(|stream| stream["codec_type"] == "audio")
            .any(|stream| number(&stream["channels"]).is_none_or(|channels| channels > limit))
        {
            return Err(StatusCode::NOT_IMPLEMENTED);
        }
    }
    // Read-only sidecar directory enumeration can touch hundreds of metadata
    // entries. Keep it off Tokio's I/O worker threads.
    let (mut source, probed) = tokio::task::spawn_blocking(move || {
        let source = source_from_probe(&item_id, kind, &file, &probed);
        (source, probed)
    })
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    if let Some(index) = req.subtitle_stream_index {
        if index == -1 {
            // No embedded subtitle can be stripped from direct-play bytes.
            if probed
                .streams
                .iter()
                .any(|stream| stream["codec_type"] == "subtitle")
            {
                return Err(StatusCode::NOT_IMPLEMENTED);
            }
        } else if let Some(selected) = source.media_streams.iter().find(|stream| {
            stream.index == index
                && stream.stream_type == "Subtitle"
                && stream.delivery_method == Some("External")
                && stream.delivery_url.is_some()
        }) {
            // A separately deliverable text track needs no rewrite of the
            // video file. Advertise the selected stream to the client.
            source.default_subtitle_stream_index = Some(selected.index);
        } else {
            return Err(StatusCode::NOT_IMPLEMENTED);
        }
    }
    // An explicit playback profile overrides the token's previously reported
    // capabilities. Session profiles are never shared across users or tokens.
    let profile = match req.device_profile {
        Some(profile) => Some(profile),
        None => {
            let dir = state.auth_dir().ok_or(StatusCode::SERVICE_UNAVAILABLE)?;
            tokio::task::spawn_blocking(move || auth::session_device_profile(&dir, &token))
                .await
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        }
    };
    // Jellyfin's posted PlaybackInfo uses an explicit MaxStreamingBitrate
    // first, then the effective DeviceProfile's MaxStreamingBitrate. A
    // caller-supplied query value has already overridden the POST body.
    // Neither a measured over-budget static file nor unknown source bitrate
    // can be offered without a transcoding fallback.
    if req.max_streaming_bitrate.is_some_and(|limit| limit <= 0) {
        return Err(StatusCode::BAD_REQUEST);
    }
    let profile_bitrate = if req.max_streaming_bitrate.is_none() {
        profile
            .as_ref()
            .and_then(|profile| profile.get("MaxStreamingBitrate"))
            .filter(|value| !value.is_null())
            .map(|value| {
                value
                    .as_i64()
                    .filter(|bitrate| *bitrate > 0)
                    .ok_or(StatusCode::BAD_REQUEST)
            })
            .transpose()?
    } else {
        None
    };
    if let Some(limit) = req.max_streaming_bitrate.or(profile_bitrate)
        && measured_bitrate(&probed).is_none_or(|bitrate| bitrate > limit)
    {
        return Err(StatusCode::NOT_IMPLEMENTED);
    }
    if let Some(profile) = profile.as_ref() {
        if !profile_allows_direct_file(profile, &source, &probed)? {
            // There is no remux or transcode fallback yet. Refuse the
            // unmatched profile rather than advertising unsupported playback.
            return Err(StatusCode::NOT_IMPLEMENTED);
        }
    }
    Ok(Json(PlaybackInfo {
        media_sources: vec![source],
        play_session_id: None,
        error_code: None,
    }))
}

async fn get_playback(
    State(state): State<Arc<AppState>>,
    RoutePath(item_id): RoutePath<String>,
    headers: HeaderMap,
    Query(query): Query<PlaybackQuery>,
) -> Result<Json<PlaybackInfo>, StatusCode> {
    playback_info(
        state,
        item_id,
        headers,
        query.apply(PlaybackRequest::default()),
    )
    .await
}

async fn post_playback(
    State(state): State<Arc<AppState>>,
    RoutePath(item_id): RoutePath<String>,
    headers: HeaderMap,
    Query(query): Query<PlaybackQuery>,
    Json(body): Json<PlaybackRequest>,
) -> Result<Json<PlaybackInfo>, StatusCode> {
    playback_info(state, item_id, headers, query.apply(body)).await
}

// Narrow UTF-8 SRT -> WebVTT normalization for ordinary numbered/unnumbered
// cues. Refuse advanced timing syntax rather than returning malformed VTT.
fn srt_timestamp(value: &str) -> Option<String> {
    let (time, milliseconds) = value.split_once(',')?;
    let mut fields = time.split(':');
    let (hours, minutes, seconds) = (fields.next()?, fields.next()?, fields.next()?);
    if fields.next().is_some()
        || !(2..=3).contains(&hours.len())
        || minutes.len() != 2
        || seconds.len() != 2
        || milliseconds.len() != 3
        || ![hours, minutes, seconds, milliseconds]
            .iter()
            .all(|value| value.bytes().all(|byte| byte.is_ascii_digit()))
        || minutes.parse::<u8>().ok()? > 59
        || seconds.parse::<u8>().ok()? > 59
    {
        return None;
    }
    Some(format!("{hours}:{minutes}:{seconds}.{milliseconds}"))
}

fn convert_srt_to_webvtt(source: &[u8]) -> Option<Vec<u8>> {
    let input = std::str::from_utf8(source)
        .ok()?
        .trim_start_matches('\u{feff}');
    let normalized = input.replace("\r\n", "\n").replace('\r', "\n");
    let mut output = String::from("WEBVTT\n\n");
    let mut count = 0_u32;
    for block in normalized
        .split("\n\n")
        .filter(|part| !part.trim().is_empty())
    {
        let mut lines = block.lines();
        let first = lines.next()?.trim();
        let cue = if first.bytes().all(|byte| byte.is_ascii_digit()) && !first.is_empty() {
            lines.next()?.trim()
        } else {
            first
        };
        let (begin, end) = cue.split_once(" --> ")?;
        let start = srt_timestamp(begin.trim())?;
        let finish = srt_timestamp(end.trim())?;
        let text = lines.collect::<Vec<_>>().join("\n");
        if text.trim().is_empty() {
            return None;
        }
        output.push_str(&start);
        output.push_str(" --> ");
        output.push_str(&finish);
        output.push('\n');
        output.push_str(text.trim_end());
        output.push_str("\n\n");
        count += 1;
        if output.len() > 4 * 1024 * 1024 + 4096 || count > 100_000 {
            return None;
        }
    }
    (count > 0).then(|| output.into_bytes())
}

// Bound on-demand embedded subtitle extraction without a persistent process.
static SUBTITLE_WORKERS: OnceLock<Arc<Semaphore>> = OnceLock::new();

async fn extract_embedded_srt(
    executable: &Path,
    source: &Path,
    index: i64,
) -> Result<Vec<u8>, StatusCode> {
    let workers = SUBTITLE_WORKERS
        .get_or_init(|| Arc::new(Semaphore::new(2)))
        .clone();
    let _permit = workers
        .try_acquire_owned()
        .map_err(|_| StatusCode::TOO_MANY_REQUESTS)?;
    let mut child = Command::new(executable)
        .arg("-nostdin")
        .arg("-hide_banner")
        .arg("-loglevel")
        .arg("error")
        .arg("-i")
        .arg(source)
        .arg("-map")
        .arg(format!("0:{index}"))
        .arg("-c:s")
        .arg("srt")
        .arg("-f")
        .arg("srt")
        .arg("pipe:1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    tokio::time::timeout(Duration::from_secs(30), async {
        let stdout = child
            .stdout
            .take()
            .ok_or(StatusCode::INTERNAL_SERVER_ERROR)?;
        let mut payload = Vec::new();
        stdout
            .take(4 * 1024 * 1024 + 1)
            .read_to_end(&mut payload)
            .await
            .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
        if payload.len() > 4 * 1024 * 1024 {
            let _ = child.kill().await;
            return Err(StatusCode::NOT_IMPLEMENTED);
        }
        let status = child
            .wait()
            .await
            .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
        if !status.success() || payload.is_empty() {
            return Err(StatusCode::NOT_IMPLEMENTED);
        }
        Ok(payload)
    })
    .await
    .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?
}

/// Recheck durable token/grant scope and indexed source revision after
/// asynchronous probing, encoding or filesystem work and before delivery.
async fn reauthorize_subtitle_source(
    state: Arc<AppState>,
    headers: HeaderMap,
    item: String,
    source: &Path,
    original: &std::fs::Metadata,
) -> Result<(), StatusCode> {
    let (_, scope) = auth_api::library_scope(state.clone(), headers).await?;
    let (latest, kind) = media_file(&state, item, scope).await?;
    let current = tokio::fs::metadata(&latest)
        .await
        .map_err(|_| StatusCode::NOT_FOUND)?;
    if kind != "video"
        || latest != source
        || current.len() != original.len()
        || current.modified().ok() != original.modified().ok()
    {
        return Err(StatusCode::NOT_FOUND);
    }
    Ok(())
}

// Only identity subtitle requests are supported. Timestamp conversion,
// subtitle transcoding, and graphical subtitles need separate implementations.
async fn serve_external_subtitle(
    state: Arc<AppState>,
    item: String,
    media_source: String,
    index: i64,
    format: &'static str,
    headers: HeaderMap,
    head: bool,
) -> Result<Response, StatusCode> {
    let (_, scope) = auth_api::library_scope(state.clone(), headers.clone()).await?;
    if item != media_source {
        return Err(StatusCode::NOT_FOUND);
    }
    let (source, kind) = media_file(&state, item.clone(), scope).await?;
    if kind != "video" {
        return Err(StatusCode::NOT_FOUND);
    }
    let source_metadata = tokio::fs::metadata(&source)
        .await
        .map_err(|_| StatusCode::NOT_FOUND)?;
    let report = probe::probe(&source, state.probe_executable(), probe::DEFAULT_TIMEOUT)
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    let embedded = report.streams.iter().any(|stream| {
        number(&stream["index"]) == Some(index)
            && stream["codec_type"] == "subtitle"
            && is_text_subtitle(stream["codec_name"].as_str())
    });
    let discovered_source = source.clone();
    let sidecar = tokio::task::spawn_blocking(move || {
        external_subtitles(&discovered_source, &report)
            .into_iter()
            .find(|sidecar| {
                sidecar.index == index
                    && (sidecar.format == format || (sidecar.format == "srt" && format == "vtt"))
            })
    })
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    if sidecar.is_none() {
        if !embedded {
            return Err(StatusCode::NOT_FOUND);
        }
        if !(0..=i64::from(i32::MAX)).contains(&index) {
            return Err(StatusCode::BAD_REQUEST);
        }
        let bytes = extract_embedded_srt(state.ffmpeg_executable(), &source, index).await?;
        let payload = if format == "vtt" {
            convert_srt_to_webvtt(&bytes).ok_or(StatusCode::NOT_IMPLEMENTED)?
        } else {
            bytes
        };
        // Revalidate the original user/session, grants and media revision
        // after FFmpeg completes but before returning extracted bytes.
        reauthorize_subtitle_source(
            state.clone(),
            headers.clone(),
            item.clone(),
            &source,
            &source_metadata,
        )
        .await?;
        let length = payload.len();
        let mut response = Response::new(if head {
            Body::empty()
        } else {
            Body::from(payload)
        });
        response.headers_mut().insert(
            CONTENT_TYPE,
            HeaderValue::from_static(if format == "vtt" {
                "text/vtt; charset=utf-8"
            } else {
                "application/x-subrip"
            }),
        );
        response
            .headers_mut()
            .insert(CACHE_CONTROL, HeaderValue::from_static("private, no-store"));
        response.headers_mut().insert(
            CONTENT_LENGTH,
            HeaderValue::from_str(&length.to_string())
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?,
        );
        return Ok(response);
    }
    let sidecar = sidecar.ok_or(StatusCode::NOT_FOUND)?;
    let convert_to_vtt = sidecar.format == "srt" && format == "vtt";
    let parent = source.parent().ok_or(StatusCode::NOT_FOUND)?.to_path_buf();
    let filename = sidecar.filename;
    let (opened, _, size) = tokio::task::spawn_blocking(move || {
        streaming::open_confined(&parent, Path::new(&filename))
    })
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)??;
    if size > 4 * 1024 * 1024 {
        return Err(StatusCode::NOT_IMPLEMENTED);
    }
    let (body, response_length) = if convert_to_vtt {
        // Compute identical payload length for GET and HEAD; never touch
        // or rewrite the user's adjacent subtitle file.
        let mut bytes = Vec::new();
        tokio::fs::File::from_std(opened)
            .take(4 * 1024 * 1024 + 1)
            .read_to_end(&mut bytes)
            .await
            .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
        if bytes.len() > 4 * 1024 * 1024 {
            return Err(StatusCode::NOT_IMPLEMENTED);
        }
        let converted = convert_srt_to_webvtt(&bytes).ok_or(StatusCode::NOT_IMPLEMENTED)?;
        let length = converted.len() as u64;
        (
            if head {
                Body::empty()
            } else {
                Body::from(converted)
            },
            length,
        )
    } else {
        let body = if head {
            Body::empty()
        } else {
            Body::from_stream(ReaderStream::new(tokio::fs::File::from_std(opened)))
        };
        (body, size)
    };
    // Sidecar decoding/reading can also span grant or source revision
    // changes. Revalidate before releasing even a previously opened file.
    reauthorize_subtitle_source(state.clone(), headers, item, &source, &source_metadata).await?;
    let mut response = Response::new(body);
    response.headers_mut().insert(
        CONTENT_TYPE,
        HeaderValue::from_static(if format == "vtt" {
            "text/vtt; charset=utf-8"
        } else {
            "application/x-subrip"
        }),
    );
    response
        .headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("private, no-store"));
    response.headers_mut().insert(
        CONTENT_LENGTH,
        HeaderValue::from_str(&response_length.to_string())
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?,
    );
    Ok(response)
}

async fn get_external_srt(
    State(state): State<Arc<AppState>>,
    RoutePath((item, media_source, index)): RoutePath<(String, String, i64)>,
    headers: HeaderMap,
) -> Result<Response, StatusCode> {
    serve_external_subtitle(state, item, media_source, index, "srt", headers, false).await
}

async fn head_external_srt(
    State(state): State<Arc<AppState>>,
    RoutePath((item, media_source, index)): RoutePath<(String, String, i64)>,
    headers: HeaderMap,
) -> Result<Response, StatusCode> {
    serve_external_subtitle(state, item, media_source, index, "srt", headers, true).await
}

async fn get_external_vtt(
    State(state): State<Arc<AppState>>,
    RoutePath((item, media_source, index)): RoutePath<(String, String, i64)>,
    headers: HeaderMap,
) -> Result<Response, StatusCode> {
    serve_external_subtitle(state, item, media_source, index, "vtt", headers, false).await
}

async fn head_external_vtt(
    State(state): State<Arc<AppState>>,
    RoutePath((item, media_source, index)): RoutePath<(String, String, i64)>,
    headers: HeaderMap,
) -> Result<Response, StatusCode> {
    serve_external_subtitle(state, item, media_source, index, "vtt", headers, true).await
}

// Pinned Jellyfin v12.3 also exposes timestamp-scoped subtitle URLs.
// Zero ticks is the identity operation; nonzero seek rebasing is not yet
// available in this bounded subtitle delivery subset.
async fn timed_subtitle(
    state: Arc<AppState>,
    (item, media_source, index, start_ticks): (String, String, i64, i64),
    format: &'static str,
    headers: HeaderMap,
    head: bool,
) -> Result<Response, StatusCode> {
    if start_ticks < 0 {
        return Err(StatusCode::BAD_REQUEST);
    }
    if start_ticks != 0 {
        return Err(StatusCode::NOT_IMPLEMENTED);
    }
    serve_external_subtitle(state, item, media_source, index, format, headers, head).await
}

async fn get_timed_srt(
    State(state): State<Arc<AppState>>,
    RoutePath((item, media_source, index, start)): RoutePath<(String, String, i64, i64)>,
    headers: HeaderMap,
) -> Result<Response, StatusCode> {
    timed_subtitle(
        state,
        (item, media_source, index, start),
        "srt",
        headers,
        false,
    )
    .await
}

async fn head_timed_srt(
    State(state): State<Arc<AppState>>,
    RoutePath((item, media_source, index, start)): RoutePath<(String, String, i64, i64)>,
    headers: HeaderMap,
) -> Result<Response, StatusCode> {
    timed_subtitle(
        state,
        (item, media_source, index, start),
        "srt",
        headers,
        true,
    )
    .await
}

async fn get_timed_vtt(
    State(state): State<Arc<AppState>>,
    RoutePath((item, media_source, index, start)): RoutePath<(String, String, i64, i64)>,
    headers: HeaderMap,
) -> Result<Response, StatusCode> {
    timed_subtitle(
        state,
        (item, media_source, index, start),
        "vtt",
        headers,
        false,
    )
    .await
}

async fn head_timed_vtt(
    State(state): State<Arc<AppState>>,
    RoutePath((item, media_source, index, start)): RoutePath<(String, String, i64, i64)>,
    headers: HeaderMap,
) -> Result<Response, StatusCode> {
    timed_subtitle(
        state,
        (item, media_source, index, start),
        "vtt",
        headers,
        true,
    )
    .await
}

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route(
            "/Items/{item_id}/PlaybackInfo",
            get(get_playback).post(post_playback),
        )
        .route(
            "/Videos/{item_id}/{media_source_id}/Subtitles/{subtitle_index}/Stream.srt",
            get(get_external_srt).head(head_external_srt),
        )
        .route(
            "/Videos/{item_id}/{media_source_id}/Subtitles/{subtitle_index}/Stream.vtt",
            get(get_external_vtt).head(head_external_vtt),
        )
        .route(
            "/Videos/{item_id}/{media_source_id}/Subtitles/{subtitle_index}/{start_time_ticks}/Stream.srt",
            get(get_timed_srt).head(head_timed_srt),
        )
        .route(
            "/Videos/{item_id}/{media_source_id}/Subtitles/{subtitle_index}/{start_time_ticks}/Stream.vtt",
            get(get_timed_vtt).head(head_timed_vtt),
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn legacy_playback_query_overrides_posted_options() {
        let body: PlaybackRequest = serde_json::from_value(json!({
            "UserId": "u1", "EnableDirectPlay": false,
            "StartTimeTicks": 100,
            "MediaSourceId": "source-a",
            "MaxAudioChannels": 1,
            "MaxStreamingBitrate": 2_000_000
        }))
        .unwrap();
        let query: PlaybackQuery = serde_json::from_value(json!({
            "userId": "u2", "enableDirectPlay": true,
            "startTimeTicks": 0,
            "maxAudioChannels": 2,
            "maxStreamingBitrate": 4_000_000
        }))
        .unwrap();
        let merged = query.apply(body);
        assert_eq!(merged.user_id.as_deref(), Some("u2"));
        assert_eq!(merged.enable_direct_play, Some(true));
        assert_eq!(merged.start_time_ticks, Some(0));
        assert_eq!(merged.media_source_id.as_deref(), Some("source-a"));
        assert_eq!(merged.max_audio_channels, Some(2));
        assert_eq!(merged.max_streaming_bitrate, Some(4_000_000));
    }

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
    fn single_track_selection_only_allows_semantic_noops() {
        let mut report = probe::ProbeReport {
            path: PathBuf::from("movie.mp4"),
            size_bytes: 128,
            format: None,
            streams: vec![
                json!({"index":0,"codec_type":"video","codec_name":"h264"}),
                json!({"index":1,"codec_type":"audio","codec_name":"aac"}),
            ],
            chapters: vec![],
        };
        assert!(stream_selection_is_noop(&report, Some(1), Some(-1)));
        assert!(stream_selection_is_noop(&report, None, None));
        assert!(!stream_selection_is_noop(&report, Some(0), None));
        assert!(!stream_selection_is_noop(&report, Some(-1), None));
        assert!(!stream_selection_is_noop(&report, None, Some(0)));
        report
            .streams
            .push(json!({"index":2,"codec_type":"audio","codec_name":"ac3"}));
        assert!(!stream_selection_is_noop(&report, Some(1), None));
        report.streams.pop();
        report
            .streams
            .push(json!({"index":2,"codec_type":"subtitle","codec_name":"subrip"}));
        assert!(!stream_selection_is_noop(&report, None, Some(-1)));
    }

    #[test]
    fn pinned_direct_play_profiles_filter_container_codecs_and_static_bitrate() {
        let probe = probe::ProbeReport {
            path: PathBuf::from("feature.mp4"),
            size_bytes: 42,
            format: Some(json!({"bit_rate":"4000000"})),
            streams: vec![
                json!({"index":0,"codec_type":"video","codec_name":"h264","bit_rate":"3500000"}),
                json!({"index":1,"codec_type":"audio","codec_name":"aac","bit_rate":"500000"}),
            ],
            chapters: vec![],
        };
        let source = source_from_probe("movie", "video", Path::new("feature.mp4"), &probe);
        let valid = json!({
            "DirectPlayProfiles":[
                {"Type":"Video","Container":"mp4,webm","VideoCodec":"h264,hevc","AudioCodec":"aac"}
            ]
        });
        assert_eq!(
            profile_allows_direct_file(&valid, &source, &probe),
            Ok(true)
        );
        assert_eq!(
            profile_allows_direct_file(
                &json!({"DirectPlayProfiles":[{"Type":1,"Container":"mp4"}]}),
                &source,
                &probe,
            ),
            Ok(true),
        );
        assert_eq!(
            profile_allows_direct_file(&json!({}), &source, &probe),
            Ok(false)
        );
        assert_eq!(
            profile_allows_direct_file(
                &json!({
                    "DirectPlayProfiles": [
                        {"Type":"Video","Container":"mp4"},
                        {"Type":"Video","Container":"mp4","Unexpected":true}
                    ]
                }),
                &source,
                &probe,
            ),
            Err(StatusCode::BAD_REQUEST),
        );
        for unsupported in [
            json!({"DirectPlayProfiles":[{"Type":"Video","Container":"mkv"}]}),
            json!({"DirectPlayProfiles":[{"Type":"Audio","Container":"mp4"}]}),
            json!({"DirectPlayProfiles":[{"Type":"Video","Container":"mp4","VideoCodec":"hevc"}]}),
            json!({"DirectPlayProfiles":[{"Type":"Video","Container":"mp4","AudioCodec":"flac"}]}),
            json!({"DirectPlayProfiles":[{"Type":"Video","Container":"mp4"}],"MaxStaticBitrate":3_000_000}),
            json!({"DirectPlayProfiles":[]}),
        ] {
            assert_eq!(
                profile_allows_direct_file(&unsupported, &source, &probe),
                Ok(false)
            );
        }
        let allowed_negative = json!({
            "DirectPlayProfiles":[{"Type":"Video","Container":"-mkv","VideoCodec":"-hevc"}]
        });
        assert_eq!(
            profile_allows_direct_file(&allowed_negative, &source, &probe),
            Ok(true)
        );
        let rejected_negative = json!({
            "DirectPlayProfiles":[{"Type":"Video","Container":"-mp4"}]
        });
        assert_eq!(
            profile_allows_direct_file(&rejected_negative, &source, &probe),
            Ok(false)
        );
        let static_limit = json!({
            "DirectPlayProfiles":[{"Type":"Video","Container":"mp4"}],
            "MaxStaticBitrate":4_000_000
        });
        assert_eq!(
            profile_allows_direct_file(&static_limit, &source, &probe),
            Ok(true)
        );
        for invalid in [
            json!({"DirectPlayProfiles":{} }),
            json!({"DirectPlayProfiles":[{"Type":"Unknown","Container":"mp4"}]}),
            json!({"DirectPlayProfiles":[{"Type":"Video","Container":"mp4","Unknown":true}]}),
            json!({"DirectPlayProfiles":[{"Type":"Video","Container":"mp4"}],"MaxStaticBitrate":-1}),
        ] {
            assert_eq!(
                profile_allows_direct_file(&invalid, &source, &probe),
                Err(StatusCode::BAD_REQUEST)
            );
        }
        assert_eq!(
            profile_allows_direct_file(
                &json!({"DirectPlayProfiles":[{"Type":"Video","Container":"mp4"}],"CodecProfiles":[{"Type":"Video"}]}),
                &source,
                &probe,
            ),
            Err(StatusCode::NOT_IMPLEMENTED)
        );
        assert!(profile_contains("", "anything"));
        assert!(profile_contains("h264,hevc", "H264"));
        assert!(!profile_contains("-h264,hevc", "h264"));
        assert!(profile_contains("-h264,hevc", "flac"));
    }

    #[test]
    fn media_source_track_defaults_are_never_ambiguous() {
        let mut report = probe::ProbeReport {
            path: PathBuf::from("movie.mp4"),
            size_bytes: 42,
            format: Some(json!({"bit_rate":"2000000"})),
            streams: vec![
                json!({"index":0,"codec_type":"video","codec_name":"h264"}),
                json!({"index":1,"codec_type":"audio","codec_name":"aac"}),
                json!({"index":2,"codec_type":"audio","codec_name":"ac3"}),
            ],
            chapters: vec![],
        };
        let source = source_from_probe("id", "video", Path::new("movie.mp4"), &report);
        assert_eq!(source.default_audio_stream_index, None);
        assert_eq!(source.bitrate, Some(2_000_000));
        assert_eq!(source.video_type, Some("VideoFile"));
        report.streams[2]["disposition"] = json!({"default":1});
        let selected = source_from_probe("id", "video", Path::new("movie.mp4"), &report);
        assert_eq!(selected.default_audio_stream_index, Some(2));
        report.format = None;
        let unknown = source_from_probe("id", "video", Path::new("movie.mp4"), &report);
        assert_eq!(unknown.bitrate, None);
    }

    #[tokio::test]
    async fn subtitle_source_revalidation_rejects_revoked_grants_and_revisions() {
        use axum::http::HeaderValue;
        let fixture = tempfile::tempdir().unwrap();
        let library = fixture.path().join("Movies");
        let state_dir = fixture.path().join("state");
        std::fs::create_dir_all(&library).unwrap();
        let movie = library.join("Feature.mp4");
        std::fs::write(&movie, b"video contents").unwrap();
        catalog::index(&state_dir, &library).unwrap();
        auth::create_user(&state_dir, "admin", "subtitle-revalidation-admin-password").unwrap();
        auth::create_user(
            &state_dir,
            "viewer",
            "subtitle-revalidation-viewer-password",
        )
        .unwrap();
        auth::grant_library(&state_dir, "viewer", &library).unwrap();
        let token = auth::authenticate(
            &state_dir,
            "viewer",
            "subtitle-revalidation-viewer-password",
            "server",
        )
        .unwrap()
        .unwrap()
        .access_token;
        let mut headers = HeaderMap::new();
        headers.insert("x-emby-token", HeaderValue::from_str(&token).unwrap());
        let state = Arc::new(AppState::with_auth_dir(
            "server".to_owned(),
            state_dir.clone(),
        ));
        let item = catalog_api::stable_id(library.to_str().unwrap(), "Feature.mp4");
        let original = std::fs::metadata(&movie).unwrap();
        let source = std::fs::canonicalize(&movie).unwrap();
        assert_eq!(
            reauthorize_subtitle_source(
                state.clone(),
                headers.clone(),
                item.clone(),
                &source,
                &original,
            )
            .await,
            Ok(())
        );
        auth::revoke_library(&state_dir, "viewer", &library).unwrap();
        assert_eq!(
            reauthorize_subtitle_source(
                state.clone(),
                headers.clone(),
                item.clone(),
                &source,
                &original,
            )
            .await,
            Err(StatusCode::NOT_FOUND)
        );
        auth::grant_library(&state_dir, "viewer", &library).unwrap();
        std::fs::write(&movie, b"updated media revision").unwrap();
        assert_eq!(
            reauthorize_subtitle_source(state, headers, item, &source, &original,).await,
            Err(StatusCode::NOT_FOUND)
        );
    }

    #[test]
    fn bounded_srt_to_webvtt_preserves_cues_and_rejects_bad_timestamps() {
        let input = b"\xEF\xBB\xBF1\r\n00:00:01,000 --> 00:00:02,500\r\nHello!\r\n\r\n2\r\n00:01:00,050 --> 00:01:01,000\r\nBye\r\n";
        let converted = convert_srt_to_webvtt(input).unwrap();
        assert_eq!(
            std::str::from_utf8(&converted).unwrap(),
            "WEBVTT\n\n00:00:01.000 --> 00:00:02.500\nHello!\n\n00:01:00.050 --> 00:01:01.000\nBye\n\n"
        );
        assert!(convert_srt_to_webvtt(b"1\n00:00:70,000 --> 00:00:02,000\ninvalid\n").is_none());
        assert!(convert_srt_to_webvtt(b"not a timestamp\ninvalid\n").is_none());
        assert!(convert_srt_to_webvtt(&[0xFF, 0xFF]).is_none());
    }

    #[test]
    fn only_convertible_embedded_subtitles_advertise_delivery() {
        let fixture = tempfile::tempdir().unwrap();
        let movie = fixture.path().join("Feature.mp4");
        std::fs::write(&movie, b"movie").unwrap();
        let report = probe::ProbeReport {
            path: movie.clone(),
            size_bytes: 5,
            format: None,
            streams: vec![
                json!({"index":0,"codec_type":"video","codec_name":"h264"}),
                json!({"index":1,"codec_type":"subtitle","codec_name":"mov_text"}),
                json!({"index":2,"codec_type":"subtitle","codec_name":"hdmv_pgs_subtitle"}),
                json!({"index":3,"codec_type":"subtitle","codec_name":"unknown_text_codec"}),
            ],
            chapters: vec![],
        };
        let source = source_from_probe("item", "video", &movie, &report);
        let text = &source.media_streams[1];
        assert_eq!(text.delivery_method, Some("External"));
        assert!(!text.is_external);
        assert_eq!(
            text.delivery_url.as_deref(),
            Some("/Videos/item/item/Subtitles/1/Stream.srt")
        );
        assert_eq!(source.media_streams[2].delivery_url, None);
        assert_eq!(source.media_streams[3].delivery_url, None);
        assert_eq!(
            profile_allows_external_subtitles(
                &[json!({"Format":"srt","Method":"External"})],
                &source
            ),
            Ok(false)
        );
    }

    #[test]
    fn external_subtitle_filename_flags_are_bounded_and_explicit() {
        let fixture = tempfile::tempdir().unwrap();
        let movie = fixture.path().join("Feature.mp4");
        std::fs::write(&movie, b"movie").unwrap();
        for filename in [
            "Feature.eng.forced.srt",
            "Feature.fr.sdh.vtt",
            "Feature.es.default.srt",
            "Feature.hi.vtt",
            "Feature.eng.forced.forced.srt",
            "Feature.unrelated.txt",
        ] {
            std::fs::write(fixture.path().join(filename), b"text").unwrap();
        }
        let report = probe::ProbeReport {
            path: movie.clone(),
            size_bytes: 5,
            format: None,
            streams: vec![json!({"index":0,"codec_type":"video","codec_name":"h264"})],
            chapters: vec![],
        };
        let streams = source_from_probe("id", "video", &movie, &report).media_streams;
        let subtitle = streams
            .iter()
            .filter(|stream| stream.stream_type == "Subtitle")
            .collect::<Vec<_>>();
        assert_eq!(subtitle.len(), 4);
        let english = subtitle
            .iter()
            .find(|stream| stream.language.as_deref() == Some("eng"))
            .unwrap();
        assert!(english.is_forced);
        assert!(!english.is_hearing_impaired);
        let french = subtitle
            .iter()
            .find(|stream| stream.language.as_deref() == Some("fr"))
            .unwrap();
        assert!(french.is_hearing_impaired);
        let spanish = subtitle
            .iter()
            .find(|stream| stream.language.as_deref() == Some("es"))
            .unwrap();
        assert!(spanish.is_default);
        assert!(
            subtitle
                .iter()
                .any(|stream| stream.language.is_none() && stream.is_hearing_impaired)
        );
    }

    #[test]
    fn pinned_subtitle_profiles_allow_only_native_external_formats() {
        let fixture = tempfile::tempdir().unwrap();
        let movie = fixture.path().join("Feature.mp4");
        std::fs::write(&movie, b"fixture").unwrap();
        std::fs::write(fixture.path().join("Feature.eng.srt"), b"subtitle").unwrap();
        std::fs::write(fixture.path().join("Feature.fr.vtt"), b"WEBVTT").unwrap();
        let report = probe::ProbeReport {
            path: movie.clone(),
            size_bytes: 7,
            format: Some(json!({"bit_rate":"1000000"})),
            streams: vec![
                json!({"index":0,"codec_type":"video","codec_name":"h264"}),
                json!({"index":1,"codec_type":"audio","codec_name":"aac"}),
            ],
            chapters: vec![],
        };
        let source = source_from_probe("id", "video", &movie, &report);
        assert_eq!(source.media_streams.len(), 4);
        for (profile, expected) in [
            (
                json!([
                    {"Method":"External","Format":"srt","Language":"eng"},
                    {"Method":2,"Format":"vtt","Language":"fr"}
                ]),
                Ok(true),
            ),
            (json!([{"Method":"Embed","Format":"srt"}]), Ok(false)),
            (json!([{"Method":"External","Format":"srt"}]), Ok(false)),
            (json!([{"Method":"External","Format":"-srt"}]), Ok(false)),
            (
                json!([{"Method":"External","Format":"vtt","Language":"eng"}]),
                Ok(false),
            ),
            (
                json!([{"Method":"External","Format":"srt","Unknown":true}]),
                Err(StatusCode::BAD_REQUEST),
            ),
        ] {
            assert_eq!(
                profile_allows_external_subtitles(profile.as_array().unwrap(), &source),
                expected
            );
        }
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
    fn embedded_subtitle_text_classification_matches_supported_codecs() {
        for (codec, is_text) in [
            ("subrip", true),
            ("ass", true),
            ("webvtt", true),
            ("mov_text", true),
            ("hdmv_pgs_subtitle", false),
            ("dvd_subtitle", false),
            ("dvb_subtitle", false),
            ("vobsub", false),
        ] {
            let stream = mapped_stream(&json!({
                "index": 2,
                "codec_type": "subtitle",
                "codec_name": codec
            }))
            .unwrap();
            assert_eq!(stream.is_text_subtitle_stream, is_text, "{codec}");
            assert!(!stream.is_external);
            assert_eq!(stream.delivery_url, None);
        }
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
