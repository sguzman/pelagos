//! Authenticated Jellyfin v12.3 theme-media response subset.
//! Recognize only pinned local extra naming rules in already-authorized item folders;
//! inherited themes, arbitrary metadata providers and sorting are not modeled.
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
    fn new(owner_id: String, items: Vec<catalog_api::ItemDto>) -> Self {
        let total_record_count = items.len();
        Self {
            items,
            total_record_count,
            start_index: 0,
            owner_id,
        }
    }

    fn empty(owner_id: String) -> Self {
        Self::new(owner_id, Vec::new())
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

/// Return direct extras only. Pinned NamingOptions recognizes "theme" audio
/// filenames, audio under "theme-music", and video under "backdrops".
/// Never attach a library-root theme to unrelated sibling media.
async fn indexed_themes(
    state: Arc<AppState>,
    item_id: String,
    headers: HeaderMap,
    query: ThemeQuery,
) -> Result<(ThemeMediaResult, ThemeMediaResult), StatusCode> {
    let (user, _) = auth_api::authenticated(state.clone(), headers.clone()).await?;
    if query.user_id.as_deref().is_some_and(|id| id != user.id) {
        return Err(StatusCode::FORBIDDEN);
    }
    let nodes = catalog_api::accessible_nodes(state, headers).await?;
    let owner = nodes
        .iter()
        .find(|node| node.id == item_id)
        .ok_or(StatusCode::NOT_FOUND)?;
    // When Jellyfin finds no local theme, inheritFromParent walks up the
    // hierarchy and changes OwnerId. Refuse until full traversal is modeled.
    if query.inherit_from_parent == Some(true)
        || query.sort_by.is_some()
        || query.sort_order.is_some()
    {
        return Err(StatusCode::NOT_IMPLEMENTED);
    }
    let container = if owner.is_folder
        && matches!(owner.item_type.as_str(), "Folder" | "Series")
        && owner.parent_id.is_some()
    {
        Some(owner.id.as_str())
    } else if owner.item_type == "Movie" {
        owner.parent_id.as_deref().filter(|parent| {
            nodes.iter().any(|item| item.id == *parent && item.item_type == "Folder")
                && nodes
                    .iter()
                    .filter(|item| {
                        item.parent_id.as_deref() == Some(*parent)
                            && item.media_type.as_deref() == Some("Video")
                    })
                    .count()
                    == 1
        })
    } else {
        None
    };
    let mut songs = Vec::new();
    let mut videos = Vec::new();
    if let Some(container) = container {
        let theme_music = nodes
            .iter()
            .find(|item| {
                item.is_folder
                    && item.name.eq_ignore_ascii_case("theme-music")
                    && item.parent_id.as_deref() == Some(container)
            })
            .map(|item| item.id.as_str());
        let backdrops = nodes
            .iter()
            .find(|item| {
                item.is_folder
                    && item.name.eq_ignore_ascii_case("backdrops")
                    && item.parent_id.as_deref() == Some(container)
            })
            .map(|item| item.id.as_str());
        for item in &nodes {
            let mut extra = item.clone();
            if item.media_type.as_deref() == Some("Audio")
                && ((item.parent_id.as_deref() == Some(container)
                    && item.name.eq_ignore_ascii_case("theme"))
                    || item.parent_id.as_deref() == theme_music)
            {
                extra.extra_type = Some("ThemeSong".to_owned());
                songs.push(extra);
            } else if item.media_type.as_deref() == Some("Video")
                && item.parent_id.as_deref() == backdrops
            {
                extra.extra_type = Some("ThemeVideo".to_owned());
                videos.push(extra);
            }
        }
    }
    let sort = |a: &catalog_api::ItemDto, b: &catalog_api::ItemDto| {
        a.name
            .to_lowercase()
            .cmp(&b.name.to_lowercase())
            .then_with(|| a.id.cmp(&b.id))
    };
    songs.sort_by(sort);
    videos.sort_by(sort);
    Ok((
        ThemeMediaResult::new(item_id.clone(), songs),
        ThemeMediaResult::new(item_id, videos),
    ))
}

async fn theme_songs(
    State(state): State<Arc<AppState>>,
    RoutePath(item_id): RoutePath<String>,
    headers: HeaderMap,
    Query(query): Query<ThemeQuery>,
) -> Result<Json<ThemeMediaResult>, StatusCode> {
    let (songs, _) = indexed_themes(state, item_id, headers, query).await?;
    Ok(Json(songs))
}

async fn theme_videos(
    State(state): State<Arc<AppState>>,
    RoutePath(item_id): RoutePath<String>,
    headers: HeaderMap,
    Query(query): Query<ThemeQuery>,
) -> Result<Json<ThemeMediaResult>, StatusCode> {
    let (_, videos) = indexed_themes(state, item_id, headers, query).await?;
    Ok(Json(videos))
}

async fn theme_media(
    State(state): State<Arc<AppState>>,
    RoutePath(item_id): RoutePath<String>,
    headers: HeaderMap,
    Query(query): Query<ThemeQuery>,
) -> Result<Json<AllThemeMediaResult>, StatusCode> {
    let (songs, videos) = indexed_themes(state, item_id, headers, query).await?;
    Ok(Json(AllThemeMediaResult {
        theme_songs_result: songs,
        theme_videos_result: videos,
        // Pinned Jellyfin GetThemeMedia constructs a default, unowned
        // ThemeMediaResult here, not a third lookup for soundtrack tracks.
        soundtrack_songs_result: ThemeMediaResult::empty(
            "00000000-0000-0000-0000-000000000000".to_owned(),
        ),
    }))
}
