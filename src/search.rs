//! Pinned Jellyfin v12.3 /Search/Hints shape for authorized local media.
//! Deliberately partial: only indexed file/folder name hints are supported.
use std::sync::Arc;

use axum::{
    Json, Router,
    extract::{Query, State},
    http::{HeaderMap, StatusCode},
    routing::get,
};
use serde::{Deserialize, Serialize};

use crate::{
    app::AppState,
    auth_api, catalog_api,
    query_filters::{filter_types, media_types},
};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct HintQuery {
    search_term: String,
    start_index: Option<usize>,
    limit: Option<usize>,
    user_id: Option<String>,
    include_item_types: Option<String>,
    exclude_item_types: Option<String>,
    media_types: Option<String>,
    parent_id: Option<String>,
    include_media: Option<bool>,
    is_movie: Option<bool>,
    is_series: Option<bool>,
    is_news: Option<bool>,
    is_kids: Option<bool>,
    is_sports: Option<bool>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
struct SearchHint {
    id: String,
    item_id: String,
    name: String,
    #[serde(rename = "Type")]
    item_type: String,
    media_type: String,
    matched_term: String,
    is_folder: bool,
    artists: Vec<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
struct SearchHintResult {
    search_hints: Vec<SearchHint>,
    total_record_count: usize,
}

async fn hints(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<HintQuery>,
) -> Result<Json<SearchHintResult>, StatusCode> {
    let (user, _) = auth_api::authenticated(state.clone(), headers.clone()).await?;
    if query.search_term.trim().is_empty() || query.search_term.len() > 256 {
        return Err(StatusCode::BAD_REQUEST);
    }
    if query.user_id.as_deref().is_some_and(|id| id != user.id)
        || query.is_movie.is_some()
        || query.is_series.is_some()
        || query.is_news.is_some()
        || query.is_kids.is_some()
        || query.is_sports.is_some()
    {
        // No cross-user or unsupported movie/series classifier claims.
        return Err(StatusCode::NOT_IMPLEMENTED);
    }
    let included = filter_types(query.include_item_types.as_deref())?;
    let excluded = filter_types(query.exclude_item_types.as_deref())?;
    let media = media_types(query.media_types.as_deref())?;
    let mut nodes = catalog_api::accessible_nodes(state, headers).await?;
    if let Some(parent) = query.parent_id.as_deref() {
        if !nodes.iter().any(|item| item.id == parent && item.is_folder) {
            return Err(StatusCode::NOT_FOUND);
        }
        nodes.retain(|item| item.parent_id.as_deref() == Some(parent));
    }
    if query.include_media == Some(false) {
        nodes.clear();
    }
    let term = query.search_term.trim().to_lowercase();
    nodes.retain(|item| {
        item.parent_id.is_some()
            && item.name.to_lowercase().contains(&term)
            && included
                .as_ref()
                .is_none_or(|kinds| kinds.contains(&item.item_type))
            && excluded
                .as_ref()
                .is_none_or(|kinds| !kinds.contains(&item.item_type))
            && media
                .as_ref()
                .is_none_or(|types| item.media_type.as_ref().is_some_and(|k| types.contains(k)))
    });
    nodes.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.id.cmp(&b.id)));
    let total_record_count = nodes.len();
    let start = query.start_index.unwrap_or(0);
    let limit = query.limit.unwrap_or(100).min(200);
    let search_hints = nodes
        .into_iter()
        .skip(start)
        .take(limit)
        .map(|item| SearchHint {
            id: item.id.clone(),
            item_id: item.id,
            name: item.name,
            item_type: item.item_type,
            media_type: item.media_type.unwrap_or_else(|| "Unknown".to_owned()),
            matched_term: query.search_term.clone(),
            is_folder: item.is_folder,
            artists: Vec::new(),
        })
        .collect();
    Ok(Json(SearchHintResult {
        search_hints,
        total_record_count,
    }))
}

pub fn router() -> Router<Arc<AppState>> {
    Router::new().route("/Search/Hints", get(hints))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_unsynthesized_type_filters() {
        assert_eq!(
            filter_types(Some("Book")).unwrap_err(),
            StatusCode::NOT_IMPLEMENTED
        );
        assert!(filter_types(Some("Video,Audio,Movie")).unwrap().is_some());
        assert_eq!(
            media_types(Some("Unknown")).unwrap_err(),
            StatusCode::NOT_IMPLEMENTED
        );
    }
}
