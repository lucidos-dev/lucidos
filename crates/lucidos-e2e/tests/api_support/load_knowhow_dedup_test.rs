//! Phase 7.2 of the loaded-knowhow + context-viewer reorg
//! (`docs/plans/2026-05-15-loaded-knowhow-and-context-viewer-reorg.md`).
//!
//! Verifies the loaded-knowhow lifecycle end-to-end against a real engine:
//! once `load_knowhow` has fired for a thread, every subsequent turn must
//!   (a) carry the doc body in a single `[LOADED KNOWHOW]` block in the
//!       user message, and
//!   (b) NOT carry the body verbatim again inside the
//!       `[CONVERSATION HISTORY]` block (Phase 4.5 strips it).
//!
//! Why we drive this from seeded events instead of a real `load_knowhow`
//! tool call: the e2e harness uses the mock LLM provider
//! (`LUCIDOS_MODEL=mock`, set by `scripts/lib/e2e.sh`), which never
//! issues tool calls. We seed a `(ToolCalled, ToolResult)` pair for
//! `load_knowhow` directly into the events table and let the engine's
//! `LoadedKnowhowStore::recover_for_thread` (called at the start of each
//! follow-up turn in `engine/chat/process.rs`) hydrate the in-memory store
//! from those rows. The follow-up chat request then produces a
//! `ContextCaptured` event we can inspect.

use crate::support::{base_url, db_url, unique_marker, user_client};
use serde_json::Value;
use uuid::Uuid;

/// Body the engine would normally write to the ToolResult after parsing the
/// shipped `system-knowhow/lucidos-cli.md`. We use a recognizable sentinel
/// so the assertions can prove the body is (or isn't) reaching the prompt.
const SEEDED_DOC_ID: &str = "lucidos-cli";
const SEEDED_DOC_BODY_MARKER: &str = "LUCIDOS_CLI_SEEDED_BODY_xyzzy";

/// Compose the body the way `SystemKnowhowStore::format_section` does:
/// the engine's own producer wraps the doc in `[SYSTEM-KNOWHOW: <id>] …
/// [END SYSTEM-KNOWHOW]`. Recovery just re-uses the recorded body, so the
/// seeded ToolResult must already carry that wrapper for the dedupe path
/// (Phase 4.5) to match it inside `format_history_content`.
fn seeded_tool_result_body() -> String {
    format!(
        "[SYSTEM-KNOWHOW: {}]\n{}\n[END SYSTEM-KNOWHOW]",
        SEEDED_DOC_ID, SEEDED_DOC_BODY_MARKER
    )
}

/// Seed a successful `(ToolCalled, ToolResult)` pair for `load_knowhow` on
/// a fresh thread. Returns the thread id. Mirrors the on-the-wire payload
/// shape produced by `engine/tools/apps.rs::load_knowhow_impl` so the
/// recovery walker in `LoadedKnowhowStore::recover_for_thread` accepts it.
async fn seed_load_knowhow_pair(pool: &sqlx::PgPool) -> Uuid {
    let thread_id = Uuid::new_v4();
    let marker = unique_marker("api-load-knowhow-init");

    // First turn: a MessageReceived + ResponseGenerated, plus the tool pair.
    // The MessageReceived is necessary so chat_test::poll_thread_summary_by_marker
    // can find the thread later if we ever needed it; the chat handler also
    // requires `thread_summaries` to know about the thread for follow-up
    // submissions.
    sqlx::query(
        "INSERT INTO thread_summaries \
         (thread_id, source, last_activity, message_count, status, state, first_message) \
         VALUES ($1, 'chat', NOW(), 1, 'idle', 'active', $2)",
    )
    .bind(thread_id)
    .bind(format!("seed for {marker}"))
    .execute(pool)
    .await
    .expect("seed thread_summaries");

    // MessageReceived (turn 1).
    // Production EventBus (`engine/event_bus.rs::persist`) populates BOTH
    // `aggregate_id` and the legacy `thread_id` column for thread events
    // (`thread_id = $aggregate_id::uuid WHEN aggregate = 'thread'`); the read
    // side `core/store/threads.rs::get_thread_events` still queries by
    // `thread_id`, so the seed must mirror production or the engine sees an
    // empty event log for this thread and skips `recover_for_thread`.
    sqlx::query(
        "INSERT INTO events (id, event_type, payload, created, aggregate_id, aggregate, thread_id) \
         VALUES ($1, 'MessageReceived', $2, NOW(), $3::text, 'thread', $3)",
    )
    .bind(Uuid::new_v4())
    .bind(serde_json::json!({
        "text": format!("seed for {marker}"),
        "channel": "chat",
    }))
    .bind(thread_id)
    .execute(pool)
    .await
    .expect("seed MessageReceived");

    // ToolCalled load_knowhow
    sqlx::query(
        "INSERT INTO events (id, event_type, payload, created, aggregate_id, aggregate, thread_id) \
         VALUES ($1, 'ToolCalled', $2, NOW(), $3::text, 'thread', $3)",
    )
    .bind(Uuid::new_v4())
    .bind(serde_json::json!({
        "name": "load_knowhow",
        "args": { "id": SEEDED_DOC_ID },
    }))
    .bind(thread_id)
    .execute(pool)
    .await
    .expect("seed ToolCalled");

    // ToolResult load_knowhow with a wrapped body (mirrors
    // SystemKnowhowStore::format_section so format_history_content can
    // strip it on the follow-up turn).
    sqlx::query(
        "INSERT INTO events (id, event_type, payload, created, aggregate_id, aggregate, thread_id) \
         VALUES ($1, 'ToolResult', $2, NOW(), $3::text, 'thread', $3)",
    )
    .bind(Uuid::new_v4())
    .bind(serde_json::json!({
        "name": "load_knowhow",
        "result": seeded_tool_result_body(),
        "success": true,
    }))
    .bind(thread_id)
    .execute(pool)
    .await
    .expect("seed ToolResult");

    // ResponseGenerated so the thread looks idle to the chat handler.
    sqlx::query(
        "INSERT INTO events (id, event_type, payload, created, aggregate_id, aggregate, thread_id) \
         VALUES ($1, 'ResponseGenerated', $2, NOW(), $3::text, 'thread', $3)",
    )
    .bind(Uuid::new_v4())
    .bind(serde_json::json!({
        "text": "ok",
        "images": [],
    }))
    .bind(thread_id)
    .execute(pool)
    .await
    .expect("seed ResponseGenerated");

    thread_id
}

