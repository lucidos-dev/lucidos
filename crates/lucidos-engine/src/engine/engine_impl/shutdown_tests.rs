//! Tests for the teardown boundary emit — the per-thread body of
//! `abort_in_flight_for_restart`'s coding-agent bucket
//! (`emit_teardown_abort_unless_question_parked`).
//!
//! The invariant under test (decision 7 of
//! `docs/plans/2026-07-01-new-engine-version-switch-flow.md`): a coding-agent
//! thread parked on an unanswered question survives ANY restart as
//! `waiting_for_user_answer` with its card answerable — no `ResponseAborted`.
//! The session is mid-turn (subprocess blocked in the AskUserQuestion hook),
//! so `is_in_flight()` cannot filter it; the emit itself must skip. Regression:
//! the unconditional pre-emit landed a device-attributed abort on every user
//! switch, which both rendered "interrupted" over the live card and counted as
//! a terminal in recovery's `thread_has_unanswered_question`, defeating the
//! preserve guard.

use super::emit_teardown_abort_unless_question_parked;
use crate::engine::agent_question::aq_test_helpers::{
    count_response_aborted, emit_user_question, seed_cc_thread,
};
use crate::engine::agent_recovery::{switch_was_user_initiated, thread_has_unanswered_question};
use crate::engine::event_bus::{BusEvent, EventBus};
use crate::engine::thread_events::{
    AnswerKind, EventChannel, EventMeta, MessageOrigin, ThreadEvent,
};
use crate::test_support::{setup_test_db, teardown_test_db};
use uuid::Uuid;

fn device_actor() -> Option<MessageOrigin> {
    Some(MessageOrigin::Device {
        device_id: "dev-1".into(),
        label: "Test Device".into(),
    })
}

/// The actor a **gateway-initiated** restart produces, built the way the
/// restart-intent handler builds it: out of an `x-lucidos-device-id` header and
/// nothing else. The picker's Restart / Stop sends that header on its control
/// request, the gateway forwards it to `/api/v1/internal/restart-intent`, and
/// `user_actor_resolved` turns it into this.
///
/// Deliberately NOT the hand-built [`device_actor`] literal: the whole feature
/// hangs on the header actually resolving to a `Device`, so the test derives it
/// from the header exactly as production does.
fn gateway_restart_actor(device_id: &str) -> Option<MessageOrigin> {
    let mut headers = axum::http::HeaderMap::new();
    headers.insert(
        crate::api::actor::HEADER_DEVICE_ID,
        axum::http::HeaderValue::from_str(device_id).expect("valid header value"),
    );
    crate::api::actor::user_actor(&headers, None, None)
}

/// This thread's `thread_summaries.status` verdict.
async fn thread_status(pool: &sqlx::PgPool, thread_id: Uuid) -> String {
    sqlx::query_scalar::<_, String>("SELECT status FROM thread_summaries WHERE thread_id = $1")
        .bind(thread_id)
        .fetch_one(pool)
        .await
        .expect("thread_summaries row")
}

