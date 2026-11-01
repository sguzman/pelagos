//! Authenticated, explicitly opted-in fallback fonts for ASS clients.
//! All fixtures are synthetic header-bearing bytes, not renderable fonts.
use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
    response::Response,
};
use pelagos::{
    app::{self, AppState},
    auth,
};
use std::{fs, sync::Arc};
use tower::ServiceExt;

async fn request(app: &Router, method: &str, url: &str, token: Option<&str>) -> Response {
    let mut request = Request::builder().method(method).uri(url);
    if let Some(token) = token {
        request = request.header("X-Emby-Token", token);
    }
    app.clone()
        .oneshot(request.body(Body::empty()).unwrap())
        .await
        .unwrap()
}

#[tokio::test]
async fn fallback_fonts_are_opt_in_bounded_and_scoped_to_authenticated_users() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("fonts");
    let state = temp.path().join("state");
    fs::create_dir_all(&root).unwrap();
    let mut ttf = vec![0_u8; 28];
    ttf[..4].copy_from_slice(&[0, 1, 0, 0]);
    ttf[4..6].copy_from_slice(&1_u16.to_be_bytes());
    let mut otf = vec![0_u8; 28];
    otf[..4].copy_from_slice(b"OTTO");
    otf[4..6].copy_from_slice(&1_u16.to_be_bytes());
    fs::write(root.join("Family.ttf"), &ttf).unwrap();
    fs::write(root.join("Alternative.otf"), &otf).unwrap();
    fs::write(root.join("Spoof.ttf"), b"not a font").unwrap();
    fs::write(root.join("Oversized.ttf"), vec![0_u8; 4 * 1024 * 1024 + 1]).unwrap();
    fs::write(root.join("readme.txt"), b"do not expose this file").unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(temp.path().join("private.ttf"), root.join("Outside.ttf")).unwrap();
    fs::write(temp.path().join("private.ttf"), &ttf).unwrap();

    auth::create_user(&state, "admin", "fallback-font-admin-password").unwrap();
    auth::create_user(&state, "viewer", "fallback-font-viewer-password").unwrap();
    let token = auth::authenticate(&state, "viewer", "fallback-font-viewer-password", "server")
        .unwrap()
        .unwrap()
        .access_token;

    let disabled = app::router(Arc::new(AppState::with_auth_dir(
        "server".to_owned(),
        state.clone(),
    )));
    assert_eq!(
        request(&disabled, "GET", "/FallbackFont/Fonts", None)
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    let none = request(&disabled, "GET", "/FallbackFont/Fonts", Some(&token)).await;
    assert_eq!(none.status(), StatusCode::OK);
    assert_eq!(
        to_bytes(none.into_body(), 64).await.unwrap().as_ref(),
        b"[]"
    );
    assert_eq!(
        request(
            &disabled,
            "GET",
            "/FallbackFont/Fonts/Family.ttf",
            Some(&token)
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );

    assert!(
        AppState::with_auth_dir("server".to_owned(), state.clone())
            .with_fallback_font_dir(std::path::PathBuf::from("relative/fonts"))
            .is_err()
    );
    assert!(
        AppState::with_auth_dir("server".to_owned(), state.clone())
            .with_fallback_font_dir(temp.path().join("missing"))
            .is_err()
    );
    #[cfg(unix)]
    {
        let link = temp.path().join("font-link");
        std::os::unix::fs::symlink(&root, &link).unwrap();
        assert!(
            AppState::with_auth_dir("server".to_owned(), state.clone())
                .with_fallback_font_dir(link)
                .is_err()
        );
    }
    let active = app::router(Arc::new(
        AppState::with_auth_dir("server".to_owned(), state.clone())
            .with_fallback_font_dir(root.clone())
            .unwrap(),
    ));
    let response = request(&active, "GET", "/FallbackFont/Fonts", Some(&token)).await;
    assert_eq!(response.status(), StatusCode::OK);
    let files: serde_json::Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 65_536).await.unwrap()).unwrap();
    assert_eq!(files.as_array().unwrap().len(), 2);
    assert_eq!(files[0]["Name"], "Alternative.otf");
    assert_eq!(files[0]["Size"], otf.len());
    assert!(files[0]["DateCreated"].is_null());
    assert!(files[0]["DateModified"].is_null());
    assert_eq!(files[1]["Name"], "Family.ttf");
    assert_eq!(files[1]["Size"], ttf.len());

    for (filename, contents, mime) in [
        ("Family.ttf", ttf.as_slice(), "font/ttf"),
        ("Alternative.otf", otf.as_slice(), "font/otf"),
    ] {
        let uri = format!("/FallbackFont/Fonts/{filename}");
        assert_eq!(
            request(&active, "GET", &uri, None).await.status(),
            StatusCode::UNAUTHORIZED
        );
        let response = request(&active, "GET", &uri, Some(&token)).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[header::CONTENT_TYPE], mime);
        assert_eq!(
            response.headers()[header::CONTENT_LENGTH],
            contents.len().to_string()
        );
        assert_eq!(
            response.headers()[header::CACHE_CONTROL],
            "private, no-store"
        );
        assert_eq!(response.headers()["x-content-type-options"], "nosniff");
        assert_eq!(
            to_bytes(response.into_body(), 1024).await.unwrap().as_ref(),
            contents
        );
        let head = request(&active, "HEAD", &uri, Some(&token)).await;
        assert_eq!(head.status(), StatusCode::OK);
        assert_eq!(
            head.headers()[header::CONTENT_LENGTH],
            contents.len().to_string()
        );
        assert!(to_bytes(head.into_body(), 64).await.unwrap().is_empty());
    }
    for name in [
        "Spoof.ttf",
        "Oversized.ttf",
        "Outside.ttf",
        "readme.txt",
        "missing.ttf",
    ] {
        let uri = format!("/FallbackFont/Fonts/{name}");
        assert_eq!(
            request(&active, "GET", &uri, Some(&token)).await.status(),
            StatusCode::NOT_FOUND,
            "{name}"
        );
    }
    assert_eq!(fs::read(root.join("Family.ttf")).unwrap(), ttf);
    assert_eq!(fs::read(root.join("Alternative.otf")).unwrap(), otf);
    assert_eq!(fs::read(temp.path().join("private.ttf")).unwrap(), ttf);
}