/// Poll the events table until any event of `event_type` on `thread_id`
/// has a `payload::text LIKE %marker%`. Used as a synchronization barrier
/// before reading downstream events the chat handler emits later in the
/// same request.
async fn poll_event_payload_contains(
    pool: &sqlx::PgPool,
    thread_id: Uuid,
    event_type: &str,
    marker: &str,
    max_secs: u64,
) {
    let pattern = format!("%{marker}%");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(max_secs);
    loop {
        let row: Option<(Uuid,)> = sqlx::query_as(
            "SELECT id FROM events \
             WHERE event_type = $1 AND aggregate_id = $2 \
             AND payload::text LIKE $3 LIMIT 1",
        )
        .bind(event_type)
        .bind(thread_id.to_string())
        .bind(&pattern)
        .fetch_optional(pool)
        .await
        .expect("query event by marker");
        if row.is_some() {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "{event_type} carrying marker `{marker}` did not appear on thread {thread_id} within {max_secs}s",
        );
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    }
}

/// Poll the events table for the most recent `ContextCaptured` row on the
/// given thread that came from the MAIN LLM call, returning its parsed payload.
/// Times out after `max_secs`.
///
/// The producer filter is load-bearing. A turn also runs *auxiliary model
/// calls*, and each captures its own context on the same thread. Query
/// classification lands during setup, before the turn's own row; fact
/// extraction lands after it, off the EventBus. Either can be the newest, and
/// its single "Memory Request" section carries no knowhow and never will.
///
/// Filtered on `producer` rather than `purpose`: `ContextPurpose::Turn` is
/// skip-serialized, so a turn row has no `purpose` key to match on at all.
async fn poll_latest_context_captured(
    pool: &sqlx::PgPool,
    thread_id: Uuid,
    max_secs: u64,
) -> Value {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(max_secs);
    loop {
        let row: Option<(Value,)> = sqlx::query_as(
            "SELECT payload FROM events \
             WHERE event_type = 'ContextCaptured' AND aggregate_id = $1 \
               AND payload->>'producer' = 'main_llm' \
             ORDER BY created DESC LIMIT 1",
        )
        .bind(thread_id.to_string())
        .fetch_optional(pool)
        .await
        .expect("query ContextCaptured");
        if let Some((payload,)) = row {
            return payload;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "a main_llm ContextCaptured for thread {thread_id} did not appear within {max_secs}s",
        );
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    }
}

/// Cleanup helper — we delete the seeded thread + its events so repeated
/// runs don't accumulate. Best-effort; failures are swallowed.
async fn cleanup(pool: &sqlx::PgPool, thread_id: Uuid) {
    let _ = sqlx::query("DELETE FROM events WHERE aggregate_id = $1")
        .bind(thread_id.to_string())
        .execute(pool)
        .await;
    let _ = sqlx::query("DELETE FROM thread_summaries WHERE thread_id = $1")
        .bind(thread_id)
        .execute(pool)
        .await;
}

