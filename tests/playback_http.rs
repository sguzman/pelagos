use std::sync::Arc;

use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode, header},
    response::Response,
};
use pelagos::{
    app::{self, AppState},
    auth,
};
use tower::ServiceExt;

async fn request(
    router: &Router,
    method: &str,
    path: &str,
    token: Option<&str>,
    body: &str,
) -> Response {
    let mut builder = Request::builder()
        .uri(path)
        .method(method)
        .header(header::CONTENT_TYPE, "application/json");
    if let Some(token) = token {
        builder = builder.header("X-Emby-Token", token);
    }
    router
        .clone()
        .oneshot(builder.body(Body::from(body.to_owned())).unwrap())
        .await
        .unwrap()
}

#[tokio::test]
async fn playback_requires_auth_and_rejects_unsupported_modes() {
    let temp = tempfile::tempdir().unwrap();
    auth::create_user(temp.path(), "admin", "long-private-test-password").unwrap();
    auth::create_user(temp.path(), "viewer", "long-viewer-test-password").unwrap();
    let admin = auth::authenticate(temp.path(), "admin", "long-private-test-password", "server")
        .unwrap()
        .unwrap()
        .access_token;
    let viewer = auth::authenticate(temp.path(), "viewer", "long-viewer-test-password", "server")
        .unwrap()
        .unwrap()
        .access_token;
    let router = app::router(Arc::new(AppState::with_auth_dir(
        "server".to_owned(),
        temp.path().to_path_buf(),
    )));
    let path = "/Items/00000000000000000000000000000001/PlaybackInfo";
    assert_eq!(
        request(&router, "GET", path, None, "",).await.status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        request(&router, "GET", path, Some(&viewer), "").await.status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        request(
            &router,
            "POST",
            path,
            Some(&admin),
            r#"{"EnableDirectPlay":false}"#,
        )
        .await
        .status(),
        StatusCode::NOT_IMPLEMENTED
    );
    assert_eq!(
        request(&router, "GET", path, Some(&admin), "").await.status(),
        StatusCode::NOT_FOUND
    );
}
