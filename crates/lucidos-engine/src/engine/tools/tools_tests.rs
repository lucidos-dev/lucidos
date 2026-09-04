//! Validation-path tests for `query_events_impl`, the dereference half of the
//! event-address surface.
//!
//! Every handler is exercised via its standalone `*_impl` function, since
//! the `LucidosEngine` methods are thin wrappers. So the tests need no engine,
//! only a Postgres pool + `EventBus`. This mirrors how `event_bus_tests.rs`
//! exercises bus paths.

use super::{
    merge_thread_queue_policy_patch, parent_filter_arg, parse_required_uuid, parse_source_arg,
    parse_status_arg, query_events_impl, status_filter_arg, BACKUP_SETTINGS_NAVIGATED,
};
use crate::core::store::{EventStore, StatusFilter};
use crate::engine::thread_lifecycle::ThreadStatus;
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

/// The schema advertises an array, but a model that has just used `source`
/// will reach for a comma-separated string. Both are the same request.
#[test]
fn parse_status_arg_takes_an_array_or_a_comma_separated_string() {
    let from_array = parse_status_arg(&json!({"status": ["running", "failed"]}))
        .expect("the advertised array shape");
    let from_string =
        parse_status_arg(&json!({"status": "running, failed"})).expect("the string shape");
    assert_eq!(from_array, from_string);
    assert_eq!(from_array, [ThreadStatus::Running, ThreadStatus::Failed]);
}

#[test]
fn parse_status_arg_is_absent_when_the_model_omits_it() {
    assert!(parse_status_arg(&json!({}))
        .expect("no status is valid")
        .is_empty());
    assert!(parse_status_arg(&json!({"status": null}))
        .expect("an explicit null is the same as omitting it")
        .is_empty());
}

/// A status the model invented must come back as something it can correct. A
/// filter that silently matches nothing would have it report an empty
/// workspace.
#[test]
fn parse_status_arg_refuses_a_value_the_model_invented() {
    let err =
        parse_status_arg(&json!({"status": ["busy"]})).expect_err("'busy' is not a thread status");
    assert!(
        err.starts_with("Error: "),
        "tool errors are prefixed: {err}"
    );
    assert!(err.contains("busy"), "must echo the bad value: {err}");
    assert!(
        err.contains("running") && err.contains("waiting_for_user_answer"),
        "must list what it could have meant: {err}"
    );

    for empty in [json!({"status": []}), json!({"status": ""})] {
        let err = parse_status_arg(&empty).expect_err("an empty status must not mean 'no filter'");
        assert!(err.contains("status"), "{err}");
    }
}

/// Two answers to one question, so the model is told to pick one rather than
/// being handed the intersection of them.
#[test]
fn parse_status_arg_refuses_status_alongside_active() {
    let err = parse_status_arg(&json!({"active": true, "status": ["running"]}))
        .expect_err("both filters at once must be refused");
    assert!(err.contains("not both"), "{err}");
    assert!(
        err.contains("waiting_for_user_answer"),
        "the refusal is also where the model learns what the union is: {err}"
    );
}

#[test]
fn status_filter_arg_prefers_explicit_statuses_and_falls_back_to_active() {
    let statuses = [ThreadStatus::Running];
    assert!(matches!(
        status_filter_arg(&json!({"status": ["running"]}), &statuses),
        StatusFilter::OneOf(_)
    ));
    assert!(matches!(
        status_filter_arg(&json!({"active": true}), &[]),
        StatusFilter::Active(true)
    ));
    assert!(matches!(
        status_filter_arg(&json!({"active": false}), &[]),
        StatusFilter::Active(false)
    ));
    assert!(matches!(
        status_filter_arg(&json!({}), &[]),
        StatusFilter::Any
    ));
}

/// Insert a raw thread event directly into the events table, bypassing the
/// EventBus. The keep handler queries by `(id, aggregate_id, event_type)`
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

// ============================================================================
// `query_events_impl`: read-by-id, the dereference half of ADR 0085
// ============================================================================

/// A caller thread for the tests that pass no `thread_id`. Only the `current`
/// alias reads it, and these tests do not use the alias.
fn some_caller() -> Uuid {
    Uuid::new_v4()
}

