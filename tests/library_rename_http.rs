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

async fn json(response: Response) -> Value {
    serde_json::from_slice(&to_bytes(response.into_body(), 65536).await.unwrap()).unwrap()
}

#[tokio::test]
async fn rename_is_administrator_only_and_preserves_ids_grants_and_source_media() {
    let fixture = tempfile::tempdir().unwrap();
    let state = fixture.path().join("state");
    let movies = fixture.path().join("Movies");
    let music = fixture.path().join("Music");
    std::fs::create_dir_all(&movies).unwrap();
    std::fs::create_dir_all(&music).unwrap();
    std::fs::write(movies.join("Film.mkv"), b"film").unwrap();
    std::fs::write(music.join("Song.mp3"), b"song").unwrap();
    catalog::index(&state, &movies).unwrap();
    catalog::index(&state, &music).unwrap();
    auth::create_user(&state, "admin", "rename-library-admin-password").unwrap();
    auth::create_user(&state, "viewer", "rename-library-viewer-password").unwrap();
    let admin = auth::authenticate(&state, "admin", "rename-library-admin-password", "server")
        .unwrap()
        .unwrap();
    let viewer = auth::authenticate(&state, "viewer", "rename-library-viewer-password", "server")
        .unwrap()
        .unwrap();
    auth::grant_library(&state, "viewer", &movies).unwrap();
    let router = app::router(Arc::new(AppState::with_auth_dir(
        "server".to_owned(),
        state.clone(),
    )));
    let rename = "/Library/VirtualFolders/Name?name=Movies&newName=Cinema";
    assert_eq!(
        call(&router, "POST", rename, None).await.status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        call(&router, "POST", rename, Some(&viewer.access_token))
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    let before = json(
        call(
            &router,
            "GET",
            "/Library/VirtualFolders",
            Some(&admin.access_token),
        )
        .await,
    )
    .await;
    assert_eq!(before[0]["Name"], "Movies");
    let original_id = before[0]["ItemId"].clone();
    let physical_path = before[0]["Locations"][0].clone();

    for invalid in [
        "/Library/VirtualFolders/Name?name=Movies&newName=Music",
        "/Library/VirtualFolders/Name?name=Movies&newName=music",
    ] {
        assert_eq!(
            call(&router, "POST", invalid, Some(&admin.access_token))
                .await
                .status(),
            StatusCode::CONFLICT
        );
    }
    for invalid in [
        "/Library/VirtualFolders/Name?name=Movies",
        "/Library/VirtualFolders/Name?name=Movies&newName=..",
        "/Library/VirtualFolders/Name?name=Movies&newName=%2Ftmp",
        "/Library/VirtualFolders/Name?name=Movies&newName=%20Bad",
    ] {
        assert_eq!(
            call(&router, "POST", invalid, Some(&admin.access_token))
                .await
                .status(),
            StatusCode::BAD_REQUEST
        );
    }
    assert_eq!(
        call(
            &router,
            "POST",
            "/Library/VirtualFolders/Name?name=Missing&newName=Other",
            Some(&admin.access_token),
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        call(
            &router,
            "POST",
            "/Library/VirtualFolders/Name?name=Movies&newName=Cinema&refreshLibrary=true",
            Some(&admin.access_token),
        )
        .await
        .status(),
        StatusCode::NOT_IMPLEMENTED
    );
    let result = call(&router, "POST", rename, Some(&admin.access_token)).await;
    assert_eq!(result.status(), StatusCode::NO_CONTENT);
    assert!(to_bytes(result.into_body(), 1024).await.unwrap().is_empty());
    let after = json(
        call(
            &router,
            "GET",
            "/Library/VirtualFolders",
            Some(&admin.access_token),
        )
        .await,
    )
    .await;
    assert_eq!(after[0]["Name"], "Cinema");
    assert_eq!(after[0]["ItemId"], original_id);
    assert_eq!(after[0]["Locations"][0], physical_path);
    assert_eq!(catalog::library_roots(&state).unwrap().len(), 2);
    assert_eq!(std::fs::read(movies.join("Film.mkv")).unwrap(), b"film");

    let views = json(call(&router, "GET", "/UserViews", Some(&viewer.access_token)).await).await;
    assert_eq!(views["TotalRecordCount"], 1);
    assert_eq!(views["Items"][0]["Name"], "Cinema");
    assert_eq!(views["Items"][0]["Id"], original_id);

    let restarted = app::router(Arc::new(AppState::with_auth_dir(
        "server".to_owned(),
        state.clone(),
    )));
    assert_eq!(
        json(
            call(
                &restarted,
                "GET",
                "/Library/VirtualFolders",
                Some(&admin.access_token),
            )
            .await,
        )
        .await,
        after
    );
    assert_eq!(
        call(
            &restarted,
            "DELETE",
            "/Library/VirtualFolders?name=Movies",
            Some(&admin.access_token),
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        call(
            &restarted,
            "DELETE",
            "/Library/VirtualFolders?name=Cinema",
            Some(&admin.access_token),
        )
        .await
        .status(),
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        json(call(&restarted, "GET", "/UserViews", Some(&viewer.access_token)).await).await["TotalRecordCount"],
        0
    );
    assert_eq!(std::fs::read(movies.join("Film.mkv")).unwrap(), b"film");
}

#[test]
fn v3_catalog_migrates_to_named_v4_without_losing_existing_files() {
    let fixture = tempfile::tempdir().unwrap();
    let state = fixture.path().join("state");
    let library = fixture.path().join("Legacy");
    std::fs::create_dir_all(&library).unwrap();
    std::fs::write(library.join("Keep.mp4"), b"keep").unwrap();
    catalog::index(&state, &library).unwrap();
    let path = state.join("pelagos.sqlite3");
    let conn = rusqlite::Connection::open(&path).unwrap();
    conn.execute("ALTER TABLE libraries DROP COLUMN display_name", [])
        .unwrap();
    conn.execute_batch("PRAGMA user_version=3;").unwrap();
    drop(conn);
    let before = catalog::list(&state, &library).unwrap();
    assert_eq!(before.len(), 1);
    let root = std::fs::canonicalize(&library)
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let entries = catalog::library_entries(&state).unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].root, root);
    assert_eq!(entries[0].name, "Legacy");
    assert_eq!(catalog::list(&state, &library).unwrap().len(), 1);
    assert_eq!(
        catalog::rename_by_name(&state, "Legacy", "Archive").unwrap(),
        catalog::LibraryRename::Renamed
    );
    assert_eq!(catalog::library_entries(&state).unwrap()[0].name, "Archive");
    assert_eq!(catalog::list(&state, &library).unwrap().len(), 1);
}
