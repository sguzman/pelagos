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
async fn counts_respect_library_grants_and_personal_favorites_without_classification_guesses() {
    let fixture = tempfile::tempdir().unwrap();
    let state = fixture.path().join("state");
    let media = fixture.path().join("Allowed");
    let private = fixture.path().join("Private");
    for path in ["Film (2000)", "Show/Season 01"] {
        std::fs::create_dir_all(media.join(path)).unwrap();
    }
    std::fs::create_dir_all(private.join("Secret (2021)")).unwrap();
    for (path, data) in [
        ("Film (2000)/Film (2000).mkv", b"movie".as_slice()),
        ("Show/Season 01/Show - S01E01 - Pilot.mkv", b"episode"),
        ("Unsorted.mp4", b"generic"),
        ("Track.flac", b"audio"),
        ("Picture.jpg", b"photo"),
    ] {
        std::fs::write(media.join(path), data).unwrap();
    }
    std::fs::write(private.join("Secret (2021)/Secret (2021).mp4"), b"private").unwrap();
    catalog::index(&state, &media).unwrap();
    catalog::index(&state, &private).unwrap();

    auth::create_user(&state, "admin", "admin-counts-test-password").unwrap();
    auth::create_user(&state, "viewer", "viewer-counts-test-password").unwrap();
    let admin = auth::authenticate(&state, "admin", "admin-counts-test-password", "server")
        .unwrap()
        .unwrap();
    let viewer = auth::authenticate(&state, "viewer", "viewer-counts-test-password", "server")
        .unwrap()
        .unwrap();
    let router = app::router(Arc::new(AppState::with_auth_dir(
        "server".to_owned(),
        state.clone(),
    )));
    assert_eq!(
        call(&router, "GET", "/Items/Counts", None).await.status(),
        StatusCode::UNAUTHORIZED
    );
    let empty = json(call(&router, "GET", "/Items/Counts", Some(&viewer.access_token)).await).await;
    assert_eq!(empty["ItemCount"], 0);
    assert_eq!(empty["MovieCount"], 0);
    auth::grant_library(&state, "viewer", &media).unwrap();

    let counts =
        json(call(&router, "GET", "/Items/Counts", Some(&viewer.access_token)).await).await;
    assert_eq!(counts["ItemCount"], 5);
    assert_eq!(counts["MovieCount"], 1);
    assert_eq!(counts["SeriesCount"], 1);
    assert_eq!(counts["EpisodeCount"], 1);
    for unsupported in [
        "ArtistCount",
        "ProgramCount",
        "TrailerCount",
        "SongCount",
        "AlbumCount",
        "MusicVideoCount",
        "BoxSetCount",
        "BookCount",
    ] {
        assert_eq!(counts[unsupported], 0, "{unsupported} cannot be inferred");
    }
    assert!(counts.get("TotalRecordCount").is_none());
    assert!(counts.get("Path").is_none());

    let items = json(call(&router, "GET", "/Items", Some(&viewer.access_token)).await).await;
    let ids = items["Items"].as_array().unwrap();
    let id = |name: &str| {
        ids.iter().find(|item| item["Name"] == name).unwrap()["Id"]
            .as_str()
            .unwrap()
            .to_owned()
    };
    let movie = id("Film");
    let episode = id("Pilot");
    let views = json(call(&router, "GET", "/UserViews", Some(&viewer.access_token)).await).await;
    let collection = views["Items"][0]["Id"].as_str().unwrap();
    let folders = json(
        call(
            &router,
            "GET",
            &format!("/Items?parentId={collection}"),
            Some(&viewer.access_token),
        )
        .await,
    )
    .await;
    let show = folders["Items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["Name"] == "Show")
        .unwrap()["Id"]
        .as_str()
        .unwrap()
        .to_owned();
    for item_id in [&movie, &episode, &show] {
        assert_eq!(
            call(
                &router,
                "POST",
                &format!("/UserFavoriteItems/{item_id}"),
                Some(&viewer.access_token),
            )
            .await
            .status(),
            StatusCode::OK
        );
    }
    let favorite = json(
        call(
            &router,
            "GET",
            "/Items/Counts?isFavorite=true",
            Some(&viewer.access_token),
        )
        .await,
    )
    .await;
    assert_eq!(favorite["ItemCount"], 2);
    assert_eq!(favorite["SeriesCount"], 1);
    assert_eq!(favorite["MovieCount"], 1);
    assert_eq!(favorite["EpisodeCount"], 1);
    assert_eq!(favorite["SongCount"], 0);

    let nonfavorite = json(
        call(
            &router,
            "GET",
            "/Items/Counts?isFavorite=false",
            Some(&viewer.access_token),
        )
        .await,
    )
    .await;
    assert_eq!(nonfavorite["ItemCount"], 3);
    assert_eq!(nonfavorite["SeriesCount"], 0);
    assert_eq!(nonfavorite["MovieCount"], 0);
    assert_eq!(nonfavorite["EpisodeCount"], 0);

    let own = json(
        call(
            &router,
            "GET",
            &format!("/Items/Counts?userId={}", viewer.user.id),
            Some(&viewer.access_token),
        )
        .await,
    )
    .await;
    assert_eq!(own, counts);
    assert_eq!(
        call(
            &router,
            "GET",
            &format!("/Items/Counts?userId={}", admin.user.id),
            Some(&viewer.access_token),
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    let admin_counts =
        json(call(&router, "GET", "/Items/Counts", Some(&admin.access_token)).await).await;
    assert_eq!(admin_counts["ItemCount"], 6);
    assert_eq!(admin_counts["MovieCount"], 2);
    let admin_favorites = json(
        call(
            &router,
            "GET",
            "/Items/Counts?isFavorite=true",
            Some(&admin.access_token),
        )
        .await,
    )
    .await;
    assert_eq!(admin_favorites["ItemCount"], 0);

    let restarted = app::router(Arc::new(AppState::with_auth_dir(
        "server".to_owned(),
        state.clone(),
    )));
    let persisted = json(
        call(
            &restarted,
            "GET",
            "/Items/Counts?isFavorite=true",
            Some(&viewer.access_token),
        )
        .await,
    )
    .await;
    assert_eq!(persisted, favorite);
    auth::revoke_library(&state, "viewer", &media).unwrap();
    let revoked = json(
        call(
            &restarted,
            "GET",
            "/Items/Counts?isFavorite=true",
            Some(&viewer.access_token),
        )
        .await,
    )
    .await;
    assert_eq!(revoked["ItemCount"], 0);
    assert_eq!(revoked["SeriesCount"], 0);
}
