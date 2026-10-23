//! A bounded subset of Jellyfin v12.3 latest media discovery.
//! Date ordering is based on Pelagos' own first-indexed catalog timestamp.
//! The pinned TV Episode -> Series grouping is supported when explicitly
//! type-scoped; music/album grouping and advanced fields remain unsupported.
use std::{collections::HashMap, sync::Arc};

use axum::{
    Json, Router,
    extract::{Path as RoutePath, Query, State},
    http::{HeaderMap, StatusCode},
    routing::get,
};
use serde::Deserialize;

use crate::{
    app::AppState, auth_api, catalog, catalog_api, query_filters::filter_types, user_data,
};

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LatestQuery {
    user_id: Option<String>,
    parent_id: Option<String>,
    include_item_types: Option<String>,
    is_played: Option<bool>,
    enable_images: Option<bool>,
    enable_user_data: Option<bool>,
    image_type_limit: Option<usize>,
    enable_image_types: Option<String>,
    fields: Option<String>,
    limit: Option<usize>,
    group_items: Option<bool>,
}

fn descendant_of(
    item: &catalog_api::ItemDto,
    parent: &str,
    graph: &HashMap<String, Option<String>>,
) -> bool {
    let mut next = item.parent_id.as_deref();
    for _ in 0..1024 {
        match next {
            Some(id) if id == parent => return true,
            Some(id) => next = graph.get(id).and_then(|value| value.as_deref()),
            None => return false,
        }
    }
    false
}

async fn latest_for_user(
    state: Arc<AppState>,
    headers: HeaderMap,
    query: LatestQuery,
    path_user: Option<String>,
) -> Result<Json<Vec<catalog_api::ItemDto>>, StatusCode> {
    let (user, _) = auth_api::authenticated(state.clone(), headers.clone()).await?;
    if query.user_id.as_deref().is_some_and(|id| id != user.id)
        || path_user.as_deref().is_some_and(|id| id != user.id)
    {
        return Err(StatusCode::FORBIDDEN);
    }
    if query.fields.is_some()
        || query.image_type_limit.is_some()
        || query.enable_image_types.is_some()
    {
        return Err(StatusCode::NOT_IMPLEMENTED);
    }
    let included = filter_types(query.include_item_types.as_deref())?;
    // The pinned source groups episodes under their series, but audio/photo
    // require album entities Pelagos does not model yet. Only the explicit
    // Episode query can enable genuine grouping; all other grouped requests
    // refuse instead of inventing a generic grouping convention.
    let group_episodes = query.group_items != Some(false);
    if group_episodes
        && !included
            .as_ref()
            .is_some_and(|types| types.len() == 1 && types.contains("Episode"))
    {
        return Err(StatusCode::NOT_IMPLEMENTED);
    }
    let nodes = catalog_api::accessible_nodes(state.clone(), headers).await?;
    let series = if group_episodes {
        nodes
            .iter()
            .filter(|item| item.item_type == "Series")
            .map(|item| (item.id.clone(), item.clone()))
            .collect::<HashMap<_, _>>()
    } else {
        HashMap::new()
    };
    if let Some(parent) = query.parent_id.as_deref() {
        if !nodes.iter().any(|item| item.id == parent && item.is_folder) {
            return Err(StatusCode::NOT_FOUND);
        }
    }
    let parents = nodes
        .iter()
        .map(|node| (node.id.clone(), node.parent_id.clone()))
        .collect::<HashMap<_, _>>();
    let mut permitted = nodes
        .into_iter()
        .filter(|item| {
            !item.is_folder
                && matches!(
                    item.media_type.as_deref(),
                    Some("Video" | "Audio" | "Photo")
                )
                && included
                    .as_ref()
                    .is_none_or(|types| types.contains(&item.item_type))
                && query
                    .parent_id
                    .as_deref()
                    .is_none_or(|id| descendant_of(item, id, &parents))
        })
        .map(|item| (item.id.clone(), item))
        .collect::<HashMap<_, _>>();

    let dir = state.auth_dir().ok_or(StatusCode::SERVICE_UNAVAILABLE)?;
    let keys_dir = dir.clone();
    let ordered_keys = tokio::task::spawn_blocking(move || catalog::latest_file_keys(&keys_dir))
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let mut ordered = ordered_keys
        .into_iter()
        .filter_map(|(root, path)| permitted.remove(&catalog_api::stable_id(&root, &path)))
        .collect::<Vec<_>>();
    let ids = ordered
        .iter()
        .map(|item| item.id.clone())
        .collect::<Vec<_>>();
    let user_id = user.id.clone();
    let saved = tokio::task::spawn_blocking(move || user_data::read_many(&dir, &user_id, &ids))
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let limit = query.limit.unwrap_or(20).min(200);
    if limit == 0 {
        return Ok(Json(Vec::new()));
    }
    let mut result = Vec::<catalog_api::ItemDto>::new();
    let mut groups = HashMap::<String, usize>::new();
    for (item, data) in ordered.drain(..).zip(saved) {
        if query.is_played.is_some_and(|played| data.played != played) {
            continue;
        }
        if group_episodes {
            if let Some(series_id) = item.series_id.as_ref() {
                if let Some(series_item) = series.get(series_id) {
                    if let Some(&index) = groups.get(series_id) {
                        if result[index].item_type == "Episode" {
                            let mut container = series_item.clone();
                            container.child_count = Some(2);
                            result[index] = container;
                        } else {
                            let count = result[index].child_count.unwrap_or(1);
                            result[index].child_count = Some(count + 1);
                        }
                        continue;
                    }
                    groups.insert(series_id.clone(), result.len());
                }
            }
        }
        result.push(item);
    }
    // The pinned controller limits *groups*, not the source episode count.
    // Finish accumulating ChildCount before truncation, so a limit of one
    // still reports the accurate group size.
    result.truncate(limit);
    if query.enable_images == Some(false) {
        for item in &mut result {
            item.image_tags = None;
        }
    }
    if query.enable_user_data != Some(false) && !result.is_empty() {
        let ids = result
            .iter()
            .map(|item| item.id.clone())
            .collect::<Vec<_>>();
        let state_dir = state.auth_dir().ok_or(StatusCode::SERVICE_UNAVAILABLE)?;
        let user_id = user.id;
        let user_states =
            tokio::task::spawn_blocking(move || user_data::read_many(&state_dir, &user_id, &ids))
                .await
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        for (item, data) in result.iter_mut().zip(user_states) {
            item.user_data = Some(data);
        }
    }
    Ok(Json(result))
}

async fn latest(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<LatestQuery>,
) -> Result<Json<Vec<catalog_api::ItemDto>>, StatusCode> {
    latest_for_user(state, headers, query, None).await
}

async fn latest_legacy(
    State(state): State<Arc<AppState>>,
    RoutePath(user_id): RoutePath<String>,
    headers: HeaderMap,
    Query(query): Query<LatestQuery>,
) -> Result<Json<Vec<catalog_api::ItemDto>>, StatusCode> {
    latest_for_user(state, headers, query, Some(user_id)).await
}

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/Items/Latest", get(latest))
        .route("/Users/{user_id}/Items/Latest", get(latest_legacy))
}
