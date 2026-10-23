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

async fn call(router: &Router, route: &str, token: Option<&str>) -> Response {
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
async fn indexed_items_report_no_unmodeled_boxsets_without_exposing_other_libraries() {
    let fixture = tempfile::tempdir().unwrap();
    let state = fixture.path().join("state");
    let public = fixture.path().join("Public");
    let private = fixture.path().join("Private");
    std::fs::create_dir_all(&public).unwrap();
    std::fs::create_dir_all(&private).unwrap();
    std::fs::write(public.join("Film.mkv"), b"public").unwrap();
    std::fs::write(private.join("Secret.mkv"), b"private").unwrap();
    catalog::index(&state, &public).unwrap();
    catalog::index(&state, &private).unwrap();
    auth::create_user(&state, "admin", "boxset-admin-password").unwrap();
    auth::create_user(&state, "viewer", "boxset-viewer-password").unwrap();
    let admin = auth::authenticate(&state, "admin", "boxset-admin-password", "server")
        .unwrap()
        .unwrap();
    let viewer = auth::authenticate(&state, "viewer", "boxset-viewer-password", "server")
        .unwrap()
        .unwrap();
    let router = app::router(Arc::new(AppState::with_auth_dir(
        "server".to_owned(),
        state.clone(),
    )));
    let items = json(call(&router, "/Items", Some(&admin.access_token)).await).await;
    let all = items["Items"].as_array().unwrap();
    let film = all.iter().find(|item| item["Name"] == "Film").unwrap()["Id"]
        .as_str()
        .unwrap();
    let secret = all.iter().find(|item| item["Name"] == "Secret").unwrap()["Id"]
        .as_str()
        .unwrap();
    let route = format!("/Items/{film}/Collections");
    let secret_route = format!("/Items/{secret}/Collections");

    assert_eq!(call(&router, &route, None).await.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(
        call(&router, &route, Some(&viewer.access_token))
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    auth::grant_library(&state, "viewer", &public).unwrap();

    let response = json(call(&router, &route, Some(&viewer.access_token)).await).await;
    assert_eq!(response["Items"], serde_json::json!([]));
    assert_eq!(response["TotalRecordCount"], 0);
    assert_eq!(response["StartIndex"], 0);
    let paged = json(
        call(
            &router,
            &format!("{route}?startIndex=17&limit=2"),
            Some(&viewer.access_token),
        )
        .await,
    )
    .await;
    assert_eq!(paged["Items"], serde_json::json!([]));
    assert_eq!(paged["TotalRecordCount"], 0);
    assert_eq!(paged["StartIndex"], 17);

    assert_eq!(
        call(&router, &secret_route, Some(&viewer.access_token))
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        call(
            &router,
            "/Items/00000000000000000000000000000000/Collections",
            Some(&viewer.access_token),
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        call(
            &router,
            &format!("{route}?userId={}", admin.user.id),
            Some(&viewer.access_token),
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        call(
            &router,
            &format!("{route}?fields=MediaStreams"),
            Some(&viewer.access_token),
        )
        .await
        .status(),
        StatusCode::NOT_IMPLEMENTED
    );
    let admin_private = json(call(&router, &secret_route, Some(&admin.access_token)).await).await;
    assert_eq!(admin_private["TotalRecordCount"], 0);

    let restarted = app::router(Arc::new(AppState::with_auth_dir(
        "server".to_owned(),
        state.clone(),
    )));
    assert_eq!(
        json(call(&restarted, &route, Some(&viewer.access_token)).await).await,
        response
    );
    auth::revoke_library(&state, "viewer", &public).unwrap();
    assert_eq!(
        call(&restarted, &route, Some(&viewer.access_token))
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
}
