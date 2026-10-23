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

async fn get(router: &Router, path: &str, method: &str, token: Option<&str>) -> Response {
    let mut request = Request::builder().uri(path).method(method);
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
    let bytes = to_bytes(response.into_body(), 65536).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

#[tokio::test]
async fn poster_is_indexed_read_only_permission_scoped_and_discoverable() {
    let fixture = tempfile::tempdir().unwrap();
    let state = fixture.path().join("state");
    let library = fixture.path().join("movies");
    std::fs::create_dir(&library).unwrap();
    std::fs::write(library.join("Film.mkv"), b"movie data").unwrap();
    std::fs::write(library.join("Film.jpg"), b"TITLE ART").unwrap();
    std::fs::write(library.join("poster.png"), b"GENERIC ART").unwrap();
    catalog::index(&state, &library).unwrap();

    auth::create_user(&state, "admin", "artwork-admin-password").unwrap();
    auth::create_user(&state, "viewer", "artwork-viewer-password").unwrap();
    let administrator = auth::authenticate(&state, "admin", "artwork-admin-password", "server")
        .unwrap()
        .unwrap();
    let viewer = auth::authenticate(&state, "viewer", "artwork-viewer-password", "server")
        .unwrap()
        .unwrap();
    let router = app::router(Arc::new(AppState::with_auth_dir(
        "server".to_owned(),
        state.clone(),
    )));
    let admin_items =
        json(get(&router, "/Items", "GET", Some(&administrator.access_token)).await).await;
    let movie = admin_items["Items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["Name"] == "Film" && item["Type"] == "Video")
        .unwrap();
    let id = movie["Id"].as_str().unwrap();
    let first_tag = movie["ImageTags"]["Primary"].as_str().unwrap().to_owned();
    assert_eq!(first_tag.len(), 32);
    let info_route = format!("/Items/{id}/Images");
    let infos = json(
        get(
            &router,
            &info_route,
            "GET",
            Some(&administrator.access_token),
        )
        .await,
    )
    .await;
    assert_eq!(infos.as_array().unwrap().len(), 1);
    assert_eq!(infos[0]["ImageType"], "Primary");
    assert_eq!(infos[0]["ImageIndex"], Value::Null);
    assert_eq!(infos[0]["ImageTag"], first_tag);
    assert_eq!(infos[0]["Size"], 9);
    assert!(infos[0].get("Path").is_none());
    assert!(infos[0].get("Width").is_none());
    let missing = get(
        &router,
        "/Items/00000000000000000000000000000000/Images",
        "GET",
        Some(&administrator.access_token),
    )
    .await;
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);
    assert_eq!(
        get(&router, &info_route, "GET", None).await.status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        get(&router, &info_route, "GET", Some(&viewer.access_token))
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    let path = format!("/Items/{id}/Images/Primary");
    assert_eq!(
        get(&router, &path, "GET", None).await.status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        get(&router, &path, "GET", Some(&viewer.access_token))
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    auth::grant_library(&state, "viewer", &library).unwrap();
    let views = json(get(&router, "/Items", "GET", Some(&viewer.access_token)).await).await;
    let assigned = views["Items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["Name"] == "Film" && item["Type"] == "Video")
        .unwrap();
    assert_eq!(assigned["ImageTags"]["Primary"], first_tag);

    let response = get(&router, &path, "GET", Some(&viewer.access_token)).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[header::CONTENT_TYPE], "image/jpeg");
    assert_eq!(
        response.headers()[header::CACHE_CONTROL],
        "private, max-age=0"
    );
    assert_eq!(
        to_bytes(response.into_body(), 100).await.unwrap().as_ref(),
        b"TITLE ART"
    );

    let response = get(&router, &path, "HEAD", Some(&viewer.access_token)).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[header::CONTENT_LENGTH], "9");
    assert!(
        to_bytes(response.into_body(), 100)
            .await
            .unwrap()
            .is_empty()
    );

    let response = get(
        &router,
        &format!("/Items/{id}/Images/Backdrop"),
        "GET",
        Some(&viewer.access_token),
    )
    .await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    auth::revoke_library(&state, "viewer", &library).unwrap();
    assert_eq!(
        get(&router, &path, "GET", Some(&viewer.access_token))
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn swapped_primary_poster_symlink_is_rejected() {
    use std::os::unix::fs::symlink;
    let fixture = tempfile::tempdir().unwrap();
    let library = fixture.path().join("media");
    let state = fixture.path().join("state");
    std::fs::create_dir(&library).unwrap();
    std::fs::write(library.join("Video.mp4"), b"video").unwrap();
    std::fs::write(library.join("poster.jpg"), b"original").unwrap();
    let outside = fixture.path().join("private.jpg");
    std::fs::write(&outside, b"PRIVATE").unwrap();
    catalog::index(&state, &library).unwrap();
    auth::create_user(&state, "admin", "poster-symlink-password").unwrap();
    let token = auth::authenticate(&state, "admin", "poster-symlink-password", "server")
        .unwrap()
        .unwrap()
        .access_token;
    let router = app::router(Arc::new(AppState::with_auth_dir(
        "server".to_owned(),
        state,
    )));
    let list = json(get(&router, "/Items", "GET", Some(&token)).await).await;
    let id = list["Items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["Name"] == "Video")
        .unwrap()["Id"]
        .as_str()
        .unwrap()
        .to_owned();
    std::fs::remove_file(library.join("poster.jpg")).unwrap();
    symlink(&outside, library.join("poster.jpg")).unwrap();
    let response = get(
        &router,
        &format!("/Items/{id}/Images/Primary"),
        "GET",
        Some(&token),
    )
    .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}
