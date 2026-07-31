//! Continue an interrupted chat or trigger thread after engine restart.
//!
//! When the user clicks "Continue" on an aborted chat/trigger thread, this
//! module:
//!
//! 1. Finds the originating MessageReceived/TriggerStarted for the most recent
//!    ResponseAborted on the thread.
//! 2. Walks events between the originating event and the abort, summarizing
//!    completed `ToolCalled` + `ToolResult` pairs into a markdown bullet list.
//! 3. Emits `ContinuationStarted { branch: "", actor }` so the frontend
//!    opens a new "resume" exchange.
//! 4. Emits `UserPromptInjected { text: <engine note>, mode: engine,
//!    actor: engine }` carrying the side-effect-aware system note. The engine
//!    note is what the LLM will read; persisting it as `UserPromptInjected`
//!    leaves the audit trail visible in the UI.
//! 5. Re-enters `process_message_with_steps_internal` with the engine note as
//!    the latest user message, using the ContinuationStarted event id as
//!    `pre_emitted_origin` so the rerun's events carry a fresh
//!    `request_event_id` linking them to the resume boundary.
//!
//! Idempotency: if a ContinuationStarted already exists newer than the most
//! recent ResponseAborted, the call is a no-op (returns Ok). Prevents
//! double-Continue from doubling the rerun.
//!
//! Steps 3–4 (the boundary + reminder) belong to THIS path only — a genuine
//! interruption the user asked to revive. The other caller of
//! [`LucidosEngine::spawn_chat_resume`] — the answer-driven resume of a
//! question-parked thread that a restart never aborted — passes
//! [`ChatResumeAnchor::ExistingTurn`] and emits neither: it continues the
//! original turn instead of opening a new one. See [`ChatResumeAnchor`].

use std::sync::Arc;

use uuid::Uuid;

use super::PreEmittedOrigin;
use crate::engine::event_bus::{BusEvent, EventBus};
use crate::engine::thread_events::{
    ActorMode, EngineReason, EventChannel, EventMeta, MessageOrigin, ThreadEvent,
};
use crate::engine::LucidosEngine;

/// Type-erased return of [`LucidosEngine::spawn_chat_resume`], yielding the
/// anchor event id. Boxed rather than opaque to break a mutual async recursion
/// — see that method's doc comment for why the erasure is load-bearing.
pub(crate) type ChatResumeFuture = std::pin::Pin<
    Box<
        dyn std::future::Future<Output = Result<Uuid, Box<dyn std::error::Error + Send + Sync>>>
            + Send,
    >,
>;

/// Caps for the engine system note's bullet list.
const MAX_BULLET_ENTRIES: usize = 50;
/// Byte cap for bullet-field truncation; `floor_char_boundary` rounds the
/// cut down to the nearest UTF-8 char boundary.
const MAX_FIELD_BYTES: usize = 200;

/// Where a re-entered chat turn is anchored in the timeline — the one
/// difference between the two callers of [`LucidosEngine::spawn_chat_resume`].
///
/// The anchor is the `request_event_id` every event of the re-entered turn is
/// stamped with, so it decides which exchange the resumed work renders under.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ChatResumeAnchor {
    /// Manual **Continue** after a genuine interruption ([`LucidosEngine::continue_chat`]).
    /// The previous turn really did end (a `ResponseAborted` sits in the
    /// timeline), and the user asked for a NEW attempt — so the resume opens
    /// its own boundary: `ContinuationStarted` ("Continued the response") plus
    /// the engine note as `UserPromptInjected` ("Reminded the model about N
    /// prior tool calls"). That reminder is the point of this path: it tells
    /// the user what the engine told the model about the side effects the
    /// aborted run already performed.
    NewBoundary,
    /// Answer-driven resume of a turn that was **never terminated** — a thread
    /// parked on an `ask_user_question` survived a restart (no abort was
    /// emitted; see `agent_recovery::thread_has_unanswered_question`) and the
    /// user then answered. Nothing was interrupted from the user's point of
    /// view and nothing needs their action, so the resume must be invisible:
    /// no boundary panel, no reminder. The re-entered turn carries the
    /// ORIGINAL turn's `request_event_id`, so its events group under the
    /// question card exactly as they would have without the restart (the chat
    /// parity of the coding agent's silent `--resume`).
    ExistingTurn(Uuid),
}

