use std::sync::Arc;

use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode},
};
use pelagos::{
    app::{self, AppState},
    auth,
};
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
    router
        .clone()
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
        .unwrap()
        .unwrap();
    let second = auth::authenticate(&state, "member", "member-initial-passphrase", "server")
        .unwrap()
        .unwrap();
    let router = app::router(Arc::new(AppState::with_auth_dir(
        "server".to_owned(),
        state.clone(),
    )));
    let valid =
        json!({"CurrentPw":"member-initial-passphrase","NewPw":"member-revised-passphrase"});
    assert_eq!(
        request(&router, None, valid.clone()).await,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        request(
            &router,
            Some(&first.access_token),
            json!({"CurrentPw":"wrong-passphrase", "NewPw":"member-revised-passphrase"})
        )
        .await,
        StatusCode::FORBIDDEN
    );
    assert!(
        auth::identify(&state, &second.access_token, "server")
            .unwrap()
            .is_some()
    );
    assert_eq!(
        request(&router, Some(&first.access_token), valid).await,
        StatusCode::NO_CONTENT
    );
    assert!(
        auth::identify(&state, &first.access_token, "server")
            .unwrap()
            .is_some()
    );
    assert!(
        auth::identify(&state, &second.access_token, "server")
            .unwrap()
            .is_none()
    );
    assert!(
        auth::authenticate(&state, "member", "member-initial-passphrase", "server")
            .unwrap()
            .is_none()
    );
    assert!(
        auth::authenticate(&state, "member", "member-revised-passphrase", "server")
            .unwrap()
            .is_some()
    );
}

#[tokio::test]
async fn self_change_cannot_target_another_account_or_reset_without_password() {
    let temp = tempfile::tempdir().unwrap();
    let state = temp.path().join("state");
    auth::create_user(&state, "admin", "admin-second-passphrase").unwrap();
    auth::create_user(&state, "member", "member-second-passphrase").unwrap();
    let admin = auth::authenticate(&state, "admin", "admin-second-passphrase", "server")
        .unwrap()
        .unwrap();
    let member = auth::authenticate(&state, "member", "member-second-passphrase", "server")
        .unwrap()
        .unwrap();
    let router = app::router(Arc::new(AppState::with_auth_dir(
        "server".to_owned(),
        state.clone(),
    )));
    let valid = json!({"CurrentPw":"member-second-passphrase","NewPw":"member-final-passphrase"});
    let bad_target = Request::builder()
        .method("POST")
        .uri(format!("/Users/Password?userId={}", admin.user.id))
        .header("Content-Type", "application/json")
        .header("X-Emby-Token", member.access_token.as_str())
        .body(Body::from(valid.to_string()))
        .unwrap();
    assert_eq!(
        router.clone().oneshot(bad_target).await.unwrap().status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        request(
            &router,
            Some(&member.access_token),
            json!({
                "CurrentPw":"member-second-passphrase",
                "NewPw":"member-final-passphrase",
                "ResetPassword":true
            }),
        )
        .await,
        StatusCode::NOT_IMPLEMENTED
    );
    assert_eq!(
        request(
            &router,
            Some(&member.access_token),
            json!({"CurrentPw":"member-second-passphrase","NewPw":"short"}),
        )
        .await,
        StatusCode::BAD_REQUEST
    );
    assert!(
        auth::authenticate(&state, "member", "member-second-passphrase", "server")
            .unwrap()
            .is_some()
    );

    assert_eq!(
        auth::change_own_password(
            &state,
            &member.user.id,
            &"0".repeat(64),
            "member-second-passphrase",
            "member-final-passphrase",
        )
        .unwrap(),
        auth::OwnPasswordChange::StaleSession
    );
    assert!(
        auth::authenticate(&state, "member", "member-final-passphrase", "server")
            .unwrap()
            .is_none()
    );
    assert_eq!(
        request(
            &router,
            Some(&admin.access_token),
            json!({"CurrentPw":"admin-second-passphrase","NewPw":"admin-final-passphrase"}),
        )
        .await,
        StatusCode::NO_CONTENT
    );
    assert!(
        auth::identify(&state, &admin.access_token, "server")
            .unwrap()
            .is_some()
    );
    assert!(
        auth::authenticate(&state, "admin", "admin-final-passphrase", "server")
            .unwrap()
            .is_some()
    );
}
