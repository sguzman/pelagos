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

async fn get(router: &Router, uri: &str, token: Option<&str>) -> Response {
    let mut request = Request::builder().uri(uri);
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
async fn recognized_type_flags_surface_series_and_movies_without_unscoped_folder_leaks() {
    let fixture = tempfile::tempdir().unwrap();
    let state = fixture.path().join("state");
    let allowed = fixture.path().join("Allowed");
    let private = fixture.path().join("Private");
    std::fs::create_dir_all(allowed.join("Signal/Season 01")).unwrap();
    std::fs::create_dir_all(allowed.join("Film (2001)")).unwrap();
    std::fs::create_dir_all(private.join("Secret/Season 01")).unwrap();
    std::fs::write(
        allowed.join("Signal/Season 01/Signal - S01E01 - Pilot.mkv"),
        b"episode",
    )
    .unwrap();
    std::fs::write(allowed.join("Film (2001)/Film (2001).mp4"), b"movie").unwrap();
    std::fs::write(
        private.join("Secret/Season 01/Secret - S01E01.mkv"),
        b"private",
    )
    .unwrap();
    catalog::index(&state, &allowed).unwrap();
    catalog::index(&state, &private).unwrap();
    auth::create_user(&state, "admin", "admin-flags-test-password").unwrap();
    auth::create_user(&state, "viewer", "viewer-flags-test-password").unwrap();
    let admin = auth::authenticate(&state, "admin", "admin-flags-test-password", "server")
        .unwrap()
        .unwrap();
    let viewer = auth::authenticate(&state, "viewer", "viewer-flags-test-password", "server")
        .unwrap()
        .unwrap();
    let router = app::router(Arc::new(AppState::with_auth_dir(
        "server".to_owned(),
        state.clone(),
    )));
    assert_eq!(
        get(&router, "/Items?isSeries=true", None).await.status(),
        StatusCode::UNAUTHORIZED
    );
    auth::grant_library(&state, "viewer", &allowed).unwrap();
    let default = json(get(&router, "/Items", Some(&viewer.access_token)).await).await;
    assert_eq!(default["TotalRecordCount"], 2);
    let selected = json(
        get(&router, "/Items?isSeries=true", Some(&viewer.access_token)).await,
    )
    .await;
    assert_eq!(selected["TotalRecordCount"], 1);
    assert_eq!(selected["Items"][0]["Type"], "Series");
    assert_eq!(selected["Items"][0]["Name"], "Signal");
    let typed = json(
        get(
            &router,
            "/Items?includeItemTypes=Series",
            Some(&viewer.access_token),
        )
        .await,
    )
    .await;
    assert_eq!(typed["TotalRecordCount"], 1);
    assert_eq!(typed["Items"][0]["Id"], selected["Items"][0]["Id"]);
    let no_series = json(
        get(&router, "/Items?isSeries=false", Some(&viewer.access_token)).await,
    )
    .await;
    assert_eq!(no_series["TotalRecordCount"], 2);
    assert!(no_series["Items"].as_array().unwrap().iter().all(|i| i["Type"] != "Series"));
    let movie = json(
        get(&router, "/Items?isMovie=true", Some(&viewer.access_token)).await,
    )
    .await;
    assert_eq!(movie["TotalRecordCount"], 1);
    assert_eq!(movie["Items"][0]["Name"], "Film");
    let excluded = json(
        get(
            &router,
            "/Items?isSeries=true&includeItemTypes=Movie",
            Some(&viewer.access_token),
        )
        .await,
    )
    .await;
    assert_eq!(excluded["TotalRecordCount"], 0);

    let series_hint = json(
        get(
            &router,
            "/Search/Hints?searchTerm=Sign&isSeries=true",
            Some(&viewer.access_token),
        )
        .await,
    )
    .await;
    assert_eq!(series_hint["TotalRecordCount"], 1);
    assert_eq!(series_hint["SearchHints"][0]["Type"], "Series");
    let movie_hint = json(
        get(
            &router,
            "/Search/Hints?searchTerm=Film&isMovie=true",
            Some(&viewer.access_token),
        )
        .await,
    )
    .await;
    assert_eq!(movie_hint["TotalRecordCount"], 1);
    assert_eq!(movie_hint["SearchHints"][0]["Type"], "Movie");
    let mismatch = json(
        get(
            &router,
            "/Search/Hints?searchTerm=Sign&isMovie=true",
            Some(&viewer.access_token),
        )
        .await,
    )
    .await;
    assert_eq!(mismatch["TotalRecordCount"], 0);
    assert_eq!(
        get(
            &router,
            &format!("/Search/Hints?searchTerm=Sign&userId={}", admin.user.id),
            Some(&viewer.access_token),
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        get(
            &router,
            "/Search/Hints?searchTerm=Sign&isNews=true",
            Some(&viewer.access_token),
        )
        .await
        .status(),
        StatusCode::NOT_IMPLEMENTED
    );

    auth::revoke_library(&state, "viewer", &allowed).unwrap();
    let gone = json(
        get(&router, "/Items?isSeries=true", Some(&viewer.access_token)).await,
    )
    .await;
    assert_eq!(gone["TotalRecordCount"], 0);
    let hidden = json(
        get(
            &router,
            "/Search/Hints?searchTerm=Sign&isSeries=true",
            Some(&viewer.access_token),
        )
        .await,
    )
    .await;
    assert_eq!(hidden["TotalRecordCount"], 0);
}
