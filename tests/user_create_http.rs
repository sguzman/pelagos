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

async fn post(router: &Router, route: &str, token: Option<&str>, body: Value) -> Response {
    let mut request = Request::builder()
        .method("POST")
        .uri(route)
        .header("Content-Type", "application/json");
    if let Some(token) = token {
        request = request.header("X-Emby-Token", token);
    }
    router
        .clone()
        .oneshot(request.body(Body::from(body.to_string())).unwrap())
        .await
        .unwrap()
}

async fn json(response: Response) -> Value {
    serde_json::from_slice(&to_bytes(response.into_body(), 65536).await.unwrap()).unwrap()
}

#[tokio::test]
async fn elevated_user_creation_persists_an_ordinary_account_without_leaking_password() {
    let fixture = tempfile::tempdir().unwrap();
    let state = fixture.path().join("state");
    auth::create_user(&state, "admin", "existing-admin-password").unwrap();
    let admin = auth::authenticate(&state, "admin", "existing-admin-password", "server")
        .unwrap()
        .unwrap();
    let router = app::router(Arc::new(AppState::with_auth_dir(
        "server".to_owned(),
        state.clone(),
    )));
    let route = "/Users/New";
    let request = serde_json::json!({"Name":"New Viewer","Password":"a-strong-new-password"});
    assert_eq!(
        post(&router, route, None, request.clone()).await.status(),
        StatusCode::UNAUTHORIZED
    );
    let response = post(&router, route, Some(&admin.access_token), request.clone()).await;
    assert_eq!(response.status(), StatusCode::OK);
    let raw = to_bytes(response.into_body(), 65536).await.unwrap();
    assert!(!raw.windows("a-strong-new-password".len()).any(|part| part == b"a-strong-new-password"));
    let created: Value = serde_json::from_slice(&raw).unwrap();
    assert_eq!(created["Name"], "New Viewer");
    assert_eq!(created["ServerId"], "server");
    assert_eq!(created["Policy"]["IsAdministrator"], false);
    assert_eq!(created["Policy"]["EnableContentDownloading"], false);
    let uid = created["Id"].as_str().unwrap();
    assert!(!uid.is_empty());
    let viewer = auth::authenticate(&state, "new viewer", "a-strong-new-password", "server")
        .unwrap()
        .unwrap();
    assert_eq!(viewer.user.id, uid);
    assert!(!viewer.user.policy.is_administrator);
    assert_eq!(
        post(
            &router,
            route,
            Some(&viewer.access_token),
            serde_json::json!({"Name":"Escalate","Password":"another-strong-password"}),
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        post(&router, route, Some(&admin.access_token), request.clone())
            .await
            .status(),
        StatusCode::CONFLICT
    );
    assert_eq!(
        post(
            &router,
            route,
            Some(&admin.access_token),
            serde_json::json!({"Name":"new viewer","Password":"some-other-long-password"}),
        )
        .await
        .status(),
        StatusCode::CONFLICT
    );
    let restarted = app::router(Arc::new(AppState::with_auth_dir(
        "server".to_owned(),
        state.clone(),
    )));
    let me = restarted
        .clone()
        .oneshot(
            Request::builder()
                .uri("/Users/Me")
                .header("X-Emby-Token", &viewer.access_token)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(json(me).await["Id"], uid);
}

#[tokio::test]
async fn unsupported_passwordless_admin_fields_and_invalid_names_never_create_users() {
    let fixture = tempfile::tempdir().unwrap();
    let state = fixture.path().join("state");
    auth::create_user(&state, "admin", "another-admin-password").unwrap();
    let admin = auth::authenticate(&state, "admin", "another-admin-password", "server")
        .unwrap()
        .unwrap();
    let router = app::router(Arc::new(AppState::with_auth_dir(
        "server".to_owned(),
        state.clone(),
    )));
    for (payload, status) in [
        (
            serde_json::json!({"Name":"No Password"}),
            StatusCode::NOT_IMPLEMENTED,
        ),
        (
            serde_json::json!({"Name":"Bad","Password":"short"}),
            StatusCode::BAD_REQUEST,
        ),
        (
            serde_json::json!({"Name":" Trailing","Password":"long-enough-password"}),
            StatusCode::BAD_REQUEST,
        ),
        (
            serde_json::json!({
                "Name":"Attempt Admin",
                "Password":"long-enough-password",
                "IsAdministrator":true
            }),
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
    ] {
        assert_eq!(
            post(&router, "/Users/New", Some(&admin.access_token), payload)
                .await
                .status(),
            status
        );
    }
    assert!(auth::authenticate(&state, "No Password", "", "server")
        .unwrap()
        .is_none());
    assert!(auth::authenticate(&state, "Attempt Admin", "long-enough-password", "server")
        .unwrap()
        .is_none());
}
