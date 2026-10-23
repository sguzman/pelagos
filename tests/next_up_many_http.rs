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

async fn call(router: &Router, method: &str, uri: &str, token: Option<&str>) -> Response {
    let mut request = Request::builder().method(method).uri(uri);
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
async fn unscoped_next_up_is_ranked_by_personal_watch_recency_and_access() {
    let fixture = tempfile::tempdir().unwrap();
    let state = fixture.path().join("state");
    let one = fixture.path().join("OneLib");
    let two = fixture.path().join("TwoLib");
    let three = fixture.path().join("ThreeLib");
    for (root, series) in [(&one, "One"), (&two, "Two"), (&three, "Three")] {
        std::fs::create_dir_all(root.join(format!("{series}/Season 01"))).unwrap();
        for (index, title) in [(1, "Beginning"), (2, "Continuation")] {
            std::fs::write(
                root.join(format!(
                    "{series}/Season 01/{series} - S01E0{index} - {title}.mp4"
                )),
                b"video",
            )
            .unwrap();
        }
        catalog::index(&state, root).unwrap();
    }
    auth::create_user(&state, "admin", "nextup-many-admin-pass").unwrap();
    auth::create_user(&state, "viewer", "nextup-many-viewer-pass").unwrap();
    let admin = auth::authenticate(&state, "admin", "nextup-many-admin-pass", "server")
        .unwrap()
        .unwrap();
    let viewer = auth::authenticate(&state, "viewer", "nextup-many-viewer-pass", "server")
        .unwrap()
        .unwrap();
    auth::grant_library(&state, "viewer", &one).unwrap();
    auth::grant_library(&state, "viewer", &two).unwrap();
    let router = app::router(Arc::new(AppState::with_auth_dir(
        "server".to_owned(),
        state.clone(),
    )));
    let no_history =
        json(call(&router, "GET", "/Shows/NextUp", Some(&viewer.access_token)).await).await;
    assert_eq!(no_history["TotalRecordCount"], 0);

    let series = json(
        call(
            &router,
            "GET",
            "/Items?includeItemTypes=Series",
            Some(&viewer.access_token),
        )
        .await,
    )
    .await;
    let series_items = series["Items"].as_array().unwrap();
    assert_eq!(series_items.len(), 2);
    for (name, watched_at) in [
        ("One", "2026-01-10T12:00:00.000Z"),
        ("Two", "2026-01-20T12:00:00.000Z"),
    ] {
        let id = series_items
            .iter()
            .find(|item| item["Name"] == name)
            .unwrap()["Id"]
            .as_str()
            .unwrap();
        let episodes = json(
            call(
                &router,
                "GET",
                &format!("/Shows/{id}/Episodes"),
                Some(&viewer.access_token),
            )
            .await,
        )
        .await;
        let first_id = episodes["Items"][0]["Id"].as_str().unwrap();
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
        let db = rusqlite::Connection::open(state.join("pelagos-user-data.sqlite3")).unwrap();
        db.execute(
            "UPDATE item_user_data SET last_played_utc=?1 WHERE user_id=?2 AND item_id=?3",
            params![watched_at, viewer.user.id, first_id],
        )
        .unwrap();
    }
    let latest =
        json(call(&router, "GET", "/Shows/NextUp", Some(&viewer.access_token)).await).await;
    assert_eq!(latest["TotalRecordCount"], 2);
    let items = latest["Items"].as_array().unwrap();
    assert_eq!(items[0]["SeriesName"], "Two");
    assert_eq!(items[1]["SeriesName"], "One");
    assert_eq!(items[0]["Name"], "Continuation");
    assert_eq!(items[1]["Name"], "Continuation");
    assert!(items.iter().all(|item| item["UserData"].is_object()));

    let page = json(
        call(
            &router,
            "GET",
            "/Shows/NextUp?startIndex=1&limit=1&enableUserData=false",
            Some(&viewer.access_token),
        )
        .await,
    )
    .await;
    assert_eq!(page["StartIndex"], 1);
    assert_eq!(page["TotalRecordCount"], 2);
    assert_eq!(page["Items"][0]["SeriesName"], "One");
    assert!(page["Items"][0].get("UserData").is_none());

    // The parent scope is a granted library CollectionFolder, not a Series,
    // a hidden library, or an arbitrary inaccessible item.
    let views = json(call(&router, "GET", "/UserViews", Some(&viewer.access_token)).await).await;
    let roots = views["Items"].as_array().unwrap();
    assert_eq!(roots.len(), 2);
    let root_id = |name: &str| {
        roots.iter().find(|item| item["Name"] == name).unwrap()["Id"]
            .as_str()
            .unwrap()
            .to_owned()
    };
    let one_root = root_id("OneLib");
    let two_root = root_id("TwoLib");
    let scoped_one = json(
        call(
            &router,
            "GET",
            &format!("/Shows/NextUp?parentId={one_root}"),
            Some(&viewer.access_token),
        )
        .await,
    )
    .await;
    assert_eq!(scoped_one["TotalRecordCount"], 1);
    assert_eq!(scoped_one["Items"][0]["SeriesName"], "One");
    let scoped_two = json(
        call(
            &router,
            "GET",
            &format!("/Shows/NextUp?parentId={two_root}"),
            Some(&viewer.access_token),
        )
        .await,
    )
    .await;
    assert_eq!(scoped_two["TotalRecordCount"], 1);
    assert_eq!(scoped_two["Items"][0]["SeriesName"], "Two");

    // The pinned latest-watched series cutoff is inclusive and applies
    // before pagination or result count.
    for (cutoff, expected) in [
        ("2026-01-15T00:00:00Z", 1),
        ("2026-01-20T12:00:00Z", 1),
        ("2026-01-20T12:00:00.001Z", 0),
        ("2026-01-01T00:00:00Z", 2),
    ] {
        let filtered = json(
            call(
                &router,
                "GET",
                &format!("/Shows/NextUp?nextUpDateCutoff={cutoff}"),
                Some(&viewer.access_token),
            )
            .await,
        )
        .await;
        assert_eq!(filtered["TotalRecordCount"], expected, "{cutoff}");
        if expected == 1 {
            assert_eq!(filtered["Items"][0]["SeriesName"], "Two");
        }
    }
    for malformed in ["2026-02-30T00:00:00Z", "2026-01-01", "2026-01-01T24:00:00Z"] {
        assert_eq!(
            call(
                &router,
                "GET",
                &format!("/Shows/NextUp?nextUpDateCutoff={malformed}"),
                Some(&viewer.access_token),
            )
            .await
            .status(),
            StatusCode::BAD_REQUEST
        );
    }
    let one_series = series_items
        .iter()
        .find(|item| item["Name"] == "One")
        .unwrap()["Id"]
        .as_str()
        .unwrap();
    for unsupported in [
        format!("/Shows/NextUp?parentId={one_series}"),
        format!("/Shows/NextUp?seriesId={one_series}&parentId={one_root}"),
        format!("/Shows/NextUp?seriesId={one_series}&nextUpDateCutoff=2026-01-01T00:00:00Z"),
    ] {
        assert_eq!(
            call(&router, "GET", &unsupported, Some(&viewer.access_token))
                .await
                .status(),
            StatusCode::NOT_IMPLEMENTED,
            "{unsupported}"
        );
    }
    assert_eq!(
        call(
            &router,
            "GET",
            "/Shows/NextUp?parentId=00000000000000000000000000000000",
            Some(&viewer.access_token),
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );

    let admin_next =
        json(call(&router, "GET", "/Shows/NextUp", Some(&admin.access_token)).await).await;
    assert_eq!(admin_next["TotalRecordCount"], 0);
    assert_eq!(
        call(
            &router,
            "GET",
            &format!("/Shows/NextUp?userId={}", admin.user.id),
            Some(&viewer.access_token),
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );

    let two_next_id = items[0]["Id"].as_str().unwrap();
    user_data::save_position(&state, &viewer.user.id, two_next_id, 20000000).unwrap();
    let no_resume = json(
        call(
            &router,
            "GET",
            "/Shows/NextUp?enableResumable=false",
            Some(&viewer.access_token),
        )
        .await,
    )
    .await;
    assert_eq!(no_resume["TotalRecordCount"], 1);
    assert_eq!(no_resume["Items"][0]["SeriesName"], "One");

    let restarted = app::router(Arc::new(AppState::with_auth_dir(
        "server".to_owned(),
        state.clone(),
    )));
    let persisted = json(
        call(
            &restarted,
            "GET",
            "/Shows/NextUp",
            Some(&viewer.access_token),
        )
        .await,
    )
    .await;
    assert_eq!(persisted["TotalRecordCount"], 2);
    auth::revoke_library(&state, "viewer", &two).unwrap();
    assert_eq!(
        call(
            &restarted,
            "GET",
            &format!("/Shows/NextUp?parentId={two_root}"),
            Some(&viewer.access_token),
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
    let revoked_two = json(
        call(
            &restarted,
            "GET",
            "/Shows/NextUp",
            Some(&viewer.access_token),
        )
        .await,
    )
    .await;
    assert_eq!(revoked_two["TotalRecordCount"], 1);
    assert_eq!(revoked_two["Items"][0]["SeriesName"], "One");
    auth::revoke_library(&state, "viewer", &one).unwrap();
    let revoked_all = json(
        call(
            &restarted,
            "GET",
            "/Shows/NextUp",
            Some(&viewer.access_token),
        )
        .await,
    )
    .await;
    assert_eq!(revoked_all["TotalRecordCount"], 0);
}
