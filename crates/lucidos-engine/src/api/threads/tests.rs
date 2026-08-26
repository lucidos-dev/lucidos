use super::events_snapshot::{
    rename_legacy_section_size, rename_legacy_section_size_in_payload,
    strip_app_capture_in_tool_result, strip_context_capture_sections,
    strip_image_content_in_tool_result, strip_inline_image_payloads, strip_tool_result_content,
};
use crate::core::ThreadEventRow;
use chrono::Utc;
use serde_json::json;
use uuid::Uuid;

fn row(event_type: &str, payload: serde_json::Value) -> ThreadEventRow {
    ThreadEventRow {
        sequence: 1,
        event_type: event_type.to_string(),
        payload,
        created: Utc::now(),
        event_id: Uuid::new_v4(),
    }
}

/// `Router::route` inserts into matchit eagerly and PANICS on a conflict, so
/// simply building the router is the assertion. Worth a test of its own
/// because the `/threads` surface mixes two path-param names (`:thread_id` on
/// most routes, `:id` on the bare leaf and its two children) and the rule that
/// makes that legal is narrow: only ONE bare `/threads/<param>` leaf may
/// exist. A future route that gets the name wrong takes the whole engine down
/// at startup, which is a bad place to find out.
#[test]
fn the_threads_router_builds_with_both_path_param_names() {
    let _router = super::router();
}

#[test]
fn read_time_strip_replaces_image_content_in_tool_result() {
    let huge_b64 = "A".repeat(2 * 1024 * 1024);
    let mut row = row(
        "ToolResult",
        json!({
            "name": "read_file",
            "result": format!("[IMAGE_CONTENT:image/png]\n{}", huge_b64),
            "images": [],
        }),
    );
    let original_size = row.payload["result"].as_str().unwrap().len();
    assert!(original_size > 1_000_000, "setup: payload should be huge");

    strip_image_content_in_tool_result(&mut row);

    let stripped = row.payload["result"].as_str().unwrap();
    assert!(stripped.len() < 100, "stripped to {} bytes", stripped.len());
    assert!(stripped.contains("image/png"));
}

#[test]
fn read_time_strip_leaves_non_image_tool_results_alone() {
    let mut row = row(
        "ToolResult",
        json!({ "name": "list_files", "result": "file1.txt\nfile2.txt", "images": [] }),
    );
    strip_image_content_in_tool_result(&mut row);
    assert_eq!(row.payload["result"], "file1.txt\nfile2.txt");
}

#[test]
fn read_time_strip_ignores_non_tool_result_events() {
    let mut row = row(
        "MessageReceived",
        json!({ "text": "[IMAGE_CONTENT:image/png]\nABCD" }),
    );
    let before = row.payload.clone();
    strip_image_content_in_tool_result(&mut row);
    assert_eq!(row.payload, before, "unrelated events untouched");
}

#[test]
fn read_time_strip_handles_missing_result_field() {
    let mut row = row("ToolResult", json!({ "name": "x", "images": [] }));
    let before = row.payload.clone();
    strip_image_content_in_tool_result(&mut row);
    assert_eq!(row.payload, before, "missing result field is a no-op");
}

/// The app-capture half of the same rescue. The write path only started
/// stubbing these on 2026-07-30, so every capture taken before that is still in
/// the events table at full size (1.53 MB rows measured) and no migration
/// rewrites them.
#[test]
fn read_time_strip_replaces_app_capture_in_tool_result() {
    let huge_b64 = "A".repeat(2 * 1024 * 1024);
    let mut row = row(
        "ToolResult",
        json!({
            "name": "capture_app",
            "result": format!("[APP_CAPTURE:{}]\nDOM snapshot:\n<html>hi</html>", huge_b64),
            "images": [],
        }),
    );
    assert!(row.payload["result"].as_str().unwrap().len() > 1_000_000);

    strip_app_capture_in_tool_result(&mut row);

    let stripped = row.payload["result"].as_str().unwrap();
    assert!(stripped.len() < 200, "stripped to {} bytes", stripped.len());
    assert!(
        stripped.contains("<html>hi</html>"),
        "the DOM is the part worth keeping, got: {}",
        stripped
    );
    assert!(!stripped.contains(&huge_b64));
}

#[test]
fn read_time_app_capture_strip_leaves_other_results_alone() {
    let mut row = row(
        "ToolResult",
        json!({ "name": "list_files", "result": "file1.txt", "images": [] }),
    );
    strip_app_capture_in_tool_result(&mut row);
    assert_eq!(row.payload["result"], "file1.txt");
}

