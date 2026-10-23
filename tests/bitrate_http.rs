use std::sync::Arc;

use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
    response::Response,
};
use pelagos::{
    app::{self, AppState},
    auth,
};
use tower::ServiceExt;

async fn get(router: &Router, url: &str, token: Option<&str>) -> Response {
    let mut req = Request::builder().uri(url);
    if let Some(token) = token {
        req = req.header("X-Emby-Token", token);
    }
    router
        .clone()
        .oneshot(req.body(Body::empty()).unwrap())
        .await
        .unwrap()
}

#[tokio::test]
async fn bitrate_test_requires_login_and_returns_exact_uncacheable_random_bytes() {
    let fixture = tempfile::tempdir().unwrap();
    let state = fixture.path().join("state");
    auth::create_user(&state, "admin", "admin-bitrate-test-password").unwrap();
    auth::create_user(&state, "viewer", "viewer-bitrate-test-password").unwrap();
    let token = auth::authenticate(&state, "viewer", "viewer-bitrate-test-password", "server")
        .unwrap()
        .unwrap()
        .access_token;
    let router = app::router(Arc::new(AppState::with_auth_dir(
        "server".to_owned(),
        state,
    )));

    assert_eq!(
        get(&router, "/Playback/BitrateTest?size=37", None)
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );

    let sample = get(&router, "/Playback/BitrateTest?size=37", Some(&token)).await;
    assert_eq!(sample.status(), StatusCode::OK);
    assert_eq!(
        sample.headers()[header::CONTENT_TYPE],
        "application/octet-stream"
    );
    assert_eq!(sample.headers()[header::CONTENT_LENGTH], "37");
    assert_eq!(sample.headers()[header::CACHE_CONTROL], "no-store");
    let bytes = to_bytes(sample.into_body(), 40).await.unwrap();
    assert_eq!(bytes.len(), 37);
    assert!(bytes.iter().any(|byte| *byte != 0));

    let second = get(&router, "/Playback/BitrateTest?size=37", Some(&token)).await;
    let second_bytes = to_bytes(second.into_body(), 40).await.unwrap();
    assert_ne!(bytes, second_bytes);

    let default = get(&router, "/Playback/BitrateTest", Some(&token)).await;
    assert_eq!(default.status(), StatusCode::OK);
    assert_eq!(default.headers()[header::CONTENT_LENGTH], "102400");
    assert_eq!(
        to_bytes(default.into_body(), 110_000).await.unwrap().len(),
        102_400
    );

    let maximum = get(
        &router,
        "/Playback/BitrateTest?size=100000000",
        Some(&token),
    )
    .await;
    assert_eq!(maximum.status(), StatusCode::OK);
    assert_eq!(maximum.headers()[header::CONTENT_LENGTH], "100000000");
    drop(maximum);

    for invalid in ["0", "-1", "100000001", "banana"] {
        let response = get(
            &router,
            &format!("/Playback/BitrateTest?size={invalid}"),
            Some(&token),
        )
        .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{invalid}");
    }
}
