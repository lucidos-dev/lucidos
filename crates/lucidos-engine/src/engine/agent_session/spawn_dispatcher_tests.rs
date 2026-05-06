use std::sync::Arc;
use std::time::Duration;

use uuid::Uuid;

use crate::engine::agent_recovery::ENGINE_RESTART_INTERRUPT_REASON;
use crate::engine::event_bus::{BusEvent, EventBus};
use crate::engine::thread_events::{
    ActorMode, EventChannel, EventMeta, SessionEndReason, ThreadEvent,
};
use crate::test_support::{setup_test_db, teardown_test_db};

use super::{SpawnDispatcher, SpawnRequest, SpawnTrigger};

fn cc_meta() -> EventMeta {
    EventMeta {
        channel: Some(EventChannel::CodingAgent),
        ..EventMeta::NONE
    }
}

fn chat_meta() -> EventMeta {
    EventMeta {
        channel: Some(EventChannel::Chat),
        ..EventMeta::NONE
    }
}

fn user_message(text: &str) -> ThreadEvent {
    ThreadEvent::MessageReceived {
        text: text.into(),
        images: vec![],
        device_id: None,
        device: None,
        image_description: None,
        parent_thread_id: None,
        spawning_event_id: None,
        mode: ActorMode::Human,
        model: None,
        reasoning_effort: None,
        origin: None,
    }
}

async fn emit_cc_thread(bus: &EventBus, thread_id: Uuid, event: ThreadEvent) -> Uuid {
    let res = bus
        .emit(BusEvent::Thread {
            thread_id,
            event,
            meta: cc_meta(),
        })
        .await
        .expect("emit succeeds")
        .expect("event persisted");
    res.event_id
}

async fn emit_chat_thread(bus: &EventBus, thread_id: Uuid, event: ThreadEvent) -> Uuid {
    let res = bus
        .emit(BusEvent::Thread {
            thread_id,
            event,
            meta: chat_meta(),
        })
        .await
        .expect("emit succeeds")
        .expect("event persisted");
    res.event_id
}

/// Build a dispatcher with a fresh outbound channel. Returns the dispatcher
/// and the receiver so tests can assert what (if anything) was sent.
fn make_dispatcher(
    pool: sqlx::PgPool,
    bus: Arc<EventBus>,
) -> (
    SpawnDispatcher,
    tokio::sync::mpsc::UnboundedReceiver<SpawnRequest>,
) {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    (SpawnDispatcher::new(pool, bus, tx), rx)
}

