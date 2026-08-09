//! Provider-agnostic Anthropic Messages API wire format: request building,
//! prompt-cache markers, and SSE stream parsing. Shared by the Vertex Claude
//! path (`llm::vertex::claude`) and the direct Anthropic provider
//! (`llm::anthropic`).
//!
//! The two transports differ only in framing, captured by [`WireTarget`]:
//! - **Vertex** `streamRawPredict` puts the model in the URL, sends
//!   `anthropic_version` in the body, and carries betas in the body
//!   `anthropic_beta` array.
//! - **Direct** `api.anthropic.com/v1/messages` puts the model in the body and
//!   sends `anthropic-version` + betas as HTTP headers (the caller adds them);
//!   the body omits both.
//!
//! Everything else — message/tool conversion, cache-control placement, the
//! thinking/effort config, and the SSE parser — is identical, so it lives here
//! once.

use crate::llm::provider::{
    ContentBlock, LlmResponse, Message, MessageContent, TokenCallback, ToolCall, ToolDefinition,
};
use futures::StreamExt;
use serde::Serialize;
use std::time::Duration;

/// Beta flag enabling the 1M token context window for Claude models. Sent in
/// the body `anthropic_beta` array on Vertex; sent as an `anthropic-beta` HTTP
/// header on the direct API (the direct provider owns that header).
pub(crate) const ANTHROPIC_BETA_1M_CONTEXT: &str = "context-1m-2025-08-07";

/// Per-chunk timeout for Claude SSE streams (seconds).
const CLAUDE_STREAM_CHUNK_TIMEOUT_SECS: u64 = 300;

/// Which transport a request targets — see module docs.
pub(crate) enum WireTarget {
    Vertex,
    Direct,
}

/// Strip `[1m]` suffix from model ID, returning (base_model, is_1m_context).
/// The `[1m]` suffix is a Lucidos convention, not part of the real model id —
/// it selects the 1M-context beta, not a different model.
pub(crate) fn parse_context_suffix(model: &str) -> (&str, bool) {
    if let Some(base) = model.strip_suffix("[1m]") {
        (base, true)
    } else {
        (model, false)
    }
}

/// Models that support extended thinking (reasoning goes to dedicated blocks
/// instead of polluting the text response).
///
/// The Sonnet arms are two rules, not one: `claude-sonnet-4` covers the 4.x
/// line but does NOT match `claude-sonnet-5`, so a new Sonnet generation needs
/// its own arm or it silently falls to the no-thinking 8192-token path.
pub(crate) fn supports_extended_thinking(model: &str) -> bool {
    model.contains("claude-3-7-sonnet")
        || model.contains("claude-sonnet-4")
        || model.contains("claude-sonnet-5")
        || model.contains("claude-opus-4")
        || model.contains("claude-opus-5")
        || model.contains("claude-fable-5")
}

/// Models that only support adaptive thinking (no `budget_tokens`, no
/// `temperature`/`top_p`/`top_k`). Effort is controlled via
/// `output_config.effort`. Covers Opus 4.7+ (incl. Opus 5), Sonnet 5, and
/// Fable 5. Sonnet 4.6 and older stay on the `budget_tokens` path.
pub(crate) fn requires_adaptive_thinking(model: &str) -> bool {
    model.contains("claude-opus-4-7")
        || model.contains("claude-opus-4-8")
        || model.contains("claude-opus-5")
        || model.contains("claude-sonnet-5")
        || model.contains("claude-fable-5")
}

