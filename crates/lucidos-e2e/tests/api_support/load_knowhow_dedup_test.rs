//! The loaded-knowhow lifecycle end-to-end against a real engine (ADR 0425).
//!
//! Once `load_knowhow` has fired for a thread, the next turns must do three
//! things:
//!   (a) carry the doc body once, in the `[LOADED KNOWHOW]` block of the user
//!       message. It never appears verbatim in `[CONVERSATION HISTORY]`.
//!   (b) drop the doc once enough finished turns pass without a load of it.
//!       It leaves the block and the resume tool blocks alike.
//!   (c) keep a doc that some turn inside the window loaded again.
//!
//! Why we drive this from seeded events instead of a real `load_knowhow`
//! tool call: the e2e harness uses the mock LLM provider
//! (`LUCIDOS_MODEL=mock`, set by `scripts/lib/e2e.sh`), which never
//! issues tool calls. We seed the thread's turns, with their
//! `(ToolCalled, ToolResult)` pairs for `load_knowhow`, directly into the
//! events table. The engine folds its loaded set from those rows on every
//! turn, exactly as it does after a restart. The follow-up chat request then
//! produces a `ContextCaptured` event we can inspect.

use crate::support::{base_url, db_url, unique_marker, user_client};
use lucidos_engine::engine::loaded_knowhow::IDLE_TURNS_BEFORE_UNLOAD;
use serde_json::Value;
use uuid::Uuid;

/// A recognizable sentinel, so the assertions can prove the body is (or
/// isn't) reaching the prompt.
const SEEDED_DOC_ID: &str = "lucidos-cli";
const SEEDED_DOC_BODY_MARKER: &str = "LUCIDOS_CLI_SEEDED_BODY_xyzzy";

/// Compose the body the way `SystemKnowhowStore::format_section` does:
/// the engine's own producer wraps the doc in `[SYSTEM-KNOWHOW: <id>] …
/// [END SYSTEM-KNOWHOW]`. The fold re-uses the recorded body, so the
/// seeded ToolResult must already carry that wrapper for the history strip
/// to match it inside `format_history_content`.
fn seeded_tool_result_body() -> String {
    format!(
        "[SYSTEM-KNOWHOW: {}]\n{}\n[END SYSTEM-KNOWHOW]",
        SEEDED_DOC_ID, SEEDED_DOC_BODY_MARKER
    )
}

/// One seeded turn: a message, a `load_knowhow` pair when `loads`, and the
/// `ResponseGenerated` that makes it a finished turn.
struct SeededTurn {
    loads: bool,
}

const LOADS: SeededTurn = SeededTurn { loads: true };
const IDLE: SeededTurn = SeededTurn { loads: false };

/// Production EventBus (`engine/event_bus.rs::persist`) fills both
/// `aggregate_id` and the legacy `thread_id` column for thread events. The
/// read side queries by `thread_id`, so the seed mirrors production.
async fn seed_event(pool: &sqlx::PgPool, thread_id: Uuid, event_type: &str, payload: Value) {
    sqlx::query(
        "INSERT INTO events (id, event_type, payload, created, aggregate_id, aggregate, thread_id) \
         VALUES ($1, $2, $3, clock_timestamp(), $4::text, 'thread', $4)",
    )
    .bind(Uuid::new_v4())
    .bind(event_type)
    .bind(payload)
    .bind(thread_id)
    .execute(pool)
    .await
    .unwrap_or_else(|e| panic!("seed {event_type}: {e}"));
}

/// Seed a fresh chat thread with `turns`, in order. Returns the thread id.
/// The payloads mirror what `engine/tools/apps.rs::load_knowhow_impl` and the
/// chat loop record, so the engine's fold reads them as it reads real rows.
async fn seed_thread(pool: &sqlx::PgPool, turns: &[SeededTurn]) -> Uuid {
    let thread_id = Uuid::new_v4();
    let marker = unique_marker("api-load-knowhow-init");

    // The chat handler requires `thread_summaries` to know about the thread
    // for follow-up submissions.
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

    for (n, turn) in turns.iter().enumerate() {
        seed_event(
            pool,
            thread_id,
            "MessageReceived",
            serde_json::json!({"text": format!("seed turn {n} for {marker}"), "channel": "chat"}),
        )
        .await;
        if turn.loads {
            seed_event(
                pool,
                thread_id,
                "ToolCalled",
                serde_json::json!({"name": "load_knowhow", "args": {"id": SEEDED_DOC_ID}}),
            )
            .await;
            seed_event(
                pool,
                thread_id,
                "ToolResult",
                serde_json::json!({
                    "name": "load_knowhow",
                    "result": seeded_tool_result_body(),
                    "success": true,
                }),
            )
            .await;
        }
        seed_event(
            pool,
            thread_id,
            "ResponseGenerated",
            serde_json::json!({"text": "ok", "images": []}),
        )
        .await;
    }
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

/// Seed `turns`, send one follow-up turn, and return that turn's captured
/// sections: what its prompt carried.
async fn next_prompt_sections(pool: &sqlx::PgPool, turns: &[SeededTurn]) -> (Uuid, Vec<Value>) {
    let client = user_client().await;
    let thread_id = seed_thread(pool, turns).await;

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
        "Follow-up chat request must succeed, got {}",
        resp.status(),
    );
    // The follow-up's MessageReceived means the handler is past the history
    // load. Its ContextCaptured is then persisted or about to be.
    poll_event_payload_contains(pool, thread_id, "MessageReceived", &follow_up_marker, 15).await;

    // The follow-up produces exactly one new ContextCaptured (mock LLM,
    // single turn).
    let snapshot = poll_latest_context_captured(pool, thread_id, 30).await;
    let sections = snapshot["sections"]
        .as_array()
        .expect("sections array on ContextCaptured payload")
        .clone();
    (thread_id, sections)
}

