use std::sync::Arc;

use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode},
    response::Response,
};
use pelagos::{
    app::{self, AppState},
    auth, catalog, user_data,
};
use rusqlite::params;
use serde_json::Value;
use tower::ServiceExt;

async fn get(router: &Router, path: &str, token: Option<&str>) -> Response {
    let mut request = Request::builder().uri(path);
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
async fn resumes_are_private_persistent_filterable_and_reject_unsupported_options() {
    let temp = tempfile::tempdir().unwrap();
    let state = temp.path().join("state");
    let allowed = temp.path().join("Allowed");
    let private = temp.path().join("Private");
    std::fs::create_dir(&allowed).unwrap();
    std::fs::create_dir(&private).unwrap();
    std::fs::write(allowed.join("Alpha.mp4"), b"video").unwrap();
    std::fs::write(allowed.join("Beta.mkv"), b"video").unwrap();
    std::fs::write(allowed.join("Song.flac"), b"audio").unwrap();
    std::fs::write(allowed.join("Picture.jpg"), b"photo").unwrap();
    std::fs::write(private.join("Secret.mp4"), b"video").unwrap();
    catalog::index(&state, &allowed).unwrap();
    catalog::index(&state, &private).unwrap();

    auth::create_user(&state, "admin", "admin-resume-test-password").unwrap();
    auth::create_user(&state, "viewer", "viewer-resume-test-password").unwrap();
    let admin = auth::authenticate(&state, "admin", "admin-resume-test-password", "server")
        .unwrap()
        .unwrap();
    let viewer = auth::authenticate(&state, "viewer", "viewer-resume-test-password", "server")
        .unwrap()
        .unwrap();
    let router = app::router(Arc::new(AppState::with_auth_dir(
        "server".to_owned(),
        state.clone(),
    )));
    let all = json(get(&router, "/Items", Some(&admin.access_token)).await).await;
    let items = all["Items"].as_array().unwrap();
    let id = |name: &str| -> String {
        items.iter().find(|item| item["Name"] == name).unwrap()["Id"]
            .as_str()
            .unwrap()
            .to_owned()
    };
    let alpha = id("Alpha");
    let beta = id("Beta");
    let song = id("Song");
    let picture = id("Picture");
    let secret = id("Secret");
    let uri = "/UserItems/Resume";
    assert_eq!(
        get(&router, uri, None).await.status(),
        StatusCode::UNAUTHORIZED
    );
    let empty = json(get(&router, uri, Some(&viewer.access_token)).await).await;
    assert_eq!(empty["TotalRecordCount"], 0);

    auth::grant_library(&state, "viewer", &allowed).unwrap();
    user_data::save_position(&state, &viewer.user.id, &alpha, 10_000_000).unwrap();
    user_data::save_position(&state, &viewer.user.id, &song, 22_000_000).unwrap();
    user_data::save_position(&state, &viewer.user.id, &picture, 33_000_000).unwrap();
    user_data::save_position(&state, &viewer.user.id, &secret, 44_000_000).unwrap();
    user_data::save_position(&state, &viewer.user.id, &beta, 50_000_000).unwrap();
    // Force distinct, deterministic fixture timestamps without sleeping or
    // depending on filesystem creation/modification times.
    let db = rusqlite::Connection::open(state.join("pelagos-user-data.sqlite3")).unwrap();
    for (item_id, played_at) in [
        (&alpha, "2026-10-20T10:00:00.000Z"),
        (&song, "2026-10-21T10:00:00.000Z"),
        (&beta, "2026-10-22T10:00:00.000Z"),
    ] {
        db.execute(
            "UPDATE item_user_data SET last_played_utc=?1 WHERE user_id=?2 AND item_id=?3",
            params![played_at, viewer.user.id, item_id],
        )
        .unwrap();
    }

    // A fully watched item with a nonzero legacy position cannot reappear in resume.
    let state_before = user_data::read(&state, &viewer.user.id, &beta).unwrap();
    assert_eq!(state_before.playback_position_ticks, 50_000_000);

    let found = json(get(&router, uri, Some(&viewer.access_token)).await).await;
    assert_eq!(found["TotalRecordCount"], 3);
    let items = found["Items"].as_array().unwrap();
    assert!(items.iter().any(|item| item["Id"] == alpha));
    assert!(items.iter().any(|item| item["Id"] == beta));
    assert!(items.iter().any(|item| item["Id"] == song));
    assert_eq!(items[0]["Name"], "Beta");
    assert_eq!(items[1]["Name"], "Song");
    assert_eq!(items[2]["Name"], "Alpha");
    assert!(
        !items
            .iter()
            .any(|item| item["Id"] == picture || item["Id"] == secret)
    );
    assert!(
        items
            .iter()
            .all(|item| item["UserData"]["PlaybackPositionTicks"].as_i64().unwrap() > 0)
    );
    assert!(
        items
            .iter()
            .all(|item| item["UserData"]["LastPlayedDate"].is_string())
    );

    let only_audio = json(
        get(
            &router,
            "/UserItems/Resume?mediaTypes=Audio",
            Some(&viewer.access_token),
        )
        .await,
    )
    .await;
    assert_eq!(only_audio["TotalRecordCount"], 1);
    assert_eq!(only_audio["Items"][0]["Name"], "Song");
    let only_video = json(
        get(
            &router,
            "/UserItems/Resume?includeItemTypes=Video&searchTerm=Alpha",
            Some(&viewer.access_token),
        )
        .await,
    )
    .await;
    assert_eq!(only_video["TotalRecordCount"], 1);
    assert_eq!(only_video["Items"][0]["Name"], "Alpha");
    let views = json(get(&router, "/UserViews", Some(&viewer.access_token)).await).await;
    let root = views["Items"][0]["Id"].as_str().unwrap();
    let under_root = json(
        get(
            &router,
            &format!("/UserItems/Resume?parentId={root}&limit=1&startIndex=1"),
            Some(&viewer.access_token),
        )
        .await,
    )
    .await;
    assert_eq!(under_root["TotalRecordCount"], 3);
    assert_eq!(under_root["Items"].as_array().unwrap().len(), 1);
    assert_eq!(under_root["StartIndex"], 1);
    let without_user_data = json(
        get(
            &router,
            "/UserItems/Resume?enableUserData=false&enableImages=false",
            Some(&viewer.access_token),
        )
        .await,
    )
    .await;
    assert!(without_user_data["Items"][0].get("UserData").is_none());
    assert_eq!(without_user_data["Items"][0]["Name"], "Beta");
    assert!(without_user_data["Items"][0].get("ImageTags").is_none());

    let legacy = format!("/Users/{}/Items/Resume", viewer.user.id);
    assert_eq!(
        json(get(&router, &legacy, Some(&viewer.access_token)).await).await["TotalRecordCount"],
        3
    );
    for denied in [
        format!("/UserItems/Resume?userId={}", admin.user.id),
        format!("/Users/{}/Items/Resume", admin.user.id),
    ] {
        assert_eq!(
            get(&router, &denied, Some(&viewer.access_token))
                .await
                .status(),
            StatusCode::FORBIDDEN
        );
    }
    for unsupported in [
        "/UserItems/Resume?excludeActiveSessions=true",
        "/UserItems/Resume?fields=Chapters",
        "/UserItems/Resume?mediaTypes=Unknown",
    ] {
        assert_eq!(
            get(&router, unsupported, Some(&viewer.access_token))
                .await
                .status(),
            StatusCode::NOT_IMPLEMENTED
        );
    }
    assert_eq!(
        get(
            &router,
            "/UserItems/Resume?parentId=00000000000000000000000000000000",
            Some(&viewer.access_token),
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );

    // Marking an item played resets its position and excludes it from resume.
    let played_url = format!("/UserPlayedItems/{beta}");
    let marked = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(played_url.as_str())
                .header("X-Emby-Token", &viewer.access_token)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(marked.status(), StatusCode::OK);
    let remaining = json(get(&router, uri, Some(&viewer.access_token)).await).await;
    assert_eq!(remaining["TotalRecordCount"], 2);
    assert_eq!(remaining["Items"][0]["Name"], "Song");
    let restarted = app::router(Arc::new(AppState::with_auth_dir(
        "server".to_owned(),
        state.clone(),
    )));
    assert_eq!(
        json(get(&restarted, uri, Some(&viewer.access_token)).await).await["TotalRecordCount"],
        2
    );
    auth::revoke_library(&state, "viewer", &allowed).unwrap();
    assert_eq!(
        json(get(&restarted, uri, Some(&viewer.access_token)).await).await["TotalRecordCount"],
        0
    );
}
