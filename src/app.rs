use std::{path::PathBuf, sync::Arc};

use axum::{
    Json, Router,
    extract::State,
    response::{Html, IntoResponse, Redirect},
    routing::get,
};
use serde::Serialize;

use crate::{auth_api, catalog_api, playback, streaming};

#[derive(Debug)]
pub struct AppState {
    server_id: String,
    auth_dir: Option<PathBuf>,
}

impl AppState {
    pub fn new(server_id: String) -> Self {
        Self {
            server_id,
            auth_dir: None,
        }
    }

    pub fn with_auth_dir(server_id: String, auth_dir: PathBuf) -> Self {
        Self {
            server_id,
            auth_dir: Some(auth_dir),
        }
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
        .route("/System/Ping", get(ping).post(ping))
        .merge(auth_api::router())
        .merge(catalog_api::router())
        .merge(streaming::router())
        .merge(playback::router())
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

async fn public_info(State(state): State<Arc<AppState>>) -> Json<PublicSystemInfo> {
    Json(PublicSystemInfo {
        id: state.server_id.clone(),
        server_name: "Pelagos",
        version: env!("CARGO_PKG_VERSION"),
        product_name: "Pelagos",
        operating_system: "",
        startup_wizard_completed: false,
        local_address: None,
    })
}
