//! E2E coverage for the knowhow HTTP surface that the `lucidos knowhow` CLI
//! subcommand wraps: `GET /api/v1/knowhow` (catalog) and
//! `GET /api/v1/knowhow/read?id=<id>` (one doc's full content).
//!
//! This is the path an *app coding-agent thread* uses to pull app-building
//! guidance — its sparse-checkout worktree can't see the engine's
//! `system-knowhow/` on disk and it has no in-process `load_knowhow` tool, so
//! the read endpoint is its only route to the docs. The engine under test
//! ships `system-knowhow/` from the repo, so `system-knowhow/building-an-app`
//! is guaranteed to resolve.

use crate::support::{base_url, http_client};
use serde_json::Value;

const BUILDING_AN_APP_ID: &str = "system-knowhow/building-an-app";

/// The catalog lists engine-shipped system-knowhow with the
/// `system-knowhow/` id prefix already applied.
#[tokio::test]
async fn knowhow_list_includes_building_an_app() {
    let client = http_client();
    let resp = client
        .get(format!("{}/api/v1/knowhow", base_url()))
        .send()
        .await
        .expect("GET /api/v1/knowhow failed");
    assert_eq!(resp.status(), 200, "knowhow list must return 200");

    let body: Value = resp.json().await.expect("knowhow list must be JSON");
    let entries = body["knowhow"]
        .as_array()
        .expect("response must carry a `knowhow` array");
    assert!(
        entries
            .iter()
            .any(|e| e["id"].as_str() == Some(BUILDING_AN_APP_ID)),
        "catalog must advertise `{BUILDING_AN_APP_ID}`; got ids: {:?}",
        entries
            .iter()
            .filter_map(|e| e["id"].as_str())
            .collect::<Vec<_>>(),
    );
}

/// Reading a known system-knowhow id returns the wrapped doc body — the same
/// `[SYSTEM-KNOWHOW: …]` block the chat agent's `load_knowhow` tool produces.
#[tokio::test]
async fn knowhow_read_returns_building_an_app_body() {
    let client = http_client();
    let resp = client
        .get(format!("{}/api/v1/knowhow/read", base_url()))
        .query(&[("id", BUILDING_AN_APP_ID)])
        .send()
        .await
        .expect("GET /api/v1/knowhow/read failed");
    assert_eq!(resp.status(), 200, "reading a known id must return 200");

    let text = resp.text().await.expect("read body must be text");
    assert!(
        text.contains("[SYSTEM-KNOWHOW:"),
        "read body must carry the SYSTEM-KNOWHOW wrapper; got: {text:.200}",
    );
    assert!(
        text.contains("Building an App"),
        "read body must carry the building-an-app content; got: {text:.200}",
    );
}

/// An unknown id is a 404 carrying the shared not-found sentinel — not a 200
/// with empty content (which would read as "the doc is empty").
#[tokio::test]
async fn knowhow_read_unknown_id_is_404() {
    let client = http_client();
    let resp = client
        .get(format!("{}/api/v1/knowhow/read", base_url()))
        .query(&[("id", "system-knowhow/this-doc-does-not-exist-xyzzy")])
        .send()
        .await
        .expect("GET /api/v1/knowhow/read failed");
    assert_eq!(resp.status(), 404, "unknown id must return 404");

    let text = resp.text().await.expect("404 body must be text");
    assert!(
        text.contains("not found"),
        "404 body must carry the not-found sentinel; got: {text}",
    );
}
