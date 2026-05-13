use super::types::*;
use super::EventStore;
use crate::core::EventRow;
use chrono::{DateTime, Utc};

/// Number of recent (ToolCalled, ToolResult) pairs reconstructed verbatim as
/// `Message::Blocks(...)` pairs on resume — see
/// [`build_resume_tool_blocks_with_skip_ids`]. `load_knowhow` results are
/// pinned regardless of N.
pub(crate) const RESUME_VERBATIM_TOOL_TAIL: usize = 3;

/// Tool names whose results survive the N-most-recent window. `load_knowhow`
/// returns reference material (a procedure body) that doesn't decay across
/// turns, so multi-step orchestrators keep their recipe across callback
/// resumes.
const PINNED_TOOL_NAMES: &[&str] = &[crate::llm::tool_names::LOAD_KNOWHOW];

/// Stable synthetic `tool_use_id` used by
/// [`build_resume_tool_blocks_with_skip_ids`] to pair the reconstructed
/// `ToolUse` and `ToolResult` blocks. Same event id → same synthetic id
/// across resumes (deterministic, idempotent).
///
/// Renders the full 32-hex-char simple-form UUID (`evt-<32 hex>`). The
/// `dismiss_from_context` tool handler accepts this form (and the bare
/// hyphenated/simple UUID) so the LLM can pass any tool-block id from
/// history directly back to dismiss without truncation guessing.
fn synthesize_tool_use_id(tool_called_event_id: &uuid::Uuid) -> String {
    format!("evt-{}", tool_called_event_id.simple())
}

/// Format a persisted `ChildThreadCompleted` event row as the `[CHILD THREAD
/// COMPLETED]` user-channel block the parent LLM sees in its conversation
/// history. Shared by [`build_session_messages`] (projects every typed
/// completion at LLM call setup) and the agentic loop's wake-from-child
/// injection drain (projects a single event inline as the next user
/// message). Both paths produce identical text.
pub fn format_child_thread_completed_block(event: &EventRow) -> String {
    use crate::engine::thread_events::ChildCompletionStatus;

    let child_thread_id = event
        .payload
        .get("child_thread_id")
        .and_then(|v| v.as_str())
        .unwrap_or("?");
    let title = event
        .payload
        .get("child_thread_title")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    // Exhaustive match against the typed enum so a new ChildCompletionStatus
    // variant becomes a compile error here, not a silent fall-through to
    // bare "completed".
    let status = match serde_json::from_value::<ChildCompletionStatus>(
        event
            .payload
            .get("status")
            .cloned()
            .unwrap_or(serde_json::Value::Null),
    ) {
        Ok(ChildCompletionStatus::Success) => "completed (success)",
        Ok(ChildCompletionStatus::Failure) => "completed (failure)",
        Ok(ChildCompletionStatus::NoChanges) => "completed (no changes)",
        Ok(ChildCompletionStatus::Canceled) => "canceled (user stop)",
        Err(_) => "completed",
    };
    let summary = event
        .payload
        .get("summary")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let pending_change_ids: Vec<String> = event
        .payload
        .get("pending_change_ids")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    let pending_section = if pending_change_ids.is_empty() {
        "none".to_string()
    } else {
        pending_change_ids.join(", ")
    };
    let title_line = if title.is_empty() {
        String::new()
    } else {
        format!("\nTitle: {}", title)
    };
    let summary_section = if summary.is_empty() {
        String::new()
    } else {
        format!("\nSummary: {}", summary)
    };
    // The event_id (not child_thread_id) is what dismiss_from_context
    // accepts as the lookup key — surface it so the LLM can pass it back
    // verbatim without inventing a synthetic prefix.
    format!(
        "[CHILD THREAD COMPLETED] {} {}\nevent_id: {}{}\nPending changes: {}{}\n\
         Note: phrases like \"session can finish\" or \"## Session Summary\" in \
         the summary describe the child subprocess only — if you were following \
         a multi-step procedure, continue with the next step. Otherwise use \
         run_thread to refine.",
        child_thread_id, status, event.id, title_line, pending_section, summary_section
    )
}

