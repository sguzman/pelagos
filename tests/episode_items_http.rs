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
async fn recognized_episode_names_build_series_and_season_dtos_without_guessing_other_video() {
    let temp = tempfile::tempdir().unwrap();
    let state = temp.path().join("state");
    let media = temp.path().join("Television");
    std::fs::create_dir_all(media.join("Signal/Season 01")).unwrap();
    std::fs::create_dir(media.join("Extras")).unwrap();
    std::fs::write(
        media.join("Signal/Season 01/Signal - S01E02 - Arrival.mkv"),
        b"synthetic episode",
    )
    .unwrap();
    std::fs::write(
        media.join("Signal/Season 01/Signal - S01E03 - Aftermath.mkv"),
        b"synthetic episode",
    )
    .unwrap();
    std::fs::write(media.join("Signal/Season 01/Featurette.mp4"), b"video").unwrap();
    std::fs::write(media.join("Extras/Unsorted S01E01.mp4"), b"video").unwrap();
    std::fs::write(media.join("Film.mp4"), b"video").unwrap();
    catalog::index(&state, &media).unwrap();

    auth::create_user(&state, "admin", "admin-episode-test-password").unwrap();
    auth::create_user(&state, "viewer", "viewer-episode-test-password").unwrap();
    let admin = auth::authenticate(&state, "admin", "admin-episode-test-password", "server")
        .unwrap()
        .unwrap();
    let viewer = auth::authenticate(&state, "viewer", "viewer-episode-test-password", "server")
        .unwrap()
        .unwrap();
    let router = app::router(Arc::new(AppState::with_auth_dir(
        "server".to_owned(),
        state.clone(),
    )));

    assert_eq!(
        json(
            get(
                &router,
                "/Items?includeItemTypes=Episode",
                Some(&viewer.access_token)
            )
            .await
        )
        .await["TotalRecordCount"],
        0
    );
    auth::grant_library(&state, "viewer", &media).unwrap();
    let views = json(get(&router, "/UserViews", Some(&viewer.access_token)).await).await;
    let collection_id = views["Items"][0]["Id"].as_str().unwrap();
    let children = json(
        get(
            &router,
            &format!("/Items?parentId={collection_id}"),
            Some(&viewer.access_token),
        )
        .await,
    )
    .await;
    let folders = children["Items"].as_array().unwrap();
    let signal = folders
        .iter()
        .find(|item| item["Name"] == "Signal")
        .unwrap();
    assert_eq!(signal["Type"], "Series");
    assert!(signal["IsFolder"].as_bool().unwrap());
    let series_id = signal["Id"].as_str().unwrap();
    let season = json(
        get(
            &router,
            &format!("/Items?parentId={series_id}"),
            Some(&viewer.access_token),
        )
        .await,
    )
    .await;
    assert_eq!(season["TotalRecordCount"], 1);
    assert_eq!(season["Items"][0]["Type"], "Season");
    assert_eq!(season["Items"][0]["IndexNumber"], 1);
    let season_id = season["Items"][0]["Id"].as_str().unwrap();
    let episodes = json(
        get(
            &router,
            &format!("/Items?parentId={season_id}"),
            Some(&viewer.access_token),
        )
        .await,
    )
    .await;
    assert_eq!(episodes["TotalRecordCount"], 3);
    let items = episodes["Items"].as_array().unwrap();
    let arrival = items.iter().find(|item| item["Name"] == "Arrival").unwrap();
    assert_eq!(arrival["Type"], "Episode");
    assert_eq!(arrival["MediaType"], "Video");
    assert_eq!(arrival["SeriesName"], "Signal");
    assert_eq!(arrival["IndexNumber"], 2);
    assert_eq!(arrival["ParentIndexNumber"], 1);
    assert_eq!(arrival["ParentId"], season_id);
    assert!(arrival.get("Path").is_none());
    let episode_id = arrival["Id"].as_str().unwrap();
    let featurette = items
        .iter()
        .find(|item| item["Name"] == "Featurette")
        .unwrap();
    assert_eq!(featurette["Type"], "Video");
    assert!(featurette.get("SeriesName").is_none());

    let only_episodes = json(
        get(
            &router,
            "/Items?includeItemTypes=Episode",
            Some(&viewer.access_token),
        )
        .await,
    )
    .await;
    assert_eq!(only_episodes["TotalRecordCount"], 2);
    let hints = json(
        get(
            &router,
            "/Search/Hints?searchTerm=Arrival&includeItemTypes=Episode",
            Some(&viewer.access_token),
        )
        .await,
    )
    .await;
    assert_eq!(hints["TotalRecordCount"], 1);
    assert_eq!(hints["SearchHints"][0]["Type"], "Episode");
    let unclassified = json(
        get(
            &router,
            "/Items?includeItemTypes=Video",
            Some(&viewer.access_token),
        )
        .await,
    )
    .await;
    assert_eq!(unclassified["TotalRecordCount"], 3);
    user_data::save_position(&state, &viewer.user.id, episode_id, 10_000_000).unwrap();
    let resume = json(get(&router, "/UserItems/Resume", Some(&viewer.access_token)).await).await;
    assert_eq!(resume["TotalRecordCount"], 1);
    assert_eq!(resume["Items"][0]["Type"], "Episode");

    auth::revoke_library(&state, "viewer", &media).unwrap();
    assert_eq!(
        get(
            &router,
            &format!("/Items/{episode_id}"),
            Some(&viewer.access_token),
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
    let admin_view = json(
        get(
            &router,
            "/Items?includeItemTypes=Episode",
            Some(&admin.access_token),
        )
        .await,
    )
    .await;
    assert_eq!(admin_view["TotalRecordCount"], 2);
}
