//! Administrator-only, read-only subset of pinned Jellyfin device inventory.
//! No token secret is exposed and no real-client acceptance is implied.
use std::sync::Arc;

use axum::{
    Json, Router,
    extract::{Query, RawQuery, State},
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
        .route(
            "/Devices/Options",
            get(device_options).post(update_device_options),
        )
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
    let dir = administrator_state(state.clone(), headers).await?;
    let requested_user = query
        .user_id
        .as_deref()
        .map(uuid::Uuid::parse_str)
        .transpose()
        .map_err(|_| StatusCode::BAD_REQUEST)?
        .map(|id| id.to_string());
    let server_id = state.server_id().to_owned();
    let devices = tokio::task::spawn_blocking(move || {
        if let Some(id) = requested_user.as_deref() {
            if auth::find_user(&dir, id, &server_id)?.is_none() {
                return Ok(None);
            }
        }
        auth::list_registered_devices(&dir, requested_user.as_deref()).map(Some)
    })
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    .ok_or(StatusCode::NOT_FOUND)?;
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

/// Revoke matching device sessions for up to 16 distinct client IDs.
/// This is the pinned DELETE /Devices?id=...&id=... array subset.
/// Percent-encoded query values are decoded before validation.
async fn delete_device(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    RawQuery(raw): RawQuery,
) -> Result<StatusCode, StatusCode> {
    let dir = administrator_state(state, headers).await?;
    let raw = raw.unwrap_or_default();
    if raw.len() > 4096 {
        return Err(StatusCode::BAD_REQUEST);
    }
    let mut device_ids = Vec::new();
    let mut parameters = 0;
    for (key, id) in form_urlencoded::parse(raw.as_bytes()) {
        parameters += 1;
        if parameters > 32
            || key != "id"
            || id.is_empty()
            || id.len() > 256
            || id.chars().any(char::is_control)
        {
            return Err(StatusCode::BAD_REQUEST);
        }
        if !device_ids.iter().any(|known: &String| known == id.as_ref()) {
            if device_ids.len() >= 16 {
                return Err(StatusCode::BAD_REQUEST);
            }
            device_ids.push(id.into_owned());
        }
    }
    if device_ids.is_empty() {
        return Err(StatusCode::BAD_REQUEST);
    }
    let result = tokio::task::spawn_blocking(move || {
        auth::revoke_registered_device_sessions(&dir, &device_ids)
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
    // Value deliberately distinguishes required JSON null from a missing
    // CustomName field. Only a string or explicit null is accepted.
    custom_name: serde_json::Value,
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
    Ok(Json(found.ok_or(StatusCode::NOT_FOUND)?))
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
    let custom_name = match &body.custom_name {
        serde_json::Value::Null => None,
        serde_json::Value::String(value) if value.is_empty() => None,
        serde_json::Value::String(value)
            if value.len() <= 128
                && value.trim() == value
                && !value.chars().any(char::is_control) =>
        {
            Some(value.to_owned())
        }
        _ => return Err(StatusCode::BAD_REQUEST),
    };
    let outcome = tokio::task::spawn_blocking(move || {
        auth::rename_registered_device(&dir, &query.id, custom_name.as_deref())
    })
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    match outcome {
        auth::DeviceCustomNameUpdate::Updated => Ok(StatusCode::NO_CONTENT),
        auth::DeviceCustomNameUpdate::NotFound => Err(StatusCode::NOT_FOUND),
    }
}
