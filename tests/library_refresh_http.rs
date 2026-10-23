use std::sync::Arc;

use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode},
    response::Response,
};
use pelagos::{
    app::{self, AppState},
    auth, catalog,
};
use tower::ServiceExt;

async fn call(router: &Router, method: &str, route: &str, token: Option<&str>) -> Response {
    let mut request = Request::builder().method(method).uri(route);
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
async fn refresh_is_admin_only_and_atomically_updates_existing_roots() {
    let fixture = tempfile::tempdir().unwrap();
    let state = fixture.path().join("state");
    let movies = fixture.path().join("Movies");
    let music = fixture.path().join("Music");
    let other = fixture.path().join("NeverRegistered");
    std::fs::create_dir_all(&movies).unwrap();
    std::fs::create_dir_all(&music).unwrap();
    std::fs::create_dir_all(&other).unwrap();
    std::fs::write(movies.join("A.mp4"), b"first").unwrap();
    std::fs::write(music.join("Song.flac"), b"audio").unwrap();
    std::fs::write(other.join("Private.mp4"), b"not-registered").unwrap();
    catalog::index(&state, &movies).unwrap();
    catalog::index(&state, &music).unwrap();
    auth::create_user(&state, "admin", "refresh-admin-password").unwrap();
    auth::create_user(&state, "viewer", "refresh-viewer-password").unwrap();
    let admin = auth::authenticate(&state, "admin", "refresh-admin-password", "server")
        .unwrap()
        .unwrap();
    let viewer = auth::authenticate(&state, "viewer", "refresh-viewer-password", "server")
        .unwrap()
        .unwrap();
    auth::grant_library(&state, "viewer", &movies).unwrap();
    let router = app::router(Arc::new(AppState::with_auth_dir(
        "server".to_owned(),
        state.clone(),
    )));
    let route = "/Library/Refresh";
    assert_eq!(
        call(&router, "POST", route, None).await.status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        call(&router, "POST", route, Some(&viewer.access_token))
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        call(&router, "GET", route, Some(&admin.access_token))
            .await
            .status(),
        StatusCode::METHOD_NOT_ALLOWED
    );

    std::fs::remove_file(movies.join("A.mp4")).unwrap();
    std::fs::write(movies.join("B.mkv"), b"new-film").unwrap();
    std::fs::write(music.join("Next.mp3"), b"soundtrack").unwrap();
    let refreshed = call(&router, "POST", route, Some(&admin.access_token)).await;
    assert_eq!(refreshed.status(), StatusCode::NO_CONTENT);
    assert_eq!(
        axum::body::to_bytes(refreshed.into_body(), 1024)
            .await
            .unwrap()
            .len(),
        0
    );
    let video = catalog::list(&state, &movies).unwrap();
    assert_eq!(video.len(), 1);
    assert_eq!(video[0].relative_path, "B.mkv");
    let audio = catalog::list(&state, &music).unwrap();
    assert_eq!(audio.len(), 2);
    assert!(audio.iter().any(|v| v.relative_path == "Next.mp3"));
    assert_eq!(catalog::library_roots(&state).unwrap().len(), 2);
    assert!(!catalog::all_files(&state)
        .unwrap()
        .iter()
        .any(|v| v.relative_path == "Private.mp4"));
    assert_eq!(
        std::fs::read(movies.join("B.mkv")).unwrap(),
        b"new-film"
    );

    // All roots must pass preflight before even one existing root is changed.
    std::fs::remove_file(music.join("Song.flac")).unwrap();
    std::fs::remove_file(music.join("Next.mp3")).unwrap();
    std::fs::remove_dir(&music).unwrap();
    std::fs::write(movies.join("C.mkv"), b"not-yet-indexed").unwrap();
    assert_eq!(
        call(&router, "POST", route, Some(&admin.access_token))
            .await
            .status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
    assert_eq!(catalog::list(&state, &movies).unwrap().len(), 1);
    assert_eq!(catalog::all_files(&state).unwrap().len(), 3);
    std::fs::create_dir_all(&music).unwrap();

    let restarted = app::router(Arc::new(AppState::with_auth_dir(
        "server".to_owned(),
        state.clone(),
    )));
    assert_eq!(
        call(&restarted, "POST", route, Some(&admin.access_token))
            .await
            .status(),
        StatusCode::NO_CONTENT
    );
    let files = catalog::list(&state, &movies).unwrap();
    assert_eq!(files.len(), 2);
    assert!(files.iter().any(|file| file.relative_path == "C.mkv"));
    assert!(catalog::list(&state, &music).unwrap().is_empty());
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn refresh_rejects_registered_root_redirected_to_another_directory() {
    use std::os::unix::fs::symlink;

    let fixture = tempfile::tempdir().unwrap();
    let state = fixture.path().join("state");
    let root = fixture.path().join("Media");
    let moved = fixture.path().join("Original");
    let outside = fixture.path().join("Outside");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::create_dir_all(&outside).unwrap();
    std::fs::write(root.join("Movie.mkv"), b"original").unwrap();
    std::fs::write(outside.join("Secret.mp4"), b"private").unwrap();
    catalog::index(&state, &root).unwrap();
    auth::create_user(&state, "admin", "refresh-symlink-password").unwrap();
    let admin = auth::authenticate(&state, "admin", "refresh-symlink-password", "server")
        .unwrap()
        .unwrap();
    std::fs::rename(&root, &moved).unwrap();
    symlink(&outside, &root).unwrap();
    let router = app::router(Arc::new(AppState::with_auth_dir(
        "server".to_owned(),
        state.clone(),
    )));
    assert_eq!(
        call(
            &router,
            "POST",
            "/Library/Refresh",
            Some(&admin.access_token),
        )
        .await
        .status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
    let records = catalog::all_files(&state).unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].relative_path, "Movie.mkv");
    assert_eq!(std::fs::read(outside.join("Secret.mp4")).unwrap(), b"private");
}