/// Emit whatever timeline boundary `anchor` calls for and return the
/// `request_event_id` the re-entered turn must be stamped with.
///
/// Split out of [`LucidosEngine::spawn_chat_resume`] so the emitted-event
/// surface — the whole user-visible difference between "the reminder appears"
/// and "the restart is invisible" — is testable against a bare `EventBus`,
/// without a live agentic loop.
pub(crate) async fn emit_resume_anchor(
    bus: &EventBus,
    thread_id: Uuid,
    anchor: ChatResumeAnchor,
    engine_note: &str,
    channel: EventChannel,
    actor: Option<MessageOrigin>,
) -> Result<Uuid, Box<dyn std::error::Error + Send + Sync>> {
    if let ChatResumeAnchor::ExistingTurn(request_event_id) = anchor {
        return Ok(request_event_id);
    }

    // ContinuationStarted opens the new exchange in the timeline.
    let continuation_started_id = bus
        .emit(BusEvent::Thread {
            thread_id,
            event: ThreadEvent::ContinuationStarted {
                branch: String::new(),
                origin: None,
                // Chat rerun carries its own engine note, not a CC
                // continuation reason.
                reason: None,
            },
            meta: EventMeta {
                channel: Some(channel),
                actor,
                ..EventMeta::NONE
            },
        })
        .await?
        .expect("ContinuationStarted is persisted")
        .event_id;

    // UserPromptInjected carries the engine note for the audit trail.
    // request_event_id points at ContinuationStarted so the resume
    // exchange groups this event as its own step in the timeline.
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::UserPromptInjected {
            text: engine_note.to_string(),
            mode: ActorMode::Engine,
            origin: Some(MessageOrigin::engine(EngineReason::ContinuationStarted)),
            injected_message_id: None,
        },
        meta: EventMeta {
            channel: Some(channel),
            request_event_id: Some(continuation_started_id),
            actor: Some(MessageOrigin::engine(EngineReason::ContinuationStarted)),
            ..EventMeta::NONE
        },
    })
    .await?;

    Ok(continuation_started_id)
}

/// Outcome of `continue_chat` — used by the HTTP handler to log clearly.
#[derive(Debug, PartialEq, Eq)]
pub enum ContinueChatOutcome {
    /// Rerun was kicked off (ContinuationStarted + UserPromptInjected
    /// emitted, agentic loop spawned).
    Resumed,
    /// A ContinuationStarted already exists newer than the latest
    /// ResponseAborted. The click is a no-op — likely a double-Continue race.
    AlreadyResumed,
    /// No ResponseAborted exists for this thread, or no originating event was
    /// found. Nothing to continue.
    NothingToContinue,
}

impl LucidosEngine {
    /// Resume an aborted chat/trigger thread on the user's behalf.
    /// See module-level docs for the full sequence.
    pub async fn continue_chat(
        self: &Arc<Self>,
        thread_id: Uuid,
        actor: Option<MessageOrigin>,
    ) -> Result<ContinueChatOutcome, Box<dyn std::error::Error + Send + Sync>> {
        // Find the most recent ResponseAborted for this thread.
        let abort_row: Option<(Uuid, Option<Uuid>, Option<String>)> = sqlx::query_as(
            "SELECT id, \
                    NULLIF(payload->>'request_event_id','')::uuid AS request_event_id, \
                    payload->>'channel' AS channel \
             FROM events \
             WHERE aggregate_id = $1 AND event_type = 'ResponseAborted' \
             ORDER BY sequence DESC LIMIT 1",
        )
        .bind(thread_id.to_string())
        .fetch_optional(self.pool())
        .await?;

        let (abort_event_id, abort_request_event_id, abort_channel) = match abort_row {
            Some(row) => row,
            None => {
                log!(
                    "[Continue] No ResponseAborted for thread {} — nothing to continue",
                    thread_id
                );
                return Ok(ContinueChatOutcome::NothingToContinue);
            }
        };

        // Idempotency: if a ContinuationStarted already exists with a higher
        // sequence than the abort, the user's previous click already started
        // a rerun. Don't double-spawn. The 20260513 migration rewrites the
        // older names (`SessionRecovered`, `SessionResumed`) to
        // `ContinuationStarted` at startup, so a single-name match suffices
        // — sqlx runs migrations before any query.
        let already_resumed: bool = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS ( \
                SELECT 1 FROM events \
                WHERE aggregate_id = $1 \
                  AND event_type = 'ContinuationStarted' \
                  AND sequence > (SELECT sequence FROM events WHERE id = $2) \
            )",
        )
        .bind(thread_id.to_string())
        .bind(abort_event_id)
        .fetch_one(self.pool())
        .await?;

