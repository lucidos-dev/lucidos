//! Resume orchestration for CC's `AskUserQuestion`. The PreToolUse hook in
//! `lucidos-cli ask-user-question-hook` handles the question lifecycle inside
//! the live CC subprocess (see `crate::engine::cc_settings` and
//! `crate::api::internal::ask_user_question`). This module's job is the
//! answer-side: emit `UserQuestionAnswered` once the user picks, then wake
//! the blocked hook so it can return CC's protocol-required `tool_result`.

use std::collections::HashMap;
use std::sync::Arc;
use uuid::Uuid;

use crate::engine::agent_recovery::ANSWERED_AFTER_IDLE_REASON;
use crate::engine::event_bus::{BusEvent, EventBus};
use crate::engine::thread_events::{
    AnswerKind, EventChannel, EventMeta, MessageOrigin, ThreadEvent,
};
use crate::engine::{AgentSession, LucidosEngine};

/// Outcome of answering a pending question. Maps to HTTP status codes in the API layer.
#[derive(Debug)]
pub enum AnswerResult {
    /// Answer persisted; any waiting hook has been notified. The CC subprocess
    /// is already alive and continuing in its existing session.
    Resumed,
    /// No matching `UserQuestionAsked` for this `tool_use_id`, or already answered.
    Conflict(String),
}

/// Resolve any pending `UserQuestionAsked` for `thread_id` as `Canceled` so the
/// QuestionCard renders a "Canceled" badge instead of leaving stale answer
/// buttons. No-op when there's no pending question. Conflicts (rare race where
/// the user answered between lookup and emit) are logged and swallowed —
/// callers should not fail because of them.
pub async fn resolve_pending_question_as_canceled(
    engine: &Arc<LucidosEngine>,
    thread_id: Uuid,
    actor: Option<MessageOrigin>,
) {
    let Some(tool_use_id) = lookup_pending_question_tool_use_id(engine.pool(), thread_id).await
    else {
        return;
    };
    let result =
        answer_pending_question(engine, thread_id, tool_use_id, AnswerKind::Canceled, actor).await;
    if let AnswerResult::Conflict(msg) = result {
        log!(
            "[CCQuestion] resolve_pending_question_as_canceled({}): {}",
            thread_id,
            msg
        );
    }
}

/// Find the `tool_use_id` of the latest unresolved `UserQuestionAsked` for
/// `thread_id`, if any. One round-trip — the LEFT JOIN filters out questions
/// that already have a matching answer.
pub async fn lookup_pending_question_tool_use_id(
    pool: &sqlx::PgPool,
    thread_id: Uuid,
) -> Option<String> {
    let result: Result<Option<(String,)>, _> = sqlx::query_as(
        "SELECT q.payload->>'tool_use_id' \
         FROM events q \
         LEFT JOIN events a ON a.thread_id = q.thread_id \
              AND a.event_type = 'UserQuestionAnswered' \
              AND a.payload->>'tool_use_id' = q.payload->>'tool_use_id' \
         WHERE q.thread_id = $1 AND q.event_type = 'UserQuestionAsked' AND a.id IS NULL \
         ORDER BY q.sequence DESC LIMIT 1",
    )
    .bind(thread_id)
    .fetch_optional(pool)
    .await;
    let row = match result {
        Ok(r) => r,
        Err(e) => {
            // Don't silently treat a DB outage as "no pending question" —
            // that would let the user's free-form text spawn a brand-new CC
            // turn over the unanswered one.
            log!(
                "[CCQuestion] DB lookup failed for pending question on {}: {}",
                thread_id,
                e
            );
            return None;
        }
    };
    row.map(|(t,)| t).filter(|t| !t.is_empty())
}

