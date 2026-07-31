//! Tests for the teardown boundary emit — the per-thread body of
//! `abort_in_flight_for_restart`'s coding-agent bucket
//! (`emit_cc_teardown_abort_unless_question_parked`).
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
/// "chat Restarted" screenshot.
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
