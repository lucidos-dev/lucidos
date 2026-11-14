//! Coverage for `POST /api/v1/threads/drafts`, an agent creating a draft for
//! the owner (ADR 0665).
//!
//! The route needs a thread-bound origin token, which only a subprocess the
//! engine spawned holds. So, as in `follow_up_test`, this suite covers what a
//! caller outside the engine reaches: the refusal, the mount and the CLI loop.
//! The engine's `agent_drafts` tests cover a created draft.

use crate::support::{base_url, unique_marker, user_client};

/// A device has no origin token, so no thread to name the draft by. It is
/// refused before the body is read, and no draft appears.
#[tokio::test]
async fn create_draft_refuses_a_caller_outside_a_thread() {
    let client = user_client().await;
    let marker = unique_marker("draft-refused");
    let resp = client
        .post(format!("{}/api/v1/threads/drafts", base_url()))
        .json(&serde_json::json!({
            "text": marker,
            "mode": "lucidos",
            "retry_key": "e2e",
        }))
        .send()
        .await
        .expect("create-draft request sends");
    assert_eq!(resp.status(), 403);
    let body: serde_json::Value = resp.json().await.expect("standard error body");
    assert!(body["error"]
        .as_str()
        .expect("error is a string")
        .contains("agent inside a thread"));

    let drafts: serde_json::Value = client
        .get(format!("{}/api/v1/threads/drafts", base_url()))
        .send()
        .await
        .expect("drafts read sends")
        .json()
        .await
        .expect("drafts read is JSON");
    assert!(
        !drafts.to_string().contains(&marker),
        "a refused create writes no draft"
    );
}

/// The CLI reaches the route and surfaces its refusal.
#[test]
fn cli_create_draft_reaches_the_route() {
    let out = crate::lucidos_cli_test::lucidos_cmd()
        .args(["threads", "create-draft", "--text", "hello"])
        .output()
        .expect("lucidos threads create-draft runs");
    assert!(!out.status.success(), "a token-less CLI call is refused");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("403"),
        "the CLI must surface the engine's refusal, got: {stderr}"
    );
}