/// Look up whether `(thread_id, tool_use_id)` has a `UserQuestionAsked` and
/// (if so) whether it's already been answered. Single round-trip.
/// Returns `None` when no question exists, `Some(true)` when answered,
/// `Some(false)` when pending.
async fn find_pending_question(
    engine: &LucidosEngine,
    thread_id: Uuid,
    tool_use_id: &str,
) -> Result<Option<bool>, sqlx::Error> {
    let row: Option<(bool,)> = sqlx::query_as(
        "SELECT EXISTS(\
             SELECT 1 FROM events a \
             WHERE a.thread_id = q.thread_id \
               AND a.event_type = 'UserQuestionAnswered' \
               AND a.payload->>'tool_use_id' = $2 \
         ) \
         FROM events q \
         WHERE q.thread_id = $1 AND q.event_type = 'UserQuestionAsked' \
           AND q.payload->>'tool_use_id' = $2 \
         ORDER BY q.sequence DESC LIMIT 1",
    )
    .bind(thread_id)
    .bind(tool_use_id)
    .fetch_optional(engine.pool())
    .await?;
    Ok(row.map(|(already_answered,)| already_answered))
}

/// Persist the user's answer and wake the PreToolUse hook blocked on this
/// `tool_use_id`. Idempotent: a partial unique index on
/// `(thread_id, tool_use_id) WHERE event_type='UserQuestionAnswered'` ensures
/// duplicate concurrent answers reject at the DB layer.
///
/// `actor` is the resolved `MessageOrigin` of whoever submitted the answer
/// (per CLAUDE.md "Mutating endpoints stamp the actor"). Engine-internal
/// callers — e.g. `archive_thread` synthesizing `AnswerKind::Canceled` —
/// pass the request actor too; only the resume side-effect uses `None`
/// because a resumed CC subprocess is engine-driven.
pub async fn answer_pending_question(
    engine: &Arc<LucidosEngine>,
    thread_id: Uuid,
    tool_use_id: String,
    answer: AnswerKind,
    actor: Option<MessageOrigin>,
) -> AnswerResult {
    match find_pending_question(engine, thread_id, &tool_use_id).await {
        Ok(Some(false)) => {}
        Ok(Some(true)) => {
            return AnswerResult::Conflict(format!(
                "Question {} on thread {} has already been answered",
                tool_use_id, thread_id
            ));
        }
        Ok(None) => {
            return AnswerResult::Conflict(format!(
                "No pending question for tool_use_id {} on thread {}",
                tool_use_id, thread_id
            ));
        }
        Err(e) => {
            log!(
                "[CCQuestion] DB lookup failed for {}/{}: {}",
                thread_id,
                tool_use_id,
                e
            );
            return AnswerResult::Conflict("Database lookup failed".into());
        }
    }

    if let Err(e) = engine
        .event_bus
        .emit(BusEvent::Thread {
            thread_id,
            event: ThreadEvent::UserQuestionAnswered {
                tool_use_id: tool_use_id.clone(),
                answer: answer.clone(),
            },
            meta: EventMeta {
                channel: Some(EventChannel::CodingAgent),
                actor: actor.clone(),
                ..EventMeta::NONE
            },
        })
        .await
    {
        let msg = e.to_string();
        if msg.contains("events_user_question_answered_unique") {
            return AnswerResult::Conflict(format!(
                "Question {} on thread {} was answered concurrently",
                tool_use_id, thread_id
            ));
        }
        log!(
            "[CCQuestion] Failed to emit UserQuestionAnswered for {}/{}: {}",
            thread_id,
            tool_use_id,
            e
        );
        return AnswerResult::Conflict(format!("Failed to persist answer: {}", e));
    }

    // CodingAgentPromptSent projects to a `success: null` Thinking step in
    // the timeline (frontend: thread-events.ts isThinking +
    // resolveLastPendingResponseStep), resolved by the next CC tool call or
    // text. Without it, the steps area sits empty during Anthropic's next
    // turn. Empty text skips the redundant thread_summaries status update —
    // UserQuestionAnswered above already sets it to Running.
    engine
        .event_bus
        .emit_or_log(
            BusEvent::Thread {
                thread_id,
                event: ThreadEvent::CodingAgentPromptSent {
                    text: String::new(),
                    agent: crate::runtime::AgentKind::ClaudeCode,
                    origin: actor.clone(),
                },
                meta: EventMeta {
                    channel: Some(EventChannel::CodingAgent),
                    actor: actor.clone(),
                    ..EventMeta::NONE
                },
            },
            "[CCQuestion] CodingAgentPromptSent (resume marker)",
        )
        .await;

    // Wake the blocked hook (if any). No-op if nothing is registered:
    // - Engine restart killed the hook; on resume the endpoint's crash-recovery
    //   path reads the just-persisted UserQuestionAnswered from the DB instead.
    // - User answered before the hook re-registered after a transient error;
    //   same crash-recovery path covers it.
    engine
        .question_wait_registry
        .notify(
            &tool_use_id,
            crate::engine::cc_question_wait::AnswerPayload {
                answers: serde_json::to_value(&answer).unwrap_or(serde_json::Value::Null),
            },
        )
        .await;

    ensure_resume_after_answer(
        &engine.event_bus,
        &engine.agent_sessions,
        thread_id,
        &answer,
        actor.clone(),
    )
    .await;

    AnswerResult::Resumed
}

