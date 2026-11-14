use std::sync::Arc;
use std::time::Duration;

use tokio::sync::mpsc::UnboundedReceiver;
use uuid::Uuid;

use crate::engine::agent_recovery::{
    AUTO_RESUME_AFTER_SWITCH_REASON, ENGINE_RESTART_INTERRUPT_REASON,
};
use crate::engine::event_bus::{BusEvent, EventBus};
use crate::engine::thread_events::{
    AbortCause, ActorMode, EventChannel, EventMeta, MessageOrigin, SessionEndReason, ThreadEvent,
};
use crate::test_support::{setup_test_db, start_cc_session, teardown_test_db};

use super::{thread_has_unactuated_continuation, SpawnDispatcher, SpawnRequest};

fn cc_meta() -> EventMeta {
    EventMeta {
        channel: Some(EventChannel::ClaudeCode),
        ..EventMeta::NONE
    }
}

fn user_message(text: &str) -> ThreadEvent {
    ThreadEvent::MessageReceived {
        provider: None,
        voice_session_id: None,
        wake_mode: None,
        text: text.into(),
        user_image_hashes: vec![],
        device_id: None,
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

/// A coding-agent thread with one `ContinuationRequested`. Returns the request.
async fn requested_thread(bus: &EventBus, reason: &str) -> SpawnRequest {
    let thread_id = Uuid::new_v4();
    start_cc_session(bus, thread_id, "claude-code/cont", None).await;
    let event_id = emit_cc_thread(bus, thread_id, continuation(reason)).await;
    SpawnRequest::Continue {
        thread_id,
        event_id,
    }
}

/// Build a dispatcher with a fresh outbound channel. Returns the dispatcher
/// and the receiver so tests can assert what (if anything) was sent.
fn make_dispatcher(pool: sqlx::PgPool) -> (SpawnDispatcher, UnboundedReceiver<SpawnRequest>) {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    (SpawnDispatcher::new(pool, tx), rx)
}

/// The next request the dispatcher sends. The timeout only bounds a failure.
async fn next_sent(spawn_rx: &mut UnboundedReceiver<SpawnRequest>) -> SpawnRequest {
    tokio::time::timeout(Duration::from_secs(5), spawn_rx.recv())
        .await
        .expect("the dispatcher sent nothing")
        .expect("channel must yield a SpawnRequest")
}

/// Emit a request on a fresh thread and assert it is the NEXT one sent. The
/// dispatcher handles requests in order, after its startup scan, so any send
/// owed before the barrier has already arrived when the barrier does.
async fn assert_nothing_else_sent(bus: &EventBus, spawn_rx: &mut UnboundedReceiver<SpawnRequest>) {
    let barrier = requested_thread(bus, AUTO_RESUME_AFTER_SWITCH_REASON).await;
    assert_eq!(
        next_sent(spawn_rx).await,
        barrier,
        "a send arrived ahead of the barrier"
    );
}

/// Every committed `ContinuationRequested` reaches the dispatcher, and the
/// dispatcher forwards it to the engine-side consumer.
#[tokio::test]
async fn continuation_requested_produces_spawn_request() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let bus = Arc::new(bus);
    let (handle, mut spawn_rx) = SpawnDispatcher::spawn(pool.clone(), bus.clone());

    let request = requested_thread(&bus, ENGINE_RESTART_INTERRUPT_REASON).await;
    assert_eq!(next_sent(&mut spawn_rx).await, request);

    handle.abort();
    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// The bus sends only `ContinuationRequested`, and it sends synchronously in
/// PostCommit, so nothing else is on the channel when `emit` returns.
#[tokio::test]
async fn only_a_continuation_request_reaches_the_channel() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let mut requests = bus.take_spawn_requests().expect("first take");

    let thread_id = Uuid::new_v4();
    start_cc_session(&bus, thread_id, "claude-code/other", None).await;
    emit_cc_thread(&bus, thread_id, user_message("hello")).await;
    emit_cc_thread(
        &bus,
        thread_id,
        agent_idled("sid-int", Some(ENGINE_RESTART_INTERRUPT_REASON)),
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
    assert!(
        requests.try_recv().is_err(),
        "no event but ContinuationRequested may produce a spawn request"
    );

    let event_id = emit_cc_thread(
        &bus,
        thread_id,
        continuation(AUTO_RESUME_AFTER_SWITCH_REASON),
    )
    .await;
    assert_eq!(
        requests.try_recv().expect("sent before emit returned"),
        SpawnRequest::Continue {
            thread_id,
            event_id,
        }
    );
    assert!(
        bus.take_spawn_requests().is_none(),
        "the requests have one consumer"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn dispatcher_does_not_double_spawn_for_same_trigger() {
    let (pool, db_name) = setup_test_db().await;
    let (dispatcher, mut spawn_rx) = make_dispatcher(pool.clone());

    let request = SpawnRequest::Continue {
        thread_id: Uuid::new_v4(),
        event_id: Uuid::new_v4(),
    };
    let SpawnRequest::Continue { event_id, .. } = request;
    assert!(!dispatcher.already_spawned(event_id).await);

    // The insert into the idempotency set is the atomic claim: two
    // dispatches, one SpawnRequest.
    dispatcher.dispatch_spawn(request.clone()).await;
    dispatcher.dispatch_spawn(request.clone()).await;
    assert!(dispatcher.already_spawned(event_id).await);
    assert_eq!(
        spawn_rx.try_recv().expect("first dispatch must send"),
        request
    );
    assert!(
        spawn_rx.try_recv().is_err(),
        "the duplicate Continue dispatch must not send a second SpawnRequest"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// A trigger already followed by a CC lifecycle event counts as spawned, so
/// the run loop never re-fires it.
#[tokio::test]
async fn dispatcher_skips_trigger_already_followed_by_cc_event() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

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

/// I12: a broadcast lag never drops a spawn request. The request is persisted,
/// then the broadcast overflows before the dispatcher starts. The request still
/// arrives, because the dispatcher reads the bus's directed channel.
#[tokio::test]
async fn a_lagged_broadcast_never_drops_a_continuation() {
    use crate::engine::event_bus::{SystemEvent, BUS_CAPACITY};
    use tokio::sync::broadcast::error::TryRecvError;

    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let (dispatcher, mut spawn_rx) = make_dispatcher(pool.clone());
    let requests = bus.take_spawn_requests().expect("first take");
    let mut lagging = bus.subscribe();

    let request = requested_thread(&bus, ENGINE_RESTART_INTERRUPT_REASON).await;
    for _ in 0..=BUS_CAPACITY {
        bus.broadcast_transient_system(SystemEvent::ThreadQueueChanged {});
    }
    assert!(
        matches!(lagging.try_recv(), Err(TryRecvError::Lagged(_))),
        "the broadcast must have overflowed past the request"
    );

    let handle = tokio::spawn(async move { dispatcher.run(requests).await });
    assert_eq!(next_sent(&mut spawn_rx).await, request);

    handle.abort();
    pool.close().await;
    teardown_test_db(&db_name).await;
}

// -- Startup delivery guarantees (live channel + orphan net) ------------------

/// A request emitted before the dispatcher starts waits on the bus's channel.
/// The startup orphan scan also sees the same persisted row, and the claim
/// collapses the two paths to one send.
#[tokio::test]
async fn a_continuation_emitted_before_the_dispatcher_starts_is_sent_once() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let bus = Arc::new(bus);

    let request = requested_thread(&bus, AUTO_RESUME_AFTER_SWITCH_REASON).await;
    let (handle, mut spawn_rx) = SpawnDispatcher::spawn(pool.clone(), bus.clone());

    assert_eq!(next_sent(&mut spawn_rx).await, request);
    assert_nothing_else_sent(&bus, &mut spawn_rx).await;

    handle.abort();
    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// A `ContinuationRequested` a previous process persisted but never actuated
/// (the thread is a `running` zombie) is re-dispatched exactly once at
/// dispatcher startup. It is not on this bus's channel: only the scan sees it.
#[tokio::test]
async fn orphaned_continuation_is_redispatched_exactly_once_on_startup() {
    let (pool, db_name) = setup_test_db().await;
    let (previous, _rx) = EventBus::new(pool.clone());
    let orphan = requested_thread(&previous, AUTO_RESUME_AFTER_SWITCH_REASON).await;

    let (bus, _rx) = EventBus::new(pool.clone());
    let bus = Arc::new(bus);
    let (handle, mut spawn_rx) = SpawnDispatcher::spawn(pool.clone(), bus.clone());

    assert_eq!(next_sent(&mut spawn_rx).await, orphan);
    assert_nothing_else_sent(&bus, &mut spawn_rx).await;

    handle.abort();
    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Supersession: when a thread carries TWO unactuated `ContinuationRequested`
/// events from a previous process, only the newest drives a spawn.
#[tokio::test]
async fn only_newest_unactuated_continuation_is_redispatched() {
    let (pool, db_name) = setup_test_db().await;
    let (previous, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    start_cc_session(&previous, thread_id, "claude-code/sup", None).await;
    emit_cc_thread(
        &previous,
        thread_id,
        continuation(AUTO_RESUME_AFTER_SWITCH_REASON),
    )
    .await;
    let newest = emit_cc_thread(
        &previous,
        thread_id,
        continuation(AUTO_RESUME_AFTER_SWITCH_REASON),
    )
    .await;

    let (bus, _rx) = EventBus::new(pool.clone());
    let bus = Arc::new(bus);
    let (handle, mut spawn_rx) = SpawnDispatcher::spawn(pool.clone(), bus.clone());

    assert_eq!(
        next_sent(&mut spawn_rx).await,
        SpawnRequest::Continue {
            thread_id,
            event_id: newest,
        },
        "only the NEWEST unactuated request drives the resume"
    );
    assert_nothing_else_sent(&bus, &mut spawn_rx).await;

    handle.abort();
    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Settled threads stay settled: a `ContinuationRequested` that was actuated
/// (lifecycle event followed) or terminally settled (a `ResponseAborted`
/// landed after it) is NOT re-dispatched at startup.
#[tokio::test]
async fn actuated_and_settled_continuations_are_not_redispatched() {
    let (pool, db_name) = setup_test_db().await;
    let (previous, _rx) = EventBus::new(pool.clone());

    // Actuated: the request was followed by an idle — the resume ran.
    let actuated = Uuid::new_v4();
    start_cc_session(&previous, actuated, "claude-code/act", None).await;
    emit_cc_thread(
        &previous,
        actuated,
        continuation(AUTO_RESUME_AFTER_SWITCH_REASON),
    )
    .await;
    emit_cc_thread(&previous, actuated, agent_idled("sid-act", None)).await;

    // Actuation marker only: the spawn consumer emits ContinuationStarted
    // BEFORE run_direct_agent, so a request followed by just that marker was
    // actuated (the resume died mid-cold-start). Re-driving it would defeat
    // the crash loop-breaker — next boot's recovery owns this thread.
    let started = Uuid::new_v4();
    start_cc_session(&previous, started, "claude-code/started", None).await;
    emit_cc_thread(
        &previous,
        started,
        continuation(AUTO_RESUME_AFTER_SWITCH_REASON),
    )
    .await;
    emit_cc_thread(
        &previous,
        started,
        ThreadEvent::ContinuationStarted {
            branch: String::new(),
            origin: None,
            reason: Some(AUTO_RESUME_AFTER_SWITCH_REASON.to_string()),
            resumed_abort_id: None,
        },
    )
    .await;

    // Settled: something terminal landed after the request (e.g. the
    // orphaned-running settle sweep) — the manual Continue affordance stands.
    let settled = Uuid::new_v4();
    start_cc_session(&previous, settled, "claude-code/set", None).await;
    emit_cc_thread(
        &previous,
        settled,
        continuation(AUTO_RESUME_AFTER_SWITCH_REASON),
    )
    .await;
    abort_as_system(&previous, settled).await;

    let (bus, _rx) = EventBus::new(pool.clone());
    let bus = Arc::new(bus);
    let (handle, mut spawn_rx) = SpawnDispatcher::spawn(pool.clone(), bus.clone());

    assert_nothing_else_sent(&bus, &mut spawn_rx).await;

    handle.abort();
    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Crash contract: a crash-interrupted thread gets NO `ContinuationRequested`
/// — recovery emits a system-attributed `ResponseAborted` plus
/// `CodingAgentIdled{engine_restart_interrupt}` and keeps the manual Continue
/// affordance. The startup safety net must not invent a resume for it.
#[tokio::test]
async fn crash_interrupted_thread_is_not_auto_resumed_on_startup() {
    let (pool, db_name) = setup_test_db().await;
    let (previous, _rx) = EventBus::new(pool.clone());

    let thread_id = Uuid::new_v4();
    start_cc_session(&previous, thread_id, "claude-code/crash", None).await;
    abort_as_system(&previous, thread_id).await;
    emit_cc_thread(
        &previous,
        thread_id,
        agent_idled("sid-crash", Some(ENGINE_RESTART_INTERRUPT_REASON)),
    )
    .await;

    let (bus, _rx) = EventBus::new(pool.clone());
    let bus = Arc::new(bus);
    let (handle, mut spawn_rx) = SpawnDispatcher::spawn(pool.clone(), bus.clone());

    assert_nothing_else_sent(&bus, &mut spawn_rx).await;

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
