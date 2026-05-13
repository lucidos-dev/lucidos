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
    AnswerKind, EventChannel, EventMeta, MessageOrigin, QuestionOption, ThreadEvent,
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

const PENDING_QUESTION_SQL: &str = "SELECT q.payload->>'tool_use_id' \
     FROM events q \
     LEFT JOIN events a ON a.thread_id = q.thread_id \
          AND a.event_type = 'UserQuestionAnswered' \
          AND a.payload->>'tool_use_id' = q.payload->>'tool_use_id' \
     WHERE q.thread_id = $1 AND q.event_type = 'UserQuestionAsked' AND a.id IS NULL \
     ORDER BY q.sequence DESC LIMIT 1";

const ACTIVE_QUESTION_SQL: &str = "SELECT q.payload->>'tool_use_id' \
     FROM events q \
     LEFT JOIN events a ON a.thread_id = q.thread_id \
          AND a.event_type = 'UserQuestionAnswered' \
          AND a.payload->>'tool_use_id' = q.payload->>'tool_use_id' \
     WHERE q.thread_id = $1 AND q.event_type = 'UserQuestionAsked' AND a.id IS NULL \
       AND NOT EXISTS ( \
         SELECT 1 FROM events t \
         WHERE t.thread_id = q.thread_id \
           AND t.sequence > q.sequence \
           AND t.event_type = ANY($2::text[]) \
       ) \
     ORDER BY q.sequence DESC LIMIT 1";

fn unwrap_tool_use_id_row(
    result: Result<Option<(String,)>, sqlx::Error>,
    thread_id: Uuid,
) -> Option<String> {
    match result {
        Ok(row) => row.map(|(t,)| t).filter(|t| !t.is_empty()),
        Err(e) => {
            // Don't silently treat a DB outage as "no pending question" —
            // that would let the user's free-form text spawn a brand-new CC
            // turn over the unanswered one.
            log!(
                "[CCQuestion] DB lookup failed for pending question on {}: {}",
                thread_id,
                e
            );
            None
        }
    }
}

/// Find the `tool_use_id` of the latest unanswered `UserQuestionAsked` for
/// `thread_id`, if any. One round-trip — the LEFT JOIN filters out questions
/// that already have a matching answer.
///
/// "Unanswered" here means literally "no `UserQuestionAnswered` row exists".
/// A question whose surrounding turn was terminated (engine restart, cancel,
/// failure, idle) still counts — `archive_thread` and the CC stop endpoint
/// rely on this to cancel-stamp the QuestionCard so its answer buttons render
/// disabled rather than dangling clickable on an archived/stopped thread.
/// Use `lookup_active_question_tool_use_id` instead when you only want
/// questions whose turn is still in flight.
pub async fn lookup_pending_question_tool_use_id(
    pool: &sqlx::PgPool,
    thread_id: Uuid,
) -> Option<String> {
    let result = sqlx::query_as::<_, (String,)>(PENDING_QUESTION_SQL)
        .bind(thread_id)
        .fetch_optional(pool)
        .await;
    unwrap_tool_use_id_row(result, thread_id)
}

