//! CC stream-output line parser, split out of claude_code.rs.
use super::*;
use crate::runtime::ReplayedInput;
use std::collections::HashSet;

/// The little parse state that spans lines of ONE Claude Code stdout stream.
/// The driver task owns an instance per spawned process and passes it to every
/// [`parse_line`] call, so the state is per session by construction: two
/// concurrent coding-agent sessions cannot suppress each other's events. Same
/// shape as the Codex side's `TurnTracker` / `AppServerTracker`.
#[derive(Debug, Default)]
pub struct CcStreamState {
    /// Assistant message ids a `Usage` event has already been emitted for in
    /// the turn now in flight. See the dedup comment in the `"assistant"` arm
    /// of [`parse_line`], and [`CcStreamState::end_turn`] for the lifetime.
    usage_reported_message_ids: HashSet<String>,
    /// The session's Vertex calls go through the relay, which asks an
    /// always-thinking model for its notes. Only then is `thinking` text a
    /// note for the user; anywhere else it is reasoning. A temporary measure:
    /// `docs/temporary-measures.md` § "Claude Code's Vertex calls go through
    /// the Vertex relay".
    notes_relayed: bool,
    /// The model of the message now streaming, from its `message_start`. Its
    /// deltas carry no model of their own.
    streaming_model: Option<String>,
    /// The parent's API call now streaming, opened by its `message_start` and
    /// reported at its `message_delta`. See [`CcStreamState::close_open_call`].
    open_call: Option<OpenCall>,
    /// A sub-agent call's `Usage`, by message id, claimed on a frame that drew
    /// nothing (a `thinking` block). It is released after the same message's
    /// next frame, so the capture follows the step it belongs to.
    held_sub_agent_usage: Vec<(String, AgentEvent)>,
}

/// One API call's token counts, as Anthropic's `usage` block spells them.
#[derive(Debug, Clone, Copy, Default)]
struct CallUsage {
    input: u32,
    output: u32,
    cache_read: u32,
    cache_creation: u32,
}

impl CallUsage {
    fn read(usage: &serde_json::Value) -> Self {
        Self::default().updated_by(usage)
    }

    /// These counts with every count `usage` carries taking precedence. A
    /// `message_delta` holds the final counts, and may omit the input ones.
    fn updated_by(self, usage: &serde_json::Value) -> Self {
        let count = |key: &str, fallback: u32| {
            usage
                .get(key)
                .and_then(|v| v.as_u64())
                .map_or(fallback, |n| {
                    crate::llm::clamp_provider_token_count(n, "ClaudeCode")
                })
        };
        Self {
            input: count("input_tokens", self.input),
            output: count("output_tokens", self.output),
            cache_read: count("cache_read_input_tokens", self.cache_read),
            cache_creation: count("cache_creation_input_tokens", self.cache_creation),
        }
    }

    /// CC sometimes emits a continuation message with zeroed usage. No API
    /// call happened, so reporting one would be misleading.
    fn is_from_a_real_call(&self) -> bool {
        self.input > 0 || self.output > 0 || self.cache_read > 0 || self.cache_creation > 0
    }
}

#[derive(Debug)]
struct OpenCall {
    message_id: Option<String>,
    model: Option<String>,
    usage: CallUsage,
}

impl CcStreamState {
    pub fn with_notes_relayed(notes_relayed: bool) -> Self {
        Self {
            notes_relayed,
            ..Self::default()
        }
    }

    /// Whether `model`'s `thinking` text is a progress note for the user.
    fn thinking_is_a_note(&self, model: Option<&str>) -> bool {
        self.notes_relayed
            && model.and_then(crate::llm::anthropic_wire::thinking_mode)
                == Some(crate::llm::anthropic_wire::ThinkingMode::AlwaysOn)
    }

    /// Claim the one `Usage` event this assistant message is entitled to.
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

