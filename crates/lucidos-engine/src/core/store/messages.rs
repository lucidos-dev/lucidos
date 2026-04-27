use super::types::*;
use super::EventStore;
use crate::core::EventRow;
use chrono::{DateTime, Utc};

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

    // Helper: extract thread_id from the event column
    let get_thread_id =
        |event: &EventRow| -> Option<String> { event.thread_id.map(|uuid| uuid.to_string()) };

    for event in events {
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
                        user_images: vec![],
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
                        user_images: vec![],
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

                let user_images: Vec<UserImagePayload> = event
                    .payload
                    .get("images")
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|img| {
                                let b64 = img.get("base64")?.as_str()?;
                                let mime = img.get("mime_type")?.as_str()?;
                                Some(UserImagePayload {
                                    base64: b64.to_string(),
                                    mime_type: mime.to_string(),
                                })
                            })
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
                    user_images,
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
                let results = event
                    .payload
                    .get("results")
                    .and_then(|v| v.as_u64())
                    .map(|v| v as usize)
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
                let desc = if results > 0 {
                    format!("Memory: {} results", results)
                } else {
                    "Memory: no results".to_string()
                };
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

                let (tool_name, description) = super::describe_tool_event(event);
                pending_steps.push(Step {
                    description: description.clone(),
                    tool_name: Some(tool_name.clone()),
                    success: true,
                    context_tokens: None,
                    context_messages: None,
                    trimmed: None,
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
                    .unwrap_or(false);

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
                    user_images: vec![],
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
                    user_images: vec![],
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
                    user_images: vec![],
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
            user_images: vec![],
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

#[cfg(test)]
#[path = "messages_tests.rs"]
mod tests;