/// The regression case: a thread parked on an unanswered question gets NO
/// boundary abort at teardown, so recovery's preserve predicate still holds
/// on the next boot and the card stays answerable.
#[tokio::test]
async fn teardown_skips_abort_for_question_parked_thread() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let thread_id = Uuid::new_v4();
    seed_cc_thread(&bus, thread_id).await;
    emit_user_question(&bus, thread_id, "toolu_teardown_1").await;

    let emitted = emit_teardown_abort_unless_question_parked(
        &pool,
        &bus,
        thread_id,
        Some(EventChannel::ClaudeCode),
        String::new(),
        None,
        device_actor(),
    )
    .await;

    assert!(
        !emitted,
        "question-parked thread must be preserved, not aborted"
    );
    assert_eq!(
        count_response_aborted(&pool, thread_id).await,
        0,
        "no boundary ResponseAborted may land on a question-parked thread"
    );
    assert!(
        thread_has_unanswered_question(&pool, thread_id).await,
        "the preserve predicate must still hold after teardown — recovery \
         reads it on the next boot to skip the abort/idle pair"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Chat parity: a Lucidos-Agent (chat) thread parked on an unanswered question
/// is preserved too — the generalized teardown helper keys on the SAME shared
/// predicate regardless of channel, so `None` (chat bucket) skips the abort just
/// like the coding-agent bucket. This is the teardown side of the reproduced
/// "chat Paused by restart" screenshot.
#[tokio::test]
async fn teardown_skips_abort_for_question_parked_chat_thread() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let thread_id = Uuid::new_v4();
    // Chat thread: a human MessageReceived establishes the thread, then a
    // chat-channel question parks it.
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::MessageReceived {
            voice_session_id: None,
            text: "make a story".into(),
            user_image_hashes: vec![],
            device_id: None,
            device: None,
            image_description: None,
            parent_thread_id: None,
            spawning_event_id: None,
            mode: crate::engine::thread_events::ActorMode::Human,
            model: None,
            reasoning_effort: None,
            origin: None,
        },
        meta: EventMeta::NONE,
    })
    .await
    .expect("MessageReceived emit")
    .expect("MessageReceived persisted");
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::UserQuestionAsked {
            tool_use_id: "toolu_chat_parked".into(),
            cc_session_id: String::new(),
            question: "Which illustration?".into(),
            options: vec![crate::engine::thread_events::QuestionOption {
                id: "opt-0".into(),
                label: "Approve".into(),
                description: None,
            }],
            worktree_path: None,
            multi_select: false,
        },
        meta: EventMeta::NONE,
    })
    .await
    .expect("UserQuestionAsked emit")
    .expect("UserQuestionAsked persisted");

    let emitted = emit_teardown_abort_unless_question_parked(
        &pool,
        &bus,
        thread_id,
        None, // chat bucket
        "This response was interrupted by an engine restart.".to_string(),
        None,
        device_actor(),
    )
    .await;

    assert!(
        !emitted,
        "question-parked chat thread must be preserved, not aborted"
    );
    assert_eq!(
        count_response_aborted(&pool, thread_id).await,
        0,
        "no boundary ResponseAborted may land on a question-parked chat thread"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// A mid-turn thread with NO pending question keeps the existing behavior:
/// the device-attributed abort lands, and it is exactly what
/// `switch_was_user_initiated` reads as "clean user switch → auto-resume".
#[tokio::test]
async fn teardown_emits_device_abort_for_working_thread() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let thread_id = Uuid::new_v4();
    seed_cc_thread(&bus, thread_id).await;

    let emitted = emit_teardown_abort_unless_question_parked(
        &pool,
        &bus,
        thread_id,
        Some(EventChannel::ClaudeCode),
        String::new(),
        None,
        device_actor(),
    )
    .await;

    assert!(
        emitted,
        "a working (non-parked) thread gets the boundary abort"
    );
    assert_eq!(count_response_aborted(&pool, thread_id).await, 1);
    assert!(
        switch_was_user_initiated(&pool, thread_id).await,
        "the emitted abort must read as a user-initiated switch so recovery \
         auto-resumes the interrupted turn"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// THE PICKER-RESTART BUG (2026-08-07). A restart the user starts from the
/// gateway workspace picker must land the same teardown boundary as the
/// in-workspace *Switch to new version*: `paused`, "Paused by restart",
/// auto-resume. It used to land the crash shape (`failed`, "Response
/// interrupted", manual Continue) because the gateway signalled the engine
/// without telling it a human had asked.
///
/// This walks the whole chain the fix adds, in production's own order: the
/// device id the picker sends becomes an actor through `user_actor` (what the
/// restart-intent handler resolves), the actor reaches the teardown emit
/// (what `abort_in_flight_for_restart` does with the stash), and the persisted
/// event is then read back by BOTH consumers that decide what the user sees:
/// the resume gate and the status verdict. Every existing test here hand-builds
/// its `MessageOrigin::Device`, so none of them would notice the header failing
/// to resolve to one, which is the single point the whole feature turns on.
#[tokio::test]
async fn a_gateway_restart_carrying_a_device_lands_the_switch_boundary() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let thread_id = Uuid::new_v4();
    seed_cc_thread(&bus, thread_id).await;

    let actor = gateway_restart_actor("picker-device");
    assert!(
        matches!(actor, Some(MessageOrigin::Device { .. })),
        "the picker's device-id header must resolve to a Device actor: an Api \
         actor satisfies no half of the switch fingerprint"
    );

    let emitted = emit_teardown_abort_unless_question_parked(
        &pool,
        &bus,
        thread_id,
        Some(EventChannel::ClaudeCode),
        String::new(),
        None,
        actor,
    )
    .await;

    assert!(emitted, "a working thread gets the boundary abort");
    assert!(
        switch_was_user_initiated(&pool, thread_id).await,
        "a gateway-initiated restart must read as user-initiated, so recovery \
         auto-resumes the interrupted turn instead of parking it on Continue"
    );
    assert_eq!(
        thread_status(&pool, thread_id).await,
        "paused",
        "the verdict must be the promised-resume one, not the red 'failed' the \
         picker restart used to leave behind"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// The other side of the same boundary, and the thing the fix must NOT change.
/// A teardown nobody asked for through the control plane (a crash, `stop.sh`, a
/// bare external SIGUSR1, the gateway supervisor's own health respawn) reaches
/// this emit with no actor, and must still settle `failed` and keep the manual
/// Continue: work that may have crashed the engine can't be looped.
///
/// The gateway enforces this upstream by not notifying at all when no device
/// asked; this is the engine-side floor under that, pinned next to the positive
/// case so the two can't drift apart.
#[tokio::test]
async fn a_teardown_nobody_requested_stays_system_attributed_and_failed() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let thread_id = Uuid::new_v4();
    seed_cc_thread(&bus, thread_id).await;

    let emitted = emit_teardown_abort_unless_question_parked(
        &pool,
        &bus,
        thread_id,
        Some(EventChannel::ClaudeCode),
        String::new(),
        None,
        None,
    )
    .await;

    assert!(
        emitted,
        "the boundary abort still lands, it is just not a switch"
    );
    assert!(
        !switch_was_user_initiated(&pool, thread_id).await,
        "an actorless teardown must NOT auto-resume"
    );
    assert_eq!(
        thread_status(&pool, thread_id).await,
        "failed",
        "nobody promised to come back for this turn, so it belongs in the \
         needs-attention count with its Continue button"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// An ANSWERED question does not park the thread: CC continued working after
/// the answer, so a restart mid-work aborts + auto-resumes like any other
/// in-flight turn.
#[tokio::test]
async fn teardown_emits_abort_when_question_already_answered() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let thread_id = Uuid::new_v4();
    seed_cc_thread(&bus, thread_id).await;
    emit_user_question(&bus, thread_id, "toolu_teardown_2").await;
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::UserQuestionAnswered {
            tool_use_id: "toolu_teardown_2".into(),
            answer: AnswerKind::Selected {
                option_id: "opt-0".into(),
            },
        },
        meta: EventMeta {
            channel: Some(EventChannel::ClaudeCode),
            ..EventMeta::NONE
        },
    })
    .await
    .expect("UserQuestionAnswered emit")
    .expect("UserQuestionAnswered persisted");

    let emitted = emit_teardown_abort_unless_question_parked(
        &pool,
        &bus,
        thread_id,
        Some(EventChannel::ClaudeCode),
        String::new(),
        None,
        device_actor(),
    )
    .await;

    assert!(
        emitted,
        "an answered question is not a parked state — the mid-work turn aborts normally"
    );
    assert_eq!(count_response_aborted(&pool, thread_id).await, 1);

    pool.close().await;
    teardown_test_db(&db_name).await;
}
