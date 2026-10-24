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

async fn get(router: &Router, path: &str, token: Option<&str>) -> Response {
    let mut request = Request::builder().method("GET").uri(path);
    if let Some(token) = token {
        request = request.header("X-Emby-Token", token);
    }
    router
        .clone()
        .oneshot(request.body(Body::empty()).unwrap())
        .await
        .unwrap()
}

async fn result_json(response: Response) -> Value {
    serde_json::from_slice(&to_bytes(response.into_body(), 65536).await.unwrap()).unwrap()
}

#[tokio::test]
async fn authentication_provider_discovery_is_elevated_and_reports_only_real_providers() {
    let fixture = tempfile::tempdir().unwrap();
    let state = fixture.path().join("state");
    auth::create_user(&state, "admin", "provider-discovery-admin-password").unwrap();
    auth::create_user(&state, "viewer", "provider-discovery-viewer-password").unwrap();
    let admin = auth::authenticate(&state, "admin", "provider-discovery-admin-password", "server")
        .unwrap()
        .unwrap();
    let viewer = auth::authenticate(&state, "viewer", "provider-discovery-viewer-password", "server")
        .unwrap()
        .unwrap();
    let router = app::router(Arc::new(AppState::with_auth_dir(
        "server".to_owned(),
        state.clone(),
    )));
    for route in ["/Auth/Providers", "/Auth/PasswordResetProviders"] {
        assert_eq!(get(&router, route, None).await.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(
            get(&router, route, Some(&viewer.access_token))
                .await
                .status(),
            StatusCode::FORBIDDEN
        );
    }
    let local = result_json(
        get(&router, "/Auth/Providers", Some(&admin.access_token)).await,
    )
    .await;
    assert_eq!(
        local,
        json!([{"Name":"Default","Id":"pelagos.auth.local.argon2id"}])
    );
    assert!(!local.to_string().contains(&admin.access_token));
    assert!(!local.to_string().contains("Jellyfin.Server.Implementations"));
    let reset = result_json(
        get(
            &router,
            "/Auth/PasswordResetProviders",
            Some(&admin.access_token),
        )
        .await,
    )
    .await;
    assert_eq!(reset, json!([]));
    auth::revoke(&state, &admin.access_token).unwrap();
    assert_eq!(
        get(&router, "/Auth/Providers", Some(&admin.access_token))
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
}
