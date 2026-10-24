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

async fn text_body(response: Response) -> String {
    let bytes = to_bytes(response.into_body(), 65536).await.unwrap();
    String::from_utf8(bytes.to_vec()).unwrap()
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
    let result = request(&app, "GET", &route, Some(&viewer)).await;
    assert_eq!(result.status(), StatusCode::OK);
    assert_eq!(
        result.headers()[header::CONTENT_TYPE],
        "application/vnd.apple.mpegurl"
    );
    assert_eq!(result.headers()[header::CACHE_CONTROL], "private, no-store");
    let manifest = text_body(result).await;
    assert!(manifest.starts_with("#EXTM3U\n"));
    assert!(manifest.contains("#EXT-X-DISCONTINUITY"));
    assert!(manifest.ends_with("#EXT-X-ENDLIST\n"));
    assert!(!manifest.contains(&viewer));
    assert!(!manifest.contains("segment00000.ts"));
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
    assert!(master.starts_with("#EXTM3U\n#EXT-X-VERSION:3\n"));
    assert!(master.contains("#EXT-X-STREAM-INF:BANDWIDTH="));
    assert!(!master.contains(&viewer));
    let media_name = master.lines().find(|l| l.starts_with("hls1/")).unwrap();
    assert!(media_name.ends_with("/index.m3u8"));
    let media_url = format!("/Videos/{id}/{media_name}");
    assert_eq!(
        request(&app, "GET", &media_url, Some(&admin)).await.status(),
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
        request(&app, "GET", &second_segment, Some(&viewer)).await.status(),
        StatusCode::OK
    );
    let media_head = request(&app, "HEAD", &media_url, Some(&viewer)).await;
    assert_eq!(media_head.status(), StatusCode::OK);
    assert_eq!(
        media_head.headers()[header::CONTENT_LENGTH].to_str().unwrap().parse::<usize>().unwrap(),
        media_text.len()
    );
    assert!(to_bytes(media_head.into_body(), 1_024).await.unwrap().is_empty());
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
    assert!(!std::env::temp_dir().join(format!("pelagos-hls-{master_id}")).exists());
}
