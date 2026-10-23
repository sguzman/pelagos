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

#[tokio::test]
async fn quick_connect_is_truthfully_disabled_without_creating_pairing_credentials() {
    let fixture = tempfile::tempdir().unwrap();
    let state = fixture.path().join("state");
    auth::create_user(&state, "admin", "disabled-pairing-test-password").unwrap();
    let admin = auth::authenticate(&state, "admin", "disabled-pairing-test-password", "server")
        .unwrap()
        .unwrap();
    let router = app::router(Arc::new(AppState::with_auth_dir(
        "server".to_owned(),
        state.clone(),
    )));
    let enabled = call(&router, "GET", "/QuickConnect/Enabled", None).await;
    assert_eq!(enabled.status(), StatusCode::OK);
    let payload = to_bytes(enabled.into_body(), 64).await.unwrap();
    assert_eq!(serde_json::from_slice::<Value>(&payload).unwrap(), false);

    for (method, path) in [
        ("POST", "/QuickConnect/Initiate"),
        ("GET", "/QuickConnect/Connect?secret=unknown"),
    ] {
        for token in [None, Some(admin.access_token.as_str())] {
            assert_eq!(
                call(&router, method, path, token).await.status(),
                StatusCode::UNAUTHORIZED
            );
        }
    }
    for token in [None, Some(admin.access_token.as_str())] {
        assert_eq!(
            call(
                &router,
                "POST",
                "/QuickConnect/Authorize?code=123456",
                token,
            )
            .await
            .status(),
            StatusCode::UNAUTHORIZED
        );
    }
    assert_eq!(
        call(&router, "GET", "/Users/Me", Some(&admin.access_token))
            .await
            .status(),
        StatusCode::OK
    );
    assert!(!state.join("quick-connect.sqlite3").exists());
}
