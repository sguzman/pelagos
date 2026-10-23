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

async fn get(router: &Router, route: &str, token: Option<&str>) -> Response {
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
async fn shows_queries_keep_series_boundaries_season_order_and_private_accounts() {
    let fixture = tempfile::tempdir().unwrap();
    let state = fixture.path().join("state");
    let library = fixture.path().join("Television");
    for path in [
        "Signal/Season 00",
        "Signal/Season 01",
        "Signal/Season 02",
        "Other/Season 01",
    ] {
        std::fs::create_dir_all(library.join(path)).unwrap();
    }
    for path in [
        "Signal/Season 00/Signal - S00E01 - Special.mkv",
        "Signal/Season 01/Signal - S01E02 - Arrival.mkv",
        "Signal/Season 01/Signal - S01E01 - Pilot.mkv",
        "Signal/Season 02/Signal - S02E01 - Return.mkv",
        "Other/Season 01/Other - S01E01 - Other Pilot.mkv",
    ] {
        std::fs::write(library.join(path), b"synthetic video").unwrap();
    }
    catalog::index(&state, &library).unwrap();
    auth::create_user(&state, "admin", "admin-show-contract-password").unwrap();
    auth::create_user(&state, "viewer", "viewer-show-contract-password").unwrap();
    let admin = auth::authenticate(&state, "admin", "admin-show-contract-password", "server")
        .unwrap()
        .unwrap();
    let viewer = auth::authenticate(&state, "viewer", "viewer-show-contract-password", "server")
        .unwrap()
        .unwrap();
    let router = app::router(Arc::new(AppState::with_auth_dir(
        "server".to_owned(),
        state.clone(),
    )));
    let admin_list = json(
        get(
            &router,
            "/Items?includeItemTypes=Episode",
            Some(&admin.access_token),
        )
        .await,
    )
    .await;
    assert_eq!(admin_list["TotalRecordCount"], 5);
    let views = json(get(&router, "/UserViews", Some(&admin.access_token)).await).await;
    let collection_id = views["Items"][0]["Id"].as_str().unwrap();
    let series = json(
        get(
            &router,
            &format!("/Items?parentId={collection_id}"),
            Some(&admin.access_token),
        )
        .await,
    )
    .await;
    let series = series["Items"].as_array().unwrap();
    let signal = series.iter().find(|s| s["Name"] == "Signal").unwrap()["Id"]
        .as_str()
        .unwrap();
    let other = series.iter().find(|s| s["Name"] == "Other").unwrap()["Id"]
        .as_str()
        .unwrap();
    let seasons_url = format!("/Shows/{signal}/Seasons");
    let episodes_url = format!("/Shows/{signal}/Episodes");

    assert_eq!(
        get(&router, &seasons_url, None).await.status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        get(&router, &episodes_url, Some(&viewer.access_token))
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    auth::grant_library(&state, "viewer", &library).unwrap();
    let seasons = json(get(&router, &seasons_url, Some(&viewer.access_token)).await).await;
    assert_eq!(seasons["TotalRecordCount"], 3);
    let list = seasons["Items"].as_array().unwrap();
    assert_eq!(list[0]["IndexNumber"], 0);
    assert_eq!(list[1]["IndexNumber"], 1);
    assert_eq!(list[2]["IndexNumber"], 2);
    assert!(list.iter().all(|s| s["SeriesId"] == signal));
    assert!(list.iter().all(|s| s["UserData"]["Played"] == false));
    let season_one = list[1]["Id"].as_str().unwrap();
    let specials = json(
        get(
            &router,
            &format!("{seasons_url}?isSpecialSeason=true"),
            Some(&viewer.access_token),
        )
        .await,
    )
    .await;
    assert_eq!(specials["TotalRecordCount"], 1);
    assert_eq!(specials["Items"][0]["IndexNumber"], 0);
    let standard = json(
        get(
            &router,
            &format!("{seasons_url}?isSpecialSeason=false"),
            Some(&viewer.access_token),
        )
        .await,
    )
    .await;
    assert_eq!(standard["TotalRecordCount"], 2);

    let episodes = json(get(&router, &episodes_url, Some(&viewer.access_token)).await).await;
    assert_eq!(episodes["TotalRecordCount"], 4);
    let list = episodes["Items"].as_array().unwrap();
    assert_eq!(list[0]["Name"], "Special");
    assert_eq!(list[1]["Name"], "Pilot");
    assert_eq!(list[2]["Name"], "Arrival");
    assert_eq!(list[3]["Name"], "Return");
    assert!(list.iter().all(|item| item["SeriesId"] == signal));
    assert!(list.iter().all(|item| item["MediaType"] == "Video"));

    let filtered = json(
        get(
            &router,
            &format!("{episodes_url}?season=1&seasonId={season_one}"),
            Some(&viewer.access_token),
        )
        .await,
    )
    .await;
    assert_eq!(filtered["TotalRecordCount"], 2);
    assert_eq!(filtered["Items"][0]["Name"], "Pilot");
    assert_eq!(filtered["Items"][1]["Name"], "Arrival");
    assert_eq!(filtered["Items"][1]["SeasonId"], season_one);
    let page = json(
        get(
            &router,
            &format!("{episodes_url}?startIndex=1&limit=1&enableImages=false&enableUserData=false"),
            Some(&viewer.access_token),
        )
        .await,
    )
    .await;
    assert_eq!(page["TotalRecordCount"], 4);
    assert_eq!(page["StartIndex"], 1);
    assert_eq!(page["Items"][0]["Name"], "Pilot");
    assert!(page["Items"][0].get("UserData").is_none());
    assert!(page["Items"][0].get("ImageTags").is_none());

    for route in [
        format!("{episodes_url}?userId={}", admin.user.id),
        format!("{seasons_url}?userId={}", admin.user.id),
    ] {
        assert_eq!(
            get(&router, &route, Some(&viewer.access_token))
                .await
                .status(),
            StatusCode::FORBIDDEN
        );
    }
    for route in [
        format!("{episodes_url}?seasonId={other}"),
        "/Shows/00000000000000000000000000000000/Episodes".to_string(),
        "/Shows/00000000000000000000000000000000/Seasons".to_string(),
    ] {
        assert_eq!(
            get(&router, &route, Some(&viewer.access_token))
                .await
                .status(),
            StatusCode::NOT_FOUND
        );
    }
    for route in [
        format!("{episodes_url}?isMissing=true"),
        format!("{episodes_url}?fields=MediaStreams"),
        format!("{episodes_url}?startItemId={other}"),
        format!("{seasons_url}?adjacentTo={other}"),
    ] {
        assert_eq!(
            get(&router, &route, Some(&viewer.access_token))
                .await
                .status(),
            StatusCode::NOT_IMPLEMENTED
        );
    }

    auth::revoke_library(&state, "viewer", &library).unwrap();
    assert_eq!(
        get(&router, &episodes_url, Some(&viewer.access_token))
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
}
