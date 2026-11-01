//! Authenticated native Matroska ASS transport with synthetic local media.
//! ASS style preservation does not establish real-client font/rendering parity.
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

const SYNTHETIC_ASS: &str = r#"[Script Info]
Title: Synthetic test
ScriptType: v4.00+
PlayResX: 640
PlayResY: 360
[V4+ Styles]
Format: Name, Fontname, Fontsize, PrimaryColour, SecondaryColour, OutlineColour, BackColour, Bold, Italic, Underline, StrikeOut, ScaleX, ScaleY, Spacing, Angle, BorderStyle, Outline, Shadow, Alignment, MarginL, MarginR, MarginV, Encoding
Style: Default,DejaVu Sans,24,&H00FFFFFF,&H000000FF,&H00000000,&H64000000,0,0,0,0,100,100,0,0,1,2,0,2,10,10,10,1
[Events]
Format: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text
Dialogue: 0,0:00:01.00,0:00:03.00,Default,,0,0,0,,Hello ASS
"#;

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

async fn post_playback_with_subtitle_profile(
    app: &Router,
    item: &str,
    token: &str,
    index: i64,
    method: &str,
    format: &str,
) -> Response {
    let body = serde_json::json!({
        "SubtitleStreamIndex": index,
        "DeviceProfile": {
            "DirectPlayProfiles": [
                {"Type":"Video","Container":"mkv","VideoCodec":"mpeg4"}
            ],
            "SubtitleProfiles": [
                {"Method":method,"Format":format}
            ]
        }
    });
    let request = Request::builder()
        .method("POST")
        .uri(format!("/Items/{item}/PlaybackInfo"))
        .header("Content-Type", "application/json")
        .header("X-Emby-Token", token)
        .body(Body::from(body.to_string()))
        .unwrap();
    app.clone().oneshot(request).await.unwrap()
}

