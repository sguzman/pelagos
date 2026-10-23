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

    if let Some(start_item_id) = query.start_item_id.as_deref() {
        if !episodes {
            return Err(StatusCode::NOT_IMPLEMENTED);
        }
        filtered = filtered
            .into_iter()
            .skip_while(|item| item.id != start_item_id)
            .collect();
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

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct NextUpQuery {
    user_id: Option<String>,
    series_id: Option<String>,
    parent_id: Option<String>,
    start_index: Option<usize>,
    limit: Option<usize>,
    fields: Option<String>,
    enable_images: Option<bool>,
    enable_user_data: Option<bool>,
    image_type_limit: Option<usize>,
    enable_image_types: Option<String>,
    next_up_date_cutoff: Option<String>,
    enable_total_record_count: Option<bool>,
    enable_resumable: Option<bool>,
    enable_rewatching: Option<bool>,
}

/// Conservative pinned Jellyfin v12.3 Shows/NextUp subset.
/// Without seriesId, only watched series with a recorded play timestamp enter
/// the suggestions, ordered by recent playback. Only locally recognized
/// season-indexed episodes participate; rewatching, specials, alternate
/// editions and provider metadata are not modeled.
async fn next_up(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<NextUpQuery>,
) -> Result<Json<ShowResult>, StatusCode> {
    let (user, _) = auth_api::authenticated(state.clone(), headers.clone()).await?;
    if query.user_id.as_deref().is_some_and(|id| id != user.id) {
        return Err(StatusCode::FORBIDDEN);
    }
    if query.parent_id.is_some()
        || query.fields.is_some()
        || query.image_type_limit.is_some()
        || query.enable_image_types.is_some()
        || query.next_up_date_cutoff.is_some()
        || query.enable_total_record_count == Some(false)
        || query.enable_rewatching == Some(true)
        || query.limit == Some(0)
    {
        return Err(StatusCode::NOT_IMPLEMENTED);
    }
    let series_id = query.series_id.as_deref();
    let nodes = catalog_api::accessible_nodes(state.clone(), headers).await?;
    if let Some(series_id) = series_id {
        if !nodes
            .iter()
            .any(|item| item.id == series_id && item.item_type == "Series")
        {
            return Err(StatusCode::NOT_FOUND);
        }
    }
    let mut episodes = nodes
        .into_iter()
        .filter(|item| {
            item.item_type == "Episode"
                && item.series_id.as_deref().is_some_and(|id| {
                    series_id.is_none_or(|selected| selected == id)
                })
                && item.parent_index_number.is_some_and(|season| season > 0)
                && item.index_number.is_some()
        })
        .collect::<Vec<_>>();
    episodes.sort_by(|a, b| {
        a.parent_index_number
            .cmp(&b.parent_index_number)
            .then_with(|| a.index_number.cmp(&b.index_number))
            .then_with(|| a.id.cmp(&b.id))
    });
    let ids = episodes
        .iter()
        .map(|item| item.id.clone())
        .collect::<Vec<_>>();
    let dir = state.auth_dir().ok_or(StatusCode::SERVICE_UNAVAILABLE)?;
    let user_id = user.id;
    let saved = tokio::task::spawn_blocking(move || user_data::read_many(&dir, &user_id, &ids))
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let mut groups = std::collections::BTreeMap::<
        String,
        Vec<(catalog_api::ItemDto, user_data::UserItemData)>,
    >::new();
    for (item, data) in episodes.into_iter().zip(saved) {
        if let Some(id) = item.series_id.as_ref() {
            groups.entry(id.clone()).or_default().push((item, data));
        }
    }
    let mut suggestions = Vec::new();
    for (_, group) in groups {
        let last_played_date = group
            .iter()
            .filter(|(_, data)| data.played)
            .filter_map(|(_, data)| data.last_played_date.clone())
            .max();
        // Jellyfin's unscoped NextUp series selection is based on series
        // with playback history. Explicit seriesId is allowed to start from
        // the first episode even without a watched episode.
        if series_id.is_none() && last_played_date.is_none() {
            continue;
        }
        let highest_watched = group
            .iter()
            .filter(|(_, data)| data.played)
            .map(|(item, _)| {
                (
                    item.parent_index_number.unwrap_or(0),
                    item.index_number.unwrap_or(0),
                )
            })
            .max();
        let next = group.into_iter().find(|(item, data)| {
            !data.played
                && highest_watched.is_none_or(|position| {
                    (
                        item.parent_index_number.unwrap_or(0),
                        item.index_number.unwrap_or(0),
                    ) > position
                })
        });
        if let Some((mut item, data)) = next {
            if query.enable_resumable == Some(false) && data.playback_position_ticks > 0 {
                continue;
            }
            if query.enable_user_data != Some(false) {
                item.user_data = Some(data);
            }
            if query.enable_images == Some(false) {
                item.image_tags = None;
            }
            suggestions.push((last_played_date, item));
        }
    }
    suggestions.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then_with(|| a.1.series_id.cmp(&b.1.series_id))
            .then_with(|| a.1.id.cmp(&b.1.id))
    });
    let total_record_count = suggestions.len();
    let start_index = query.start_index.unwrap_or(0);
    let limit = query.limit.unwrap_or(100).min(200);
    let items = suggestions
        .into_iter()
        .skip(start_index)
        .take(limit)
        .map(|(_, item)| item)
        .collect();
    Ok(Json(ShowResult {
        items,
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
        .route("/Shows/NextUp", get(next_up))
        .route("/Shows/{series_id}/Seasons", get(seasons))
        .route("/Shows/{series_id}/Episodes", get(episodes))
}
