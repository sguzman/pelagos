use std::sync::Arc;

use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
    response::Response,
};
use pelagos::{
    app::{self, AppState},
    auth,
};
use serde_json::{Value, json};
use tower::ServiceExt;

async fn send(
    router: &Router,
    method: &str,
    path: &str,
    body: Body,
    token: Option<&str>,
) -> Response {
    let mut request = Request::builder()
        .method(method)
        .uri(path)
        .header(header::CONTENT_TYPE, "application/json");
    if let Some(value) = token {
        request = request.header("X-Emby-Token", value);
    }
    router
        .clone()
        .oneshot(request.body(body).unwrap())
        .await
        .unwrap()
}

async fn json_response(response: Response) -> Value {
    let bytes = to_bytes(response.into_body(), 65536).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

#[tokio::test]
async fn unconfigured_auth_is_closed_by_default() {
    let router = app::router(Arc::new(AppState::new("synthetic".to_string())));
    let response = send(&router, "GET", "/Users/Me", Body::empty(), None).await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn jellyfin_login_me_logout_and_public_directory_are_isolated() {
    let state = tempfile::tempdir().unwrap();
    auth::create_user(state.path(), "viewer", "strong-local-test-passphrase").unwrap();
    let router = app::router(Arc::new(AppState::with_auth_dir(
        "00000000-0000-4000-8000-000000000001".to_string(),
        state.path().to_path_buf(),
    )));

    let public = send(&router, "GET", "/Users/Public", Body::empty(), None).await;
    assert_eq!(json_response(public).await, json!([]));

    let wrong = send(
        &router,
        "POST",
        "/Users/AuthenticateByName",
        Body::from(r#"{"Username":"viewer","Pw":"wrong"}"#),
        None,
    )
    .await;
    assert_eq!(wrong.status(), StatusCode::UNAUTHORIZED);

    let login = send(
        &router,
        "POST",
        "/Users/AuthenticateByName",
        Body::from(r#"{"Username":"viewer","Pw":"strong-local-test-passphrase"}"#),
        None,
    )
    .await;
    assert_eq!(login.status(), StatusCode::OK);
    let authenticated = json_response(login).await;
    assert_eq!(authenticated["User"]["Name"], "viewer");
    assert_eq!(authenticated["User"]["Policy"]["IsAdministrator"], true);
    let token = authenticated["AccessToken"].as_str().unwrap().to_owned();
    assert_eq!(token.len(), 64);
    assert_eq!(authenticated["SessionInfo"]["UserName"], "viewer");
    assert_eq!(
        authenticated["SessionInfo"]["UserId"],
        authenticated["User"]["Id"]
    );
    assert_eq!(authenticated["SessionInfo"]["IsActive"], true);
    assert_eq!(authenticated["SessionInfo"]["SupportsRemoteControl"], false);

    let unauthenticated = send(&router, "GET", "/Users/Me", Body::empty(), None).await;
    assert_eq!(unauthenticated.status(), StatusCode::UNAUTHORIZED);

    let user = send(&router, "GET", "/Users/Me", Body::empty(), Some(&token)).await;
    assert_eq!(user.status(), StatusCode::OK);
    assert_eq!(json_response(user).await["Name"], "viewer");

    let exit = send(
        &router,
        "POST",
        "/Sessions/Logout",
        Body::empty(),
        Some(&token),
    )
    .await;
    assert_eq!(exit.status(), StatusCode::NO_CONTENT);
    let invalid = send(&router, "GET", "/Users/Me", Body::empty(), Some(&token)).await;
    assert_eq!(invalid.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn jellyfin_client_header_survives_login_session_response() {
    let state = tempfile::tempdir().unwrap();
    auth::create_user(state.path(), "viewer", "long-credential-test-password").unwrap();
    let router = app::router(Arc::new(AppState::with_auth_dir(
        "server".to_owned(),
        state.path().to_path_buf(),
    )));
    let header = r#"MediaBrowser Client="Jellyfin Android TV", Device="Living, Room", DeviceId="tv-test", Version="2.0""#;
    let response = router
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/Users/AuthenticateByName")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::AUTHORIZATION, header)
                .body(Body::from(
                    r#"{"Username":"viewer","Pw":"long-credential-test-password"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let result = json_response(response).await;
    assert_eq!(result["SessionInfo"]["Client"], "Jellyfin Android TV");
    assert_eq!(result["SessionInfo"]["DeviceName"], "Living, Room");
    assert_eq!(result["SessionInfo"]["DeviceId"], "tv-test");
    assert_eq!(result["SessionInfo"]["ApplicationVersion"], "2.0");
    assert_eq!(result["SessionInfo"]["ServerId"], "server");
}

#[tokio::test]
async fn sessions_list_obeys_user_scope_and_explicitly_rejects_unimplemented_filters() {
    let state = tempfile::tempdir().unwrap();
    auth::create_user(state.path(), "admin", "admin-session-test-password").unwrap();
    auth::create_user(state.path(), "viewer", "viewer-session-test-password").unwrap();
    let admin = auth::authenticate(
        state.path(),
        "admin",
        "admin-session-test-password",
        "server",
    )
    .unwrap()
    .unwrap();
    let viewer = auth::authenticate(
        state.path(),
        "viewer",
        "viewer-session-test-password",
        "server",
    )
    .unwrap()
    .unwrap();
    let router = app::router(Arc::new(AppState::with_auth_dir(
        "server".to_owned(),
        state.path().to_path_buf(),
    )));
    let denied = send(&router, "GET", "/Sessions", Body::empty(), None).await;
    assert_eq!(denied.status(), StatusCode::UNAUTHORIZED);
    let viewer_sessions = json_response(
        send(
            &router,
            "GET",
            "/Sessions",
            Body::empty(),
            Some(&viewer.access_token),
        )
        .await,
    )
    .await;
    assert_eq!(viewer_sessions.as_array().unwrap().len(), 1);
    assert_eq!(viewer_sessions[0]["UserName"], "viewer");
    let all_sessions = json_response(
        send(
            &router,
            "GET",
            "/Sessions",
            Body::empty(),
            Some(&admin.access_token),
        )
        .await,
    )
    .await;
    assert_eq!(all_sessions.as_array().unwrap().len(), 2);
    let unsupported = send(
        &router,
        "GET",
        "/Sessions?activeWithinSeconds=30",
        Body::empty(),
        Some(&admin.access_token),
    )
    .await;
    assert_eq!(unsupported.status(), StatusCode::NOT_IMPLEMENTED);
}