        if already_resumed {
            log!(
                "[Continue] ContinuationStarted already exists newer than abort for thread {} — no-op",
                thread_id
            );
            return Ok(ContinueChatOutcome::AlreadyResumed);
        }

        // Resolve the originating event. Prefer `request_event_id` stamped on
        // the ResponseAborted (set by the engine-restart paths). Fall back to
        // the most recent originating event before the abort for legacy DB
        // rows that lack the field. Uses `CHAT_ORIGINATING_EVENT_TYPES` so a
        // chat-agent turn woken from a finished child resolves to the CTC,
        // not a stale older MR from a previous completed turn.
        let originating_event_id = match abort_request_event_id {
            Some(id) => Some(id),
            None => {
                crate::engine::agent_session::latest_originating_event_id(
                    self.pool(),
                    thread_id,
                    crate::engine::agent_session::CHAT_ORIGINATING_EVENT_TYPES,
                )
                .await
            }
        };

        let originating_event_id = match originating_event_id {
            Some(id) => id,
            None => {
                log!(
                    "[Continue] No originating event found for abort {} in thread {} — nothing to continue",
                    abort_event_id,
                    thread_id
                );
                return Ok(ContinueChatOutcome::NothingToContinue);
            }
        };

        // Walk events between originating and abort, summarize completed tool
        // pairs and count thinking blocks.
        let between_events: Vec<(String, serde_json::Value)> = sqlx::query_as(
            "SELECT event_type, payload FROM events \
             WHERE aggregate_id = $1 \
               AND sequence > (SELECT sequence FROM events WHERE id = $2) \
               AND sequence < (SELECT sequence FROM events WHERE id = $3) \
             ORDER BY sequence ASC",
        )
        .bind(thread_id.to_string())
        .bind(originating_event_id)
        .bind(abort_event_id)
        .fetch_all(self.pool())
        .await?;

        let summary = build_side_effect_summary(&between_events);
        let engine_note = build_engine_note(&summary);

        // Which channel the resumed turn is emitted on. The abort's own channel
        // wins when it has one; otherwise fall back to the thread's recorded
        // `source` — see `resolve_resume_channel` for why the bare `Chat` default
        // was wrong for trigger threads.
        let thread_source: Option<String> = sqlx::query_scalar::<_, String>(
            "SELECT source FROM thread_summaries WHERE thread_id = $1",
        )
        .bind(thread_id)
        .fetch_optional(self.pool())
        .await?;
        let resume_channel =
            resolve_resume_channel(abort_channel.as_deref(), thread_source.as_deref());

        // A genuine interruption the user asked to revive → the resume gets its
        // own boundary + the side-effect reminder.
        self.spawn_chat_resume(
            thread_id,
            engine_note,
            resume_channel,
            actor,
            ChatResumeAnchor::NewBoundary,
        )
        .await?;

