//! OpenAI Responses API path (GPT-5+ / codex models) for `OpenAiProvider`.
//! The struct, shared stream types, and dispatch live in the parent `openai`
//! module.

use crate::llm::provider::{
    ContentBlock, LlmResponse, Message, MessageContent, TokenCallback, ToolDefinition,
};
use crate::llm::ModelSelection;
use futures::StreamExt;
use std::collections::HashMap;
use std::time::Duration;

use super::{
    AccumulatedToolCall, OpenAiProvider, StreamMeta, CHUNK_TIMEOUT_SECS,
    DEFAULT_MAX_COMPLETION_TOKENS, STREAM_LOG_TAG,
};

impl OpenAiProvider {
    /// Convert internal messages to Responses API input items.
    ///
    /// Responses API uses a flat array of typed items:
    /// - `{"role": "user"|"assistant", "content": "..."}`
    /// - `{"type": "function_call", "id": "...", "call_id": "...", "name": "...", "arguments": "..."}`
    /// - `{"type": "function_call_output", "call_id": "...", "output": "..."}`
    ///
    /// `breakpoints` is [`super::takes_cache_breakpoints`] for the model.
    fn convert_messages_responses(
        messages: &[Message],
        breakpoints: bool,
    ) -> Vec<serde_json::Value> {
        let mut input: Vec<serde_json::Value> = Vec::new();
        let mut marked = 0;

        for msg in messages {
            match &msg.content {
                MessageContent::Text(text) => {
                    input.push(serde_json::json!({
                        "role": msg.role,
                        "content": text,
                    }));
                }
                MessageContent::Blocks(blocks) => {
                    let mut text_parts: Vec<String> = Vec::new();
                    let marked_text = breakpoints
                        && msg.role == "user"
                        && blocks
                            .iter()
                            .any(|b| matches!(b, ContentBlock::MemoryView { .. }));
                    if marked_text {
                        input.push(serde_json::json!({
                            "role": msg.role,
                            "content": marked_parts(blocks, &mut marked),
                        }));
                        continue;
                    }

                    for block in blocks {
                        match block {
                            // A tail block is ordinary text here. Only the
                            // engine and the Anthropic cache anchor care who
                            // wrote it.
                            ContentBlock::Text { text }
                            | ContentBlock::EngineTail { text }
                            | ContentBlock::MemoryView { text } => {
                                text_parts.push(text.clone());
                            }
                            ContentBlock::ToolUse {
                                id,
                                name,
                                input: tool_input,
                                ..
                            } => {
                                // Flush accumulated text before emitting function_call
                                if !text_parts.is_empty() {
                                    input.push(serde_json::json!({
                                        "role": "assistant",
                                        "content": text_parts.join("\n"),
                                    }));
                                    text_parts.clear();
                                }
                                // Responses API requires function_call "id" to start
                                // with "fc_".  Our internal ToolUse.id stores the
                                // call_id from the API (starts with "call_"), so
                                // derive an fc-prefixed item id for the "id" field.
                                let item_id = if id.starts_with("fc_") {
                                    id.clone()
                                } else {
                                    format!("fc_{}", id)
                                };
                                input.push(serde_json::json!({
                                    "type": "function_call",
                                    "id": item_id,
                                    "call_id": id,
                                    "name": name,
                                    "arguments": serde_json::to_string(tool_input)
                                        .unwrap_or_else(|e| {
                                            log!("[OpenAI] Failed to serialize tool arguments for Responses API: {}", e);
                                            "{}".to_string()
                                        }),
                                }));
                            }
                            ContentBlock::Image { .. } => {
                                // Responses API (codex models) doesn't support images — skip
                            }
                            ContentBlock::ToolResult {
                                tool_use_id,
                                content,
                            } => {
                                if !text_parts.is_empty() {
                                    input.push(serde_json::json!({
                                        "role": msg.role,
                                        "content": text_parts.join("\n"),
                                    }));
                                    text_parts.clear();
                                }
                                input.push(serde_json::json!({
                                    "type": "function_call_output",
                                    "call_id": tool_use_id,
                                    "output": content,
                                }));
                            }
                        }
                    }

                    if !text_parts.is_empty() {
                        input.push(serde_json::json!({
                            "role": msg.role,
                            "content": text_parts.join("\n"),
                        }));
                    }
                }
            }
        }

        input
    }

