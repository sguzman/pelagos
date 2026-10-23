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
    let body = to_bytes(response.into_body(), 65536).await.unwrap();
    serde_json::from_slice(&body).unwrap()
}

#[tokio::test]
async fn ancestors_follow_immediate_parent_first_without_exposing_other_libraries() {
    let fixture = tempfile::tempdir().unwrap();
    let state = fixture.path().join("state");
    let allowed = fixture.path().join("Allowed");
    let private = fixture.path().join("Private");
    std::fs::create_dir_all(allowed.join("Show/Season One")).unwrap();
    std::fs::create_dir(&private).unwrap();
    std::fs::write(allowed.join("Show/Season One/Episode.mkv"), b"fixture").unwrap();
    std::fs::write(private.join("Secret.mkv"), b"fixture").unwrap();
    catalog::index(&state, &allowed).unwrap();
    catalog::index(&state, &private).unwrap();

    auth::create_user(&state, "admin", "admin-ancestor-test-password").unwrap();
    auth::create_user(&state, "viewer", "viewer-ancestor-test-password").unwrap();
    let admin = auth::authenticate(&state, "admin", "admin-ancestor-test-password", "server")
        .unwrap()
        .unwrap();
    let viewer = auth::authenticate(&state, "viewer", "viewer-ancestor-test-password", "server")
        .unwrap()
        .unwrap();
    let router = app::router(Arc::new(AppState::with_auth_dir(
        "server".to_owned(),
        state.clone(),
    )));

    let items = json(get(&router, "/Items", Some(&admin.access_token)).await).await;
    let media = items["Items"].as_array().unwrap();
    let episode = media.iter().find(|item| item["Name"] == "Episode").unwrap();
    let episode_id = episode["Id"].as_str().unwrap();
    let secret_id = media.iter().find(|item| item["Name"] == "Secret").unwrap()["Id"]
        .as_str()
        .unwrap();
    let path = format!("/Items/{episode_id}/Ancestors");

    assert_eq!(
        get(&router, &path, None).await.status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        get(&router, &path, Some(&viewer.access_token))
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    assert!(auth::grant_library(&state, "viewer", &allowed).unwrap());
    let parents = json(get(&router, &path, Some(&viewer.access_token)).await).await;
    let parents = parents.as_array().unwrap();
    let names: Vec<_> = parents
        .iter()
        .map(|item| item["Name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["Season One", "Show", "Allowed", "Media"]);
    assert_eq!(parents[0]["ParentId"], parents[1]["Id"]);
    assert_eq!(parents[1]["ParentId"], parents[2]["Id"]);
    assert_eq!(parents[2]["ParentId"], parents[3]["Id"]);
    assert!(parents[3].get("ParentId").is_none());
    assert_eq!(parents[2]["Type"], "CollectionFolder");
    assert_eq!(parents[0]["UserData"]["Played"], false);
    for parent in parents {
        assert!(parent.get("Path").is_none());
    }

    let views = json(get(&router, "/UserViews", Some(&viewer.access_token)).await).await;
    let root_id = views["Items"][0]["Id"].as_str().unwrap();
    let root_parents = json(
        get(
            &router,
            &format!("/Items/{root_id}/Ancestors"),
            Some(&viewer.access_token),
        )
        .await,
    )
    .await;
    assert_eq!(root_parents.as_array().unwrap().len(), 1);
    assert_eq!(root_parents[0]["Name"], "Media");

    assert_eq!(
        get(
            &router,
            &format!("/Items/{secret_id}/Ancestors"),
            Some(&viewer.access_token),
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        get(
            &router,
            &format!("/Items/{episode_id}/Ancestors?userId={}", admin.user.id),
            Some(&viewer.access_token),
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        get(
            &router,
            "/Items/00000000000000000000000000000000/Ancestors",
            Some(&viewer.access_token),
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
    assert!(auth::revoke_library(&state, "viewer", &allowed).unwrap());
    assert_eq!(
        get(&router, &path, Some(&viewer.access_token))
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
}
