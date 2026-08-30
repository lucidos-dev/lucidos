//! Verify the lazy-load contract for `CodingAgentToolCalled.args`, the sibling
//! of `tool_result_lazy_test.rs`:
//!
//! 1. `GET /api/v1/threads/:tid/events` drops `args` and stamps an
//!    `args_stripped: true` marker, keeping `name`, `description` and
//!    `tool_use_id` so the inline step row still labels and pairs itself.
//! 2. A row with NO `description` gets one filled from `describe_cc_tool`
//!    before the drop, so a pre-May-2026 row does not degrade to a bare tool
//!    name.
//! 3. `GET /api/v1/events/:eid/tool-args` returns the original args on demand.
//! 4. `?include_context=true` opts back into the full payload, which is what
//!    keeps an exported bug report complete.
//!
//! `args` is the single heaviest thing a coding-agent snapshot carries: an
//! `Edit`'s two versions of a hunk, a `Write`'s whole file. Nothing inline
//! renders it, so the modal fetches it only when the user opens a step.

use crate::support::{base_url, db_url, http_client, seed_chat_thread_summary, unique_marker};
use serde_json::Value;
use uuid::Uuid;

async fn pool() -> sqlx::PgPool {
    sqlx::PgPool::connect(&db_url())
        .await
        .expect("Failed to connect to e2e DB")
}

/// Seed a thread plus one `CodingAgentToolCalled`. `description` is omitted
/// when `describe` is `None`, which is the shape of every row written before
/// the engine started stamping it.
async fn seed_thread_with_tool_call(
    pool: &sqlx::PgPool,
    describe: Option<&str>,
) -> (Uuid, Uuid, Value) {
    let thread_id = Uuid::new_v4();
    let event_id = Uuid::new_v4();
    let marker = unique_marker("api-tool-args-lazy");
    let args = serde_json::json!({
        "file_path": "/repo/crates/lucidos-app/src/styles/shell.css",
        "old_string": format!("/* before {marker} */\n").repeat(200),
        "new_string": format!("/* after {marker} */\n").repeat(200),
    });

    seed_chat_thread_summary(pool, thread_id, "idle").await;

    sqlx::query(
        "INSERT INTO events (id, event_type, payload, created, aggregate_id, aggregate, thread_id) \
         VALUES ($1, 'MessageReceived', $2, NOW(), $3::text, 'thread', $3)",
    )
    .bind(Uuid::new_v4())
    .bind(serde_json::json!({ "text": format!("seed for {marker}"), "channel": "claude_code" }))
    .bind(thread_id)
    .execute(pool)
    .await
    .expect("seed MessageReceived");

    let mut payload = serde_json::json!({
        "name": "Edit",
        "args": args.clone(),
        "tool_use_id": "tu-A",
        "coding_agent": "claude-code",
    });
    if let Some(text) = describe {
        payload["description"] = Value::String(text.to_string());
    }

    sqlx::query(
        "INSERT INTO events (id, event_type, payload, created, aggregate_id, aggregate, thread_id) \
         VALUES ($1, 'CodingAgentToolCalled', $2, NOW(), $3::text, 'thread', $3)",
    )
    .bind(event_id)
    .bind(payload)
    .bind(thread_id)
    .execute(pool)
    .await
    .expect("seed CodingAgentToolCalled");

    (thread_id, event_id, args)
}

/// Fetch one thread's snapshot and return the tool-call row's payload.
async fn tool_call_payload(thread_id: Uuid, query: &str) -> Value {
    let client = http_client();
    let url = format!(
        "{}/api/v1/threads/{}/events{}",
        base_url(),
        thread_id,
        query
    );
    let body: Value = client
        .get(&url)
        .send()
        .await
        .expect("snapshot request failed")
        .json()
        .await
        .expect("invalid JSON");
    body["events"]
        .as_array()
        .expect("events array")
        .iter()
        .find(|e| e["event_type"] == "CodingAgentToolCalled")
        .unwrap_or_else(|| panic!("CodingAgentToolCalled not present for {thread_id}"))["payload"]
        .clone()
}

#[tokio::test]
async fn snapshot_strips_tool_call_args_and_stamps_marker() {
    let pool = pool().await;
    let (thread_id, _event_id, _args) =
        seed_thread_with_tool_call(&pool, Some("Edit shell.css")).await;

    let payload = tool_call_payload(thread_id, "").await;

    assert!(
        payload.get("args").is_none(),
        "args should be stripped from the snapshot payload, got: {payload}"
    );
    assert_eq!(payload["args_stripped"], Value::Bool(true));
    // The inline step row labels by `description` and pairs by `tool_use_id`.
    assert_eq!(payload["description"], "Edit shell.css");
    assert_eq!(payload["name"], "Edit");
    assert_eq!(payload["tool_use_id"], "tu-A");
}

