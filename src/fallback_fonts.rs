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
    time::{SystemTime, UNIX_EPOCH},
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
    date_created: Option<String>,
    date_modified: Option<String>,
}

/// Dates come only from the opened filesystem descriptor. When a platform
/// does not expose creation time, DateCreated stays null instead of fake.
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
            date_created: font.date_created.clone(),
            date_modified: font.date_modified.clone(),
        }
    }
}

// Bounded, platform-independent UTC conversion from nonnegative SystemTime.
// Keep the representation parseable by v12.3 clients without locale APIs.
fn system_time_utc(value: SystemTime) -> Option<String> {
    let duration = value.duration_since(UNIX_EPOCH).ok()?;
    let seconds = i64::try_from(duration.as_secs()).ok()?;
    let days = seconds / 86_400;
    let day_seconds = seconds % 86_400;
    let z = days.checked_add(719_468)?;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let mut year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = mp + if mp < 10 { 3 } else { -9 };
    if month <= 2 {
        year += 1;
    }
    if !(1970..=9999).contains(&year) {
        return None;
    }
    let hour = day_seconds / 3_600;
    let minute = (day_seconds % 3_600) / 60;
    let second = day_seconds % 60;
    let ticks = duration.subsec_nanos() / 100;
    Some(format!(
        "{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.{ticks:07}Z"
    ))
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
        let Ok(hint) = entry.metadata() else {
            continue;
        };
        names.push((name, mime, hint.len()));
    }
    // Jellyfin v12.3 orders fallback fonts by size first, then name.
    // Sorting before the bounded reads also makes the 20 MiB subset stable.
    names.sort_unstable_by(|a, b| a.2.cmp(&b.2).then_with(|| a.0.cmp(&b.0)));
    let mut listed = Vec::new();
    let mut total = 0_u64;
    let mut scanned = 0_u64;
    for (name, mime, _) in names {
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
        if total.saturating_add(size) >= MAX_TOTAL_BYTES {
            break;
        }
        let metadata = file.metadata().ok();
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
        if next_total >= MAX_TOTAL_BYTES {
            break;
        }
        total = next_total;
        listed.push(FontInfo {
            name,
            size,
            mime,
            date_created: metadata
                .as_ref()
                .and_then(|meta| meta.created().ok())
                .and_then(system_time_utc),
            date_modified: metadata
                .as_ref()
                .and_then(|meta| meta.modified().ok())
                .and_then(system_time_utc),
        });
    }
    listed.sort_unstable_by(|a, b| a.size.cmp(&b.size).then_with(|| a.name.cmp(&b.name)));
    Ok(listed)
}

// Jellyfin uses ordinal case-insensitive lookup for fallback font names.
// An exact match takes precedence. Refuse ambiguous case-folded matches on
// case-sensitive filesystems instead of unpredictably selecting a file.
fn select_font(fonts: Vec<FontInfo>, requested: &str) -> Option<FontInfo> {
    if let Some(exact) = fonts.iter().find(|font| font.name == requested) {
        return Some(exact.clone());
    }
    let mut matching = fonts
        .into_iter()
        .filter(|font| font.name.eq_ignore_ascii_case(requested));
    let first = matching.next()?;
    matching.next().is_none().then_some(first)
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

// Pinned Jellyfin v12.3 SubtitleController returns HTTP 200 with an
// empty body for an absent fallback font, not 204. The upstream comments
// call out compatibility with SubtitlesOctopus. We deliberately still
// reject unsafe route names, but follow that behavior for safe names.
fn empty_font_response() -> Response {
    let mut response = Response::new(Body::empty());
    response
        .headers_mut()
        .insert(CONTENT_LENGTH, HeaderValue::from_static("0"));
    response
        .headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("private, no-store"));
    response.headers_mut().insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    response
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
    let Some(root) = state.fallback_font_dir().map(Path::to_path_buf) else {
        return Ok(empty_font_response());
    };
    let _permit = font_read_permit()?;
    let found = tokio::task::spawn_blocking(move || {
        let Some(font) = select_font(available(&root)?, &name) else {
            return Ok::<_, StatusCode>(None);
        };
        let Ok((file, _, size)) = streaming::open_confined(&root, Path::new(&font.name)) else {
            return Ok(None);
        };
        if size != font.size {
            return Ok(None);
        }
        let mut bytes = Vec::new();
        file.take(attachments::MAX_FONT_BYTES as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| StatusCode::NOT_FOUND)?;
        if bytes.len() as u64 != size || !attachments::font_header_matches(font.mime, &bytes) {
            return Ok(None);
        }
        Ok(Some((bytes, font.mime)))
    })
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)??;
    let Some((data, mime)) = found else {
        return Ok(empty_font_response());
    };
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
    fn filesystem_timestamps_format_as_utc_without_local_time_dependencies() {
        assert_eq!(
            system_time_utc(UNIX_EPOCH).as_deref(),
            Some("1970-01-01T00:00:00.0000000Z")
        );
        let leap = UNIX_EPOCH + std::time::Duration::from_secs(951_782_400);
        assert_eq!(
            system_time_utc(leap).as_deref(),
            Some("2000-02-29T00:00:00.0000000Z")
        );
        let fractional = UNIX_EPOCH
            + std::time::Duration::from_secs(1)
            + std::time::Duration::from_nanos(123_456_700);
        assert_eq!(
            system_time_utc(fractional).as_deref(),
            Some("1970-01-01T00:00:01.1234567Z")
        );
        assert!(system_time_utc(UNIX_EPOCH - std::time::Duration::from_secs(1)).is_none());
    }

    #[test]
    fn fallback_font_names_lookup_ignore_ascii_case_without_ambiguity() {
        let fonts = vec![
            FontInfo {
                name: "Family.ttf".to_owned(),
                size: 28,
                mime: "font/ttf",
                date_created: None,
                date_modified: None,
            },
            FontInfo {
                name: "Other.otf".to_owned(),
                size: 32,
                mime: "font/otf",
                date_created: None,
                date_modified: None,
            },
        ];
        assert_eq!(
            select_font(fonts.clone(), "family.TTF").unwrap().name,
            "Family.ttf"
        );
        assert_eq!(
            select_font(fonts.clone(), "Other.otf").unwrap().name,
            "Other.otf"
        );
        assert!(select_font(fonts.clone(), "missing.ttf").is_none());
        let ambiguous = vec![
            FontInfo {
                name: "Face.ttf".to_owned(),
                size: 28,
                mime: "font/ttf",
                date_created: None,
                date_modified: None,
            },
            FontInfo {
                name: "FACE.TTF".to_owned(),
                size: 28,
                mime: "font/ttf",
                date_created: None,
                date_modified: None,
            },
        ];
        assert!(select_font(ambiguous.clone(), "face.ttf").is_none());
        assert_eq!(select_font(ambiguous, "Face.ttf").unwrap().name, "Face.ttf");
    }

    #[test]
    fn inventory_fails_closed_before_unbounded_directory_enumeration() {
        let root = tempfile::tempdir().unwrap();
        assert!(available(root.path()).unwrap().is_empty());
        for index in 0..=MAX_DIRECTORY_ENTRIES {
            std::fs::write(root.path().join(format!("unrelated-{index:03}.txt")), b"x").unwrap();
        }
        assert!(matches!(
            available(root.path()),
            Err(StatusCode::NOT_IMPLEMENTED)
        ));
    }

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