/// The three spellings of one address must all reach the same row. The `evt-`
/// form is what a tool result states, and the two bare forms are what a
/// `query_events` result or a log line hands back.
#[tokio::test]
async fn query_events_resolves_one_event_by_any_spelling_of_its_address() {
    let (pool, db) = setup_test_db().await;
    let store = EventStore::new(pool.clone());

    let thread_id = Uuid::new_v4();
    let wanted = Uuid::new_v4();
    insert_thread_event(&pool, wanted, thread_id, "ToolResult", json!({"r": "hit"})).await;
    // A decoy the newest-first window would otherwise return first.
    insert_thread_event(
        &pool,
        Uuid::new_v4(),
        thread_id,
        "ToolResult",
        json!({"r": "decoy"}),
    )
    .await;

    for spelling in [
        format!("evt-{}", wanted.simple()),
        wanted.to_string(),
        wanted.simple().to_string(),
    ] {
        let out = query_events_impl(
            &store,
            &json!({"event_id": spelling.clone()}),
            some_caller(),
        )
        .await
        .unwrap_or_else(|e| panic!("{spelling} must resolve, got: {e}"));
        let parsed: serde_json::Value = serde_json::from_str(&out).expect("json wrapper");
        assert_eq!(parsed["returned"], 1, "{spelling} must return one row");
        assert_eq!(parsed["events"][0]["id"], wanted.to_string());
        assert_eq!(parsed["events"][0]["payload"]["r"], "hit");
    }

    pool.close().await;
    teardown_test_db(&db).await;
}

/// THE point of the dereference. The address a tool result states is its
/// CALL's id, because that is the form a keep takes. What the sweep dropped is
/// the RESULT, so reading the call must bring it back. One row of arguments
/// would be the half the agent still remembers.
#[tokio::test]
async fn dereferencing_a_tool_call_brings_back_its_result() {
    let (pool, db) = setup_test_db().await;
    let store = EventStore::new(pool.clone());

    let thread_id = Uuid::new_v4();
    let call = Uuid::new_v4();
    insert_thread_event(
        &pool,
        call,
        thread_id,
        "ToolCalled",
        json!({"name": "run_bash", "args": {"command": "ls"}}),
    )
    .await;
    insert_thread_event(
        &pool,
        Uuid::new_v4(),
        thread_id,
        "ToolResult",
        json!({"name": "run_bash", "result": "total 4", "tool_called_event_id": call}),
    )
    .await;
    // A second call's result, to prove the pairing is by id and not by
    // "the next ToolResult in the thread".
    insert_thread_event(
        &pool,
        Uuid::new_v4(),
        thread_id,
        "ToolResult",
        json!({"name": "read_file", "result": "other", "tool_called_event_id": Uuid::new_v4()}),
    )
    .await;

    let out = query_events_impl(
        &store,
        &json!({"event_id": format!("evt-{}", call.simple())}),
        some_caller(),
    )
    .await
    .expect("the call resolves");
    let parsed: serde_json::Value = serde_json::from_str(&out).expect("json wrapper");
    assert_eq!(parsed["returned"], 2, "the pair, not just the call");
    assert_eq!(parsed["events"][0]["event_type"], "ToolCalled");
    assert_eq!(parsed["events"][1]["event_type"], "ToolResult");
    assert_eq!(parsed["events"][1]["payload"]["result"], "total 4");

    pool.close().await;
    teardown_test_db(&db).await;
}

/// An orphan call has no result to add, which is an answer rather than a
/// failure: the engine crashed mid-tool, and the call really is all there is.
#[tokio::test]
async fn dereferencing_an_orphan_call_returns_the_call_alone() {
    let (pool, db) = setup_test_db().await;
    let store = EventStore::new(pool.clone());

    let thread_id = Uuid::new_v4();
    let call = Uuid::new_v4();
    insert_thread_event(&pool, call, thread_id, "ToolCalled", json!({"name": "x"})).await;

    let out = query_events_impl(
        &store,
        &json!({"event_id": call.to_string()}),
        some_caller(),
    )
    .await
    .expect("an orphan call still resolves");
    let parsed: serde_json::Value = serde_json::from_str(&out).expect("json wrapper");
    assert_eq!(parsed["returned"], 1);
    assert_eq!(parsed["events"][0]["event_type"], "ToolCalled");

    pool.close().await;
    teardown_test_db(&db).await;
}

