use std::sync::Arc;

use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode, header},
    response::Response,
};
use pelagos::{
    app::{self, AppState},
    auth,
};
use tower::ServiceExt;

async fn request(
    router: &Router,
    method: &str,
    path: &str,
    token: Option<&str>,
    body: &str,
) -> Response {
    let mut builder = Request::builder()
        .uri(path)
        .method(method)
        .header(header::CONTENT_TYPE, "application/json");
    if let Some(token) = token {
        builder = builder.header("X-Emby-Token", token);
    }
    router
        .clone()
        .oneshot(builder.body(Body::from(body.to_owned())).unwrap())
        .await
        .unwrap()
}

#[tokio::test]
async fn playback_requires_auth_and_rejects_unsupported_modes() {
    let temp = tempfile::tempdir().unwrap();
    auth::create_user(temp.path(), "admin", "long-private-test-password").unwrap();
    auth::create_user(temp.path(), "viewer", "long-viewer-test-password").unwrap();
    let admin = auth::authenticate(temp.path(), "admin", "long-private-test-password", "server")
        .unwrap()
        .unwrap()
        .access_token;
    let viewer = auth::authenticate(temp.path(), "viewer", "long-viewer-test-password", "server")
        .unwrap()
        .unwrap()
        .access_token;
    let router = app::router(Arc::new(AppState::with_auth_dir(
        "server".to_owned(),
        temp.path().to_path_buf(),
    )));
    let path = "/Items/00000000000000000000000000000001/PlaybackInfo";
    assert_eq!(
        request(&router, "GET", path, None, "",).await.status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        request(&router, "GET", path, Some(&viewer), "")
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        request(
            &router,
            "POST",
            path,
            Some(&admin),
            r#"{"EnableDirectPlay":false}"#,
        )
        .await
        .status(),
        StatusCode::NOT_IMPLEMENTED
    );
    assert_eq!(
        request(&router, "GET", path, Some(&admin), "")
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
}

#[cfg(unix)]
#[tokio::test]
async fn indexed_video_playback_metadata_comes_from_bounded_ffprobe_fixture() {
    use std::{fs, os::unix::fs::PermissionsExt};

    use axum::body::to_bytes;
    use pelagos::catalog;
    use serde_json::Value;

    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("Movies");
    let state_dir = temp.path().join("state");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("Feature.mp4"), b"synthetic ffprobe fixture").unwrap();
    catalog::index(&state_dir, &root).unwrap();

    let executable = temp.path().join("fake-ffprobe");
    fs::write(
        &executable,
        "#!/bin/sh\nprintf '%s' '{\"format\":{\"duration\":\"61.25\"},\"streams\":[{\"index\":0,\"codec_type\":\"video\",\"codec_name\":\"h264\",\"width\":640,\"height\":360},{\"index\":1,\"codec_type\":\"audio\",\"codec_name\":\"aac\",\"sample_rate\":\"48000\",\"channels\":2}]}'\n",
    )
    .unwrap();
    fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();

    auth::create_user(&state_dir, "admin", "strong-playback-test-password").unwrap();
    let token = auth::authenticate(
        &state_dir,
        "admin",
        "strong-playback-test-password",
        "server",
    )
    .unwrap()
    .unwrap()
    .access_token;
    let app_state =
        AppState::with_auth_dir("server".to_owned(), state_dir).with_probe_executable(executable);
    let router = app::router(Arc::new(app_state));

    let catalog_response = request(&router, "GET", "/Items", Some(&token), "").await;
    assert_eq!(catalog_response.status(), StatusCode::OK);
    let bytes = to_bytes(catalog_response.into_body(), 65536).await.unwrap();
    let items: Value = serde_json::from_slice(&bytes).unwrap();
    let id = items["Items"][0]["Id"].as_str().unwrap();
    let route = format!("/Items/{id}/PlaybackInfo");

    let response = request(&router, "GET", &route, Some(&token), "").await;
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = to_bytes(response.into_body(), 65536).await.unwrap();
    let info: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(info["MediaSources"][0]["Protocol"], "File");
    assert_eq!(info["MediaSources"][0]["Container"], "mp4");
    assert_eq!(info["MediaSources"][0]["RunTimeTicks"], 612_500_000);
    assert_eq!(info["MediaSources"][0]["MediaStreams"][0]["Codec"], "h264");
    assert_eq!(
        info["MediaSources"][0]["MediaStreams"][1]["SampleRate"],
        48000
    );
    assert_eq!(info["MediaSources"][0]["SupportsTranscoding"], false);
    assert_eq!(info["MediaSources"][0]["SupportsDirectPlay"], true);
    assert!(info["MediaSources"][0].get("Path").is_none());

    let post = request(
        &router,
        "POST",
        &route,
        Some(&token),
        r#"{"EnableDirectPlay":true,"EnableTranscoding":false}"#,
    )
    .await;
    assert_eq!(post.status(), StatusCode::OK);
    let rejected = request(
        &router,
        "POST",
        &route,
        Some(&token),
        r#"{"EnableDirectPlay":false}"#,
    )
    .await;
    assert_eq!(rejected.status(), StatusCode::NOT_IMPLEMENTED);
}
