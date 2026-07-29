//! Shared test helpers for the agent_question test suites.

use super::*;
use crate::engine::AgentUserInput;
use crate::test_support::{setup_test_db, teardown_test_db};
use uuid::Uuid;

pub(crate) fn opt(id: &str, label: &str) -> QuestionOption {
    QuestionOption {
        id: id.into(),
        label: label.into(),
        description: None,
    }
}

pub(crate) fn cc_meta() -> EventMeta {
    EventMeta {
        channel: Some(EventChannel::ClaudeCode),
        ..EventMeta::NONE
    }
}

/// Returns the receiver alongside the session: hold it (`let (s, _rx) = …`) for
/// the test's lifetime, or the session reads as a phantom and every
/// `is_live()` check treats it as dead. See `AgentSession::is_live`.
pub(crate) fn make_session(
    process_exited: bool,
) -> (
    AgentSession,
    tokio::sync::mpsc::UnboundedReceiver<AgentUserInput>,
) {
    let (mut session, msg_rx) = AgentSession::for_test();
    session.is_waiting = !process_exited;
    session.process_exited = process_exited;
    (session, msg_rx)
}

pub(crate) async fn count_continuation_requests(pool: &sqlx::PgPool, thread_id: Uuid) -> i64 {
    count_events_of_type(pool, thread_id, "ContinuationRequested").await
}

pub(crate) async fn count_response_aborted(pool: &sqlx::PgPool, thread_id: Uuid) -> i64 {
    count_events_of_type(pool, thread_id, "ResponseAborted").await
}

/// SessionStarted is the lifecycle precondition for any CC-channel event;
/// the bus projection rejects ContinuationRequested otherwise (mirrors the
/// pattern in spawn_dispatcher_tests.rs::continuation_requested_produces_spawn_request).
pub(crate) async fn seed_cc_thread(bus: &EventBus, thread_id: Uuid) {
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
        meta: cc_meta(),
    })
    .await
    .expect("SessionStarted emit")
    .expect("SessionStarted persisted");
}

pub(crate) async fn count_events_of_type(
    pool: &sqlx::PgPool,
    thread_id: Uuid,
    event_type: &str,
) -> i64 {
    sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM events WHERE aggregate_id = $1 AND event_type = $2",
    )
    .bind(thread_id.to_string())
    .bind(event_type)
    .fetch_one(pool)
    .await
    .expect("count query")
}

/// Bootstrap a chat thread the way the agentic loop does — `MessageReceived`
/// (chat threads bootstrap on it; `SessionStarted` is CC-only per the lifecycle
/// validator) — and return its event id, which IS the turn's
/// `request_event_id`.
pub(crate) async fn seed_chat_thread(bus: &EventBus, thread_id: Uuid, text: &str) -> Uuid {
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::MessageReceived {
            text: text.into(),
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
        meta: EventMeta {
            channel: Some(EventChannel::Chat),
            ..EventMeta::NONE
        },
    })
    .await
    .expect("MessageReceived emit")
    .expect("MessageReceived persisted")
    .event_id
}

/// The chat agentic loop's `ToolCalled{ask_user_question}` — emitted right
/// before the loop blocks on the wait registry, and stamped with the turn's
/// `request_event_id`. `turn_request_event_id: None` reproduces a legacy row
/// from before the loop stamped the field.
pub(crate) async fn emit_ask_user_question_call(
    bus: &EventBus,
    thread_id: Uuid,
    turn_request_event_id: Option<Uuid>,
) -> Uuid {
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ToolCalled {
            name: crate::llm::tool_names::ASK_USER_QUESTION.to_string(),
            args: serde_json::json!({ "questions": questions() }),
            description: "Executing ask_user_question...".into(),
        },
        meta: EventMeta {
            channel: Some(EventChannel::Chat),
            request_event_id: turn_request_event_id,
            ..EventMeta::NONE
        },
    })
    .await
    .expect("ToolCalled emit")
    .expect("ToolCalled persisted")
    .event_id
}

pub(crate) async fn count_coding_agent_prompt_sent(pool: &sqlx::PgPool, thread_id: Uuid) -> i64 {
    count_events_of_type(pool, thread_id, "CodingAgentPromptSent").await
}

pub(crate) async fn emit_user_question(bus: &EventBus, thread_id: Uuid, tool_use_id: &str) {
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::UserQuestionAsked {
            tool_use_id: tool_use_id.into(),
            cc_session_id: "sid-test".into(),
            question: "Pick one".into(),
            options: vec![opt("opt-0", "A")],
            worktree_path: None,
            multi_select: false,
        },
        meta: cc_meta(),
    })
    .await
    .expect("UserQuestionAsked emit")
    .expect("UserQuestionAsked persisted");
}

/// Drives both regression tests below. Seeds the question, emits the
/// caller's chosen terminator, and asserts that:
///   - the active-only lookup skips the orphaned question (so
///     `chat::process`'s FreeText fast-path doesn't consume the user's
///     next typed follow-up as a `FreeText` answer — without this the
///     typed text vanishes from the timeline);
///   - the broad lookup STILL returns it (so `archive_thread` and the
///     CC stop endpoint can still cancel-stamp the QuestionCard, which
///     otherwise leaves clickable answer buttons dangling on the
///     archived thread).
pub(crate) async fn assert_terminator_orphans_only_active_lookup<F, Fut>(tool_use_id: &str, emit_terminator: F)
where
    F: FnOnce(EventBus, Uuid) -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let thread_id = Uuid::new_v4();
    seed_cc_thread(&bus, thread_id).await;
    emit_user_question(&bus, thread_id, tool_use_id).await;
    emit_terminator(bus, thread_id).await;

    let active = lookup_active_question_tool_use_id(&pool, thread_id).await;
    assert!(
        active.is_none(),
        "orphaned question must not intercept follow-ups via active lookup, got {active:?}"
    );
    assert_eq!(
        lookup_pending_question_tool_use_id(&pool, thread_id)
            .await
            .as_deref(),
        Some(tool_use_id),
        "broad lookup must still surface the orphan so archive can cancel-stamp the card"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

pub(crate) fn questions() -> serde_json::Value {
    serde_json::json!([{
        "question": "Fav color?",
        "options": [
            {"label": "Red", "description": ""},
            {"label": "Blue", "description": ""}
        ]
    }])
}

pub(crate) fn three_questions() -> serde_json::Value {
    serde_json::json!([
        {
            "question": "Fav color?",
            "options": [{"label": "Red"}, {"label": "Blue"}],
        },
        {
            "question": "Fav animal?",
            "options": [{"label": "Cat"}, {"label": "Dog"}],
        },
        {
            "question": "Pick all toppings",
            "multiSelect": true,
            "options": [{"label": "Cheese"}, {"label": "Olives"}],
        },
    ])
}

