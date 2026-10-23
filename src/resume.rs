//! Pinned Jellyfin v12.3 resumable item routes (partial).
//! Resume inclusion is based on Pelagos-owned saved positive playback ticks.
//! Session exclusion, external media variants and full metadata semantics are
//! deliberately not claimed.
use std::{collections::HashMap, sync::Arc};

use axum::{
    Json, Router,
    extract::{Path as RoutePath, Query, State},
    http::{HeaderMap, StatusCode},
    routing::get,
};
use serde::{Deserialize, Serialize};

use crate::{
    app::AppState,
    auth_api, catalog_api,
    query_filters::{filter_types, media_types},
    user_data,
};

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ResumeQuery {
    user_id: Option<String>,
    start_index: Option<usize>,
    limit: Option<usize>,
    search_term: Option<String>,
    parent_id: Option<String>,
    media_types: Option<String>,
    include_item_types: Option<String>,
    exclude_item_types: Option<String>,
    enable_images: Option<bool>,
    enable_user_data: Option<bool>,
    enable_total_record_count: Option<bool>,
    exclude_active_sessions: Option<bool>,
    fields: Option<String>,
    image_type_limit: Option<usize>,
    enable_image_types: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "PascalCase")]
struct ResumeResult {
    items: Vec<catalog_api::ItemDto>,
    total_record_count: usize,
    start_index: usize,
}

fn under_parent(
    item: &catalog_api::ItemDto,
    parent: &str,
    parents: &HashMap<String, Option<String>>,
) -> bool {
    let mut current = item.parent_id.as_deref();
    for _ in 0..1024 {
        match current {
            Some(candidate) if candidate == parent => return true,
            Some(candidate) => current = parents.get(candidate).and_then(|id| id.as_deref()),
            None => return false,
        }
    }
    false
}

async fn list_resume(
    state: Arc<AppState>,
    headers: HeaderMap,
    query: ResumeQuery,
    path_user: Option<String>,
) -> Result<Json<ResumeResult>, StatusCode> {
    let (user, _) = auth_api::authenticated(state.clone(), headers.clone()).await?;
    if query.user_id.as_deref().is_some_and(|id| id != user.id)
        || path_user.as_deref().is_some_and(|id| id != user.id)
    {
        return Err(StatusCode::FORBIDDEN);
    }
    if query.exclude_active_sessions == Some(true)
        || query.enable_total_record_count == Some(false)
        || query.fields.is_some()
        || query.image_type_limit.is_some()
        || query.enable_image_types.is_some()
    {
        return Err(StatusCode::NOT_IMPLEMENTED);
    }
    let included = filter_types(query.include_item_types.as_deref())?;
    let excluded = filter_types(query.exclude_item_types.as_deref())?;
    let media = media_types(query.media_types.as_deref())?;
    let mut nodes = catalog_api::accessible_nodes(state.clone(), headers).await?;
    if let Some(parent) = query.parent_id.as_deref() {
        if !nodes.iter().any(|item| item.id == parent && item.is_folder) {
            return Err(StatusCode::NOT_FOUND);
        }
        let parents: HashMap<String, Option<String>> = nodes
            .iter()
            .map(|item| (item.id.clone(), item.parent_id.clone()))
            .collect();
        nodes.retain(|item| under_parent(item, parent, &parents));
    }
    nodes.retain(|item| {
        !item.is_folder
            && matches!(item.media_type.as_deref(), Some("Video" | "Audio"))
            && included
                .as_ref()
                .is_none_or(|types| types.contains(&item.item_type))
            && excluded
                .as_ref()
                .is_none_or(|types| !types.contains(&item.item_type))
            && media.as_ref().is_none_or(|types| {
                item.media_type
                    .as_ref()
                    .is_some_and(|kind| types.contains(kind))
            })
    });
    if let Some(term) = query.search_term.as_deref() {
        let needle = term.to_lowercase();
        nodes.retain(|item| item.name.to_lowercase().contains(&needle));
    }
    let ids: Vec<String> = nodes.iter().map(|item| item.id.clone()).collect();
    let dir = state.auth_dir().ok_or(StatusCode::SERVICE_UNAVAILABLE)?;
    let user_id = user.id;
    let saved = tokio::task::spawn_blocking(move || user_data::read_many(&dir, &user_id, &ids))
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let mut resumable = nodes
        .into_iter()
        .zip(saved)
        .filter_map(|(mut item, data)| {
            if data.played || data.playback_position_ticks <= 0 {
                return None;
            }
            item.user_data = Some(data);
            if query.enable_images == Some(false) {
                item.image_tags = None;
            }
            Some(item)
        })
        .collect::<Vec<_>>();
    resumable.sort_by(|a, b| {
        b.user_data
            .as_ref()
            .and_then(|data| data.last_played_date.as_ref())
            .cmp(
                &a.user_data
                    .as_ref()
                    .and_then(|data| data.last_played_date.as_ref()),
            )
            .then_with(|| a.name.cmp(&b.name))
            .then_with(|| a.id.cmp(&b.id))
    });
    if query.enable_user_data == Some(false) {
        for item in &mut resumable {
            item.user_data = None;
        }
    }
    let total_record_count = resumable.len();
    let start_index = query.start_index.unwrap_or(0);
    let limit = query.limit.unwrap_or(100).min(200);
    Ok(Json(ResumeResult {
        items: resumable
            .into_iter()
            .skip(start_index)
            .take(limit)
            .collect(),
        total_record_count,
        start_index,
    }))
}

async fn resume(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<ResumeQuery>,
) -> Result<Json<ResumeResult>, StatusCode> {
    list_resume(state, headers, query, None).await
}

async fn resume_legacy(
    State(state): State<Arc<AppState>>,
    RoutePath(user_id): RoutePath<String>,
    headers: HeaderMap,
    Query(query): Query<ResumeQuery>,
) -> Result<Json<ResumeResult>, StatusCode> {
    list_resume(state, headers, query, Some(user_id)).await
}

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/UserItems/Resume", get(resume))
        .route("/Users/{user_id}/Items/Resume", get(resume_legacy))
}
