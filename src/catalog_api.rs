//! Read-only Jellyfin-shaped item queries from Pelagos' private SQLite catalog.
//! This is not yet full BaseItemDto or Jellyfin's library hierarchy.
use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::Arc,
};

use axum::{
    Json, Router,
    extract::{Path as RoutePath, Query, State},
    http::{HeaderMap, StatusCode},
    routing::get,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::{app::AppState, auth_api, catalog};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct ItemDto {
    pub id: String,
    pub name: String,
    #[serde(rename = "Type")]
    pub item_type: String,
    pub media_type: Option<String>,
    pub is_folder: bool,
    pub server_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<i64>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
struct QueryResult {
    items: Vec<ItemDto>,
    total_record_count: usize,
    start_index: usize,
}

#[derive(Default, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ItemQuery {
    start_index: Option<usize>,
    limit: Option<usize>,
    search_term: Option<String>,
    parent_id: Option<String>,
    recursive: Option<bool>,
}

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/Items", get(items))
        .route("/Items/{item_id}", get(item))
        .route("/Users/{user_id}/Items", get(user_items))
        .route("/UserViews", get(user_views))
        .route("/Users/{user_id}/Views", get(user_views_legacy))
}

pub(crate) fn stable_id(root: &str, name: &str) -> String {
    let mut digest = Sha256::new();
    digest.update(root.as_bytes());
    digest.update([0]);
    digest.update(name.as_bytes());
    let mut bytes: [u8; 32] = digest.finalize().into();
    bytes[6] = (bytes[6] & 0x0f) | 0x50;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Uuid::from_slice(&bytes[..16])
        .expect("SHA-256 output is at least 16 bytes")
        .simple()
        .to_string()
}

fn map_item(file: catalog::CatalogFile, server_id: &str, parent_id: String) -> ItemDto {
    let name = Path::new(&file.relative_path)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or(file.relative_path.clone());
    let (item_type, media_type) = match file.kind.as_str() {
        "video" => ("Video", "Video"),
        "audio" => ("Audio", "Audio"),
        "image" => ("Photo", "Photo"),
        _ => ("Unknown", "Unknown"),
    };
    ItemDto {
        id: stable_id(&file.root, &file.relative_path),
        name,
        item_type: item_type.to_owned(),
        media_type: Some(media_type.to_owned()),
        is_folder: false,
        server_id: server_id.to_owned(),
        parent_id: Some(parent_id),
        size: Some(file.size_bytes),
    }
}

fn root_item(root: &str, server_id: &str) -> ItemDto {
    ItemDto {
        id: stable_id(root, ""),
        name: Path::new(root)
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Library".to_owned()),
        item_type: "CollectionFolder".to_owned(),
        media_type: None,
        is_folder: true,
        server_id: server_id.to_owned(),
        parent_id: None,
        size: None,
    }
}

/// Build a virtual directory hierarchy from indexed filenames. No filesystem writes,
/// metadata guesses, or symlink traversal occur here.
fn catalog_nodes(
    files: Vec<catalog::CatalogFile>,
    roots: Vec<String>,
    server_id: &str,
) -> Vec<ItemDto> {
    let mut result = Vec::new();
    let mut seen = HashSet::new();
    for root in roots {
        let root_id = stable_id(&root, "");
        if seen.insert(root_id) {
            result.push(root_item(&root, server_id));
        }
    }
    for file in files {
        let root = file.root.clone();
        let root_id = stable_id(&root, "");
        if seen.insert(root_id.clone()) {
            result.push(root_item(&root, server_id));
        }
        let mut parent_id = root_id;
        let mut folder = PathBuf::new();
        if let Some(directory) = Path::new(&file.relative_path).parent() {
            for component in directory.components() {
                folder.push(component.as_os_str());
                let path = folder.to_string_lossy();
                let folder_id = stable_id(&root, &format!("folder:{path}"));
                if seen.insert(folder_id.clone()) {
                    result.push(ItemDto {
                        id: folder_id.clone(),
                        name: component.as_os_str().to_string_lossy().into_owned(),
                        item_type: "Folder".to_owned(),
                        media_type: None,
                        is_folder: true,
                        server_id: server_id.to_owned(),
                        parent_id: Some(parent_id.clone()),
                        size: None,
                    });
                }
                parent_id = folder_id;
            }
        }
        result.push(map_item(file, server_id, parent_id));
    }
    result
}

fn is_descendant(id: &str, parent: &str, parents: &HashMap<String, Option<String>>) -> bool {
    let mut current = parents.get(id).and_then(|p| p.as_deref());
    for _ in 0..1024 {
        match current {
            Some(candidate) if candidate == parent => return true,
            Some(candidate) => current = parents.get(candidate).and_then(|p| p.as_deref()),
            None => return false,
        }
    }
    false
}

/// One shared permission boundary for item browsing and Jellyfin search.
/// No item may become discoverable merely because a different API calls it.
pub(crate) async fn accessible_nodes(
    state: Arc<AppState>,
    headers: HeaderMap,
) -> Result<Vec<ItemDto>, StatusCode> {
    let (state_dir, allowed) = auth_api::library_scope(state.clone(), headers).await?;
    let server_id = state.server_id().to_owned();
    let (mut files, mut roots) = tokio::task::spawn_blocking(move || {
        let files = catalog::all_files(&state_dir)?;
        let roots = catalog::library_roots(&state_dir)?;
        Ok::<_, Box<dyn std::error::Error + Send + Sync>>((files, roots))
    })
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    if let Some(granted) = allowed {
        files.retain(|file| granted.contains(&file.root));
        roots.retain(|root| granted.contains(root));
    }
    Ok(catalog_nodes(files, roots, &server_id))
}

