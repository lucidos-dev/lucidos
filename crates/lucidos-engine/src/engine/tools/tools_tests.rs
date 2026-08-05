//! Validation-path tests for `dismiss_from_context_impl`.
//!
//! The handler is exercised via the standalone `dismiss_from_context_impl`
//! function (the `LucidosEngine::execute_dismiss_from_context` method is a
//! thin wrapper) so the tests don't need to boot a full engine — only a
//! Postgres pool + `EventBus`. This mirrors how `event_bus_tests.rs`
//! exercises bus paths.

use super::{
    dismiss_from_context_impl, merge_thread_queue_policy_patch, parent_filter_arg,
    parse_apply_change_id, parse_source_arg, BACKUP_SETTINGS_NAVIGATED,
};
use crate::engine::event_bus::EventBus;
use crate::engine::thread_queue::{CapacityPolicy, OverflowPolicy};
use crate::test_support::{setup_test_db, teardown_test_db};
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

#[test]
fn parse_source_arg_normalizes_coding_agent_alias() {
    let parsed = parse_source_arg(Some(&json!("chat, coding-agent, claude_code, trigger")))
        .expect("source filter");
    assert_eq!(
        parsed,
        vec!["chat", "claude_code", "claude_code", "trigger"]
    );

    let parsed =
        parse_source_arg(Some(&json!(["coding-agent", "trigger", " "]))).expect("source filter");
    assert_eq!(parsed, vec!["claude_code", "trigger"]);
}

#[test]
fn parse_source_arg_collapses_blank_inputs() {
    assert_eq!(parse_source_arg(Some(&json!(" , , "))), None);
    assert_eq!(parse_source_arg(Some(&json!(["", "  "]))), None);
    assert_eq!(parse_source_arg(Some(&json!(42))), None);
}

/// Insert a raw thread event directly into the events table, bypassing the
/// EventBus. The dismiss handler queries by `(id, aggregate_id, event_type)`
/// — that's the entire fixture surface we need to drive the validation
/// branches.
async fn insert_thread_event(
    pool: &PgPool,
    event_id: Uuid,
    thread_id: Uuid,
    event_type: &str,
    payload: serde_json::Value,
) {
    sqlx::query(
        "INSERT INTO events (id, aggregate, aggregate_id, event_type, payload, created, thread_id) \
         VALUES ($1, 'thread', $2, $3, $4, NOW(), $2::uuid)",
    )
    .bind(event_id)
    .bind(thread_id.to_string())
    .bind(event_type)
    .bind(payload)
    .execute(pool)
    .await
    .expect("insert thread event");
}

