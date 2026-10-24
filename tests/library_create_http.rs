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
    let mut request = Request::builder().method(method).uri(route);
    if let Some(token) = token {
        request = request.header("X-Emby-Token", token);
    }
    router
        .clone()
        .oneshot(request.body(Body::from(body.to_owned())).unwrap())
        .await
        .unwrap()
}

async fn json(response: Response) -> Value {
    let bytes = to_bytes(response.into_body(), 65536).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

#[tokio::test]
async fn administrator_can_register_one_existing_canonical_library_without_affecting_others() {
    let fixture = tempfile::tempdir().unwrap();
    let state = fixture.path().join("state");
    let movies = fixture.path().join("Movies");
    let other = fixture.path().join("NeverRegistered");
    std::fs::create_dir_all(&movies).unwrap();
    std::fs::create_dir_all(&other).unwrap();
    std::fs::write(movies.join("Movie.mkv"), b"film").unwrap();
    std::fs::write(other.join("Secret.mkv"), b"private").unwrap();
    auth::create_user(&state, "admin", "create-library-admin-password").unwrap();
    auth::create_user(&state, "viewer", "create-library-viewer-password").unwrap();
    let admin = auth::authenticate(&state, "admin", "create-library-admin-password", "server")
        .unwrap()
        .unwrap();
    let viewer = auth::authenticate(&state, "viewer", "create-library-viewer-password", "server")
        .unwrap()
        .unwrap();
    let router = app::router(Arc::new(AppState::with_auth_dir(
        "server".to_owned(),
        state.clone(),
    )));
    let route = format!(
        "/Library/VirtualFolders?name=Movies&paths={}",
        movies.display()
    );

    assert_eq!(
        call(&router, "POST", &route, None, "").await.status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        call(&router, "POST", &route, Some(&viewer.access_token), "")
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    assert!(catalog::library_roots(&state).unwrap().is_empty());
    for invalid in [
        "/Library/VirtualFolders?name=Movies&paths=relative/path".to_owned(),
        format!(
            "/Library/VirtualFolders?name=Movies&paths={},{}",
            movies.display(),
            other.display()
        ),
        format!(
            "/Library/VirtualFolders?name=Movies&paths={}&collectionType=movies",
            movies.display()
        ),
        format!(
            "/Library/VirtualFolders?name=Movies&paths={}&refreshLibrary=true",
            movies.display()
        ),
    ] {
        let status = call(&router, "POST", &invalid, Some(&admin.access_token), "")
            .await
            .status();
        assert!(
            status == StatusCode::BAD_REQUEST || status == StatusCode::NOT_IMPLEMENTED,
            "{invalid}: {status}"
        );
    }
    assert_eq!(
        call(&router, "POST", &route, Some(&admin.access_token), "{}")
            .await
            .status(),
        StatusCode::NOT_IMPLEMENTED
    );
    assert!(catalog::library_roots(&state).unwrap().is_empty());

    let created = call(&router, "POST", &route, Some(&admin.access_token), "").await;
    assert_eq!(created.status(), StatusCode::NO_CONTENT);
    assert!(
        to_bytes(created.into_body(), 1024)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(catalog::library_roots(&state).unwrap().len(), 1);
    assert_eq!(catalog::list(&state, &movies).unwrap().len(), 1);
    assert!(
        !catalog::all_files(&state)
            .unwrap()
            .iter()
            .any(|file| file.relative_path == "Secret.mkv")
    );
    assert_eq!(
        call(&router, "POST", &route, Some(&admin.access_token), "")
            .await
            .status(),
        StatusCode::CONFLICT
    );
    let folders = json(
        call(
            &router,
            "GET",
            "/Library/VirtualFolders",
            Some(&admin.access_token),
            "",
        )
        .await,
    )
    .await;
    assert_eq!(folders.as_array().unwrap().len(), 1);
    assert_eq!(folders[0]["Name"], "Movies");
    assert_eq!(
        folders[0]["Locations"][0],
        std::fs::canonicalize(&movies).unwrap().to_str().unwrap()
    );
    let views =
        json(call(&router, "GET", "/UserViews", Some(&viewer.access_token), "").await).await;
    assert_eq!(views["TotalRecordCount"], 0);
    auth::grant_library(&state, "viewer", &movies).unwrap();
    let views =
        json(call(&router, "GET", "/UserViews", Some(&viewer.access_token), "").await).await;
    assert_eq!(views["TotalRecordCount"], 1);
    assert_eq!(views["Items"][0]["Id"], folders[0]["ItemId"]);

    let restarted = app::router(Arc::new(AppState::with_auth_dir(
        "server".to_owned(),
        state.clone(),
    )));
    let listed = json(
        call(
            &restarted,
            "GET",
            "/Library/VirtualFolders",
            Some(&admin.access_token),
            "",
        )
        .await,
    )
    .await;
    assert_eq!(listed, folders);
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn virtual_library_registration_rejects_symlinked_root_and_missing_paths() {
    use std::os::unix::fs::symlink;

    let fixture = tempfile::tempdir().unwrap();
    let state = fixture.path().join("state");
    let actual = fixture.path().join("Actual");
    let alias = fixture.path().join("Alias");
    std::fs::create_dir_all(&actual).unwrap();
    symlink(&actual, &alias).unwrap();
    auth::create_user(&state, "admin", "create-library-path-password").unwrap();
    let admin = auth::authenticate(&state, "admin", "create-library-path-password", "server")
        .unwrap()
        .unwrap();
    let router = app::router(Arc::new(AppState::with_auth_dir(
        "server".to_owned(),
        state.clone(),
    )));
    let route = format!(
        "/Library/VirtualFolders?name=Alias&paths={}",
        alias.display()
    );
    assert_eq!(
        call(&router, "POST", &route, Some(&admin.access_token), "")
            .await
            .status(),
        StatusCode::BAD_REQUEST
    );
    let missing = format!(
        "/Library/VirtualFolders?name=Missing&paths={}",
        fixture.path().join("Missing").display()
    );
    assert_eq!(
        call(&router, "POST", &missing, Some(&admin.access_token), "")
            .await
            .status(),
        StatusCode::BAD_REQUEST
    );
    assert!(catalog::library_roots(&state).unwrap().is_empty());
}

#[tokio::test]
async fn custom_library_names_are_registered_once_and_displayed_without_moving_directories() {
    let fixture = tempfile::tempdir().unwrap();
    let state = fixture.path().join("state");
    let root = fixture.path().join("PhysicalMedia");
    let second = fixture.path().join("OtherFiles");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::create_dir_all(&second).unwrap();
    std::fs::write(root.join("Film.mp4"), b"movie").unwrap();
    std::fs::write(second.join("Audio.mp3"), b"song").unwrap();
    auth::create_user(&state, "admin", "custom-library-admin-password").unwrap();
    auth::create_user(&state, "viewer", "custom-library-viewer-password").unwrap();
    let admin = auth::authenticate(&state, "admin", "custom-library-admin-password", "server")
        .unwrap()
        .unwrap();
    let viewer = auth::authenticate(&state, "viewer", "custom-library-viewer-password", "server")
        .unwrap()
        .unwrap();
    let router = app::router(Arc::new(AppState::with_auth_dir(
        "server".to_owned(),
        state.clone(),
    )));
    let route = format!(
        "/Library/VirtualFolders?name=Cinema&paths={}",
        root.display()
    );
    assert_eq!(
        call(&router, "POST", &route, Some(&admin.access_token), "")
            .await
            .status(),
        StatusCode::NO_CONTENT
    );
    let entries = catalog::library_entries(&state).unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].name, "Cinema");
    assert_eq!(std::fs::read(root.join("Film.mp4")).unwrap(), b"movie");
    let folders = json(
        call(
            &router,
            "GET",
            "/Library/VirtualFolders",
            Some(&admin.access_token),
            "",
        )
        .await,
    )
    .await;
    assert_eq!(folders[0]["Name"], "Cinema");
    assert_eq!(
        folders[0]["Locations"][0],
        std::fs::canonicalize(&root).unwrap().to_str().unwrap()
    );
    for duplicate in [
        format!("/Library/VirtualFolders?name=Another&paths={}", root.display()),
        format!("/Library/VirtualFolders?name=cinema&paths={}", second.display()),
    ] {
        assert_eq!(
            call(&router, "POST", &duplicate, Some(&admin.access_token), "")
                .await
                .status(),
            StatusCode::CONFLICT
        );
    }
    assert_eq!(catalog::library_entries(&state).unwrap().len(), 1);
    assert_eq!(catalog::all_files(&state).unwrap().len(), 1);
    assert_eq!(
        json(call(&router, "GET", "/UserViews", Some(&viewer.access_token), "").await)
            .await["TotalRecordCount"],
        0
    );
    auth::grant_library(&state, "viewer", &root).unwrap();
    let views = json(
        call(&router, "GET", "/UserViews", Some(&viewer.access_token), "").await,
    )
    .await;
    assert_eq!(views["Items"][0]["Name"], "Cinema");
    assert_eq!(views["Items"][0]["Id"], folders[0]["ItemId"]);
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
                "",
            )
            .await,
        )
        .await,
        folders
    );
}