#[tokio::test]
async fn embedded_ass_preserves_native_styles_and_does_not_modify_source() {
    if std::env::var_os("PELAGOS_REQUIRE_FFMPEG").is_none() {
        return;
    }
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("Movies");
    let state_dir = temp.path().join("state");
    fs::create_dir_all(&root).unwrap();
    let subtitle = temp.path().join("fixture.ass");
    fs::write(&subtitle, SYNTHETIC_ASS).unwrap();
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
            "color=s=32x32:r=5:d=5",
            "-i",
        ])
        .arg(&subtitle)
        .args([
            "-map", "0:v:0", "-map", "1:s:0", "-c:v", "mpeg4", "-c:s", "ass", "-y",
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
    auth::create_user(&state_dir, "admin", "native-ass-admin-password").unwrap();
    auth::create_user(&state_dir, "viewer", "native-ass-viewer-password").unwrap();
    let admin = auth::authenticate(&state_dir, "admin", "native-ass-admin-password", "server")
        .unwrap()
        .unwrap()
        .access_token;
    let viewer = auth::authenticate(&state_dir, "viewer", "native-ass-viewer-password", "server")
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
    let playback = request(
        &app,
        "GET",
        &format!("/Items/{id}/PlaybackInfo"),
        Some(&viewer),
    )
    .await;
    assert_eq!(playback.status(), StatusCode::OK);
    let info: serde_json::Value =
        serde_json::from_slice(&to_bytes(playback.into_body(), 65_536).await.unwrap()).unwrap();
    let embedded = info["MediaSources"][0]["MediaStreams"]
        .as_array()
        .unwrap()
        .iter()
        .find(|stream| stream["Type"] == "Subtitle")
        .unwrap();
    assert_eq!(embedded["Codec"], "ass");
    assert_eq!(embedded["IsExternal"], false);
    let index = embedded["Index"].as_i64().unwrap();
    let url = format!("/Videos/{id}/{id}/Subtitles/{index}/Stream.ass");
    assert_eq!(embedded["DeliveryUrl"].as_str(), Some(url.as_str()));
    // The same selected embedded ASS index can be separately delivered
    // through the format explicitly permitted by this client's profile.
    for (method, format, extension, expected) in [
        ("External", "ass", "ass", StatusCode::OK),
        ("External", "srt", "srt", StatusCode::OK),
        ("External", "webvtt", "vtt", StatusCode::OK),
        ("External", "-ass", "srt", StatusCode::OK),
        ("External", "ssa", "ass", StatusCode::NOT_IMPLEMENTED),
        ("Embed", "srt", "ass", StatusCode::NOT_IMPLEMENTED),
    ] {
        let response = post_playback_with_subtitle_profile(
            &app, id, &viewer, index, method, format,
        )
        .await;
        assert_eq!(response.status(), expected, "{method}/{format}");
        if expected != StatusCode::OK {
            continue;
        }
        let info: serde_json::Value = serde_json::from_slice(
            &to_bytes(response.into_body(), 65_536).await.unwrap()
        )
        .unwrap();
        assert_eq!(
            info["MediaSources"][0]["DefaultSubtitleStreamIndex"],
            index
        );
        let selected = info["MediaSources"][0]["MediaStreams"]
            .as_array()
            .unwrap()
            .iter()
            .find(|track| track["Index"] == index && track["Type"] == "Subtitle")
            .unwrap();
        assert_eq!(selected["Codec"], "ass");
        assert_eq!(selected["IsExternal"], false);
        assert_eq!(
            selected["DeliveryUrl"],
            format!("/Videos/{id}/{id}/Subtitles/{index}/Stream.{extension}")
        );
    }
    assert_eq!(
        request(&app, "GET", &url, None).await.status(),
        StatusCode::UNAUTHORIZED
    );
    let response = request(&app, "GET", &url, Some(&viewer)).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers()[header::CONTENT_TYPE],
        "text/x-ass; charset=utf-8"
    );
    assert_eq!(
        response.headers()[header::CACHE_CONTROL],
        "private, no-store"
    );
    let length = response.headers()[header::CONTENT_LENGTH].clone();
    let bytes = to_bytes(response.into_body(), 4 * 1024 * 1024 + 4096)
        .await
        .unwrap();
    let text = std::str::from_utf8(&bytes).unwrap();
    assert!(text.contains("Style: Default,DejaVu Sans"));
    assert!(text.contains("Hello ASS"));
    assert_eq!(length.to_str().unwrap(), bytes.len().to_string());
    let head = request(&app, "HEAD", &url, Some(&viewer)).await;
    assert_eq!(head.status(), StatusCode::OK);
    assert_eq!(head.headers()[header::CONTENT_LENGTH], length);
    assert!(to_bytes(head.into_body(), 64).await.unwrap().is_empty());

    let srt_url = format!("/Videos/{id}/{id}/Subtitles/{index}/Stream.srt");
    let srt = request(&app, "GET", &srt_url, Some(&viewer)).await;
    assert_eq!(srt.status(), StatusCode::OK);
    let srt_bytes = to_bytes(srt.into_body(), 65_536).await.unwrap();
    assert!(
        std::str::from_utf8(&srt_bytes)
            .unwrap()
            .contains("Hello ASS")
    );
    // Advertising native ASS must not remove the text-HLS fallback.
    let playlist_url =
        format!("/Videos/{id}/{id}/Subtitles/{index}/subtitles.m3u8?segmentLength=2");
    let playlist = request(&app, "GET", &playlist_url, Some(&viewer)).await;
    assert_eq!(playlist.status(), StatusCode::OK);
    let playlist_bytes = to_bytes(playlist.into_body(), 65_536).await.unwrap();
    let manifest = std::str::from_utf8(&playlist_bytes).unwrap();
    let segment = manifest
        .lines()
        .find(|line| line.starts_with("stream.vtt?"))
        .unwrap();
    let segment_url = format!("/Videos/{id}/{id}/Subtitles/{index}/{segment}");
    let response = request(&app, "GET", &segment_url, Some(&viewer)).await;
    assert_eq!(response.status(), StatusCode::OK);
    let segment_bytes = to_bytes(response.into_body(), 65_536).await.unwrap();
    assert!(
        std::str::from_utf8(&segment_bytes)
            .unwrap()
            .contains("Hello ASS")
    );
    assert_eq!(
        request(
            &app,
            "GET",
            &format!("/Videos/{id}/{id}/Subtitles/{index}/Stream.ssa"),
            Some(&viewer),
        )
        .await
        .status(),
        StatusCode::NOT_IMPLEMENTED
    );
    assert_eq!(fs::read(&movie).unwrap(), original);
    auth::revoke_library(&state_dir, "viewer", &root).unwrap();
    assert_eq!(
        request(&app, "GET", &url, Some(&viewer)).await.status(),
        StatusCode::NOT_FOUND
    );
}
