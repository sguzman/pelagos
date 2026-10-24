//! Administrator-only, read-only subset of pinned Jellyfin device inventory.
//! No token secret is exposed and no real-client acceptance is implied.
use std::sync::Arc;

use axum::{
    Json, Router,
    extract::{Query, State},
    http::{HeaderMap, StatusCode},
    routing::get,
};
use serde::{Deserialize, Serialize};

use crate::{app::AppState, auth, auth_api};

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DeviceListQuery {
    user_id: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DeviceByIdQuery {
    id: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
struct DeviceResults {
    items: Vec<auth::DeviceInfoDto>,
    total_record_count: usize,
    start_index: usize,
}

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/Devices", get(devices).delete(delete_device))
        .route("/Devices/Info", get(device_info))
}

async fn administrator_state(
    state: Arc<AppState>,
    headers: HeaderMap,
) -> Result<std::path::PathBuf, StatusCode> {
    let (requester, _) = auth_api::authenticated(state.clone(), headers).await?;
    if !requester.policy.is_administrator {
        return Err(StatusCode::FORBIDDEN);
    }
    state.auth_dir().ok_or(StatusCode::SERVICE_UNAVAILABLE)
}

/// Pinned GET /Devices subset: all registered device IDs or devices
/// whose last successful login was the requested user. No paging is
/// claimed and activity timing not established by login is null.
async fn devices(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<DeviceListQuery>,
) -> Result<Json<DeviceResults>, StatusCode> {
    let dir = administrator_state(state, headers).await?;
    if query
        .user_id
        .as_deref()
        .is_some_and(|id| id.is_empty() || id.len() > 128)
    {
        return Err(StatusCode::BAD_REQUEST);
    }
    let devices = tokio::task::spawn_blocking(move || {
        auth::list_registered_devices(&dir, query.user_id.as_deref())
    })
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let count = devices.len();
    Ok(Json(DeviceResults {
        items: devices,
        total_record_count: count,
        start_index: 0,
    }))
}

/// Pinned GET /Devices/Info returns an observed registration only.
/// Unknown IDs do not produce synthetic placeholder devices.
async fn device_info(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<DeviceByIdQuery>,
) -> Result<Json<auth::DeviceInfoDto>, StatusCode> {
    let dir = administrator_state(state, headers).await?;
    if query.id.is_empty() || query.id.len() > 256 {
        return Err(StatusCode::BAD_REQUEST);
    }
    let id = query.id;
    let devices = tokio::task::spawn_blocking(move || auth::list_registered_devices(&dir, None))
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    devices
        .into_iter()
        .find(|device| device.id == id)
        .map(Json)
        .ok_or(StatusCode::NOT_FOUND)
}

/** Revokes all access tokens attached to one recorded device ID.
 * The pinned upstream endpoint can revoke an array of IDs; this subset
 * accepts exactly one and refuses unknown devices rather than treating
 * the operation as a successful deletion of unseen state.
 */
async fn delete_device(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<DeviceByIdQuery>,
) -> Result<StatusCode, StatusCode> {
    let dir = administrator_state(state, headers).await?;
    if query.id.is_empty() || query.id.len() > 256 {
        return Err(StatusCode::BAD_REQUEST);
    }
    let device_id = query.id;
    let result = tokio::task::spawn_blocking(move || {
        auth::revoke_registered_device_sessions(&dir, &device_id)
    })
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    match result {
        auth::DeviceSessionRevocation::Revoked => Ok(StatusCode::NO_CONTENT),
        auth::DeviceSessionRevocation::NotFound => Err(StatusCode::BAD_REQUEST),
    }
}
