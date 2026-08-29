use super::super::types::*;
use super::collect_dismissed_event_ids;
use crate::core::EventRow;
use crate::engine::thread_events::{AgentParticipant, MessageOrigin};
use chrono::{DateTime, Utc};

/// Which agent authored this event, when its actor names one (ADR 0150).
///
/// Read from the persisted actor rather than inferred from position, because
/// two agents can interleave on one thread. A row from before the actor
/// existed, or one whose actor names a human, yields `None`.
fn authoring_agent(event: &EventRow) -> Option<AgentParticipant> {
    serde_json::from_value::<MessageOrigin>(event.payload.get("actor")?.clone())
        .ok()?
        .agent()
        .cloned()
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
    // The event_id (not child_thread_id) is the lookup key the event-reading
    // tools accept, so surface it. The LLM can then pass it back verbatim
    // without inventing a synthetic prefix. `dismiss_from_context` was the
    // original reader and is retired (ADR 0109); the `events` tool's
    // `event_id` argument takes the same form.
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

    // `ContextDismissed` records ask the projection to drop the corresponding
    // history entry on every future read. They came from the retired
    // `dismiss_from_context` tool (ADR 0109), so every row is historical. The
    // set is collected up-front so a dismissal that landed out of order,
    // before the event it names, is still respected.
    let dismissed_event_ids = collect_dismissed_event_ids(events);

    // `ImageDescribed` events carry the Flash-generated description for an
    // earlier `MessageReceived`'s attached images, keyed by `source_event_id`.
    // Pre-walk so the description is available whether the events arrive in
    // emission order or get reshuffled by replay (the agentic loop emits
    // ImageDescribed AFTER MessageReceived but a strict-chronological read
    // shouldn't be assumed). The latest event per source wins — if a future
    // path retries the description on the same source, the most recent
    // wording is what consumers see. Falls back to legacy
    // `MessageReceived.image_description` when no event is present (covers
    // pre-backfill rows in the deprecation window).
    let image_descriptions = collect_image_descriptions(events);

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
                        agent: None,
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
                        agent: None,
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

                // Prefer the typed `ImageDescribed` event (post-refactor) and
                // fall back to the legacy `MessageReceived.image_description`
                // payload field for pre-backfill rows in the deprecation
                // window. Both carry the same Flash output; the new event
                // additionally records which model produced it.
                let image_description = image_descriptions
                    .get(&event.id.to_string())
                    .cloned()
                    .or_else(|| {
                        event
                            .payload
                            .get("image_description")
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string())
                    });

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
                    agent: None,
                });
            }
            // "Thinking" is the legacy DB string; T7 renamed the variant to
            // ThoughtStreamed. New rows store "ThoughtStreamed"; old rows are
            // unchanged. Both must hit this arm so legacy threads still render.
            "Thinking" | "ThoughtStreamed" => {
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
            // "MemorySearched" is the legacy DB string; the variant was renamed
            // to MemoryRecalled so it stops mirroring the `memory` tool's own
            // "Searching memory" step. New rows store "MemoryRecalled"; old
            // rows are unchanged. Both must hit this arm so legacy threads
            // still render.
            "MemorySearched" | "MemoryRecalled" => {
                let results = event
                    .payload
                    .get("results")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0);
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
                let desc = crate::core::store::memory_recalled_label(results);
                let detail = if queries.is_empty() {
                    None
                } else {
                    Some(queries.join(", "))
                };
                pending_steps.push(Step {
                    description: desc.clone(),
                    tool_name: None,
                    success: true,
                    context_tokens: None,
                    context_messages: None,
                    trimmed: None,
                    tool_called_event_id: None,
                });
                pending_events.push(ResponseEvent::Step {
                    description: desc,
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

                let (tool_name, description) = super::super::describe_tool_event(event);
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
                let detail =
                    super::super::super::describe_tool_result(tool_name, result_text, success);

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
                        if let Some(path) = super::super::extract_screenshot_path(result) {
                            pending_images.push(path);
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
                    agent: authoring_agent(event),
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
                    agent: None,
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
                    agent: None,
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
                    agent: None,
                });
            }
            "SpokenReplyGenerated" => {
                // What the talker said out loud. It reaches the reasoner under
                // the talker's own speaker label. So the reasoner reads what
                // was already said in its name, never as its own turn.
                //
                // It consumes NOTHING pending. A spoken reply lands mid-call,
                // often while the reasoner's turn is still running, and those
                // steps and text belong to that turn.
                let text = event
                    .payload
                    .get("text")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default();
                if !text.trim().is_empty() {
                    messages.push(SessionMessage {
                        role: "assistant".to_string(),
                        content: text.to_string(),
                        created_at: event.created,
                        channel: None,
                        steps: vec![],
                        images: vec![],
                        user_image_hashes: vec![],
                        image_description: None,
                        completed: Some(true),
                        canceled: false,
                        aborted: false,
                        text_chunks: vec![],
                        events: vec![],
                        request_event_id: None,
                        event_id: Some(event.id.to_string()),
                        thread_id: get_thread_id(event).or_else(|| current_thread_id.clone()),
                        agent: authoring_agent(event),
                    });
                }
            }
            _ => {}
        }
    }

    // Flush any remaining streaming text or pending events as a single assistant message.
    // Both text_buf (outer LLM preamble) and claude_code_text_buf (CC output) may
    // have content when the user reloads mid-Claude Code session. They're part of the same
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
            agent: None,
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

