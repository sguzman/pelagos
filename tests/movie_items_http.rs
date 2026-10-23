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
async fn conservative_movies_are_filterable_private_and_keep_stable_media_identity() {
    let fixture = tempfile::tempdir().unwrap();
    let state = fixture.path().join("state");
    let movies = fixture.path().join("Films");
    for folder in ["Solaris (1972)", "Café (2024)", "Unrelated (2020)"] {
        std::fs::create_dir_all(movies.join(folder)).unwrap();
    }
    std::fs::write(
        movies.join("Solaris (1972)/Solaris (1972).mkv"),
        b"synthetic",
    )
    .unwrap();
    std::fs::write(movies.join("Café (2024)/Café (2024).mp4"), b"synthetic").unwrap();
    std::fs::write(movies.join("Unrelated (2020)/Loose (2020).mkv"), b"generic").unwrap();
    std::fs::write(movies.join("Unsorted.mp4"), b"generic").unwrap();
    catalog::index(&state, &movies).unwrap();

    auth::create_user(&state, "admin", "movie-admin-test-password").unwrap();
    auth::create_user(&state, "viewer", "movie-viewer-test-password").unwrap();
    let admin = auth::authenticate(&state, "admin", "movie-admin-test-password", "server")
        .unwrap()
        .unwrap();
    let viewer = auth::authenticate(&state, "viewer", "movie-viewer-test-password", "server")
        .unwrap()
        .unwrap();
    let router = app::router(Arc::new(AppState::with_auth_dir(
        "server".to_owned(),
        state.clone(),
    )));
    assert_eq!(
        get(&router, "/Items?includeItemTypes=Movie", None)
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    let hidden = json(
        get(
            &router,
            "/Items?includeItemTypes=Movie",
            Some(&viewer.access_token),
        )
        .await,
    )
    .await;
    assert_eq!(hidden["TotalRecordCount"], 0);
    auth::grant_library(&state, "viewer", &movies).unwrap();

    let recognized = json(
        get(
            &router,
            "/Items?includeItemTypes=Movie",
            Some(&viewer.access_token),
        )
        .await,
    )
    .await;
    assert_eq!(recognized["TotalRecordCount"], 2);
    let items = recognized["Items"].as_array().unwrap();
    let cafe = items.iter().find(|item| item["Name"] == "Café").unwrap();
    assert_eq!(cafe["Type"], "Movie");
    assert_eq!(cafe["MediaType"], "Video");
    assert_eq!(cafe["ProductionYear"], 2024);
    assert_eq!(cafe["Container"], "mp4");
    assert!(cafe.get("SeriesId").is_none());
    assert!(cafe.get("Path").is_none());
    let movie_id = cafe["Id"].as_str().unwrap();
    let source = json(
        get(
            &router,
            &format!("/Items/{movie_id}"),
            Some(&viewer.access_token),
        )
        .await,
    )
    .await;
    assert_eq!(source["Id"], movie_id);
    assert_eq!(source["ProductionYear"], 2024);

    let by_flag = json(get(&router, "/Items?isMovie=true", Some(&viewer.access_token)).await).await;
    assert_eq!(by_flag["TotalRecordCount"], 2);
    let other = json(get(&router, "/Items?isMovie=false", Some(&viewer.access_token)).await).await;
    assert_eq!(other["TotalRecordCount"], 2);
    assert!(
        other["Items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|item| { item["Type"] == "Video" && item.get("ProductionYear").is_none() })
    );
    let hints = json(
        get(
            &router,
            "/Search/Hints?searchTerm=Caf&includeItemTypes=Movie",
            Some(&viewer.access_token),
        )
        .await,
    )
    .await;
    assert_eq!(hints["TotalRecordCount"], 1);
    assert_eq!(hints["SearchHints"][0]["Name"], "Café");
    assert_eq!(hints["SearchHints"][0]["Type"], "Movie");
    user_data::save_position(&state, &viewer.user.id, movie_id, 10000000).unwrap();
    let resume = json(get(&router, "/UserItems/Resume", Some(&viewer.access_token)).await).await;
    assert_eq!(resume["TotalRecordCount"], 1);
    assert_eq!(resume["Items"][0]["Type"], "Movie");

    auth::revoke_library(&state, "viewer", &movies).unwrap();
    assert_eq!(
        get(
            &router,
            &format!("/Items/{movie_id}"),
            Some(&viewer.access_token),
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
    let admin_items = json(
        get(
            &router,
            "/Items?includeItemTypes=Movie",
            Some(&admin.access_token),
        )
        .await,
    )
    .await;
    assert_eq!(admin_items["TotalRecordCount"], 2);
}
