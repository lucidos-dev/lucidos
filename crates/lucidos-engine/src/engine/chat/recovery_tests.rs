//! Tests for the orphan-thread / orphan-toolcall recovery sweeps.
//!
//! Covers the user-spotted regression where a thread the user archived got
//! reanimated to inbox days later by an engine restart: the sweep emitted
//! `ResponseAborted { cause: RecoveryAfterRestart }` against the archived
//! row, and `thread_lifecycle::resolve_transition` routes every
//! `ResponseAborted` `to_inbox` regardless of cause. The fix is at the
//! source: the sweep's enumeration query filters out archived threads, so
//! the recovery event never gets emitted in the first place.
//!
//! The tests run the same `ORPHAN_THREADS_SQL` / `ORPHAN_TOOL_CALLS_SQL`
//! `const` strings the production code uses, so a future change to either
//! query that drops the `archive_state != 'archived'` filter fails this
//! suite immediately.

use super::{ORPHAN_THREADS_SQL, ORPHAN_TOOL_CALLS_SQL};
use crate::core::EventRow;
use crate::engine::event_bus::{BusEvent, EventBus};
use crate::engine::thread_events::{ActorMode, EventMeta, ThreadEvent};
use crate::test_support::{setup_test_db, teardown_test_db};
use serde_json::json;
use std::time::Duration;
use uuid::Uuid;

/// `ORPHAN_THREADS_SQL` uses a strict `last_activity > last_start` predicate
/// keyed off the `created` timestamp (PG `now()`, microsecond resolution).
/// Two `bus.emit()` calls without any gap can land in the same microsecond
/// on fast hardware / under CI load, collapsing the predicate to `T > T =
/// FALSE` and silently dropping the orphan from the result set. Production
/// streaming has tens of ms gaps; tests have zero gap unless we add one
/// here. A single millisecond is plenty — the per-event persist work
/// already takes much longer than that, this just guarantees the timestamps
/// are distinct.
async fn tick() {
    tokio::time::sleep(Duration::from_millis(1)).await;
}

