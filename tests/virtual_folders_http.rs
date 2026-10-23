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

async fn call(router: &Router, method: &str, path: &str, token: Option<&str>) -> Response {
    let mut request = Request::builder().method(method).uri(path);
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
async fn admin_virtual_folder_inventory_uses_only_indexed_roots_and_stable_item_ids() {
    let fixture = tempfile::tempdir().unwrap();
    let state = fixture.path().join("state");
    let movies = fixture.path().join("Movies");
    let empty = fixture.path().join("Empty");
    std::fs::create_dir_all(&movies).unwrap();
    std::fs::create_dir_all(&empty).unwrap();
    std::fs::write(movies.join("Film.mp4"), b"movie").unwrap();
    catalog::index(&state, &movies).unwrap();
    catalog::index(&state, &empty).unwrap();
    auth::create_user(&state, "admin", "virtual-admin-password").unwrap();
    auth::create_user(&state, "viewer", "virtual-viewer-password").unwrap();
    let admin = auth::authenticate(&state, "admin", "virtual-admin-password", "server")
        .unwrap()
        .unwrap();
    let viewer = auth::authenticate(&state, "viewer", "virtual-viewer-password", "server")
        .unwrap()
        .unwrap();
    auth::grant_library(&state, "viewer", &movies).unwrap();
    let router = app::router(Arc::new(AppState::with_auth_dir(
        "server".to_owned(),
        state.clone(),
    )));
    let route = "/Library/VirtualFolders";
    assert_eq!(
        call(&router, "GET", route, None).await.status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        call(&router, "GET", route, Some(&viewer.access_token))
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        call(&router, "POST", route, Some(&admin.access_token))
            .await
            .status(),
        StatusCode::BAD_REQUEST
    );
    let folders = json(call(&router, "GET", route, Some(&admin.access_token)).await).await;
    let array = folders.as_array().unwrap();
    assert_eq!(array.len(), 2);
    assert_eq!(array[0]["Name"], "Empty");
    assert_eq!(array[1]["Name"], "Movies");
    for (entry, root) in array.iter().zip([&empty, &movies]) {
        let canonical = std::fs::canonicalize(root).unwrap();
        assert_eq!(entry["Locations"][0], canonical.to_str().unwrap());
        let id = entry["ItemId"].as_str().unwrap();
        assert_eq!(id.len(), 32);
        assert!(entry.get("CollectionType").is_none());
        assert!(entry.get("LibraryOptions").is_none());
        assert!(entry.get("RefreshProgress").is_none());
        assert!(entry.get("PrimaryImageItemId").is_none());
    }

    let media = json(
        call(
            &router,
            "GET",
            "/Library/MediaFolders",
            Some(&admin.access_token),
        )
        .await,
    )
    .await;
    let items = media["Items"].as_array().unwrap();
    assert_eq!(items.len(), 2);
    for entry in array {
        let name = entry["Name"].as_str().unwrap();
        let listed = items.iter().find(|item| item["Name"] == name).unwrap();
        assert_eq!(entry["ItemId"], listed["Id"]);
    }
    let viewer_views =
        json(call(&router, "GET", "/UserViews", Some(&viewer.access_token)).await).await;
    assert_eq!(viewer_views["TotalRecordCount"], 1);
    assert!(viewer_views["Items"][0].get("Path").is_none());

    // A rescan cannot assign a different Jellyfin collection identifier.
    catalog::index(&state, &movies).unwrap();
    let after_rescan = json(call(&router, "GET", route, Some(&admin.access_token)).await).await;
    assert_eq!(after_rescan, folders);

    let restarted = app::router(Arc::new(AppState::with_auth_dir(
        "server".to_owned(),
        state.clone(),
    )));
    let after_restart = json(call(&restarted, "GET", route, Some(&admin.access_token)).await).await;
    assert_eq!(after_restart, folders);
    auth::revoke_library(&state, "viewer", &movies).unwrap();
    assert_eq!(
        call(&restarted, "GET", route, Some(&viewer.access_token))
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
}
