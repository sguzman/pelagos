use std::sync::Arc;

use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode},
    response::Response,
};
use pelagos::{
    app::{self, AppState},
    auth::{self, ClientMetadata},
};
use serde_json::Value;
use tower::ServiceExt;

async fn request(router: &Router, path: &str, token: &str) -> Response {
    router
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(path)
                .header("X-Emby-Token", token)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
}

async fn json_body(response: Response) -> Value {
    serde_json::from_slice(&to_bytes(response.into_body(), 65_536).await.unwrap()).unwrap()
}

#[tokio::test]
async fn last_activity_filter_is_based_on_valid_authenticated_requests() {
    let fixture = tempfile::tempdir().unwrap();
    let state = fixture.path().join("state");
    auth::create_user(&state, "admin", "session-activity-admin-password").unwrap();
    auth::create_user(&state, "viewer", "session-activity-viewer-password").unwrap();
    let admin = auth::authenticate(&state, "admin", "session-activity-admin-password", "server")
        .unwrap()
        .unwrap();
    let viewer = auth::authenticate_with_client(
        &state,
        "viewer",
        "session-activity-viewer-password",
        "server",
        ClientMetadata {
            client: Some("Jellyfin Android TV".to_owned()),
            device_id: Some("tv-session-activity".to_owned()),
            device_name: Some("Living Room".to_owned()),
            application_version: Some("1.0".to_owned()),
        },
    )
    .unwrap()
    .unwrap();
    assert!(
        viewer
            .session_info
            .last_activity_date
            .as_ref()
            .unwrap()
            .ends_with('Z')
    );
    let router = app::router(Arc::new(AppState::with_auth_dir(
        "server".to_owned(),
        state.clone(),
    )));
    let path = "/Sessions?deviceId=tv-session-activity&activeWithinSeconds=60";

    // A stored session may be authenticated but too old to match the filter.
    let db = rusqlite::Connection::open(state.join("pelagos-auth.sqlite3")).unwrap();
    db.execute(
        "UPDATE pelagos_session_activity
         SET last_seen_unix=unixepoch()-3600 WHERE session_id=?1",
        [&viewer.session_info.id],
    )
    .unwrap();
    drop(db);
    let old = json_body(request(&router, path, &admin.access_token).await).await;
    assert_eq!(old.as_array().unwrap().len(), 0);
    let unfiltered = json_body(
        request(
            &router,
            "/Sessions?deviceId=tv-session-activity",
            &admin.access_token,
        )
        .await,
    )
    .await;
    assert_eq!(unfiltered.as_array().unwrap().len(), 1);
    assert!(
        unfiltered[0]["LastActivityDate"]
            .as_str()
            .unwrap()
            .ends_with('Z')
    );

    // A real authenticated call moves last-seen forward.
    assert_eq!(
        request(&router, "/Users/Me", &viewer.access_token)
            .await
            .status(),
        StatusCode::OK
    );
    let recent = json_body(request(&router, path, &admin.access_token).await).await;
    assert_eq!(recent.as_array().unwrap().len(), 1);
    assert_eq!(recent[0]["DeviceId"], "tv-session-activity");
    assert_eq!(recent[0]["UserId"], viewer.user.id);
    assert_eq!(recent[0]["HasCustomDeviceName"], false);
    let current_date = recent[0]["LastActivityDate"].as_str().unwrap();
    assert!(current_date.ends_with('Z'));
    assert_ne!(
        current_date,
        unfiltered[0]["LastActivityDate"].as_str().unwrap()
    );

    auth::rename_registered_device(&state, "tv-session-activity", Some("Den")).unwrap();
    let renamed = json_body(request(&router, path, &admin.access_token).await).await;
    assert_eq!(renamed[0]["HasCustomDeviceName"], true);

    let restarted = app::router(Arc::new(AppState::with_auth_dir(
        "server".to_owned(),
        state.clone(),
    )));
    let persisted = json_body(request(&restarted, path, &admin.access_token).await).await;
    assert_eq!(persisted.as_array().unwrap().len(), 1);
    assert_eq!(persisted[0]["HasCustomDeviceName"], true);

    assert_eq!(
        request(
            &router,
            "/Sessions?deviceId=tv-session-activity&activeWithinSeconds=31536001",
            &admin.access_token,
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        request(
            &router,
            "/Sessions?controllableByUserId=00000000-0000-0000-0000-000000000001",
            &admin.access_token,
        )
        .await
        .status(),
        StatusCode::NOT_IMPLEMENTED
    );

    auth::revoke(&state, &viewer.access_token).unwrap();
    let db = rusqlite::Connection::open(state.join("pelagos-auth.sqlite3")).unwrap();
    let count: i64 = db
        .query_row(
            "SELECT COUNT(*) FROM pelagos_session_activity WHERE session_id=?1",
            [&viewer.session_info.id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 0);
    let revoked = json_body(request(&router, path, &admin.access_token).await).await;
    assert_eq!(revoked.as_array().unwrap().len(), 0);
}
