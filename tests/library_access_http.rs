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

async fn get(router: &Router, route: &str, token: &str) -> Response {
    router
        .clone()
        .oneshot(
            Request::builder()
                .uri(route)
                .header("X-Emby-Token", token)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
}

async fn read_json(response: Response) -> Value {
    let bytes = to_bytes(response.into_body(), 65536).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

#[tokio::test]
async fn library_grants_are_enforced_for_every_catalog_and_direct_play_surface() {
    let temp = tempfile::tempdir().unwrap();
    let state = temp.path().join("state");
    let public = temp.path().join("public");
    let private = temp.path().join("private");
    std::fs::create_dir(&public).unwrap();
    std::fs::create_dir(&private).unwrap();
    std::fs::write(public.join("Allowed.mp4"), b"ALLOWED").unwrap();
    std::fs::write(private.join("Secret.mp4"), b"SECRET").unwrap();
    catalog::index(&state, &public).unwrap();
    catalog::index(&state, &private).unwrap();

    auth::create_user(&state, "admin", "admin-local-test-password").unwrap();
    auth::create_user(&state, "viewer", "viewer-local-test-password").unwrap();
    let administrator = auth::authenticate(&state, "admin", "admin-local-test-password", "server")
        .unwrap()
        .unwrap();
    let viewer = auth::authenticate(&state, "viewer", "viewer-local-test-password", "server")
        .unwrap()
        .unwrap();
    let router = app::router(Arc::new(AppState::with_auth_dir(
        "server".to_owned(),
        state.clone(),
    )));
    let all = read_json(get(&router, "/Items", &administrator.access_token).await).await;
    assert_eq!(all["TotalRecordCount"], 2);
    let allowed_id = all["Items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["Name"] == "Allowed")
        .unwrap()["Id"]
        .as_str()
        .unwrap()
        .to_owned();
    let secret_id = all["Items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["Name"] == "Secret")
        .unwrap()["Id"]
        .as_str()
        .unwrap()
        .to_owned();
    let none = read_json(get(&router, "/Items", &viewer.access_token).await).await;
    assert_eq!(none["TotalRecordCount"], 0);
    let none_views = read_json(get(&router, "/UserViews", &viewer.access_token).await).await;
    assert_eq!(none_views["TotalRecordCount"], 0);
    assert_eq!(
        get(
            &router,
            &format!("/Items/{secret_id}"),
            &viewer.access_token
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );

    assert!(auth::grant_library(&state, "viewer", &public).unwrap());
    let visible = read_json(get(&router, "/Items", &viewer.access_token).await).await;
    assert_eq!(visible["TotalRecordCount"], 1);
    assert_eq!(visible["Items"][0]["Name"], "Allowed");
    let views = read_json(get(&router, "/UserViews", &viewer.access_token).await).await;
    assert_eq!(views["TotalRecordCount"], 1);
    assert_eq!(views["Items"][0]["Name"], "public");

    let video = get(
        &router,
        &format!("/Videos/{allowed_id}/stream?static=true"),
        &viewer.access_token,
    )
    .await;
    assert_eq!(video.status(), StatusCode::OK);
    assert_eq!(
        to_bytes(video.into_body(), 64).await.unwrap().as_ref(),
        b"ALLOWED"
    );
    let denied_video = get(
        &router,
        &format!("/Videos/{secret_id}/stream?static=true"),
        &viewer.access_token,
    )
    .await;
    assert_eq!(denied_video.status(), StatusCode::NOT_FOUND);
    let denied_info = get(
        &router,
        &format!("/Items/{secret_id}/PlaybackInfo"),
        &viewer.access_token,
    )
    .await;
    assert_eq!(denied_info.status(), StatusCode::NOT_FOUND);

    assert!(auth::revoke_library(&state, "viewer", &public).unwrap());
    let gone = read_json(get(&router, "/Items", &viewer.access_token).await).await;
    assert_eq!(gone["TotalRecordCount"], 0);
    assert_eq!(
        get(
            &router,
            &format!("/Videos/{allowed_id}/stream?static=true"),
            &viewer.access_token,
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
}
