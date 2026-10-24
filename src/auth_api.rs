//! First Jellyfin-compatible password-login and authenticated user boundary.
//! Device/session DTO parity, Quick Connect and broader user policies are still pending.
use std::{collections::HashSet, path::PathBuf, sync::Arc};

use axum::{
    Json, Router,
    extract::{Path as RoutePath, Query, State},
    http::{HeaderMap, StatusCode, header::AUTHORIZATION},
    routing::{get, post},
};
use serde::{Deserialize, Serialize};

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
        .route("/Users", get(users).post(update_user_name_modern))
        .route("/Users/New", post(create_account))
        .route("/Users/Me", get(me))
        .route("/Users/{user_id}", get(user_by_id).post(update_user_name))
        .route("/Users/{user_id}/Policy", post(update_user_policy))
        .route("/Users/Password", post(change_password))
        .route("/Users/{user_id}/Password", post(admin_reset_password))
        .route("/Auth/Providers", get(auth_providers))
        .route(
            "/Auth/PasswordResetProviders",
            get(password_reset_providers),
        )
        .route("/Sessions/Logout", post(logout))
        .route("/Sessions", get(sessions))
        .route("/Sessions/Capabilities", post(report_session_capabilities))
        .route(
            "/Sessions/Capabilities/Full",
            post(report_session_capabilities_full),
        )
        // Public account listing intentionally discloses no usernames yet.
        .route("/Users/Public", get(public_users))
        .route("/QuickConnect/Enabled", get(quick_connect_enabled))
        .route("/QuickConnect/Initiate", post(quick_connect_unavailable))
        .route("/QuickConnect/Connect", get(quick_connect_unavailable))
        .route("/QuickConnect/Authorize", post(quick_connect_authorize))
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
struct AuthProviderDto {
    name: &'static str,
    id: &'static str,
}

