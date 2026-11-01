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

async fn segment_bytes(
    app: &Router,
    id: &str,
    manifest: &str,
    token: &str,
    segment: usize,
) -> Vec<u8> {
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
            "-frames:v",
            "1",
            "-vf",
            "select=eq(n\\,3),format=gray",
            "-f",
            "rawvideo",
            "pipe:1",
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
            "-nostdin",
            "-hide_banner",
            "-loglevel",
            "error",
            "-f",
            "lavfi",
            "-i",
            "color=c=black:s=320x128:r=5:d=4",
            "-f",
            "srt",
            "-i",
        ])
        .arg(&subtitles)
        .args([
            "-map", "0:v:0", "-map", "1:s:0", "-c:v", "mpeg4", "-c:s", "srt", "-y",
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
    let sibling = auth::authenticate(&state_dir, "admin", "subtitle-burnin-password", "server")
        .unwrap()
        .unwrap()
        .access_token;
    assert_ne!(token, sibling);
    let app = app::router(Arc::new(AppState::with_auth_dir(
        "server".to_owned(),
        state_dir,
    )));
    let items = call(&app, "/Items", Some(&token)).await;
    assert_eq!(items.status(), StatusCode::OK);
    let values: Value = serde_json::from_str(&body_string(items).await).unwrap();
    let id = values["Items"][0]["Id"].as_str().unwrap();
    let info = call(&app, &format!("/Items/{id}/PlaybackInfo"), Some(&token)).await;
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
    let burn_url = format!("{plain_url}&subtitleStreamIndex={index}&subtitleMethod=Encode");
    assert_eq!(
        call(&app, &burn_url, None).await.status(),
        StatusCode::UNAUTHORIZED
    );
    // Upstream v12.3 defaults to External delivery. An index alone cannot
    // silently trigger burn-in. Embed/Hls/Drop are not implemented here.
    for unsupported in [
        format!("{plain_url}&subtitleStreamIndex={index}"),
        format!("{plain_url}&subtitleStreamIndex={index}&subtitleMethod=External"),
        format!("{plain_url}&subtitleStreamIndex={index}&subtitleMethod=Embed"),
        format!("{plain_url}&subtitleStreamIndex={index}&subtitleMethod=Hls"),
        format!("{plain_url}&subtitleStreamIndex={index}&subtitleMethod=Drop"),
    ] {
        assert_eq!(
            call(&app, &unsupported, Some(&token)).await.status(),
            StatusCode::NOT_IMPLEMENTED
        );
    }
    assert_eq!(
        call(
            &app,
            &format!("{plain_url}&subtitleStreamIndex=999"),
            Some(&token)
        )
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
    // An /Audio route cannot use this video item ID; it must preserve
    // the existing item-type 404 instead of starting any burn-in job.
    let audio_burn = format!(
        "/Audio/{id}/main.m3u8?segmentLength=2&subtitleStreamIndex={index}&subtitleMethod=Encode"
    );
    assert_eq!(
        call(&app, &audio_burn, Some(&token)).await.status(),
        StatusCode::NOT_FOUND
    );

    let plain_response = call(&app, &plain_url, Some(&token)).await;
    assert_eq!(plain_response.status(), StatusCode::OK);
    let plain = body_string(plain_response).await;
    let burn_response = call(&app, &burn_url, Some(&token)).await;
    assert_eq!(burn_response.status(), StatusCode::OK);
    let burned = body_string(burn_response).await;
    // Enum form 0 is the pinned v12.3 Encode delivery method; it selects
    // the same publication, not a second FFmpeg render or cross-track cache.
    let numeric_encode = format!("{plain_url}&subtitleStreamIndex={index}&subtitleMethod=0");
    assert_eq!(
        body_string(call(&app, &numeric_encode, Some(&token)).await).await,
        burned
    );
    assert_ne!(plain, burned);
    // ASP.NET's pinned enum accepts the named Encode method and its 0
    // numeric value, without creating a differently keyed HLS publication.
    let numeric = format!("{plain_url}&subtitleStreamIndex={index}&subtitleMethod=0");
    assert_eq!(
        body_string(call(&app, &numeric, Some(&token)).await).await,
        burned
    );
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
    let segment_url = format!(
        "/Videos/{id}/{}",
        burned
            .lines()
            .find(|line| line.starts_with("hls1/") && line.ends_with(".ts"))
            .unwrap()
    );
    assert_eq!(
        call(&app, &segment_url, None).await.status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        call(&app, &segment_url, Some(&sibling)).await.status(),
        StatusCode::NOT_FOUND
    );
    assert_ne!(undecorated, first);
    let reference = bright_pixels(&undecorated, &temp.path().join("plain.ts"));
    let first_white = bright_pixels(&first, &temp.path().join("first.ts"));
    let second_white = bright_pixels(&second, &temp.path().join("second.ts"));
    assert!(first_white > reference + 20);
    assert!(second_white > reference + 20);
    assert_eq!(fs::read(&source).unwrap(), original);
}

#[cfg(unix)]
#[tokio::test]
async fn selected_ass_uses_subtitle_relative_position_not_global_stream_index() {
    if std::env::var_os("PELAGOS_REQUIRE_FFMPEG").is_none() {
        return;
    }
    let temp = tempfile::tempdir().unwrap();
    let library = temp.path().join("Movies");
    let state_dir = temp.path().join("state");
    fs::create_dir_all(&library).unwrap();
    let source = library.join("Multiple.mkv");
    let srt = temp.path().join("late.srt");
    let ass = temp.path().join("early.ass");
    fs::write(
        &srt,
        b"1\n00:00:02,100 --> 00:00:03,600\nLATER SUBRIP TRACK\n\n",
    )
    .unwrap();
    fs::write(
        &ass,
        r#"[Script Info]
ScriptType: v4.00+
PlayResX: 320
PlayResY: 128
[V4+ Styles]
Format: Name, Fontname, Fontsize, PrimaryColour, SecondaryColour, OutlineColour, BackColour, Bold, Italic, Underline, StrikeOut, ScaleX, ScaleY, Spacing, Angle, BorderStyle, Outline, Shadow, Alignment, MarginL, MarginR, MarginV, Encoding
Style: Default,DejaVu Sans,32,&H00FFFFFF,&H000000FF,&H00000000,&H00000000,0,0,0,0,100,100,0,0,1,1,0,2,10,10,10,1
[Events]
Format: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text
Dialogue: 0,0:00:00.10,0:00:01.90,Default,,0,0,0,,ASS SECOND TRACK
"#,
    )
    .unwrap();
    let created = Command::new("ffmpeg")
        .args([
            "-nostdin",
            "-hide_banner",
            "-loglevel",
            "error",
            "-f",
            "lavfi",
            "-i",
            "color=c=black:s=320x128:r=5:d=4",
            "-f",
            "srt",
            "-i",
        ])
        .arg(&srt)
        .args(["-f", "ass", "-i"])
        .arg(&ass)
        .args([
            "-map", "0:v:0", "-map", "1:s:0", "-map", "2:s:0", "-c:v", "mpeg4", "-c:s", "copy",
            "-y",
        ])
        .arg(&source)
        .output()
        .unwrap();
    assert!(
        created.status.success(),
        "fixture FFmpeg failed: {}",
        String::from_utf8_lossy(&created.stderr)
    );
    let untouched = fs::read(&source).unwrap();
    catalog::index(&state_dir, &library).unwrap();
    auth::create_user(&state_dir, "admin", "multiple-subtitles-password").unwrap();
    let token = auth::authenticate(&state_dir, "admin", "multiple-subtitles-password", "server")
        .unwrap()
        .unwrap()
        .access_token;
    let app = app::router(Arc::new(AppState::with_auth_dir(
        "server".to_owned(),
        state_dir,
    )));
    let items = call(&app, "/Items", Some(&token)).await;
    let items: Value = serde_json::from_str(&body_string(items).await).unwrap();
    let id = items["Items"][0]["Id"].as_str().unwrap();
    let info = call(&app, &format!("/Items/{id}/PlaybackInfo"), Some(&token)).await;
    let info: Value = serde_json::from_str(&body_string(info).await).unwrap();
    let streams = info["MediaSources"][0]["MediaStreams"].as_array().unwrap();
    let srt_index = streams
        .iter()
        .find(|stream| stream["Codec"] == "subrip")
        .unwrap()["Index"]
        .as_i64()
        .unwrap();
    let ass_index = streams
        .iter()
        .find(|stream| stream["Codec"] == "ass")
        .unwrap()["Index"]
        .as_i64()
        .unwrap();
    assert_ne!(srt_index, ass_index);

    let base = format!("/Videos/{id}/main.m3u8?segmentLength=2");
    let first = format!("{base}&subtitleStreamIndex={srt_index}&subtitleMethod=Encode");
    let second = format!("{base}&subtitleStreamIndex={ass_index}&subtitleMethod=Encode");
    let srt_response = call(&app, &first, Some(&token)).await;
    assert_eq!(srt_response.status(), StatusCode::OK);
    let srt_manifest = body_string(srt_response).await;
    let ass_response = call(&app, &second, Some(&token)).await;
    assert_eq!(ass_response.status(), StatusCode::OK);
    let ass_manifest = body_string(ass_response).await;
    assert_ne!(srt_manifest, ass_manifest);
    assert_eq!(
        body_string(call(&app, &second, Some(&token)).await).await,
        ass_manifest
    );
    let srt_bytes = segment_bytes(&app, id, &srt_manifest, &token, 0).await;
    let ass_bytes = segment_bytes(&app, id, &ass_manifest, &token, 0).await;
    let srt_bright = bright_pixels(&srt_bytes, &temp.path().join("srt.ts"));
    let ass_bright = bright_pixels(&ass_bytes, &temp.path().join("ass.ts"));
    assert!(ass_bright > srt_bright + 20);
    assert_eq!(fs::read(&source).unwrap(), untouched);
}

#[cfg(unix)]
#[tokio::test]
async fn external_srt_burn_in_rechecks_content_and_grants_without_mutating_media() {
    if std::env::var_os("PELAGOS_REQUIRE_FFMPEG").is_none() {
        return;
    }
    let temp = tempfile::tempdir().unwrap();
    let library = temp.path().join("Movies");
    let state_dir = temp.path().join("state");
    fs::create_dir_all(&library).unwrap();
    let source = library.join("Feature.mkv");
    let sidecar = library.join("Feature.es.srt");
    let created = Command::new("ffmpeg")
        .args([
            "-nostdin",
            "-hide_banner",
            "-loglevel",
            "error",
            "-f",
            "lavfi",
            "-i",
            "color=c=black:s=320x128:r=5:d=4",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:sample_rate=44100:duration=4",
            "-map",
            "0:v:0",
            "-map",
            "1:a:0",
            "-c:v",
            "mpeg4",
            "-c:a",
            "aac",
            "-y",
        ])
        .arg(&source)
        .output()
        .unwrap();
    assert!(
        created.status.success(),
        "source fixture FFmpeg: {}",
        String::from_utf8_lossy(&created.stderr)
    );
    let first_srt =
        b"1\n00:00:00,200 --> 00:00:01,800\nEXTERNAL FIRST CAPTION\n\n2\n00:00:02,200 --> 00:00:03,800\nEXTERNAL SECOND CAPTION\n\n";
    fs::write(&sidecar, first_srt).unwrap();
    let original = fs::read(&source).unwrap();

    catalog::index(&state_dir, &library).unwrap();
    auth::create_user(&state_dir, "admin", "srt-overlay-admin-password").unwrap();
    auth::create_user(&state_dir, "viewer", "srt-overlay-viewer-password").unwrap();
    let token = auth::authenticate(
        &state_dir,
        "viewer",
        "srt-overlay-viewer-password",
        "server",
    )
    .unwrap()
    .unwrap()
    .access_token;
    auth::grant_library(&state_dir, "viewer", &library).unwrap();
    let app = app::router(Arc::new(AppState::with_auth_dir(
        "server".to_owned(),
        state_dir.clone(),
    )));
    let items = call(&app, "/Items", Some(&token)).await;
    assert_eq!(items.status(), StatusCode::OK);
    let items: Value = serde_json::from_str(&body_string(items).await).unwrap();
    let id = items["Items"][0]["Id"].as_str().unwrap();
    let response = call(&app, &format!("/Items/{id}/PlaybackInfo"), Some(&token)).await;
    assert_eq!(response.status(), StatusCode::OK);
    let info: Value = serde_json::from_str(&body_string(response).await).unwrap();
    let sidecar_stream = info["MediaSources"][0]["MediaStreams"]
        .as_array()
        .unwrap()
        .iter()
        .find(|stream| stream["IsExternal"] == true && stream["Codec"] == "subrip")
        .unwrap();
    let index = sidecar_stream["Index"].as_i64().unwrap();
    let base = format!("/Videos/{id}/main.m3u8?segmentLength=2");
    let burn = format!("{base}&subtitleStreamIndex={index}&subtitleMethod=Encode");
    assert_eq!(
        call(
            &app,
            &format!("{base}&subtitleStreamIndex={index}"),
            Some(&token)
        )
        .await
        .status(),
        StatusCode::NOT_IMPLEMENTED
    );
    let plain_response = call(&app, &base, Some(&token)).await;
    assert_eq!(plain_response.status(), StatusCode::OK);
    let plain = body_string(plain_response).await;
    let burned_response = call(&app, &burn, Some(&token)).await;
    assert_eq!(burned_response.status(), StatusCode::OK);
    let burned = body_string(burned_response).await;
    assert_ne!(burned, plain);
    let plain_bytes = segment_bytes(&app, id, &plain, &token, 0).await;
    let first_bytes = segment_bytes(&app, id, &burned, &token, 0).await;
    let second_bytes = segment_bytes(&app, id, &burned, &token, 1).await;
    let black = bright_pixels(&plain_bytes, &temp.path().join("external-plain.ts"));
    let first = bright_pixels(&first_bytes, &temp.path().join("external-first.ts"));
    let second = bright_pixels(&second_bytes, &temp.path().join("external-second.ts"));
    assert!(first > black + 20);
    assert!(second > black + 20);
    // FFmpeg subtitle rendering must not silently discard a source audio
    // stream while producing otherwise playable H.264 MPEG-TS transport.
    let audio_probe = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-select_streams",
            "a:0",
            "-show_entries",
            "stream=codec_name",
            "-of",
            "default=noprint_wrappers=1:nokey=1",
        ])
        .arg(temp.path().join("external-first.ts"))
        .output()
        .unwrap();
    assert!(audio_probe.status.success());
    assert!(String::from_utf8_lossy(&audio_probe.stdout).contains("aac"));
    assert_eq!(fs::read(&source).unwrap(), original);

    let first_segment = burned
        .lines()
        .find(|line| line.starts_with("hls1/") && line.ends_with(".ts"))
        .unwrap();
    let segment_url = format!("/Videos/{id}/{first_segment}");
    assert_eq!(
        call(&app, &segment_url, None).await.status(),
        StatusCode::UNAUTHORIZED
    );
    fs::write(
        &sidecar,
        b"1\n00:00:00,200 --> 00:00:01,800\nUPDATED FIRST CAPTION\n\n2\n00:00:02,200 --> 00:00:03,800\nUPDATED SECOND CAPTION\n\n",
    )
    .unwrap();
    assert_eq!(
        call(&app, &segment_url, Some(&token)).await.status(),
        StatusCode::NOT_FOUND
    );
    let updated_response = call(&app, &burn, Some(&token)).await;
    assert_eq!(updated_response.status(), StatusCode::OK);
    let updated = body_string(updated_response).await;
    assert_ne!(updated, burned);
    assert_eq!(fs::read(&source).unwrap(), original);

    // A real UTF-16LE BOM SRT should enter FFmpeg as normalized UTF-8.
    let mut utf16 = vec![0xff, 0xfe];
    let subtitle_text = "1\n00:00:00,200 --> 00:00:01,800\nUTF16 SUBTITLE VISIBLE\n\n";
    for unit in subtitle_text.encode_utf16() {
        utf16.extend_from_slice(&unit.to_le_bytes());
    }
    fs::write(&sidecar, &utf16).unwrap();
    let unicode_response = call(&app, &burn, Some(&token)).await;
    assert_eq!(unicode_response.status(), StatusCode::OK);
    let unicode_manifest = body_string(unicode_response).await;
    assert_ne!(unicode_manifest, updated);
    let unicode_bytes = segment_bytes(&app, id, &unicode_manifest, &token, 0).await;
    let unicode_bright = bright_pixels(&unicode_bytes, &temp.path().join("external-utf16.ts"));
    assert!(unicode_bright > black + 20);
    assert_eq!(fs::read(&sidecar).unwrap(), utf16);
    assert_eq!(fs::read(&source).unwrap(), original);

    // Separately indexed external WebVTT uses the already verified
    // cue-to-SRT conversion, not a second FFmpeg filter syntax.
    let vtt = library.join("Feature.fr.vtt");
    fs::write(
        &vtt,
        b"WEBVTT\n\n00:00:00.200 --> 00:00:01.800\nEXTERNAL WEBVTT VISIBLE\n\n",
    )
    .unwrap();
    let playback_info = call(&app, &format!("/Items/{id}/PlaybackInfo"), Some(&token)).await;
    assert_eq!(playback_info.status(), StatusCode::OK);
    let playback_info: Value = serde_json::from_str(&body_string(playback_info).await).unwrap();
    let vtt_index = playback_info["MediaSources"][0]["MediaStreams"]
        .as_array()
        .unwrap()
        .iter()
        .find(|stream| stream["IsExternal"] == true && stream["Codec"] == "webvtt")
        .unwrap()["Index"]
        .as_i64()
        .unwrap();
    let vtt_url = format!("{base}&subtitleStreamIndex={vtt_index}&subtitleMethod=Encode");
    let vtt_response = call(&app, &vtt_url, Some(&token)).await;
    assert_eq!(vtt_response.status(), StatusCode::OK);
    let vtt_manifest = body_string(vtt_response).await;
    let vtt_bytes = segment_bytes(&app, id, &vtt_manifest, &token, 0).await;
    let vtt_bright = bright_pixels(&vtt_bytes, &temp.path().join("external-vtt.ts"));
    assert!(vtt_bright > black + 20);
    assert_eq!(fs::read(&source).unwrap(), original);

    fs::write(&vtt, b"WEBVTT\n\nSTYLE\n::cue { color: yellow; }\n\n").unwrap();
    assert_eq!(
        call(&app, &vtt_url, Some(&token)).await.status(),
        StatusCode::NOT_IMPLEMENTED
    );

    fs::write(&sidecar, b"this is not a valid SRT cue").unwrap();
    assert_eq!(
        call(&app, &burn, Some(&token)).await.status(),
        StatusCode::NOT_IMPLEMENTED
    );
    assert_eq!(
        call(&app, &segment_url, Some(&token)).await.status(),
        StatusCode::NOT_FOUND
    );
    // Replacement with an escape symlink must disappear from the bounded
    // sidecar inventory, never becoming FFmpeg input outside the library.
    let outside = temp.path().join("outside.srt");
    fs::write(&outside, first_srt).unwrap();
    fs::remove_file(&sidecar).unwrap();
    std::os::unix::fs::symlink(&outside, &sidecar).unwrap();
    assert_eq!(
        call(&app, &burn, Some(&token)).await.status(),
        StatusCode::NOT_IMPLEMENTED
    );
    assert_eq!(
        call(&app, &segment_url, Some(&token)).await.status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(fs::read(&source).unwrap(), original);
    auth::revoke_library(&state_dir, "viewer", &library).unwrap();
    assert_eq!(
        call(&app, &burn, Some(&token)).await.status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        call(&app, &segment_url, Some(&token)).await.status(),
        StatusCode::NOT_FOUND
    );
}