    /// Release the turn's claims at its terminal `result`. An id belongs to one
    /// API response, so no id from a closed turn can be claimed again and
    /// nothing is lost by forgetting them. What it buys is the bound: the set
    /// holds one turn's messages rather than a whole session's, which matters
    /// on a session that stays up for days. A straggler frame arriving after
    /// the terminal re-reports its usage, which is the safe direction.
    fn end_turn(&mut self) {
        self.usage_reported_message_ids.clear();
    }

    /// The `Usage` event for one API call, unless it is all zero or its
    /// message already reported.
    fn report(
        &mut self,
        message_id: Option<&str>,
        model: Option<String>,
        usage: CallUsage,
        parent_tool_use_id: Option<String>,
    ) -> Option<AgentEvent> {
        // The `&&` order matters: an all-zero frame must not claim the id,
        // so a later real frame carrying the same id still reports.
        (usage.is_from_a_real_call() && self.claim_usage_report(message_id)).then_some(
            AgentEvent::Usage {
                model,
                input_tokens: usage.input,
                output_tokens: usage.output,
                cache_read_tokens: usage.cache_read,
                cache_creation_tokens: usage.cache_creation,
                parent_tool_use_id,
                api_call_id: message_id.map(String::from),
            },
        )
    }

    /// Whether the open call's `message_delta` will report this frame's
    /// message, so the frame itself must not.
    fn reports_at_message_delta(&self, message_id: Option<&str>) -> bool {
        message_id.is_some()
            && self
                .open_call
                .as_ref()
                .is_some_and(|open| open.message_id.as_deref() == message_id)
    }

    /// Hold a sub-agent call's `Usage` until its message draws something.
    fn hold_usage(&mut self, message_id: &str, usage: AgentEvent) {
        self.held_sub_agent_usage
            .push((message_id.to_string(), usage));
    }

    /// The held `Usage` of one message, now that a frame of it drew something.
    fn take_held_usage(&mut self, message_id: &str) -> Vec<AgentEvent> {
        let (taken, kept) = std::mem::take(&mut self.held_sub_agent_usage)
            .into_iter()
            .partition(|(id, _)| id == message_id);
        self.held_sub_agent_usage = kept;
        taken.into_iter().map(|(_, usage)| usage).collect()
    }

    /// Every held `Usage`, for a turn or a stream that ended before the
    /// messages drew anything. The provider billed them either way.
    pub fn release_held_usage(&mut self) -> Vec<AgentEvent> {
        std::mem::take(&mut self.held_sub_agent_usage)
            .into_iter()
            .map(|(_, usage)| usage)
            .collect()
    }

    /// Report the open call with the counts its `message_start` gave. This
    /// runs when no `message_delta` came: the next call started, the turn
    /// ended mid-stream, or the stream itself ended. The driver calls it once
    /// stdout is done, since a killed process writes no `result`. The provider
    /// billed the prompt either way.
    pub fn close_open_call(&mut self) -> Option<AgentEvent> {
        let open = self.open_call.take()?;
        // Only the parent's calls open (the `stream_event` arm).
        self.report(open.message_id.as_deref(), open.model, open.usage, None)
    }
}

/// Render a CC tool result's `content` field as the string the step shows.
///
/// CC sends `content` either as a plain string or as an ARRAY of blocks, and
/// reading it with `as_str()` alone dropped every array to `""`. That blanked
/// 1,530 steps in 30 days on one workspace: subagent reports (`Agent`),
/// `ToolSearch` results, and image reads. Measurements and the four observed
/// shapes are in
/// `docs/plans/2026-08-26-cc-tool-results-and-showing-the-user-an-image.md`.
///
/// Both call sites below resolve `content` through here, because the bug
/// existed in each of them and was fixed in neither.
fn tool_result_content(content: Option<&serde_json::Value>) -> String {
    match content {
        // Absent is the one shape that IS empty. Everything present renders.
        None | Some(serde_json::Value::Null) => String::new(),
        Some(serde_json::Value::String(s)) => s.clone(),
        Some(serde_json::Value::Array(blocks)) => blocks
            .iter()
            .map(describe_content_block)
            .collect::<Vec<_>>()
            .join("\n"),
        // A lone block sent unwrapped. It must take the SAME path a wrapped
        // one does, or an unwrapped image would spill base64 the array form
        // is careful to label.
        Some(obj) if obj.get("type").is_some() => describe_content_block(obj),
        // A shape CC has never sent, carrying no block type to dispatch on.
        // Its JSON beats dropping it, and the 200-char cap in
        // `run_session/run.rs` bounds what that can cost.
        Some(other) => other.to_string(),
    }
}

