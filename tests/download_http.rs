use std::sync::Arc;

use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use pelagos::{
    app::{self, AppState},
    auth, catalog,
};
use serde_json::Value;
use tower::ServiceExt;

async fn request(
    router: &Router,
    path: &str,
    token: Option<&str>,
    range: Option<&str>,
) -> axum::response::Response {
    let mut builder = Request::builder().uri(path);
    if let Some(token) = token {
        builder = builder.header("X-Emby-Token", token);
    }
    if let Some(range) = range {
        builder = builder.header(header::RANGE, range);
    }
    router
        .clone()
        .oneshot(builder.body(Body::empty()).unwrap())
        .await
        .unwrap()
}

#[tokio::test]
async fn admin_only_download_is_confined_and_has_attachment_headers() {
    let fixture = tempfile::tempdir().unwrap();
    let state = fixture.path().join("state");
    let root = fixture.path().join("library");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("Café.mkv"), b"VIDEO012345").unwrap();
    std::fs::write(root.join("Landscape.jpg"), b"PHOTO012345").unwrap();
    catalog::index(&state, &root).unwrap();
    auth::create_user(&state, "admin", "download-admin-password").unwrap();
    auth::create_user(&state, "viewer", "download-viewer-password").unwrap();
    let admin = auth::authenticate(&state, "admin", "download-admin-password", "server")
        .unwrap()
        .unwrap();
    let viewer = auth::authenticate(&state, "viewer", "download-viewer-password", "server")
        .unwrap()
        .unwrap();
    auth::grant_library(&state, "viewer", &root).unwrap();
    let router = app::router(Arc::new(AppState::with_auth_dir(
        "server".to_owned(),
        state.clone(),
    )));
    let response = request(&router, "/Items", Some(&admin.access_token), None).await;
    let items: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 65536).await.unwrap()).unwrap();
    let id = |name: &str| {
        items["Items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|v| v["Name"] == name)
            .unwrap()["Id"]
            .as_str()
            .unwrap()
            .to_owned()
    };
    let video = format!("/Items/{}/Download", id("Café"));
    let photo = format!("/Items/{}/Download", id("Landscape"));
    assert_eq!(
        request(&router, &video, None, None).await.status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        request(&router, &video, Some(&viewer.access_token), None)
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        request(&router, &photo, Some(&admin.access_token), None)
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    assert!(auth::set_content_download_permission(&state, "viewer", true).unwrap());
    assert!(auth::revoke_library(&state, "viewer", &root).unwrap());
    assert_eq!(
        request(&router, &video, Some(&viewer.access_token), None)
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    assert!(auth::grant_library(&state, "viewer", &root).unwrap());
    let restarted = app::router(Arc::new(AppState::with_auth_dir(
        "server".to_owned(),
        state.clone(),
    )));
    assert_eq!(
        request(&restarted, &video, Some(&viewer.access_token), None)
            .await
            .status(),
        StatusCode::OK
    );
    let user = request(&router, "/Users/Me", Some(&viewer.access_token), None).await;
    let current: Value =
        serde_json::from_slice(&to_bytes(user.into_body(), 65536).await.unwrap()).unwrap();
    assert_eq!(current["Policy"]["EnableContentDownloading"], true);
    let granted = request(&router, &video, Some(&viewer.access_token), None).await;
    assert_eq!(granted.status(), StatusCode::OK);
    assert_eq!(
        to_bytes(granted.into_body(), 64).await.unwrap().as_ref(),
        b"VIDEO012345"
    );
    assert!(auth::set_content_download_permission(&state, "viewer", false).unwrap());
    let user = request(&router, "/Users/Me", Some(&viewer.access_token), None).await;
    let current: Value =
        serde_json::from_slice(&to_bytes(user.into_body(), 65536).await.unwrap()).unwrap();
    assert_eq!(current["Policy"]["EnableContentDownloading"], false);
    assert_eq!(
        request(&router, &video, Some(&viewer.access_token), None)
            .await
            .status(),
        StatusCode::FORBIDDEN
    );

    let full = request(&router, &video, Some(&admin.access_token), None).await;
    assert_eq!(full.status(), StatusCode::OK);
    assert_eq!(full.headers()[header::CONTENT_TYPE], "video/x-matroska");
    assert_eq!(full.headers()[header::CACHE_CONTROL], "private, no-store");
    let disposition = full.headers()[header::CONTENT_DISPOSITION]
        .to_str()
        .unwrap();
    assert!(disposition.contains("attachment; filename=\"Caf_.mkv\""));
    assert!(disposition.contains("%C3%A9"));
    assert_eq!(
        to_bytes(full.into_body(), 64).await.unwrap().as_ref(),
        b"VIDEO012345"
    );

    let partial = request(
        &router,
        &video,
        Some(&admin.access_token),
        Some("bytes=5-8"),
    )
    .await;
    assert_eq!(partial.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(partial.headers()[header::CONTENT_RANGE], "bytes 5-8/11");
    assert_eq!(
        to_bytes(partial.into_body(), 64).await.unwrap().as_ref(),
        b"0123"
    );
    let bad = request(
        &router,
        &video,
        Some(&admin.access_token),
        Some("bytes=99-"),
    )
    .await;
    assert_eq!(bad.status(), StatusCode::RANGE_NOT_SATISFIABLE);
    // An opaque non-ASCII Range must not silently become a full-file response.
    let opaque = axum::http::HeaderValue::from_bytes(b"bytes=\xff").unwrap();
    let opaque_download = router
        .clone()
        .oneshot(
            Request::builder()
                .uri(video.as_str())
                .header("X-Emby-Token", &admin.access_token)
                .header(header::RANGE, opaque.clone())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(opaque_download.status(), StatusCode::RANGE_NOT_SATISFIABLE);
    assert_eq!(
        opaque_download.headers()[header::CONTENT_RANGE],
        "bytes */11"
    );

    let old = format!("/Items/{}/File", id("Café"));
    let opaque_direct = router
        .clone()
        .oneshot(
            Request::builder()
                .uri(old.as_str())
                .header("X-Emby-Token", &viewer.access_token)
                .header(header::RANGE, opaque)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(opaque_direct.status(), StatusCode::RANGE_NOT_SATISFIABLE);
    assert_eq!(opaque_direct.headers()[header::CONTENT_RANGE], "bytes */11");
    let original = request(&router, &old, Some(&viewer.access_token), None).await;
    assert!(
        original
            .headers()
            .get(header::CONTENT_DISPOSITION)
            .is_none()
    );
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn download_rejects_symlink_swap_outside_indexed_root() {
    use std::os::unix::fs::symlink;

    let fixture = tempfile::tempdir().unwrap();
    let state = fixture.path().join("state");
    let root = fixture.path().join("library");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("Film.mkv"), b"allowed").unwrap();
    catalog::index(&state, &root).unwrap();
    auth::create_user(&state, "admin", "download-symlink-password").unwrap();
    let admin = auth::authenticate(&state, "admin", "download-symlink-password", "server")
        .unwrap()
        .unwrap();
    let router = app::router(Arc::new(AppState::with_auth_dir(
        "server".to_owned(),
        state.clone(),
    )));
    let response = request(&router, "/Items", Some(&admin.access_token), None).await;
    let all: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 65536).await.unwrap()).unwrap();
    let id = all["Items"][0]["Id"].as_str().unwrap();
    let outside = fixture.path().join("private.mkv");
    std::fs::write(&outside, b"restricted").unwrap();
    std::fs::remove_file(root.join("Film.mkv")).unwrap();
    symlink(&outside, root.join("Film.mkv")).unwrap();
    assert_eq!(
        request(
            &router,
            &format!("/Items/{id}/Download"),
            Some(&admin.access_token),
            None,
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
}
