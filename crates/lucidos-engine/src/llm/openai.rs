use crate::llm::provider::{
    ContentBlock, LlmProvider, LlmResponse, Message, MessageContent, TokenCallback, ToolCall,
    ToolDefinition,
};
use async_trait::async_trait;
use futures::StreamExt;
use std::collections::HashMap;
use std::time::Duration;

const OPENAI_CHAT_URL: &str = "https://api.openai.com/v1/chat/completions";
const OPENAI_RESPONSES_URL: &str = "https://api.openai.com/v1/responses";
const CHUNK_TIMEOUT_SECS: u64 = 300;
const DEFAULT_MAX_COMPLETION_TOKENS: u32 = 16384;

/// GPT-5+ and Codex models use the Responses API, all others use Chat Completions.
fn uses_responses_api(model: &str) -> bool {
    model.contains("codex") || model.starts_with("gpt-5")
}

pub struct OpenAiProvider {
    api_key: String,
    model: String,
    /// Client without per-request timeout, used for streaming where
    /// we apply per-chunk timeouts instead.
    streaming_client: reqwest::Client,
}

impl OpenAiProvider {
    pub fn new(api_key: String, model: String) -> Self {
        Self {
            api_key,
            model,
            streaming_client: reqwest::Client::builder()
                .pool_idle_timeout(Duration::from_secs(30))
                .build()
                .expect("Failed to build streaming HTTP client"),
        }
    }

    // ---------------------------------------------------------------
    // Chat Completions API (non-codex models)
    // ---------------------------------------------------------------

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
                            ContentBlock::ToolUse { id, name, input } => {
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

                    if !tool_calls.is_empty() {
                        let mut assistant_msg = serde_json::json!({
                            "role": "assistant",
                            "tool_calls": tool_calls,
                        });
                        if !text_parts.is_empty() {
                            assistant_msg["content"] =
                                serde_json::Value::String(text_parts.join("\n"));
                        }
                        openai_messages.push(assistant_msg);
                    } else if !image_parts.is_empty() {
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

                    for (tool_call_id, content) in tool_results {
                        openai_messages.push(serde_json::json!({
                            "role": "tool",
                            "tool_call_id": tool_call_id,
                            "content": content,
                        }));
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
            "max_completion_tokens": DEFAULT_MAX_COMPLETION_TOKENS,
            "messages": openai_messages,
        });

        if let Some(tool_defs) = Self::convert_tools_chat(tools) {
            body["tools"] = serde_json::Value::Array(tool_defs);
        }

        // Map unified reasoning_effort to OpenAI's format ("max" → "xhigh")
        if let Some(effort) = reasoning_effort {
            let openai_effort = if effort == "max" { "xhigh" } else { effort };
            body["reasoning_effort"] = serde_json::Value::String(openai_effort.to_string());
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
        let chunk_timeout = Duration::from_secs(CHUNK_TIMEOUT_SECS);

        'outer: loop {
            let chunk = match tokio::time::timeout(chunk_timeout, stream.next()).await {
                Ok(Some(Ok(bytes))) => bytes,
                Ok(Some(Err(e))) => return Err(format!("Stream read error: {}", e).into()),
                Ok(None) => break,
                Err(_) => return Err("OpenAI stream timed out (no data for 5 minutes)".into()),
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
                    Self::process_chat_chunk(data_str, &mut content, &mut tool_call_map)?;
                    if content.len() > prev_len {
                        if let Some(cb) = on_token {
                            cb(&content[prev_len..]);
                        }
                    }
                }
            }
        }

