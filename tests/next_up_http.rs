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
async fn selected_series_next_up_respects_episode_order_resume_and_library_grants() {
    let fixture = tempfile::tempdir().unwrap();
    let state = fixture.path().join("state");
    let allowed = fixture.path().join("Allowed");
    let private = fixture.path().join("Private");
    std::fs::create_dir_all(allowed.join("Signal/Season 01")).unwrap();
    std::fs::create_dir_all(allowed.join("Signal/Season 02")).unwrap();
    std::fs::create_dir_all(private.join("Hidden/Season 01")).unwrap();
    for (path, data) in [
        (
            "Signal/Season 01/Signal - S01E01 - Dawn.mp4",
            b"one".as_slice(),
        ),
        ("Signal/Season 01/Signal - S01E02 - Noon.mp4", b"two"),
        ("Signal/Season 01/Signal - S01E03 - Dusk.mp4", b"three"),
        ("Signal/Season 02/Signal - S02E01 - Morning.mp4", b"four"),
    ] {
        std::fs::write(allowed.join(path), data).unwrap();
    }
    std::fs::write(
        private.join("Hidden/Season 01/Hidden - S01E01 - Secret.mp4"),
        b"private",
    )
    .unwrap();
    catalog::index(&state, &allowed).unwrap();
    catalog::index(&state, &private).unwrap();
    auth::create_user(&state, "admin", "nextup-admin-test-password").unwrap();
    auth::create_user(&state, "viewer", "nextup-viewer-test-password").unwrap();
    let admin = auth::authenticate(&state, "admin", "nextup-admin-test-password", "server")
        .unwrap()
        .unwrap();
    let viewer = auth::authenticate(&state, "viewer", "nextup-viewer-test-password", "server")
        .unwrap()
        .unwrap();
    let router = app::router(Arc::new(AppState::with_auth_dir(
        "server".to_owned(),
        state.clone(),
    )));
    let admin_series = json(
        call(
            &router,
            "GET",
            "/Items?includeItemTypes=Series",
            Some(&admin.access_token),
        )
        .await,
    )
    .await;
    let records = admin_series["Items"].as_array().unwrap();
    let id_for = |name: &str| {
        records.iter().find(|item| item["Name"] == name).unwrap()["Id"]
            .as_str()
            .unwrap()
            .to_owned()
    };
    let series_id = id_for("Signal");
    let hidden_id = id_for("Hidden");
    let url = format!("/Shows/NextUp?seriesId={series_id}");
    assert_eq!(
        call(&router, "GET", &url, None).await.status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        call(&router, "GET", &url, Some(&viewer.access_token))
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    auth::grant_library(&state, "viewer", &allowed).unwrap();
    assert_eq!(
        call(
            &router,
            "GET",
            &format!("/Shows/NextUp?seriesId={hidden_id}"),
            Some(&viewer.access_token),
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
    let unstarted =
        json(call(&router, "GET", "/Shows/NextUp", Some(&viewer.access_token)).await).await;
    assert_eq!(unstarted["TotalRecordCount"], 0);
    for unsupported in [
        format!("{url}&enableRewatching=true"),
        format!("{url}&enableTotalRecordCount=false"),
        format!("{url}&nextUpDateCutoff=2026-01-01"),
        format!("{url}&parentId={series_id}"),
        format!("{url}&fields=MediaStreams"),
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
            &format!("{url}&userId={}", admin.user.id),
            Some(&viewer.access_token),
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );

    let episodes = json(
        call(
            &router,
            "GET",
            &format!("/Shows/{series_id}/Episodes"),
            Some(&viewer.access_token),
        )
        .await,
    )
    .await;
    let list = episodes["Items"].as_array().unwrap();
    let episode_id = |name: &str| {
        list.iter().find(|item| item["Name"] == name).unwrap()["Id"]
            .as_str()
            .unwrap()
            .to_owned()
    };
    let dawn = episode_id("Dawn");
    let noon = episode_id("Noon");
    let dusk = episode_id("Dusk");
    let morning = episode_id("Morning");
    let initial = json(call(&router, "GET", &url, Some(&viewer.access_token)).await).await;
    assert_eq!(initial["TotalRecordCount"], 1);
    assert_eq!(initial["Items"][0]["Name"], "Dawn");
    assert_eq!(initial["Items"][0]["Id"], dawn);

    assert_eq!(
        call(
            &router,
            "POST",
            &format!("/UserPlayedItems/{dawn}"),
            Some(&viewer.access_token),
        )
        .await
        .status(),
        StatusCode::OK
    );
    let after_dawn = json(call(&router, "GET", &url, Some(&viewer.access_token)).await).await;
    assert_eq!(after_dawn["Items"][0]["Name"], "Noon");
    let general =
        json(call(&router, "GET", "/Shows/NextUp", Some(&viewer.access_token)).await).await;
    assert_eq!(general["TotalRecordCount"], 1);
    assert_eq!(general["Items"][0]["Name"], "Noon");
    let paged = json(
        call(
            &router,
            "GET",
            &format!("{url}&startIndex=1"),
            Some(&viewer.access_token),
        )
        .await,
    )
    .await;
    assert_eq!(paged["TotalRecordCount"], 1);
    assert_eq!(paged["StartIndex"], 1);
    assert!(paged["Items"].as_array().unwrap().is_empty());
    let without_user_data = json(
        call(
            &router,
            "GET",
            &format!("{url}&enableUserData=false&enableImages=false"),
            Some(&viewer.access_token),
        )
        .await,
    )
    .await;
    assert_eq!(without_user_data["Items"][0]["Name"], "Noon");
    assert!(without_user_data["Items"][0].get("UserData").is_none());

    user_data::save_position(&state, &viewer.user.id, &noon, 12000000).unwrap();
    let resumable = json(call(&router, "GET", &url, Some(&viewer.access_token)).await).await;
    assert_eq!(resumable["Items"][0]["Name"], "Noon");
    let excluded_resume = json(
        call(
            &router,
            "GET",
            &format!("{url}&enableResumable=false"),
            Some(&viewer.access_token),
        )
        .await,
    )
    .await;
    assert_eq!(excluded_resume["TotalRecordCount"], 0);
    assert!(excluded_resume["Items"].as_array().unwrap().is_empty());
    assert_eq!(
        call(
            &router,
            "POST",
            &format!("/UserPlayedItems/{noon}"),
            Some(&viewer.access_token),
        )
        .await
        .status(),
        StatusCode::OK
    );
    let after_noon = json(call(&router, "GET", &url, Some(&viewer.access_token)).await).await;
    assert_eq!(after_noon["Items"][0]["Name"], "Dusk");

    // Following the pinned service, a later watched position takes precedence
    // over an earlier episode that remains unplayed.
    assert_eq!(
        call(
            &router,
            "POST",
            &format!("/UserPlayedItems/{dusk}"),
            Some(&viewer.access_token),
        )
        .await
        .status(),
        StatusCode::OK
    );
    let next_season = json(call(&router, "GET", &url, Some(&viewer.access_token)).await).await;
    assert_eq!(next_season["Items"][0]["Id"], morning);
    assert_eq!(next_season["Items"][0]["ParentIndexNumber"], 2);

    let restarted = app::router(Arc::new(AppState::with_auth_dir(
        "server".to_owned(),
        state.clone(),
    )));
    let persisted = json(call(&restarted, "GET", &url, Some(&viewer.access_token)).await).await;
    assert_eq!(persisted["Items"][0]["Id"], morning);
    auth::revoke_library(&state, "viewer", &allowed).unwrap();
    assert_eq!(
        call(&restarted, "GET", &url, Some(&viewer.access_token))
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
}
