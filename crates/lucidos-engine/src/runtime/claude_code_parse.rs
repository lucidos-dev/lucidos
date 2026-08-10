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
            // therefore printed the failure twice: once as a paragraph glued into
            // the response body (the engine concatenates consecutive assistant
            // messages with no separator, so it ran on mid-sentence), and again in
            // the red card right beneath it.
            //
            // Only the text is skipped. A synthetic error line carries no tool_use
            // and zeroed usage, so nothing else is lost, and a sub-agent's banner,
            // which rides the parent's stream with the same flag, stops leaking into
            // the parent's prose too.
            let is_api_error_banner = val
                .get("is_api_error_message")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            if let Some(content) = message
                .and_then(|m| m.get("content"))
                .and_then(|c| c.as_array())
            {
                for block in content {
                    let block_type = block.get("type").and_then(|v| v.as_str()).unwrap_or("");
                    match block_type {
                        "text" if !is_api_error_banner => {
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
            if let Some(text) = val
                .get("event")
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
