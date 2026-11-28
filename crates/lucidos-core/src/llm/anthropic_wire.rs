use super::claude_model::{adaptive_model, supports_extended_thinking, AdaptiveModel};
pub use super::claude_model::{thinking_mode, ThinkingMode};
use crate::llm::api_usage::ApiUsage;
use crate::llm::provider::{
    ContentBlock, LlmResponse, Message, MessageContent, TokenCallback, ToolCall, ToolDefinition,
};
use futures::StreamExt;
use serde::Serialize;
use std::time::Duration;

pub const ANTHROPIC_BETA_1M_CONTEXT: &str = "context-1m-2025-08-07";

/// INVARIANT: Beta flag that makes `thinking.display: "updates"` a legal value. Sent as an
/// `anthropic-beta` HTTP header on both targets: Vertex ignores it in the body
/// array and then rejects the `display` value.
pub(crate) const ANTHROPIC_BETA_THINKING_DISPLAY_UPDATES: &str =
    "thinking-display-updates-2026-08-18";

pub(crate) const DISPLAY_PROGRESS_UPDATES: &str = "updates";

/// INVARIANT: Whether a response's `thinking` blocks carry progress notes for the user.
/// Read off the request, so the parser shows thinking text only when the
/// request asked for progress updates: anywhere else that text is reasoning.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ThinkingDisplay {
    Hidden,
    ProgressUpdates,
}

const INTERRUPTED_PROGRESS_SENTINEL: &str =
    "This part of the response was interrupted before it finished.";

const CLAUDE_STREAM_CHUNK_TIMEOUT_SECS: u64 = 300;

