use super::*;
use crate::test_support::{seed_chat_thread, setup_test_db, teardown_test_db};

fn pair(model: &str, effort: &str) -> ResolvedModelSelection {
    ResolvedModelSelection {
        model: Some(model.to_string()),
        reasoning_effort: Some(effort.to_string()),
        provider: None,
    }
}

async fn set_status(pool: &sqlx::PgPool, thread_id: Uuid, status: &str) {
    sqlx::query("UPDATE thread_summaries SET status = $2 WHERE thread_id = $1")
        .bind(thread_id)
        .bind(status)
        .execute(pool)
        .await
        .unwrap();
}

async fn picks_on(pool: &sqlx::PgPool, thread_id: Uuid) -> Vec<serde_json::Value> {
    sqlx::query_scalar(
        "SELECT payload FROM events \
         WHERE thread_id = $1 AND event_type = 'ChatSettingsChanged' ORDER BY sequence",
    )
    .bind(thread_id)
    .fetch_all(pool)
    .await
    .unwrap()
}

async fn pick(bus: &EventBus, gate: &TurnStartGate, thread_id: Uuid) -> ChatSettingsOutcome {
    record_pick(
        bus,
        gate,
        thread_id,
        std::future::ready(pair("model-b", "high")),
        "model-b".to_string(),
        None,
    )
    .await
    .unwrap()
}

#[tokio::test]
async fn an_idle_thread_records_the_resolved_pick() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let gate = TurnStartGate::default();
    let thread = seed_chat_thread(&bus, "hello").await;
    set_status(&pool, thread, "idle").await;
    assert_eq!(
        pick(&bus, &gate, thread).await,
        ChatSettingsOutcome::Recorded
    );
    let picks = picks_on(&pool, thread).await;
    assert_eq!(picks.len(), 1);
    assert_eq!(picks[0]["model"], "model-b");
    assert_eq!(picks[0]["reasoning_effort"], "high");
    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn every_open_turn_status_records_the_pick() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let gate = TurnStartGate::default();
    let thread = seed_chat_thread(&bus, "hello").await;
    for status in ["running", "waiting_for_user_answer", "paused"] {
        set_status(&pool, thread, status).await;
        assert_eq!(
            pick(&bus, &gate, thread).await,
            ChatSettingsOutcome::Recorded,
            "{status}"
        );
    }
    assert_eq!(picks_on(&pool, thread).await.len(), 3);
    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn a_pick_waits_only_until_the_starting_turn_releases_its_gate() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let gate = TurnStartGate::default();
    let thread = seed_chat_thread(&bus, "hello").await;
    let turn = gate.hold(thread).await;
    let picking = pick(&bus, &gate, thread);
    tokio::pin!(picking);
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(20), &mut picking)
            .await
            .is_err()
    );
    drop(turn);
    assert_eq!(picking.await, ChatSettingsOutcome::Recorded);
    assert_eq!(picks_on(&pool, thread).await.len(), 1);
    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Two quick picks on an idle thread both land. Neither reads the other
/// as a starting turn.
#[tokio::test]
async fn concurrent_picks_on_an_idle_thread_are_both_recorded() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let gate = TurnStartGate::default();
    let thread = seed_chat_thread(&bus, "hello").await;
    set_status(&pool, thread, "idle").await;
    let (first, second) = tokio::join!(pick(&bus, &gate, thread), pick(&bus, &gate, thread));
    assert_eq!(first, ChatSettingsOutcome::Recorded);
    assert_eq!(second, ChatSettingsOutcome::Recorded);
    assert_eq!(picks_on(&pool, thread).await.len(), 2);
    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// A turn that waits on the gate while a pick is recorded resolves after
