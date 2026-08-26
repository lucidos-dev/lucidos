//! OpenAI Chat Completions API path (non-codex models) for `OpenAiProvider`.
//! The struct, shared stream types, and dispatch live in the parent `openai`
//! module.

use crate::llm::provider::{
    ContentBlock, LlmResponse, Message, MessageContent, TokenCallback, ToolDefinition,
};
use futures::StreamExt;
use std::time::Duration;

use super::{
    AccumulatedToolCall, OpenAiProvider, StreamMeta, CHUNK_TIMEOUT_SECS,
    DEFAULT_MAX_COMPLETION_TOKENS,
};

impl OpenAiProvider {
    /// Convert internal messages to OpenAI Chat Completions format.
    fn convert_messages_chat(messages: &[Message]) -> Vec<serde_json::Value> {
        let mut openai_messages: Vec<serde_json::Value> = Vec::new();

        for msg in messages {
            match &msg.content {
                MessageContent::Text(text) => {
                    openai_messages.push(serde_json::json!({
                        "role": msg.role,
                        "content": text,
                    }));
                }
                MessageContent::Blocks(blocks) => {
                    let mut text_parts: Vec<String> = Vec::new();
                    let mut image_parts: Vec<serde_json::Value> = Vec::new();
                    let mut tool_calls: Vec<serde_json::Value> = Vec::new();
                    let mut tool_results: Vec<(String, String)> = Vec::new();

                    for block in blocks {
                        match block {
                            // A tail block is ordinary text here. Only the
                            // engine and the Anthropic cache anchor care who
                            // wrote it.
                            ContentBlock::Text { text } | ContentBlock::EngineTail { text } => {
                                text_parts.push(text.clone());
                            }
                            ContentBlock::Image {
                                media_type, data, ..
                            } => {
                                image_parts.push(serde_json::json!({
                                    "type": "image_url",
                                    "image_url": {
                                        "url": format!("data:{};base64,{}", media_type, data),
                                    }
                                }));
                            }
                            ContentBlock::ToolUse {
                                id, name, input, ..
                            } => {
                                tool_calls.push(serde_json::json!({
                                    "id": id,
                                    "type": "function",
                                    "function": {
                                        "name": name,
                                        "arguments": serde_json::to_string(input)
                                            .unwrap_or_else(|e| {
                                                crate::log!("[OpenAI] Failed to serialize tool arguments: {}", e);
                                                "{}".to_string()
                                            }),
                                    }
                                }));
                            }
                            ContentBlock::ToolResult {
                                tool_use_id,
                                content,
                            } => {
                                tool_results.push((tool_use_id.clone(), content.clone()));
                            }
                        }
                    }

                    let has_tool_calls = !tool_calls.is_empty();

                    // 1. Assistant `tool_calls` message first — it owns any text
                    //    as its `content` field.
                    if has_tool_calls {
                        let mut assistant_msg = serde_json::json!({
                            "role": "assistant",
                            "tool_calls": tool_calls,
                        });
                        if !text_parts.is_empty() {
                            assistant_msg["content"] =
                                serde_json::Value::String(text_parts.join("\n"));
                        }
                        openai_messages.push(assistant_msg);
                    }

                    // 2. Tool-result `tool` messages MUST immediately follow the
                    //    assistant `tool_calls` message — before any user
                    //    text/image content — or strict providers (Moonshot/Kimi,
                    //    OpenAI) reject with HTTP 400 "an assistant message with
                    //    'tool_calls' must be followed by tool messages responding
                    //    to each 'tool_call_id'". The agentic loop packs the
                    //    ToolResult block(s) and a trailing instruction Text into
                    //    ONE user `Message::Blocks`, so emitting the text before
                    //    the tool messages would wedge a `user` message between the
                    //    assistant `tool_calls` and its responses (the observed
                    //    kimi-k3 failure — thread 85239abe).
                    for (tool_call_id, content) in tool_results {
                        openai_messages.push(serde_json::json!({
                            "role": "tool",
                            "tool_call_id": tool_call_id,
                            "content": content,
                        }));
                    }

                    // 3. Remaining user text/image content, AFTER the tool
                    //    messages. Skipped when this block was an assistant
                    //    `tool_calls` message (step 1 already consumed its text).
                    if !has_tool_calls {
                        if !image_parts.is_empty() {
                            // When images are present, use array-of-parts content format
                            let mut content_parts: Vec<serde_json::Value> = Vec::new();
                            if !text_parts.is_empty() {
                                content_parts.push(serde_json::json!({
                                    "type": "text",
                                    "text": text_parts.join("\n"),
                                }));
                            }
                            content_parts.extend(image_parts);
                            openai_messages.push(serde_json::json!({
                                "role": msg.role,
                                "content": content_parts,
                            }));
                        } else if !text_parts.is_empty() {
                            openai_messages.push(serde_json::json!({
                                "role": msg.role,
                                "content": text_parts.join("\n"),
                            }));
                        }
                    }
                }
            }
        }

        openai_messages
    }

