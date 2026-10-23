//! First Jellyfin-compatible password-login and authenticated user boundary.
//! Device/session DTO parity, Quick Connect and broader user policies are still pending.
use std::{collections::HashSet, path::PathBuf, sync::Arc};

use axum::{
    Json, Router,
    extract::{Path as RoutePath, Query, State},
    http::{HeaderMap, StatusCode, header::AUTHORIZATION},
    routing::{get, post},
};
use serde::Deserialize;

use crate::{
    app::AppState,
    auth::{self, Login, User},
};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct PasswordLogin {
    username: Option<String>,
    pw: Option<String>,
}

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/Users/AuthenticateByName", post(password_login))
        .route("/Users/Me", get(me))
        .route("/Users/{user_id}", get(user_by_id))
        .route("/Sessions/Logout", post(logout))
        .route("/Sessions", get(sessions))
        // Public account listing intentionally discloses no usernames yet.
        .route("/Users/Public", get(public_users))
        .route("/QuickConnect/Enabled", get(quick_connect_enabled))
        .route("/QuickConnect/Initiate", post(quick_connect_unavailable))
        .route("/QuickConnect/Connect", get(quick_connect_unavailable))
        .route("/QuickConnect/Authorize", post(quick_connect_authorize))
}

/// Pinned Jellyfin v12.3 Quick Connect is not enabled in this pre-alpha.
/// Do not manufacture pairing codes, persist requests, or imply TV support.
async fn quick_connect_enabled() -> Json<bool> {
    Json(false)
}

async fn quick_connect_unavailable() -> StatusCode {
    StatusCode::UNAUTHORIZED
}

async fn quick_connect_authorize(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<StatusCode, StatusCode> {
    authenticated(state, headers).await?;
    Err(StatusCode::UNAUTHORIZED)
}

async fn public_users() -> Json<Vec<User>> {
    Json(Vec::new())
}

fn state_dir(state: &AppState) -> Result<PathBuf, StatusCode> {
    state.auth_dir().ok_or(StatusCode::SERVICE_UNAVAILABLE)
}

async fn password_login(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(payload): Json<PasswordLogin>,
) -> Result<Json<Login>, StatusCode> {
    let state_dir = state_dir(&state)?;
    let username = payload.username.unwrap_or_default();
    let password = payload.pw.unwrap_or_default();
    let server_id = state.server_id().to_string();
    let client = client_metadata(&headers);
    let outcome = tokio::task::spawn_blocking(move || {
        auth::authenticate_with_client(&state_dir, &username, &password, &server_id, client)
    })
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    match outcome {
        Ok(Some(login)) => Ok(Json(login)),
        Ok(None) => Err(StatusCode::UNAUTHORIZED),
        Err(_) => Err(StatusCode::INTERNAL_SERVER_ERROR),
    }
}

/// Parse the quoted, comma-delimited MediaBrowser/Emby header without
/// accidentally splitting a device name that contains a comma.
fn jellyfin_parameters(headers: &HeaderMap) -> Vec<(String, String)> {
    let raw = match headers.get(AUTHORIZATION).and_then(|h| h.to_str().ok()) {
        Some(raw) if raw.len() <= 4096 => raw.trim(),
        _ => return Vec::new(),
    };
    let Some((scheme, rest)) = raw.split_once(' ') else {
        return Vec::new();
    };
    if !scheme.eq_ignore_ascii_case("mediabrowser") && !scheme.eq_ignore_ascii_case("emby") {
        return Vec::new();
    }
    let mut fields = Vec::new();
    let mut quoted = false;
    let mut escaped = false;
    let mut start = 0;
    for (index, ch) in rest.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        match ch {
            '\\' if quoted => escaped = true,
            '"' => quoted = !quoted,
            ',' if !quoted => {
                fields.push(&rest[start..index]);
                start = index + 1;
            }
            _ => {}
        }
    }
    if quoted {
        return Vec::new();
    }
    fields.push(&rest[start..]);
    fields
        .into_iter()
        .filter_map(|field| {
            let (key, value) = field.trim().split_once('=')?;
            let key = key.trim();
            let value = value.trim();
            let value = value
                .strip_prefix('"')
                .and_then(|v| v.strip_suffix('"'))
                .unwrap_or(value);
            if value.len() > 256 || value.chars().any(char::is_control) {
                return None;
            }
            Some((
                key.to_owned(),
                value.replace("\\\"", "\"").replace("\\\\", "\\"),
            ))
        })
        .collect()
}

fn client_metadata(headers: &HeaderMap) -> auth::ClientMetadata {
    let mut metadata = auth::ClientMetadata::default();
    for (key, value) in jellyfin_parameters(headers) {
        if value.is_empty() {
            continue;
        }
        if key.eq_ignore_ascii_case("Client") {
            metadata.client = Some(value);
        } else if key.eq_ignore_ascii_case("Device") {
            metadata.device_name = Some(value);
        } else if key.eq_ignore_ascii_case("DeviceId") {
            metadata.device_id = Some(value);
        } else if key.eq_ignore_ascii_case("Version") {
            metadata.application_version = Some(value);
        }
    }
    metadata
}

