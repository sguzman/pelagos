use std::sync::Arc;

use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
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
    path: &str,
    method: &str,
    token: Option<&str>,
    payload: Option<Value>,
) -> Response {
    let mut req = Request::builder().uri(path).method(method);
    if let Some(token) = token {
        req = req.header("X-Emby-Token", token);
    }
    if payload.is_some() {
        req = req.header(header::CONTENT_TYPE, "application/json");
    }
    let body = payload.map_or_else(Body::empty, |v| Body::from(v.to_string()));
    router.clone().oneshot(req.body(body).unwrap()).await.unwrap()
}

async fn item_state(router: &Router, id: &str, token: &str) -> Value {
    let response = call(router, &format!("/Items/{id}"), "GET", Some(token), None).await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), 65536).await.unwrap();
    let result: Value = serde_json::from_slice(&body).unwrap();
    result["UserData"].clone()
}

#[tokio::test]
async fn direct_play_start_progress_and_stop_persist_resume_position() {
    let temp = tempfile::tempdir().unwrap();
    let media = temp.path().join("Media");
    let state = temp.path().join("state");
    std::fs::create_dir(&media).unwrap();
    std::fs::write(media.join("Test.mp4"), b"fixture").unwrap();
    catalog::index(&state, &media).unwrap();
    auth::create_user(&state, "admin", "admin-progress-test-password").unwrap();
    auth::create_user(&state, "viewer", "viewer-progress-test-password").unwrap();
    let admin = auth::authenticate(&state, "admin", "admin-progress-test-password", "server")
        .unwrap()
        .unwrap();
    let viewer = auth::authenticate(&state, "viewer", "viewer-progress-test-password", "server")
        .unwrap()
        .unwrap();
    let router = app::router(Arc::new(AppState::with_auth_dir(
        "server".to_owned(),
        state.clone(),
    )));
    let items = call(&router, "/Items", "GET", Some(&admin.access_token), None).await;
    let bytes = to_bytes(items.into_body(), 65536).await.unwrap();
    let value: Value = serde_json::from_slice(&bytes).unwrap();
    let id = value["Items"][0]["Id"].as_str().unwrap().to_owned();
    let progress = "/Sessions/Playing/Progress";
    let start = "/Sessions/Playing";
    let stop = "/Sessions/Playing/Stopped";

    assert_eq!(
        call(
            &router,
            start,
            "POST",
            None,
            Some(json!({"ItemId":id,"PlayMethod":"DirectPlay"})),
        )
        .await
        .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        call(
            &router,
            start,
            "POST",
            Some(&viewer.access_token),
            Some(json!({"ItemId":id,"PlayMethod":"DirectPlay"})),
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
    auth::grant_library(&state, "viewer", &media).unwrap();
    assert_eq!(
        call(
            &router,
            start,
            "POST",
            Some(&viewer.access_token),
            Some(json!({"ItemId":id,"PlayMethod":"DirectPlay"})),
        )
        .await
        .status(),
        StatusCode::NO_CONTENT
    );

    assert_eq!(
        call(
            &router,
            progress,
            "POST",
            Some(&viewer.access_token),
            Some(json!({"ItemId":id,"PlayMethod":"DirectPlay","PositionTicks":420000000})),
        )
        .await
        .status(),
        StatusCode::NO_CONTENT
    );
    let mine = item_state(&router, &id, &viewer.access_token).await;
    assert_eq!(mine["PlaybackPositionTicks"], 420000000);
    assert_eq!(mine["Played"], false);
    assert_eq!(
        item_state(&router, &id, &admin.access_token).await["PlaybackPositionTicks"],
        0
    );

    for payload in [
        json!({"ItemId":id,"PlayMethod":"DirectStream","PositionTicks":10}),
        json!({"ItemId":id,"PlayMethod":"Transcode","PositionTicks":10}),
        json!({"ItemId":id,"PlayMethod":2,"PositionTicks":-1}),
        json!({"ItemId":id,"PlayMethod":2,"PositionTicks":10,"MediaSourceId":"other"}),
    ] {
        let response = call(
            &router,
            progress,
            "POST",
            Some(&viewer.access_token),
            Some(payload.clone()),
        )
        .await;
        assert_ne!(response.status(), StatusCode::NO_CONTENT);
    }
    assert_eq!(
        item_state(&router, &id, &viewer.access_token).await["PlaybackPositionTicks"],
        420000000
    );
    assert_eq!(
        call(
            &router,
            stop,
            "POST",
            Some(&viewer.access_token),
            Some(json!({"ItemId":id,"PositionTicks":500000000,"Failed":false})),
        )
        .await
        .status(),
        StatusCode::NO_CONTENT
    );
    let restarted = app::router(Arc::new(AppState::with_auth_dir("server".to_owned(), state.clone())));
    assert_eq!(
        item_state(&restarted, &id, &viewer.access_token).await["PlaybackPositionTicks"],
        500000000
    );
    auth::revoke_library(&state, "viewer", &media).unwrap();
    assert_eq!(
        call(
            &restarted,
            progress,
            "POST",
            Some(&viewer.access_token),
            Some(json!({"ItemId":id,"PlayMethod":"DirectPlay","PositionTicks":123})),
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
}
