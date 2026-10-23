//! Authenticated, read-only primary artwork from already indexed sidecar files.
//! No external image providers, image resizing, metadata rewrites or source writes.
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    sync::Arc,
    time::UNIX_EPOCH,
};

use axum::{
    Json, Router,
    body::Body,
    extract::{Path as RoutePath, State},
    http::{
        HeaderMap, HeaderValue, StatusCode,
        header::{CACHE_CONTROL, CONTENT_LENGTH, CONTENT_TYPE, ETAG, IF_NONE_MATCH},
    },
    response::Response,
    routing::get,
};
use serde::Serialize;
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

/// Folder-level artwork is distinct from media-specific artwork and never
/// escapes its exact indexed directory.
pub(crate) fn primary_for_folder<'a>(
    images: &'a [catalog::CatalogFile],
    root: &str,
    directory: &Path,
) -> Option<&'a catalog::CatalogFile> {
    for label in ["poster", "folder", "cover"] {
        if let Some(image) = images.iter().find(|image| {
            let path = Path::new(&image.relative_path);
            image.root == root
                && image.kind == "image"
                && path.parent().unwrap_or_else(|| Path::new("")) == directory
                && path
                    .file_stem()
                    .is_some_and(|stem| stem.to_string_lossy().eq_ignore_ascii_case(label))
                && supported_mime(path).is_some()
        }) {
            return Some(image);
        }
    }
    None
}

/// Resolve media, collection, or nested-folder artwork from explicitly indexed
/// paths. The caller must first apply its authenticated library-root scope.
fn primary_for_id<'a>(
    files: &'a [catalog::CatalogFile],
    roots: &[String],
    item_id: &str,
) -> Option<&'a catalog::CatalogFile> {
    for file in files {
        if matches!(file.kind.as_str(), "video" | "audio")
            && catalog_api::stable_id(&file.root, &file.relative_path) == item_id
        {
            return primary_for_file(files, file);
        }
    }
    for root in roots {
        if catalog_api::stable_id(root, "") == item_id {
            return primary_for_folder(files, root, Path::new(""));
        }
    }
    for file in files {
        let mut folder = PathBuf::new();
        if let Some(parent) = Path::new(&file.relative_path).parent() {
            for component in parent.components() {
                folder.push(component.as_os_str());
                if catalog_api::stable_id(
                    &file.root,
                    &format!("folder:{}", folder.to_string_lossy()),
                ) == item_id
                {
                    return primary_for_folder(files, &file.root, &folder);
                }
            }
        }
    }
    None
}

/// Cover files linked to a video/audio library are artwork, not standalone
/// photographs in browsing/search. Independent photos retain their own items.
/// Files remain indexed and available to the authenticated artwork endpoints.
pub(crate) fn sidecar_keys(files: &[catalog::CatalogFile]) -> HashSet<(String, String)> {
    let mut media_directories = HashSet::new();
    let mut media_titles = HashSet::new();
    for file in files {
        if !matches!(file.kind.as_str(), "video" | "audio") {
            continue;
        }
        let path = Path::new(&file.relative_path);
        let parent = path.parent().unwrap_or_else(|| Path::new(""));
        if let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) {
            media_titles.insert((file.root.clone(), parent.to_path_buf(), stem.to_lowercase()));
        }
        for directory in parent.ancestors() {
            media_directories.insert((file.root.clone(), directory.to_path_buf()));
        }
    }
    files
        .iter()
        .filter(|image| image.kind == "image")
        .filter_map(|image| {
            let path = Path::new(&image.relative_path);
            supported_mime(path)?;
            let parent = path.parent().unwrap_or_else(|| Path::new(""));
            let name = path.file_stem()?.to_str()?.to_lowercase();
            let folder_art = matches!(name.as_str(), "poster" | "folder" | "cover")
                && media_directories.contains(&(image.root.clone(), parent.to_path_buf()));
            let title_art =
                media_titles.contains(&(image.root.clone(), parent.to_path_buf(), name.clone()))
                    || name.strip_suffix("-poster").is_some_and(|title| {
                        media_titles.contains(&(
                            image.root.clone(),
                            parent.to_path_buf(),
                            title.to_owned(),
                        ))
                    });
            (folder_art || title_art).then(|| (image.root.clone(), image.relative_path.clone()))
        })
        .collect()
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

