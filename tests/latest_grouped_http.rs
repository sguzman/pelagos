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

async fn call(router: &Router, method: &str, route: &str, token: Option<&str>) -> Response {
    let mut request = Request::builder().method(method).uri(route);
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
async fn latest_grouped_episodes_use_series_only_for_multiple_items_and_count_before_limit() {
    let fixture = tempfile::tempdir().unwrap();
    let state = fixture.path().join("state");
    let library = fixture.path().join("Allowed");
    let private = fixture.path().join("Private");
    std::fs::create_dir_all(library.join("Signal/Season 01")).unwrap();
    std::fs::create_dir_all(library.join("Orbit/Season 01")).unwrap();
    std::fs::create_dir_all(private.join("Hidden/Season 01")).unwrap();
    for (path, value) in [
        (
            "Signal/Season 01/Signal - S01E01 - First.mp4",
            b"one".as_slice(),
        ),
        ("Signal/Season 01/Signal - S01E02 - Second.mp4", b"two"),
        ("Orbit/Season 01/Orbit - S01E01 - Only.mp4", b"three"),
    ] {
        std::fs::write(library.join(path), value).unwrap();
    }
    std::fs::write(
        private.join("Hidden/Season 01/Hidden - S01E01 - Private.mp4"),
        b"secret",
    )
    .unwrap();
    catalog::index(&state, &library).unwrap();
    catalog::index(&state, &private).unwrap();
    let db = rusqlite::Connection::open(state.join("pelagos.sqlite3")).unwrap();
    for (name, first_seen) in [
        ("Signal - S01E02 - Second.mp4", 40),
        ("Orbit - S01E01 - Only.mp4", 30),
        ("Signal - S01E01 - First.mp4", 20),
    ] {
        db.execute(
            "UPDATE media_files SET first_indexed_unix=?1 WHERE relative_path LIKE ?2",
            params![first_seen, format!("%{name}")],
        )
        .unwrap();
    }
    drop(db);
    auth::create_user(&state, "admin", "grouped-admin-test-password").unwrap();
    auth::create_user(&state, "viewer", "grouped-viewer-test-password").unwrap();
    let admin = auth::authenticate(&state, "admin", "grouped-admin-test-password", "server")
        .unwrap()
        .unwrap();
    let viewer = auth::authenticate(&state, "viewer", "grouped-viewer-test-password", "server")
        .unwrap()
        .unwrap();
    let router = app::router(Arc::new(AppState::with_auth_dir(
        "server".to_owned(),
        state.clone(),
    )));
    let grouped = "/Items/Latest?includeItemTypes=Episode";
    assert_eq!(
        call(&router, "GET", grouped, None).await.status(),
        StatusCode::UNAUTHORIZED
    );
    for unsupported in [
        "/Items/Latest?groupItems=true&includeItemTypes=Audio",
    ] {
        assert_eq!(
            call(&router, "GET", unsupported, Some(&viewer.access_token))
                .await
                .status(),
            StatusCode::NOT_IMPLEMENTED,
            "{unsupported}"
        );
    }
    let empty = json(call(&router, "GET", grouped, Some(&viewer.access_token)).await).await;
    assert!(empty.as_array().unwrap().is_empty());
    auth::grant_library(&state, "viewer", &library).unwrap();

    let grouped_result =
        json(call(&router, "GET", grouped, Some(&viewer.access_token)).await).await;
    let mixed_without_movies = json(
        call(
            &router,
            "GET",
            "/Items/Latest?includeItemTypes=Episode,Movie",
            Some(&viewer.access_token),
        )
        .await,
    )
    .await;
    assert_eq!(mixed_without_movies, grouped_result);
    let default_video_only = json(
        call(
            &router,
            "GET",
            "/Items/Latest",
            Some(&viewer.access_token),
        )
        .await,
    )
    .await;
    assert_eq!(default_video_only, grouped_result);
    assert_eq!(grouped_result.as_array().unwrap().len(), 2);
    assert_eq!(grouped_result[0]["Type"], "Series");
    assert_eq!(grouped_result[0]["Name"], "Signal");
    assert_eq!(grouped_result[0]["ChildCount"], 2);
    assert_eq!(grouped_result[1]["Type"], "Episode");
    assert_eq!(grouped_result[1]["Name"], "Only");
    assert!(grouped_result[1].get("ChildCount").is_none());
    let series_id = grouped_result[0]["Id"].as_str().unwrap().to_owned();

    let one_group = json(
        call(
            &router,
            "GET",
            "/Items/Latest?includeItemTypes=Episode&groupItems=true&limit=1",
            Some(&viewer.access_token),
        )
        .await,
    )
    .await;
    assert_eq!(one_group.as_array().unwrap().len(), 1);
    assert_eq!(one_group[0]["Name"], "Signal");
    assert_eq!(one_group[0]["ChildCount"], 2);
    let ungrouped = json(
        call(
            &router,
            "GET",
            "/Items/Latest?includeItemTypes=Episode&groupItems=false",
            Some(&viewer.access_token),
        )
        .await,
    )
    .await;
    assert_eq!(ungrouped.as_array().unwrap().len(), 3);
    assert_eq!(ungrouped[0]["Name"], "Second");
    assert_eq!(ungrouped[1]["Name"], "Only");
    assert_eq!(ungrouped[2]["Name"], "First");
    assert!(ungrouped[0].get("ChildCount").is_none());

    assert_eq!(
        call(
            &router,
            "POST",
            &format!("/UserFavoriteItems/{series_id}"),
            Some(&viewer.access_token),
        )
        .await
        .status(),
        StatusCode::OK
    );
    let favorites = json(call(&router, "GET", grouped, Some(&viewer.access_token)).await).await;
    assert_eq!(favorites[0]["UserData"]["IsFavorite"], true);
    let suppressed = json(
        call(
            &router,
            "GET",
            "/Items/Latest?includeItemTypes=Episode&enableUserData=false&limit=1",
            Some(&viewer.access_token),
        )
        .await,
    )
    .await;
    assert!(suppressed[0].get("UserData").is_none());
    assert_eq!(suppressed[0]["ChildCount"], 2);

    let first_id = ungrouped[2]["Id"].as_str().unwrap();
    assert_eq!(
        call(
            &router,
            "POST",
            &format!("/UserPlayedItems/{first_id}"),
            Some(&viewer.access_token),
        )
        .await
        .status(),
        StatusCode::OK
    );
    let unplayed = json(
        call(
            &router,
            "GET",
            "/Items/Latest?includeItemTypes=Episode&isPlayed=false",
            Some(&viewer.access_token),
        )
        .await,
    )
    .await;
    assert_eq!(unplayed.as_array().unwrap().len(), 2);
    assert_eq!(unplayed[0]["Type"], "Episode");
    assert_eq!(unplayed[0]["Name"], "Second");
    assert!(unplayed[0].get("ChildCount").is_none());
    assert_eq!(unplayed[1]["Name"], "Only");

    let parent_group = json(
        call(
            &router,
            "GET",
            &format!("/Items/Latest?includeItemTypes=Episode&parentId={series_id}"),
            Some(&viewer.access_token),
        )
        .await,
    )
    .await;
    assert_eq!(parent_group.as_array().unwrap().len(), 1);
    assert_eq!(parent_group[0]["Name"], "Signal");
    assert_eq!(parent_group[0]["ChildCount"], 2);
    assert_eq!(
        call(
            &router,
            "GET",
            &format!(
                "/Items/Latest?includeItemTypes=Episode&userId={}",
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
            "GET",
            &format!(
                "/Users/{}/Items/Latest?includeItemTypes=Episode",
                viewer.user.id
            ),
            Some(&viewer.access_token),
        )
        .await,
    )
    .await;
    assert_eq!(legacy, favorites);

    let restarted = app::router(Arc::new(AppState::with_auth_dir(
        "server".to_owned(),
        state.clone(),
    )));
    let after_restart =
        json(call(&restarted, "GET", grouped, Some(&viewer.access_token)).await).await;
    assert_eq!(after_restart[0]["ChildCount"], 2);
    auth::revoke_library(&state, "viewer", &library).unwrap();
    let revoked = json(call(&restarted, "GET", grouped, Some(&viewer.access_token)).await).await;
    assert!(revoked.as_array().unwrap().is_empty());
}