/// Build session messages from a list of events (pure function, no DB access).
///
/// Interruption semantics:
/// - `completed: Some(false)` — the response was interrupted by a follow-up user message
///   arriving mid-stream (a `MessageReceived` event flushed the text buffer before the
///   response could complete with `ResponseGenerated`).
/// - `completed: None` — still in progress or unknown (trailing buffer flush at end of events).
/// - `completed: Some(true)` — completed normally (`ResponseGenerated` or `ResponseFailed`).
pub(crate) fn build_session_messages(events: &[EventRow]) -> Vec<SessionMessage> {
    let mut messages: Vec<SessionMessage> = Vec::new();
    let mut pending_steps: Vec<Step> = Vec::new();
    let mut pending_images: Vec<String> = Vec::new();
    let mut claude_code_text_buf = String::new();
    let mut claude_code_text_last_ts: Option<DateTime<Utc>> = None;
    let mut text_buf = String::new();
    let mut text_last_ts: Option<DateTime<Utc>> = None;
    let mut pending_text_chunks: Vec<String> = Vec::new();
    let mut last_cc_chunk_len: usize = 0;
    let mut last_text_chunk_len: usize = 0;
    let mut pending_events: Vec<ResponseEvent> = Vec::new();
    let mut last_cc_event_len: usize = 0;
    let mut last_text_event_len: usize = 0;
    let mut current_request_event_id: Option<String> = None;
    let mut current_thread_id: Option<String> = None;

    // `ContextDismissed` records (emitted by the agent's `dismiss_from_context`
    // tool) ask the projection to drop the corresponding history entry on every
    // future read. The set is collected up-front so dismissals can land out of
    // order relative to the dismissed event itself — emitting a dismissal
    // before its target is still respected.
    let dismissed_event_ids = collect_dismissed_event_ids(events);

    // Helper: extract thread_id from the event column
    let get_thread_id =
        |event: &EventRow| -> Option<String> { event.thread_id.map(|uuid| uuid.to_string()) };

    for event in events {
        // Skip dismissed entries before any per-event handling so accumulated
        // text buffers / pending steps stay untouched. Today this only fires
        // for `ChildThreadCompleted` (`ToolCalled` + `ToolResult` skipping
        // happens at the resume helper level so the projection can still show
        // step rows in the UI).
        if event.event_type == "ChildThreadCompleted"
            && dismissed_event_ids.contains(&event.id.to_string())
        {
            continue;
        }
        match event.event_type.as_str() {
            "MessageReceived" | "UserMessage" => {
                // Flush any accumulated streaming text as a response
                // before starting a new user message (e.g. follow-up sent mid-stream).
                // completed: false — the exchange was interrupted, no ResponseGenerated.
                if !claude_code_text_buf.is_empty() {
                    // Snapshot remaining text delta as a final chunk
                    if claude_code_text_buf.len() > last_cc_chunk_len {
                        pending_text_chunks
                            .push(claude_code_text_buf[last_cc_chunk_len..].to_string());
                    }
                    // Snapshot remaining text as event
                    if claude_code_text_buf.len() > last_cc_event_len {
                        pending_events.push(ResponseEvent::Text {
                            md: claude_code_text_buf[last_cc_event_len..].to_string(),
                        });
                    }
                    let created_at = claude_code_text_last_ts.take().unwrap_or(event.created);
                    messages.push(SessionMessage {
                        role: "assistant".to_string(),
                        content: std::mem::take(&mut claude_code_text_buf),
                        created_at,
                        channel: Some("claude_code".to_string()),
                        steps: std::mem::take(&mut pending_steps),
                        images: std::mem::take(&mut pending_images),
                        user_image_hashes: vec![],
                        image_description: None,
                        completed: Some(false),
                        canceled: false,
                        aborted: false,
                        text_chunks: std::mem::take(&mut pending_text_chunks),
                        events: std::mem::take(&mut pending_events),

                        request_event_id: current_request_event_id.clone(),
                        event_id: None,
                        thread_id: current_thread_id.clone(),
                    });
                    last_cc_chunk_len = 0;
                    last_cc_event_len = 0;
                }
                if !text_buf.is_empty() {
                    // Snapshot remaining text delta as a final chunk
                    if text_buf.len() > last_text_chunk_len {
                        pending_text_chunks.push(text_buf[last_text_chunk_len..].to_string());
                    }
                    // Snapshot remaining text as event
                    if text_buf.len() > last_text_event_len {
                        pending_events.push(ResponseEvent::Text {
                            md: text_buf[last_text_event_len..].to_string(),
                        });
                    }
                    let created_at = text_last_ts.take().unwrap_or(event.created);
                    messages.push(SessionMessage {
                        role: "assistant".to_string(),
                        content: std::mem::take(&mut text_buf),
                        created_at,
                        channel: None,
                        steps: std::mem::take(&mut pending_steps),
                        images: std::mem::take(&mut pending_images),
                        user_image_hashes: vec![],
                        image_description: None,
                        completed: Some(false),
                        canceled: false,
                        aborted: false,
                        text_chunks: std::mem::take(&mut pending_text_chunks),
                        events: std::mem::take(&mut pending_events),

                        request_event_id: current_request_event_id.clone(),
                        event_id: None,
                        thread_id: current_thread_id.clone(),
                    });
                    last_text_chunk_len = 0;
                    last_text_event_len = 0;
                }

                let content = event
                    .payload
                    .get("text")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();

                // Hashes are written by the API layer (or backfilled by the
                // startup migration). Old DB rows whose payload still carries
                // the legacy `images: [{base64, mime_type}, ...]` shape have
                // been rewritten to `user_image_hashes` before HTTP binds —
                // see `core::image_migration`. Reading from `images` is
                // therefore a dead fallback and intentionally not implemented.
                let user_image_hashes: Vec<String> = event
                    .payload
                    .get("user_image_hashes")
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|h| h.as_str().map(str::to_string))
                            .collect()
                    })
                    .unwrap_or_default();

                pending_steps.clear();
                pending_images.clear();
                pending_events.clear();

                let image_description = event
                    .payload
                    .get("image_description")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());

                let channel = event
                    .payload
                    .get("channel")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());

                current_request_event_id = Some(event.id.to_string());
                current_thread_id = get_thread_id(event);

                messages.push(SessionMessage {
                    role: "user".to_string(),
                    content,
                    created_at: event.created,
                    channel,
                    steps: vec![],
                    images: vec![],
                    user_image_hashes,
                    image_description,
                    completed: None,
                    canceled: false,
                    aborted: false,
                    text_chunks: vec![],
                    events: vec![],
                    request_event_id: None,
                    event_id: Some(event.id.to_string()),
                    thread_id: current_thread_id.clone(),
                });
            }
            "Thinking" => {
                let context_tokens = event
                    .payload
                    .get("context_tokens")
                    .and_then(|v| v.as_u64())
                    .map(|v| v as usize);
                let context_messages = event
                    .payload
                    .get("context_messages")
                    .and_then(|v| v.as_u64())
                    .map(|v| v as usize);
                let trimmed = event.payload.get("trimmed").and_then(|v| v.as_bool());
                pending_steps.push(Step {
                    description: "Requesting".to_string(),
                    tool_name: None,
                    success: true,
                    context_tokens,
                    context_messages,
                    trimmed,
                    tool_called_event_id: None,
                });
                pending_events.push(ResponseEvent::Step {
                    description: "Requesting".to_string(),
                    tool_name: None,
                    success: true,
                    detail: None,
                    context_tokens,
                    context_messages,
                    trimmed,
                });
            }
            "MemorySearched" => {
                let has_results = event
                    .payload
                    .get("results")
                    .and_then(|v| v.as_u64())
                    .is_some_and(|n| n > 0);
                let queries: Vec<String> = event
                    .payload
                    .get("queries")
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|v| v.as_str().map(String::from))
                            .collect()
                    })
                    .unwrap_or_default();
                let desc = if has_results { "Memory searched" } else { "Memory: no results" };
                let detail = if queries.is_empty() {
                    None
                } else {
                    Some(queries.join(", "))
                };
                pending_steps.push(Step {
                    description: desc.to_string(),
                    tool_name: None,
                    success: true,
                    context_tokens: None,
                    context_messages: None,
                    trimmed: None,
                    tool_called_event_id: None,
                });
                pending_events.push(ResponseEvent::Step {
                    description: desc.to_string(),
                    tool_name: None,
                    success: true,
                    detail,
                    context_tokens: None,
                    context_messages: None,
                    trimmed: None,
                });
            }
            "ToolCalled" => {
                // Snapshot new text since last chunk as a delta before the tool call.
                // Skip if the delta is trivially small (< 80 chars) — merge it into the
                // next chunk instead, so the More/Less toggle only appears when collapsing
                // would hide meaningful content.
                if claude_code_text_buf.len() > last_cc_chunk_len {
                    let delta = &claude_code_text_buf[last_cc_chunk_len..];
                    if delta.len() >= 80 {
                        pending_text_chunks.push(delta.to_string());
                        last_cc_chunk_len = claude_code_text_buf.len();
                    }
                } else if text_buf.len() > last_text_chunk_len {
                    let delta = &text_buf[last_text_chunk_len..];
                    if delta.len() >= 80 {
                        pending_text_chunks.push(delta.to_string());
                        last_text_chunk_len = text_buf.len();
                    }
                }

                // For events: snapshot text delta (no minimum size — interleaving
                // with step events matters more than avoiding small text blocks)
                if claude_code_text_buf.len() > last_cc_event_len {
                    let delta = &claude_code_text_buf[last_cc_event_len..];
                    pending_events.push(ResponseEvent::Text {
                        md: delta.to_string(),
                    });
                    last_cc_event_len = claude_code_text_buf.len();
                } else if text_buf.len() > last_text_event_len {
                    let delta = &text_buf[last_text_event_len..];
                    pending_events.push(ResponseEvent::Text {
                        md: delta.to_string(),
                    });
                    last_text_event_len = text_buf.len();
                }

                let (tool_name, description) = super::describe_tool_event(event);
                pending_steps.push(Step {
                    description: description.clone(),
                    tool_name: Some(tool_name.clone()),
                    success: true,
                    context_tokens: None,
                    context_messages: None,
                    trimmed: None,
                    tool_called_event_id: Some(event.id.to_string()),
                });
                pending_events.push(ResponseEvent::Step {
                    description,
                    tool_name: Some(tool_name),
                    success: true,
                    detail: None,
                    context_tokens: None,
                    context_messages: None,
                    trimmed: None,
                });
            }
            "ToolResult" => {
                let success = event
                    .payload
                    .get("success")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(true); // missing field = legacy or pre-Phase-0; bias to success

                if let Some(last_step) = pending_steps.last_mut() {
                    last_step.success = success;
                }

                // Track screenshots for image embedding
                let tool_name = event
                    .payload
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");

                let result_text = event
                    .payload
                    .get("result")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let detail = super::super::describe_tool_result(tool_name, result_text, success);

                // Update the last step event with success and detail
                if let Some(ResponseEvent::Step {
                    success: ref mut s,
                    detail: ref mut d,
                    ..
                }) = pending_events
                    .iter_mut()
                    .rev()
                    .find(|e| matches!(e, ResponseEvent::Step { .. }))
                {
                    *s = success;
                    *d = detail;
                }

                if tool_name == "browser_screenshot" && success {
                    if let Some(result) = event.payload.get("result").and_then(|v| v.as_str()) {
                        if let Some(start) = result.find("screenshots/") {
                            let path_part = &result[start..];
                            let end = path_part
                                .find(['"', '\n', ' ', ')'])
                                .unwrap_or(path_part.len());
                            let path = &path_part[..end];
                            pending_images.push(path.to_string());
                        }
                    }
                }
            }
            "CodingAgentTextStreamed" | "TextStreamed" => {
                if let Some(text) = event.payload.get("text").and_then(|v| v.as_str()) {
                    let is_cc = event.event_type == "CodingAgentTextStreamed"
                        || event.payload.get("channel").and_then(|v| v.as_str())
                            == Some("claude_code");
                    if is_cc {
                        claude_code_text_buf.push_str(text);
                        claude_code_text_last_ts = Some(event.created);
                    } else {
                        text_buf.push_str(text);
                        text_last_ts = Some(event.created);
                    }
                }
            }
            "ResponseAborted" | "ResponseCanceled" | "ResponseGenerated" | "AssistantResponse" => {
                let is_canceled = event.event_type == "ResponseCanceled";
                let is_aborted = event.event_type == "ResponseAborted";

                let result_text = event
                    .payload
                    .get("text")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();

                // Snapshot remaining text after the last tool call as a final chunk
                if claude_code_text_buf.len() > last_cc_chunk_len {
                    pending_text_chunks.push(claude_code_text_buf[last_cc_chunk_len..].to_string());
                    last_cc_chunk_len = claude_code_text_buf.len();
                } else if text_buf.len() > last_text_chunk_len {
                    pending_text_chunks.push(text_buf[last_text_chunk_len..].to_string());
                    last_text_chunk_len = text_buf.len();
                }

                // Snapshot remaining text as event
                if claude_code_text_buf.len() > last_cc_event_len {
                    pending_events.push(ResponseEvent::Text {
                        md: claude_code_text_buf[last_cc_event_len..].to_string(),
                    });
                } else if text_buf.len() > last_text_event_len {
                    pending_events.push(ResponseEvent::Text {
                        md: text_buf[last_text_event_len..].to_string(),
                    });
                }

                // Capture any result_text content not covered by streaming events.
                // The CC Result event may contain text beyond what was streamed via
                // Message events (e.g., a final summary). Events-based rendering uses
                // only events, so uncovered text would be invisible in the UI.
                let is_cc_channel =
                    event.payload.get("channel").and_then(|v| v.as_str()) == Some("claude_code");
                if is_cc_channel && !result_text.is_empty() && !claude_code_text_buf.is_empty() {
                    let buf = claude_code_text_buf.trim();
                    let result = result_text.trim();
                    if result.len() > buf.len() && result.starts_with(buf) {
                        let extra = result[buf.len()..].trim();
                        if !extra.is_empty() {
                            pending_events.push(ResponseEvent::Text {
                                md: extra.to_string(),
                            });
                        }
                    }
                } else if !is_cc_channel && !result_text.is_empty() && !text_buf.is_empty() {
                    let buf = text_buf.trim();
                    let result = result_text.trim();
                    if result.len() > buf.len() && result.starts_with(buf) {
                        let extra = result[buf.len()..].trim();
                        if !extra.is_empty() {
                            pending_events.push(ResponseEvent::Text {
                                md: extra.to_string(),
                            });
                        }
                    }
                } else if !result_text.is_empty()
                    && pending_events
                        .iter()
                        .any(|e| matches!(e, ResponseEvent::Step { .. }))
                    && !pending_events
                        .iter()
                        .any(|e| matches!(e, ResponseEvent::Text { .. }))
                {
                    // No streaming occurred but step events exist (e.g. Thinking).
                    // The frontend uses the events-based rendering path when events
                    // are present, but without a text event the response is invisible.
                    pending_events.push(ResponseEvent::Text {
                        md: result_text.clone(),
                    });
                }

                // For Claude Code responses, the streaming text (CodingAgentTextStreamed)
                // and the result contain the same content. Avoid duplication:
                // - If streamed text exists and differs from result, prepend it (legacy compat)
                // - If they're the same content, just use the result text
                let channel = event
                    .payload
                    .get("channel")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());

                let content = if channel.as_deref() == Some("claude_code")
                    && !claude_code_text_buf.is_empty()
                {
                    // Dedup: if the streamed text is a prefix of or equal to the result, skip it
                    if result_text.starts_with(claude_code_text_buf.trim())
                        || claude_code_text_buf.trim() == result_text.trim()
                    {
                        claude_code_text_buf.clear();
                        claude_code_text_last_ts = None;
                        last_cc_chunk_len = 0;
                        result_text
                    } else {
                        let full = format!("{}\n\n{}", claude_code_text_buf, result_text);
                        claude_code_text_buf.clear();
                        claude_code_text_last_ts = None;
                        last_cc_chunk_len = 0;
                        full
                    }
                } else if !text_buf.is_empty() {
                    // Dedup TextStreamed against ResponseGenerated
                    if result_text.starts_with(text_buf.trim())
                        || text_buf.trim() == result_text.trim()
                    {
                        text_buf.clear();
                        text_last_ts = None;
                        last_text_chunk_len = 0;
                        result_text
                    } else {
                        let full = format!("{}\n\n{}", text_buf, result_text);
                        text_buf.clear();
                        text_last_ts = None;
                        last_text_chunk_len = 0;
                        full
                    }
                } else {
                    result_text
                };

                // Check for images stored in the event payload (new events)
                let stored_images: Vec<String> = event
                    .payload
                    .get("images")
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|v| v.as_str().map(String::from))
                            .collect()
                    })
                    .unwrap_or_default();

                let images = if !stored_images.is_empty() {
                    stored_images
                } else {
                    std::mem::take(&mut pending_images)
                };

                // Prefer explicit request_event_id from payload; fall back to positional tracking
                let user_eid = event
                    .payload
                    .get("request_event_id")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
                    .or_else(|| current_request_event_id.clone());

                messages.push(SessionMessage {
                    role: "assistant".to_string(),
                    content,
                    created_at: event.created,
                    channel,
                    steps: std::mem::take(&mut pending_steps),
                    images,
                    user_image_hashes: vec![],
                    image_description: None,
                    completed: Some(true),
                    canceled: is_canceled,
                    aborted: is_aborted,
                    text_chunks: std::mem::take(&mut pending_text_chunks),
                    events: std::mem::take(&mut pending_events),

                    request_event_id: user_eid,
                    event_id: Some(event.id.to_string()),
                    thread_id: get_thread_id(event).or_else(|| current_thread_id.clone()),
                });
                last_cc_event_len = 0;
                last_text_event_len = 0;
            }
            "ResponseFailed" => {
                let error = event
                    .payload
                    .get("error")
                    .and_then(|v| v.as_str())
                    .unwrap_or("Unknown error");

                // Prefer explicit request_event_id from payload; fall back to positional tracking
                let user_eid = event
                    .payload
                    .get("request_event_id")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
                    .or_else(|| current_request_event_id.clone());

                pending_images.clear();
                // Append the error text as a text event so events-based
                // rendering shows the error alongside any accumulated steps.
                let error_content = format!("[ERROR] **Error:** {}", error);
                let mut events = std::mem::take(&mut pending_events);
                if !events.is_empty() {
                    events.push(ResponseEvent::Text {
                        md: error_content.clone(),
                    });
                }
                messages.push(SessionMessage {
                    role: "assistant".to_string(),
                    content: error_content,
                    created_at: event.created,
                    channel: None,
                    steps: std::mem::take(&mut pending_steps),
                    images: vec![],
                    user_image_hashes: vec![],
                    image_description: None,
                    completed: Some(true),
                    canceled: false,
                    aborted: false,
                    text_chunks: std::mem::take(&mut pending_text_chunks),
                    events,

                    request_event_id: user_eid,
                    event_id: Some(event.id.to_string()),
                    thread_id: get_thread_id(event).or_else(|| current_thread_id.clone()),
                });
            }
            "TriggerStarted" => {
                let prompt = event
                    .payload
                    .get("prompt")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();

                pending_steps.clear();
                pending_images.clear();
                pending_events.clear();

                let channel = event
                    .payload
                    .get("channel")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());

                current_request_event_id = Some(event.id.to_string());
                current_thread_id = get_thread_id(event);

                messages.push(SessionMessage {
                    role: "user".to_string(),
                    content: prompt,
                    created_at: event.created,
                    channel,
                    steps: vec![],
                    images: vec![],
                    user_image_hashes: vec![],
                    image_description: None,
                    completed: None,
                    canceled: false,
                    aborted: false,
                    text_chunks: vec![],
                    events: vec![],

                    request_event_id: None,
                    event_id: Some(event.id.to_string()),
                    thread_id: current_thread_id.clone(),
                });
            }
            "TriggerCompleted" => {
                // TriggerCompleted is a bookkeeping event — the actual response
                // is already captured by the ResponseGenerated event in the same session.
                // Attach any pending steps to the existing assistant message instead of
                // creating a duplicate that would overwrite it.
                if !pending_steps.is_empty() {
                    if let Some(last_assistant) =
                        messages.iter_mut().rev().find(|m| m.role == "assistant")
                    {
                        last_assistant.steps.append(&mut pending_steps);
                    } else {
                        pending_steps.clear();
                    }
                }
            }
            "ChildThreadCompleted" => {
                // Render the typed event as a user-channel block so the parent's
                // resume LLM sees a clearly-attributed structured callback in
                // its conversation history. Replaces the pre-Phase-4 prose
                // `[Child thread completed]` UserPromptInjected which used to
                // arrive on the parent thread carrying the same info.
                let content = format_child_thread_completed_block(event);

                messages.push(SessionMessage {
                    role: "user".to_string(),
                    content,
                    created_at: event.created,
                    channel: None,
                    steps: vec![],
                    images: vec![],
                    user_image_hashes: vec![],
                    image_description: None,
                    completed: None,
                    canceled: false,
                    aborted: false,
                    text_chunks: vec![],
                    events: vec![],
                    request_event_id: None,
                    event_id: Some(event.id.to_string()),
                    thread_id: get_thread_id(event).or_else(|| current_thread_id.clone()),
                });
            }
            _ => {}
        }
    }

    // Flush any remaining streaming text or pending events as a single assistant message.
    // Both text_buf (outer LLM preamble) and claude_code_text_buf (CC output) may
    // have content when the user reloads mid-CC session. They're part of the same
    // response and must be combined — creating separate messages would orphan one.
    // Also flush when pending_events exist without text (e.g. CC made tool calls
    // before sending any text) — without this, step events are lost on reload and
    // the frontend has no events to show in the More/Less and Steps toggles.
    // completed: None — still in progress, not an interruption.
    let has_cc_text = !claude_code_text_buf.is_empty();
    let has_text = !text_buf.is_empty();
    let has_pending_events = !pending_events.is_empty();
    if has_cc_text || has_text || has_pending_events {
        // Snapshot remaining text deltas as final chunks
        if claude_code_text_buf.len() > last_cc_chunk_len {
            pending_text_chunks.push(claude_code_text_buf[last_cc_chunk_len..].to_string());
        }
        if text_buf.len() > last_text_chunk_len {
            pending_text_chunks.push(text_buf[last_text_chunk_len..].to_string());
        }
        // Snapshot remaining text as events
        if claude_code_text_buf.len() > last_cc_event_len {
            pending_events.push(ResponseEvent::Text {
                md: claude_code_text_buf[last_cc_event_len..].to_string(),
            });
        }
        if text_buf.len() > last_text_event_len {
            pending_events.push(ResponseEvent::Text {
                md: text_buf[last_text_event_len..].to_string(),
            });
        }

        // Combine content — outer LLM preamble + CC text are part of the same response
        let content = if has_text && has_cc_text {
            format!("{}\n\n{}", text_buf, claude_code_text_buf)
        } else if has_cc_text {
            claude_code_text_buf.clone()
        } else if has_text {
            text_buf.clone()
        } else {
            String::new() // Events-only: no text yet (e.g. CC tool calls before text)
        };
        let channel = if has_cc_text || (!has_text && has_pending_events) {
            Some("claude_code".to_string())
        } else {
            None
        };
        let created_at = claude_code_text_last_ts
            .or(text_last_ts)
            .unwrap_or_else(|| messages.last().map(|m| m.created_at).unwrap_or_default());

        claude_code_text_buf.clear();
        text_buf.clear();

        messages.push(SessionMessage {
            role: "assistant".to_string(),
            content,
            created_at,
            channel,
            steps: std::mem::take(&mut pending_steps),
            images: std::mem::take(&mut pending_images),
            user_image_hashes: vec![],
            image_description: None,
            completed: None,
            canceled: false,
            aborted: false,
            text_chunks: std::mem::take(&mut pending_text_chunks),
            events: std::mem::take(&mut pending_events),
            request_event_id: current_request_event_id.clone(),
            event_id: None,
            thread_id: current_thread_id.clone(),
        });
    }

    // If there are pending steps (tool events after the last user message but before any
    // response), attach them to the last user message so the frontend can display them
    // in the "still working" state during reconnection.
    if !pending_steps.is_empty() {
        if let Some(last_user) = messages.iter_mut().rev().find(|m| m.role == "user") {
            last_user.steps = pending_steps;
        }
    }

    messages
}

