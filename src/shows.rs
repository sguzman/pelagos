//! Permission-scoped, conservative Jellyfin v12.3 TV show navigation.
//! Only folders/episodes recognized by Pelagos' local filename parser are
//! available here; no remote/missing/virtual episodes or metadata guesses.
use std::sync::Arc;

use axum::{
    Json, Router,
    extract::{Path as RoutePath, Query, State},
    http::{HeaderMap, StatusCode},
    routing::get,
};
use serde::{Deserialize, Serialize};

use crate::{app::AppState, auth_api, catalog_api, user_data};

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ShowQuery {
    user_id: Option<String>,
    season: Option<u16>,
    season_id: Option<String>,
    is_special_season: Option<bool>,
    start_index: Option<usize>,
    limit: Option<usize>,
    sort_by: Option<String>,
    enable_images: Option<bool>,
    enable_user_data: Option<bool>,
    fields: Option<String>,
    image_type_limit: Option<usize>,
    enable_image_types: Option<String>,
    is_missing: Option<bool>,
    adjacent_to: Option<String>,
    start_item_id: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
struct ShowResult {
    items: Vec<catalog_api::ItemDto>,
    total_record_count: usize,
    start_index: usize,
}

fn validate_query(query: &ShowQuery, episodes: bool) -> Result<(), StatusCode> {
    if query.fields.is_some()
        || query.image_type_limit.is_some()
        || query.enable_image_types.is_some()
        || query.adjacent_to.is_some()
        || query.start_item_id.is_some()
        || query.is_missing.is_some()
        || query.sort_by.as_deref().is_some_and(|s| s != "SortName")
        || !episodes && (query.season.is_some() || query.season_id.is_some())
        || episodes && query.is_special_season.is_some()
    {
        return Err(StatusCode::NOT_IMPLEMENTED);
    }
    Ok(())
}

async fn show_items(
    state: Arc<AppState>,
    headers: HeaderMap,
    series_id: String,
    query: ShowQuery,
    episodes: bool,
) -> Result<Json<ShowResult>, StatusCode> {
    let (user, _) = auth_api::authenticated(state.clone(), headers.clone()).await?;
    if query.user_id.as_deref().is_some_and(|id| id != user.id) {
        return Err(StatusCode::FORBIDDEN);
    }
    validate_query(&query, episodes)?;
    let nodes = catalog_api::accessible_nodes(state.clone(), headers).await?;
    if !nodes
        .iter()
        .any(|item| item.id == series_id && item.item_type == "Series")
    {
        return Err(StatusCode::NOT_FOUND);
    }
    if let Some(season_id) = query.season_id.as_deref() {
        if !nodes.iter().any(|item| {
            item.id == season_id
                && item.item_type == "Season"
                && item.parent_id.as_deref() == Some(series_id.as_str())
        }) {
            return Err(StatusCode::NOT_FOUND);
        }
    }

    let mut filtered = nodes
        .into_iter()
        .filter(|item| {
            if episodes {
                item.item_type == "Episode"
                    && item.series_id.as_deref() == Some(series_id.as_str())
                    && query
                        .season
                        .is_none_or(|season| item.parent_index_number == Some(season))
                    && query
                        .season_id
                        .as_ref()
                        .is_none_or(|id| item.season_id.as_deref() == Some(id))
            } else {
                item.item_type == "Season"
                    && item.parent_id.as_deref() == Some(series_id.as_str())
                    && query
                        .is_special_season
                        .is_none_or(|special| (item.index_number == Some(0)) == special)
            }
        })
        .collect::<Vec<_>>();

    if query.sort_by.as_deref() == Some("SortName") {
        filtered.sort_by(|a, b| {
            a.name
                .to_lowercase()
                .cmp(&b.name.to_lowercase())
                .then_with(|| a.id.cmp(&b.id))
        });
    } else if episodes {
        filtered.sort_by(|a, b| {
            a.parent_index_number
                .cmp(&b.parent_index_number)
                .then_with(|| a.index_number.cmp(&b.index_number))
                .then_with(|| a.id.cmp(&b.id))
        });
    } else {
        filtered.sort_by(|a, b| {
            a.index_number
                .cmp(&b.index_number)
                .then_with(|| a.id.cmp(&b.id))
        });
    }

    let total_record_count = filtered.len();
    let start_index = query.start_index.unwrap_or(0);
    let limit = query.limit.unwrap_or(100).min(200);
    let mut page = filtered
        .into_iter()
        .skip(start_index)
        .take(limit)
        .collect::<Vec<_>>();
    if query.enable_images == Some(false) {
        for item in &mut page {
            item.image_tags = None;
        }
    }
    if query.enable_user_data != Some(false) && !page.is_empty() {
        let dir = state.auth_dir().ok_or(StatusCode::SERVICE_UNAVAILABLE)?;
        let ids = page.iter().map(|item| item.id.clone()).collect::<Vec<_>>();
        let user_id = user.id;
        let data = tokio::task::spawn_blocking(move || user_data::read_many(&dir, &user_id, &ids))
            .await
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        for (item, data) in page.iter_mut().zip(data) {
            item.user_data = Some(data);
        }
    }
    Ok(Json(ShowResult {
        items: page,
        total_record_count,
        start_index,
    }))
}

async fn seasons(
    State(state): State<Arc<AppState>>,
    RoutePath(series_id): RoutePath<String>,
    headers: HeaderMap,
    Query(query): Query<ShowQuery>,
) -> Result<Json<ShowResult>, StatusCode> {
    show_items(state, headers, series_id, query, false).await
}

async fn episodes(
    State(state): State<Arc<AppState>>,
    RoutePath(series_id): RoutePath<String>,
    headers: HeaderMap,
    Query(query): Query<ShowQuery>,
) -> Result<Json<ShowResult>, StatusCode> {
    show_items(state, headers, series_id, query, true).await
}

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/Shows/{series_id}/Seasons", get(seasons))
        .route("/Shows/{series_id}/Episodes", get(episodes))
}