async fn query_items(
    state: Arc<AppState>,
    headers: HeaderMap,
    query: ItemQuery,
) -> Result<Json<QueryResult>, StatusCode> {
    let mut all = accessible_nodes(state, headers).await?;
    match query.parent_id.as_deref() {
        Some(parent) => {
            let parents: HashMap<String, Option<String>> = all
                .iter()
                .map(|item| (item.id.clone(), item.parent_id.clone()))
                .collect();
            if !all.iter().any(|item| item.id == parent && item.is_folder) {
                return Err(StatusCode::NOT_FOUND);
            }
            let recursive = query.recursive.unwrap_or(false);
            all.retain(|item| {
                if recursive {
                    !item.is_folder && is_descendant(&item.id, parent, &parents)
                } else {
                    item.parent_id.as_deref() == Some(parent)
                }
            });
        }
        None => all.retain(|item| !item.is_folder),
    }
    if let Some(search) = query.search_term.filter(|s| !s.is_empty()) {
        let search = search.to_lowercase();
        all.retain(|item| item.name.to_lowercase().contains(&search));
    }
    all.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.id.cmp(&b.id)));
    let total_record_count = all.len();
    let start_index = query.start_index.unwrap_or(0);
    let limit = query.limit.unwrap_or(100).min(200);
    let page = all.into_iter().skip(start_index).take(limit).collect();
    Ok(Json(QueryResult {
        items: page,
        total_record_count,
        start_index,
    }))
}

async fn items(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<ItemQuery>,
) -> Result<Json<QueryResult>, StatusCode> {
    query_items(state, headers, query).await
}

async fn user_items(
    State(state): State<Arc<AppState>>,
    RoutePath(user_id): RoutePath<String>,
    headers: HeaderMap,
    Query(query): Query<ItemQuery>,
) -> Result<Json<QueryResult>, StatusCode> {
    let (user, _) = auth_api::authenticated(state.clone(), headers.clone()).await?;
    if user.id != user_id {
        return Err(StatusCode::FORBIDDEN);
    }
    query_items(state, headers, query).await
}

async fn item(
    State(state): State<Arc<AppState>>,
    RoutePath(item_id): RoutePath<String>,
    headers: HeaderMap,
) -> Result<Json<ItemDto>, StatusCode> {
    accessible_nodes(state, headers)
        .await?
        .into_iter()
        .find(|item| item.id == item_id)
        .map(Json)
        .ok_or(StatusCode::NOT_FOUND)
}

async fn user_views(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Json<QueryResult>, StatusCode> {
    let (dir, allowed) = auth_api::library_scope(state.clone(), headers).await?;
    let mut roots = tokio::task::spawn_blocking(move || catalog::library_roots(&dir))
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    if let Some(granted) = allowed {
        roots.retain(|root| granted.contains(root));
    }
    let items = roots
        .into_iter()
        .map(|root| ItemDto {
            id: stable_id(&root, ""),
            name: Path::new(&root)
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| "Library".to_owned()),
            item_type: "CollectionFolder".to_owned(),
            media_type: None,
            is_folder: true,
            server_id: state.server_id().to_owned(),
            parent_id: None,
            size: None,
        })
        .collect::<Vec<_>>();
    Ok(Json(QueryResult {
        total_record_count: items.len(),
        items,
        start_index: 0,
    }))
}

async fn user_views_legacy(
    State(state): State<Arc<AppState>>,
    RoutePath(user_id): RoutePath<String>,
    headers: HeaderMap,
) -> Result<Json<QueryResult>, StatusCode> {
    let (user, _) = auth_api::authenticated(state.clone(), headers.clone()).await?;
    if user.id != user_id {
        return Err(StatusCode::FORBIDDEN);
    }
    user_views(State(state), headers).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_folder_hierarchy_is_stable_and_selectable() {
        let files = vec![
            catalog::CatalogFile {
                root: "/media/movies".to_owned(),
                relative_path: "Series/Season 1/Episode.mkv".to_owned(),
                kind: "video".to_owned(),
                size_bytes: 7,
            },
            catalog::CatalogFile {
                root: "/media/movies".to_owned(),
                relative_path: "Standalone.mp4".to_owned(),
                kind: "video".to_owned(),
                size_bytes: 5,
            },
        ];
        let tree = catalog_nodes(files, vec!["/media/movies".to_owned()], "server");
        assert_eq!(tree.len(), 5);
        let root = tree
            .iter()
            .find(|item| item.item_type == "CollectionFolder")
            .unwrap();
        let series = tree.iter().find(|item| item.name == "Series").unwrap();
        let episode = tree.iter().find(|item| item.name == "Episode").unwrap();
        assert_eq!(series.parent_id.as_deref(), Some(root.id.as_str()));
        assert!(is_descendant(
            &episode.id,
            &root.id,
            &tree
                .iter()
                .map(|item| (item.id.clone(), item.parent_id.clone()))
                .collect()
        ));
    }

    #[test]
    fn deterministic_path_id_with_no_filenames_leaked() {
        let first = stable_id("/media/movies", "A/movie.mp4");
        assert_eq!(first, stable_id("/media/movies", "A/movie.mp4"));
        assert_ne!(first, stable_id("/media/music", "A/movie.mp4"));
        assert_eq!(first.len(), 32);
    }
}
