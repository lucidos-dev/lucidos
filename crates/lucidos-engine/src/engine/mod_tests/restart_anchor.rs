//! Which turn an out-of-loop abort terminates.
//!
//! The reported bug (`docs/plans/2026-08-06-restart-abort-anchors-on-the-in-flight-turn.md`):
//! a *Switch to new version* rendered BOTH "Paused by restart" and "Response
//! canceled" on a thread whose user pressed Stop on neither. The restart
//! teardown guessed the in-flight turn's `request_event_id` with
//! `latest_originating_event_id`, which returns the NEWEST originating-type
//! event on the thread. That is the wrong turn whenever a follow-up was queued
//! mid-turn (the queued `MessageReceived` is newer but never anchors a turn) or
//! the running turn was started by an event the query's list does not name
//! (`ContinuationStarted` for a chat Continue, `ContinuationRequested` for a
//! coding-agent resume). With the abort naming one turn and the loop's cancel
//! naming another, the gate in `emit_response_canceled` matched nothing and both
//! terminators landed, on two different exchanges.
//!
//! [`in_flight_request_event_id`] reads the anchor the running turn recorded on
//! its own [`ThreadHandle`], so the two agree and the gate fires. These tests
//! pin both halves: the anchor that comes back, and the single terminator that
//! results.

use super::common::*;
use super::*;
use crate::engine::event_bus::{BusEvent, EventBus};
use crate::engine::thread_events::{
    AbortCause, ActorMode, CancelCause, EventMeta, MessageOrigin, ThreadEvent,
};
use crate::test_support::{setup_test_db, teardown_test_db};
use uuid::Uuid;

