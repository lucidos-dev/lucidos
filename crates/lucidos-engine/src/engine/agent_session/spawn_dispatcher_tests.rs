use std::sync::Arc;
use std::time::Duration;

use uuid::Uuid;

use crate::engine::agent_recovery::{
    AUTO_RESUME_AFTER_SWITCH_REASON, ENGINE_RESTART_INTERRUPT_REASON,
};
use crate::engine::event_bus::{BusEvent, EventBus};
use crate::engine::thread_events::{
    AbortCause, ActorMode, EventChannel, EventMeta, MessageOrigin, SessionEndReason, ThreadEvent,
};
use crate::test_support::{setup_test_db, start_cc_session, teardown_test_db};

use super::{thread_has_unactuated_continuation, SpawnDispatcher, SpawnRequest, SpawnTrigger};

fn cc_meta() -> EventMeta {
    EventMeta {
        channel: Some(EventChannel::ClaudeCode),
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
    }
}

fn agent_idled(session_id: &str, reason: Option<&str>) -> ThreadEvent {
    ThreadEvent::CodingAgentIdled {
        has_changes: false,
        is_external_repo: false,
        requires_restart: false,
        cc_session_id: Some(session_id.into()),
        coding_agent: crate::runtime::CodingAgent::ClaudeCode,
        reason: reason.map(String::from),
        worktree_path: None,
        worktree_head_sha: None,
        bg_bash_pending: false,
    }
}

fn continuation(reason: &str) -> ThreadEvent {
    ThreadEvent::ContinuationRequested {
        reason: reason.to_string(),
    }
}

