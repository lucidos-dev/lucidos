use super::*;
use crate::engine::ApiUsage;
use std::collections::HashSet;

/// INVARIANT: The little parse state that spans lines of ONE Claude Code stdout stream.
/// The driver task owns an instance per spawned process and passes it to every
/// [`parse_line`] call, so the state is per session by construction: two
/// concurrent coding-agent sessions cannot suppress each other's events. Same
/// shape as the Codex side's `TurnTracker` / `AppServerTracker`.
#[derive(Debug, Default)]
pub struct CcStreamState {
    billing_account: Option<crate::runtime::billing_provider::BillingAttribution>,
    billing_default_endpoint: bool,
    usage_reported_message_ids: HashSet<String>,
    /// An always-thinking model's `thinking` text has proven to be a note for
    /// the user, by arriving non-empty. One-way: a later empty block does not
    /// undo it. Until then the text counts as reasoning. A temporary measure:
    /// `docs/temporary-measures.md` § "Claude Code's progress notes start
    /// hidden" (ADR 0608).
    notes_proven: bool,
    pending_promotion: bool,
    streaming_model: Option<String>,
    open_call: Option<OpenCall>,
    /// INVARIANT: A sub-agent call's `Usage`, by message id, claimed on a frame that drew
    /// nothing (a `thinking` block). It is released after the same message's
    /// next frame, so the capture follows the step it belongs to.
    held_sub_agent_usage: Vec<(String, AgentEvent)>,
    turn_ending_api_error: Option<String>,
}

fn is_always_thinking(model: Option<&str>) -> bool {
    model.and_then(crate::llm::anthropic_wire::thinking_mode)
        == Some(crate::llm::anthropic_wire::ThinkingMode::AlwaysOn)
}

fn call_usage(usage: &serde_json::Value) -> ApiUsage {
    crate::llm::usage_wire::from_block(usage).unwrap_or_default()
}

fn is_from_a_real_call(usage: &ApiUsage) -> bool {
    usage.input_tokens > 0 || usage.output_tokens > 0
}

fn message_text(message: Option<&serde_json::Value>) -> String {
    message
        .and_then(|m| m.get("content"))
        .and_then(|c| c.as_array())
        .into_iter()
        .flatten()
        .filter(|block| block.get("type").and_then(|v| v.as_str()) == Some("text"))
        .filter_map(|block| block.get("text").and_then(|v| v.as_str()))
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string()
}

#[derive(Debug)]
struct OpenCall {
    message_id: Option<String>,
    model: Option<String>,
    usage: ApiUsage,
    opened_at: std::time::Instant,
}

impl OpenCall {
    fn duration_ms(&self) -> Option<u64> {
        u64::try_from(self.opened_at.elapsed().as_millis()).ok()
    }
}

impl CcStreamState {
    fn thinking_is_a_note(&self, model: Option<&str>) -> bool {
        self.notes_proven && is_always_thinking(model)
    }

    fn prove_notes(&mut self) -> bool {
        let first = !std::mem::replace(&mut self.notes_proven, true);
        self.pending_promotion |= first;
        first
    }

    pub fn take_promotion(&mut self) -> bool {
        std::mem::take(&mut self.pending_promotion)
    }

    /// INVARIANT: Claim the one `Usage` event this assistant message is entitled to.
    /// `true` means the caller may emit: nothing has reported this id yet, and
    /// the claim is now recorded so the message's remaining frames get `false`.
    /// It mutates on purpose, which is why it is not named as a question.
    ///
    /// Every id claimed this turn is remembered, not just the most recent few.
    /// A parallel sub-agent's frames ride the PARENT's stream (the same reason
    /// its error banner has to be filtered out below), so any number of
    /// messages can interleave, and forgetting one would let its next frame
    /// report the same call twice.
    ///
    /// A frame with no `message.id` always wins the claim and records nothing.
    /// With no key there is no telling a repeat from a fresh call, and
    /// under-counting spend is worse than over-counting it.
    fn claim_usage_report(&mut self, message_id: Option<&str>) -> bool {
        let Some(id) = message_id else { return true };
        self.usage_reported_message_ids.insert(id.to_string())
    }