#[tokio::test]
async fn dismiss_from_context_rejects_missing_event_id() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let thread_id = Uuid::new_v4();
    let out = dismiss_from_context_impl(&pool, &bus, &json!({}), thread_id).await;
    assert!(
        matches!(&out, Err(msg) if msg.contains("event_id is required")),
        "missing event_id should error, got: {:?}",
        out
    );

    // Empty string also counts as missing — guard against the LLM passing "".
    let out = dismiss_from_context_impl(&pool, &bus, &json!({"event_id": ""}), thread_id).await;
    assert!(
        matches!(&out, Err(msg) if msg.contains("event_id is required")),
        "empty event_id should error, got: {:?}",
        out
    );

    // Whitespace-only also counts as missing.
    let out = dismiss_from_context_impl(&pool, &bus, &json!({"event_id": "   "}), thread_id).await;
    assert!(
        matches!(&out, Err(msg) if msg.contains("event_id is required")),
        "whitespace-only event_id should error, got: {:?}",
        out
    );

    pool.close().await;
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn dismiss_from_context_rejects_malformed_event_id() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let thread_id = Uuid::new_v4();
    let out = dismiss_from_context_impl(
        &pool,
        &bus,
        &json!({"event_id": "not-a-uuid-at-all"}),
        thread_id,
    )
    .await;
    assert!(
        matches!(&out, Err(msg) if msg.contains("must be a UUID")),
        "malformed event_id should error, got: {:?}",
        out
    );

    // The `evt-` prefix without a valid UUID body must also fail validation
    // (otherwise typos in the prefix path silently succeed).
    let out = dismiss_from_context_impl(
        &pool,
        &bus,
        &json!({"event_id": "evt-not-a-uuid"}),
        thread_id,
    )
    .await;
    assert!(
        matches!(&out, Err(msg) if msg.contains("must be a UUID")),
        "evt-<garbage> should error, got: {:?}",
        out
    );

    pool.close().await;
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn dismiss_from_context_rejects_event_in_different_thread() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    // Insert a ToolCalled on thread A; the dismiss call is scoped to thread B
    // — the (event_id, aggregate_id) join must not match across threads.
    let thread_a = Uuid::new_v4();
    let thread_b = Uuid::new_v4();
    let event_id = Uuid::new_v4();
    insert_thread_event(
        &pool,
        event_id,
        thread_a,
        "ToolCalled",
        json!({"tool": "read_file"}),
    )
    .await;

    let out = dismiss_from_context_impl(
        &pool,
        &bus,
        &json!({"event_id": event_id.to_string()}),
        thread_b,
    )
    .await;
    assert!(
        matches!(&out, Err(msg) if msg.contains("not found or not dismissible")),
        "cross-thread dismiss must error, got: {:?}",
        out
    );

    // And the negative side: nothing should have been emitted on thread B.
    let dismissed_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM events WHERE aggregate_id = $1 AND event_type = 'ContextDismissed'",
    )
    .bind(thread_b.to_string())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        dismissed_count, 0,
        "no ContextDismissed should have been emitted on the wrong-thread call"
    );

    pool.close().await;
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn dismiss_from_context_rejects_non_dismissible_event_type() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    // Same-thread, same event_id, but the event_type is `ResponseGenerated`
    // — not in the `('ToolCalled', 'ChildThreadCompleted')` allow list.
    // Dismissing a ResponseGenerated would corrupt history rendering.
    let thread_id = Uuid::new_v4();
    let event_id = Uuid::new_v4();
    insert_thread_event(
        &pool,
        event_id,
        thread_id,
        "ResponseGenerated",
        json!({"text": "done"}),
    )
    .await;

    let out = dismiss_from_context_impl(
        &pool,
        &bus,
        &json!({"event_id": event_id.to_string()}),
        thread_id,
    )
    .await;
    assert!(
        matches!(&out, Err(msg) if msg.contains("not found or not dismissible")),
        "non-dismissible event_type must error, got: {:?}",
        out
    );

    pool.close().await;
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn dismiss_from_context_succeeds_for_tool_called_in_same_thread() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let thread_id = Uuid::new_v4();
    let event_id = Uuid::new_v4();
    insert_thread_event(
        &pool,
        event_id,
        thread_id,
        "ToolCalled",
        json!({"tool": "read_file"}),
    )
    .await;

    let out = dismiss_from_context_impl(
        &pool,
        &bus,
        &json!({"event_id": event_id.to_string()}),
        thread_id,
    )
    .await;
    let success_text = out
        .as_ref()
        .expect("valid same-thread ToolCalled dismiss must succeed")
        .clone();
    assert!(
        success_text.contains("Dismissed event") && success_text.contains(&event_id.to_string()),
        "success string must echo the event id, got: {}",
        success_text
    );

    // Verify the ContextDismissed event was actually persisted on the same
    // thread, with the correct dismissed_event_id payload.
    let row: Option<(serde_json::Value,)> = sqlx::query_as(
        "SELECT payload FROM events \
         WHERE aggregate_id = $1 AND event_type = 'ContextDismissed'",
    )
    .bind(thread_id.to_string())
    .fetch_optional(&pool)
    .await
    .unwrap();
    let payload = row.expect("ContextDismissed must be persisted").0;
    let dismissed_id = payload
        .get("dismissed_event_id")
        .and_then(|v| v.as_str())
        .expect("dismissed_event_id field present");
    assert_eq!(
        dismissed_id,
        event_id.to_string(),
        "dismissed_event_id must round-trip the input event id"
    );

    pool.close().await;
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn dismiss_from_context_accepts_evt_prefixed_form() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    // C1: tool blocks render the synthetic id as `evt-<32-hex-uuid>`. The
    // handler must accept that shape verbatim — the LLM never sees the raw
    // hyphenated UUID for tool blocks, only this form.
    let thread_id = Uuid::new_v4();
    let event_id = Uuid::new_v4();
    insert_thread_event(
        &pool,
        event_id,
        thread_id,
        "ToolCalled",
        json!({"tool": "read_file"}),
    )
    .await;

    let prefixed = format!("evt-{}", event_id.simple());
    let out =
        dismiss_from_context_impl(&pool, &bus, &json!({"event_id": prefixed}), thread_id).await;
    assert!(out.is_ok(), "evt-<uuid> form must succeed, got: {:?}", out);

    // And the bare hyphenated form should also still work (regression
    // check — both shapes the description promises are accepted).
    let event_id_2 = Uuid::new_v4();
    insert_thread_event(
        &pool,
        event_id_2,
        thread_id,
        "ChildThreadCompleted",
        json!({"child_thread_id": Uuid::new_v4().to_string(), "status": "success"}),
    )
    .await;
    let out = dismiss_from_context_impl(
        &pool,
        &bus,
        &json!({"event_id": event_id_2.to_string()}),
        thread_id,
    )
    .await;
    assert!(
        out.is_ok(),
        "bare hyphenated UUID must succeed, got: {:?}",
        out
    );

    pool.close().await;
    teardown_test_db(&db).await;
}

