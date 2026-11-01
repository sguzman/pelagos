//! Optional, authenticated fallback-font routes for clients rendering ASS.
//! No system-font discovery or media-folder enumeration occurs by default.
//! The operator must explicitly supply a canonical, read-only font folder.
use crate::{app::AppState, attachments, auth_api, streaming};
use axum::{
    Json, Router,
    body::Body,
    extract::{Path as RoutePath, State},
    http::{
        HeaderMap, HeaderValue, StatusCode,
        header::{CACHE_CONTROL, CONTENT_LENGTH, CONTENT_TYPE},
    },
    response::Response,
    routing::get,
};
use serde::Serialize;
use std::{
    io::Read,
    path::Path,
    sync::{Arc, OnceLock},
};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

const MAX_FONT_COUNT: usize = 64;
const MAX_DIRECTORY_ENTRIES: usize = 256;
const MAX_TOTAL_BYTES: u64 = 20 * 1024 * 1024;
const MAX_SCANNED_BYTES: u64 = 64 * 1024 * 1024;
static FONT_READ_WORKERS: OnceLock<Arc<Semaphore>> = OnceLock::new();

fn font_read_permit() -> Result<OwnedSemaphorePermit, StatusCode> {
    FONT_READ_WORKERS
        .get_or_init(|| Arc::new(Semaphore::new(2)))
        .clone()
        .try_acquire_owned()
        .map_err(|_| StatusCode::TOO_MANY_REQUESTS)
}

#[derive(Clone)]
struct FontInfo {
    name: String,
    size: u64,
    mime: &'static str,
}

/// Dates are null until Pelagos has a pinned, platform-independent UTC
/// formatter for actual filesystem creation/change timestamps. Never invent
/// modification dates merely to make the DTO appear complete.
#[derive(Serialize)]
#[serde(rename_all = "PascalCase")]
struct FontFile {
    name: String,
    size: u64,
    date_created: Option<String>,
    date_modified: Option<String>,
}

impl From<&FontInfo> for FontFile {
    fn from(font: &FontInfo) -> Self {
        Self {
            name: font.name.clone(),
            size: font.size,
            date_created: None,
            date_modified: None,
        }
    }
}

fn safe_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && name != "."
        && name != ".."
        && name.trim() == name
        && !name.ends_with('.')
        && !name
            .chars()
            .any(|ch| ch.is_control() || "/\\:<>\"|?*".contains(ch))
}

fn mime_for_name(name: &str) -> Option<&'static str> {
    match Path::new(name)
        .extension()?
        .to_str()?
        .to_ascii_lowercase()
        .as_str()
    {
        "ttf" => Some("font/ttf"),
        "otf" => Some("font/otf"),
        "woff" => Some("font/woff"),
        "woff2" => Some("font/woff2"),
        _ => None,
    }
}

/// Bounded directory inventory and header checks. Every candidate is opened
/// via the existing Linux confined descriptor helper, never by joining an
/// untrusted HTTP name with a filesystem path.
fn available(root: &Path) -> Result<Vec<FontInfo>, StatusCode> {
    if !root.is_absolute() || std::fs::canonicalize(root).ok().as_deref() != Some(root) {
        return Err(StatusCode::FORBIDDEN);
    }
    let directory = std::fs::read_dir(root).map_err(|_| StatusCode::NOT_FOUND)?;
    let mut names = Vec::new();
    for (count, entry) in directory.enumerate() {
        if count >= MAX_DIRECTORY_ENTRIES {
            return Err(StatusCode::NOT_IMPLEMENTED);
        }
        let entry = entry.map_err(|_| StatusCode::NOT_FOUND)?;
        let Ok(name) = entry.file_name().into_string() else {
            continue;
        };
        if !safe_name(&name) {
            continue;
        }
        let Some(mime) = mime_for_name(&name) else {
            continue;
        };
        if !entry.file_type().is_ok_and(|kind| kind.is_file()) {
            continue;
        }
        names.push((name, mime));
    }
    names.sort_unstable_by(|a, b| a.0.cmp(&b.0));
    let mut listed = Vec::new();
    let mut total = 0_u64;
    let mut scanned = 0_u64;
    for (name, mime) in names {
        if listed.len() >= MAX_FONT_COUNT {
            break;
        }
        let Ok((file, _, size)) = streaming::open_confined(root, Path::new(&name)) else {
            continue;
        };
        if size < 12 || size > attachments::MAX_FONT_BYTES as u64 {
            continue;
        }
        scanned = scanned
            .checked_add(size)
            .ok_or(StatusCode::NOT_IMPLEMENTED)?;
        if scanned > MAX_SCANNED_BYTES {
            return Err(StatusCode::NOT_IMPLEMENTED);
        }
        if total.saturating_add(size) > MAX_TOTAL_BYTES {
            continue;
        }
        let mut bytes = Vec::new();
        let read = file
            .take(attachments::MAX_FONT_BYTES as u64 + 1)
            .read_to_end(&mut bytes);
        if read.is_err()
            || bytes.len() as u64 != size
            || !attachments::font_header_matches(mime, &bytes)
        {
            continue;
        }
        let Some(next_total) = total.checked_add(size) else {
            continue;
        };
        if next_total > MAX_TOTAL_BYTES {
            continue;
        }
        total = next_total;
        listed.push(FontInfo { name, size, mime });
    }
    Ok(listed)
}