        Ok(ContinueChatOutcome::Resumed)
    }

    /// Re-enter the agentic loop with `engine_note` as the current turn, anchored
    /// per `anchor` (see [`ChatResumeAnchor`] — a new `ContinuationStarted`
    /// boundary for the manual Continue, or the interrupted turn's own
    /// `request_event_id` for an answer-driven resume).
    /// Shared by the manual-Continue path ([`continue_chat`], which computes the
    /// note from the aborted run's side effects) AND the question answer-resume
    /// path ([`crate::engine::agent_question::resume_chat_after_answer`], which
    /// re-runs a chat thread whose question was answered with no live loop after
    /// a restart — the chat parity of CC's `--resume`). One re-entry site so the
    /// two paths can't drift on how a chat continuation is spawned.
    ///
    /// Returns the anchor event id (the `pre_emitted_origin` every resulting
    /// event is stamped with, so the frontend groups them into the resume
    /// exchange — or, for `ExistingTurn`, back into the turn that was
    /// interrupted).
    ///
    /// Returns a boxed `dyn Future` rather than being a plain `async fn` on
    /// purpose: this method is reachable from `process_message_with_steps` (the
    /// chat FreeText answer path → `answer_pending_question` →
    /// `resume_chat_after_answer` → here), and it in turn spawns a fresh
    /// `process_message_with_steps` turn — a mutual async recursion whose opaque
    /// return types would form an infinite cycle (`E0391`) and whose `Send`
    /// inference would loop. The concrete boxed return type is the type-erasure
    /// boundary that terminates both: callers await a `dyn Future + Send`, not an
    /// opaque type that transitively contains itself.
    pub(crate) fn spawn_chat_resume(
        self: &Arc<Self>,
        thread_id: Uuid,
        engine_note: String,
        channel: EventChannel,
        actor: Option<MessageOrigin>,
        anchor: ChatResumeAnchor,
    ) -> ChatResumeFuture {
        let engine = self.clone_arc();
        Box::pin(async move {
            let anchor_event_id = emit_resume_anchor(
                &engine.event_bus,
                thread_id,
                anchor,
                &engine_note,
                channel,
                actor,
            )
            .await?;

            // The engine note itself is the user prompt to the LLM (the original
            // prompt is in the thread history). `pre_emitted_origin =
            // Some(anchor_event_id)` skips a fresh MessageReceived emit and
            // stamps every resulting event with `request_event_id =
            // anchor_event_id` — the frontend uses this to gather them into the
            // resume exchange (or back into the interrupted turn's own exchange).
            let loop_engine = engine.clone();
            let prompt_for_llm = engine_note;
            tokio::spawn(async move {
                if let Err(e) = loop_engine
                    .process_message_with_steps(
                        &prompt_for_llm,
                        None,
                        None,
                        None,
                        None,
                        None,
                        None,
                        None,
                        None,
                        Some(thread_id),
                        None,
                        None,
                        None,
                        None,
                        None,
                        ActorMode::Engine,
                        None,
                        None,
                        Some(PreEmittedOrigin::EngineReentry(anchor_event_id)),
                        None,
                        Some(MessageOrigin::engine(EngineReason::ContinuationStarted)),
                    )
                    .await
                {
                    log!("[Continue] Rerun for thread {} failed: {}", thread_id, e);
                }
            });

            Ok(anchor_event_id)
        })
    }
}

/// Which [`EventChannel`] a resumed turn is emitted on, given the abort's persisted
/// channel and the thread's recorded `source`.
///
/// The abort's own channel wins when present. When it is absent the thread's `source`
/// decides — and that fallback is load-bearing, not cosmetic: the restart teardown
/// (`abort_in_flight_for_restart`) deliberately stamps `channel: None` on the
/// chat/trigger branch (it uses `Some(ClaudeCode)` only to mark the coding-agent
/// bucket), so EVERY switch-interrupted chat *and trigger* thread reaches here with
/// no channel. A bare `Chat` default therefore resumed a **trigger** thread on the
/// chat channel, and the `ContinuationStarted` projection arm writes
/// `source = <channel>` — silently rewriting the thread's `source` from `trigger` to
/// `chat`. Deriving from `source` keeps that round-trip identity-preserving.
///
/// Anything unrecognised (a legacy row, a wire variant this build predates) falls back
/// to `Chat`, matching the previous behavior. Parsing goes through
/// `EventChannel::from_wire`, the shared wire decoder — which also accepts the legacy
/// `scheduled_trigger` alias, so an old trigger row resolves too.
fn resolve_resume_channel(
    abort_channel: Option<&str>,
    thread_source: Option<&str>,
) -> EventChannel {
    abort_channel
        .and_then(EventChannel::from_wire)
        .or_else(|| thread_source.and_then(EventChannel::from_wire))
        .unwrap_or(EventChannel::Chat)
}

