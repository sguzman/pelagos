use std::sync::Arc;

use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode},
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
    body: &str,
) -> Response {
    let mut builder = Request::builder().method(method).uri(route);
    if let Some(token) = token {
        builder = builder.header("X-Emby-Token", token);
    }
    router
        .clone()
        .oneshot(builder.body(Body::from(body.to_owned())).unwrap())
        .await
        .unwrap()
}

async fn json(response: Response) -> Value {
    serde_json::from_slice(&to_bytes(response.into_body(), 65536).await.unwrap()).unwrap()
}

async fn view_count(router: &Router, token: &str) -> i64 {
    json(call(router, "GET", "/UserViews", Some(token), "").await)
        .await["TotalRecordCount"]
        .as_i64()
        .unwrap()
}

#[tokio::test]
async fn deleting_registered_virtual_folder_clears_permissions_and_preserves_media() {
    let fixture = tempfile::tempdir().unwrap();
    let state = fixture.path().join("state");
    let movies = fixture.path().join("Movies");
    let music = fixture.path().join("Music");
    std::fs::create_dir_all(&movies).unwrap();
    std::fs::create_dir_all(&music).unwrap();
    std::fs::write(movies.join("Film.mkv"), b"film").unwrap();
    std::fs::write(music.join("Song.flac"), b"song").unwrap();
    catalog::index(&state, &movies).unwrap();
    catalog::index(&state, &music).unwrap();
    auth::create_user(&state, "admin", "remove-library-admin-password").unwrap();
    auth::create_user(&state, "viewer", "remove-library-viewer-password").unwrap();
    let admin = auth::authenticate(&state, "admin", "remove-library-admin-password", "server")
        .unwrap()
        .unwrap();
    let viewer = auth::authenticate(&state, "viewer", "remove-library-viewer-password", "server")
        .unwrap()
        .unwrap();
    auth::grant_library(&state, "viewer", &movies).unwrap();
    let router = app::router(Arc::new(AppState::with_auth_dir(
        "server".to_owned(),
        state.clone(),
    )));
    let remove = "/Library/VirtualFolders?name=Movies";
    for (method, token, status) in [
        ("DELETE", None, StatusCode::UNAUTHORIZED),
        (
            "DELETE",
            Some(viewer.access_token.as_str()),
            StatusCode::FORBIDDEN,
        ),
    ] {
        assert_eq!(
            call(&router, method, remove, token, "").await.status(),
            status
        );
    }
    assert_eq!(
        call(
            &router,
            "DELETE",
            "/Library/VirtualFolders",
            Some(&admin.access_token),
            "",
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        call(
            &router,
            "DELETE",
            "/Library/VirtualFolders?name=Missing",
            Some(&admin.access_token),
            "",
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
    for (path, body) in [
        (
            "/Library/VirtualFolders?name=Movies&refreshLibrary=true",
            "",
        ),
        (remove, "{}"),
    ] {
        assert_eq!(
            call(&router, "DELETE", path, Some(&admin.access_token), body)
                .await
                .status(),
            StatusCode::NOT_IMPLEMENTED
        );
    }
    assert_eq!(view_count(&router, &viewer.access_token).await, 1);
    let response = call(&router, "DELETE", remove, Some(&admin.access_token), "").await;
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert!(
        to_bytes(response.into_body(), 512)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(catalog::library_roots(&state).unwrap().len(), 1);
    assert!(catalog::list(&state, &movies).unwrap().is_empty());
    assert_eq!(catalog::list(&state, &music).unwrap().len(), 1);
    assert_eq!(std::fs::read(movies.join("Film.mkv")).unwrap(), b"film");
    assert_eq!(
        auth::visible_library_roots(&state, &viewer.user)
            .unwrap()
            .unwrap()
            .len(),
        0
    );
    assert_eq!(
        call(&router, "DELETE", remove, Some(&admin.access_token), "")
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    let restarted = app::router(Arc::new(AppState::with_auth_dir(
        "server".to_owned(),
        state.clone(),
    )));
    assert_eq!(view_count(&restarted, &viewer.access_token).await, 0);

    // Re-adding an identical path must never resurrect obsolete read grants.
    let register = format!(
        "/Library/VirtualFolders?name=Movies&paths={}",
        movies.display()
    );
    assert_eq!(
        call(&restarted, "POST", &register, Some(&admin.access_token), "")
            .await
            .status(),
        StatusCode::NO_CONTENT
    );
    assert_eq!(view_count(&restarted, &viewer.access_token).await, 0);
    assert!(auth::grant_library(&state, "viewer", &movies).unwrap());
    assert_eq!(view_count(&restarted, &viewer.access_token).await, 1);
}

#[tokio::test]
async fn removal_handles_offline_roots_and_refuses_ambiguous_library_names() {
    let fixture = tempfile::tempdir().unwrap();
    let state = fixture.path().join("state");
    let root = fixture.path().join("Disconnected");
    let relocated = fixture.path().join("SavedMedia");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("A.mkv"), b"original").unwrap();
    catalog::index(&state, &root).unwrap();
    auth::create_user(&state, "admin", "remove-offline-admin-password").unwrap();
    let admin = auth::authenticate(&state, "admin", "remove-offline-admin-password", "server")
        .unwrap()
        .unwrap();
    std::fs::rename(&root, &relocated).unwrap();
    let router = app::router(Arc::new(AppState::with_auth_dir(
        "server".to_owned(),
        state.clone(),
    )));
    assert_eq!(
        call(
            &router,
            "DELETE",
            "/Library/VirtualFolders?name=Disconnected",
            Some(&admin.access_token),
            "",
        )
        .await
        .status(),
        StatusCode::NO_CONTENT
    );
    assert!(catalog::all_files(&state).unwrap().is_empty());
    assert_eq!(std::fs::read(relocated.join("A.mkv")).unwrap(), b"original");

    let first = fixture.path().join("One/Media");
    let second = fixture.path().join("Two/Media");
    std::fs::create_dir_all(&first).unwrap();
    std::fs::create_dir_all(&second).unwrap();
    std::fs::write(first.join("X.mp4"), b"x").unwrap();
    std::fs::write(second.join("Y.mp4"), b"y").unwrap();
    catalog::index(&state, &first).unwrap();
    catalog::index(&state, &second).unwrap();
    assert_eq!(
        call(
            &router,
            "DELETE",
            "/Library/VirtualFolders?name=Media",
            Some(&admin.access_token),
            "",
        )
        .await
        .status(),
        StatusCode::CONFLICT
    );
    assert_eq!(catalog::library_roots(&state).unwrap().len(), 2);
    assert_eq!(catalog::all_files(&state).unwrap().len(), 2);
}