impl EventStore {
    /// Get all messages for a specific request (for history time travel)
    /// Get session messages as raw text. HTML conversion happens at the API layer.
    /// Queries directly by request_id using the idx_events_request_id index.
    pub async fn get_request_messages_by_id(
        &self,
        request_event_id: &str,
    ) -> Result<Vec<SessionMessage>, Box<dyn std::error::Error + Send + Sync>> {
        let events = sqlx::query_as::<_, EventRow>(
            r#"
            SELECT id, event_type, payload, created, thread_id, sequence
            FROM events
            WHERE payload->>'request_id' = $1
            ORDER BY created ASC
            "#,
        )
        .bind(request_event_id)
        .fetch_all(&self.pool)
        .await?;

        Ok(build_session_messages(&events))
    }

    /// Get recent messages across all conversations, ordered chronologically (oldest first).
    /// `limit` controls the number of **user messages** (exchanges) returned, not raw events.
    ///
    /// Scopes by thread_id: finds the distinct thread_ids from the N most recent
    /// MessageReceived events, then loads ALL events belonging to those threads.
    /// This prevents cross-thread contamination — events from scheduled triggers
    /// (which have their own thread_ids) can never leak into chat thread results,
    /// regardless of whether individual events have the correct channel tag.
    pub async fn get_recent_messages(
        &self,
        limit: i64,
        before: Option<DateTime<Utc>>,
    ) -> Result<Vec<SessionMessage>, Box<dyn std::error::Error + Send + Sync>> {
        let events = if let Some(before_ts) = before {
            sqlx::query_as::<_, EventRow>(
                r#"
                WITH recent_threads AS (
                    SELECT DISTINCT thread_id
                    FROM events
                    WHERE event_type = 'MessageReceived'
                      AND thread_id IS NOT NULL
                      AND created < $2
                    ORDER BY thread_id
                    LIMIT $1
                )
                SELECT e.id, e.event_type, e.payload, e.created, e.thread_id, e.sequence
                FROM events e
                WHERE e.thread_id IN (SELECT thread_id FROM recent_threads)
                  AND e.event_type IN ('MessageReceived', 'Thinking', 'MemorySearched', 'ResponseGenerated', 'ResponseCanceled', 'ResponseAborted', 'ResponseFailed', 'TextStreamed', 'ToolCalled', 'ToolResult')
                  AND e.created < $2
                ORDER BY e.created ASC
                "#,
            )
            .bind(limit)
            .bind(before_ts)
            .fetch_all(&self.pool)
            .await?
        } else {
            sqlx::query_as::<_, EventRow>(
                r#"
                WITH recent_threads AS (
                    SELECT DISTINCT thread_id
                    FROM events
                    WHERE event_type = 'MessageReceived'
                      AND thread_id IS NOT NULL
                    ORDER BY thread_id
                    LIMIT $1
                )
                SELECT e.id, e.event_type, e.payload, e.created, e.thread_id, e.sequence
                FROM events e
                WHERE e.thread_id IN (SELECT thread_id FROM recent_threads)
                  AND e.event_type IN ('MessageReceived', 'Thinking', 'MemorySearched', 'ResponseGenerated', 'ResponseCanceled', 'ResponseAborted', 'ResponseFailed', 'TextStreamed', 'ToolCalled', 'ToolResult')
                ORDER BY e.created ASC
                "#,
            )
            .bind(limit)
            .fetch_all(&self.pool)
            .await?
        };

        Ok(build_session_messages(&events))
    }
}

