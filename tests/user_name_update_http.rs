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
    route: &str,
    token: Option<&str>,
    body: Value,
) -> Response {
    let mut request = Request::builder().method(method).uri(route);
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
async fn administrator_account_rename_preserves_user_identity_sessions_and_permissions() {
    let fixture = tempfile::tempdir().unwrap();
    let state = fixture.path().join("state");
    let root = fixture.path().join("Media");
    std::fs::create_dir_all(&root).unwrap();
    auth::create_user(&state, "admin", "rename-user-admin-password").unwrap();
    auth::create_user(&state, "viewer", "rename-user-viewer-password").unwrap();
    auth::create_user(&state, "taken", "rename-user-taken-password").unwrap();
    let admin = auth::authenticate(&state, "admin", "rename-user-admin-password", "server")
        .unwrap()
        .unwrap();
    let viewer = auth::authenticate(&state, "viewer", "rename-user-viewer-password", "server")
        .unwrap()
        .unwrap();
    auth::grant_library(&state, "viewer", &root).unwrap();
    let router = app::router(Arc::new(AppState::with_auth_dir(
        "server".to_owned(),
        state.clone(),
    )));
    let route = format!("/Users/{}", viewer.user.id);
    let patch = json!({"Name":"Renamed Viewer"});
    assert_eq!(
        call(&router, "POST", &route, None, patch.clone())
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
            patch.clone(),
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        call(
            &router,
            "POST",
            "/Users/00000000-0000-0000-0000-000000000000",
            Some(&admin.access_token),
            patch.clone(),
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        call(
            &router,
            "POST",
            &route,
            Some(&admin.access_token),
            json!({"Name":"TAKEN"}),
        )
        .await
        .status(),
        StatusCode::CONFLICT
    );
    let unknown = call(
        &router,
        "POST",
        &route,
        Some(&admin.access_token),
        json!({"Name":"Bad","Policy":{"IsAdministrator":true}}),
    )
    .await;
    assert!(
        unknown.status() == StatusCode::UNPROCESSABLE_ENTITY
            || unknown.status() == StatusCode::BAD_REQUEST
    );
    assert_eq!(
        call(
            &router,
            "POST",
            &route,
            Some(&admin.access_token),
            json!({"Name":"  Bad"}),
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        call(&router, "POST", &route, Some(&admin.access_token), patch)
            .await
            .status(),
        StatusCode::NO_CONTENT
    );
    let after = json_body(
        call(
            &router,
            "GET",
            &route,
            Some(&viewer.access_token),
            json!({}),
        )
        .await,
    )
    .await;
    assert_eq!(after["Id"], viewer.user.id);
    assert_eq!(after["Name"], "Renamed Viewer");
    assert_eq!(after["Policy"]["IsAdministrator"], false);
    assert_eq!(after["Policy"]["IsDisabled"], false);
    let seen_by_admin = json_body(
        call(
            &router,
            "GET",
            &route,
            Some(&admin.access_token),
            json!({}),
        )
        .await,
    )
    .await;
    assert_eq!(after, seen_by_admin);
    assert!(auth::visible_library_roots(
        &state,
        &auth::identify(&state, &viewer.access_token, "server")
            .unwrap()
            .unwrap(),
    )
    .unwrap()
    .unwrap()
    .contains(&std::fs::canonicalize(&root).unwrap().to_string_lossy().into_owned()));

    assert!(
        auth::authenticate(&state, "viewer", "rename-user-viewer-password", "server")
            .unwrap()
            .is_none()
    );
    let new_login =
        auth::authenticate(&state, "Renamed Viewer", "rename-user-viewer-password", "server")
            .unwrap()
            .unwrap();
    assert_eq!(new_login.user.id, viewer.user.id);
    let restarted = app::router(Arc::new(AppState::with_auth_dir(
        "server".to_owned(),
        state.clone(),
    )));
    assert_eq!(
        json_body(
            call(
                &restarted,
                "GET",
                &route,
                Some(&viewer.access_token),
                json!({}),
            )
            .await,
        )
        .await["Name"],
        "Renamed Viewer"
    );
    assert_eq!(
        json_body(
            call(
                &restarted,
                "GET",
                "/Users",
                Some(&admin.access_token),
                json!({}),
            )
            .await,
        )
        .await[1]["Name"],
        "Renamed Viewer"
    );
}
