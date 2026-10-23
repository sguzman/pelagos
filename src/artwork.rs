//! Authenticated, read-only primary artwork from already indexed sidecar files.
//! No external image providers, image resizing, metadata rewrites or source writes.
use std::{path::Path, sync::Arc, time::UNIX_EPOCH};

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
use tokio::io::AsyncReadExt;
use tokio_util::io::ReaderStream;

use crate::{app::AppState, auth_api, catalog, catalog_api, streaming};

const MAX_ARTWORK_BYTES: u64 = 16 * 1024 * 1024;

/// Deterministically select a local sidecar without scanning unindexed paths.
/// Explicit per-title images take precedence over generic directory artwork.
pub(crate) fn primary_for_file<'a>(
    images: &'a [catalog::CatalogFile],
    media: &catalog::CatalogFile,
) -> Option<&'a catalog::CatalogFile> {
    if !matches!(media.kind.as_str(), "video" | "audio") {
        return None;
    }
    let source = Path::new(&media.relative_path);
    let parent = source.parent().unwrap_or_else(|| Path::new(""));
    let stem = source.file_stem()?.to_str()?.to_lowercase();
    let candidates = [
        format!("{stem}-poster"),
        stem,
        "poster".to_owned(),
        "folder".to_owned(),
        "cover".to_owned(),
    ];

    candidates.iter().find_map(|candidate| {
        images.iter().find(|image| {
            if image.kind != "image" || image.root != media.root {
                return false;
            }
            let path = Path::new(&image.relative_path);
            path.parent().unwrap_or_else(|| Path::new("")) == parent
                && path
                    .file_stem()
                    .is_some_and(|name| name.to_string_lossy().to_lowercase() == *candidate)
                && supported_mime(path).is_some()
        })
    })
}

fn supported_mime(path: &Path) -> Option<&'static str> {
    match path.extension()?.to_str()?.to_ascii_lowercase().as_str() {
        "jpg" | "jpeg" => Some("image/jpeg"),
        "png" => Some("image/png"),
        "webp" => Some("image/webp"),
        "gif" => Some("image/gif"),
        _ => None,
    }
}

/// A change of size or mtime invalidates the advertised artwork tag.
pub(crate) fn image_tag(image: &catalog::CatalogFile) -> Option<String> {
    let path = Path::new(&image.root).join(&image.relative_path);
    let metadata = std::fs::symlink_metadata(&path).ok()?;
    if !metadata.file_type().is_file() || metadata.len() > MAX_ARTWORK_BYTES {
        return None;
    }
    let modified = metadata
        .modified()
        .ok()?
        .duration_since(UNIX_EPOCH)
        .ok()?
        .as_nanos();
    Some(catalog_api::stable_id(
        &image.root,
        &format!(
            "artwork:{}:{}:{modified}",
            image.relative_path,
            metadata.len()
        ),
    ))
}

async fn image_response(
    state: Arc<AppState>,
    headers: HeaderMap,
    item_id: String,
    image_type: String,
    head_only: bool,
) -> Result<Response, StatusCode> {
    if image_type != "Primary" {
        return Err(StatusCode::NOT_FOUND);
    }
    let (dir, allowed) = auth_api::library_scope(state, headers).await?;
    let selected = tokio::task::spawn_blocking(move || {
        let files = catalog::all_files(&dir)?;
        let media = files.iter().find(|file| {
            matches!(file.kind.as_str(), "video" | "audio")
                && allowed
                    .as_ref()
                    .is_none_or(|roots| roots.contains(&file.root))
                && catalog_api::stable_id(&file.root, &file.relative_path) == item_id
        });
        let image = media.and_then(|media| primary_for_file(&files, media));
        Ok::<_, Box<dyn std::error::Error + Send + Sync>>(image.cloned())
    })
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    .ok_or(StatusCode::NOT_FOUND)?;
    let (opened, path, size) = tokio::task::spawn_blocking(move || {
        streaming::open_confined(
            Path::new(&selected.root),
            Path::new(&selected.relative_path),
        )
    })
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)??;
    if size > MAX_ARTWORK_BYTES {
        return Err(StatusCode::PAYLOAD_TOO_LARGE);
    }
    let mime = supported_mime(&path).ok_or(StatusCode::UNSUPPORTED_MEDIA_TYPE)?;
    let mut response = if head_only {
        Response::new(Body::empty())
    } else {
        let input = tokio::fs::File::from_std(opened);
        Response::new(Body::from_stream(ReaderStream::new(input.take(size))))
    };
    response
        .headers_mut()
        .insert(CONTENT_TYPE, HeaderValue::from_static(mime));
    response.headers_mut().insert(
        CONTENT_LENGTH,
        HeaderValue::from_str(&size.to_string()).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?,
    );
    // Keep image validators honest if a user edits the original artwork.
    response.headers_mut().insert(
        CACHE_CONTROL,
        HeaderValue::from_static("private, max-age=0"),
    );
    Ok(response)
}

async fn primary(
    State(state): State<Arc<AppState>>,
    RoutePath((item_id, image_type)): RoutePath<(String, String)>,
    headers: HeaderMap,
) -> Result<Response, StatusCode> {
    image_response(state, headers, item_id, image_type, false).await
}

async fn head_primary(
    State(state): State<Arc<AppState>>,
    RoutePath((item_id, image_type)): RoutePath<(String, String)>,
    headers: HeaderMap,
) -> Result<Response, StatusCode> {
    image_response(state, headers, item_id, image_type, true).await
}

pub fn router() -> Router<Arc<AppState>> {
    Router::new().route(
        "/Items/{item_id}/Images/{image_type}",
        get(primary).head(head_primary),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(root: &str, relative: &str, kind: &str) -> catalog::CatalogFile {
        catalog::CatalogFile {
            root: root.to_owned(),
            relative_path: relative.to_owned(),
            kind: kind.to_owned(),
            size_bytes: 8,
        }
    }

    #[test]
    fn prioritizes_same_title_over_generic_cover_and_never_crosses_library() {
        let images = vec![
            entry("/a", "Movie/cover.png", "image"),
            entry("/b", "Movie/Movie-poster.jpg", "image"),
            entry("/a", "Movie/Movie.jpg", "image"),
            entry("/a", "Movie/Movie-poster.webp", "image"),
        ];
        let media = entry("/a", "Movie/Movie.mkv", "video");
        assert_eq!(
            primary_for_file(&images, &media).unwrap().relative_path,
            "Movie/Movie-poster.webp"
        );
        let other = entry("/c", "Movie/Movie.mkv", "video");
        assert!(primary_for_file(&images, &other).is_none());
        let photo = entry("/a", "Movie/image.jpg", "image");
        assert!(primary_for_file(&images, &photo).is_none());
    }
}
