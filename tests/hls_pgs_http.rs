//! Authenticated opt-in graphical PGS burn-in through private HLS VOD.
//! Synthetic FFmpeg media only; unchanged Smart TV parity is not asserted.
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

async fn call(app: &Router, uri: &str, token: Option<&str>) -> Response {
    let mut request = Request::builder().uri(uri);
    if let Some(token) = token {
        request = request.header("X-Emby-Token", token);
    }
    app.clone()
        .oneshot(request.body(Body::empty()).unwrap())
        .await
        .unwrap()
}

async fn body(response: Response) -> String {
    String::from_utf8(
        to_bytes(response.into_body(), 64 * 1024)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap()
}

fn packet(kind: u8, pts: u32, contents: &[u8]) -> Vec<u8> {
    let mut bytes = b"PG".to_vec();
    bytes.extend_from_slice(&pts.to_be_bytes());
    bytes.extend_from_slice(&[0; 4]);
    bytes.push(kind);
    bytes.extend_from_slice(&(contents.len() as u16).to_be_bytes());
    bytes.extend_from_slice(contents);
    bytes
}

fn synthetic_sup() -> Vec<u8> {
    let mut result = packet(0x16, 0, &[0, 32, 0, 32, 0x10, 0, 0, 0x80, 0, 0, 0]);
    result.extend(packet(0x80, 0, &[]));
    result.extend(packet(
        0x16,
        90_000,
        &[0, 32, 0, 32, 0x10, 0, 1, 0, 0, 0, 1, 0, 1, 0, 0, 0, 8, 0, 8],
    ));
    result.extend(packet(0x17, 90_000, &[1, 0, 0, 0, 0, 0, 0, 32, 0, 32]));
    result.extend(packet(0x14, 90_000, &[0, 0, 1, 235, 128, 128, 255]));
    let mut object = vec![0, 1, 0, 0xc0, 0, 0, 84, 0, 16, 0, 16];
    for _ in 0..16 {
        object.extend_from_slice(&[0, 0x90, 1, 0, 0]);
    }
    result.extend(packet(0x15, 90_000, &object));
    result.extend(packet(0x80, 90_000, &[]));
    result.extend(packet(
        0x16,
        225_000,
        &[0, 32, 0, 32, 0x10, 0, 2, 0, 0, 0, 0],
    ));
    result.extend(packet(0x80, 225_000, &[]));
    result
}

fn visible_center(media: &[u8], path: &std::path::Path) -> bool {
    fs::write(path, media).unwrap();
    let decoded = Command::new("ffmpeg")
        .args(["-nostdin", "-hide_banner", "-loglevel", "error", "-i"])
        .arg(path)
        .args(["-frames:v", "1", "-pix_fmt", "rgb24", "-f", "rawvideo", "-"])
        .output()
        .unwrap();
    assert!(
        decoded.status.success(),
        "{}",
        String::from_utf8_lossy(&decoded.stderr)
    );
    assert_eq!(decoded.stdout.len(), 32 * 32 * 3);
    let center = &decoded.stdout[(16 * 32 + 16) * 3..(16 * 32 + 16) * 3 + 3];
    assert!(
        center.iter().all(|channel| *channel < 65) || center.iter().all(|channel| *channel > 170),
        "ambiguous synthetic PGS pixel: {center:?}"
    );
    center.iter().all(|channel| *channel > 170)
}

#[cfg(unix)]
#[tokio::test]
async fn external_pgs_opt_in_hls_is_authorized_timed_and_rechecks_sidecar_revision() {
    if std::env::var_os("PELAGOS_REQUIRE_FFMPEG").is_none() {
        return;
    }
    let temp = tempfile::tempdir().unwrap();
    let library = temp.path().join("Movies");
    let state_dir = temp.path().join("state");
    fs::create_dir_all(&library).unwrap();
    let source = library.join("Feature.mkv");
    let sidecar = library.join("Feature.es.sup");
    let fixture = Command::new("ffmpeg")
        .args([
            "-nostdin",
            "-hide_banner",
            "-loglevel",
            "error",
            "-f",
            "lavfi",
            "-i",
            "color=c=black:s=32x32:r=5:d=3",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=600:duration=3",
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
        fixture.status.success(),
        "{}",
        String::from_utf8_lossy(&fixture.stderr)
    );
    let original = fs::read(&source).unwrap();
    let sup = synthetic_sup();
    fs::write(&sidecar, &sup).unwrap();
    catalog::index(&state_dir, &library).unwrap();
    auth::create_user(&state_dir, "admin", "pgs-test-admin-password").unwrap();
    auth::create_user(&state_dir, "viewer", "pgs-test-viewer-password").unwrap();
    auth::grant_library(&state_dir, "viewer", &library).unwrap();
    let token = auth::authenticate(&state_dir, "viewer", "pgs-test-viewer-password", "server")
        .unwrap()
        .unwrap()
        .access_token;
    let app = app::router(Arc::new(AppState::with_auth_dir(
        "server".to_owned(),
        state_dir,
    )));
    let response = call(&app, "/Items", Some(&token)).await;
    assert_eq!(response.status(), StatusCode::OK);
    let items: Value = serde_json::from_str(&body(response).await).unwrap();
    let id = items["Items"][0]["Id"].as_str().unwrap();
    let response = call(&app, &format!("/Items/{id}/PlaybackInfo"), Some(&token)).await;
    assert_eq!(response.status(), StatusCode::OK);
    let info: Value = serde_json::from_str(&body(response).await).unwrap();
    let subtitle = info["MediaSources"][0]["MediaStreams"]
        .as_array()
        .unwrap()
        .iter()
        .find(|stream| stream["IsExternal"] == true && stream["Codec"] == "pgssub")
        .expect("raw SUP track advertised by PlaybackInfo");
    let index = subtitle["Index"].as_i64().unwrap();
    let base = format!("/Videos/{id}/main.m3u8?segmentLength=2");
    let selected = format!("{base}&subtitleStreamIndex={index}&subtitleMethod=Encode");
    assert_eq!(
        call(&app, &selected, None).await.status(),
        StatusCode::UNAUTHORIZED
    );
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
    assert_eq!(
        call(
            &app,
            &format!(
                "/Videos/{id}/master.m3u8?segmentLength=2&subtitleStreamIndex={index}&subtitleMethod=Encode&enableAdaptiveBitrateStreaming=true"
            ),
            Some(&token)
        )
        .await
        .status(),
        StatusCode::NOT_IMPLEMENTED
    );
    let result = call(&app, &selected, Some(&token)).await;
    assert_eq!(result.status(), StatusCode::OK);
    let manifest = body(result).await;
    let urls = manifest
        .lines()
        .filter(|line| line.starts_with("hls1/") && line.ends_with(".ts"))
        .collect::<Vec<_>>();
    assert_eq!(urls.len(), 4);
    assert_eq!(manifest.matches("#EXT-X-DISCONTINUITY").count(), 3);
    for (index, expected) in [false, true, true, false].into_iter().enumerate() {
        let uri = format!("/Videos/{id}/{}", urls[index]);
        let response = call(&app, &uri, Some(&token)).await;
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = to_bytes(response.into_body(), 8 * 1024 * 1024)
            .await
            .unwrap()
            .to_vec();
        assert!(bytes.chunks_exact(188).all(|chunk| chunk[0] == 0x47));
        assert_eq!(
            visible_center(&bytes, &temp.path().join(format!("segment-{index}.ts"))),
            expected
        );
    }
    // A seeked PGS session starts in the visible composition and must
    // still split at the 2.5-second clear. Its cache identity must not
    // alias the previous playback session starting at timestamp zero.
    let seeked_url = format!("{selected}&startTimeTicks=10000000");
    let seeked_response = call(&app, &seeked_url, Some(&token)).await;
    assert_eq!(seeked_response.status(), StatusCode::OK);
    let seeked = body(seeked_response).await;
    assert_ne!(seeked, manifest);
    let seeked_segments = seeked
        .lines()
        .filter(|line| line.starts_with("hls1/") && line.ends_with(".ts"))
        .collect::<Vec<_>>();
    assert_eq!(seeked_segments.len(), 2);
    for (position, expected) in [true, false].into_iter().enumerate() {
        let response = call(
            &app,
            &format!("/Videos/{id}/{}", seeked_segments[position]),
            Some(&token),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = to_bytes(response.into_body(), 8 * 1024 * 1024)
            .await
            .unwrap()
            .to_vec();
        assert_eq!(
            visible_center(
                &bytes,
                &temp.path().join(format!("seeked-pgs-{position}.ts"))
            ),
            expected
        );
    }
    let first_uri = format!("/Videos/{id}/{}", urls[0]);
    assert_eq!(
        call(&app, &first_uri, None).await.status(),
        StatusCode::UNAUTHORIZED
    );
    let mut modified = sup;
    let palette = modified
        .windows(7)
        .position(|bytes| bytes == [0, 0, 1, 235, 128, 128, 255])
        .unwrap();
    modified[palette + 6] = 0; // explicit transparent palette color
    fs::write(&sidecar, &modified).unwrap();
    assert_eq!(
        call(&app, &first_uri, Some(&token)).await.status(),
        StatusCode::NOT_FOUND
    );
    // A malformed replacement must not produce a new graphical playlist,
    // and must not resurrect the stale completed segments.
    modified.pop();
    fs::write(&sidecar, &modified).unwrap();
    assert_eq!(
        call(&app, &selected, Some(&token)).await.status(),
        StatusCode::NOT_IMPLEMENTED
    );
    assert_eq!(
        call(&app, &first_uri, Some(&token)).await.status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(fs::read(&source).unwrap(), original);
}

#[cfg(unix)]
#[tokio::test]
async fn embedded_matroska_pgs_is_burned_through_authenticated_hls() {
    if std::env::var_os("PELAGOS_REQUIRE_FFMPEG").is_none() {
        return;
    }
    let temp = tempfile::tempdir().unwrap();
    let library = temp.path().join("Movies");
    let state_dir = temp.path().join("state");
    fs::create_dir_all(&library).unwrap();
    let raw_sup = temp.path().join("raw-embedded.sup");
    fs::write(&raw_sup, synthetic_sup()).unwrap();
    let source = library.join("Embedded.mkv");
    let created = Command::new("ffmpeg")
        .args([
            "-nostdin",
            "-hide_banner",
            "-loglevel",
            "error",
            "-f",
            "lavfi",
            "-i",
            "color=c=black:s=32x32:r=5:d=3",
            "-i",
        ])
        .arg(&raw_sup)
        .args([
            "-map", "0:v:0", "-map", "1:s:0", "-c:v", "mpeg4", "-c:s", "copy", "-y",
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
    auth::create_user(&state_dir, "admin", "embedded-hls-admin-password").unwrap();
    auth::create_user(&state_dir, "viewer", "embedded-hls-viewer-password").unwrap();
    auth::grant_library(&state_dir, "viewer", &library).unwrap();
    let token = auth::authenticate(
        &state_dir,
        "viewer",
        "embedded-hls-viewer-password",
        "server",
    )
    .unwrap()
    .unwrap()
    .access_token;
    let app = app::router(Arc::new(AppState::with_auth_dir(
        "server".to_owned(),
        state_dir.clone(),
    )));
    let response = call(&app, "/Items", Some(&token)).await;
    assert_eq!(response.status(), StatusCode::OK);
    let items: Value = serde_json::from_str(&body(response).await).unwrap();
    let id = items["Items"][0]["Id"].as_str().unwrap();
    let response = call(&app, &format!("/Items/{id}/PlaybackInfo"), Some(&token)).await;
    assert_eq!(response.status(), StatusCode::OK);
    let metadata: Value = serde_json::from_str(&body(response).await).unwrap();
    let stream = metadata["MediaSources"][0]["MediaStreams"]
        .as_array()
        .unwrap()
        .iter()
        .find(|stream| stream["IsExternal"] == false && stream["Codec"] == "hdmv_pgs_subtitle")
        .expect("embedded graphical subtitle index");
    let index = stream["Index"].as_i64().unwrap();
    let url = format!(
        "/Videos/{id}/main.m3u8?segmentLength=2&subtitleStreamIndex={index}&subtitleMethod=Encode"
    );
    assert_eq!(
        call(&app, &url, None).await.status(),
        StatusCode::UNAUTHORIZED
    );
    let response = call(&app, &url, Some(&token)).await;
    assert_eq!(response.status(), StatusCode::OK);
    let playlist = body(response).await;
    let segments = playlist
        .lines()
        .filter(|line| line.starts_with("hls1/") && line.ends_with(".ts"))
        .collect::<Vec<_>>();
    assert_eq!(segments.len(), 4);
    for (position, expected_visible) in [false, true, true, false].into_iter().enumerate() {
        let response = call(
            &app,
            &format!("/Videos/{id}/{}", segments[position]),
            Some(&token),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = to_bytes(response.into_body(), 8 * 1024 * 1024)
            .await
            .unwrap()
            .to_vec();
        assert_eq!(
            visible_center(&bytes, &temp.path().join(format!("embedded-{position}.ts"))),
            expected_visible
        );
    }
    assert_eq!(fs::read(&source).unwrap(), original);

    // Publication ownership does not override current library grants.
    // Revocation must deny already-encoded private TS segments.
    let segment_url = format!("/Videos/{id}/{}", segments[1]);
    assert_eq!(
        call(&app, &segment_url, None).await.status(),
        StatusCode::UNAUTHORIZED
    );
    auth::revoke_library(&state_dir, "viewer", &library).unwrap();
    assert_eq!(
        call(&app, &segment_url, Some(&token)).await.status(),
        StatusCode::NOT_FOUND
    );
    auth::grant_library(&state_dir, "viewer", &library).unwrap();
    assert_eq!(
        call(&app, &segment_url, Some(&token)).await.status(),
        StatusCode::OK
    );

    // An embedded PGS track is part of the source itself: modifying the
    // source invalidates every previously published encoded segment.
    let mut modified = original.clone();
    modified.extend_from_slice(b"synthetic source revision");
    fs::write(&source, &modified).unwrap();
    assert_eq!(
        call(&app, &segment_url, Some(&token)).await.status(),
        StatusCode::NOT_FOUND
    );
}
