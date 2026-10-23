//! Bounded direct-file streaming using the v12.3 Jellyfin video URL shape.
//! This is static direct play only: no remuxing, transcoding or broad client parity.
use std::{
    io,
    path::{Path, PathBuf},
    sync::Arc,
};

use axum::{
    Router,
    body::Body,
    extract::{Path as RoutePath, Query, State},
    http::{
        HeaderMap, HeaderValue, StatusCode,
        header::{ACCEPT_RANGES, CONTENT_LENGTH, CONTENT_RANGE, CONTENT_TYPE, RANGE},
    },
    response::Response,
    routing::get,
};
use serde::Deserialize;
use tokio::io::{AsyncReadExt, AsyncSeekExt};
use tokio_util::io::ReaderStream;

use crate::{app::AppState, auth_api, catalog, catalog_api};

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StreamQuery {
    #[serde(rename = "static")]
    static_play: Option<bool>,
    container: Option<String>,
    start_time_ticks: Option<i64>,
    media_source_id: Option<String>,
    audio_codec: Option<String>,
    video_codec: Option<String>,
    subtitle_stream_index: Option<i64>,
}

fn media_type(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "mp4" | "m4v" => "video/mp4",
        "mp3" => "audio/mpeg",
        "flac" => "audio/flac",
        "m4a" | "alac" => "audio/mp4",
        "wav" => "audio/wav",
        "ogg" | "oga" | "opus" => "audio/ogg",
        "aac" => "audio/aac",
        "wma" => "audio/x-ms-wma",
        "mkv" => "video/x-matroska",
        "webm" => "video/webm",
        "ts" | "m2ts" => "video/mp2t",
        "mov" => "video/quicktime",
        "avi" => "video/x-msvideo",
        "mpeg" | "mpg" => "video/mpeg",
        _ => "application/octet-stream",
    }
}

#[derive(Debug, PartialEq, Eq)]
struct ByteRange {
    start: u64,
    end: u64,
}

fn parse_range(value: &str, size: u64) -> Option<ByteRange> {
    if size == 0 {
        return None;
    }
    let bounds = value.strip_prefix("bytes=")?;
    if bounds.contains(',') {
        return None;
    }
    let (first, last) = bounds.split_once('-')?;
    if first.is_empty() {
        let suffix = last.parse::<u64>().ok()?;
        if suffix == 0 {
            return None;
        }
        return Some(ByteRange {
            start: size.saturating_sub(suffix),
            end: size - 1,
        });
    }
    let start = first.parse::<u64>().ok()?;
    if start >= size {
        return None;
    }
    let end = if last.is_empty() {
        size - 1
    } else {
        last.parse::<u64>().ok()?.min(size - 1)
    };
    if end < start {
        return None;
    }
    Some(ByteRange { start, end })
}

fn response_headers(response: &mut Response, length: u64, mime: &str) -> Result<(), StatusCode> {
    response
        .headers_mut()
        .insert(ACCEPT_RANGES, HeaderValue::from_static("bytes"));
    response.headers_mut().insert(
        CONTENT_LENGTH,
        HeaderValue::from_str(&length.to_string())
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?,
    );
    response.headers_mut().insert(
        CONTENT_TYPE,
        HeaderValue::from_str(mime).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?,
    );
    Ok(())
}

fn invalid_range(size: u64) -> Response {
    let mut response = Response::new(Body::empty());
    *response.status_mut() = StatusCode::RANGE_NOT_SATISFIABLE;
    if let Ok(header) = HeaderValue::from_str(&format!("bytes */{size}")) {
        response.headers_mut().insert(CONTENT_RANGE, header);
    }
    response
}

/// Verify the exact opened file, not an earlier pathname validation.
/// The descriptor remains pinned to the checked inode even if a path is swapped.
#[cfg(target_os = "linux")]
fn open_confined(
    root: &Path,
    relative: &Path,
) -> Result<(std::fs::File, PathBuf, u64), StatusCode> {
    use std::os::fd::AsRawFd;

    let canonical_root = std::fs::canonicalize(root).map_err(|_| StatusCode::NOT_FOUND)?;
    if canonical_root != root {
        // Indexing stores canonical roots; replacing a root with a symlink is unsafe.
        return Err(StatusCode::FORBIDDEN);
    }
    let opened = std::fs::File::open(root.join(relative)).map_err(|_| StatusCode::NOT_FOUND)?;
    let actual = std::fs::canonicalize(format!("/proc/self/fd/{}", opened.as_raw_fd()))
        .map_err(|_| StatusCode::NOT_FOUND)?;
    if !actual.starts_with(&canonical_root) {
        return Err(StatusCode::FORBIDDEN);
    }
    let metadata = opened.metadata().map_err(|_| StatusCode::NOT_FOUND)?;
    if !metadata.is_file() {
        return Err(StatusCode::NOT_FOUND);
    }
    Ok((opened, actual, metadata.len()))
}

