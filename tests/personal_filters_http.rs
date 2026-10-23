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
async fn played_and_favorite_filters_apply_before_paging_without_cross_user_state() {
    let fixture = tempfile::tempdir().unwrap();
    let state = fixture.path().join("state");
    let allowed = fixture.path().join("Allowed");
    std::fs::create_dir(&allowed).unwrap();
    for name in ["Alpha", "Beta", "Gamma", "Delta"] {
        std::fs::write(allowed.join(format!("{name}.mp4")), b"test video").unwrap();
    }
    catalog::index(&state, &allowed).unwrap();
    auth::create_user(&state, "admin", "filter-admin-test-password").unwrap();
    auth::create_user(&state, "viewer", "filter-viewer-test-password").unwrap();
    let admin = auth::authenticate(&state, "admin", "filter-admin-test-password", "server")
        .unwrap()
        .unwrap();
    let viewer = auth::authenticate(&state, "viewer", "filter-viewer-test-password", "server")
        .unwrap()
        .unwrap();
    let router = app::router(Arc::new(AppState::with_auth_dir(
        "server".to_owned(),
        state.clone(),
    )));
    assert_eq!(
        call(&router, "GET", "/Items?isPlayed=true", None)
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    auth::grant_library(&state, "viewer", &allowed).unwrap();

    let initial = json(call(&router, "GET", "/Items", Some(&viewer.access_token)).await).await;
    let items = initial["Items"].as_array().unwrap();
    let id = |name: &str| {
        items.iter().find(|i| i["Name"] == name).unwrap()["Id"]
            .as_str()
            .unwrap()
            .to_owned()
    };
    for (route, item_id) in [
        ("/UserFavoriteItems", id("Alpha")),
        ("/UserPlayedItems", id("Beta")),
        ("/UserPlayedItems", id("Gamma")),
        ("/UserFavoriteItems", id("Gamma")),
    ] {
        assert_eq!(
            call(
                &router,
                "POST",
                &format!("{route}/{item_id}"),
                Some(&viewer.access_token),
            )
            .await
            .status(),
            StatusCode::OK
        );
    }

    let played = json(
        call(
            &router,
            "GET",
            "/Items?isPlayed=true&limit=1",
            Some(&viewer.access_token),
        )
        .await,
    )
    .await;
    assert_eq!(played["TotalRecordCount"], 2);
    assert_eq!(played["Items"][0]["Name"], "Beta");
    let favorite = json(
        call(
            &router,
            "GET",
            "/Items?isFavorite=true&enableUserData=false",
            Some(&viewer.access_token),
        )
        .await,
    )
    .await;
    assert_eq!(favorite["TotalRecordCount"], 2);
    assert_eq!(favorite["Items"][0]["Name"], "Alpha");
    assert_eq!(favorite["Items"][1]["Name"], "Gamma");
    assert!(favorite["Items"][0].get("UserData").is_none());

    let combined = json(
        call(
            &router,
            "GET",
            "/Items?isPlayed=true&isFavorite=true",
            Some(&viewer.access_token),
        )
        .await,
    )
    .await;
    assert_eq!(combined["TotalRecordCount"], 1);
    assert_eq!(combined["Items"][0]["Name"], "Gamma");
    assert_eq!(combined["Items"][0]["UserData"]["Played"], true);
    assert_eq!(combined["Items"][0]["UserData"]["IsFavorite"], true);
    let both_false = json(
        call(
            &router,
            "GET",
            "/Items?isPlayed=false&isFavorite=false",
            Some(&viewer.access_token),
        )
        .await,
    )
    .await;
    assert_eq!(both_false["TotalRecordCount"], 1);
    assert_eq!(both_false["Items"][0]["Name"], "Delta");
    let search = json(
        call(
            &router,
            "GET",
            "/Search/Hints?searchTerm=a&isFavorite=true&isPlayed=true",
            Some(&viewer.access_token),
        )
        .await,
    )
    .await;
    assert_eq!(search["TotalRecordCount"], 1);
    assert_eq!(search["SearchHints"][0]["Name"], "Gamma");
    let search_page = json(
        call(
            &router,
            "GET",
            "/Search/Hints?searchTerm=a&isPlayed=true&limit=1&startIndex=1",
            Some(&viewer.access_token),
        )
        .await,
    )
    .await;
    assert_eq!(search_page["TotalRecordCount"], 2);
    assert_eq!(search_page["SearchHints"][0]["Name"], "Gamma");
    let search_false = json(
        call(
            &router,
            "GET",
            "/Search/Hints?searchTerm=a&isFavorite=false&isPlayed=false&includeItemTypes=Video",
            Some(&viewer.access_token),
        )
        .await,
    )
    .await;
    assert_eq!(search_false["TotalRecordCount"], 1);
    assert_eq!(search_false["SearchHints"][0]["Name"], "Delta");
    for route in [
        format!("/Items?isFavorite=true&userId={}", admin.user.id),
        format!(
            "/Search/Hints?searchTerm=a&isPlayed=true&userId={}",
            admin.user.id
        ),
    ] {
        assert_eq!(
            call(&router, "GET", &route, Some(&viewer.access_token))
                .await
                .status(),
            StatusCode::FORBIDDEN
        );
    }
    let other = json(
        call(
            &router,
            "GET",
            "/Items?isFavorite=true",
            Some(&admin.access_token),
        )
        .await,
    )
    .await;
    assert_eq!(other["TotalRecordCount"], 0);
    let restarted = app::router(Arc::new(AppState::with_auth_dir(
        "server".to_owned(),
        state.clone(),
    )));
    let persisted = json(
        call(
            &restarted,
            "GET",
            "/Items?isPlayed=true&isFavorite=true",
            Some(&viewer.access_token),
        )
        .await,
    )
    .await;
    assert_eq!(persisted["Items"][0]["Name"], "Gamma");
    auth::revoke_library(&state, "viewer", &allowed).unwrap();
    let gone = json(
        call(
            &restarted,
            "GET",
            "/Items?isPlayed=true&isFavorite=true",
            Some(&viewer.access_token),
        )
        .await,
    )
    .await;
    assert_eq!(gone["TotalRecordCount"], 0);
    let hidden = json(
        call(
            &restarted,
            "GET",
            "/Search/Hints?searchTerm=a&isFavorite=true",
            Some(&viewer.access_token),
        )
        .await,
    )
    .await;
    assert_eq!(hidden["TotalRecordCount"], 0);
}