/// One block of an array-shaped tool result, as a short line of text.
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
    // A bare string element carries its own text and has no `type` to read.
    if let Some(s) = block.as_str() {
        return s.to_string();
    }
    let kind = block
        .get("type")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");
    match (kind, block.get("text").and_then(|v| v.as_str())) {
        // `text` is the one block readable as-is, and only when it has one.
        // Missing its text takes the label path below instead.
        ("text", Some(t)) => t.to_string(),
        // A `tool_reference` names its subject, and that name is the only
        // part a reader can act on.
        _ => match block.get("tool_name").and_then(|v| v.as_str()) {
            Some(name) => format!("[{kind}: {name}]"),
            None => format!("[{kind}]"),
        },
    }
}

/// A replay of an input the engine wrote to stdin (`--replay-user-messages`).
/// A tool-result line is never one, whatever its flag. A tool result is Claude
/// Code's own output, not an input it took in.
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

/// What a replay carried: its text blocks and how many images. A string
/// content is one text block.
fn replayed_input(val: &serde_json::Value) -> ReplayedInput {
    let content = val.get("message").and_then(|m| m.get("content"));
    if let Some(text) = content.and_then(|c| c.as_str()) {
        return ReplayedInput {
            texts: vec![text.to_string()],
            images: 0,
        };
    }
    let blocks = content.and_then(|c| c.as_array()).map_or(&[][..], |b| b);
    let of_type = |kind: &'static str| {
        blocks
            .iter()
            .filter(move |b| b.get("type").and_then(|t| t.as_str()) == Some(kind))
    };
    ReplayedInput {
        texts: of_type("text")
            .filter_map(|b| b.get("text").and_then(|t| t.as_str()))
            .map(str::to_string)
            .collect(),
        images: of_type("image").count(),
    }
}