/// Format the side-effect summary for the engine note.
/// Returns either a non-empty markdown bullet list or an explanatory line.
fn build_side_effect_summary(events: &[(String, serde_json::Value)]) -> String {
    // Pair ToolCalled + ToolResult by walking forward; the ToolResult event
    // for a call may follow immediately or after ThoughtStreamed/TextStreamed events.
    let mut summary_lines: Vec<String> = Vec::new();
    let mut pending_calls: Vec<(String, String)> = Vec::new(); // (tool_name, args_summary)

    for (event_type, payload) in events {
        match event_type.as_str() {
            "ToolCalled" => {
                let name = payload
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("(unknown)")
                    .to_string();
                let args_str = payload
                    .get("description")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| {
                        payload
                            .get("args")
                            .map(|a| a.to_string())
                            .unwrap_or_default()
                    });
                pending_calls.push((name, truncate_for_note(&args_str)));
            }
            "ToolResult" => {
                let result_text = payload
                    .get("result")
                    .and_then(|v| v.as_str())
                    .unwrap_or("(no result)")
                    .to_string();
                let result_summary = truncate_for_note(&result_text);
                if let Some((name, args)) = pending_calls.pop() {
                    summary_lines.push(format!("- {}({}) → {}", name, args, result_summary));
                    if summary_lines.len() >= MAX_BULLET_ENTRIES {
                        summary_lines.push(format!(
                            "- ...(truncated at {} entries)",
                            MAX_BULLET_ENTRIES
                        ));
                        break;
                    }
                }
            }
            _ => {} // ThoughtStreamed + TextStreamed don't appear in the LLM-visible summary
        }
    }

    if summary_lines.is_empty() {
        "No actions completed before the abort.".to_string()
    } else {
        summary_lines.join("\n")
    }
}

fn truncate_for_note(s: &str) -> String {
    let trimmed = s.trim().replace('\n', " ");
    if trimmed.len() > MAX_FIELD_BYTES {
        let cut = trimmed.floor_char_boundary(MAX_FIELD_BYTES);
        format!("{}...", &trimmed[..cut])
    } else {
        trimmed
    }
}

