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
        .route("/Devices/Options", get(device_options).post(update_device_options))
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
    let device = tokio::task::spawn_blocking(move || auth::registered_device_info(&dir, &id))
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(device.ok_or(StatusCode::NOT_FOUND)?))
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

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase", deny_unknown_fields)]
struct DeviceOptionsPatch {
    custom_name: String,
}

async fn device_options(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<DeviceByIdQuery>,
) -> Result<Json<auth::DeviceOptionsDto>, StatusCode> {
    let dir = administrator_state(state, headers).await?;
    if query.id.is_empty() || query.id.len() > 256 {
        return Err(StatusCode::BAD_REQUEST);
    }
    let id = query.id;
    let found = tokio::task::spawn_blocking(move || auth::registered_device_options(&dir, &id))
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(
        found
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
            .ok_or(StatusCode::NOT_FOUND)?,
    ))
}

/// Pinned POST /Devices/Options name-only subset. Unsupported options
/// and passwordless anonymous device mutation are not accepted.
async fn update_device_options(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<DeviceByIdQuery>,
    Json(body): Json<DeviceOptionsPatch>,
) -> Result<StatusCode, StatusCode> {
    let dir = administrator_state(state, headers).await?;
    if query.id.is_empty() || query.id.len() > 256 {
        return Err(StatusCode::BAD_REQUEST);
    }
    if body.custom_name.is_empty()
        || body.custom_name.len() > 128
        || body.custom_name.trim() != body.custom_name
        || body.custom_name.chars().any(char::is_control)
    {
        return Err(StatusCode::BAD_REQUEST);
    }
    let outcome = tokio::task::spawn_blocking(move || {
        auth::rename_registered_device(&dir, &query.id, &body.custom_name)
    })
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    match outcome {
        auth::DeviceCustomNameUpdate::Updated => Ok(StatusCode::NO_CONTENT),
        auth::DeviceCustomNameUpdate::NotFound => Err(StatusCode::NOT_FOUND),
    }
}
