//! E2E for the *wake mode* a spawn declares (ADR 0451), against a real engine:
//!
//! * a `run_thread` child stores the mode it declared, and a plain chat
//!   parent's default, `wake`, when it declares none
//! * `POST /api/v1/chat/stream` refuses a wake mode with no parent, and a name
//!   that is not a mode
//!
//! The spawn runs through the mock provider's `MOCK_TOOL_CALL:` sentinel, so no
//! LLM round-trip is needed.

use crate::support::{base_url, db_url, poll_thread_summary_by_marker, unique_marker, user_client};
use serde_json::json;

/// Have a fresh human thread call `run_thread` with `args`, and return the
/// stored wake mode of the child it spawned.
async fn spawned_wake_mode(pool: &sqlx::PgPool, args: serde_json::Value) -> Option<String> {
    let marker = unique_marker("api-wake-mode");
    let resp = user_client()
        .await
        .post(format!("{}/api/v1/chat/stream", base_url()))
        .json(&json!({
            "message": format!("{marker} MOCK_TOOL_CALL: run_thread {args}"),
            "mode": "human",
        }))
        .send()
        .await
        .expect("chat request failed");
    assert_eq!(resp.status(), 200, "chat/stream should accept the message");
    let parent = poll_thread_summary_by_marker(pool, &marker, 25)
        .await
        .thread_id;

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        let row: Option<(Option<String>,)> = sqlx::query_as(
            "SELECT wake_mode FROM thread_summaries WHERE parent_thread_id = $1 LIMIT 1",
        )
        .bind(parent)
        .fetch_optional(pool)
        .await
        .expect("DB query failed");
        if let Some((mode,)) = row {
            return mode;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "no child of {parent} appeared"
        );
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }
}

#[tokio::test]
async fn a_run_thread_child_stores_its_wake_mode() {
    let pool = sqlx::PgPool::connect(&db_url())
        .await
        .expect("connect to the e2e workspace database");

    let declared = spawned_wake_mode(
        &pool,
        json!({"prompt": "count the files", "wake_mode": "wake-when-all-done"}),
    )
    .await;
    assert_eq!(declared.as_deref(), Some("wake-when-all-done"));

    let defaulted = spawned_wake_mode(&pool, json!({"prompt": "count the files"})).await;
    assert_eq!(
        defaulted.as_deref(),
        Some("wake"),
        "a parent that is not Home defaults to wake"
    );
}

#[tokio::test]
async fn chat_stream_refuses_a_wake_mode_it_cannot_honour() {
    let client = user_client().await;
    let post = |body: serde_json::Value| {
        let client = client.clone();
        async move {
            client
                .post(format!("{}/api/v1/chat/stream", base_url()))
                .json(&body)
                .send()
                .await
                .expect("chat request failed")
                .status()
        }
    };

    let top = post(json!({
        "message": "work on it",
        "mode": "agent",
        "caller_workspace": "dev",
        "wake_mode": "wake",
    }))
    .await;
    assert_eq!(top, 400, "a top spawn has no wake mode");

    let unknown = post(json!({
        "message": "work on it",
        "mode": "agent",
        "parent_thread_id": uuid::Uuid::new_v4().to_string(),
        "wake_mode": "file",
    }))
    .await;
    assert_eq!(unknown, 400, "`file` is not a wake mode");
}