/// Convert MessageContent to the serde_json::Value format Claude expects.
/// Text → JSON string, Blocks → JSON array of content block objects.
fn message_content_to_claude_value(content: &MessageContent) -> serde_json::Value {
    match content {
        MessageContent::Text(s) => serde_json::Value::String(s.clone()),
        MessageContent::Blocks(blocks) => {
            let json_blocks: Vec<serde_json::Value> = blocks
                .iter()
                .filter(|block| !matches!(block, ContentBlock::Text { text } if text.is_empty()))
                .map(|block| match block {
                    ContentBlock::Text { text } => serde_json::json!({
                        "type": "text",
                        "text": text,
                    }),
                    ContentBlock::ToolUse {
                        id,
                        name,
                        input,
                        // Claude API doesn't accept a thought_signature field on
                        // tool_use blocks; the signature is Gemini-only and lives
                        // on the engine-side ContentBlock so it can flow back
                        // through the Vertex Gemini request path.
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
                })
                .collect();
            serde_json::Value::Array(json_blocks)
        }
    }
}

/// Compute the `(thinking, output_config, max_tokens)` triple from the base
/// model + reasoning effort. Adaptive-thinking models (Opus 4.7+, Fable 5) use
/// `output_config.effort`; older thinking models use a `budget_tokens` ceiling;
/// non-thinking models get neither.
fn thinking_config(
    base_model: &str,
    reasoning_effort: Option<&str>,
) -> (Option<ClaudeThinking>, Option<ClaudeOutputConfig>, u32) {
    if !supports_extended_thinking(base_model) {
        return (None, None, 8192);
    }
    let effort = reasoning_effort.unwrap_or("high");
    if effort == "none" {
        (None, None, 8192)
    } else if requires_adaptive_thinking(base_model) {
        (
            Some(ClaudeThinking {
                thinking_type: "adaptive".to_string(),
                budget_tokens: None,
            }),
            Some(ClaudeOutputConfig {
                effort: effort.to_string(),
            }),
            32768,
        )
    } else {
        let budget = crate::llm::thinking_budget_for_effort(effort);
        (
            Some(ClaudeThinking {
                thinking_type: "enabled".to_string(),
                budget_tokens: Some(budget),
            }),
            None,
            budget + 16384,
        )
    }
}

/// Build the Anthropic Messages request body for either transport. Returns the
/// request plus `is_1m` so the direct provider can attach the 1M beta as an
/// HTTP header (Vertex carries it in the body, so it ignores the flag).
/// `provider_tag` labels the pre-flight stub-injection log line.
pub(crate) fn build_claude_request(
    mut messages: Vec<Message>,
    tools: Vec<ToolDefinition>,
    model: &str,
    system_prompt: Option<&str>,
    reasoning_effort: Option<&str>,
    target: WireTarget,
    provider_tag: &str,
) -> (ClaudeRequest, bool) {
    let (base_model, is_1m) = parse_context_suffix(model);

    // Pre-flight: repair orphan `tool_use` blocks before Anthropic 400s.
    // Last line of defense — ANY caller that builds `messages` goes through this
    // gate, even ones that bypass the agentic loop. Lives in `llm::validate` so
    // this layer doesn't reach `up` into `engine::*`.
    let stubs = crate::llm::validate::validate_tool_use_pairing(&mut messages);
    if stubs > 0 {
        crate::log!(
            "[{}] WARNING: pre-flight injected {} stub tool_result block(s) before Claude API call (model={})",
            provider_tag,
            stubs,
            model
        );
    }

    let mut claude_messages: Vec<ClaudeMessage> = messages
        .iter()
        .map(|m| ClaudeMessage {
            role: m.role.clone(),
            content: message_content_to_claude_value(&m.content),
        })
        .collect();

    let claude_tools: Option<Vec<ClaudeTool>> = if tools.is_empty() {
        None
    } else {
        let mut converted: Vec<ClaudeTool> = tools
            .into_iter()
            .map(|t| ClaudeTool {
                name: t.name,
                description: t.description,
                input_schema: t.parameters,
                cache_control: None,
            })
            .collect();
        apply_cache_control_to_last_tool(&mut converted);
        Some(converted)
    };

    apply_cache_control_to_last_message(&mut claude_messages);

    let (thinking, output_config, max_tokens) = thinking_config(base_model, reasoning_effort);

    let (anthropic_version, model_field, anthropic_beta) = match target {
        WireTarget::Vertex => (
            Some("vertex-2023-10-16".to_string()),
            None,
            if is_1m {
                Some(vec![ANTHROPIC_BETA_1M_CONTEXT.to_string()])
            } else {
                None
            },
        ),
        // Direct API: model goes in the body; `anthropic-version` and betas are
        // HTTP headers supplied by the provider, so the body omits both.
        WireTarget::Direct => (None, Some(base_model.to_string()), None),
    };

    let request = ClaudeRequest {
        anthropic_version,
        model: model_field,
        max_tokens,
        stream: true,
        system: system_with_cache_control(system_prompt),
        messages: claude_messages,
        tools: claude_tools,
        thinking,
        output_config,
        anthropic_beta,
    };

    (request, is_1m)
}

/// Parse an SSE stream from Claude's streaming API into an LlmResponse.
/// `provider_tag` labels diagnostic log lines (e.g. "Vertex", "Anthropic").
pub(crate) async fn parse_claude_stream(
    response: reqwest::Response,
    on_token: &Option<TokenCallback>,
    provider_tag: &str,
) -> Result<LlmResponse, Box<dyn std::error::Error + Send + Sync>> {
    let mut stream = response.bytes_stream();
    let mut buffer = String::new();

    // Accumulated content blocks by index
    let mut blocks: Vec<AccumulatedBlock> = Vec::new();
    let mut turn_meta = TurnMeta::default();

    let chunk_timeout = Duration::from_secs(CLAUDE_STREAM_CHUNK_TIMEOUT_SECS);

    loop {
        let chunk = match tokio::time::timeout(chunk_timeout, stream.next()).await {
            Ok(Some(Ok(bytes))) => bytes,
            Ok(Some(Err(e))) => return Err(format!("Stream read error: {}", e).into()),
            Ok(None) => break, // Stream ended
            Err(_) => {
                return Err(format!(
                    "Claude stream timed out (no data for {}s)",
                    CLAUDE_STREAM_CHUNK_TIMEOUT_SECS
                )
                .into())
            }
        };

        buffer.push_str(&String::from_utf8_lossy(&chunk));

        // Process complete lines
        while let Some(newline_pos) = buffer.find('\n') {
            let line = buffer[..newline_pos].trim_end_matches('\r').to_string();
            buffer = buffer[newline_pos + 1..].to_string();

            // SSE data lines
            if let Some(data_str) = line.strip_prefix("data: ") {
                let prev_text_len: usize = blocks
                    .iter()
                    .map(|b| match b {
                        AccumulatedBlock::Text(t) => t.len(),
                        _ => 0,
                    })
                    .sum();
                process_sse_data(data_str, &mut blocks, &mut turn_meta, provider_tag)?;
                if let Some(cb) = on_token {
                    let new_text_len: usize = blocks
                        .iter()
                        .map(|b| match b {
                            AccumulatedBlock::Text(t) => t.len(),
                            _ => 0,
                        })
                        .sum();
                    if new_text_len > prev_text_len {
                        // Floor defensively — `raw_start` is byte-derived and
                        // could land mid-codepoint after future accumulator
                        // changes.
                        for block in blocks.iter().rev() {
                            if let AccumulatedBlock::Text(t) = block {
                                let raw_start = t.len() - (new_text_len - prev_text_len);
                                let delta_start = t.floor_char_boundary(raw_start);
                                cb(&t[delta_start..]);
                                break;
                            }
                        }
                    }
                }
            }
            // Ignore event:, comments (:), and empty lines
        }
    }

    // Build LlmResponse from accumulated blocks
    let mut content = None;
    let mut tool_calls = Vec::new();
    let mut thinking_chars: usize = 0;

    for block in blocks {
        match block {
            AccumulatedBlock::Text(text) => {
                content = Some(text);
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
                        format!(
                            "Failed to parse tool arguments: {} (json: {})",
                            e, json_parts
                        )
                    })?
                };
                tool_calls.push(ToolCall {
                    id,
                    name,
                    arguments,
                    // Claude doesn't emit Gemini-style thought signatures.
                    thought_signature: None,
                });
            }
            AccumulatedBlock::Thinking(t) => {
                thinking_chars = thinking_chars.saturating_add(t.len());
            }
        }
    }

    Ok(LlmResponse {
        content,
        tool_calls,
        stop_reason: turn_meta.stop_reason,
        output_tokens: turn_meta.output_tokens,
        input_tokens: turn_meta.input_tokens,
        cache_creation_tokens: turn_meta.cache_creation_tokens,
        cache_read_tokens: turn_meta.cache_read_tokens,
        thinking_chars: Some(thinking_chars),
        unknown_sse_dropped: turn_meta.unknown_sse_dropped,
    })
}