fn loaded_knowhow_row(sections: &[Value]) -> Option<&Value> {
    sections.iter().find(|s| {
        s["group"].as_str() == Some("Loaded knowhow")
            && s["name"]
                .as_str()
                .is_some_and(|n| n.contains(SEEDED_DOC_ID))
    })
}

fn section_names(sections: &[Value]) -> Vec<(Option<&str>, Option<&str>)> {
    sections
        .iter()
        .map(|s| (s["name"].as_str(), s["group"].as_str()))
        .collect()
}

/// On the turn after a load, the capture must:
///   - contain a section in the "Loaded knowhow" group named for the doc,
///   - and NOT carry the doc body verbatim inside "Conversation History".
#[tokio::test]
async fn load_knowhow_body_lives_once_after_first_call() {
    let pool = sqlx::PgPool::connect(&db_url())
        .await
        .expect("connect e2e db");
    let (thread_id, sections) = next_prompt_sections(&pool, &[LOADS]).await;

    let loaded_section = loaded_knowhow_row(&sections).unwrap_or_else(|| {
        panic!(
            "Expected a 'Loaded knowhow' section for `{SEEDED_DOC_ID}`. Got sections: {:#?}",
            section_names(&sections)
        )
    });
    // The section's role must be "user": it lives inside the user message.
    assert_eq!(
        loaded_section["role"].as_str(),
        Some("user"),
        "Loaded knowhow sections must be tagged role=user",
    );

    // The history must not repeat the body the block already carries.
    if let Some(content) = sections
        .iter()
        .find(|s| s["name"].as_str() == Some("Conversation History"))
        .and_then(|s| s["content"].as_str())
    {
        let marker = format!("[SYSTEM-KNOWHOW: {SEEDED_DOC_ID}]");
        assert!(
            !content.contains(&marker),
            "Conversation History must not carry `{marker}` verbatim. Got: {content}",
        );
        assert!(
            !content.contains(SEEDED_DOC_BODY_MARKER),
            "Conversation History must not carry the seeded doc body sentinel",
        );
    }

    // The "User Message" section captures the user's typed request only.
    if let Some(content) = sections
        .iter()
        .find(|s| s["name"].as_str() == Some("User Message"))
        .and_then(|s| s["content"].as_str())
    {
        assert!(
            !content.contains(SEEDED_DOC_BODY_MARKER),
            "the seeded doc body sentinel must not leak into the 'User Message' section",
        );
    }

    cleanup(&pool, thread_id).await;
}

/// A doc loaded more finished turns ago than the limit, and never since, is
/// gone from the next prompt: no tier row, and no copy of its body in any
/// section, the resume tool blocks included.
#[tokio::test]
async fn a_doc_unused_past_the_limit_is_gone_from_the_next_prompt() {
    let pool = sqlx::PgPool::connect(&db_url())
        .await
        .expect("connect e2e db");
    let mut turns = vec![LOADS];
    turns.extend((0..IDLE_TURNS_BEFORE_UNLOAD).map(|_| IDLE));
    let (thread_id, sections) = next_prompt_sections(&pool, &turns).await;

    assert!(
        loaded_knowhow_row(&sections).is_none(),
        "`{SEEDED_DOC_ID}` should have left the Loaded knowhow tier. Got sections: {:#?}",
        section_names(&sections)
    );
    let carrying: Vec<_> = sections
        .iter()
        .filter(|s| {
            s["content"]
                .as_str()
                .is_some_and(|c| c.contains(SEEDED_DOC_BODY_MARKER))
        })
        .filter_map(|s| s["name"].as_str())
        .collect();
    assert!(
        carrying.is_empty(),
        "no section may carry the evicted doc's body, found it in {carrying:?}"
    );

    cleanup(&pool, thread_id).await;
}

/// A doc loaded again inside the window stays, and the tier row says this
/// is its last turn unless it is loaded once more.
#[tokio::test]
async fn a_doc_loaded_again_inside_the_window_stays() {
    let pool = sqlx::PgPool::connect(&db_url())
        .await
        .expect("connect e2e db");
    let mut turns = vec![LOADS];
    turns.extend((1..IDLE_TURNS_BEFORE_UNLOAD).map(|_| IDLE));
    turns.push(LOADS);
    turns.extend((1..IDLE_TURNS_BEFORE_UNLOAD).map(|_| IDLE));
    let (thread_id, sections) = next_prompt_sections(&pool, &turns).await;

    let row = loaded_knowhow_row(&sections).unwrap_or_else(|| {
        panic!(
            "`{SEEDED_DOC_ID}` was loaded again inside the window and must stay. Got sections: {:#?}",
            section_names(&sections)
        )
    });
    // The row text is what the Context viewer shows, so it is the contract.
    let name = row["name"].as_str().unwrap_or_default();
    assert!(
        name.contains("leaves after this turn unless loaded again"),
        "the row should say why the doc is still loaded, got `{name}`"
    );

    cleanup(&pool, thread_id).await;
}