pub enum WireTarget<'a> {
    Vertex { url: &'a str },
    Direct { url: &'a str },
}

pub fn parse_context_suffix(model: &str) -> (&str, bool) {
    if let Some(base) = model.strip_suffix("[1m]") {
        (base, true)
    } else {
        (model, false)
    }
}

/// INVARIANT: One message's content as Claude takes it, with the places a cache marker
/// may go. The indexes are computed where the emitted array is built. The
/// filter in [`message_content_to_claude_value`] drops blocks, so an emitted
/// index is not a source index.
struct ClaudeContent {
    value: serde_json::Value,
    anchor: Option<usize>,
    views: Vec<usize>,
    stable: Option<usize>,
}

fn message_content_to_claude_value(content: &MessageContent) -> ClaudeContent {
    match content {
        MessageContent::Text(s) => ClaudeContent {
            value: serde_json::Value::String(s.clone()),
            anchor: None,
            views: Vec::new(),
            stable: None,
        },
        MessageContent::Blocks(blocks) => {
            let mut json_blocks: Vec<serde_json::Value> = Vec::with_capacity(blocks.len());
            let mut anchor: Option<usize> = None;
            let mut views: Vec<usize> = Vec::new();
            let mut stable: Option<usize> = None;
            for block in blocks.iter().filter(|block| {
                !matches!(
                    block,
                    ContentBlock::Text { text }
                        | ContentBlock::EngineTail { text }
                        | ContentBlock::MemoryView { text }
                        | ContentBlock::StableContext { text }
                        if text.is_empty()
                )
            }) {
                if !matches!(block, ContentBlock::EngineTail { .. }) {
                    anchor = Some(json_blocks.len());
                }
                if matches!(block, ContentBlock::MemoryView { .. }) {
                    views.push(json_blocks.len());
                }
                if matches!(block, ContentBlock::StableContext { .. }) {
                    stable = Some(json_blocks.len());
                }
                json_blocks.push(match block {
                    ContentBlock::Text { text }
                    | ContentBlock::EngineTail { text }
                    | ContentBlock::MemoryView { text }
                    | ContentBlock::StableContext { text } => {
                        serde_json::json!({
                            "type": "text",
                            "text": text,
                        })
                    }
                    ContentBlock::ToolUse {
                        id,
                        name,
                        input,
                        thought_signature: _,
                    } => serde_json::json!({
                        "type": "tool_use",
                        "id": id,
                        "name": name,
                        "input": input,
                    }),
                    ContentBlock::ToolResult {
                        tool_use_id,
                        content,
                    } => serde_json::json!({
                        "type": "tool_result",
                        "tool_use_id": tool_use_id,
                        "content": content,
                    }),
                    ContentBlock::Image {
                        source_type,
                        media_type,
                        data,
                    } => serde_json::json!({
                        "type": "image",
                        "source": {
                            "type": source_type,
                            "media_type": media_type,
                            "data": data,
                        },
                    }),
                });
            }
            ClaudeContent {
                value: serde_json::Value::Array(json_blocks),
                anchor,
                views,
                stable,
            }
        }
    }
}

const NO_THINKING_MAX_TOKENS: u32 = 8192;

type ThinkingConfig = (Option<ClaudeThinking>, Option<ClaudeOutputConfig>, u32);

fn thinking_config(base_model: &str, reasoning_effort: Option<&str>) -> ThinkingConfig {
    if !supports_extended_thinking(base_model) {
        return (None, None, NO_THINKING_MAX_TOKENS);
    }
    match (adaptive_model(base_model), reasoning_effort) {
        (Some(entry), None) if entry.mode == ThinkingMode::OffByDefault => {
            (None, None, NO_THINKING_MAX_TOKENS)
        }
        (Some(entry), None) => adaptive_request(entry, None),
        (Some(entry), Some(effort)) => adaptive_thinking_config(entry, effort),
        (None, None | Some("none")) => (None, None, NO_THINKING_MAX_TOKENS),
        (None, Some(effort)) => {
            let budget = crate::llm::thinking_budget_for_effort(effort);
            (
                Some(ClaudeThinking {
                    thinking_type: "enabled".to_string(),
                    budget_tokens: Some(budget),
                    display: None,
                }),
                None,
                budget + 16384,
            )
        }
    }
}

/// INVARIANT: Safe request template for SDK callers, derived from the same thinking
/// authority as the provider transport. Contains no credentials or origins.
pub fn sdk_request_defaults(model: &str, effort: Option<&str>, vertex: bool) -> serde_json::Value {
    let (base, is_1m) = parse_context_suffix(model);
    let (thinking, output_config, max_tokens) = thinking_config(base, effort);
    let mut body = serde_json::json!({ "max_tokens": max_tokens });
    let mut headers = serde_json::json!({});
    let mut betas = Vec::new();
    if ThinkingDisplay::of(thinking.as_ref()) == ThinkingDisplay::ProgressUpdates {
        betas.push(ANTHROPIC_BETA_THINKING_DISPLAY_UPDATES);
    }
    if let Some(thinking) = thinking {
        body["thinking"] = serde_json::to_value(thinking).expect("thinking serializes");
    }
    if let Some(config) = output_config {
        body["output_config"] = serde_json::to_value(config).expect("output config serializes");
    }
    if vertex {
        body["anthropic_version"] = serde_json::json!("vertex-2023-10-16");
        if is_1m {
            body["anthropic_beta"] = serde_json::json!([ANTHROPIC_BETA_1M_CONTEXT]);
        }
    } else {
        body["model"] = serde_json::json!(base);
        headers["anthropic-version"] = serde_json::json!("2023-06-01");
        if is_1m {
            betas.insert(0, ANTHROPIC_BETA_1M_CONTEXT);
        }
    }
    if !betas.is_empty() {
        headers["anthropic-beta"] = serde_json::json!(betas.join(","));
    }
    serde_json::json!({ "body": body, "headers": headers })
}

/// INVARIANT: Effort `none` asks for no thinking, and each [`ThinkingMode`] needs a
/// different request to honour it. Omitting `thinking` is not "off" on a model
/// that thinks by default: it runs adaptive thinking anyway.
fn adaptive_thinking_config(entry: &AdaptiveModel, effort: &str) -> ThinkingConfig {
    match (entry.mode, effort) {
        (ThinkingMode::OffByDefault, "none") => (None, None, NO_THINKING_MAX_TOKENS),
        (ThinkingMode::OnByDefault, "none") => (
            Some(ClaudeThinking {
                thinking_type: "disabled".to_string(),
                budget_tokens: None,
                display: None,
            }),
            None,
            NO_THINKING_MAX_TOKENS,
        ),
        (ThinkingMode::AlwaysOn, "none") => adaptive_request(entry, Some("low")),
        (_, effort) => adaptive_request(entry, Some(effort)),
    }
}

/// INVARIANT: Adaptive thinking at `effort`, or at the API's default effort for `None`.
/// It takes the model's own `max_tokens` ceiling, so `end_turn` ends the turn
/// rather than a budget we invented. See
/// [`ADAPTIVE_THINKING_MODELS`](super::claude_model::ADAPTIVE_THINKING_MODELS).
///
/// A model that always thinks writes its notes between tool calls as
/// `thinking` blocks, empty unless the request asks for progress updates. So
/// it always asks: without them a long tool-calling turn streams nothing.
fn adaptive_request(entry: &AdaptiveModel, effort: Option<&str>) -> ThinkingConfig {
    (
        Some(ClaudeThinking {
            thinking_type: "adaptive".to_string(),
            budget_tokens: None,
            display: (entry.mode == ThinkingMode::AlwaysOn).then_some(DISPLAY_PROGRESS_UPDATES),
        }),
        effort.map(|effort| ClaudeOutputConfig {
            effort: effort.to_string(),
        }),
        entry.max_output_tokens,
    )
}

pub fn build_claude_request(
    mut messages: Vec<Message>,
    mut tools: Vec<ToolDefinition>,
    model: &str,
    system_prompt: Option<&str>,
    reasoning_effort: Option<&str>,
    target: WireTarget<'_>,
    provider_tag: &str,
) -> (ClaudeRequest, Vec<&'static str>) {
    let (base_model, is_1m) = parse_context_suffix(model);

    let stubs = crate::llm::validate::validate_tool_use_pairing(&mut messages);
    if stubs > 0 {
        crate::log_legacy!(
            "[{}] WARNING: pre-flight injected {} stub tool_result block(s) before Claude API call (model={})",
            provider_tag,
            stubs,
            model
        );
    }

    for name in crate::llm::validate::drop_unsafe_tool_names(&mut tools) {
        crate::log_legacy!(
            "[{}] WARNING: dropped tool {:?} before Claude API call: name is not \
             ^[a-zA-Z0-9_-]{{1,128}}$ (model={})",
            provider_tag,
            name,
            model
        );
    }

    let mut view_blocks: Vec<(usize, usize)> = Vec::new();
    let mut stable_block: Option<(usize, usize)> = None;
    let mut claude_messages: Vec<ClaudeMessage> = messages
        .iter()
        .enumerate()
        .map(|(i, m)| {
            let content = message_content_to_claude_value(&m.content);
            view_blocks.extend(content.views.into_iter().map(|block| (i, block)));
            stable_block = stable_block.or(content.stable.map(|block| (i, block)));
            ClaudeMessage {
                role: m.role.clone(),
                content: content.value,
                cache_anchor: content.anchor,
            }
        })
        .collect();

    let mut claude_tools: Option<Vec<ClaudeTool>> = if tools.is_empty() {
        None
    } else {
        let converted: Vec<ClaudeTool> = tools
            .into_iter()
            .map(|t| ClaudeTool {
                name: t.name,
                description: t.description,
                input_schema: t.parameters,
                cache_control: None,
            })
            .collect();
        Some(converted)
    };

    let mut breakpoints = usize::from(apply_cache_control_to_last_message(&mut claude_messages))
        + usize::from(apply_cache_control_to_penultimate_message(
            &mut claude_messages,
        ));
    for &(message, block) in &view_blocks {
        if breakpoints == MAX_CACHE_BREAKPOINTS {
            break;
        }
        breakpoints += usize::from(mark_block_for_cache(&mut claude_messages[message], block));
    }
    let mark_system = breakpoints < MAX_CACHE_BREAKPOINTS;
    let system = system_block(system_prompt, mark_system);
    breakpoints += usize::from(mark_system && system.is_some());
    if let Some((message, block)) = stable_block.filter(|_| breakpoints < MAX_CACHE_BREAKPOINTS) {
        breakpoints += usize::from(mark_block_for_cache(&mut claude_messages[message], block));
    }
    if breakpoints < MAX_CACHE_BREAKPOINTS {
        if let Some(tools) = claude_tools.as_mut() {
            apply_cache_control_to_last_tool(tools);
        }
    }

    let (thinking, output_config, max_tokens) = thinking_config(base_model, reasoning_effort);

    let mut header_betas: Vec<&'static str> = Vec::new();
    // INVARIANT: Without the beta, `display: "updates"` is rejected as an unknown value.
    // Vertex ignores it in the body array, so it rides the header everywhere.
    if ThinkingDisplay::of(thinking.as_ref()) == ThinkingDisplay::ProgressUpdates {
        header_betas.push(ANTHROPIC_BETA_THINKING_DISPLAY_UPDATES);
    }

    let (request_url, anthropic_version, model_field, anthropic_beta) = match target {
        WireTarget::Vertex { url } => (
            url,
            Some("vertex-2023-10-16".to_string()),
            None,
            is_1m.then(|| vec![ANTHROPIC_BETA_1M_CONTEXT.to_string()]),
        ),
        WireTarget::Direct { url } => {
            if is_1m {
                header_betas.insert(0, ANTHROPIC_BETA_1M_CONTEXT);
            }
            (url, None, Some(base_model.to_string()), None)
        }
    };

    let request = ClaudeRequest {
        anthropic_version,
        model: model_field,
        max_tokens,
        stream: true,
        system,
        messages: claude_messages,
        tools: claude_tools,
        thinking,
        output_config,
        anthropic_beta,
        tool_choice: None,
        stable_prefix_end: view_blocks.last().copied(),
    };

    crate::llm::cache_probe::log_request(&request, model, request_url, provider_tag);

    (request, header_betas)
}

pub(crate) async fn parse_claude_stream(
    response: reqwest::Response,
    on_token: &Option<TokenCallback>,
    display: ThinkingDisplay,
    provider_tag: &str,
) -> Result<LlmResponse, Box<dyn std::error::Error + Send + Sync>> {
    let mut stream = response.bytes_stream();
    let mut buffer = String::new();
    let mut carry: Vec<u8> = Vec::new();

    let mut blocks: Vec<AccumulatedBlock> = Vec::new();
    let mut turn_meta = TurnMeta::default();
    let mut rendered_any = false;

    let chunk_timeout = Duration::from_secs(CLAUDE_STREAM_CHUNK_TIMEOUT_SECS);

    loop {
        let chunk = match tokio::time::timeout(chunk_timeout, stream.next()).await {
            Ok(Some(Ok(bytes))) => bytes,
            Ok(Some(Err(e))) => {
                return Err(crate::llm::stream_failure(
                    format!("Stream read error: {}", e),
                    rendered_any,
                    provider_tag,
                ))
            }
            Ok(None) => break,
            Err(_) => {
                return Err(crate::llm::stream_failure(
                    format!(
                        "Claude stream timed out (no data for {}s)",
                        CLAUDE_STREAM_CHUNK_TIMEOUT_SECS
                    ),
                    rendered_any,
                    provider_tag,
                ))
            }
        };

        crate::llm::push_utf8_chunk(&mut carry, &chunk, &mut buffer);

        while let Some(newline_pos) = buffer.find('\n') {
            let line = buffer[..newline_pos].trim_end_matches('\r').to_string();
            buffer = buffer[newline_pos + 1..].to_string();

            if let Some(data_str) = line.strip_prefix("data: ") {
                let prev_text_len: usize = blocks
                    .iter()
                    .map(|b| match b {
                        AccumulatedBlock::Text(t) => t.len(),
                        _ => 0,
                    })
                    .sum();
                let stopped_block =
                    process_sse_data(data_str, &mut blocks, &mut turn_meta, provider_tag)
                        .map_err(|e| crate::llm::stream_failure(e, rendered_any, provider_tag))?;
                let finished_note = stopped_block
                    .filter(|&index| settle_progress_note(&mut blocks, index, display));
                if let Some(cb) = on_token {
                    let new_text_len: usize = blocks
                        .iter()
                        .map(|b| match b {
                            AccumulatedBlock::Text(t) => t.len(),
                            _ => 0,
                        })
                        .sum();
                    if new_text_len > prev_text_len {
                        let delta = new_text_len - prev_text_len;
                        // INVARIANT: Floor defensively: `raw_start` is byte-derived and
                        // could land mid-codepoint after future accumulator
                        // changes. Saturate for the same reason. `delta` is
                        // summed over every text block, so a delta appended to
                        // an earlier block than the last one would underflow.
                        for block in blocks.iter().rev() {
                            if let AccumulatedBlock::Text(t) = block {
                                let raw_start = t.len().saturating_sub(delta);
                                let delta_start = t.floor_char_boundary(raw_start);
                                // INVARIANT: `delta_start == 0` with something already
                                // streamed means this chunk is a new text
                                // block's first content. Mirror the storage
                                // side's newline join here too, or a replayed
                                // history reads differently from what the
                                // user watched stream by.
                                if delta_start == 0 && rendered_any {
                                    cb("\n");
                                }
                                cb(&t[delta_start..]);
                                rendered_any = true;
                                break;
                            }
                        }
                    }
                    if let Some(AccumulatedBlock::ProgressNote(note)) =
                        finished_note.and_then(|index| blocks.get(index))
                    {
                        if rendered_any {
                            cb("\n");
                        }
                        cb(note);
                        rendered_any = true;
                    }
                }
            }
        }
    }

    crate::llm::cache_probe::log_response(&turn_meta, provider_tag);

    if turn_meta.stop_reason.is_none() && !blocks.iter().any(carries_output) {
        return Err(format!(
            "Claude stream truncated: {} closed the stream before message_delta, with nothing to show for {} input tokens",
            provider_tag,
            turn_meta.input_tokens.unwrap_or(0),
        )
        .into());
    }

    if turn_meta.stop_reason.as_deref() == Some("tool_use") && !blocks.iter().any(carries_output) {
        return Err(format!(
            "Claude stream truncated: {} stopped on tool_use without sending the tool call",
            provider_tag,
        )
        .into());
    }

    let already_rendered_text = on_token.is_some()
        && blocks.iter().any(|b| match b {
            AccumulatedBlock::Text(text) => !text.is_empty(),
            AccumulatedBlock::ProgressNote(_) => true,
            _ => false,
        });

    let mut content: Option<String> = None;
    let mut tool_calls = Vec::new();
    let mut progress_notes = Vec::new();
    let mut thinking_chars: usize = 0;
    let mut thinking_blocks: usize = 0;

    let mut append_content = |text: &str| {
        if text.is_empty() {
            return;
        }
        content = Some(match content.take() {
            Some(existing) => format!("{existing}\n{text}"),
            None => text.to_string(),
        });
    };

    for block in blocks {
        match block {
            AccumulatedBlock::Text(text) => append_content(&text),
            AccumulatedBlock::ProgressNote(note) => {
                thinking_blocks += 1;
                append_content(&note);
                progress_notes.push(note);
            }
            AccumulatedBlock::ToolUse {
                id,
                name,
                json_parts,
            } => {
                let arguments: serde_json::Value = if json_parts.is_empty() {
                    serde_json::json!({})
                } else {
                    serde_json::from_str(&json_parts).map_err(|e| {
                        tool_argument_parse_error(
                            e,
                            &name,
                            &json_parts,
                            &turn_meta,
                            provider_tag,
                            already_rendered_text,
                        )
                    })?
                };
                tool_calls.push(ToolCall {
                    id,
                    name,
                    arguments,
                    thought_signature: None,
                });
            }
            AccumulatedBlock::Thinking(text) => {
                thinking_blocks += 1;
                thinking_chars = thinking_chars.saturating_add(text.len());
            }
            AccumulatedBlock::RedactedThinking { payload_len } => {
                thinking_blocks += 1;
                thinking_chars = thinking_chars.saturating_add(payload_len);
            }
            AccumulatedBlock::Dropped => {}
        }
    }

    let usage = turn_meta.usage();
    Ok(LlmResponse {
        content,
        tool_calls,
        stop_reason: turn_meta.stop_reason,
        usage,
        served_model: turn_meta.served_model,
        provider: None,
        stable_prefix_fingerprint: None,
        thinking_chars: Some(thinking_chars),
        thinking_blocks: Some(thinking_blocks),
        unknown_sse_dropped: turn_meta.unknown_sse_dropped,
        model_only_text: None,
        progress_notes,
        prefill: None,
    })
}

/// INVARIANT: Turn the `thinking` block at `index`, which just received its
/// `content_block_stop`, into a [`AccumulatedBlock::ProgressNote`] when it is
/// one. Returns whether it did.
///
/// Under progress-update display every non-empty `thinking` block is a note,
/// except the interrupted-work sentinel. Only a finished block qualifies. A
/// stream cut mid-note leaves it `Thinking`, which counts as no output. So the
/// truncation still retries instead of ending the turn on half a sentence.
fn settle_progress_note(
    blocks: &mut [AccumulatedBlock],
    index: usize,
    display: ThinkingDisplay,
) -> bool {
    if display != ThinkingDisplay::ProgressUpdates {
        return false;
    }
    let Some(AccumulatedBlock::Thinking(text)) = blocks.get(index) else {
        return false;
    };
    let Some(note) = progress_note(text) else {
        return false;
    };
    blocks[index] = AccumulatedBlock::ProgressNote(note.to_string());
    true
}

pub fn progress_note(thinking_text: &str) -> Option<&str> {
    let note = thinking_text.trim();
    (!note.is_empty() && note != INTERRUPTED_PROGRESS_SENTINEL).then_some(note)
}

fn tool_argument_parse_error(
    err: serde_json::Error,
    tool_name: &str,
    json_parts: &str,
    meta: &TurnMeta,
    provider_tag: &str,
    already_rendered_text: bool,
) -> String {
    match meta.stop_reason.as_deref() {
        None if already_rendered_text => stays_non_retryable(
            format!(
                "Claude ended the stream partway through the arguments for '{}', after already \
                 sending text. Retrying would render that text twice, so the turn stops here.",
                tool_name,
            ),
            "Claude ended the stream partway through a tool call, after already sending text. \
             Retrying would render that text twice, so the turn stops here.",
        ),
        None => format!(
            "Claude stream truncated: {} closed the stream {} bytes into the arguments for '{}', before message_delta",
            provider_tag,
            json_parts.len(),
            tool_name,
        ),
        Some("max_tokens") => stays_non_retryable(
            format!(
                "Claude hit the model's output token limit while streaming the arguments for \
                 '{}'. The call was cut off, so the tool never ran. Ask for the work in smaller \
                 steps.",
                tool_name,
            ),
            "Claude hit the model's output token limit while streaming a tool call's arguments. \
             The call was cut off, so the tool never ran. Ask for the work in smaller steps.",
        ),
        Some(_) => stays_non_retryable(
            format!("Failed to parse tool arguments: {} (json: {})", err, json_parts),
            "Failed to parse tool arguments, and the arguments are withheld here because they \
             read as a transient transport error.",
        ),
    }
}

/// INVARIANT: Guarantee an error that must not retry does not read as one that should.
///
/// `is_retryable_error` decides by sniffing the text, and all three of the
/// non-retryable arms above interpolate model-supplied content. A tool name may
/// legally be `502` (`^[a-zA-Z0-9_-]{1,128}$`), raw arguments may contain one,
/// and a serde message ends in a column number. Any of those reads as a
/// transient HTTP status.
///
/// So the detailed message is used only when it classifies correctly, and a
/// fixed fallback carrying no model text is used when it does not. Losing the
/// detail in that case is the cheap side of the trade: the alternative is a
/// budget cut that retries forever, or a rendered turn that renders twice.
fn stays_non_retryable(detailed: String, fallback: &str) -> String {
    if crate::llm::is_retryable_error(&detailed) {
        fallback.to_string()
    } else {
        detailed
    }
}

fn carries_output(block: &AccumulatedBlock) -> bool {
    match block {
        AccumulatedBlock::Text(text) => !text.is_empty(),
        AccumulatedBlock::ToolUse { .. } | AccumulatedBlock::ProgressNote(_) => true,
        AccumulatedBlock::Thinking(_)
        | AccumulatedBlock::RedactedThinking { .. }
        | AccumulatedBlock::Dropped => false,
    }
}

const MAX_CONTENT_BLOCKS: usize = 1024;

fn process_sse_data(
    data_str: &str,
    blocks: &mut Vec<AccumulatedBlock>,
    meta: &mut TurnMeta,
    provider_tag: &str,
) -> Result<Option<usize>, Box<dyn std::error::Error + Send + Sync>> {
    let data: serde_json::Value = serde_json::from_str(data_str)?;
    let event_type = data["type"].as_str().unwrap_or("");

    match event_type {
        "content_block_start" => {
            let index = data["index"].as_u64().unwrap_or(0) as usize;
            if index >= MAX_CONTENT_BLOCKS {
                return Err(format!(
                    "provider streamed content_block index {index}, past the {MAX_CONTENT_BLOCKS} cap"
                )
                .into());
            }
            let block = &data["content_block"];
            let block_type = block["type"].as_str().unwrap_or("");

            while blocks.len() <= index {
                blocks.push(AccumulatedBlock::Dropped);
            }

            blocks[index] = match block_type {
                "text" => AccumulatedBlock::Text(block["text"].as_str().unwrap_or("").to_string()),
                "tool_use" => AccumulatedBlock::ToolUse {
                    id: block["id"].as_str().unwrap_or("").to_string(),
                    name: block["name"].as_str().unwrap_or("").to_string(),
                    json_parts: String::new(),
                },
                "thinking" => {
                    AccumulatedBlock::Thinking(block["thinking"].as_str().unwrap_or("").to_string())
                }
                "redacted_thinking" => AccumulatedBlock::RedactedThinking {
                    payload_len: block["data"].as_str().map_or(0, str::len),
                },
                other => {
                    meta.unknown_sse_dropped = meta.unknown_sse_dropped.saturating_add(1);
                    crate::log_legacy!(
                        "[{}] WARNING: unknown content_block_start type '{}' at index {}; content lost (parser needs update)",
                        provider_tag,
                        other,
                        index,
                    );
                    AccumulatedBlock::Dropped
                }
            };
        }
        "content_block_stop" => {
            return Ok(data["index"].as_u64().map(|index| index as usize));
        }
        "content_block_delta" => {
            crate::llm::call_trace::content_arrived();
            let index = data["index"].as_u64().unwrap_or(0) as usize;
            if let Some(block) = blocks.get_mut(index) {
                let delta = &data["delta"];
                let delta_type = delta["type"].as_str().unwrap_or("");

                match (delta_type, block) {
                    ("text_delta", AccumulatedBlock::Text(ref mut text)) => {
                        if let Some(t) = delta["text"].as_str() {
                            text.push_str(t);
                        }
                    }
                    (
                        "input_json_delta",
                        AccumulatedBlock::ToolUse {
                            ref mut json_parts, ..
                        },
                    ) => {
                        if let Some(j) = delta["partial_json"].as_str() {
                            json_parts.push_str(j);
                        }
                    }
                    ("thinking_delta", AccumulatedBlock::Thinking(ref mut text)) => {
                        if let Some(t) = delta["thinking"].as_str() {
                            text.push_str(t);
                        }
                    }
                    ("signature_delta", _) => {}
                    (other, block) => {
                        let block_kind = match block {
                            AccumulatedBlock::Text(_) => "text",
                            AccumulatedBlock::ToolUse { .. } => "tool_use",
                            AccumulatedBlock::Thinking(_) => "thinking",
                            AccumulatedBlock::RedactedThinking { .. } => "redacted_thinking",
                            AccumulatedBlock::ProgressNote(_) => "progress_note",
                            AccumulatedBlock::Dropped => "dropped",
                        };
                        meta.unknown_sse_dropped = meta.unknown_sse_dropped.saturating_add(1);
                        crate::log_legacy!(
                            "[{}] WARNING: unknown content_block_delta type '{}' for {} block at index {}; content lost (parser needs update)",
                            provider_tag,
                            other,
                            block_kind,
                            index,
                        );
                    }
                }
            }
        }
        "message_start" => {
            let usage =
                crate::llm::usage_wire::from_block(&data["message"]["usage"]).unwrap_or_default();
            let nonzero = |n: u32| (n > 0).then_some(n);
            meta.input_tokens = nonzero(usage.input_tokens).or(meta.input_tokens);
            meta.cache_creation_tokens =
                nonzero(usage.cache_creation_tokens).or(meta.cache_creation_tokens);
            meta.cache_read_tokens = nonzero(usage.cache_read_tokens).or(meta.cache_read_tokens);
            meta.cache_creation_1h_tokens = usage.cache_creation_1h_tokens;
            meta.served_model = crate::llm::served_model::served_model_of(&data["message"]);
        }
        "message_delta" => {
            if let Some(sr) = data["delta"]["stop_reason"].as_str() {
                meta.stop_reason = Some(sr.to_string());
            }
            if let Some(ot) = data["usage"]["output_tokens"].as_u64() {
                meta.output_tokens = Some(crate::llm::clamp_provider_token_count(ot, provider_tag));
            }
        }
        "error" => {
            return Err(
                crate::llm::ProviderError::from_stream_error(provider_tag, &data["error"]).into(),
            );
        }
        _ => {}
    }

    Ok(None)
}

pub(crate) const MAX_CACHE_BREAKPOINTS: usize = 4;

pub(crate) fn ephemeral_cache_marker() -> serde_json::Value {
    serde_json::json!({"type": "ephemeral"})
}

/// INVARIANT: Build a typed-content text block with a `cache_control` marker, taking
/// ownership of the body so callers don't pay a copy. Used to wrap both the
/// system prompt and the last message's bare-string content into the array
/// form Anthropic requires for cache breakpoints.
fn text_block_with_cache_control(text: String) -> serde_json::Value {
    let mut obj = serde_json::Map::with_capacity(3);
    obj.insert("type".to_string(), serde_json::Value::from("text"));
    obj.insert("text".to_string(), serde_json::Value::String(text));
    obj.insert("cache_control".to_string(), ephemeral_cache_marker());
    serde_json::Value::Object(obj)
}

fn system_block(system: Option<&str>, marked: bool) -> Option<serde_json::Value> {
    let s = system?;
    if s.is_empty() {
        return None;
    }
    let block = if marked {
        text_block_with_cache_control(s.to_string())
    } else {
        serde_json::json!({"type": "text", "text": s})
    };
    Some(serde_json::Value::Array(vec![block]))
}

fn apply_cache_control_to_last_tool(tools: &mut [ClaudeTool]) {
    if let Some(last) = tools.last_mut() {
        last.cache_control = Some(ephemeral_cache_marker());
    }
}

/// INVARIANT: Mark one message's last non-tail content block, so the prefix ending there
/// becomes a cache breakpoint.
///
/// The panel and the working understanding ride on the message holding the
/// round's tool results. Both are rewritten at the top of the next round, so a
/// mark on the final block re-sends those results at write price every round.
/// Under the sweep a result stays whole for 6 to 15 rounds, so that is
/// thousands of tokens rather than a hundred.
///
/// Bare-string content is rewritten into the array form, the only shape that
/// accepts `cache_control`. An array is marked at `cache_anchor`, which
/// [`message_content_to_claude_value`] set from the typed blocks. Nothing is
/// given up by stopping short: the blocks after the mark are collapsed next
/// round anyway, so no request could ever have read them. We skip an empty
/// string. It has nothing worth caching, and the array form would turn it into
/// an empty text block, which the API rejects.
fn mark_message_for_cache(message: &mut ClaudeMessage) -> bool {
    let len = match &mut message.content {
        serde_json::Value::String(s) if !s.is_empty() => {
            let text = std::mem::take(s);
            message.content = serde_json::Value::Array(vec![text_block_with_cache_control(text)]);
            message.cache_anchor = Some(0);
            return true;
        }
        serde_json::Value::Array(arr) => arr.len(),
        _ => return false,
    };
    // INVARIANT: A message of nothing but tail blocks falls back to its final one. Still
    // exactly one mark, so the count of four holds.
    let at = message.cache_anchor.unwrap_or(len.saturating_sub(1));
    mark_block_for_cache(message, at)
}

fn mark_block_for_cache(message: &mut ClaudeMessage, at: usize) -> bool {
    let serde_json::Value::Array(arr) = &mut message.content else {
        return false;
    };
    let Some(block) = arr.get_mut(at).and_then(|b| b.as_object_mut()) else {
        return false;
    };
    block
        .insert("cache_control".to_string(), ephemeral_cache_marker())
        .is_none()
}

fn apply_cache_control_to_last_message(messages: &mut [ClaudeMessage]) -> bool {
    messages.last_mut().is_some_and(mark_message_for_cache)
}

/// INVARIANT: Mark the message in front of the tail, so a round that rewrites the tail can
/// still read everything before it.
///
/// A read needs a breakpoint some earlier request wrote. Anthropic's ~20-block
/// lookback finds only such an entry. Behind the tail marker there is nothing
/// but the system block, so touching the last message drops the whole array
/// back to full price.
///
/// Two things touch it. The context mode rewrites one message at the top of
/// every round, always the one that was last in the previous round: see
/// `chat::process::context_panel::collapse_tail_blocks`. And in either mode, a
/// round can put more than ~20 blocks between consecutive tail markers, which
/// puts the previous entry out of lookback range.
///
/// The penultimate message is the newest position neither case disturbs. The
/// measurements are in
/// `docs/investigations/2026-08-23-context-mode-cache-breakpoint-diagnosis.md`.
fn apply_cache_control_to_penultimate_message(messages: &mut [ClaudeMessage]) -> bool {
    let Some(index) = messages.len().checked_sub(2) else {
        return false;
    };
    mark_message_for_cache(&mut messages[index])
}

#[derive(Clone, Serialize)]
pub struct ClaudeRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub anthropic_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    pub max_tokens: u32,
    pub stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system: Option<serde_json::Value>,
    pub messages: Vec<ClaudeMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<ClaudeTool>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) thinking: Option<ClaudeThinking>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) output_config: Option<ClaudeOutputConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub anthropic_beta: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) tool_choice: Option<ClaudeToolChoice>,
    #[serde(skip)]
    pub stable_prefix_end: Option<(usize, usize)>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct ClaudeToolChoice {
    #[serde(rename = "type")]
    pub choice_type: &'static str,
    pub name: String,
}