/// Derive a validator from an actual file's metadata. Reuse this for both
/// catalog tags and the descriptor-backed image response.
fn tag_from_metadata(image: &catalog::CatalogFile, metadata: &std::fs::Metadata) -> Option<String> {
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

/// A change of size or mtime invalidates the advertised artwork tag.
pub(crate) fn image_tag(image: &catalog::CatalogFile) -> Option<String> {
    let path = Path::new(&image.root).join(&image.relative_path);
    let metadata = std::fs::symlink_metadata(&path).ok()?;
    tag_from_metadata(image, &metadata)
}

async fn selected_image(
    state: Arc<AppState>,
    headers: HeaderMap,
    item_id: String,
) -> Result<catalog::CatalogFile, StatusCode> {
    let (dir, allowed) = auth_api::library_scope(state, headers).await?;
    tokio::task::spawn_blocking(move || {
        let mut files = catalog::all_files(&dir)?;
        let mut roots = catalog::library_roots(&dir)?;
        if let Some(allowed) = allowed {
            files.retain(|file| allowed.contains(&file.root));
            roots.retain(|root| allowed.contains(root));
        }
        let image = primary_for_id(&files, &roots, &item_id);
        Ok::<_, Box<dyn std::error::Error + Send + Sync>>(image.cloned())
    })
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    .ok_or(StatusCode::NOT_FOUND)
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
    let conditional = headers
        .get(IF_NONE_MATCH)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let selected = selected_image(state, headers, item_id).await?;
    let (opened, path, size, etag) = tokio::task::spawn_blocking(move || {
        let (opened, path, size) = streaming::open_confined(
            Path::new(&selected.root),
            Path::new(&selected.relative_path),
        )?;
        // Validate against the already opened confined descriptor, never an
        // unverified path after an image was swapped or symlinked.
        let tag = opened
            .metadata()
            .ok()
            .and_then(|meta| tag_from_metadata(&selected, &meta));
        Ok::<_, StatusCode>((opened, path, size, tag.map(|tag| format!("\"{tag}\""))))
    })
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)??;
    if size > MAX_ARTWORK_BYTES {
        return Err(StatusCode::PAYLOAD_TOO_LARGE);
    }
    let mime = supported_mime(&path).ok_or(StatusCode::UNSUPPORTED_MEDIA_TYPE)?;
    if let (Some(request), Some(etag)) = (conditional.as_deref(), etag.as_deref())
        && request.split(',').any(|candidate| candidate.trim() == etag)
    {
        let mut response = Response::new(Body::empty());
        *response.status_mut() = StatusCode::NOT_MODIFIED;
        response
            .headers_mut()
            .insert(CACHE_CONTROL, HeaderValue::from_static("private, no-cache"));
        response.headers_mut().insert(
            ETAG,
            HeaderValue::from_str(etag).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?,
        );
        return Ok(response);
    }
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
    response
        .headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("private, no-cache"));
    if let Some(etag) = etag {
        response.headers_mut().insert(
            ETAG,
            HeaderValue::from_str(&etag).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?,
        );
    }
    Ok(response)
}

#[derive(Serialize)]
#[serde(rename_all = "PascalCase")]
struct ImageInfo {
    image_type: &'static str,
    image_index: Option<usize>,
    image_tag: String,
    size: u64,
}

/// The image collection API lists only present, indexed local primary artwork.
/// Original filesystem paths, blurhash and unknown image dimensions are not
/// invented or exposed.
async fn image_infos(
    State(state): State<Arc<AppState>>,
    RoutePath(item_id): RoutePath<String>,
    headers: HeaderMap,
) -> Result<Json<Vec<ImageInfo>>, StatusCode> {
    let readable = catalog_api::accessible_nodes(state.clone(), headers.clone()).await?;
    if !readable.iter().any(|item| item.id == item_id) {
        return Err(StatusCode::NOT_FOUND);
    }
    let image = match selected_image(state, headers, item_id).await {
        Ok(image) => image,
        Err(StatusCode::NOT_FOUND) => return Ok(Json(Vec::new())),
        Err(error) => return Err(error),
    };
    let details = tokio::task::spawn_blocking(move || {
        let tag = image_tag(&image);
        let size = std::fs::symlink_metadata(Path::new(&image.root).join(&image.relative_path))
            .ok()
            .filter(|meta| meta.file_type().is_file())
            .map(|meta| meta.len());
        tag.zip(size)
    })
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let Some((image_tag, size)) = details else {
        return Ok(Json(Vec::new()));
    };
    Ok(Json(vec![ImageInfo {
        image_type: "Primary",
        image_index: None,
        image_tag,
        size,
    }]))
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

async fn indexed_primary(
    State(state): State<Arc<AppState>>,
    RoutePath((item_id, image_type, index)): RoutePath<(String, String, usize)>,
    headers: HeaderMap,
) -> Result<Response, StatusCode> {
    if index != 0 {
        return Err(StatusCode::NOT_FOUND);
    }
    image_response(state, headers, item_id, image_type, false).await
}

async fn indexed_head_primary(
    State(state): State<Arc<AppState>>,
    RoutePath((item_id, image_type, index)): RoutePath<(String, String, usize)>,
    headers: HeaderMap,
) -> Result<Response, StatusCode> {
    if index != 0 {
        return Err(StatusCode::NOT_FOUND);
    }
    image_response(state, headers, item_id, image_type, true).await
}

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/Items/{item_id}/Images", get(image_infos))
        .route(
            "/Items/{item_id}/Images/{image_type}",
            get(primary).head(head_primary),
        )
        .route(
            "/Items/{item_id}/Images/{image_type}/{index}",
            get(indexed_primary).head(indexed_head_primary),
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
    fn media_sidecars_are_hidden_without_hiding_independent_photos() {
        let entries = vec![
            entry("/media", "Film.mkv", "video"),
            entry("/media", "Film.jpg", "image"),
            entry("/media", "Film-poster.webp", "image"),
            entry("/media", "folder.jpg", "image"),
            entry("/media", "Portrait.jpg", "image"),
            entry("/media", "Album/Song.flac", "audio"),
            entry("/media", "Album/cover.png", "image"),
            entry("/photos", "cover.png", "image"),
            entry("/photos", "Portrait.jpg", "image"),
        ];
        let hidden = sidecar_keys(&entries);
        assert!(hidden.contains(&("/media".to_owned(), "Film.jpg".to_owned())));
        assert!(hidden.contains(&("/media".to_owned(), "Film-poster.webp".to_owned())));
        assert!(hidden.contains(&("/media".to_owned(), "folder.jpg".to_owned())));
        assert!(hidden.contains(&("/media".to_owned(), "Album/cover.png".to_owned())));
        assert!(!hidden.contains(&("/media".to_owned(), "Portrait.jpg".to_owned())));
        assert!(!hidden.contains(&("/photos".to_owned(), "cover.png".to_owned())));
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