/// One reconstructed `(ToolCalled, ToolResult)` pair, with the originating
/// `ToolCalled` event id preserved so the caller can deduplicate the
/// stringified `[tools: ...]` summary on the same assistant turn.
///
/// `result.is_none()` marks a synthetic stub for an orphan `ToolCalled` —
/// the originating call has no matching `ToolResult` event in the stream
/// (engine crash mid-tool, projection race, etc.). The builder still emits
/// both messages so every assistant `tool_use` block has a paired user
/// `tool_result` block on the wire — the alternative (silently dropping the
/// pair) yields a Claude API 400.
struct ResumeToolPair {
    tool_called_event_id: uuid::Uuid,
    tool_name: String,
    args: serde_json::Value,
    /// `None` when the matching `ToolResult` event is missing — emit a
    /// synthetic `[tool result unavailable: orphaned]` stub on the wire.
    result: Option<String>,
}

/// Synthetic stub body for orphan `ToolCalled` events (no matching
/// `ToolResult` row). Same wording as the agentic-loop pre-flight stubs in
/// [`crate::engine::context::validate_tool_use_pairing`] so the LLM sees a
/// consistent signal regardless of which layer caught the gap.
pub(crate) const ORPHAN_TOOL_RESULT_STUB: &str = "[tool result unavailable: orphaned]";