/// Direct file serving requires the Linux descriptor verification above.
#[cfg(not(target_os = "linux"))]
fn open_confined(
    _root: &Path,
    _relative: &Path,
) -> Result<(std::fs::File, PathBuf, u64), StatusCode> {
    Err(StatusCode::NOT_IMPLEMENTED)
}

async fn serve_file(
    state: Arc<AppState>,
    item_id: String,
    headers: HeaderMap,
    query: StreamQuery,
    head_only: bool,
    kind: &'static str,
    requested_container: Option<String>,
) -> Result<Response, StatusCode> {
    if query.static_play != Some(true) {
        return Err(StatusCode::NOT_IMPLEMENTED);
    }
    if query.start_time_ticks.is_some_and(|value| value < 0) {
        return Err(StatusCode::BAD_REQUEST);
    }
    if query.start_time_ticks.is_some_and(|value| value > 0)
        || query.audio_codec.is_some()
        || query.video_codec.is_some()
        || query.subtitle_stream_index.is_some()
        || query.media_source_id.as_deref().is_some_and(|source| source != item_id)
    {
        // Seeking by playback ticks, stream selection and transcoding need a
        // real FFmpeg negotiation pipeline. Only HTTP byte ranges are supported.
        return Err(StatusCode::NOT_IMPLEMENTED);
    }
    let (directory, allowed) = auth_api::library_scope(state.clone(), headers.clone()).await?;
    let matched = tokio::task::spawn_blocking(move || catalog::all_files(&directory))
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .into_iter()
        .find(|row| {
            let can_read = allowed
                .as_ref()
                .is_none_or(|roots| roots.contains(&row.root));
            can_read
                && row.kind == kind
                && catalog_api::stable_id(&row.root, &row.relative_path) == item_id
        })
        .ok_or(StatusCode::NOT_FOUND)?;

    // A Jellyfin stream.<container> URL must not silently remux. Only return
    // the already indexed source when its original extension matches.
    for requested in requested_container.into_iter().chain(query.container) {
        let valid = requested.len() <= 16
            && !requested.is_empty()
            && requested.bytes().all(|byte| byte.is_ascii_alphanumeric());
        if !valid {
            return Err(StatusCode::BAD_REQUEST);
        }
        let original = Path::new(&matched.relative_path)
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or("");
        if !requested.eq_ignore_ascii_case(original) {
            return Err(StatusCode::NOT_IMPLEMENTED);
        }
    }

    let (opened, actual, size) = tokio::task::spawn_blocking(move || {
        open_confined(Path::new(&matched.root), Path::new(&matched.relative_path))
    })
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)??;
    let range = headers
        .get(RANGE)
        .and_then(|header| header.to_str().ok())
        .map(|value| parse_range(value, size));
    let range = match range {
        Some(None) => return Ok(invalid_range(size)),
        Some(Some(range)) => Some(range),
        None => None,
    };
    let (start, length, status) = match &range {
        Some(range) => (
            range.start,
            range.end - range.start + 1,
            StatusCode::PARTIAL_CONTENT,
        ),
        None => (0, size, StatusCode::OK),
    };
    let mut response = if head_only {
        Response::new(Body::empty())
    } else {
        let mut file = tokio::fs::File::from_std(opened);
        file.seek(io::SeekFrom::Start(start))
            .await
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        Response::new(Body::from_stream(ReaderStream::new(file.take(length))))
    };
    *response.status_mut() = status;
    response_headers(&mut response, length, media_type(&actual))?;
    if let Some(range) = range {
        response.headers_mut().insert(
            CONTENT_RANGE,
            HeaderValue::from_str(&format!("bytes {}-{}/{}", range.start, range.end, size))
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?,
        );
    }
    Ok(response)
}

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/Videos/{item_id}/stream", get(stream).head(head_stream))
        .route(
            "/Videos/{item_id}/{file_name}",
            get(stream_named).head(head_stream_named),
        )
        .route(
            "/Audio/{item_id}/stream",
            get(audio_stream).head(head_audio_stream),
        )
        .route(
            "/Audio/{item_id}/{file_name}",
            get(audio_named).head(head_audio_named),
        )
}

async fn stream(
    State(state): State<Arc<AppState>>,
    RoutePath(item_id): RoutePath<String>,
    headers: HeaderMap,
    Query(query): Query<StreamQuery>,
) -> Result<Response, StatusCode> {
    serve_file(state, item_id, headers, query, false, "video", None).await
}

async fn head_stream(
    State(state): State<Arc<AppState>>,
    RoutePath(item_id): RoutePath<String>,
    headers: HeaderMap,
    Query(query): Query<StreamQuery>,
) -> Result<Response, StatusCode> {
    serve_file(state, item_id, headers, query, true, "video", None).await
}

async fn audio_stream(
    State(state): State<Arc<AppState>>,
    RoutePath(item_id): RoutePath<String>,
    headers: HeaderMap,
    Query(query): Query<StreamQuery>,
) -> Result<Response, StatusCode> {
    serve_file(state, item_id, headers, query, false, "audio", None).await
}

