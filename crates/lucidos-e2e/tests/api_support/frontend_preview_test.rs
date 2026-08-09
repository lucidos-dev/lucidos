//! The frontend preview's HTTP surface (`engine::frontend_preview`, ADR 0055).
//!
//! What is covered here is the boundary where an untrusted thread id enters the
//! engine and turns into a process working directory. The happy path is not:
//! it spawns a real Vite dev server inside a coding-agent worktree, and the e2e
//! workspace has no worktrees and no `node_modules` to spawn one from. Starting
//! one would also leave a node process bound to a port behind a suite whose
//! whole job is to be disposable. The refusal path IS the security-relevant
//! half, and it is exercised for real here.

use crate::support::{base_url, http_client};

fn url(suffix: &str) -> String {
    format!("{}/api/v1/frontend-preview{}", base_url(), suffix)
}

#[tokio::test]
async fn a_workspace_with_no_preview_says_so_and_offers_nothing_to_open() {
    let body: serde_json::Value = http_client()
        .get(url(""))
        .send()
        .await
        .expect("frontend-preview status request failed")
        .json()
        .await
        .expect("Invalid JSON");

    assert_eq!(body["running"], false);
    // A stopped slot carries no port, no thread and no URL: a client that read
    // a stale port off a stopped preview would offer a dead link.
    for absent in ["port", "thread_id", "url", "worktree", "started_at"] {
        assert!(
            body.get(absent).is_none(),
            "a stopped preview must not report `{absent}`, got {body}"
        );
    }
}

#[tokio::test]
async fn a_thread_with_no_worktree_is_refused_by_name() {
    // The `start` body is the one place an id from outside becomes a directory
    // the engine will run a process in. A thread that never had a worktree must
    // be refused, and the refusal must name the path so the caller can act.
    let resp = http_client()
        .post(url("/start"))
        .json(&serde_json::json!({ "thread_id": "00000000-0000-4000-8000-000000000000" }))
        .send()
        .await
        .expect("frontend-preview start request failed");

    assert_eq!(resp.status(), 400);
    let body: serde_json::Value = resp.json().await.expect("Invalid JSON");
    let message = body["error"].as_str().unwrap_or_default();
    assert!(
        message.contains(".lucidos/worktrees"),
        "the refusal must name the worktree it looked for, got: {message}"
    );
}

#[tokio::test]
async fn a_malformed_thread_id_never_reaches_the_worktree_resolver() {
    // Rejected by deserialization, before any path is built from it. The status
    // distinguishes the two: 422 is "this is not a thread id", 400 is "this
    // thread has no worktree".
    let resp = http_client()
        .post(url("/start"))
        .json(&serde_json::json!({ "thread_id": "../../../etc" }))
        .send()
        .await
        .expect("frontend-preview start request failed");

    assert!(
        resp.status() == 422 || resp.status() == 400,
        "a non-uuid thread id must be refused, got {}",
        resp.status()
    );
}

#[tokio::test]
async fn stopping_a_preview_that_is_not_running_is_a_no_op() {
    // Idempotent on purpose: the UI shows a Stop button driven by SSE state
    // that can lag a stop from another device, and a second stop must not error.
    let body: serde_json::Value = http_client()
        .post(url("/stop"))
        .json(&serde_json::json!({}))
        .send()
        .await
        .expect("frontend-preview stop request failed")
        .json()
        .await
        .expect("Invalid JSON");

    assert_eq!(body["running"], false);
}
