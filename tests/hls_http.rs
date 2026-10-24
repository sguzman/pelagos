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
use serde_json::Value;
use std::{path::Path, process::Command, sync::Arc};
use tower::ServiceExt;

async fn request(app: &Router, method: &str, path: &str, token: Option<&str>) -> Response {
    let mut req = Request::builder().method(method).uri(path);
    if let Some(token) = token {
        req = req.header("X-Emby-Token", token);
    }
    app.clone()
        .oneshot(req.body(Body::empty()).unwrap())
        .await
        .unwrap()
}

async fn request_range(
    app: &Router,
    method: &str,
    path: &str,
    token: &str,
    range: &str,
) -> Response {
    app.clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(path)
                .header("X-Emby-Token", token)
                .header(header::RANGE, range)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
}

async fn request_if_range(
    app: &Router,
    method: &str,
    path: &str,
    token: &str,
    byte_range: &str,
    validator: &str,
) -> Response {
    app.clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(path)
                .header("X-Emby-Token", token)
                .header(header::RANGE, byte_range)
                .header(header::IF_RANGE, validator)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
}

async fn text_body(response: Response) -> String {
    let bytes = to_bytes(response.into_body(), 65536).await.unwrap();
    String::from_utf8(bytes.to_vec()).unwrap()
}

async fn assert_playlist_head(app: &Router, path: &str, token: &str, size: usize) {
    let response = request(app, "HEAD", path, Some(token)).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers()[header::CONTENT_LENGTH]
            .to_str()
            .unwrap()
            .parse::<usize>()
            .unwrap(),
        size
    );
    assert!(
        to_bytes(response.into_body(), 4096)
            .await
            .unwrap()
            .is_empty()
    );
}