/// If no live CC subprocess exists for `thread_id`, emit a `ContinueSignal`
/// so the spawn dispatcher boots a fresh subprocess via `--resume`. The new
/// subprocess re-runs the `AskUserQuestion` PreToolUse hook, whose
/// crash-recovery path reads the just-persisted `UserQuestionAnswered` from
/// the DB and lets CC continue the turn.
///
/// `AnswerKind::Canceled` is the engine-internal sentinel used by
/// `archive_thread` to resolve a pending question card before tearing the
/// thread down — resuming there would race the subsequent `cancel_agent`
/// call.
///
/// Returns `true` when `ContinueSignal` was emitted, `false` when a live
/// subprocess was found (`notify()` already woke the in-flight hook) or the
/// answer was a `Canceled` sentinel.
async fn ensure_resume_after_answer(
    event_bus: &EventBus,
    agent_sessions: &Arc<tokio::sync::Mutex<HashMap<Uuid, AgentSession>>>,
    thread_id: Uuid,
    answer: &AnswerKind,
    actor: Option<MessageOrigin>,
) -> bool {
    if matches!(answer, AnswerKind::Canceled) {
        return false;
    }
    let has_live_subprocess = {
        let sessions = agent_sessions.lock().await;
        sessions
            .get(&thread_id)
            .map(|s| !s.process_exited)
            .unwrap_or(false)
    };
    if has_live_subprocess {
        return false;
    }
    event_bus
        .emit_or_log(
            BusEvent::Thread {
                thread_id,
                event: ThreadEvent::ContinueSignal {
                    reason: ANSWERED_AFTER_IDLE_REASON.to_string(),
                },
                meta: EventMeta {
                    channel: Some(EventChannel::CodingAgent),
                    actor,
                    ..EventMeta::NONE
                },
            },
            "[CCQuestion] ContinueSignal (resume after idle answer)",
        )
        .await;
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::AgentUserInput;
    use crate::test_support::{setup_test_db, teardown_test_db};
    use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU32};

    fn cc_meta() -> EventMeta {
        EventMeta {
            channel: Some(EventChannel::CodingAgent),
            ..EventMeta::NONE
        }
    }

    fn make_session(process_exited: bool) -> AgentSession {
        let (msg_tx, _msg_rx) = tokio::sync::mpsc::unbounded_channel::<AgentUserInput>();
        let (control_tx, _control_rx) = tokio::sync::mpsc::unbounded_channel();
        AgentSession {
            msg_tx,
            is_waiting: !process_exited,
            has_changes: false,
            requires_restart: false,
            auto_apply: false,
            discard: false,
            cancel: Arc::new(tokio::sync::Notify::new()),
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
        }
    }

    async fn count_continue_signals(pool: &sqlx::PgPool, thread_id: Uuid) -> i64 {
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM events \
             WHERE aggregate_id = $1 AND event_type = 'ContinueSignal'",
        )
        .bind(thread_id.to_string())
        .fetch_one(pool)
        .await
        .expect("count query")
    }

    /// SessionStarted is the lifecycle precondition for any CC-channel event;
    /// the bus projection rejects ContinueSignal otherwise (mirrors the
    /// pattern in spawn_dispatcher_tests.rs::continue_signal_produces_spawn_request).
    async fn seed_cc_thread(bus: &EventBus, thread_id: Uuid) {
        bus.emit(BusEvent::Thread {
            thread_id,
            event: ThreadEvent::SessionStarted {
                session_id: "sid-test".into(),
                branch: "claude-code/test".into(),
                repo_id: None,
            },
            meta: cc_meta(),
        })
        .await
        .expect("SessionStarted emit")
        .expect("SessionStarted persisted");
    }

    /// No `agent_sessions` entry means `notify()` cannot reach a hook; the
    /// answer would silently strand without a `ContinueSignal`.
    #[tokio::test]
    async fn ensure_resume_emits_continue_signal_when_no_live_session() {
        let (pool, db_name) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());

        let thread_id = Uuid::new_v4();
        seed_cc_thread(&bus, thread_id).await;
        let sessions = Arc::new(tokio::sync::Mutex::new(HashMap::new()));

        let emitted = ensure_resume_after_answer(
            &bus,
            &sessions,
            thread_id,
            &AnswerKind::FreeText { text: "Y".into() },
            None,
        )
        .await;
        assert!(
            emitted,
            "must emit ContinueSignal when agent_sessions has no entry"
        );
        assert_eq!(count_continue_signals(&pool, thread_id).await, 1);

        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    /// `process_exited == true` means the hook went down with the
    /// subprocess; `notify()` can't wake it, so we still need a Continue spawn.
    #[tokio::test]
    async fn ensure_resume_emits_continue_signal_when_session_exited() {
        let (pool, db_name) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());

        let thread_id = Uuid::new_v4();
        seed_cc_thread(&bus, thread_id).await;
        let mut map = HashMap::new();
        map.insert(thread_id, make_session(true));
        let sessions = Arc::new(tokio::sync::Mutex::new(map));

        let emitted = ensure_resume_after_answer(
            &bus,
            &sessions,
            thread_id,
            &AnswerKind::FreeText { text: "Y".into() },
            None,
        )
        .await;
        assert!(
            emitted,
            "must emit ContinueSignal when session exists but its subprocess has exited"
        );
        assert_eq!(count_continue_signals(&pool, thread_id).await, 1);

        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    /// Live subprocess: `notify()` already woke the in-flight hook. A
    /// `ContinueSignal` would race that and could spawn a duplicate.
    #[tokio::test]
    async fn ensure_resume_skips_emit_when_session_is_alive() {
        let (pool, db_name) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());

        let thread_id = Uuid::new_v4();
        seed_cc_thread(&bus, thread_id).await;
        let mut map = HashMap::new();
        map.insert(thread_id, make_session(false));
        let sessions = Arc::new(tokio::sync::Mutex::new(map));

        let emitted = ensure_resume_after_answer(
            &bus,
            &sessions,
            thread_id,
            &AnswerKind::FreeText { text: "Y".into() },
            None,
        )
        .await;
        assert!(
            !emitted,
            "must NOT emit ContinueSignal when subprocess is alive"
        );
        assert_eq!(count_continue_signals(&pool, thread_id).await, 0);

        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    /// `archive_thread` calls `answer_pending_question(.., Canceled)` to
    /// resolve the question card right before `cancel_agent`.
    /// A `ContinueSignal` here would race the imminent SessionEnded and
    /// spawn a fresh subprocess for a thread the user just archived.
    #[tokio::test]
    async fn ensure_resume_skips_emit_for_canceled_answer() {
        let (pool, db_name) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());

        let thread_id = Uuid::new_v4();
        seed_cc_thread(&bus, thread_id).await;
        let sessions = Arc::new(tokio::sync::Mutex::new(HashMap::new()));

        let emitted =
            ensure_resume_after_answer(&bus, &sessions, thread_id, &AnswerKind::Canceled, None)
                .await;
        assert!(
            !emitted,
            "Canceled is the archive sentinel and must never spawn a Continue"
        );
        assert_eq!(count_continue_signals(&pool, thread_id).await, 0);

        pool.close().await;
        teardown_test_db(&db_name).await;
    }
}
