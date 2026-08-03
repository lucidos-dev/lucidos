//! OpenAI Chat Completions API path (non-codex models) for `OpenAiProvider`.
//! The struct, shared stream types, and dispatch live in the parent `openai`
//! module.

use crate::llm::provider::{
    ContentBlock, LlmResponse, Message, MessageContent, TokenCallback, ToolDefinition,
};
use futures::StreamExt;
use std::time::Duration;

use super::{
    openai_reasoning_effort, AccumulatedToolCall, OpenAiProvider, StreamMeta, CHUNK_TIMEOUT_SECS,
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
                            ContentBlock::Text { text } => {
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

        // Map unified reasoning_effort to OpenAI's per-model vocabulary
        // (GPT-5.6 keeps "max"; earlier models map "max" → "xhigh").
        if let Some(effort) = reasoning_effort {
            body["reasoning_effort"] =
                serde_json::Value::String(openai_reasoning_effort(effort, model).to_string());
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

            buffer.push_str(&String::from_utf8_lossy(&chunk));

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
            let error_msg = error["message"]
                .as_str()
                .unwrap_or("Unknown streaming error");
            let error_type = error["type"].as_str().unwrap_or("unknown");
            return Err(format!("OpenAI streaming error [{}]: {}", error_type, error_msg).into());
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
                    if crate::llm::is_retryable_error(&err_str)
                        && attempt <= crate::llm::MAX_RETRIES
                    {
                        let delay = crate::llm::retry_delay(attempt, 2);
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
}