/// Walk events chronologically, pairing every `ToolCalled` with its matching
/// `ToolResult` and surfacing any leftover `ToolCalled` as an orphan
/// (`result == None`). Each call reserves a slot in chronological position
/// so the downstream N-window logic sees the same ordering whether a slot
/// is paired or orphaned. Never silently drops — see
/// `engine/context.rs::validate_tool_use_pairing` for why.
fn collect_tool_pairs_chronological(events: &[EventRow]) -> Vec<ResumeToolPair> {
    let mut slots: Vec<ResumeToolPair> = Vec::new();
    // Pending pairings: (slot_index, tool_name) — name is duplicated here so
    // the rposition match doesn't have to dereference into `slots`.
    let mut pending: Vec<(usize, String)> = Vec::new();

    for event in events {
        match event.event_type.as_str() {
            "ToolCalled" => {
                let tool_name = event
                    .payload
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown")
                    .to_string();
                let args = event
                    .payload
                    .get("args")
                    .cloned()
                    .unwrap_or_else(|| serde_json::Value::Object(serde_json::Map::new()));
                let slot_idx = slots.len();
                slots.push(ResumeToolPair {
                    tool_called_event_id: event.id,
                    tool_name: tool_name.clone(),
                    args,
                    result: None,
                });
                pending.push((slot_idx, tool_name));
            }
            "ToolResult" => {
                let result_name = event
                    .payload
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                // Pair with the most recent pending ToolCalled of the same
                // name. If names don't match (legacy events, racing tools),
                // fall back to the most recent pending entry — same forgiving
                // rule build_session_messages applies via `last_mut()`.
                let idx = pending
                    .iter()
                    .rposition(|(_, n)| n == result_name)
                    .or_else(|| pending.len().checked_sub(1));
                if let Some(i) = idx {
                    let (slot_idx, _) = pending.remove(i);
                    let result = event
                        .payload
                        .get("result")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    if let Some(slot) = slots.get_mut(slot_idx) {
                        slot.result = Some(result);
                    }
                }
            }
            _ => {}
        }
    }

    // `slots` is already in chronological call order. Any slot whose
    // `result` is still `None` is an orphan — surfaced as a synthetic stub
    // by `build_resume_tool_blocks_with_skip_ids`.
    slots
}

