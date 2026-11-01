use std::sync::Arc;

use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode, header},
    response::Response,
};
use pelagos::{
    app::{self, AppState},
    auth,
};
use tower::ServiceExt;

async fn request(
    router: &Router,
    method: &str,
    path: &str,
    token: Option<&str>,
    body: &str,
) -> Response {
    let mut builder = Request::builder()
        .uri(path)
        .method(method)
        .header(header::CONTENT_TYPE, "application/json");
    if let Some(token) = token {
        builder = builder.header("X-Emby-Token", token);
    }
    router
        .clone()
        .oneshot(builder.body(Body::from(body.to_owned())).unwrap())
        .await
        .unwrap()
}

#[tokio::test]
async fn playback_requires_auth_and_rejects_unsupported_modes() {
    let temp = tempfile::tempdir().unwrap();
    auth::create_user(temp.path(), "admin", "long-private-test-password").unwrap();
    auth::create_user(temp.path(), "viewer", "long-viewer-test-password").unwrap();
    let admin = auth::authenticate(temp.path(), "admin", "long-private-test-password", "server")
        .unwrap()
        .unwrap()
        .access_token;
    let viewer = auth::authenticate(temp.path(), "viewer", "long-viewer-test-password", "server")
        .unwrap()
        .unwrap()
        .access_token;
    let router = app::router(Arc::new(AppState::with_auth_dir(
        "server".to_owned(),
        temp.path().to_path_buf(),
    )));
    let path = "/Items/00000000000000000000000000000001/PlaybackInfo";
    assert_eq!(
        request(&router, "GET", path, None, "",).await.status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        request(&router, "GET", path, Some(&viewer), "")
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        request(
            &router,
            "POST",
            path,
            Some(&admin),
            r#"{"EnableDirectPlay":false}"#,
        )
        .await
        .status(),
        StatusCode::NOT_IMPLEMENTED
    );
    assert_eq!(
        request(&router, "GET", path, Some(&admin), "")
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
}

#[cfg(unix)]
#[tokio::test]
async fn indexed_video_playback_metadata_comes_from_bounded_ffprobe_fixture() {
    use std::{fs, os::unix::fs::PermissionsExt};

    use axum::body::to_bytes;
    use pelagos::catalog;
    use serde_json::Value;

    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("Movies");
    let state_dir = temp.path().join("state");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("Feature.mp4"), b"synthetic ffprobe fixture").unwrap();
    catalog::index(&state_dir, &root).unwrap();

    let executable = temp.path().join("fake-ffprobe");
    fs::write(
        &executable,
        "#!/bin/sh\nprintf '%s' '{\"format\":{\"duration\":\"61.25\",\"bit_rate\":\"4000000\"},\"streams\":[{\"index\":0,\"codec_type\":\"video\",\"codec_name\":\"h264\",\"profile\":\"High\",\"avg_frame_rate\":\"30000/1001\",\"r_frame_rate\":\"30/1\",\"pix_fmt\":\"yuv420p\",\"color_primaries\":\"bt709\",\"width\":640,\"height\":360},{\"index\":1,\"codec_type\":\"audio\",\"codec_name\":\"aac\",\"sample_rate\":\"48000\",\"channels\":2}]}'\n",
    )
    .unwrap();
    fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();

    auth::create_user(&state_dir, "admin", "strong-playback-test-password").unwrap();
    let token = auth::authenticate(
        &state_dir,
        "admin",
        "strong-playback-test-password",
        "server",
    )
    .unwrap()
    .unwrap()
    .access_token;
    let current_user = auth::authenticate(
        &state_dir,
        "admin",
        "strong-playback-test-password",
        "server",
    )
    .unwrap()
    .unwrap()
    .user
    .id;
    let sibling_token = auth::authenticate(
        &state_dir,
        "admin",
        "strong-playback-test-password",
        "server",
    )
    .unwrap()
    .unwrap()
    .access_token;
    let app_state =
        AppState::with_auth_dir("server".to_owned(), state_dir).with_probe_executable(executable);
    let router = app::router(Arc::new(app_state));

    let catalog_response = request(&router, "GET", "/Items", Some(&token), "").await;
    assert_eq!(catalog_response.status(), StatusCode::OK);
    let bytes = to_bytes(catalog_response.into_body(), 65536).await.unwrap();
    let items: Value = serde_json::from_slice(&bytes).unwrap();
    let id = items["Items"][0]["Id"].as_str().unwrap();
    let route = format!("/Items/{id}/PlaybackInfo");

    let response = request(&router, "GET", &route, Some(&token), "").await;
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = to_bytes(response.into_body(), 65536).await.unwrap();
    let info: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(info["MediaSources"][0]["Protocol"], "File");
    assert_eq!(info["MediaSources"][0]["Container"], "mp4");
    assert_eq!(info["MediaSources"][0]["Bitrate"], 4_000_000);
    assert_eq!(info["MediaSources"][0]["VideoType"], "VideoFile");
    assert_eq!(info["MediaSources"][0]["DefaultAudioStreamIndex"], 1);

    assert_eq!(info["MediaSources"][0]["RunTimeTicks"], 612_500_000);
    assert_eq!(info["MediaSources"][0]["MediaStreams"][0]["Codec"], "h264");
    assert_eq!(
        info["MediaSources"][0]["MediaStreams"][0]["Profile"],
        "High"
    );
    assert_eq!(
        info["MediaSources"][0]["MediaStreams"][0]["PixelFormat"],
        "yuv420p"
    );
    assert_eq!(
        info["MediaSources"][0]["MediaStreams"][0]["ColorPrimaries"],
        "bt709"
    );
    let fps = info["MediaSources"][0]["MediaStreams"][0]["AverageFrameRate"]
        .as_f64()
        .unwrap();
    assert!((fps - 29.970_029_97).abs() < 1e-7);
    assert_eq!(
        info["MediaSources"][0]["MediaStreams"][1]["SampleRate"],
        48000
    );
    assert_eq!(info["MediaSources"][0]["SupportsTranscoding"], false);
    assert_eq!(info["MediaSources"][0]["SupportsDirectPlay"], true);
    assert!(info["MediaSources"][0].get("Path").is_none());
    assert_eq!(
        info["MediaSources"][0]["DirectStreamUrl"],
        format!("/Videos/{id}/stream?static=true")
    );

    let post = request(
        &router,
        "POST",
        &route,
        Some(&token),
        r#"{"EnableDirectPlay":true,"EnableTranscoding":false}"#,
    )
    .await;
    assert_eq!(post.status(), StatusCode::OK);
    let defaults = request(
        &router,
        "POST",
        &route,
        Some(&token),
        r#"{"EnableDirectPlay":true,"EnableDirectStream":true,"EnableTranscoding":true}"#,
    )
    .await;
    assert_eq!(defaults.status(), StatusCode::OK);
    let body = to_bytes(defaults.into_body(), 65536).await.unwrap();
    let response: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(response["MediaSources"][0]["SupportsDirectStream"], false);
    assert_eq!(response["MediaSources"][0]["SupportsTranscoding"], false);
    // Pinned v12.3 DirectPlayProfile request: matching static container
    // and codecs may be offered; unsupported profiles must not be advertised.
    for (profile, expected) in [
        (
            serde_json::json!({
                "DirectPlayProfiles": [{
                    "Type":"Video", "Container":"mp4,mkv",
                    "VideoCodec":"h264", "AudioCodec":"aac"
                }]
            }),
            StatusCode::OK,
        ),
        (
            serde_json::json!({
                "DirectPlayProfiles": [{
                    "Type":"Video", "Container":"mkv",
                    "VideoCodec":"h264", "AudioCodec":"aac"
                }]
            }),
            StatusCode::NOT_IMPLEMENTED,
        ),
        (
            serde_json::json!({
                "DirectPlayProfiles": [{
                    "Type":"Video", "Container":"mp4",
                    "VideoCodec":"hevc", "AudioCodec":"aac"
                }]
            }),
            StatusCode::NOT_IMPLEMENTED,
        ),
        (
            serde_json::json!({
                "DirectPlayProfiles": [{
                    "Type":"Video", "Container":"mp4",
                    "VideoCodec":"h264", "AudioCodec":"ac3"
                }]
            }),
            StatusCode::NOT_IMPLEMENTED,
        ),
        (
            serde_json::json!({
                "DirectPlayProfiles": [{
                    "Type":"Video", "Container":"mp4"
                }],
                "CodecProfiles":[{"Type":"Video"}]
            }),
            StatusCode::NOT_IMPLEMENTED,
        ),
        (
            serde_json::json!({
                "DirectPlayProfiles": [{
                    "Type":"Video", "Container":"mp4", "UnsupportedField":true
                }]
            }),
            StatusCode::BAD_REQUEST,
        ),
    ] {
        let response = request(
            &router,
            "POST",
            &route,
            Some(&token),
            &serde_json::json!({"DeviceProfile":profile}).to_string(),
        )
        .await;
        assert_eq!(response.status(), expected);
    }
    // Session-declared capabilities are used even without a profile repeated
    // in the playback body; explicit per-request profiles take precedence.
    let allowed_profile = serde_json::json!({
        "DirectPlayProfiles":[
            {"Type":"Video","Container":"mp4","VideoCodec":"h264","AudioCodec":"aac"}
        ]
    });
    let denied_profile = serde_json::json!({
        "DirectPlayProfiles":[{"Type":"Video","Container":"mkv"}]
    });
    assert_eq!(
        request(
            &router,
            "POST",
            "/Sessions/Capabilities/Full",
            Some(&token),
            &serde_json::json!({"DeviceProfile":allowed_profile}).to_string(),
        )
        .await
        .status(),
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        request(&router, "POST", &route, Some(&token), "{}")
            .await
            .status(),
        StatusCode::OK
    );
    assert_eq!(
        request(&router, "GET", &route, Some(&token), "")
            .await
            .status(),
        StatusCode::OK
    );
    assert_eq!(
        request(
            &router,
            "POST",
            "/Sessions/Capabilities/Full",
            Some(&token),
            &serde_json::json!({"DeviceProfile":denied_profile}).to_string(),
        )
        .await
        .status(),
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        request(&router, "POST", &route, Some(&token), "{}")
            .await
            .status(),
        StatusCode::NOT_IMPLEMENTED
    );
    assert_eq!(
        request(&router, "POST", &route, Some(&sibling_token), "{}")
            .await
            .status(),
        StatusCode::OK
    );
    assert_eq!(
        request(
            &router,
            "POST",
            &route,
            Some(&token),
            &serde_json::json!({"DeviceProfile":allowed_profile}).to_string(),
        )
        .await
        .status(),
        StatusCode::OK
    );
    // The profile fallback is stored on the authenticated device session,
    // never inherited by another token, and loses to an explicit request.
    let low_bandwidth_profile = serde_json::json!({
        "DeviceProfile": {
            "MaxStreamingBitrate": 3_000_000,
            "DirectPlayProfiles": [
                {"Type":"Video","Container":"mp4","VideoCodec":"h264","AudioCodec":"aac"}
            ]
        }
    });
    assert_eq!(
        request(
            &router,
            "POST",
            "/Sessions/Capabilities/Full",
            Some(&token),
            &low_bandwidth_profile.to_string(),
        )
        .await
        .status(),
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        request(&router, "POST", &route, Some(&token), "{}")
            .await
            .status(),
        StatusCode::NOT_IMPLEMENTED
    );
    assert_eq!(
        request(&router, "GET", &route, Some(&token), "")
            .await
            .status(),
        StatusCode::NOT_IMPLEMENTED
    );
    assert_eq!(
        request(
            &router,
            "POST",
            &route,
            Some(&token),
            r#"{"MaxStreamingBitrate":4000000}"#,
        )
        .await
        .status(),
        StatusCode::OK
    );
    assert_eq!(
        request(&router, "POST", &route, Some(&sibling_token), "{}")
            .await
            .status(),
        StatusCode::OK
    );
    assert_eq!(
        request(
            &router,
            "POST",
            "/Sessions/Capabilities/Full",
            Some(&token),
            "{}"
        )
        .await
        .status(),
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        request(&router, "POST", &route, Some(&token), "{}")
            .await
            .status(),
        StatusCode::OK
    );

    // No transcoder is available to satisfy a requested bandwidth or
    // output-channel ceiling. A static source is only offered when measured
    // FFprobe properties already fit the limits.
    for (body, expected) in [
        (
            serde_json::json!({"MaxStreamingBitrate":4_000_000,"MaxAudioChannels":2}),
            StatusCode::OK,
        ),
        (
            serde_json::json!({"MaxStreamingBitrate":3_000_000}),
            StatusCode::NOT_IMPLEMENTED,
        ),
        (
            serde_json::json!({"MaxAudioChannels":1}),
            StatusCode::NOT_IMPLEMENTED,
        ),
        (
            serde_json::json!({"MaxAudioChannels":0}),
            StatusCode::BAD_REQUEST,
        ),
    ] {
        assert_eq!(
            request(&router, "POST", &route, Some(&token), &body.to_string())
                .await
                .status(),
            expected
        );
    }
    assert_eq!(
        request(
            &router,
            "POST",
            &format!("{route}?maxStreamingBitrate=4000000&maxAudioChannels=2"),
            Some(&token),
            r#"{"MaxStreamingBitrate":3000000,"MaxAudioChannels":1}"#
        )
        .await
        .status(),
        StatusCode::OK
    );

    // The explicit playback bitrate wins over the profile's fallback;
    // the profile itself remains authoritative when no value was posted.
    for (body, expected) in [
        (
            serde_json::json!({
                "DeviceProfile": {
                    "MaxStreamingBitrate": 3_000_000,
                    "DirectPlayProfiles": [{"Type": "Video", "Container": "mp4"}]
                }
            }),
            StatusCode::NOT_IMPLEMENTED,
        ),
        (
            serde_json::json!({
                "MaxStreamingBitrate": 4_000_000,
                "DeviceProfile": {
                    "MaxStreamingBitrate": 3_000_000,
                    "DirectPlayProfiles": [{"Type": "Video", "Container": "mp4"}]
                }
            }),
            StatusCode::OK,
        ),
        (
            serde_json::json!({
                "DeviceProfile": {
                    "MaxStreamingBitrate": 0,
                    "DirectPlayProfiles": [{"Type": "Video", "Container": "mp4"}]
                }
            }),
            StatusCode::BAD_REQUEST,
        ),
        (
            serde_json::json!({
                "MaxStreamingBitrate": 4_000_000,
                "DeviceProfile": {
                    "MaxStreamingBitrate": 0,
                    "DirectPlayProfiles": [{"Type": "Video", "Container": "mp4"}]
                }
            }),
            StatusCode::OK,
        ),
    ] {
        assert_eq!(
            request(&router, "POST", &route, Some(&token), &body.to_string())
                .await
                .status(),
            expected
        );
    }
    assert_eq!(
        request(
            &router,
            "POST",
            &format!("{route}?maxStreamingBitrate=4000000"),
            Some(&token),
            r#"{"DeviceProfile":{"MaxStreamingBitrate":3000000,"DirectPlayProfiles":[{"Type":"Video","Container":"mp4"}]}}"#,
        )
        .await
        .status(),
        StatusCode::OK
    );

    let accepted_user = format!("{route}?userId={current_user}");
    assert_eq!(
        request(&router, "GET", &accepted_user, Some(&token), "")
            .await
            .status(),
        StatusCode::OK
    );
    assert_eq!(
        request(
            &router,
            "GET",
            &format!("{route}?userId=someone-else"),
            Some(&token),
            ""
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        request(
            &router,
            "POST",
            &route,
            Some(&token),
            r#"{"UserId":"someone-else","EnableDirectPlay":true}"#,
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    // Deprecated query arguments take precedence over POST body arguments.
    assert_eq!(
        request(
            &router,
            "POST",
            &accepted_user,
            Some(&token),
            r#"{"UserId":"someone-else","EnableDirectPlay":true}"#,
        )
        .await
        .status(),
        StatusCode::OK
    );
    assert_eq!(
        request(
            &router,
            "POST",
            &format!("{route}?enableDirectPlay=true"),
            Some(&token),
            r#"{"EnableDirectPlay":false}"#,
        )
        .await
        .status(),
        StatusCode::OK
    );
    assert_eq!(
        request(
            &router,
            "POST",
            &format!("{route}?startTimeTicks=0"),
            Some(&token),
            r#"{"StartTimeTicks":1000}"#,
        )
        .await
        .status(),
        StatusCode::OK
    );
    assert_eq!(
        request(
            &router,
            "POST",
            &format!("{route}?audioStreamIndex=1&subtitleStreamIndex=-1"),
            Some(&token),
            r#"{"AudioStreamIndex":99,"SubtitleStreamIndex":3}"#,
        )
        .await
        .status(),
        StatusCode::OK
    );
    for (uri, expected) in [
        (
            format!("{route}?startTimeTicks=-1"),
            StatusCode::BAD_REQUEST,
        ),
        (
            format!("{route}?startTimeTicks=1000"),
            StatusCode::NOT_IMPLEMENTED,
        ),
        (format!("{route}?audioStreamIndex=1"), StatusCode::OK),
        (
            format!("{route}?audioStreamIndex=0"),
            StatusCode::NOT_IMPLEMENTED,
        ),
        (format!("{route}?subtitleStreamIndex=-1"), StatusCode::OK),
        (
            format!("{route}?subtitleStreamIndex=0"),
            StatusCode::NOT_IMPLEMENTED,
        ),
        (
            format!("{route}?autoOpenLiveStream=true"),
            StatusCode::NOT_IMPLEMENTED,
        ),
        (
            format!("{route}?mediaSourceId=unmatched"),
            StatusCode::NOT_IMPLEMENTED,
        ),
        (
            format!("{route}?enableDirectPlay=false"),
            StatusCode::NOT_IMPLEMENTED,
        ),
    ] {
        assert_eq!(
            request(&router, "POST", &uri, Some(&token), "{}")
                .await
                .status(),
            expected,
            "{uri}"
        );
    }
    let rejected = request(
        &router,
        "POST",
        &route,
        Some(&token),
        r#"{"EnableDirectPlay":false}"#,
    )
    .await;
    assert_eq!(rejected.status(), StatusCode::NOT_IMPLEMENTED);
}

#[cfg(unix)]
#[tokio::test]
async fn audio_playback_info_is_scoped_and_reports_direct_source_only() {
    use std::{fs, os::unix::fs::PermissionsExt};

    use axum::body::to_bytes;
    use pelagos::catalog;
    use serde_json::Value;

    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("Music");
    let state_dir = temp.path().join("state");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("Track.flac"), b"synthetic audio").unwrap();
    catalog::index(&state_dir, &root).unwrap();

    let executable = temp.path().join("fake-ffprobe");
    fs::write(
        &executable,
        "#!/bin/sh\nprintf '%s' '{\"format\":{\"duration\":\"125.5\"},\"streams\":[{\"index\":0,\"codec_type\":\"audio\",\"codec_name\":\"flac\",\"sample_rate\":\"44100\",\"channels\":2}]}'\n",
    )
    .unwrap();
    fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();

    auth::create_user(&state_dir, "admin", "long-audio-admin-password").unwrap();
    auth::create_user(&state_dir, "listener", "long-audio-user-password").unwrap();
    let admin = auth::authenticate(&state_dir, "admin", "long-audio-admin-password", "server")
        .unwrap()
        .unwrap()
        .access_token;
    let viewer = auth::authenticate(&state_dir, "listener", "long-audio-user-password", "server")
        .unwrap()
        .unwrap()
        .access_token;
    let router = app::router(Arc::new(
        AppState::with_auth_dir("server".to_owned(), state_dir.clone())
            .with_probe_executable(executable),
    ));
    let items = request(&router, "GET", "/Items", Some(&admin), "").await;
    let body = to_bytes(items.into_body(), 65536).await.unwrap();
    let response: Value = serde_json::from_slice(&body).unwrap();
    let id = response["Items"][0]["Id"].as_str().unwrap();
    let route = format!("/Items/{id}/PlaybackInfo");

    assert_eq!(
        request(&router, "GET", &route, Some(&viewer), "")
            .await
            .status(),
        StatusCode::NOT_FOUND
    );

    auth::grant_library(&state_dir, "listener", &root).unwrap();
    let info = request(
        &router,
        "POST",
        &route,
        Some(&viewer),
        r#"{"EnableDirectPlay":true,"EnableDirectStream":true,"EnableTranscoding":true}"#,
    )
    .await;
    assert_eq!(info.status(), StatusCode::OK);
    let bytes = to_bytes(info.into_body(), 65536).await.unwrap();
    let payload: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(
        payload["MediaSources"][0]["DirectStreamUrl"],
        format!("/Audio/{id}/stream?static=true")
    );
    assert_eq!(payload["MediaSources"][0]["RunTimeTicks"], 1_255_000_000);
    assert_eq!(
        payload["MediaSources"][0]["MediaStreams"][0]["Codec"],
        "flac"
    );
    assert_eq!(payload["MediaSources"][0]["SupportsDirectPlay"], true);
    assert_eq!(payload["MediaSources"][0]["SupportsTranscoding"], false);

    for (profile, expected) in [
        (
            serde_json::json!({
                "DirectPlayProfiles":[{"Type":"Audio","Container":"flac","AudioCodec":"flac"}]
            }),
            StatusCode::OK,
        ),
        (
            serde_json::json!({
                "DirectPlayProfiles":[{"Type":0,"AudioCodec":"flac"}]
            }),
            StatusCode::OK,
        ),
        (
            serde_json::json!({
                "DirectPlayProfiles":[{"AudioCodec":"flac"}]
            }),
            StatusCode::OK,
        ),
        (
            serde_json::json!({
                "DirectPlayProfiles":[{"Type":"Audio","Container":"mp3","AudioCodec":"flac"}]
            }),
            StatusCode::NOT_IMPLEMENTED,
        ),
        (
            serde_json::json!({
                "DirectPlayProfiles":[{"Type":"Audio","Container":"flac","AudioCodec":"aac"}]
            }),
            StatusCode::NOT_IMPLEMENTED,
        ),
    ] {
        assert_eq!(
            request(
                &router,
                "POST",
                &route,
                Some(&viewer),
                &serde_json::json!({"DeviceProfile":profile}).to_string(),
            )
            .await
            .status(),
            expected
        );
    }

    auth::revoke_library(&state_dir, "listener", &root).unwrap();
    assert_eq!(
        request(&router, "GET", &route, Some(&viewer), "")
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
}

#[cfg(unix)]
#[tokio::test]
async fn read_only_srt_sidecars_are_scoped_and_opened_inside_the_media_directory() {
    use axum::body::to_bytes;
    use pelagos::catalog;
    use std::{
        fs,
        os::unix::fs::{PermissionsExt, symlink},
    };

    let fixture = tempfile::tempdir().unwrap();
    let library = fixture.path().join("Movies");
    let state_dir = fixture.path().join("state");
    fs::create_dir_all(&library).unwrap();
    fs::write(library.join("Feature.mp4"), b"video fixture").unwrap();
    let subtitle = b"1\n00:00:01,000 --> 00:00:02,000\nHello!\n";
    fs::write(library.join("Feature.eng.srt"), subtitle).unwrap();
    let webvtt = b"WEBVTT\n\n00:00:01.000 --> 00:00:02.000\nBonjour!\n";
    fs::write(library.join("Feature.fr.vtt"), webvtt).unwrap();
    let outside = fixture.path().join("outside.srt");
    fs::write(&outside, b"SECRET OUTSIDE LIBRARY").unwrap();
    symlink(&outside, library.join("Feature.es.srt")).unwrap();
    catalog::index(&state_dir, &library).unwrap();

    let ffprobe = fixture.path().join("fake-ffprobe");
    fs::write(
        &ffprobe,
        "#!/bin/sh\nprintf '%s' '{\"format\":{\"duration\":\"4\"},\"streams\":[{\"index\":0,\"codec_type\":\"video\",\"codec_name\":\"h264\"},{\"index\":1,\"codec_type\":\"audio\",\"codec_name\":\"aac\"}]}'\n",
    ).unwrap();
    fs::set_permissions(&ffprobe, fs::Permissions::from_mode(0o700)).unwrap();
    auth::create_user(&state_dir, "admin", "subtitle-admin-password").unwrap();
    auth::create_user(&state_dir, "viewer", "subtitle-viewer-password").unwrap();
    let admin = auth::authenticate(&state_dir, "admin", "subtitle-admin-password", "server")
        .unwrap()
        .unwrap()
        .access_token;
    let viewer = auth::authenticate(&state_dir, "viewer", "subtitle-viewer-password", "server")
        .unwrap()
        .unwrap()
        .access_token;
    auth::grant_library(&state_dir, "viewer", &library).unwrap();
    let app = app::router(Arc::new(
        AppState::with_auth_dir("server".to_owned(), state_dir.clone())
            .with_probe_executable(ffprobe),
    ));
    let response = request(&app, "GET", "/Items", Some(&admin), "").await;
    let listing: serde_json::Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 65_536).await.unwrap()).unwrap();
    let item = listing["Items"][0]["Id"].as_str().unwrap();
    let info = request(
        &app,
        "GET",
        &format!("/Items/{item}/PlaybackInfo"),
        Some(&viewer),
        "",
    )
    .await;
    assert_eq!(info.status(), StatusCode::OK);
    let value: serde_json::Value =
        serde_json::from_slice(&to_bytes(info.into_body(), 65_536).await.unwrap()).unwrap();
    let streams = value["MediaSources"][0]["MediaStreams"].as_array().unwrap();
    assert_eq!(streams.len(), 4);
    assert_eq!(streams[2]["Index"], 2);
    assert_eq!(streams[2]["Language"], "eng");
    assert_eq!(streams[2]["IsExternal"], true);
    assert_eq!(streams[2]["IsTextSubtitleStream"], true);
    assert_eq!(streams[3]["Index"], 3);
    assert_eq!(streams[3]["Codec"], "webvtt");
    assert_eq!(streams[3]["Language"], "fr");
    assert_eq!(streams[3]["IsExternal"], true);
    let supported_profile = serde_json::json!({
        "DeviceProfile":{
            "DirectPlayProfiles":[{"Type":"Video","Container":"mp4"}],
            "SubtitleProfiles":[
                {"Method":"External","Format":"srt","Language":"eng"},
                {"Method":"External","Format":"vtt","Language":"fr"}
            ]
        }
    });
    assert_eq!(
        request(
            &app,
            "POST",
            &format!("/Items/{item}/PlaybackInfo"),
            Some(&viewer),
            &supported_profile.to_string()
        )
        .await
        .status(),
        StatusCode::OK
    );
    let blocked_profile = serde_json::json!({
        "DeviceProfile":{
            "DirectPlayProfiles":[{"Type":"Video","Container":"mp4"}],
            "SubtitleProfiles":[{"Method":"Embed","Format":"srt"}]
        }
    });
    assert_eq!(
        request(
            &app,
            "POST",
            &format!("/Items/{item}/PlaybackInfo"),
            Some(&viewer),
            &blocked_profile.to_string()
        )
        .await
        .status(),
        StatusCode::NOT_IMPLEMENTED
    );
    let url = streams[2]["DeliveryUrl"].as_str().unwrap();
    assert_eq!(url, format!("/Videos/{item}/{item}/Subtitles/2/Stream.srt"));
    assert_eq!(
        request(&app, "GET", url, None, "").await.status(),
        StatusCode::UNAUTHORIZED
    );
    let response = request(&app, "GET", url, Some(&viewer), "").await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers()[header::CONTENT_TYPE],
        "application/x-subrip"
    );
    assert_eq!(
        to_bytes(response.into_body(), 1_024)
            .await
            .unwrap()
            .as_ref(),
        subtitle
    );
    let response = request(&app, "HEAD", url, Some(&viewer), "").await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers()[header::CONTENT_LENGTH],
        subtitle.len().to_string()
    );
    assert!(
        to_bytes(response.into_body(), 1_024)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        request(
            &app,
            "GET",
            &format!("/Videos/{item}/wrong/Subtitles/2/Stream.srt"),
            Some(&viewer),
            ""
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        request(
            &app,
            "GET",
            &format!("/Videos/{item}/{item}/Subtitles/3/Stream.srt"),
            Some(&viewer),
            ""
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
    // The same authorized external SRT index can be requested as WebVTT.
    let converted_url = format!("/Videos/{item}/{item}/Subtitles/2/Stream.vtt");
    let response = request(&app, "GET", &converted_url, Some(&viewer), "").await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers()[header::CONTENT_TYPE],
        "text/vtt; charset=utf-8"
    );
    let expected_vtt = b"WEBVTT\n\n00:00:01.000 --> 00:00:02.000\nHello!\n\n";
    assert_eq!(
        to_bytes(response.into_body(), 1_024)
            .await
            .unwrap()
            .as_ref(),
        expected_vtt
    );
    let response = request(&app, "HEAD", &converted_url, Some(&viewer), "").await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers()[header::CONTENT_LENGTH],
        expected_vtt.len().to_string()
    );
    assert!(
        to_bytes(response.into_body(), 1_024)
            .await
            .unwrap()
            .is_empty()
    );
    let vtt_url = streams[3]["DeliveryUrl"].as_str().unwrap();
    assert_eq!(
        vtt_url,
        format!("/Videos/{item}/{item}/Subtitles/3/Stream.vtt")
    );
    let response = request(&app, "GET", vtt_url, Some(&viewer), "").await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers()[header::CONTENT_TYPE],
        "text/vtt; charset=utf-8"
    );
    assert_eq!(
        to_bytes(response.into_body(), 1_024)
            .await
            .unwrap()
            .as_ref(),
        webvtt
    );
    assert_eq!(
        request(
            &app,
            "GET",
            &format!("/Videos/{item}/{item}/Subtitles/2/Stream.vtt"),
            Some(&viewer),
            "",
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
    auth::revoke_library(&state_dir, "viewer", &library).unwrap();
    assert_eq!(
        request(&app, "GET", url, Some(&viewer), "").await.status(),
        StatusCode::NOT_FOUND
    );
}
