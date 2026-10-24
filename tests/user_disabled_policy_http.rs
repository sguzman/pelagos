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
use serde_json::{Value, json};
use tower::ServiceExt;

async fn call(
    router: &Router,
    method: &str,
    path: &str,
    token: Option<&str>,
    body: Value,
) -> Response {
    let mut request = Request::builder().method(method).uri(path);
    if let Some(token) = token {
        request = request.header("X-Emby-Token", token);
    }
    router
        .clone()
        .oneshot(
            request
                .header("Content-Type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap()
}

async fn json_body(response: Response) -> Value {
    serde_json::from_slice(&to_bytes(response.into_body(), 65536).await.unwrap()).unwrap()
}

#[tokio::test]
async fn disabled_policy_subset_revokes_sessions_and_protects_administrators() {
    let fixture = tempfile::tempdir().unwrap();
    let state = fixture.path().join("state");
    auth::create_user(&state, "admin", "disable-account-admin-password").unwrap();
    auth::create_user(&state, "viewer", "disable-account-viewer-password").unwrap();
    let admin = auth::authenticate(&state, "admin", "disable-account-admin-password", "server")
        .unwrap()
        .unwrap();
    let viewer = auth::authenticate(&state, "viewer", "disable-account-viewer-password", "server")
        .unwrap()
        .unwrap();
    let other_session = auth::authenticate(
        &state,
        "viewer",
        "disable-account-viewer-password",
        "server",
    )
    .unwrap()
    .unwrap();
    let router = app::router(Arc::new(AppState::with_auth_dir(
        "server".to_owned(),
        state.clone(),
    )));
    let route = format!("/Users/{}/Policy", viewer.user.id);
    let valid = json!({"IsDisabled":true});
    assert_eq!(
        call(&router, "POST", &route, None, valid.clone())
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
            valid.clone(),
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        call(
            &router,
            "POST",
            &format!("/Users/{}/Policy", admin.user.id),
            Some(&admin.access_token),
            valid.clone(),
        )
        .await
        .status(),
        StatusCode::CONFLICT
    );
    assert_eq!(
        call(
            &router,
            "POST",
            "/Users/not-a-user/Policy",
            Some(&admin.access_token),
            valid.clone(),
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
    let unsupported = call(
        &router,
        "POST",
        &route,
        Some(&admin.access_token),
        json!({"IsDisabled":true,"IsAdministrator":true}),
    )
    .await;
    assert!(
        unsupported.status() == StatusCode::UNPROCESSABLE_ENTITY
            || unsupported.status() == StatusCode::BAD_REQUEST
    );
    assert!(auth::identify(&state, &viewer.access_token, "server")
        .unwrap()
        .is_some());

    assert_eq!(
        call(&router, "POST", &route, Some(&admin.access_token), valid)
            .await
            .status(),
        StatusCode::NO_CONTENT
    );
    for token in [&viewer.access_token, &other_session.access_token] {
        assert_eq!(
            call(&router, "GET", "/Users/Me", Some(token), json!({}))
                .await
                .status(),
            StatusCode::UNAUTHORIZED
        );
    }
    assert!(
        auth::authenticate(
            &state,
            "viewer",
            "disable-account-viewer-password",
            "server",
        )
        .unwrap()
        .is_none()
    );
    let disabled_list = json_body(
        call(
            &router,
            "GET",
            "/Users?isDisabled=true",
            Some(&admin.access_token),
            json!({}),
        )
        .await,
    )
    .await;
    assert_eq!(disabled_list.as_array().unwrap().len(), 1);
    assert_eq!(disabled_list[0]["Id"], viewer.user.id);
    assert_eq!(disabled_list[0]["Policy"]["IsDisabled"], true);

    let restored = app::router(Arc::new(AppState::with_auth_dir(
        "server".to_owned(),
        state.clone(),
    )));
    assert_eq!(
        call(
            &restored,
            "POST",
            &route,
            Some(&admin.access_token),
            json!({"IsDisabled":false}),
        )
        .await
        .status(),
        StatusCode::NO_CONTENT
    );
    assert!(
        auth::identify(&state, &viewer.access_token, "server")
            .unwrap()
            .is_none()
    );
    assert_eq!(
        json_body(
            call(
                &restored,
                "GET",
                "/Users?isDisabled=true",
                Some(&admin.access_token),
                json!({}),
            )
            .await,
        )
        .await,
        json!([])
    );
    assert!(
        auth::authenticate(
            &state,
            "viewer",
            "disable-account-viewer-password",
            "server",
        )
        .unwrap()
        .is_some()
    );
    assert_eq!(
        call(
            &restored,
            "GET",
            "/Users/Me",
            Some(&admin.access_token),
            json!({}),
        )
        .await
        .status(),
        StatusCode::OK
    );
}
