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

async fn body_json(response: Response) -> Value {
    let body = to_bytes(response.into_body(), 65536).await.unwrap();
    serde_json::from_slice(&body).unwrap()
}

#[tokio::test]
async fn jellyfin_hints_obey_library_grants_filters_and_pagination() {
    let temp = tempfile::tempdir().unwrap();
    let state = temp.path().join("state");
    let public = temp.path().join("public");
    let private = temp.path().join("private");
    std::fs::create_dir(&public).unwrap();
    std::fs::create_dir(&private).unwrap();
    std::fs::write(public.join("Film A.mp4"), b"film").unwrap();
    std::fs::write(public.join("Film B.mkv"), b"film").unwrap();
    std::fs::write(public.join("Cover.jpg"), b"image").unwrap();
    std::fs::write(private.join("Secret Film.mp4"), b"private").unwrap();
    catalog::index(&state, &public).unwrap();
    catalog::index(&state, &private).unwrap();
    auth::create_user(&state, "admin", "admin-hints-test-password").unwrap();
    auth::create_user(&state, "viewer", "viewer-hints-test-password").unwrap();
    let admin = auth::authenticate(&state, "admin", "admin-hints-test-password", "server")
        .unwrap()
        .unwrap();
    let viewer = auth::authenticate(&state, "viewer", "viewer-hints-test-password", "server")
        .unwrap()
        .unwrap();
    let router = app::router(Arc::new(AppState::with_auth_dir(
        "server".to_owned(),
        state.clone(),
    )));
    assert_eq!(
        get(&router, "/Search/Hints?searchTerm=Film", None)
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    let none = body_json(
        get(
            &router,
            "/Search/Hints?searchTerm=Film",
            Some(&viewer.access_token),
        )
        .await,
    )
    .await;
    assert_eq!(none["TotalRecordCount"], 0);

    auth::grant_library(&state, "viewer", &public).unwrap();
    let filtered = body_json(
        get(
            &router,
            "/Search/Hints?searchTerm=fIlM&includeItemTypes=Video",
            Some(&viewer.access_token),
        )
        .await,
    )
    .await;
    assert_eq!(filtered["TotalRecordCount"], 2);
    assert_eq!(filtered["SearchHints"][0]["Name"], "Film A");
    assert_eq!(filtered["SearchHints"][0]["Type"], "Video");
    assert_eq!(
        filtered["SearchHints"][0]["ItemId"],
        filtered["SearchHints"][0]["Id"]
    );
    assert!(filtered["SearchHints"][0].get("Path").is_none());

    let page = body_json(
        get(
            &router,
            "/Search/Hints?searchTerm=Film&limit=1&startIndex=1",
            Some(&viewer.access_token),
        )
        .await,
    )
    .await;
    assert_eq!(page["TotalRecordCount"], 2);
    assert_eq!(page["SearchHints"].as_array().unwrap().len(), 1);
    assert_eq!(page["SearchHints"][0]["Name"], "Film B");

    let no_video = body_json(
        get(
            &router,
            "/Search/Hints?searchTerm=Film&excludeItemTypes=Video",
            Some(&viewer.access_token),
        )
        .await,
    )
    .await;
    assert_eq!(no_video["TotalRecordCount"], 0);

    let photo = body_json(
        get(
            &router,
            "/Search/Hints?searchTerm=Cover&mediaTypes=Photo",
            Some(&viewer.access_token),
        )
        .await,
    )
    .await;
    assert_eq!(photo["TotalRecordCount"], 1);
    assert_eq!(photo["SearchHints"][0]["Name"], "Cover");

    let views = body_json(get(&router, "/UserViews", Some(&viewer.access_token)).await).await;
    let root_id = views["Items"][0]["Id"].as_str().unwrap();
    let parent_hints = body_json(
        get(
            &router,
            &format!("/Search/Hints?searchTerm=Film&parentId={root_id}"),
            Some(&viewer.access_token),
        )
        .await,
    )
    .await;
    assert_eq!(parent_hints["TotalRecordCount"], 2);

    let admin_matches = body_json(
        get(
            &router,
            "/Search/Hints?searchTerm=film",
            Some(&admin.access_token),
        )
        .await,
    )
    .await;
    assert_eq!(admin_matches["TotalRecordCount"], 3);
    let rejected = get(
        &router,
        "/Search/Hints?searchTerm=film&includeItemTypes=Movie",
        Some(&viewer.access_token),
    )
    .await;
    assert_eq!(rejected.status(), StatusCode::NOT_IMPLEMENTED);
    let missing = get(&router, "/Search/Hints", Some(&viewer.access_token)).await;
    assert_eq!(missing.status(), StatusCode::BAD_REQUEST);
    let unavailable = get(
        &router,
        "/Search/Hints?searchTerm=film&isSeries=true",
        Some(&viewer.access_token),
    )
    .await;
    assert_eq!(unavailable.status(), StatusCode::NOT_IMPLEMENTED);
    auth::revoke_library(&state, "viewer", &public).unwrap();
    let after = body_json(
        get(
            &router,
            "/Search/Hints?searchTerm=Film",
            Some(&viewer.access_token),
        )
        .await,
    )
    .await;
    assert_eq!(after["TotalRecordCount"], 0);
}
