use std::sync::Arc;
use uuid::Uuid;

use super::{ENGINE_RESTART_INTERRUPT_REASON, USER_CLICKED_CONTINUE_REASON};
use crate::engine::event_bus::{BusEvent, EventBus};
use crate::engine::thread_events::{EventChannel, EventMeta, ThreadEvent};
use crate::test_support::{setup_test_db, teardown_test_db};

/// The Phase 5.3 contract for the continue endpoint: emitting a
/// `ContinueSignal` on the CC channel persists with the user's reason tag,
/// and the spawn dispatcher's classifier (subscribed to the same bus) will
/// see it as a `SpawnTrigger::ContinueSignal`. This test exercises the
/// "endpoint emits → bus receives → dispatcher classifies" chain without
/// the full Axum router (which requires a complete engine).
#[tokio::test]
async fn continue_signal_emission_classifies_as_spawn_trigger() {
    use crate::engine::agent_session::spawn_dispatcher::{SpawnDispatcher, SpawnRequest};

    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let bus = Arc::new(bus);
    let (tx, mut spawn_rx) = tokio::sync::mpsc::unbounded_channel::<SpawnRequest>();
    let dispatcher = SpawnDispatcher::new(pool.clone(), bus.clone(), tx);
    let handle = tokio::spawn(async move { dispatcher.run().await });
    // Let the run loop subscribe before producing.
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let thread_id = Uuid::new_v4();
    // Seed SessionStarted so the lifecycle contract accepts CC events.
    let cc_meta = EventMeta {
        channel: Some(EventChannel::CodingAgent),
        ..EventMeta::NONE
    };
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::SessionStarted {
            session_id: "sid-cont".into(),
            branch: "claude-code/cont".into(),
            repo_id: None,
        },
        meta: cc_meta.clone(),
    })
    .await
    .expect("session start emits")
    .expect("event persisted");

    // What the continue endpoint does:
    let res = bus
        .emit(BusEvent::Thread {
            thread_id,
            event: ThreadEvent::ContinueSignal {
                reason: USER_CLICKED_CONTINUE_REASON.to_string(),
            },
            meta: cc_meta,
        })
        .await
        .expect("emit succeeds")
        .expect("event persisted");

    let received = tokio::time::timeout(std::time::Duration::from_secs(2), spawn_rx.recv())
        .await
        .expect("dispatcher must produce a SpawnRequest within 2s")
        .expect("channel must yield");
    assert_eq!(
        received,
        SpawnRequest::Continue {
            thread_id,
            event_id: res.event_id,
        },
        "ContinueSignal from the continue endpoint must produce SpawnRequest::Continue"
    );

    handle.abort();
    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Phase 5.3: when recovery emits a synthetic `CodingAgentIdled` with
/// `reason = engine_restart_interrupt`, the spawn dispatcher's classifier
/// must NOT treat it as a trigger. Without this guarantee, the very
/// "interrupted" event we emit to surface the continue affordance would
/// loop back into auto-spawning CC — exactly the behavior we removed.
#[tokio::test]
async fn synthetic_idled_with_engine_restart_interrupt_does_not_dispatch() {
    use crate::engine::agent_session::spawn_dispatcher::{SpawnDispatcher, SpawnRequest};
    use std::sync::atomic::Ordering;

    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let bus = Arc::new(bus);
    let (tx, _spawn_rx) = tokio::sync::mpsc::unbounded_channel::<SpawnRequest>();
    let dispatcher = SpawnDispatcher::new(pool.clone(), bus.clone(), tx);
    let dispatch_count = dispatcher.dispatch_count.clone();
    let handle = tokio::spawn(async move { dispatcher.run().await });
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let thread_id = Uuid::new_v4();
    let cc_meta = EventMeta {
        channel: Some(EventChannel::CodingAgent),
        ..EventMeta::NONE
    };
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::SessionStarted {
            session_id: "sid-int".into(),
            branch: "claude-code/int".into(),
            repo_id: None,
        },
        meta: cc_meta.clone(),
    })
    .await
    .expect("emit succeeds")
    .expect("persisted");

    // Simulate recovery surfacing the interrupt:
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::CodingAgentIdled {
            has_changes: true,
            is_external_repo: false,
            requires_restart: false,
            cc_session_id: Some("sid-int".into()),
            agent: crate::runtime::AgentKind::ClaudeCode,
            reason: Some(ENGINE_RESTART_INTERRUPT_REASON.to_string()),
            worktree_path: None,
            worktree_head_sha: None,
        },
        meta: cc_meta,
    })
    .await
    .expect("emit succeeds")
    .expect("persisted");

    // Give the dispatcher time to (NOT) act.
    tokio::time::sleep(std::time::Duration::from_millis(150)).await;
    assert_eq!(
            dispatch_count.load(Ordering::SeqCst),
            0,
            "synthetic CodingAgentIdled must not produce any dispatch — only the user's continue click does"
        );

    handle.abort();
    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Regression: the spawn consumer's Continue path must hand CC a non-empty
/// `user_message`. `claude --print --resume` parks indefinitely on stdin
/// when no input is sent (verified empirically against claude 2.1.123),
/// the engine's `events_rx` never resolves, and the thread sits "Running"
/// forever — the second-stage zombie observed on thread `ca025588-...`.
///
/// Two assertions:
///   1. The constant itself stays non-empty (and non-whitespace).
///   2. The consumer in `engine/mod.rs` actually references the constant —
///      catches a regression where a future edit reverts to a literal `""`
///      while the constant stays defined elsewhere.
#[test]
fn spawn_consumer_continue_must_send_non_empty_user_message() {
    use super::CONTINUE_RESUME_USER_MESSAGE;

    assert!(
            !CONTINUE_RESUME_USER_MESSAGE.trim().is_empty(),
            "CONTINUE_RESUME_USER_MESSAGE is empty — CC --print --resume would hang on stdin and zombie the thread"
        );

    let consumer_src = include_str!("engine_impl.rs");
    assert!(
        consumer_src.contains("CONTINUE_RESUME_USER_MESSAGE"),
        "engine/engine_impl.rs no longer references CONTINUE_RESUME_USER_MESSAGE — \
             the SpawnConsumer's Continue path may have reverted to passing a \
             literal user_message and risks the empty-stdin zombie regression"
    );
}
