use std::sync::Arc;

use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode},
    response::Response,
};
use pelagos::{
    app::{self, AppState},
    auth::{self, ClientMetadata},
};
use serde_json::{Value, json};
use tower::ServiceExt;

async fn call(router: &Router, path: &str, token: Option<&str>) -> Response {
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

async fn report(router: &Router, path: &str, token: &str) -> StatusCode {
    router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(path)
                .header("X-Emby-Token", token)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
        .status()
}

async fn json_body(response: Response) -> Value {
    serde_json::from_slice(&to_bytes(response.into_body(), 65536).await.unwrap()).unwrap()
}

#[tokio::test]
async fn registered_devices_are_admin_only_stable_deduplicated_and_credential_free() {
    let fixture = tempfile::tempdir().unwrap();
    let state = fixture.path().join("state");
    auth::create_user(&state, "admin", "device-inventory-admin-password").unwrap();
    auth::create_user(&state, "viewer", "device-inventory-viewer-password").unwrap();
    let admin = auth::authenticate(&state, "admin", "device-inventory-admin-password", "server")
        .unwrap()
        .unwrap();
    let client = ClientMetadata {
        client: Some("Jellyfin Android TV".to_owned()),
        device_id: Some("living-room-tv".to_owned()),
        device_name: Some("Living Room".to_owned()),
        application_version: Some("0.18".to_owned()),
    };
    let viewer = auth::authenticate_with_client(
        &state,
        "viewer",
        "device-inventory-viewer-password",
        "server",
        client.clone(),
    )
    .unwrap()
    .unwrap();
    let duplicate = auth::authenticate_with_client(
        &state,
        "viewer",
        "device-inventory-viewer-password",
        "server",
        client,
    )
    .unwrap()
    .unwrap();
    let second = auth::authenticate_with_client(
        &state,
        "viewer",
        "device-inventory-viewer-password",
        "server",
        ClientMetadata {
            client: Some("Jellyfin Roku".to_owned()),
            device_id: Some("roku-1".to_owned()),
            device_name: Some("Den".to_owned()),
            application_version: Some("2.0".to_owned()),
        },
    )
    .unwrap()
    .unwrap();
    let router = app::router(Arc::new(AppState::with_auth_dir(
        "server".to_owned(),
        state.clone(),
    )));
    assert_eq!(
        call(&router, "/Devices", None).await.status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        call(&router, "/Devices", Some(&viewer.access_token))
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        call(
            &router,
            "/Devices/Info?id=roku-1",
            Some(&viewer.access_token)
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    let listing = json_body(call(&router, "/Devices", Some(&admin.access_token)).await).await;
    assert_eq!(listing["TotalRecordCount"], 2);
    assert_eq!(listing["StartIndex"], 0);
    let entries = listing["Items"].as_array().unwrap();
    assert_eq!(entries.len(), 2);
    let tv = entries
        .iter()
        .find(|d| d["Id"] == "living-room-tv")
        .unwrap();
    assert_eq!(tv["Name"], "Living Room");
    assert_eq!(tv["AppName"], "Jellyfin Android TV");
    assert_eq!(tv["AppVersion"], "0.18");
    assert_eq!(tv["LastUserId"], viewer.user.id);
    assert_eq!(tv["LastUserName"], "viewer");
    assert_eq!(tv["AccessToken"], Value::Null);
    assert_eq!(tv["DateLastActivity"], Value::Null);
    assert_eq!(tv["Capabilities"]["SupportsMediaControl"], false);
    assert!(!listing.to_string().contains(&viewer.access_token));
    assert!(!listing.to_string().contains(&duplicate.access_token));

    let filtered = json_body(
        call(
            &router,
            &format!("/Devices?userId={}", viewer.user.id),
            Some(&admin.access_token),
        )
        .await,
    )
    .await;
    assert_eq!(filtered["TotalRecordCount"], 2);
    let unknown = json_body(
        call(
            &router,
            "/Devices?userId=00000000-0000-0000-0000-000000000000",
            Some(&admin.access_token),
        )
        .await,
    )
    .await;
    assert_eq!(unknown["Items"], json!([]));
    assert_eq!(
        call(
            &router,
            "/Devices/Info?id=missing",
            Some(&admin.access_token)
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );

    assert_eq!(
        report(
            &router,
            "/Sessions/Capabilities?playableMediaTypes=Audio,Video&supportedCommands=Play&supportsMediaControl=true",
            &duplicate.access_token,
        )
        .await,
        StatusCode::NO_CONTENT
    );
    let reported = json_body(
        call(
            &router,
            "/Devices/Info?id=living-room-tv",
            Some(&admin.access_token),
        )
        .await,
    )
    .await;
    assert_eq!(
        reported["Capabilities"]["PlayableMediaTypes"],
        json!(["Audio", "Video"])
    );
    assert_eq!(
        reported["Capabilities"]["SupportedCommands"],
        json!(["Play"])
    );
    assert_eq!(reported["Capabilities"]["SupportsMediaControl"], true);

    auth::revoke(&state, &viewer.access_token).unwrap();
    auth::revoke(&state, &duplicate.access_token).unwrap();
    auth::revoke(&state, &second.access_token).unwrap();
    let restarted = app::router(Arc::new(AppState::with_auth_dir(
        "server".to_owned(),
        state.clone(),
    )));
    let by_id = json_body(
        call(
            &restarted,
            "/Devices/Info?id=living-room-tv",
            Some(&admin.access_token),
        )
        .await,
    )
    .await;
    assert_eq!(by_id["Id"], "living-room-tv");
    assert_eq!(by_id["LastUserId"], viewer.user.id);
    assert_eq!(by_id["Capabilities"]["SupportedCommands"], json!([]));
}

#[tokio::test]
async fn login_without_device_id_does_not_fabricate_device() {
    let fixture = tempfile::tempdir().unwrap();
    let state = fixture.path().join("state");
    auth::create_user(&state, "admin", "anonymous-device-admin-password").unwrap();
    let admin = auth::authenticate(&state, "admin", "anonymous-device-admin-password", "server")
        .unwrap()
        .unwrap();
    let router = app::router(Arc::new(AppState::with_auth_dir(
        "server".to_owned(),
        state,
    )));
    let listing = json_body(call(&router, "/Devices", Some(&admin.access_token)).await).await;
    assert_eq!(listing["Items"], json!([]));
    assert_eq!(listing["TotalRecordCount"], 0);
}
