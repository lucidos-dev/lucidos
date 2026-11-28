use super::{emit_teardown_abort_unless_question_parked, TeardownBoundary};
use crate::engine::agent_question::aq_test_helpers::{
    count_response_aborted, emit_user_question, seed_cc_thread,
};
use crate::engine::agent_recovery::switch_was_user_initiated;
use crate::engine::event_bus::{BusEvent, EventBus};
use crate::engine::question_park_sql::thread_has_unanswered_question;
use crate::engine::thread_events::{
    AnswerKind, EventChannel, EventMeta, MessageOrigin, ThreadEvent,
};
use crate::test_support::{setup_test_db, teardown_test_db};
use uuid::Uuid;

fn device_actor() -> Option<MessageOrigin> {
    Some(MessageOrigin::Device {
        device_id: "dev-1".into(),
    })
}

fn gateway_restart_actor(device_id: &str) -> Option<MessageOrigin> {
    let headers = crate::api::gateway_stamp::test_support::stamped_device_headers(device_id);
    crate::api::actor::user_actor(&headers, None)
}

async fn thread_status(pool: &sqlx::PgPool, thread_id: Uuid) -> String {
    sqlx::query_scalar::<_, String>("SELECT status FROM thread_summaries WHERE thread_id = $1")
        .bind(thread_id)
        .fetch_one(pool)
        .await
        .expect("thread_summaries row")
}

#[tokio::test]
async fn teardown_skips_abort_for_question_parked_thread() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let thread_id = Uuid::new_v4();
    seed_cc_thread(&bus, thread_id).await;
    emit_user_question(&bus, thread_id, "toolu_teardown_1").await;

    let boundary = emit_teardown_abort_unless_question_parked(
        &pool,
        &bus,
        thread_id,
        Some(EventChannel::ClaudeCode),
        String::new(),
        None,
        device_actor(),
    )
    .await;

    assert_eq!(
        boundary,
        TeardownBoundary::Preserved,
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

#[tokio::test]
async fn teardown_skips_abort_for_question_parked_chat_thread() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let thread_id = Uuid::new_v4();
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::MessageReceived {
            provider: None,
            voice_session_id: None,
            wake_mode: None,
            text: "make a story".into(),
            user_image_hashes: vec![],
            device_id: None,
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
                preview: None,
                widget: None,
                quiet: false,
                quiet_reply: None,
            }],
            worktree_path: None,
            multi_select: false,
            owner_approval: None,
            permission_request: None,
        },
        meta: EventMeta::NONE,
    })
    .await
    .expect("UserQuestionAsked emit")
    .expect("UserQuestionAsked persisted");

    let boundary = emit_teardown_abort_unless_question_parked(
        &pool,
        &bus,
        thread_id,
        None,
        "This response was interrupted by an engine restart.".to_string(),
        None,
        device_actor(),
    )
    .await;

    assert_eq!(
        boundary,
        TeardownBoundary::Preserved,
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

#[tokio::test]
async fn teardown_emits_device_abort_for_working_thread() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let thread_id = Uuid::new_v4();
    seed_cc_thread(&bus, thread_id).await;

    let boundary = emit_teardown_abort_unless_question_parked(
        &pool,
        &bus,
        thread_id,
        Some(EventChannel::ClaudeCode),
        String::new(),
        None,
        device_actor(),
    )
    .await;

    assert_eq!(
        boundary,
        TeardownBoundary::Aborted,
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

#[tokio::test]
async fn a_boundary_abort_that_fails_to_persist_leaves_the_turn_owed() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let thread_id = Uuid::new_v4();
    seed_cc_thread(&bus, thread_id).await;
    pool.close().await;

    let boundary = emit_teardown_abort_unless_question_parked(
        &pool,
        &bus,
        thread_id,
        Some(EventChannel::ClaudeCode),
        String::new(),
        None,
        device_actor(),
    )
    .await;

    assert_eq!(boundary, TeardownBoundary::Unwritten);
    assert!(
        !boundary.settles_turn(),
        "the session's own shutdown abort must still be allowed to land"
    );

    teardown_test_db(&db_name).await;
}

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

    let boundary = emit_teardown_abort_unless_question_parked(
        &pool,
        &bus,
        thread_id,
        Some(EventChannel::ClaudeCode),
        String::new(),
        None,
        actor,
    )
    .await;

    assert_eq!(
        boundary,
        TeardownBoundary::Aborted,
        "a working thread gets the boundary abort"
    );
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

#[tokio::test]
async fn a_teardown_nobody_requested_stays_system_attributed_and_failed() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let thread_id = Uuid::new_v4();
    seed_cc_thread(&bus, thread_id).await;

    let boundary = emit_teardown_abort_unless_question_parked(
        &pool,
        &bus,
        thread_id,
        Some(EventChannel::ClaudeCode),
        String::new(),
        None,
        None,
    )
    .await;

    assert_eq!(
        boundary,
        TeardownBoundary::Aborted,
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
            compose: None,
            tool_use_id: "toolu_teardown_2".into(),
            quiet: false,
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

    let boundary = emit_teardown_abort_unless_question_parked(
        &pool,
        &bus,
        thread_id,
        Some(EventChannel::ClaudeCode),
        String::new(),
        None,
        device_actor(),
    )
    .await;

    assert_eq!(
        boundary,
        TeardownBoundary::Aborted,
        "an answered question is not a parked state — the mid-work turn aborts normally"
    );
    assert_eq!(count_response_aborted(&pool, thread_id).await, 1);

    pool.close().await;
    teardown_test_db(&db_name).await;
}
