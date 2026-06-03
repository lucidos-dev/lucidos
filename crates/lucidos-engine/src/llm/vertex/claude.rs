//! Claude/Anthropic request-build + SSE stream-parse for `VertexProvider`.
//! Split out of the monolithic `vertex.rs`; the `VertexProvider` struct and
//! shared auth/config live in the parent `vertex` module.


use crate::llm::provider::{
    ContentBlock, LlmResponse, Message, MessageContent, TokenCallback, ToolCall, ToolDefinition,
};
use futures::StreamExt;
use serde::Serialize;
use std::time::Duration;
use super::{thinking_budget_for_effort, VertexProvider};

/// Beta header enabling 1M token context window for Claude models on Vertex AI.
const ANTHROPIC_BETA_1M_CONTEXT: &str = "context-1m-2025-08-07";

/// Per-chunk timeout for Claude SSE streams (seconds).
const CLAUDE_STREAM_CHUNK_TIMEOUT_SECS: u64 = 300;

impl VertexProvider {
    /// Convert MessageContent to the serde_json::Value format Claude expects.
    /// Text → JSON string, Blocks → JSON array of content block objects.
    fn message_content_to_claude_value(content: &MessageContent) -> serde_json::Value {
        match content {
            MessageContent::Text(s) => serde_json::Value::String(s.clone()),
            MessageContent::Blocks(blocks) => {
                let json_blocks: Vec<serde_json::Value> = blocks
                    .iter()
                    .filter(
                        |block| !matches!(block, ContentBlock::Text { text } if text.is_empty()),
                    )
                    .map(|block| match block {
                        ContentBlock::Text { text } => serde_json::json!({
                            "type": "text",
                            "text": text,
                        }),
                        ContentBlock::ToolUse {
                            id,
                            name,
                            input,
                            // Claude API doesn't accept a thought_signature
                            // field on tool_use blocks; the signature is
                            // Gemini-only and lives on the engine-side
                            // ContentBlock so it can flow back through the
                            // Vertex Gemini request path.
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

    pub(super) async fn chat_claude(
        &self,
        mut messages: Vec<Message>,
        tools: Vec<ToolDefinition>,
        model: &str,
        system_prompt: Option<&str>,
        on_token: Option<TokenCallback>,
        reasoning_effort: Option<&str>,
    ) -> Result<LlmResponse, Box<dyn std::error::Error + Send + Sync>> {
        // Strip [1m] suffix — same model ID, just needs beta header
        let (base_model, is_1m) = Self::parse_context_suffix(model);

        // Pre-flight: repair orphan `tool_use` blocks before Anthropic 400s.
        // Last line of defense — ANY caller that builds `messages` goes
        // through this gate, even ones that bypass the agentic loop. Lives in
        // `llm::validate` so this layer doesn't reach `up` into `engine::*`
        // (enforced by `llm::validate::tests::llm_does_not_depend_on_engine`).
        let stubs = crate::llm::validate::validate_tool_use_pairing(&mut messages);
        if stubs > 0 {
            crate::log!(
                "[Vertex] WARNING: pre-flight injected {} stub tool_result block(s) before Claude API call (model={})",
                stubs,
                model
            );
        }

        // Convert to Anthropic Messages API format
        let mut claude_messages: Vec<ClaudeMessage> = messages
            .iter()
            .map(|m| ClaudeMessage {
                role: m.role.clone(),
                content: Self::message_content_to_claude_value(&m.content),
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

        let (thinking, output_config, max_tokens) = if Self::supports_extended_thinking(base_model)
        {
            let effort = reasoning_effort.unwrap_or("high");
            if effort == "none" {
                (None, None, 8192)
            } else if Self::requires_adaptive_thinking(base_model) {
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
                let budget = thinking_budget_for_effort(effort);
                (
                    Some(ClaudeThinking {
                        thinking_type: "enabled".to_string(),
                        budget_tokens: Some(budget),
                    }),
                    None,
                    budget + 16384,
                )
            }
        } else {
            (None, None, 8192)
        };

        let anthropic_beta = if is_1m {
            Some(vec![ANTHROPIC_BETA_1M_CONTEXT.to_string()])
        } else {
            None
        };

        let request = ClaudeRequest {
            anthropic_version: "vertex-2023-10-16".to_string(),
            max_tokens,
            stream: true,
            system: system_with_cache_control(system_prompt),
            messages: claude_messages,
            tools: claude_tools,
            thinking,
            output_config,
            anthropic_beta,
        };

        let mut access_token = self.get_access_token().await?;
        let url = self.endpoint_for_model(base_model);

        // Retry loop for connection errors, retryable HTTP status codes, and
        // mid-stream overload errors. Content is accumulated internally by
        // parse_claude_stream, so retrying the full request is safe.
        let mut attempt = 0u32;
        let mut retried_auth = false;
        loop {
            attempt += 1;

            let resp = match self
                .streaming_client
                .post(&url)
                .header("Authorization", format!("Bearer {}", access_token))
                .header("Content-Type", "application/json")
                .json(&request)
                .send()
                .await
            {
                Ok(r) => r,
                Err(e) => {
                    if attempt <= crate::llm::MAX_RETRIES {
                        let delay = crate::llm::retry_delay(attempt, 1);
                        crate::llm::log_retry(model, &format!("Network error: {:?}", e), attempt, delay);
                        tokio::time::sleep(delay).await;
                        continue;
                    }
                    return Err(crate::llm::with_retry_context(e, attempt).into());
                }
            };

            let status = resp.status();
            if !status.is_success() {
                let error_body = resp.text().await.unwrap_or_default();

                if status.as_u16() == 401 && !retried_auth {
                    if let Some(new_token) = self.handle_auth_refresh(model, &mut retried_auth).await {
                        access_token = new_token;
                        continue;
                    }
                    return Err(format!("Claude API error ({}): {}", status, error_body).into());
                }

                if crate::llm::should_retry_http(status.as_u16(), &error_body, attempt) {
                    let delay = crate::llm::retry_delay(attempt, 1);
                    crate::llm::log_retry(model, &format!("HTTP {}", status), attempt, delay);
                    tokio::time::sleep(delay).await;
                    continue;
                }

                return Err(crate::llm::with_retry_context(
                    format!("Claude API error ({}): {}", status, error_body),
                    attempt,
                )
                .into());
            }

            // Parse SSE stream — retry on overload errors
            match self.parse_claude_stream(resp, &on_token).await {
                Ok(response) => return Ok(response),
                Err(e) => {
                    let err_str = e.to_string();

                    if crate::llm::is_retryable_error(&err_str) && attempt <= crate::llm::MAX_RETRIES {
                        let delay = crate::llm::retry_delay(attempt, 2); // longer for stream errors
                        crate::llm::log_retry(
                            model,
                            &format!("Stream error: {}", err_str),
                            attempt,
                            delay,
                        );
                        tokio::time::sleep(delay).await;
                        continue;
                    }

                    return Err(crate::llm::with_retry_context(e, attempt).into());
                }
            }
        }
    }

    /// Parse an SSE stream from Claude's streaming API into an LlmResponse.
    async fn parse_claude_stream(
        &self,
        response: reqwest::Response,
        on_token: &Option<TokenCallback>,
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
                    Self::process_sse_data(data_str, &mut blocks, &mut turn_meta)?;
                    if let Some(cb) = on_token {
                        let new_text_len: usize = blocks
                            .iter()
                            .map(|b| match b {
                                AccumulatedBlock::Text(t) => t.len(),
                                _ => 0,
                            })
                            .sum();
                        if new_text_len > prev_text_len {
                            // Floor defensively — `raw_start` is byte-derived
                            // and could land mid-codepoint after future
                            // accumulator changes.
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
                    "text" => {
                        AccumulatedBlock::Text(block["text"].as_str().unwrap_or("").to_string())
                    }
                    "tool_use" => AccumulatedBlock::ToolUse {
                        id: block["id"].as_str().unwrap_or("").to_string(),
                        name: block["name"].as_str().unwrap_or("").to_string(),
                        json_parts: String::new(),
                    },
                    "thinking" => AccumulatedBlock::Thinking(
                        block["thinking"].as_str().unwrap_or("").to_string(),
                    ),
                    "redacted_thinking" => AccumulatedBlock::Thinking(
                        // Encrypted payload lives in `data`, not `thinking`.
                        // Capture its length so thinking_chars > 0 reflects
                        // that the model spent tokens on hidden reasoning.
                        block["data"].as_str().unwrap_or("").to_string(),
                    ),
                    other => {
                        meta.unknown_sse_dropped =
                            meta.unknown_sse_dropped.saturating_add(1);
                        crate::log!(
                            "[Vertex] WARNING: unknown content_block_start type '{}' at index {}; content lost (parser needs update)",
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
                            meta.unknown_sse_dropped =
                                meta.unknown_sse_dropped.saturating_add(1);
                            crate::log!(
                                "[Vertex] WARNING: unknown content_block_delta type '{}' for {} block at index {}; content lost (parser needs update)",
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
                let total = input + cache_write + cache_read;
                if total > 0 {
                    meta.input_tokens = Some(crate::llm::clamp_provider_token_count(total, "Vertex"));
                }
                if cache_write > 0 {
                    meta.cache_creation_tokens = Some(crate::llm::clamp_provider_token_count(cache_write, "Vertex"));
                }
                if cache_read > 0 {
                    meta.cache_read_tokens = Some(crate::llm::clamp_provider_token_count(cache_read, "Vertex"));
                }
            }
            "message_delta" => {
                if let Some(sr) = data["delta"]["stop_reason"].as_str() {
                    meta.stop_reason = Some(sr.to_string());
                }
                if let Some(ot) = data["usage"]["output_tokens"].as_u64() {
                    meta.output_tokens = Some(crate::llm::clamp_provider_token_count(ot, "Vertex"));
                }
            }
            "error" => {
                let error_type = data["error"]["type"].as_str().unwrap_or("unknown");
                let error_msg = data["error"]["message"]
                    .as_str()
                    .unwrap_or("Unknown streaming error");
                return Err(
                    format!("Claude streaming error [{}]: {}", error_type, error_msg).into(),
                );
            }
            // content_block_stop, message_stop, ping — ignore
            _ => {}
        }

        Ok(())
    }
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
/// `cache_control`. Returning `None` for empty/missing input keeps the
/// request body minimal (`system` is omitted entirely) and avoids sending
/// an empty text block, which Anthropic rejects.
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
/// cache breakpoint on the next turn. Bare-string content is rewritten into
/// the array form (the only shape that accepts `cache_control`); existing
/// arrays get the marker on their final block. Empty strings are left alone
/// — they have nothing worth caching and round-tripping them through the
/// array form would produce an empty text block, which the API rejects.
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
struct ClaudeRequest {
    anthropic_version: String,
    max_tokens: u32,
    stream: bool,
    /// Either a bare string or an array of typed content blocks (the latter
    /// is required to attach `cache_control`). `system_with_cache_control`
    /// emits the array form so the system prompt becomes a cache breakpoint.
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<serde_json::Value>,
    messages: Vec<ClaudeMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<ClaudeTool>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    thinking: Option<ClaudeThinking>,
    #[serde(skip_serializing_if = "Option::is_none")]
    output_config: Option<ClaudeOutputConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    anthropic_beta: Option<Vec<String>>,
}

#[derive(Serialize)]
struct ClaudeThinking {
    #[serde(rename = "type")]
    thinking_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    budget_tokens: Option<u32>,
}

#[derive(Serialize)]
struct ClaudeOutputConfig {
    effort: String,
}

#[derive(Serialize)]
struct ClaudeMessage {
    role: String,
    content: serde_json::Value,
}

#[derive(Serialize)]
struct ClaudeTool {
    name: String,
    description: String,
    input_schema: serde_json::Value,
    /// Set on the last tool to make tools+system a cached prefix. The cap is
    /// 4 cache_control breakpoints per request; we use 3 (this + system + the
    /// last message), so the budget is comfortably under.
    #[serde(skip_serializing_if = "Option::is_none")]
    cache_control: Option<serde_json::Value>,
}

enum AccumulatedBlock {
    Text(String),
    ToolUse {
        id: String,
        name: String,
        json_parts: String,
    },
    /// Thinking content is internal — kept only so the empty-completion log
    /// can report `thinking_chars` to distinguish "thought then gave up"
    /// from "said nothing without thinking".
    Thinking(String),
}

/// Per-turn metadata captured from Anthropic's streaming SSE events.
/// `input_tokens` is the real prompt size (uncached + cache write + cache read)
/// from `message_start`; the cache breakdown stays available separately so
/// the unified ContextCaptured modal can show cache hit rate. `stop_reason`
/// and `output_tokens` come from `message_delta`.
#[derive(Default)]
struct TurnMeta {
    stop_reason: Option<String>,
    output_tokens: Option<u32>,
    input_tokens: Option<u32>,
    cache_read_tokens: Option<u32>,
    cache_creation_tokens: Option<u32>,
    /// Count of SSE shapes the parser saw but couldn't classify — unknown
    /// `content_block_start.content_block.type` or unknown
    /// `content_block_delta.delta.type` (excluding the known-quiet
    /// `signature_delta`). Non-zero means model output was silently
    /// dropped; the empty-completion diagnostic surfaces this to
    /// distinguish parser misses from intentional silence.
    unknown_sse_dropped: u32,
}

#[cfg(test)]
#[path = "claude_tests.rs"]
mod tests;
