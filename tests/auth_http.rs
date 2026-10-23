use std::sync::Arc;

use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use pelagos::{app::{self, AppState}, auth};
use serde_json::{Value, json};
use tower::ServiceExt;

async fn body_json(response: axum::response::Response) -> Value {
    let bytes = to_bytes(response.into_body(), 65536).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

#[tokio::test]
async fn unconfigured_auth_is_closed_by_default() {
    let router = app::router(Arc::new(AppState::new("synthetic".to_string())));
    let response = router
        .oneshot(
            Request::builder().uri("/Users/Me").body(Body::empty()).unwrap(),
        ).await.unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn jellyfin_password_login_me_and_logout_are_isolated() {
    let state = tempfile::tempdir().unwrap();
    auth::create_user(state.path(), "viewer", "strong-local-test-passphrase").unwrap();
    let router = app::router(Arc::new(AppState::with_auth_dir(
        "00000000-0000-4000-8000-000000000001".to_string(),
        state.path().to_path_buf(),
    )));

    let public = router.clone().oneshot(
        Request::builder().uri("/Users/Public").body(Body::empty()).unwrap(),
    ).await.unwrap();
    assert_eq!(body_json(public).await, json!([]));

    let wrong = router.clone().oneshot(
        Request::builder()
            .method("POST")
            .uri("/Users/AuthenticateByName")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(r#"{"Username":"viewer","Pw":"wrong"}"#))
            .unwrap(),
    ).await.unwrap();
    assert_eq!(wrong.status(), StatusCode::UNAUTHORIZED);

    let login_response = router.clone().oneshot(
        Request::builder()
            .method("POST")
            .uri("/Users/AuthenticateByName")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(r#"{"Username":"viewer","Pw":"strong-local-test-passphrase"}"#))
            .unwrap(),
    ).await.unwrap();
    assert_eq!(login_response.status(), StatusCode::OK);
    let logged_in = body_json(login_response).await;
    assert_eq!(logged_in["User"]["Name"], "viewer");
    assert_eq!(logged_in["User"]["Policy"]["IsAdministrator"], true);
    let token = logged_in["AccessToken"].as_str().unwrap();
    assert_eq!(token.len(), 64);
    assert_eq!(logged_in["ServerId"], "00000000-0000-4000-8000-000000000001");

    let unauthorized = router.clone().oneshot(
        Request::builder().uri("/Users/Me").body(Body::empty()).unwrap(),
    ).await.unwrap();
    assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);

    let authorized = router.clone().oneshot(
        Request::builder()
            .uri("/Users/Me")
            .header(header::AUTHORIZATION, format!("MediaBrowser Client=\"Jellyfin Android TV\", Token=\"{token}\""))
            .body(Body::empty())
            .unwrap(),
    ).await.unwrap();
    assert_eq!(authorized.status(), StatusCode::OK);
    assert_eq!(body_json(authorized).await["Name"], "viewer");

    let logged_out = router.clone().oneshot(
        Request::builder()
            .method("POST")
            .uri("/Sessions/Logout")
            .header("X-Emby-Token", token)
            .body(Body::empty())
            .unwrap(),
    ).await.unwrap();
    assert_eq!(logged_out.status(), StatusCode::NO_CONTENT);
    let rejected = router.oneshot(
        Request::builder()
            .uri("/Users/Me")
            .header("X-Emby-Token", token)
            .body(Body::empty())
            .unwrap(),
    ).await.unwrap();
    assert_eq!(rejected.status(), StatusCode::UNAUTHORIZED);
}
