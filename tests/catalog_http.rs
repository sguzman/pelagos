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

async fn get(router: &Router, path: &str, token: Option<&str>) -> Response {
    let mut request = Request::builder().uri(path);
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
    let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

#[tokio::test]
async fn query_results_are_administrator_only_and_deterministically_paged() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path().join("Movies");
    let state = fixture.path().join("state");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("Alpha.mp4"), b"synthetic").unwrap();
    std::fs::write(root.join("Bravo.mkv"), b"synthetic").unwrap();
    catalog::index(&state, &root).unwrap();

    auth::create_user(&state, "admin", "admin-safe-test-passphrase").unwrap();
    auth::create_user(&state, "viewer", "viewer-safe-test-passphrase").unwrap();
    let admin_token = auth::authenticate(&state, "admin", "admin-safe-test-passphrase", "server")
        .unwrap()
        .unwrap()
        .access_token;
    let viewer_token =
        auth::authenticate(&state, "viewer", "viewer-safe-test-passphrase", "server")
            .unwrap()
            .unwrap()
            .access_token;
    let router = app::router(Arc::new(AppState::with_auth_dir(
        "server".to_string(),
        state.clone(),
    )));

    let denied = get(&router, "/Items", None).await;
    assert_eq!(denied.status(), StatusCode::UNAUTHORIZED);
    let viewer = get(&router, "/Items", Some(&viewer_token)).await;
    assert_eq!(viewer.status(), StatusCode::FORBIDDEN);

    let all = json(get(&router, "/Items", Some(&admin_token)).await).await;
    assert_eq!(all["TotalRecordCount"], 2);
    assert_eq!(all["Items"][0]["Name"], "Alpha");
    assert_eq!(all["Items"][1]["Name"], "Bravo");
    assert!(all["Items"][0].get("Path").is_none());

    let page = json(get(&router, "/Items?startIndex=1&limit=1", Some(&admin_token)).await).await;
    assert_eq!(page["TotalRecordCount"], 2);
    assert_eq!(page["StartIndex"], 1);
    assert_eq!(page["Items"].as_array().unwrap().len(), 1);
    assert_eq!(page["Items"][0]["Name"], "Bravo");

    let search = json(get(&router, "/Items?searchTerm=alpha", Some(&admin_token)).await).await;
    assert_eq!(search["TotalRecordCount"], 1);

    let views = json(get(&router, "/UserViews", Some(&admin_token)).await).await;
    assert_eq!(views["TotalRecordCount"], 1);
    assert_eq!(views["Items"][0]["Name"], "Movies");
    assert!(views["Items"][0].get("Path").is_none());
}


#[tokio::test]
async fn folder_views_and_recursive_browse_match_parent_chain() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path().join("Media");
    let state = fixture.path().join("state");
    std::fs::create_dir_all(root.join("Series/Season 1")).unwrap();
    std::fs::write(root.join("Series/Season 1/Pilot.mkv"), b"synthetic").unwrap();
    std::fs::write(root.join("StandAlone.mp4"), b"synthetic").unwrap();
    catalog::index(&state, &root).unwrap();
    auth::create_user(&state, "admin", "very-long-test-password").unwrap();
    let token = auth::authenticate(&state, "admin", "very-long-test-password", "server")
        .unwrap()
        .unwrap()
        .access_token;
    let router = app::router(Arc::new(AppState::with_auth_dir(
        "server".to_owned(),
        state,
    )));
    let views = json(get(&router, "/UserViews", Some(&token)).await).await;
    let root_id = views["Items"][0]["Id"].as_str().unwrap();
    let direct = json(
        get(&router, &format!("/Items?parentId={root_id}"), Some(&token)).await,
    )
    .await;
    assert_eq!(direct["TotalRecordCount"], 2);
    let items = direct["Items"].as_array().unwrap();
    let folder_id = items
        .iter()
        .find(|item| item["Name"] == "Series")
        .unwrap()["Id"]
        .as_str()
        .unwrap();
    let nested = json(
        get(
            &router,
            &format!("/Items?parentId={folder_id}&recursive=true"),
            Some(&token),
        )
        .await,
    )
    .await;
    assert_eq!(nested["TotalRecordCount"], 1);
    assert_eq!(nested["Items"][0]["Name"], "Pilot");
    let id = nested["Items"][0]["Id"].as_str().unwrap();
    let direct_by_id = json(
        get(&router, &format!("/Items/{id}"), Some(&token)).await,
    )
    .await;
    assert_eq!(direct_by_id["Id"], id);
    let missing = get(&router, "/Items?parentId=00000000000000000000000000000000", Some(&token)).await;
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);
}
