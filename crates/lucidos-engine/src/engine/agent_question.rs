//! Question-lifecycle primitives shared by every `AskUserQuestion` channel:
//! CC's PreToolUse hook (`lucidos-cli ask-user-question-hook`, served by
//! `api::internal::ask_user_question`), Codex's `ask_user_question` MCP tool
//! (`lucidos mcp-permission-server`, same endpoint), and the chat agent's
//! `ask_user_question` tool
//! (`engine::agentic_loop_special_tool::handle_chat_ask_user_question`).
//! All flow through `walk_question_batch` here; all resolve via
//! `answer_pending_question` here. The HTTP / tool layers stay thin — they
//! only translate the outcome into their respective wire shapes.

use std::collections::HashMap;
use std::sync::Arc;
use uuid::Uuid;

use crate::engine::agent_recovery::ANSWERED_AFTER_IDLE_REASON;
use crate::engine::chat::rerun::ChatResumeAnchor;
use crate::engine::event_bus::{BusEvent, EventBus};
use crate::engine::thread_events::{
    AnswerKind, EventChannel, EventMeta, MessageOrigin, QuestionOption, ThreadEvent,
};
use crate::engine::{AgentSession, LucidosEngine};

/// Outcome of answering a pending question. Maps to HTTP status codes in the API layer.
#[derive(Debug)]
pub enum AnswerResult {
    /// Answer persisted; any waiting hook has been notified. The Claude Code subprocess
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
///
/// Returns `true` when a pending question was actually cancel-stamped (a
/// `UserQuestionAnswered(Canceled)` was emitted). The cancel/stop handlers fold
/// this into their `{"canceled": bool}` response so a Stop that only had a
/// question card to resolve still reports that it did something — the client
/// then keeps its optimistic state (the card resolution is the incoming event)
/// rather than treating the click as a stale no-op. A `Conflict` (already
/// answered) reports `false`: nothing new was emitted.
pub async fn resolve_pending_question_as_canceled(
    engine: &Arc<LucidosEngine>,
    thread_id: Uuid,
    actor: Option<MessageOrigin>,
) -> bool {
    let Some(tool_use_id) = lookup_pending_question_tool_use_id(engine.pool(), thread_id).await
    else {
        return false;
    };
    let result =
        answer_pending_question(engine, thread_id, tool_use_id, AnswerKind::Canceled, actor).await;
    match result {
        AnswerResult::Resumed => true,
        AnswerResult::Conflict(msg) => {
            log!(
                "[CCQuestion] resolve_pending_question_as_canceled({}): {}",
                thread_id,
                msg
            );
            false
        }
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
/// the agent has already moved past (terminal events, progression events, or
/// a replacement question — see `ThreadEvent::QUESTION_OVERTAKEN_EVENT_TYPES`)
/// before any answer landed.
///
/// Used by the chat::process FreeText fast-path. Two failure modes this
/// defends:
///   1. Engine restart while a question was on-screen leaves it "unanswered"
///      forever; routing the user's next typed follow-up to it as
///      `FreeText` means `MessageReceived` is never emitted and the typed
///      message vanishes from the timeline.
///   2. CC parallel-calls `AskUserQuestion` alongside other tool_uses in one
///      assistant message. The hook blocks the AskUserQuestion tool_use,
///      but sibling tool_uses dispatch and emit
///      `CodingAgent{TextStreamed,ToolCalled,ToolResult,…}` events.
///      The user types a comment minutes later — without the progression
///      filter, the comment is silently absorbed as a `FreeText` answer.
pub async fn lookup_active_question_tool_use_id(
    pool: &sqlx::PgPool,
    thread_id: Uuid,
) -> Option<String> {
    let result = sqlx::query_as::<_, (String,)>(ACTIVE_QUESTION_SQL)
        .bind(thread_id)
        .bind(ThreadEvent::QUESTION_OVERTAKEN_EVENT_TYPES)
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

/// Outcome of `walk_question_batch`. `answer_kinds` is the per-question
/// `AnswerKind` JSON (Selected / FreeText / MultiSelected / Canceled), in
/// the same order as the input questions. Callers feed it to
/// `build_hook_answers` to produce the joined `{question_text: label}` map.
#[derive(Debug)]
pub(crate) struct QuestionWalkOutcome {
    pub answer_kinds: Vec<serde_json::Value>,
}

/// Why a `walk_question_batch` call failed. Boxed because no caller today
/// branches on a variant — both consumers (`api::internal::ask_user_question`
/// and `agentic_loop_special_tool::handle_chat_ask_user_question`) only
/// format the error via `Display`. The only legitimate failure mode is
/// infrastructure (DB / bus / shutdown), which a tool retry won't fix.
/// Reintroduce a typed enum if a caller ever needs to branch.
pub(crate) type QuestionWalkError = Box<dyn std::error::Error + Send + Sync>;

impl LucidosEngine {
    /// Walk a batch of questions sequentially (one card on screen at a
    /// time), emit `UserQuestionAsked` for each, and block until each is
    /// answered. Crash-recovery fast-path: any prior `UserQuestionAnswered`
    /// from a pre-restart session is returned without re-emitting the
    /// `Asked`. Cancel short-circuits — the remaining questions get
    /// `UserQuestionAnswered { Canceled }` rows persisted so the next
    /// re-entry (e.g. CC's hook re-firing after restart) sees the cancel
    /// for every trailing sub_id and doesn't re-ask.
    ///
    /// `channel` is stamped onto every emitted `UserQuestionAsked` and on
    /// the trailing Canceled padding answers so `answer_pending_question`
    /// can branch on the originating channel — coding-agent threads (CC and
    /// Codex both ride `EventChannel::ClaudeCode`, the coding-agent channel)
    /// get the resume marker + `ContinuationRequested` spawn; chat threads
    /// skip both because the chat tool returns the answer directly as a
    /// tool result.
    pub(crate) async fn walk_question_batch(
        &self,
        thread_id: Uuid,
        outer_tool_use_id: &str,
        questions: &serde_json::Value,
        cc_session_id: String,
        channel: crate::engine::thread_events::EventChannel,
    ) -> Result<QuestionWalkOutcome, QuestionWalkError> {
        let parser_input = serde_json::json!({ "questions": questions });
        let parsed = crate::engine::agent_session::parse_ask_user_question_inputs(&parser_input);
        let total = parsed.len();
        if total == 0 {
            return Ok(QuestionWalkOutcome {
                answer_kinds: vec![],
            });
        }

        // Strict enforcement: every question MUST carry a non-empty
        // `question` field. The schema marks it `required`, but tool-call
        // schemas are advisory — the model can still omit it (that was the
        // "(no question text)" bug). Reject the batch up front — before any
        // `UserQuestionAsked` is persisted — so the caller surfaces a
        // tool-result error and the model re-asks with the full text filled
        // in, instead of the user seeing a blank card they can't act on. The
        // `header` chip-label is never accepted as a substitute.
        if let Some(idx) = parsed.iter().position(|q| q.question.is_empty()) {
            return Err(format!(
                "Question at index {idx} has no text — every question needs a non-empty \
                 `question` field (the `header` chip-label is not a substitute). Re-call \
                 ask_user_question with the full question text filled in."
            )
            .into());
        }

        let mut answer_kinds: Vec<serde_json::Value> = Vec::with_capacity(total);
        let mut first_canceled_index: Option<usize> = None;
        for (i, q) in parsed.into_iter().enumerate() {
            let sub_id = synth_question_id(outer_tool_use_id, i);

            // Register FIRST. If the lookup ran first and the user
            // answered between lookup and register, the broadcast send
            // would find no subscriber and we'd block forever on `recv`.
            // Registering first guarantees the wake either lands in the
            // channel buffer (caught by `recv`) or fires before we get
            // here (caught by the lookup below).
            let mut waiter = self.question_wait_registry.register(&sub_id).await;

            match lookup_existing_answer(self.pool(), thread_id, &sub_id).await {
                Ok(Some(prior)) => {
                    self.question_wait_registry.forget(&sub_id).await;
                    let canceled = is_canceled_answer(&prior);
                    answer_kinds.push(prior);
                    if canceled {
                        first_canceled_index = Some(i);
                        break;
                    }
                    continue;
                }
                Ok(None) => {}
                Err(e) => {
                    self.question_wait_registry.forget(&sub_id).await;
                    log!("[QuestionWalk] prior-answer lookup failed {thread_id}/{sub_id}: {e}");
                    return Err(format!("DB lookup for question state failed: {e}").into());
                }
            }

            let already_asked = match user_question_already_asked(self.pool(), thread_id, &sub_id)
                .await
            {
                Ok(b) => b,
                Err(e) => {
                    self.question_wait_registry.forget(&sub_id).await;
                    log!("[QuestionWalk] already-asked lookup failed {thread_id}/{sub_id}: {e}");
                    return Err(format!("DB lookup for question state failed: {e}").into());
                }
            };
            if !already_asked {
                if let Err(e) = self
                    .event_bus
                    .emit(BusEvent::Thread {
                        thread_id,
                        event: ThreadEvent::UserQuestionAsked {
                            tool_use_id: sub_id.clone(),
                            cc_session_id: cc_session_id.clone(),
                            question: q.question,
                            options: q.options,
                            worktree_path: None,
                            multi_select: q.multi_select,
                        },
                        meta: EventMeta {
                            channel: Some(channel),
                            ..EventMeta::NONE
                        },
                    })
                    .await
                {
                    self.question_wait_registry.forget(&sub_id).await;
                    log!("[QuestionWalk] emit UserQuestionAsked failed {thread_id}/{sub_id}: {e}");
                    return Err(format!("Failed to persist UserQuestionAsked: {e}").into());
                }
            }

            // Block until UserQuestionAnswered fires. No timeout — same as
            // MCP permission (the user is the rate-limiter).
            let payload = match waiter.recv().await {
                Ok(p) => p,
                Err(_) => {
                    self.question_wait_registry.forget(&sub_id).await;
                    return Err("wait registry channel closed".into());
                }
            };
            self.question_wait_registry.forget(&sub_id).await;

            let canceled = is_canceled_answer(&payload.answers);
            answer_kinds.push(payload.answers);
            if canceled {
                first_canceled_index = Some(i);
                break;
            }
        }

        // Persist Canceled markers for any sub_ids the loop short-circuited
        // past. Without this, an engine restart between the cancel and the
        // caller's next re-entry would re-fire the walk, the per-question
        // crash-recovery lookup would see no answer for the trailing
        // sub_ids, and we'd re-emit `UserQuestionAsked` for cards the user
        // already implicitly canceled.
        if let Some(canceled_at) = first_canceled_index {
            for j in (canceled_at + 1)..total {
                let remaining_sub = synth_question_id(outer_tool_use_id, j);
                self.event_bus
                    .emit_or_log(
                        BusEvent::Thread {
                            thread_id,
                            event: ThreadEvent::UserQuestionAnswered {
                                tool_use_id: remaining_sub,
                                answer: AnswerKind::Canceled,
                            },
                            meta: EventMeta {
                                channel: Some(channel),
                                ..EventMeta::NONE
                            },
                        },
                        "[QuestionWalk] cancel padding for remaining sub_id",
                    )
                    .await;
            }
        }

        Ok(QuestionWalkOutcome { answer_kinds })
    }
}

/// Read the `channel` field from the most recent `UserQuestionAsked` for
/// `(thread_id, tool_use_id)`. `Ok(None)` covers no matching row, NULL
/// channel, and unparseable channel string (legacy rows or a hand-edited
/// payload); callers must default such rows to today's CC behaviour for
/// back-compat. An unparseable non-NULL string also logs a warning so the
/// silent dispatch to CC is at least visible to operators.
pub(crate) async fn lookup_question_channel(
    pool: &sqlx::PgPool,
    thread_id: Uuid,
    tool_use_id: &str,
) -> Result<Option<crate::engine::thread_events::EventChannel>, sqlx::Error> {
    let row: Option<(Option<String>,)> = sqlx::query_as(
        "SELECT payload->>'channel' \
         FROM events \
         WHERE thread_id = $1 AND event_type = 'UserQuestionAsked' \
           AND payload->>'tool_use_id' = $2 \
         ORDER BY sequence DESC LIMIT 1",
    )
    .bind(thread_id)
    .bind(tool_use_id)
    .fetch_optional(pool)
    .await?;
    let Some((Some(channel_str),)) = row else {
        return Ok(None);
    };
    match serde_json::from_value::<crate::engine::thread_events::EventChannel>(
        serde_json::Value::String(channel_str.clone()),
    ) {
        Ok(channel) => Ok(Some(channel)),
        Err(_) => {
            log!(
                "[CCQuestion] UserQuestionAsked payload->>'channel' = {:?} did not parse as EventChannel for {}/{}; defaulting to CC resume behaviour",
                channel_str,
                thread_id,
                tool_use_id
            );
            Ok(None)
        }
    }
}

/// Decide whether to fire the CC-specific resume side-effects after
/// emitting `UserQuestionAnswered`. Today those side-effects are:
///
/// - emit an empty `CodingAgentPromptSent` so the timeline shows a Thinking
///   placeholder while CC processes the `tool_result`;
/// - call `ensure_resume_after_answer`, which may emit `ContinuationRequested` to
///   respawn the Claude Code subprocess with `--resume`.
///
/// The chat agent is in-process: its `ask_user_question` tool blocks on
/// `QuestionWaitRegistry`, gets woken by the notify path, and returns the
/// answer as the tool result on the same turn. It needs neither the marker
/// nor the resume spawn. Legacy rows without a channel default to today's
/// CC behaviour for back-compat.
pub(crate) fn should_emit_cc_resume_side_effects(
    channel: Option<crate::engine::thread_events::EventChannel>,
) -> bool {
    use crate::engine::thread_events::EventChannel;
    match channel {
        Some(EventChannel::ClaudeCode) | None => true,
        Some(EventChannel::Chat) | Some(EventChannel::Trigger) => false,
    }
}

/// Per-question `tool_use_id` derived from the outer batch id and the
/// question's 0-based index. CC sends one outer `tool_use_id` per
/// `AskUserQuestion` call regardless of how many questions are inside; the
/// engine renders them sequentially and needs a unique key per individual
/// question for the wait registry, the answered-already crash-recovery
/// lookup, and the `events_user_question_answered_unique` partial index.
pub(crate) fn synth_question_id(outer: &str, index: usize) -> String {
    format!("{outer}#q{index}")
}

/// True when an `AnswerKind` JSON object has `kind == "Canceled"`. Used by
/// `walk_question_batch` to short-circuit the multi-question loop.
pub(crate) fn is_canceled_answer(answer_kind: &serde_json::Value) -> bool {
    answer_kind.get("kind").and_then(|k| k.as_str()) == Some("Canceled")
}

/// Most recent `UserQuestionAnswered.answer` for `tool_use_id`, if any. DB
/// errors propagate so a transient failure isn't read as "no prior answer".
pub(crate) async fn lookup_existing_answer(
    pool: &sqlx::PgPool,
    thread_id: Uuid,
    tool_use_id: &str,
) -> Result<Option<serde_json::Value>, sqlx::Error> {
    let row = sqlx::query_scalar::<_, serde_json::Value>(
        "SELECT payload->'answer' FROM events
         WHERE thread_id = $1 AND event_type = 'UserQuestionAnswered'
           AND payload->>'tool_use_id' = $2
         LIMIT 1",
    )
    .bind(thread_id)
    .bind(tool_use_id)
    .fetch_optional(pool)
    .await?;
    Ok(row.filter(|v| !v.is_null()))
}

/// True iff a `UserQuestionAsked` already exists for this `tool_use_id` (used
/// to suppress duplicate emits on hook re-fire after an engine restart). DB
/// errors propagate so a transient failure isn't read as "no prior question".
pub(crate) async fn user_question_already_asked(
    pool: &sqlx::PgPool,
    thread_id: Uuid,
    tool_use_id: &str,
) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(
           SELECT 1 FROM events
           WHERE thread_id = $1 AND event_type = 'UserQuestionAsked'
             AND payload->>'tool_use_id' = $2
         )",
    )
    .bind(thread_id)
    .bind(tool_use_id)
    .fetch_one(pool)
    .await
}

/// Resolve a synthesized `opt-N` id to its label from the `question_index`-th
/// question's options. Falls back to the bare id on miss so the model sees a
/// recognizable error rather than a silent drop.
fn lookup_option_label(
    opt_id: &str,
    cc_questions: &serde_json::Value,
    question_index: usize,
) -> String {
    opt_id
        .strip_prefix("opt-")
        .and_then(|n| n.parse::<usize>().ok())
        .and_then(|idx| {
            cc_questions
                .as_array()
                .and_then(|arr| arr.get(question_index))
                .and_then(|q| q.get("options"))
                .and_then(|opts| opts.as_array())
                .and_then(|opts| opts.get(idx))
                .and_then(|opt| opt.get("label"))
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
        })
        .unwrap_or_else(|| opt_id.to_string())
}

/// `Canceled` produces a `(canceled)` marker rather than an empty string —
/// an empty answer causes CC's model to read the question as unanswered and
/// re-invoke the tool in a loop. `MultiSelected` joins resolved labels with
/// `", "` and appends any non-empty `text` (freetext typed in the prompt
/// textarea while the card was on screen) on the same separator.
fn answer_kind_to_hook_value(
    answer_kind: &serde_json::Value,
    cc_questions: &serde_json::Value,
    question_index: usize,
) -> serde_json::Value {
    let kind = answer_kind
        .get("kind")
        .and_then(|k| k.as_str())
        .unwrap_or("");
    match kind {
        "Selected" => {
            let opt_id = answer_kind
                .get("option_id")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            serde_json::Value::String(lookup_option_label(opt_id, cc_questions, question_index))
        }
        "MultiSelected" => {
            let mut parts: Vec<String> = answer_kind
                .get("option_ids")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str())
                        .map(|id| lookup_option_label(id, cc_questions, question_index))
                        .collect()
                })
                .unwrap_or_default();
            if let Some(text) = answer_kind.get("text").and_then(|v| v.as_str()) {
                if !text.is_empty() {
                    parts.push(text.to_string());
                }
            }
            serde_json::Value::String(parts.join(", "))
        }
        "FreeText" => answer_kind
            .get("text")
            .cloned()
            .unwrap_or(serde_json::Value::String(String::new())),
        "Canceled" => serde_json::Value::String("(canceled)".to_string()),
        _ => serde_json::Value::String(format!("(unknown answer kind: {})", kind)),
    }
}