    /// Convert internal tool definitions to Chat Completions function tool format.
    fn convert_tools_chat(tools: &[ToolDefinition]) -> Option<Vec<serde_json::Value>> {
        if tools.is_empty() {
            return None;
        }
        Some(
            tools
                .iter()
                .map(|t| {
                    serde_json::json!({
                        "type": "function",
                        "function": {
                            "name": t.name,
                            "description": t.description,
                            "parameters": t.parameters,
                        }
                    })
                })
                .collect(),
        )
    }

    fn build_chat_body(
        &self,
        model: &str,
        messages: &[Message],
        tools: &[ToolDefinition],
        system_prompt: Option<&str>,
        reasoning_effort: Option<&str>,
    ) -> serde_json::Value {
        let mut openai_messages = Vec::new();

        if let Some(system) = system_prompt {
            openai_messages.push(serde_json::json!({
                "role": "system",
                "content": system,
            }));
        }

        openai_messages.extend(Self::convert_messages_chat(messages));

        let mut body = serde_json::json!({
            "model": model,
            "stream": true,
            // Ask OpenAI to append a final chunk carrying token usage so the
            // provider parser can populate `LlmResponse.{input,output}_tokens`
            // — without this, streaming Chat Completions silently drops
            // usage, breaking cross-provider cost analytics.
            "stream_options": { "include_usage": true },
            "max_completion_tokens": DEFAULT_MAX_COMPLETION_TOKENS,
            "messages": openai_messages,
        });

        if let Some(tool_defs) = Self::convert_tools_chat(tools) {
            body["tools"] = serde_json::Value::Array(tool_defs);
        }

        // Verbatim, deliberately. Which tiers this model supports is decided
        // once, in `llm::reasoning`, and enforced by `RoutingProvider`'s clamp.
        // This builder also serves OpenRouter and local servers, so it cannot
        // tell whose vocabulary applies from the model id alone: rewriting
        // `max` into `xhigh` here because the id was not `gpt-5.6` is what 400'd
        // a local turn on 2026-08-12 (see `llm/reasoning.rs`).
        if let Some(effort) = reasoning_effort {
            body["reasoning_effort"] = serde_json::Value::String(effort.to_string());
        }

        body
    }

    /// Parse an SSE stream from the Chat Completions API.
    async fn parse_chat_stream(
        &self,
        response: reqwest::Response,
        on_token: &Option<TokenCallback>,
    ) -> Result<LlmResponse, Box<dyn std::error::Error + Send + Sync>> {
        let mut stream = response.bytes_stream();
        let mut buffer = String::new();
        // Bytes of a character the transport split across two chunks.
        let mut carry: Vec<u8> = Vec::new();
        let mut content = String::new();
        let mut tool_call_map: Vec<AccumulatedToolCall> = Vec::new();
        let mut meta = StreamMeta::default();
        let chunk_timeout = Duration::from_secs(CHUNK_TIMEOUT_SECS);

        'outer: loop {
            let chunk = match tokio::time::timeout(chunk_timeout, stream.next()).await {
                Ok(Some(Ok(bytes))) => bytes,
                Ok(Some(Err(e))) => return Err(format!("Stream read error: {}", e).into()),
                Ok(None) => break,
                Err(_) => {
                    return Err(format!(
                        "OpenAI stream timed out (no data for {}s)",
                        CHUNK_TIMEOUT_SECS
                    )
                    .into())
                }
            };

            crate::llm::push_utf8_chunk(&mut carry, &chunk, &mut buffer);

            while let Some(newline_pos) = buffer.find('\n') {
                let line = buffer[..newline_pos].trim_end_matches('\r').to_string();
                buffer = buffer[newline_pos + 1..].to_string();

                if line.is_empty() {
                    continue;
                }

                if let Some(data_str) = line.strip_prefix("data: ") {
                    if data_str.trim() == "[DONE]" {
                        break 'outer;
                    }

                    let prev_len = content.len();
                    Self::process_chat_chunk(
                        data_str,
                        &mut content,
                        &mut tool_call_map,
                        &mut meta,
                    )?;
                    if content.len() > prev_len {
                        if let Some(cb) = on_token {
                            // Floor defensively — `prev_len` is a byte length
                            // captured before the chunk appended, so a future
                            // accumulator change could leave it mid-codepoint.
                            cb(&content[content.floor_char_boundary(prev_len)..]);
                        }
                    }
                }
            }
        }