impl ClaudeRequest {
    pub(crate) fn thinking_display(&self) -> ThinkingDisplay {
        ThinkingDisplay::of(self.thinking.as_ref())
    }

    pub(crate) fn force_tool(&mut self, tool: &str, model: &str) -> Result<(), String> {
        let (base_model, _) = parse_context_suffix(model);
        if thinking_mode(base_model) == Some(ThinkingMode::AlwaysOn) {
            return Err(format!(
                "{base_model} always thinks, so it cannot be made to call {tool}. \
                 Pick a model that can for this background task."
            ));
        }
        let (thinking, output_config, max_tokens) = thinking_config(base_model, Some("none"));
        self.thinking = thinking;
        self.output_config = output_config;
        self.max_tokens = max_tokens;
        self.tool_choice = Some(ClaudeToolChoice {
            choice_type: "tool",
            name: tool.to_string(),
        });
        Ok(())
    }
}

impl ThinkingDisplay {
    fn of(thinking: Option<&ClaudeThinking>) -> Self {
        match thinking.and_then(|t| t.display) {
            Some(DISPLAY_PROGRESS_UPDATES) => Self::ProgressUpdates,
            _ => Self::Hidden,
        }
    }
}

#[derive(Clone, Serialize)]
pub(crate) struct ClaudeThinking {
    #[serde(rename = "type")]
    pub thinking_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub budget_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display: Option<&'static str>,
}

#[derive(Clone, Serialize)]
pub(crate) struct ClaudeOutputConfig {
    pub effort: String,
}

#[derive(Clone, Serialize)]
pub struct ClaudeMessage {
    pub role: String,
    pub content: serde_json::Value,
    /// INVARIANT: Where a cache marker on this message belongs: the index in `content` of
    /// the last block the engine did not append at the tail. `None` for string
    /// content, and for an array of nothing but tail blocks.
    ///
    /// Carried rather than re-derived, because by this point the array is
    /// untyped JSON and a tail block is indistinguishable from any other text.
    #[serde(skip)]
    pub cache_anchor: Option<usize>,
}

#[derive(Clone, Serialize)]
pub struct ClaudeTool {
    pub name: String,
    pub description: String,
    pub input_schema: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_control: Option<serde_json::Value>,
}

pub(crate) enum AccumulatedBlock {
    Text(String),
    ToolUse {
        id: String,
        name: String,
        json_parts: String,
    },
    Thinking(String),
    ProgressNote(String),
    RedactedThinking {
        payload_len: usize,
    },
    Dropped,
}

#[derive(Default)]
pub(crate) struct TurnMeta {
    pub stop_reason: Option<String>,
    pub output_tokens: Option<u32>,
    pub input_tokens: Option<u32>,
    pub cache_read_tokens: Option<u32>,
    pub cache_creation_tokens: Option<u32>,
    pub cache_creation_1h_tokens: Option<u32>,
    pub served_model: Option<String>,
    pub unknown_sse_dropped: u32,
}