// ============================================================================
// `build_query_events_response` — byte-budget + wrapper shape
//
// Pure synchronous fn that fans the LLM-tool result through a compact-JSON
// budget. No DB needed — we feed `EventRow`s directly. Motivating bug: a
// single `query_events(event_type=ToolResult, limit=300)` returned 2.3 MB
// and blew the 1M-token prompt cap on the next turn.
// ============================================================================

mod build_query_events_response_tests {
    use crate::core::EventRow;
    use crate::engine::tools::build_query_events_response;
    use chrono::{TimeZone, Utc};
    use serde_json::json;
    use uuid::Uuid;

    fn row(event_type: &str, payload_chars: usize) -> EventRow {
        EventRow {
            id: Uuid::new_v4(),
            event_type: event_type.to_string(),
            payload: json!({ "summary": "x".repeat(payload_chars) }),
            created: Utc.timestamp_opt(1_700_000_000, 0).unwrap(),
            thread_id: None,
            sequence: None,
        }
    }

    /// Plenty of budget → every event passes through. `truncated:false`, no
    /// `hint`. The wrapper shape is the contract — the LLM tool description
    /// promises it, so keep this test load-bearing.
    #[test]
    fn returns_full_set_when_under_budget() {
        let events = vec![row("Small", 10), row("Small", 10), row("Small", 10)];
        let out = build_query_events_response(&events, 100_000);
        let parsed: serde_json::Value = serde_json::from_str(&out).expect("compact JSON");
        assert_eq!(parsed["total_matching"], 3);
        assert_eq!(parsed["returned"], 3);
        assert_eq!(parsed["truncated"], false);
        assert!(parsed.get("hint").is_none(), "no hint when not truncated");
        assert_eq!(parsed["events"].as_array().unwrap().len(), 3);
    }

    /// Stop on the first event that doesn't fit; include everything before.
    /// `truncated:true` plus a non-empty `hint` is the LLM's signal to
    /// narrow the next call.
    #[test]
    fn stops_mid_list_when_byte_limit_hit() {
        let events = vec![
            row("Tiny", 10),
            row("Tiny", 10),
            row("Huge", 50_000),
            row("Tiny", 10),
        ];
        // Enough room for the two small events but not the 50 KB blob.
        let out = build_query_events_response(&events, 1_000);
        let parsed: serde_json::Value = serde_json::from_str(&out).expect("compact JSON");
        assert_eq!(parsed["total_matching"], 4);
        assert_eq!(parsed["returned"], 2);
        assert_eq!(parsed["truncated"], true);
        assert!(parsed["hint"].as_str().unwrap().contains("narrow"));
    }