/// Build the `{question_text: answer_label}` map both channels send back to
/// their LLM (CC via the hook tool result; chat via the tool result string).
/// A question with no collected answer (loop short-circuited on cancel)
/// surfaces as `(canceled)` so the model never reads it as "unanswered" and
/// re-invokes the tool. Duplicate question texts disambiguate with a
/// `" (#i)"` suffix — without it, a naive `Map::insert` would silently
/// overwrite the first answer.
pub(crate) fn build_hook_answers(
    answer_kinds: &[serde_json::Value],
    cc_questions: &serde_json::Value,
) -> serde_json::Value {
    let Some(arr) = cc_questions.as_array() else {
        return serde_json::Value::Object(serde_json::Map::new());
    };
    let mut map = serde_json::Map::with_capacity(arr.len());
    for (i, q) in arr.iter().enumerate() {
        // Key on the same `question` field the card renders, so the answer-map
        // key the LLM reads back matches the text the user saw. The
        // "(unknown question)" fallback is defensive only — walk_question_batch
        // already rejected any entry missing a `question`, so it's unreachable
        // in the normal flow.
        let raw_text = crate::engine::agent_session::question_text(q)
            .unwrap_or_else(|| "(unknown question)".to_string());
        let key = if map.contains_key(&raw_text) {
            crate::log!(
                "[AskUserQuestion] duplicate question text in tool input — disambiguating with suffix: {raw_text:?}"
            );
            format!("{raw_text} (#{})", i + 1)
        } else {
            raw_text
        };
        let value = match answer_kinds.get(i) {
            Some(ans) => answer_kind_to_hook_value(ans, cc_questions, i),
            None => serde_json::Value::String("(canceled)".to_string()),
        };
        map.insert(key, value);
    }
    serde_json::Value::Object(map)
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
    let known: std::collections::HashSet<&str> = options.iter().map(|o| o.id.as_str()).collect();
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
        let options: Vec<QuestionOption> = serde_json::from_value(opts_json).unwrap_or_default();
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
/// because a resumed Claude Code subprocess is engine-driven.
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

    // Read the originating question's channel so we can propagate it onto
    // the answer (so the persisted pair shares a wire-channel) and decide
    // whether to fire the CC-specific resume side-effects below. Legacy
    // rows without a channel field default to today's CC behaviour, both
    // here and in `should_emit_cc_resume_side_effects`.
    let original_channel =
        match lookup_question_channel(engine.pool(), thread_id, &tool_use_id).await {
            Ok(ch) => ch,
            Err(e) => {
                log!(
                    "[CCQuestion] DB lookup for question channel failed {}/{}: {}",
                    thread_id,
                    tool_use_id,
                    e
                );
                return AnswerResult::Conflict("Database lookup failed".into());
            }
        };
    let answer_channel = original_channel.unwrap_or(EventChannel::ClaudeCode);

    if let Err(e) = engine
        .event_bus
        .emit(BusEvent::Thread {
            thread_id,
            event: ThreadEvent::UserQuestionAnswered {
                tool_use_id: tool_use_id.clone(),
                answer: answer.clone(),
            },
            meta: EventMeta {
                channel: Some(answer_channel),
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

    // CC-specific resume side-effects: `CodingAgentPromptSent` (timeline
    // Thinking placeholder while CC processes the tool_result) and the
    // `ContinuationRequested` spawn (respawn Claude Code subprocess if no live one). The
    // chat agent is in-process — its `ask_user_question` tool blocks on
    // `QuestionWaitRegistry`, gets woken below, and returns the answer as
    // a tool_result on the same turn. Skip both for chat-channel answers.
    if !should_emit_cc_resume_side_effects(original_channel) {
        let had_waiter = notify_and_release_waiter(engine, &tool_use_id, &answer).await;
        // No live in-process loop means the engine restarted while the card was
        // on screen (the chat `ask_user_question` blocks the loop on the wait
        // registry; a restart drops it). Re-enter the loop so answering RESUMES
        // the thread — the chat parity of CC's `--resume`. With a live loop the
        // notify already woke the blocked tool, which returns the answer on the
        // same turn. `Canceled` is a teardown sentinel (archive/stop), never a
        // resume.
        if !had_waiter && !matches!(answer, AnswerKind::Canceled) {
            // Resume on the ORIGINATING channel — this branch runs for both
            // `Chat` and `Trigger` questions (both skip CC side effects), so a
            // restart-preserved trigger question must resume as `Trigger`, not
            // be misclassified as chat. `answer_channel` is the persisted
            // question's channel (Chat or Trigger here; the CC/None case took
            // the branch below).
            resume_chat_after_answer(
                engine,
                thread_id,
                &tool_use_id,
                answer_channel,
                actor.clone(),
            )
            .await;
        }
        return AnswerResult::Resumed;
    }

    let coding_agent = engine.thread_coding_agent(thread_id).await;
    emit_resume_marker_for_cc_answer(
        &engine.event_bus,
        thread_id,
        &answer,
        actor.clone(),
        coding_agent,
    )
    .await;

    // Arm the run-loop resume signal on a live subprocess BEFORE waking its hook.
    // The answer reaches CC through the PreToolUse hook (notify below), not via
    // `msg_tx`, so it never hits the `msg_rx` arm's `reset_per_turn_flags` — the
    // only place the run loop clears `emitted_terminal_event`. Without this the
    // continued turn's output is dropped as "post-terminal stragglers" (see
    // `AgentSession::question_resume_pending`). Set before notify so the flag is
    // visible before CC's first post-answer event reaches the loop.
    arm_question_resume_if_live(&engine.agent_sessions, thread_id, &answer).await;

    // Wake the blocked hook (if any). No-op if nothing is registered:
    // - Engine restart killed the hook; on resume the endpoint's crash-recovery
    //   path reads the just-persisted UserQuestionAnswered from the DB instead.
    // - User answered before the hook re-registered after a transient error;
    //   same crash-recovery path covers it.
    notify_and_release_waiter(engine, &tool_use_id, &answer).await;

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

/// Wake every waiter blocked on `tool_use_id`, then drop the registry
/// entry. The broadcast buffers the payload, so a waiter that registered
/// before the send still receives it after the forget; a waiter that
/// registers later hits the persisted `UserQuestionAnswered` via the
/// crash-recovery DB lookup instead. The forget is the registry's only
/// cleanup for waiters that died mid-question (agent killed while the card
/// was on screen — the dropped handler never reaches its own `forget`;
/// without this, every cancel-stamped question leaked one entry until
/// engine restart).
///
/// Returns whether a LIVE in-process waiter received the wake. The chat
/// answer path uses `false` (no live loop — the process restarted while the
/// card was on screen) to decide it must re-enter the agentic loop rather
/// than rely on the in-memory wake.
async fn notify_and_release_waiter(
    engine: &Arc<LucidosEngine>,
    tool_use_id: &str,
    answer: &AnswerKind,
) -> bool {
    let had_waiter = engine
        .question_wait_registry
        .notify(
            tool_use_id,
            crate::engine::cc_question_wait::AnswerPayload {
                answers: serde_json::to_value(answer).unwrap_or(serde_json::Value::Null),
            },
        )
        .await;
    engine.question_wait_registry.forget(tool_use_id).await;
    had_waiter
}

/// The `ask_user_question` tool call a restart interrupted — the most recent
/// one on the thread, read back out of the event store by
/// [`lookup_interrupted_ask`].
#[derive(Debug, sqlx::FromRow)]
pub(crate) struct InterruptedAsk {
    /// The `ToolCalled` event id. Pairs the synthetic `ToolResult` back to the
    /// call so the timeline (and the resume reconstruction) sees a complete
    /// tool_use/tool_result pair instead of a dangling call.
    #[sqlx(rename = "id")]
    pub(crate) call_event_id: Uuid,
    /// The `questions` array the tool was called with — `build_hook_answers`
    /// needs it to turn persisted `option_id`s back into labels.
    pub(crate) questions: serde_json::Value,
    /// The asking turn's `request_event_id` — the resume anchor. `None` only on
    /// legacy rows persisted before the loop stamped it.
    pub(crate) request_event_id: Option<Uuid>,
}

/// Most recent `ToolCalled{ask_user_question}` on `thread_id`, or `None` when
/// the thread has none (a coding-agent question never emits one — CC/Codex use
/// `CodingAgentToolCalled` — and neither does a hand-seeded legacy thread).
pub(crate) async fn lookup_interrupted_ask(
    pool: &sqlx::PgPool,
    thread_id: Uuid,
) -> Option<InterruptedAsk> {
    sqlx::query_as::<_, InterruptedAsk>(
        "SELECT id, \
                COALESCE(payload->'args'->'questions', '[]'::jsonb) AS questions, \
                NULLIF(payload->>'request_event_id','')::uuid AS request_event_id \
         FROM events \
         WHERE aggregate_id = $1 AND event_type = 'ToolCalled' \
           AND payload->>'name' = $2 \
         ORDER BY sequence DESC LIMIT 1",
    )
    .bind(thread_id.to_string())
    .bind(crate::llm::tool_names::ASK_USER_QUESTION)
    .fetch_optional(pool)
    .await
    .unwrap_or_else(|e| {
        log!("[CCQuestion] ask_user_question call lookup failed for {thread_id}: {e}");
        None
    })
}

/// Where the answer-driven resume anchors: on the turn that asked the question
/// (so the restart stays invisible), falling back to a fresh Continue boundary
/// only when there is no turn to continue — a legacy `ToolCalled` with no
/// `request_event_id`, or no `ToolCalled` at all. Without an anchor the
/// resumed events would carry no `request_event_id` and strand outside every
/// exchange, which is worse than a redundant boundary panel.
pub(crate) fn resume_anchor_for_ask(
    ask: Option<&InterruptedAsk>,
    thread_id: Uuid,
) -> ChatResumeAnchor {
    match ask.and_then(|a| a.request_event_id) {
        Some(request_event_id) => ChatResumeAnchor::ExistingTurn(request_event_id),
        None => {
            log!(
                "[CCQuestion] no interrupted turn to continue for thread {} — resuming with a Continue boundary",
                thread_id
            );
            ChatResumeAnchor::NewBoundary
        }
    }
}

/// Re-enter a chat thread's agentic loop after its `ask_user_question` was
/// answered with NO live in-process loop — the loop died on an engine restart
/// while the card was on screen. This is the chat parity of the coding-agent
/// resume: CC re-fires its hook via `--resume` and reads the persisted answer;
/// chat has no subprocess, so we reconstruct the tool pair and re-run the
/// agentic loop as a continuation.
///
/// Emits the `ToolResult{ask_user_question}` the dead loop never got to emit —
/// pairing the dangling `ToolCalled` with the REAL answer (built from the
/// persisted sub-answers via `build_hook_answers`), so the resumed turn's
/// reconstruction shows the answer instead of the "[tool result unavailable:
/// orphaned]" stub — then spawns the continuation with a short engine note as
/// the current turn (reusing `spawn_chat_resume`, shared with `continue_chat`).
///
/// The resume is anchored on the interrupted turn's OWN `request_event_id`
/// ([`ChatResumeAnchor::ExistingTurn`], read off the originating
/// `ToolCalled{ask_user_question}`), so it emits **no** `ContinuationStarted` /
/// `UserPromptInjected` boundary: this thread was never aborted (the restart
/// preserve guard, `agent_recovery::thread_has_unanswered_question`, is what
/// kept the card live) and the user already did the one thing that was needed —
/// they answered. Opening the manual-Continue boundary here mislabelled the
/// resume as "Continued the response" and surfaced a stale "Reminded the model
/// that no actions had completed" note under an answered card. Anchoring on the
/// original turn makes the resumed reply group under the question card exactly
/// as it would have without the restart. A thread that genuinely still needs
/// user action keeps its abort + Continue button, and Continue still emits the
/// boundary + reminder ([`ChatResumeAnchor::NewBoundary`]).
///
/// The multi-question walk asks one card at a time, so on a mid-batch restart
/// only the asked-so-far sub-answers exist; `build_hook_answers` fills any
/// not-yet-asked question with `(canceled)`, and the LLM may re-ask — an
/// accepted degradation for that rare case (single-question asks resume cleanly).
async fn resume_chat_after_answer(
    engine: &Arc<LucidosEngine>,
    thread_id: Uuid,
    sub_tool_use_id: &str,
    // The originating question's channel (`Chat` or `Trigger`) — stamped on the
    // reconstructed ToolResult and the continuation so a trigger thread's resume
    // isn't misclassified as chat.
    channel: EventChannel,
    actor: Option<MessageOrigin>,
) {
    const RESUME_NOTE: &str = "[Engine note — resumed after restart] Your \
        ask_user_question was answered while the engine was restarting; its \
        result is in the tool result above. Continue the turn — do not ask the \
        same question again.";

    let ask = lookup_interrupted_ask(engine.pool(), thread_id).await;
    let anchor = resume_anchor_for_ask(ask.as_ref(), thread_id);

    if let Some(InterruptedAsk {
        call_event_id,
        questions,
        request_event_id,
    }) = ask
    {
        // Gather the persisted sub-answers in index order (`{outer}#q{i}`), so a
        // multi-question batch rebuilds its full answer map.
        let outer = sub_tool_use_id
            .rsplit_once("#q")
            .map(|(o, _)| o)
            .unwrap_or(sub_tool_use_id);
        let mut answer_kinds: Vec<serde_json::Value> = Vec::new();
        let mut i = 0usize;
        while let Ok(Some(a)) =
            lookup_existing_answer(engine.pool(), thread_id, &synth_question_id(outer, i)).await
        {
            answer_kinds.push(a);
            i += 1;
        }
        let answers_map = build_hook_answers(&answer_kinds, &questions);
        let result_str =
            serde_json::to_string(&answers_map).unwrap_or_else(|_| answers_map.to_string());

        engine
            .event_bus
            .emit_or_log(
                BusEvent::Thread {
                    thread_id,
                    event: ThreadEvent::ToolResult {
                        name: crate::llm::tool_names::ASK_USER_QUESTION.to_string(),
                        result: result_str,
                        images: vec![],
                        success: true,
                        // Pair with the originating ToolCalled so the frontend
                        // groups it into the question's exchange (and the resume
                        // reconstruction pairs the tool_use with the real answer).
                        tool_called_event_id: Some(call_event_id),
                    },
                    meta: EventMeta {
                        channel: Some(channel),
                        // Same turn as the call it answers — a live emit carries
                        // this too, and it keeps the pair together if the
                        // `tool_called_event_id` route ever misses.
                        request_event_id,
                        ..EventMeta::NONE
                    },
                },
                "[CCQuestion] ToolResult (ask_user_question resume after restart)",
            )
            .await;
    }

    // `spawn_chat_resume` returns a boxed `dyn Future` (its concrete return type
    // is the type-erasure boundary that breaks the mutual-async-recursion cycle —
    // see its doc), so a plain `.await` here is Send-safe.
    if let Err(e) = engine
        .spawn_chat_resume(thread_id, RESUME_NOTE.to_string(), channel, actor, anchor)
        .await
    {
        log!(
            "[CCQuestion] chat resume-after-answer failed for thread {}: {}",
            thread_id,
            e
        );
    }
}

/// Emit the empty `CodingAgentPromptSent` "resume marker" that projects to a
/// `success: null` Thinking step in the timeline (frontend:
/// thread-events.ts isThinking + resolveLastPendingResponseStep), resolved
/// by the next CC tool call or text. Without it, the steps area sits empty
/// during Anthropic's next turn. Empty text skips the redundant
/// thread_summaries status update — `UserQuestionAnswered` already set it
/// to Running.
///
/// Skipped for `AnswerKind::Canceled`: the cancel-stamp path (HTTP
/// `claude_code_stop`, `archive_thread`) immediately follows with `stop_agent`,
/// and `ensure_resume_after_answer` short-circuits the spawn — no next CC
/// turn ever runs, so the marker would strand as an empty `Thinking ✓`
/// placeholder under the QuestionCard's own ✓ Cancel state. Returns `true`
/// when the marker was emitted, `false` when skipped.
pub(crate) async fn emit_resume_marker_for_cc_answer(
    bus: &EventBus,
    thread_id: Uuid,
    answer: &AnswerKind,
    actor: Option<MessageOrigin>,
    // Which backend is processing the answer — CC via its PreToolUse hook,
    // Codex via the `mcp__lucidos__ask_user_question` MCP tool. The marker
    // must stamp the real backend or a Codex thread's Thinking placeholder
    // would attribute the next turn to Claude Code.
    coding_agent: crate::runtime::CodingAgent,
) -> bool {
    if matches!(answer, AnswerKind::Canceled) {
        return false;
    }
    bus.emit_or_log(
        BusEvent::Thread {
            thread_id,
            event: ThreadEvent::CodingAgentPromptSent {
                text: String::new(),
                coding_agent,
                origin: actor.clone(),
            },
            meta: EventMeta {
                channel: Some(EventChannel::ClaudeCode),
                actor,
                ..EventMeta::NONE
            },
        },
        "[CCQuestion] CodingAgentPromptSent (resume marker)",
    )
    .await;
    true
}

/// If no live Claude Code subprocess exists for `thread_id`, emit a `ContinuationRequested`
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
/// Returns `true` when `ContinuationRequested` was emitted, `false` when a live
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
            .map(|s| s.is_live())
            .unwrap_or(false)
    };
    if has_live_subprocess {
        return false;
    }
    crate::engine::thread_events::emit_continuation_requested_or_log(
        event_bus,
        thread_id,
        ANSWERED_AFTER_IDLE_REASON,
        actor,
        "[CCQuestion] ContinuationRequested (resume after idle answer)",
    )
    .await;
    true
}

/// Arm the run-loop resume signal on a *live* coding-agent subprocess so the turn
/// it continues after a question answer re-arms event emission. The answer is
/// delivered to CC via its PreToolUse hook (`notify_and_release_waiter`), not via
/// `msg_tx`, so it never reaches the `msg_rx` arm's `reset_per_turn_flags` — the
/// only place the run loop clears `emitted_terminal_event`. Setting
/// `question_resume_pending` lets the loop self-heal: it reads-and-clears the flag
/// on the per-event lock it already takes and, if the turn was terminal-armed,
/// resets the per-turn flags before matching CC's first post-answer event instead
/// of dropping it as a "post-terminal straggler".
///
/// Must run BEFORE `notify_and_release_waiter` so the flag is visible before CC
/// resumes. No-op for `AnswerKind::Canceled` (the thread is being torn down) and
/// for a dead/absent subprocess (that path spawns a fresh `--resume` turn via
/// `ensure_resume_after_answer`, whose new run loop starts with clean flags).
/// Returns whether a live subprocess was armed.
async fn arm_question_resume_if_live(
    agent_sessions: &Arc<tokio::sync::Mutex<HashMap<Uuid, AgentSession>>>,
    thread_id: Uuid,
    answer: &AnswerKind,
) -> bool {
    if matches!(answer, AnswerKind::Canceled) {
        return false;
    }
    let mut sessions = agent_sessions.lock().await;
    match sessions.get_mut(&thread_id) {
        Some(s) if s.is_live() => {
            s.question_resume_pending = true;
            true
        }
        _ => false,
    }
}

// `pub(crate)`: the CC seeding helpers (`seed_cc_thread`, `emit_user_question`)
// are shared with `engine_impl/shutdown_tests.rs` — the teardown-preserve tests
// park a thread on the same question shape this suite uses.
#[cfg(test)]
#[path = "agent_question_tests/common.rs"]
pub(crate) mod aq_test_helpers;

#[cfg(test)]
#[path = "agent_question_tests/tests.rs"]
mod aq_tests;
