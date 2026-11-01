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

pub(crate) const MAX_FONT_BYTES: usize = 4 * 1024 * 1024;
static FONT_WORKERS: OnceLock<Arc<Semaphore>> = OnceLock::new();

pub(crate) fn font_mime(raw: &str) -> Option<&'static str> {
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

/// Reject a Matroska attachment whose declared font MIME disagrees with
/// the extracted binary header. This is a bounded container/header check,
/// not a general font parser or a guarantee that an untrusted font is safe
/// for client-side shaping/rendering.
pub(crate) fn font_header_matches(mime: &str, bytes: &[u8]) -> bool {
    if bytes.len() < 12 {
        return false;
    }
    let sfnt = || {
        if bytes.len() < 12 {
            return false;
        }
        let tables = usize::from(u16::from_be_bytes([bytes[4], bytes[5]]));
        tables > 0
            && tables <= 256
            && 12_usize
                .checked_add(16 * tables)
                .is_some_and(|directory| directory <= bytes.len())
    };
    match mime {
        "font/ttf" => matches!(&bytes[..4], b"\x00\x01\x00\x00" | b"true" | b"typ1") && sfnt(),
        "font/otf" => {
            (matches!(&bytes[..4], b"OTTO" | b"\x00\x01\x00\x00") || &bytes[..4] == b"true")
                && sfnt()
        }
        "font/woff" => {
            if bytes.len() < 44 || &bytes[..4] != b"wOFF" {
                return false;
            }
            let declared = u32::from_be_bytes(bytes[8..12].try_into().unwrap()) as usize;
            let tables = usize::from(u16::from_be_bytes([bytes[12], bytes[13]]));
            let uncompressed = u32::from_be_bytes(bytes[16..20].try_into().unwrap());
            tables > 0
                && tables <= 256
                && uncompressed >= 12
                && declared == bytes.len()
                && 44 + 20 * tables < bytes.len()
        }
        "font/woff2" => {
            if bytes.len() <= 48 || &bytes[..4] != b"wOF2" {
                return false;
            }
            let declared = u32::from_be_bytes(bytes[8..12].try_into().unwrap()) as usize;
            let tables = usize::from(u16::from_be_bytes([bytes[12], bytes[13]]));
            let uncompressed = u32::from_be_bytes(bytes[16..20].try_into().unwrap());
            tables > 0 && tables <= 256 && uncompressed >= 12 && declared == bytes.len()
        }
        _ => false,
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
    let attachment = report
        .streams
        .iter()
        .find(|stream| {
            stream["codec_type"] == "attachment" && stream["index"].as_i64() == Some(index)
        })
        .ok_or(StatusCode::NOT_FOUND)?;
    let mime = attachment["tags"]["mimetype"]
        .as_str()
        .and_then(font_mime)
        .ok_or(StatusCode::NOT_FOUND)?;
    if attachment["extradata_size"]
        .as_u64()
        .is_some_and(|size| size > MAX_FONT_BYTES as u64)
    {
        return Err(StatusCode::NOT_IMPLEMENTED);
    }
    let data = dump_font(state.ffmpeg_executable(), &file, index).await?;
    if !font_header_matches(mime, &data) {
        return Err(StatusCode::NOT_IMPLEMENTED);
    }
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
    fn font_header_requires_matching_bounded_container_signature() {
        let mut ttf = vec![0_u8; 28];
        ttf[0..4].copy_from_slice(&[0, 1, 0, 0]);
        ttf[4..6].copy_from_slice(&1_u16.to_be_bytes());
        assert!(font_header_matches("font/ttf", &ttf));
        assert!(font_header_matches("font/otf", &ttf));
        assert!(!font_header_matches("font/woff", &ttf));
        assert!(!font_header_matches("font/woff2", &ttf));
        assert!(!font_header_matches("font/ttf", b"not actually a font"));
        assert!(!font_header_matches("font/ttf", &ttf[..27]));
        ttf[4..6].copy_from_slice(&257_u16.to_be_bytes());
        assert!(!font_header_matches("font/ttf", &ttf));
        let mut otf = vec![0_u8; 28];
        otf[0..4].copy_from_slice(b"OTTO");
        otf[4..6].copy_from_slice(&1_u16.to_be_bytes());
        assert!(font_header_matches("font/otf", &otf));
        assert!(!font_header_matches("font/ttf", &otf));

        let mut woff = vec![0_u8; 65];
        woff[0..4].copy_from_slice(b"wOFF");
        woff[8..12].copy_from_slice(&65_u32.to_be_bytes());
        woff[12..14].copy_from_slice(&1_u16.to_be_bytes());
        woff[16..20].copy_from_slice(&28_u32.to_be_bytes());
        assert!(font_header_matches("font/woff", &woff));
        assert!(!font_header_matches("font/woff2", &woff));
        woff[8..12].copy_from_slice(&64_u32.to_be_bytes());
        assert!(!font_header_matches("font/woff", &woff));

        let mut woff2 = vec![0_u8; 49];
        woff2[0..4].copy_from_slice(b"wOF2");
        woff2[8..12].copy_from_slice(&49_u32.to_be_bytes());
        woff2[12..14].copy_from_slice(&1_u16.to_be_bytes());
        woff2[16..20].copy_from_slice(&28_u32.to_be_bytes());
        assert!(font_header_matches("font/woff2", &woff2));
        assert!(!font_header_matches("font/woff", &woff2));
        assert!(!font_header_matches("font/woff2", &woff2[..48]));
    }

    #[test]
    fn serves_only_explicit_font_types() {
        assert_eq!(font_mime("application/x-truetype-font"), Some("font/ttf"));
        assert_eq!(font_mime("FONT/OTF"), Some("font/otf"));
        assert_eq!(font_mime("text/html"), None);
        assert_eq!(font_mime("application/octet-stream"), None);
    }
}