    /// A first event larger than the entire budget must return zero events,
    /// `truncated:true`, and a hint — never an unbounded blob, never a
    /// silent empty array that looks like "no matching events".
    #[test]
    fn empty_events_when_first_alone_exceeds_budget() {
        let events = vec![row("Huge", 50_000)];
        let out = build_query_events_response(&events, 1_000);
        let parsed: serde_json::Value = serde_json::from_str(&out).expect("compact JSON");
        assert_eq!(parsed["total_matching"], 1);
        assert_eq!(parsed["returned"], 0);
        assert_eq!(parsed["truncated"], true);
        assert_eq!(parsed["byte_size"], 0);
        assert!(parsed["hint"].is_string());
    }

    /// The truncation hint must only name arguments `query_events` accepts.
    ///
    /// It used to say "Narrow by aggregate_id". `aggregate_id` is a real
    /// column on the `events` table but NOT an argument of this tool, so a
    /// model following the advice re-issued the same query with an ignored
    /// parameter and got the identical truncated result. The events table's
    /// other non-argument columns are checked too, because that is the reach
    /// that produced the bug: someone read the schema of the storage and
    /// mistook it for the schema of the tool.
    #[test]
    fn truncation_hint_names_only_real_query_arguments() {
        let events_domain = crate::capability_manifest::domain_for_tool("events")
            .expect("events domain is in the capability manifest");
        let query_op = events_domain
            .operations
            .iter()
            .find(|op| op.action == "query")
            .expect("events domain has a `query` operation");
        let schema: serde_json::Value = serde_json::from_str(
            query_op
                .llm_schema
                .expect("the query operation declares an explicit LLM schema"),
        )
        .expect("the LLM schema is a JSON object");
        let properties = schema.as_object().expect("schema is a JSON object");

        for filter in crate::engine::tools::QUERY_EVENTS_HINT_FILTERS {
            assert!(
                properties.contains_key(*filter),
                "the truncation hint tells the model to narrow with `{filter}`, \
                 which is not a property of the events `query` LLM schema \
                 (it accepts: {:?}). Naming a filter the tool ignores makes the \
                 model retry into the identical truncated result.",
                properties.keys().collect::<Vec<_>>(),
            );
            assert!(
                crate::engine::tools::QUERY_EVENTS_TRUNCATION_HINT.contains(*filter),
                "`{filter}` is listed in QUERY_EVENTS_HINT_FILTERS but the hint \
                 text never mentions it, so the check above guards nothing.",
            );
        }

        // Columns of the `events` table that are NOT query arguments. Reaching
        // for one of these in the hint is the exact mistake being guarded.
        for column in ["aggregate", "aggregate_id", "thread_id", "sequence"] {
            assert!(
                !crate::engine::tools::QUERY_EVENTS_TRUNCATION_HINT.contains(column),
                "the truncation hint mentions `{column}`, an events-table column \
                 that `query_events` cannot filter on.",
            );
        }
    }

    /// Empty store input round-trips to an empty array with no hint.
    #[test]
    fn empty_input_yields_empty_array_no_hint() {
        let out = build_query_events_response(&[], 100_000);
        let parsed: serde_json::Value = serde_json::from_str(&out).expect("compact JSON");
        assert_eq!(parsed["total_matching"], 0);
        assert_eq!(parsed["returned"], 0);
        assert_eq!(parsed["truncated"], false);
        assert_eq!(parsed["byte_size"], 0);
        assert!(parsed.get("hint").is_none());
        assert_eq!(parsed["events"].as_array().unwrap().len(), 0);
    }