async fn fallback_font_list(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Json<Vec<FontFile>>, StatusCode> {
    auth_api::authenticated(state.clone(), headers).await?;
    let Some(root) = state.fallback_font_dir().map(Path::to_path_buf) else {
        return Ok(Json(Vec::new()));
    };
    let _permit = font_read_permit()?;
    let fonts = tokio::task::spawn_blocking(move || available(&root))
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)??;
    Ok(Json(fonts.iter().map(FontFile::from).collect()))
}

async fn serve_font(
    state: Arc<AppState>,
    name: String,
    headers: HeaderMap,
    head: bool,
) -> Result<Response, StatusCode> {
    auth_api::authenticated(state.clone(), headers).await?;
    if !safe_name(&name) {
        return Err(StatusCode::NOT_FOUND);
    }
    let root = state
        .fallback_font_dir()
        .map(Path::to_path_buf)
        .ok_or(StatusCode::NOT_FOUND)?;
    let _permit = font_read_permit()?;
    let (data, mime) = tokio::task::spawn_blocking(move || {
        let font = available(&root)?
            .into_iter()
            .find(|font| font.name == name)
            .ok_or(StatusCode::NOT_FOUND)?;
        let (file, _, size) = streaming::open_confined(&root, Path::new(&name))?;
        if size != font.size {
            return Err(StatusCode::NOT_FOUND);
        }
        let mut bytes = Vec::new();
        file.take(attachments::MAX_FONT_BYTES as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| StatusCode::NOT_FOUND)?;
        if bytes.len() as u64 != size || !attachments::font_header_matches(font.mime, &bytes) {
            return Err(StatusCode::NOT_IMPLEMENTED);
        }
        Ok::<_, StatusCode>((bytes, font.mime))
    })
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)??;
    let length = data.len();
    let mut response = Response::new(if head {
        Body::empty()
    } else {
        Body::from(data)
    });
    response
        .headers_mut()
        .insert(CONTENT_TYPE, HeaderValue::from_static(mime));
    response.headers_mut().insert(
        CONTENT_LENGTH,
        HeaderValue::from_str(&length.to_string())
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?,
    );
    response
        .headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("private, no-store"));
    response.headers_mut().insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    Ok(response)
}

async fn get_font(
    State(state): State<Arc<AppState>>,
    RoutePath(name): RoutePath<String>,
    headers: HeaderMap,
) -> Result<Response, StatusCode> {
    serve_font(state, name, headers, false).await
}

async fn head_font(
    State(state): State<Arc<AppState>>,
    RoutePath(name): RoutePath<String>,
    headers: HeaderMap,
) -> Result<Response, StatusCode> {
    serve_font(state, name, headers, true).await
}

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/FallbackFont/Fonts", get(fallback_font_list))
        .route("/FallbackFont/Fonts/{name}", get(get_font).head(head_font))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn permits_only_simple_font_names_and_four_known_file_types() {
        assert!(safe_name("Noto Sans CJK.woff2"));
        assert!(safe_name("Fuente_Ágil.ttf"));
        for invalid in [
            "",
            ".",
            "..",
            "../secret.ttf",
            "C:\\secret.ttf",
            "font\n.ttf",
            "foo.ttf/",
            "trailing.otf.",
            "double//slash.otf",
        ] {
            assert!(!safe_name(invalid), "{invalid:?}");
        }
        assert_eq!(mime_for_name("font.TTF"), Some("font/ttf"));
        assert_eq!(mime_for_name("font.otf"), Some("font/otf"));
        assert_eq!(mime_for_name("font.woff2"), Some("font/woff2"));
        assert_eq!(mime_for_name("readme.txt"), None);
    }
}
