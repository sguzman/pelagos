//! Two FFprobe "ass" streams with distinct script types must retain their
//! original per-index ASS/SSA delivery identities and authorization.
use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use pelagos::{
    app::{self, AppState},
    auth, catalog,
};
use std::{fs, process::Command, sync::Arc};
use tower::ServiceExt;

const ASS: &str = r#"[Script Info]
ScriptType: v4.00+
[V4+ Styles]
Format: Name, Fontname, Fontsize, PrimaryColour, SecondaryColour, OutlineColour, BackColour, Bold, Italic, Underline, StrikeOut, ScaleX, ScaleY, Spacing, Angle, BorderStyle, Outline, Shadow, Alignment, MarginL, MarginR, MarginV, Encoding
Style: Default,Arial,24,&H00FFFFFF,&H000000FF,&H00000000,&H00000000,0,0,0,0,100,100,0,0,1,1,0,2,10,10,10,1
[Events]
Format: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text
Dialogue: 0,0:00:01.00,0:00:03.00,Default,,0,0,0,,ASS track
"#;
const SSA: &str = r#"[Script Info]
ScriptType: v4.00
[V4 Styles]
Format: Name, Fontname, Fontsize, PrimaryColour, SecondaryColour, BackColour, Bold, Italic, BorderStyle, Outline, Shadow, Alignment, MarginL, MarginR, MarginV, AlphaLevel, Encoding
Style: Default,Arial,24,&H00FFFFFF,&H000000FF,&H00000000,0,0,1,2,0,2,10,10,10,0,1
[Events]
Format: Marked, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text
Dialogue: Marked=0,0:00:01.00,0:00:03.00,Default,,0,0,0,,SSA track
"#;

async fn get(app: &Router, url: &str, token: Option<&str>) -> axum::response::Response {
    let mut request = Request::builder().uri(url);
    if let Some(token) = token {
        request = request.header("X-Emby-Token", token);
    }
    app.clone()
        .oneshot(request.body(Body::empty()).unwrap())
        .await
        .unwrap()
}

#[tokio::test]
async fn same_matroska_ass_codec_is_not_confused_between_script_versions() {
    if std::env::var_os("PELAGOS_REQUIRE_FFMPEG").is_none() {
        return;
    }
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("Movies");
    let state_dir = temp.path().join("state");
    fs::create_dir_all(&root).unwrap();
    let ass = temp.path().join("modern.ass");
    let ssa = temp.path().join("legacy.ssa");
    fs::write(&ass, ASS).unwrap();
    fs::write(&ssa, SSA).unwrap();
    let movie = root.join("Mixed.mkv");
    let ffmpeg = Command::new("ffmpeg")
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
        .arg(&ass)
        .arg("-i")
        .arg(&ssa)
        .args([
            "-map", "0:v:0", "-map", "1:s:0", "-map", "2:s:0", "-c:v", "mpeg4", "-c:s", "ass", "-y",
        ])
        .arg(&movie)
        .output()
        .unwrap();
    assert!(
        ffmpeg.status.success(),
        "{}",
        String::from_utf8_lossy(&ffmpeg.stderr)
    );
    let original = fs::read(&movie).unwrap();
    catalog::index(&state_dir, &root).unwrap();
    auth::create_user(&state_dir, "admin", "mixed-ass-admin-password").unwrap();
    auth::create_user(&state_dir, "viewer", "mixed-ass-viewer-password").unwrap();
    let token = auth::authenticate(&state_dir, "viewer", "mixed-ass-viewer-password", "server")
        .unwrap()
        .unwrap()
        .access_token;
    auth::grant_library(&state_dir, "viewer", &root).unwrap();
    let app = app::router(Arc::new(AppState::with_auth_dir(
        "server".to_owned(),
        state_dir.clone(),
    )));
    let items = get(&app, "/Items", Some(&token)).await;
    assert_eq!(items.status(), StatusCode::OK);
    let items: serde_json::Value =
        serde_json::from_slice(&to_bytes(items.into_body(), 65_536).await.unwrap()).unwrap();
    let id = items["Items"][0]["Id"].as_str().unwrap();
    let playback = get(&app, &format!("/Items/{id}/PlaybackInfo"), Some(&token)).await;
    assert_eq!(playback.status(), StatusCode::OK);
    let data: serde_json::Value =
        serde_json::from_slice(&to_bytes(playback.into_body(), 65_536).await.unwrap()).unwrap();
    let subs = data["MediaSources"][0]["MediaStreams"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|stream| stream["Type"] == "Subtitle")
        .collect::<Vec<_>>();
    assert_eq!(subs.len(), 2);
    let mut indices = Vec::new();
    for (position, ext, marker) in [(0, "ass", "ASS track"), (1, "ssa", "SSA track")] {
        let stream = subs[position];
        assert_eq!(stream["Codec"], "ass");
        let index = stream["Index"].as_i64().unwrap();
        indices.push(index);
        let url = format!("/Videos/{id}/{id}/Subtitles/{index}/Stream.{ext}");
        assert_eq!(stream["DeliveryUrl"].as_str(), Some(url.as_str()));
        assert_eq!(
            get(&app, &url, None).await.status(),
            StatusCode::UNAUTHORIZED
        );
        let response = get(&app, &url, Some(&token)).await;
        assert_eq!(response.status(), StatusCode::OK);
        let payload = to_bytes(response.into_body(), 65_536).await.unwrap();
        let text = std::str::from_utf8(&payload).unwrap();
        assert!(text.contains(marker));
        let script = if ext == "ass" { "v4.00+" } else { "v4.00" };
        assert!(text.contains(&format!("ScriptType: {script}")));
    }
    for (index, invalid) in [(indices[0], "ssa"), (indices[1], "ass")] {
        let url = format!("/Videos/{id}/{id}/Subtitles/{index}/Stream.{invalid}");
        assert_eq!(
            get(&app, &url, Some(&token)).await.status(),
            StatusCode::NOT_IMPLEMENTED
        );
    }
    assert_eq!(fs::read(&movie).unwrap(), original);
    auth::revoke_library(&state_dir, "viewer", &root).unwrap();
    assert_eq!(
        get(&app, &format!("/Items/{id}/PlaybackInfo"), Some(&token))
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
}
