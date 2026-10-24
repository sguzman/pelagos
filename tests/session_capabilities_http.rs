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

async fn call(router: &Router, method: &str, path: &str, token: Option<&str>) -> Response {
    let mut request = Request::builder().method(method).uri(path);
    if let Some(token) = token {
        request = request.header("X-Emby-Token", token);
    }
    router
        .clone()
        .oneshot(request.body(Body::empty()).unwrap())
        .await
        .unwrap()
}

async fn body(response: Response) -> Value {
    serde_json::from_slice(&to_bytes(response.into_body(), 65536).await.unwrap()).unwrap()
}

#[tokio::test]
async fn capabilities_are_confined_to_the_reporting_session_and_persist() {
    let fixture = tempfile::tempdir().unwrap();
    let state = fixture.path().join("state");
    auth::create_user(&state, "admin", "capabilities-admin-password").unwrap();
    auth::create_user(&state, "viewer", "capabilities-viewer-password").unwrap();
    let first = auth::authenticate(&state, "viewer", "capabilities-viewer-password", "server")
        .unwrap()
        .unwrap();
    let second = auth::authenticate(&state, "viewer", "capabilities-viewer-password", "server")
        .unwrap()
        .unwrap();
    let router = app::router(Arc::new(AppState::with_auth_dir(
        "server".to_owned(),
        state.clone(),
    )));
    let route = "/Sessions/Capabilities?playableMediaTypes=Audio,Video&supportedCommands=Play,Pause&supportsMediaControl=true";
    assert_eq!(
        call(&router, "POST", route, None).await.status(),
        StatusCode::UNAUTHORIZED
    );
    let other = format!("/Sessions/Capabilities?id={}", second.session_info.id);
    assert_eq!(
        call(&router, "POST", &other, Some(&first.access_token))
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        call(
            &router,
            "POST",
            "/Sessions/Capabilities?playableMediaTypes=Audio,,Video",
            Some(&first.access_token),
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        call(&router, "POST", route, Some(&first.access_token))
            .await
            .status(),
        StatusCode::NO_CONTENT
    );

    let rows = body(call(&router, "GET", "/Sessions", Some(&first.access_token)).await).await;
    let sessions = rows.as_array().unwrap();
    assert_eq!(sessions.len(), 2);
    let reported = sessions.iter().find(|row| row["Id"] == first.session_info.id).unwrap();
    let untouched = sessions.iter().find(|row| row["Id"] == second.session_info.id).unwrap();
    assert_eq!(reported["SupportsMediaControl"], true);
    assert_eq!(reported["SupportsRemoteControl"], false);
    assert_eq!(reported["PlayableMediaTypes"], serde_json::json!(["Audio", "Video"]));
    assert_eq!(reported["SupportedCommands"], serde_json::json!(["Play", "Pause"]));
    assert_eq!(untouched["SupportsMediaControl"], false);
    assert_eq!(untouched["PlayableMediaTypes"], serde_json::json!([]));

    let restarted = app::router(Arc::new(AppState::with_auth_dir(
        "server".to_owned(),
        state.clone(),
    )));
    let after = body(call(&restarted, "GET", "/Sessions", Some(&first.access_token)).await).await;
    assert!(after.as_array().unwrap().iter().any(|row| {
        row["Id"] == first.session_info.id && row["SupportsMediaControl"] == true
    }));
    auth::revoke(&state, &first.access_token).unwrap();
    let after_logout = body(call(&restarted, "GET", "/Sessions", Some(&second.access_token)).await).await;
    assert_eq!(after_logout.as_array().unwrap().len(), 1);
    assert_eq!(after_logout[0]["Id"], second.session_info.id);
}
