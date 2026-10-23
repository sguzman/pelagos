use std::sync::Arc;

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
use tower::ServiceExt;

async fn call(router: &Router, route: &str, token: Option<&str>) -> Response {
    let mut request = Request::builder().uri(route);
    if let Some(token) = token {
        request = request.header("X-Emby-Token", token);
    }
    router
        .clone()
        .oneshot(request.body(Body::empty()).unwrap())
        .await
        .unwrap()
}

async fn json(response: Response) -> Value {
    let body = to_bytes(response.into_body(), 65536).await.unwrap();
    serde_json::from_slice(&body).unwrap()
}

#[tokio::test]
async fn theme_song_video_and_combined_contracts_are_private_and_explicitly_empty() {
    let fixture = tempfile::tempdir().unwrap();
    let state = fixture.path().join("state");
    let public = fixture.path().join("Public");
    let private = fixture.path().join("Private");
    std::fs::create_dir_all(&public).unwrap();
    std::fs::create_dir_all(&private).unwrap();
    std::fs::write(public.join("Film.mkv"), b"movie").unwrap();
    std::fs::write(private.join("Hidden.mkv"), b"secret").unwrap();
    catalog::index(&state, &public).unwrap();
    catalog::index(&state, &private).unwrap();
    auth::create_user(&state, "admin", "theme-admin-password").unwrap();
    auth::create_user(&state, "viewer", "theme-viewer-password").unwrap();
    let admin = auth::authenticate(&state, "admin", "theme-admin-password", "server")
        .unwrap()
        .unwrap();
    let viewer = auth::authenticate(&state, "viewer", "theme-viewer-password", "server")
        .unwrap()
        .unwrap();
    let router = app::router(Arc::new(AppState::with_auth_dir(
        "server".to_owned(),
        state.clone(),
    )));
    let inventory = json(call(&router, "/Items", Some(&admin.access_token)).await).await;
    let all = inventory["Items"].as_array().unwrap();
    let id = |name: &str| {
        all.iter()
            .find(|item| item["Name"] == name)
            .unwrap()["Id"]
            .as_str()
            .unwrap()
            .to_owned()
    };
    let film = id("Film");
    let hidden = id("Hidden");
    let route = format!("/Items/{film}/ThemeSongs");
    assert_eq!(
        call(&router, &route, None).await.status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        call(&router, &route, Some(&viewer.access_token))
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    auth::grant_library(&state, "viewer", &public).unwrap();

    for segment in ["ThemeSongs", "ThemeVideos"] {
        let path = format!("/Items/{film}/{segment}");
        let result = json(call(&router, &path, Some(&viewer.access_token)).await).await;
        assert_eq!(result["Items"], serde_json::json!([]));
        assert_eq!(result["TotalRecordCount"], 0);
        assert_eq!(result["StartIndex"], 0);
        assert_eq!(result["OwnerId"], film);
        let inherited = format!("{path}?inheritFromParent=true");
        assert_eq!(
            call(&router, &inherited, Some(&viewer.access_token))
                .await
                .status(),
            StatusCode::NOT_IMPLEMENTED
        );
        let sorted = format!("{path}?sortBy=SortName");
        assert_eq!(
            call(&router, &sorted, Some(&viewer.access_token))
                .await
                .status(),
            StatusCode::NOT_IMPLEMENTED
        );
    }

    let combined_route = format!("/Items/{film}/ThemeMedia?inheritFromParent=false");
    let combined = json(
        call(&router, &combined_route, Some(&viewer.access_token)).await,
    )
    .await;
    assert_eq!(combined["ThemeSongsResult"]["OwnerId"], film);
    assert_eq!(combined["ThemeSongsResult"]["Items"], serde_json::json!([]));
    assert_eq!(combined["ThemeVideosResult"], combined["ThemeSongsResult"]);
    assert_eq!(combined["SoundtrackSongsResult"]["Items"], serde_json::json!([]));
    assert_eq!(combined["SoundtrackSongsResult"]["TotalRecordCount"], 0);
    assert_eq!(
        combined["SoundtrackSongsResult"]["OwnerId"],
        "00000000-0000-0000-0000-000000000000"
    );

    let other_user = format!("/Items/{film}/ThemeMedia?userId={}", admin.user.id);
    assert_eq!(
        call(&router, &other_user, Some(&viewer.access_token))
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    for bad in [
        format!("/Items/{hidden}/ThemeSongs"),
        format!("/Items/{hidden}/ThemeMedia"),
        "/Items/00000000000000000000000000000000/ThemeVideos".to_owned(),
    ] {
        assert_eq!(
            call(&router, &bad, Some(&viewer.access_token))
                .await
                .status(),
            StatusCode::NOT_FOUND
        );
    }
    assert_eq!(
        json(
            call(
                &router,
                &format!("/Items/{hidden}/ThemeSongs"),
                Some(&admin.access_token),
            )
            .await,
        )
        .await["OwnerId"],
        hidden
    );
    let restarted = app::router(Arc::new(AppState::with_auth_dir(
        "server".to_owned(),
        state.clone(),
    )));
    assert_eq!(
        json(call(&restarted, &combined_route, Some(&viewer.access_token)).await).await,
        combined
    );
    auth::revoke_library(&state, "viewer", &public).unwrap();
    assert_eq!(
        call(&restarted, &combined_route, Some(&viewer.access_token))
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
}
