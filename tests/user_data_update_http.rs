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
use serde_json::{Value, json};
use tower::ServiceExt;

async fn call(
    router: &Router,
    method: &str,
    path: &str,
    token: Option<&str>,
    payload: Option<Value>,
) -> Response {
    let mut req = Request::builder().uri(path).method(method);
    if let Some(token) = token {
        req = req.header("X-Emby-Token", token);
    }
    let body = if let Some(value) = payload {
        req = req.header("Content-Type", "application/json");
        Body::from(value.to_string())
    } else {
        Body::empty()
    };
    router
        .clone()
        .oneshot(req.body(body).unwrap())
        .await
        .unwrap()
}

async fn body(response: Response) -> Value {
    let bytes = to_bytes(response.into_body(), 65536).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

#[tokio::test]
async fn partial_user_data_update_is_transactional_scoped_and_persistent() {
    let fixture = tempfile::tempdir().unwrap();
    let state = fixture.path().join("state");
    let public = fixture.path().join("Public");
    let private = fixture.path().join("Private");
    std::fs::create_dir(&public).unwrap();
    std::fs::create_dir(&private).unwrap();
    std::fs::write(public.join("Clip.mp4"), b"synthetic").unwrap();
    std::fs::write(private.join("Secret.mp4"), b"synthetic").unwrap();
    catalog::index(&state, &public).unwrap();
    catalog::index(&state, &private).unwrap();
    auth::create_user(&state, "admin", "admin-update-test-password").unwrap();
    auth::create_user(&state, "viewer", "viewer-update-test-password").unwrap();
    let admin = auth::authenticate(&state, "admin", "admin-update-test-password", "server")
        .unwrap()
        .unwrap();
    let viewer = auth::authenticate(&state, "viewer", "viewer-update-test-password", "server")
        .unwrap()
        .unwrap();
    let router = app::router(Arc::new(AppState::with_auth_dir(
        "server".to_owned(),
        state.clone(),
    )));
    let listing = body(call(&router, "GET", "/Items", Some(&admin.access_token), None).await).await;
    let items = listing["Items"].as_array().unwrap();
    let id = items.iter().find(|item| item["Name"] == "Clip").unwrap()["Id"]
        .as_str()
        .unwrap();
    let secret_id = items.iter().find(|item| item["Name"] == "Secret").unwrap()["Id"]
        .as_str()
        .unwrap();
    let route = format!("/UserItems/{id}/UserData");
    assert_eq!(
        call(&router, "POST", &route, None, Some(json!({"Played":true})))
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        call(
            &router,
            "POST",
            &route,
            Some(&viewer.access_token),
            Some(json!({"Played":true})),
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
    auth::grant_library(&state, "viewer", &public).unwrap();
    let saved = body(
        call(
            &router,
            "POST",
            &route,
            Some(&viewer.access_token),
            Some(json!({"PlaybackPositionTicks":1234567,"IsFavorite":true})),
        )
        .await,
    )
    .await;
    assert_eq!(saved["PlaybackPositionTicks"], 1234567);
    assert_eq!(saved["IsFavorite"], true);
    assert_eq!(saved["Played"], false);
    assert_eq!(saved["PlayCount"], 0);

    for rejected in [
        (json!({"PlaybackPositionTicks":-1}), StatusCode::BAD_REQUEST),
        (
            json!({"Played":true,"PlaybackPositionTicks":10}),
            StatusCode::BAD_REQUEST,
        ),
        (
            json!({"ItemId":"other","Played":true}),
            StatusCode::BAD_REQUEST,
        ),
        (json!({"Rating":4.5}), StatusCode::NOT_IMPLEMENTED),
        (json!({"UnknownField":false}), StatusCode::NOT_IMPLEMENTED),
        (json!({}), StatusCode::BAD_REQUEST),
    ] {
        let response = call(
            &router,
            "POST",
            &route,
            Some(&viewer.access_token),
            Some(rejected.0),
        )
        .await;
        assert_eq!(response.status(), rejected.1);
    }
    let unchanged =
        body(call(&router, "GET", &route, Some(&viewer.access_token), None).await).await;
    assert_eq!(unchanged["PlaybackPositionTicks"], 1234567);
    assert_eq!(unchanged["IsFavorite"], true);

    let played = body(
        call(
            &router,
            "POST",
            &route,
            Some(&viewer.access_token),
            Some(json!({"Played":true})),
        )
        .await,
    )
    .await;
    assert_eq!(played["Played"], true);
    assert_eq!(played["PlaybackPositionTicks"], 0);
    assert_eq!(played["PlayCount"], 1);
    let again = body(
        call(
            &router,
            "POST",
            &route,
            Some(&viewer.access_token),
            Some(json!({"Played":true})),
        )
        .await,
    )
    .await;
    assert_eq!(again["PlayCount"], 1);

    let legacy = format!("/Users/{}/Items/{id}/UserData", viewer.user.id);
    let cleared = body(
        call(
            &router,
            "POST",
            &legacy,
            Some(&viewer.access_token),
            Some(json!({"Played":false})),
        )
        .await,
    )
    .await;
    assert_eq!(cleared["Played"], false);
    assert_eq!(cleared["PlayCount"], 1);
    assert_eq!(
        call(
            &router,
            "POST",
            &format!("/Users/{}/Items/{id}/UserData", admin.user.id),
            Some(&viewer.access_token),
            Some(json!({"IsFavorite":false})),
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        call(
            &router,
            "POST",
            &format!("/UserItems/{secret_id}/UserData"),
            Some(&viewer.access_token),
            Some(json!({"Played":true})),
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
    let restarted = app::router(Arc::new(AppState::with_auth_dir(
        "server".to_owned(),
        state.clone(),
    )));
    let persisted =
        body(call(&restarted, "GET", &route, Some(&viewer.access_token), None).await).await;
    assert_eq!(persisted["PlayCount"], 1);
    assert_eq!(persisted["IsFavorite"], true);
    assert_eq!(persisted["Played"], false);

    auth::revoke_library(&state, "viewer", &public).unwrap();
    assert_eq!(
        call(
            &restarted,
            "POST",
            &route,
            Some(&viewer.access_token),
            Some(json!({"IsFavorite":false})),
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
}