/// Partition resume pairs into (pinned, tail) — the shared step both the
/// resume-block builder and the skip-set builder need. Pinned pairs survive
/// regardless of N (see [`PINNED_TOOL_NAMES`]); tail is the last N non-pinned
/// pairs in chronological order.
fn select_pinned_and_tail<'a>(
    pairs: &'a [ResumeToolPair],
    n: usize,
) -> (Vec<&'a ResumeToolPair>, Vec<&'a ResumeToolPair>) {
    let mut pinned: Vec<&ResumeToolPair> = Vec::new();
    let mut others: Vec<&ResumeToolPair> = Vec::new();
    for p in pairs {
        if PINNED_TOOL_NAMES.contains(&p.tool_name.as_str()) {
            pinned.push(p);
        } else {
            others.push(p);
        }
    }
    let tail_start = others.len().saturating_sub(n);
    let tail: Vec<&ResumeToolPair> = others[tail_start..].to_vec();
    (pinned, tail)
}

/// Return `(ToolCalled.event_id, tool_name)` for every `ToolCalled` whose
/// matching `ToolResult` is missing in `events`. Single source of truth for
/// the orphan-detection rule used both by the resume builder (which emits
/// synthetic stubs in the LLM messages payload) and the startup recovery
/// sweep (which writes a synthetic `ToolResult` event to settle the orphan).
pub(crate) fn find_orphan_tool_called_ids(events: &[EventRow]) -> Vec<(uuid::Uuid, String)> {
    collect_tool_pairs_chronological(events)
        .into_iter()
        .filter(|p| p.result.is_none())
        .map(|p| (p.tool_called_event_id, p.tool_name))
        .collect()
}

