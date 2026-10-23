//! Authenticated Jellyfin v12.3 theme-media response subset.
//! The indexed catalog does not yet identify theme songs or theme videos.
//! Only direct, non-inherited empty membership can be represented honestly.
use std::sync::Arc;

use axum::{
    Json, Router,
    extract::{Path as RoutePath, Query, State},
    http::{HeaderMap, StatusCode},
    routing::get,
};
use serde::{Deserialize, Serialize};

use crate::{app::AppState, auth_api, catalog_api};

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ThemeQuery {
    user_id: Option<String>,
    inherit_from_parent: Option<bool>,
    sort_by: Option<String>,
    sort_order: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
struct ThemeMediaResult {
    items: Vec<catalog_api::ItemDto>,
    total_record_count: usize,
    start_index: usize,
    owner_id: String,
}

impl ThemeMediaResult {
    fn empty(owner_id: String) -> Self {
        Self {
            items: Vec::new(),
            total_record_count: 0,
            start_index: 0,
            owner_id,
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
struct AllThemeMediaResult {
    theme_songs_result: ThemeMediaResult,
    theme_videos_result: ThemeMediaResult,
    soundtrack_songs_result: ThemeMediaResult,
}

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/Items/{item_id}/ThemeSongs", get(theme_songs))
        .route("/Items/{item_id}/ThemeVideos", get(theme_videos))
        .route("/Items/{item_id}/ThemeMedia", get(theme_media))
}

async fn empty_theme(
    state: Arc<AppState>,
    item_id: String,
    headers: HeaderMap,
    query: ThemeQuery,
) -> Result<ThemeMediaResult, StatusCode> {
    let (user, _) = auth_api::authenticated(state.clone(), headers.clone()).await?;
    if query.user_id.as_deref().is_some_and(|id| id != user.id) {
        return Err(StatusCode::FORBIDDEN);
    }
    let exists = catalog_api::accessible_nodes(state, headers)
        .await?
        .into_iter()
        .any(|item| item.id == item_id);
    if !exists {
        return Err(StatusCode::NOT_FOUND);
    }
    // In pinned Jellyfin, an unsuccessful inherited-theme search walks up
    // the item tree and changes OwnerId. We must not claim an owner we did not
    // traverse. Likewise, sort projections await actual indexed theme items.
    if query.inherit_from_parent == Some(true)
        || query.sort_by.is_some()
        || query.sort_order.is_some()
    {
        return Err(StatusCode::NOT_IMPLEMENTED);
    }
    Ok(ThemeMediaResult::empty(item_id))
}

async fn theme_songs(
    State(state): State<Arc<AppState>>,
    RoutePath(item_id): RoutePath<String>,
    headers: HeaderMap,
    Query(query): Query<ThemeQuery>,
) -> Result<Json<ThemeMediaResult>, StatusCode> {
    Ok(Json(empty_theme(state, item_id, headers, query).await?))
}

async fn theme_videos(
    State(state): State<Arc<AppState>>,
    RoutePath(item_id): RoutePath<String>,
    headers: HeaderMap,
    Query(query): Query<ThemeQuery>,
) -> Result<Json<ThemeMediaResult>, StatusCode> {
    Ok(Json(empty_theme(state, item_id, headers, query).await?))
}

async fn theme_media(
    State(state): State<Arc<AppState>>,
    RoutePath(item_id): RoutePath<String>,
    headers: HeaderMap,
    Query(query): Query<ThemeQuery>,
) -> Result<Json<AllThemeMediaResult>, StatusCode> {
    let items = empty_theme(state, item_id, headers, query).await?;
    Ok(Json(AllThemeMediaResult {
        theme_songs_result: items.clone(),
        theme_videos_result: items,
        // Pinned Jellyfin GetThemeMedia constructs a default, unowned
        // ThemeMediaResult here, not a third lookup for soundtrack tracks.
        soundtrack_songs_result: ThemeMediaResult::empty(
            "00000000-0000-0000-0000-000000000000".to_owned(),
        ),
    }))
}