/// On a follow-up turn after a `load_knowhow` ToolResult exists in the
/// thread's history, the resulting `ContextCaptured` snapshot must:
///   - contain a section in the "Loaded knowhow" group named for the doc
///     (Phase 5.2),
///   - and NOT carry the doc body verbatim inside the
///     "Conversation History" section (Phase 4.5 strip).
#[tokio::test]
async fn load_knowhow_body_lives_once_after_first_call() {
    let pool = sqlx::PgPool::connect(&db_url())
        .await
        .expect("connect e2e db");
    let client = user_client().await;

    let thread_id = seed_load_knowhow_pair(&pool).await;

    // Follow-up turn on the seeded thread.
    let follow_up_marker = unique_marker("api-load-knowhow-followup");
    let body = serde_json::json!({
        "message": format!("noop follow-up {follow_up_marker}"),
        "mode": "human",
        "thread_id": thread_id.to_string(),
    });
    let resp = client
        .post(format!("{}/api/v1/chat/stream", base_url()))
        .json(&body)
        .send()
        .await
        .expect("follow-up chat request failed");
    assert_eq!(
        resp.status(),
        200,
        "Follow-up chat request must succeed for the dedupe path to run, got {}",
        resp.status(),
    );
    // Wait until the follow-up's MessageReceived has landed for this thread.
    // Once it's there the chat handler is well past the recovery + capture
    // step, so the ContextCaptured event is either persisted or about to be.
    poll_event_payload_contains(&pool, thread_id, "MessageReceived", &follow_up_marker, 15).await;

    // Walk to the latest ContextCaptured for this thread. The follow-up
    // produces exactly one new ContextCaptured (mock LLM, single turn).
    let snapshot = poll_latest_context_captured(&pool, thread_id, 30).await;
    let sections = snapshot["sections"]
        .as_array()
        .expect("sections array on ContextCaptured payload");

    // (1) A "Loaded knowhow" section for our seeded doc must exist.
    let loaded_section = sections.iter().find(|s| {
        s["group"].as_str() == Some("Loaded knowhow")
            && s["name"]
                .as_str()
                .map(|n| n.contains(SEEDED_DOC_ID))
                .unwrap_or(false)
    });
    assert!(
        loaded_section.is_some(),
        "Expected a 'Loaded knowhow' section for `{SEEDED_DOC_ID}` in the follow-up's \
         ContextCaptured payload. Got sections: {:#?}",
        sections
            .iter()
            .map(|s| (s["name"].as_str(), s["group"].as_str()))
            .collect::<Vec<_>>(),
    );
    let loaded_section = loaded_section.unwrap();
    // The section's role must be "user" — it lives inside the user message.
    assert_eq!(
        loaded_section["role"].as_str(),
        Some("user"),
        "Loaded knowhow sections must be tagged role=user",
    );

    // (2) The "Conversation History" section's body MUST NOT carry the
    // SYSTEM-KNOWHOW block for our id verbatim — Phase 4.5 strips it
    // because the body is already in (1) above.
    if let Some(history_section) = sections
        .iter()
        .find(|s| s["name"].as_str() == Some("Conversation History"))
    {
        if let Some(content) = history_section["content"].as_str() {
            let marker = format!("[SYSTEM-KNOWHOW: {SEEDED_DOC_ID}]");
            assert!(
                !content.contains(&marker),
                "Conversation History must not carry `{marker}` verbatim once the doc is \
                 in the Loaded knowhow section (Phase 4.5 strip). Got history content: {content}",
            );
            assert!(
                !content.contains(SEEDED_DOC_BODY_MARKER),
                "Conversation History must not carry the seeded doc body sentinel \
                 `{SEEDED_DOC_BODY_MARKER}` (Phase 4.5 strip)",
            );
        }
    }

    // (3) If the per-turn user-message section ("User Message" / "The
    // request") body is captured, it should NOT contain the
    // [LOADED KNOWHOW] block — that block is appended to the assembled
    // user message AFTER the request line and lives in its own
    // captured section. We only check the negative side here because
    // the assembled user message itself is not a `ContextSection` —
    // its body lives across multiple sections (Identity, Inventory,
    // …, Loaded knowhow, …, The request).

    // The "User Message" section captures the user's typed request only
    // (per Phase 5.1 labelling) — it must NOT include the
    // SYSTEM-KNOWHOW marker block.
    if let Some(user_msg_section) = sections
        .iter()
        .find(|s| s["name"].as_str() == Some("User Message"))
    {
        if let Some(content) = user_msg_section["content"].as_str() {
            assert!(
                !content.contains(SEEDED_DOC_BODY_MARKER),
                "The 'User Message' section captures the user's typed request only — \
                 the seeded doc body sentinel must not leak into it",
            );
        }
    }

    cleanup(&pool, thread_id).await;
}
