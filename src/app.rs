use std::{
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::Arc,
};

use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{Html, IntoResponse, Redirect},
    routing::get,
};
use serde::Serialize;

use crate::{
    artwork, auth_api, bitrate, catalog_api, latest, playback, playstate, resume, search, shows,
    streaming, user_data,
};

#[derive(Debug)]
pub struct AppState {
    server_id: String,
    auth_dir: Option<PathBuf>,
    probe_executable: PathBuf,
    local_address: Option<String>,
}

impl AppState {
    pub fn new(server_id: String) -> Self {
        Self {
            server_id,
            auth_dir: None,
            probe_executable: PathBuf::from("ffprobe"),
            local_address: None,
        }
    }

    pub fn with_auth_dir(server_id: String, auth_dir: PathBuf) -> Self {
        Self {
            server_id,
            auth_dir: Some(auth_dir),
            probe_executable: PathBuf::from("ffprobe"),
            local_address: None,
        }
    }

    /// Override the external probe binary for integration fixtures or deployments.
    /// This is local Rust configuration, not user-controlled HTTP input.
    pub fn with_probe_executable(mut self, executable: PathBuf) -> Self {
        self.probe_executable = executable;
        self
    }

    /// Advertise only an explicitly bound, concrete HTTP listener address.
    /// Wildcard binds must never leak an unreachable 0.0.0.0/:: URL or
    /// reflect an untrusted client-supplied HTTP Host header.
    pub fn with_listen_addr(mut self, address: SocketAddr) -> Self {
        self.local_address = (!address.ip().is_unspecified()).then(|| format!("http://{address}"));
        self
    }

    pub fn probe_executable(&self) -> &Path {
        &self.probe_executable
    }

    pub fn auth_dir(&self) -> Option<PathBuf> {
        self.auth_dir.clone()
    }

    pub fn server_id(&self) -> &str {
        &self.server_id
    }
}

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/", get(|| async { Redirect::temporary("/web/") }))
        .route("/web/", get(web))
        .route("/health", get(health))
        .route("/api/pelagos/status", get(status))
        // This compatibility-shaped endpoint alone does NOT establish Jellyfin compatibility.
        .route("/System/Info/Public", get(public_info))
        .route("/System/Info", get(system_info))
        .route("/System/Ping", get(ping).post(ping))
        .merge(auth_api::router())
        .merge(catalog_api::router())
        .merge(latest::router())
        .merge(streaming::router())
        .merge(playback::router())
        .merge(search::router())
        .merge(shows::router())
        .merge(user_data::router())
        .merge(playstate::router())
        .merge(resume::router())
        .merge(artwork::router())
        .merge(bitrate::router())
        .with_state(state)
}

async fn web() -> impl IntoResponse {
    Html(include_str!("../web/index.html"))
}

#[derive(Serialize)]
struct Health {
    status: &'static str,
    version: &'static str,
}

async fn health() -> Json<Health> {
    Json(Health {
        status: "ok",
        version: env!("CARGO_PKG_VERSION"),
    })
}

#[derive(Serialize)]
struct Status {
    project: &'static str,
    maturity: &'static str,
    jellyfin_compatible: bool,
    tv_client_playback: bool,
    server_version: &'static str,
}

async fn status() -> Json<Status> {
    Json(Status {
        project: "Pelagos",
        maturity: "pre-alpha",
        jellyfin_compatible: false,
        tv_client_playback: false,
        server_version: env!("CARGO_PKG_VERSION"),
    })
}

#[derive(Serialize)]
#[serde(rename_all = "PascalCase")]
struct PublicSystemInfo {
    id: String,
    server_name: &'static str,
    version: &'static str,
    product_name: &'static str,
    operating_system: &'static str,
    startup_wizard_completed: bool,
    local_address: Option<String>,
}

async fn ping() -> &'static str {
    "Pelagos"
}

#[derive(Serialize)]
#[serde(rename_all = "PascalCase")]
struct SystemInfo {
    #[serde(flatten)]
    public: PublicSystemInfo,
    has_pending_restart: bool,
    is_shutting_down: bool,
    supports_library_monitor: bool,
    web_socket_port_number: u16,
    completed_installations: Vec<serde_json::Value>,
    can_self_restart: bool,
    can_launch_web_browser: bool,
    has_update_available: bool,
}

async fn system_info(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Json<SystemInfo>, StatusCode> {
    auth_api::authenticated(state.clone(), headers).await?;
    Ok(Json(SystemInfo {
        public: public_fields(&state),
        has_pending_restart: false,
        is_shutting_down: false,
        supports_library_monitor: false,
        web_socket_port_number: 0,
        completed_installations: Vec::new(),
        can_self_restart: false,
        can_launch_web_browser: false,
        has_update_available: false,
    }))
}

fn public_fields(state: &AppState) -> PublicSystemInfo {
    PublicSystemInfo {
        id: state.server_id.clone(),
        server_name: "Pelagos",
        version: env!("CARGO_PKG_VERSION"),
        product_name: "Pelagos",
        operating_system: std::env::consts::OS,
        startup_wizard_completed: false,
        local_address: state.local_address.clone(),
    }
}

async fn public_info(State(state): State<Arc<AppState>>) -> Json<PublicSystemInfo> {
    Json(public_fields(&state))
}
