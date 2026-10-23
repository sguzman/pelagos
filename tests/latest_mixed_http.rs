//! Pinned Jellyfin latest-media grouping for videos without album metadata.
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
use rusqlite::params;
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
    let bytes = to_bytes(response.into_body(), 65536).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

#[tokio::test]
async fn movies_and_mixed_video_feeds_keep_jellyfin_episode_grouping_and_permissions() {
    let fixture = tempfile::tempdir().unwrap();
    let state = fixture.path().join("state");
    let root = fixture.path().join("Public");
    let private = fixture.path().join("Private");
    std::fs::create_dir_all(root.join("Film (2022)")).unwrap();
    std::fs::create_dir_all(root.join("Signal/Season 01")).unwrap();
    std::fs::create_dir_all(root.join("Orbit/Season 01")).unwrap();
    std::fs::create_dir_all(private.join("Hidden (2025)")).unwrap();
    for (path, content) in [
        ("Film (2022)/Film (2022).mkv", b"movie".as_slice()),
        ("Signal/Season 01/Signal - S01E01 - One.mp4", b"one"),
        ("Signal/Season 01/Signal - S01E02 - Two.mp4", b"two"),
        ("Orbit/Season 01/Orbit - S01E01 - Pilot.mp4", b"pilot"),
        ("Loose.mp4", b"generic"),
    ] {
        std::fs::write(root.join(path), content).unwrap();
    }
    std::fs::write(
        private.join("Hidden (2025)/Hidden (2025).mkv"),
        b"secret",
    )
    .unwrap();
    catalog::index(&state, &root).unwrap();
    catalog::index(&state, &private).unwrap();
    let db = rusqlite::Connection::open(state.join("pelagos.sqlite3")).unwrap();
    for (name, age) in [
        ("Hidden (2025).mkv", 110),
        ("Signal - S01E02 - Two.mp4", 100),
        ("Film (2022).mkv", 90),
        ("Orbit - S01E01 - Pilot.mp4", 80),
        ("Signal - S01E01 - One.mp4", 70),
        ("Loose.mp4", 60),
    ] {
        db.execute(
            "UPDATE media_files SET first_indexed_unix=?1 WHERE relative_path LIKE ?2",
            params![age, format!("%{name}")],
        )
        .unwrap();
    }
    drop(db);

    auth::create_user(&state, "admin", "latest-mixed-admin-password").unwrap();
    auth::create_user(&state, "viewer", "latest-mixed-viewer-password").unwrap();
    let admin = auth::authenticate(&state, "admin", "latest-mixed-admin-password", "server")
        .unwrap()
        .unwrap();
    let viewer = auth::authenticate(&state, "viewer", "latest-mixed-viewer-password", "server")
        .unwrap()
        .unwrap();
    let router = app::router(Arc::new(AppState::with_auth_dir(
        "server".to_owned(),
        state.clone(),
    )));
    let movie_route = "/Items/Latest?includeItemTypes=Movie";
    let mixed_route = "/Items/Latest?includeItemTypes=Movie,Episode,Video";

    assert_eq!(
        call(&router, movie_route, None).await.status(),
        StatusCode::UNAUTHORIZED
    );
    assert!(
        json(call(&router, movie_route, Some(&viewer.access_token)).await)
            .await
            .as_array()
            .unwrap()
            .is_empty()
    );
    auth::grant_library(&state, "viewer", &root).unwrap();

    let movies = json(call(&router, movie_route, Some(&viewer.access_token)).await).await;
    assert_eq!(movies.as_array().unwrap().len(), 1);
    assert_eq!(movies[0]["Name"], "Film");
    assert_eq!(movies[0]["Type"], "Movie");
    assert!(movies[0].get("ChildCount").is_none());

    let mixed = json(call(&router, mixed_route, Some(&viewer.access_token)).await).await;
    assert_eq!(mixed.as_array().unwrap().len(), 4);
    assert_eq!(mixed[0]["Name"], "Signal");
    assert_eq!(mixed[0]["Type"], "Series");
    assert_eq!(mixed[0]["ChildCount"], 2);
    assert_eq!(mixed[1]["Name"], "Film");
    assert_eq!(mixed[1]["Type"], "Movie");
    assert_eq!(mixed[2]["Name"], "Pilot");
    assert_eq!(mixed[2]["Type"], "Episode");
    assert!(mixed[2].get("ChildCount").is_none());
    assert_eq!(mixed[3]["Name"], "Loose");
    assert_eq!(mixed[3]["Type"], "Video");
    assert!(mixed
        .as_array()
        .unwrap()
        .iter()
        .all(|item| item["Name"] != "Hidden"));

    let only_two = json(
        call(
            &router,
            "/Items/Latest?includeItemTypes=Movie,Episode,Video&limit=2",
            Some(&viewer.access_token),
        )
        .await,
    )
    .await;
    assert_eq!(only_two.as_array().unwrap().len(), 2);
    assert_eq!(only_two[0]["ChildCount"], 2);
    assert_eq!(only_two[1]["Type"], "Movie");

    let no_group = json(
        call(
            &router,
            "/Items/Latest?includeItemTypes=Movie,Episode,Video&groupItems=false",
            Some(&viewer.access_token),
        )
        .await,
    )
    .await;
    assert_eq!(no_group.as_array().unwrap().len(), 5);
    assert_eq!(no_group[0]["Name"], "Two");
    assert_eq!(no_group[1]["Type"], "Movie");
    assert_eq!(no_group[3]["Name"], "One");

    // Album and photo group entities are not implemented; never fake their grouping.
    for unsupported in [
        "/Items/Latest",
        "/Items/Latest?includeItemTypes=Movie,Audio",
        "/Items/Latest?includeItemTypes=Photo",
        "/Items/Latest?includeItemTypes=Movie,Episode,Photo",
    ] {
        assert_eq!(
            call(&router, unsupported, Some(&viewer.access_token))
                .await
                .status(),
            StatusCode::NOT_IMPLEMENTED,
            "{unsupported}"
        );
    }
    assert_eq!(
        call(
            &router,
            &format!(
                "/Items/Latest?includeItemTypes=Movie&userId={}",
                admin.user.id
            ),
            Some(&viewer.access_token),
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    let legacy = json(
        call(
            &router,
            &format!(
                "/Users/{}/Items/Latest?includeItemTypes=Movie,Episode,Video",
                viewer.user.id
            ),
            Some(&viewer.access_token),
        )
        .await,
    )
    .await;
    assert_eq!(legacy, mixed);
    auth::revoke_library(&state, "viewer", &root).unwrap();
    assert!(
        json(call(&router, mixed_route, Some(&viewer.access_token)).await)
            .await
            .as_array()
            .unwrap()
            .is_empty()
    );
}
