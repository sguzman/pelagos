//! Short-VOD subtitle HLS HTTP regression using isolated synthetic media.
//! This verifies header authentication only, not unchanged TV client parity.
use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
    response::Response,
};
use pelagos::{
    app::{self, AppState},
    auth, catalog,
};
use std::{fs, process::Command, sync::Arc};
use tower::ServiceExt;

async fn request(app: &Router, method: &str, url: &str, token: Option<&str>) -> Response {
    let mut builder = Request::builder().method(method).uri(url);
    if let Some(token) = token {
        builder = builder.header("X-Emby-Token", token);
    }
    app.clone()
        .oneshot(builder.body(Body::empty()).unwrap())
        .await
        .unwrap()
}

#[tokio::test]
async fn short_vod_subtitle_playlist_clips_segments_and_rechecks_authorization() {
    if std::env::var_os("PELAGOS_REQUIRE_FFMPEG").is_none() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("Videos");
    let state_dir = dir.path().join("state");
    fs::create_dir_all(&root).unwrap();
    let movie = root.join("Feature.mp4");
    let output = Command::new("ffmpeg")
        .args([
            "-nostdin",
            "-hide_banner",
            "-loglevel",
            "error",
            "-f",
            "lavfi",
            "-i",
            "color=s=32x32:r=5:d=5",
            "-c:v",
            "mpeg4",
            "-y",
        ])
        .arg(&movie)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let original = fs::read(&movie).unwrap();
    fs::write(
        root.join("Feature.en.vtt"),
        b"WEBVTT\n\nintro\n00:00:01.000 --> 00:00:03.000 align:start\nHello!\n\n",
    )
    .unwrap();
    catalog::index(&state_dir, &root).unwrap();
    auth::create_user(&state_dir, "admin", "subtitle-admin-password").unwrap();
    auth::create_user(&state_dir, "viewer", "subtitle-viewer-password").unwrap();
    let admin = auth::authenticate(
        &state_dir,
        "admin",
        "subtitle-admin-password",
        "server",
    )
    .unwrap()
    .unwrap()
    .access_token;
    let viewer = auth::authenticate(
        &state_dir,
        "viewer",
        "subtitle-viewer-password",
        "server",
    )
    .unwrap()
    .unwrap()
    .access_token;
    auth::grant_library(&state_dir, "viewer", &root).unwrap();
    let app = app::router(Arc::new(AppState::with_auth_dir(
        "server".to_owned(),
        state_dir.clone(),
    )));
    let items = request(&app, "GET", "/Items", Some(&admin)).await;
    assert_eq!(items.status(), StatusCode::OK);
    let items: serde_json::Value =
        serde_json::from_slice(&to_bytes(items.into_body(), 65_536).await.unwrap()).unwrap();
    let id = items["Items"][0]["Id"].as_str().unwrap();
    let source = request(
        &app,
        "GET",
        &format!("/Items/{id}/PlaybackInfo"),
        Some(&viewer),
    )
    .await;
    assert_eq!(source.status(), StatusCode::OK);
    let info: serde_json::Value =
        serde_json::from_slice(&to_bytes(source.into_body(), 65_536).await.unwrap()).unwrap();
    let subtitle = info["MediaSources"][0]["MediaStreams"]
        .as_array()
        .unwrap()
        .iter()
        .find(|stream| stream["Type"] == "Subtitle" && stream["IsExternal"].as_bool() == Some(true))
        .expect("adjacent VTT subtitle is advertised");
    let index = subtitle["Index"].as_i64().unwrap();
    let playlist_url = format!(
        "/Videos/{id}/{id}/Subtitles/{index}/subtitles.m3u8?segmentLength=2"
    );
    assert_eq!(
        request(&app, "GET", &playlist_url, None).await.status(),
        StatusCode::UNAUTHORIZED
    );
    let playlist = request(&app, "GET", &playlist_url, Some(&viewer)).await;
    assert_eq!(playlist.status(), StatusCode::OK);
    assert_eq!(
        playlist.headers()[header::CONTENT_TYPE],
        "application/vnd.apple.mpegurl"
    );
    assert_eq!(playlist.headers()[header::CACHE_CONTROL], "private, no-store");
    let bytes = to_bytes(playlist.into_body(), 65_536).await.unwrap();
    let manifest = std::str::from_utf8(&bytes).unwrap();
    assert!(manifest.starts_with("#EXTM3U\n"));
    assert!(manifest.ends_with("#EXT-X-ENDLIST\n"));
    assert!(
        !manifest.contains(viewer.as_str()),
        "no session token in HLS URLs"
    );
    let segments = manifest
        .lines()
        .filter(|line| line.starts_with("stream.vtt?"))
        .collect::<Vec<_>>();
    assert_eq!(segments.len(), 3, "{manifest}");
    let first = format!("/Videos/{id}/{id}/Subtitles/{index}/{}", segments[0]);
    let second = format!("/Videos/{id}/{id}/Subtitles/{index}/{}", segments[1]);
    assert_eq!(
        request(&app, "GET", &first, None).await.status(),
        StatusCode::UNAUTHORIZED
    );
    let first_response = request(&app, "GET", &first, Some(&viewer)).await;
    assert_eq!(first_response.status(), StatusCode::OK);
    let first_content = to_bytes(first_response.into_body(), 65_536).await.unwrap();
    let first_text = std::str::from_utf8(&first_content).unwrap();
    assert!(first_text.contains("00:00:01.000 --> 00:00:02.000 align:start"));
    assert!(first_text.contains("Hello!"));
    let response = request(&app, "GET", &second, Some(&viewer)).await;
    assert_eq!(response.status(), StatusCode::OK);
    let length = response.headers()[header::CONTENT_LENGTH].clone();
    let content = to_bytes(response.into_body(), 65_536).await.unwrap();
    let text = std::str::from_utf8(&content).unwrap();
    assert!(text.starts_with(
        "WEBVTT\nX-TIMESTAMP-MAP=MPEGTS:900000,LOCAL:00:00:00.000\n\n"
    ));
    assert!(text.contains("00:00:02.000 --> 00:00:03.000 align:start"));
    assert!(text.contains("Hello!"));
    assert_eq!(length.to_str().unwrap(), content.len().to_string());
    let head = request(&app, "HEAD", &second, Some(&viewer)).await;
    assert_eq!(head.status(), StatusCode::OK);
    assert_eq!(head.headers()[header::CONTENT_LENGTH], length);
    assert!(to_bytes(head.into_body(), 64).await.unwrap().is_empty());
    let third = format!("/Videos/{id}/{id}/Subtitles/{index}/{}", segments[2]);
    let final_response = request(&app, "GET", &third, Some(&viewer)).await;
    assert_eq!(final_response.status(), StatusCode::OK);
    let final_body = to_bytes(final_response.into_body(), 65_536).await.unwrap();
    assert_eq!(
        final_body.as_ref(),
        b"WEBVTT\nX-TIMESTAMP-MAP=MPEGTS:900000,LOCAL:00:00:00.000\n\n"
    );
    assert_eq!(fs::read(&movie).unwrap(), original);
    auth::revoke_library(&state_dir, "viewer", &root).unwrap();
    assert_eq!(
        request(&app, "GET", &first, Some(&viewer)).await.status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        request(&app, "GET", &playlist_url, Some(&viewer))
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
}