    /// Convert internal tool definitions to Responses API format (flat, no "function" wrapper).
    /// Explicitly sets `strict: false` because our schemas have optional parameters and
    /// freeform objects (e.g. http_request headers) that aren't compatible with strict mode
    /// (which requires `additionalProperties: false` and all properties in `required`).
    /// The Responses API defaults `strict` to `true`, so without this the API rejects our
    /// tool schemas with a 400 error and the model never sees the tools.
    fn convert_tools_responses(tools: &[ToolDefinition]) -> Option<Vec<serde_json::Value>> {
        if tools.is_empty() {
            return None;
        }
        Some(
            tools
                .iter()
                .map(|t| {
                    serde_json::json!({
                        "type": "function",
                        "name": t.name,
                        "description": t.description,
                        "parameters": t.parameters,
                        "strict": false,
                    })
                })
                .collect(),
        )
    }

    fn build_responses_body(
        &self,
        model: &str,
        messages: &[Message],
        tools: &[ToolDefinition],
        system_prompt: Option<&str>,
        reasoning_effort: Option<&str>,
    ) -> serde_json::Value {
        let input =
            Self::convert_messages_responses(messages, super::takes_cache_breakpoints(model));

        let mut body = serde_json::json!({
            "model": model,
            "stream": true,
            "max_output_tokens": DEFAULT_MAX_COMPLETION_TOKENS,
            "input": input,
            // Opt out of OpenAI's server-side response storage: the Responses
            // API defaults to `store: true` and keeps the response object
            // (prompt and output alike) for at least 30 days. We are stateless
            // by construction, since `convert_messages_responses` rebuilds the
            // full history as `input` items every turn and we never send
            // `previous_response_id`, so the stored copy is retention with no
            // upside. What it forecloses is chaining on `previous_response_id`
            // to save input tokens: that id resolves to
            // `previous_response_not_found` under `store: false`, so the
            // optimization would have to carry reasoning items client-side
            // instead of flipping this back.
            "store": false,
        });

        if let Some(instructions) = system_prompt {
            body["instructions"] = serde_json::Value::String(instructions.to_string());
        }

        if let Some(tool_defs) = Self::convert_tools_responses(tools) {
            body["tools"] = serde_json::Value::Array(tool_defs);
        }

        // Verbatim, deliberately. Which tiers this model supports is decided
        // once, in `llm::reasoning`, and enforced by `RoutingProvider`'s clamp;
        // a second per-model rule here is what let the picker and the wire
        // disagree (see `llm/reasoning.rs`).
        if let Some(effort) = reasoning_effort {
            body["reasoning"] = serde_json::json!({ "effort": effort });
        }

        body
    }

