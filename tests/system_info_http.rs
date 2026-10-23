use std::sync::Arc;

use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode},
    response::Response,
};
use pelagos::{
    app::{self, AppState},
    auth,
};
use serde_json::Value;
use tower::ServiceExt;

async fn get(router: &Router, uri: &str, token: Option<&str>) -> Response {
    let mut builder = Request::builder().uri(uri);
    if let Some(token) = token {
        builder = builder.header("X-Emby-Token", token);
    }
    router
        .clone()
        .oneshot(builder.body(Body::empty()).unwrap())
        .await
        .unwrap()
}

async fn json(response: Response) -> Value {
    let body = to_bytes(response.into_body(), 65536).await.unwrap();
    serde_json::from_slice(&body).unwrap()
}

#[tokio::test]
async fn authenticated_system_info_matches_public_identity_and_truthful_capabilities() {
    let fixture = tempfile::tempdir().unwrap();
    let state = fixture.path().join("state");
    auth::create_user(&state, "admin", "admin-info-test-password").unwrap();
    auth::create_user(&state, "viewer", "viewer-info-test-password").unwrap();
    let token = auth::authenticate(&state, "viewer", "viewer-info-test-password", "server-id")
        .unwrap()
        .unwrap()
        .access_token;
    let router = app::router(Arc::new(AppState::with_auth_dir(
        "server-id".to_owned(),
        state,
    )));
    let public = json(get(&router, "/System/Info/Public", None).await).await;
    assert_eq!(public["Id"], "server-id");
    assert_eq!(public["ProductName"], "Pelagos");
    assert_eq!(public["OperatingSystem"], std::env::consts::OS);
    assert_eq!(
        get(&router, "/System/Info", None).await.status(),
        StatusCode::UNAUTHORIZED
    );
    let info = get(&router, "/System/Info", Some(&token)).await;
    assert_eq!(info.status(), StatusCode::OK);
    let content = json(info).await;
    for key in [
        "Id",
        "ServerName",
        "ProductName",
        "Version",
        "OperatingSystem",
    ] {
        assert_eq!(content[key], public[key], "{key}");
    }
    assert_eq!(content["SupportsLibraryMonitor"], false);
    assert_eq!(content["WebSocketPortNumber"], 0);
    assert_eq!(content["CanSelfRestart"], false);
    assert_eq!(content["HasPendingRestart"], false);
    assert_eq!(content["IsShuttingDown"], false);
    assert!(
        content["CompletedInstallations"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    for disallowed in [
        "ProgramDataPath",
        "CachePath",
        "InternalMetadataPath",
        "LogPath",
        "TranscodingTempPath",
    ] {
        assert!(content.get(disallowed).is_none(), "{disallowed}");
    }

    let addressed = app::router(Arc::new(
        AppState::with_auth_dir("server-id".to_owned(), fixture.path().join("state"))
            .with_listen_addr("192.168.1.25:8097".parse().unwrap()),
    ));
    let public = json(get(&addressed, "/System/Info/Public", None).await).await;
    let private = json(get(&addressed, "/System/Info", Some(&token)).await).await;
    assert_eq!(public["LocalAddress"], "http://192.168.1.25:8097");
    assert_eq!(private["LocalAddress"], public["LocalAddress"]);
}