fn build_engine_note(summary: &str) -> String {
    format!(
        "[Engine note — this is a rerun]\n\
         Your previous attempt at this turn was interrupted by an engine restart.\n\
         The interrupted run performed the following actions before the abort:\n\
         {}\n\
         Decide whether to:\n\
         - Skip actions that already completed successfully (don't re-run \"send_notification\" if the user has already been pinged).\n\
         - Re-verify state where it might have changed since the abort (file edits, git state).\n\
         - Continue with anything that didn't get to run.\n\
         Engine performed no automatic skipping — your judgment owns this.",
        summary
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// The restart teardown stamps `channel: None` on every chat AND trigger
    /// abort (it only sets `Some(ClaudeCode)` to mark the coding-agent bucket),
    /// so the `source` fallback is what a switch-interrupted thread actually
    /// resolves through. Getting it wrong is not cosmetic: the
    /// `ContinuationStarted` projection arm writes `source = <channel>`, so a
    /// bare `Chat` default rewrites a trigger thread's `source` to `chat`.
    #[test]
    fn resume_channel_prefers_the_abort_then_the_thread_source() {
        // Abort carries a channel → it wins outright.
        assert_eq!(
            resolve_resume_channel(Some("claude_code"), Some("chat")),
            EventChannel::ClaudeCode
        );

        // Channel-less teardown abort → derive from the thread's source, so a
        // trigger thread resumes as Trigger and keeps its identity.
        assert_eq!(
            resolve_resume_channel(None, Some("trigger")),
            EventChannel::Trigger
        );
        assert_eq!(
            resolve_resume_channel(None, Some("chat")),
            EventChannel::Chat
        );
        // Legacy source spelling still resolves via the shared wire decoder.
        assert_eq!(
            resolve_resume_channel(None, Some("scheduled_trigger")),
            EventChannel::Trigger
        );

        // Nothing usable → the historical Chat default, never a panic.
        assert_eq!(resolve_resume_channel(None, None), EventChannel::Chat);
        assert_eq!(
            resolve_resume_channel(Some("from_the_future"), Some("also_unknown")),
            EventChannel::Chat
        );
    }

    #[test]
    fn summary_pairs_tool_call_with_result() {
        let events = vec![
            (
                "ToolCalled".to_string(),
                json!({"name": "send_notification", "description": "Notify: Ping"}),
            ),
            (
                "ToolResult".to_string(),
                json!({"name": "send_notification", "result": "ok"}),
            ),
        ];
        let s = build_side_effect_summary(&events);
        assert!(
            s.contains("- send_notification(Notify: Ping) → ok"),
            "got {}",
            s
        );
    }

    #[test]
    fn summary_handles_zero_pairs() {
        let events: Vec<(String, serde_json::Value)> =
            vec![("Thinking".to_string(), json!({"text": "musing"}))];
        let s = build_side_effect_summary(&events);
        assert_eq!(s, "No actions completed before the abort.");
    }

    #[test]
    fn summary_skips_thinking() {
        let events = vec![
            ("Thinking".to_string(), json!({"text": "thinking..."})),
            (
                "ToolCalled".to_string(),
                json!({"name": "read_file", "description": "Read foo.txt"}),
            ),
            (
                "ToolResult".to_string(),
                json!({"name": "read_file", "result": "contents"}),
            ),
        ];
        let s = build_side_effect_summary(&events);
        assert!(
            !s.contains("thinking"),
            "summary must not include thinking text: {}",
            s
        );
        assert!(s.contains("read_file"));
    }

    #[test]
    fn summary_truncates_long_args_and_results() {
        let big = "x".repeat(500);
        let events = vec![
            (
                "ToolCalled".to_string(),
                json!({"name": "run_bash", "args": {"command": big.clone()}}),
            ),
            (
                "ToolResult".to_string(),
                json!({"name": "run_bash", "result": big}),
            ),
        ];
        let s = build_side_effect_summary(&events);
        assert!(s.contains("..."), "expected ellipsis on truncation: {}", s);
        // Args + result both capped at 200 bytes (truncate_for_note's contract).
        for line in s.lines() {
            for field in line.split("→") {
                assert!(
                    field.len() < MAX_FIELD_BYTES + 50,
                    "field too long: {}",
                    field
                );
            }
        }
    }

    #[test]
    fn summary_caps_at_max_entries() {
        let mut events = Vec::new();
        for i in 0..(MAX_BULLET_ENTRIES + 5) {
            events.push((
                "ToolCalled".to_string(),
                json!({"name": "tool", "description": format!("call {}", i)}),
            ));
            events.push((
                "ToolResult".to_string(),
                json!({"name": "tool", "result": format!("ok {}", i)}),
            ));
        }
        let s = build_side_effect_summary(&events);
        let line_count = s.lines().count();
        assert!(
            line_count <= MAX_BULLET_ENTRIES + 1,
            "expected ≤{} bullet lines, got {}",
            MAX_BULLET_ENTRIES + 1,
            line_count
        );
        assert!(
            s.contains("(truncated"),
            "expected truncation marker: {}",
            s
        );
    }

    #[test]
    fn engine_note_mentions_rerun_and_summary() {
        let note = build_engine_note("- send_notification(Hi) → ok");
        assert!(note.contains("[Engine note — this is a rerun]"));
        assert!(note.contains("send_notification(Hi)"));
        assert!(note.contains("Engine performed no automatic skipping"));
    }
}