async fn head_audio_stream(
    State(state): State<Arc<AppState>>,
    RoutePath(item_id): RoutePath<String>,
    headers: HeaderMap,
    Query(query): Query<StreamQuery>,
) -> Result<Response, StatusCode> {
    serve_file(state, item_id, headers, query, true, "audio", None).await
}

fn container_name(filename: &str) -> Result<String, StatusCode> {
    filename
        .strip_prefix("stream.")
        .filter(|ext| !ext.is_empty())
        .map(str::to_owned)
        .ok_or(StatusCode::NOT_FOUND)
}

async fn stream_named(
    State(state): State<Arc<AppState>>,
    RoutePath((item_id, filename)): RoutePath<(String, String)>,
    headers: HeaderMap,
    Query(query): Query<StreamQuery>,
) -> Result<Response, StatusCode> {
    serve_file(
        state,
        item_id,
        headers,
        query,
        false,
        "video",
        Some(container_name(&filename)?),
    )
    .await
}

async fn head_stream_named(
    State(state): State<Arc<AppState>>,
    RoutePath((item_id, filename)): RoutePath<(String, String)>,
    headers: HeaderMap,
    Query(query): Query<StreamQuery>,
) -> Result<Response, StatusCode> {
    serve_file(
        state,
        item_id,
        headers,
        query,
        true,
        "video",
        Some(container_name(&filename)?),
    )
    .await
}

async fn audio_named(
    State(state): State<Arc<AppState>>,
    RoutePath((item_id, filename)): RoutePath<(String, String)>,
    headers: HeaderMap,
    Query(query): Query<StreamQuery>,
) -> Result<Response, StatusCode> {
    serve_file(
        state,
        item_id,
        headers,
        query,
        false,
        "audio",
        Some(container_name(&filename)?),
    )
    .await
}

async fn head_audio_named(
    State(state): State<Arc<AppState>>,
    RoutePath((item_id, filename)): RoutePath<(String, String)>,
    headers: HeaderMap,
    Query(query): Query<StreamQuery>,
) -> Result<Response, StatusCode> {
    serve_file(
        state,
        item_id,
        headers,
        query,
        true,
        "audio",
        Some(container_name(&filename)?),
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(target_os = "linux")]
    #[test]
    fn opened_handle_cannot_be_redirected_after_validation() {
        use std::io::Read;
        use std::os::unix::fs::symlink;

        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("media");
        std::fs::create_dir(&root).unwrap();
        let video = root.join("video.mp4");
        let outside = tmp.path().join("secret.mp4");
        std::fs::write(&video, b"permitted").unwrap();
        std::fs::write(&outside, b"restricted").unwrap();
        let (mut opened, _path, _) = open_confined(&root, Path::new("video.mp4")).unwrap();
        std::fs::remove_file(&video).unwrap();
        symlink(&outside, &video).unwrap();

        let mut data = Vec::new();
        opened.read_to_end(&mut data).unwrap();
        assert_eq!(data, b"permitted");
        assert_eq!(
            open_confined(&root, Path::new("video.mp4")).unwrap_err(),
            StatusCode::FORBIDDEN
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn replaced_library_root_is_rejected() {
        use std::os::unix::fs::symlink;

        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("media");
        let original = tmp.path().join("saved");
        let external = tmp.path().join("external");
        std::fs::create_dir(&root).unwrap();
        std::fs::create_dir(&external).unwrap();
        std::fs::write(external.join("video.mp4"), b"secret").unwrap();
        std::fs::rename(&root, &original).unwrap();
        symlink(&external, &root).unwrap();
        assert_eq!(
            open_confined(&root, Path::new("video.mp4")).unwrap_err(),
            StatusCode::FORBIDDEN
        );
    }

    #[test]
    fn extension_routes_accept_only_stream_prefix() {
        assert_eq!(container_name("stream.mp4").unwrap(), "mp4");
        assert_eq!(container_name("stream.flac").unwrap(), "flac");
        assert_eq!(
            container_name("video.mp4").unwrap_err(),
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            container_name("stream.").unwrap_err(),
            StatusCode::NOT_FOUND
        );
    }

    #[test]
    fn byte_ranges_reject_invalid_and_support_suffix() {
        assert_eq!(
            parse_range("bytes=2-5", 10),
            Some(ByteRange { start: 2, end: 5 })
        );
        assert_eq!(
            parse_range("bytes=-3", 10),
            Some(ByteRange { start: 7, end: 9 })
        );
        assert_eq!(
            parse_range("bytes=8-", 10),
            Some(ByteRange { start: 8, end: 9 })
        );
        assert_eq!(parse_range("bytes=20-", 10), None);
        assert_eq!(parse_range("bytes=1-3,5-7", 10), None);
        assert_eq!(parse_range("bytes=-0", 10), None);
    }
}
