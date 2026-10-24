use std::sync::Arc;

use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode},
};
use pelagos::{app::{self, AppState}, auth};
use serde_json::{Value, json};
use tower::ServiceExt;

async fn request(router: &Router, token: Option<&str>, body: Value) -> StatusCode {
    let mut builder = Request::builder()
        .method("POST")
        .uri("/Users/Password")
        .header("Content-Type", "application/json");
    if let Some(token) = token {
        builder = builder.header("X-Emby-Token", token);
    }
    router.clone()
        .oneshot(builder.body(Body::from(body.to_string())).unwrap())
        .await
        .unwrap()
        .status()
}

#[tokio::test]
async fn user_must_prove_current_password_and_other_sessions_are_revoked() {
    let temp = tempfile::tempdir().unwrap();
    let state = temp.path().join("state");
    auth::create_user(&state, "admin", "admin-initial-passphrase").unwrap();
    auth::create_user(&state, "member", "member-initial-passphrase").unwrap();
    let first = auth::authenticate(&state, "member", "member-initial-passphrase", "server")
        .unwrap().unwrap();
    let second = auth::authenticate(&state, "member", "member-initial-passphrase", "server")
        .unwrap().unwrap();
    let router = app::router(Arc::new(AppState::with_auth_dir(
        "server".to_owned(), state.clone()
    )));
    let valid = json!({"CurrentPw":"member-initial-passphrase","NewPw":"member-revised-passphrase"});
    assert_eq!(request(&router, None, valid.clone()).await, StatusCode::UNAUTHORIZED);
    assert_eq!(
        request(&router, Some(&first.access_token),
            json!({"CurrentPw":"wrong-passphrase", "NewPw":"member-revised-passphrase"})).await,
        StatusCode::FORBIDDEN
    );
    assert!(auth::identify(&state, &second.access_token, "server").unwrap().is_some());
    assert_eq!(request(&router, Some(&first.access_token), valid).await, StatusCode::NO_CONTENT);
    assert!(auth::identify(&state, &first.access_token, "server").unwrap().is_some());
    assert!(auth::identify(&state, &second.access_token, "server").unwrap().is_none());
    assert!(auth::authenticate(&state, "member", "member-initial-passphrase", "server")
        .unwrap().is_none());
    assert!(auth::authenticate(&state, "member", "member-revised-passphrase", "server")
        .unwrap().is_some());
}