    /// Parse an SSE stream from the Responses API.
    ///
    /// Responses API SSE format uses `event: <type>` + `data: <json>` pairs.
    /// Key events:
    /// - `response.output_text.delta` — text content
    /// - `response.output_item.added` (function_call) — new tool call
    /// - `response.function_call_arguments.delta` — streaming args
    /// - `response.function_call_arguments.done` — finalized args
    /// - `response.completed` / `response.failed` — stream end
    async fn parse_responses_stream(
        &self,
        response: reqwest::Response,
        on_token: &Option<TokenCallback>,
    ) -> Result<LlmResponse, Box<dyn std::error::Error + Send + Sync>> {
        let mut stream = response.bytes_stream();
        let mut buffer = String::new();
        // Bytes of a character the transport split across two chunks.
        let mut carry: Vec<u8> = Vec::new();
        let chunk_timeout = Duration::from_secs(CHUNK_TIMEOUT_SECS);

        let mut content = String::new();
        let mut tool_calls: Vec<AccumulatedToolCall> = Vec::new();
        let mut item_id_map: HashMap<String, usize> = HashMap::new();
        let mut current_event_type = String::new();
        let mut meta = StreamMeta::default();

        'outer: loop {
            let chunk = match tokio::time::timeout(chunk_timeout, stream.next()).await {
                Ok(Some(Ok(bytes))) => bytes,
                Ok(Some(Err(e))) => {
                    return Err(crate::llm::stream_failure(
                        format!("Stream read error: {}", e),
                        on_token.is_some() && !content.is_empty(),
                        STREAM_LOG_TAG,
                    ))
                }
                Ok(None) => break,
                Err(_) => {
                    return Err(crate::llm::stream_failure(
                        format!(
                            "OpenAI stream timed out (no data for {}s)",
                            CHUNK_TIMEOUT_SECS
                        ),
                        on_token.is_some() && !content.is_empty(),
                        STREAM_LOG_TAG,
                    ))
                }
            };

            crate::llm::push_utf8_chunk(&mut carry, &chunk, &mut buffer);

            while let Some(newline_pos) = buffer.find('\n') {
                let line = buffer[..newline_pos].trim_end_matches('\r').to_string();
                buffer = buffer[newline_pos + 1..].to_string();

                if line.is_empty() {
                    continue;
                }

                if let Some(event_type) = line.strip_prefix("event: ") {
                    current_event_type = event_type.trim().to_string();
                    continue;
                }

                if let Some(data_str) = line.strip_prefix("data: ") {
                    let prev_len = content.len();
                    let done = Self::process_responses_chunk(
                        &current_event_type,
                        data_str,
                        &mut content,
                        &mut tool_calls,
                        &mut item_id_map,
                        &mut meta,
                    )
                    .map_err(|e| {
                        crate::llm::stream_failure(
                            e.to_string(),
                            on_token.is_some() && prev_len > 0,
                            STREAM_LOG_TAG,
                        )
                    })?;
                    if content.len() > prev_len {
                        if let Some(cb) = on_token {
                            // Floor defensively — `prev_len` is a byte length
                            // captured before the chunk appended, so a future
                            // accumulator change could leave it mid-codepoint.
                            cb(&content[content.floor_char_boundary(prev_len)..]);
                        }
                    }
                    if done {
                        break 'outer;
                    }
                    current_event_type.clear();
                }
            }
        }

