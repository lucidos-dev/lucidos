use super::super::*;
use super::*;

#[tokio::test]
async fn startup_orphan_reset_drift_recovered_by_rebuild() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

    let (parent_id, children) = spawn_cc_parent_with_n_running_cc_children(&bus, 3).await;
    assert_eq!(read_blocking_descendant_count(&pool, parent_id).await, 3);

    sqlx::query("UPDATE thread_summaries SET status = 'idle' WHERE status = 'running'")
        .execute(&pool)
        .await
        .unwrap();

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

#[tokio::test]
async fn engine_restart_continue_cycle_leaves_parent_blocking_count_at_zero() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

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
                reason: crate::engine::continuation_reason::USER_CLICKED_CONTINUE_REASON
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
                resumed_abort_id: None,
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

        emit_change_proposed(
            &bus,
            child_id,
            &format!("claude-code/test-{child_id}"),
            false,
        )
        .await;
        bus.emit(BusEvent::Thread {
            thread_id: child_id,
            event: ThreadEvent::ChangeApplied {
                content: Default::default(),
                change_id: test_change_id(child_id).to_string(),
                requires_restart: false,
                client_update: false,
                commits: vec![],
                thread_title: None,
                actor: None,
                pre_merge_sha: None,
                post_merge_sha: None,
                landing_merge_sha: None,
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
async fn a_required_user_actor_reaches_the_persisted_system_event() {
    use crate::core::DeviceStore;
    use crate::engine::thread_events::MessageOrigin;

    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

    DeviceStore::register(
        &pool,
        &bus,
        "test-device-emit",
        Some("Mozilla/5.0"),
        None,
        None,
    )
    .await
    .unwrap();
    DeviceStore::rename(&pool, &bus, "test-device-emit", Some("Test MacBook"), None)
        .await
        .unwrap();

    let headers =
        crate::api::gateway_stamp::test_support::stamped_device_headers("test-device-emit");

    let actor = crate::api::actor::require_user_actor(&headers, &pool, None)
        .await
        .expect("a registered device identifies itself");
    bus.emit_or_log(
        BusEvent::System(SystemEvent::DataFileWritten {
            path: "artifacts/fixture.txt".into(),
            commit: None,
            actor: Some(actor),
        }),
        "[Test] DataFileWritten",
    )
    .await;

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
        MessageOrigin::Device { device_id } => assert_eq!(device_id, "test-device-emit"),
        other => panic!("expected Device actor, got {:?}", other),
    }

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[test]
fn emit_pipeline_has_six_named_phases_in_order() {
    const SOURCE: &str = include_str!("../../../../lucidos-core/src/engine/event_bus/mod.rs");
    const EXPECTED: &[&str] = &[
        "Serialize",
        "Validate",
        "Persist",
        "Project",
        "CaptureAggregate",
        "PostCommit",
    ];
    const PREFIX: &str = "=== Phase: ";
    const SUFFIX: &str = " ===";

    let banners: Vec<&str> = SOURCE
        .lines()
        .filter_map(|line| {
            let (opener, banner) = line.trim_start().split_once(PREFIX)?;
            (opener.starts_with("// ") && !opener.contains('`'))
                .then(|| banner.strip_suffix(SUFFIX))
                .flatten()
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

#[tokio::test]
async fn subscriber_observes_event_with_committed_projection() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

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

    let committed_first_message: Option<String> =
        sqlx::query_scalar("SELECT first_message FROM thread_summaries WHERE thread_id = $1")
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
