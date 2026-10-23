use std::sync::Arc;

use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use pelagos::app::{self, AppState};
use serde_json::Value;
use tower::ServiceExt;

fn test_app() -> axum::Router {
    app::router(Arc::new(AppState::new(
        "00000000-0000-4000-8000-000000000001".to_string(),
    )))
}

#[tokio::test]
async fn health_is_public_and_json() {
    let response = test_app()
        .oneshot(
            Request::builder()
                .uri("/health")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), 1024).await.unwrap();
    let value: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(value["status"], "ok");
}

#[tokio::test]
async fn pre_alpha_status_never_claims_tv_compatibility() {
    let response = test_app()
        .oneshot(
            Request::builder()
                .uri("/api/pelagos/status")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let body = to_bytes(response.into_body(), 2048).await.unwrap();
    let value: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(value["jellyfin_compatible"], false);
    assert_eq!(value["tv_client_playback"], false);
}

#[tokio::test]
async fn admin_page_contains_system_default_and_theme_controls() {
    let response = test_app()
        .oneshot(Request::builder().uri("/web/").body(Body::empty()).unwrap())
        .await
        .unwrap();
    let body = to_bytes(response.into_body(), 32 * 1024).await.unwrap();
    let html = String::from_utf8(body.to_vec()).unwrap();
    assert!(html.contains("data-choice=\"system\""));
    assert!(html.contains("data-choice=\"light\""));
    assert!(html.contains("data-choice=\"dark\""));
    assert!(html.contains("let choice = [\"system\", \"light\", \"dark\"]"));
}

#[tokio::test]
async fn system_info_follows_jellyfin_v12_3_public_schema() {
    let response = test_app()
        .oneshot(
            Request::builder()
                .uri("/System/Info/Public")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let body = to_bytes(response.into_body(), 4096).await.unwrap();
    let value: Value = serde_json::from_slice(&body).unwrap();
    for key in [
        "Id",
        "ServerName",
        "LocalAddress",
        "Version",
        "ProductName",
        "OperatingSystem",
        "StartupWizardCompleted",
    ] {
        assert!(value.get(key).is_some(), "missing {key}");
    }
    assert!(value.get("CanSelfRestart").is_none());
    assert_eq!(value["OperatingSystem"], std::env::consts::OS);
    assert_eq!(value["StartupWizardCompleted"], false);
}

#[tokio::test]
async fn jellyfin_system_ping_accepts_get_and_post() {
    for method in ["GET", "POST"] {
        let response = test_app()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri("/System/Ping")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), 1024).await.unwrap();
        assert_eq!(body.as_ref(), b"Pelagos");
    }
}

#[tokio::test]
async fn concrete_listener_address_is_advertised_without_trusting_host_headers() {
    for (listen, expected) in [
        ("127.0.0.1:8097", Some("http://127.0.0.1:8097")),
        ("192.168.1.3:8097", Some("http://192.168.1.3:8097")),
        ("[::1]:8097", Some("http://[::1]:8097")),
        ("0.0.0.0:8097", None),
        ("[::]:8097", None),
    ] {
        let state = AppState::new("fixture".to_owned()).with_listen_addr(listen.parse().unwrap());
        let response = app::router(Arc::new(state))
            .oneshot(
                Request::builder()
                    .uri("/System/Info/Public")
                    .header("Host", "attacker.example:6666")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), 4096).await.unwrap();
        let result: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(
            result["LocalAddress"].as_str(),
            expected,
            "incorrect advertised address for {listen}"
        );
    }
}