        Self::build_llm_response(content, tool_calls, meta)
    }

    /// Process a single Responses API SSE event. Returns `true` when the stream is done.
    fn process_responses_chunk(
        event_type: &str,
        data_str: &str,
        content: &mut String,
        tool_calls: &mut Vec<AccumulatedToolCall>,
        item_id_map: &mut HashMap<String, usize>,
        meta: &mut StreamMeta,
    ) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
        let data: serde_json::Value = serde_json::from_str(data_str)?;

        match event_type {
            "response.output_text.delta" => {
                if let Some(delta) = data.get("delta").and_then(|d| d.as_str()) {
                    content.push_str(delta);
                }
            }

            "response.output_item.added" => {
                if let Some(item) = data.get("item") {
                    if item.get("type").and_then(|t| t.as_str()) == Some("function_call") {
                        let call_id = item
                            .get("call_id")
                            .and_then(|c| c.as_str())
                            .unwrap_or("")
                            .to_string();
                        let name = item
                            .get("name")
                            .and_then(|n| n.as_str())
                            .unwrap_or("")
                            .to_string();
                        let item_id = item
                            .get("id")
                            .and_then(|i| i.as_str())
                            .unwrap_or("")
                            .to_string();

                        let idx = tool_calls.len();
                        tool_calls.push(AccumulatedToolCall {
                            id: call_id,
                            name,
                            arguments_json: String::new(),
                        });
                        if !item_id.is_empty() {
                            item_id_map.insert(item_id, idx);
                        }
                    }
                }
            }

            "response.function_call_arguments.delta" => {
                if let Some(delta) = data.get("delta").and_then(|d| d.as_str()) {
                    let item_id = data.get("item_id").and_then(|i| i.as_str()).unwrap_or("");
                    if let Some(&idx) = item_id_map.get(item_id) {
                        if let Some(tc) = tool_calls.get_mut(idx) {
                            tc.arguments_json.push_str(delta);
                        }
                    }
                }
            }

            "response.function_call_arguments.done" => {
                let call_id = data.get("call_id").and_then(|c| c.as_str()).unwrap_or("");
                if let Some(full_args) = data.get("arguments").and_then(|a| a.as_str()) {
                    if let Some(tc) = tool_calls.iter_mut().find(|tc| tc.id == call_id) {
                        tc.arguments_json = full_args.to_string();
                    }
                }
                if let Some(name) = data.get("name").and_then(|n| n.as_str()) {
                    if let Some(tc) = tool_calls.iter_mut().find(|tc| tc.id == call_id) {
                        if tc.name.is_empty() {
                            tc.name = name.to_string();
                        }
                    }
                }
            }

            // Both terminal events carry the full Response object with its
            // usage block and a status. `response.incomplete` adds the
            // `incomplete_details` reason, such as `max_output_tokens`. Missing
            // it read a cut-off turn as a truncated stream, and retried it.
            "response.completed" | "response.incomplete" => {
                if let Some(resp) = data.get("response") {
                    meta.absorb_responses_completion(resp);
                }
                return Ok(true);
            }

            // A top-level `error` event carries `code` and `message` on the
            // event itself.
            "response.failed" | "error" => {
                let error_msg = data
                    .pointer("/response/error/message")
                    .or_else(|| data.pointer("/error/message"))
                    .or_else(|| data.get("message"))
                    .and_then(|m| m.as_str())
                    .unwrap_or("Unknown error");
                let error_code = data
                    .pointer("/response/error/code")
                    .or_else(|| data.pointer("/error/code"))
                    .or_else(|| data.get("code"))
                    .and_then(|c| c.as_str())
                    .unwrap_or("unknown");
                return Err(
                    format!("OpenAI Responses API error [{}]: {}", error_code, error_msg).into(),
                );
            }

            _ => {}
        }

        Ok(false)
    }

    /// Responses API flow with retry logic.
    pub(super) async fn chat_responses(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
        selection: ModelSelection<'_>,
        system_prompt: Option<&str>,
        on_token: Option<TokenCallback>,
    ) -> Result<LlmResponse, Box<dyn std::error::Error + Send + Sync>> {
        let model = selection.model.unwrap_or(&self.model);
        let reasoning_effort = selection.reasoning_effort;
        let body =
            self.build_responses_body(model, messages, tools, system_prompt, reasoning_effort);

        // Debug: log tool count and names
        if let Some(tools_arr) = body.get("tools").and_then(|t| t.as_array()) {
            let names: Vec<&str> = tools_arr
                .iter()
                .filter_map(|t| t.get("name").and_then(|n| n.as_str()))
                .collect();
            log!(
                "[OpenAI] Responses API request: model={}, tools={}, tool_names={:?}",
                model,
                tools_arr.len(),
                names
            );
        } else {
            log!("[OpenAI] Responses API request: model={}, tools=0", model);
        }

        let mut attempt = 0u32;
        loop {
            attempt += 1;

            let builder = self
                .apply_headers(self.streaming_client.post(&self.responses_url))
                .json(&body);
            let resp = match crate::llm::send_streaming_request(
                builder,
                model,
                attempt,
                selection.attempt_timeout,
            )
            .await
            {
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

                log!(
                    "[OpenAI] Responses API error ({}): {}",
                    status,
                    &error_body[..error_body.floor_char_boundary(500)]
                );
                return Err(crate::llm::not_served::failure(
                    status.as_u16(),
                    &error_body,
                    crate::llm::with_retry_context(
                        format!("OpenAI Responses API error ({}): {}", status, error_body),
                        attempt,
                    ),
                ));
            }

            match self.parse_responses_stream(resp, &on_token).await {
                Ok(response) => {
                    log!(
                        "[OpenAI] Responses API result: text={}chars, tool_calls={}",
                        response.content.as_ref().map(|c| c.len()).unwrap_or(0),
                        response.tool_calls.len()
                    );
                    if !response.tool_calls.is_empty() {
                        let names: Vec<&str> = response
                            .tool_calls
                            .iter()
                            .map(|tc| tc.name.as_str())
                            .collect();
                        log!("[OpenAI] Tool calls: {:?}", names);
                    }
                    return Ok(response);
                }
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

/// Explicit breakpoints a request may carry. A request writes the cache at
/// most four times, and the default mode spends one at the end of the latest
/// message.
pub(crate) const MAX_BREAKPOINTS: usize = 3;

/// A user message holding memory view blocks, as typed text parts. Each
/// memory view block takes a breakpoint, so a later request that marks the
/// same place reads the cache up to there. A read needs a breakpoint in the
/// very same place, which is why the view marks fixed places.
///
/// The parts read as the joined string would, with a newline between two
/// parts only where the first does not already end in one. Only text
/// travels, as images do not on this wire.
fn marked_parts(blocks: &[ContentBlock], breakpoints: &mut usize) -> Vec<serde_json::Value> {
    let texts: Vec<(&str, bool)> = blocks
        .iter()
        .filter_map(|block| match block {
            ContentBlock::MemoryView { text } => Some((text.as_str(), true)),
            ContentBlock::Text { text } | ContentBlock::EngineTail { text } => {
                Some((text.as_str(), false))
            }
            _ => None,
        })
        .filter(|(text, _)| !text.is_empty())
        .collect();
    let last = texts.len().saturating_sub(1);
    texts
        .iter()
        .enumerate()
        .map(|(i, &(text, marked))| {
            let mut text = text.to_string();
            if i < last && !text.ends_with('\n') {
                text.push('\n');
            }
            let mut part = serde_json::json!({"type": "input_text", "text": text});
            if marked && *breakpoints < MAX_BREAKPOINTS {
                *breakpoints += 1;
                part["prompt_cache_breakpoint"] = serde_json::json!({"mode": "explicit"});
            }
            part
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn user(blocks: Vec<ContentBlock>) -> Message {
        Message {
            role: "user".to_string(),
            content: MessageContent::Blocks(blocks),
        }
    }

    fn view(text: &str) -> ContentBlock {
        ContentBlock::MemoryView {
            text: text.to_string(),
        }
    }

    fn text(text: &str) -> ContentBlock {
        ContentBlock::Text {
            text: text.to_string(),
        }
    }

    /// A memory view block is a cache mark on this wire too: it travels as its
    /// own part with a breakpoint, so a later request marking the same place
    /// reads up to it.
    #[test]
    fn a_memory_view_block_takes_a_breakpoint() {
        let input = OpenAiProvider::convert_messages_responses(
            &[user(vec![
                view("<chat>\na\nb\n"),
                text("c\n</chat>\n"),
                text("the task"),
            ])],
            true,
        );
        let parts = input[0]["content"].as_array().expect("typed parts");
        assert_eq!(parts.len(), 3);
        assert_eq!(parts[0]["prompt_cache_breakpoint"]["mode"], "explicit");
        assert!(parts[1].get("prompt_cache_breakpoint").is_none());
        assert!(parts[2].get("prompt_cache_breakpoint").is_none());
        let joined: String = parts.iter().map(|p| p["text"].as_str().unwrap()).collect();
        assert_eq!(joined, "<chat>\na\nb\nc\n</chat>\nthe task");
    }

    /// A message with no memory view block, which is every Classic message,
    /// keeps its one joined string.
    #[test]
    fn a_message_with_no_memory_view_block_stays_one_string() {
        let input =
            OpenAiProvider::convert_messages_responses(&[user(vec![text("a"), text("b")])], true);
        assert_eq!(input[0]["content"], "a\nb");
    }

    /// A model before GPT-5.6 refuses a breakpoint with a 400, so a message
    /// holding a memory view block goes to it as one joined string.
    #[test]
    fn a_model_without_breakpoints_gets_the_joined_string() {
        assert!(!super::super::takes_cache_breakpoints("gpt-5.5"));
        assert!(!super::super::takes_cache_breakpoints("gpt-5.3-codex"));
        assert!(super::super::takes_cache_breakpoints("gpt-5.6-sol"));
        assert!(super::super::takes_cache_breakpoints("gpt-6.1-sol"));
        let input = OpenAiProvider::convert_messages_responses(
            &[user(vec![view("<chat>\na\n"), text("the task")])],
            false,
        );
        assert_eq!(input[0]["content"], "<chat>\na\n\nthe task");
    }

    /// Breakpoints stop at three, since the default mode writes at the end of
    /// the latest message too and a request allows four writes.
    #[test]
    fn a_request_carries_at_most_three_breakpoints() {
        let blocks = (0..5).map(|i| view(&format!("block {i}\n"))).collect();
        let input = OpenAiProvider::convert_messages_responses(&[user(blocks)], true);
        let marked = input[0]["content"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|p| p.get("prompt_cache_breakpoint").is_some())
            .count();
        assert_eq!(marked, MAX_BREAKPOINTS);
    }

    /// The Responses API retains response objects for at least 30 days under
    /// its `store: true` default. We rebuild the full history as `input` on
    /// every turn and never read a stored response back, so every request must
    /// carry the opt-out.
    #[test]
    fn responses_body_opts_out_of_server_side_storage() {
        let provider = OpenAiProvider::new("k".to_string(), "gpt-5.5".to_string()).unwrap();
        let messages = vec![Message {
            role: "user".to_string(),
            content: MessageContent::Text("hi".to_string()),
        }];

        let body = provider.build_responses_body("gpt-5.5", &messages, &[], None, None);

        assert_eq!(body["store"], false);
    }

    /// Sibling of `chat_body_sends_the_reasoning_effort_verbatim`: the two
    /// builders shared the per-model rewrite, so they must both stay verbatim
    /// or they drift apart again. `llm::reasoning` decides the tiers;
    /// `RoutingProvider` enforces them.
    #[test]
    fn responses_body_sends_the_reasoning_effort_verbatim() {
        let provider = OpenAiProvider::new("k".to_string(), "gpt-5.5".to_string()).unwrap();
        for model in ["gpt-5.6-sol", "gpt-5.5-pro", "gpt-5.4"] {
            for effort in crate::llm::reasoning::EFFORT_LADDER {
                let body = provider.build_responses_body(model, &[], &[], None, Some(effort));
                assert_eq!(
                    body["reasoning"]["effort"], *effort,
                    "{model} rewrote {effort}"
                );
            }
        }
    }

    /// The Responses API `response.completed` terminal event carries the
    /// full Response object (usage + status). We must absorb both into the
    /// per-turn meta so codex-model traffic has stop_reason + token parity
    /// with Chat Completions.
    #[test]
    fn responses_stream_captures_completion_usage_and_status() {
        let mut content = String::new();
        let mut tools: Vec<AccumulatedToolCall> = Vec::new();
        let mut item_id_map: HashMap<String, usize> = HashMap::new();
        let mut meta = StreamMeta::default();

        // Mid-stream text delta.
        OpenAiProvider::process_responses_chunk(
            "response.output_text.delta",
            r#"{"delta":"ok"}"#,
            &mut content,
            &mut tools,
            &mut item_id_map,
            &mut meta,
        )
        .unwrap();

        // Terminal event — `meta` populated from the embedded Response.
        let done = OpenAiProvider::process_responses_chunk(
            "response.completed",
            r#"{"response":{"status":"completed","model":"gpt-5.6-luna-2026-04-01","usage":{"input_tokens":11,"output_tokens":3,"input_tokens_details":{"cached_tokens":4}}}}"#,
            &mut content,
            &mut tools,
            &mut item_id_map,
            &mut meta,
        )
        .unwrap();
        assert!(done, "response.completed must terminate the stream");

        assert_eq!(content, "ok");
        // 11 processed, 4 of them a cache read. `input_tokens` already covers
        // `input_tokens_details.cached_tokens`, so the two overlap.
        assert_eq!(meta.input_tokens, Some(11));
        assert_eq!(meta.output_tokens, Some(3));
        assert_eq!(meta.cache_read_tokens, Some(4));
        assert_eq!(meta.stop_reason.as_deref(), Some("completed"));
        assert_eq!(
            meta.served_model.as_deref(),
            Some("gpt-5.6-luna-2026-04-01")
        );
    }

    /// The overlap, on its own, matching the Chat Completions case exactly.
    /// One convention across both APIs, or a cost figure depends on which one
    /// the request happened to take.
    #[test]
    fn a_cached_prefix_overlaps_the_input_total_rather_than_reducing_it() {
        let mut content = String::new();
        let mut tools: Vec<AccumulatedToolCall> = Vec::new();
        let mut item_id_map = std::collections::HashMap::new();
        let mut meta = StreamMeta::default();
        OpenAiProvider::process_responses_chunk(
            "response.completed",
            r#"{"response":{"status":"completed","usage":{"input_tokens":42,"output_tokens":0,"input_tokens_details":{"cached_tokens":12}}}}"#,
            &mut content,
            &mut tools,
            &mut item_id_map,
            &mut meta,
        )
        .unwrap();
        assert_eq!(meta.input_tokens, Some(42));
        assert_eq!(meta.cache_read_tokens, Some(12));
    }

    /// GPT-5.6 on reports what it wrote to the cache, also as a part of the
    /// input total. It costs more than plain input, so it is recorded.
    #[test]
    fn a_cache_write_is_recorded_as_part_of_the_input_total() {
        let mut content = String::new();
        let mut tools: Vec<AccumulatedToolCall> = Vec::new();
        let mut item_id_map = std::collections::HashMap::new();
        let mut meta = StreamMeta::default();
        OpenAiProvider::process_responses_chunk(
            "response.completed",
            r#"{"response":{"status":"completed","usage":{"input_tokens":5476,"output_tokens":9,"input_tokens_details":{"cache_write_tokens":5435,"cached_tokens":0}}}}"#,
            &mut content,
            &mut tools,
            &mut item_id_map,
            &mut meta,
        )
        .unwrap();
        assert_eq!(meta.input_tokens, Some(5476));
        assert_eq!(meta.cache_write_tokens, Some(5435));
        assert_eq!(meta.cache_read_tokens, None);
    }

    /// A response that read no cache leaves the count unset.
    #[test]
    fn an_input_total_with_no_cache_reports_no_cache_read() {
        let mut content = String::new();
        let mut tools: Vec<AccumulatedToolCall> = Vec::new();
        let mut item_id_map = std::collections::HashMap::new();
        let mut meta = StreamMeta::default();
        OpenAiProvider::process_responses_chunk(
            "response.completed",
            r#"{"response":{"status":"completed","usage":{"input_tokens":42,"output_tokens":0}}}"#,
            &mut content,
            &mut tools,
            &mut item_id_map,
            &mut meta,
        )
        .unwrap();
        assert_eq!(meta.input_tokens, Some(42));
        assert_eq!(meta.cache_read_tokens, None);
    }

    /// `incomplete_details.reason` takes precedence over `status` when both
    /// are present — that's where "max_output_tokens" / "content_filter"
    /// surface for cut-short responses.
    #[test]
    fn responses_stream_prefers_incomplete_reason_over_status() {
        let mut content = String::new();
        let mut tools: Vec<AccumulatedToolCall> = Vec::new();
        let mut item_id_map: HashMap<String, usize> = HashMap::new();
        let mut meta = StreamMeta::default();

        let done = OpenAiProvider::process_responses_chunk(
            "response.incomplete",
            r#"{"response":{"status":"incomplete","incomplete_details":{"reason":"max_output_tokens"},"usage":{"input_tokens":50,"output_tokens":4096}}}"#,
            &mut content,
            &mut tools,
            &mut item_id_map,
            &mut meta,
        )
        .unwrap();

        assert!(done, "response.incomplete is a terminal event");
        assert_eq!(meta.stop_reason.as_deref(), Some("max_output_tokens"));
        assert_eq!(meta.input_tokens, Some(50));
        assert_eq!(meta.output_tokens, Some(4096));
    }

    /// A top-level `error` event surfaces its own message, never the generic
    /// truncation error.
    #[test]
    fn responses_stream_error_event_carries_its_message() {
        let mut content = String::new();
        let mut tools: Vec<AccumulatedToolCall> = Vec::new();
        let mut item_id_map: HashMap<String, usize> = HashMap::new();
        let mut meta = StreamMeta::default();

        let err = OpenAiProvider::process_responses_chunk(
            "error",
            r#"{"type":"error","code":"server_error","message":"The server had an error"}"#,
            &mut content,
            &mut tools,
            &mut item_id_map,
            &mut meta,
        )
        .unwrap_err()
        .to_string();

        assert!(err.contains("server_error"), "got: {err}");
        assert!(err.contains("The server had an error"), "got: {err}");
    }
}
