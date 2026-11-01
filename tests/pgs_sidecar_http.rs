//! Bounded raw external PGS SUP transport: synthetic indexed media, a
//! confidential adjacent graphical track, and read-only ranged GET/HEAD.
//! This does not claim bitmap decoding, embedded extraction or TV parity.
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

const PGS: &[u8] = &[
    // 32x32 presentation without bitmap objects, window and palette
    // metadata, followed by a display-set terminator.
    b'P', b'G', 0, 0, 0, 1, 0, 0, 0, 1, 0x16, 0, 11, 0, 32, 0, 32, 0x10, 0, 0, 0x80, 0, 0, 0, b'P',
    b'G', 0, 0, 0, 2, 0, 0, 0, 2, 0x17, 0, 10, 1, 0, 0, 0, 0, 0, 0, 32, 0, 32, b'P', b'G', 0, 0, 0,
    3, 0, 0, 0, 3, 0x14, 0, 2, 0, 0, b'P', b'G', 0, 0, 0, 4, 0, 0, 0, 4, 0x80, 0, 0,
];

async fn send(
    app: &Router,
    method: &str,
    path: &str,
    token: Option<&str>,
    extra: &[(&str, &str)],
) -> Response {
    let mut request = Request::builder().method(method).uri(path);
    if let Some(token) = token {
        request = request.header("X-Emby-Token", token);
    }
    for &(key, value) in extra {
        request = request.header(key, value);
    }
    app.clone()
        .oneshot(request.body(Body::empty()).unwrap())
        .await
        .unwrap()
}