/// Parse a single JSON line from Claude Code's stream output.
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
    // A sub-agent's lines ride the parent's stream, each naming the `Agent`
    // call that spawned it.
    let parent_tool_use_id = val
        .get("parent_tool_use_id")
        .and_then(|v| v.as_str())
        .map(String::from);

    match event_type {
        // Only parse subtype "init" — hook events (hook_started, hook_response,
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
                    vec![AgentEvent::Init {
                        session_id: sid.to_string(),
                        model,
                        slash_commands,
                        skills,
                        agent_version,
                    }]
                } else {
                    Vec::new()
                }
            } else {
                Vec::new()
            }
        }
        "assistant" => {
            let mut events = Vec::new();
            let message = val.get("message");
            // CC's own error banner ("API Error: Stream idle timeout - no chunks
            // received", "API Error: Response stalled mid-stream. …") arrives as a
            // SYNTHETIC assistant message: `message.model` is `<synthetic>` and the
            // line carries `is_api_error_message: true`, the stream-json name for
            // CC's internal `isApiErrorMessage`, documented in its SDK schema as
            // "True when this assistant message wraps an API error".
            //
            // It is CC's error SURFACE, not model prose. The same string comes back
            // as the turn's `result` error and becomes `ResponseFailed`, which the
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
            let notes_are_text = state.thinking_is_a_note(model);
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
                        // The trailing break flushes the note at once, so it
                        // shows while the model keeps thinking.
                        "thinking" if notes_are_text && !is_api_error_banner => {
                            if let Some(note) = block
                                .get("thinking")
                                .and_then(|v| v.as_str())
                                .and_then(crate::llm::anthropic_wire::progress_note)
                            {
                                events.push(prose(format!("{note}\n\n")));
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
            // A `Usage` event is how the consumer emits `ContextCaptured`, but
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
                        CallUsage::read(usage),
                        parent_tool_use_id,
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
        // CC 2.1.76+ sends tool results as "type": "user" with tool_result content
        // blocks. A replay of a stdin input is a "user" line too, flagged
        // `isReplay`, and it is how Claude Code reports the input read.
        "user" => {
            let mut events = Vec::new();
            if is_input_replay(&val) {
                events.push(AgentEvent::InputRead(Some(replayed_input(&val))));
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
                        // Contradictory success + is_error, no errors[]: preserve
                        // only CC's own "API Error: …" message (a genuine drop),
                        // else None so classify_result decides on content.
                        text.trim_start()
                            .starts_with("API Error")
                            .then(|| text.trim().to_string())
                    } else {
                        Some(subtype.to_string())
                    }
                })
            } else {
                None
            };
            // A call still open here was cut off before its `message_delta`.
            // Then the turn is over, so the ids it reported can go.
            let mut events: Vec<AgentEvent> = state.close_open_call().into_iter().collect();
            events.extend(state.release_held_usage());
            state.end_turn();
            events.push(AgentEvent::Result {
                text,
                duration_ms: duration,
                error,
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
        // DORMANT TODAY — and NOT provider-specific (corrected 2026-07-02). For the
        // current models (Fable 5, Opus 4.8/4.7, Sonnet 5) Anthropic's
        // `thinking.display` defaults to "omitted", so thinking blocks stream with
        // EMPTY text (encrypted signature only) and no `thinking_delta` ever
        // arrives — this branch produces nothing. That holds on BOTH Vertex AND the
        // first-party Anthropic API (verified empirically on `.claude-personal`:
        // one `signature_delta`, zero `thinking_delta`), and even
        // `--thinking-display summarized` does not populate it through Claude Code's
        // headless `--output-format stream-json` path (an upstream CC limitation —
        // GitHub anthropics/claude-code#7840, #56356; and the raw chain of thought
        // is never returned regardless — a summary is the most any display mode
        // yields). So `CodingAgentThoughtStreamed` stays empty for these models
        // regardless of provider or flag; switching CC's provider does NOT fix it.
        // See the `cc-reasoning-dormant` investigation in docs/temporary-measures.md.
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
            // Only the parent's own calls open and close here. A sub-agent's
            // line names its spawning tool call, and its calls report from
            // their frames instead (the `"assistant"` arm).
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
                            .map(CallUsage::read)
                            .unwrap_or_default(),
                    });
                }
                Some("message_delta") if from_the_parent => {
                    if let Some(open) = state.open_call.take() {
                        let usage = event
                            .and_then(|e| e.get("usage"))
                            .map_or(open.usage, |u| open.usage.updated_by(u));
                        events.extend(state.report(
                            open.message_id.as_deref(),
                            open.model,
                            usage,
                            None,
                        ));
                    }
                }
                _ => {}
            }
            // A relayed note arrives whole as a message, from the complete
            // assistant frame. Streaming it as a Thought too would show it twice.
            let note_arrives_as_message =
                state.thinking_is_a_note(state.streaming_model.as_deref());
            if let Some(text) = event
                .filter(|_| !note_arrives_as_message)
                .filter(|e| e.get("type").and_then(|v| v.as_str()) == Some("content_block_delta"))
                .and_then(|e| e.get("delta"))
                .filter(|d| d.get("type").and_then(|v| v.as_str()) == Some("thinking_delta"))
                // The reasoning text rides on `delta.thinking`, NOT `delta.text`
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
        // control_response is CC's reply to a control_request (e.g. interrupt).
        // We don't need to act on it — the interrupt itself triggers a Result event.
        "control_response" => Vec::new(),
        other => {
            if !other.is_empty() {
                log!("[ClaudeCode] Unrecognized event type: {}", other);
            }
            Vec::new()
        }
    }
}
