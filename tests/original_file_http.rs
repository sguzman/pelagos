use std::sync::Arc;

use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
    response::Response,
};
use pelagos::{
    app::{self, AppState},
    auth, catalog,
};
use serde_json::Value;
use tower::ServiceExt;

async fn call(
    router: &Router,
    method: &str,
    route: &str,
    token: Option<&str>,
    range: Option<&str>,
) -> Response {
    let mut request = Request::builder().method(method).uri(route);
    if let Some(token) = token {
        request = request.header("X-Emby-Token", token);
    }
    if let Some(range) = range {
        request = request.header(header::RANGE, range);
    }
    router
        .clone()
        .oneshot(request.body(Body::empty()).unwrap())
        .await
        .unwrap()
}

async fn json(response: Response) -> Value {
    let bytes = to_bytes(response.into_body(), 65536).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

#[tokio::test]
async fn original_file_is_private_seekable_and_keeps_original_media_bytes() {
    let fixture = tempfile::tempdir().unwrap();
    let state = fixture.path().join("state");
    let allowed = fixture.path().join("Allowed");
    let private = fixture.path().join("Private");
    std::fs::create_dir_all(allowed.join("Pictures")).unwrap();
    std::fs::create_dir(&private).unwrap();
    std::fs::write(allowed.join("Film.mkv"), b"VIDEO012345").unwrap();
    std::fs::write(allowed.join("Track.flac"), b"AUDIO012345").unwrap();
    std::fs::write(allowed.join("Pictures/Photo.jpg"), b"PHOTO012345").unwrap();
    std::fs::write(private.join("Secret.mkv"), b"SECRET01234").unwrap();
    catalog::index(&state, &allowed).unwrap();
    catalog::index(&state, &private).unwrap();
    auth::create_user(&state, "admin", "original-admin-test-pass").unwrap();
    auth::create_user(&state, "viewer", "original-viewer-test-pass").unwrap();
    let admin = auth::authenticate(&state, "admin", "original-admin-test-pass", "server")
        .unwrap()
        .unwrap();
    let viewer = auth::authenticate(&state, "viewer", "original-viewer-test-pass", "server")
        .unwrap()
        .unwrap();
    let router = app::router(Arc::new(AppState::with_auth_dir(
        "server".to_owned(),
        state.clone(),
    )));
    let all = json(call(&router, "GET", "/Items", Some(&admin.access_token), None).await).await;
    let items = all["Items"].as_array().unwrap();
    let item_id = |name: &str| {
        items
            .iter()
            .find(|item| item["Name"] == name)
            .unwrap()["Id"]
            .as_str()
            .unwrap()
            .to_owned()
    };
    let film = item_id("Film");
    let audio = item_id("Track");
    let photo = item_id("Photo");
    let secret = item_id("Secret");
    let route = format!("/Items/{film}/File");
    assert_eq!(
        call(&router, "GET", &route, None, None).await.status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        call(&router, "GET", &route, Some(&viewer.access_token), None)
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    auth::grant_library(&state, "viewer", &allowed).unwrap();

    for (id, bytes, mime) in [
        (film.as_str(), b"VIDEO012345".as_slice(), "video/x-matroska"),
        (audio.as_str(), b"AUDIO012345".as_slice(), "audio/flac"),
        (photo.as_str(), b"PHOTO012345".as_slice(), "image/jpeg"),
    ] {
        let path = format!("/Items/{id}/File");
        let full = call(&router, "GET", &path, Some(&viewer.access_token), None).await;
        assert_eq!(full.status(), StatusCode::OK);
        assert_eq!(full.headers()[header::CONTENT_TYPE], mime);
        assert_eq!(full.headers()[header::CONTENT_LENGTH], "11");
        assert_eq!(full.headers()[header::ACCEPT_RANGES], "bytes");
        assert!(full.headers().get(header::CONTENT_DISPOSITION).is_none());
        assert_eq!(to_bytes(full.into_body(), 64).await.unwrap().as_ref(), bytes);

        let partial = call(
            &router,
            "GET",
            &path,
            Some(&viewer.access_token),
            Some("bytes=5-8"),
        )
        .await;
        assert_eq!(partial.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(partial.headers()[header::CONTENT_RANGE], "bytes 5-8/11");
        assert_eq!(
            to_bytes(partial.into_body(), 64).await.unwrap().as_ref(),
            b"0123"
        );
        let head = call(&router, "HEAD", &path, Some(&viewer.access_token), None).await;
        assert_eq!(head.status(), StatusCode::OK);
        assert_eq!(head.headers()[header::CONTENT_LENGTH], "11");
        assert!(to_bytes(head.into_body(), 64).await.unwrap().is_empty());
    }
    let invalid = call(
        &router,
        "GET",
        &route,
        Some(&viewer.access_token),
        Some("bytes=99-"),
    )
    .await;
    assert_eq!(invalid.status(), StatusCode::RANGE_NOT_SATISFIABLE);
    assert_eq!(invalid.headers()[header::CONTENT_RANGE], "bytes */11");
    assert_eq!(
        call(
            &router,
            "GET",
            &format!("/Items/{secret}/File"),
            Some(&viewer.access_token),
            None,
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );

    let restarted = app::router(Arc::new(AppState::with_auth_dir(
        "server".to_owned(),
        state.clone(),
    )));
    assert_eq!(
        call(&restarted, "HEAD", &route, Some(&viewer.access_token), None)
            .await
            .status(),
        StatusCode::OK
    );
    auth::revoke_library(&state, "viewer", &allowed).unwrap();
    assert_eq!(
        call(&restarted, "GET", &route, Some(&viewer.access_token), None)
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn original_file_rejects_indexed_path_swapped_to_outside_symlink() {
    use std::os::unix::fs::symlink;
    let fixture = tempfile::tempdir().unwrap();
    let state = fixture.path().join("state");
    let root = fixture.path().join("library");
    std::fs::create_dir(&root).unwrap();
    std::fs::write(root.join("safe.mp4"), b"permitted").unwrap();
    catalog::index(&state, &root).unwrap();
    auth::create_user(&state, "admin", "original-symlink-test-pass").unwrap();
    let admin = auth::authenticate(&state, "admin", "original-symlink-test-pass", "server")
        .unwrap()
        .unwrap();
    let router = app::router(Arc::new(AppState::with_auth_dir(
        "server".to_owned(),
        state.clone(),
    )));
    let all = json(call(&router, "GET", "/Items", Some(&admin.access_token), None).await).await;
    let id = all["Items"][0]["Id"].as_str().unwrap();
    let outside = fixture.path().join("private.mp4");
    std::fs::write(&outside, b"private").unwrap();
    std::fs::remove_file(root.join("safe.mp4")).unwrap();
    symlink(&outside, root.join("safe.mp4")).unwrap();
    let result = call(
        &router,
        "GET",
        &format!("/Items/{id}/File"),
        Some(&admin.access_token),
        None,
    )
    .await;
    assert_eq!(result.status(), StatusCode::FORBIDDEN);
}