        Self::build_llm_response(content, tool_call_map, meta)
    }

    /// Process a single Chat Completions SSE chunk. The final usage chunk
    /// (requested via `stream_options.include_usage`) arrives with
    /// `choices: []` and a top-level `usage` object — we extract token
    /// counts from there. `finish_reason` rides the LAST per-choice chunk
    /// alongside (or instead of) the delta payload.
    fn process_chat_chunk(
        data_str: &str,
        content: &mut String,
        tool_call_map: &mut Vec<AccumulatedToolCall>,
        meta: &mut StreamMeta,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let data: serde_json::Value = serde_json::from_str(data_str)?;

        if let Some(error) = data.get("error") {
            return Err(format_stream_error(error).into());
        }

        // Top-level usage object (final include_usage chunk has choices: []).
        if let Some(usage) = data.get("usage") {
            meta.absorb_chat_usage(usage);
        }

        let choices = match data.get("choices").and_then(|c| c.as_array()) {
            Some(c) => c,
            None => return Ok(()),
        };

        let choice = match choices.first() {
            Some(c) => c,
            None => return Ok(()),
        };

        // finish_reason rides on the final per-choice chunk. May coexist
        // with an empty `delta` or a delta carrying only the role field.
        if let Some(reason) = choice.get("finish_reason").and_then(|r| r.as_str()) {
            meta.stop_reason = Some(reason.to_string());
        }

        let delta = match choice.get("delta") {
            Some(d) => d,
            None => return Ok(()),
        };

        if let Some(text) = delta.get("content").and_then(|c| c.as_str()) {
            content.push_str(text);
        }

        if let Some(tc_array) = delta.get("tool_calls").and_then(|t| t.as_array()) {
            for tc in tc_array {
                let index = tc.get("index").and_then(|i| i.as_u64()).unwrap_or(0) as usize;

                while tool_call_map.len() <= index {
                    tool_call_map.push(AccumulatedToolCall {
                        id: String::new(),
                        name: String::new(),
                        arguments_json: String::new(),
                    });
                }

                let accumulated = &mut tool_call_map[index];

                if let Some(id) = tc.get("id").and_then(|i| i.as_str()) {
                    accumulated.id = id.to_string();
                }

                if let Some(function) = tc.get("function") {
                    if let Some(name) = function.get("name").and_then(|n| n.as_str()) {
                        accumulated.name = name.to_string();
                    }
                    if let Some(args) = function.get("arguments").and_then(|a| a.as_str()) {
                        accumulated.arguments_json.push_str(args);
                    }
                }
            }
        }

        Ok(())
    }

    /// Chat Completions flow with retry logic.
    pub(super) async fn chat_completions(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
        model: &str,
        system_prompt: Option<&str>,
        on_token: Option<TokenCallback>,
        reasoning_effort: Option<&str>,
    ) -> Result<LlmResponse, Box<dyn std::error::Error + Send + Sync>> {
        let body = self.build_chat_body(model, messages, tools, system_prompt, reasoning_effort);

        let mut attempt = 0u32;
        loop {
            attempt += 1;

            let builder = self
                .apply_headers(self.streaming_client.post(&self.chat_url))
                .json(&body);
            let resp = match crate::llm::send_streaming_request(builder, model, attempt).await {
                crate::llm::StreamSend::Got(r) => r,
                crate::llm::StreamSend::Retry => continue,
                crate::llm::StreamSend::Failed(e) => return Err(e),
            };

            let status = resp.status();
            if !status.is_success() {
                let error_body = resp.text().await.unwrap_or_default();

                if crate::llm::is_retryable_status(status.as_u16())
                    && attempt <= crate::llm::MAX_RETRIES
                {
                    let delay = crate::llm::retry_delay(attempt, 1);
                    crate::llm::log_retry(model, &format!("HTTP {}", status), attempt, delay);
                    tokio::time::sleep(delay).await;
                    continue;
                }

                return Err(crate::llm::with_retry_context(
                    format!("OpenAI API error ({}): {}", status, error_body),
                    attempt,
                )
                .into());
            }

            match self.parse_chat_stream(resp, &on_token).await {
                Ok(response) => return Ok(response),
                Err(e) => {
                    let err_str = e.to_string();
                    if crate::llm::retry_after_stream_error(model, &err_str, attempt).await {
                        continue;
                    }
                    return Err(crate::llm::with_retry_context(e, attempt).into());
                }
            }
        }
    }
}

/// Longest provider detail we append. The whole string lands in a
/// `ResponseFailed` payload the UI renders, so an unbounded provider blob
/// would be unreadable there.
const PROVIDER_DETAIL_MAX: usize = 300;

/// Render one Chat Completions SSE error frame as a single readable line.
///
/// This builder serves OpenRouter and local servers as well as OpenAI, and
/// they disagree about which field names the failure. OpenAI sends `type`;
/// OpenRouter sends `code` plus a `metadata` object and no `type` at all. A
/// `type`-only reader therefore labelled EVERY OpenRouter failure `unknown`
/// and dropped the one field saying why. The Responses path already falls
/// back to `code`, so this is the two paths agreeing rather than a new rule.
fn format_stream_error(error: &serde_json::Value) -> String {
    let label = scalar_label(error.get("type"))
        .or_else(|| scalar_label(error.get("code")))
        .unwrap_or_else(|| "unknown".to_string());

    // A local OpenAI-compatible server often sends `{"error": "..."}` with no
    // object around it. Reading only `error.message` there discarded the only
    // text the frame had and reported "Unknown streaming error".
    let message = error
        .as_str()
        .or_else(|| error.get("message").and_then(|m| m.as_str()))
        .filter(|m| !m.is_empty())
        .unwrap_or("Unknown streaming error");

    match provider_detail(error.get("metadata")) {
        Some(detail) => format!("OpenAI streaming error [{label}]: {message} ({detail})"),
        None => format!("OpenAI streaming error [{label}]: {message}"),
    }
}

