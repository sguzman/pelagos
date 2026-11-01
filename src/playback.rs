//! Conservative Jellyfin-shaped playback metadata, derived from FFprobe.
//! This is not playback negotiation parity: only static file sources are offered.
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    sync::Arc,
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
        let (format, language) = if suffix == "srt" {
            ("srt", None)
        } else if suffix == "vtt" {
            ("vtt", None)
        } else {
            let Some((language, format)) = suffix.rsplit_once('.') else {
                continue;
            };
            if !matches!(format, "srt" | "vtt")
                || !(2..=3).contains(&language.len())
                || !language.bytes().all(|byte| byte.is_ascii_alphabetic())
            {
                continue;
            }
            (
                if format == "vtt" { "vtt" } else { "srt" },
                Some(language.to_ascii_lowercase()),
            )
        };
        if entry.file_type().is_ok_and(|kind| kind.is_file())
            && entry
                .metadata()
                .is_ok_and(|meta| meta.len() <= 4 * 1024 * 1024)
        {
            names.push((filename, language, format));
        }
    }
    names.sort_unstable_by(|a, b| a.0.cmp(&b.0));
    names
        .into_iter()
        .take(16)
        .enumerate()
        .map(|(offset, (filename, language, format))| ExternalSubtitle {
            index: first_index + offset as i64,
            filename,
            language,
            format,
        })
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
                "tags": {"language": language}
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
            .filter(|_| stream.is_external)
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
    if !stream_selection_is_noop(&probed, req.audio_stream_index, req.subtitle_stream_index) {
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
    let (source, probed) = tokio::task::spawn_blocking(move || {
        let source = source_from_probe(&item_id, kind, &file, &probed);
        (source, probed)
    })
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
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
    let (_, scope) = auth_api::library_scope(state.clone(), headers).await?;
    if item != media_source {
        return Err(StatusCode::NOT_FOUND);
    }
    let (source, kind) = media_file(&state, item, scope).await?;
    if kind != "video" {
        return Err(StatusCode::NOT_FOUND);
    }
    let report = probe::probe(&source, state.probe_executable(), probe::DEFAULT_TIMEOUT)
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    let discovered_source = source.clone();
    let sidecar = tokio::task::spawn_blocking(move || {
        external_subtitles(&discovered_source, &report)
            .into_iter()
            .find(|sidecar| sidecar.index == index && sidecar.format == format)
    })
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    .ok_or(StatusCode::NOT_FOUND)?;
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
    let mut response = if head {
        Response::new(Body::empty())
    } else {
        Response::new(Body::from_stream(ReaderStream::new(
            tokio::fs::File::from_std(opened),
        )))
    };
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
        HeaderValue::from_str(&size.to_string()).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?,
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