/// Emit the system-attributed teardown `ResponseAborted` recovery uses to mark a
/// crash-interrupted turn (via the typed helper, per the emit rule).
async fn abort_as_system(bus: &EventBus, thread_id: Uuid) {
    crate::engine::thread_events::emit_response_aborted(
        bus,
        thread_id,
        AbortCause::RecoveryAfterRestart,
        String::new(),
        vec![],
        None,
        None,
        EventMeta {
            channel: Some(EventChannel::ClaudeCode),
            actor: Some(MessageOrigin::system()),
            ..EventMeta::NONE
        },
        "[test] crash-marking abort",
    )
    .await;
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
) -> (
    SpawnDispatcher,
    tokio::sync::mpsc::UnboundedReceiver<SpawnRequest>,
) {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    (SpawnDispatcher::new(pool, tx), rx)
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
    let (dispatcher, _spawn_rx) = make_dispatcher(pool.clone());

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
async fn classify_trigger_recognizes_continuation_requested() {
    let (pool, db_name) = setup_test_db().await;
    let (dispatcher, _spawn_rx) = make_dispatcher(pool.clone());

    let thread_id = Uuid::new_v4();
    let event_id = Uuid::new_v4();
    let bus_event = BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ContinuationRequested {
            reason: ENGINE_RESTART_INTERRUPT_REASON.to_string(),
        },
        meta: cc_meta(),
    };
    let trigger = dispatcher
        .classify_trigger(&bus_event, event_id)
        .expect("ContinuationRequested must classify as a trigger");
    match trigger {
        SpawnTrigger::ContinuationRequested {
            thread_id: tid,
            event_id: eid,
        } => {
            assert_eq!(tid, thread_id);
            assert_eq!(eid, event_id);
        }
        other => panic!("expected ContinuationRequested, got {:?}", other),
    }

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn classify_trigger_skips_chat_channel_messages() {
    let (pool, db_name) = setup_test_db().await;
    let (dispatcher, _spawn_rx) = make_dispatcher(pool.clone());

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
    let (dispatcher, _spawn_rx) = make_dispatcher(pool.clone());

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

    let (dispatcher, mut spawn_rx) = make_dispatcher(pool.clone());
    let dispatch_count = dispatcher.dispatch_count.clone();
    // Subscribe before starting the loop — mirrors `SpawnDispatcher::spawn()`,
    // so no settling sleep is needed before the producer emits.
    let rx = bus.subscribe();
    let handle = tokio::spawn(async move {
        dispatcher.run(rx).await;
    });

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

/// ContinuationRequested is the actuated path — emitting one must produce a
/// `SpawnRequest::Continue` on the outbound channel for the engine-side
/// receiver to consume.
#[tokio::test]
async fn continuation_requested_produces_spawn_request() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let bus = Arc::new(bus);

    let (dispatcher, mut spawn_rx) = make_dispatcher(pool.clone());
    let rx = bus.subscribe();
    let handle = tokio::spawn(async move {
        dispatcher.run(rx).await;
    });

    // The thread needs a SessionStarted to satisfy the lifecycle contract
    // (ContinuationRequested is CC-only and the projection rejects CC events on a
    // chat thread).
    let thread_id = Uuid::new_v4();
    start_cc_session(&bus, thread_id, "claude-code/cont", None).await;
    let event_id = emit_cc_thread(
        &bus,
        thread_id,
        continuation(ENGINE_RESTART_INTERRUPT_REASON),
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
        "ContinuationRequested must produce a matching SpawnRequest::Continue"
    );

    handle.abort();
    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn dispatcher_does_not_double_spawn_for_same_trigger() {
    let (pool, db_name) = setup_test_db().await;

    let (dispatcher, mut spawn_rx) = make_dispatcher(pool.clone());
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

    // A second dispatch with the SAME event id must be a no-op: the insert
    // into the idempotency set is the atomic claim, so the duplicate neither
    // counts nor sends.
    dispatcher.dispatch_spawn(trigger).await;
    assert_eq!(
        dispatch_count.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "a duplicate dispatch for the same trigger event must not count"
    );

    // Same contract on the actuating variant: two dispatches, one SpawnRequest.
    let cont_event = Uuid::new_v4();
    let cont = SpawnTrigger::ContinuationRequested {
        thread_id,
        event_id: cont_event,
    };
    dispatcher.dispatch_spawn(cont.clone()).await;
    dispatcher.dispatch_spawn(cont).await;
    assert_eq!(
        spawn_rx.try_recv().expect("first dispatch must send"),
        SpawnRequest::Continue {
            thread_id,
            event_id: cont_event,
        }
    );
    assert!(
        spawn_rx.try_recv().is_err(),
        "the duplicate Continue dispatch must not send a second SpawnRequest"
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
    start_cc_session(&bus, thread_id, "claude-code/x", None).await;

    let (dispatcher, _spawn_rx) = make_dispatcher(pool.clone());
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
    emit_cc_thread(&bus, handled_thread, agent_idled("handled-sid", None)).await;

    let pending_thread = Uuid::new_v4();
    let pending_msg_id = emit_cc_thread(&bus, pending_thread, user_message("pending msg")).await;

    let (dispatcher, _spawn_rx) = make_dispatcher(pool.clone());
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
    let (dispatcher, mut spawn_rx) = make_dispatcher(pool.clone());

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

    let (dispatcher, _spawn_rx) = make_dispatcher(pool.clone());
    let dispatch_count = dispatcher.dispatch_count.clone();
    let rx = bus.subscribe();
    let handle = tokio::spawn(async move {
        dispatcher.run(rx).await;
    });

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

    let (dispatcher, _spawn_rx) = make_dispatcher(pool.clone());
    let dispatch_count = dispatcher.dispatch_count.clone();
    let rx = bus.subscribe();
    let handle = tokio::spawn(async move {
        dispatcher.run(rx).await;
    });

    // Seed a SessionStarted first so the lifecycle contract accepts the
    // SessionEnded that follows. The dispatcher counts dispatches across
    // ALL events on the bus, so we capture the baseline (which should be
    // zero — SessionStarted is not a trigger) and ensure it stays zero
    // after SessionEnded.
    let thread_id = Uuid::new_v4();
    start_cc_session(&bus, thread_id, "claude-code/end", None).await;
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

// -- Startup delivery guarantees (subscribe-before-backfill + orphan net) -----

/// Regression for the switch-resume zombie: `SpawnDispatcher::spawn()` must
/// open its broadcast subscription BEFORE the startup backfill runs —
/// synchronously, before it returns — so a `ContinuationRequested` emitted
/// immediately after `spawn()` (exactly the shape of `main.rs` calling
/// `resume_pending_switches()` moments later) is buffered by the receiver and
/// dispatched. With the old subscribe-inside-`run()` ordering, this emit raced
/// the seconds-long backfill query and was broadcast to zero subscribers —
/// lost forever, leaving the thread `running` with no CC subprocess.
#[tokio::test]
async fn continuation_emitted_immediately_after_spawn_is_not_lost() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let bus = Arc::new(bus);

    let (handle, mut spawn_rx) = SpawnDispatcher::spawn(pool.clone(), bus.clone());

    // Emit with NO settling sleep — the subscription must already exist when
    // spawn() returns, whatever the backfill is still doing.
    let thread_id = Uuid::new_v4();
    start_cc_session(&bus, thread_id, "claude-code/race", None).await;
    let event_id = emit_cc_thread(
        &bus,
        thread_id,
        continuation(AUTO_RESUME_AFTER_SWITCH_REASON),
    )
    .await;

    let received = tokio::time::timeout(Duration::from_secs(2), spawn_rx.recv())
        .await
        .expect(
            "SpawnRequest must arrive: a timeout means the subscribe-after-backfill race is back",
        )
        .expect("channel must yield a SpawnRequest");
    assert_eq!(
        received,
        SpawnRequest::Continue {
            thread_id,
            event_id,
        }
    );

    // Exactly once: the startup orphan re-dispatch may also observe the same
    // persisted event; the atomic mark-and-send collapses the two paths.
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert!(
        spawn_rx.try_recv().is_err(),
        "exactly one SpawnRequest per trigger event"
    );

    handle.abort();
    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Regression for the durable safety net: a `ContinuationRequested` emitted by
/// a prior boot but never actuated (no subsequent CC lifecycle event — the
/// thread is a `running` zombie) is re-dispatched exactly once at dispatcher
/// startup, onto the same durable mpsc channel as the live path.
#[tokio::test]
async fn orphaned_continuation_is_redispatched_exactly_once_on_startup() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let bus = Arc::new(bus);

    // Prior-boot state, seeded BEFORE the dispatcher exists: the emit was
    // persisted (projection flipped status → running) but its live broadcast
    // was lost, so no lifecycle event ever followed.
    let thread_id = Uuid::new_v4();
    start_cc_session(&bus, thread_id, "claude-code/orphan", None).await;
    let orphan_id = emit_cc_thread(
        &bus,
        thread_id,
        continuation(AUTO_RESUME_AFTER_SWITCH_REASON),
    )
    .await;

    let (handle, mut spawn_rx) = SpawnDispatcher::spawn(pool.clone(), bus.clone());

    let received = tokio::time::timeout(Duration::from_secs(2), spawn_rx.recv())
        .await
        .expect("the orphaned ContinuationRequested must be re-dispatched at startup")
        .expect("channel must yield a SpawnRequest");
    assert_eq!(
        received,
        SpawnRequest::Continue {
            thread_id,
            event_id: orphan_id,
        }
    );

    tokio::time::sleep(Duration::from_millis(150)).await;
    assert!(
        spawn_rx.try_recv().is_err(),
        "the orphan must be re-dispatched exactly once"
    );

    handle.abort();
    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Supersession: when a thread carries TWO unactuated `ContinuationRequested`
/// events, only the newest drives a spawn — an older request is superseded,
/// never dispatched alongside it.
#[tokio::test]
async fn only_newest_unactuated_continuation_is_redispatched() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let bus = Arc::new(bus);

    let thread_id = Uuid::new_v4();
    start_cc_session(&bus, thread_id, "claude-code/sup", None).await;
    let _older = emit_cc_thread(
        &bus,
        thread_id,
        continuation(AUTO_RESUME_AFTER_SWITCH_REASON),
    )
    .await;
    let newest = emit_cc_thread(
        &bus,
        thread_id,
        continuation(AUTO_RESUME_AFTER_SWITCH_REASON),
    )
    .await;

    let (handle, mut spawn_rx) = SpawnDispatcher::spawn(pool.clone(), bus.clone());

    let received = tokio::time::timeout(Duration::from_secs(2), spawn_rx.recv())
        .await
        .expect("the newest orphan must be re-dispatched")
        .expect("channel must yield a SpawnRequest");
    assert_eq!(
        received,
        SpawnRequest::Continue {
            thread_id,
            event_id: newest,
        },
        "only the NEWEST unactuated request drives the resume"
    );

    tokio::time::sleep(Duration::from_millis(150)).await;
    assert!(
        spawn_rx.try_recv().is_err(),
        "the superseded older request must not also dispatch"
    );

    handle.abort();
    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Settled threads stay settled: a `ContinuationRequested` that was actuated
/// (lifecycle event followed) or terminally settled (a `ResponseAborted`
/// landed after it) is NOT re-dispatched at startup — the safety net never
/// resurrects a thread the user saw finish.
#[tokio::test]
async fn actuated_and_settled_continuations_are_not_redispatched() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let bus = Arc::new(bus);

    // Actuated: the request was followed by an idle — the resume ran.
    let actuated = Uuid::new_v4();
    start_cc_session(&bus, actuated, "claude-code/act", None).await;
    emit_cc_thread(
        &bus,
        actuated,
        continuation(AUTO_RESUME_AFTER_SWITCH_REASON),
    )
    .await;
    emit_cc_thread(&bus, actuated, agent_idled("sid-act", None)).await;

    // Actuation marker only: the spawn consumer emits ContinuationStarted
    // BEFORE run_direct_agent, so a request followed by just that marker was
    // actuated (the resume died mid-cold-start). Re-driving it would defeat
    // the crash loop-breaker — next boot's recovery owns this thread.
    let started = Uuid::new_v4();
    start_cc_session(&bus, started, "claude-code/started", None).await;
    emit_cc_thread(&bus, started, continuation(AUTO_RESUME_AFTER_SWITCH_REASON)).await;
    emit_cc_thread(
        &bus,
        started,
        ThreadEvent::ContinuationStarted {
            branch: String::new(),
            origin: None,
            reason: Some(AUTO_RESUME_AFTER_SWITCH_REASON.to_string()),
        },
    )
    .await;

    // Settled: something terminal landed after the request (e.g. the
    // orphaned-running settle sweep) — the manual Continue affordance stands.
    let settled = Uuid::new_v4();
    start_cc_session(&bus, settled, "claude-code/set", None).await;
    emit_cc_thread(&bus, settled, continuation(AUTO_RESUME_AFTER_SWITCH_REASON)).await;
    abort_as_system(&bus, settled).await;

    let (handle, mut spawn_rx) = SpawnDispatcher::spawn(pool.clone(), bus.clone());

    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(
        spawn_rx.try_recv().is_err(),
        "no actuated, started, or settled continuation may be re-dispatched"
    );

    handle.abort();
    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Crash contract: a crash-interrupted thread gets NO `ContinuationRequested`
/// — recovery emits a system-attributed `ResponseAborted` plus
/// `CodingAgentIdled{engine_restart_interrupt}` and keeps the manual Continue
/// affordance. The startup safety net must not invent a resume for it, so work
/// that may have crashed the engine can't loop.
#[tokio::test]
async fn crash_interrupted_thread_is_not_auto_resumed_on_startup() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let bus = Arc::new(bus);

    // The crash-recovery shape (see `recover_orphaned_worktrees`' else-branch):
    // abort marking the dead turn, then the synthetic idle. No continuation.
    let thread_id = Uuid::new_v4();
    start_cc_session(&bus, thread_id, "claude-code/crash", None).await;
    abort_as_system(&bus, thread_id).await;
    emit_cc_thread(
        &bus,
        thread_id,
        agent_idled("sid-crash", Some(ENGINE_RESTART_INTERRUPT_REASON)),
    )
    .await;

    let (handle, mut spawn_rx) = SpawnDispatcher::spawn(pool.clone(), bus.clone());

    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(
        spawn_rx.try_recv().is_err(),
        "a crash-interrupted thread (no ContinuationRequested) must NOT be auto-resumed"
    );

    handle.abort();
    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// The predicate `resume_pending_switches` uses to avoid stacking a second
/// request event id onto a thread whose existing request the startup orphan
/// re-dispatch already owns.
#[tokio::test]
async fn thread_has_unactuated_continuation_tracks_request_lifecycle() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let thread_id = Uuid::new_v4();
    start_cc_session(&bus, thread_id, "claude-code/pred", None).await;
    assert!(
        !thread_has_unactuated_continuation(&pool, thread_id).await,
        "no request at all → false"
    );

    emit_cc_thread(
        &bus,
        thread_id,
        continuation(AUTO_RESUME_AFTER_SWITCH_REASON),
    )
    .await;
    assert!(
        thread_has_unactuated_continuation(&pool, thread_id).await,
        "an emitted-but-unactuated request → true"
    );

    emit_cc_thread(&bus, thread_id, agent_idled("sid-pred", None)).await;
    assert!(
        !thread_has_unactuated_continuation(&pool, thread_id).await,
        "a lifecycle event after the request means it was actuated → false"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}