fn process_sse_data(
    data_str: &str,
    blocks: &mut Vec<AccumulatedBlock>,
    meta: &mut TurnMeta,
    provider_tag: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let data: serde_json::Value = serde_json::from_str(data_str)?;
    let event_type = data["type"].as_str().unwrap_or("");

    match event_type {
        "content_block_start" => {
            let index = data["index"].as_u64().unwrap_or(0) as usize;
            let block = &data["content_block"];
            let block_type = block["type"].as_str().unwrap_or("");

            while blocks.len() <= index {
                blocks.push(AccumulatedBlock::Thinking(String::new()));
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
                "redacted_thinking" => AccumulatedBlock::Thinking(
                    // Encrypted payload lives in `data`, not `thinking`. Capture
                    // its length so thinking_chars > 0 reflects that the model
                    // spent tokens on hidden reasoning.
                    block["data"].as_str().unwrap_or("").to_string(),
                ),
                other => {
                    meta.unknown_sse_dropped = meta.unknown_sse_dropped.saturating_add(1);
                    crate::log!(
                        "[{}] WARNING: unknown content_block_start type '{}' at index {}; content lost (parser needs update)",
                        provider_tag,
                        other,
                        index,
                    );
                    AccumulatedBlock::Thinking(String::new())
                }
            };
        }
        "content_block_delta" => {
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
                    // Known-quiet: thinking-block signature, no user content.
                    ("signature_delta", _) => {}
                    (other, block) => {
                        let block_kind = match block {
                            AccumulatedBlock::Text(_) => "text",
                            AccumulatedBlock::ToolUse { .. } => "tool_use",
                            AccumulatedBlock::Thinking(_) => "thinking",
                        };
                        meta.unknown_sse_dropped = meta.unknown_sse_dropped.saturating_add(1);
                        crate::log!(
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
            // Anthropic's first SSE event carries the exact input-token cost.
            // Sum uncached + cache write + cache read — the user's "context
            // size" is everything the model processed, not just the uncached
            // remainder.
            let usage = &data["message"]["usage"];
            let input = usage["input_tokens"].as_u64().unwrap_or(0);
            let cache_write = usage["cache_creation_input_tokens"].as_u64().unwrap_or(0);
            let cache_read = usage["cache_read_input_tokens"].as_u64().unwrap_or(0);
            // saturating_add so a corrupt upstream usage block with near-u64::MAX
            // counts in all three fields can't overflow before
            // clamp_provider_token_count clamps it to u32::MAX.
            let total = input.saturating_add(cache_write).saturating_add(cache_read);
            if total > 0 {
                meta.input_tokens =
                    Some(crate::llm::clamp_provider_token_count(total, provider_tag));
            }
            if cache_write > 0 {
                meta.cache_creation_tokens = Some(crate::llm::clamp_provider_token_count(
                    cache_write,
                    provider_tag,
                ));
            }
            if cache_read > 0 {
                meta.cache_read_tokens = Some(crate::llm::clamp_provider_token_count(
                    cache_read,
                    provider_tag,
                ));
            }
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
            let error_type = data["error"]["type"].as_str().unwrap_or("unknown");
            let error_msg = data["error"]["message"]
                .as_str()
                .unwrap_or("Unknown streaming error");
            return Err(format!("Claude streaming error [{}]: {}", error_type, error_msg).into());
        }
        // content_block_stop, message_stop, ping — ignore
        _ => {}
    }

    Ok(())
}

// ===== Claude/Anthropic prompt caching =====

/// Anthropic ephemeral cache marker (5-minute TTL, the default). Writes cost
/// ~1.25× input price; reads cost ~0.1×, so a single cache hit pays back the
/// write premium and everything beyond is pure savings. Render order is
/// `tools` → `system` → `messages` — a marker on the last block of each tier
/// caches everything before it. We place markers on tools[-1], the system
/// block, and the last message's last content block (3 of the 4 allowed).
fn ephemeral_cache_marker() -> serde_json::Value {
    serde_json::json!({"type": "ephemeral"})
}

/// Build a typed-content text block with a `cache_control` marker, taking
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

/// Wrap the system prompt as a one-block content array tagged with
/// `cache_control`. Returning `None` for empty/missing input keeps the request
/// body minimal (`system` is omitted entirely) and avoids sending an empty text
/// block, which Anthropic rejects.
fn system_with_cache_control(system: Option<&str>) -> Option<serde_json::Value> {
    let s = system?;
    if s.is_empty() {
        return None;
    }
    Some(serde_json::Value::Array(vec![
        text_block_with_cache_control(s.to_string()),
    ]))
}

fn apply_cache_control_to_last_tool(tools: &mut [ClaudeTool]) {
    if let Some(last) = tools.last_mut() {
        last.cache_control = Some(ephemeral_cache_marker());
    }
}

/// Mark the final message so the entire prior conversation prefix becomes a
/// cache breakpoint on the next turn. Bare-string content is rewritten into the
/// array form (the only shape that accepts `cache_control`); existing arrays get
/// the marker on their final block. Empty strings are left alone — they have
/// nothing worth caching and round-tripping them through the array form would
/// produce an empty text block, which the API rejects.
fn apply_cache_control_to_last_message(messages: &mut [ClaudeMessage]) {
    let Some(last) = messages.last_mut() else {
        return;
    };
    match &mut last.content {
        serde_json::Value::String(s) if !s.is_empty() => {
            let text = std::mem::take(s);
            last.content = serde_json::Value::Array(vec![text_block_with_cache_control(text)]);
        }
        serde_json::Value::Array(arr) => {
            let Some(last_block) = arr.last_mut().and_then(|b| b.as_object_mut()) else {
                return;
            };
            last_block.insert("cache_control".to_string(), ephemeral_cache_marker());
        }
        _ => {}
    }
}

// ===== Claude/Anthropic request/response types =====

#[derive(Serialize)]
pub(crate) struct ClaudeRequest {
    /// Body API version — `Some("vertex-2023-10-16")` for Vertex; `None` for the
    /// direct API (which sends `anthropic-version` as an HTTP header instead).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub anthropic_version: Option<String>,
    /// Model id in the body — `Some` only on the direct API; Vertex carries the
    /// model in the URL and omits this.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    pub max_tokens: u32,
    pub stream: bool,
    /// Either a bare string or an array of typed content blocks (the latter is
    /// required to attach `cache_control`). `system_with_cache_control` emits
    /// the array form so the system prompt becomes a cache breakpoint.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system: Option<serde_json::Value>,
    pub messages: Vec<ClaudeMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<ClaudeTool>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking: Option<ClaudeThinking>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_config: Option<ClaudeOutputConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub anthropic_beta: Option<Vec<String>>,
}

#[derive(Serialize)]
pub(crate) struct ClaudeThinking {
    #[serde(rename = "type")]
    pub thinking_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub budget_tokens: Option<u32>,
}

#[derive(Serialize)]
pub(crate) struct ClaudeOutputConfig {
    pub effort: String,
}

#[derive(Serialize)]
pub(crate) struct ClaudeMessage {
    pub role: String,
    pub content: serde_json::Value,
}

#[derive(Serialize)]
pub(crate) struct ClaudeTool {
    pub name: String,
    pub description: String,
    pub input_schema: serde_json::Value,
    /// Set on the last tool to make tools+system a cached prefix. The cap is 4
    /// cache_control breakpoints per request; we use 3 (this + system + the last
    /// message), so the budget is comfortably under.
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
    /// Thinking content is internal — kept only so the empty-completion log can
    /// report `thinking_chars` to distinguish "thought then gave up" from "said
    /// nothing without thinking".
    Thinking(String),
}

/// Per-turn metadata captured from Anthropic's streaming SSE events.
/// `input_tokens` is the real prompt size (uncached + cache write + cache read)
/// from `message_start`; the cache breakdown stays available separately so the
/// unified ContextCaptured modal can show cache hit rate. `stop_reason` and
/// `output_tokens` come from `message_delta`.
#[derive(Default)]
pub(crate) struct TurnMeta {
    pub stop_reason: Option<String>,
    pub output_tokens: Option<u32>,
    pub input_tokens: Option<u32>,
    pub cache_read_tokens: Option<u32>,
    pub cache_creation_tokens: Option<u32>,
    /// Count of SSE shapes the parser saw but couldn't classify — unknown
    /// `content_block_start.content_block.type` or unknown
    /// `content_block_delta.delta.type` (excluding the known-quiet
    /// `signature_delta`). Non-zero means model output was silently dropped; the
    /// empty-completion diagnostic surfaces this to distinguish parser misses
    /// from intentional silence.
    pub unknown_sse_dropped: u32,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::provider::{ContentBlock, MessageContent};

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
        // The 1M variant must resolve to the SAME model plus the beta, never to
        // a literal `...[1m]` id in the Vertex URL (which 404s).
        assert_eq!(
            parse_context_suffix("claude-sonnet-5[1m]"),
            ("claude-sonnet-5", true)
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
    fn fable_5_is_adaptive_thinking() {
        assert!(supports_extended_thinking("claude-fable-5"));
        assert!(requires_adaptive_thinking("claude-fable-5"));
    }

    #[test]
    fn opus_5_is_adaptive_thinking() {
        // Opus 5 is adaptive-only (extended `thinking.type:"enabled"` 400s). The
        // real registry ids carry the `@default` alias and the `[1m]` suffix, so
        // both gates must fire for those exact strings — otherwise Opus 5 falls
        // into the no-thinking 8192-cap path or the deprecated budget_tokens path.
        for id in [
            "claude-opus-5",
            "claude-opus-5@default",
            "claude-opus-5@default[1m]",
        ] {
            assert!(supports_extended_thinking(id), "extended: {id}");
            assert!(requires_adaptive_thinking(id), "adaptive: {id}");
        }
    }

    #[test]
    fn sonnet_5_is_adaptive_thinking_and_sonnet_4_6_is_not() {
        // Sonnet 5 removed `budget_tokens` and the sampling params, so it needs
        // BOTH gates. Neither fires by accident: `supports_extended_thinking`
        // matches Sonnet 4.x on `claude-sonnet-4`, which does not cover
        // `claude-sonnet-5`, and the adaptive list was Opus-only before.
        for id in ["claude-sonnet-5", "claude-sonnet-5[1m]"] {
            assert!(supports_extended_thinking(id), "extended: {id}");
            assert!(requires_adaptive_thinking(id), "adaptive: {id}");
        }
        // Sonnet 4.6 still takes the `budget_tokens` path, so the new arm must
        // not drag the older generation onto adaptive with it.
        assert!(supports_extended_thinking("claude-sonnet-4-6"));
        assert!(!requires_adaptive_thinking("claude-sonnet-4-6"));
    }

    #[test]
    fn sonnet_5_thinking_config_uses_adaptive_not_budget_tokens() {
        let (thinking, output_config, max_tokens) =
            thinking_config("claude-sonnet-5", Some("xhigh"));
        let thinking = thinking.expect("adaptive thinking present");
        assert_eq!(thinking.thinking_type, "adaptive");
        assert!(thinking.budget_tokens.is_none());
        assert_eq!(output_config.expect("effort present").effort, "xhigh");
        assert_eq!(max_tokens, 32768);
    }

    #[test]
    fn opus_5_thinking_config_uses_adaptive_not_budget_tokens() {
        // Guards the correctness invariant end-to-end: the request Opus 5 gets
        // must be `thinking.type == "adaptive"` with an `output_config.effort`
        // and no `budget_tokens` (Vertex rejects `enabled`/`budget_tokens`).
        let (thinking, output_config, max_tokens) =
            thinking_config("claude-opus-5@default", Some("high"));
        let thinking = thinking.expect("adaptive thinking present");
        assert_eq!(thinking.thinking_type, "adaptive");
        assert!(thinking.budget_tokens.is_none());
        assert_eq!(output_config.expect("effort present").effort, "high");
        assert_eq!(max_tokens, 32768);
    }

    #[test]
    fn message_content_to_claude_value_filters_empty_text_blocks() {
        // When pasting images without text, empty text blocks must be filtered
        // or the Claude API rejects with "text content blocks must be non-empty"
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
        let value = message_content_to_claude_value(&content);
        let arr = value.as_array().unwrap();
        // Empty text block should be filtered out, leaving only the image
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
        let value = message_content_to_claude_value(&content);
        let arr = value.as_array().unwrap();
        assert_eq!(arr.len(), 2);
        assert_eq!(arr[0]["type"], "text");
        assert_eq!(arr[0]["text"], "describe this");
        assert_eq!(arr[1]["type"], "image");
    }

    #[test]
    fn process_sse_captures_input_tokens_from_message_start() {
        // Anthropic streams `message_start` early in every response with the
        // exact prompt-token cost. Capturing it lets the UI replace the chars/4
        // estimate (which over-counts base64 image bytes by orders of magnitude)
        // with the real number.
        let mut blocks = Vec::new();
        let mut meta = TurnMeta::default();
        let event = r#"{"type":"message_start","message":{"id":"msg_x","type":"message","role":"assistant","content":[],"model":"claude-opus-4-7","stop_reason":null,"stop_sequence":null,"usage":{"input_tokens":4321,"cache_creation_input_tokens":1000,"cache_read_input_tokens":500,"output_tokens":1}}}"#;

        process_sse_data(event, &mut blocks, &mut meta, "Test").unwrap();

        // Real prompt size = uncached input + cache writes + cache reads
        // (everything the model actually processed). 4321 + 1000 + 500 = 5821.
        assert_eq!(meta.input_tokens, Some(5821));
        // Cache breakdown survives separately so the modal can show hit rate.
        assert_eq!(meta.cache_creation_tokens, Some(1000));
        assert_eq!(meta.cache_read_tokens, Some(500));
    }

    #[test]
    fn process_sse_token_sum_saturates_instead_of_overflowing() {
        // A corrupt upstream usage block with near-u64::MAX counts in all three
        // input fields must not overflow the sum (a debug build would panic, a
        // release build would wrap). The sum saturates and then clamps to
        // u32::MAX.
        let mut blocks = Vec::new();
        let mut meta = TurnMeta::default();
        let huge = u64::MAX;
        let event = format!(
            r#"{{"type":"message_start","message":{{"id":"msg_x","type":"message","role":"assistant","content":[],"model":"claude-opus-4-7","stop_reason":null,"stop_sequence":null,"usage":{{"input_tokens":{huge},"cache_creation_input_tokens":{huge},"cache_read_input_tokens":{huge},"output_tokens":1}}}}}}"#
        );

        // The key assertion is that this does not panic on the addition.
        process_sse_data(&event, &mut blocks, &mut meta, "Test").unwrap();

        assert_eq!(meta.input_tokens, Some(u32::MAX));
        assert_eq!(meta.cache_creation_tokens, Some(u32::MAX));
        assert_eq!(meta.cache_read_tokens, Some(u32::MAX));
    }

    #[test]
    fn system_with_cache_control_none_returns_none() {
        assert!(system_with_cache_control(None).is_none());
    }

    #[test]
    fn system_with_cache_control_empty_string_returns_none() {
        assert!(system_with_cache_control(Some("")).is_none());
    }

    #[test]
    fn system_with_cache_control_wraps_string_in_block_with_marker() {
        let value = system_with_cache_control(Some("you are a helpful assistant")).unwrap();
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
        }];
        apply_cache_control_to_last_message(&mut messages);
        let arr = messages[0].content.as_array().unwrap();
        assert!(arr[0].get("cache_control").is_none());
        assert_eq!(arr[1]["cache_control"]["type"], "ephemeral");
    }

    #[test]
    fn apply_cache_control_to_last_message_only_touches_final_message() {
        let mut messages = vec![
            ClaudeMessage {
                role: "user".into(),
                content: serde_json::Value::String("first turn".into()),
            },
            ClaudeMessage {
                role: "assistant".into(),
                content: serde_json::Value::String("second turn".into()),
            },
        ];
        apply_cache_control_to_last_message(&mut messages);
        // First message untouched (still a bare string)
        assert!(messages[0].content.is_string());
        // Last message converted to a block array with cache_control
        assert!(messages[1].content.is_array());
    }

    #[test]
    fn apply_cache_control_to_last_message_empty_is_noop() {
        let mut messages: Vec<ClaudeMessage> = Vec::new();
        apply_cache_control_to_last_message(&mut messages);
        assert!(messages.is_empty());
    }

    #[test]
    fn apply_cache_control_to_last_message_skips_empty_string() {
        // An empty string would round-trip into an empty text block, which
        // Anthropic rejects. Cache_control on nothing is meaningless anyway.
        let mut messages = vec![ClaudeMessage {
            role: "user".into(),
            content: serde_json::Value::String(String::new()),
        }];
        apply_cache_control_to_last_message(&mut messages);
        // Untouched
        assert!(messages[0].content.is_string());
        assert_eq!(messages[0].content.as_str(), Some(""));
    }

    #[test]
    fn vertex_request_carries_anthropic_version_no_model_field() {
        // Vertex framing: model lives in the URL (body `model` omitted), API
        // version in the body, 1M beta in the body `anthropic_beta` array.
        let (req, is_1m) = build_claude_request(
            vec![Message {
                role: "user".into(),
                content: MessageContent::Text("hi".into()),
            }],
            vec![],
            "claude-opus-4-8@default[1m]",
            Some("system prompt body"),
            Some("high"),
            WireTarget::Vertex,
            "Vertex",
        );
        assert!(is_1m);
        let json = serde_json::to_value(&req).unwrap();
        assert_eq!(json["anthropic_version"], "vertex-2023-10-16");
        assert!(json.get("model").is_none(), "Vertex omits body model field");
        assert_eq!(json["anthropic_beta"][0], ANTHROPIC_BETA_1M_CONTEXT);
    }

    #[test]
    fn direct_request_carries_model_no_anthropic_version_or_beta() {
        // Direct framing: model in the body (base, [1m] stripped), no body
        // `anthropic_version` and no body `anthropic_beta` — both are HTTP
        // headers the direct provider adds.
        let (req, is_1m) = build_claude_request(
            vec![Message {
                role: "user".into(),
                content: MessageContent::Text("hi".into()),
            }],
            vec![],
            "claude-fable-5[1m]",
            None,
            Some("high"),
            WireTarget::Direct,
            "Anthropic",
        );
        assert!(is_1m);
        let json = serde_json::to_value(&req).unwrap();
        assert_eq!(json["model"], "claude-fable-5");
        assert!(json.get("anthropic_version").is_none());
        assert!(json.get("anthropic_beta").is_none());
        // Fable 5 → adaptive thinking with output_config.effort
        assert_eq!(json["thinking"]["type"], "adaptive");
        assert_eq!(json["output_config"]["effort"], "high");
    }

    #[test]
    fn cache_control_serializes_into_wire_format() {
        // End-to-end: build a request the way build_claude_request does,
        // serialize it, and check cache_control lands on tools[-1], the system
        // block, and messages[-1]'s last content block.
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
            },
            ClaudeMessage {
                role: "assistant".into(),
                content: serde_json::Value::String("response".into()),
            },
            ClaudeMessage {
                role: "user".into(),
                content: serde_json::Value::String("follow-up".into()),
            },
        ];
        apply_cache_control_to_last_message(&mut messages);

        let req = ClaudeRequest {
            anthropic_version: Some("vertex-2023-10-16".into()),
            model: None,
            max_tokens: 1024,
            stream: true,
            system: system_with_cache_control(Some("system prompt body")),
            messages,
            tools: Some(tools),
            thinking: None,
            output_config: None,
            anthropic_beta: None,
        };

        let json = serde_json::to_value(&req).unwrap();

        // Tools: only the last one carries cache_control
        let tools_arr = json["tools"].as_array().unwrap();
        assert!(tools_arr[0].get("cache_control").is_none());
        assert_eq!(tools_arr[1]["cache_control"]["type"], "ephemeral");

        // System: array form with cache_control on its single block
        let system_arr = json["system"].as_array().unwrap();
        assert_eq!(system_arr[0]["cache_control"]["type"], "ephemeral");

        // Messages: only the final message's last block carries cache_control
        let msgs = json["messages"].as_array().unwrap();
        assert!(msgs[0]["content"].is_string());
        assert!(msgs[1]["content"].is_string());
        let last_blocks = msgs[2]["content"].as_array().unwrap();
        assert_eq!(
            last_blocks.last().unwrap()["cache_control"]["type"],
            "ephemeral"
        );
    }

    #[test]
    fn process_sse_captures_redacted_thinking_data_field() {
        // redacted_thinking blocks carry their (encrypted) payload in `data`,
        // not `thinking`. Reading `thinking` produced 0 chars even though the
        // model spent output tokens on the block — the engine then surfaced "no
        // response" with a misleading hint. Capturing the data length keeps
        // thinking_chars non-zero so the empty-completion diagnostic can
        // distinguish encrypted reasoning from true silence.
        let mut blocks = Vec::new();
        let mut meta = TurnMeta::default();
        let event = r#"{"type":"content_block_start","index":0,"content_block":{"type":"redacted_thinking","data":"AAAAAA=="}}"#;

        process_sse_data(event, &mut blocks, &mut meta, "Test").unwrap();

        match &blocks[0] {
            AccumulatedBlock::Thinking(s) => assert_eq!(s, "AAAAAA=="),
            _ => panic!("redacted_thinking must bucket as Thinking"),
        }
    }

    #[test]
    fn process_sse_increments_unknown_for_new_block_type() {
        // When Anthropic emits a block type the parser doesn't recognize, every
        // delta for that block falls through silently and the model's output
        // tokens disappear from the LlmResponse. Tracking the count lets the
        // empty-completion diagnostic say "engine dropped unknown SSE shapes"
        // instead of "model decided no action was needed".
        let mut blocks = Vec::new();
        let mut meta = TurnMeta::default();
        let event = r#"{"type":"content_block_start","index":0,"content_block":{"type":"some_new_block_type"}}"#;

        process_sse_data(event, &mut blocks, &mut meta, "Test").unwrap();

        assert_eq!(meta.unknown_sse_dropped, 1);
    }

    #[test]
    fn process_sse_increments_unknown_for_new_delta_type() {
        let mut blocks = Vec::new();
        let mut meta = TurnMeta::default();
        // Start a text block so the index is populated, then send a delta type
        // the parser doesn't recognize.
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
        // signature_delta arrives on every thinking block to sign it. It carries
        // no user-visible content and the parser intentionally ignores it — must
        // NOT count as a dropped unknown shape.
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

    /// The transport half of the HTML-entity bug hunt
    /// (`docs/plans/2026-08-09-tool-arg-html-entity-repair.md`): tool arguments
    /// that reach us with `& < > " '` in them must come out of the SSE
    /// accumulator byte-identical.
    ///
    /// This pins the bisection result rather than a fix. The escaping turned
    /// out to be the model's own, and `engine::tool_arg_entity_repair` corrects
    /// it downstream. If the transport ever DID start escaping, that repair
    /// would quietly mask it on the allow-listed label arguments while
    /// corrupting everything else, so the accumulator gets its own guard.
    ///
    /// The deltas deliberately split mid-value and mid-escape-sequence, which
    /// is how a real stream arrives, so a per-chunk transform could not hide
    /// behind chunk boundaries.
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
        // `{"name": "Machine & Tooling <Health> \"q\" 'a'", "action": "create"}`
        // arriving in five chunks, one of them splitting the `\"` escape.
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
}