    /// The whole point of the byte budget is to keep the wire small. Verify
    /// the output is compact (no pretty-print whitespace) — the original
    /// bug was a `to_string_pretty` call that inflated every result ~30%.
    #[test]
    fn output_is_compact_json_not_pretty_printed() {
        let events = vec![row("Tiny", 10)];
        let out = build_query_events_response(&events, 100_000);
        assert!(
            !out.contains("\n  "),
            "result must be compact JSON (no indentation): {}",
            out
        );
    }

    /// Pin the per-call caps that gate `query_events`. The May 25
    /// workspace-learning trigger sent 1.54 M tokens to a 1 M-cap Opus
    /// API after chaining 8 calls at the prior (looser) defaults
    /// (`limit: 300/500`, `byte_limit: 256 KB`). The tightened bounds
    /// must not silently regress in a future "let me bump this for
    /// flexibility" refactor — the LLM cannot enforce its own
    /// discipline if the schema lets it bypass.
    #[test]
    fn query_events_caps_match_workspace_learning_recipe() {
        use crate::engine::tools::{QUERY_EVENTS_BYTE_BUDGET, QUERY_EVENTS_LIMIT};
        assert_eq!(QUERY_EVENTS_LIMIT.default, 50);
        assert_eq!(QUERY_EVENTS_LIMIT.max, 200);
        assert_eq!(QUERY_EVENTS_BYTE_BUDGET.default, 128 * 1024);
        assert_eq!(QUERY_EVENTS_BYTE_BUDGET.max, 512 * 1024);
    }
}

// ============================================================================
// `parse_apply_change_id` — the `apply_change` tool's required-UUID guard.
//
// Pure synchronous fn (factored out of the handler so these validation
// branches need no engine). The handler refuses to call the heavyweight
// `LucidosEngine::apply_change` merge pipeline without a well-formed target.
// ============================================================================

#[test]
fn apply_change_rejects_missing_change_id() {
    // Missing, null, empty, and whitespace-only all collapse to "required".
    for bad in [
        json!({}),
        json!({"change_id": null}),
        json!({"change_id": ""}),
        json!({"change_id": "   "}),
    ] {
        let out = parse_apply_change_id(&bad);
        assert!(
            matches!(&out, Err(msg) if msg.contains("change_id is required")),
            "{bad:?} should error as required, got: {out:?}"
        );
    }
}

#[test]
fn apply_change_rejects_malformed_change_id() {
    let out = parse_apply_change_id(&json!({"change_id": "not-a-uuid"}));
    assert!(
        matches!(&out, Err(msg) if msg.contains("not a valid UUID")),
        "malformed change_id should error, got: {out:?}"
    );
}

#[test]
fn apply_change_accepts_valid_uuid_trimming_whitespace() {
    let id = Uuid::new_v4();
    // Surrounding whitespace is trimmed before parsing — the LLM occasionally
    // pads string args.
    let out = parse_apply_change_id(&json!({"change_id": format!("  {id}  ")}));
    assert_eq!(out.expect("valid padded UUID must parse"), id);
}

#[test]
fn thread_queue_policy_patch_merges_with_current_policy() {
    let current = CapacityPolicy {
        max_concurrent_total: 16,
        max_concurrent_event_trigger: 6,
        max_concurrent_cron: 6,
        max_concurrent_sub_thread: 8,
        max_concurrent_coding_agent: 12,
        max_concurrent_per_trigger: 1,
        max_queued_per_trigger: 25,
        reserved_background: 4,
        overflow: OverflowPolicy::DropOldest,
    };

    let out = merge_thread_queue_policy_patch(
        current.clone(),
        &json!({
            "max_concurrent_coding_agent": 14,
            "overflow": "pause-trigger"
        }),
    )
    .expect("valid patch should merge");

    assert_eq!(out.max_concurrent_total, current.max_concurrent_total);
    assert_eq!(
        out.max_concurrent_event_trigger,
        current.max_concurrent_event_trigger
    );
    assert_eq!(out.max_concurrent_coding_agent, 14);
    assert_eq!(out.overflow, OverflowPolicy::PauseTrigger);
}