/// The thread's cached *conversation summary*, or `None` before its first
/// successful summarisation (ADR 0102).
///
/// The newest `ConversationSummarized` wins. Events arrive in chronological
/// order, so the last one seen is it. A row missing either field is skipped
/// rather than trusted: a summary with no boundary cannot be checked for
/// staleness, and a boundary with no text is not a summary.
pub(crate) fn newest_conversation_summary(events: &[EventRow]) -> Option<CachedSummary> {
    let mut found: Option<CachedSummary> = None;
    for event in events {
        if event.event_type != "ConversationSummarized" {
            continue;
        }
        let Some(summary) = event
            .payload
            .get("summary")
            .and_then(|v| v.as_str())
            .filter(|s| !s.trim().is_empty())
        else {
            continue;
        };
        let Some(covers_through) = event
            .payload
            .get("covers_through_event_id")
            .and_then(|v| v.as_str())
        else {
            continue;
        };
        found = Some(CachedSummary {
            summary: summary.to_string(),
            covers_through_event_id: covers_through.to_string(),
        });
    }
    found
}

/// A `ConversationSummarized` payload, reduced to what the history builder
/// reads. The count and model live on the event for the audit trail only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CachedSummary {
    pub summary: String,
    /// The `SessionMessage::event_id` of the newest turn this paragraph covers.
    pub covers_through_event_id: String,
}

/// Walk every `ImageDescribed` event and collect the latest description per
/// `source_event_id`. Latest-wins because the source can in principle be
/// described more than once (the current emit site fires only on iteration 1
/// of the agentic loop, so re-describes don't happen today, but a future retry
/// path should produce the most recent text without history projection
/// rewrites). Multi-image messages emit one `ImageDescribed` per attached
/// hash all carrying the same description text — collapsing on
/// `source_event_id` joins them back into the single per-message description
/// that consumers expect.
fn collect_image_descriptions(events: &[EventRow]) -> std::collections::HashMap<String, String> {
    let mut by_source: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    for event in events {
        if event.event_type != "ImageDescribed" {
            continue;
        }
        let Some(source_id) = event
            .payload
            .get("source_event_id")
            .and_then(|v| v.as_str())
        else {
            continue;
        };
        let Some(desc) = event.payload.get("description").and_then(|v| v.as_str()) else {
            continue;
        };
        // Last write wins — `events` arrives in chronological order from
        // the loader, so the most recently emitted description survives.
        by_source.insert(source_id.to_string(), desc.to_string());
    }
    by_source
}
