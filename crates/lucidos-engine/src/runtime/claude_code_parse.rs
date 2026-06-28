//! CC stream-output line parser, split out of claude_code.rs.
use super::*;

/// Parse a single JSON line from Claude Code's stream output.
/// Returns all recognized events from the line. An assistant message with
/// multiple content blocks (text + tool_use) produces multiple events.
/// Never produces `AgentEvent::Exited` — that variant is emitted by the
/// driver task on process exit.
pub fn parse_line(line: &str) -> Vec<AgentEvent> {
    let line = line.trim();
    if line.is_empty() {
        return Vec::new();
    }

    let val: serde_json::Value = match serde_json::from_str(line) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };
    let event_type = val.get("type").and_then(|v| v.as_str()).unwrap_or("");

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
                    vec![AgentEvent::Init {
                        session_id: sid.to_string(),
                        model,
                        slash_commands,
                        skills,
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
            if let Some(content) = message
                .and_then(|m| m.get("content"))
                .and_then(|c| c.as_array())
            {
                for block in content {
                    let block_type = block.get("type").and_then(|v| v.as_str()).unwrap_or("");
                    match block_type {
                        "text" => {
                            if let Some(text) = block.get("text").and_then(|v| v.as_str()) {
                                events.push(AgentEvent::Message {
                                    role: "assistant".to_string(),
                                    text: text.to_string(),
                                });
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
                            events.push(AgentEvent::ToolUse { name, input, id });
                        }
                        _ => {}
                    }
                }
            }
            // CC mirrors Anthropic's `usage` block on each assistant
            // message (one per LLM API call). Surfaced as a separate
            // `Usage` event so the consumer can emit `ContextCaptured`.
            if let Some(usage) = message.and_then(|m| m.get("usage")) {
                let model = message
                    .and_then(|m| m.get("model"))
                    .and_then(|v| v.as_str())
                    .map(String::from);
                let input_tokens = crate::llm::clamp_provider_token_count(
                    usage
                        .get("input_tokens")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0),
                    "ClaudeCode",
                );
                let output_tokens = crate::llm::clamp_provider_token_count(
                    usage
                        .get("output_tokens")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0),
                    "ClaudeCode",
                );
                let cache_read_tokens = crate::llm::clamp_provider_token_count(
                    usage
                        .get("cache_read_input_tokens")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0),
                    "ClaudeCode",
                );
                let cache_creation_tokens = crate::llm::clamp_provider_token_count(
                    usage
                        .get("cache_creation_input_tokens")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0),
                    "ClaudeCode",
                );
                // Skip empty-usage frames (CC sometimes emits a continuation
                // assistant message with zeroed usage — no real API call
                // happened, so a snapshot would be misleading).
                if input_tokens > 0
                    || output_tokens > 0
                    || cache_read_tokens > 0
                    || cache_creation_tokens > 0
                {
                    events.push(AgentEvent::Usage {
                        model,
                        input_tokens,
                        output_tokens,
                        cache_read_tokens,
                        cache_creation_tokens,
                    });
                }
            }
            events
        }
        "tool_result" => {
            let content = val
                .get("content")
                .and_then(|c| c.as_str())
                .unwrap_or("")
                .to_string();
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
            }]
        }
        // CC 2.1.76+ sends tool results as "type": "user" with tool_result content blocks
        "user" => {
            let mut events = Vec::new();
            if let Some(content) = val
                .get("message")
                .and_then(|m| m.get("content"))
                .and_then(|c| c.as_array())
            {
                for block in content {
                    if block.get("type").and_then(|v| v.as_str()) == Some("tool_result") {
                        let output = block
                            .get("content")
                            .and_then(|c| c.as_str())
                            .unwrap_or("")
                            .to_string();
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
                // Fall back to the subtype label (e.g. "error_max_turns") when
                // CC omits `errors` so ResponseFailed still has a non-empty
                // user-facing reason string. Skip "success" — CC sometimes
                // emits `is_error: true` with `subtype: "success"` (observed
                // on upstream API drops after the conversation has streamed
                // an `API Error: …` text). Using it would render as
                // `[ERROR] **Error:** success` in the timeline.
                Some(joined.unwrap_or_else(|| {
                    let subtype = val.get("subtype").and_then(|v| v.as_str()).unwrap_or("");
                    if subtype.is_empty() || subtype == "success" {
                        "Unknown error".to_string()
                    } else {
                        subtype.to_string()
                    }
                }))
            } else {
                None
            };
            vec![AgentEvent::Result {
                text,
                duration_ms: duration,
                error,
            }]
        }
        // CC 2.1.76+ sends streaming deltas as "type": "stream_event" wrappers.
        // Every one is positive proof the subprocess is alive and actively
        // producing output, so we always emit a content-free StreamActivity ping
        // to keep the watchdog's inactivity clock fresh through a long single step
        // (e.g. extended thinking on a hard problem) — without it the clock only
        // ticks at step boundaries and a step longer than
        // WATCHDOG_INACTIVITY_LIMIT_MS is killed mid-work even while CC streams.
        //
        // Beyond liveness we extract ONE thing: the plaintext reasoning carried by
        // a `content_block_delta` whose `delta.type` is `thinking_delta`. CC's
        // *complete* assistant message keeps thinking as a signature-only block
        // (plaintext stripped from the persisted JSONL), so the live stream is the
        // only place the reasoning text exists — capture it as `AgentEvent::Thought`
        // or it is lost. Streamed text deltas are deliberately NOT taken here: the
        // full assistant text arrives separately as `AgentEvent::Message`, so
        // reading it from the delta too would duplicate it. The StreamActivity ping
        // is always emitted regardless, so the watchdog contract is unchanged.
        "stream_event" => {
            let mut events = Vec::new();
            if let Some(text) = val
                .get("event")
                .filter(|e| e.get("type").and_then(|v| v.as_str()) == Some("content_block_delta"))
                .and_then(|e| e.get("delta"))
                .filter(|d| d.get("type").and_then(|v| v.as_str()) == Some("thinking_delta"))
                .and_then(|d| d.get("text"))
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
