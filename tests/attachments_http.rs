//! Real FFmpeg test with synthetic Matroska font and non-font attachments.
use std::sync::Arc;
use axum::{Router, body::{Body, to_bytes}, http::{Request, StatusCode, header}, response::Response};
use pelagos::{app::{self, AppState}, auth, catalog};
use tower::ServiceExt;

async fn request(app: &Router, method: &str, url: &str, token: Option<&str>) -> Response {
    let mut req = Request::builder().method(method).uri(url);
    if let Some(token) = token { req = req.header("X-Emby-Token", token); }
    app.clone().oneshot(req.body(Body::empty()).unwrap()).await.unwrap()
}

#[tokio::test]
async fn scoped_matroska_font_attachment_never_modifies_media() {
    use std::{fs, process::Command};
    if std::env::var_os("PELAGOS_REQUIRE_FFMPEG").is_none() { return; }
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("Movies");
    let state_dir = temp.path().join("state");
    fs::create_dir_all(&root).unwrap();
    let font = temp.path().join("fixture.ttf");
    let nonfont = temp.path().join("not-a-font.bin");
    let bytes = b"SYNTHETIC_FONT_ATTACHMENT_BYTES";
    fs::write(&font, bytes).unwrap();
    fs::write(&nonfont, b"ARBITRARY_NONFONT_PAYLOAD").unwrap();
    let movie = root.join("Feature.mkv");
    let output = Command::new("ffmpeg")
        .args(["-nostdin", "-hide_banner", "-loglevel", "error", "-f", "lavfi",
            "-i", "color=s=32x32:r=5:d=1", "-c:v", "ffv1", "-attach"])
        .arg(&font)
        .args(["-metadata:s:t:0", "mimetype=application/x-truetype-font", "-attach"])
        .arg(&nonfont)
        .args(["-metadata:s:t:1", "mimetype=application/octet-stream", "-y"])
        .arg(&movie).output().unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    let original = fs::read(&movie).unwrap();
    catalog::index(&state_dir, &root).unwrap();
    auth::create_user(&state_dir, "admin", "secure-attachment-admin-password").unwrap();
    auth::create_user(&state_dir, "viewer", "secure-attachment-viewer-password").unwrap();
    let admin = auth::authenticate(&state_dir, "admin", "secure-attachment-admin-password", "server")
        .unwrap().unwrap().access_token;
    let viewer = auth::authenticate(&state_dir, "viewer", "secure-attachment-viewer-password", "server")
        .unwrap().unwrap().access_token;
    auth::grant_library(&state_dir, "viewer", &root).unwrap();
    let app = app::router(Arc::new(AppState::with_auth_dir("server".to_owned(), state_dir.clone())));
    let response = request(&app, "GET", "/Items", Some(&admin)).await;
    let items: serde_json::Value = serde_json::from_slice(
        &to_bytes(response.into_body(), 65_536).await.unwrap()
    ).unwrap();
    let id = items["Items"][0]["Id"].as_str().unwrap();
    let url = format!("/Videos/{id}/{id}/Attachments/1");
    assert_eq!(request(&app, "GET", &url, None).await.status(), StatusCode::UNAUTHORIZED);
    let response = request(&app, "GET", &url, Some(&viewer)).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[header::CONTENT_TYPE], "font/ttf");
    assert_eq!(response.headers()["x-content-type-options"], "nosniff");
    assert_eq!(to_bytes(response.into_body(), 1024).await.unwrap().as_ref(), bytes);
    let response = request(&app, "HEAD", &url, Some(&viewer)).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[header::CONTENT_LENGTH], bytes.len().to_string());
    assert!(to_bytes(response.into_body(), 1024).await.unwrap().is_empty());
    assert_eq!(
        request(&app, "GET", &format!("/Videos/{id}/{id}/Attachments/2"), Some(&viewer))
            .await.status(), StatusCode::NOT_FOUND
    );
    assert_eq!(
        request(&app, "GET", &format!("/Videos/{id}/wrong/Attachments/1"), Some(&viewer))
            .await.status(), StatusCode::NOT_FOUND
    );
    assert_eq!(fs::read(&movie).unwrap(), original);
    auth::revoke_library(&state_dir, "viewer", &root).unwrap();
    assert_eq!(request(&app, "GET", &url, Some(&viewer)).await.status(), StatusCode::NOT_FOUND);
}
