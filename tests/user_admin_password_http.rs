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

#[tokio::test]
async fn elevated_ordinary_password_reset_invalidates_existing_sessions() {
    let fixture = tempfile::tempdir().unwrap();
    let state = fixture.path().join("state");
    auth::create_user(&state, "admin", "reset-admin-initial-password").unwrap();
    auth::create_user(&state, "viewer", "reset-viewer-initial-password").unwrap();
    let admin = auth::authenticate(&state, "admin", "reset-admin-initial-password", "server")
        .unwrap()
        .unwrap();
    let viewer = auth::authenticate(&state, "viewer", "reset-viewer-initial-password", "server")
        .unwrap()
        .unwrap();
    let second = auth::authenticate(&state, "viewer", "reset-viewer-initial-password", "server")
        .unwrap()
        .unwrap();
    let router = app::router(Arc::new(AppState::with_auth_dir(
        "server".to_owned(),
        state.clone(),
    )));
    let target = format!("/Users/{}/Password", viewer.user.id);
    let password = "replacement-account-password";
    let request = json!({"NewPw":password});
    assert_eq!(
        call(&router, "POST", &target, None, request.clone())
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        call(
            &router,
            "POST",
            &target,
            Some(&viewer.access_token),
            request.clone(),
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        call(
            &router,
            "POST",
            &format!("/Users/{}/Password", admin.user.id),
            Some(&admin.access_token),
            request.clone(),
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        call(
            &router,
            "POST",
            "/Users/nonexistent/Password",
            Some(&admin.access_token),
            request.clone(),
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
    let unsupported = call(
        &router,
        "POST",
        &target,
        Some(&admin.access_token),
        json!({"NewPw":password,"ResetPassword":true}),
    )
    .await;
    assert!(
        unsupported.status() == StatusCode::UNPROCESSABLE_ENTITY
            || unsupported.status() == StatusCode::BAD_REQUEST
    );
    assert_eq!(
        call(
            &router,
            "POST",
            &target,
            Some(&admin.access_token),
            json!({"NewPw":"short"}),
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );
    assert!(
        auth::identify(&state, &viewer.access_token, "server")
            .unwrap()
            .is_some()
    );

    let response = call(&router, "POST", &target, Some(&admin.access_token), request).await;
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert!(
        to_bytes(response.into_body(), 1024)
            .await
            .unwrap()
            .is_empty()
    );
    for token in [&viewer.access_token, &second.access_token] {
        assert_eq!(
            call(&router, "GET", "/Users/Me", Some(token), json!({}))
                .await
                .status(),
            StatusCode::UNAUTHORIZED
        );
    }
    assert!(
        auth::authenticate(&state, "viewer", "reset-viewer-initial-password", "server")
            .unwrap()
            .is_none()
    );
    let new_login = auth::authenticate(&state, "viewer", password, "server")
        .unwrap()
        .unwrap();
    assert_eq!(new_login.user.id, viewer.user.id);
    let restarted = app::router(Arc::new(AppState::with_auth_dir(
        "server".to_owned(),
        state.clone(),
    )));
    assert_eq!(
        call(
            &restarted,
            "GET",
            "/Users/Me",
            Some(&new_login.access_token),
            json!({}),
        )
        .await
        .status(),
        StatusCode::OK
    );
    assert_eq!(
        call(
            &restarted,
            "GET",
            "/Users/Me",
            Some(&admin.access_token),
            json!({}),
        )
        .await
        .status(),
        StatusCode::OK
    );
}