/// it, so it reads the pick.
#[tokio::test]
async fn a_turn_waiting_on_the_gate_starts_after_the_pick() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let gate = std::sync::Arc::new(TurnStartGate::default());
    let thread = seed_chat_thread(&bus, "hello").await;
    set_status(&pool, thread, "idle").await;
    let (resolving_tx, resolving_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel::<()>();
    let resolve = async move {
        resolving_tx.send(()).unwrap();
        release_rx.await.unwrap();
        pair("model-b", "high")
    };
    let picking = {
        let (bus, gate) = (bus.clone(), gate.clone());
        tokio::spawn(async move {
            record_pick(&bus, &gate, thread, resolve, "model-b".into(), None).await
        })
    };
    resolving_rx.await.unwrap();
    let turn = {
        let (pool, gate) = (pool.clone(), gate.clone());
        tokio::spawn(async move {
            let _start = gate.hold(thread).await;
            crate::core::PreferenceStore::last_thread_chat_settings(&pool, thread, None, None).await
        })
    };
    release_tx.send(()).unwrap();
    assert_eq!(
        picking.await.unwrap().unwrap(),
        ChatSettingsOutcome::Recorded
    );
    assert_eq!(turn.await.unwrap().model.as_deref(), Some("model-b"));
    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[test]
fn queued_turns_read_pending_picks_and_keep_explicit_overrides() {
    crate::test_support::run_on_engine_runtime(
        "engine::chat::rung::tests::queued_turns_read_pending_picks_and_keep_explicit_overrides",
        queued_turn_case,
    );
}

async fn queued_turn_case() {
    use crate::engine::{thread_events::ActorMode, FollowUpUrgency};
    use std::time::Duration;
    let (_pool, db) = setup_test_db().await;
    let (engine, _workspace) = crate::test_support::boot_mock_engine(&db).await;
    for explicit in [false, true] {
        let thread = seed_chat_thread(&engine.event_bus, "hello").await;
        let (_, _rx, guard) = engine.try_register_thread(thread).unwrap();
        engine
            .event_bus
            .emit(BusEvent::Thread {
                thread_id: thread,
                event: ThreadEvent::ChatTurnSettingsResolved {
                    model: "mock".into(),
                    reasoning_effort: Some("low".into()),
                    provider: None,
                },
                meta: EventMeta {
                    request_event_id: Some(Uuid::new_v4()),
                    ..EventMeta::NONE
                },
            })
            .await
            .unwrap();
        let engine_task = engine.clone();
        let queued = tokio::spawn(async move {
            engine_task
                .process_message_with_steps(
                    "queued regression",
                    explicit.then_some("mock"),
                    None,
                    None,
                    explicit.then_some("high"),
                    None,
                    None,
                    None,
                    Some(false),
                    None,
                    Some(thread),
                    None,
                    None,
                    None,
                    None,
                    None,
                    None,
                    ActorMode::Human,
                    None,
                    None,
                    None,
                    Some("Rung regression"),
                    None,
                    None,
                    if explicit {
                        FollowUpUrgency::Normal
                    } else {
                        FollowUpUrgency::Urgent
                    },
                    None,
                )
                .await
        });
        // Poll the real process through admission, while its live holder remains.
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(!queued.is_finished());
        for effort in ["high", "medium"] {
            tokio::time::timeout(
                Duration::from_secs(2),
                engine.record_chat_settings(thread, pair("mock", effort), None),
            )
            .await
            .expect("a queued turn must release the start gate")
            .unwrap();
        }
        assert_eq!(
            engine
                .live_chat_pair(thread)
                .await
                .unwrap()
                .reasoning_effort
                .as_deref(),
            Some("low")
        );
        drop(guard);
        tokio::time::timeout(Duration::from_secs(20), queued)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let applied = engine.live_chat_pair(thread).await.unwrap();
        assert_eq!(
            picks_on(engine.pool(), thread).await.len(),
            2,
            "applied settings must not masquerade as user picks"
        );
        assert_eq!(
            applied.reasoning_effort.as_deref(),
            Some(if explicit { "high" } else { "medium" })
        );
    }
    engine.pool().close().await;
    teardown_test_db(&db).await;
}

#[test]
fn an_applied_stamp_failure_settles_the_exchange() {
    crate::test_support::run_on_engine_runtime(
        "engine::chat::rung::tests::an_applied_stamp_failure_settles_the_exchange",
        applied_stamp_failure_case,
    );
}

async fn applied_stamp_failure_case() {
    let (_pool, db) = setup_test_db().await;
    let (engine, _workspace) = crate::test_support::boot_mock_engine(&db).await;
    let thread = seed_chat_thread(&engine.event_bus, "hello").await;
    crate::test_support::refuse_event_type(engine.pool(), "ChatTurnSettingsResolved").await;
    let result = engine
        .process_message_with_steps(
            "stamp failure",
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            Some(false),
            None,
            Some(thread),
            None,
            None,
            None,
            None,
            None,
            None,
            crate::engine::thread_events::ActorMode::Human,
            None,
            None,
            None,
            Some("Rung regression"),
            None,
            None,
            crate::engine::FollowUpUrgency::Normal,
            None,
        )
        .await;
    assert!(result.is_err());
    let failures: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM events WHERE thread_id = $1 AND event_type = 'ResponseFailed'",
    )
    .bind(thread)
    .fetch_one(engine.pool())
    .await
    .unwrap();
    assert_eq!(failures, 1);
    assert!(!engine.processing_thread_ids().contains(&thread));
    engine.pool().close().await;
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn a_running_turn_overrides_a_pending_pick_and_an_idle_thread_does_not() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let gate = TurnStartGate::default();
    let thread = seed_chat_thread(&bus, "hello").await;
    bus.emit(BusEvent::Thread {
        thread_id: thread,
        event: ThreadEvent::ChatTurnSettingsResolved {
            model: "model-a".into(),
            reasoning_effort: Some("low".into()),
            provider: None,
        },
        meta: EventMeta {
            request_event_id: Some(Uuid::new_v4()),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();
    pick(&bus, &gate, thread).await;
    set_status(&pool, thread, "running").await;
    assert_eq!(
        running_turn_overrides(&pool, &gate, thread).await.unwrap(),
        pair("model-a", "low")
    );
    set_status(&pool, thread, "idle").await;
    assert_eq!(
        running_turn_overrides(&pool, &gate, thread).await.unwrap(),
        ResolvedModelSelection::default()
    );
    pool.close().await;
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn an_injected_live_stamp_cannot_consume_a_pending_pick_or_its_snapshot() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread = seed_chat_thread(&bus, "hello").await;
    let anchor = Uuid::new_v4();
    bus.emit(BusEvent::Thread {
        thread_id: thread,
        event: ThreadEvent::ChatTurnSettingsResolved {
            model: "model-a".into(),
            reasoning_effort: Some("low".into()),
            provider: None,
        },
        meta: EventMeta {
            request_event_id: Some(anchor),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();
    pick(&bus, &TurnStartGate::default(), thread).await;
    let message = crate::engine::chat::make_message_received(
        std::path::Path::new("."),
        "injected",
        None,
        None,
        None,
        None,
        None,
        crate::engine::thread_events::ActorMode::Human,
        Some("model-a"),
        Some("low"),
        None,
        None,
        None,
    );
    bus.emit(BusEvent::Thread {
        thread_id: thread,
        event: message,
        meta: EventMeta {
            request_event_id: Some(anchor),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();
    assert_eq!(
        live_pair(&pool, thread).await.unwrap(),
        pair("model-a", "low")
    );
    assert_eq!(
        crate::core::PreferenceStore::last_thread_chat_settings(&pool, thread, None, None).await,
        pair("model-b", "high")
    );
    let stamps = crate::core::PreferenceStore::thread_pair_stamps(&pool, thread)
        .await
        .unwrap();
    assert!(stamps
        .iter()
        .any(|s| s.request_event_id.is_some() && s.model == "model-a"));
    assert!(stamps
        .iter()
        .any(|s| s.request_event_id.is_none() && s.model == "model-b"));
    assert!(!stamps
        .iter()
        .any(|s| s.event_type == "MessageReceived" && s.request_event_id.is_some()));
    pool.close().await;
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn a_pick_cannot_take_the_gate_while_a_turn_starts() {
    let gate = TurnStartGate::default();
    let thread = Uuid::new_v4();
    let turn = gate.hold(thread).await;
    assert!(gate.try_hold(thread).is_none());
    assert!(gate.try_hold(Uuid::new_v4()).is_some());
    drop(turn);
    assert!(gate.try_hold(thread).is_some());
}

#[tokio::test]
async fn a_freed_gate_forgets_its_thread() {
    let gate = TurnStartGate::default();
    let thread = Uuid::new_v4();
    drop(gate.hold(thread).await);
    assert!(gate.try_hold(thread).is_some());
    assert_eq!(gate.tracked(), 0);
}
