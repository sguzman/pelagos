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
    let route = "/Sessions/Capabilities?playableMediaTypes=Audio,Video&supportedCommands=Play,PlayState&supportsMediaControl=true";
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
    for invalid in [
        "/Sessions/Capabilities?supportedCommands=Pause",
        "/Sessions/Capabilities?supportedCommands=NoSuchCommand",
        "/Sessions/Capabilities?playableMediaTypes=Video,Music",
    ] {
        assert_eq!(
            call(&router, "POST", invalid, Some(&first.access_token))
                .await
                .status(),
            StatusCode::BAD_REQUEST
        );
    }
    assert_eq!(
        call(&router, "POST", route, Some(&first.access_token))
            .await
            .status(),
        StatusCode::NO_CONTENT
    );

    let rows = body(call(&router, "GET", "/Sessions", Some(&first.access_token)).await).await;
    let sessions = rows.as_array().unwrap();
    assert_eq!(sessions.len(), 2);
    let reported = sessions
        .iter()
        .find(|row| row["Id"] == first.session_info.id)
        .unwrap();
    let untouched = sessions
        .iter()
        .find(|row| row["Id"] == second.session_info.id)
        .unwrap();
    assert_eq!(reported["SupportsMediaControl"], true);
    assert_eq!(reported["Capabilities"]["SupportsMediaControl"], true);
    assert_eq!(
        reported["Capabilities"]["PlayableMediaTypes"],
        serde_json::json!(["Audio", "Video"])
    );
    assert_eq!(reported["SupportsRemoteControl"], false);
    assert_eq!(
        reported["PlayableMediaTypes"],
        serde_json::json!(["Audio", "Video"])
    );
    assert_eq!(
        reported["SupportedCommands"],
        serde_json::json!(["Play", "PlayState"])
    );
    assert_eq!(untouched["SupportsMediaControl"], false);
    assert_eq!(untouched["Capabilities"], Value::Null);
    assert_eq!(untouched["PlayableMediaTypes"], serde_json::json!([]));

    let restarted = app::router(Arc::new(AppState::with_auth_dir(
        "server".to_owned(),
        state.clone(),
    )));
    let after = body(call(&restarted, "GET", "/Sessions", Some(&first.access_token)).await).await;
    assert!(
        after.as_array().unwrap().iter().any(|row| {
            row["Id"] == first.session_info.id && row["SupportsMediaControl"] == true
        })
    );
    auth::revoke(&state, &first.access_token).unwrap();
    let after_logout =
        body(call(&restarted, "GET", "/Sessions", Some(&second.access_token)).await).await;
    assert_eq!(after_logout.as_array().unwrap().len(), 1);
    assert_eq!(after_logout[0]["Id"], second.session_info.id);
}

async fn report_full(
    router: &Router,
    token: Option<&str>,
    route: &str,
    value: Value,
) -> StatusCode {
    let mut request = Request::builder()
        .method("POST")
        .uri(route)
        .header("Content-Type", "application/json");
    if let Some(token) = token {
        request = request.header("X-Emby-Token", token);
    }
    router
        .clone()
        .oneshot(request.body(Body::from(value.to_string())).unwrap())
        .await
        .unwrap()
        .status()
}

#[tokio::test]
async fn full_capabilities_accept_basic_json_forms_and_reject_malformed_device_profiles() {
    let fixture = tempfile::tempdir().unwrap();
    let state = fixture.path().join("state");
    auth::create_user(&state, "admin", "full-capabilities-admin-password").unwrap();
    auth::create_user(&state, "viewer", "full-capabilities-viewer-password").unwrap();
    let viewer = auth::authenticate(
        &state,
        "viewer",
        "full-capabilities-viewer-password",
        "server",
    )
    .unwrap()
    .unwrap();
    let router = app::router(Arc::new(AppState::with_auth_dir(
        "server".to_owned(),
        state.clone(),
    )));
    let route = "/Sessions/Capabilities/Full";
    let payload = serde_json::json!({
        "PlayableMediaTypes": "Audio,Video",
        "SupportedCommands": ["Play", "PlayState"],
        "SupportsMediaControl": true,
        "SupportsPersistentIdentifier": true
    });
    assert_eq!(
        report_full(&router, None, route, payload.clone()).await,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        report_full(
            &router,
            Some(&viewer.access_token),
            route,
            serde_json::json!({
                "PlayableMediaTypes":"Video",
                "DeviceProfile":["not a profile object"]
            }),
        )
        .await,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        report_full(
            &router,
            Some(&viewer.access_token),
            "/Sessions/Capabilities/Full?id=nonexistent",
            payload.clone(),
        )
        .await,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        report_full(
            &router,
            Some(&viewer.access_token),
            route,
            serde_json::json!({"SupportedCommands":["Pause"]}),
        )
        .await,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        report_full(&router, Some(&viewer.access_token), route, payload).await,
        StatusCode::NO_CONTENT
    );
    let sessions = body(call(&router, "GET", "/Sessions", Some(&viewer.access_token)).await).await;
    assert_eq!(sessions[0]["SupportsMediaControl"], true);
    assert_eq!(sessions[0]["SupportsRemoteControl"], false);
    assert_eq!(
        sessions[0]["PlayableMediaTypes"],
        serde_json::json!(["Audio", "Video"])
    );
    assert_eq!(
        sessions[0]["SupportedCommands"],
        serde_json::json!(["Play", "PlayState"])
    );
}