/// Wait until `predicate` becomes true or 2 seconds elapse. Used so tests
/// don't block forever if the dispatcher never fires.
async fn wait_until<F>(mut predicate: F)
where
    F: FnMut() -> bool,
{
    let start = std::time::Instant::now();
    while !predicate() {
        if start.elapsed() > Duration::from_secs(2) {
            panic!("predicate did not become true within 2s");
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

#[tokio::test]
async fn classify_trigger_recognizes_cc_user_message() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let bus = Arc::new(bus);
    let (dispatcher, _spawn_rx) = make_dispatcher(pool.clone(), bus.clone());

    let thread_id = Uuid::new_v4();
    let event = user_message("hi");
    let bus_event = BusEvent::Thread {
        thread_id,
        event,
        meta: cc_meta(),
    };
    let trigger = dispatcher
        .classify_trigger(&bus_event, Uuid::new_v4())
        .expect("CC user message must classify as a trigger");
    match trigger {
        SpawnTrigger::UserMessage {
            thread_id: tid,
            text,
            ..
        } => {
            assert_eq!(tid, thread_id);
            assert_eq!(text, "hi");
        }
        other => panic!("expected UserMessage, got {:?}", other),
    }

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn classify_trigger_recognizes_continue_signal() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let bus = Arc::new(bus);
    let (dispatcher, _spawn_rx) = make_dispatcher(pool.clone(), bus.clone());

    let thread_id = Uuid::new_v4();
    let event_id = Uuid::new_v4();
    let bus_event = BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ContinueSignal {
            reason: ENGINE_RESTART_INTERRUPT_REASON.to_string(),
        },
        meta: cc_meta(),
    };
    let trigger = dispatcher
        .classify_trigger(&bus_event, event_id)
        .expect("ContinueSignal must classify as a trigger");
    match trigger {
        SpawnTrigger::ContinueSignal {
            thread_id: tid,
            event_id: eid,
        } => {
            assert_eq!(tid, thread_id);
            assert_eq!(eid, event_id);
        }
        other => panic!("expected ContinueSignal, got {:?}", other),
    }

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn classify_trigger_skips_chat_channel_messages() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let bus = Arc::new(bus);
    let (dispatcher, _spawn_rx) = make_dispatcher(pool.clone(), bus.clone());

    let thread_id = Uuid::new_v4();
    let bus_event = BusEvent::Thread {
        thread_id,
        event: user_message("chat msg"),
        meta: chat_meta(),
    };
    assert!(
        dispatcher
            .classify_trigger(&bus_event, Uuid::new_v4())
            .is_none(),
        "chat-channel messages must NOT trigger a CC spawn"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn classify_trigger_skips_unrelated_thread_events() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let bus = Arc::new(bus);
    let (dispatcher, _spawn_rx) = make_dispatcher(pool.clone(), bus.clone());

    let thread_id = Uuid::new_v4();
    let bus_event = BusEvent::Thread {
        thread_id,
        event: ThreadEvent::TextStreamed { text: "x".into() },
        meta: cc_meta(),
    };
    assert!(
        dispatcher
            .classify_trigger(&bus_event, Uuid::new_v4())
            .is_none(),
        "non-trigger events must not produce SpawnTriggers"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// MessageReceived stays in shadow mode — counter increments, but no
/// `SpawnRequest` reaches the receiver. The chat HTTP handler is still the
/// authoritative spawner for that trigger.
#[tokio::test]
async fn user_message_is_shadow_only_no_spawn_request_sent() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let bus = Arc::new(bus);

    let (dispatcher, mut spawn_rx) = make_dispatcher(pool.clone(), bus.clone());
    let dispatch_count = dispatcher.dispatch_count.clone();
    let handle = tokio::spawn(async move {
        dispatcher.run().await;
    });
    // Give the consumer a moment to subscribe before the producer emits.
    tokio::time::sleep(Duration::from_millis(50)).await;

    let thread_id = Uuid::new_v4();
    let _eid = emit_cc_thread(&bus, thread_id, user_message("hello")).await;

    wait_until(|| dispatch_count.load(std::sync::atomic::Ordering::SeqCst) >= 1).await;
    assert_eq!(
        dispatch_count.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "exactly one shadow dispatch for one MessageReceived"
    );
    // The shadow path must NOT push a SpawnRequest.
    assert!(
        spawn_rx.try_recv().is_err(),
        "UserMessage triggers must not produce a SpawnRequest in this phase"
    );

    handle.abort();
    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// ContinueSignal is the actuated path — emitting one must produce a
/// `SpawnRequest::Continue` on the outbound channel for the engine-side
/// receiver to consume.
#[tokio::test]
async fn continue_signal_produces_spawn_request() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let bus = Arc::new(bus);

    let (dispatcher, mut spawn_rx) = make_dispatcher(pool.clone(), bus.clone());
    let handle = tokio::spawn(async move {
        dispatcher.run().await;
    });
    tokio::time::sleep(Duration::from_millis(50)).await;

    // The thread needs a SessionStarted to satisfy the lifecycle contract
    // (ContinueSignal is CC-only and the projection rejects CC events on a
    // chat thread).
    let thread_id = Uuid::new_v4();
    emit_cc_thread(
        &bus,
        thread_id,
        ThreadEvent::SessionStarted {
            session_id: "sid-cont".into(),
            branch: "claude-code/cont".into(),
            repo_id: None,
        },
    )
    .await;
    let event_id = emit_cc_thread(
        &bus,
        thread_id,
        ThreadEvent::ContinueSignal {
            reason: ENGINE_RESTART_INTERRUPT_REASON.to_string(),
        },
    )
    .await;

    let received = tokio::time::timeout(Duration::from_secs(2), spawn_rx.recv())
        .await
        .expect("spawn_rx.recv must complete within 2s")
        .expect("channel must yield a SpawnRequest");
    assert_eq!(
        received,
        SpawnRequest::Continue {
            thread_id,
            event_id,
        },
        "ContinueSignal must produce a matching SpawnRequest::Continue"
    );

    handle.abort();
    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn dispatcher_does_not_double_spawn_for_same_trigger() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let bus = Arc::new(bus);

    let (dispatcher, _spawn_rx) = make_dispatcher(pool.clone(), bus.clone());
    let dispatch_count = dispatcher.dispatch_count.clone();

    let thread_id = Uuid::new_v4();
    let event_id = Uuid::new_v4();

    let trigger = SpawnTrigger::UserMessage {
        thread_id,
        event_id,
        text: "first".into(),
    };
    assert!(
        !dispatcher.already_spawned(event_id).await,
        "trigger should be unhandled before dispatch"
    );
    dispatcher.dispatch_spawn(trigger.clone()).await;
    assert!(
        dispatcher.already_spawned(event_id).await,
        "after dispatch the trigger must be marked spawned"
    );

    assert_eq!(
        dispatch_count.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "dispatch_spawn was called once"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Integration variant of the no-double-dispatch contract: if a trigger event
/// is already followed by a CC lifecycle event in the DB, `already_spawned`
/// must report true so the run loop never re-fires it.
#[tokio::test]
async fn dispatcher_skips_trigger_already_followed_by_cc_event() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let bus = Arc::new(bus);

    // Seed: a message followed by SessionStarted — the message is "already
    // handled" because CC clearly picked it up.
    let thread_id = Uuid::new_v4();
    let trig_id = emit_cc_thread(&bus, thread_id, user_message("done")).await;
    emit_cc_thread(
        &bus,
        thread_id,
        ThreadEvent::SessionStarted {
            session_id: "sid-x".into(),
            branch: "claude-code/x".into(),
            repo_id: None,
        },
    )
    .await;

    let (dispatcher, _spawn_rx) = make_dispatcher(pool.clone(), bus.clone());
    assert!(
        dispatcher.already_spawned(trig_id).await,
        "trigger followed by CC lifecycle event must register as already spawned"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn dispatcher_dispatches_pending_triggers_on_startup() {
    // Q9b — "user-action-not-yet-delivered" recovery path. A trigger event
    // exists in the DB with no subsequent CC events. The dispatcher's backfill
    // must NOT mark it as already-handled, so a future broadcast (or the
    // recovery code in Task 5.3) can dispatch it.
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let bus = Arc::new(bus);

    // Seed two threads:
    //   1. "handled" — message followed by CodingAgentIdled
    //   2. "pending" — message with no CC follow-up
    let handled_thread = Uuid::new_v4();
    let handled_msg_id = emit_cc_thread(&bus, handled_thread, user_message("handled msg")).await;
    emit_cc_thread(
        &bus,
        handled_thread,
        ThreadEvent::CodingAgentIdled {
            has_changes: false,
            is_external_repo: false,
            requires_restart: false,
            cc_session_id: Some("handled-sid".into()),
            agent: crate::runtime::AgentKind::ClaudeCode,
            reason: None,
            worktree_path: None,
            worktree_head_sha: None,
        },
    )
    .await;

    let pending_thread = Uuid::new_v4();
    let pending_msg_id =
        emit_cc_thread(&bus, pending_thread, user_message("pending msg")).await;

    let (dispatcher, _spawn_rx) = make_dispatcher(pool.clone(), bus.clone());
    dispatcher
        .backfill_pending_triggers_on_startup()
        .await
        .expect("backfill succeeds");

    assert!(
        dispatcher.already_spawned(handled_msg_id).await,
        "the handled message must be flagged as already spawned"
    );
    assert!(
        !dispatcher.already_spawned(pending_msg_id).await,
        "the pending message must remain unhandled so recovery can dispatch it"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Direct dispatch_spawn for a UserMessage trigger increments the counter
/// (observability), but never enqueues a SpawnRequest in this phase.
#[tokio::test]
async fn shadow_dispatch_for_user_message_does_not_enqueue() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let bus = Arc::new(bus);
    let (dispatcher, mut spawn_rx) = make_dispatcher(pool.clone(), bus.clone());

    let trigger = SpawnTrigger::UserMessage {
        thread_id: Uuid::new_v4(),
        event_id: Uuid::new_v4(),
        text: "shadow".into(),
    };
    dispatcher.dispatch_spawn(trigger).await;
    assert_eq!(
        dispatcher.dispatch_count(),
        1,
        "shadow-mode dispatch still increments the counter for observability"
    );
    assert!(
        spawn_rx.try_recv().is_err(),
        "shadow path must not enqueue a SpawnRequest"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn dispatcher_ignores_chat_channel_message_in_live_loop() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let bus = Arc::new(bus);

    let (dispatcher, _spawn_rx) = make_dispatcher(pool.clone(), bus.clone());
    let dispatch_count = dispatcher.dispatch_count.clone();
    let handle = tokio::spawn(async move {
        dispatcher.run().await;
    });
    tokio::time::sleep(Duration::from_millis(50)).await;

    let thread_id = Uuid::new_v4();
    emit_chat_thread(&bus, thread_id, user_message("hi from chat")).await;

    // Give the consumer time to (NOT) act.
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert_eq!(
        dispatch_count.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "chat-channel messages must not trigger CC dispatches"
    );

    handle.abort();
    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn session_ended_does_not_trigger_dispatch() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let bus = Arc::new(bus);

    let (dispatcher, _spawn_rx) = make_dispatcher(pool.clone(), bus.clone());
    let dispatch_count = dispatcher.dispatch_count.clone();
    let handle = tokio::spawn(async move {
        dispatcher.run().await;
    });
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Seed a SessionStarted first so the lifecycle contract accepts the
    // SessionEnded that follows. The dispatcher counts dispatches across
    // ALL events on the bus, so we capture the baseline (which should be
    // zero — SessionStarted is not a trigger) and ensure it stays zero
    // after SessionEnded.
    let thread_id = Uuid::new_v4();
    emit_cc_thread(
        &bus,
        thread_id,
        ThreadEvent::SessionStarted {
            session_id: "sid-end".into(),
            branch: "claude-code/end".into(),
            repo_id: None,
        },
    )
    .await;
    emit_cc_thread(
        &bus,
        thread_id,
        ThreadEvent::SessionEnded {
            reason: SessionEndReason::Shutdown,
        },
    )
    .await;

    tokio::time::sleep(Duration::from_millis(150)).await;
    assert_eq!(
        dispatch_count.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "neither SessionStarted nor SessionEnded should trigger a dispatch"
    );

    handle.abort();
    pool.close().await;
    teardown_test_db(&db_name).await;
}
