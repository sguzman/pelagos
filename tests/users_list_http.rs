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

async fn call(router: &Router, path: &str, token: Option<&str>) -> Response {
    let mut builder = Request::builder().uri(path);
    if let Some(token) = token {
        builder = builder.header("X-Emby-Token", token);
    }
    router
        .clone()
        .oneshot(builder.body(Body::empty()).unwrap())
        .await
        .unwrap()
}

async fn json(response: Response) -> Value {
    serde_json::from_slice(&to_bytes(response.into_body(), 65536).await.unwrap()).unwrap()
}

#[tokio::test]
async fn account_inventory_is_administrator_only_scoped_and_durable() {
    let fixture = tempfile::tempdir().unwrap();
    let state = fixture.path().join("state");
    auth::create_user(&state, "admin", "list-users-admin-password").unwrap();
    auth::create_user(&state, "viewer", "list-users-viewer-password").unwrap();
    let admin = auth::authenticate(&state, "admin", "list-users-admin-password", "server")
        .unwrap()
        .unwrap();
    let viewer = auth::authenticate(&state, "viewer", "list-users-viewer-password", "server")
        .unwrap()
        .unwrap();
    let router = app::router(Arc::new(AppState::with_auth_dir(
        "server".to_owned(),
        state.clone(),
    )));
    assert_eq!(
        call(&router, "/Users", None).await.status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        call(&router, "/Users", Some(&viewer.access_token))
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    let raw = to_bytes(
        call(&router, "/Users", Some(&admin.access_token))
            .await
            .into_body(),
        65536,
    )
    .await
    .unwrap();
    assert!(!raw.windows("list-users-admin-password".len()).any(|part| part == b"list-users-admin-password"));
    assert!(!raw.windows("list-users-viewer-password".len()).any(|part| part == b"list-users-viewer-password"));
    let users: Value = serde_json::from_slice(&raw).unwrap();
    let array = users.as_array().unwrap();
    assert_eq!(array.len(), 2);
    assert_eq!(array[0]["Name"], "admin");
    assert_eq!(array[1]["Name"], "viewer");
    assert_eq!(array[0]["ServerId"], "server");
    assert_eq!(array[0]["Policy"]["IsAdministrator"], true);
    assert_eq!(array[1]["Policy"]["IsAdministrator"], false);
    assert_eq!(array[0]["Policy"]["IsDisabled"], false);
    assert!(array[0].get("PasswordHash").is_none());
    assert_eq!(
        json(call(&router, "/Users?isHidden=true", Some(&admin.access_token)).await).await,
        serde_json::json!([])
    );
    assert_eq!(
        json(call(&router, "/Users?isDisabled=true", Some(&admin.access_token)).await).await,
        serde_json::json!([])
    );
    assert_eq!(
        json(call(&router, "/Users?isDisabled=false", Some(&admin.access_token)).await).await,
        users
    );
    assert_eq!(
        json(call(&router, "/Users/Public", None).await).await,
        serde_json::json!([])
    );

    let db = rusqlite::Connection::open(state.join("pelagos-auth.sqlite3")).unwrap();
    db.execute(
        "UPDATE pelagos_users SET disabled=1 WHERE username_key='viewer'",
        [],
    )
    .unwrap();
    drop(db);
    let disabled = json(call(&router, "/Users?isDisabled=true", Some(&admin.access_token)).await).await;
    assert_eq!(disabled.as_array().unwrap().len(), 1);
    assert_eq!(disabled[0]["Name"], "viewer");
    assert_eq!(disabled[0]["Policy"]["IsDisabled"], true);
    let active = json(call(&router, "/Users?isDisabled=false", Some(&admin.access_token)).await).await;
    assert_eq!(active.as_array().unwrap().len(), 1);
    assert_eq!(active[0]["Name"], "admin");

    let restarted = app::router(Arc::new(AppState::with_auth_dir(
        "server".to_owned(),
        state.clone(),
    )));
    assert_eq!(
        json(call(&restarted, "/Users", Some(&admin.access_token)).await).await[1]["Policy"]["IsDisabled"],
        true
    );
}
