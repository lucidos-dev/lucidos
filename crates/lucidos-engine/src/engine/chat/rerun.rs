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

use std::sync::Arc;

use uuid::Uuid;

use crate::engine::event_bus::BusEvent;
use crate::engine::thread_events::{
    ActorMode, EngineReason, EventChannel, EventMeta, MessageOrigin, ThreadEvent,
};
use crate::engine::LucidosEngine;

/// Caps for the engine system note's bullet list.
const MAX_BULLET_ENTRIES: usize = 50;
/// Byte cap for bullet-field truncation; `floor_char_boundary` rounds the
/// cut down to the nearest UTF-8 char boundary.
const MAX_FIELD_BYTES: usize = 200;

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

        // ContinuationStarted opens the new exchange in the timeline. Use
        // serde to map the abort's persisted channel string back to
        // EventChannel — a hand-rolled match would silently downgrade unknown
        // variants to Chat if the wire format ever grows.
        let resume_channel: EventChannel = abort_channel
            .as_deref()
            .and_then(|s| serde_json::from_value(serde_json::Value::String(s.to_string())).ok())
            .unwrap_or(EventChannel::Chat);
        let recovered = self
            .event_bus
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
                    channel: Some(resume_channel),
                    actor: actor.clone(),
                    ..EventMeta::NONE
                },
            })
            .await?
            .expect("ContinuationStarted is persisted");
        let continuation_started_id = recovered.event_id;

        // UserPromptInjected carries the engine note for the audit trail.
        // request_event_id points at ContinuationStarted so the resume
        // exchange groups this event as its own step in the timeline.
        self.event_bus
            .emit(BusEvent::Thread {
                thread_id,
                event: ThreadEvent::UserPromptInjected {
                    text: engine_note.clone(),
                    mode: ActorMode::Engine,
                    origin: Some(MessageOrigin::engine(EngineReason::ContinuationStarted)),
                    injected_message_id: None,
                },
                meta: EventMeta {
                    channel: Some(resume_channel),
                    request_event_id: Some(continuation_started_id),
                    actor: Some(MessageOrigin::engine(EngineReason::ContinuationStarted)),
                    ..EventMeta::NONE
                },
            })
            .await?;

        // The engine note itself is the user prompt to the LLM (the original
        // prompt is in the thread history). `pre_emitted_origin =
        // Some(continuation_started_id)` skips a fresh MessageReceived emit
        // and stamps every resulting event with `request_event_id =
        // continuation_started_id` — the frontend uses this to gather them
        // into the resume exchange.
        let engine = self.clone_arc();
        let prompt_for_llm = engine_note;
        tokio::spawn(async move {
            if let Err(e) = engine
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
                    Some(continuation_started_id),
                    None,
                    Some(MessageOrigin::engine(EngineReason::ContinuationStarted)),
                )
                .await
            {
                log!("[Continue] Rerun for thread {} failed: {}", thread_id, e);
            }
        });

        Ok(ContinueChatOutcome::Resumed)
    }
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