/// Walk every `ContextDismissed` event in the stream and collect the set of
/// dismissed event ids. The agent emits these via the `dismiss_from_context`
/// tool — see `engine/tools/mod.rs::execute_dismiss_from_context`. The
/// resume helper consults this set to drop both `(ToolCalled, ToolResult)`
/// pairs (matched on the `ToolCalled.id`) and `ChildThreadCompleted` blocks.
fn collect_dismissed_event_ids(events: &[EventRow]) -> std::collections::HashSet<String> {
    let mut dismissed = std::collections::HashSet::new();
    for event in events {
        if event.event_type == "ContextDismissed" {
            if let Some(id) = event
                .payload
                .get("dismissed_event_id")
                .and_then(|v| v.as_str())
            {
                dismissed.insert(id.to_string());
            }
        }
    }
    dismissed
}

/// Reconstruct verbatim `(ToolUse, ToolResult)` `Message` pairs for the most
/// recent N tool calls — and the set of `ToolCalled` event ids those pairs
/// derive from, so callers can skip the same tools in their stringified
/// history summaries to avoid double-billing.
///
/// `load_knowhow` results are pinned: any pair where `tool_name == "load_knowhow"`
/// is included regardless of where it falls in the N-most-recent ordering.
/// Other tools follow the N window (most recent N kept, older dropped — they
/// keep their `[tools: ...]` summary in stringified history).
///
/// `ContextDismissed` records: any tool pair whose `tool_called_event_id`
/// appears in the dismissed set is dropped from BOTH the rebuilt blocks and
/// the skip set — the latter so the stringified `[tools: ...]` summary also
/// stops rendering for the dismissed tool (the agent asked for the entry to
/// be gone, not just shrunk).
///
/// Output ordering: pinned pairs first (chronological), then tail pairs
/// (chronological). Each pair emits an assistant `ToolUse` block followed by
/// a user `ToolResult` block, matching the wire shape Anthropic and OpenAI
/// providers expect.
///
/// Single-pass: events are walked once and partitioned once — the previous
/// split between `build_resume_tool_blocks` and a separate
/// `build_resume_tool_emitted_event_ids` did both twice per resume.
pub(crate) fn build_resume_tool_blocks_with_skip_ids(
    events: &[EventRow],
    n: usize,
) -> (Vec<crate::llm::Message>, std::collections::HashSet<String>) {
    let pairs = collect_tool_pairs_chronological(events);
    let dismissed = collect_dismissed_event_ids(events);
    let pairs: Vec<ResumeToolPair> = pairs
        .into_iter()
        .filter(|p| !dismissed.contains(&p.tool_called_event_id.to_string()))
        .collect();
    if pairs.is_empty() {
        return (Vec::new(), std::collections::HashSet::new());
    }
    let (pinned, tail) = select_pinned_and_tail(&pairs, n);

    let total = pinned.len() + tail.len();
    let mut messages: Vec<crate::llm::Message> = Vec::with_capacity(total * 2);
    let mut skip_ids: std::collections::HashSet<String> =
        std::collections::HashSet::with_capacity(total);
    for p in pinned.into_iter().chain(tail.into_iter()) {
        skip_ids.insert(p.tool_called_event_id.to_string());
        let id = synthesize_tool_use_id(&p.tool_called_event_id);
        messages.push(crate::llm::Message {
            role: "assistant".to_string(),
            content: crate::llm::MessageContent::Blocks(vec![crate::llm::ContentBlock::ToolUse {
                id: id.clone(),
                name: p.tool_name.clone(),
                input: p.args.clone(),
            }]),
        });
        // Orphans (`result: None`) emit a synthetic stub so every assistant
        // tool_use block has its paired user tool_result on the wire.
        // Anthropic 400s otherwise — see thread b101c3d7 repro.
        let content = p
            .result
            .clone()
            .unwrap_or_else(|| ORPHAN_TOOL_RESULT_STUB.to_string());
        messages.push(crate::llm::Message {
            role: "user".to_string(),
            content: crate::llm::MessageContent::Blocks(vec![crate::llm::ContentBlock::ToolResult {
                tool_use_id: id,
                content,
            }]),
        });
    }
    (messages, skip_ids)
}

#[cfg(test)]
#[path = "messages_tests.rs"]
mod tests;
