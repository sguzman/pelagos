//! Real FFmpeg test with synthetic Matroska font and non-font attachments.
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
use std::sync::Arc;
use tower::ServiceExt;

async fn request(app: &Router, method: &str, url: &str, token: Option<&str>) -> Response {
    let mut req = Request::builder().method(method).uri(url);
    if let Some(token) = token {
        req = req.header("X-Emby-Token", token);
    }
    app.clone()
        .oneshot(req.body(Body::empty()).unwrap())
        .await
        .unwrap()
}

#[tokio::test]
async fn scoped_matroska_font_attachment_never_modifies_media() {
    use std::{fs, process::Command};
    if std::env::var_os("PELAGOS_REQUIRE_FFMPEG").is_none() {
        return;
    }
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("Movies");
    let state_dir = temp.path().join("state");
    fs::create_dir_all(&root).unwrap();
    let font = temp.path().join("fixture.ttf");
    let nonfont = temp.path().join("not-a-font.bin");
    let oversized = temp.path().join("oversized.ttf");
    let spoofed = temp.path().join("spoofed.ttf");
    // Minimal offset-table signature for the bounded transport-header check:
    // this is synthetic data, not a complete font for rendering.
    let mut bytes = vec![0_u8; 28];
    bytes[0..4].copy_from_slice(&[0, 1, 0, 0]);
    bytes[4..6].copy_from_slice(&1_u16.to_be_bytes());
    fs::write(&font, &bytes).unwrap();
    fs::write(&nonfont, b"ARBITRARY_NONFONT_PAYLOAD").unwrap();
    fs::write(&oversized, vec![0_u8; 4 * 1024 * 1024 + 1]).unwrap();
    fs::write(&spoofed, b"NOT_A_REAL_TRUETYPE_FONT").unwrap();
    let movie = root.join("Feature.mkv");
    let output = Command::new("ffmpeg")
        .args([
            "-nostdin",
            "-hide_banner",
            "-loglevel",
            "error",
            "-f",
            "lavfi",
            "-i",
            "color=s=32x32:r=5:d=1",
            "-c:v",
            "ffv1",
            "-attach",
        ])
        .arg(&font)
        .args([
            "-metadata:s:t:0",
            "mimetype=application/x-truetype-font",
            // A Matroska attachment can supply a path-shaped untrusted name.
            "-metadata:s:t:0",
            "filename=../private.ttf",
            "-attach",
        ])
        .arg(&nonfont)
        .args([
            "-metadata:s:t:1",
            "mimetype=application/octet-stream",
            "-attach",
        ])
        .arg(&oversized)
        .args([
            "-metadata:s:t:2",
            "mimetype=application/x-truetype-font",
            "-attach",
        ])
        .arg(&spoofed)
        .args([
            "-metadata:s:t:3",
            "mimetype=font/ttf",
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
    catalog::index(&state_dir, &root).unwrap();
    auth::create_user(&state_dir, "admin", "secure-attachment-admin-password").unwrap();
    auth::create_user(&state_dir, "viewer", "secure-attachment-viewer-password").unwrap();
    let admin = auth::authenticate(
        &state_dir,
        "admin",
        "secure-attachment-admin-password",
        "server",
    )
    .unwrap()
    .unwrap()
    .access_token;
    let viewer = auth::authenticate(
        &state_dir,
        "viewer",
        "secure-attachment-viewer-password",
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
    let response = request(&app, "GET", "/Items", Some(&admin)).await;
    let items: serde_json::Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 65_536).await.unwrap()).unwrap();
    let id = items["Items"][0]["Id"].as_str().unwrap();
    // A Jellyfin client needs the real font attachment stream index
    // advertised through PlaybackInfo, without exposing arbitrary blobs.
    let media_info = format!("/Items/{id}/PlaybackInfo");
    let source = request(&app, "GET", &media_info, Some(&viewer)).await;
    assert_eq!(source.status(), StatusCode::OK);
    let info: serde_json::Value =
        serde_json::from_slice(&to_bytes(source.into_body(), 65_536).await.unwrap()).unwrap();
    let streams = info["MediaSources"][0]["MediaStreams"].as_array().unwrap();
    assert_eq!(streams.len(), 1);
    let attachments = info["MediaSources"][0]["MediaAttachments"]
        .as_array()
        .unwrap();
    assert_eq!(attachments.len(), 2);
    assert_eq!(attachments[0]["Index"], 1);
    assert_eq!(attachments[0]["Codec"], "ttf");
    assert!(attachments[0]["FileName"].is_null());
    assert_eq!(attachments[0]["MimeType"], "font/ttf");
    assert_eq!(
        attachments[0]["DeliveryUrl"],
        format!("/Videos/{id}/{id}/Attachments/1")
    );
    let url = format!("/Videos/{id}/{id}/Attachments/1");
    assert_eq!(
        request(&app, "GET", &url, None).await.status(),
        StatusCode::UNAUTHORIZED
    );
    let response = request(&app, "GET", &url, Some(&viewer)).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[header::CONTENT_TYPE], "font/ttf");
    assert_eq!(response.headers()["x-content-type-options"], "nosniff");
    assert_eq!(
        to_bytes(response.into_body(), 1024).await.unwrap().as_ref(),
        bytes.as_slice()
    );
    let response = request(&app, "HEAD", &url, Some(&viewer)).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers()[header::CONTENT_LENGTH],
        bytes.len().to_string()
    );
    assert!(
        to_bytes(response.into_body(), 1024)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        request(
            &app,
            "GET",
            &format!("/Videos/{id}/{id}/Attachments/2"),
            Some(&viewer)
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        request(
            &app,
            "GET",
            &format!("/Videos/{id}/wrong/Attachments/1"),
            Some(&viewer)
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
    // A probed but oversized font is never sent to a client and never
    // written to a temporary directory by FFmpeg.
    let too_big = format!("/Videos/{id}/{id}/Attachments/3");
    assert_eq!(
        request(&app, "GET", &too_big, Some(&viewer)).await.status(),
        StatusCode::NOT_IMPLEMENTED
    );
    // A forged Matroska MIME tag must never turn arbitrary extracted
    // attachment bytes into a font payload.
    let forged = format!("/Videos/{id}/{id}/Attachments/4");
    assert_eq!(
        request(&app, "GET", &forged, Some(&viewer)).await.status(),
        StatusCode::NOT_IMPLEMENTED
    );
    assert_eq!(
        request(&app, "HEAD", &forged, Some(&viewer)).await.status(),
        StatusCode::NOT_IMPLEMENTED
    );
    assert_eq!(fs::read(&movie).unwrap(), original);
    auth::revoke_library(&state_dir, "viewer", &root).unwrap();
    assert_eq!(
        request(&app, "GET", &url, Some(&viewer)).await.status(),
        StatusCode::NOT_FOUND
    );
}
