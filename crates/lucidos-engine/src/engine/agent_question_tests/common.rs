//! Shared test helpers for the agent_question test suites.

use super::*;
use crate::engine::AgentUserInput;
use crate::test_support::{setup_test_db, teardown_test_db};
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicI64, AtomicU32};
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

pub(crate) fn make_session(process_exited: bool) -> AgentSession {
    let (msg_tx, _msg_rx) = tokio::sync::mpsc::unbounded_channel::<AgentUserInput>();
    let (control_tx, _control_rx) = tokio::sync::mpsc::unbounded_channel();
    AgentSession {
        msg_tx,
        is_waiting: !process_exited,
        has_changes: false,
        requires_restart: false,
        pending_stop: None,
        cancel_actor: None,
        stop: Arc::new(tokio::sync::Notify::new()),
        interrupt: Arc::new(tokio::sync::Notify::new()),
        idle_notify: Arc::new(tokio::sync::Notify::new()),
        apply_now_in_progress: false,
        process_exited,
        worktree_path: None,
        branch_name: None,
        repo_root: None,
        cc_session_id: None,
        shutting_down: Arc::new(AtomicBool::new(false)),
        external_terminal_emitted: Arc::new(AtomicBool::new(false)),
        control_tx,
        builtin_commands: vec![],
        skill_commands: vec![],
        current_model: None,
        current_reasoning_effort: None,
        last_event_at: Arc::new(AtomicI64::new(0)),
        pending_followups: Arc::new(AtomicU32::new(0)),
        tools_in_flight: Arc::new(AtomicI32::new(0)),
    }
}

pub(crate) async fn count_continuation_requests(pool: &sqlx::PgPool, thread_id: Uuid) -> i64 {
    sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM events \
         WHERE aggregate_id = $1 AND event_type = 'ContinuationRequested'",
    )
    .bind(thread_id.to_string())
    .fetch_one(pool)
    .await
    .expect("count query")
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

pub(crate) async fn count_coding_agent_prompt_sent(pool: &sqlx::PgPool, thread_id: Uuid) -> i64 {
    sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM events \
         WHERE aggregate_id = $1 AND event_type = 'CodingAgentPromptSent'",
    )
    .bind(thread_id.to_string())
    .fetch_one(pool)
    .await
    .expect("count query")
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