impl TurnMeta {
    fn usage(&self) -> Option<ApiUsage> {
        if self.input_tokens.is_none() && self.output_tokens.is_none() {
            return None;
        }
        Some(ApiUsage {
            input_tokens: self.input_tokens.unwrap_or(0),
            output_tokens: self.output_tokens.unwrap_or(0),
            cache_read_tokens: self.cache_read_tokens.unwrap_or(0),
            cache_creation_tokens: self.cache_creation_tokens.unwrap_or(0),
            cache_creation_1h_tokens: self.cache_creation_1h_tokens,
            ..ApiUsage::default()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::claude_model::ADAPTIVE_THINKING_MODELS;
    use crate::llm::provider::{ContentBlock, MessageContent};

    const VERTEX_TEST_URL: &str = "https://aiplatform.eu.rep.googleapis.com/v1/projects/p\
                                   /locations/eu/publishers/anthropic/models/m:streamRawPredict";

    #[test]
    fn the_probe_url_argument_never_reaches_the_body() {
        let (req, _) = build_claude_request(
            vec![Message {
                role: "user".into(),
                content: MessageContent::Text("hi".into()),
            }],
            vec![],
            "claude-opus-5",
            Some("system prompt body"),
            Some("high"),
            WireTarget::Vertex {
                url: VERTEX_TEST_URL,
            },
            "Vertex",
        );

        let body = serde_json::to_string(&req).unwrap();
        assert!(!body.contains("aiplatform"), "URL leaked into body: {body}");
        assert!(!body.contains("streamRawPredict"));
    }

    #[test]
    fn parse_context_suffix_strips_1m() {
        assert_eq!(
            parse_context_suffix("claude-opus-4-6[1m]"),
            ("claude-opus-4-6", true)
        );
        assert_eq!(
            parse_context_suffix("claude-sonnet-4-6[1m]"),
            ("claude-sonnet-4-6", true)
        );
        assert_eq!(
            parse_context_suffix("claude-sonnet-5[1m]"),
            ("claude-sonnet-5", true)
        );
        assert_eq!(
            parse_context_suffix("claude-fable-5-1[1m]"),
            ("claude-fable-5-1", true)
        );
        assert_eq!(
            parse_context_suffix("claude-opus-5-5[1m]"),
            ("claude-opus-5-5", true)
        );
    }

    #[test]
    fn parse_context_suffix_preserves_base_model() {
        assert_eq!(
            parse_context_suffix("claude-opus-4-6"),
            ("claude-opus-4-6", false)
        );
        assert_eq!(
            parse_context_suffix("gemini-2.5-pro"),
            ("gemini-2.5-pro", false)
        );
    }

    #[test]
    fn every_fable_generation_is_adaptive_thinking() {
        for id in [
            "claude-fable-5",
            "claude-fable-5[1m]",
            "claude-fable-5-1",
            "claude-fable-5-2",
            "claude-fable-5-1[1m]",
        ] {
            assert!(supports_extended_thinking(id), "extended: {id}");
            assert!(thinking_mode(id).is_some(), "adaptive: {id}");
            assert_eq!(
                adaptive_model(id).map(|entry| entry.max_output_tokens),
                Some(128_000),
                "{id}"
            );
        }
    }

    #[test]
    fn every_opus_5_generation_is_adaptive_thinking() {
        for id in [
            "claude-opus-5",
            "claude-opus-5[1m]",
            "claude-opus-5-5",
            "claude-opus-5-5[1m]",
        ] {
            assert!(supports_extended_thinking(id), "extended: {id}");
            assert!(thinking_mode(id).is_some(), "adaptive: {id}");
            assert_eq!(
                adaptive_model(id).map(|entry| entry.max_output_tokens),
                Some(128_000),
                "{id}"
            );
        }
    }

    #[test]
    fn sonnet_5_is_adaptive_thinking_and_sonnet_4_6_is_not() {
        for id in [
            "claude-sonnet-5",
            "claude-sonnet-5[1m]",
            "claude-sonnet-5-5",
            "claude-sonnet-5-5[1m]",
        ] {
            assert!(supports_extended_thinking(id), "extended: {id}");
            assert!(thinking_mode(id).is_some(), "adaptive: {id}");
        }
        assert!(supports_extended_thinking("claude-sonnet-4-6"));
        assert!(thinking_mode("claude-sonnet-4-6").is_none());
    }

    #[test]
    fn sonnet_5_thinking_config_uses_adaptive_not_budget_tokens() {
        let (thinking, output_config, max_tokens) =
            thinking_config("claude-sonnet-5", Some("xhigh"));
        let thinking = thinking.expect("adaptive thinking present");
        assert_eq!(thinking.thinking_type, "adaptive");
        assert!(thinking.budget_tokens.is_none());
        assert_eq!(output_config.expect("effort present").effort, "xhigh");
        assert_eq!(max_tokens, 128_000);
    }

    #[test]
    fn opus_5_thinking_config_uses_adaptive_not_budget_tokens() {
        let (thinking, output_config, max_tokens) = thinking_config("claude-opus-5", Some("high"));
        let thinking = thinking.expect("adaptive thinking present");
        assert_eq!(thinking.thinking_type, "adaptive");
        assert!(thinking.budget_tokens.is_none());
        assert_eq!(output_config.expect("effort present").effort, "high");
        assert_eq!(max_tokens, 128_000);
    }

    #[test]
    fn every_adaptive_model_gets_its_full_output_ceiling_at_every_effort() {
        for entry in ADAPTIVE_THINKING_MODELS {
            for effort in ["low", "medium", "high", "xhigh", "max"] {
                let (_, _, max_tokens) = thinking_config(entry.fragment, Some(effort));
                assert_eq!(
                    max_tokens, entry.max_output_tokens,
                    "{} at {effort} effort must get its full ceiling",
                    entry.fragment
                );
            }
        }
    }

    #[test]
    fn each_family_resolves_to_its_own_thinking_mode() {
        for (id, mode) in [
            ("claude-opus-4-7", ThinkingMode::OffByDefault),
            ("claude-opus-4-8", ThinkingMode::OffByDefault),
            ("claude-opus-5", ThinkingMode::OnByDefault),
            ("claude-opus-5[1m]", ThinkingMode::OnByDefault),
            ("claude-sonnet-5", ThinkingMode::OnByDefault),
            ("claude-sonnet-5[1m]", ThinkingMode::OnByDefault),
            ("claude-sonnet-5-5", ThinkingMode::AlwaysOn),
            ("claude-sonnet-5-5[1m]", ThinkingMode::AlwaysOn),
            ("claude-opus-5-5", ThinkingMode::AlwaysOn),
            ("claude-opus-5-5[1m]", ThinkingMode::AlwaysOn),
            ("claude-fable-5", ThinkingMode::AlwaysOn),
            ("claude-fable-5-1[1m]", ThinkingMode::AlwaysOn),
            ("claude-haiku-5-5", ThinkingMode::OnByDefault),
            ("claude-haiku-5-6", ThinkingMode::OnByDefault),
        ] {
            assert_eq!(thinking_mode(id), Some(mode), "{id}");
        }
        assert_eq!(thinking_mode("claude-sonnet-4-6"), None);
        for id in HAIKU_4_5_IDS {
            assert_eq!(thinking_mode(id), None, "{id}");
        }
    }

    #[test]
    fn effort_none_never_reaches_an_always_thinking_model() {
        for id in [
            "claude-opus-5-5",
            "claude-opus-5-5[1m]",
            "claude-sonnet-5-5",
            "claude-sonnet-5-5[1m]",
            "claude-fable-5",
            "claude-fable-5-1",
            "claude-fable-5-2",
        ] {
            let (base, _) = parse_context_suffix(id);
            let (thinking, output_config, max_tokens) = thinking_config(base, Some("none"));
            let thinking = thinking.expect("thinking config present");
            assert_eq!(thinking.thinking_type, "adaptive", "{id}");
            assert!(thinking.budget_tokens.is_none(), "{id}");
            assert_eq!(output_config.expect("effort").effort, "low", "{id}");
            assert_eq!(max_tokens, 128_000, "{id}");
            for effort in crate::llm::EFFORT_LADDER {
                let (thinking, output_config, _) = thinking_config(base, Some(effort));
                let thinking = thinking.expect("thinking config present");
                assert_eq!(thinking.thinking_type, "adaptive", "{id} at {effort}");
                assert!(thinking.budget_tokens.is_none(), "{id} at {effort}");
                assert_ne!(output_config.expect("effort").effort, "none");
            }
        }
    }

    #[test]
    fn effort_none_disables_thinking_on_a_model_that_thinks_by_default() {
        for id in ["claude-opus-5", "claude-sonnet-5", "claude-haiku-5-5"] {
            let (thinking, output_config, max_tokens) = thinking_config(id, Some("none"));
            let thinking = thinking.expect("explicit thinking config");
            assert_eq!(thinking.thinking_type, "disabled", "{id}");
            assert!(thinking.budget_tokens.is_none(), "{id}");
            assert!(output_config.is_none(), "{id}");
            assert_eq!(max_tokens, NO_THINKING_MAX_TOKENS, "{id}");
        }
    }

    #[test]
    fn no_effort_runs_the_model_at_its_own_default() {
        for id in ["claude-opus-4-8", "claude-haiku-4-5"] {
            let (thinking, output_config, max_tokens) = thinking_config(id, None);
            assert!(thinking.is_none(), "{id}");
            assert!(output_config.is_none(), "{id}");
            assert_eq!(max_tokens, NO_THINKING_MAX_TOKENS, "{id}");
        }
        for id in ["claude-opus-5", "claude-opus-5-5", "claude-fable-5"] {
            let (thinking, output_config, _) = thinking_config(id, None);
            assert_eq!(
                thinking.expect("adaptive").thinking_type,
                "adaptive",
                "{id}"
            );
            assert!(output_config.is_none(), "{id} sends no effort");
        }
    }

    #[test]
    fn effort_none_still_omits_thinking_where_that_means_off() {
        for id in ["claude-opus-4-8", "claude-sonnet-4-6"] {
            let (thinking, output_config, max_tokens) = thinking_config(id, Some("none"));
            assert!(thinking.is_none(), "{id}");
            assert!(output_config.is_none(), "{id}");
            assert_eq!(max_tokens, NO_THINKING_MAX_TOKENS, "{id}");
        }
    }

    #[test]
    fn the_output_ceiling_survives_the_context_suffix() {
        for id in [
            "claude-opus-5",
            "claude-opus-5[1m]",
            "claude-sonnet-5[1m]",
            "claude-sonnet-5-5[1m]",
        ] {
            assert_eq!(
                adaptive_model(id).map(|entry| entry.max_output_tokens),
                Some(128_000),
                "{id}"
            );
        }
    }

    #[test]
    fn the_budget_tokens_path_keeps_its_own_max_tokens() {
        assert_eq!(
            adaptive_model("claude-sonnet-4-6").map(|entry| entry.max_output_tokens),
            None
        );
        let (thinking, output_config, max_tokens) =
            thinking_config("claude-sonnet-4-6", Some("xhigh"));
        let thinking = thinking.expect("extended thinking present");
        assert_eq!(thinking.thinking_type, "enabled");
        let budget = thinking.budget_tokens.expect("budget present");
        assert!(output_config.is_none());
        assert_eq!(max_tokens, budget + 16384);
    }

    const HAIKU_4_5_MAX_OUTPUT_TOKENS: u32 = 64_000;

    const HAIKU_4_5_IDS: [&str; 3] = [
        "claude-haiku-4-5",
        "claude-haiku-4-5-20251001",
        "claude-haiku-4-5@20251001",
    ];

    #[test]
    fn haiku_4_5_sends_the_tier_the_picker_offers() {
        use crate::llm::model_registry::ProviderKind;
        for id in HAIKU_4_5_IDS {
            assert!(supports_extended_thinking(id), "extended: {id}");
            assert!(thinking_mode(id).is_none(), "budget path: {id}");
            for provider in [ProviderKind::Vertex, ProviderKind::Anthropic] {
                let offered = crate::llm::reasoning::supported_efforts(provider, id);
                assert!(offered.contains(&"high"), "{provider:?}/{id} offers high");
                for &effort in offered {
                    for target in [
                        WireTarget::Vertex {
                            url: VERTEX_TEST_URL,
                        },
                        WireTarget::Direct {
                            url: "https://api.anthropic.com/v1/messages",
                        },
                    ] {
                        let (req, _) = build_claude_request(
                            vec![Message {
                                role: "user".into(),
                                content: MessageContent::Text("hi".into()),
                            }],
                            vec![],
                            id,
                            None,
                            Some(effort),
                            target,
                            "test",
                        );
                        let json = serde_json::to_value(&req).unwrap();
                        let max_tokens = json["max_tokens"].as_u64().unwrap();
                        assert!(
                            max_tokens <= u64::from(HAIKU_4_5_MAX_OUTPUT_TOKENS),
                            "{id} at {effort}: max_tokens {max_tokens} over the ceiling"
                        );
                        assert!(json.get("output_config").is_none(), "{id} at {effort}");
                        if effort == "none" {
                            assert!(json.get("thinking").is_none(), "{id} at none");
                            continue;
                        }
                        assert_eq!(json["thinking"]["type"], "enabled", "{id} at {effort}");
                        let budget = json["thinking"]["budget_tokens"].as_u64().unwrap();
                        assert_eq!(
                            budget,
                            u64::from(crate::llm::thinking_budget_for_effort(effort)),
                            "{id} at {effort}"
                        );
                        assert!(budget < max_tokens, "{id} at {effort}");
                    }
                }
            }
        }
    }

    #[test]
    fn every_tier_offered_to_haiku_4_5_is_distinct_on_the_wire() {
        use crate::llm::model_registry::ProviderKind;
        let offered =
            crate::llm::reasoning::supported_efforts(ProviderKind::Vertex, "claude-haiku-4-5");
        let mut sent: Vec<(Option<u32>, u32)> = offered
            .iter()
            .map(|effort| {
                let (thinking, _, max_tokens) = thinking_config("claude-haiku-4-5", Some(effort));
                (thinking.and_then(|t| t.budget_tokens), max_tokens)
            })
            .collect();
        sent.sort_unstable();
        sent.dedup();
        assert_eq!(sent.len(), offered.len(), "{offered:?} sent {sent:?}");
    }

    #[test]
    fn haiku_5_5_sends_only_fields_it_accepts_at_every_tier() {
        use crate::llm::model_registry::ProviderKind;
        let id = "claude-haiku-5-5";
        for provider in [ProviderKind::Vertex, ProviderKind::Anthropic] {
            let offered = crate::llm::reasoning::supported_efforts(provider, id);
            assert_eq!(offered, crate::llm::EFFORT_LADDER, "{provider:?}");
            for &effort in offered {
                for target in [
                    WireTarget::Vertex {
                        url: VERTEX_TEST_URL,
                    },
                    WireTarget::Direct {
                        url: "https://api.anthropic.com/v1/messages",
                    },
                ] {
                    let (req, _) = build_claude_request(
                        vec![Message {
                            role: "user".into(),
                            content: MessageContent::Text("hi".into()),
                        }],
                        vec![],
                        id,
                        None,
                        Some(effort),
                        target,
                        "test",
                    );
                    let json = serde_json::to_value(&req).unwrap();
                    for field in ["temperature", "top_p", "top_k"] {
                        assert!(json.get(field).is_none(), "{field} at {effort}");
                    }
                    assert!(json["thinking"].get("budget_tokens").is_none());
                    let max_tokens = json["max_tokens"].as_u64().unwrap();
                    if effort == "none" {
                        assert_eq!(json["thinking"]["type"], "disabled");
                        assert!(json.get("output_config").is_none());
                        assert_eq!(max_tokens, u64::from(NO_THINKING_MAX_TOKENS));
                        continue;
                    }
                    assert_eq!(json["thinking"]["type"], "adaptive", "{effort}");
                    assert_eq!(json["output_config"]["effort"], effort);
                    assert_eq!(max_tokens, 128_000, "{effort}");
                }
            }
        }
    }

    #[test]
    fn message_content_to_claude_value_filters_empty_text_blocks() {
        let content = MessageContent::Blocks(vec![
            ContentBlock::Text {
                text: String::new(),
            },
            ContentBlock::Image {
                source_type: "base64".to_string(),
                media_type: "image/png".to_string(),
                data: "AAAA".to_string(),
            },
        ]);
        let value = message_content_to_claude_value(&content).value;
        let arr = value.as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["type"], "image");
    }

    #[test]
    fn message_content_to_claude_value_keeps_nonempty_text() {
        let content = MessageContent::Blocks(vec![
            ContentBlock::Text {
                text: "describe this".to_string(),
            },
            ContentBlock::Image {
                source_type: "base64".to_string(),
                media_type: "image/png".to_string(),
                data: "AAAA".to_string(),
            },
        ]);
        let value = message_content_to_claude_value(&content).value;
        let arr = value.as_array().unwrap();
        assert_eq!(arr.len(), 2);
        assert_eq!(arr[0]["type"], "text");
        assert_eq!(arr[0]["text"], "describe this");
        assert_eq!(arr[1]["type"], "image");
    }

    #[test]
    fn process_sse_captures_input_tokens_from_message_start() {
        let mut blocks = Vec::new();
        let mut meta = TurnMeta::default();
        let event = r#"{"type":"message_start","message":{"id":"msg_x","type":"message","role":"assistant","content":[],"model":"claude-opus-4-7","stop_reason":null,"stop_sequence":null,"usage":{"input_tokens":4321,"cache_creation_input_tokens":1000,"cache_read_input_tokens":500,"output_tokens":1}}}"#;

        process_sse_data(event, &mut blocks, &mut meta, "Test").unwrap();

        assert_eq!(meta.input_tokens, Some(5821));
        assert_eq!(meta.cache_creation_tokens, Some(1000));
        assert_eq!(meta.cache_read_tokens, Some(500));
        assert_eq!(meta.served_model.as_deref(), Some("claude-opus-4-7"));
        assert_eq!(meta.cache_creation_1h_tokens, None);
    }

    #[test]
    fn process_sse_reads_the_cache_write_lifetime_split() {
        let event = |one_hour: u32| {
            format!(
                r#"{{"type":"message_start","message":{{"model":"claude-opus-5-5","usage":{{"input_tokens":3,"cache_creation_input_tokens":1200,"cache_read_input_tokens":0,"cache_creation":{{"ephemeral_5m_input_tokens":{},"ephemeral_1h_input_tokens":{one_hour}}},"output_tokens":1}}}}}}"#,
                1200 - one_hour
            )
        };
        for one_hour in [0, 800] {
            let mut meta = TurnMeta::default();
            process_sse_data(&event(one_hour), &mut Vec::new(), &mut meta, "Test").unwrap();
            assert_eq!(meta.cache_creation_tokens, Some(1200));
            assert_eq!(meta.cache_creation_1h_tokens, Some(one_hour));
        }
    }

    #[test]
    fn process_sse_token_sum_saturates_instead_of_overflowing() {
        let mut blocks = Vec::new();
        let mut meta = TurnMeta::default();
        let huge = u64::MAX;
        let event = format!(
            r#"{{"type":"message_start","message":{{"id":"msg_x","type":"message","role":"assistant","content":[],"model":"claude-opus-4-7","stop_reason":null,"stop_sequence":null,"usage":{{"input_tokens":{huge},"cache_creation_input_tokens":{huge},"cache_read_input_tokens":{huge},"output_tokens":1}}}}}}"#
        );

        process_sse_data(&event, &mut blocks, &mut meta, "Test").unwrap();

        assert_eq!(meta.input_tokens, Some(u32::MAX));
        assert_eq!(meta.cache_creation_tokens, Some(u32::MAX));
        assert_eq!(meta.cache_read_tokens, Some(u32::MAX));
    }

    #[test]
    fn process_sse_rejects_an_out_of_range_content_block_index() {
        let mut blocks = Vec::new();
        let mut meta = TurnMeta::default();
        let event = r#"{"type":"content_block_start","index":4000000000,"content_block":{"type":"text","text":""}}"#;

        let result = process_sse_data(event, &mut blocks, &mut meta, "Test");
        assert!(
            result.is_err(),
            "out-of-range content_block index must error"
        );
        assert!(blocks.is_empty(), "must not grow the block list");
    }

    #[test]
    fn system_block_none_returns_none() {
        assert!(system_block(None, true).is_none());
    }

    #[test]
    fn system_block_empty_string_returns_none() {
        assert!(system_block(Some(""), true).is_none());
    }

    #[test]
    fn system_block_wraps_string_in_block_with_marker() {
        let value = system_block(Some("you are a helpful assistant"), true).unwrap();
        let arr = value.as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["type"], "text");
        assert_eq!(arr[0]["text"], "you are a helpful assistant");
        assert_eq!(arr[0]["cache_control"]["type"], "ephemeral");
    }

    #[test]
    fn apply_cache_control_to_last_tool_marks_only_last() {
        let mut tools = vec![
            ClaudeTool {
                name: "a".into(),
                description: "first".into(),
                input_schema: serde_json::json!({}),
                cache_control: None,
            },
            ClaudeTool {
                name: "b".into(),
                description: "second".into(),
                input_schema: serde_json::json!({}),
                cache_control: None,
            },
        ];
        apply_cache_control_to_last_tool(&mut tools);
        assert!(tools[0].cache_control.is_none());
        assert_eq!(
            tools[1].cache_control.as_ref().unwrap()["type"],
            "ephemeral"
        );
    }

    #[test]
    fn apply_cache_control_to_last_tool_empty_is_noop() {
        let mut tools: Vec<ClaudeTool> = Vec::new();
        apply_cache_control_to_last_tool(&mut tools);
        assert!(tools.is_empty());
    }

    #[test]
    fn apply_cache_control_to_last_message_string_content_becomes_block() {
        let mut messages = vec![ClaudeMessage {
            role: "user".into(),
            content: serde_json::Value::String("hello there".into()),
            cache_anchor: None,
        }];
        apply_cache_control_to_last_message(&mut messages);
        let arr = messages[0].content.as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["type"], "text");
        assert_eq!(arr[0]["text"], "hello there");
        assert_eq!(arr[0]["cache_control"]["type"], "ephemeral");
    }