/// Emit a "user typed + assistant streamed partial text" pair, leaving the
/// turn intentionally orphaned (no `ResponseGenerated`). Mirrors the shape
/// the recovery sweep targets: activity events after the last start with
/// no terminal in between.
async fn emit_orphan_turn(bus: &EventBus, thread_id: Uuid) {
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::MessageReceived {
            text: "stuck request".into(),
            user_image_hashes: vec![],
            device_id: None,
            device: None,
            image_description: None,
            parent_thread_id: None,
            spawning_event_id: None,
            mode: ActorMode::Human,
            model: None,
            reasoning_effort: None,
            origin: None,
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();
    tick().await;
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::TextStreamed {
            text: "partial reply".into(),
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();
}

/// Emit a `MessageReceived` followed by an unmatched `ToolCalled`. The
/// orphan-tool-call sweep walks the (`ToolCalled`, `ToolResult`) pairs and
/// emits a synthetic `ToolResult` for any `ToolCalled` without a partner.
async fn emit_orphan_tool_call(bus: &EventBus, thread_id: Uuid) {
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::MessageReceived {
            text: "trigger a tool".into(),
            user_image_hashes: vec![],
            device_id: None,
            device: None,
            image_description: None,
            parent_thread_id: None,
            spawning_event_id: None,
            mode: ActorMode::Human,
            model: None,
            reasoning_effort: None,
            origin: None,
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();
    tick().await;
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ToolCalled {
            name: "Read".into(),
            args: json!({"path": "/tmp/x"}),
            description: String::new(),
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();
}

async fn archive(bus: &EventBus, thread_id: Uuid) {
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ThreadArchived,
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();
}

/// SQL row shape `ORPHAN_THREADS_SQL` returns. Mirrors the tuple production
/// uses (kept private to the test module because it's only meaningful here).
type OrphanThreadRow = (
    Uuid,
    Option<String>,
    Option<Uuid>,
    Option<String>,
    Option<String>,
);

#[tokio::test]
async fn orphan_threads_query_includes_unarchived_orphan() {
    // Precondition assertion that anchors the negative test below: the
    // exact same query DOES return a row for an orphan turn on a regular
    // inbox thread. Without this, the negative case ("archived → no row")
    // could be passing for the wrong reason (e.g. a typo in the orphan
    // fixture making no thread ever match).
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let thread_id = Uuid::new_v4();
    emit_orphan_turn(&bus, thread_id).await;

    let rows: Vec<OrphanThreadRow> = sqlx::query_as(ORPHAN_THREADS_SQL)
        .bind(Vec::<Uuid>::new())
        .fetch_all(&pool)
        .await
        .unwrap();
    let hit = rows.iter().find(|(tid, _, _, _, _)| *tid == thread_id);
    assert!(
        hit.is_some(),
        "unarchived orphan must be visible to the recovery sweep — got {:?}",
        rows.iter().map(|r| r.0).collect::<Vec<_>>()
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn orphan_threads_query_skips_archived_thread() {
    // The user's scenario: a thread had an in-flight turn that never
    // settled, the user archived the row, and a later engine restart's
    // recovery sweep flipped it back to inbox via the contract layer's
    // `ResponseAborted → to_inbox` rule. Fix: filter the sweep's
    // candidate set on `archive_state != 'archived'` so the recovery
    // event never gets emitted.
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let archived_id = Uuid::new_v4();
    emit_orphan_turn(&bus, archived_id).await;
    archive(&bus, archived_id).await;

    // Sanity: the projection records the archive on `archive_state` (sole
    // archive flag post-collapse).
    let archive_state: String =
        sqlx::query_scalar("SELECT archive_state FROM thread_summaries WHERE thread_id = $1")
            .bind(archived_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(archive_state, "archived");

    // Add a control row: an unarchived thread with the same orphan shape.
    // The query MUST return it but NOT the archived one.
    let control_id = Uuid::new_v4();
    emit_orphan_turn(&bus, control_id).await;

    let rows: Vec<OrphanThreadRow> = sqlx::query_as(ORPHAN_THREADS_SQL)
        .bind(Vec::<Uuid>::new())
        .fetch_all(&pool)
        .await
        .unwrap();
    let returned: Vec<Uuid> = rows.iter().map(|r| r.0).collect();
    assert!(
        returned.contains(&control_id),
        "control (unarchived) orphan must be returned — got {:?}",
        returned
    );
    assert!(
        !returned.contains(&archived_id),
        "archived orphan must NOT be returned — got {:?}",
        returned
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn orphan_tool_calls_query_skips_archived_thread() {
    // Mirror the orphan-thread test for the inner-tool-layer sweep: a
    // synthetic `ToolResult` for an archived thread would bump
    // `last_activity` (via the projection's ToolResult branch) and surface
    // the row in any activity-sorted list. Filter at the SQL JOIN.
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let archived_id = Uuid::new_v4();
    emit_orphan_tool_call(&bus, archived_id).await;
    archive(&bus, archived_id).await;

    let control_id = Uuid::new_v4();
    emit_orphan_tool_call(&bus, control_id).await;

    let rows: Vec<EventRow> = sqlx::query_as(ORPHAN_TOOL_CALLS_SQL)
        .fetch_all(&pool)
        .await
        .unwrap();
    let returned_threads: std::collections::HashSet<Uuid> =
        rows.iter().filter_map(|r| r.thread_id).collect();
    assert!(
        returned_threads.contains(&control_id),
        "control thread's ToolCalled must be returned — got {:?}",
        returned_threads
    );
    assert!(
        !returned_threads.contains(&archived_id),
        "archived thread's ToolCalled must NOT be returned — got {:?}",
        returned_threads
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}
