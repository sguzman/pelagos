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
async fn latest_ungrouped_media_is_stable_permission_scoped_and_uses_played_filters() {
    let fixture = tempfile::tempdir().unwrap();
    let state = fixture.path().join("state");
    let allowed = fixture.path().join("Allowed");
    let private = fixture.path().join("Private");
    std::fs::create_dir_all(allowed.join("Film (2001)")).unwrap();
    std::fs::create_dir_all(allowed.join("Signal/Season 01")).unwrap();
    std::fs::create_dir(&private).unwrap();
    std::fs::write(allowed.join("Film (2001)/Film (2001).mkv"), b"movie").unwrap();
    std::fs::write(
        allowed.join("Signal/Season 01/Signal - S01E01 - Pilot.mp4"),
        b"episode",
    )
    .unwrap();
    std::fs::write(allowed.join("Track.flac"), b"audio").unwrap();
    std::fs::write(allowed.join("Landscape.jpg"), b"photo").unwrap();
    std::fs::write(private.join("Secret.mkv"), b"secret").unwrap();
    catalog::index(&state, &allowed).unwrap();
    catalog::index(&state, &private).unwrap();
    // Independent of filename order, file mtime and the wall clock.
    let db = rusqlite::Connection::open(state.join("pelagos.sqlite3")).unwrap();
    for (path, time) in [
        ("Film (2001)/Film (2001).mkv", 10),
        ("Signal/Season 01/Signal - S01E01 - Pilot.mp4", 30),
        ("Track.flac", 20),
        ("Landscape.jpg", 40),
        ("Secret.mkv", 50),
    ] {
        db.execute(
            "UPDATE media_files SET first_indexed_unix=?1 WHERE relative_path=?2",
            params![time, path],
        )
        .unwrap();
    }
    drop(db);
    // Normal rescans do not turn old media into new latest additions.
    catalog::index(&state, &allowed).unwrap();

    auth::create_user(&state, "admin", "latest-admin-test-password").unwrap();
    auth::create_user(&state, "viewer", "latest-viewer-test-password").unwrap();
    let admin = auth::authenticate(&state, "admin", "latest-admin-test-password", "server")
        .unwrap()
        .unwrap();
    let viewer = auth::authenticate(&state, "viewer", "latest-viewer-test-password", "server")
        .unwrap()
        .unwrap();
    let router = app::router(Arc::new(AppState::with_auth_dir(
        "server".to_owned(),
        state.clone(),
    )));
    let route = "/Items/Latest?groupItems=false";
    assert_eq!(
        call(&router, "GET", route, None).await.status(),
        StatusCode::UNAUTHORIZED
    );
    for unsupported in [
        "/Items/Latest",
        "/Items/Latest?groupItems=true",
        "/Items/Latest?groupItems=false&fields=MediaStreams",
        "/Items/Latest?groupItems=false&imageTypeLimit=1",
        "/Items/Latest?groupItems=false&includeItemTypes=Book",
    ] {
        assert_eq!(
            call(&router, "GET", unsupported, Some(&viewer.access_token))
                .await
                .status(),
            StatusCode::NOT_IMPLEMENTED,
            "{unsupported}"
        );
    }
    let no_grant = json(call(&router, "GET", route, Some(&viewer.access_token)).await).await;
    assert!(no_grant.as_array().unwrap().is_empty());
    auth::grant_library(&state, "viewer", &allowed).unwrap();
    let latest = json(call(&router, "GET", route, Some(&viewer.access_token)).await).await;
    let names = latest
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["Name"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(names, ["Landscape", "Pilot", "Track", "Film"]);
    assert_eq!(latest[0]["Type"], "Photo");
    assert_eq!(latest[1]["Type"], "Episode");
    assert_eq!(latest[3]["Type"], "Movie");
    assert!(latest.as_array().unwrap().iter().all(|item| item["UserData"].is_object()));

    let only_movies = json(
        call(
            &router,
            "GET",
            "/Items/Latest?groupItems=false&includeItemTypes=Movie",
            Some(&viewer.access_token),
        )
        .await,
    )
    .await;
    assert_eq!(only_movies.as_array().unwrap().len(), 1);
    assert_eq!(only_movies[0]["Name"], "Film");
    let hidden = json(
        call(
            &router,
            "GET",
            "/Items/Latest?groupItems=false&enableImages=false&enableUserData=false&limit=2",
            Some(&viewer.access_token),
        )
        .await,
    )
    .await;
    assert_eq!(hidden.as_array().unwrap().len(), 2);
    assert_eq!(hidden[0]["Name"], "Landscape");
    assert_eq!(hidden[1]["Name"], "Pilot");
    assert!(hidden.as_array().unwrap().iter().all(|item| item.get("UserData").is_none()));
    assert!(hidden.as_array().unwrap().iter().all(|item| item.get("ImageTags").is_none()));
    let zero = json(
        call(
            &router,
            "GET",
            "/Items/Latest?groupItems=false&limit=0",
            Some(&viewer.access_token),
        )
        .await,
    )
    .await;
    assert!(zero.as_array().unwrap().is_empty());

    let audio_id = latest[2]["Id"].as_str().unwrap();
    assert_eq!(
        call(
            &router,
            "POST",
            &format!("/UserPlayedItems/{audio_id}"),
            Some(&viewer.access_token),
        )
        .await
        .status(),
        StatusCode::OK
    );
    let played = json(
        call(
            &router,
            "GET",
            "/Items/Latest?groupItems=false&isPlayed=true",
            Some(&viewer.access_token),
        )
        .await,
    )
    .await;
    assert_eq!(played.as_array().unwrap().len(), 1);
    assert_eq!(played[0]["Name"], "Track");
    let unplayed = json(
        call(
            &router,
            "GET",
            "/Items/Latest?groupItems=false&isPlayed=false&limit=2&enableUserData=false",
            Some(&viewer.access_token),
        )
        .await,
    )
    .await;
    assert_eq!(unplayed.as_array().unwrap().len(), 2);
    assert_eq!(unplayed[0]["Name"], "Landscape");
    assert_eq!(unplayed[1]["Name"], "Pilot");
    assert!(unplayed[0].get("UserData").is_none());

    let legacy = json(
        call(
            &router,
            "GET",
            &format!("/Users/{}/Items/Latest?groupItems=false", viewer.user.id),
            Some(&viewer.access_token),
        )
        .await,
    )
    .await;
    assert_eq!(legacy.as_array().unwrap().len(), 4);
    for foreign in [
        format!("/Items/Latest?groupItems=false&userId={}", admin.user.id),
        format!("/Users/{}/Items/Latest?groupItems=false", admin.user.id),
    ] {
        assert_eq!(
            call(&router, "GET", &foreign, Some(&viewer.access_token))
                .await
                .status(),
            StatusCode::FORBIDDEN
        );
    }
    let views = json(call(&router, "GET", "/UserViews", Some(&viewer.access_token)).await).await;
    let root = views["Items"][0]["Id"].as_str().unwrap();
    let by_root = json(
        call(
            &router,
            "GET",
            &format!("/Items/Latest?groupItems=false&parentId={root}"),
            Some(&viewer.access_token),
        )
        .await,
    )
    .await;
    assert_eq!(by_root.as_array().unwrap().len(), 4);
    assert_eq!(
        call(
            &router,
            "GET",
            "/Items/Latest?groupItems=false&parentId=00000000000000000000000000000000",
            Some(&viewer.access_token),
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );

    let restarted = app::router(Arc::new(AppState::with_auth_dir(
        "server".to_owned(),
        state.clone(),
    )));
    let persisted = json(
        call(&restarted, "GET", route, Some(&viewer.access_token)).await,
    )
    .await;
    assert_eq!(persisted[0]["Name"], "Landscape");
    assert_eq!(persisted[3]["Name"], "Film");
    auth::revoke_library(&state, "viewer", &allowed).unwrap();
    let revoked = json(
        call(&restarted, "GET", route, Some(&viewer.access_token)).await,
    )
    .await;
    assert!(revoked.as_array().unwrap().is_empty());
}