/// Read paths call the combined entry point so neither sentinel can be missed
/// at a new call site, which is exactly how the app-capture half went
/// unstripped on every read path for months.
#[test]
fn combined_strip_covers_both_sentinels() {
    let b64 = "A".repeat(2000);

    let mut image_row = row(
        "ToolResult",
        json!({ "name": "read_file", "result": format!("[IMAGE_CONTENT:image/png]\n{}", b64) }),
    );
    strip_inline_image_payloads(&mut image_row);
    assert!(!image_row.payload["result"].as_str().unwrap().contains(&b64));

    let mut capture_row = row(
        "ToolResult",
        json!({ "name": "capture_app", "result": format!("[APP_CAPTURE:{}]\nDOM", b64) }),
    );
    strip_inline_image_payloads(&mut capture_row);
    assert!(!capture_row.payload["result"]
        .as_str()
        .unwrap()
        .contains(&b64));
}

/// The lazy-fetch endpoint holds an `EventRow`, not a `ThreadEventRow`. Both
/// implement `HasEventPayload`, and the strippers are generic over it so the
/// two read paths cannot drift apart.
#[test]
fn combined_strip_works_on_the_lazy_fetch_row_type() {
    let b64 = "A".repeat(2000);
    let mut event_row = crate::core::events::EventRow::new(
        "ToolResult",
        json!({ "name": "capture_app", "result": format!("[APP_CAPTURE:{}]\nDOM", b64) }),
    );

    strip_inline_image_payloads(&mut event_row);

    assert!(!event_row.payload["result"].as_str().unwrap().contains(&b64));
}

#[test]
fn context_capture_strip_drops_sections_and_tools_stamps_marker() {
    let mut row = row(
        "ContextCaptured",
        json!({
            "producer": "main_llm",
            "model": "claude-opus-4-7",
            "context_window": 200_000,
            "sections": [
                { "name": "system", "budget_delta_chars": 10_000, "content_chars": 10_000, "content": "A".repeat(10_000) },
                { "name": "history", "budget_delta_chars": 5_000, "content_chars": 5_000 },
            ],
            "tools": ["search", "edit"],
            "estimated_total_tokens": 4_200,
            "usage": { "input_tokens": 4_100, "output_tokens": 50, "cache_read_tokens": 0, "cache_creation_tokens": 0 },
            "trimmed": false,
        }),
    );
    strip_context_capture_sections(&mut row);
    let obj = row.payload.as_object().unwrap();
    assert!(!obj.contains_key("sections"), "sections must be dropped");
    assert!(!obj.contains_key("tools"), "tools must be dropped");
    assert_eq!(obj.get("sections_stripped"), Some(&json!(true)));
    // Lightweight fields preserved so the inline chip still renders.
    assert_eq!(obj.get("producer"), Some(&json!("main_llm")));
    assert_eq!(obj.get("model"), Some(&json!("claude-opus-4-7")));
    assert_eq!(obj.get("context_window"), Some(&json!(200_000)));
    assert_eq!(obj.get("estimated_total_tokens"), Some(&json!(4_200)));
    assert!(obj.get("usage").is_some(), "usage preserved");
}

/// The read paths serve stored sections verbatim, so the serde alias never
/// runs on them. A months-old row must still arrive with a size the viewer
/// can read.
#[test]
fn a_stored_char_count_reaches_the_client_as_the_budget_delta() {
    let mut sections = json!([
        { "name": "System Instructions", "char_count": 49_380 },
        { "name": "Conversation", "char_count": 500 },
    ]);
    rename_legacy_section_size(&mut sections);
    assert_eq!(sections[0]["budget_delta_chars"], 49_380);
    assert_eq!(sections[1]["budget_delta_chars"], 500);
    assert!(sections[0].get("char_count").is_none());
    assert!(
        sections[0].get("content_chars").is_none(),
        "nobody measured the content size when this row was written"
    );
}

/// A row written today already spells it right, and the rename must not touch
/// it. A row carrying both keys keeps the current one.
#[test]
fn a_current_row_passes_through_the_rename_untouched() {
    let mut sections = json!([
        { "name": "Conversation", "budget_delta_chars": 600, "content_chars": 645_368 },
        { "name": "Odd", "budget_delta_chars": 7, "char_count": 9 },
    ]);
    let before = sections.clone();
    rename_legacy_section_size(&mut sections);
    assert_eq!(sections, before);
}

/// `ContextAssembled` is the retired predecessor. The snapshot never strips
/// it, so its sections reach the viewer through the payload arm.
#[test]
fn the_payload_arm_renames_a_legacy_assembled_row() {
    let mut payload = json!({
        "model": "claude-opus-4-7",
        "sections": [{ "name": "System Instructions", "char_count": 147_800 }],
        "tools": [],
    });
    rename_legacy_section_size_in_payload(&mut payload);
    assert_eq!(payload["sections"][0]["budget_delta_chars"], 147_800);
}

