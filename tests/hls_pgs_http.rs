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
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::{fs, process::Command, sync::Arc};
use tower::ServiceExt;

#[path = "hls_pgs_http/demux.rs"]
mod demux;

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
    // A real HTTP demuxer must follow the token-scoped adaptive master into
    // an authorized child rendition and decode both video and AAC. Segment
    // parsing alone cannot verify that its nested URLs are usable.
    demux::assert_authenticated_hls_network_demux(&app, &token, &master_url, false, false).await;
    demux::assert_authenticated_hls_network_demux(&app, &token, &master_url, false, true).await;
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
    // A gated ffprobe wrapper permits deterministic revocation after the
    // source probe begins, without changing the real ffprobe's output.
    let probe_gate = temp.path().join("hold-ffprobe");
    let probe_entered = temp.path().join("ffprobe-entered");
    let probe_release = temp.path().join("resume-ffprobe");
    let probe_wrapper = temp.path().join("ffprobe-gated.sh");
    fs::write(
        &probe_wrapper,
        format!(
            "#!/bin/sh\nif [ -e '{}' ]; then\n  : > '{}'\n  attempt=0\n  while [ ! -e '{}' ] && [ \"$attempt\" -lt 200 ]; do\n    sleep 0.05\n    attempt=$((attempt + 1))\n  done\nfi\nexec ffprobe \"$@\"\n",
            probe_gate.display(),
            probe_entered.display(),
            probe_release.display()
        ),
    )
    .unwrap();
    fs::set_permissions(&probe_wrapper, fs::Permissions::from_mode(0o700)).unwrap();
    let app = app::router(Arc::new(
        AppState::with_auth_dir("server".to_owned(), state_dir.clone())
            .with_probe_executable(probe_wrapper),
    ));
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
    // Both sessions are still live: a same-source, same-token excerpt with
    // the wrong start position must not be accepted as a continuation.
    let original_id = urls[0].split('/').nth(1).unwrap();
    let shorter_id = shorter_playlist
        .lines()
        .find(|line| line.starts_with("hls1/") && line.ends_with(".ts"))
        .unwrap()
        .split('/')
        .nth(1)
        .unwrap();
    let invalid_pair = format!("/Videos/{id}/hls1/{original_id}/stitch.m3u8?next={shorter_id}");
    assert_eq!(
        call(&app, &invalid_pair, Some(&token)).await.status(),
        StatusCode::NOT_FOUND
    );

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
    // One-step advance performs the next bounded encode and composes a
    // guarded two-window preview; regular next requests reuse that result.
    let advance_url = format!("/Videos/{id}/hls1/{publication_id}/advance.m3u8");
    assert_eq!(
        call(&app, &advance_url, None).await.status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        call(&app, &advance_url, Some(&other)).await.status(),
        StatusCode::NOT_FOUND
    );
    let head = Request::builder()
        .method("HEAD")
        .uri(advance_url.as_str())
        .header("X-Emby-Token", token.as_str())
        .body(Body::empty())
        .unwrap();
    assert_eq!(
        app.clone().oneshot(head).await.unwrap().status(),
        StatusCode::METHOD_NOT_ALLOWED
    );
    let advanced = call(&app, &advance_url, Some(&token)).await;
    assert_eq!(advanced.status(), StatusCode::OK);
    assert_eq!(advanced.headers()["x-pelagos-stitched"], "true");
    assert_eq!(
        advanced.headers()["x-pelagos-excerpt-start-ticks"],
        "6050000000"
    );
    let advanced_next = advanced.headers()["x-pelagos-next-path"]
        .to_str()
        .unwrap()
        .to_owned();
    let advanced_playlist = body(advanced).await;
    assert!(advanced_playlist.contains("#EXT-X-PLAYLIST-TYPE:VOD\n"));
    assert!(advanced_playlist.contains(&format!("/Videos/{id}/hls1/{publication_id}/0.ts")));
    assert_eq!(advanced_playlist.matches("#EXTINF:").count(), 4);
    assert_eq!(
        advanced_playlist.matches("#EXT-X-DISCONTINUITY\n").count(),
        3
    );
    assert!(advanced_playlist.ends_with("#EXT-X-ENDLIST\n"));
    // Three-window requests near EOF clamp to the actual two published
    // excerpts; they must not fabricate a third window past source end.
    let three_near_eof = format!("{advance_url}?windows=3");
    let response = call(&app, &three_near_eof, Some(&token)).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(body(response).await, advanced_playlist);
    for invalid in ["1", "4", "255", "bogus"] {
        let uri = format!("{advance_url}?windows={invalid}");
        assert_eq!(
            call(&app, &uri, Some(&token)).await.status(),
            StatusCode::BAD_REQUEST
        );
    }

    let response = call(&app, &next_url, Some(&token)).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["x-pelagos-next-path"], advanced_next);
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
    // The optional third-window UUID is never synthesized or accepted unless
    // it names an existing adjacent publication owned by this same token.
    let missing_third = format!("{stitch_url}&afterNext=missing-publication");
    assert_eq!(
        call(&app, &missing_third, Some(&token)).await.status(),
        StatusCode::NOT_FOUND
    );
    let replay_first = format!("{stitch_url}&afterNext={publication_id}");
    assert_eq!(
        call(&app, &replay_first, Some(&token)).await.status(),
        StatusCode::NOT_FOUND
    );
    let stitched = call(&app, &stitch_url, Some(&token)).await;
    assert_eq!(stitched.status(), StatusCode::OK);
    assert_eq!(stitched.headers()["x-pelagos-stitched"], "true");
    assert_eq!(
        stitched.headers()["x-pelagos-excerpt-start-ticks"],
        "6050000000"
    );
    let stitched_duration = stitched.headers()["x-pelagos-excerpt-duration-ticks"]
        .to_str()
        .unwrap()
        .parse::<u64>()
        .unwrap();
    assert_eq!(stitched_duration, 40_000_000 + final_duration);
    let stitched_playlist = body(stitched).await;
    let head = Request::builder()
        .method("HEAD")
        .uri(stitch_url.as_str())
        .header("X-Emby-Token", token.as_str())
        .body(Body::empty())
        .unwrap();
    assert_eq!(
        app.clone().oneshot(head).await.unwrap().status(),
        StatusCode::METHOD_NOT_ALLOWED
    );
    assert!(stitched_playlist.contains("#EXT-X-PLAYLIST-TYPE:VOD\n"));
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
    // Both excerpts are still live here. Revoke the viewer grant while
    // both segment roots exist, so a 404 proves reauthorization rather than
    // cache eviction or missing media.
    auth::revoke_library(&state_dir, "viewer", &library).unwrap();
    assert_eq!(
        call(&app, &stitch_url, Some(&token)).await.status(),
        StatusCode::NOT_FOUND
    );
    auth::grant_library(&state_dir, "viewer", &library).unwrap();

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
    let advance_eof = format!("/Videos/{id}/hls1/{final_id}/advance.m3u8");
    assert_eq!(
        call(&app, &advance_eof, Some(&token)).await.status(),
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
    // Force the next EOF request to pause *inside* FFprobe, revoke its
    // library grant while the probe is running, and then release it.
    // Without the post-probe authorization guard this would incorrectly
    // return HTTP 204 based on an obsolete permission check.
    fs::write(&probe_gate, b"hold").unwrap();
    let concurrent_app = app.clone();
    let concurrent_url = adaptive_exhausted.clone();
    let concurrent_token = token.clone();
    let pending_eof = tokio::spawn(async move {
        call(&concurrent_app, &concurrent_url, Some(&concurrent_token))
            .await
            .status()
    });
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while !probe_entered.exists() {
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
    })
    .await
    .unwrap();
    auth::revoke_library(&state_dir, "viewer", &library).unwrap();
    fs::write(&probe_release, b"resume").unwrap();
    assert_eq!(pending_eof.await.unwrap(), StatusCode::NOT_FOUND);
    fs::remove_file(&probe_gate).unwrap();
    auth::grant_library(&state_dir, "viewer", &library).unwrap();

    // A subtitle sidecar revision is equally significant. FFprobe only
    // examines the media container, so a sidecar replacement during its
    // asynchronous wait must not produce a successful EOF response.
    fs::remove_file(&probe_entered).unwrap();
    fs::remove_file(&probe_release).unwrap();
    fs::write(&probe_gate, b"hold").unwrap();
    let concurrent_app = app.clone();
    let concurrent_url = adaptive_exhausted.clone();
    let concurrent_token = token.clone();
    let pending_sidecar_eof = tokio::spawn(async move {
        call(&concurrent_app, &concurrent_url, Some(&concurrent_token))
            .await
            .status()
    });
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while !probe_entered.exists() {
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
    })
    .await
    .unwrap();
    let mut stale_sidecar = sup.clone();
    stale_sidecar.push(0);
    fs::write(&sidecar, stale_sidecar).unwrap();
    fs::write(&probe_release, b"resume").unwrap();
    assert_eq!(pending_sidecar_eof.await.unwrap(), StatusCode::NOT_FOUND);
    fs::remove_file(&probe_gate).unwrap();
    fs::write(&sidecar, &sup).unwrap();
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
        call(&app, &advance_url, Some(&token)).await.status(),
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
    let palette_offset = modified
        .windows(7)
        .position(|bytes| bytes == [0, 0, 1, 235, 128, 128, 255])
        .unwrap();
    modified[palette_offset + 6] = 0;
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
    let mut replacement = original.clone();
    replacement.extend_from_slice(b"synthetic changed source revision");
    fs::write(&source, replacement).unwrap();
    assert_eq!(
        call(&app, &adaptive_exhausted, Some(&token)).await.status(),
        StatusCode::NOT_FOUND
    );

    // Reuse the 610-second real-media fixture to exercise three independent
    // adjacent PGS excerpts, their actual encoded pixels and authenticated
    // segment transport. This is not proof of a rolling HLS scheduler.
    fs::write(&source, &original).unwrap();
    catalog::index(&state_dir, &library).unwrap();
    let three_first = format!(
        "/Videos/{id}/main.m3u8?segmentLength=10&startTimeTicks=6050000000&subtitleStreamIndex={index}&subtitleMethod=Encode&pelagosExcerptDurationTicks=10000000"
    );
    let first_response = call(&app, &three_first, Some(&token)).await;
    assert_eq!(first_response.status(), StatusCode::OK);
    let first_body = body(first_response).await;
    let first_segment = first_body
        .lines()
        .find(|line| line.starts_with("hls1/") && line.ends_with(".ts"))
        .unwrap()
        .to_owned();
    let first_id = first_segment.split('/').nth(1).unwrap();
    let second_url = format!("/Videos/{id}/hls1/{first_id}/next.m3u8");
    let second_response = call(&app, &second_url, Some(&token)).await;
    assert_eq!(second_response.status(), StatusCode::OK);
    assert_eq!(
        second_response.headers()["x-pelagos-excerpt-start-ticks"],
        "6060000000"
    );
    let second_body = body(second_response).await;
    let second_segment = second_body
        .lines()
        .find(|line| line.starts_with("hls1/") && line.ends_with(".ts"))
        .unwrap()
        .to_owned();
    let second_id = second_segment.split('/').nth(1).unwrap();
    let third_url = format!("/Videos/{id}/hls1/{second_id}/next.m3u8");
    let third_response = call(&app, &third_url, Some(&token)).await;
    assert_eq!(third_response.status(), StatusCode::OK);
    assert_eq!(
        third_response.headers()["x-pelagos-excerpt-start-ticks"],
        "6070000000"
    );
    let third_body = body(third_response).await;
    let third_segment = third_body
        .lines()
        .find(|line| line.starts_with("hls1/") && line.ends_with(".ts"))
        .unwrap()
        .to_owned();
    let third_id = third_segment.split('/').nth(1).unwrap();
    assert_ne!(first_id, second_id);
    assert_ne!(second_id, third_id);

    let three_url =
        format!("/Videos/{id}/hls1/{first_id}/stitch.m3u8?next={second_id}&afterNext={third_id}");
    assert_eq!(
        call(&app, &three_url, None).await.status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        call(&app, &three_url, Some(&other)).await.status(),
        StatusCode::NOT_FOUND
    );
    let response = call(&app, &three_url, Some(&token)).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["x-pelagos-stitched"], "true");
    assert_eq!(
        response.headers()["x-pelagos-excerpt-start-ticks"],
        "6050000000"
    );
    assert_eq!(
        response.headers()["x-pelagos-excerpt-duration-ticks"],
        "30000000"
    );
    let after_third = format!("/Videos/{id}/hls1/{third_id}/next.m3u8");
    assert_eq!(
        response.headers()["x-pelagos-next-path"].to_str().unwrap(),
        after_third.as_str()
    );
    let three_playlist = body(response).await;
    assert!(three_playlist.contains(&format!("/Videos/{id}/{first_segment}")));
    assert!(three_playlist.contains(&format!("/Videos/{id}/{second_segment}")));
    assert!(three_playlist.contains(&format!("/Videos/{id}/{third_segment}")));
    let entries = three_playlist
        .lines()
        .filter(|line| line.starts_with(&format!("/Videos/{id}/hls1/")) && line.ends_with(".ts"))
        .collect::<Vec<_>>();
    assert!(entries.len() >= 3);
    assert_eq!(
        three_playlist.matches("#EXT-X-DISCONTINUITY\n").count(),
        entries.len() - 1
    );
    assert!(three_playlist.ends_with("#EXT-X-ENDLIST\n"));

    // Automatic, bounded three-window mode must reuse these same private
    // publications and produce exactly the verified manual composition.
    let auto_three_url = format!("/Videos/{id}/hls1/{first_id}/advance.m3u8?windows=3");
    assert_eq!(
        call(&app, &auto_three_url, None).await.status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        call(&app, &auto_three_url, Some(&other)).await.status(),
        StatusCode::NOT_FOUND
    );
    let auto_three = call(&app, &auto_three_url, Some(&token)).await;
    assert_eq!(auto_three.status(), StatusCode::OK);
    assert_eq!(auto_three.headers()["x-pelagos-stitched"], "true");
    assert_eq!(
        auto_three.headers()["x-pelagos-excerpt-duration-ticks"],
        "30000000"
    );
    let retained_path = auto_three.headers()["x-pelagos-retained-path"]
        .to_str()
        .unwrap()
        .to_owned();
    assert_eq!(body(auto_three).await, three_playlist);
    assert_eq!(
        call(&app, &retained_path, None).await.status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        call(&app, &retained_path, Some(&other)).await.status(),
        StatusCode::NOT_FOUND
    );
    let retained_head = Request::builder()
        .method("HEAD")
        .uri(retained_path.as_str())
        .header("X-Emby-Token", token.as_str())
        .body(Body::empty())
        .unwrap();
    assert_eq!(
        app.clone().oneshot(retained_head).await.unwrap().status(),
        StatusCode::METHOD_NOT_ALLOWED
    );
    let retained = call(&app, &retained_path, Some(&token)).await;
    assert_eq!(retained.status(), StatusCode::OK);
    assert_eq!(body(retained).await, three_playlist);

    // Start the separate, one-slot sliding mode from a token-owned PGS
    // excerpt. Its stable media URI is reloaded by an HLS client, never
    // handed an ENDLIST before EOF. A one-second interval gives deterministic
    // two-step rolling coverage without a multi-hour fixture.
    let rolling_start = format!("/Videos/{id}/hls1/{first_id}/rolling-start.m3u8");
    assert_eq!(
        call(&app, &rolling_start, None).await.status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        call(&app, &rolling_start, Some(&other)).await.status(),
        StatusCode::NOT_FOUND
    );
    let started = call(&app, &rolling_start, Some(&token)).await;
    assert_eq!(started.status(), StatusCode::OK);
    assert_eq!(started.headers()["x-pelagos-rolling"], "true");
    let opening_etag = started.headers()["etag"].to_str().unwrap().to_owned();
    assert!(opening_etag.starts_with("\"rolling-"));
    let rolling_path = started.headers()["x-pelagos-rolling-path"]
        .to_str()
        .unwrap()
        .to_owned();
    let opening = body(started).await;
    let stable_target = opening
        .lines()
        .find(|line| line.starts_with("#EXT-X-TARGETDURATION:"))
        .unwrap()
        .to_owned();
    assert!(opening.contains(&format!("/Videos/{id}/{first_segment}")));
    assert!(!opening.contains("#EXT-X-ENDLIST"));
    assert!(!opening.contains("#EXT-X-PLAYLIST-TYPE:VOD"));
    let conditional = Request::builder()
        .uri(rolling_path.as_str())
        .header("X-Emby-Token", token.as_str())
        .header("If-None-Match", opening_etag.as_str())
        .body(Body::empty())
        .unwrap();
    let unchanged = app.clone().oneshot(conditional).await.unwrap();
    assert_eq!(unchanged.status(), StatusCode::NOT_MODIFIED);
    assert_eq!(unchanged.headers()["etag"], opening_etag);
    assert!(
        to_bytes(unchanged.into_body(), 64)
            .await
            .unwrap()
            .is_empty()
    );
    let invalid_etag = Request::builder()
        .uri(rolling_path.as_str())
        .header("X-Emby-Token", token.as_str())
        .header("If-None-Match", "\"unknown-generation\"")
        .body(Body::empty())
        .unwrap();
    let changed = app.clone().oneshot(invalid_etag).await.unwrap();
    assert_eq!(changed.status(), StatusCode::OK);
    assert_eq!(changed.headers()["etag"], opening_etag);
    assert_eq!(body(changed).await, opening);
    let wrong_token_conditional = Request::builder()
        .uri(rolling_path.as_str())
        .header("X-Emby-Token", other.as_str())
        .header("If-None-Match", opening_etag.as_str())
        .body(Body::empty())
        .unwrap();
    assert_eq!(
        app.clone()
            .oneshot(wrong_token_conditional)
            .await
            .unwrap()
            .status(),
        StatusCode::NOT_FOUND
    );
    auth::revoke_library(&state_dir, "viewer", &library).unwrap();
    let revoked_conditional = Request::builder()
        .uri(rolling_path.as_str())
        .header("X-Emby-Token", token.as_str())
        .header("If-None-Match", opening_etag.as_str())
        .body(Body::empty())
        .unwrap();
    assert_eq!(
        app.clone()
            .oneshot(revoked_conditional)
            .await
            .unwrap()
            .status(),
        StatusCode::NOT_FOUND
    );
    auth::grant_library(&state_dir, "viewer", &library).unwrap();
    assert_eq!(
        call(&app, &rolling_path, Some(&other)).await.status(),
        StatusCode::NOT_FOUND
    );
    let head = Request::builder()
        .method("HEAD")
        .uri(rolling_path.as_str())
        .header("X-Emby-Token", token.as_str())
        .body(Body::empty())
        .unwrap();
    assert_eq!(
        app.clone().oneshot(head).await.unwrap().status(),
        StatusCode::METHOD_NOT_ALLOWED
    );
    tokio::time::sleep(std::time::Duration::from_millis(1_100)).await;
    let second_rolling = call(&app, &rolling_path, Some(&token)).await;
    assert_eq!(second_rolling.status(), StatusCode::OK);
    assert_ne!(second_rolling.headers()["etag"], opening_etag);
    let second_rolling = body(second_rolling).await;
    assert!(second_rolling.lines().any(|line| line == stable_target));
    assert!(second_rolling.contains(&format!("/Videos/{id}/{second_segment}")));
    assert_eq!(
        second_rolling.matches("#EXTINF:").count(),
        first_body.matches("#EXTINF:").count() + second_body.matches("#EXTINF:").count()
    );
    tokio::time::sleep(std::time::Duration::from_millis(1_100)).await;
    let third_rolling = call(&app, &rolling_path, Some(&token)).await;
    assert_eq!(third_rolling.status(), StatusCode::OK);
    let third_rolling = body(third_rolling).await;
    assert!(third_rolling.lines().any(|line| line == stable_target));
    assert!(third_rolling.contains(&format!("/Videos/{id}/{third_segment}")));
    assert_eq!(
        third_rolling.matches("#EXTINF:").count(),
        first_body.matches("#EXTINF:").count()
            + second_body.matches("#EXTINF:").count()
            + third_body.matches("#EXTINF:").count()
    );
    assert!(!third_rolling.contains("#EXT-X-ENDLIST"));
    demux::assert_authenticated_hls_demux(
        &app,
        &token,
        &third_rolling,
        &temp.path().join("rolling-three-demux"),
    )
    .await;
    demux::assert_authenticated_hls_network_demux(&app, &token, &retained_path, false, false).await;
    demux::assert_authenticated_hls_network_demux(&app, &token, &rolling_path, true, false).await;

    // Force LRU eviction with four distinct authenticated publications
    // from the same synthetic source. The two/three-window lease must keep
    // all referenced segment roots readable despite unrelated encodes.
    for seconds in [600_u64, 601, 602, 603] {
        let start_ticks = seconds * 10_000_000;
        let unrelated = format!(
            "/Videos/{id}/main.m3u8?segmentLength=10&startTimeTicks={start_ticks}&subtitleStreamIndex={index}&subtitleMethod=Encode&pelagosExcerptDurationTicks=10000000"
        );
        assert_eq!(
            call(&app, &unrelated, Some(&token)).await.status(),
            StatusCode::OK
        );
    }
    let retained_again = call(&app, &retained_path, Some(&token)).await;
    assert_eq!(retained_again.status(), StatusCode::OK);
    assert_eq!(body(retained_again).await, three_playlist);

    // Release the active sliding session, then reuse the final second of
    // this same real source to verify that only a confirmed EOF adds ENDLIST.
    let stop_first = Request::builder()
        .method("DELETE")
        .uri(rolling_path.as_str())
        .header("X-Emby-Token", token.as_str())
        .body(Body::empty())
        .unwrap();
    assert_eq!(
        app.clone().oneshot(stop_first).await.unwrap().status(),
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        call(&app, &rolling_path, Some(&token)).await.status(),
        StatusCode::NOT_FOUND
    );
    // The old final-second UUID predates an intentional source mutation
    // above. Restore bytes alone must not resurrect its source revision.
    // Publish a fresh final excerpt against the current indexed revision.
    let eof_first = format!(
        "/Videos/{id}/main.m3u8?segmentLength=10&startTimeTicks=6090000000&subtitleStreamIndex={index}&subtitleMethod=Encode&pelagosExcerptDurationTicks=10000000"
    );
    let eof_publication = call(&app, &eof_first, Some(&token)).await;
    assert_eq!(eof_publication.status(), StatusCode::OK);
    let eof_manifest = body(eof_publication).await;
    let eof_segment = eof_manifest
        .lines()
        .find(|line| line.starts_with("hls1/") && line.ends_with(".ts"))
        .unwrap();
    let eof_segment_uri = format!("/Videos/{id}/{eof_segment}");
    let eof_id = eof_segment.split('/').nth(1).unwrap();
    let eof_start = format!("/Videos/{id}/hls1/{eof_id}/rolling-start.m3u8");
    let started_eof = call(&app, &eof_start, Some(&token)).await;
    assert_eq!(started_eof.status(), StatusCode::OK);
    let eof_rolling_path = started_eof.headers()["x-pelagos-rolling-path"]
        .to_str()
        .unwrap()
        .to_owned();
    let eof_live_etag = started_eof.headers()["etag"].to_str().unwrap().to_owned();
    let eof_opening = body(started_eof).await;
    assert!(!eof_opening.contains("#EXT-X-ENDLIST"));
    // Retrying the same start URI with a larger prebuffer request must
    // not trigger an unexpected source probe or advance an existing session.
    let retry_eof = call(&app, &format!("{eof_start}?prebuffer=2"), Some(&token)).await;
    assert_eq!(retry_eof.status(), StatusCode::OK);
    assert_eq!(body(retry_eof).await, eof_opening);
    tokio::time::sleep(std::time::Duration::from_millis(1_100)).await;
    let eof_rolling = call(&app, &eof_rolling_path, Some(&token)).await;
    assert_eq!(eof_rolling.status(), StatusCode::OK);
    let eof_ended_etag = eof_rolling.headers()["etag"].to_str().unwrap().to_owned();
    assert_ne!(eof_ended_etag, eof_live_etag);
    let ended = body(eof_rolling).await;
    assert!(ended.ends_with("#EXT-X-ENDLIST\n"));
    assert!(ended.contains(&eof_segment_uri));
    // EOF changes the generation despite publishing no additional segment.
    // An older validator must receive the ended playlist, while the current
    // validator may return 304 only after full library/source reauthorization.
    let old_etag_request = Request::builder()
        .uri(eof_rolling_path.as_str())
        .header("X-Emby-Token", token.as_str())
        .header("If-None-Match", eof_live_etag.as_str())
        .body(Body::empty())
        .unwrap();
    let old_etag_response = app.clone().oneshot(old_etag_request).await.unwrap();
    assert_eq!(old_etag_response.status(), StatusCode::OK);
    assert_eq!(body(old_etag_response).await, ended);
    let current_etag_request = Request::builder()
        .uri(eof_rolling_path.as_str())
        .header("X-Emby-Token", token.as_str())
        .header("If-None-Match", eof_ended_etag.as_str())
        .body(Body::empty())
        .unwrap();
    let current_etag_response = app.clone().oneshot(current_etag_request).await.unwrap();
    assert_eq!(current_etag_response.status(), StatusCode::NOT_MODIFIED);
    assert!(to_bytes(current_etag_response.into_body(), 64)
        .await
        .unwrap()
        .is_empty());
    assert_eq!(
        body(call(&app, &eof_rolling_path, Some(&token)).await).await,
        ended
    );

    // The first excerpt is clear; the second starts after the PGS show cue,
    // and the third still contains the visible graphic before its clear cue.
    for (position, (segment, visible)) in [
        (first_segment.as_str(), false),
        (second_segment.as_str(), true),
        (third_segment.as_str(), true),
    ]
    .into_iter()
    .enumerate()
    {
        let url = format!("/Videos/{id}/{segment}");
        assert_eq!(
            call(&app, &url, Some(&other)).await.status(),
            StatusCode::NOT_FOUND
        );
        let transport = call(&app, &url, Some(&token)).await;
        assert_eq!(transport.status(), StatusCode::OK);
        let bytes = to_bytes(transport.into_body(), 8 * 1024 * 1024)
            .await
            .unwrap()
            .to_vec();
        assert!(!bytes.is_empty());
        assert!(bytes.chunks_exact(188).all(|packet| packet[0] == 0x47));
        assert_eq!(
            visible_center(
                &bytes,
                &temp.path().join(format!("three-window-{position}.ts"))
            ),
            visible
        );
    }

    // The final-second rolling session has reached ENDLIST. Release it
    // before starting a new independent session over four clear PGS windows.
    // This must *slide*: after four published windows, the first drops out of
    // both the three-window session and the three-entry global cache.
    let stop_eof = Request::builder()
        .method("DELETE")
        .uri(eof_rolling_path.as_str())
        .header("X-Emby-Token", token.as_str())
        .body(Body::empty())
        .unwrap();
    assert_eq!(
        app.clone().oneshot(stop_eof).await.unwrap().status(),
        StatusCode::NO_CONTENT
    );
    // Explicit prebuffering at the source boundary must clamp immediately
    // to a completed live playlist. No phantom successor is generated and
    // the original authenticated segment URI remains valid.
    let eof_prebuffer_start = format!("/Videos/{id}/hls1/{eof_id}/rolling-start.m3u8?prebuffer=2");
    let eof_ready = call(&app, &eof_prebuffer_start, Some(&token)).await;
    assert_eq!(eof_ready.status(), StatusCode::OK);
    let eof_ready_path = eof_ready.headers()["x-pelagos-rolling-path"]
        .to_str()
        .unwrap()
        .to_owned();
    assert_eq!(body(eof_ready).await, ended);
    let release_eof_ready = Request::builder()
        .method("DELETE")
        .uri(eof_ready_path.as_str())
        .header("X-Emby-Token", token.as_str())
        .body(Body::empty())
        .unwrap();
    assert_eq!(
        app.clone()
            .oneshot(release_eof_ready)
            .await
            .unwrap()
            .status(),
        StatusCode::NO_CONTENT
    );
    let sliding_first = format!(
        "/Videos/{id}/main.m3u8?segmentLength=10&startTimeTicks=6000000000&subtitleStreamIndex={index}&subtitleMethod=Encode&pelagosExcerptDurationTicks=10000000"
    );
    let start_segmented = call(&app, &sliding_first, Some(&token)).await;
    assert_eq!(start_segmented.status(), StatusCode::OK);
    let start_segments = body(start_segmented).await;
    let start_leaf = start_segments
        .lines()
        .find(|line| line.starts_with("hls1/") && line.ends_with(".ts"))
        .unwrap();
    let initial_segment_path = format!("/Videos/{id}/{start_leaf}");
    let initial_id = start_leaf.split('/').nth(1).unwrap();
    let sliding_start = format!("/Videos/{id}/hls1/{initial_id}/rolling-start.m3u8?prebuffer=2");
    let invalid_prebuffer =
        format!("/Videos/{id}/hls1/{initial_id}/rolling-start.m3u8?prebuffer=3");
    assert_eq!(
        call(&app, &invalid_prebuffer, Some(&token)).await.status(),
        StatusCode::BAD_REQUEST
    );
    let sliding_response = call(&app, &sliding_start, Some(&token)).await;
    assert_eq!(sliding_response.status(), StatusCode::OK);
    let sliding_path = sliding_response.headers()["x-pelagos-rolling-path"]
        .to_str()
        .unwrap()
        .to_owned();
    let initial_live = body(sliding_response).await;
    let stable_target = initial_live
        .lines()
        .find(|line| line.starts_with("#EXT-X-TARGETDURATION:"))
        .unwrap()
        .to_owned();
    assert!(initial_live.contains(&initial_segment_path));
    assert!(initial_live.matches("#EXTINF:").count() >= 3);
    let retried_start = call(&app, &sliding_start, Some(&token)).await;
    assert_eq!(retried_start.status(), StatusCode::OK);
    assert_eq!(body(retried_start).await, initial_live);
    tokio::time::sleep(std::time::Duration::from_millis(1_100)).await;
    let step = call(&app, &sliding_path, Some(&token)).await;
    assert_eq!(step.status(), StatusCode::OK);
    let last_live = body(step).await;
    assert!(last_live.lines().any(|line| line == stable_target));
    assert!(!last_live.contains("#EXT-X-ENDLIST"));
    let sequence = last_live
        .lines()
        .find_map(|line| line.strip_prefix("#EXT-X-MEDIA-SEQUENCE:"))
        .unwrap()
        .parse::<u64>()
        .unwrap();
    assert!(sequence > 0);
    assert!(last_live.contains(&format!("#EXT-X-DISCONTINUITY-SEQUENCE:{sequence}\n")));
    assert!(!last_live.contains(&initial_segment_path));
    demux::assert_authenticated_hls_demux(
        &app,
        &token,
        &last_live,
        &temp.path().join("rolling-slid-demux"),
    )
    .await;
    // A startup retry can independently promote the first publication in
    // the global three-entry LRU. Sliding removal must not revoke that
    // separate cache ownership. Force unrelated publications to evict it,
    // then confirm the rolling session still pins its newest TS segments.
    let active_segment = last_live
        .lines()
        .rfind(|line| line.starts_with(&format!("/Videos/{id}/hls1/")) && line.ends_with(".ts"))
        .unwrap()
        .to_owned();
    for seconds in [590_u64, 591, 592, 593] {
        let start_ticks = seconds * 10_000_000;
        let unrelated = format!(
            "/Videos/{id}/main.m3u8?segmentLength=10&startTimeTicks={start_ticks}&subtitleStreamIndex={index}&subtitleMethod=Encode&pelagosExcerptDurationTicks=10000000"
        );
        assert_eq!(
            call(&app, &unrelated, Some(&token)).await.status(),
            StatusCode::OK
        );
    }
    assert_eq!(
        call(&app, &initial_segment_path, Some(&token))
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        call(&app, &active_segment, Some(&token)).await.status(),
        StatusCode::OK
    );

    auth::revoke_library(&state_dir, "viewer", &library).unwrap();
    assert_eq!(
        call(&app, &sliding_path, Some(&token)).await.status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        call(&app, &retained_path, Some(&token)).await.status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        call(&app, &three_url, Some(&token)).await.status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        call(&app, &format!("/Videos/{id}/{third_segment}"), Some(&token))
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    let wrong_release = Request::builder()
        .method("DELETE")
        .uri(retained_path.as_str())
        .header("X-Emby-Token", other.as_str())
        .body(Body::empty())
        .unwrap();
    assert_eq!(
        app.clone().oneshot(wrong_release).await.unwrap().status(),
        StatusCode::NOT_FOUND
    );
    let release = Request::builder()
        .method("DELETE")
        .uri(retained_path.as_str())
        .header("X-Emby-Token", token.as_str())
        .body(Body::empty())
        .unwrap();
    assert_eq!(
        app.clone().oneshot(release).await.unwrap().status(),
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        call(&app, &retained_path, Some(&token)).await.status(),
        StatusCode::NOT_FOUND
    );
    let release_rolling = Request::builder()
        .method("DELETE")
        .uri(sliding_path.as_str())
        .header("X-Emby-Token", token.as_str())
        .body(Body::empty())
        .unwrap();
    assert_eq!(
        app.clone().oneshot(release_rolling).await.unwrap().status(),
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        call(&app, &sliding_path, Some(&token)).await.status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(fs::read(&source).unwrap(), original);
}
