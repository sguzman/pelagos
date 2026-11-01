//! Source-backed authenticated Jellyfin-shaped HLS subtitle burn-in.
//! Uses only synthetic media; does not prove unmodified TV client behavior.
use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode},
    response::Response,
};
use pelagos::{
    app::{self, AppState},
    auth, catalog,
};
use serde_json::Value;
use std::{fs, process::Command, sync::Arc};
use tower::ServiceExt;

async fn call(app: &Router, path: &str, token: Option<&str>) -> Response {
    let mut builder = Request::builder().uri(path);
    if let Some(token) = token {
        builder = builder.header("X-Emby-Token", token);
    }
    app.clone()
        .oneshot(builder.body(Body::empty()).unwrap())
        .await
        .unwrap()
}

async fn body_string(response: Response) -> String {
    String::from_utf8(
        to_bytes(response.into_body(), 64 * 1024)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap()
}

async fn segment_bytes(app: &Router, id: &str, manifest: &str, token: &str, segment: usize) -> Vec<u8> {
    let line = manifest
        .lines()
        .filter(|line| line.starts_with("hls1/") && line.ends_with(".ts"))
        .nth(segment)
        .expect("published HLS segment URL");
    let response = call(app, &format!("/Videos/{id}/{line}"), Some(token)).await;
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = to_bytes(response.into_body(), 8 * 1024 * 1024)
        .await
        .unwrap()
        .to_vec();
    assert!(!bytes.is_empty() && bytes.len() % 188 == 0);
    assert!(bytes.chunks_exact(188).all(|packet| packet[0] == 0x47));
    bytes
}

fn bright_pixels(media: &[u8], path: &std::path::Path) -> usize {
    fs::write(path, media).unwrap();
    let output = Command::new("ffmpeg")
        .args(["-nostdin", "-hide_banner", "-loglevel", "error", "-i"])
        .arg(path)
        .args([
            "-frames:v", "1", "-vf", "select=eq(n\\,3),format=gray",
            "-f", "rawvideo", "pipe:1",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "decode FFmpeg failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout.len(), 320 * 128);
    output.stdout.iter().filter(|pixel| **pixel > 80).count()
}

#[cfg(unix)]
#[tokio::test]
async fn selected_subrip_track_is_burned_into_seeked_hls_without_cross_cache_reuse() {
    if std::env::var_os("PELAGOS_REQUIRE_FFMPEG").is_none() {
        return;
    }
    let temp = tempfile::tempdir().unwrap();
    let library = temp.path().join("Movies");
    let state_dir = temp.path().join("state");
    fs::create_dir_all(&library).unwrap();
    let source = library.join("Feature.mkv");
    let subtitles = temp.path().join("input.srt");
    fs::write(
        &subtitles,
        b"1\n00:00:00,200 --> 00:00:01,800\nPELAGOS FIRST SUBTITLE\n\n2\n00:00:02,200 --> 00:00:03,800\nPELAGOS SECOND SUBTITLE\n\n",
    )
    .unwrap();
    let created = Command::new("ffmpeg")
        .args([
            "-nostdin", "-hide_banner", "-loglevel", "error", "-f", "lavfi", "-i",
            "color=c=black:s=320x128:r=5:d=4", "-f", "srt", "-i",
        ])
        .arg(&subtitles)
        .args([
            "-map", "0:v:0", "-map", "1:s:0", "-c:v", "mpeg4", "-c:s", "srt",
            "-y",
        ])
        .arg(&source)
        .output()
        .unwrap();
    assert!(
        created.status.success(),
        "{}",
        String::from_utf8_lossy(&created.stderr)
    );
    let original = fs::read(&source).unwrap();
    catalog::index(&state_dir, &library).unwrap();
    auth::create_user(&state_dir, "admin", "subtitle-burnin-password").unwrap();
    let token = auth::authenticate(&state_dir, "admin", "subtitle-burnin-password", "server")
        .unwrap()
        .unwrap()
        .access_token;
    let app = app::router(Arc::new(AppState::with_auth_dir(
        "server".to_owned(),
        state_dir,
    )));
    let items = call(&app, "/Items", Some(&token)).await;
    assert_eq!(items.status(), StatusCode::OK);
    let values: Value =
        serde_json::from_str(&body_string(items).await).unwrap();
    let id = values["Items"][0]["Id"].as_str().unwrap();
    let info = call(
        &app,
        &format!("/Items/{id}/PlaybackInfo"),
        Some(&token),
    )
    .await;
    let info: Value = serde_json::from_str(&body_string(info).await).unwrap();
    let subtitle = info["MediaSources"][0]["MediaStreams"]
        .as_array()
        .unwrap()
        .iter()
        .find(|stream| stream["Type"] == "Subtitle")
        .expect("embedded SubRip stream");
    let index = subtitle["Index"].as_i64().unwrap();
    assert_eq!(subtitle["Codec"], "subrip");

    let plain_url = format!("/Videos/{id}/main.m3u8?segmentLength=2");
    let burn_url = format!("{plain_url}&subtitleStreamIndex={index}");
    assert_eq!(call(&app, &burn_url, None).await.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(
        call(&app, &format!("{plain_url}&subtitleStreamIndex=999"), Some(&token))
            .await
            .status(),
        StatusCode::NOT_IMPLEMENTED
    );
    assert_eq!(
        call(
            &app,
            &format!("{burn_url}&renditions=2&enableAdaptiveBitrateStreaming=true"),
            Some(&token)
        )
        .await
        .status(),
        StatusCode::NOT_IMPLEMENTED
    );

    let plain_response = call(&app, &plain_url, Some(&token)).await;
    assert_eq!(plain_response.status(), StatusCode::OK);
    let plain = body_string(plain_response).await;
    let burn_response = call(&app, &burn_url, Some(&token)).await;
    assert_eq!(burn_response.status(), StatusCode::OK);
    let burned = body_string(burn_response).await;
    assert_ne!(plain, burned);
    assert_eq!(
        body_string(call(&app, &burn_url, Some(&token)).await).await,
        burned
    );
    let disable = format!("{plain_url}&subtitleStreamIndex=-1");
    assert_eq!(
        body_string(call(&app, &disable, Some(&token)).await).await,
        plain
    );
    assert_eq!(burned.matches("#EXTINF:").count(), 2);

    let undecorated = segment_bytes(&app, id, &plain, &token, 0).await;
    let first = segment_bytes(&app, id, &burned, &token, 0).await;
    let second = segment_bytes(&app, id, &burned, &token, 1).await;
    assert_ne!(undecorated, first);
    let reference = bright_pixels(&undecorated, &temp.path().join("plain.ts"));
    let first_white = bright_pixels(&first, &temp.path().join("first.ts"));
    let second_white = bright_pixels(&second, &temp.path().join("second.ts"));
    assert!(first_white > reference + 20);
    assert!(second_white > reference + 20);
    assert_eq!(fs::read(&source).unwrap(), original);
}
