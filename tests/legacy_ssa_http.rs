//! Legacy SSA in Matroska is FFprobe-reported as ASS; native ASS must fail
//! closed for v4.00 scripts while SRT conversion remains usable.
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

const LEGACY_SSA: &str = r#"[Script Info]
Title: Legacy SSA
ScriptType: v4.00
[V4 Styles]
Format: Name, Fontname, Fontsize, PrimaryColour, SecondaryColour, BackColour, Bold, Italic, BorderStyle, Outline, Shadow, Alignment, MarginL, MarginR, MarginV, AlphaLevel, Encoding
Style: Default,Arial,24,&H00FFFFFF,&H000000FF,&H00000000,0,0,1,2,0,2,10,10,10,0,1
[Events]
Format: Marked, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text
Dialogue: Marked=0,0:00:01.00,0:00:03.00,Default,,0,0,0,,Hello SSA
"#;

async fn request(app: &Router, url: &str, token: Option<&str>) -> Response {
    let mut builder = Request::builder().uri(url);
    if let Some(token) = token {
        builder = builder.header("X-Emby-Token", token);
    }
    app.clone()
        .oneshot(builder.body(Body::empty()).unwrap())
        .await
        .unwrap()
}

#[tokio::test]
async fn legacy_ssa_does_not_serve_a_mislabeled_native_ass_response() {
    if std::env::var_os("PELAGOS_REQUIRE_FFMPEG").is_none() {
        return;
    }
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("Movies");
    let state_dir = temp.path().join("state");
    fs::create_dir_all(&root).unwrap();
    let subtitle = temp.path().join("legacy.ssa");
    fs::write(&subtitle, LEGACY_SSA).unwrap();
    let movie = root.join("Legacy.mkv");
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
    // The first account is an administrator; exercise library revocation
    // with a distinct non-admin viewer, not the bootstrap owner.
    auth::create_user(&state_dir, "admin", "legacy-ssa-admin-password").unwrap();
    auth::create_user(&state_dir, "viewer", "legacy-ssa-viewer-password").unwrap();
    let token = auth::authenticate(&state_dir, "viewer", "legacy-ssa-viewer-password", "server")
        .unwrap()
        .unwrap()
        .access_token;
    auth::grant_library(&state_dir, "viewer", &root).unwrap();
    let app = app::router(Arc::new(AppState::with_auth_dir(
        "server".to_owned(),
        state_dir.clone(),
    )));
    let items = request(&app, "/Items", Some(&token)).await;
    assert_eq!(items.status(), StatusCode::OK);
    let items: serde_json::Value =
        serde_json::from_slice(&to_bytes(items.into_body(), 65_536).await.unwrap()).unwrap();
    let id = items["Items"][0]["Id"].as_str().unwrap();
    let info = request(&app, &format!("/Items/{id}/PlaybackInfo"), Some(&token)).await;
    assert_eq!(info.status(), StatusCode::OK);
    let info: serde_json::Value =
        serde_json::from_slice(&to_bytes(info.into_body(), 65_536).await.unwrap()).unwrap();
    let stream = info["MediaSources"][0]["MediaStreams"]
        .as_array()
        .unwrap()
        .iter()
        .find(|stream| stream["Type"] == "Subtitle")
        .unwrap();
    // FFprobe reports the legacy script as codec "ass". This cannot
    // establish the original style-script format.
    assert_eq!(stream["Codec"], "ass");
    let index = stream["Index"].as_i64().unwrap();
    // Metadata must no longer advertise the false native ASS extension;
    // the known SRT conversion remains the truthful default.
    let srt = format!("/Videos/{id}/{id}/Subtitles/{index}/Stream.srt");
    assert_eq!(stream["DeliveryUrl"].as_str(), Some(srt.as_str()));
    // Device format negotiation must not upgrade legacy SSA to a false
    // native ASS offer even though FFprobe calls its source codec "ass".
    for (format, expected) in [
        ("ass", StatusCode::NOT_IMPLEMENTED),
        ("srt", StatusCode::OK),
        ("webvtt", StatusCode::OK),
    ] {
        let body = serde_json::json!({
            "SubtitleStreamIndex": index,
            "DeviceProfile": {
                "DirectPlayProfiles": [
                    {"Type":"Video","Container":"mkv","VideoCodec":"mpeg4"}
                ],
                "SubtitleProfiles": [{"Method":"External","Format":format}]
            }
        });
        let req = Request::builder()
            .method("POST")
            .uri(format!("/Items/{id}/PlaybackInfo"))
            .header("Content-Type", "application/json")
            .header("X-Emby-Token", token.as_str())
            .body(Body::from(body.to_string()))
            .unwrap();
        let response = app.clone().oneshot(req).await.unwrap();
        assert_eq!(response.status(), expected, "{format}");
        if expected == StatusCode::OK {
            let info: serde_json::Value = serde_json::from_slice(
                &to_bytes(response.into_body(), 65_536).await.unwrap()
            )
            .unwrap();
            let selected = info["MediaSources"][0]["MediaStreams"]
                .as_array()
                .unwrap()
                .iter()
                .find(|stream| stream["Index"] == index)
                .unwrap();
            let extension = if format == "webvtt" { "vtt" } else { "srt" };
            assert_eq!(
                selected["DeliveryUrl"],
                format!("/Videos/{id}/{id}/Subtitles/{index}/Stream.{extension}")
            );
            assert_eq!(selected["Codec"], "ass");
        }
    }
    let ass = format!("/Videos/{id}/{id}/Subtitles/{index}/Stream.ass");
    assert_eq!(
        request(&app, &ass, None).await.status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        request(&app, &ass, Some(&token)).await.status(),
        StatusCode::NOT_IMPLEMENTED
    );
    let response = request(&app, &srt, Some(&token)).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers()[header::CACHE_CONTROL],
        "private, no-store"
    );
    let bytes = to_bytes(response.into_body(), 65_536).await.unwrap();
    assert!(std::str::from_utf8(&bytes).unwrap().contains("Hello SSA"));
    assert_eq!(fs::read(&movie).unwrap(), original);
    auth::revoke_library(&state_dir, "viewer", &root).unwrap();
    assert_eq!(
        request(&app, &ass, Some(&token)).await.status(),
        StatusCode::NOT_FOUND
    );
}