#[tokio::test]
async fn native_pgs_sidecar_is_ranged_and_keeps_source_untouched() {
    if std::env::var_os("PELAGOS_REQUIRE_FFMPEG").is_none() {
        return;
    }
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("Movies");
    let state = temp.path().join("state");
    fs::create_dir_all(&root).unwrap();
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
            "color=s=32x32:r=5:d=4",
            "-c:v",
            "mpeg4",
            "-y",
        ])
        .arg(&movie)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let source_bytes = fs::read(&movie).unwrap();
    fs::write(root.join("Feature.es.sup"), PGS).unwrap();
    // A valid first packet must not hide trailing malformed PGS bytes.
    let mut malformed = PGS.to_vec();
    malformed.extend_from_slice(b"trailing non-PGS bytes");
    fs::write(root.join("Feature.fr.sup"), &malformed).unwrap();
    let mut bad_windows = PGS.to_vec();
    // The WDS declares two windows but provides only one record.
    bad_windows[13 + 11 + 13] = 2;
    fs::write(root.join("Feature.de.sup"), &bad_windows).unwrap();
    let mut unfinished_object = PGS[..24].to_vec();
    unfinished_object.extend_from_slice(&[
        b'P', b'G', 0, 0, 0, 2, 0, 0, 0, 2, 0x15, 0, 12, 0, 1, 0, 0x80, 0, 0, 6, 0, 8, 0, 8, 0xaa,
    ]);
    unfinished_object.extend_from_slice(&PGS[24..]);
    fs::write(root.join("Feature.pt.sup"), &unfinished_object).unwrap();
    let mut completed_object = PGS[..24].to_vec();
    completed_object.extend_from_slice(&[
        b'P', b'G', 0, 0, 0, 2, 0, 0, 0, 2, 0x15, 0, 12, 0, 1, 0, 0x80, 0, 0, 6, 0, 8, 0, 8, 0xaa,
    ]);
    completed_object.extend_from_slice(&[
        b'P', b'G', 0, 0, 0, 3, 0, 0, 0, 3, 0x15, 0, 5, 0, 1, 0, 0x40, 0xbb,
    ]);
    completed_object.extend_from_slice(&PGS[24..]);
    fs::write(root.join("Feature.nl.sup"), &completed_object).unwrap();
    // Unsupported large graphical tracks must not enter playback metadata.
    let giant = fs::File::create(root.join("Feature.it.sup")).unwrap();
    giant.set_len(32 * 1024 * 1024 + 1).unwrap();

    catalog::index(&state, &root).unwrap();
    auth::create_user(&state, "admin", "pgs-admin-password").unwrap();
    auth::create_user(&state, "viewer", "pgs-viewer-password").unwrap();
    let token = auth::authenticate(&state, "viewer", "pgs-viewer-password", "server")
        .unwrap()
        .unwrap()
        .access_token;
    auth::grant_library(&state, "viewer", &root).unwrap();
    let app = app::router(Arc::new(AppState::with_auth_dir(
        "server".to_owned(),
        state.clone(),
    )));
    let response = send(&app, "GET", "/Items", Some(&token), &[]).await;
    assert_eq!(response.status(), StatusCode::OK);
    let items: serde_json::Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 65_536).await.unwrap()).unwrap();
    let id = items["Items"][0]["Id"].as_str().unwrap();
    let response = send(
        &app,
        "GET",
        &format!("/Items/{id}/PlaybackInfo"),
        Some(&token),
        &[],
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let info: serde_json::Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 65_536).await.unwrap()).unwrap();
    let streams = info["MediaSources"][0]["MediaStreams"].as_array().unwrap();
    let pgs = streams
        .iter()
        .filter(|stream| stream["Codec"] == "pgssub")
        .collect::<Vec<_>>();
    assert_eq!(pgs.len(), 5);
    assert!(
        pgs.iter()
            .all(|stream| stream["IsExternal"] == true && stream["IsTextSubtitleStream"] == false)
    );
    assert!(streams.iter().all(|stream| stream["Language"] != "it"));
    let track = pgs.iter().find(|track| track["Language"] == "es").unwrap();
    let index = track["Index"].as_i64().unwrap();
    let url = format!("/Videos/{id}/{id}/Subtitles/{index}/Stream.pgssub");
    assert_eq!(track["DeliveryUrl"].as_str(), Some(url.as_str()));

    let body = serde_json::json!({
        "SubtitleStreamIndex": index,
        "DeviceProfile": {
            "DirectPlayProfiles": [{"Type":"Video","Container":"mkv","VideoCodec":"mpeg4"}],
            "SubtitleProfiles": [{"Method":"External","Format":"pgssub"}]
        }
    });
    let request = Request::builder()
        .method("POST")
        .uri(format!("/Items/{id}/PlaybackInfo"))
        .header("X-Emby-Token", token.as_str())
        .header("Content-Type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let negotiated: serde_json::Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 65_536).await.unwrap()).unwrap();
    assert_eq!(
        negotiated["MediaSources"][0]["DefaultSubtitleStreamIndex"],
        index
    );
    let incompatible = serde_json::json!({
        "SubtitleStreamIndex": index,
        "DeviceProfile": {
            "DirectPlayProfiles": [{"Type":"Video","Container":"mkv","VideoCodec":"mpeg4"}],
            "SubtitleProfiles": [{"Method":"External","Format":"srt"}]
        }
    });
    let request = Request::builder()
        .method("POST")
        .uri(format!("/Items/{id}/PlaybackInfo"))
        .header("X-Emby-Token", token.as_str())
        .header("Content-Type", "application/json")
        .body(Body::from(incompatible.to_string()))
        .unwrap();
    assert_eq!(
        app.clone().oneshot(request).await.unwrap().status(),
        StatusCode::NOT_IMPLEMENTED
    );

    assert_eq!(
        send(&app, "GET", &url, None, &[]).await.status(),
        StatusCode::UNAUTHORIZED
    );
    let response = send(&app, "GET", &url, Some(&token), &[]).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[header::ACCEPT_RANGES], "bytes");
    assert_eq!(
        response.headers()[header::CONTENT_TYPE],
        "application/octet-stream"
    );
    assert_eq!(
        response.headers()[header::CONTENT_LENGTH],
        PGS.len().to_string()
    );
    assert_eq!(
        response.headers()[header::CACHE_CONTROL],
        "private, no-store"
    );
    assert_eq!(response.headers()["x-content-type-options"], "nosniff");
    assert_eq!(
        to_bytes(response.into_body(), 1024).await.unwrap().as_ref(),
        PGS
    );

    for (range, expected) in [
        ("bytes=2-8", PGS[2..=8].to_vec()),
        ("bytes=15-", PGS[15..].to_vec()),
        ("bytes=-4", PGS[PGS.len() - 4..].to_vec()),
    ] {
        let response = send(&app, "GET", &url, Some(&token), &[("Range", range)]).await;
        assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT, "{range}");
        assert_eq!(
            response.headers()[header::CONTENT_LENGTH],
            expected.len().to_string()
        );
        let (start, end) = range
            .strip_prefix("bytes=")
            .unwrap()
            .split_once('-')
            .unwrap();
        let first = if start.is_empty() {
            PGS.len() - expected.len()
        } else {
            start.parse::<usize>().unwrap()
        };
        let last = if start.is_empty() || end.is_empty() {
            PGS.len() - 1
        } else {
            end.parse::<usize>().unwrap()
        };
        assert_eq!(
            response.headers()[header::CONTENT_RANGE],
            format!("bytes {first}-{last}/{}", PGS.len())
        );
        let response_body = to_bytes(response.into_body(), 1024).await.unwrap();
        assert_eq!(response_body.as_ref(), expected.as_slice(), "{range}");
    }
    let response = send(&app, "HEAD", &url, Some(&token), &[("Range", "bytes=2-8")]).await;
    assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(
        response.headers()[header::CONTENT_RANGE],
        format!("bytes 2-8/{}", PGS.len())
    );
    assert_eq!(response.headers()[header::CONTENT_LENGTH], "7");
    assert!(to_bytes(response.into_body(), 16).await.unwrap().is_empty());

    for bad in ["bytes=999-", "bytes=0-1,5-7", "bytes=-0"] {
        let response = send(&app, "GET", &url, Some(&token), &[("Range", bad)]).await;
        assert_eq!(
            response.status(),
            StatusCode::RANGE_NOT_SATISFIABLE,
            "{bad}"
        );
        assert_eq!(
            response.headers()[header::CONTENT_RANGE],
            format!("bytes */{}", PGS.len())
        );
    }
    let response = send(
        &app,
        "GET",
        &url,
        Some(&token),
        &[("Range", "bytes=2-8"), ("Range", "bytes=10-12")],
    )
    .await;
    assert_eq!(response.status(), StatusCode::RANGE_NOT_SATISFIABLE);
    let response = send(
        &app,
        "GET",
        &url,
        Some(&token),
        &[("Range", "bytes=2-8"), ("If-Range", "unknown")],
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        to_bytes(response.into_body(), 1024).await.unwrap().as_ref(),
        PGS
    );

    let alias = format!("/Videos/{id}/{id}/Subtitles/{index}/0/Stream.sup");
    assert_eq!(
        send(&app, "GET", &alias, Some(&token), &[]).await.status(),
        StatusCode::OK
    );
    // A properly completed pair of RLE object fragments stays deliverable
    // without any mutation or attempted bitmap decoding.
    let complete_index = pgs.iter().find(|track| track["Language"] == "nl").unwrap()["Index"]
        .as_i64()
        .unwrap();
    let complete_url = format!("/Videos/{id}/{id}/Subtitles/{complete_index}/Stream.pgssub");
    let complete = send(&app, "GET", &complete_url, Some(&token), &[]).await;
    assert_eq!(complete.status(), StatusCode::OK);
    assert_eq!(
        to_bytes(complete.into_body(), 1024).await.unwrap().as_ref(),
        completed_object.as_slice()
    );
    for language in ["fr", "de", "pt"] {
        let invalid_index = pgs
            .iter()
            .find(|track| track["Language"] == language)
            .unwrap()["Index"]
            .as_i64()
            .unwrap();
        let invalid = format!("/Videos/{id}/{id}/Subtitles/{invalid_index}/Stream.pgssub");
        assert_eq!(
            send(&app, "GET", &invalid, Some(&token), &[])
                .await
                .status(),
            StatusCode::NOT_IMPLEMENTED,
            "{language}"
        );
    }
    assert_eq!(
        send(
            &app,
            "GET",
            &format!("/Videos/{id}/wrong/Subtitles/{index}/Stream.pgssub"),
            Some(&token),
            &[]
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(fs::read(root.join("Feature.fr.sup")).unwrap(), malformed);
    assert_eq!(fs::read(root.join("Feature.de.sup")).unwrap(), bad_windows);
    assert_eq!(
        fs::read(root.join("Feature.pt.sup")).unwrap(),
        unfinished_object
    );
    assert_eq!(
        fs::read(root.join("Feature.nl.sup")).unwrap(),
        completed_object
    );
    assert_eq!(fs::read(root.join("Feature.es.sup")).unwrap(), PGS);
    assert_eq!(fs::read(&movie).unwrap(), source_bytes);
    auth::revoke_library(&state, "viewer", &root).unwrap();
    assert_eq!(
        send(&app, "GET", &url, Some(&token), &[]).await.status(),
        StatusCode::NOT_FOUND
    );
}