/// A payload with no sections, and a sections value that is not an array,
/// both pass through rather than panicking.
#[test]
fn the_rename_is_a_no_op_on_a_payload_with_nothing_to_rename() {
    let mut stripped = json!({ "sections_stripped": true });
    rename_legacy_section_size_in_payload(&mut stripped);
    assert_eq!(stripped, json!({ "sections_stripped": true }));

    let mut corrupt = json!({ "sections": "not an array" });
    rename_legacy_section_size_in_payload(&mut corrupt);
    assert_eq!(corrupt, json!({ "sections": "not an array" }));
}

#[test]
fn context_capture_strip_ignores_other_event_types() {
    let mut row = row(
        "ToolCalled",
        json!({ "name": "do_thing", "args": {}, "sections": ["should_not_be_touched"] }),
    );
    let before = row.payload.clone();
    strip_context_capture_sections(&mut row);
    assert_eq!(row.payload, before);
}

#[test]
fn context_capture_strip_is_idempotent() {
    let mut row = row(
        "ContextCaptured",
        json!({
            "producer": "main_llm",
            "model": "m",
            "context_window": 1,
            "sections": [],
            "tools": [],
            "estimated_total_tokens": 0,
            "trimmed": false,
        }),
    );
    strip_context_capture_sections(&mut row);
    let after_first = row.payload.clone();
    strip_context_capture_sections(&mut row);
    assert_eq!(row.payload, after_first, "second call is a no-op");
}

#[test]
fn tool_result_strip_drops_result_keeps_name_and_images_stamps_marker() {
    let huge_output = "bash output line\n".repeat(10_000);
    let mut row = row(
        "ToolResult",
        json!({
            "name": "run_bash",
            "result": huge_output,
            "images": ["abc123", "def456"],
        }),
    );
    strip_tool_result_content(&mut row);
    let obj = row.payload.as_object().unwrap();
    assert!(!obj.contains_key("result"), "result must be dropped");
    assert_eq!(obj.get("result_stripped"), Some(&json!(true)));
    // Inline step row still needs the tool name + images (the chat
    // exchange renders generated images inline via the existing
    // `ToolResult` handler in thread-events.ts).
    assert_eq!(obj.get("name"), Some(&json!("run_bash")));
    assert_eq!(obj.get("images"), Some(&json!(["abc123", "def456"])));
}

#[test]
fn tool_result_strip_ignores_other_event_types() {
    let mut row = row(
        "ContextCaptured",
        json!({ "producer": "main_llm", "result": "should_not_be_touched" }),
    );
    let before = row.payload.clone();
    strip_tool_result_content(&mut row);
    assert_eq!(row.payload, before);
}

#[test]
fn tool_result_strip_is_idempotent() {
    let mut row = row(
        "ToolResult",
        json!({ "name": "x", "result": "out", "images": [] }),
    );
    strip_tool_result_content(&mut row);
    let after_first = row.payload.clone();
    strip_tool_result_content(&mut row);
    assert_eq!(row.payload, after_first, "second call is a no-op");
}

#[test]
fn tool_result_strip_handles_missing_result_field() {
    // Image-only tool result — no `result` string was ever written.
    let mut row = row("ToolResult", json!({ "name": "x", "images": ["abc"] }));
    strip_tool_result_content(&mut row);
    let obj = row.payload.as_object().unwrap();
    assert!(!obj.contains_key("result"), "no result to begin with");
    assert_eq!(obj.get("result_stripped"), Some(&json!(true)));
    assert_eq!(obj.get("images"), Some(&json!(["abc"])));
}

// ── event-wait routes are the calling thread's own ──────────────────

use super::actions::refuse_event_waits_for_another_thread;
use crate::api::actor::{
    init_agent_origin_secret, mint_agent_origin_token, HEADER_AGENT_ORIGIN_TOKEN,
};
use axum::http::HeaderMap;

/// Headers as a Lucidos-spawned subprocess sends them: a thread-bound origin
/// token it cannot re-point, minted over `thread_id`.
///
/// The secret is per-engine-startup and installed first-writer-wins, so each
/// test installs one rather than assuming a booted engine did: without it
/// minting returns `None`, every header map comes out empty, and the guard
/// tests would pass by reading a forged-token caller as an ordinary untokened
/// one, which is the opposite of what they assert.
fn agent_headers(thread_id: Option<Uuid>) -> HeaderMap {
    init_agent_origin_secret("harden-test-secret".to_string());
    let mut h = HeaderMap::new();
    let token = mint_agent_origin_token(thread_id, 0, None)
        .expect("the secret is installed above, so minting cannot fail");
    h.insert(HEADER_AGENT_ORIGIN_TOKEN, token.parse().unwrap());
    h
}