#[test]
fn thread_queue_policy_patch_rejects_empty_or_bad_fields() {
    let current = CapacityPolicy::default();
    let out = merge_thread_queue_policy_patch(current.clone(), &json!({}));
    assert!(
        matches!(&out, Err(msg) if msg.contains("at least one")),
        "empty patch should error, got: {out:?}"
    );

    let out =
        merge_thread_queue_policy_patch(current.clone(), &json!({"max_concurrent_total": "12"}));
    assert!(
        matches!(&out, Err(msg) if msg.contains("max_concurrent_total")),
        "string cap should error, got: {out:?}"
    );

    let out =
        merge_thread_queue_policy_patch(current.clone(), &json!({"max_queued_per_trigger": 0}));
    assert!(
        matches!(&out, Err(msg) if msg.contains("at least 1")),
        "zero queue cap should error, got: {out:?}"
    );

    let out = merge_thread_queue_policy_patch(current, &json!({"overflow": "delete-all"}));
    assert!(
        matches!(&out, Err(msg) if msg.contains("overflow")),
        "unknown overflow should error, got: {out:?}"
    );
}

#[test]
fn thread_queue_policy_patch_rejects_unknown_fields() {
    let out = merge_thread_queue_policy_patch(CapacityPolicy::default(), &json!({"max_total": 12}));
    assert!(
        matches!(&out, Err(msg) if msg.contains("unknown Thread Queue policy field")),
        "unknown field should error, got: {out:?}"
    );
}

/// The blurb the agent gets after landing on Settings → System → Backup must
/// name Settings → Accounts as where an account is connected, and must not
/// re-acquire the two claims that rotted: an in-app backup LIST and an in-app
/// RESTORE. Both moved to the workspace picker, and the stale text sent a user
/// hunting for accounts on the Backup page (2026-08-05).
#[test]
fn backup_navigation_names_the_accounts_page_and_not_a_restore_ui() {
    let s = BACKUP_SETTINGS_NAVIGATED;
    assert!(
        s.contains("Settings → Accounts"),
        "must point at the page that actually connects an account: {s}"
    );
    assert!(
        s.contains("workspace picker"),
        "restore lives in the workspace picker and the agent must say so: {s}"
    );
    let lower = s.to_lowercase();
    assert!(
        !lower.contains("restore from an existing"),
        "there is no in-app restore button to advertise: {s}"
    );
    assert!(
        !lower.contains("list of available cloud backups"),
        "the page shows no backup list: {s}"
    );
}

/// `my_children` resolves to the CALLER's ambient thread id, never to anything
/// the model supplies. There is no `parent` argument on the LLM surface to
/// supply, which is the point: a model asking for "my children" cannot name a
/// thread that is not its own.
#[test]
fn my_children_resolves_to_the_ambient_caller_thread() {
    let caller = Uuid::new_v4();

    assert_eq!(
        parent_filter_arg(&json!({"my_children": true}), caller),
        Some(caller),
        "my_children: true scopes the listing to the caller's own children"
    );
    assert_eq!(
        parent_filter_arg(&json!({"my_children": false}), caller),
        None,
        "my_children: false is no filter, not an empty one"
    );
    assert_eq!(
        parent_filter_arg(&json!({}), caller),
        None,
        "omitted is no filter"
    );

    // A model that invents a `parent` argument gets no filter at all rather
    // than a listing of someone else's children.
    let someone_else = Uuid::new_v4();
    assert_eq!(
        parent_filter_arg(&json!({"parent": someone_else.to_string()}), caller),
        None,
        "the LLM surface exposes no parent argument, so an invented one is inert"
    );
    assert_eq!(
        parent_filter_arg(
            &json!({"my_children": true, "parent": someone_else.to_string()}),
            caller
        ),
        Some(caller),
        "and an invented one alongside my_children still resolves to the caller"
    );
}