    fn end_turn(&mut self) {
        self.usage_reported_message_ids.clear();
        self.turn_ending_api_error = None;
    }

    fn report(
        &mut self,
        message_id: Option<&str>,
        model: Option<String>,
        usage: ApiUsage,
        parent_tool_use_id: Option<String>,
        duration_ms: Option<u64>,
    ) -> Option<AgentEvent> {
        // INVARIANT: The `&&` order matters: an all-zero frame must not claim the id,
        // so a later real frame carrying the same id still reports.
        (is_from_a_real_call(&usage) && self.claim_usage_report(message_id)).then_some(
            AgentEvent::Usage {
                model,
                input_tokens: usage.uncached_input_tokens(),
                output_tokens: usage.output_tokens,
                cache_read_tokens: usage.cache_read_tokens,
                cache_creation_tokens: usage.cache_creation_tokens,
                cache_creation_1h_tokens: usage.cache_creation_1h_tokens,
                parent_tool_use_id,
                api_call_id: message_id.map(String::from),
                duration_ms,
                reasoning_effort: None,
            },
        )
    }

    fn reports_at_message_delta(&self, message_id: Option<&str>) -> bool {
        message_id.is_some()
            && self
                .open_call
                .as_ref()
                .is_some_and(|open| open.message_id.as_deref() == message_id)
    }

    fn hold_usage(&mut self, message_id: &str, usage: AgentEvent) {
        self.held_sub_agent_usage
            .push((message_id.to_string(), usage));
    }

    fn take_held_usage(&mut self, message_id: &str) -> Vec<AgentEvent> {
        let (taken, kept) = std::mem::take(&mut self.held_sub_agent_usage)
            .into_iter()
            .partition(|(id, _)| id == message_id);
        self.held_sub_agent_usage = kept;
        taken.into_iter().map(|(_, usage)| usage).collect()
    }

    /// INVARIANT: Every held `Usage`, for a turn or a stream that ended before the
    /// messages drew anything. The provider billed them either way.
    pub fn release_held_usage(&mut self) -> Vec<AgentEvent> {
        std::mem::take(&mut self.held_sub_agent_usage)
            .into_iter()
            .map(|(_, usage)| usage)
            .collect()
    }

    /// INVARIANT: Report the open call with the counts its `message_start` gave. This
    /// runs when no `message_delta` came: the next call started, the turn
    /// ended mid-stream, or the stream itself ended. The driver calls it once
    /// stdout is done, since a killed process writes no `result`. The provider
    /// billed the prompt either way.
    pub fn close_open_call(&mut self) -> Option<AgentEvent> {
        let open = self.open_call.take()?;
        let duration_ms = open.duration_ms();
        self.report(
            open.message_id.as_deref(),
            open.model,
            open.usage,
            None,
            duration_ms,
        )
    }
}

fn tool_result_content(content: Option<&serde_json::Value>) -> String {
    match content {
        None | Some(serde_json::Value::Null) => String::new(),
        Some(serde_json::Value::String(s)) => s.clone(),
        Some(serde_json::Value::Array(blocks)) => blocks
            .iter()
            .map(describe_content_block)
            .collect::<Vec<_>>()
            .join("\n"),
        Some(obj) if obj.get("type").is_some() => describe_content_block(obj),
        // INVARIANT: A shape CC has never sent, carrying no block type to dispatch on.
        // Its JSON beats dropping it, and the 200-char cap in
        // `run_session/run.rs` bounds what that can cost.
        Some(other) => other.to_string(),
    }
}