#[tokio::test]
async fn full_capability_profile_is_bounded_persisted_scoped_and_replaced_atomically() {
    let fixture = tempfile::tempdir().unwrap();
    let state = fixture.path().join("state");
    auth::create_user(&state, "admin", "profile-echo-admin-password").unwrap();
    auth::create_user(&state, "viewer", "profile-echo-viewer-password").unwrap();
    let admin = auth::authenticate(&state, "admin", "profile-echo-admin-password", "server")
        .unwrap()
        .unwrap();
    let viewer = auth::authenticate_with_client(
        &state,
        "viewer",
        "profile-echo-viewer-password",
        "server",
        auth::ClientMetadata {
            client: Some("Jellyfin Android TV".to_owned()),
            device_id: Some("device-profile-tv".to_owned()),
            device_name: Some("Living Room".to_owned()),
            application_version: Some("1.0".to_owned()),
        },
    )
    .unwrap()
    .unwrap();
    let router = app::router(Arc::new(AppState::with_auth_dir(
        "server".to_owned(),
        state.clone(),
    )));
    let path = "/Sessions/Capabilities/Full";
    let profile = serde_json::json!({
        "Name": "Observed TV negotiation offer",
        "MaxStreamingBitrate": 3_000_000,
        "DirectPlayProfiles": [
            {"Container": "mp4", "VideoCodec": "h264", "AudioCodec": "aac", "Type": "Video"}
        ],
        "TranscodingProfiles": [],
        "SubtitleProfiles": []
    });
    let payload = serde_json::json!({
        "PlayableMediaTypes": ["Video","Audio"],
        "SupportedCommands": "Play,PlayState",
        "SupportsMediaControl": true,
        "SupportsPersistentIdentifier": true,
        "DeviceProfile": profile,
        "AppStoreUrl": "https://example.org/app",
        "IconUrl": "https://example.org/icon.png"
    });
    assert_eq!(
        report_full(&router, Some(&viewer.access_token), path, payload.clone()).await,
        StatusCode::NO_CONTENT
    );
    let info = body(
        call(
            &router,
            "GET",
            "/Devices/Info?id=device-profile-tv",
            Some(&admin.access_token),
        )
        .await,
    )
    .await;
    assert_eq!(info["Capabilities"]["DeviceProfile"], profile);
    let sessions = body(call(&router, "GET", "/Sessions", Some(&viewer.access_token)).await).await;
    assert_eq!(sessions[0]["Capabilities"]["DeviceProfile"], profile);
    assert_eq!(
        sessions[0]["Capabilities"]["AppStoreUrl"],
        "https://example.org/app"
    );
    assert_eq!(
        info["Capabilities"]["AppStoreUrl"],
        "https://example.org/app"
    );
    assert_eq!(
        info["Capabilities"]["IconUrl"],
        "https://example.org/icon.png"
    );
    assert_eq!(info["IconUrl"], "https://example.org/icon.png");
    assert_eq!(info["AccessToken"], Value::Null);
    assert!(!info.to_string().contains(&viewer.access_token));

    let restarted = app::router(Arc::new(AppState::with_auth_dir(
        "server".to_owned(),
        state.clone(),
    )));
    let persisted = body(
        call(
            &restarted,
            "GET",
            "/Devices/Info?id=device-profile-tv",
            Some(&admin.access_token),
        )
        .await,
    )
    .await;
    assert_eq!(persisted["Capabilities"]["DeviceProfile"], profile);

    for bad in [
        serde_json::json!({"DeviceProfile":{"Name":"x".repeat(70_000)}}),
        serde_json::json!({"DeviceProfile":{"Name":2},"IconUrl":"javascript:alert(1)"}),
        serde_json::json!({"DeviceProfile":"not-an-object"}),
        serde_json::json!({"AppStoreUrl":"file:///etc/passwd"}),
    ] {
        assert_eq!(
            report_full(&router, Some(&viewer.access_token), path, bad).await,
            StatusCode::BAD_REQUEST
        );
    }
    // An invalid update cannot erase a previously successful capability report.
    let after_bad = body(
        call(
            &router,
            "GET",
            "/Devices/Info?id=device-profile-tv",
            Some(&admin.access_token),
        )
        .await,
    )
    .await;
    assert_eq!(after_bad["Capabilities"]["DeviceProfile"], profile);

    assert_eq!(
        call(
            &router,
            "POST",
            "/Sessions/Capabilities?playableMediaTypes=Audio&supportsPersistentIdentifier=true",
            Some(&viewer.access_token),
        )
        .await
        .status(),
        StatusCode::NO_CONTENT
    );
    let basic = body(
        call(
            &router,
            "GET",
            "/Devices/Info?id=device-profile-tv",
            Some(&admin.access_token),
        )
        .await,
    )
    .await;
    assert_eq!(basic["Capabilities"]["DeviceProfile"], Value::Null);
    assert_eq!(basic["Capabilities"]["AppStoreUrl"], Value::Null);
    assert_eq!(basic["Capabilities"]["IconUrl"], Value::Null);
    assert_eq!(
        basic["Capabilities"]["PlayableMediaTypes"],
        serde_json::json!(["Audio"])
    );
    auth::revoke(&state, &viewer.access_token).unwrap();
    let after_revoke = body(
        call(
            &router,
            "GET",
            "/Devices/Info?id=device-profile-tv",
            Some(&admin.access_token),
        )
        .await,
    )
    .await;
    assert_eq!(after_revoke["Capabilities"]["DeviceProfile"], Value::Null);
}
