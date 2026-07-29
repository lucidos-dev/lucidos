use std::sync::Arc;
use uuid::Uuid;

use super::{ENGINE_RESTART_INTERRUPT_REASON, USER_CLICKED_CONTINUE_REASON};
use crate::engine::event_bus::{BusEvent, EventBus};
use crate::engine::thread_events::{EventChannel, EventMeta, ThreadEvent};
use crate::test_support::{setup_test_db, teardown_test_db};

/// The Phase 5.3 contract for the continue endpoint: emitting a
/// `ContinuationRequested` on the CC channel persists with the user's reason tag,
/// and the spawn dispatcher's classifier (subscribed to the same bus) will
/// see it as a `SpawnTrigger::ContinuationRequested`. This test exercises the
/// "endpoint emits → bus receives → dispatcher classifies" chain without
/// the full Axum router (which requires a complete engine).
#[tokio::test]
async fn continuation_requested_emission_classifies_as_spawn_trigger() {
    use crate::engine::agent_session::spawn_dispatcher::{SpawnDispatcher, SpawnRequest};

    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let bus = Arc::new(bus);
    let (tx, mut spawn_rx) = tokio::sync::mpsc::unbounded_channel::<SpawnRequest>();
    let dispatcher = SpawnDispatcher::new(pool.clone(), tx);
    // Subscribe before starting the loop — mirrors `SpawnDispatcher::spawn()`,
    // so no settling sleep is needed before producing.
    let rx = bus.subscribe();
    let handle = tokio::spawn(async move { dispatcher.run(rx).await });

    let thread_id = Uuid::new_v4();
    // Seed SessionStarted so the lifecycle contract accepts CC events.
    let cc_meta = EventMeta {
        channel: Some(EventChannel::ClaudeCode),
        ..EventMeta::NONE
    };
    crate::test_support::start_cc_session(&bus, thread_id, "claude-code/cont", None).await;

    // What the continue endpoint does:
    let res = bus
        .emit(BusEvent::Thread {
            thread_id,
            event: ThreadEvent::ContinuationRequested {
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
        "ContinuationRequested from the continue endpoint must produce SpawnRequest::Continue"
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
    let dispatcher = SpawnDispatcher::new(pool.clone(), tx);
    let dispatch_count = dispatcher.dispatch_count.clone();
    let rx = bus.subscribe();
    let handle = tokio::spawn(async move { dispatcher.run(rx).await });

    let thread_id = Uuid::new_v4();
    let cc_meta = EventMeta {
        channel: Some(EventChannel::ClaudeCode),
        ..EventMeta::NONE
    };
    crate::test_support::start_cc_session(&bus, thread_id, "claude-code/int", None).await;

    // Simulate recovery surfacing the interrupt:
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::CodingAgentIdled {
            has_changes: true,
            is_external_repo: false,
            requires_restart: false,
            cc_session_id: Some("sid-int".into()),
            coding_agent: crate::runtime::CodingAgent::ClaudeCode,
            reason: Some(ENGINE_RESTART_INTERRUPT_REASON.to_string()),
            worktree_path: None,
            worktree_head_sha: None,
            bg_bash_pending: false,
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

/// The stale-resume retry of a continuation runs with NO resume sid — the one
/// we had is dead — so the fresh subprocess starts with zero context. Its input
/// must therefore carry the reconstructed conversation ahead of
/// `CONTINUE_RESUME_USER_MESSAGE`, or the agent is told to "continue from where
/// you left off" with no idea where that was. Same contract the chat path's
/// retry has always had (`chat::process_cc`).
#[tokio::test]
async fn continuation_retry_input_recaps_the_thread_before_the_continue_message() {
    use crate::engine::agent_recovery::{continue_retry_input, CONTINUE_RESUME_USER_MESSAGE};

    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    let cc_meta = EventMeta {
        channel: Some(EventChannel::ClaudeCode),
        ..EventMeta::NONE
    };
    crate::test_support::start_cc_session(&bus, thread_id, "claude-code/retry", None).await;

    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::CodingAgentUserMessageSent {
            text: "fix the flaky drafts spec".into(),
            coding_agent: crate::runtime::CodingAgent::ClaudeCode,
        },
        meta: cc_meta.clone(),
    })
    .await
    .expect("emit succeeds")
    .expect("persisted");
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::CodingAgentTextStreamed {
            text: "Traced it to the debounced compose PUT.".into(),
            coding_agent: crate::runtime::CodingAgent::ClaudeCode,
        },
        meta: cc_meta,
    })
    .await
    .expect("emit succeeds")
    .expect("persisted");

    let input = continue_retry_input(&pool, thread_id).await;

    let recap_pos = input
        .find("fix the flaky drafts spec")
        .expect("prior user turn must be recapped into the retry input");
    let continue_pos = input
        .find(CONTINUE_RESUME_USER_MESSAGE)
        .expect("retry input must still carry the continue message");
    assert!(
        recap_pos < continue_pos,
        "the recap must come FIRST so the agent reads what happened before being told to continue"
    );
    assert!(
        input.contains("Traced it to the debounced compose PUT."),
        "prior assistant turn missing from the retry input"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// A thread with no reconstructable history still yields a usable retry input:
/// the bare continue message, never an empty string. An empty stdin parks
/// `claude --print --resume` forever waiting for input — the exact hang
/// `CONTINUE_RESUME_USER_MESSAGE` exists to prevent.
#[tokio::test]
async fn continuation_retry_input_is_never_empty() {
    use crate::engine::agent_recovery::{continue_retry_input, CONTINUE_RESUME_USER_MESSAGE};

    let (pool, db_name) = setup_test_db().await;
    let thread_id = Uuid::new_v4();

    let input = continue_retry_input(&pool, thread_id).await;

    assert_eq!(
        input, CONTINUE_RESUME_USER_MESSAGE,
        "with nothing to recap the retry input must fall back to the bare continue message"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}
