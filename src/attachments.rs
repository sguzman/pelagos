//! Pinned video font attachment GET/HEAD: authenticated, bounded and read-only.
use crate::{app::AppState, auth_api, playback, probe};
use axum::{
    Router,
    body::Body,
    extract::{Path as RoutePath, State},
    http::{
        HeaderMap, HeaderValue, StatusCode,
        header::{CACHE_CONTROL, CONTENT_LENGTH, CONTENT_TYPE},
    },
    response::Response,
    routing::get,
};
use std::{
    path::Path,
    process::Stdio,
    sync::{Arc, OnceLock},
    time::Duration,
};
use tokio::{io::AsyncReadExt, process::Command, sync::Semaphore};

const MAX_FONT_BYTES: usize = 4 * 1024 * 1024;
static FONT_WORKERS: OnceLock<Arc<Semaphore>> = OnceLock::new();

fn font_mime(raw: &str) -> Option<&'static str> {
    match raw.to_ascii_lowercase().as_str() {
        "font/ttf" | "application/x-truetype-font" | "application/x-font-ttf" => Some("font/ttf"),
        "font/otf" | "application/vnd.ms-opentype" | "application/x-font-opentype" => {
            Some("font/otf")
        }
        "font/woff" => Some("font/woff"),
        "font/woff2" => Some("font/woff2"),
        _ => None,
    }
}

async fn dump_font(ffmpeg: &Path, file: &Path, index: i64) -> Result<Vec<u8>, StatusCode> {
    let semaphore = FONT_WORKERS
        .get_or_init(|| Arc::new(Semaphore::new(2)))
        .clone();
    let _permit = semaphore
        .try_acquire_owned()
        .map_err(|_| StatusCode::TOO_MANY_REQUESTS)?;
    let mut child = Command::new(ffmpeg)
        .args(["-nostdin", "-hide_banner", "-loglevel", "error"])
        .arg(format!("-dump_attachment:{index}"))
        .arg("pipe:1")
        .arg("-i")
        .arg(file)
        .args(["-t", "0", "-f", "null", "-"])
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
        let mut data = Vec::new();
        stdout
            .take((MAX_FONT_BYTES + 1) as u64)
            .read_to_end(&mut data)
            .await
            .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
        if data.len() > MAX_FONT_BYTES {
            let _ = child.kill().await;
            return Err(StatusCode::NOT_IMPLEMENTED);
        }
        let exit = child
            .wait()
            .await
            .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
        if !exit.success() || data.is_empty() {
            return Err(StatusCode::NOT_IMPLEMENTED);
        }
        Ok(data)
    })
    .await
    .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?
}

async fn serve(
    state: Arc<AppState>,
    (item, media_source, index): (String, String, i64),
    headers: HeaderMap,
    head: bool,
) -> Result<Response, StatusCode> {
    let (_, scope) = auth_api::library_scope(state.clone(), headers.clone()).await?;
    if item != media_source || !(0..=i64::from(i32::MAX)).contains(&index) {
        return Err(StatusCode::NOT_FOUND);
    }
    let (file, kind) = playback::media_file(&state, item.clone(), scope).await?;
    if kind != "video" {
        return Err(StatusCode::NOT_FOUND);
    }
    let revision = tokio::fs::metadata(&file)
        .await
        .map_err(|_| StatusCode::NOT_FOUND)?;
    let report = probe::probe(&file, state.probe_executable(), probe::DEFAULT_TIMEOUT)
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    let mime = report
        .streams
        .iter()
        .find(|stream| {
            stream["codec_type"] == "attachment" && stream["index"].as_i64() == Some(index)
        })
        .and_then(|stream| stream["tags"]["mimetype"].as_str())
        .and_then(font_mime)
        .ok_or(StatusCode::NOT_FOUND)?;
    let data = dump_font(state.ffmpeg_executable(), &file, index).await?;
    playback::reauthorize_video_source(state, headers, item, &file, &revision).await?;
    let length = data.len();
    let mut response = Response::new(if head {
        Body::empty()
    } else {
        Body::from(data)
    });
    response
        .headers_mut()
        .insert(CONTENT_TYPE, HeaderValue::from_static(mime));
    response
        .headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("private, no-store"));
    response.headers_mut().insert(
        CONTENT_LENGTH,
        HeaderValue::from_str(&length.to_string())
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?,
    );
    response.headers_mut().insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    Ok(response)
}

async fn get_attachment(
    State(state): State<Arc<AppState>>,
    RoutePath(route): RoutePath<(String, String, i64)>,
    headers: HeaderMap,
) -> Result<Response, StatusCode> {
    serve(state, route, headers, false).await
}

async fn head_attachment(
    State(state): State<Arc<AppState>>,
    RoutePath(route): RoutePath<(String, String, i64)>,
    headers: HeaderMap,
) -> Result<Response, StatusCode> {
    serve(state, route, headers, true).await
}

pub fn router() -> Router<Arc<AppState>> {
    Router::new().route(
        "/Videos/{video_id}/{media_source_id}/Attachments/{index}",
        get(get_attachment).head(head_attachment),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn serves_only_explicit_font_types() {
        assert_eq!(font_mime("application/x-truetype-font"), Some("font/ttf"));
        assert_eq!(font_mime("FONT/OTF"), Some("font/otf"));
        assert_eq!(font_mime("text/html"), None);
        assert_eq!(font_mime("application/octet-stream"), None);
    }
}