        Ok(Self::build_llm_response(content, tool_call_map))
    }

    /// Process a single Chat Completions SSE chunk.
    fn process_chat_chunk(
        data_str: &str,
        content: &mut String,
        tool_call_map: &mut Vec<AccumulatedToolCall>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let data: serde_json::Value = serde_json::from_str(data_str)?;

        if let Some(error) = data.get("error") {
            let error_msg = error["message"]
                .as_str()
                .unwrap_or("Unknown streaming error");
            let error_type = error["type"].as_str().unwrap_or("unknown");
            return Err(format!("OpenAI streaming error [{}]: {}", error_type, error_msg).into());
        }

        let choices = match data.get("choices").and_then(|c| c.as_array()) {
            Some(c) => c,
            None => return Ok(()),
        };

        let delta = match choices.first().and_then(|c| c.get("delta")) {
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
    async fn chat_completions(
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

            let resp = match self
                .streaming_client
                .post(OPENAI_CHAT_URL)
                .header("Authorization", format!("Bearer {}", self.api_key))
                .header("Content-Type", "application/json")
                .json(&body)
                .send()
                .await
            {
                Ok(r) => r,
                Err(e) => {
                    if attempt <= super::MAX_RETRIES {
                        let delay = super::retry_delay(attempt, 1);
                        super::log_retry(model, &format!("Network error: {:?}", e), attempt, delay);
                        tokio::time::sleep(delay).await;
                        continue;
                    }
                    return Err(super::with_retry_context(e, attempt).into());
                }
            };

            let status = resp.status();
            if !status.is_success() {
                let error_body = resp.text().await.unwrap_or_default();

                if super::is_retryable_status(status.as_u16()) && attempt <= super::MAX_RETRIES {
                    let delay = super::retry_delay(attempt, 1);
                    super::log_retry(model, &format!("HTTP {}", status), attempt, delay);
                    tokio::time::sleep(delay).await;
                    continue;
                }

                return Err(super::with_retry_context(
                    format!("OpenAI API error ({}): {}", status, error_body),
                    attempt,
                )
                .into());
            }

            match self.parse_chat_stream(resp, &on_token).await {
                Ok(response) => return Ok(response),
                Err(e) => {
                    let err_str = e.to_string();
                    if super::is_retryable_error(&err_str) && attempt <= super::MAX_RETRIES {
                        let delay = super::retry_delay(attempt, 2);
                        super::log_retry(
                            model,
                            &format!("Stream error: {}", err_str),
                            attempt,
                            delay,
                        );
                        tokio::time::sleep(delay).await;
                        continue;
                    }

                    return Err(super::with_retry_context(e, attempt).into());
                }
            }
        }
    }

    // ---------------------------------------------------------------
    // Responses API (codex models)
    // ---------------------------------------------------------------

    /// Convert internal messages to Responses API input items.
    ///
    /// Responses API uses a flat array of typed items:
    /// - `{"role": "user"|"assistant", "content": "..."}`
    /// - `{"type": "function_call", "id": "...", "call_id": "...", "name": "...", "arguments": "..."}`
    /// - `{"type": "function_call_output", "call_id": "...", "output": "..."}`
    fn convert_messages_responses(messages: &[Message]) -> Vec<serde_json::Value> {
        let mut input: Vec<serde_json::Value> = Vec::new();

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

                    for block in blocks {
                        match block {
                            ContentBlock::Text { text } => {
                                text_parts.push(text.clone());
                            }
                            ContentBlock::ToolUse {
                                id,
                                name,
                                input: tool_input,
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
        let input = Self::convert_messages_responses(messages);

        let mut body = serde_json::json!({
            "model": model,
            "stream": true,
            "max_output_tokens": DEFAULT_MAX_COMPLETION_TOKENS,
            "input": input,
        });

        if let Some(instructions) = system_prompt {
            body["instructions"] = serde_json::Value::String(instructions.to_string());
        }

        if let Some(tool_defs) = Self::convert_tools_responses(tools) {
            body["tools"] = serde_json::Value::Array(tool_defs);
        }

        // Map unified reasoning_effort to OpenAI's format ("max" → "xhigh")
        if let Some(effort) = reasoning_effort {
            let openai_effort = if effort == "max" { "xhigh" } else { effort };
            body["reasoning"] = serde_json::json!({ "effort": openai_effort });
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
        let chunk_timeout = Duration::from_secs(CHUNK_TIMEOUT_SECS);

        let mut content = String::new();
        let mut tool_calls: Vec<AccumulatedToolCall> = Vec::new();
        let mut item_id_map: HashMap<String, usize> = HashMap::new();
        let mut current_event_type = String::new();

        'outer: loop {
            let chunk = match tokio::time::timeout(chunk_timeout, stream.next()).await {
                Ok(Some(Ok(bytes))) => bytes,
                Ok(Some(Err(e))) => return Err(format!("Stream read error: {}", e).into()),
                Ok(None) => break,
                Err(_) => return Err("OpenAI stream timed out (no data for 5 minutes)".into()),
            };

            buffer.push_str(&String::from_utf8_lossy(&chunk));

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
                    )?;
                    if content.len() > prev_len {
                        if let Some(cb) = on_token {
                            cb(&content[prev_len..]);
                        }
                    }
                    if done {
                        break 'outer;
                    }
                    current_event_type.clear();
                }
            }
        }

        Ok(Self::build_llm_response(content, tool_calls))
    }

    /// Process a single Responses API SSE event. Returns `true` when the stream is done.
    fn process_responses_chunk(
        event_type: &str,
        data_str: &str,
        content: &mut String,
        tool_calls: &mut Vec<AccumulatedToolCall>,
        item_id_map: &mut HashMap<String, usize>,
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

            "response.completed" => {
                return Ok(true);
            }

            "response.failed" => {
                let error_msg = data
                    .pointer("/response/error/message")
                    .or_else(|| data.pointer("/error/message"))
                    .and_then(|m| m.as_str())
                    .unwrap_or("Unknown error");
                let error_code = data
                    .pointer("/response/error/code")
                    .or_else(|| data.pointer("/error/code"))
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
    async fn chat_responses(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
        model: &str,
        system_prompt: Option<&str>,
        on_token: Option<TokenCallback>,
        reasoning_effort: Option<&str>,
    ) -> Result<LlmResponse, Box<dyn std::error::Error + Send + Sync>> {
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

            let resp = match self
                .streaming_client
                .post(OPENAI_RESPONSES_URL)
                .header("Authorization", format!("Bearer {}", self.api_key))
                .header("Content-Type", "application/json")
                .json(&body)
                .send()
                .await
            {
                Ok(r) => r,
                Err(e) => {
                    if attempt <= super::MAX_RETRIES {
                        let delay = super::retry_delay(attempt, 1);
                        super::log_retry(model, &format!("Network error: {:?}", e), attempt, delay);
                        tokio::time::sleep(delay).await;
                        continue;
                    }
                    return Err(super::with_retry_context(e, attempt).into());
                }
            };

            let status = resp.status();
            if !status.is_success() {
                let error_body = resp.text().await.unwrap_or_default();

                if super::is_retryable_status(status.as_u16()) && attempt <= super::MAX_RETRIES {
                    let delay = super::retry_delay(attempt, 1);
                    super::log_retry(model, &format!("HTTP {}", status), attempt, delay);
                    tokio::time::sleep(delay).await;
                    continue;
                }

                log!(
                    "[OpenAI] Responses API error ({}): {}",
                    status,
                    &error_body[..error_body.floor_char_boundary(500)]
                );
                return Err(super::with_retry_context(
                    format!("OpenAI Responses API error ({}): {}", status, error_body),
                    attempt,
                )
                .into());
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
                    if super::is_retryable_error(&err_str) && attempt <= super::MAX_RETRIES {
                        let delay = super::retry_delay(attempt, 2);
                        super::log_retry(
                            model,
                            &format!("Stream error: {}", err_str),
                            attempt,
                            delay,
                        );
                        tokio::time::sleep(delay).await;
                        continue;
                    }

                    return Err(super::with_retry_context(e, attempt).into());
                }
            }
        }
    }

    // ---------------------------------------------------------------
    // Shared helpers
    // ---------------------------------------------------------------

    /// Build an LlmResponse from accumulated text content and tool calls.
    fn build_llm_response(content: String, tool_call_map: Vec<AccumulatedToolCall>) -> LlmResponse {
        let final_content = if content.is_empty() {
            None
        } else {
            Some(content)
        };

        let tool_calls: Vec<ToolCall> = tool_call_map
            .into_iter()
            .map(|tc| {
                let arguments = if tc.arguments_json.is_empty() {
                    serde_json::json!({})
                } else {
                    serde_json::from_str(&tc.arguments_json).unwrap_or_else(|e| {
                        log!(
                            "[OpenAI] Failed to parse OpenAI tool arguments: {} (json: {})",
                            e,
                            tc.arguments_json
                        );
                        serde_json::json!({})
                    })
                };
                ToolCall {
                    id: tc.id,
                    name: tc.name,
                    arguments,
                }
            })
            .collect();

        LlmResponse {
            content: final_content,
            tool_calls,
            // TODO: capture finish_reason and usage from streamed chunks
            stop_reason: None,
            output_tokens: None,
            input_tokens: None,
            thinking_chars: None,
        }
    }
}

/// Intermediate state for accumulating a streamed tool call.
struct AccumulatedToolCall {
    id: String,
    name: String,
    arguments_json: String,
}

#[async_trait]
impl LlmProvider for OpenAiProvider {
    async fn chat(
        &self,
        messages: Vec<Message>,
        tools: Vec<ToolDefinition>,
        model_override: Option<&str>,
        system_prompt: Option<&str>,
        on_token: Option<TokenCallback>,
        reasoning_effort: Option<&str>,
    ) -> Result<LlmResponse, Box<dyn std::error::Error + Send + Sync>> {
        let model = model_override.unwrap_or(&self.model);

        if uses_responses_api(model) {
            self.chat_responses(
                &messages,
                &tools,
                model,
                system_prompt,
                on_token,
                reasoning_effort,
            )
            .await
        } else {
            self.chat_completions(
                &messages,
                &tools,
                model,
                system_prompt,
                on_token,
                reasoning_effort,
            )
            .await
        }
    }
}
