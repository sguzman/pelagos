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
async fn virtual_root_is_stable_browsable_and_grant_scoped() {
    let fixture = tempfile::tempdir().unwrap();
    let state = fixture.path().join("state");
    let allowed = fixture.path().join("Allowed");
    let private = fixture.path().join("Private");
    std::fs::create_dir_all(allowed.join("Season")).unwrap();
    std::fs::create_dir(&private).unwrap();
    std::fs::write(allowed.join("Season/Episode.mkv"), b"synthetic").unwrap();
    std::fs::write(private.join("Secret.mp4"), b"synthetic").unwrap();
    catalog::index(&state, &allowed).unwrap();
    catalog::index(&state, &private).unwrap();

    auth::create_user(&state, "admin", "root-admin-test-password").unwrap();
    auth::create_user(&state, "viewer", "root-viewer-test-password").unwrap();
    let admin = auth::authenticate(&state, "admin", "root-admin-test-password", "server")
        .unwrap()
        .unwrap();
    let viewer = auth::authenticate(&state, "viewer", "root-viewer-test-password", "server")
        .unwrap()
        .unwrap();
    let router = app::router(Arc::new(AppState::with_auth_dir(
        "server".to_owned(),
        state.clone(),
    )));
    assert_eq!(
        get(&router, "/Items/Root", None).await.status(),
        StatusCode::UNAUTHORIZED
    );

    let root = json(get(&router, "/Items/Root", Some(&viewer.access_token)).await).await;
    assert_eq!(root["Name"], "Media");
    assert_eq!(root["Type"], "Folder");
    assert_eq!(root["IsFolder"], true);
    assert_eq!(root["ServerId"], "server");
    assert!(root.get("Path").is_none());
    assert!(root.get("ParentId").is_none());
    assert!(root.get("ImageTags").is_none());
    let root_id = root["Id"].as_str().unwrap();

    let admin_root = json(get(&router, "/Items/Root", Some(&admin.access_token)).await).await;
    assert_eq!(admin_root["Id"], root_id);
    let legacy = json(
        get(
            &router,
            &format!("/Users/{}/Items/Root", viewer.user.id),
            Some(&viewer.access_token),
        )
        .await,
    )
    .await;
    assert_eq!(legacy, root);
    let direct = json(
        get(
            &router,
            &format!("/Items/{root_id}"),
            Some(&viewer.access_token),
        )
        .await,
    )
    .await;
    assert_eq!(direct["Id"], root_id);
    let no_grants = json(
        get(
            &router,
            &format!("/Items?parentId={root_id}"),
            Some(&viewer.access_token),
        )
        .await,
    )
    .await;
    assert_eq!(no_grants["TotalRecordCount"], 0);
    assert_eq!(
        get(
            &router,
            &format!("/Items/Root?userId={}", admin.user.id),
            Some(&viewer.access_token),
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        get(
            &router,
            &format!("/Users/{}/Items/Root", admin.user.id),
            Some(&viewer.access_token),
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );

    auth::grant_library(&state, "viewer", &allowed).unwrap();
    let children = json(
        get(
            &router,
            &format!("/Items?parentId={root_id}"),
            Some(&viewer.access_token),
        )
        .await,
    )
    .await;
    assert_eq!(children["TotalRecordCount"], 1);
    assert_eq!(children["Items"][0]["Name"], "Allowed");
    assert_eq!(children["Items"][0]["ParentId"], root_id);
    assert_eq!(children["Items"][0]["Type"], "CollectionFolder");
    assert!(children["Items"][0].get("Path").is_none());
    let descendants = json(
        get(
            &router,
            &format!("/Items?parentId={root_id}&recursive=true"),
            Some(&viewer.access_token),
        )
        .await,
    )
    .await;
    assert_eq!(descendants["TotalRecordCount"], 1);
    assert_eq!(descendants["Items"][0]["Name"], "Episode");
    let owned_id = descendants["Items"][0]["Id"].as_str().unwrap();
    let ancestors = json(
        get(
            &router,
            &format!("/Items/{owned_id}/Ancestors"),
            Some(&viewer.access_token),
        )
        .await,
    )
    .await;
    assert_eq!(ancestors.as_array().unwrap().len(), 3);
    assert_eq!(ancestors[0]["Name"], "Season");
    assert_eq!(ancestors[1]["Name"], "Allowed");
    assert_eq!(ancestors[2]["Id"], root_id);
    let admin_libraries = json(
        get(
            &router,
            &format!("/Items?parentId={root_id}"),
            Some(&admin.access_token),
        )
        .await,
    )
    .await;
    assert_eq!(admin_libraries["TotalRecordCount"], 2);
    assert_eq!(admin_libraries["Items"][0]["Name"], "Allowed");
    assert_eq!(admin_libraries["Items"][1]["Name"], "Private");

    let restarted = app::router(Arc::new(AppState::with_auth_dir(
        "server".to_owned(),
        state.clone(),
    )));
    let after_restart =
        json(get(&restarted, "/Items/Root", Some(&viewer.access_token)).await).await;
    assert_eq!(after_restart["Id"], root_id);

    auth::revoke_library(&state, "viewer", &allowed).unwrap();
    let gone = json(
        get(
            &restarted,
            &format!("/Items?parentId={root_id}"),
            Some(&viewer.access_token),
        )
        .await,
    )
    .await;
    assert_eq!(gone["TotalRecordCount"], 0);
    assert_eq!(
        get(
            &restarted,
            &format!("/Items/{owned_id}"),
            Some(&viewer.access_token),
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
}
