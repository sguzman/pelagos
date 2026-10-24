use std::sync::Arc;

use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode},
    response::Response,
};
use pelagos::{
    app::{self, AppState},
    auth::{self, ClientMetadata},
};
use serde_json::{Value, json};
use tower::ServiceExt;

async fn call(router: &Router, method: &str, path: &str, token: Option<&str>) -> Response {
    let mut request = Request::builder().method(method).uri(path);
    if let Some(token) = token {
        request = request.header("X-Emby-Token", token);
    }
    router
        .clone()
        .oneshot(request.body(Body::empty()).unwrap())
        .await
        .unwrap()
}

async fn json_body(response: Response) -> Value {
    serde_json::from_slice(&to_bytes(response.into_body(), 65536).await.unwrap()).unwrap()
}

#[tokio::test]
async fn privileged_single_device_logout_revokes_only_matching_device_sessions() {
    let fixture = tempfile::tempdir().unwrap();
    let state = fixture.path().join("state");
    auth::create_user(&state, "admin", "device-revoke-admin-password").unwrap();
    auth::create_user(&state, "viewer", "device-revoke-viewer-password").unwrap();
    let admin = auth::authenticate(&state, "admin", "device-revoke-admin-password", "server")
        .unwrap()
        .unwrap();
    let metadata = |id: &str| ClientMetadata {
        client: Some("Jellyfin Android TV".to_owned()),
        device_id: Some(id.to_owned()),
        device_name: Some("Living room".to_owned()),
        application_version: Some("1.0".to_owned()),
    };
    let first = auth::authenticate_with_client(
        &state,
        "viewer",
        "device-revoke-viewer-password",
        "server",
        metadata("tv-1"),
    )
    .unwrap()
    .unwrap();
    let second = auth::authenticate_with_client(
        &state,
        "viewer",
        "device-revoke-viewer-password",
        "server",
        metadata("tv-1"),
    )
    .unwrap()
    .unwrap();
    let other = auth::authenticate_with_client(
        &state,
        "viewer",
        "device-revoke-viewer-password",
        "server",
        metadata("tv-2"),
    )
    .unwrap()
    .unwrap();
    let router = app::router(Arc::new(AppState::with_auth_dir(
        "server".to_owned(),
        state.clone(),
    )));
    let endpoint = "/Devices?id=tv-1";
    assert_eq!(
        call(&router, "DELETE", endpoint, None).await.status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        call(&router, "DELETE", endpoint, Some(&first.access_token))
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        call(
            &router,
            "DELETE",
            "/Devices?id=unknown",
            Some(&admin.access_token),
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        call(&router, "DELETE", "/Devices", Some(&admin.access_token))
            .await
            .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        call(&router, "DELETE", endpoint, Some(&admin.access_token))
            .await
            .status(),
        StatusCode::NO_CONTENT
    );
    for token in [&first.access_token, &second.access_token] {
        assert!(
            auth::identify(&state, token, "server")
                .unwrap()
                .is_none()
        );
    }
    assert!(auth::identify(&state, &other.access_token, "server").unwrap().is_some());
    assert!(auth::identify(&state, &admin.access_token, "server").unwrap().is_some());
    let device = json_body(
        call(
            &router,
            "GET",
            "/Devices/Info?id=tv-1",
            Some(&admin.access_token),
        )
        .await,
    )
    .await;
    assert_eq!(device["Id"], "tv-1");
    assert_eq!(device["Capabilities"]["SupportedCommands"], json!([]));
    let fresh = auth::authenticate_with_client(
        &state,
        "viewer",
        "device-revoke-viewer-password",
        "server",
        metadata("tv-1"),
    )
    .unwrap()
    .unwrap();
    assert!(auth::identify(&state, &fresh.access_token, "server").unwrap().is_some());
    let restarted = app::router(Arc::new(AppState::with_auth_dir(
        "server".to_owned(),
        state,
    )));
    assert_eq!(
        call(
            &restarted,
            "GET",
            "/Devices",
            Some(&admin.access_token),
        )
        .await
        .status(),
        StatusCode::OK
    );
}