/// Pinned administrator-only provider inventory. The provider is Rust
/// native and has a stable local ID, not the old .NET type name.
async fn auth_providers(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Json<Vec<AuthProviderDto>>, StatusCode> {
    let (user, _) = authenticated(state, headers).await?;
    if !user.policy.is_administrator {
        return Err(StatusCode::FORBIDDEN);
    }
    Ok(Json(vec![AuthProviderDto {
        name: "Default",
        id: "pelagos.auth.local.argon2id",
    }]))
}

/// No password-reset plugin is installed; ordinary administrator resets
/// and current-password self-service are not pluggable providers.
async fn password_reset_providers(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Json<Vec<AuthProviderDto>>, StatusCode> {
    let (user, _) = authenticated(state, headers).await?;
    if !user.policy.is_administrator {
        return Err(StatusCode::FORBIDDEN);
    }
    Ok(Json(Vec::new()))
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct UsersQuery {
    is_hidden: Option<bool>,
    is_disabled: Option<bool>,
}

/// Pinned GET /Users response subset, explicitly more restrictive than
/// upstream: only administrators can enumerate account names in this stage.
async fn users(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<UsersQuery>,
) -> Result<Json<Vec<User>>, StatusCode> {
    let (requester, _) = authenticated(state.clone(), headers).await?;
    if !requester.policy.is_administrator {
        return Err(StatusCode::FORBIDDEN);
    }
    if query.is_hidden == Some(true) {
        // Pelagos has no hidden account configuration or public login list.
        return Ok(Json(Vec::new()));
    }
    let dir = state_dir(&state)?;
    let id = state.server_id().to_owned();
    let mut entries = tokio::task::spawn_blocking(move || auth::list_users(&dir, &id))
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    if let Some(disabled) = query.is_disabled {
        entries.retain(|user| user.policy.is_disabled == disabled);
    }
    Ok(Json(entries))
}

/// A bounded implementation of legacy POST /Users/{userId}: only Name
/// can be changed. Refuse all unimplemented UserDto fields rather than
/// silently mutating only part of an upstream account update.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase", deny_unknown_fields)]
struct UserNamePatch {
    name: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct UpdateUserQuery {
    user_id: Option<String>,
}

/// Modern pinned POST /Users?userId=... shares the strict Name-only
/// mutation used by the legacy route. The absent userId defaults to
/// the authenticated account, as it does in the upstream controller.
async fn update_user_name_modern(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<UpdateUserQuery>,
    Json(patch): Json<UserNamePatch>,
) -> Result<StatusCode, StatusCode> {
    let (current, _) = authenticated(state.clone(), headers).await?;
    let user_id = query.user_id.unwrap_or_else(|| current.id.clone());
    update_user_name_inner(state, current, user_id, patch).await
}

async fn update_user_name(
    State(state): State<Arc<AppState>>,
    RoutePath(user_id): RoutePath<String>,
    headers: HeaderMap,
    Json(patch): Json<UserNamePatch>,
) -> Result<StatusCode, StatusCode> {
    let (current, _) = authenticated(state.clone(), headers).await?;
    update_user_name_inner(state, current, user_id, patch).await
}

async fn update_user_name_inner(
    state: Arc<AppState>,
    current: User,
    user_id: String,
    patch: UserNamePatch,
) -> Result<StatusCode, StatusCode> {
    if !current.policy.is_administrator {
        return Err(StatusCode::FORBIDDEN);
    }
    if user_id.is_empty() || user_id.len() > 128 {
        return Err(StatusCode::BAD_REQUEST);
    }
    if patch.name.is_empty()
        || patch.name.len() > 128
        || patch.name.trim() != patch.name
        || patch.name.chars().any(char::is_control)
    {
        return Err(StatusCode::BAD_REQUEST);
    }
    let dir = state_dir(&state)?;
    match tokio::task::spawn_blocking(move || auth::rename_user(&dir, &user_id, &patch.name))
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    {
        auth::UserNameUpdate::Updated => Ok(StatusCode::NO_CONTENT),
        auth::UserNameUpdate::NotFound => Err(StatusCode::NOT_FOUND),
        auth::UserNameUpdate::Conflict => Err(StatusCode::CONFLICT),
    }
}

/// Pinned POST /Users/Password subset: a logged-in user may change
/// their own password after supplying the current password. ResetPassword
/// and changes to another account are not silently treated as self-updates.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SelfPasswordQuery {
    user_id: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase", deny_unknown_fields)]
struct SelfPasswordPatch {
    current_pw: String,
    new_pw: String,
    #[serde(default)]
    reset_password: bool,
}

async fn change_password(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<SelfPasswordQuery>,
    Json(body): Json<SelfPasswordPatch>,
) -> Result<StatusCode, StatusCode> {
    let (user, token) = authenticated(state.clone(), headers).await?;
    if query
        .user_id
        .as_deref()
        .is_some_and(|id| id != user.id.as_str())
    {
        return Err(StatusCode::FORBIDDEN);
    }
    if body.reset_password {
        return Err(StatusCode::NOT_IMPLEMENTED);
    }
    if !(12..=1024).contains(&body.new_pw.len()) {
        return Err(StatusCode::BAD_REQUEST);
    }
    let dir = state_dir(&state)?;
    let user_id = user.id;
    let outcome = tokio::task::spawn_blocking(move || {
        auth::change_own_password(&dir, &user_id, &token, &body.current_pw, &body.new_pw)
    })
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    match outcome {
        auth::OwnPasswordChange::Updated => Ok(StatusCode::NO_CONTENT),
        auth::OwnPasswordChange::InvalidCurrentPassword => Err(StatusCode::FORBIDDEN),
        auth::OwnPasswordChange::StaleSession => Err(StatusCode::UNAUTHORIZED),
    }
}

/// Password resets for ordinary accounts only. The complete pinned
/// UpdateUserPassword shape (including current-password validation and
/// passwordless reset) remains intentionally unsupported here.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase", deny_unknown_fields)]
struct AdministratorPasswordRequest {
    new_pw: String,
}

async fn admin_reset_password(
    State(state): State<Arc<AppState>>,
    RoutePath(user_id): RoutePath<String>,
    headers: HeaderMap,
    Json(body): Json<AdministratorPasswordRequest>,
) -> Result<StatusCode, StatusCode> {
    let (current, _) = authenticated(state.clone(), headers).await?;
    if !current.policy.is_administrator {
        return Err(StatusCode::FORBIDDEN);
    }
    if user_id.is_empty() || user_id.len() > 128 {
        return Err(StatusCode::BAD_REQUEST);
    }
    if !(12..=1024).contains(&body.new_pw.len()) {
        return Err(StatusCode::BAD_REQUEST);
    }
    let dir = state_dir(&state)?;
    match tokio::task::spawn_blocking(move || {
        auth::reset_ordinary_user_password(&dir, &user_id, &body.new_pw)
    })
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    {
        auth::PasswordReset::Updated => Ok(StatusCode::NO_CONTENT),
        auth::PasswordReset::NotFound => Err(StatusCode::NOT_FOUND),
        auth::PasswordReset::AdministratorProtected => Err(StatusCode::FORBIDDEN),
    }
}

/// Deliberately strict subset of the pinned Jellyfin UserPolicy object.
/// The full policy has many more fields and is not currently implemented.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase", deny_unknown_fields)]
struct DisabledPolicyPatch {
    is_disabled: bool,
}

async fn update_user_policy(
    State(state): State<Arc<AppState>>,
    RoutePath(user_id): RoutePath<String>,
    headers: HeaderMap,
    Json(patch): Json<DisabledPolicyPatch>,
) -> Result<StatusCode, StatusCode> {
    let (current, _) = authenticated(state.clone(), headers).await?;
    if !current.policy.is_administrator {
        return Err(StatusCode::FORBIDDEN);
    }
    if user_id.len() > 128 || user_id.is_empty() {
        return Err(StatusCode::BAD_REQUEST);
    }
    let dir = state_dir(&state)?;
    match tokio::task::spawn_blocking(move || {
        auth::set_account_disabled(&dir, &user_id, patch.is_disabled)
    })
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    {
        auth::AccountDisabledUpdate::Updated => Ok(StatusCode::NO_CONTENT),
        auth::AccountDisabledUpdate::NotFound => Err(StatusCode::NOT_FOUND),
        auth::AccountDisabledUpdate::AdministratorProtected => Err(StatusCode::FORBIDDEN),
    }
}

/// Pinned Jellyfin CreateUserByName subset. HTTP cannot bootstrap the first
/// administrator; it requires a current administrator and a real password.
#[derive(Deserialize)]
#[serde(rename_all = "PascalCase", deny_unknown_fields)]
struct CreateUserRequest {
    name: String,
    password: Option<String>,
}

async fn create_account(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(payload): Json<CreateUserRequest>,
) -> Result<Json<User>, StatusCode> {
    let (current, _) = authenticated(state.clone(), headers).await?;
    if !current.policy.is_administrator {
        return Err(StatusCode::FORBIDDEN);
    }
    let password = payload.password.ok_or(StatusCode::NOT_IMPLEMENTED)?;
    if payload.name.is_empty()
        || payload.name.len() > 128
        || payload.name.trim() != payload.name
        || payload.name.chars().any(char::is_control)
        || !(12..=1024).contains(&password.len())
    {
        return Err(StatusCode::BAD_REQUEST);
    }
    let dir = state_dir(&state)?;
    let server_id = state.server_id().to_owned();
    let user = tokio::task::spawn_blocking(move || {
        match auth::create_user(&dir, &payload.name, &password) {
            Ok(mut user) => {
                user.server_id = server_id;
                Ok(user)
            }
            Err(error) => {
                if let Some(rusqlite::Error::SqliteFailure(details, _)) =
                    error.downcast_ref::<rusqlite::Error>()
                {
                    if details.code == rusqlite::ErrorCode::ConstraintViolation {
                        return Err(StatusCode::CONFLICT);
                    }
                }
                Err(StatusCode::INTERNAL_SERVER_ERROR)
            }
        }
    })
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)??;
    Ok(Json(user))
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
    let (current, _) = authenticated(state.clone(), headers).await?;
    if current.id == user_id {
        return Ok(Json(current));
    }
    if !current.policy.is_administrator || user_id.len() > 128 {
        return Err(StatusCode::NOT_FOUND);
    }
    let dir = state_dir(&state)?;
    let server_id = state.server_id().to_owned();
    let requested =
        tokio::task::spawn_blocking(move || auth::find_user(&dir, &user_id, &server_id))
            .await
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(requested.ok_or(StatusCode::NOT_FOUND)?))
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
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SessionCapabilitiesQuery {
    id: Option<String>,
    playable_media_types: Option<String>,
    supported_commands: Option<String>,
    supports_media_control: Option<bool>,
    supports_persistent_identifier: Option<bool>,
}

fn parse_reported_capabilities(
    raw: Option<String>,
    max_entries: usize,
) -> Result<Vec<String>, StatusCode> {
    let value = raw.unwrap_or_default();
    if value.len() > 4096 {
        return Err(StatusCode::BAD_REQUEST);
    }
    if value.is_empty() {
        return Ok(Vec::new());
    }
    let entries: Vec<String> = value.split(',').map(str::trim).map(str::to_owned).collect();
    if entries.len() > max_entries
        || entries.iter().any(|entry| {
            entry.is_empty()
                || entry.len() > 64
                || !entry
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
        })
    {
        return Err(StatusCode::BAD_REQUEST);
    }
    Ok(entries)
}

/// Bounded pinned Jellyfin POST /Sessions/Capabilities. Clients may report
/// metadata about their own session, never another user's session. Reporting
/// a remote-control command does not implement a server-side command route.
async fn report_session_capabilities(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<SessionCapabilitiesQuery>,
) -> Result<StatusCode, StatusCode> {
    let report = auth::SessionCapabilitiesReport {
        playable_media_types: parse_reported_capabilities(query.playable_media_types, 16)?,
        supported_commands: parse_reported_capabilities(query.supported_commands, 64)?,
        supports_media_control: query.supports_media_control.unwrap_or(false),
        supports_persistent_identifier: query.supports_persistent_identifier.unwrap_or(true),
        device_profile: None,
        app_store_url: None,
        icon_url: None,
    };
    save_session_capabilities(state, headers, query.id, report).await
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum CapabilitySequence {
    CommaDelimited(String),
    Entries(Vec<String>),
}

fn from_capability_sequence(
    field: Option<CapabilitySequence>,
    max_entries: usize,
) -> Result<Vec<String>, StatusCode> {
    let text = match field {
        None => String::new(),
        Some(CapabilitySequence::CommaDelimited(text)) => text,
        Some(CapabilitySequence::Entries(entries)) => {
            if entries.iter().any(|entry| entry.contains(',')) {
                return Err(StatusCode::BAD_REQUEST);
            }
            entries.join(",")
        }
    };
    parse_reported_capabilities(Some(text), max_entries)
}

/// ClientCapabilitiesDto has additional complex fields (notably DeviceProfile)
/// which must not be silently accepted before they have meaningful support.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase", deny_unknown_fields)]
struct FullSessionCapabilities {
    playable_media_types: Option<CapabilitySequence>,
    supported_commands: Option<CapabilitySequence>,
    supports_media_control: Option<bool>,
    supports_persistent_identifier: Option<bool>,
    device_profile: Option<serde_json::Value>,
    app_store_url: Option<String>,
    icon_url: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct FullSessionCapabilitiesQuery {
    id: Option<String>,
}

/// Pinned POST /Sessions/Capabilities/Full accepts a bounded JSON subset.
/// Both upstream comma-delimited JSON strings and string arrays are parsed.
async fn report_session_capabilities_full(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<FullSessionCapabilitiesQuery>,
    Json(body): Json<FullSessionCapabilities>,
) -> Result<StatusCode, StatusCode> {
    if let Some(profile) = body.device_profile.as_ref() {
        if !profile.is_object()
            || serde_json::to_vec(profile)
                .map_err(|_| StatusCode::BAD_REQUEST)?
                .len()
                > 65_536
        {
            return Err(StatusCode::BAD_REQUEST);
        }
        let mut visited = 0;
        if !bounded_device_profile(profile, 0, &mut visited) {
            return Err(StatusCode::BAD_REQUEST);
        }
    }
    for url in [&body.app_store_url, &body.icon_url].into_iter().flatten() {
        if url.len() > 2048
            || url.chars().any(char::is_control)
            || !(url.starts_with("https://") || url.starts_with("http://"))
        {
            return Err(StatusCode::BAD_REQUEST);
        }
    }
    let report = auth::SessionCapabilitiesReport {
        playable_media_types: from_capability_sequence(body.playable_media_types, 16)?,
        supported_commands: from_capability_sequence(body.supported_commands, 64)?,
        supports_media_control: body.supports_media_control.unwrap_or(false),
        supports_persistent_identifier: body.supports_persistent_identifier.unwrap_or(false),
        device_profile: body.device_profile,
        app_store_url: body.app_store_url,
        icon_url: body.icon_url,
    };
    save_session_capabilities(state, headers, query.id, report).await
}

/// Bound the untrusted nested device profile structure. This only stores a
/// negotiation offer; transcoding/direct-play decisions do not use it yet.
fn bounded_device_profile(value: &serde_json::Value, depth: usize, count: &mut usize) -> bool {
    *count += 1;
    if depth > 16 || *count > 4096 {
        return false;
    }
    match value {
        serde_json::Value::String(s) => s.len() <= 4096 && !s.chars().any(char::is_control),
        serde_json::Value::Array(items) => {
            items.len() <= 512
                && items
                    .iter()
                    .all(|item| bounded_device_profile(item, depth + 1, count))
        }
        serde_json::Value::Object(entries) => {
            entries.len() <= 256
                && entries.iter().all(|(key, value)| {
                    key.len() <= 128
                        && !key.chars().any(char::is_control)
                        && bounded_device_profile(value, depth + 1, count)
                })
        }
        _ => true,
    }
}

async fn save_session_capabilities(
    state: Arc<AppState>,
    headers: HeaderMap,
    requested_id: Option<String>,
    report: auth::SessionCapabilitiesReport,
) -> Result<StatusCode, StatusCode> {
    let (_, token) = authenticated(state.clone(), headers).await?;
    if requested_id
        .as_deref()
        .is_some_and(|id| id.is_empty() || id.len() > 64)
    {
        return Err(StatusCode::BAD_REQUEST);
    }
    let dir = state_dir(&state)?;
    let result = tokio::task::spawn_blocking(move || {
        auth::set_session_capabilities(&dir, &token, requested_id.as_deref(), &report)
    })
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    match result {
        auth::SessionCapabilityUpdate::Updated => Ok(StatusCode::NO_CONTENT),
        auth::SessionCapabilityUpdate::StaleSession => Err(StatusCode::UNAUTHORIZED),
        auth::SessionCapabilityUpdate::OtherSession => Err(StatusCode::FORBIDDEN),
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SessionQuery {
    device_id: Option<String>,
    active_within_seconds: Option<u32>,
    controllable_by_user_id: Option<String>,
}

/// Last-activity filtering uses timestamps of successful authenticated
/// requests, not playback heartbeat or client remote-control capabilities.
async fn sessions(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<SessionQuery>,
) -> Result<Json<Vec<auth::SessionInfo>>, StatusCode> {
    let (user, _) = authenticated(state.clone(), headers).await?;
    if query.controllable_by_user_id.is_some() {
        return Err(StatusCode::NOT_IMPLEMENTED);
    }
    if query
        .active_within_seconds
        .is_some_and(|value| value > 31_536_000)
    {
        return Err(StatusCode::BAD_REQUEST);
    }
    let dir = state_dir(&state)?;
    let server_id = state.server_id().to_owned();
    let device = query.device_id;
    let active_within = query.active_within_seconds;
    let found = tokio::task::spawn_blocking(move || {
        auth::list_sessions_with_activity(&dir, &user, &server_id, device.as_deref(), active_within)
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
