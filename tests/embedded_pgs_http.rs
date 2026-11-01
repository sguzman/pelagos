//! Synthetic real-FFmpeg Matroska with an embedded raw PGS packet.
//! Verifies indexed graphical metadata and read-only ranged extraction, not
//! bitmap rendering or unchanged TV-client parity.
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

const RAW_PGS: &[u8] = &[b'P', b'G', 0, 0, 0, 0, 0, 0, 0, 0, 0x80, 0, 0];

async fn request(
    app: &Router,
    method: &str,
    url: &str,
    token: Option<&str>,
    range: Option<&str>,
) -> Response {
    let mut builder = Request::builder().method(method).uri(url);
    if let Some(token) = token {
        builder = builder.header("X-Emby-Token", token);
    }
    if let Some(range) = range {
        builder = builder.header("Range", range);
    }
    app.clone()
        .oneshot(builder.body(Body::empty()).unwrap())
        .await
        .unwrap()
}

#[tokio::test]
async fn embedded_matroska_pgs_stream_is_extracted_raw_and_read_by_ranges() {
    if std::env::var_os("PELAGOS_REQUIRE_FFMPEG").is_none() {
        return;
    }
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("Movies");
    let state = temp.path().join("state");
    fs::create_dir_all(&root).unwrap();
    let sup = temp.path().join("synthetic.sup");
    fs::write(&sup, RAW_PGS).unwrap();
    let movie = root.join("Embedded.mkv");
    let result = Command::new("ffmpeg")
        .args([
            "-nostdin",
            "-hide_banner",
            "-loglevel",
            "error",
            "-f",
            "lavfi",
            "-i",
            "color=s=32x32:r=5:d=3",
            "-i",
        ])
        .arg(&sup)
        .args([
            "-map", "0:v:0", "-map", "1:s:0", "-c:v", "mpeg4", "-c:s", "copy", "-y",
        ])
        .arg(&movie)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let original = fs::read(&movie).unwrap();
    catalog::index(&state, &root).unwrap();
    auth::create_user(&state, "admin", "embedded-pgs-admin-password").unwrap();
    auth::create_user(&state, "viewer", "embedded-pgs-viewer-password").unwrap();
    let token = auth::authenticate(&state, "viewer", "embedded-pgs-viewer-password", "server")
        .unwrap()
        .unwrap()
        .access_token;
    auth::grant_library(&state, "viewer", &root).unwrap();
    let app = app::router(Arc::new(AppState::with_auth_dir(
        "server".to_owned(),
        state.clone(),
    )));
    let response = request(&app, "GET", "/Items", Some(&token), None).await;
    assert_eq!(response.status(), StatusCode::OK);
    let items: serde_json::Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 65_536).await.unwrap()).unwrap();
    let id = items["Items"][0]["Id"].as_str().unwrap();
    let info = request(
        &app,
        "GET",
        &format!("/Items/{id}/PlaybackInfo"),
        Some(&token),
        None,
    )
    .await;
    assert_eq!(info.status(), StatusCode::OK);
    let info: serde_json::Value =
        serde_json::from_slice(&to_bytes(info.into_body(), 65_536).await.unwrap()).unwrap();
    let sub = info["MediaSources"][0]["MediaStreams"]
        .as_array()
        .unwrap()
        .iter()
        .find(|stream| stream["Type"] == "Subtitle")
        .unwrap();
    assert_eq!(sub["Codec"], "hdmv_pgs_subtitle");
    assert_eq!(sub["IsExternal"], false);
    assert_eq!(sub["IsTextSubtitleStream"], false);
    assert_eq!(sub["DeliveryMethod"], "External");
    let index = sub["Index"].as_i64().unwrap();
    let url = format!("/Videos/{id}/{id}/Subtitles/{index}/Stream.pgssub");
    assert_eq!(sub["DeliveryUrl"].as_str(), Some(url.as_str()));
    assert_eq!(
        request(&app, "GET", &url, None, None).await.status(),
        StatusCode::UNAUTHORIZED
    );
    let response = request(&app, "GET", &url, Some(&token), None).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers()[header::CONTENT_LENGTH],
        RAW_PGS.len().to_string()
    );
    assert_eq!(response.headers()[header::ACCEPT_RANGES], "bytes");
    assert_eq!(
        response.headers()[header::CONTENT_TYPE],
        "application/octet-stream"
    );
    assert_eq!(
        to_bytes(response.into_body(), 1024).await.unwrap().as_ref(),
        RAW_PGS
    );
    let response = request(&app, "GET", &url, Some(&token), Some("bytes=2-7")).await;
    assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(response.headers()[header::CONTENT_LENGTH], "6");
    assert_eq!(response.headers()[header::CONTENT_RANGE], "bytes 2-7/13");
    assert_eq!(
        to_bytes(response.into_body(), 1024).await.unwrap().as_ref(),
        &RAW_PGS[2..=7]
    );
    let timed = format!("/Videos/{id}/{id}/Subtitles/{index}/0/Stream.sup");
    let response = request(&app, "HEAD", &timed, Some(&token), Some("bytes=-3")).await;
    assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(response.headers()[header::CONTENT_LENGTH], "3");
    assert_eq!(response.headers()[header::CONTENT_RANGE], "bytes 10-12/13");
    assert!(to_bytes(response.into_body(), 64).await.unwrap().is_empty());
    assert_eq!(
        request(&app, "GET", &url, Some(&token), Some("bytes=99-"))
            .await
            .status(),
        StatusCode::RANGE_NOT_SATISFIABLE
    );
    let unsupported = format!("/Videos/{id}/{id}/Subtitles/{index}/Stream.srt");
    assert_eq!(
        request(&app, "GET", &unsupported, Some(&token), None)
            .await
            .status(),
        StatusCode::NOT_IMPLEMENTED
    );
    assert_eq!(fs::read(&movie).unwrap(), original);
    auth::revoke_library(&state, "viewer", &root).unwrap();
    assert_eq!(
        request(&app, "GET", &url, Some(&token), None)
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
}
