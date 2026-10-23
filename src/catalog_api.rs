//! Read-only Jellyfin-shaped item queries from Pelagos' private SQLite catalog.
//! This is not yet full BaseItemDto or Jellyfin's library hierarchy.
use std::{path::Path, sync::Arc};

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
}

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/Items", get(items))
        .route("/Items/{item_id}", get(item))
        .route("/Users/{user_id}/Items", get(user_items))
        .route("/UserViews", get(user_views))
        .route("/Users/{user_id}/Views", get(user_views_legacy))
}

fn stable_id(root: &str, name: &str) -> String {
    let mut digest = Sha256::new();
    digest.update(root.as_bytes());
    digest.update([0]);
    digest.update(name.as_bytes());
    let mut bytes: [u8; 32] = digest.finalize().into();
    bytes[6] = (bytes[6] & 0x0f) | 0x50;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Uuid::from_slice(&bytes[..16]).expect("SHA-256 output is at least 16 bytes").simple().to_string()
}

fn map_item(file: catalog::CatalogFile, server_id: &str) -> ItemDto {
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
        size: Some(file.size_bytes),
    }
}

/// Until per-library policies exist, catalog access is administrator-only.
/// This intentionally does not claim ordinary client browsing support.
async fn admin_state(
    state: Arc<AppState>,
    headers: HeaderMap,
) -> Result<std::path::PathBuf, StatusCode> {
    let (user, _) = auth_api::authenticated(state.clone(), headers).await?;
    if !user.policy.is_administrator {
        return Err(StatusCode::FORBIDDEN);
    }
    state.auth_dir().ok_or(StatusCode::SERVICE_UNAVAILABLE)
}

async fn query_items(
    state: Arc<AppState>,
    headers: HeaderMap,
    query: ItemQuery,
) -> Result<Json<QueryResult>, StatusCode> {
    let state_dir = admin_state(state.clone(), headers).await?;
    let server_id = state.server_id().to_owned();
    let mut all = tokio::task::spawn_blocking(move || catalog::all_files(&state_dir))
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .into_iter()
        .map(|row| map_item(row, &server_id))
        .collect::<Vec<_>>();
    if let Some(search) = query.search_term.filter(|s| !s.is_empty()) {
        let search = search.to_lowercase();
        all.retain(|item| item.name.to_lowercase().contains(&search));
    }
    // The directory hierarchy is not yet implemented; reject rather than silently
    // returning unrelated items for a parent's children.
    if query.parent_id.is_some() {
        return Err(StatusCode::NOT_IMPLEMENTED);
    }
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
    let items = query_items(state, headers, ItemQuery::default()).await?;
    // Dedicated indexed ID lookup will replace this capped diagnostic route.
    items.0.items.into_iter().find(|item| item.id == item_id)
        .map(Json).ok_or(StatusCode::NOT_FOUND)
}

async fn user_views(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Json<QueryResult>, StatusCode> {
    let dir = admin_state(state.clone(), headers).await?;
    let roots = tokio::task::spawn_blocking(move || catalog::library_roots(&dir))
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let items = roots.into_iter().map(|root| ItemDto {
        id: stable_id(&root, ""),
        name: Path::new(&root).file_name().map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Library".to_owned()),
        item_type: "CollectionFolder".to_owned(),
        media_type: None,
        is_folder: true,
        server_id: state.server_id().to_owned(),
        size: None,
    }).collect::<Vec<_>>();
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
    fn deterministic_path_id_with_no_filenames_leaked() {
        let first = stable_id("/media/movies", "A/movie.mp4");
        assert_eq!(first, stable_id("/media/movies", "A/movie.mp4"));
        assert_ne!(first, stable_id("/media/music", "A/movie.mp4"));
        assert_eq!(first.len(), 32);
    }
}