/// The pair is added only for a by-id dereference. A windowed query full of
/// `ToolCalled` rows must return exactly what it matched, or `limit` would
/// stop meaning what it says.
#[tokio::test]
async fn a_windowed_query_never_pulls_in_paired_results() {
    let (pool, db) = setup_test_db().await;
    let store = EventStore::new(pool.clone());

    let thread_id = Uuid::new_v4();
    let call = Uuid::new_v4();
    insert_thread_event(&pool, call, thread_id, "ToolCalled", json!({"name": "x"})).await;
    insert_thread_event(
        &pool,
        Uuid::new_v4(),
        thread_id,
        "ToolResult",
        json!({"name": "x", "result": "r", "tool_called_event_id": call}),
    )
    .await;

    let out = query_events_impl(&store, &json!({"event_type": "ToolCalled"}), some_caller())
        .await
        .expect("plain query");
    let parsed: serde_json::Value = serde_json::from_str(&out).expect("json wrapper");
    assert_eq!(parsed["returned"], 1, "only the matched row");
    assert_eq!(parsed["events"][0]["event_type"], "ToolCalled");

    pool.close().await;
    teardown_test_db(&db).await;
}

/// A pointer that resolves to nothing fails loudly. An empty window would
/// read to the agent as "the event is gone", which is a different claim and
/// the wrong one to act on.
#[tokio::test]
async fn query_events_errors_when_an_address_resolves_to_nothing() {
    let (pool, db) = setup_test_db().await;
    let store = EventStore::new(pool.clone());

    let missing = Uuid::new_v4();
    let out = query_events_impl(
        &store,
        &json!({"event_id": format!("evt-{}", missing.simple())}),
        some_caller(),
    )
    .await;
    assert!(
        matches!(&out, Err(msg) if msg.contains("no event has id") && msg.contains(&missing.to_string())),
        "a dangling pointer must name the id it could not resolve, got: {:?}",
        out
    );

    // Same when a contradicting filter is what excluded it, and the message
    // says so, because the id itself is fine in that case.
    let present = Uuid::new_v4();
    let thread_id = Uuid::new_v4();
    insert_thread_event(&pool, present, thread_id, "ToolResult", json!({})).await;
    let out = query_events_impl(
        &store,
        &json!({"event_id": present.to_string(), "event_type": "MessageReceived"}),
        some_caller(),
    )
    .await;
    assert!(
        matches!(&out, Err(msg) if msg.contains("drop any other filter")),
        "a contradicting filter must be named as a possible cause, got: {:?}",
        out
    );

    pool.close().await;
    teardown_test_db(&db).await;
}

/// A malformed address is refused, never dropped. Ignoring it would widen a
/// one-row lookup into the newest 50 events of every type. The agent would
/// then read that window as the event it asked for.
#[tokio::test]
async fn query_events_refuses_a_malformed_address_rather_than_widening() {
    let (pool, db) = setup_test_db().await;
    let store = EventStore::new(pool.clone());

    for bad in [json!("evt-not-a-uuid"), json!("nonsense"), json!("")] {
        let out = query_events_impl(&store, &json!({"event_id": bad}), some_caller()).await;
        assert!(
            matches!(&out, Err(msg) if msg.contains("is not an event address")),
            "{bad} must be refused, got: {:?}",
            out
        );
    }
    // A non-string must not fall down the "absent" arm either.
    let out = query_events_impl(&store, &json!({"event_id": ["deadbeef"]}), some_caller()).await;
    assert!(
        matches!(&out, Err(msg) if msg.contains("is not an event address")),
        "an array must be refused, got: {:?}",
        out
    );

    pool.close().await;
    teardown_test_db(&db).await;
}

/// Read-by-id inherits the byte budget. The row a pointer names is usually a
/// `ToolResult`, which is exactly the row that runs to megabytes. Returning
/// one unconditionally is how a dereference blows the next turn's prompt.
#[tokio::test]
async fn query_events_by_id_still_honours_the_byte_limit() {
    let (pool, db) = setup_test_db().await;
    let store = EventStore::new(pool.clone());

    let thread_id = Uuid::new_v4();
    let big = Uuid::new_v4();
    insert_thread_event(
        &pool,
        big,
        thread_id,
        "ToolResult",
        json!({"result": "x".repeat(8000)}),
    )
    .await;

    let out = query_events_impl(
        &store,
        &json!({"event_id": big.to_string(), "byte_limit": 1024}),
        some_caller(),
    )
    .await
    .expect("a truncated response is still a response");
    let parsed: serde_json::Value = serde_json::from_str(&out).expect("json wrapper");
    assert_eq!(parsed["truncated"], true);
    assert_eq!(parsed["returned"], 0);
    assert_eq!(
        parsed["total_matching"], 1,
        "the row matched, it just did not fit"
    );
    assert!(parsed["hint"].is_string(), "truncation must carry its hint");

    pool.close().await;
    teardown_test_db(&db).await;
}

