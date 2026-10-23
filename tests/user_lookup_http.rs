use std::sync::Arc;

use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode},
    response::Response,
};
use pelagos::{
    app::{self, AppState},
    auth,
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
    let bytes = to_bytes(response.into_body(), 65536).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

#[tokio::test]
async fn user_id_lookup_matches_me_without_leaking_other_accounts() {
    let fixture = tempfile::tempdir().unwrap();
    let state = fixture.path().join("state");
    auth::create_user(&state, "admin", "admin-lookup-test-password").unwrap();
    auth::create_user(&state, "viewer", "viewer-lookup-test-password").unwrap();
    let admin = auth::authenticate(&state, "admin", "admin-lookup-test-password", "server")
        .unwrap()
        .unwrap();
    let viewer = auth::authenticate(&state, "viewer", "viewer-lookup-test-password", "server")
        .unwrap()
        .unwrap();
    let router = app::router(Arc::new(AppState::with_auth_dir(
        "server".to_owned(),
        state.clone(),
    )));
    let own = format!("/Users/{}", viewer.user.id);
    assert_eq!(
        get(&router, &own, None).await.status(),
        StatusCode::UNAUTHORIZED
    );
    let me = json(get(&router, "/Users/Me", Some(&viewer.access_token)).await).await;
    let by_id = json(get(&router, &own, Some(&viewer.access_token)).await).await;
    assert_eq!(by_id, me);
    assert_eq!(by_id["Id"], viewer.user.id);
    assert_eq!(by_id["Name"], "viewer");
    assert!(by_id.get("PasswordHash").is_none());
    assert_eq!(
        get(
            &router,
            &format!("/Users/{}", admin.user.id),
            Some(&viewer.access_token),
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        get(&router, &own, Some(&admin.access_token)).await.status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        get(
            &router,
            "/Users/00000000-0000-0000-0000-000000000000",
            Some(&viewer.access_token),
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
    auth::revoke(&state, &viewer.access_token).unwrap();
    assert_eq!(
        get(&router, &own, Some(&viewer.access_token))
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
}
