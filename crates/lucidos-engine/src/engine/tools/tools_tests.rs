//! Validation-path tests for `dismiss_from_context_impl`.
//!
//! The handler is exercised via the standalone `dismiss_from_context_impl`
//! function (the `LucidosEngine::execute_dismiss_from_context` method is a
//! thin wrapper) so the tests don't need to boot a full engine — only a
//! Postgres pool + `EventBus`. This mirrors how `event_bus_tests.rs`
//! exercises bus paths.

use super::dismiss_from_context_impl;
use crate::engine::event_bus::EventBus;
use crate::test_support::{setup_test_db, teardown_test_db};
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

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
    let out =
        dismiss_from_context_impl(&pool, &bus, &json!({"event_id": "   "}), thread_id).await;
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
    let out = dismiss_from_context_impl(
        &pool,
        &bus,
        &json!({"event_id": prefixed}),
        thread_id,
    )
    .await;
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