/// Do not accept tokens from URL query parameters or cookies.
fn session_token(headers: &HeaderMap) -> Option<String> {
    for name in ["x-emby-token", "x-mediabrowser-token"] {
        if let Some(token) = headers.get(name).and_then(|h| h.to_str().ok())
            && token.len() == 64
            && token.bytes().all(|b| b.is_ascii_hexdigit())
        {
            return Some(token.to_owned());
        }
    }
    if let Some(raw) = headers.get(AUTHORIZATION).and_then(|h| h.to_str().ok())
        && let Some((kind, token)) = raw.trim().split_once(' ')
        && kind.eq_ignore_ascii_case("bearer")
        && token.len() == 64
        && token.bytes().all(|b| b.is_ascii_hexdigit())
    {
        return Some(token.to_owned());
    }
    for (key, value) in jellyfin_parameters(headers) {
        if key.eq_ignore_ascii_case("Token")
            && value.len() == 64
            && value.bytes().all(|b| b.is_ascii_hexdigit())
        {
            return Some(value);
        }
    }
    None
}

pub(crate) async fn authenticated(
    state: Arc<AppState>,
    headers: HeaderMap,
) -> Result<(User, String), StatusCode> {
    let token = session_token(&headers).ok_or(StatusCode::UNAUTHORIZED)?;
    let dir = state_dir(&state)?;
    let server_id = state.server_id().to_owned();
    let token_clone = token.clone();
    let outcome =
        tokio::task::spawn_blocking(move || auth::identify(&dir, &token_clone, &server_id))
            .await
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    match outcome {
        Ok(Some(user)) => Ok((user, token)),
        Ok(None) => Err(StatusCode::UNAUTHORIZED),
        Err(_) => Err(StatusCode::INTERNAL_SERVER_ERROR),
    }
}

/// Resolve the authenticated user's readable roots without exposing grants to
/// unauthenticated callers. None means administrator access to all indexed roots.
pub(crate) async fn library_scope(
    state: Arc<AppState>,
    headers: HeaderMap,
) -> Result<(PathBuf, Option<HashSet<String>>), StatusCode> {
    let (user, _) = authenticated(state.clone(), headers).await?;
    let dir = state_dir(&state)?;
    let state_dir = dir.clone();
    let allowed =
        tokio::task::spawn_blocking(move || auth::visible_library_roots(&state_dir, &user))
            .await
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok((dir, allowed))
}

async fn me(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Json<User>, StatusCode> {
    let (user, _) = authenticated(state, headers).await?;
    Ok(Json(user))
}

/// Read the current account through Jellyfin's ID-based user route without
/// publishing other account identities. Full administrator directory access
/// and broader UserDto parity remain unimplemented.
async fn user_by_id(
    State(state): State<Arc<AppState>>,
    RoutePath(user_id): RoutePath<String>,
    headers: HeaderMap,
) -> Result<Json<User>, StatusCode> {
    let (user, _) = authenticated(state, headers).await?;
    if user.id != user_id {
        return Err(StatusCode::NOT_FOUND);
    }
    Ok(Json(user))
}

async fn logout(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<StatusCode, StatusCode> {
    let (_, token) = authenticated(state.clone(), headers).await?;
    let dir = state_dir(&state)?;
    let result = tokio::task::spawn_blocking(move || auth::revoke(&dir, &token))
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    result.map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SessionQuery {
    device_id: Option<String>,
    active_within_seconds: Option<u32>,
    controllable_by_user_id: Option<String>,
}

/// The current session index is persistent, not yet Jellyfin's activity/remote
/// control model. Refuse those unimplemented filters instead of pretending parity.
async fn sessions(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<SessionQuery>,
) -> Result<Json<Vec<auth::SessionInfo>>, StatusCode> {
    let (user, _) = authenticated(state.clone(), headers).await?;
    if query.active_within_seconds.is_some() || query.controllable_by_user_id.is_some() {
        return Err(StatusCode::NOT_IMPLEMENTED);
    }
    let dir = state_dir(&state)?;
    let server_id = state.server_id().to_owned();
    let device = query.device_id;
    let found = tokio::task::spawn_blocking(move || {
        auth::list_sessions(&dir, &user, &server_id, device.as_deref())
    })
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(found))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_jellyfin_device_token_without_query_credentials() {
        let mut headers = HeaderMap::new();
        let token = "a".repeat(64);
        headers.insert(
            AUTHORIZATION,
            format!("MediaBrowser Client=\"TV\", Token=\"{token}\", DeviceId=\"tv-1\"")
                .parse()
                .unwrap(),
        );
        assert_eq!(session_token(&headers), Some(token));
        assert!(session_token(&HeaderMap::new()).is_none());
    }

    #[test]
    fn quoted_device_metadata_preserves_commas_and_sanitizes_controls() {
        let mut headers = HeaderMap::new();
        headers.insert(
            AUTHORIZATION,
            "MediaBrowser Client=\"Jellyfin Android TV\", Device=\"Living, Room\", DeviceId=\"tv-1\", Version=\"2.0\""
                .parse()
                .unwrap(),
        );
        let metadata = client_metadata(&headers);
        assert_eq!(metadata.client.as_deref(), Some("Jellyfin Android TV"));
        assert_eq!(metadata.device_name.as_deref(), Some("Living, Room"));
        assert_eq!(metadata.device_id.as_deref(), Some("tv-1"));
        assert_eq!(metadata.application_version.as_deref(), Some("2.0"));
    }

    #[test]
    fn rejects_invalid_token_formats() {
        let mut headers = HeaderMap::new();
        headers.insert("x-emby-token", "bad-token".parse().unwrap());
        assert!(session_token(&headers).is_none());
    }
}