/// Same as `lookup_pending_question_tool_use_id`, but also excludes questions
/// whose surrounding turn was terminated (see
/// `ThreadEvent::QUESTION_ORPHANING_EVENT_TYPES`) before any answer landed.
///
/// Used by the chat::process FreeText fast-path: an engine restart while a
/// question was on-screen leaves the question "unanswered" forever, but
/// routing the user's next typed follow-up to it as a `FreeText` answer means
/// `MessageReceived` is never emitted and the typed message vanishes from
/// the timeline. The terminator filter prevents that — typed text after
/// `ResponseAborted`/`Canceled`/`Failed`/`CodingAgentIdled` starts a fresh
/// follow-up instead.
pub async fn lookup_active_question_tool_use_id(
    pool: &sqlx::PgPool,
    thread_id: Uuid,
) -> Option<String> {
    let result = sqlx::query_as::<_, (String,)>(ACTIVE_QUESTION_SQL)
        .bind(thread_id)
        .bind(ThreadEvent::QUESTION_ORPHANING_EVENT_TYPES)
        .fetch_optional(pool)
        .await;
    unwrap_tool_use_id_row(result, thread_id)
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

/// Validate a user-supplied answer against the question's option list and
/// `multi_select` flag. Pure function; the surrounding I/O lives in
/// `answer_pending_question`.
///
/// - `MultiSelected` requires at least one id OR non-empty `text`. Every id
///   must exist in `options`, and the question must be marked `multi_select`.
///   The `text` field carries freetext typed in the prompt textarea while the
///   question was on screen — the prompt-row Submit button folds it in.
/// - `Selected`/`FreeText`/`Canceled` are unrestricted here — the existing
///   pre-validation (option lookup on the hook side) covers their well-formedness.
pub(crate) fn validate_answer(
    answer: &AnswerKind,
    options: &[QuestionOption],
    multi_select: bool,
) -> Result<(), String> {
    let AnswerKind::MultiSelected { option_ids, text } = answer else {
        return Ok(());
    };
    let has_text = text.as_deref().is_some_and(|t| !t.is_empty());
    if option_ids.is_empty() && !has_text {
        return Err("MultiSelected requires at least one option_id or non-empty text".into());
    }
    if !multi_select {
        return Err("MultiSelected answer for single-select question".into());
    }
    let known: std::collections::HashSet<&str> =
        options.iter().map(|o| o.id.as_str()).collect();
    for id in option_ids {
        if !known.contains(id.as_str()) {
            return Err(format!("MultiSelected contains unknown option_id: {id}"));
        }
    }
    Ok(())
}

/// Look up `(options, multi_select)` for the most recent `UserQuestionAsked`
/// matching `tool_use_id` on `thread_id`. Returns the parsed pair so the
/// validator can run against the canonical persisted shape.
async fn lookup_question_options(
    pool: &sqlx::PgPool,
    thread_id: Uuid,
    tool_use_id: &str,
) -> Result<Option<(Vec<QuestionOption>, bool)>, sqlx::Error> {
    let row: Option<(serde_json::Value, Option<bool>)> = sqlx::query_as(
        "SELECT COALESCE(payload->'options', '[]'::jsonb), \
                (payload->>'multi_select')::bool \
         FROM events \
         WHERE thread_id = $1 AND event_type = 'UserQuestionAsked' \
           AND payload->>'tool_use_id' = $2 \
         ORDER BY sequence DESC LIMIT 1",
    )
    .bind(thread_id)
    .bind(tool_use_id)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|(opts_json, multi)| {
        let options: Vec<QuestionOption> =
            serde_json::from_value(opts_json).unwrap_or_default();
        (options, multi.unwrap_or(false))
    }))
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

    // Validate the answer shape against the persisted question. The pending
    // lookup above guarantees the question exists, so a `None` here is a
    // race we treat as a conflict for symmetry with the answered-already arm.
    match lookup_question_options(engine.pool(), thread_id, &tool_use_id).await {
        Ok(Some((options, multi_select))) => {
            if let Err(msg) = validate_answer(&answer, &options, multi_select) {
                return AnswerResult::Conflict(msg);
            }
        }
        Ok(None) => {
            return AnswerResult::Conflict(format!(
                "Question {} on thread {} disappeared before validation",
                tool_use_id, thread_id
            ));
        }
        Err(e) => {
            log!(
                "[CCQuestion] DB lookup for question options failed {}/{}: {}",
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
/// thread down — resuming there would race the subsequent `stop_agent`
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

    fn opt(id: &str, label: &str) -> QuestionOption {
        QuestionOption {
            id: id.into(),
            label: label.into(),
            description: None,
        }
    }

    #[test]
    fn validate_answer_accepts_selected_and_freetext_and_canceled() {
        let opts = vec![opt("opt-0", "A"), opt("opt-1", "B")];
        // Single-select question accepts Selected/FreeText/Canceled.
        assert!(validate_answer(
            &AnswerKind::Selected { option_id: "opt-0".into() },
            &opts,
            false
        )
        .is_ok());
        assert!(validate_answer(
            &AnswerKind::FreeText { text: "x".into() },
            &opts,
            false
        )
        .is_ok());
        assert!(validate_answer(&AnswerKind::Canceled, &opts, false).is_ok());

        // Multi-select question accepts the same fall-throughs (single Selected
        // is allowed — equivalent to MultiSelected with one id).
        assert!(validate_answer(
            &AnswerKind::Selected { option_id: "opt-0".into() },
            &opts,
            true
        )
        .is_ok());
        assert!(validate_answer(&AnswerKind::Canceled, &opts, true).is_ok());
    }

    #[test]
    fn validate_answer_accepts_multi_selected_with_known_ids() {
        let opts = vec![opt("opt-0", "A"), opt("opt-1", "B"), opt("opt-2", "C")];
        let answer = AnswerKind::MultiSelected {
            option_ids: vec!["opt-0".into(), "opt-2".into()],
            text: None,
        };
        assert!(validate_answer(&answer, &opts, true).is_ok());
    }

    #[test]
    fn validate_answer_accepts_multi_selected_with_only_text() {
        // Prompt-row Submit folds typed text into MultiSelected even when
        // no toggles are active. Empty option_ids + non-empty text is valid.
        let opts = vec![opt("opt-0", "A")];
        let answer = AnswerKind::MultiSelected {
            option_ids: vec![],
            text: Some("just text".into()),
        };
        assert!(validate_answer(&answer, &opts, true).is_ok());
    }

    #[test]
    fn validate_answer_accepts_multi_selected_with_ids_and_text() {
        let opts = vec![opt("opt-0", "A"), opt("opt-1", "B")];
        let answer = AnswerKind::MultiSelected {
            option_ids: vec!["opt-0".into()],
            text: Some("plus this".into()),
        };
        assert!(validate_answer(&answer, &opts, true).is_ok());
    }

    #[test]
    fn validate_answer_rejects_empty_multi_selected() {
        let opts = vec![opt("opt-0", "A")];
        let answer = AnswerKind::MultiSelected {
            option_ids: vec![],
            text: None,
        };
        let err = validate_answer(&answer, &opts, true).expect_err("must reject empty");
        assert!(
            err.contains("at least one"),
            "error must mention requirement; got {err:?}"
        );
    }

    #[test]
    fn validate_answer_rejects_multi_selected_with_only_empty_text() {
        // Empty string is still empty — must be rejected like no text at all.
        let opts = vec![opt("opt-0", "A")];
        let answer = AnswerKind::MultiSelected {
            option_ids: vec![],
            text: Some(String::new()),
        };
        assert!(validate_answer(&answer, &opts, true).is_err());
    }

    #[test]
    fn validate_answer_rejects_unknown_multi_selected_id() {
        let opts = vec![opt("opt-0", "A")];
        let answer = AnswerKind::MultiSelected {
            option_ids: vec!["opt-0".into(), "opt-99".into()],
            text: None,
        };
        let err = validate_answer(&answer, &opts, true).expect_err("must reject unknown id");
        assert!(
            err.contains("opt-99"),
            "error must surface the unknown id; got {err:?}"
        );
    }

    #[test]
    fn validate_answer_rejects_multi_selected_for_single_select_question() {
        let opts = vec![opt("opt-0", "A")];
        let answer = AnswerKind::MultiSelected {
            option_ids: vec!["opt-0".into()],
            text: None,
        };
        let err = validate_answer(&answer, &opts, false).expect_err("single-select rejects multi");
        assert!(
            err.contains("single-select"),
            "error must explain mismatch; got {err:?}"
        );
    }


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
            pending_stop: None,
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
    /// resolve the question card right before `stop_agent`.
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

    async fn emit_user_question(bus: &EventBus, thread_id: Uuid, tool_use_id: &str) {
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

    /// Baseline: an unanswered question with nothing after it is returned by
    /// both lookup variants — the turn is still in flight.
    #[tokio::test]
    async fn both_lookups_return_question_when_unanswered_and_no_terminator() {
        let (pool, db_name) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());

        let thread_id = Uuid::new_v4();
        seed_cc_thread(&bus, thread_id).await;
        emit_user_question(&bus, thread_id, "toolu-pending").await;

        assert_eq!(
            lookup_pending_question_tool_use_id(&pool, thread_id).await.as_deref(),
            Some("toolu-pending"),
            "broad lookup must return the live unanswered question"
        );
        assert_eq!(
            lookup_active_question_tool_use_id(&pool, thread_id).await.as_deref(),
            Some("toolu-pending"),
            "active-only lookup must return the live unanswered question"
        );

        pool.close().await;
        teardown_test_db(&db_name).await;
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
    async fn assert_terminator_orphans_only_active_lookup<F, Fut>(
        tool_use_id: &str,
        emit_terminator: F,
    ) where
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
            lookup_pending_question_tool_use_id(&pool, thread_id).await.as_deref(),
            Some(tool_use_id),
            "broad lookup must still surface the orphan so archive can cancel-stamp the card"
        );

        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    /// Engine-restart-style abort: the user's "Restarted" exchange in the UI
    /// is paired with this `ResponseAborted` row in the DB.
    #[tokio::test]
    async fn response_aborted_orphans_only_active_lookup() {
        assert_terminator_orphans_only_active_lookup(
            "toolu-orphaned-aborted",
            |bus, thread_id| async move {
                crate::engine::thread_events::emit_response_aborted(
                    &bus,
                    thread_id,
                    crate::engine::thread_events::AbortCause::EngineShutdown,
                    String::new(),
                    vec![],
                    None,
                    None,
                    cc_meta(),
                    "[test] engine_shutdown",
                )
                .await;
            },
        )
        .await;
    }

    /// `CodingAgentIdled` boundary: the synthetic idle the engine-restart
    /// sweep emits alongside the abort. Filtering on idled too means an
    /// unanswered question can't intercept follow-ups even if only the idle
    /// boundary made it to the DB.
    #[tokio::test]
    async fn coding_agent_idled_orphans_only_active_lookup() {
        assert_terminator_orphans_only_active_lookup("toolu-orphaned-idle", |bus, thread_id| async move {
            bus.emit(BusEvent::Thread {
                thread_id,
                event: ThreadEvent::CodingAgentIdled {
                    has_changes: false,
                    is_external_repo: false,
                    requires_restart: false,
                    cc_session_id: None,
                    agent: crate::runtime::AgentKind::ClaudeCode,
                    reason: Some("engine_restart_interrupt".into()),
                    worktree_path: None,
                    worktree_head_sha: None,
                },
                meta: cc_meta(),
            })
            .await
            .expect("CodingAgentIdled emit")
            .expect("CodingAgentIdled persisted");
        })
        .await;
    }
}
