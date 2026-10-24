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

async fn call(
    router: &Router,
    method: &str,
    route: &str,
    token: Option<&str>,
    body: Value,
) -> Response {
    let mut request = Request::builder().method(method).uri(route);
    if let Some(token) = token {
        request = request.header("X-Emby-Token", token);
    }
    router
        .clone()
        .oneshot(
            request
                .header("Content-Type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap()
}

async fn response_json(response: Response) -> Value {
    serde_json::from_slice(&to_bytes(response.into_body(), 65536).await.unwrap()).unwrap()
}

#[tokio::test]
async fn administrator_device_name_update_is_durable_and_does_not_change_identity() {
    let fixture = tempfile::tempdir().unwrap();
    let state = fixture.path().join("state");
    auth::create_user(&state, "admin", "device-options-admin-password").unwrap();
    auth::create_user(&state, "viewer", "device-options-viewer-password").unwrap();
    let admin = auth::authenticate(&state, "admin", "device-options-admin-password", "server")
        .unwrap()
        .unwrap();
    let viewer = auth::authenticate_with_client(
        &state,
        "viewer",
        "device-options-viewer-password",
        "server",
        ClientMetadata {
            client: Some("Jellyfin Android TV".to_owned()),
            device_id: Some("smart-tv".to_owned()),
            device_name: Some("Living Room".to_owned()),
            application_version: Some("1.0".to_owned()),
        },
    )
    .unwrap()
    .unwrap();
    let router = app::router(Arc::new(AppState::with_auth_dir(
        "server".to_owned(),
        state.clone(),
    )));
    let path = "/Devices/Options?id=smart-tv";
    assert_eq!(
        call(&router, "GET", path, None, json!({}))
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        call(&router, "POST", path, Some(&viewer.access_token), json!({"CustomName":"Den"}))
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    let initial = response_json(
        call(&router, "GET", path, Some(&admin.access_token), json!({})).await,
    )
    .await;
    assert_eq!(initial["DeviceId"], "smart-tv");
    assert_eq!(initial["CustomName"], Value::Null);
    assert_eq!(
        call(
            &router,
            "POST",
            path,
            Some(&admin.access_token),
            json!({"CustomName":"  Invalid"}),
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );
    let unknown = call(
        &router,
        "POST",
        path,
        Some(&admin.access_token),
        json!({"CustomName":"Den","IsAdministrator":true}),
    )
    .await;
    assert!(
        unknown.status() == StatusCode::UNPROCESSABLE_ENTITY
            || unknown.status() == StatusCode::BAD_REQUEST
    );
    assert_eq!(
        call(
            &router,
            "POST",
            "/Devices/Options?id=unknown",
            Some(&admin.access_token),
            json!({"CustomName":"Den"}),
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        call(&router, "POST", path, Some(&admin.access_token), json!({"CustomName":"Den"}))
            .await
            .status(),
        StatusCode::NO_CONTENT
    );
    assert!(auth::identify(&state, &viewer.access_token, "server").unwrap().is_some());
    let after = response_json(
        call(
            &router,
            "GET",
            "/Devices/Info?id=smart-tv",
            Some(&admin.access_token),
            json!({}),
        )
        .await,
    )
    .await;
    assert_eq!(after["Name"], "Living Room");
    assert_eq!(after["CustomName"], "Den");
    assert_eq!(after["Id"], "smart-tv");
    let restarted = app::router(Arc::new(AppState::with_auth_dir(
        "server".to_owned(),
        state.clone(),
    )));
    let options = response_json(
        call(&restarted, "GET", path, Some(&admin.access_token), json!({})).await,
    )
    .await;
    assert_eq!(options["CustomName"], "Den");
    assert_eq!(options["DeviceId"], "smart-tv");
    assert_eq!(options["Id"], Value::Null);
    let user = auth::identify(&state, &viewer.access_token, "server").unwrap().unwrap();
    assert_eq!(user.id, viewer.user.id);
}
