use std::sync::Arc;

use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
    response::Response,
};
use pelagos::{app::{self, AppState}, auth, catalog};
use serde_json::Value;
use tower::ServiceExt;

async fn call(
    app: &Router,
    method: &str,
    path: &str,
    token: Option<&str>,
    range: Option<&str>,
) -> Response {
    let mut req = Request::builder().method(method).uri(path);
    if let Some(token) = token {
        req = req.header("X-Emby-Token", token);
    }
    if let Some(range) = range {
        req = req.header(header::RANGE, range);
    }
    app.clone().oneshot(req.body(Body::empty()).unwrap()).await.unwrap()
}

#[tokio::test]
async fn static_video_supports_ranges_head_auth_and_no_transcode_claims() {
    let dir = tempfile::tempdir().unwrap();
    let library = dir.path().join("Movies");
    let state = dir.path().join("state");
    std::fs::create_dir_all(&library).unwrap();
    std::fs::write(library.join("sample.mp4"), b"0123456789").unwrap();
    catalog::index(&state, &library).unwrap();
    auth::create_user(&state, "admin", "strong-test-passphrase").unwrap();
    let token = auth::authenticate(&state, "admin", "strong-test-passphrase", "server")
        .unwrap()
        .unwrap()
        .access_token;
    let app = app::router(Arc::new(AppState::with_auth_dir("server".to_owned(), state)));
    let items = call(&app, "GET", "/Items", Some(&token), None).await;
    let body = to_bytes(items.into_body(), 1024 * 1024).await.unwrap();
    let items: Value = serde_json::from_slice(&body).unwrap();
    let id = items["Items"][0]["Id"].as_str().unwrap();
    let path = format!("/Videos/{id}/stream?static=true");
    assert_eq!(call(&app, "GET", &path, None, None).await.status(), StatusCode::UNAUTHORIZED);

    let full = call(&app, "GET", &path, Some(&token), None).await;
    assert_eq!(full.status(), StatusCode::OK);
    assert_eq!(full.headers()[header::CONTENT_LENGTH], "10");
    assert_eq!(full.headers()[header::ACCEPT_RANGES], "bytes");
    assert_eq!(to_bytes(full.into_body(), 100).await.unwrap().as_ref(), b"0123456789");

    let partial = call(&app, "GET", &path, Some(&token), Some("bytes=2-5")).await;
    assert_eq!(partial.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(partial.headers()[header::CONTENT_RANGE], "bytes 2-5/10");
    assert_eq!(to_bytes(partial.into_body(), 100).await.unwrap().as_ref(), b"2345");

    let head = call(&app, "HEAD", &path, Some(&token), None).await;
    assert_eq!(head.status(), StatusCode::OK);
    assert_eq!(head.headers()[header::CONTENT_LENGTH], "10");
    assert!(to_bytes(head.into_body(), 100).await.unwrap().is_empty());

    let invalid = call(&app, "GET", &path, Some(&token), Some("bytes=20-")).await;
    assert_eq!(invalid.status(), StatusCode::RANGE_NOT_SATISFIABLE);
    assert_eq!(invalid.headers()[header::CONTENT_RANGE], "bytes */10");
    let unsupported = call(&app, "GET", &format!("/Videos/{id}/stream"), Some(&token), None).await;
    assert_eq!(unsupported.status(), StatusCode::NOT_IMPLEMENTED);
}

#[cfg(unix)]
#[tokio::test]
async fn indexed_file_replaced_by_external_symlink_is_denied() {
    use std::os::unix::fs::symlink;
    let dir = tempfile::tempdir().unwrap();
    let media = dir.path().join("media");
    let state = dir.path().join("state");
    std::fs::create_dir(&media).unwrap();
    std::fs::write(media.join("video.mp4"), b"original").unwrap();
    catalog::index(&state, &media).unwrap();
    auth::create_user(&state, "admin", "strong-test-passphrase").unwrap();
    let token = auth::authenticate(&state, "admin", "strong-test-passphrase", "server")
        .unwrap()
        .unwrap()
        .access_token;
    let app = app::router(Arc::new(AppState::with_auth_dir("server".to_owned(), state)));
    let response = call(&app, "GET", "/Items", Some(&token), None).await;
    let bytes = to_bytes(response.into_body(), 10000).await.unwrap();
    let items: Value = serde_json::from_slice(&bytes).unwrap();
    let id = items["Items"][0]["Id"].as_str().unwrap();

    let outside = dir.path().join("private.mp4");
    std::fs::write(&outside, b"private-data").unwrap();
    std::fs::remove_file(media.join("video.mp4")).unwrap();
    symlink(&outside, media.join("video.mp4")).unwrap();
    let video = call(
        &app,
        "GET",
        &format!("/Videos/{id}/stream?static=true"),
        Some(&token),
        None,
    )
    .await;
    assert_eq!(video.status(), StatusCode::FORBIDDEN);
}
