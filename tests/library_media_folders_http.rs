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

async fn get(router: &Router, route: &str, token: Option<&str>) -> Response {
    let mut request = Request::builder().uri(route);
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
async fn library_media_folders_requires_admin_and_exposes_only_indexed_visible_roots() {
    let temp = tempfile::tempdir().unwrap();
    let state = temp.path().join("state");
    let movies = temp.path().join("Movies");
    let empty = temp.path().join("Empty");
    std::fs::create_dir(&movies).unwrap();
    std::fs::create_dir(&empty).unwrap();
    std::fs::write(movies.join("Film.mkv"), b"synthetic").unwrap();
    catalog::index(&state, &movies).unwrap();
    catalog::index(&state, &empty).unwrap();

    auth::create_user(&state, "admin", "admin-folders-test-password").unwrap();
    auth::create_user(&state, "viewer", "viewer-folders-test-password").unwrap();
    let admin = auth::authenticate(&state, "admin", "admin-folders-test-password", "server")
        .unwrap()
        .unwrap();
    let viewer = auth::authenticate(&state, "viewer", "viewer-folders-test-password", "server")
        .unwrap()
        .unwrap();
    let router = app::router(Arc::new(AppState::with_auth_dir(
        "server".to_owned(),
        state.clone(),
    )));
    assert_eq!(
        get(&router, "/Library/MediaFolders", None)
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        get(
            &router,
            "/Library/MediaFolders",
            Some(&viewer.access_token),
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    auth::grant_library(&state, "viewer", &movies).unwrap();
    assert_eq!(
        get(
            &router,
            "/Library/MediaFolders",
            Some(&viewer.access_token),
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    let folders = json(
        get(
            &router,
            "/Library/MediaFolders?isHidden=false",
            Some(&admin.access_token),
        )
        .await,
    )
    .await;
    assert_eq!(folders["TotalRecordCount"], 2);
    assert_eq!(folders["StartIndex"], 0);
    let items = folders["Items"].as_array().unwrap();
    assert_eq!(items[0]["Name"], "Empty");
    assert_eq!(items[1]["Name"], "Movies");
    for item in items {
        assert_eq!(item["Type"], "CollectionFolder");
        assert!(item["IsFolder"].as_bool().unwrap());
        assert!(item.get("Path").is_none());
        assert!(item.get("UserData").is_none());
    }
    let root = json(get(&router, "/Items/Root", Some(&admin.access_token)).await).await;
    assert_eq!(items[0]["ParentId"], root["Id"]);
    assert_eq!(items[1]["ParentId"], root["Id"]);
    let hidden = json(
        get(
            &router,
            "/Library/MediaFolders?isHidden=true",
            Some(&admin.access_token),
        )
        .await,
    )
    .await;
    assert_eq!(hidden["TotalRecordCount"], 0);
    assert!(hidden["Items"].as_array().unwrap().is_empty());

    // Admin sees indexed libraries; library grants do not elevate the viewer.
    auth::revoke_library(&state, "viewer", &movies).unwrap();
    assert_eq!(
        get(
            &router,
            "/Library/MediaFolders",
            Some(&viewer.access_token),
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
}
