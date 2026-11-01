//! Synthetic auth/path-confinement and deterministic mutation regression.
//! Never accesses real Jellyfin state, media, credentials or user machines.
use std::{fs, sync::Arc};

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
use tower::ServiceExt;

async fn send(router: &Router, uri: &str, token: Option<&str>, range: Option<&str>) -> Response {
    let mut request = Request::builder().method("GET").uri(uri);
    if let Some(value) = token {
        request = request.header("X-Emby-Token", value);
    }
    if let Some(value) = range {
        request = request.header(header::RANGE, value);
    }
    router
        .clone()
        .oneshot(request.body(Body::empty()).unwrap())
        .await
        .unwrap()
}

#[cfg(unix)]
#[tokio::test]
async fn synthetic_library_permissions_path_escapes_and_range_mutations_fail_closed() {
    use std::os::unix::fs::symlink;

    let fixture = tempfile::tempdir().unwrap();
    let state = fixture.path().join("state");
    let allowed = fixture.path().join("Allowed");
    let hidden = fixture.path().join("Hidden");
    fs::create_dir_all(&allowed).unwrap();
    fs::create_dir_all(&hidden).unwrap();
    let content = b"AUTHORIZED_ONLY_MEDIA";
    let forbidden = b"HIDDEN_PRIVATE_MEDIA_SENTINEL";
    fs::write(allowed.join("clip.mp4"), content).unwrap();
    fs::write(hidden.join("private.mp4"), forbidden).unwrap();
    symlink(hidden.join("private.mp4"), allowed.join("linked.mp4")).unwrap();
    catalog::index(&state, &allowed).unwrap();
    catalog::index(&state, &hidden).unwrap();
    auth::create_user(&state, "admin", "test-admin-password-only").unwrap();
    auth::create_user(&state, "viewer", "test-viewer-password-only").unwrap();
    let admin = auth::authenticate(&state, "admin", "test-admin-password-only", "server")
        .unwrap()
        .unwrap()
        .access_token;
    let token = auth::authenticate(&state, "viewer", "test-viewer-password-only", "server")
        .unwrap()
        .unwrap()
        .access_token;
    auth::grant_library(&state, "viewer", &allowed).unwrap();
    let router = app::router(Arc::new(AppState::with_auth_dir(
        "server".to_owned(),
        state.clone(),
    )));
    let all = send(&router, "/Items", Some(&admin), None).await;
    assert_eq!(all.status(), StatusCode::OK);
    let all_body = to_bytes(all.into_body(), 65_536).await.unwrap();
    let listing: serde_json::Value = serde_json::from_slice(&all_body).unwrap();
    let items = listing["Items"].as_array().unwrap();
    assert_eq!(items.len(), 2);
    assert!(!items.iter().any(|item| item["Name"] == "linked"));
    let allowed_id = items.iter().find(|item| item["Name"] == "clip").unwrap()["Id"]
        .as_str()
        .unwrap()
        .to_owned();
    let hidden_id = items.iter().find(|item| item["Name"] == "private").unwrap()["Id"]
        .as_str()
        .unwrap()
        .to_owned();
    let stream = format!("/Videos/{allowed_id}/stream?static=true");
    let hidden_stream = format!("/Videos/{hidden_id}/stream?static=true");
    assert_eq!(
        send(&router, &stream, None, None).await.status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        send(&router, &hidden_stream, Some(&token), None)
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        send(
            &router,
            &format!("{hidden_stream}&ApiKey={token}"),
            None,
            None
        )
        .await
        .status(),
        StatusCode::UNAUTHORIZED
    );
    let good = send(&router, &stream, Some(&token), None).await;
    assert_eq!(good.status(), StatusCode::OK);
    assert_eq!(
        to_bytes(good.into_body(), 1024).await.unwrap().as_ref(),
        content
    );

    // Encoded dot/slash, path separators, NULs and very long paths must not
    // escape the catalog's indexed authorized-item boundary.
    for mutated in [
        "/Videos/%2e%2e/stream?static=true".to_owned(),
        "/Videos/%2e%2e%2fprivate.mp4/stream?static=true".to_owned(),
        "/Videos/%00/stream?static=true".to_owned(),
        "/Videos/%2fetc%2fpasswd/stream?static=true".to_owned(),
        format!("/Videos/{allowed_id}%2f..%2fprivate.mp4/stream?static=true"),
        format!("/Videos/{}/stream?static=true", "A".repeat(2_000)),
    ] {
        let response = send(&router, &mutated, Some(&token), None).await;
        assert!(!response.status().is_server_error(), "{mutated}");
        assert_ne!(response.status(), StatusCode::OK, "{mutated}");
        let bytes = to_bytes(response.into_body(), 4096).await.unwrap();
        assert!(!bytes.windows(forbidden.len()).any(|part| part == forbidden));
    }

    // A bounded deterministic input-mutation corpus runs against actual
    // HTTP range handling, not a fake parser, with no external fuzz binary.
    let mut seed = 0x15a6_b98d_d68c_0ff5_u64;
    let alphabet = b"bytes=0123456789-,xabcdefXYZ";
    let mut corpus = vec![
        "bytes=-".to_owned(),
        "bytes=1-0".to_owned(),
        "bytes=0-1,3-5".to_owned(),
        "bytes=18446744073709551616-".to_owned(),
        "bytes=0-18446744073709551616".to_owned(),
        "items=0-1".to_owned(),
        "bytes=-99999999999999999999".to_owned(),
    ];
    for _ in 0..192 {
        let mut value = String::new();
        for _ in 0..28 {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
            value.push(alphabet[(seed >> 32) as usize % alphabet.len()] as char);
        }
        corpus.push(value);
    }
    for value in corpus {
        let response = send(&router, &stream, Some(&token), Some(&value)).await;
        assert!(!response.status().is_server_error(), "fuzz range {value}");
        assert!(
            matches!(
                response.status(),
                StatusCode::OK | StatusCode::PARTIAL_CONTENT | StatusCode::RANGE_NOT_SATISFIABLE
            ),
            "unexpected range response {} for {value}",
            response.status()
        );
        let bytes = to_bytes(response.into_body(), 1024).await.unwrap();
        assert!(bytes.len() <= content.len());
        assert!(!bytes.windows(forbidden.len()).any(|part| part == forbidden));
    }
    auth::revoke_library(&state, "viewer", &allowed).unwrap();
    assert_eq!(
        send(&router, &stream, Some(&token), None).await.status(),
        StatusCode::NOT_FOUND
    );
}