#[cfg(unix)]
#[tokio::test]
async fn authenticated_hls_playlist_segments_and_current_library_grants() {
    if std::env::var_os("PELAGOS_REQUIRE_FFMPEG").is_none() {
        return;
    }
    let fixture = tempfile::tempdir().unwrap();
    let library = fixture.path().join("Movies");
    let state_dir = fixture.path().join("state");
    std::fs::create_dir_all(&library).unwrap();
    let source = library.join("Feature.avi");
    let created = Command::new("ffmpeg")
        .args([
            "-nostdin",
            "-hide_banner",
            "-loglevel",
            "error",
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=160x90:rate=15:duration=2.5",
            "-c:v",
            "mpeg4",
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
    catalog::index(&state_dir, &library).unwrap();
    auth::create_user(&state_dir, "admin", "hls-route-admin-password").unwrap();
    auth::create_user(&state_dir, "viewer", "hls-route-viewer-password").unwrap();
    let admin = auth::authenticate(&state_dir, "admin", "hls-route-admin-password", "server")
        .unwrap()
        .unwrap()
        .access_token;
    let sibling = auth::authenticate(&state_dir, "admin", "hls-route-admin-password", "server")
        .unwrap()
        .unwrap()
        .access_token;
    let viewer = auth::authenticate(&state_dir, "viewer", "hls-route-viewer-password", "server")
        .unwrap()
        .unwrap()
        .access_token;
    let app = app::router(Arc::new(AppState::with_auth_dir(
        "server".to_owned(),
        state_dir.clone(),
    )));
    let items = request(&app, "GET", "/Items", Some(&admin)).await;
    let values: Value =
        serde_json::from_slice(&to_bytes(items.into_body(), 65536).await.unwrap()).unwrap();
    let id = values["Items"][0]["Id"].as_str().unwrap();
    let route = format!("/Videos/{id}/main.m3u8?segmentLength=2");
    assert_eq!(
        request(&app, "GET", &route, None).await.status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        request(&app, "GET", &route, Some(&viewer)).await.status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        request(
            &app,
            "GET",
            &format!("/Videos/{id}/main.m3u8?segmentLength=0"),
            Some(&admin)
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        request(
            &app,
            "GET",
            &format!("/Videos/{id}/main.m3u8?bogus=1"),
            Some(&admin)
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );
    auth::grant_library(&state_dir, "viewer", &library).unwrap();
    // Players can request main and master concurrently. Join the bounded
    // publication queue instead of returning 429 for the second request.
    let concurrent_master = format!("/Videos/{id}/master.m3u8?segmentLength=2");
    let (result, paired) = tokio::join!(
        request(&app, "GET", &route, Some(&viewer)),
        request(&app, "GET", &concurrent_master, Some(&viewer))
    );
    assert_eq!(paired.status(), StatusCode::OK);
    let paired_manifest = text_body(paired).await;
    assert_eq!(result.status(), StatusCode::OK);
    assert_eq!(
        result.headers()[header::CONTENT_TYPE],
        "application/vnd.apple.mpegurl"
    );
    assert_eq!(result.headers()[header::CACHE_CONTROL], "private, no-store");
    let manifest = text_body(result).await;
    assert_playlist_head(&app, &route, &viewer, manifest.len()).await;
    assert!(manifest.starts_with("#EXTM3U\n"));
    assert!(manifest.contains("#EXT-X-DISCONTINUITY"));
    assert!(manifest.ends_with("#EXT-X-ENDLIST\n"));
    // Seeked HLS is a new publication containing only the remaining
    // source duration. Identical requests reuse that publication rather
    // than sharing an incorrect zero-offset cache entry.
    let seek_route = format!("/Videos/{id}/main.m3u8?segmentLength=2&startTimeTicks=10000000");
    let seek_response = request(&app, "GET", &seek_route, Some(&viewer)).await;
    assert_eq!(seek_response.status(), StatusCode::OK);
    let seek_manifest = text_body(seek_response).await;
    assert_eq!(
        seek_manifest
            .lines()
            .filter(|line| line.starts_with("hls1/") && line.ends_with(".ts"))
            .count(),
        1
    );
    assert_ne!(seek_manifest, manifest);
    let seek_master = format!("/Videos/{id}/master.m3u8?segmentLength=2&startTimeTicks=10000000");
    let seek_master_text = text_body(request(&app, "GET", &seek_master, Some(&viewer)).await).await;
    let sought_id = seek_manifest
        .lines()
        .find(|line| line.starts_with("hls1/") && line.ends_with(".ts"))
        .unwrap()
        .split('/')
        .nth(1)
        .unwrap();
    assert!(seek_master_text.contains(&format!("hls1/{sought_id}/index.m3u8")));
    assert_eq!(
        request(
            &app,
            "HEAD",
            &format!("/Videos/{id}/hls1/{sought_id}/0.ts"),
            Some(&viewer),
        )
        .await
        .status(),
        StatusCode::OK
    );
    let seek_part = request(
        &app,
        "GET",
        &format!("/Videos/{id}/hls1/{sought_id}/0.ts"),
        Some(&viewer),
    )
    .await;
    assert_eq!(seek_part.status(), StatusCode::OK);
    let seek_bytes = to_bytes(seek_part.into_body(), 4 * 1024 * 1024)
        .await
        .unwrap();
    assert!(!seek_bytes.is_empty() && seek_bytes.len() % 188 == 0);
    assert!(seek_bytes.chunks_exact(188).all(|packet| packet[0] == 0x47));
    assert_eq!(
        text_body(request(&app, "GET", &seek_route, Some(&viewer)).await).await,
        seek_manifest
    );
    let zero_offset_route = format!("/Videos/{id}/main.m3u8?segmentLength=2&startTimeTicks=0");
    assert_eq!(
        text_body(request(&app, "GET", &zero_offset_route, Some(&viewer)).await).await,
        manifest
    );
    for invalid in [
        format!("/Videos/{id}/main.m3u8?segmentLength=2&startTimeTicks=-1"),
        format!("/Videos/{id}/main.m3u8?segmentLength=2&startTimeTicks=6000000001"),
        format!("/Videos/{id}/main.m3u8?segmentLength=2&startTimeTicks=invalid"),
        format!("/Videos/{id}/main.m3u8?segmentLength=2&startTimeTicks=30000000"),
    ] {
        assert_eq!(
            request(&app, "GET", &invalid, Some(&viewer)).await.status(),
            StatusCode::BAD_REQUEST
        );
    }

    assert!(!manifest.contains(&viewer));
    assert!(!manifest.contains("segment00000.ts"));
    let first_segment_id = manifest
        .lines()
        .find(|line| line.starts_with("hls1/") && line.ends_with(".ts"))
        .unwrap()
        .split('/')
        .nth(1)
        .unwrap();
    assert!(paired_manifest.contains(&format!("hls1/{first_segment_id}/index.m3u8")));
    let segment_paths = manifest
        .lines()
        .filter(|line| line.starts_with("hls1/") && line.ends_with(".ts"))
        .map(|line| format!("/Videos/{id}/{line}"))
        .collect::<Vec<_>>();
    assert_eq!(segment_paths.len(), 2);
    let first_path = &segment_paths[0];
    let response = request(&app, "GET", first_path, Some(&viewer)).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[header::CONTENT_TYPE], "video/mp2t");
    let bytes = to_bytes(response.into_body(), 4 * 1024 * 1024)
        .await
        .unwrap();
    assert!(bytes.len() >= 188 && bytes.len() % 188 == 0);
    for packet in bytes.chunks_exact(188) {
        assert_eq!(packet[0], 0x47);
    }
    let partial = request_range(&app, "GET", first_path, &viewer, "bytes=0-187").await;
    assert_eq!(partial.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(partial.headers()[header::CONTENT_LENGTH], "188");
    let etag = partial.headers()[header::ETAG].to_str().unwrap().to_owned();
    assert!(etag.starts_with("\"hls-") && etag.ends_with('"'));
    assert!(!etag.contains(&viewer));
    let resumed = request_if_range(&app, "GET", first_path, &viewer, "bytes=0-187", &etag).await;
    assert_eq!(resumed.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(resumed.headers()[header::ETAG], etag);
    assert_eq!(to_bytes(resumed.into_body(), 188).await.unwrap().len(), 188);
    // Weak validators are never sufficient for safe byte-range resumption.
    let weak = request_if_range(
        &app,
        "HEAD",
        first_path,
        &viewer,
        "bytes=0-187",
        &format!("W/{etag}"),
    )
    .await;
    assert_eq!(weak.status(), StatusCode::OK);
    assert!(weak.headers().get(header::CONTENT_RANGE).is_none());
    assert_eq!(
        weak.headers()[header::CONTENT_LENGTH]
            .to_str()
            .unwrap()
            .parse::<usize>()
            .unwrap(),
        bytes.len()
    );
    let stale = request_if_range(
        &app,
        "GET",
        first_path,
        &viewer,
        "bytes=0-187",
        "\"other-published-part\"",
    )
    .await;
    assert_eq!(stale.status(), StatusCode::OK);
    assert_eq!(stale.headers()[header::ETAG], etag);
    assert!(stale.headers().get(header::CONTENT_RANGE).is_none());
    assert_eq!(
        to_bytes(stale.into_body(), 4 * 1024 * 1024)
            .await
            .unwrap()
            .len(),
        bytes.len()
    );
    let stale_head = request_if_range(
        &app,
        "HEAD",
        first_path,
        &viewer,
        "bytes=0-187",
        "\"other-published-part\"",
    )
    .await;
    assert_eq!(stale_head.status(), StatusCode::OK);
    assert_eq!(
        stale_head.headers()[header::CONTENT_LENGTH]
            .to_str()
            .unwrap()
            .parse::<usize>()
            .unwrap(),
        bytes.len()
    );
    assert!(
        to_bytes(stale_head.into_body(), 4096)
            .await
            .unwrap()
            .is_empty()
    );

    assert_eq!(
        partial.headers()[header::CONTENT_RANGE],
        format!("bytes 0-187/{}", bytes.len())
    );
    assert_eq!(partial.headers()[header::ACCEPT_RANGES], "bytes");
    let first_packet = to_bytes(partial.into_body(), 188).await.unwrap();
    assert_eq!(first_packet.len(), 188);
    assert_eq!(first_packet[0], 0x47);
    let suffix = request_range(&app, "GET", first_path, &viewer, "bytes=-188").await;
    assert_eq!(suffix.status(), StatusCode::PARTIAL_CONTENT);
    let last_packet = to_bytes(suffix.into_body(), 188).await.unwrap();
    assert_eq!(last_packet.len(), 188);
    assert_eq!(last_packet[0], 0x47);
    let partial_head = request_range(&app, "HEAD", first_path, &viewer, "bytes=188-375").await;
    assert_eq!(partial_head.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(partial_head.headers()[header::CONTENT_LENGTH], "188");
    assert!(
        to_bytes(partial_head.into_body(), 4096)
            .await
            .unwrap()
            .is_empty()
    );
    for invalid in ["bytes=0-1,3-4", "bytes=999999999-", "bytes=-0"] {
        let response = request_range(&app, "GET", first_path, &viewer, invalid).await;
        assert_eq!(response.status(), StatusCode::RANGE_NOT_SATISFIABLE);
        assert_eq!(
            response.headers()[header::CONTENT_RANGE],
            format!("bytes */{}", bytes.len())
        );
    }
    let head = request(&app, "HEAD", first_path, Some(&viewer)).await;
    assert_eq!(head.status(), StatusCode::OK);
    assert_eq!(
        head.headers()[header::CONTENT_LENGTH]
            .to_str()
            .unwrap()
            .parse::<usize>()
            .unwrap(),
        bytes.len()
    );
    assert!(to_bytes(head.into_body(), 4096).await.unwrap().is_empty());
    assert_eq!(
        request(&app, "GET", first_path, None).await.status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        request(&app, "GET", first_path, Some(&sibling))
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        request(&app, "GET", first_path, Some(&admin))
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        request(&app, "GET", &format!("{first_path}X"), Some(&viewer))
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        request(&app, "GET", &segment_paths[1], Some(&viewer))
            .await
            .status(),
        StatusCode::OK
    );
    let other_etag = request(&app, "HEAD", &segment_paths[1], Some(&viewer)).await;
    assert_ne!(other_etag.headers()[header::ETAG], etag);
    // The pinned master endpoint serves one measured, completed rendition;
    // its nested media playlist uses paths relative to the hls1 directory.
    let master_route = format!("/Videos/{id}/master.m3u8?segmentLength=2");
    assert_eq!(
        request(&app, "GET", &master_route, None).await.status(),
        StatusCode::UNAUTHORIZED
    );
    let master_response = request(&app, "GET", &master_route, Some(&viewer)).await;
    assert_eq!(master_response.status(), StatusCode::OK);
    let master = text_body(master_response).await;
    assert_playlist_head(&app, &master_route, &viewer, master.len()).await;
    assert!(master.starts_with("#EXTM3U\n#EXT-X-VERSION:3\n"));
    assert!(master.contains("#EXT-X-STREAM-INF:BANDWIDTH="));
    assert!(!master.contains(&viewer));
    let media_name = master.lines().find(|l| l.starts_with("hls1/")).unwrap();
    assert!(media_name.ends_with("/index.m3u8"));
    // GET master after main must reuse the same completed publication.
    let first_id = first_path.split('/').nth(4).unwrap();
    assert_eq!(media_name.split('/').nth(1), Some(first_id));
    let repeated_master = text_body(request(&app, "GET", &master_route, Some(&viewer)).await).await;
    assert_eq!(repeated_master, master);
    let repeated_main = text_body(request(&app, "GET", &route, Some(&viewer)).await).await;
    assert_eq!(repeated_main, manifest);
    let media_url = format!("/Videos/{id}/{media_name}");
    assert_eq!(
        request(&app, "GET", &media_url, Some(&admin))
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    let media_response = request(&app, "GET", &media_url, Some(&viewer)).await;
    assert_eq!(media_response.status(), StatusCode::OK);
    let media_text = text_body(media_response).await;
    assert!(media_text.contains("#EXT-X-DISCONTINUITY"));
    assert!(media_text.contains("\n0.ts\n"));
    assert!(media_text.contains("\n1.ts\n"));
    assert!(!media_text.contains("segment00000.ts"));
    let parent = media_url.strip_suffix("index.m3u8").unwrap();
    let second_segment = format!("{parent}1.ts");
    assert_eq!(
        request(&app, "GET", &second_segment, Some(&viewer))
            .await
            .status(),
        StatusCode::OK
    );
    let media_head = request(&app, "HEAD", &media_url, Some(&viewer)).await;
    assert_eq!(media_head.status(), StatusCode::OK);
    assert_eq!(
        media_head.headers()[header::CONTENT_LENGTH]
            .to_str()
            .unwrap()
            .parse::<usize>()
            .unwrap(),
        media_text.len()
    );
    assert!(
        to_bytes(media_head.into_body(), 1_024)
            .await
            .unwrap()
            .is_empty()
    );
    auth::revoke_library(&state_dir, "viewer", &library).unwrap();
    assert_eq!(
        request(&app, "GET", first_path, Some(&viewer))
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    auth::grant_library(&state_dir, "viewer", &library).unwrap();
    assert_eq!(
        request(&app, "GET", first_path, Some(&viewer))
            .await
            .status(),
        StatusCode::OK
    );
    std::fs::write(&source, b"source replaced after HLS publication").unwrap();
    assert_eq!(
        request(&app, "GET", first_path, Some(&viewer))
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        request(&app, "HEAD", &media_url, Some(&viewer))
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    auth::revoke(&state_dir, &viewer).unwrap();
    assert_eq!(
        request(&app, "GET", first_path, Some(&viewer))
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    drop(app);
    let playlist_id = manifest
        .lines()
        .find(|l| l.starts_with("hls1/"))
        .unwrap()
        .split('/')
        .nth(1)
        .unwrap();
    assert!(!Path::new(&std::env::temp_dir().join(format!("pelagos-hls-{playlist_id}"))).exists());
    let master_id = media_name.split('/').nth(1).unwrap();
    assert!(
        !std::env::temp_dir()
            .join(format!("pelagos-hls-{master_id}"))
            .exists()
    );
}

#[cfg(unix)]
#[tokio::test]
async fn audio_hls_routes_are_token_scoped_and_deliver_probeable_transport() {
    if std::env::var_os("PELAGOS_REQUIRE_FFMPEG").is_none() {
        return;
    }
    let fixture = tempfile::tempdir().unwrap();
    let library = fixture.path().join("Music");
    let state_dir = fixture.path().join("state");
    std::fs::create_dir_all(&library).unwrap();
    let source = library.join("Tone.wav");
    let created = Command::new("ffmpeg")
        .args([
            "-nostdin",
            "-hide_banner",
            "-loglevel",
            "error",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:duration=2.6",
            "-c:a",
            "pcm_s16le",
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
    catalog::index(&state_dir, &library).unwrap();
    auth::create_user(&state_dir, "admin", "audio-hls-admin-password").unwrap();
    auth::create_user(&state_dir, "listener", "audio-hls-listener-password").unwrap();
    let admin = auth::authenticate(&state_dir, "admin", "audio-hls-admin-password", "server")
        .unwrap()
        .unwrap()
        .access_token;
    let listener = auth::authenticate(
        &state_dir,
        "listener",
        "audio-hls-listener-password",
        "server",
    )
    .unwrap()
    .unwrap()
    .access_token;
    let sibling = auth::authenticate(
        &state_dir,
        "listener",
        "audio-hls-listener-password",
        "server",
    )
    .unwrap()
    .unwrap()
    .access_token;
    auth::grant_library(&state_dir, "listener", &library).unwrap();
    let app = app::router(Arc::new(AppState::with_auth_dir(
        "server".to_owned(),
        state_dir.clone(),
    )));
    let items = request(&app, "GET", "/Items", Some(&admin)).await;
    let values: Value =
        serde_json::from_slice(&to_bytes(items.into_body(), 65_536).await.unwrap()).unwrap();
    let id = values["Items"][0]["Id"].as_str().unwrap();
    let master_route = format!("/Audio/{id}/master.m3u8?segmentLength=2");
    assert_eq!(
        request(&app, "GET", &master_route, None).await.status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        request(
            &app,
            "GET",
            &format!("/Videos/{id}/master.m3u8"),
            Some(&listener)
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
    let master_response = request(&app, "GET", &master_route, Some(&listener)).await;
    assert_eq!(master_response.status(), StatusCode::OK);
    let master = text_body(master_response).await;
    assert_playlist_head(&app, &master_route, &listener, master.len()).await;
    assert!(master.contains("#EXT-X-STREAM-INF:BANDWIDTH="));
    let media_name = master
        .lines()
        .find(|line| line.starts_with("hls1/"))
        .unwrap();
    let media_url = format!("/Audio/{id}/{media_name}");
    let media_response = request(&app, "GET", &media_url, Some(&listener)).await;
    assert_eq!(media_response.status(), StatusCode::OK);
    let playlist = text_body(media_response).await;
    assert!(playlist.contains("#EXT-X-ENDLIST"));
    assert!(playlist.contains("\n0.ts\n") && playlist.contains("\n1.ts\n"));
    assert_eq!(
        request(&app, "GET", &media_url, Some(&sibling))
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    let first = format!("{}0.ts", media_url.strip_suffix("index.m3u8").unwrap());
    let segment = request(&app, "GET", &first, Some(&listener)).await;
    assert_eq!(segment.status(), StatusCode::OK);
    assert_eq!(segment.headers()[header::CONTENT_TYPE], "video/mp2t");
    let bytes = to_bytes(segment.into_body(), 4 * 1024 * 1024)
        .await
        .unwrap();
    assert!(bytes.len() > 188 && bytes.len() % 188 == 0);
    assert!(bytes.chunks_exact(188).all(|packet| packet[0] == 0x47));
    let partial = request_range(&app, "GET", &first, &listener, "bytes=0-187").await;
    assert_eq!(partial.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(partial.headers()[header::CONTENT_LENGTH], "188");
    let etag = partial.headers()[header::ETAG].to_str().unwrap().to_owned();
    let good = request_if_range(&app, "HEAD", &first, &listener, "bytes=0-187", &etag).await;
    assert_eq!(good.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(good.headers()[header::CONTENT_LENGTH], "188");
    let stale = request_if_range(
        &app,
        "GET",
        &first,
        &listener,
        "bytes=0-187",
        "\"obsolete-segment\"",
    )
    .await;
    assert_eq!(stale.status(), StatusCode::OK);
    assert!(stale.headers().get(header::CONTENT_RANGE).is_none());
    assert_eq!(
        to_bytes(stale.into_body(), 4 * 1024 * 1024)
            .await
            .unwrap()
            .len(),
        bytes.len()
    );

    assert_eq!(to_bytes(partial.into_body(), 188).await.unwrap().len(), 188);
    assert_eq!(
        request(
            &app,
            "GET",
            &format!("/Videos/{id}/{media_name}"),
            Some(&listener)
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
    let main = request(
        &app,
        "GET",
        &format!("/Audio/{id}/main.m3u8?segmentLength=2"),
        Some(&listener),
    )
    .await;
    assert_eq!(main.status(), StatusCode::OK);
    let main_text = text_body(main).await;
    let main_route = format!("/Audio/{id}/main.m3u8?segmentLength=2");
    assert_playlist_head(&app, &main_route, &listener, main_text.len()).await;
    assert!(main_text.lines().any(|line| line.starts_with("hls1/")));
    // Audio main and master represent the same authenticated rendition.
    assert!(main_text.contains(&format!(
        "hls1/{}/0.ts",
        media_name.split('/').nth(1).unwrap()
    )));
    let repeated = text_body(request(&app, "GET", &master_route, Some(&listener)).await).await;
    assert_eq!(repeated, master);
    let audio_seek_route = format!("/Audio/{id}/main.m3u8?segmentLength=2&startTimeTicks=10000000");
    let audio_seek =
        text_body(request(&app, "GET", &audio_seek_route, Some(&listener)).await).await;
    assert_eq!(
        audio_seek
            .lines()
            .filter(|line| line.starts_with("hls1/") && line.ends_with(".ts"))
            .count(),
        1
    );
    assert_ne!(audio_seek, main_text);
    auth::revoke_library(&state_dir, "listener", &library).unwrap();
    assert_eq!(
        request(&app, "GET", &first, Some(&listener)).await.status(),
        StatusCode::NOT_FOUND
    );
    auth::grant_library(&state_dir, "listener", &library).unwrap();
    assert_eq!(
        request(&app, "HEAD", &first, Some(&listener))
            .await
            .status(),
        StatusCode::OK
    );
    std::fs::write(&source, b"modified audio media after publication").unwrap();
    assert_eq!(
        request(&app, "GET", &first, Some(&listener)).await.status(),
        StatusCode::NOT_FOUND
    );
    auth::revoke(&state_dir, &listener).unwrap();
    assert_eq!(
        request(&app, "GET", &first, Some(&listener)).await.status(),
        StatusCode::UNAUTHORIZED
    );
    drop(app);
    let id = media_name.split('/').nth(1).unwrap();
    assert!(
        !std::env::temp_dir()
            .join(format!("pelagos-hls-{id}"))
            .exists()
    );
}

#[cfg(unix)]
#[tokio::test]
async fn two_verified_video_hls_renditions_are_scoped_and_transport_accessible() {
    if std::env::var_os("PELAGOS_REQUIRE_FFMPEG").is_none() {
        return;
    }
    let fixture = tempfile::tempdir().unwrap();
    let library = fixture.path().join("Movies");
    let state_dir = fixture.path().join("state");
    std::fs::create_dir_all(&library).unwrap();
    let source = library.join("Adaptive.avi");
    let encoded = Command::new("ffmpeg")
        .args([
            "-nostdin",
            "-hide_banner",
            "-loglevel",
            "error",
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=640x360:rate=30:duration=4.2",
            "-c:v",
            "mpeg4",
            "-y",
        ])
        .arg(&source)
        .output()
        .unwrap();
    assert!(
        encoded.status.success(),
        "{}",
        String::from_utf8_lossy(&encoded.stderr)
    );
    catalog::index(&state_dir, &library).unwrap();
    auth::create_user(&state_dir, "admin", "adaptive-hls-admin-password").unwrap();
    auth::create_user(&state_dir, "viewer", "adaptive-hls-viewer-password").unwrap();
    let admin = auth::authenticate(&state_dir, "admin", "adaptive-hls-admin-password", "server")
        .unwrap()
        .unwrap()
        .access_token;
    let owner = auth::authenticate(
        &state_dir,
        "viewer",
        "adaptive-hls-viewer-password",
        "server",
    )
    .unwrap()
    .unwrap()
    .access_token;
    let sibling = auth::authenticate(
        &state_dir,
        "viewer",
        "adaptive-hls-viewer-password",
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
    let items = request(&app, "GET", "/Items", Some(&admin)).await;
    let values: Value =
        serde_json::from_slice(&to_bytes(items.into_body(), 65_536).await.unwrap()).unwrap();
    let id = values["Items"][0]["Id"].as_str().unwrap();
    let route = format!("/Videos/{id}/master.m3u8?segmentLength=2&renditions=2");
    for (uri, expected) in [
        (
            format!("/Videos/{id}/master.m3u8?renditions=0"),
            StatusCode::BAD_REQUEST,
        ),
        (
            format!("/Videos/{id}/master.m3u8?renditions=5"),
            StatusCode::BAD_REQUEST,
        ),
        (
            format!("/Videos/{id}/main.m3u8?renditions=2"),
            StatusCode::NOT_IMPLEMENTED,
        ),
        (
            format!("/Audio/{id}/master.m3u8?renditions=2"),
            StatusCode::NOT_IMPLEMENTED,
        ),
    ] {
        assert_eq!(
            request(&app, "GET", &uri, Some(&owner)).await.status(),
            expected,
            "{uri}"
        );
    }
    assert_eq!(
        request(&app, "GET", &route, None).await.status(),
        StatusCode::UNAUTHORIZED
    );
    let master_response = request(&app, "GET", &route, Some(&owner)).await;
    assert_eq!(master_response.status(), StatusCode::OK);
    let master = text_body(master_response).await;
    assert_eq!(master.matches("#EXT-X-STREAM-INF:BANDWIDTH=").count(), 2);
    assert_eq!(master.matches(",RESOLUTION=640x360").count(), 2);
    assert!(!master.contains(&owner));
    assert!(master.contains("variant00/index.m3u8"));
    assert!(master.contains("variant01/index.m3u8"));

    // Both independently encoded variants start at the same requested
    // validated source offset; the cache key and private manifest differ
    // from the zero-offset publication.
    let seek_route =
        format!("/Videos/{id}/master.m3u8?segmentLength=2&renditions=2&startTimeTicks=10000000");
    let seek_response = request(&app, "GET", &seek_route, Some(&owner)).await;
    assert_eq!(seek_response.status(), StatusCode::OK);
    let seek_master = text_body(seek_response).await;
    assert_eq!(
        seek_master.matches("#EXT-X-STREAM-INF:BANDWIDTH=").count(),
        2
    );
    assert_eq!(seek_master.matches(",RESOLUTION=640x360").count(), 2);
    let seek_variants = seek_master
        .lines()
        .filter(|line| line.starts_with("hls1/"))
        .collect::<Vec<_>>();
    assert_eq!(seek_variants.len(), 2);
    assert_ne!(seek_master, master);
    let seek_pub = seek_variants[0].split('/').nth(1).unwrap();
    for (index, variant) in seek_variants.iter().enumerate() {
        let media_route = format!("/Videos/{id}/{variant}");
        assert_eq!(
            request(&app, "GET", &media_route, Some(&sibling))
                .await
                .status(),
            StatusCode::NOT_FOUND
        );
        let playlist_response = request(&app, "GET", &media_route, Some(&owner)).await;
        assert_eq!(playlist_response.status(), StatusCode::OK);
        let playlist = text_body(playlist_response).await;
        assert!(playlist.contains("#EXT-X-ENDLIST"));
        // A one-second seek into a 4.2-second synthetic source leaves
        // exactly two independently encoded, 2-second-target windows.
        assert_eq!(playlist.matches("#EXTINF:").count(), 2);
        let transport_route = format!("/Videos/{id}/hls1/{seek_pub}/variant{index:02}/0.ts");
        let transport = request(&app, "GET", &transport_route, Some(&owner)).await;
        assert_eq!(transport.status(), StatusCode::OK);
        let bytes = to_bytes(transport.into_body(), 8 * 1024 * 1024)
            .await
            .unwrap();
        assert!(!bytes.is_empty());
        assert!(bytes.chunks_exact(188).all(|packet| packet[0] == 0x47));
    }
    assert_eq!(
        text_body(request(&app, "GET", &seek_route, Some(&owner)).await).await,
        seek_master
    );
    assert_eq!(
        request(
            &app,
            "GET",
            &format!("/Videos/{id}/master.m3u8?segmentLength=2&renditions=2&startTimeTicks=-1"),
            Some(&owner),
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );
    let paths = master
        .lines()
        .filter(|line| line.starts_with("hls1/"))
        .collect::<Vec<_>>();
    assert_eq!(paths.len(), 2);
    let bandwidths = master
        .lines()
        .filter_map(|line| line.strip_prefix("#EXT-X-STREAM-INF:BANDWIDTH="))
        .map(|attributes| {
            attributes
                .split(',')
                .next()
                .unwrap()
                .parse::<u64>()
                .unwrap()
        })
        .collect::<Vec<_>>();
    assert!(bandwidths[0] > 0 && bandwidths[0] < bandwidths[1]);
    let playlist_id = paths[0].split('/').nth(1).unwrap().to_owned();
    assert_eq!(
        text_body(request(&app, "GET", &route, Some(&owner)).await).await,
        master
    );
    assert_playlist_head(&app, &route, &owner, master.len()).await;
    for (index, relative) in paths.iter().enumerate() {
        let media_route = format!("/Videos/{id}/{relative}");
        let wrong_route = format!("/Videos/{id}/hls1/{playlist_id}/variant99/index.m3u8");
        assert_eq!(
            request(&app, "GET", &wrong_route, Some(&owner))
                .await
                .status(),
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            request(&app, "GET", &media_route, Some(&sibling))
                .await
                .status(),
            StatusCode::NOT_FOUND
        );
        let media = request(&app, "GET", &media_route, Some(&owner)).await;
        assert_eq!(media.status(), StatusCode::OK);
        let manifest = text_body(media).await;
        assert_playlist_head(&app, &media_route, &owner, manifest.len()).await;
        assert!(manifest.contains("#EXT-X-ENDLIST"));
        assert!(manifest.contains("\n0.ts\n"));
        let segment_route = format!("/Videos/{id}/hls1/{playlist_id}/variant{index:02}/0.ts");
        let bytes_response = request(&app, "GET", &segment_route, Some(&owner)).await;
        assert_eq!(bytes_response.status(), StatusCode::OK);
        assert_eq!(bytes_response.headers()[header::CONTENT_TYPE], "video/mp2t");
        let bytes = to_bytes(bytes_response.into_body(), 8 * 1024 * 1024)
            .await
            .unwrap();
        assert!(bytes.len() >= 188 && bytes.len() % 188 == 0);
        assert!(bytes.chunks_exact(188).all(|packet| packet[0] == 0x47));
        let range = request_range(&app, "GET", &segment_route, &owner, "bytes=0-187").await;
        assert_eq!(range.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(range.headers()[header::CONTENT_LENGTH], "188");
        assert_eq!(to_bytes(range.into_body(), 188).await.unwrap().len(), 188);
    }
    auth::revoke_library(&state_dir, "viewer", &library).unwrap();
    let seek_first = format!("/Videos/{id}/{}", seek_variants[0]);
    assert_eq!(
        request(&app, "GET", &seek_first, Some(&owner))
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    let first = format!("/Videos/{id}/{}", paths[0]);
    assert_eq!(
        request(&app, "GET", &first, Some(&owner)).await.status(),
        StatusCode::NOT_FOUND
    );
    auth::grant_library(&state_dir, "viewer", &library).unwrap();
    assert_eq!(
        request(&app, "GET", &first, Some(&owner)).await.status(),
        StatusCode::OK
    );
    auth::revoke(&state_dir, &owner).unwrap();
    assert_eq!(
        request(&app, "GET", &first, Some(&owner)).await.status(),
        StatusCode::UNAUTHORIZED
    );
    drop(app);
    assert!(
        !std::env::temp_dir()
            .join(format!("pelagos-hls-{playlist_id}"))
            .exists()
    );
}
