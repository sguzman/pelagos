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
        .args([
            "-frames:v",
            "1",
            "-vf",
            "crop=32:32:0:0",
            "-pix_fmt",
            "rgb24",
            "-f",
            "rawvideo",
            "-",
        ])
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
                "/Videos/{id}/master.m3u8?segmentLength=2&renditions=5&subtitleStreamIndex={index}&subtitleMethod=Encode"
            ),
            Some(&token)
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
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
    // A normal Jellyfin-compatible VOD publication is never implicitly
    // promoted into the Pelagos-only long-source continuation protocol.
    let legacy_id = urls[0].split('/').nth(1).unwrap();
    let forbidden_next = format!("/Videos/{id}/hls1/{legacy_id}/next.m3u8");
    assert_eq!(
        call(&app, &forbidden_next, Some(&token)).await.status(),
        StatusCode::NOT_FOUND
    );
    // HEAD requests must not accidentally invoke an expensive continuation
    // encoder through axum's automatic GET-to-HEAD fallback.
    let head = Request::builder()
        .method("HEAD")
        .uri(forbidden_next.as_str())
        .header("X-Emby-Token", token.as_str())
        .body(Body::empty())
        .unwrap();
    assert_eq!(
        app.clone().oneshot(head).await.unwrap().status(),
        StatusCode::METHOD_NOT_ALLOWED
    );
    assert_eq!(
        call(&app, &forbidden_next, None).await.status(),
        StatusCode::UNAUTHORIZED
    );
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
async fn adaptive_external_pgs_hls_aligns_cues_across_real_bitrate_renditions() {
    if std::env::var_os("PELAGOS_REQUIRE_FFMPEG").is_none() {
        return;
    }
    let temp = tempfile::tempdir().unwrap();
    let library = temp.path().join("Movies");
    let state_dir = temp.path().join("state");
    fs::create_dir_all(&library).unwrap();
    let source = library.join("Adaptive.mkv");
    let sidecar = library.join("Adaptive.es.sup");
    let created = Command::new("ffmpeg")
        .args([
            "-nostdin",
            "-hide_banner",
            "-loglevel",
            "error",
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=640x360:rate=15:duration=3",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=600:duration=3",
            "-map",
            "0:v:0",
            "-map",
            "1:a:0",
            "-vf",
            "drawbox=x=0:y=0:w=32:h=32:color=black:t=fill",
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
        "{}",
        String::from_utf8_lossy(&created.stderr)
    );
    let original = fs::read(&source).unwrap();
    let sup = synthetic_sup();
    fs::write(&sidecar, &sup).unwrap();
    catalog::index(&state_dir, &library).unwrap();
    auth::create_user(&state_dir, "admin", "pgs-adaptive-admin-password").unwrap();
    let token = auth::authenticate(&state_dir, "admin", "pgs-adaptive-admin-password", "server")
        .unwrap()
        .unwrap()
        .access_token;
    let other = auth::authenticate(&state_dir, "admin", "pgs-adaptive-admin-password", "server")
        .unwrap()
        .unwrap()
        .access_token;
    let app = app::router(Arc::new(AppState::with_auth_dir(
        "server".to_owned(),
        state_dir,
    )));
    let items = call(&app, "/Items", Some(&token)).await;
    let items: Value = serde_json::from_str(&body(items).await).unwrap();
    let id = items["Items"][0]["Id"].as_str().unwrap();
    let details = call(&app, &format!("/Items/{id}/PlaybackInfo"), Some(&token)).await;
    let details: Value = serde_json::from_str(&body(details).await).unwrap();
    let index = details["MediaSources"][0]["MediaStreams"]
        .as_array()
        .unwrap()
        .iter()
        .find(|stream| stream["IsExternal"] == true && stream["Codec"] == "pgssub")
        .unwrap()["Index"]
        .as_i64()
        .unwrap();
    let master_url = format!(
        "/Videos/{id}/master.m3u8?segmentLength=2&renditions=2&subtitleStreamIndex={index}&subtitleMethod=Encode"
    );
    assert_eq!(
        call(&app, &master_url, None).await.status(),
        StatusCode::UNAUTHORIZED
    );
    let response = call(&app, &master_url, Some(&token)).await;
    assert_eq!(response.status(), StatusCode::OK);
    let master = body(response).await;
    assert_eq!(master.matches("#EXT-X-STREAM-INF:BANDWIDTH=").count(), 2);
    assert_eq!(master.matches(",RESOLUTION=640x360").count(), 2);
    assert_eq!(
        body(call(&app, &master_url, Some(&token)).await).await,
        master
    );
    let variants = master
        .lines()
        .filter(|line| line.starts_with("hls1/") && line.ends_with("/index.m3u8"))
        .collect::<Vec<_>>();
    assert_eq!(variants.len(), 2);
    let mut stale_uri = String::new();
    for (variant_index, variant) in variants.iter().enumerate() {
        let response = call(&app, &format!("/Videos/{id}/{variant}"), Some(&token)).await;
        assert_eq!(response.status(), StatusCode::OK);
        let playlist = body(response).await;
        assert_eq!(playlist.matches("#EXTINF:").count(), 4);
        assert_eq!(playlist.matches("#EXT-X-DISCONTINUITY").count(), 3);
        assert!(playlist.ends_with("#EXT-X-ENDLIST\n"));
        for (position, expected) in [false, true, true, false].into_iter().enumerate() {
            let segment = format!(
                "{}{}.ts",
                variant.strip_suffix("index.m3u8").unwrap(),
                position
            );
            let uri = format!("/Videos/{id}/{segment}");
            if variant_index == 0 && position == 1 {
                stale_uri = uri.clone();
            }
            assert_eq!(
                call(&app, &uri, Some(&other)).await.status(),
                StatusCode::NOT_FOUND
            );
            let response = call(&app, &uri, Some(&token)).await;
            assert_eq!(response.status(), StatusCode::OK);
            let bytes = to_bytes(response.into_body(), 8 * 1024 * 1024)
                .await
                .unwrap()
                .to_vec();
            assert!(!bytes.is_empty() && bytes.len() % 188 == 0);
            assert_eq!(
                visible_center(
                    &bytes,
                    &temp
                        .path()
                        .join(format!("adaptive-{variant_index}-{position}.ts"))
                ),
                expected
            );
        }
    }
    assert!(!stale_uri.is_empty());
    assert_eq!(fs::read(&source).unwrap(), original);
    let mut changed = sup;
    let palette = changed
        .windows(7)
        .position(|bytes| bytes == [0, 0, 1, 235, 128, 128, 255])
        .unwrap();
    changed[palette + 6] = 0;
    fs::write(&sidecar, changed).unwrap();
    assert_eq!(
        call(&app, &stale_uri, Some(&token)).await.status(),
        StatusCode::NOT_FOUND
    );
}

#[cfg(unix)]
#[tokio::test]
async fn adaptive_embedded_matroska_pgs_renders_every_variant_without_source_changes() {
    if std::env::var_os("PELAGOS_REQUIRE_FFMPEG").is_none() {
        return;
    }
    let temp = tempfile::tempdir().unwrap();
    let library = temp.path().join("Movies");
    let state_dir = temp.path().join("state");
    fs::create_dir_all(&library).unwrap();
    let raw_sup = temp.path().join("input.sup");
    fs::write(&raw_sup, synthetic_sup()).unwrap();
    let source = library.join("AdaptiveEmbedded.mkv");
    let created = Command::new("ffmpeg")
        .args([
            "-nostdin",
            "-hide_banner",
            "-loglevel",
            "error",
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=640x360:rate=15:duration=3",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=600:duration=3",
            "-i",
        ])
        .arg(&raw_sup)
        .args([
            "-map",
            "0:v:0",
            "-map",
            "1:a:0",
            "-map",
            "2:s:0",
            "-vf",
            "drawbox=x=0:y=0:w=32:h=32:color=black:t=fill",
            "-c:v",
            "mpeg4",
            "-c:a",
            "aac",
            "-c:s",
            "copy",
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
    auth::create_user(&state_dir, "admin", "adaptive-embedded-pgs-admin-password").unwrap();
    let token = auth::authenticate(
        &state_dir,
        "admin",
        "adaptive-embedded-pgs-admin-password",
        "server",
    )
    .unwrap()
    .unwrap()
    .access_token;
    let app = app::router(Arc::new(AppState::with_auth_dir(
        "server".to_owned(),
        state_dir,
    )));
    let items = call(&app, "/Items", Some(&token)).await;
    let items: Value = serde_json::from_str(&body(items).await).unwrap();
    let id = items["Items"][0]["Id"].as_str().unwrap();
    let details = call(&app, &format!("/Items/{id}/PlaybackInfo"), Some(&token)).await;
    let details: Value = serde_json::from_str(&body(details).await).unwrap();
    let stream = details["MediaSources"][0]["MediaStreams"]
        .as_array()
        .unwrap()
        .iter()
        .find(|stream| stream["IsExternal"] == false && stream["Codec"] == "hdmv_pgs_subtitle")
        .expect("embedded PGS track must be advertised");
    let index = stream["Index"].as_i64().unwrap();
    let master_url = format!(
        "/Videos/{id}/master.m3u8?segmentLength=2&renditions=2&subtitleStreamIndex={index}&subtitleMethod=Encode"
    );
    let response = call(&app, &master_url, Some(&token)).await;
    assert_eq!(response.status(), StatusCode::OK);
    let master = body(response).await;
    assert_eq!(master.matches("#EXT-X-STREAM-INF:BANDWIDTH=").count(), 2);
    let variants = master
        .lines()
        .filter(|line| line.starts_with("hls1/") && line.ends_with("/index.m3u8"))
        .collect::<Vec<_>>();
    assert_eq!(variants.len(), 2);
    for (position, variant) in variants.iter().enumerate() {
        let response = call(&app, &format!("/Videos/{id}/{variant}"), Some(&token)).await;
        assert_eq!(response.status(), StatusCode::OK);
        let playlist = body(response).await;
        assert_eq!(playlist.matches("#EXTINF:").count(), 4);
        assert_eq!(playlist.matches("#EXT-X-DISCONTINUITY").count(), 3);
        for (index, expected) in [(1, true), (3, false)] {
            let prefix = variant.strip_suffix("index.m3u8").unwrap();
            let response = call(
                &app,
                &format!("/Videos/{id}/{prefix}{index}.ts"),
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
                    &temp
                        .path()
                        .join(format!("embedded-adaptive-{position}-{index}.ts"))
                ),
                expected
            );
        }
    }
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

#[cfg(unix)]
#[tokio::test]
async fn private_long_source_pgs_excerpt_http_reauthorizes_segments_and_separates_cache_keys() {
    if std::env::var_os("PELAGOS_REQUIRE_FFMPEG").is_none() {
        return;
    }
    let temp = tempfile::tempdir().unwrap();
    let library = temp.path().join("Movies");
    let state_dir = temp.path().join("state");
    fs::create_dir_all(&library).unwrap();
    let source = library.join("LongFeature.mkv");
    let sidecar = library.join("LongFeature.es.sup");
    let fixture = Command::new("ffmpeg")
        .args([
            "-nostdin",
            "-hide_banner",
            "-loglevel",
            "error",
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=160x90:rate=5:duration=610",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=500:duration=610",
            "-map",
            "0:v:0",
            "-map",
            "1:a:0",
            "-vf",
            "drawbox=x=0:y=0:w=32:h=32:color=black:t=fill",
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
    let mut sup = synthetic_sup();
    let mut pos = 0;
    while pos < sup.len() {
        let length = usize::from(u16::from_be_bytes([sup[pos + 11], sup[pos + 12]]));
        let pts = u32::from_be_bytes(sup[pos + 2..pos + 6].try_into().unwrap());
        sup[pos + 2..pos + 6].copy_from_slice(&(pts + 605 * 90_000).to_be_bytes());
        pos += 13 + length;
    }
    fs::write(&sidecar, &sup).unwrap();
    catalog::index(&state_dir, &library).unwrap();
    auth::create_user(&state_dir, "admin", "private-excerpt-admin-password").unwrap();
    auth::create_user(&state_dir, "viewer", "private-excerpt-viewer-password").unwrap();
    auth::grant_library(&state_dir, "viewer", &library).unwrap();
    let token = auth::authenticate(
        &state_dir,
        "viewer",
        "private-excerpt-viewer-password",
        "server",
    )
    .unwrap()
    .unwrap()
    .access_token;
    let other = auth::authenticate(
        &state_dir,
        "viewer",
        "private-excerpt-viewer-password",
        "server",
    )
    .unwrap()
    .unwrap()
    .access_token;
    let app = app::router(Arc::new(AppState::with_auth_dir(
        "server".to_owned(),
        state_dir.clone(),
    )));
    let items = call(&app, "/Items", Some(&token)).await;
    assert_eq!(items.status(), StatusCode::OK);
    let items: Value = serde_json::from_str(&body(items).await).unwrap();
    let id = items["Items"][0]["Id"].as_str().unwrap();
    let info = call(&app, &format!("/Items/{id}/PlaybackInfo"), Some(&token)).await;
    assert_eq!(info.status(), StatusCode::OK);
    let info: Value = serde_json::from_str(&body(info).await).unwrap();
    let index = info["MediaSources"][0]["MediaStreams"]
        .as_array()
        .unwrap()
        .iter()
        .find(|stream| stream["IsExternal"] == true && stream["Codec"] == "pgssub")
        .unwrap()["Index"]
        .as_i64()
        .unwrap();

    let base = format!("/Videos/{id}/main.m3u8?segmentLength=10&startTimeTicks=6050000000");
    let selected = format!("{base}&subtitleStreamIndex={index}&subtitleMethod=Encode");
    // Without an explicitly bounded excerpt the existing client-facing
    // VOD route must not publish a truncated 610-second source.
    let legacy = format!(
        "/Videos/{id}/main.m3u8?segmentLength=10&subtitleStreamIndex={index}&subtitleMethod=Encode"
    );
    assert_eq!(
        call(&app, &legacy, Some(&token)).await.status(),
        StatusCode::NOT_IMPLEMENTED
    );
    let excerpt = format!("{selected}&pelagosExcerptDurationTicks=40000000");
    assert_eq!(
        call(&app, &excerpt, None).await.status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        call(&app, &selected, Some(&token)).await.status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        call(
            &app,
            &format!("{base}&pelagosExcerptDurationTicks=40000000"),
            Some(&token),
        )
        .await
        .status(),
        StatusCode::NOT_IMPLEMENTED
    );
    for bad in ["0", "-1", "6000000001", "9223372036854775807"] {
        let uri = format!("{selected}&pelagosExcerptDurationTicks={bad}");
        assert_eq!(
            call(&app, &uri, Some(&token)).await.status(),
            StatusCode::BAD_REQUEST
        );
    }
    assert_eq!(
        call(
            &app,
            &format!("{selected}&pelagosExcerptDurationTicks=60000000"),
            Some(&token),
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );
    let response = call(&app, &excerpt, Some(&token)).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get("x-pelagos-excerpt")
            .unwrap()
            .to_str()
            .unwrap(),
        "true"
    );
    assert_eq!(
        response.headers()["x-pelagos-excerpt-start-ticks"],
        "6050000000"
    );
    assert_eq!(
        response.headers()["x-pelagos-excerpt-duration-ticks"],
        "40000000"
    );
    let advertised_next = response.headers()["x-pelagos-next-path"]
        .to_str()
        .unwrap()
        .to_owned();
    let playlist = body(response).await;
    assert!(playlist.contains("#EXT-X-MEDIA-SEQUENCE:0\n"));
    let urls = playlist
        .lines()
        .filter(|line| line.starts_with("hls1/") && line.ends_with(".ts"))
        .collect::<Vec<_>>();
    assert_eq!(urls.len(), 3);
    assert_eq!(playlist.matches("#EXT-X-DISCONTINUITY").count(), 2);
    let cached = call(&app, &excerpt, Some(&token)).await;
    assert_eq!(cached.status(), StatusCode::OK);
    assert_eq!(cached.headers()["x-pelagos-excerpt"], "true");
    assert_eq!(
        cached.headers()["x-pelagos-excerpt-start-ticks"],
        "6050000000"
    );
    assert_eq!(
        cached.headers()["x-pelagos-excerpt-duration-ticks"],
        "40000000"
    );
    assert_eq!(body(cached).await, playlist);

    for (position, expected_visible) in [false, true, false].into_iter().enumerate() {
        let uri = format!("/Videos/{id}/{}", urls[position]);
        assert_eq!(
            call(&app, &uri, Some(&other)).await.status(),
            StatusCode::NOT_FOUND
        );
        let response = call(&app, &uri, Some(&token)).await;
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = to_bytes(response.into_body(), 8 * 1024 * 1024)
            .await
            .unwrap()
            .to_vec();
        assert!(bytes.chunks_exact(188).all(|chunk| chunk[0] == 0x47));
        assert_eq!(
            visible_center(&bytes, &temp.path().join(format!("excerpt-{position}.ts"))),
            expected_visible
        );
    }

    // Different explicit intervals cannot alias a cached 4-second excerpt.
    let shorter = format!("{selected}&pelagosExcerptDurationTicks=30000000");
    let response = call(&app, &shorter, Some(&token)).await;
    assert_eq!(response.status(), StatusCode::OK);
    let shorter_playlist = body(response).await;
    assert!(shorter_playlist.contains("#EXT-X-MEDIA-SEQUENCE:0\n"));
    assert_ne!(shorter_playlist, playlist);

    // Explicit adaptive excerpts share a cue timeline but publish separate,
    // token-scoped quality variants. The old full-source path remains closed.
    let master_url = format!(
        "/Videos/{id}/master.m3u8?segmentLength=10&startTimeTicks=6050000000&renditions=2&subtitleStreamIndex={index}&subtitleMethod=Encode&pelagosExcerptDurationTicks=40000000"
    );
    assert_eq!(
        call(&app, &master_url, None).await.status(),
        StatusCode::UNAUTHORIZED
    );
    let result = call(&app, &master_url, Some(&token)).await;
    assert_eq!(result.status(), StatusCode::OK);
    assert_eq!(result.headers()["x-pelagos-excerpt"], "true");
    assert_eq!(
        result.headers()["x-pelagos-excerpt-start-ticks"],
        "6050000000"
    );
    assert_eq!(
        result.headers()["x-pelagos-excerpt-duration-ticks"],
        "40000000"
    );
    let adaptive_advertised_next = result.headers()["x-pelagos-next-path"]
        .to_str()
        .unwrap()
        .to_owned();
    let master = body(result).await;
    let variants = master
        .lines()
        .filter(|line| line.starts_with("hls1/") && line.ends_with("/index.m3u8"))
        .collect::<Vec<_>>();
    assert_eq!(variants.len(), 2);
    assert_eq!(master.matches("#EXT-X-STREAM-INF:BANDWIDTH=").count(), 2);
    let reused = call(&app, &master_url, Some(&token)).await;
    assert_eq!(reused.status(), StatusCode::OK);
    assert_eq!(reused.headers()["x-pelagos-excerpt"], "true");
    assert_eq!(
        reused.headers()["x-pelagos-excerpt-start-ticks"],
        "6050000000"
    );
    assert_eq!(
        reused.headers()["x-pelagos-excerpt-duration-ticks"],
        "40000000"
    );
    assert_eq!(body(reused).await, master);
    for (variant_index, path) in variants.iter().enumerate() {
        let response = call(&app, &format!("/Videos/{id}/{path}"), Some(&token)).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()["x-pelagos-excerpt"], "true");
        assert_eq!(
            response.headers()["x-pelagos-excerpt-start-ticks"],
            "6050000000"
        );
        assert_eq!(
            response.headers()["x-pelagos-excerpt-duration-ticks"],
            "40000000"
        );
        let manifest = body(response).await;
        assert!(manifest.contains("#EXT-X-MEDIA-SEQUENCE:0\n"));
        assert_eq!(manifest.matches("#EXTINF:").count(), 3);
        assert_eq!(manifest.matches("#EXT-X-DISCONTINUITY").count(), 2);
        let prefix = path.strip_suffix("index.m3u8").unwrap();
        for (position, expected_visible) in [false, true, false].into_iter().enumerate() {
            let segment_url = format!("/Videos/{id}/{prefix}{position}.ts");
            assert_eq!(
                call(&app, &segment_url, Some(&other)).await.status(),
                StatusCode::NOT_FOUND
            );
            let response = call(&app, &segment_url, Some(&token)).await;
            assert_eq!(response.status(), StatusCode::OK);
            let bytes = to_bytes(response.into_body(), 8 * 1024 * 1024)
                .await
                .unwrap()
                .to_vec();
            assert!(bytes.chunks_exact(188).all(|packet| packet[0] == 0x47));
            assert_eq!(
                visible_center(
                    &bytes,
                    &temp
                        .path()
                        .join(format!("variant-{variant_index}-{position}.ts"))
                ),
                expected_visible
            );
        }
    }

    // The private continuation route is only available to the exact token
    // that owns the selected excerpt. It must clamp the final interval to
    // the real source end rather than inventing a 600-second full playlist.
    let publication_id = urls[0].split('/').nth(1).unwrap();
    let next_url = format!("/Videos/{id}/hls1/{publication_id}/next.m3u8");
    assert_eq!(advertised_next, next_url);
    assert_eq!(
        call(&app, &next_url, None).await.status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        call(&app, &next_url, Some(&other)).await.status(),
        StatusCode::NOT_FOUND
    );
    let response = call(&app, &next_url, Some(&token)).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["x-pelagos-excerpt"], "true");
    assert_eq!(
        response.headers()["x-pelagos-excerpt-start-ticks"],
        "6090000000"
    );
    let final_duration = response.headers()["x-pelagos-excerpt-duration-ticks"]
        .to_str()
        .unwrap()
        .parse::<u64>()
        .unwrap();
    assert!((10_000_000..=11_000_000).contains(&final_duration));
    let final_manifest = body(response).await;
    assert!(final_manifest.contains("#EXT-X-MEDIA-SEQUENCE:3\n"));
    assert!(final_manifest.contains("#EXT-X-DISCONTINUITY-SEQUENCE:3\n"));
    let final_segments = final_manifest
        .lines()
        .filter(|line| line.starts_with("hls1/") && line.ends_with(".ts"))
        .collect::<Vec<_>>();
    assert_eq!(final_segments.len(), 1);
    let final_segment_uri = format!("/Videos/{id}/{}", final_segments[0]);
    let response = call(&app, &final_segment_uri, Some(&token)).await;
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = to_bytes(response.into_body(), 8 * 1024 * 1024)
        .await
        .unwrap()
        .to_vec();
    assert!(!visible_center(
        &bytes,
        &temp.path().join("continuation-clear.ts")
    ));

    // A continued private excerpt uses the same authenticated range/HEAD
    // delivery path as the original publication, not an unguarded file URL.
    let ranged = Request::builder()
        .uri(final_segment_uri.as_str())
        .header("X-Emby-Token", token.as_str())
        .header("Range", "bytes=0-187")
        .body(Body::empty())
        .unwrap();
    let response = app.clone().oneshot(ranged).await.unwrap();
    assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(response.headers()["Content-Length"], "188");
    assert!(
        response.headers()["Content-Range"]
            .to_str()
            .unwrap()
            .starts_with("bytes 0-187/")
    );
    let first_packet = to_bytes(response.into_body(), 188).await.unwrap();
    assert_eq!(first_packet.len(), 188);
    assert_eq!(first_packet[0], 0x47);
    let head = Request::builder()
        .method("HEAD")
        .uri(final_segment_uri.as_str())
        .header("X-Emby-Token", token.as_str())
        .body(Body::empty())
        .unwrap();
    let response = app.clone().oneshot(head).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(to_bytes(response.into_body(), 16).await.unwrap().len(), 0);

    let final_id = final_segments[0].split('/').nth(1).unwrap();
    let stitch_url = format!("/Videos/{id}/hls1/{publication_id}/stitch.m3u8?next={final_id}");
    assert_eq!(
        call(&app, &stitch_url, None).await.status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        call(&app, &stitch_url, Some(&other)).await.status(),
        StatusCode::NOT_FOUND
    );
    let stitched = call(&app, &stitch_url, Some(&token)).await;
    assert_eq!(stitched.status(), StatusCode::OK);
    assert_eq!(stitched.headers()["x-pelagos-stitched"], "true");
    assert_eq!(
        stitched.headers()["x-pelagos-excerpt-start-ticks"],
        "6050000000"
    );
    assert_eq!(
        stitched.headers()["x-pelagos-excerpt-duration-ticks"],
        "50000000"
    );
    let stitched_playlist = body(stitched).await;
    assert!(stitched_playlist.contains("#EXT-X-MEDIA-SEQUENCE:0\n"));
    assert!(stitched_playlist.contains("#EXT-X-DISCONTINUITY-SEQUENCE:0\n"));
    assert_eq!(stitched_playlist.matches("#EXTINF:").count(), 4);
    assert_eq!(
        stitched_playlist.matches("#EXT-X-DISCONTINUITY\n").count(),
        3
    );
    assert!(stitched_playlist.ends_with("#EXT-X-ENDLIST\n"));
    assert!(stitched_playlist.contains(&format!("/Videos/{id}/hls1/{publication_id}/0.ts")));
    assert!(stitched_playlist.contains(&format!("/Videos/{id}/hls1/{final_id}/0.ts")));
    let reversed = format!("/Videos/{id}/hls1/{final_id}/stitch.m3u8?next={publication_id}");
    assert_eq!(
        call(&app, &reversed, Some(&token)).await.status(),
        StatusCode::NOT_FOUND
    );

    let exhausted = format!("/Videos/{id}/hls1/{final_id}/next.m3u8");
    assert_eq!(
        call(&app, &exhausted, Some(&token)).await.status(),
        StatusCode::NO_CONTENT
    );

    // The selected two-quality publication must also continue with two
    // independently encoded variants and the same exact source interval.
    let adaptive_id = variants[0].split('/').nth(1).unwrap();
    let adaptive_next = format!("/Videos/{id}/hls1/{adaptive_id}/next.m3u8");
    assert_eq!(adaptive_advertised_next, adaptive_next);
    assert_eq!(
        call(&app, &adaptive_next, Some(&other)).await.status(),
        StatusCode::NOT_FOUND
    );
    let response = call(&app, &adaptive_next, Some(&token)).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["x-pelagos-excerpt"], "true");
    assert_eq!(
        response.headers()["x-pelagos-excerpt-start-ticks"],
        "6090000000"
    );
    let adaptive_final = body(response).await;
    let final_variants = adaptive_final
        .lines()
        .filter(|line| line.starts_with("hls1/") && line.ends_with("/index.m3u8"))
        .collect::<Vec<_>>();
    assert_eq!(final_variants.len(), 2);
    for (variant_index, path) in final_variants.iter().enumerate() {
        let uri = format!("/Videos/{id}/{path}");
        let response = call(&app, &uri, Some(&token)).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers()["x-pelagos-excerpt-start-ticks"],
            "6090000000"
        );
        let media_playlist = body(response).await;
        assert!(media_playlist.contains("#EXT-X-MEDIA-SEQUENCE:3\n"));
        assert!(media_playlist.contains("#EXT-X-DISCONTINUITY-SEQUENCE:3\n"));
        assert_eq!(media_playlist.matches("#EXTINF:").count(), 1);
        let prefix = path.strip_suffix("index.m3u8").unwrap();
        let segment_url = format!("/Videos/{id}/{prefix}0.ts");
        assert_eq!(
            call(&app, &segment_url, Some(&other)).await.status(),
            StatusCode::NOT_FOUND
        );
        let response = call(&app, &segment_url, Some(&token)).await;
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = to_bytes(response.into_body(), 8 * 1024 * 1024)
            .await
            .unwrap()
            .to_vec();
        assert!(!visible_center(
            &bytes,
            &temp
                .path()
                .join(format!("adaptive-next-{variant_index}.ts"))
        ));
    }
    let final_adaptive_id = final_variants[0].split('/').nth(1).unwrap();
    let adaptive_exhausted = format!("/Videos/{id}/hls1/{final_adaptive_id}/next.m3u8");
    assert_eq!(
        call(&app, &adaptive_exhausted, Some(&token)).await.status(),
        StatusCode::NO_CONTENT
    );

    // A token-scoped continuation cannot outlive the viewer's library grant,
    // including at EOF where no new FFmpeg job is needed.
    auth::revoke_library(&state_dir, "viewer", &library).unwrap();
    assert_eq!(
        call(&app, &stitch_url, Some(&token)).await.status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        call(&app, &adaptive_exhausted, Some(&token)).await.status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        call(
            &app,
            &format!("/Videos/{id}/{}", final_variants[0]),
            Some(&token)
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
    auth::grant_library(&state_dir, "viewer", &library).unwrap();
    assert_eq!(
        call(&app, &adaptive_exhausted, Some(&token)).await.status(),
        StatusCode::NO_CONTENT
    );

    let first_uri = final_segment_uri;
    let mut modified = sup.clone();
    let index = modified
        .windows(7)
        .position(|bytes| bytes == [0, 0, 1, 235, 128, 128, 255])
        .unwrap();
    modified[index + 6] = 0;
    fs::write(&sidecar, modified).unwrap();
    assert_eq!(
        call(&app, &stitch_url, Some(&token)).await.status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        call(&app, &first_uri, Some(&token)).await.status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        call(&app, &adaptive_exhausted, Some(&token)).await.status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(fs::read(&source).unwrap(), original);
    // Even if a sidecar is restored, replacing the underlying media must
    // invalidate the original continuation UUID, including at EOF.
    fs::write(&sidecar, &sup).unwrap();
    assert_eq!(
        call(&app, &adaptive_exhausted, Some(&token)).await.status(),
        StatusCode::NO_CONTENT
    );
    let mut replacement = original;
    replacement.extend_from_slice(b"synthetic changed source revision");
    fs::write(&source, replacement).unwrap();
    assert_eq!(
        call(&app, &adaptive_exhausted, Some(&token)).await.status(),
        StatusCode::NOT_FOUND
    );
}