/// The isolation the three agent-facing event-wait routes promise. The tools
/// take no thread argument at all, but the HTTP form has a path segment, and a
/// subprocess substituting another thread's id there would get back exactly the
/// capability the argument-less shape removes.
#[test]
fn an_agent_may_only_reach_its_own_thread_s_event_waits() {
    let mine = Uuid::new_v4();
    let theirs = Uuid::new_v4();

    assert!(
        refuse_event_waits_for_another_thread(&agent_headers(Some(mine)), mine).is_ok(),
        "its own thread is the whole point of the route"
    );

    let refused = refuse_event_waits_for_another_thread(&agent_headers(Some(mine)), theirs)
        .expect_err("another thread's id must be refused");
    assert_eq!(refused.0, axum::http::StatusCode::FORBIDDEN);
    // Actionable: it says what to do instead, which is to drop the id.
    assert!(refused.1.contains("event-waits"), "{}", refused.1);
}

/// A subprocess with a token but NO thread (a scheduled `script:` trigger) has
/// no subscriptions of its own, so there is no thread to scope it to. Refused
/// rather than handed the run of every thread.
#[test]
fn a_threadless_subprocess_is_refused_rather_than_given_every_thread() {
    let headers = agent_headers(None);
    assert!(
        refuse_event_waits_for_another_thread(&headers, Uuid::new_v4()).is_err(),
        "a caller with no thread of its own cannot act on one"
    );
}

/// A caller presenting no token is not an agent claiming to be another thread.
/// It is the ordinary local API surface, which every other `/threads/:id/...`
/// route treats the same way, so this check leaves it exactly where it was
/// rather than quietly moving a trust boundary that is not its to move.
#[test]
fn an_untokened_caller_is_left_to_the_ordinary_local_api_rules() {
    assert!(refuse_event_waits_for_another_thread(&HeaderMap::new(), Uuid::new_v4()).is_ok());
}

// ── the Apply gate reads the parking facts off the row ──
//
// `guard_change_action` (api/changes.rs) is a thin wrapper: it asks
// `available_thread_actions_for` and refuses anything absent. So these tests
// ARE the server-side gate, and they also pin the `SELECT`. Drop a column from
// it and `ThreadActionFacts` fails to build the row, which no unit test over
// the pure predicate would ever notice.

/// Seed the minimum a coding-agent thread with a proposed change needs.
#[cfg(test)]
async fn seed_parked_cc_thread(
    pool: &sqlx::PgPool,
    thread_id: Uuid,
    live_event_waits: i32,
    active_children: i32,
) {
    sqlx::query(
        "INSERT INTO thread_summaries
            (thread_id, is_coding_agent, status, coding_agent_proposed,
             live_event_wait_count, active_children_count)
         VALUES ($1, true, 'idle', true, $2, $3)",
    )
    .bind(thread_id)
    .bind(live_event_waits)
    .bind(active_children)
    .execute(pool)
    .await
    .expect("seed thread_summaries");
}

#[tokio::test]
async fn a_parked_thread_is_refused_apply_and_discard_server_side() {
    use crate::engine::thread_lifecycle::Action;
    use crate::test_support::{setup_test_db, teardown_test_db};

    let (pool, db_name) = setup_test_db().await;

    let subscribed = Uuid::new_v4();
    seed_parked_cc_thread(&pool, subscribed, 1, 0).await;
    let actions = super::available_thread_actions_for(&pool, subscribed)
        .await
        .expect("query the parked thread's actions");
    assert!(
        !actions.contains(&Action::Apply) && !actions.contains(&Action::Discard),
        "a thread holding a live event wait must not be resolvable: {:?}",
        actions
    );

    let with_child = Uuid::new_v4();
    seed_parked_cc_thread(&pool, with_child, 0, 1).await;
    let actions = super::available_thread_actions_for(&pool, with_child)
        .await
        .expect("query the parent's actions");
    assert!(
        !actions.contains(&Action::Apply),
        "an active sub-thread must withhold Apply too: {:?}",
        actions
    );

    // The control, and the escape hatch: with nothing left to wake it, the same
    // row is resolvable again. This is the state Stop waiting produces.
    let settled = Uuid::new_v4();
    seed_parked_cc_thread(&pool, settled, 0, 0).await;
    let actions = super::available_thread_actions_for(&pool, settled)
        .await
        .expect("query the settled thread's actions");
    assert!(
        actions.contains(&Action::Apply) && actions.contains(&Action::Discard),
        "clearing the wait must restore both: {:?}",
        actions
    );

    teardown_test_db(&db_name).await;
}