    #[test]
    fn apply_cache_control_to_last_message_array_content_marks_last_block_only() {
        let mut messages = vec![ClaudeMessage {
            role: "user".into(),
            content: serde_json::json!([
                {"type": "text", "text": "first block"},
                {"type": "text", "text": "second block"},
            ]),
            cache_anchor: None,
        }];
        apply_cache_control_to_last_message(&mut messages);
        let arr = messages[0].content.as_array().unwrap();
        assert!(arr[0].get("cache_control").is_none());
        assert_eq!(arr[1]["cache_control"]["type"], "ephemeral");
    }

    fn from_blocks(role: &str, blocks: Vec<ContentBlock>) -> ClaudeMessage {
        let content = message_content_to_claude_value(&MessageContent::Blocks(blocks));
        ClaudeMessage {
            role: role.into(),
            content: content.value,
            cache_anchor: content.anchor,
        }
    }

    #[test]
    fn the_last_message_mark_clears_the_engines_tail_blocks() {
        let mut messages = vec![from_blocks(
            "user",
            vec![
                ContentBlock::ToolResult {
                    tool_use_id: "call-1".into(),
                    content: "a big result".into(),
                },
                ContentBlock::Text {
                    text: "Results above.".into(),
                },
                ContentBlock::EngineTail {
                    text: "[CONTEXT PANEL]\nYou are holding …".into(),
                },
                ContentBlock::EngineTail {
                    text: "[WORKING UNDERSTANDING]\nwhat I know\n".into(),
                },
            ],
        )];
        apply_cache_control_to_last_message(&mut messages);
        let arr = messages[0].content.as_array().unwrap();
        assert_eq!(
            arr[1]["cache_control"]["type"], "ephemeral",
            "the mark belongs on the instruction, in front of the tail blocks"
        );
        assert_eq!(
            arr.iter()
                .filter(|b| b.get("cache_control").is_some())
                .count(),
            1,
            "still exactly one mark on this message, so the count of four holds"
        );
    }

    #[test]
    fn a_message_of_only_tail_blocks_falls_back_to_its_last() {
        let mut messages = vec![from_blocks(
            "user",
            vec![
                ContentBlock::EngineTail {
                    text: "[CONTEXT PANEL]\nx".into(),
                },
                ContentBlock::EngineTail {
                    text: "[WORKING UNDERSTANDING]\ny".into(),
                },
            ],
        )];
        apply_cache_control_to_last_message(&mut messages);
        let arr = messages[0].content.as_array().unwrap();
        assert!(arr[0].get("cache_control").is_none());
        assert_eq!(arr[1]["cache_control"]["type"], "ephemeral");
    }