/// A JSON scalar as a bare label: `429`, never `"429"`. `None` for absent,
/// null, empty, or a container, so the caller can fall through to the next
/// candidate field.
fn scalar_label(value: Option<&serde_json::Value>) -> Option<String> {
    match value? {
        serde_json::Value::String(s) if !s.is_empty() => Some(s.clone()),
        serde_json::Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

/// The upstream cause OpenRouter tucks into `metadata`, as one bounded line.
/// `raw` holds the provider's own error body, which is what explains a
/// message as bare as "ERROR"; `provider_name` says who produced it.
fn provider_detail(metadata: Option<&serde_json::Value>) -> Option<String> {
    let metadata = metadata?.as_object()?;
    let provider = metadata
        .get("provider_name")
        .and_then(|p| p.as_str())
        .filter(|p| !p.is_empty());
    let raw = metadata.get("raw").and_then(collapse_to_one_line);

    let detail = match (provider, raw) {
        (Some(p), Some(r)) => format!("{p}: {r}"),
        (Some(p), None) => p.to_string(),
        (None, Some(r)) => r,
        (None, None) => return None,
    };
    Some(truncate_detail(detail))
}

/// Flatten a `raw` value to one whitespace-normalised line. Providers send it
/// as a string or as a nested object, and either can carry the newlines that
/// would break the single-line error.
///
/// `None` for a null or empty `raw`. A present-but-null one is what a provider
/// sends when it failed with no body, and rendering it appended a literal
/// "(null)" to the error.
fn collapse_to_one_line(value: &serde_json::Value) -> Option<String> {
    if value.is_null() {
        return None;
    }
    let text = match value {
        serde_json::Value::String(s) => s.clone(),
        other => other.to_string(),
    };
    let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
    (!collapsed.is_empty()).then_some(collapsed)
}

fn truncate_detail(mut detail: String) -> String {
    if detail.len() > PROVIDER_DETAIL_MAX {
        detail.truncate(detail.floor_char_boundary(PROVIDER_DETAIL_MAX));
        detail.push('…');
    }
    detail
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The Chat Completions stream MUST surface `finish_reason` (from the
    /// last per-choice chunk) and token usage (from the final empty-choices
    /// chunk added by `stream_options.include_usage`). Without this, the
    /// OpenAI provider was discarding both, breaking cost/context analytics
    /// parity with Vertex (which captures the same data via `message_start`
    /// / `message_delta`).
    #[test]
    fn chat_stream_captures_finish_reason_and_usage() {
        let mut content = String::new();
        let mut tools: Vec<AccumulatedToolCall> = Vec::new();
        let mut meta = StreamMeta::default();

        // Mid-stream text chunk: no finish_reason yet, no usage.
        OpenAiProvider::process_chat_chunk(
            r#"{"choices":[{"delta":{"content":"hi"},"finish_reason":null}]}"#,
            &mut content,
            &mut tools,
            &mut meta,
        )
        .unwrap();

        // Per-choice terminal chunk: empty delta, finish_reason set.
        OpenAiProvider::process_chat_chunk(
            r#"{"choices":[{"delta":{},"finish_reason":"stop"}]}"#,
            &mut content,
            &mut tools,
            &mut meta,
        )
        .unwrap();

        // include_usage final chunk: empty choices, usage carried alone.
        OpenAiProvider::process_chat_chunk(
            r#"{"choices":[],"usage":{"prompt_tokens":42,"completion_tokens":7,"total_tokens":49,"prompt_tokens_details":{"cached_tokens":12}}}"#,
            &mut content,
            &mut tools,
            &mut meta,
        )
        .unwrap();

        assert_eq!(content, "hi");
        assert_eq!(meta.stop_reason.as_deref(), Some("stop"));
        // 42 processed, 12 of them a cache read. The two overlap, because a
        // stored `input_tokens` is the whole prompt the model read.
        assert_eq!(meta.input_tokens, Some(42));
        assert_eq!(meta.output_tokens, Some(7));
        assert_eq!(meta.cache_read_tokens, Some(12));

        let resp = OpenAiProvider::build_llm_response(content, tools, meta).unwrap();
        assert_eq!(resp.content.as_deref(), Some("hi"));
        assert_eq!(resp.stop_reason.as_deref(), Some("stop"));
        assert_eq!(resp.input_tokens, Some(42));
        assert_eq!(resp.output_tokens, Some(7));
        assert_eq!(resp.cache_read_tokens, Some(12));
        // OpenAI doesn't separate cache writes from reads, so this stays None.
        assert_eq!(resp.cache_creation_tokens, None);
    }

    /// The overlap, on its own, so it is not buried in a stream test. The
    /// cached subset is recorded beside the prompt total, never out of it. That
    /// is what makes the stored figure the size of the whole prompt.
    #[test]
    fn a_cached_prefix_overlaps_the_prompt_total_rather_than_reducing_it() {
        let mut content = String::new();
        let mut tools: Vec<AccumulatedToolCall> = Vec::new();
        let mut meta = StreamMeta::default();
        OpenAiProvider::process_chat_chunk(
            r#"{"choices":[],"usage":{"prompt_tokens":42,"completion_tokens":0,"prompt_tokens_details":{"cached_tokens":12}}}"#,
            &mut content,
            &mut tools,
            &mut meta,
        )
        .unwrap();
        assert_eq!(meta.input_tokens, Some(42));
        assert_eq!(meta.cache_read_tokens, Some(12));
    }

    /// A prompt that read no cache leaves the count unset, so "read nothing"
    /// stays distinct from "this server reports no cached count at all".
    #[test]
    fn a_prompt_with_no_cache_reports_no_cache_read() {
        let mut content = String::new();
        let mut tools: Vec<AccumulatedToolCall> = Vec::new();
        let mut meta = StreamMeta::default();
        OpenAiProvider::process_chat_chunk(
            r#"{"choices":[],"usage":{"prompt_tokens":42,"completion_tokens":0}}"#,
            &mut content,
            &mut tools,
            &mut meta,
        )
        .unwrap();
        assert_eq!(meta.input_tokens, Some(42));
        assert_eq!(meta.cache_read_tokens, None);
    }

    /// More cached than prompt is a corrupt block. The wire records what
    /// arrived rather than repairing it, and the one consumer that subtracts
    /// saturates there instead.
    #[test]
    fn a_cached_count_over_the_prompt_total_is_recorded_as_reported() {
        let mut content = String::new();
        let mut tools: Vec<AccumulatedToolCall> = Vec::new();
        let mut meta = StreamMeta::default();
        OpenAiProvider::process_chat_chunk(
            r#"{"choices":[],"usage":{"prompt_tokens":10,"completion_tokens":0,"prompt_tokens_details":{"cached_tokens":99}}}"#,
            &mut content,
            &mut tools,
            &mut meta,
        )
        .unwrap();
        assert_eq!(meta.input_tokens, Some(10));
        assert_eq!(meta.cache_read_tokens, Some(99));
    }

    /// Empty content with `finish_reason=length` is the truncation case. We
    /// must surface it so the empty-completion diagnostic can distinguish
    /// "the model said nothing" from "we hit max_completion_tokens".
    #[test]
    fn chat_stream_captures_truncation_when_content_empty() {
        let mut content = String::new();
        let mut tools: Vec<AccumulatedToolCall> = Vec::new();
        let mut meta = StreamMeta::default();

        OpenAiProvider::process_chat_chunk(
            r#"{"choices":[{"delta":{},"finish_reason":"length"}]}"#,
            &mut content,
            &mut tools,
            &mut meta,
        )
        .unwrap();
        OpenAiProvider::process_chat_chunk(
            r#"{"choices":[],"usage":{"prompt_tokens":16000,"completion_tokens":0,"total_tokens":16000}}"#,
            &mut content,
            &mut tools,
            &mut meta,
        )
        .unwrap();

        let resp = OpenAiProvider::build_llm_response(content, tools, meta).unwrap();
        assert_eq!(resp.content, None);
        assert!(resp.tool_calls.is_empty());
        assert_eq!(resp.stop_reason.as_deref(), Some("length"));
        assert_eq!(resp.input_tokens, Some(16000));
        assert_eq!(resp.output_tokens, Some(0));
    }

    /// The incident this guard exists for: `finish_reason: stop`, no content,
    /// no tool call, and no usage chunk at all. `include_usage` is
    /// unconditional, so a compliant server always closes with usage. Its
    /// absence means the terminal frame never arrived, which is ADR 0089's
    /// truncation reached by a different signal than Claude's missing
    /// `message_delta`. Without this the turn reached
    /// `classify_empty_completion` as a clean stop with `output_tokens`
    /// defaulted to 0, graded benign, and the thread ended Idle in silence.
    #[test]
    fn a_clean_stop_that_reported_no_usage_at_all_is_a_truncation() {
        let mut content = String::new();
        let mut tools: Vec<AccumulatedToolCall> = Vec::new();
        let mut meta = StreamMeta::default();

        OpenAiProvider::process_chat_chunk(
            r#"{"choices":[{"delta":{"role":"assistant"},"finish_reason":null}]}"#,
            &mut content,
            &mut tools,
            &mut meta,
        )
        .unwrap();
        OpenAiProvider::process_chat_chunk(
            r#"{"choices":[{"delta":{},"finish_reason":"stop"}]}"#,
            &mut content,
            &mut tools,
            &mut meta,
        )
        .unwrap();

        assert_eq!(meta.stop_reason.as_deref(), Some("stop"));
        assert_eq!(meta.input_tokens, None, "no usage chunk arrived");

        let err = OpenAiProvider::build_llm_response(content, tools, meta)
            .expect_err("a usage-less empty stream must not build a response")
            .to_string();
        // The literal `is_transient_error` matches on, which is what routes
        // this into the retry loop rather than failing the turn.
        assert!(err.contains("stream truncated"), "wording: {err}");
        assert!(crate::llm::is_retryable_error(&err), "wording: {err}");
    }

    /// A stream that already streamed text stays a success even with no usage
    /// chunk. The token callback has rendered that text to the frontend, so a
    /// retry would render the answer twice. That is the alternative ADR 0089
    /// rejected outright.
    #[test]
    fn a_stream_that_rendered_text_is_never_a_truncation() {
        let mut content = String::new();
        let mut tools: Vec<AccumulatedToolCall> = Vec::new();
        let mut meta = StreamMeta::default();

        OpenAiProvider::process_chat_chunk(
            r#"{"choices":[{"delta":{"content":"partial answer"},"finish_reason":"stop"}]}"#,
            &mut content,
            &mut tools,
            &mut meta,
        )
        .unwrap();

        let resp = OpenAiProvider::build_llm_response(content, tools, meta).unwrap();
        assert_eq!(resp.content.as_deref(), Some("partial answer"));
        assert_eq!(resp.input_tokens, None);
    }

    /// A stream that produced a tool call stays a success even with no usage
    /// chunk. Retrying would run the tool a second time, and a side-effecting
    /// tool cannot be replayed for free.
    #[test]
    fn a_stream_that_produced_a_tool_call_is_never_a_truncation() {
        let mut content = String::new();
        let mut tools: Vec<AccumulatedToolCall> = Vec::new();
        let mut meta = StreamMeta::default();

        OpenAiProvider::process_chat_chunk(
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_1","function":{"name":"read_file","arguments":"{\"path\":\"a.txt\"}"}}]},"finish_reason":"tool_calls"}]}"#,
            &mut content,
            &mut tools,
            &mut meta,
        )
        .unwrap();

        let resp = OpenAiProvider::build_llm_response(content, tools, meta).unwrap();
        assert_eq!(resp.tool_calls.len(), 1);
        assert_eq!(resp.tool_calls[0].name, "read_file");
        assert_eq!(resp.input_tokens, None);
    }

    /// A server that reports only one half of the token pair is odd, not
    /// truncated. Either count proves the terminal frame arrived, so this must
    /// not retry every silent turn such a server produces.
    #[test]
    fn a_partial_usage_block_still_proves_the_stream_completed() {
        let mut content = String::new();
        let mut tools: Vec<AccumulatedToolCall> = Vec::new();
        let mut meta = StreamMeta::default();

        OpenAiProvider::process_chat_chunk(
            r#"{"choices":[{"delta":{},"finish_reason":"stop"}]}"#,
            &mut content,
            &mut tools,
            &mut meta,
        )
        .unwrap();
        OpenAiProvider::process_chat_chunk(
            r#"{"choices":[],"usage":{"completion_tokens":0}}"#,
            &mut content,
            &mut tools,
            &mut meta,
        )
        .unwrap();

        assert_eq!(meta.input_tokens, None, "this server omits prompt_tokens");
        let resp = OpenAiProvider::build_llm_response(content, tools, meta).unwrap();
        assert_eq!(resp.content, None);
        assert_eq!(resp.output_tokens, Some(0));
    }

    /// The case ADR 0009 protects: the model finished cleanly and chose to say
    /// nothing. The usage chunk arrived, so the stream completed. This stays a
    /// benign empty completion and must not be dragged into a retry.
    #[test]
    fn an_empty_turn_that_reported_usage_stays_a_benign_completion() {
        let mut content = String::new();
        let mut tools: Vec<AccumulatedToolCall> = Vec::new();
        let mut meta = StreamMeta::default();

        OpenAiProvider::process_chat_chunk(
            r#"{"choices":[{"delta":{},"finish_reason":"stop"}]}"#,
            &mut content,
            &mut tools,
            &mut meta,
        )
        .unwrap();
        OpenAiProvider::process_chat_chunk(
            r#"{"choices":[],"usage":{"prompt_tokens":900,"completion_tokens":0,"total_tokens":900}}"#,
            &mut content,
            &mut tools,
            &mut meta,
        )
        .unwrap();

        let resp = OpenAiProvider::build_llm_response(content, tools, meta).unwrap();
        assert_eq!(resp.content, None);
        assert!(resp.tool_calls.is_empty());
        assert_eq!(resp.stop_reason.as_deref(), Some("stop"));
        assert_eq!(resp.input_tokens, Some(900));
        assert_eq!(resp.output_tokens, Some(0));
    }

    /// Feed one SSE error frame through the chunk parser and return the
    /// message it fails with. Every error-frame test goes through the real
    /// entry point rather than calling the formatter directly.
    fn stream_error_for(frame: &str) -> String {
        let mut content = String::new();
        let mut tools: Vec<AccumulatedToolCall> = Vec::new();
        let mut meta = StreamMeta::default();
        OpenAiProvider::process_chat_chunk(frame, &mut content, &mut tools, &mut meta)
            .expect_err("an error frame must fail the chunk")
            .to_string()
    }

    /// OpenRouter sends `code` and `metadata` and no `type`, so a
    /// `type`-only reader called every one of its failures `[unknown]` and
    /// threw away the upstream text. Label from `code`, and name the
    /// provider that actually produced the error.
    #[test]
    fn an_openrouter_error_frame_labels_the_code_and_names_the_provider() {
        let msg = stream_error_for(
            r#"{"error":{"code":429,"message":"Provider returned error",
                "metadata":{"provider_name":"Chutes","raw":"rate limit exceeded"}}}"#,
        );
        assert_eq!(
            msg,
            "OpenAI streaming error [429]: Provider returned error \
             (Chutes: rate limit exceeded)"
        );
    }

    /// The frame that ended the diagnosed turn: a bare message, no `type`
    /// and no `code`. It still reads `[unknown]`, because there is genuinely
    /// nothing else in the frame to say.
    #[test]
    fn an_error_frame_with_no_type_and_no_code_stays_unknown() {
        assert_eq!(
            stream_error_for(r#"{"error":{"message":"ERROR"}}"#),
            "OpenAI streaming error [unknown]: ERROR"
        );
    }

    /// Regression: OpenAI's own frames carry `type`, which still wins over
    /// `code` when both are present.
    #[test]
    fn an_openai_error_frame_still_labels_the_type() {
        assert_eq!(
            stream_error_for(
                r#"{"error":{"type":"server_error","code":"internal",
                    "message":"upstream failed"}}"#
            ),
            "OpenAI streaming error [server_error]: upstream failed"
        );
    }

    /// A provider can put its whole error body in `raw`, newlines and all.
    /// The line stays one line and stays bounded, because this string is
    /// persisted in a `ResponseFailed` payload and rendered in the UI.
    #[test]
    fn a_long_multiline_provider_detail_is_flattened_and_bounded() {
        let raw = format!("upstream said:\n{}", "x".repeat(1000));
        let frame = serde_json::json!({
            "error": {
                "code": 502,
                "message": "Provider returned error",
                "metadata": { "provider_name": "Chutes", "raw": raw },
            }
        })
        .to_string();

        let msg = stream_error_for(&frame);
        assert!(!msg.contains('\n'), "must stay one line: {msg}");
        assert!(msg.ends_with("…)"), "must be marked as truncated: {msg}");
        assert!(msg.len() < 400, "must stay bounded, got {}", msg.len());
        assert!(msg.starts_with("OpenAI streaming error [502]: Provider returned error (Chutes: "));
    }

    /// An error frame carrying nothing usable must still fail the stream,
    /// rather than being read as an ordinary empty chunk.
    #[test]
    fn an_empty_error_object_still_fails_the_stream() {
        assert_eq!(
            stream_error_for(r#"{"error":{}}"#),
            "OpenAI streaming error [unknown]: Unknown streaming error"
        );
    }

    /// A local OpenAI-compatible server often sends the error as a bare
    /// string. Reading only `error.message` threw away the only text the
    /// frame carried.
    #[test]
    fn a_bare_string_error_keeps_its_text() {
        assert_eq!(
            stream_error_for(r#"{"error":"context window exceeded"}"#),
            "OpenAI streaming error [unknown]: context window exceeded"
        );
    }

    /// A provider that failed with no body sends `raw: null`. Rendering that
    /// appended a literal "(null)" to an otherwise clean error.
    #[test]
    fn a_null_provider_body_appends_nothing() {
        assert_eq!(
            stream_error_for(
                r#"{"error":{"code":502,"message":"Provider returned error",
                    "metadata":{"provider_name":"Chutes","raw":null}}}"#
            ),
            "OpenAI streaming error [502]: Provider returned error (Chutes)"
        );
    }

    /// Deliberate consequence of carrying `code`: the retry classifier reads
    /// the formatted string, so a transient upstream status now reaches it.
    /// Before, every OpenRouter frame read `[unknown]` and no retry fired.
    /// A genuine client error still must not retry.
    #[test]
    fn a_transient_upstream_status_now_reaches_the_retry_classifier() {
        let transient =
            stream_error_for(r#"{"error":{"code":502,"message":"Provider returned error"}}"#);
        assert!(
            crate::llm::is_retryable_error(&transient),
            "502 must retry: {transient}"
        );

        let client_error = stream_error_for(
            r#"{"error":{"code":400,"message":"invalid request: bad tool schema"}}"#,
        );
        assert!(
            !crate::llm::is_retryable_error(&client_error),
            "400 must not retry: {client_error}"
        );
    }

    /// Regression: after a tool call the agentic loop packs the ToolResult and
    /// a trailing instruction Text into ONE user `Message::Blocks`. The
    /// serializer must emit the `tool` message IMMEDIATELY after the assistant
    /// `tool_calls` message — never wedge the user text between them — or strict
    /// providers (Moonshot/Kimi, OpenAI) reject with HTTP 400 "an assistant
    /// message with 'tool_calls' must be followed by tool messages … the
    /// following tool_call_ids did not have response messages: load_knowhow:0".
    /// (Observed on moonshotai/kimi-k3 via OpenRouter, thread 85239abe.)
    #[test]
    fn tool_result_immediately_follows_assistant_tool_calls() {
        let messages = vec![
            Message {
                role: "assistant".to_string(),
                content: MessageContent::Blocks(vec![ContentBlock::ToolUse {
                    id: "load_knowhow:0".to_string(),
                    name: "load_knowhow".to_string(),
                    input: serde_json::json!({ "id": "browser-learning/reflection" }),
                    thought_signature: None,
                }]),
            },
            Message {
                role: "user".to_string(),
                content: MessageContent::Blocks(vec![
                    ContentBlock::ToolResult {
                        tool_use_id: "load_knowhow:0".to_string(),
                        content: "…knowhow body…".to_string(),
                    },
                    ContentBlock::Text {
                        text: "Results above. Do NOT repeat analysis you already gave.".to_string(),
                    },
                ]),
            },
        ];

        let wire = OpenAiProvider::convert_messages_chat(&messages);

        // Exactly three wire messages, in order:
        //   assistant(tool_calls) → tool(result) → user(text)
        assert_eq!(
            wire.len(),
            3,
            "expected assistant, tool, user; got {:?}",
            wire
        );
        assert_eq!(wire[0]["role"], "assistant");
        assert_eq!(wire[0]["tool_calls"][0]["id"], "load_knowhow:0");
        assert_eq!(wire[1]["role"], "tool");
        assert_eq!(wire[1]["tool_call_id"], "load_knowhow:0");
        assert_eq!(wire[2]["role"], "user");
        assert!(wire[2]["content"]
            .as_str()
            .unwrap()
            .contains("Results above"));

        // The tool response must be ADJACENT to the assistant tool_calls — no
        // `user` message wedged between (that is the Moonshot/Kimi 400).
        let assistant_idx = wire.iter().position(|m| m["role"] == "assistant").unwrap();
        let tool_idx = wire
            .iter()
            .position(|m| m["role"] == "tool" && m["tool_call_id"] == "load_knowhow:0")
            .unwrap();
        assert_eq!(
            tool_idx,
            assistant_idx + 1,
            "tool result must immediately follow the assistant tool_calls message"
        );
    }

    /// `stream_options.include_usage` must be present in the Chat
    /// Completions request body — without it, OpenAI never sends the final
    /// usage chunk and `meta.input_tokens` / `meta.output_tokens` stay None.
    #[test]
    fn chat_body_requests_usage_in_stream() {
        let provider = OpenAiProvider::new("k".to_string(), "gpt-4o".to_string()).unwrap();
        let body = provider.build_chat_body("gpt-4o", &[], &[], None, None);
        let opts = body
            .get("stream_options")
            .expect("stream_options must be sent");
        assert_eq!(opts["include_usage"], serde_json::Value::Bool(true));
    }

    /// The effort reaches the body exactly as handed in, for every model.
    ///
    /// This builder also serves OpenRouter and local servers, so it cannot tell
    /// whose vocabulary applies from the model id. It used to rewrite `max`
    /// into `xhigh` whenever the id was not `gpt-5.6`, which sent an
    /// OpenAI-proprietary tier to a local server that rejected it (400,
    /// 2026-08-12). Deciding what a model supports belongs to `llm::reasoning`
    /// and is enforced by `RoutingProvider`; a second rule here is drift.
    #[test]
    fn chat_body_sends_the_reasoning_effort_verbatim() {
        let provider = OpenAiProvider::new("k".to_string(), "gpt-4o".to_string()).unwrap();
        for model in [
            "gpt-5.6-sol",
            "gpt-5.4",
            "muse-glimmer:30b-mlx",
            "z-ai/glm-5.2",
        ] {
            for effort in crate::llm::reasoning::EFFORT_LADDER {
                let body = provider.build_chat_body(model, &[], &[], None, Some(effort));
                assert_eq!(
                    body["reasoning_effort"], *effort,
                    "{model} rewrote {effort}"
                );
            }
        }
    }

    /// No effort means no key at all, so the server applies its own default
    /// rather than being told a value the caller never chose.
    #[test]
    fn chat_body_omits_reasoning_effort_when_none_is_given() {
        let provider = OpenAiProvider::new("k".to_string(), "gpt-4o".to_string()).unwrap();
        let body = provider.build_chat_body("gpt-4o", &[], &[], None, None);
        assert!(body.get("reasoning_effort").is_none());
    }
}