/// INVARIANT: One block of an array-shaped tool result, as a short line of text.
///
/// Only `text` yields its content. An image yields a label instead: one
/// full-page screenshot is hundreds of KB of base64, which would ride the event
/// payload and every SSE frame carrying it. Any other type yields a label too,
/// naming itself, so the next block type CC invents is visible rather than
/// silent. That default is the whole point: silence is the defect.
///
/// A block reduces to a LABEL, never to nothing. The one way to get the empty
/// string back is a `text` block whose text really is empty, which is content
/// rather than a gap.
fn describe_content_block(block: &serde_json::Value) -> String {
    if let Some(s) = block.as_str() {
        return s.to_string();
    }
    let kind = block
        .get("type")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");
    match (kind, block.get("text").and_then(|v| v.as_str())) {
        ("text", Some(t)) => t.to_string(),
        _ => match block.get("tool_name").and_then(|v| v.as_str()) {
            Some(name) => format!("[{kind}: {name}]"),
            None => format!("[{kind}]"),
        },
    }
}

pub(super) fn is_input_replay(val: &serde_json::Value) -> bool {
    let replayed = val.get("isReplay").and_then(|v| v.as_bool()) == Some(true);
    let carries_tool_result = val
        .get("message")
        .and_then(|m| m.get("content"))
        .and_then(|c| c.as_array())
        .is_some_and(|blocks| {
            blocks
                .iter()
                .any(|b| b.get("type").and_then(|t| t.as_str()) == Some("tool_result"))
        });
    replayed && !carries_tool_result
}

fn replayed_input_uuid(val: &serde_json::Value) -> Option<uuid::Uuid> {
    val.get("uuid")
        .and_then(|v| v.as_str())
        .and_then(|s| uuid::Uuid::parse_str(s).ok())
}