    #[test]
    fn an_empty_tail_block_never_reaches_the_wire() {
        let message = from_blocks(
            "user",
            vec![
                ContentBlock::Text { text: "".into() },
                ContentBlock::EngineTail { text: "".into() },
                ContentBlock::Text {
                    text: "the only survivor".into(),
                },
            ],
        );
        let arr = message.content.as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["text"], "the only survivor");
    }

    #[test]
    fn a_control_arm_message_is_marked_on_its_final_block() {
        let mut messages = vec![from_blocks(
            "user",
            vec![
                ContentBlock::ToolResult {
                    tool_use_id: "call-1".into(),
                    content: "a result".into(),
                },
                ContentBlock::Text {
                    text: "Results above.".into(),
                },
            ],
        )];
        apply_cache_control_to_last_message(&mut messages);
        let arr = messages[0].content.as_array().unwrap();
        assert_eq!(arr[1]["cache_control"]["type"], "ephemeral");
    }

    #[test]
    fn a_user_message_opening_with_the_panel_heading_still_anchors_on_itself() {
        let mut messages = vec![from_blocks(
            "user",
            vec![ContentBlock::Text {
                text: "[CONTEXT PANEL] is what you sent me. What is it?".into(),
            }],
        )];
        apply_cache_control_to_last_message(&mut messages);
        let arr = messages[0].content.as_array().unwrap();
        assert_eq!(
            arr[0]["cache_control"]["type"], "ephemeral",
            "the user's own text is not a tail block, whatever it opens with"
        );
    }

    #[test]
    fn apply_cache_control_to_last_message_only_touches_final_message() {
        let mut messages = vec![
            ClaudeMessage {
                role: "user".into(),
                content: serde_json::Value::String("first turn".into()),
                cache_anchor: None,
            },
            ClaudeMessage {
                role: "assistant".into(),
                content: serde_json::Value::String("second turn".into()),
                cache_anchor: None,
            },
        ];
        apply_cache_control_to_last_message(&mut messages);
        assert!(messages[0].content.is_string());
        assert!(messages[1].content.is_array());
    }

    #[test]
    fn apply_cache_control_to_last_message_empty_is_noop() {
        let mut messages: Vec<ClaudeMessage> = Vec::new();
        apply_cache_control_to_last_message(&mut messages);
        assert!(messages.is_empty());
    }

    #[test]
    fn apply_cache_control_to_penultimate_message_string_content_becomes_block() {
        let mut messages = vec![
            ClaudeMessage {
                role: "user".into(),
                content: serde_json::Value::String("first turn".into()),
                cache_anchor: None,
            },
            ClaudeMessage {
                role: "assistant".into(),
                content: serde_json::Value::String("second turn".into()),
                cache_anchor: None,
            },
        ];
        apply_cache_control_to_penultimate_message(&mut messages);
        let arr = messages[0].content.as_array().unwrap();
        assert_eq!(arr[0]["text"], "first turn");
        assert_eq!(arr[0]["cache_control"]["type"], "ephemeral");
        assert!(messages[1].content.is_string());
    }

    #[test]
    fn apply_cache_control_to_penultimate_message_marks_last_block_only() {
        let mut messages = vec![
            ClaudeMessage {
                role: "user".into(),
                content: serde_json::json!([
                    {"type": "text", "text": "first block"},
                    {"type": "text", "text": "second block"},
                ]),
                cache_anchor: None,
            },
            ClaudeMessage {
                role: "assistant".into(),
                content: serde_json::Value::String("tail".into()),
                cache_anchor: None,
            },
        ];
        apply_cache_control_to_penultimate_message(&mut messages);
        let arr = messages[0].content.as_array().unwrap();
        assert!(arr[0].get("cache_control").is_none());
        assert_eq!(arr[1]["cache_control"]["type"], "ephemeral");
    }

    #[test]
    fn apply_cache_control_to_penultimate_message_picks_the_second_from_last() {
        let mut messages = vec![
            ClaudeMessage {
                role: "user".into(),
                content: serde_json::Value::String("oldest".into()),
                cache_anchor: None,
            },
            ClaudeMessage {
                role: "assistant".into(),
                content: serde_json::Value::String("middle".into()),
                cache_anchor: None,
            },
            ClaudeMessage {
                role: "user".into(),
                content: serde_json::Value::String("newest".into()),
                cache_anchor: None,
            },
        ];
        apply_cache_control_to_penultimate_message(&mut messages);
        assert!(messages[0].content.is_string());
        assert!(messages[1].content.is_array());
        assert!(messages[2].content.is_string());
    }

    fn forced_request(model: &str, effort: Option<&str>) -> Result<serde_json::Value, String> {
        let (mut req, _) = build_claude_request(
            vec![Message {
                role: "user".into(),
                content: MessageContent::Text("hi".into()),
            }],
            vec![crate::llm::judgment::chat::answer_tool(&[])],
            model,
            None,
            effort,
            WireTarget::Direct {
                url: "https://api.anthropic.com/v1/messages",
            },
            "Anthropic",
        );
        req.force_tool(crate::llm::judgment::chat::ANSWER_TOOL, model)?;
        Ok(serde_json::to_value(&req).unwrap())
    }

    #[test]
    fn a_forced_call_names_its_tool_and_turns_thinking_off() {
        for model in ["claude-opus-4-8", "claude-sonnet-4-6", "claude-haiku-4-5"] {
            let json = forced_request(model, Some("high")).unwrap();
            assert_eq!(
                json["tool_choice"],
                serde_json::json!({ "type": "tool", "name": crate::llm::judgment::chat::ANSWER_TOOL }),
                "{model}"
            );
            assert!(json.get("thinking").is_none(), "{model}: {json}");
            assert!(json.get("output_config").is_none(), "{model}");
            assert_eq!(json["max_tokens"], NO_THINKING_MAX_TOKENS, "{model}");
        }
        let json = forced_request("claude-opus-5", Some("high")).unwrap();
        assert_eq!(json["thinking"]["type"], "disabled");
        assert!(json.get("output_config").is_none());
    }

    #[test]
    fn an_always_thinking_model_refuses_a_forced_call_before_it_is_sent() {
        for model in [
            "claude-opus-5-5[1m]",
            "claude-sonnet-5-5",
            "claude-fable-5-1",
            "claude-fable-5-2",
        ] {
            let err = forced_request(model, None).expect_err(model);
            assert!(err.contains("always thinks"), "{err}");
        }
    }

    #[test]
    fn an_unforced_request_sends_no_tool_choice() {
        let (req, _) = build_claude_request(
            vec![Message {
                role: "user".into(),
                content: MessageContent::Text("hi".into()),
            }],
            vec![crate::llm::judgment::chat::answer_tool(&[])],
            "claude-opus-4-8",
            None,
            None,
            WireTarget::Direct {
                url: "https://api.anthropic.com/v1/messages",
            },
            "Anthropic",
        );
        assert!(serde_json::to_value(&req)
            .unwrap()
            .get("tool_choice")
            .is_none());
    }

    #[test]
    fn apply_cache_control_to_penultimate_message_needs_two_messages() {
        let mut messages = vec![ClaudeMessage {
            role: "user".into(),
            content: serde_json::Value::String("only turn".into()),
            cache_anchor: None,
        }];
        apply_cache_control_to_penultimate_message(&mut messages);
        assert!(messages[0].content.is_string());

        let mut empty: Vec<ClaudeMessage> = Vec::new();
        apply_cache_control_to_penultimate_message(&mut empty);
        assert!(empty.is_empty());
    }

    #[test]
    fn apply_cache_control_to_last_message_skips_empty_string() {
        let mut messages = vec![ClaudeMessage {
            role: "user".into(),
            content: serde_json::Value::String(String::new()),
            cache_anchor: None,
        }];
        apply_cache_control_to_last_message(&mut messages);
        assert!(messages[0].content.is_string());
        assert_eq!(messages[0].content.as_str(), Some(""));
    }

    #[test]
    fn vertex_request_carries_anthropic_version_no_model_field() {
        let (req, betas) = build_claude_request(
            vec![Message {
                role: "user".into(),
                content: MessageContent::Text("hi".into()),
            }],
            vec![],
            "claude-opus-4-8[1m]",
            Some("system prompt body"),
            Some("high"),
            WireTarget::Vertex {
                url: VERTEX_TEST_URL,
            },
            "Vertex",
        );
        assert!(betas.is_empty(), "the 1M beta rides the body on Vertex");
        let json = serde_json::to_value(&req).unwrap();
        assert_eq!(json["anthropic_version"], "vertex-2023-10-16");
        assert!(json.get("model").is_none(), "Vertex omits body model field");
        assert_eq!(
            json["anthropic_beta"],
            serde_json::json!([ANTHROPIC_BETA_1M_CONTEXT])
        );
        assert!(json["thinking"].get("display").is_none());
    }

    #[test]
    fn vertex_opus_5_5_sends_the_progress_update_beta_as_a_header() {
        let (req, betas) = build_claude_request(
            vec![Message {
                role: "user".into(),
                content: MessageContent::Text("hi".into()),
            }],
            vec![],
            "claude-opus-5-5",
            None,
            Some("medium"),
            WireTarget::Vertex {
                url: VERTEX_TEST_URL,
            },
            "Vertex",
        );
        assert_eq!(betas, vec![ANTHROPIC_BETA_THINKING_DISPLAY_UPDATES]);
        assert_eq!(req.thinking_display(), ThinkingDisplay::ProgressUpdates);
        let json = serde_json::to_value(&req).unwrap();
        assert_eq!(json["thinking"]["type"], "adaptive");
        assert_eq!(json["thinking"]["display"], "updates");
        assert!(json.get("anthropic_beta").is_none());
    }

    #[test]
    fn only_always_thinking_models_ask_for_progress_updates() {
        for (model, expected) in [
            ("claude-opus-5-5", ThinkingDisplay::ProgressUpdates),
            ("claude-fable-5-1", ThinkingDisplay::ProgressUpdates),
            ("claude-fable-5", ThinkingDisplay::ProgressUpdates),
            ("claude-sonnet-5-5", ThinkingDisplay::ProgressUpdates),
            ("claude-sonnet-5-5[1m]", ThinkingDisplay::ProgressUpdates),
            ("claude-opus-5", ThinkingDisplay::Hidden),
            ("claude-sonnet-5", ThinkingDisplay::Hidden),
            ("claude-sonnet-5[1m]", ThinkingDisplay::Hidden),
            ("claude-opus-4-8", ThinkingDisplay::Hidden),
            ("claude-sonnet-4-6", ThinkingDisplay::Hidden),
            ("claude-haiku-4-5", ThinkingDisplay::Hidden),
        ] {
            let (req, betas) = build_claude_request(
                vec![Message {
                    role: "user".into(),
                    content: MessageContent::Text("hi".into()),
                }],
                vec![],
                model,
                None,
                Some("high"),
                WireTarget::Direct {
                    url: "https://api.anthropic.com/v1/messages",
                },
                "Anthropic",
            );
            assert_eq!(req.thinking_display(), expected, "{model}");
            assert_eq!(
                betas.contains(&ANTHROPIC_BETA_THINKING_DISPLAY_UPDATES),
                expected == ThinkingDisplay::ProgressUpdates,
                "{model}"
            );
        }
    }

    #[test]
    fn direct_request_carries_model_no_anthropic_version_or_beta() {
        let (req, betas) = build_claude_request(
            vec![Message {
                role: "user".into(),
                content: MessageContent::Text("hi".into()),
            }],
            vec![],
            "claude-fable-5[1m]",
            None,
            Some("high"),
            WireTarget::Direct {
                url: "https://api.anthropic.com/v1/messages",
            },
            "Anthropic",
        );
        assert_eq!(
            betas,
            vec![
                ANTHROPIC_BETA_1M_CONTEXT,
                ANTHROPIC_BETA_THINKING_DISPLAY_UPDATES
            ]
        );
        let json = serde_json::to_value(&req).unwrap();
        assert_eq!(json["model"], "claude-fable-5");
        assert!(json.get("anthropic_version").is_none());
        assert!(json.get("anthropic_beta").is_none());
        assert_eq!(json["thinking"]["type"], "adaptive");
        assert_eq!(json["thinking"]["display"], "updates");
        assert_eq!(json["output_config"]["effort"], "high");
    }

    #[test]
    fn cache_control_serializes_into_wire_format() {
        let mut tools = vec![
            ClaudeTool {
                name: "search".into(),
                description: "search the web".into(),
                input_schema: serde_json::json!({"type": "object"}),
                cache_control: None,
            },
            ClaudeTool {
                name: "calculator".into(),
                description: "do math".into(),
                input_schema: serde_json::json!({"type": "object"}),
                cache_control: None,
            },
        ];
        apply_cache_control_to_last_tool(&mut tools);

        let mut messages = vec![
            ClaudeMessage {
                role: "user".into(),
                content: serde_json::Value::String("first turn".into()),
                cache_anchor: None,
            },
            ClaudeMessage {
                role: "assistant".into(),
                content: serde_json::Value::String("response".into()),
                cache_anchor: None,
            },
            ClaudeMessage {
                role: "user".into(),
                content: serde_json::Value::String("follow-up".into()),
                cache_anchor: None,
            },
        ];
        apply_cache_control_to_last_message(&mut messages);
        apply_cache_control_to_penultimate_message(&mut messages);

        let req = ClaudeRequest {
            anthropic_version: Some("vertex-2023-10-16".into()),
            model: None,
            max_tokens: 1024,
            stream: true,
            system: system_block(Some("system prompt body"), true),
            messages,
            tools: Some(tools),
            thinking: None,
            output_config: None,
            anthropic_beta: None,
            tool_choice: None,
            stable_prefix_end: None,
        };

        let json = serde_json::to_value(&req).unwrap();

        let tools_arr = json["tools"].as_array().unwrap();
        assert!(tools_arr[0].get("cache_control").is_none());
        assert_eq!(tools_arr[1]["cache_control"]["type"], "ephemeral");

        let system_arr = json["system"].as_array().unwrap();
        assert_eq!(system_arr[0]["cache_control"]["type"], "ephemeral");

        let msgs = json["messages"].as_array().unwrap();
        assert!(msgs[0]["content"].is_string());
        for index in [1, 2] {
            let blocks = msgs[index]["content"].as_array().unwrap();
            assert_eq!(
                blocks.last().unwrap()["cache_control"]["type"],
                "ephemeral",
                "message {index} should anchor a cache prefix"
            );
        }

        assert_eq!(breakpoints(&req), 4);
    }

    #[test]
    fn process_sse_captures_redacted_thinking_data_field() {
        let mut blocks = Vec::new();
        let mut meta = TurnMeta::default();
        let event = r#"{"type":"content_block_start","index":0,"content_block":{"type":"redacted_thinking","data":"AAAAAA=="}}"#;

        process_sse_data(event, &mut blocks, &mut meta, "Test").unwrap();

        match &blocks[0] {
            AccumulatedBlock::RedactedThinking { payload_len } => assert_eq!(*payload_len, 8),
            _ => panic!("redacted_thinking must bucket as RedactedThinking"),
        }
        assert!(!settle_progress_note(
            &mut blocks,
            0,
            ThinkingDisplay::ProgressUpdates
        ));
    }

    #[test]
    fn process_sse_increments_unknown_for_new_block_type() {
        let mut blocks = Vec::new();
        let mut meta = TurnMeta::default();
        let event = r#"{"type":"content_block_start","index":0,"content_block":{"type":"some_new_block_type"}}"#;

        process_sse_data(event, &mut blocks, &mut meta, "Test").unwrap();

        assert_eq!(meta.unknown_sse_dropped, 1);
        assert!(matches!(blocks[0], AccumulatedBlock::Dropped));
    }

    #[test]
    fn process_sse_increments_unknown_for_new_delta_type() {
        let mut blocks = Vec::new();
        let mut meta = TurnMeta::default();
        process_sse_data(
            r#"{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#,
            &mut blocks,
            &mut meta,
            "Test",
        )
        .unwrap();
        process_sse_data(
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"future_delta_type","value":"ignored"}}"#,
            &mut blocks,
            &mut meta,
            "Test",
        )
        .unwrap();

        assert_eq!(meta.unknown_sse_dropped, 1);
    }

    #[test]
    fn process_sse_signature_delta_is_known_quiet() {
        let mut blocks = Vec::new();
        let mut meta = TurnMeta::default();
        process_sse_data(
            r#"{"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":""}}"#,
            &mut blocks,
            &mut meta,
            "Test",
        )
        .unwrap();
        process_sse_data(
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"abc"}}"#,
            &mut blocks,
            &mut meta,
            "Test",
        )
        .unwrap();

        assert_eq!(meta.unknown_sse_dropped, 0);
    }

    #[test]
    fn tool_argument_special_characters_survive_the_sse_accumulator() {
        let mut blocks = Vec::new();
        let mut meta = TurnMeta::default();
        process_sse_data(
            r#"{"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"tu_1","name":"trigger_groups"}}"#,
            &mut blocks,
            &mut meta,
            "Test",
        )
        .unwrap();
        for partial in [
            r#"{\"name\": \"Machine & Too"#,
            r#"ling <Health> \\"#,
            r#"\"q\\\" 'a'\", "#,
            r#"\"action\": "#,
            r#"\"create\"}"#,
        ] {
            process_sse_data(
                &format!(
                    r#"{{"type":"content_block_delta","index":0,"delta":{{"type":"input_json_delta","partial_json":"{partial}"}}}}"#
                ),
                &mut blocks,
                &mut meta,
                "Test",
            )
            .unwrap();
        }

        let AccumulatedBlock::ToolUse { json_parts, .. } = &blocks[0] else {
            panic!("expected a tool_use block");
        };
        let args: serde_json::Value = serde_json::from_str(json_parts).expect("valid tool JSON");
        assert_eq!(
            args["name"], "Machine & Tooling <Health> \"q\" 'a'",
            "the accumulator must not entity-escape tool argument text"
        );
        assert_eq!(args["action"], "create");
        assert_eq!(meta.unknown_sse_dropped, 0);
    }

    const MESSAGE_START: &str = concat!(
        "event: message_start\n",
        r#"data: {"type":"message_start","message":{"id":"msg_x","type":"message","#,
        r#""role":"assistant","content":[],"model":"claude-opus-5","stop_reason":null,"#,
        r#""stop_sequence":null,"usage":{"input_tokens":2,"cache_creation_input_tokens":2369,"#,
        r#""cache_read_input_tokens":117174,"output_tokens":1}}}"#,
        "\n\n"
    );

    fn sse_response(body: impl Into<reqwest::Body>) -> reqwest::Response {
        reqwest::Response::from(axum::http::Response::new(body.into()))
    }

    fn truncated_tool_args_sse(stop_reason: Option<&str>) -> String {
        truncated_tool_args_named("write_file", stop_reason)
    }

    fn truncated_tool_args_named(tool_name: &str, stop_reason: Option<&str>) -> String {
        let mut body = format!(
            concat!(
                r#"data: {{"type":"message_start","message":{{"usage":{{"input_tokens":112000}}}}}}"#,
                "\n\n",
                r#"data: {{"type":"content_block_start","index":0,"#,
                r#""content_block":{{"type":"tool_use","id":"tu_1","name":"{}"}}}}"#,
                "\n\n",
                r#"data: {{"type":"content_block_delta","index":0,"#,
                r#""delta":{{"type":"input_json_delta","#,
                r#""partial_json":"{{\"path\": \"artifacts/research/architecture.md\""}}}}"#,
                "\n\n"
            ),
            tool_name
        );
        if let Some(reason) = stop_reason {
            body.push_str(&format!(
                r#"data: {{"type":"message_delta","delta":{{"stop_reason":"{reason}"}},"#
            ));
            body.push_str(r#""usage":{"output_tokens":128000}}"#);
            body.push_str("\n\n");
        }
        body
    }

    fn text_then_truncated_tool_args_sse() -> String {
        String::from(concat!(
            r#"data: {"type":"message_start","message":{"usage":{"input_tokens":112000}}}"#,
            "\n\n",
            r#"data: {"type":"content_block_start","index":0,"#,
            r#""content_block":{"type":"text","text":""}}"#,
            "\n\n",
            r#"data: {"type":"content_block_delta","index":0,"#,
            r#""delta":{"type":"text_delta","text":"Writing that now."}}"#,
            "\n\n",
            r#"data: {"type":"content_block_start","index":1,"#,
            r#""content_block":{"type":"tool_use","id":"tu_1","name":"write_file"}}"#,
            "\n\n",
            r#"data: {"type":"content_block_delta","index":1,"#,
            r#""delta":{"type":"input_json_delta","#,
            r#""partial_json":"{\"path\": \"artifacts/research/architecture.md\""}}"#,
            "\n\n"
        ))
    }

    fn recording_callback() -> (TokenCallback, std::sync::Arc<std::sync::Mutex<String>>) {
        let seen = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
        let sink = seen.clone();
        let cb: TokenCallback = Box::new(move |t: &str| {
            sink.lock().expect("callback mutex").push_str(t);
        });
        (cb, seen)
    }

    #[tokio::test]
    async fn a_stream_closed_before_message_delta_is_a_retryable_truncation() {
        let err = parse_claude_stream(
            sse_response(MESSAGE_START),
            &None,
            ThinkingDisplay::Hidden,
            "Test",
        )
        .await
        .expect_err("a stream that ended before message_delta is not a completed turn");

        let msg = err.to_string();
        assert!(
            msg.contains("stream truncated"),
            "the error must name the truncation, got: {msg}"
        );
        assert!(
            crate::llm::is_retryable_error(&msg),
            "the truncation must reach the retry path, got: {msg}"
        );
    }

    #[tokio::test]
    async fn a_clean_empty_turn_still_parses_as_a_completed_response() {
        const BODY: &str = concat!(
            r#"data: {"type":"message_start","message":{"usage":{"input_tokens":9}}}"#,
            "\n\n",
            r#"data: {"type":"message_delta","delta":{"stop_reason":"end_turn"},"#,
            r#""usage":{"output_tokens":3}}"#,
            "\n\n",
            r#"data: {"type":"message_stop"}"#,
            "\n\n"
        );

        let response =
            parse_claude_stream(sse_response(BODY), &None, ThinkingDisplay::Hidden, "Test")
                .await
                .expect("a stream carrying a stop reason is a completed turn");

        assert_eq!(response.stop_reason.as_deref(), Some("end_turn"));
        assert_eq!(response.content, None);
        assert!(response.tool_calls.is_empty());
    }

    #[tokio::test]
    async fn a_truncated_stream_that_already_carried_text_keeps_its_content() {
        const BODY: &str = concat!(
            r#"data: {"type":"message_start","message":{"usage":{"input_tokens":9}}}"#,
            "\n\n",
            r#"data: {"type":"content_block_start","index":0,"#,
            r#""content_block":{"type":"text","text":""}}"#,
            "\n\n",
            r#"data: {"type":"content_block_delta","index":0,"#,
            r#""delta":{"type":"text_delta","text":"Checking"}}"#,
            "\n\n"
        );

        let response =
            parse_claude_stream(sse_response(BODY), &None, ThinkingDisplay::Hidden, "Test")
                .await
                .expect("partial text is still a parse, not a truncation");

        assert_eq!(response.content.as_deref(), Some("Checking"));
        assert_eq!(response.stop_reason, None);
    }

    #[tokio::test]
    async fn two_text_blocks_are_accumulated_not_overwritten() {
        const BODY: &str = concat!(
            r#"data: {"type":"message_start","message":{"usage":{"input_tokens":9}}}"#,
            "\n\n",
            r#"data: {"type":"content_block_start","index":0,"#,
            r#""content_block":{"type":"text","text":""}}"#,
            "\n\n",
            r#"data: {"type":"content_block_delta","index":0,"#,
            r#""delta":{"type":"text_delta","text":"First sentence."}}"#,
            "\n\n",
            r#"data: {"type":"content_block_start","index":1,"#,
            r#""content_block":{"type":"text","text":""}}"#,
            "\n\n",
            r#"data: {"type":"content_block_delta","index":1,"#,
            r#""delta":{"type":"text_delta","text":"Second sentence."}}"#,
            "\n\n",
            r#"data: {"type":"message_delta","delta":{"stop_reason":"end_turn"},"#,
            r#""usage":{"output_tokens":6}}"#,
            "\n\n"
        );

        let (cb, seen) = recording_callback();
        let response = parse_claude_stream(
            sse_response(BODY),
            &Some(cb),
            ThinkingDisplay::Hidden,
            "Test",
        )
        .await
        .expect("a two-text-block turn parses");

        assert_eq!(
            response.content.as_deref(),
            Some("First sentence.\nSecond sentence.")
        );
        assert_eq!(
            *seen.lock().expect("callback mutex"),
            response.content.unwrap()
        );
    }

    #[tokio::test]
    async fn three_text_blocks_across_a_thinking_block_are_all_kept() {
        const BODY: &str = concat!(
            r#"data: {"type":"message_start","message":{"usage":{"input_tokens":9}}}"#,
            "\n\n",
            r#"data: {"type":"content_block_start","index":0,"#,
            r#""content_block":{"type":"text","text":""}}"#,
            "\n\n",
            r#"data: {"type":"content_block_delta","index":0,"#,
            r#""delta":{"type":"text_delta","text":"One."}}"#,
            "\n\n",
            r#"data: {"type":"content_block_start","index":1,"#,
            r#""content_block":{"type":"text","text":""}}"#,
            "\n\n",
            r#"data: {"type":"content_block_delta","index":1,"#,
            r#""delta":{"type":"text_delta","text":"Two."}}"#,
            "\n\n",
            r#"data: {"type":"content_block_start","index":2,"#,
            r#""content_block":{"type":"thinking","thinking":""}}"#,
            "\n\n",
            r#"data: {"type":"content_block_delta","index":2,"#,
            r#""delta":{"type":"thinking_delta","thinking":"weighing it"}}"#,
            "\n\n",
            r#"data: {"type":"content_block_start","index":3,"#,
            r#""content_block":{"type":"text","text":""}}"#,
            "\n\n",
            r#"data: {"type":"content_block_delta","index":3,"#,
            r#""delta":{"type":"text_delta","text":"Three."}}"#,
            "\n\n",
            r#"data: {"type":"message_delta","delta":{"stop_reason":"end_turn"},"#,
            r#""usage":{"output_tokens":9}}"#,
            "\n\n"
        );

        let (cb, seen) = recording_callback();
        let response = parse_claude_stream(
            sse_response(BODY),
            &Some(cb),
            ThinkingDisplay::Hidden,
            "Test",
        )
        .await
        .expect("a three-text-block turn parses");

        assert_eq!(response.content.as_deref(), Some("One.\nTwo.\nThree."));
        assert_eq!(
            *seen.lock().expect("callback mutex"),
            response.content.unwrap()
        );
    }

    const PROGRESS_NOTE_TURN: &str = concat!(
        r#"data: {"type":"message_start","message":{"usage":{"input_tokens":9}}}"#,
        "\n\n",
        r#"data: {"type":"content_block_start","index":0,"#,
        r#""content_block":{"type":"thinking","thinking":""}}"#,
        "\n\n",
        r#"data: {"type":"content_block_delta","index":0,"#,
        r#""delta":{"type":"signature_delta","signature":"sig0"}}"#,
        "\n\n",
        r#"data: {"type":"content_block_stop","index":0}"#,
        "\n\n",
        r#"data: {"type":"content_block_start","index":1,"#,
        r#""content_block":{"type":"thinking","thinking":""}}"#,
        "\n\n",
        r#"data: {"type":"content_block_delta","index":1,"#,
        r#""delta":{"type":"thinking_delta","thinking":"Found the config. "}}"#,
        "\n\n",
        r#"data: {"type":"content_block_delta","index":1,"#,
        r#""delta":{"type":"thinking_delta","thinking":"Checking the port next."}}"#,
        "\n\n",
        r#"data: {"type":"content_block_stop","index":1}"#,
        "\n\n",
        r#"data: {"type":"content_block_start","index":2,"#,
        r#""content_block":{"type":"text","text":""}}"#,
        "\n\n",
        r#"data: {"type":"content_block_delta","index":2,"#,
        r#""delta":{"type":"text_delta","text":"The port is 8080."}}"#,
        "\n\n",
        r#"data: {"type":"content_block_stop","index":2}"#,
        "\n\n",
        r#"data: {"type":"message_delta","delta":{"stop_reason":"end_turn"},"#,
        r#""usage":{"output_tokens":40}}"#,
        "\n\n"
    );

    #[tokio::test]
    async fn a_progress_note_streams_and_joins_content_in_block_order() {
        let (cb, seen) = recording_callback();
        let response = parse_claude_stream(
            sse_response(PROGRESS_NOTE_TURN),
            &Some(cb),
            ThinkingDisplay::ProgressUpdates,
            "Test",
        )
        .await
        .expect("a progress-note turn parses");

        assert_eq!(
            response.content.as_deref(),
            Some("Found the config. Checking the port next.\nThe port is 8080.")
        );
        assert_eq!(
            *seen.lock().expect("callback mutex"),
            response.content.clone().unwrap()
        );
        assert_eq!(response.thinking_blocks, Some(2));
        assert_eq!(
            response.progress_notes,
            ["Found the config. Checking the port next."],
            "the note is reported as a summary, and the text block is not"
        );
    }

    #[tokio::test]
    async fn thinking_text_stays_hidden_when_progress_updates_were_not_requested() {
        let (cb, seen) = recording_callback();
        let response = parse_claude_stream(
            sse_response(PROGRESS_NOTE_TURN),
            &Some(cb),
            ThinkingDisplay::Hidden,
            "Test",
        )
        .await
        .expect("the turn parses");

        assert_eq!(response.content.as_deref(), Some("The port is 8080."));
        assert_eq!(*seen.lock().expect("callback mutex"), "The port is 8080.");
        assert_eq!(response.thinking_blocks, Some(2));
        assert!(response.thinking_chars.unwrap() > 0);
        assert!(response.progress_notes.is_empty());
    }

    #[tokio::test]
    async fn the_interrupted_work_sentinel_never_reaches_the_user() {
        const BODY: &str = concat!(
            r#"data: {"type":"message_start","message":{"usage":{"input_tokens":9}}}"#,
            "\n\n",
            r#"data: {"type":"content_block_start","index":0,"#,
            r#""content_block":{"type":"text","text":""}}"#,
            "\n\n",
            r#"data: {"type":"content_block_delta","index":0,"#,
            r#""delta":{"type":"text_delta","text":"Started on it."}}"#,
            "\n\n",
            r#"data: {"type":"content_block_stop","index":0}"#,
            "\n\n",
            r#"data: {"type":"content_block_start","index":1,"#,
            r#""content_block":{"type":"thinking","thinking":""}}"#,
            "\n\n",
            r#"data: {"type":"content_block_delta","index":1,"#,
            r#""delta":{"type":"thinking_delta","#,
            r#""thinking":"This part of the response was interrupted before it finished."}}"#,
            "\n\n",
            r#"data: {"type":"content_block_stop","index":1}"#,
            "\n\n",
            r#"data: {"type":"message_delta","delta":{"stop_reason":"max_tokens"},"#,
            r#""usage":{"output_tokens":40}}"#,
            "\n\n"
        );

        let (cb, seen) = recording_callback();
        let response = parse_claude_stream(
            sse_response(BODY),
            &Some(cb),
            ThinkingDisplay::ProgressUpdates,
            "Test",
        )
        .await
        .expect("the turn parses");

        assert_eq!(response.content.as_deref(), Some("Started on it."));
        assert_eq!(*seen.lock().expect("callback mutex"), "Started on it.");
        assert!(response.progress_notes.is_empty());
    }

    fn tool_use_stop_without_its_block_sse(preamble: &str) -> String {
        format!(
            concat!(
                r#"data: {{"type":"message_start","message":{{"usage":{{"input_tokens":9}}}}}}"#,
                "\n\n",
                "{preamble}",
                r#"data: {{"type":"content_block_start","index":1,"#,
                r#""content_block":{{"type":"thinking","thinking":""}}}}"#,
                "\n\n",
                r#"data: {{"type":"content_block_delta","index":1,"#,
                r#""delta":{{"type":"thinking_delta","#,
                r#""thinking":"This part of the response was interrupted before it finished."}}}}"#,
                "\n\n",
                r#"data: {{"type":"content_block_stop","index":1}}"#,
                "\n\n",
                r#"data: {{"type":"message_delta","delta":{{"stop_reason":"tool_use"}},"#,
                r#""usage":{{"output_tokens":1126}}}}"#,
                "\n\n"
            ),
            preamble = preamble,
        )
    }

    #[tokio::test]
    async fn a_tool_use_stop_that_carried_no_tool_call_retries() {
        let (cb, seen) = recording_callback();
        let err = parse_claude_stream(
            sse_response(tool_use_stop_without_its_block_sse("")),
            &Some(cb),
            ThinkingDisplay::ProgressUpdates,
            "Test",
        )
        .await
        .expect_err("a tool_use stop with no tool call is not a completed turn");

        assert!(seen.lock().unwrap().is_empty(), "nothing reached the user");
        let msg = err.to_string();
        assert!(
            crate::llm::is_retryable_error(&msg),
            "the lost tool call must reach the retry path, got: {msg}"
        );
    }

    #[tokio::test]
    async fn a_tool_use_stop_with_no_tool_call_after_streamed_text_keeps_the_text() {
        const PREAMBLE: &str = concat!(
            r#"data: {"type":"content_block_start","index":0,"#,
            r#""content_block":{"type":"text","text":""}}"#,
            "\n\n",
            r#"data: {"type":"content_block_delta","index":0,"#,
            r#""delta":{"type":"text_delta","text":"Checking the filter."}}"#,
            "\n\n",
            r#"data: {"type":"content_block_stop","index":0}"#,
            "\n\n",
        );
        let (cb, seen) = recording_callback();
        let response = parse_claude_stream(
            sse_response(tool_use_stop_without_its_block_sse(PREAMBLE)),
            &Some(cb),
            ThinkingDisplay::ProgressUpdates,
            "Test",
        )
        .await
        .expect("streamed text must not be retried");

        assert_eq!(response.content.as_deref(), Some("Checking the filter."));
        assert_eq!(seen.lock().unwrap().as_str(), "Checking the filter.");
        assert!(response.tool_calls.is_empty());
    }

    #[tokio::test]
    async fn a_stream_cut_mid_progress_note_is_still_a_retryable_truncation() {
        const BODY: &str = concat!(
            r#"data: {"type":"message_start","message":{"usage":{"input_tokens":9}}}"#,
            "\n\n",
            r#"data: {"type":"content_block_start","index":0,"#,
            r#""content_block":{"type":"thinking","thinking":""}}"#,
            "\n\n",
            r#"data: {"type":"content_block_delta","index":0,"#,
            r#""delta":{"type":"thinking_delta","thinking":"Found the config. Checking"}}"#,
            "\n\n"
        );

        let (cb, seen) = recording_callback();
        let err = parse_claude_stream(
            sse_response(BODY),
            &Some(cb),
            ThinkingDisplay::ProgressUpdates,
            "Test",
        )
        .await
        .expect_err("a stream cut mid-note is not a completed turn");

        assert!(seen.lock().unwrap().is_empty(), "nothing reached the user");
        assert!(
            crate::llm::is_retryable_error(&err.to_string()),
            "the truncation must reach the retry path, got: {err}"
        );
    }

    #[tokio::test]
    async fn a_cut_tool_call_after_a_progress_note_reports_instead_of_retrying() {
        const BODY: &str = concat!(
            r#"data: {"type":"message_start","message":{"usage":{"input_tokens":9}}}"#,
            "\n\n",
            r#"data: {"type":"content_block_start","index":0,"#,
            r#""content_block":{"type":"thinking","thinking":""}}"#,
            "\n\n",
            r#"data: {"type":"content_block_delta","index":0,"#,
            r#""delta":{"type":"thinking_delta","thinking":"Writing the report now."}}"#,
            "\n\n",
            r#"data: {"type":"content_block_stop","index":0}"#,
            "\n\n",
            r#"data: {"type":"content_block_start","index":1,"#,
            r#""content_block":{"type":"tool_use","id":"tu_1","name":"write_file"}}"#,
            "\n\n",
            r#"data: {"type":"content_block_delta","index":1,"#,
            r#""delta":{"type":"input_json_delta","partial_json":"{\"path\": \"a"}}"#,
            "\n\n"
        );

        let (cb, seen) = recording_callback();
        let err = parse_claude_stream(
            sse_response(BODY),
            &Some(cb),
            ThinkingDisplay::ProgressUpdates,
            "Test",
        )
        .await
        .expect_err("a cut tool call is still an error");

        assert_eq!(seen.lock().unwrap().as_str(), "Writing the report now.");
        let msg = err.to_string();
        assert!(
            !crate::llm::is_retryable_error(&msg),
            "a rendered note must not be re-streamed by a retry, got: {msg}"
        );
    }

    #[tokio::test]
    async fn tool_arguments_cut_by_a_dropped_connection_retry() {
        let (on_token, seen) = recording_callback();
        let err = parse_claude_stream(
            sse_response(truncated_tool_args_sse(None)),
            &Some(on_token),
            ThinkingDisplay::Hidden,
            "Test",
        )
        .await
        .expect_err("a tool call cut before message_delta is not a completed turn");

        assert!(
            seen.lock().unwrap().is_empty(),
            "no text should have gone out"
        );
        let msg = err.to_string();
        assert!(
            msg.contains("stream truncated") && msg.contains("write_file"),
            "the error must name the truncation and the tool, got: {msg}"
        );
        assert!(
            crate::llm::is_retryable_error(&msg),
            "a dropped connection must reach the retry path, got: {msg}"
        );
    }

    #[tokio::test]
    async fn a_cut_tool_call_after_streamed_text_reports_instead_of_retrying() {
        let (on_token, seen) = recording_callback();
        let err = parse_claude_stream(
            sse_response(text_then_truncated_tool_args_sse()),
            &Some(on_token),
            ThinkingDisplay::Hidden,
            "Test",
        )
        .await
        .expect_err("a cut tool call is still an error");

        assert_eq!(
            seen.lock().unwrap().as_str(),
            "Writing that now.",
            "the callback must have rendered the text already"
        );
        let msg = err.to_string();
        assert!(
            msg.contains("already sending text"),
            "the error must say why it will not retry, got: {msg}"
        );
        assert!(
            !crate::llm::is_retryable_error(&msg),
            "retrying would render the streamed text twice, got: {msg}"
        );
    }

    const TEXT_THEN_OVERLOADED: &str = concat!(
        r#"data: {"type":"message_start","message":{"usage":{"input_tokens":9}}}"#,
        "\n\n",
        r#"data: {"type":"content_block_start","index":0,"#,
        r#""content_block":{"type":"text","text":""}}"#,
        "\n\n",
        r#"data: {"type":"content_block_delta","index":0,"#,
        r#""delta":{"type":"text_delta","text":"Here is the plan."}}"#,
        "\n\n",
        r#"data: {"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}"#,
        "\n\n"
    );

    #[tokio::test]
    async fn an_error_frame_after_streamed_text_reports_instead_of_retrying() {
        let (on_token, seen) = recording_callback();
        let err = parse_claude_stream(
            sse_response(TEXT_THEN_OVERLOADED),
            &Some(on_token),
            ThinkingDisplay::Hidden,
            "Test",
        )
        .await
        .expect_err("an error frame fails the parse");

        assert_eq!(seen.lock().unwrap().as_str(), "Here is the plan.");
        let msg = err.to_string();
        assert!(
            !crate::llm::is_retryable_error(&msg),
            "retrying would render the streamed text twice, got: {msg}"
        );
    }

    #[tokio::test]
    async fn an_error_frame_with_nothing_rendered_still_retries() {
        let err = parse_claude_stream(
            sse_response(TEXT_THEN_OVERLOADED),
            &None,
            ThinkingDisplay::Hidden,
            "Test",
        )
        .await
        .expect_err("an error frame fails the parse");

        let msg = err.to_string();
        assert!(
            crate::llm::is_retryable_error(&msg),
            "nothing reached the user, so the overload must retry, got: {msg}"
        );
    }

    #[tokio::test]
    async fn an_api_error_frame_with_nothing_rendered_retries_and_explains_itself() {
        const BODY: &str = concat!(
            r#"data: {"type":"error","error":{"type":"api_error","message":"upstream error"}}"#,
            "\n\n"
        );
        let err = parse_claude_stream(
            sse_response(BODY),
            &None,
            ThinkingDisplay::Hidden,
            "Anthropic",
        )
        .await
        .expect_err("an error frame fails the parse");

        assert!(
            crate::llm::error_is_retryable(&*err),
            "an api_error is Anthropic's internal error and must retry, got: {err}"
        );
        let msg = err.to_string();
        assert!(msg.contains("[api_error]: upstream error"), "{msg}");
        assert!(msg.contains("internal error"), "{msg}");
    }

    #[tokio::test]
    async fn a_dropped_connection_after_streamed_text_reports_instead_of_retrying() {
        let head = TEXT_THEN_OVERLOADED
            .split("data: {\"type\":\"error\"")
            .next()
            .expect("the body has a text prefix")
            .to_string();
        let chunks: Vec<Result<bytes::Bytes, std::io::Error>> = vec![
            Ok(bytes::Bytes::from(head)),
            Err(std::io::Error::new(
                std::io::ErrorKind::ConnectionReset,
                "connection reset by peer",
            )),
        ];
        let body = reqwest::Body::wrap_stream(futures::stream::iter(chunks));

        let (on_token, seen) = recording_callback();
        let err = parse_claude_stream(
            sse_response(body),
            &Some(on_token),
            ThinkingDisplay::Hidden,
            "Test",
        )
        .await
        .expect_err("a dropped connection fails the parse");

        assert_eq!(seen.lock().unwrap().as_str(), "Here is the plan.");
        let msg = err.to_string();
        assert!(
            !crate::llm::is_retryable_error(&msg),
            "retrying would render the streamed text twice, got: {msg}"
        );
    }

    #[tokio::test]
    async fn a_tool_named_like_an_http_status_cannot_flip_the_classification() {
        for name in ["502", "503", "529"] {
            for stop in ["max_tokens", "tool_use"] {
                let err = parse_claude_stream(
                    sse_response(truncated_tool_args_named(name, Some(stop))),
                    &None,
                    ThinkingDisplay::Hidden,
                    "Test",
                )
                .await
                .expect_err("unparseable arguments are still an error");

                let msg = err.to_string();
                assert!(
                    !crate::llm::is_retryable_error(&msg),
                    "tool '{name}' at stop '{stop}' must stay non-retryable, got: {msg}"
                );
            }
        }
    }

    #[tokio::test]
    async fn the_same_cut_retries_when_no_callback_was_rendering() {
        let err = parse_claude_stream(
            sse_response(text_then_truncated_tool_args_sse()),
            &None,
            ThinkingDisplay::Hidden,
            "Test",
        )
        .await
        .expect_err("a cut tool call is still an error");

        let msg = err.to_string();
        assert!(
            crate::llm::is_retryable_error(&msg),
            "with nothing rendered the truncation is retryable, got: {msg}"
        );
    }

    #[tokio::test]
    async fn tool_arguments_cut_by_the_token_budget_name_the_budget_and_do_not_retry() {
        let err = parse_claude_stream(
            sse_response(truncated_tool_args_sse(Some("max_tokens"))),
            &None,
            ThinkingDisplay::Hidden,
            "Test",
        )
        .await
        .expect_err("a tool call cut at the token budget is not a completed turn");

        let msg = err.to_string();
        assert!(
            msg.contains("output token limit") && msg.contains("write_file"),
            "the error must name the budget and the tool, got: {msg}"
        );
        assert!(
            !msg.contains("EOF while parsing"),
            "the serde wording is what this arm replaces, got: {msg}"
        );
        assert!(
            !crate::llm::is_retryable_error(&msg),
            "an identical retry is cut identically, got: {msg}"
        );
    }

    #[tokio::test]
    async fn malformed_tool_arguments_under_a_clean_stop_keep_the_parse_error() {
        let err = parse_claude_stream(
            sse_response(truncated_tool_args_sse(Some("tool_use"))),
            &None,
            ThinkingDisplay::Hidden,
            "Test",
        )
        .await
        .expect_err("unparseable arguments are still an error");

        let msg = err.to_string();
        assert!(
            msg.contains("Failed to parse tool arguments"),
            "a model bug keeps its own wording, got: {msg}"
        );
        assert!(
            msg.contains("artifacts/research/architecture.md"),
            "the raw arguments are the diagnostic here, got: {msg}"
        );
        assert!(
            !crate::llm::is_retryable_error(&msg),
            "malformed output is not a transport error, got: {msg}"
        );
    }

    fn breakpoints(req: &ClaudeRequest) -> usize {
        serde_json::to_string(req)
            .expect("the request serializes")
            .matches("\"cache_control\"")
            .count()
    }

    fn one_tool() -> Vec<ToolDefinition> {
        vec![ToolDefinition {
            name: "read_file".into(),
            description: "read a file".into(),
            parameters: serde_json::json!({"type": "object"}),
        }]
    }

    fn request_for(messages: Vec<Message>) -> ClaudeRequest {
        build_claude_request(
            messages,
            one_tool(),
            "claude-opus-5",
            Some("system prompt body"),
            Some("high"),
            WireTarget::Vertex {
                url: VERTEX_TEST_URL,
            },
            "Vertex",
        )
        .0
    }

    fn image_block() -> ContentBlock {
        ContentBlock::Image {
            source_type: "base64".into(),
            media_type: "image/png".into(),
            data: "AAAA".into(),
        }
    }

    #[test]
    fn a_single_message_turn_carries_three_breakpoints() {
        let req = request_for(vec![Message {
            role: "user".into(),
            content: MessageContent::Text("the whole payload".into()),
        }]);
        assert_eq!(breakpoints(&req), 3);
    }

    #[test]
    fn several_blocks_in_one_message_still_anchor_nothing() {
        let req = request_for(vec![Message {
            role: "user".into(),
            content: MessageContent::Blocks(vec![
                ContentBlock::Text {
                    text: "the whole payload".into(),
                },
                image_block(),
                ContentBlock::Text {
                    text: "[Below: image attached to current message]".into(),
                },
                image_block(),
            ]),
        }]);
        assert_eq!(
            breakpoints(&req),
            3,
            "one message cannot carry both the tail marker and the anchor"
        );
    }

    #[test]
    fn a_turn_past_its_first_round_carries_four_breakpoints() {
        let req = request_for(vec![
            Message {
                role: "user".into(),
                content: MessageContent::Text("the whole payload".into()),
            },
            Message {
                role: "assistant".into(),
                content: MessageContent::Text("reading the file now".into()),
            },
            Message {
                role: "user".into(),
                content: MessageContent::Text("[tool result]".into()),
            },
        ]);
        assert_eq!(breakpoints(&req), 4);
    }

    fn classic_round_two() -> Vec<Message> {
        vec![
            Message {
                role: "user".into(),
                content: MessageContent::Blocks(vec![
                    ContentBlock::Text {
                        text: "[Long-term Memory]\n- a fact [id: 1]\n\nRequest: hello".into(),
                    },
                    image_block(),
                ]),
            },
            Message {
                role: "assistant".into(),
                content: MessageContent::Blocks(vec![
                    ContentBlock::Text {
                        text: "reading the file now".into(),
                    },
                    ContentBlock::ToolUse {
                        id: "toolu_1".into(),
                        name: "read_file".into(),
                        input: serde_json::json!({"path": "notes.md"}),
                        thought_signature: None,
                    },
                ]),
            },
            Message {
                role: "user".into(),
                content: MessageContent::Blocks(vec![
                    ContentBlock::ToolResult {
                        tool_use_id: "toolu_1".into(),
                        content: "# Notes".into(),
                    },
                    ContentBlock::EngineTail {
                        text: "[CONTEXT PANEL]".into(),
                    },
                ]),
            },
        ]
    }

    fn stable_block() -> ContentBlock {
        ContentBlock::StableContext {
            text: "[USER PROFILE]\n...\n[LOADED KNOWHOW]\n...".into(),
        }
    }

    fn text_block(text: &str) -> ContentBlock {
        ContentBlock::Text { text: text.into() }
    }

    fn user_blocks(blocks: Vec<ContentBlock>) -> Message {
        Message {
            role: "user".into(),
            content: MessageContent::Blocks(blocks),
        }
    }

    fn tree_message(snapshot: bool, rungs: usize) -> Message {
        let mut blocks = Vec::new();
        if snapshot {
            blocks.push(ContentBlock::MemoryView {
                text: "[WORKSPACE MEMORY VIEW]\n[w/0+4] shared\n".into(),
            });
        }
        blocks.push(stable_block());
        if rungs > 0 {
            blocks.push(ContentBlock::MemoryView {
                text: "[THREAD MEMORY VIEW]\n[0+8] old\n".into(),
            });
        }
        if rungs > 1 {
            blocks.push(ContentBlock::MemoryView {
                text: "[8+4] middle\n".into(),
            });
        }
        if rungs > 0 {
            blocks.push(text_block("[12+1] new\n[END THREAD MEMORY VIEW]"));
        }
        blocks.push(text_block("Request: hello"));
        user_blocks(blocks)
    }

    fn turn_of(opening: Message, rounds: usize) -> ClaudeRequest {
        let mut messages = vec![opening];
        for _ in 1..rounds {
            messages.push(Message {
                role: "assistant".into(),
                content: MessageContent::Text("reading the file now".into()),
            });
            messages.push(Message {
                role: "user".into(),
                content: MessageContent::Text("[tool result]".into()),
            });
        }
        request_for(messages)
    }

    fn tree_turn(snapshot: bool, rounds: usize) -> ClaudeRequest {
        turn_of(tree_message(snapshot, 2), rounds)
    }

    fn classic_message() -> Message {
        user_blocks(vec![stable_block(), text_block("Request: hello")])
    }

    fn marked(req: &ClaudeRequest) -> Vec<String> {
        let body = serde_json::to_value(req).expect("the request serializes");
        let has = |v: &serde_json::Value| v.get("cache_control").is_some();
        let mut out = Vec::new();
        if body["tools"].as_array().is_some_and(|t| t.iter().any(has)) {
            out.push("tools".to_string());
        }
        if body["system"].as_array().is_some_and(|s| s.iter().any(has)) {
            out.push("system".to_string());
        }
        for (m, message) in body["messages"].as_array().unwrap().iter().enumerate() {
            for (b, block) in message["content"]
                .as_array()
                .into_iter()
                .flatten()
                .enumerate()
            {
                if has(block) {
                    out.push(format!("m{m}b{b}"));
                }
            }
        }
        out
    }

    #[test]
    fn a_tree_first_round_marks_the_snapshot_and_both_thread_marks() {
        assert_eq!(
            marked(&tree_turn(true, 1)),
            ["m0b0", "m0b2", "m0b3", "m0b5"]
        );
    }

    #[test]
    fn a_later_tree_round_keeps_the_snapshot_and_the_first_thread_mark() {
        assert_eq!(
            marked(&tree_turn(true, 2)),
            ["m0b0", "m0b2", "m1b0", "m2b0"]
        );
    }

    #[test]
    fn a_new_tree_thread_marks_the_system_and_the_stable_block() {
        assert_eq!(
            marked(&turn_of(tree_message(true, 0), 1)),
            ["system", "m0b0", "m0b1", "m0b2"]
        );
    }

    #[test]
    fn a_later_tree_trigger_round_marks_the_snapshot_and_the_system_block() {
        assert_eq!(
            marked(&turn_of(tree_message(true, 0), 2)),
            ["system", "m0b0", "m1b0", "m2b0"]
        );
    }

    #[test]
    fn a_classic_first_round_marks_the_stable_block_and_the_tools() {
        assert_eq!(
            marked(&turn_of(classic_message(), 1)),
            ["tools", "system", "m0b0", "m0b1"]
        );
    }

    #[test]
    fn a_classic_turn_with_an_earlier_message_drops_the_tools_marker() {
        assert_eq!(
            marked(&turn_of(classic_message(), 2)),
            ["system", "m0b0", "m1b0", "m2b0"]
        );
        let resumed = request_for(vec![
            Message {
                role: "assistant".into(),
                content: MessageContent::Text("earlier work".into()),
            },
            Message {
                role: "user".into(),
                content: MessageContent::Text("[tool result]".into()),
            },
            classic_message(),
        ]);
        assert_eq!(marked(&resumed), ["system", "m1b0", "m2b0", "m2b1"]);
    }

    #[test]
    fn a_message_without_a_stable_block_keeps_the_old_markers() {
        assert_eq!(
            marked(&turn_of(user_blocks(vec![text_block("Request: hello")]), 1)),
            ["tools", "system", "m0b0"]
        );
    }

    fn compaction_request(retry: bool) -> ClaudeRequest {
        let view = |text: &str| ContentBlock::MemoryView { text: text.into() };
        let text = |text: &str| ContentBlock::Text { text: text.into() };
        let mut messages = vec![Message {
            role: "user".into(),
            content: MessageContent::Blocks(vec![
                view("<chat>\na\nb\nc\nd\n"),
                view("e\nf\ng\nh\n"),
                view("i\nj\nk\nl\n"),
                text("m\n</chat>\n"),
                text("Compaction: compress this message"),
            ]),
        }];
        if retry {
            messages.push(Message {
                role: "assistant".into(),
                content: MessageContent::Text("a line too long".into()),
            });
            messages.push(Message {
                role: "user".into(),
                content: MessageContent::Text("Too long: your line is 600 bytes".into()),
            });
        }
        request_for(messages)
    }

    #[test]
    fn a_compaction_request_marks_its_rungs_within_the_limit() {
        assert_eq!(
            marked(&compaction_request(false)),
            ["m0b0", "m0b1", "m0b2", "m0b4"]
        );
        assert_eq!(
            marked(&compaction_request(true)),
            ["m0b0", "m0b1", "m1b0", "m2b0"]
        );
    }

    #[test]
    fn a_tree_turn_with_no_snapshot_marks_the_system_block() {
        assert_eq!(
            marked(&tree_turn(false, 1)),
            ["system", "m0b1", "m0b2", "m0b4"]
        );
    }

    #[test]
    fn no_tree_request_carries_more_than_four_breakpoints() {
        for snapshot in [true, false] {
            for rungs in 0..3 {
                for rounds in 1..5 {
                    let req = turn_of(tree_message(snapshot, rungs), rounds);
                    assert!(
                        breakpoints(&req) <= MAX_CACHE_BREAKPOINTS,
                        "snapshot {snapshot}, {rungs} rungs, {rounds} rounds"
                    );
                }
            }
        }
    }

    #[test]
    fn no_classic_request_carries_more_than_four_breakpoints() {
        for rounds in 1..5 {
            for opening in [classic_message(), user_blocks(vec![text_block("hi")])] {
                assert!(breakpoints(&turn_of(opening, rounds)) <= MAX_CACHE_BREAKPOINTS);
            }
        }
    }

    #[test]
    fn a_classic_request_is_byte_identical_to_the_pre_tree_build() {
        let body = serde_json::to_string_pretty(&request_for(classic_round_two()))
            .expect("the request serializes");
        assert_eq!(body, include_str!("anthropic_wire_classic_request.json"));
    }
}
