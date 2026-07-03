use super::super::*;
use super::*;

/// Regression for the cascading-archive bug on thread
/// `0f547346-c63f-43af-b73a-6f36b12ca859`: parent's Archive button stuck
/// disabled because `blocking_descendant_count = 3` while every descendant
/// is idle. Root cause + fix: see `crates/lucidos-engine/src/main.rs`
/// (rebuild_blocking_descendant_count at startup).
#[tokio::test]
async fn startup_orphan_reset_drift_recovered_by_rebuild() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

    let (parent_id, children) = spawn_cc_parent_with_n_running_cc_children(&bus, 3).await;
    assert_eq!(read_blocking_descendant_count(&pool, parent_id).await, 3);

    // Direct UPDATE that mirrors the orphan-running reset in `main.rs` —
    // bypasses the projection's sampling wrapper, so no decrement reaches
    // the parent's `blocking_descendant_count`.
    sqlx::query("UPDATE thread_summaries SET status = 'idle' WHERE status = 'running'")
        .execute(&pool)
        .await
        .unwrap();

    // The recovery sweep that follows still emits through EventBus, but
    // `prev_sample` already sees status='idle' (the UPDATE above landed
    // first), so delta=0 and the count stays drifted at 3.
    for &child_id in &children {
        emit_cc_recovery_pair(&bus, child_id).await;
    }
    assert_eq!(
        read_blocking_descendant_count(&pool, parent_id).await,
        3,
        "drift reproduced: count stays at 3 after direct UPDATE bypasses the projection"
    );

    EventBus::rebuild_blocking_descendant_count(&pool)
        .await
        .unwrap();

    assert_eq!(
        read_blocking_descendant_count(&pool, parent_id).await,
        0,
        "rebuild must heal the drift left by the direct UPDATE"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Companion to `startup_orphan_reset_drift_recovered_by_rebuild`: drive the
/// full restart→continue→respond→propose→apply lifecycle entirely through
/// EventBus (no direct SQL), and assert the projection alone holds the count
/// at 0. Failure here means the bug is in the projection itself, not the
/// startup bypass — which would require a different fix than the rebuild.
#[tokio::test]
async fn engine_restart_continue_cycle_leaves_parent_blocking_count_at_zero() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

    // Parent itself is a CC thread — production trace has `is_coding_agent=t`
    // on the parent because the user typed into CC mode before sub-tasks
    // spawned. Idle it before children spawn so its own MessageReceived
    // doesn't leave it Running.
    let parent_id = Uuid::new_v4();
    emit_cc_message_received(&bus, parent_id, None, "parent").await;
    emit_cc_session_started(&bus, parent_id).await;
    emit_cc_idle(&bus, parent_id, false, None).await;

    let children: Vec<Uuid> = (0..3).map(|_| Uuid::new_v4()).collect();

    for &child_id in &children {
        emit_cc_message_received(&bus, child_id, Some(parent_id), "sub task").await;
        emit_cc_session_started(&bus, child_id).await;

        emit_cc_recovery_pair(&bus, child_id).await;

        bus.emit(BusEvent::Thread {
            thread_id: child_id,
            event: ThreadEvent::ContinuationRequested {
                reason: crate::engine::agent_recovery::USER_CLICKED_CONTINUE_REASON
                    .to_string(),
            },
            meta: EventMeta {
                channel: Some(EventChannel::ClaudeCode),
                ..EventMeta::NONE
            },
        })
        .await
        .unwrap();
        bus.emit(BusEvent::Thread {
            thread_id: child_id,
            event: ThreadEvent::ContinuationStarted {
                branch: "claude-code/test".into(),
                origin: None,
                reason: None,
            },
            meta: EventMeta {
                channel: Some(EventChannel::ClaudeCode),
                ..EventMeta::NONE
            },
        })
        .await
        .unwrap();
        emit_cc_session_started(&bus, child_id).await;

        bus.emit(BusEvent::Thread {
            thread_id: child_id,
            event: ThreadEvent::ResponseGenerated {
                text: "ok".into(),
                images: vec![],
                model: None,
                reasoning_effort: None,
            },
            meta: EventMeta::NONE,
        })
        .await
        .unwrap();
        emit_cc_idle(&bus, child_id, false, None).await;

        emit_change_proposed(&bus, child_id, "claude-code/test", false).await;
        bus.emit(BusEvent::Thread {
            thread_id: child_id,
            event: ThreadEvent::ChangeApplied {
                change_id: format!("change-{child_id}"),
                requires_restart: false,
                client_update: false,
                commits: vec![],
                thread_title: None,
                actor: None,
                pre_merge_sha: None,
                post_merge_sha: None,
                path: String::new(),
            },
            meta: EventMeta::NONE,
        })
        .await
        .unwrap();
    }

    assert_eq!(
        read_blocking_descendant_count(&pool, parent_id).await,
        0,
        "projection alone must keep parent's count at 0 — no direct SQL ran here"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn emit_user_system_resolves_actor_and_emits_system_event() {
    use crate::api::actor::HEADER_DEVICE_ID;
    use crate::core::DeviceStore;
    use crate::engine::thread_events::MessageOrigin;
    use axum::http::{HeaderMap, HeaderName, HeaderValue};

    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

    DeviceStore::register(&pool, "test-device-emit", Some("Mozilla/5.0"))
        .await
        .unwrap();
    DeviceStore::rename(&pool, "test-device-emit", Some("Test MacBook"))
        .await
        .unwrap();

    let mut headers = HeaderMap::new();
    headers.insert(
        HeaderName::from_static(HEADER_DEVICE_ID),
        HeaderValue::from_static("test-device-emit"),
    );

    // `DataFileWritten` (rather than the spec's `AppDeleted`) so the
    // assertion below can read the row back from the `events` table with the
    // actor attached: App* variants are transient and never persisted, and
    // Trigger* variants persist but their custom `to_payload` drops the
    // actor. `DataFileWritten` uses the default `serde_json::to_value(self)`
    // branch, which serializes the whole `#[serde(tag = "type", content =
    // "data")]` shape — so `payload` is `{"type": "DataFileWritten",
    // "data": {"path": …, "actor": {…}}}` and the actor round-trips.
    bus.emit_user_system(&headers, &pool, "[Test] DataFileWritten", |actor| {
        SystemEvent::DataFileWritten {
            path: "artifacts/fixture.txt".into(),
            commit: None,
            actor,
        }
    })
    .await;

    // Find the emitted event in the events table.
    let row: (serde_json::Value,) = sqlx::query_as(
        "SELECT payload FROM events WHERE event_type = 'DataFileWritten' \
         AND payload->'data'->>'path' = 'artifacts/fixture.txt' \
         ORDER BY created DESC LIMIT 1",
    )
    .fetch_one(&pool)
    .await
    .expect("emitted DataFileWritten row");

    let actor: MessageOrigin = serde_json::from_value(
        row.0
            .get("data")
            .and_then(|d| d.get("actor"))
            .expect("actor present")
            .clone(),
    )
    .expect("actor deserializes");
    match actor {
        MessageOrigin::Device { device_id, label } => {
            assert_eq!(device_id, "test-device-emit");
            assert_eq!(label, "Test MacBook");
        }
        other => panic!("expected Device actor from db lookup, got {:?}", other),
    }

    pool.close().await;
    teardown_test_db(&db_name).await;
}

// ---------------------------------------------------------------------------
// Phase contract tests — codify the EventBus::emit pipeline.
//
// `EventBus::emit` is the single emission point for every domain event, but the
// ordering of its internal steps used to be folkloric: validate-reads-projection,
// project-cascades-into-parent, capture-aggregate-must-stay-in-tx, broadcast-
// fires-only-after-commit. The two tests below pin both halves of the contract:
//
//   1. `emit_pipeline_has_five_named_phases_in_order` — a source-level check
//      that the five `// === Phase: <Name> ===` banners exist inside
//      `event_bus.rs` in the documented order. Catches the case where someone
//      adds a chunk of code outside a declared phase, or reorders the phases.
//
//   2. `subscriber_observes_event_with_committed_projection` — a behavioural
//      check that a `bus.subscribe()` consumer sees the `EmittedEvent` after
//      the projection row has been committed, and that the carried aggregate
//      snapshot reflects the post-projection state. Catches the case where
//      someone moves the broadcast send before `tx.commit()`, or captures the
//      aggregate from a pre-projection read.
// ---------------------------------------------------------------------------
/// Parse `event_bus/mod.rs` at compile time and assert the five phase banners
/// inside `EventBus::emit` appear in the documented order. Drift triggers
/// here before the behavioural test or production code can mask it.
///
/// Adding a new phase: update both the source banners and `EXPECTED` here,
/// and document the phase in the `EventBus` struct doc-comment.
#[test]
fn emit_pipeline_has_five_named_phases_in_order() {
    const SOURCE: &str = include_str!("../event_bus/mod.rs");
    const EXPECTED: &[&str] = &[
        "Validate",
        "Persist",
        "Project",
        "CaptureAggregate",
        "PostCommit",
    ];
    const PREFIX: &str = "// === Phase: ";
    const SUFFIX: &str = " ===";

    let banners: Vec<&str> = SOURCE
        .lines()
        .filter_map(|line| {
            let trimmed = line.trim_start();
            trimmed
                .strip_prefix(PREFIX)
                .and_then(|s| s.strip_suffix(SUFFIX))
        })
        .collect();

    assert_eq!(
        banners.as_slice(),
        EXPECTED,
        "EventBus::emit phase banners drifted from the documented contract. \
         If you added a new phase, update both the source banners and the \
         EXPECTED list here, and document the phase in the EventBus struct \
         doc-comment. If you reordered code, restore the order. See the \
         EventBus struct header in event_bus/mod.rs for the full contract.",
    );
}

/// Behavioural smoke test for the post-commit + read-your-write contract.
///
/// A subscriber receives an `EmittedEvent` after `tx.commit()`, and the
/// carried `aggregate` snapshot reflects the just-committed projection state.
/// Cross-checked against a direct DB query so a pre-commit broadcast (where
/// the subscriber sees the event but no other connection sees the row) would
/// fail loudly.
#[tokio::test]
async fn subscriber_observes_event_with_committed_projection() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

    // Subscribe before emit so we observe the very first broadcast.
    let mut rx = bus.subscribe();

    let thread_id = Uuid::new_v4();
    let message_text = "subscriber smoke test message";
    emit_thread_message(&bus, thread_id, None, message_text).await;

    let aggregate = drain_aggregate_broadcasts(&mut rx)
        .into_iter()
        .find_map(|(tid, etype, agg)| {
            if tid == thread_id && etype == "MessageReceived" {
                agg
            } else {
                None
            }
        })
        .expect(
            "subscriber must receive the MessageReceived broadcast with an aggregate snapshot \
             — if the aggregate is None, CaptureAggregate either failed or wasn't run",
        );

    // The aggregate must reflect the just-emitted event. `title` falls back to
    // `first_message` when no ThreadTitleGenerated has fired; we verify both
    // the message count and the title fallback to prove the projection ran
    // before the aggregate was captured.
    assert_eq!(
        aggregate.message_count, 1,
        "EmittedEvent.aggregate must carry the post-projection message_count; \
         got {} (expected 1 after the first MessageReceived)",
        aggregate.message_count,
    );
    assert_eq!(
        aggregate.title.as_str(),
        message_text,
        "EmittedEvent.aggregate.title must reflect the post-projection first_message; \
         got {:?} (expected {:?})",
        aggregate.title,
        message_text,
    );

    // Cross-check: the projection row must be queryable from a separate
    // connection via the pool. If the broadcast had fired pre-commit, the
    // subscriber could observe the event before any other connection saw the
    // row — this query would return None and fail the test.
    let committed_first_message: Option<String> = sqlx::query_scalar(
        "SELECT first_message FROM thread_summaries WHERE thread_id = $1",
    )
    .bind(thread_id)
    .fetch_optional(&pool)
    .await
    .unwrap()
    .flatten();

    assert_eq!(
        committed_first_message.as_deref(),
        Some(message_text),
        "subscriber observed the event but the projection row is missing from a \
         fresh pool connection — broadcast must fire only after `tx.commit()`",
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}