/// Absent `event_id`, the query behaves exactly as it always did. Every
/// caller predating the argument passes nothing, so the filter can only
/// narrow, never widen or reorder.
#[tokio::test]
async fn query_events_without_an_address_is_unchanged() {
    let (pool, db) = setup_test_db().await;
    let store = EventStore::new(pool.clone());

    let thread_id = Uuid::new_v4();
    for i in 0..3 {
        insert_thread_event(
            &pool,
            Uuid::new_v4(),
            thread_id,
            "ToolResult",
            json!({"i": i}),
        )
        .await;
    }

    let out = query_events_impl(&store, &json!({"event_type": "ToolResult"}), some_caller())
        .await
        .expect("plain query");
    let parsed: serde_json::Value = serde_json::from_str(&out).expect("json wrapper");
    assert_eq!(parsed["returned"], 3);
    assert_eq!(parsed["truncated"], false);

    pool.close().await;
    teardown_test_db(&db).await;
}

/// `thread_id: "current"` cost a whole round every time the model guessed it,
/// because reading your own thread back is the commonest reason to call this.
/// The alias resolves from the tool-execution context, so the model cannot aim
/// it at another conversation.
#[tokio::test]
async fn thread_id_current_reads_the_calling_thread() {
    let (pool, db) = setup_test_db().await;
    let store = EventStore::new(pool.clone());

    let caller = Uuid::new_v4();
    let other = Uuid::new_v4();
    insert_thread_event(
        &pool,
        Uuid::new_v4(),
        caller,
        "ToolResult",
        json!({"r": "mine"}),
    )
    .await;
    insert_thread_event(
        &pool,
        Uuid::new_v4(),
        other,
        "ToolResult",
        json!({"r": "theirs"}),
    )
    .await;

    for alias in ["current", "this", "  Current  ", "THIS"] {
        let out = query_events_impl(&store, &json!({"thread_id": alias}), caller)
            .await
            .unwrap_or_else(|e| panic!("{alias} must resolve, got: {e}"));
        let parsed: serde_json::Value = serde_json::from_str(&out).expect("json wrapper");
        assert_eq!(
            parsed["returned"], 1,
            "{alias} must return the caller's row"
        );
        assert_eq!(parsed["events"][0]["payload"]["r"], "mine");
    }

    pool.close().await;
    teardown_test_db(&db).await;
}

/// The alias is the only string that is not an id. Every other guard stands:
/// a bad uuid and a non-string are still refused, never widened to every
/// thread. The refusal now states the alias, which is the one-step answer.
#[tokio::test]
async fn a_bad_thread_id_is_still_refused_and_names_the_alias() {
    let (pool, db) = setup_test_db().await;
    let store = EventStore::new(pool.clone());

    let caller = Uuid::new_v4();
    insert_thread_event(&pool, Uuid::new_v4(), caller, "ToolResult", json!({})).await;

    for bad in [json!("currently"), json!("the one we discussed"), json!("")] {
        let out = query_events_impl(&store, &json!({"thread_id": bad}), caller).await;
        assert!(
            matches!(&out, Err(msg) if msg.contains("is not a uuid") && msg.contains("'current'")),
            "{bad} must be refused and point at the alias, got: {:?}",
            out
        );
    }

    // A non-string must not fall down the "absent" arm, which would widen the
    // query to every thread.
    for bad in [
        json!([caller.to_string()]),
        json!({"id": "current"}),
        json!(7),
    ] {
        let out = query_events_impl(&store, &json!({"thread_id": bad}), caller).await;
        assert!(
            matches!(&out, Err(msg) if msg.contains("must be a uuid string or 'current'")),
            "{bad} must be refused rather than widening, got: {:?}",
            out
        );
    }

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
// `parse_required_uuid`: the required-UUID guard behind `apply_change`'s
// `change_id` and `apply_when_settled`'s `thread_id`.
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
        let out = parse_required_uuid(&bad, "change_id");
        assert!(
            matches!(&out, Err(msg) if msg.contains("change_id is required")),
            "{bad:?} should error as required, got: {out:?}"
        );
    }
}

#[test]
fn apply_change_rejects_malformed_change_id() {
    let out = parse_required_uuid(&json!({"change_id": "not-a-uuid"}), "change_id");
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
    let out = parse_required_uuid(&json!({"change_id": format!("  {id}  ")}), "change_id");
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
        max_event_trigger_depth: 5,
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
