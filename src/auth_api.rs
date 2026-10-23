//! First Jellyfin-compatible password-login and authenticated user boundary.
//! Device/session DTO parity, Quick Connect and broader user policies are still pending.
use std::{path::PathBuf, sync::Arc};

use axum::{
    Json, Router,
    extract::State,
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
        .route("/Sessions/Logout", post(logout))
        // Public account listing intentionally discloses no usernames yet.
        .route("/Users/Public", get(public_users))
}

async fn public_users() -> Json<Vec<User>> {
    Json(Vec::new())
}

fn state_dir(state: &AppState) -> Result<PathBuf, StatusCode> {
    state.auth_dir().ok_or(StatusCode::SERVICE_UNAVAILABLE)
}

async fn password_login(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<PasswordLogin>,
) -> Result<Json<Login>, StatusCode> {
    let state_dir = state_dir(&state)?;
    let username = payload.username.unwrap_or_default();
    let password = payload.pw.unwrap_or_default();
    let server_id = state.server_id().to_string();
    let outcome = tokio::task::spawn_blocking(move || {
        auth::authenticate(&state_dir, &username, &password, &server_id)
    })
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    match outcome {
        Ok(Some(login)) => Ok(Json(login)),
        Ok(None) => Err(StatusCode::UNAUTHORIZED),
        Err(_) => Err(StatusCode::INTERNAL_SERVER_ERROR),
    }
}

/// Do not accept credentials from query parameters, cookies, or body for read-only
/// authenticated routes. Header names are case-insensitive per HTTP.
fn session_token(headers: &HeaderMap) -> Option<String> {
    for name in ["x-emby-token", "x-mediabrowser-token"] {
        if let Some(token) = headers.get(name).and_then(|h| h.to_str().ok()) {
            if token.len() == 64 && token.bytes().all(|b| b.is_ascii_hexdigit()) {
                return Some(token.to_string());
            }
        }
    }
    let auth_header = headers.get(AUTHORIZATION)?.to_str().ok()?.trim();
    if let Some((kind, token)) = auth_header.split_once(' ') {
        if kind.eq_ignore_ascii_case("bearer")
            && token.len() == 64
            && token.bytes().all(|b| b.is_ascii_hexdigit())
        {
            return Some(token.to_string());
        }
        if kind.eq_ignore_ascii_case("mediabrowser") || kind.eq_ignore_ascii_case("emby") {
            for part in token.split(',') {
                if let Some((key, value)) = part.trim().split_once('=')
                    && key.trim().eq_ignore_ascii_case("Token")
                {
                    let value = value.trim().trim_matches('"');
                    if value.len() == 64 && value.bytes().all(|b| b.is_ascii_hexdigit()) {
                        return Some(value.to_string());
                    }
                }
            }
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

async fn me(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Json<User>, StatusCode> {
    let (user, _) = authenticated(state, headers).await?;
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
    fn rejects_invalid_token_formats() {
        let mut headers = HeaderMap::new();
        headers.insert("x-emby-token", "bad-token".parse().unwrap());
        assert!(session_token(&headers).is_none());
    }
}
