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

async fn get(router: &Router, path: &str, verb: &str, token: Option<&str>) -> Response {
    let mut request = Request::builder().uri(path).method(verb);
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
    let body = to_bytes(response.into_body(), 65536).await.unwrap();
    serde_json::from_slice(&body).unwrap()
}

#[tokio::test]
async fn collection_and_nested_folders_expose_and_serve_primary_artwork() {
    let fixture = tempfile::tempdir().unwrap();
    let state = fixture.path().join("state");
    let library = fixture.path().join("music");
    let album = library.join("Album");
    std::fs::create_dir_all(&album).unwrap();
    std::fs::write(library.join("folder.jpg"), b"LIBRARY COVER").unwrap();
    std::fs::write(album.join("cover.png"), b"ALBUM COVER").unwrap();
    std::fs::write(album.join("Song.flac"), b"audio").unwrap();
    catalog::index(&state, &library).unwrap();
    auth::create_user(&state, "admin", "folder-art-admin-password").unwrap();
    auth::create_user(&state, "viewer", "folder-art-viewer-password").unwrap();
    let admin = auth::authenticate(&state, "admin", "folder-art-admin-password", "server")
        .unwrap()
        .unwrap();
    let viewer = auth::authenticate(&state, "viewer", "folder-art-viewer-password", "server")
        .unwrap()
        .unwrap();
    let router = app::router(Arc::new(AppState::with_auth_dir(
        "server".to_owned(),
        state.clone(),
    )));

    let views = json(get(&router, "/UserViews", "GET", Some(&admin.access_token)).await).await;
    assert_eq!(views["TotalRecordCount"], 1);
    let root = &views["Items"][0];
    let root_id = root["Id"].as_str().unwrap();
    assert!(root["ImageTags"]["Primary"].as_str().is_some());
    let root_art = get(
        &router,
        &format!("/Items/{root_id}/Images/Primary"),
        "GET",
        Some(&admin.access_token),
    )
    .await;
    assert_eq!(root_art.status(), StatusCode::OK);
    assert_eq!(root_art.headers()[header::CONTENT_TYPE], "image/jpeg");
    assert_eq!(
        to_bytes(root_art.into_body(), 100).await.unwrap().as_ref(),
        b"LIBRARY COVER"
    );
    assert_eq!(
        get(
            &router,
            &format!("/Items/{root_id}/Images/Primary/1"),
            "GET",
            Some(&admin.access_token),
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );

    let children = json(
        get(
            &router,
            &format!("/Items?parentId={root_id}"),
            "GET",
            Some(&admin.access_token),
        )
        .await,
    )
    .await;
    let album_node = children["Items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["Name"] == "Album" && item["IsFolder"] == true)
        .unwrap();
    assert!(album_node["ImageTags"]["Primary"].as_str().is_some());
    let album_id = album_node["Id"].as_str().unwrap();
    let album_art = get(
        &router,
        &format!("/Items/{album_id}/Images/Primary/0"),
        "GET",
        Some(&admin.access_token),
    )
    .await;
    assert_eq!(album_art.status(), StatusCode::OK);
    assert_eq!(album_art.headers()[header::CONTENT_TYPE], "image/png");
    assert_eq!(
        to_bytes(album_art.into_body(), 100).await.unwrap().as_ref(),
        b"ALBUM COVER"
    );
    assert_eq!(
        get(
            &router,
            &format!("/Items/{album_id}/Images/Primary"),
            "GET",
            Some(&viewer.access_token),
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
    auth::grant_library(&state, "viewer", &library).unwrap();
    let viewer_views =
        json(get(&router, "/UserViews", "GET", Some(&viewer.access_token)).await).await;
    assert_eq!(
        viewer_views["Items"][0]["ImageTags"]["Primary"],
        root["ImageTags"]["Primary"]
    );
    assert_eq!(
        get(
            &router,
            &format!("/Items/{album_id}/Images/Primary"),
            "HEAD",
            Some(&viewer.access_token),
        )
        .await
        .status(),
        StatusCode::OK
    );
    auth::revoke_library(&state, "viewer", &library).unwrap();
    assert_eq!(
        get(
            &router,
            &format!("/Items/{album_id}/Images/Primary"),
            "GET",
            Some(&viewer.access_token),
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
}