/// A row written before the engine stamped `description` still renders its real
/// label. The strip fills it from the args it is about to drop.
#[tokio::test]
async fn snapshot_fills_a_missing_description_before_dropping_the_args() {
    let pool = pool().await;
    let (thread_id, _event_id, _args) = seed_thread_with_tool_call(&pool, None).await;

    let payload = tool_call_payload(thread_id, "").await;

    assert!(payload.get("args").is_none());
    assert_eq!(
        payload["description"], "Edit shell.css",
        "a legacy row must not degrade to a bare tool name, got: {payload}"
    );
}

#[tokio::test]
async fn tool_args_endpoint_returns_the_full_args() {
    let pool = pool().await;
    let (_thread_id, event_id, args) =
        seed_thread_with_tool_call(&pool, Some("Edit shell.css")).await;

    let client = http_client();
    let url = format!("{}/api/v1/events/{}/tool-args", base_url(), event_id);
    let resp = client
        .get(&url)
        .send()
        .await
        .expect("tool-args request failed");
    assert_eq!(resp.status(), 200);

    let body: Value = resp.json().await.expect("invalid JSON");
    assert_eq!(body["args"], args);
}

/// A tool called with no arguments is a real answer rather than a missing one.
/// So the endpoint serves null and the modal draws its label-only state.
#[tokio::test]
async fn tool_args_endpoint_returns_null_when_no_args_were_recorded() {
    let pool = pool().await;
    let thread_id = Uuid::new_v4();
    let event_id = Uuid::new_v4();
    let marker = unique_marker("api-tool-args-lazy-none");
    seed_chat_thread_summary(&pool, thread_id, "idle").await;
    sqlx::query(
        "INSERT INTO events (id, event_type, payload, created, aggregate_id, aggregate, thread_id) \
         VALUES ($1, 'CodingAgentToolCalled', $2, NOW(), $3::text, 'thread', $3)",
    )
    .bind(event_id)
    .bind(serde_json::json!({ "name": "TodoWrite", "description": format!("Update plan {marker}") }))
    .bind(thread_id)
    .execute(&pool)
    .await
    .expect("seed argless CodingAgentToolCalled");

    let client = http_client();
    let url = format!("{}/api/v1/events/{}/tool-args", base_url(), event_id);
    let resp = client
        .get(&url)
        .send()
        .await
        .expect("tool-args request failed");
    assert_eq!(resp.status(), 200);

    let body: Value = resp.json().await.expect("invalid JSON");
    assert_eq!(body["args"], Value::Null);
}

/// The chat channel's own `ToolCalled` is not stripped, so the endpoint must
/// refuse it rather than leak an arbitrary payload through a second door.
#[tokio::test]
async fn tool_args_endpoint_rejects_a_non_coding_agent_call() {
    let pool = pool().await;
    let thread_id = Uuid::new_v4();
    let event_id = Uuid::new_v4();
    let marker = unique_marker("api-tool-args-lazy-chat");
    seed_chat_thread_summary(&pool, thread_id, "idle").await;
    sqlx::query(
        "INSERT INTO events (id, event_type, payload, created, aggregate_id, aggregate, thread_id) \
         VALUES ($1, 'ToolCalled', $2, NOW(), $3::text, 'thread', $3)",
    )
    .bind(event_id)
    .bind(serde_json::json!({ "name": "write_file", "args": { "path": format!("{marker}.md") } }))
    .bind(thread_id)
    .execute(&pool)
    .await
    .expect("seed chat ToolCalled");

    let client = http_client();
    let url = format!("{}/api/v1/events/{}/tool-args", base_url(), event_id);
    let resp = client
        .get(&url)
        .send()
        .await
        .expect("tool-args request failed");
    assert_eq!(resp.status(), 404);
}

#[tokio::test]
async fn tool_args_endpoint_404_on_unknown_event_id() {
    let _pool = pool().await;
    let client = http_client();
    let url = format!("{}/api/v1/events/{}/tool-args", base_url(), Uuid::new_v4());
    let resp = client
        .get(&url)
        .send()
        .await
        .expect("tool-args request failed");
    assert_eq!(resp.status(), 404);
}

/// `?include_context=true` opts back into the full payload, which is what the
/// export-thread path reads so a bug report keeps every tool input.
#[tokio::test]
async fn snapshot_include_context_true_preserves_tool_call_args() {
    let pool = pool().await;
    let (thread_id, _event_id, args) =
        seed_thread_with_tool_call(&pool, Some("Edit shell.css")).await;

    let payload = tool_call_payload(thread_id, "?include_context=true").await;

    assert_eq!(payload["args"], args);
    assert!(
        payload.get("args_stripped").is_none(),
        "include_context=true must not stamp the strip marker"
    );
}