fn message_received(text: &str) -> ThreadEvent {
    ThreadEvent::MessageReceived {
        text: text.to_string(),
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

/// Persist an event with a caller-chosen id, the way the chat path does when it
/// forwards the frontend's optimistic UUID. Returns that id.
async fn emit_with_id(bus: &EventBus, thread_id: Uuid, event: ThreadEvent, id: Uuid) -> Uuid {
    bus.emit(BusEvent::Thread {
        thread_id,
        event,
        meta: EventMeta {
            event_id: Some(id),
            ..EventMeta::NONE
        },
    })
    .await
    .expect("event must persist");
    id
}

/// Record `anchor` through the production writer, the way
/// `chat/process/run.rs` does once the turn's originating event is resolved.
fn record_anchor(
    threads: &Arc<std::sync::Mutex<HashMap<Uuid, ThreadHandle>>>,
    thread_id: Uuid,
    guard: &ThreadGuard,
    anchor: Uuid,
) {
    crate::engine::record_request_event_id(threads, thread_id, guard.generation(), anchor);
}

fn read_anchor(
    threads: &Arc<std::sync::Mutex<HashMap<Uuid, ThreadHandle>>>,
    thread_id: Uuid,
) -> Option<Uuid> {
    threads
        .lock()
        .unwrap()
        .get(&thread_id)
        .and_then(|h| *h.request_event_id.lock().unwrap())
}

async fn count_of(pool: &sqlx::PgPool, thread_id: Uuid, event_type: &str) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM events WHERE aggregate_id = $1 AND event_type = $2")
        .bind(thread_id.to_string())
        .bind(event_type)
        .fetch_one(pool)
        .await
        .expect("count query")
}

// ---------------------------------------------------------------------------
// The reported shape: a follow-up queued mid-turn
// ---------------------------------------------------------------------------

/// The exact reproduction of the reported event log. A turn anchored on MR#1 is
/// running; the user types MR#2, which is persisted at once and injected into
/// that same turn rather than starting its own. The restart teardown must
/// terminate MR#1's turn, and the loop's cancel arm that fires moments later
/// must then find that terminator and skip.
///
/// The decoy is load-bearing: `latest_originating_event_id` returns MR#2 here,
/// so a regression to the old lookup makes both assertions fail together.
#[tokio::test]
async fn queued_followup_does_not_steal_the_in_flight_anchor() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let threads = make_threads();
    let thread_id = Uuid::new_v4();

    let in_flight = emit_with_id(
        &bus,
        thread_id,
        message_received("summarize the tickets"),
        Uuid::new_v4(),
    )
    .await;
    let (_token, _rx2, guard) = register(&threads, thread_id);
    record_anchor(&threads, thread_id, &guard, in_flight);

    // The follow-up the user typed while the turn above was working. Newer, and
    // never a turn anchor.
    let queued = emit_with_id(
        &bus,
        thread_id,
        message_received("actually, only the open ones"),
        Uuid::new_v4(),
    )
    .await;

    // Pin the delta: the fallback really would pick the queued message here, so
    // this test still fails if the handle read is ever removed.
    assert_eq!(
        crate::engine::agent_session::latest_originating_event_id(
            &pool,
            thread_id,
            crate::engine::agent_session::CHAT_ORIGINATING_EVENT_TYPES,
        )
        .await,
        Some(queued),
        "decoy check: the origin-type query returns the queued follow-up"
    );

    let resolved = crate::engine::in_flight_request_event_id(
        &threads,
        &pool,
        thread_id,
        crate::engine::agent_session::CHAT_ORIGINATING_EVENT_TYPES,
    )
    .await;
    assert_eq!(
        resolved,
        Some(in_flight),
        "the abort must name the turn in flight, not the queued follow-up"
    );

    // The teardown pre-emit, then the loop's cancel arm with the turn's own meta.
    let turn_meta = EventMeta {
        request_event_id: resolved,
        ..EventMeta::NONE
    };
    crate::engine::thread_events::emit_response_aborted(
        &bus,
        thread_id,
        AbortCause::EngineShutdown,
        String::new(),
        vec![],
        None,
        None,
        EventMeta {
            actor: Some(MessageOrigin::Device {
                device_id: "dev-1".into(),
                label: "Test Device".into(),
            }),
            ..turn_meta.clone()
        },
        "[Test] teardown abort",
    )
    .await;
    crate::engine::thread_events::emit_response_canceled(
        &bus,
        &pool,
        thread_id,
        CancelCause::UserStop,
        String::new(),
        vec![],
        None,
        None,
        EventMeta {
            request_event_id: Some(in_flight),
            ..EventMeta::NONE
        },
        "[Test] loop cancel arm",
    )
    .await;

    assert_eq!(
        count_of(&pool, thread_id, "ResponseAborted").await,
        1,
        "the teardown abort is the one terminator for this turn"
    );
    assert_eq!(
        count_of(&pool, thread_id, "ResponseCanceled").await,
        0,
        "no phantom 'Response canceled' boundary: the gate in \
         emit_response_canceled must recognise the abort by request id"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

// ---------------------------------------------------------------------------
// Turns anchored on an event neither origin-type list names
// ---------------------------------------------------------------------------

/// A chat Continue anchors its turn on `ContinuationStarted`, which is absent
/// from `CHAT_ORIGINATING_EVENT_TYPES`. The stale `MessageReceived` left over
/// from the already-finished turn is what the query would return.
#[tokio::test]
async fn continuation_started_turn_keeps_its_own_anchor() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let threads = make_threads();
    let thread_id = Uuid::new_v4();

    let stale = emit_with_id(
        &bus,
        thread_id,
        message_received("the finished turn"),
        Uuid::new_v4(),
    )
    .await;
    let resumed = emit_with_id(
        &bus,
        thread_id,
        ThreadEvent::ContinuationStarted {
            branch: String::new(),
            origin: None,
            reason: Some("user_clicked_continue".into()),
        },
        Uuid::new_v4(),
    )
    .await;

    let (_token, _rx2, guard) = register(&threads, thread_id);
    record_anchor(&threads, thread_id, &guard, resumed);

    // Pin the delta: the fallback really would pick the stale message here.
    assert_eq!(
        crate::engine::agent_session::latest_originating_event_id(
            &pool,
            thread_id,
            crate::engine::agent_session::CHAT_ORIGINATING_EVENT_TYPES,
        )
        .await,
        Some(stale),
        "decoy check: ContinuationStarted is absent from the origin-type list, \
         so the query returns the already-finished turn's MessageReceived"
    );

    let resolved = crate::engine::in_flight_request_event_id(
        &threads,
        &pool,
        thread_id,
        crate::engine::agent_session::CHAT_ORIGINATING_EVENT_TYPES,
    )
    .await;
    assert_eq!(
        resolved,
        Some(resumed),
        "a Continue turn anchors on its own ContinuationStarted"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// The coding-agent counterpart: a resume anchors on `ContinuationRequested`,
/// absent from `CC_ORIGINATING_EVENT_TYPES` for the same reason.
#[tokio::test]
async fn continuation_requested_turn_keeps_its_own_anchor() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let threads = make_threads();
    let thread_id = Uuid::new_v4();

    // `ContinuationRequested` is coding-agent-only, so the thread has to be one
    // (the lifecycle validator rejects it on a chat thread).
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::SessionStarted {
            coding_agent: crate::runtime::CodingAgent::ClaudeCode,
            session_id: "sid-test".into(),
            branch: "claude-code/test".into(),
            repo_id: None,
            coding_agent_kind: Default::default(),
            coding_agent_folder: String::new(),
            app_id: None,
        },
        meta: EventMeta {
            channel: Some(crate::engine::thread_events::EventChannel::ClaudeCode),
            ..EventMeta::NONE
        },
    })
    .await
    .expect("SessionStarted must persist");

    let stale = emit_with_id(
        &bus,
        thread_id,
        message_received("the finished coding-agent turn"),
        Uuid::new_v4(),
    )
    .await;
    let resumed = emit_with_id(
        &bus,
        thread_id,
        ThreadEvent::ContinuationRequested {
            reason: "user_clicked_continue".into(),
        },
        Uuid::new_v4(),
    )
    .await;

    let (_token, _rx2, guard) = register(&threads, thread_id);
    record_anchor(&threads, thread_id, &guard, resumed);

    // Pin the delta: the fallback really would pick the stale message here.
    assert_eq!(
        crate::engine::agent_session::latest_originating_event_id(
            &pool,
            thread_id,
            crate::engine::agent_session::CC_ORIGINATING_EVENT_TYPES,
        )
        .await,
        Some(stale),
        "decoy check: ContinuationRequested is absent from the origin-type list"
    );

    let resolved = crate::engine::in_flight_request_event_id(
        &threads,
        &pool,
        thread_id,
        crate::engine::agent_session::CC_ORIGINATING_EVENT_TYPES,
    )
    .await;
    assert_eq!(
        resolved,
        Some(resumed),
        "a coding-agent resume anchors on its own ContinuationRequested"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

// ---------------------------------------------------------------------------
// The fallback
// ---------------------------------------------------------------------------

/// No live handle (boot-time recovery, or a turn evicted before it recorded
/// anything) still yields an anchor. A `NULL` `request_event_id` would break
/// `chat/rerun.rs`'s Continue window and the frontend's exchange grouping, so
/// the guess is kept rather than dropped.
#[tokio::test]
async fn falls_back_to_the_origin_type_query_without_a_handle() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let threads = make_threads();
    let thread_id = Uuid::new_v4();

    emit_with_id(&bus, thread_id, message_received("older"), Uuid::new_v4()).await;
    let newest = emit_with_id(&bus, thread_id, message_received("newest"), Uuid::new_v4()).await;

    let resolved = crate::engine::in_flight_request_event_id(
        &threads,
        &pool,
        thread_id,
        crate::engine::agent_session::CHAT_ORIGINATING_EVENT_TYPES,
    )
    .await;
    assert_eq!(
        resolved,
        Some(newest),
        "with no handle the resolver falls back to latest_originating_event_id"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// A registered handle that has not recorded its anchor yet (the window between
/// `register_thread_queued` and the turn resolving its originating event) also
/// falls through to the query rather than returning `None`.
#[tokio::test]
async fn falls_back_when_the_handle_has_not_recorded_yet() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let threads = make_threads();
    let thread_id = Uuid::new_v4();

    let only = emit_with_id(&bus, thread_id, message_received("hi"), Uuid::new_v4()).await;
    let (_token, _rx2, _guard) = register(&threads, thread_id);

    let resolved = crate::engine::in_flight_request_event_id(
        &threads,
        &pool,
        thread_id,
        crate::engine::agent_session::CHAT_ORIGINATING_EVENT_TYPES,
    )
    .await;
    assert_eq!(resolved, Some(only));

    pool.close().await;
    teardown_test_db(&db_name).await;
}

// ---------------------------------------------------------------------------
// Lifetime of the recorded anchor (no DB needed)
// ---------------------------------------------------------------------------

/// The handle IS the turn, so dropping the guard clears the anchor with it.
/// Nothing has to remember to reset it.
#[test]
fn dropping_the_guard_clears_the_anchor() {
    let threads = make_threads();
    let thread_id = Uuid::new_v4();
    let anchor = Uuid::new_v4();

    let (_token, _rx, guard) = register(&threads, thread_id);
    record_anchor(&threads, thread_id, &guard, anchor);
    assert_eq!(read_anchor(&threads, thread_id), Some(anchor));

    drop(guard);
    assert_eq!(
        read_anchor(&threads, thread_id),
        None,
        "the guard removes the handle, and the anchor goes with it"
    );
}

/// A turn force-evicted after the 60 s timeout keeps unwinding while its
/// replacement is already registered under the same `thread_id`. The dying
/// turn's late write must not overwrite the live turn's anchor, or the next
/// abort terminates the turn the user already abandoned. Mirrors the generation
/// filter in `ThreadGuard::drop` and `note_injections_drained`.
#[test]
fn a_stale_generation_cannot_overwrite_the_live_anchor() {
    let threads = make_threads();
    let thread_id = Uuid::new_v4();

    let (_token, _rx, evicted_guard) = register(&threads, thread_id);

    // Force-evict and re-register, as `register_thread_queued` does.
    threads.lock().unwrap().remove(&thread_id);
    let (_token2, _rx2, live_guard) = register(&threads, thread_id);
    let live_anchor = Uuid::new_v4();
    record_anchor(&threads, thread_id, &live_guard, live_anchor);

    // The evicted turn, still unwinding, tries to stamp its own anchor.
    record_anchor(&threads, thread_id, &evicted_guard, Uuid::new_v4());

    assert_eq!(
        read_anchor(&threads, thread_id),
        Some(live_anchor),
        "the generation filter must reject the evicted turn's late write"
    );
}
