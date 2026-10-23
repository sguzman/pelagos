//! Initial Jellyfin v12.3 direct-play progress reporting.
//! These routes persist resume ticks, not a complete live session/WebSocket model.
use std::sync::Arc;

use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, StatusCode},
    routing::post,
};
use serde::Deserialize;
use serde_json::Value;

use crate::{app::AppState, auth_api, catalog_api, user_data};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct PlaybackReport {
    item_id: String,
    position_ticks: Option<i64>,
    play_method: Option<Value>,
    media_source_id: Option<String>,
    failed: Option<bool>,
}

#[derive(Clone, Copy)]
enum Event {
    Started,
    Progress,
    Stopped,
}

fn direct_play(method: Option<&Value>) -> bool {
    method.is_some_and(|value| value == "DirectPlay" || value == 2)
}

async fn record(
    state: Arc<AppState>,
    headers: HeaderMap,
    report: PlaybackReport,
    event: Event,
) -> Result<StatusCode, StatusCode> {
    let (user, _) = auth_api::authenticated(state.clone(), headers.clone()).await?;
    if !matches!(event, Event::Stopped) && !direct_play(report.play_method.as_ref()) {
        return Err(StatusCode::NOT_IMPLEMENTED);
    }
    if report.failed == Some(true)
        || report
            .media_source_id
            .as_deref()
            .is_some_and(|id| id != report.item_id)
    {
        return Err(StatusCode::NOT_IMPLEMENTED);
    }
    let ticks = match (event, report.position_ticks) {
        (Event::Started, None) => 0,
        (_, Some(ticks)) if ticks >= 0 => ticks,
        _ => return Err(StatusCode::BAD_REQUEST),
    };
    if !catalog_api::accessible_nodes(state.clone(), headers)
        .await?
        .iter()
        .any(|item| {
            item.id == report.item_id
                && !item.is_folder
                && matches!(item.media_type.as_deref(), Some("Video" | "Audio"))
        })
    {
        return Err(StatusCode::NOT_FOUND);
    }
    let dir = state.auth_dir().ok_or(StatusCode::SERVICE_UNAVAILABLE)?;
    tokio::task::spawn_blocking(move || {
        user_data::save_position(&dir, &user.id, &report.item_id, ticks)
    })
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn start(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(report): Json<PlaybackReport>,
) -> Result<StatusCode, StatusCode> {
    record(state, headers, report, Event::Started).await
}

async fn progress(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(report): Json<PlaybackReport>,
) -> Result<StatusCode, StatusCode> {
    record(state, headers, report, Event::Progress).await
}

async fn stopped(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(report): Json<PlaybackReport>,
) -> Result<StatusCode, StatusCode> {
    record(state, headers, report, Event::Stopped).await
}

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/Sessions/Playing", post(start))
        .route("/Sessions/Playing/Progress", post(progress))
        .route("/Sessions/Playing/Stopped", post(stopped))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn only_direct_play_is_accepted_in_partial_playback_handler() {
        assert!(direct_play(Some(&json!("DirectPlay"))));
        assert!(direct_play(Some(&json!(2))));
        assert!(!direct_play(Some(&json!("DirectStream"))));
        assert!(!direct_play(Some(&json!(0))));
        assert!(!direct_play(None));
    }
}
