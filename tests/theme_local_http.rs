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
    serde_json::from_slice(&to_bytes(response.into_body(), 65536).await.unwrap()).unwrap()
}

#[tokio::test]
async fn local_theme_rules_are_scoped_to_a_single_recognized_owner() {
    let fixture = tempfile::tempdir().unwrap();
    let state = fixture.path().join("state");
    let public = fixture.path().join("Public");
    let private = fixture.path().join("Private");
    for path in [
        "Film (2022)",
        "Film (2022)/theme-music",
        "Film (2022)/backdrops",
        "Film (2022)/trailers",
        "Second (2023)",
        "Series/Season 01",
        "Series/theme-music",
        "Series/backdrops",
    ] {
        std::fs::create_dir_all(public.join(path)).unwrap();
    }
    std::fs::create_dir_all(&private).unwrap();
    for path in [
        "Film (2022)/Film (2022).mkv",
        "Film (2022)/theme.mp3",
        "Film (2022)/theme-music/score.flac",
        "Film (2022)/backdrops/loop.mp4",
        "Film (2022)/trailers/feature.mp4",
        "Second (2023)/Second (2023).mkv",
        "Series/Season 01/Series - S01E01 - Pilot.mp4",
        "Series/theme-music/series.flac",
        "Series/backdrops/intro.webm",
        "Loose.mkv",
        "theme.mp3",
    ] {
        std::fs::write(public.join(path), b"media").unwrap();
    }
    std::fs::write(private.join("Secret.mkv"), b"private").unwrap();
    catalog::index(&state, &public).unwrap();
    catalog::index(&state, &private).unwrap();
    auth::create_user(&state, "admin", "theme-index-admin-password").unwrap();
    auth::create_user(&state, "viewer", "theme-index-viewer-password").unwrap();
    let admin = auth::authenticate(&state, "admin", "theme-index-admin-password", "server")
        .unwrap()
        .unwrap();
    let viewer = auth::authenticate(&state, "viewer", "theme-index-viewer-password", "server")
        .unwrap()
        .unwrap();
    let router = app::router(Arc::new(AppState::with_auth_dir(
        "server".to_owned(),
        state.clone(),
    )));
    let inventory = json(
        call(
            &router,
            "/Items?includeItemTypes=Movie,Series",
            Some(&admin.access_token),
        )
        .await,
    )
    .await;
    let all = inventory["Items"].as_array().unwrap();
    let id = |name: &str| {
        all.iter().find(|item| item["Name"] == name).unwrap()["Id"]
            .as_str()
            .unwrap()
            .to_owned()
    };
    let film = id("Film");
    let series = id("Series");
    let second = id("Second");
    let film_song_path = format!("/Items/{film}/ThemeSongs");
    let film_video_path = format!("/Items/{film}/ThemeVideos");
    assert_eq!(
        call(&router, &film_song_path, Some(&viewer.access_token))
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    auth::grant_library(&state, "viewer", &public).unwrap();

    let songs = json(call(&router, &film_song_path, Some(&viewer.access_token)).await).await;
    assert_eq!(songs["OwnerId"], film);
    assert_eq!(songs["TotalRecordCount"], 2);
    assert_eq!(songs["Items"][0]["Name"], "score");
    assert_eq!(songs["Items"][1]["Name"], "theme");
    assert_eq!(songs["Items"][0]["ExtraType"], "ThemeSong");
    assert_eq!(songs["Items"][1]["ExtraType"], "ThemeSong");
    assert_eq!(songs["Items"][0]["Type"], "Audio");

    let videos = json(call(&router, &film_video_path, Some(&viewer.access_token)).await).await;
    assert_eq!(videos["TotalRecordCount"], 1);
    assert_eq!(videos["Items"][0]["Name"], "loop");
    assert_eq!(videos["Items"][0]["ExtraType"], "ThemeVideo");

    let combined = json(
        call(
            &router,
            &format!("/Items/{film}/ThemeMedia"),
            Some(&viewer.access_token),
        )
        .await,
    )
    .await;
    assert_eq!(combined["ThemeSongsResult"], songs);
    assert_eq!(combined["ThemeVideosResult"], videos);
    assert_eq!(combined["SoundtrackSongsResult"]["TotalRecordCount"], 0);

    let series_theme = json(
        call(
            &router,
            &format!("/Items/{series}/ThemeMedia"),
            Some(&viewer.access_token),
        )
        .await,
    )
    .await;
    assert_eq!(
        series_theme["ThemeSongsResult"]["Items"][0]["Name"],
        "series"
    );
    assert_eq!(
        series_theme["ThemeVideosResult"]["Items"][0]["Name"],
        "intro"
    );
    for path in [
        format!("/Items/{second}/ThemeSongs"),
        format!("/Items/{second}/ThemeVideos"),
    ] {
        assert_eq!(
            json(call(&router, &path, Some(&viewer.access_token)).await).await["TotalRecordCount"],
            0
        );
    }
    assert_eq!(
        call(
            &router,
            &format!("/Items/{film}/ThemeSongs?inheritFromParent=true"),
            Some(&viewer.access_token),
        )
        .await
        .status(),
        StatusCode::NOT_IMPLEMENTED
    );
    let restarted = app::router(Arc::new(AppState::with_auth_dir(
        "server".to_owned(),
        state.clone(),
    )));
    assert_eq!(
        json(call(&restarted, &film_song_path, Some(&viewer.access_token)).await).await,
        songs
    );
    auth::revoke_library(&state, "viewer", &public).unwrap();
    assert_eq!(
        call(&restarted, &film_song_path, Some(&viewer.access_token))
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
}
