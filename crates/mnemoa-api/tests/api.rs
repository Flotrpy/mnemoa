use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use mnemoa_api::{AppState, router};
use mnemoa_core::config::Config;
use serde_json::{Value, json};
use tower::ServiceExt;

struct Fixture {
    _root: tempfile::TempDir,
    _outside: tempfile::TempDir,
    app: axum::Router,
}

fn fixture(allow_write: bool) -> Fixture {
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let repo = root.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    std::fs::write(
        repo.join("AGENTS.md"),
        "- Read [x](missing.md) before releases.\n- Use 2 spaces for indentation.\n",
    )
    .unwrap();
    std::fs::write(outside.path().join("AGENTS.md"), "- secret\n").unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(outside.path(), root.path().join("escape")).unwrap();
    let mut config = Config::default();
    config.api.root = root.path().to_path_buf();
    config.api.allow_write = allow_write;
    let state = Arc::new(AppState::new(config).unwrap());
    Fixture {
        _root: root,
        _outside: outside,
        app: router(state),
    }
}

async fn call(
    app: &axum::Router,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let req = Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json");
    let req = match body {
        Some(b) => req.body(Body::from(b.to_string())).unwrap(),
        None => req.body(Body::empty()).unwrap(),
    };
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

#[tokio::test]
async fn health_reports_version_and_llm() {
    let f = fixture(false);
    let (s, v) = call(&f.app, "GET", "/health", None).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(v["status"], "ok");
    assert_eq!(v["version"], mnemoa_core::VERSION);
    assert_eq!(v["llm"]["enabled"], false);
    assert_eq!(v["allow_write"], false);
}

#[tokio::test]
async fn scan_budget_drift_and_stats() {
    let f = fixture(false);
    let (s, v) = call(&f.app, "POST", "/scan", Some(json!({"path": "repo"}))).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["summary"]["errors"], 1);
    let (s, v) = call(
        &f.app,
        "POST",
        "/budget",
        Some(json!({"path": "repo", "targets": ["codex"]})),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(v["targets"][0]["target"], "codex");
    let (s, v) = call(&f.app, "POST", "/drift", Some(json!({"path": "repo"}))).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(v["items"][0]["kind"], "missing-path");
    let (_, v) = call(&f.app, "GET", "/stats", None).await;
    assert_eq!(v["scans"], 1);
    assert_eq!(v["findings_by_rule"]["MNA007"], 1);
    assert!(v["requests"].as_u64().unwrap() >= 4);
}

#[tokio::test]
async fn path_traversal_is_rejected() {
    let f = fixture(false);
    for path in ["../", "repo/../..", "/etc", "escape"] {
        let (s, v) = call(&f.app, "POST", "/scan", Some(json!({"path": path}))).await;
        assert!(
            s == StatusCode::FORBIDDEN || s == StatusCode::BAD_REQUEST,
            "{path}: {s} {v}"
        );
        assert!(v["error"].is_string());
    }
    // A symlink escape is refused specifically as outside the root.
    #[cfg(unix)]
    {
        let (s, v) = call(&f.app, "POST", "/scan", Some(json!({"path": "escape"}))).await;
        assert_eq!(s, StatusCode::FORBIDDEN, "{v}");
        assert!(v["error"].as_str().unwrap().contains("outside"));
    }
    let (s, _) = call(&f.app, "POST", "/scan", Some(json!({"path": "nope"}))).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn apply_needs_allow_write() {
    let f = fixture(false);
    let (s, v) = call(
        &f.app,
        "POST",
        "/promote",
        Some(json!({"path": "repo", "apply": true})),
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    assert!(
        v["error"]
            .as_str()
            .unwrap()
            .contains("MNEMOA_API_ALLOW_WRITE")
    );
    let (s, _) = call(
        &f.app,
        "POST",
        "/forget",
        Some(json!({"path": "repo", "apply": true})),
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    // Dry runs are fine.
    let (s, v) = call(&f.app, "POST", "/promote", Some(json!({"path": "repo"}))).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(v["promotions"][0]["enforcer"], "editorconfig");
    assert_eq!(v["written"], json!([]));
}

#[tokio::test]
async fn apply_with_allow_write_writes() {
    let f = fixture(true);
    let (s, v) = call(
        &f.app,
        "POST",
        "/promote",
        Some(json!({"path": "repo", "apply": true})),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert!(
        v["written"]
            .as_array()
            .unwrap()
            .iter()
            .any(|w| w == ".editorconfig")
    );
    let (s, v) = call(&f.app, "POST", "/forget", Some(json!({"path": "repo"}))).await;
    assert_eq!(s, StatusCode::OK);
    assert!(v["tokens"].is_array());
}

#[tokio::test]
async fn rules_endpoints() {
    let f = fixture(false);
    let (s, v) = call(&f.app, "GET", "/rules", None).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(v.as_array().unwrap().len(), 13);
    let (s, v) = call(&f.app, "GET", "/rules/mna006", None).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(v["name"], "blind-reference");
    let (s, _) = call(&f.app, "GET", "/rules/MNA999", None).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn llm_without_feature_or_config_is_a_bad_request() {
    let f = fixture(false);
    let (s, _) = call(
        &f.app,
        "POST",
        "/promote",
        Some(json!({"path": "repo", "llm": true})),
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn bad_target_is_a_bad_request() {
    let f = fixture(false);
    let (s, v) = call(
        &f.app,
        "POST",
        "/scan",
        Some(json!({"path": "repo", "targets": ["vim"]})),
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert!(v["error"].as_str().unwrap().contains("unknown target"));
}