/// INVARIANT: Parse a single JSON line from Claude Code's stream output.
/// Returns all recognized events from the line. An assistant message with
/// multiple content blocks (text + tool_use) produces multiple events.
/// Never produces `AgentEvent::Exited`: that variant is emitted by the
/// driver task on process exit.
pub fn parse_line(state: &mut CcStreamState, line: &str) -> Vec<AgentEvent> {
    let line = line.trim();
    if line.is_empty() {
        return Vec::new();
    }

    let val: serde_json::Value = match serde_json::from_str(line) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };
    let event_type = val.get("type").and_then(|v| v.as_str()).unwrap_or("");
    let parent_tool_use_id = val
        .get("parent_tool_use_id")
        .and_then(|v| v.as_str())
        .map(String::from);

    match event_type {
        // INVARIANT: Only parse subtype "init" — hook events (hook_started, hook_response,
        // hook_progress) also have type "system" + session_id but lack slash_commands.
        // Without this guard, hook events after init overwrite commands with empty arrays.
        "system" => {
            let subtype = val.get("subtype").and_then(|v| v.as_str()).unwrap_or("");
            if subtype == "init" {
                if let Some(sid) = val.get("session_id").and_then(|v| v.as_str()) {
                    let slash_commands = val
                        .get("slash_commands")
                        .and_then(|v| v.as_array())
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|v| v.as_str().map(String::from))
                                .collect()
                        })
                        .unwrap_or_default();
                    let skills = val
                        .get("skills")
                        .and_then(|v| v.as_array())
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|v| v.as_str().map(String::from))
                                .collect()
                        })
                        .unwrap_or_default();
                    let model = val.get("model").and_then(|v| v.as_str()).map(String::from);
                    let agent_version = val
                        .get("claude_code_version")
                        .and_then(|v| v.as_str())
                        .map(String::from);
                    let mcp_servers =
                        val.get("mcp_servers")
                            .and_then(|v| v.as_array())
                            .and_then(|servers| {
                                servers
                                    .iter()
                                    .map(|server| {
                                        let status = server.get("status")?.as_str()?;
                                        Some(crate::runtime::agent_runtime::McpServerStatus {
                                            name: server.get("name")?.as_str()?.to_owned(),
                                            status: status.to_owned(),
                                            needs_sign_in: status == "needs-auth",
                                        })
                                    })
                                    .collect()
                            });
                    vec![AgentEvent::Init {
                        session_id: sid.to_string(),
                        model,
                        slash_commands,
                        skills,
                        mcp_servers,
                        agent_version,
                        can_steer: true,
                    }]
                } else {
                    Vec::new()
                }
            } else if subtype == "compact_boundary" {
                let metadata = val.get("compact_metadata");
                let tokens = |key: &str| metadata.and_then(|m| m.get(key)).and_then(|v| v.as_u64());
                vec![AgentEvent::Compacted {
                    trigger: metadata
                        .and_then(|m| m.get("trigger"))
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .to_string(),
                    pre_tokens: tokens("pre_tokens"),
                    post_tokens: tokens("post_tokens"),
                }]
            } else {
                Vec::new()
            }
        }
        "assistant" => {
            let mut events = Vec::new();
            let message = val.get("message");
            // INVARIANT: CC's own error banner ("API Error: Stream idle timeout - no chunks
            // received", "API Error: Response stalled mid-stream. …") arrives as a
            // SYNTHETIC assistant message: `message.model` is `<synthetic>` and the
            // line carries `is_api_error_message: true`, the stream-json name for
            // CC's internal `isApiErrorMessage`, documented in its SDK schema as
            // "True when this assistant message wraps an API error".
            //
            // It is CC's error SURFACE, not model prose. When it ends the turn, its
            // text becomes the `result` error and then `ResponseFailed`, which the
            // transcript already renders in the failure card. Ingesting it as text
            // therefore printed the failure twice: once as a paragraph in the
            // response body, and again in the red card right beneath it.
            //
            // Only the text is skipped. A synthetic error line carries no tool_use
            // and zeroed usage, so nothing else is lost.
            //
            // A sub-agent's prose is the sub-agent's narration, so it arrives as
            // `SubAgentMessage` and never joins the session's own reply.
            let is_api_error_banner = val
                .get("is_api_error_message")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            // INVARIANT: The banner's text is the turn's failure reason, even when CC
            // stamps the `result` successful. A gateway's own error body
            // (a spend cap, say) carries no `API Error` prefix to match on.
            if parent_tool_use_id.is_none() {
                state.turn_ending_api_error = is_api_error_banner
                    .then(|| message_text(message))
                    .filter(|text| !text.is_empty());
            }
            let prose = |text: String| match &parent_tool_use_id {
                Some(parent) => AgentEvent::SubAgentMessage {
                    text,
                    parent_tool_use_id: parent.clone(),
                },
                None => AgentEvent::Message {
                    role: "assistant".to_string(),
                    text,
                    opens_block: true,
                },
            };
            let model = message
                .and_then(|m| m.get("model"))
                .and_then(|v| v.as_str());
            let message_id = message.and_then(|m| m.get("id")).and_then(|v| v.as_str());
            let mut first_note_streamed = false;
            if let Some(content) = message
                .and_then(|m| m.get("content"))
                .and_then(|c| c.as_array())
            {
                for block in content {
                    let block_type = block.get("type").and_then(|v| v.as_str()).unwrap_or("");
                    match block_type {
                        "text" if !is_api_error_banner => {
                            if let Some(text) = block.get("text").and_then(|v| v.as_str()) {
                                events.push(prose(text.to_string()));
                            }
                        }
                        "thinking" if is_always_thinking(model) && !is_api_error_banner => {
                            if let Some(note) = block
                                .get("thinking")
                                .and_then(|v| v.as_str())
                                .and_then(crate::llm::anthropic_wire::progress_note)
                            {
                                first_note_streamed |= state.prove_notes();
                                match &parent_tool_use_id {
                                    Some(_) => events.push(prose(note.to_string())),
                                    None if first_note_streamed => {}
                                    None => events.push(AgentEvent::ProgressNote {
                                        text: note.to_string(),
                                    }),
                                }
                            }
                        }
                        "tool_use" => {
                            let name = block
                                .get("name")
                                .and_then(|v| v.as_str())
                                .unwrap_or("unknown")
                                .to_string();
                            let input = block
                                .get("input")
                                .cloned()
                                .unwrap_or(serde_json::Value::Null);
                            let id = block
                                .get("id")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string();
                            events.push(AgentEvent::ToolUse {
                                name,
                                input,
                                id,
                                parent_tool_use_id: parent_tool_use_id.clone(),
                                api_call_id: message_id.map(String::from),
                            });
                        }
                        _ => {}
                    }
                }
            }
            // INVARIANT: A `Usage` event is how the consumer emits `ContextCaptured`, but
            // a frame is NOT an API call. CC splits one message into a frame
            // per content block. Each repeats the same `message.id` and the
            // usage `message_start` gave, whose output count is a placeholder.
            //
            // So the parent's calls report at their `message_delta`, which
            // carries the final counts (the `stream_event` arm). A frame reports
            // only a message no `message_start` announced, which is how a
            // sub-agent's calls arrive. The message id dedups those: an id is
            // unique per API response, so a second sighting is a re-report.
            // Only the `Usage` is suppressed; the frame's content emits above.
            //
            // A sub-agent message that opens with a `thinking` block claims on
            // that frame, which draws nothing. Its `Usage` waits for the
            // message's next frame, so it lands after the step it measured.
            let frame_drew_nothing = events.is_empty();
            let held_by = message_id.filter(|_| parent_tool_use_id.is_some());
            if let (Some(id), false) = (held_by, frame_drew_nothing) {
                events.extend(state.take_held_usage(id));
            }
            if let Some(usage) = message.and_then(|m| m.get("usage")) {
                if !state.reports_at_message_delta(message_id) {
                    let report = state.report(
                        message_id,
                        model.map(String::from),
                        call_usage(usage),
                        parent_tool_use_id,
                        None,
                    );
                    match (report, held_by) {
                        (Some(report), Some(id)) if frame_drew_nothing => {
                            state.hold_usage(id, report)
                        }
                        (report, _) => events.extend(report),
                    }
                }
            }
            events
        }
        "tool_result" => {
            let content = tool_result_content(val.get("content"));
            let is_error = val
                .get("is_error")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let id = val
                .get("tool_use_id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let status = if is_error { "error" } else { "success" };
            vec![AgentEvent::ToolResult {
                output: content,
                status: status.to_string(),
                id,
                parent_tool_use_id,
            }]
        }
        "user" => {
            let mut events = Vec::new();
            if is_input_replay(&val) {
                match replayed_input_uuid(&val) {
                    Some(input) => events.push(AgentEvent::InputRead { input }),
                    None => {
                        log_legacy!("[ClaudeCode] a replay named no input uuid; it reads nothing")
                    }
                }
                return events;
            }
            if let Some(content) = val
                .get("message")
                .and_then(|m| m.get("content"))
                .and_then(|c| c.as_array())
            {
                for block in content {
                    if block.get("type").and_then(|v| v.as_str()) == Some("tool_result") {
                        let output = tool_result_content(block.get("content"));
                        let is_error = block
                            .get("is_error")
                            .and_then(|v| v.as_bool())
                            .unwrap_or(false);
                        let id = block
                            .get("tool_use_id")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        let status = if is_error { "error" } else { "success" };
                        events.push(AgentEvent::ToolResult {
                            output,
                            status: status.to_string(),
                            id,
                            parent_tool_use_id: parent_tool_use_id.clone(),
                        });
                    }
                }
            }
            events
        }
        "result" => {
            let text = val
                .get("result")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let duration = val.get("duration_ms").and_then(|v| v.as_u64()).unwrap_or(0);
            let is_error = val
                .get("is_error")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let error = if is_error {
                let joined = val
                    .get("errors")
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|v| v.as_str())
                            .collect::<Vec<_>>()
                            .join("; ")
                    })
                    .filter(|s| !s.is_empty());
                // Fall back to the subtype label (e.g. "error_max_turns") when CC
                // omits `errors[]`, so ResponseFailed still has a non-empty,
                // user-readable reason string.
                //
                // BUT a `subtype` of "success" (or an absent subtype) is NOT a
                // usable failure reason: CC sometimes stamps `is_error: true` on a
                // turn it *also* labels structurally successful. Two shapes:
                //
                //   (1) A genuine upstream API drop CC still labelled successful:
                //       CC's own "API Error: …" message became the final result
                //       text (see the Claude Code error surface — the exact prefix
                //       is `API Error:`, e.g. `API Error: 500 {…}` / `API Error:
                //       Stream idle timeout`). Preserve it as the failure reason —
                //       the streamed text IS the honest cause, and surfacing it
                //       beats the old generic "Unknown error". Matched on a
                //       leading `API Error` prefix (a genuinely successful turn's
                //       result text never *starts* with it), never a loose
                //       substring — so a turn that merely mentions "api error"
                //       mid-sentence is not mis-flagged as Failed.
                //
                //   (2) Everything else — a genuinely completed turn CC merely
                //       mis-stamped (streamed a full response, committed work).
                //       There is nothing actionable to report, and fabricating a
                //       generic "Unknown error" flips it to `Failed` — a red
                //       "Event stream error / Unknown error" on a turn that
                //       produced real output and proposed a change (and it even
                //       trips ResponseFailed-subscribed triggers). Return `None`
                //       and let `classify_result` decide on the turn's ACTUAL
                //       content: real text/tools → `Generated`; produced *nothing*
                //       → still fails, via the accurate empty-response branch.
                //
                // Model-tolerance measure — see docs/temporary-measures.md
                // ("CC is_error:true + subtype:success").
                joined.or_else(|| {
                    let subtype = val.get("subtype").and_then(|v| v.as_str()).unwrap_or("");
                    if subtype.is_empty() || subtype == "success" {
                        text.trim_start()
                            .starts_with("API Error")
                            .then(|| text.trim().to_string())
                    } else {
                        Some(subtype.to_string())
                    }
                })
            } else {
                None
            }
            .or_else(|| state.turn_ending_api_error.take());
            let mut events: Vec<AgentEvent> = state.close_open_call().into_iter().collect();
            events.extend(state.release_held_usage());
            state.end_turn();
            events.push(AgentEvent::Result {
                text,
                duration_ms: duration,
                error,
                provider_error: None,
                turn_end: None,
            });
            events
        }
        // CC 2.1.76+ sends streaming deltas as "type": "stream_event" wrappers.
        // Every one is positive proof the subprocess is alive and actively
        // producing output, so we always emit a content-free StreamActivity ping
        // to keep the watchdog's inactivity clock fresh through a long single step
        // (e.g. extended thinking on a hard problem) — without it the clock only
        // ticks at step boundaries and a step longer than
        // WATCHDOG_INACTIVITY_LIMIT_MS is killed mid-work even while CC streams.
        //
        // Beyond liveness we extract ONE thing WHEN it is present: plaintext
        // reasoning carried by a `content_block_delta` whose `delta.type` is
        // `thinking_delta` (the text rides on `delta.thinking`, not `delta.text`).
        // When the stream carries it, capture it as `AgentEvent::Thought` — the
        // *complete* assistant message keeps thinking as a signature-only block
        // (plaintext stripped from the persisted JSONL), so the live stream is the
        // only place any reasoning text would appear. Streamed text deltas are
        // deliberately NOT taken here: the full assistant text arrives separately
        // as `AgentEvent::Message`, so reading it from the delta too would
        // duplicate it. The StreamActivity ping is always emitted regardless, so
        // the watchdog contract is unchanged.
        //
        // The text a thinking block carries depends on `thinking.display`. With
        // "omitted" the block streams EMPTY (a signature only) and this branch
        // yields nothing. Claude Code sends "updates" for current models, so an
        // always-thinking model streams its short notes before a tool call here.
        // Until a note has arrived (`notes_proven`) the delta is a Thought, and
        // the note that proves it is not repeated from its frame. After that the
        // complete assistant frame carries each note as a message instead. Any
        // other model's `thinking_delta` stays a Thought. The raw chain of thought is never returned: a summary is
        // the most any display mode yields (the `cc-reasoning-dormant`
        // investigation in docs/temporary-measures.md).
        "stream_event" => {
            let mut events = Vec::new();
            let event = val.get("event");
            let event_type = event.and_then(|e| e.get("type")).and_then(|v| v.as_str());
            let message = event.and_then(|e| e.get("message"));
            if event_type == Some("message_start") {
                state.streaming_model = message
                    .and_then(|m| m.get("model"))
                    .and_then(|v| v.as_str())
                    .map(String::from);
            }
            let from_the_parent = parent_tool_use_id.is_none();
            match event_type {
                Some("message_start") if from_the_parent => {
                    events.extend(state.close_open_call());
                    state.open_call = Some(OpenCall {
                        message_id: message
                            .and_then(|m| m.get("id"))
                            .and_then(|v| v.as_str())
                            .map(String::from),
                        model: state.streaming_model.clone(),
                        usage: message
                            .and_then(|m| m.get("usage"))
                            .map(call_usage)
                            .unwrap_or_default(),
                        opened_at: std::time::Instant::now(),
                    });
                }
                Some("message_delta") if from_the_parent => {
                    if let Some(open) = state.open_call.take() {
                        let usage = event.and_then(|e| e.get("usage")).map_or(open.usage, |u| {
                            crate::llm::usage_wire::merge(open.usage, call_usage(u))
                        });
                        let duration_ms = open.duration_ms();
                        events.extend(state.report(
                            open.message_id.as_deref(),
                            open.model,
                            usage,
                            None,
                            duration_ms,
                        ));
                    }
                }
                _ => {}
            }
            let note_arrives_as_message =
                state.thinking_is_a_note(state.streaming_model.as_deref());
            if let Some(text) = event
                .filter(|_| !note_arrives_as_message)
                .filter(|e| e.get("type").and_then(|v| v.as_str()) == Some("content_block_delta"))
                .and_then(|e| e.get("delta"))
                .filter(|d| d.get("type").and_then(|v| v.as_str()) == Some("thinking_delta"))
                // INVARIANT: The reasoning text rides on `delta.thinking`, NOT `delta.text`
                // (only a `text_delta` uses `text`) — matching the chat path's
                // Anthropic-wire parser in `llm/anthropic_wire.rs`. Reading `text`
                // here silently dropped every thought (the original bug).
                .and_then(|d| d.get("thinking"))
                .and_then(|v| v.as_str())
            {
                if !text.is_empty() {
                    events.push(AgentEvent::Thought {
                        text: text.to_string(),
                    });
                }
            }
            events.push(AgentEvent::StreamActivity);
            events
        }
        "control_response" => {
            let response = &val["response"];
            let request_id = response["request_id"].as_str();
            if request_id == Some(BILLING_INITIALIZE_REQUEST_ID) {
                state.billing_account = Some(if response["subtype"].as_str() == Some("success") {
                    crate::runtime::billing_provider::BillingAttribution::claude_initialize(
                        &response["response"],
                    )
                } else {
                    crate::runtime::billing_provider::BillingAttribution::default()
                });
            } else if request_id == Some(BILLING_STATUS_REQUEST_ID) {
                state.billing_default_endpoint = response["subtype"].as_str() == Some("success")
                    && crate::runtime::billing_provider::BillingAttribution::claude_default_endpoint(&response["response"]);
            } else {
                return Vec::new();
            }
            state
                .billing_account
                .clone()
                .map(|account| AgentEvent::BillingObserved {
                    attribution: account.claude_endpoint_checked(state.billing_default_endpoint),
                })
                .into_iter()
                .collect()
        }
        other => {
            if !other.is_empty() {
                log_legacy!("[ClaudeCode] Unrecognized event type: {}", other);
            }
            Vec::new()
        }
    }
}
