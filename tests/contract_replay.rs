//! Deterministic, synthetic protocol replay. Fixture data contains no
//! user media, real credentials, or captured private Jellyfin responses.
//! This is a Pelagos contract-harness scaffold, not an upstream golden oracle.
use std::{collections::BTreeMap, sync::Arc};

use axum::{
    Router,
    body::{Body, to_bytes},
    http::Request,
};
use pelagos::{
    app::{self, AppState},
    auth,
};
use serde::Deserialize;
use serde_json::Value;
use tower::ServiceExt;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReplayCorpus {
    pinned_reference: String,
    cases: Vec<ReplayCase>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReplayCase {
    name: String,
    method: String,
    path: String,
    auth: String,
    status: u16,
    #[serde(default)]
    headers: BTreeMap<String, String>,
    #[serde(default)]
    body: Option<Value>,
    #[serde(default)]
    content_type_prefix: Option<String>,
    #[serde(default)]
    text_exact: Option<String>,
    #[serde(default)]
    json_types: BTreeMap<String, String>,
}

fn assert_value_type(case: &str, path: &str, value: &Value, expected: &str) {
    let correct = match expected {
        "string" => value.is_string(),
        "bool" => value.is_boolean(),
        "number" => value.is_number(),
        "array" => value.is_array(),
        "object" => value.is_object(),
        "null" => value.is_null(),
        _ => panic!("{case}: unsupported fixture type {expected}"),
    };
    assert!(correct, "{case}: {path} expected {expected}, found {value}");
}

async fn replay(router: &Router, corpus: &ReplayCorpus, user_token: &str) {
    assert_eq!(
        corpus.pinned_reference,
        "jellyfin/jellyfin@06de749d435e2d0be0c5119a41f2300fdd7143e5"
    );
    assert!(!corpus.cases.is_empty());
    for case in &corpus.cases {
        assert!(case.path.starts_with('/'), "{}: invalid path", case.name);
        assert!(!case.name.is_empty());
        let mut builder = Request::builder().method(case.method.as_str()).uri(&case.path);
        for (key, value) in &case.headers {
            builder = builder.header(key, value);
        }
        match case.auth.as_str() {
            "none" => {}
            "synthetic-viewer" => {
                builder = builder.header("X-Emby-Token", user_token);
            }
            other => panic!("{}: invalid fixture auth label {other}", case.name),
        }
        let payload = case.body.as_ref().map(Value::to_string).unwrap_or_default();
        if case.body.is_some() {
            builder = builder.header("Content-Type", "application/json");
        }
        let response = router
            .clone()
            .oneshot(builder.body(Body::from(payload)).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status().as_u16(), case.status, "{}", case.name);
        if let Some(prefix) = &case.content_type_prefix {
            let found = response
                .headers()
                .get("content-type")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("");
            assert!(
                found.starts_with(prefix),
                "{}: content-type {found} missing {prefix}",
                case.name
            );
        }
        let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
        if let Some(expected) = &case.text_exact {
            assert_eq!(bytes.as_ref(), expected.as_bytes(), "{}", case.name);
        }
        if !case.json_types.is_empty() {
            let body: Value = serde_json::from_slice(&bytes).unwrap();
            for (pointer, expected) in &case.json_types {
                let value = body.pointer(pointer).unwrap_or_else(|| {
                    panic!("{}: missing response JSON pointer {pointer}", case.name)
                });
                assert_value_type(&case.name, pointer, value, expected);
            }
        }
    }
}

#[tokio::test]
async fn synthetic_pinned_controller_contract_replay() {
    let corpus: ReplayCorpus =
        serde_json::from_str(include_str!("fixtures/pinned_public_replay.json")).unwrap();
    let isolated = tempfile::tempdir().unwrap();
    let auth_dir = isolated.path().join("state");
    auth::create_user(&auth_dir, "fixture-viewer", "synthetic-fixture-password").unwrap();
    let token = auth::authenticate(
        &auth_dir,
        "fixture-viewer",
        "synthetic-fixture-password",
        "fixture-server",
    )
    .unwrap()
    .unwrap()
    .access_token;
    let router = app::router(Arc::new(AppState::with_auth_dir(
        "fixture-server".to_owned(),
        auth_dir,
    )));
    replay(&router, &corpus, &token).await;
}
