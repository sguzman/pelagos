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
async fn item_query_options_preserve_scoping_sorting_and_requested_dto_fields() {
    let fixture = tempfile::tempdir().unwrap();
    let state = fixture.path().join("state");
    let root = fixture.path().join("Movies");
    std::fs::create_dir(&root).unwrap();
    std::fs::write(root.join("zeta.mp4"), b"fixture").unwrap();
    std::fs::write(root.join("Alpha.mp4"), b"fixture").unwrap();
    std::fs::write(root.join("beta.mp4"), b"fixture").unwrap();
    catalog::index(&state, &root).unwrap();
    auth::create_user(&state, "admin", "admin-sort-test-password").unwrap();
    auth::create_user(&state, "viewer", "viewer-sort-test-password").unwrap();
    let admin = auth::authenticate(&state, "admin", "admin-sort-test-password", "server")
        .unwrap()
        .unwrap();
    let viewer = auth::authenticate(&state, "viewer", "viewer-sort-test-password", "server")
        .unwrap()
        .unwrap();
    let router = app::router(Arc::new(AppState::with_auth_dir(
        "server".to_owned(),
        state.clone(),
    )));
    assert_eq!(
        get(&router, "/Items?sortBy=SortName", None).await.status(),
        StatusCode::UNAUTHORIZED
    );
    let hidden = json(
        get(
            &router,
            "/Items?sortBy=SortName",
            Some(&viewer.access_token),
        )
        .await,
    )
    .await;
    assert_eq!(hidden["TotalRecordCount"], 0);
    auth::grant_library(&state, "viewer", &root).unwrap();

    let sorted = json(
        get(
            &router,
            "/Items?sortBy=SortName",
            Some(&viewer.access_token),
        )
        .await,
    )
    .await;
    let names: Vec<_> = sorted["Items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["Name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["Alpha", "beta", "zeta"]);
    assert!(sorted["Items"][0]["UserData"].is_object());

    let descending = json(
        get(
            &router,
            "/Items?sortBy=SortName&sortOrder=Descending&limit=2&startIndex=1&enableUserData=false&enableImages=false",
            Some(&viewer.access_token),
        )
        .await,
    )
    .await;
    assert_eq!(descending["TotalRecordCount"], 3);
    assert_eq!(descending["StartIndex"], 1);
    let page = descending["Items"].as_array().unwrap();
    assert_eq!(page.len(), 2);
    assert_eq!(page[0]["Name"], "beta");
    assert_eq!(page[1]["Name"], "Alpha");
    assert!(page.iter().all(|item| item.get("UserData").is_none()));
    assert!(page.iter().all(|item| item.get("ImageTags").is_none()));
    let scoped = json(
        get(
            &router,
            &format!("/Items?userId={}&sortOrder=Ascending", viewer.user.id),
            Some(&viewer.access_token),
        )
        .await,
    )
    .await;
    assert_eq!(scoped["TotalRecordCount"], 3);
    assert_eq!(
        get(
            &router,
            &format!("/Items?userId={}", admin.user.id),
            Some(&viewer.access_token),
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    for unsupported in [
        "/Items?sortBy=DatePlayed",
        "/Items?sortOrder=Sideways",
        "/Items?fields=Chapters",
        "/Items?imageTypeLimit=1",
        "/Items?enableTotalRecordCount=false",
    ] {
        assert_eq!(
            get(&router, unsupported, Some(&viewer.access_token))
                .await
                .status(),
            StatusCode::NOT_IMPLEMENTED
        );
    }
    auth::revoke_library(&state, "viewer", &root).unwrap();
    let gone = json(
        get(
            &router,
            "/Items?sortBy=SortName",
            Some(&viewer.access_token),
        )
        .await,
    )
    .await;
    assert_eq!(gone["TotalRecordCount"], 0);
}
