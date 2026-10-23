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

async fn call(router: &Router, verb: &str, uri: &str, token: Option<&str>) -> Response {
    let mut req = Request::builder().uri(uri).method(verb);
    if let Some(token) = token {
        req = req.header("X-Emby-Token", token);
    }
    router
        .clone()
        .oneshot(req.body(Body::empty()).unwrap())
        .await
        .unwrap()
}

async fn body(response: Response) -> Value {
    let data = to_bytes(response.into_body(), 65536).await.unwrap();
    serde_json::from_slice(&data).unwrap()
}

#[tokio::test]
async fn played_and_favorites_are_scoped_persistent_and_revocable() {
    let temp = tempfile::tempdir().unwrap();
    let state = temp.path().join("state");
    let media = temp.path().join("media");
    std::fs::create_dir(&media).unwrap();
    std::fs::write(media.join("Movie.mp4"), b"synthetic").unwrap();
    catalog::index(&state, &media).unwrap();
    auth::create_user(&state, "admin", "admin-user-data-password").unwrap();
    auth::create_user(&state, "viewer", "viewer-user-data-password").unwrap();
    let admin = auth::authenticate(&state, "admin", "admin-user-data-password", "server")
        .unwrap()
        .unwrap();
    let viewer = auth::authenticate(&state, "viewer", "viewer-user-data-password", "server")
        .unwrap()
        .unwrap();
    let router = app::router(Arc::new(AppState::with_auth_dir(
        "server".to_owned(),
        state.clone(),
    )));

    let items = body(call(&router, "GET", "/Items", Some(&admin.access_token)).await).await;
    let id = items["Items"][0]["Id"].as_str().unwrap();
    let mark = format!("/UserPlayedItems/{id}");
    let favorite = format!("/UserFavoriteItems/{id}");
    assert_eq!(
        call(&router, "POST", &mark, None).await.status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        call(&router, "POST", &mark, Some(&viewer.access_token))
            .await
            .status(),
        StatusCode::NOT_FOUND
    );

    auth::grant_library(&state, "viewer", &media).unwrap();
    let first = body(call(&router, "POST", &mark, Some(&viewer.access_token)).await).await;
    assert_eq!(first["Played"], true);
    assert_eq!(first["PlayCount"], 1);
    assert_eq!(first["ItemId"], id);
    assert_eq!(first["PlaybackPositionTicks"], 0);
    assert_eq!(first["IsFavorite"], false);
    assert!(first["LastPlayedDate"].is_string());

    let replay = body(call(&router, "POST", &mark, Some(&viewer.access_token)).await).await;
    assert_eq!(replay["PlayCount"], 1);
    let starred = body(call(&router, "POST", &favorite, Some(&viewer.access_token)).await).await;
    assert_eq!(starred["IsFavorite"], true);
    assert_eq!(starred["PlayCount"], 1);

    let admin_data = body(call(&router, "POST", &favorite, Some(&admin.access_token)).await).await;
    assert_eq!(admin_data["PlayCount"], 0);

    let other = format!("/Users/{}/PlayedItems/{id}", admin.user.id);
    assert_eq!(
        call(&router, "POST", &other, Some(&viewer.access_token))
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    let denied = format!("/UserPlayedItems/{id}?userId={}", admin.user.id);
    assert_eq!(
        call(&router, "POST", &denied, Some(&viewer.access_token))
            .await
            .status(),
        StatusCode::FORBIDDEN
    );

    let unplayed = body(call(&router, "DELETE", &mark, Some(&viewer.access_token)).await).await;
    assert_eq!(unplayed["Played"], false);
    assert_eq!(unplayed["PlayCount"], 1);
    assert_eq!(unplayed["IsFavorite"], true);
    let unstar = body(call(&router, "DELETE", &favorite, Some(&viewer.access_token)).await).await;
    assert_eq!(unstar["IsFavorite"], false);
    assert_eq!(unstar["PlayCount"], 1);

    // Rebuild the server with the same Pelagos state directory.
    let restarted = app::router(Arc::new(AppState::with_auth_dir(
        "server".to_owned(),
        state.clone(),
    )));
    let legacy = format!("/Users/{}/PlayedItems/{id}", viewer.user.id);
    let replayed = body(call(&restarted, "POST", &legacy, Some(&viewer.access_token)).await).await;
    assert_eq!(replayed["PlayCount"], 2);
    auth::revoke_library(&state, "viewer", &media).unwrap();
    assert_eq!(
        call(&restarted, "DELETE", &mark, Some(&viewer.access_token))
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
}
