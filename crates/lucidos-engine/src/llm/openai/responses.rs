//! OpenAI Responses API path (GPT-5+ / codex models) for `OpenAiProvider`.
//! The struct, shared stream types, and dispatch live in the parent `openai`
//! module.

use crate::llm::chatgpt_plan::{ChatGptPlanError, ChatGptPlanFailure, TOOL_NAMESPACE};
use crate::llm::prefill::{pair_of, BeatShape, PrefillPair, StableEnd, TurnRoute};
use crate::llm::provider::{
    ContentBlock, LlmResponse, Message, MessageContent, TokenCallback, ToolDefinition,
};
use crate::llm::{CacheMode, ModelSelection};
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
    /// - `{"role": "user"|"assistant", "content": "..."}`, or typed
    ///   `input_text` / `input_image` parts when the message holds an image
    ///   or a cache mark
    /// - `{"type": "function_call", "id": "...", "call_id": "...", "name": "...", "arguments": "..."}`
    /// - `{"type": "function_call_output", "call_id": "...", "output": "..."}`
    ///
    /// `breakpoints` is how many marks the request may carry, or `None` for
    /// a model that takes none ([`super::takes_cache_breakpoints`]).
    fn convert_messages_responses(
        messages: &[Message],
        breakpoints: Option<usize>,
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
                    let mut pending = PendingMessage::default();
                    let marked_text = breakpoints.is_some()
                        && msg.role == "user"
                        && blocks
                            .iter()
                            .any(|b| matches!(b, ContentBlock::MemoryView { .. }));
                    if marked_text {
                        input.push(serde_json::json!({
                            "role": msg.role,
                            "content": marked_parts(
                                blocks,
                                &mut marked,
                                breakpoints.unwrap_or_default(),
                            ),
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
                            | ContentBlock::MemoryView { text }
                            | ContentBlock::StableContext { text } => {
                                pending.texts.push(text.clone());
                            }
                            ContentBlock::ToolUse {
                                id,
                                name,
                                input: tool_input,
                                ..
                            } => {
                                pending.flush(&mut input, "assistant");
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
                            ContentBlock::Image {
                                media_type, data, ..
                            } => {
                                pending.image(media_type, data);
                            }
                            ContentBlock::ToolResult {
                                tool_use_id,
                                content,
                            } => {
                                pending.flush(&mut input, &msg.role);
                                input.push(serde_json::json!({
                                    "type": "function_call_output",
                                    "call_id": tool_use_id,
                                    "output": content,
                                }));
                            }
                        }
                    }

                    pending.flush(&mut input, &msg.role);
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

    /// The Responses API `tool_choice` that makes the model call `tool`.
    fn forced_tool_choice_responses(tool: &str) -> serde_json::Value {
        serde_json::json!({ "type": "function", "name": tool })
    }

    /// The plan route's tool list: every function inside one namespace, as
    /// its docs require.
    fn namespaced_tools(functions: Vec<serde_json::Value>) -> Vec<serde_json::Value> {
        vec![serde_json::json!({
            "type": "namespace",
            "name": TOOL_NAMESPACE,
            "description": "Lucidos workspace tools.",
            "tools": functions,
        })]
    }

    /// `model` is the one sent, already resolved. The rest of the request's
    /// options come from `selection`.
    ///
    /// The plan endpoint sends its own profile (ADR 0636): no output cap, no
    /// tier, no cache marks, and tools in a namespace. Each field it drops is
    /// one the plan route refuses or the docs say to omit.
    ///
    /// A forced tool keeps its tools top-level, which the plan route accepts.
    /// A `tool_choice` has no namespace field, so it may not name a function
    /// inside one.
    fn build_responses_body(
        &self,
        model: &str,
        messages: &[Message],
        tools: &[ToolDefinition],
        system_prompt: Option<&str>,
        selection: &ModelSelection<'_>,
    ) -> serde_json::Value {
        let plan = self.plan_tokens().is_some();
        let namespaced = plan && selection.forced_tool.is_none();
        let breakpoints = (!plan && super::takes_cache_breakpoints(model)).then_some(
            match selection.cache_mode {
                CacheMode::Implicit => MAX_BREAKPOINTS,
                CacheMode::Explicit => MAX_EXPLICIT_BREAKPOINTS,
            },
        );
        let mut input = Self::convert_messages_responses(messages, breakpoints);
        if namespaced {
            // A replayed call names the namespace its function lives in.
            for item in input.iter_mut().filter(|i| i["type"] == "function_call") {
                item["namespace"] = TOOL_NAMESPACE.into();
            }
        }

        let mut body = serde_json::json!({
            "model": model,
            "stream": true,
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
        if !plan {
            body["max_output_tokens"] = DEFAULT_MAX_COMPLETION_TOKENS.into();
        }

        if let Some(instructions) = system_prompt {
            body["instructions"] = serde_json::Value::String(instructions.to_string());
        }

        if let Some(tool_defs) = Self::convert_tools_responses(tools) {
            let tool_defs = match namespaced {
                true => Self::namespaced_tools(tool_defs),
                false => tool_defs,
            };
            body["tools"] = serde_json::Value::Array(tool_defs);
        }

        // Verbatim, deliberately. Which tiers this model supports is decided
        // once, in `llm::reasoning`, and enforced by `RoutingProvider`'s clamp;
        // a second per-model rule here is what let the picker and the wire
        // disagree (see `llm/reasoning.rs`).
        if let Some(effort) = selection.reasoning_effort {
            body["reasoning"] = serde_json::json!({ "effort": effort });
        }

        if breakpoints.is_some() && selection.cache_mode == CacheMode::Explicit {
            body["prompt_cache_options"] = serde_json::json!({ "mode": "explicit" });
        }

        if let Some(tier) = selection.service_tier.filter(|_| !plan) {
            body["service_tier"] = serde_json::Value::String(tier.as_wire().to_string());
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
                    .map_err(|e| match e.downcast::<ChatGptPlanError>() {
                        Err(e) => crate::llm::stream_failure(
                            e.to_string(),
                            on_token.is_some() && prev_len > 0,
                            STREAM_LOG_TAG,
                        ),
                        // Kept typed, so the limit pause can read it.
                        Ok(plan) => plan,
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
                    if !delta.is_empty() {
                        crate::llm::call_trace::content_arrived();
                    }
                    content.push_str(delta);
                }
            }

            "response.output_item.added" => {
                if let Some(item) = data.get("item") {
                    if item.get("type").and_then(|t| t.as_str()) == Some("function_call") {
                        crate::llm::call_trace::content_arrived();
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
                let error_object = data
                    .pointer("/response/error")
                    .or_else(|| data.get("error"))
                    .unwrap_or(&data);
                if let Some(plan) = ChatGptPlanError::from_error_object(error_object) {
                    return Err(Box::new(plan));
                }
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
        let mut body = self.build_responses_body(model, messages, tools, system_prompt, &selection);
        if let Some(tool) = selection.forced_tool {
            body["tool_choice"] = Self::forced_tool_choice_responses(tool);
        }
        let prefill = prefill_pair(
            &body,
            TurnRoute {
                wire_model: model,
                reasoning_effort: selection.reasoning_effort,
                betas: &[],
            },
        );

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

        let mut bearer = match self.plan_bearer(model).await? {
            Some(plan) => Some(plan),
            None => self.bearer().await?,
        };
        let mut refreshed = false;
        let mut attempt = 0u32;
        loop {
            attempt += 1;

            let builder = self
                .apply_headers(
                    self.streaming_client.post(&self.responses_url),
                    bearer.as_deref(),
                )
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
                let request_id = crate::llm::call_trace::request_id_of(resp.headers());
                let error_body = resp.text().await.unwrap_or_default();
                let plan_error = self.plan_tokens().and_then(|_| {
                    ChatGptPlanError::from_http(status.as_u16(), &error_body, request_id.clone())
                });

                match http_failure_step(
                    self.plan_tokens().is_some(),
                    status.as_u16(),
                    plan_error.as_ref().map(|e| &e.failure),
                    refreshed,
                    attempt,
                ) {
                    HttpFailureStep::Refresh => {
                        if let (Some(tokens), Some(rejected)) = (self.plan_tokens(), &bearer) {
                            log!("[OpenAI] ChatGPT plan rejected its token ({status}); refreshing once");
                            bearer = Some(tokens.after_rejection(rejected).await?);
                            refreshed = true;
                            continue;
                        }
                    }
                    HttpFailureStep::Backoff => {
                        let delay = crate::llm::retry_delay(attempt, 1);
                        crate::llm::log_retry(model, &format!("HTTP {}", status), attempt, delay);
                        tokio::time::sleep(delay).await;
                        continue;
                    }
                    HttpFailureStep::Fail => {}
                }

                if let Some(plan_error) = plan_error {
                    self.observe_plan_failure(&plan_error).await;
                    return Err(Box::new(plan_error));
                }
                log!(
                    "[OpenAI] Responses API error ({}, request_id={}): {}",
                    status,
                    request_id.as_deref().unwrap_or("none"),
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
                    return Ok(LlmResponse {
                        prefill,
                        ..response
                    });
                }
                Err(e) => {
                    // A mid-stream plan failure may follow rendered text, so it
                    // ends the turn rather than re-sending.
                    if let Some(plan_error) = e.downcast_ref::<ChatGptPlanError>() {
                        self.observe_plan_failure(plan_error).await;
                        return Err(e);
                    }
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

impl OpenAiProvider {
    /// Log a plan failure, and pause plan requests when it is the limit.
    async fn observe_plan_failure(&self, error: &ChatGptPlanError) {
        log!("[OpenAI] ChatGPT plan error: {}", error.log_line());
        if let Some(access) = self.plan_access() {
            access.pause.observe(error).await;
        }
    }
}

/// What the retry loop does after an HTTP error status.
#[derive(Debug, PartialEq, Eq)]
enum HttpFailureStep {
    /// Refresh the plan token, then send again.
    Refresh,
    /// Back off, then send again.
    Backoff,
    Fail,
}

/// The step after an HTTP error. `plan_failure` is the classified body on the
/// plan endpoint.
///
/// The platform keeps its old rule. The plan route never retries a 429 in
/// place: its docs say to pause new plan requests instead. A 401 there gets
/// one refresh, since an expired or revoked token is the usual cause.
fn http_failure_step(
    plan: bool,
    status: u16,
    plan_failure: Option<&ChatGptPlanFailure>,
    refreshed: bool,
    attempt: u32,
) -> HttpFailureStep {
    let attempts_left = attempt <= crate::llm::MAX_RETRIES;
    if !plan {
        return match crate::llm::is_retryable_status(status) && attempts_left {
            true => HttpFailureStep::Backoff,
            false => HttpFailureStep::Fail,
        };
    }
    let rejected_token = status == 401 || plan_failure == Some(&ChatGptPlanFailure::InvalidUser);
    match plan_failure {
        _ if rejected_token && !refreshed => HttpFailureStep::Refresh,
        Some(failure) if failure.is_retryable() && attempts_left => HttpFailureStep::Backoff,
        None if status != 429 && crate::llm::is_retryable_status(status) && attempts_left => {
            HttpFailureStep::Backoff
        }
        _ => HttpFailureStep::Fail,
    }
}

/// Explicit breakpoints a request may carry. A request writes the cache at
/// most four times, and the default mode spends one at the end of the latest
/// message.
pub(crate) const MAX_BREAKPOINTS: usize = 3;

/// Breakpoints a request in explicit mode may carry: all four writes, since
/// nothing writes at the end.
pub(crate) const MAX_EXPLICIT_BREAKPOINTS: usize = 4;

/// The most output a Responses prefill allows: the API's minimum
/// `max_output_tokens`. Probe P5 settled that a beat capped there reads.
const RESPONSES_OUTPUT_CAP: u32 = 16;

/// The pair a beat on a Responses request sends. The stable cut ends at the
/// last part that carries a breakpoint, where the turn wrote an entry.
///
/// `None` when no part carries one: a model before GPT-5.6, or a request with
/// no memory view. A cut there would make the beat write, not read.
fn prefill_pair(body: &serde_json::Value, route: TurnRoute<'_>) -> Option<PrefillPair> {
    let input = body.get("input")?.as_array()?;
    let end = input.iter().enumerate().rev().find_map(|(item, value)| {
        let part = value
            .get("content")?
            .as_array()?
            .iter()
            .rposition(|part| part.get("prompt_cache_breakpoint").is_some())?;
        Some(StableEnd::Part { item, part })
    })?;
    let mut value = body.clone();
    value["max_output_tokens"] = RESPONSES_OUTPUT_CAP.into();
    value["stream"] = false.into();
    pair_of(
        &value,
        |v: &mut serde_json::Value| v.get_mut("input")?.as_array_mut(),
        end,
        BeatShape::json_reply(RESPONSES_OUTPUT_CAP),
        route,
    )
}

/// A user message holding memory view blocks, as typed text parts. Each
/// memory view block takes a breakpoint, so a later request that marks the
/// same place reads the cache up to there. A read needs a breakpoint in the
/// very same place, which is why the view marks fixed places.
///
/// The text parts read as the joined string would, with a newline between two
/// parts only where the first does not already end in one. An image keeps its
/// place among them and takes no breakpoint.
fn marked_parts(
    blocks: &[ContentBlock],
    breakpoints: &mut usize,
    limit: usize,
) -> Vec<serde_json::Value> {
    let last_text = blocks
        .iter()
        .rposition(|block| marked_text(block).is_some());
    blocks
        .iter()
        .enumerate()
        .filter_map(|(i, block)| {
            if let ContentBlock::Image {
                media_type, data, ..
            } = block
            {
                return Some(input_image(media_type, data));
            }
            let (text, marked) = marked_text(block)?;
            let mut text = text.to_string();
            if Some(i) != last_text && !text.ends_with('\n') {
                text.push('\n');
            }
            let mut part = serde_json::json!({"type": "input_text", "text": text});
            if marked && *breakpoints < limit {
                *breakpoints += 1;
                part["prompt_cache_breakpoint"] = serde_json::json!({"mode": "explicit"});
            }
            Some(part)
        })
        .collect()
}

/// A block's non-empty text, and whether it is a memory view block.
fn marked_text(block: &ContentBlock) -> Option<(&str, bool)> {
    let (text, marked) = match block {
        ContentBlock::MemoryView { text } => (text, true),
        ContentBlock::Text { text }
        | ContentBlock::EngineTail { text }
        | ContentBlock::StableContext { text } => (text, false),
        _ => return None,
    };
    (!text.is_empty()).then_some((text.as_str(), marked))
}

/// An image as a Responses `input_image` part, inlined as a data URL.
fn input_image(media_type: &str, data: &str) -> serde_json::Value {
    serde_json::json!({
        "type": "input_image",
        "image_url": format!("data:{media_type};base64,{data}"),
        "detail": "auto",
    })
}

/// The text and images of one message item, gathered in block order. A label
/// between two images must stay between them.
#[derive(Default)]
struct PendingMessage {
    /// Adjacent text not yet folded into `parts`.
    texts: Vec<String>,
    /// Typed parts so far. Stays empty for a message with no image.
    parts: Vec<serde_json::Value>,
}

impl PendingMessage {
    fn image(&mut self, media_type: &str, data: &str) {
        self.fold_text();
        self.parts.push(input_image(media_type, data));
    }

    /// Fold the adjacent text into one `input_text` part.
    fn fold_text(&mut self) {
        let text = self.texts.join("\n");
        self.texts.clear();
        if !text.is_empty() {
            self.parts
                .push(serde_json::json!({"type": "input_text", "text": text}));
        }
    }

    /// Push what was gathered as one message item, and start over. Text alone
    /// stays one joined string.
    fn flush(&mut self, input: &mut Vec<serde_json::Value>, role: &str) {
        let content = if !self.parts.is_empty() {
            self.fold_text();
            serde_json::Value::Array(std::mem::take(&mut self.parts))
        } else if !self.texts.is_empty() {
            let text = self.texts.join("\n");
            self.texts.clear();
            serde_json::Value::String(text)
        } else {
            return;
        };
        input.push(serde_json::json!({ "role": role, "content": content }));
    }
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

    fn image(data: &str) -> ContentBlock {
        ContentBlock::Image {
            source_type: "base64".to_string(),
            media_type: "image/png".to_string(),
            data: data.to_string(),
        }
    }

    fn input_images(item: &serde_json::Value) -> Vec<&serde_json::Value> {
        item["content"]
            .as_array()
            .map(|parts| {
                parts
                    .iter()
                    .filter(|p| p["type"] == "input_image")
                    .collect()
            })
            .unwrap_or_default()
    }

    /// An attached image reaches the model as an `input_image` part after the
    /// text. Which models read images is the vision flag's call, not this
    /// wire's.
    #[test]
    fn an_attached_image_travels_as_an_input_image_part() {
        let input = OpenAiProvider::convert_messages_responses(
            &[user(vec![text("describe this"), image("AAAA")])],
            None,
        );
        assert_eq!(input.len(), 1);
        let parts = input[0]["content"].as_array().expect("typed parts");
        assert_eq!(parts[0]["type"], "input_text");
        assert_eq!(parts[0]["text"], "describe this");
        assert_eq!(
            input_images(&input[0]),
            [&serde_json::json!({
                "type": "input_image",
                "image_url": "data:image/png;base64,AAAA",
                "detail": "auto",
            })]
        );
    }

    /// A tool's image rides after its function call output, in a user
    /// message, so the output still directly follows its call.
    #[test]
    fn a_tool_image_follows_the_function_call_output() {
        let input = OpenAiProvider::convert_messages_responses(
            &[user(vec![
                ContentBlock::ToolResult {
                    tool_use_id: "call_1".to_string(),
                    content: "screenshot taken".to_string(),
                },
                image("BBBB"),
                text("continue"),
            ])],
            None,
        );
        assert_eq!(input.len(), 2);
        assert_eq!(input[0]["type"], "function_call_output");
        assert_eq!(input[1]["role"], "user");
        let images = input_images(&input[1]);
        assert_eq!(images.len(), 1);
        assert_eq!(images[0]["image_url"], "data:image/png;base64,BBBB");
    }

    /// A message marked for the cache keeps its image, and the image takes no
    /// breakpoint.
    #[test]
    fn a_marked_message_keeps_its_image_unmarked() {
        let input = OpenAiProvider::convert_messages_responses(
            &[user(vec![view("<chat>\na\n"), text("look"), image("CCCC")])],
            Some(MAX_BREAKPOINTS),
        );
        let images = input_images(&input[0]);
        assert_eq!(images.len(), 1);
        assert!(images[0].get("prompt_cache_breakpoint").is_none());
        let parts = input[0]["content"].as_array().unwrap();
        assert_eq!(parts.last().unwrap()["type"], "input_image");
    }

    /// A label between two images stays between them, on both paths, so the
    /// "[Below: image attached to current message]" label names the right one.
    #[test]
    fn a_label_between_images_keeps_its_place() {
        let blocks = || {
            vec![
                text("hi"),
                image("OLD"),
                text("[Below: current]"),
                image("NEW"),
            ]
        };
        let shape = |item: &serde_json::Value| -> Vec<String> {
            item["content"]
                .as_array()
                .expect("typed parts")
                .iter()
                .map(|p| {
                    p.get("text")
                        .or_else(|| p.get("image_url"))
                        .and_then(|v| v.as_str())
                        .unwrap()
                        .to_string()
                })
                .collect()
        };
        let plain = OpenAiProvider::convert_messages_responses(&[user(blocks())], None);
        assert_eq!(
            shape(&plain[0]),
            [
                "hi",
                "data:image/png;base64,OLD",
                "[Below: current]",
                "data:image/png;base64,NEW",
            ]
        );
        let mut with_view = blocks();
        with_view.insert(0, view("<chat>\na\n"));
        let marked =
            OpenAiProvider::convert_messages_responses(&[user(with_view)], Some(MAX_BREAKPOINTS));
        assert_eq!(
            shape(&marked[0]),
            [
                "<chat>\na\n",
                "hi\n",
                "data:image/png;base64,OLD",
                "[Below: current]",
                "data:image/png;base64,NEW",
            ]
        );
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
            Some(MAX_BREAKPOINTS),
        );
        let parts = input[0]["content"].as_array().expect("typed parts");
        assert_eq!(parts.len(), 3);
        assert_eq!(parts[0]["prompt_cache_breakpoint"]["mode"], "explicit");
        assert!(parts[1].get("prompt_cache_breakpoint").is_none());
        assert!(parts[2].get("prompt_cache_breakpoint").is_none());
        let joined: String = parts.iter().map(|p| p["text"].as_str().unwrap()).collect();
        assert_eq!(joined, "<chat>\na\nb\nc\n</chat>\nthe task");
    }

    /// The stable context block is plain text here, with or without
    /// breakpoints: this wire never marks it (I8).
    #[test]
    fn a_stable_context_block_is_plain_text_on_this_wire() {
        let stable = || ContentBlock::StableContext {
            text: "profile".to_string(),
        };
        let marked = OpenAiProvider::convert_messages_responses(
            &[user(vec![view("<chat>\na\n"), stable(), text("the task")])],
            Some(MAX_BREAKPOINTS),
        );
        let parts = marked[0]["content"].as_array().expect("typed parts");
        assert_eq!(parts[1]["text"], "profile\n");
        assert!(parts[1].get("prompt_cache_breakpoint").is_none());
        let joined = OpenAiProvider::convert_messages_responses(
            &[user(vec![stable(), text("the task")])],
            Some(MAX_BREAKPOINTS),
        );
        assert_eq!(joined[0]["content"], "profile\nthe task");
    }

    /// A message with no memory view block, which is every Classic message,
    /// keeps its one joined string.
    #[test]
    fn a_message_with_no_memory_view_block_stays_one_string() {
        let input = OpenAiProvider::convert_messages_responses(
            &[user(vec![text("a"), text("b")])],
            Some(MAX_BREAKPOINTS),
        );
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
            None,
        );
        assert_eq!(input[0]["content"], "<chat>\na\n\nthe task");
    }

    /// Breakpoints stop at three, since the default mode writes at the end of
    /// the latest message too and a request allows four writes.
    #[test]
    fn a_request_carries_at_most_three_breakpoints() {
        let blocks = (0..5).map(|i| view(&format!("block {i}\n"))).collect();
        let input =
            OpenAiProvider::convert_messages_responses(&[user(blocks)], Some(MAX_BREAKPOINTS));
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

        let body = provider.build_responses_body(
            "gpt-5.5",
            &messages,
            &[],
            None,
            &ModelSelection::default(),
        );

        assert_eq!(body["store"], false);
    }

    /// Explicit mode turns off the write at the end and frees its slot for a
    /// fourth mark. A turn keeps implicit mode, since its next request reads
    /// what that end write stored.
    #[test]
    fn explicit_mode_writes_only_at_the_marks() {
        let provider = OpenAiProvider::new("k".to_string(), "gpt-6.1-sol".to_string()).unwrap();
        let blocks: Vec<ContentBlock> = (0..6).map(|i| view(&format!("block {i}\n"))).collect();
        let messages = [user(blocks)];
        let marks = |body: &serde_json::Value| {
            body["input"][0]["content"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|p| p.get("prompt_cache_breakpoint").is_some())
                .count()
        };

        let turn = provider.build_responses_body(
            "gpt-6.1-sol",
            &messages,
            &[],
            None,
            &ModelSelection::default(),
        );
        assert!(turn.get("prompt_cache_options").is_none());
        assert_eq!(marks(&turn), MAX_BREAKPOINTS);

        let explicit = ModelSelection::default().with_cache_mode(CacheMode::Explicit);
        let body = provider.build_responses_body("gpt-6.1-sol", &messages, &[], None, &explicit);
        assert_eq!(body["prompt_cache_options"]["mode"], "explicit");
        assert_eq!(marks(&body), MAX_EXPLICIT_BREAKPOINTS);

        let old = provider.build_responses_body("gpt-5.5", &messages, &[], None, &explicit);
        assert!(
            old.get("prompt_cache_options").is_none(),
            "a model before GPT-5.6 refuses cache options"
        );
    }

    /// Flex goes on the body only when the selection asks for it, so every
    /// other call keeps OpenAI's standard tier.
    #[test]
    fn responses_body_asks_for_flex_only_when_selected() {
        let provider = OpenAiProvider::new("k".to_string(), "gpt-6.1-sol".to_string()).unwrap();
        let standard = provider.build_responses_body(
            "gpt-6.1-sol",
            &[],
            &[],
            None,
            &ModelSelection::default(),
        );
        assert!(standard.get("service_tier").is_none());
        let flex = ModelSelection::default().with_service_tier(Some(crate::llm::ServiceTier::Flex));
        let body = provider.build_responses_body("gpt-6.1-sol", &[], &[], None, &flex);
        assert_eq!(body["service_tier"], "flex");
    }

    /// The completed response names the tier that served it, which the
    /// capture records so the call is priced at that tier's rate.
    #[test]
    fn the_served_tier_is_read_from_the_completed_response() {
        let mut content = String::new();
        let mut tools: Vec<AccumulatedToolCall> = Vec::new();
        let mut item_id_map = std::collections::HashMap::new();
        let mut meta = StreamMeta::default();
        OpenAiProvider::process_responses_chunk(
            "response.completed",
            r#"{"response":{"status":"completed","service_tier":"flex","usage":{"input_tokens":10,"output_tokens":2}}}"#,
            &mut content,
            &mut tools,
            &mut item_id_map,
            &mut meta,
        )
        .unwrap();
        assert_eq!(meta.service_tier, Some(crate::llm::ServiceTier::Flex));

        let mut meta = StreamMeta::default();
        OpenAiProvider::process_responses_chunk(
            "response.completed",
            r#"{"response":{"status":"completed","service_tier":"default","usage":{"input_tokens":10,"output_tokens":2}}}"#,
            &mut content,
            &mut tools,
            &mut item_id_map,
            &mut meta,
        )
        .unwrap();
        assert_eq!(meta.service_tier, None, "the standard tier records nothing");
    }

    /// The Responses API names the function at the top level, unlike Chat
    /// Completions.
    #[test]
    fn a_forced_call_names_its_function() {
        assert_eq!(
            OpenAiProvider::forced_tool_choice_responses("request_read"),
            serde_json::json!({ "type": "function", "name": "request_read" })
        );
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
                let selection = ModelSelection::default().with_effort(Some(effort));
                let body = provider.build_responses_body(model, &[], &[], None, &selection);
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
        assert_eq!(meta.usage.map(|u| u.input_tokens), Some(11));
        assert_eq!(meta.usage.map(|u| u.output_tokens), Some(3));
        assert_eq!(meta.usage.map(|u| u.cache_read_tokens), Some(4));
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
        assert_eq!(meta.usage.map(|u| u.input_tokens), Some(42));
        assert_eq!(meta.usage.map(|u| u.cache_read_tokens), Some(12));
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
        assert_eq!(meta.usage.map(|u| u.input_tokens), Some(5476));
        assert_eq!(meta.usage.map(|u| u.cache_creation_tokens), Some(5435));
        assert_eq!(meta.usage.map(|u| u.cache_read_tokens), Some(0));
    }

    /// The `response.completed` data of one live turn, with `usage` details
    /// and the response's `prompt_cache_options` echo.
    fn completed_response(details: &str, echo: &str) -> LlmResponse {
        let mut content = String::new();
        let mut tools: Vec<AccumulatedToolCall> = Vec::new();
        let mut item_id_map = std::collections::HashMap::new();
        let mut meta = StreamMeta::default();
        let data = format!(
            r#"{{"response":{{"status":"completed",{echo}"usage":{{"input_tokens":16500,"output_tokens":9,"input_tokens_details":{details}}}}}}}"#
        );
        OpenAiProvider::process_responses_chunk(
            "response.completed",
            &data,
            &mut content,
            &mut tools,
            &mut item_id_map,
            &mut meta,
        )
        .unwrap();
        OpenAiProvider::build_llm_response("ok".to_string(), tools, meta).unwrap()
    }

    /// Probe A1 and A3: the write splits by lifetime, and the TTL is reported.
    #[test]
    fn a_responses_write_split_by_lifetime_is_reported() {
        let response = completed_response(
            r#"{"cached_tokens":0,"cache_write_tokens":16439,"cache_write_tokens_details":{"ttl_30m_tokens":16439,"ttl_12h_tokens":0}}"#,
            r#""prompt_cache_options":{"mode":"implicit","ttl":"30m"},"#,
        );
        assert_eq!(response.usage.map(|u| u.cache_creation_tokens), Some(16439));
        assert_eq!(
            response.usage.and_then(|u| u.cache_creation_30m_tokens),
            Some(16439)
        );
        assert_eq!(
            response.usage.and_then(|u| u.cache_creation_12h_tokens),
            Some(0)
        );
    }

    /// With no split, the echoed TTL names the whole write's lifetime.
    #[test]
    fn the_echoed_ttl_names_the_lifetime_of_a_write_with_no_split() {
        let response = completed_response(
            r#"{"cached_tokens":0,"cache_write_tokens":16007}"#,
            r#""prompt_cache_options":{"mode":"explicit","ttl":"12h"},"#,
        );
        assert_eq!(
            response.usage.and_then(|u| u.cache_creation_30m_tokens),
            None
        );
        assert_eq!(
            response.usage.and_then(|u| u.cache_creation_12h_tokens),
            Some(16007)
        );
    }

    /// A split whose parts each fit but together claim more than the write is
    /// dropped, echo or not, so nothing records tokens nobody wrote.
    #[test]
    fn an_over_summed_split_is_dropped() {
        let response = completed_response(
            r#"{"cached_tokens":0,"cache_write_tokens":1000,"cache_write_tokens_details":{"ttl_30m_tokens":900,"ttl_12h_tokens":900}}"#,
            r#""prompt_cache_options":{"mode":"implicit","ttl":"30m"},"#,
        );
        assert_eq!(response.usage.map(|u| u.cache_creation_tokens), Some(1000));
        assert_eq!(
            response.usage.and_then(|u| u.cache_creation_30m_tokens),
            None
        );
        assert_eq!(
            response.usage.and_then(|u| u.cache_creation_12h_tokens),
            None
        );
    }

    /// A write with neither a split nor an echo has no known TTL.
    #[test]
    fn a_responses_write_with_no_split_and_no_echo_is_unknown() {
        let response = completed_response(r#"{"cached_tokens":0,"cache_write_tokens":880}"#, "");
        assert_eq!(
            response.usage.and_then(|u| u.cache_creation_30m_tokens),
            None
        );
        assert_eq!(
            response.usage.and_then(|u| u.cache_creation_12h_tokens),
            None
        );
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
        assert_eq!(meta.usage.map(|u| u.input_tokens), Some(42));
        assert_eq!(meta.usage.map(|u| u.cache_read_tokens), Some(0));
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
        assert_eq!(meta.usage.map(|u| u.input_tokens), Some(50));
        assert_eq!(meta.usage.map(|u| u.output_tokens), Some(4096));
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

    // --- Prefill beats (ADR 0556) ---

    const SOL: &str = "gpt-6.1-sol";

    fn route(model: &str) -> TurnRoute<'_> {
        TurnRoute {
            wire_model: model,
            reasoning_effort: Some("medium"),
            betas: &[],
        }
    }

    fn json(bytes: &[u8]) -> serde_json::Value {
        serde_json::from_slice(bytes).expect("a body is JSON")
    }

    /// A Tree turn: two memory view blocks and the task, an answer, then a
    /// follow-up with no view.
    fn tree_turn() -> Vec<Message> {
        vec![
            user(vec![
                view("[THREAD MEMORY VIEW]\n[0+8] old\n"),
                view("[8+4] newer\n"),
                text("the task"),
            ]),
            Message {
                role: "assistant".to_string(),
                content: MessageContent::Text("an answer".to_string()),
            },
            Message {
                role: "user".to_string(),
                content: MessageContent::Text("and then?".to_string()),
            },
        ]
    }

    fn turn_body(model: &str, messages: &[Message]) -> serde_json::Value {
        let provider = OpenAiProvider::new("k".to_string(), model.to_string()).unwrap();
        let selection = ModelSelection::default().with_effort(Some("medium"));
        let mut body =
            provider.build_responses_body(model, messages, &[], Some("system"), &selection);
        body["tool_choice"] = OpenAiProvider::forced_tool_choice_responses("request_read");
        body
    }

    /// The whole cut is the turn capped at the API's minimum output and not
    /// streamed, with its reasoning and tool choice. The stable cut ends at
    /// the last breakpoint.
    #[test]
    fn a_responses_pair_caps_output_and_cuts_at_the_last_breakpoint() {
        let body = turn_body(SOL, &tree_turn());
        let pair = prefill_pair(&body, route(SOL)).expect("a marked request beats");

        let mut whole = body.clone();
        whole["max_output_tokens"] = RESPONSES_OUTPUT_CAP.into();
        whole["stream"] = false.into();
        assert_eq!(json(&pair.whole.body), whole);
        assert_eq!(whole["reasoning"]["effort"], "medium");
        assert_eq!(whole["tool_choice"]["name"], "request_read");

        let mut stable = whole;
        let input = stable["input"].as_array_mut().unwrap();
        input.truncate(1);
        input[0]["content"].as_array_mut().unwrap().truncate(2);
        assert_eq!(json(&pair.stable.body), stable);
        let last = &stable["input"][0]["content"][1];
        assert!(last.get("prompt_cache_breakpoint").is_some(), "{last}");

        for body in [&pair.stable, &pair.whole] {
            assert_eq!(body.output_cap, Some(RESPONSES_OUTPUT_CAP));
            assert_eq!(body.wire_model, SOL);
            assert_eq!(body.reasoning_effort.as_deref(), Some("medium"));
        }
    }

    /// The beat writes the bytes the turn writes: the same serializer over
    /// the same body, edited only where a beat differs (ADR 0581).
    #[test]
    fn a_responses_beat_keeps_the_turns_bytes() {
        let body = turn_body(SOL, &tree_turn());
        let pair = prefill_pair(&body, route(SOL)).expect("a marked request beats");

        let mut whole = body.clone();
        whole["max_output_tokens"] = RESPONSES_OUTPUT_CAP.into();
        whole["stream"] = false.into();
        assert_eq!(&*pair.whole.body, serde_json::to_vec(&whole).unwrap());

        let input = whole["input"].as_array_mut().unwrap();
        input.truncate(1);
        input[0]["content"].as_array_mut().unwrap().truncate(2);
        assert_eq!(&*pair.stable.body, serde_json::to_vec(&whole).unwrap());
    }

    /// No breakpoint means no entry a beat could read: a model before
    /// GPT-5.6 takes none, and a Classic turn has no memory view.
    #[test]
    fn a_request_with_no_breakpoint_gets_no_pair() {
        assert!(prefill_pair(&turn_body("gpt-5.5", &tree_turn()), route("gpt-5.5")).is_none());
        let classic = [user(vec![text("profile"), text("the task")])];
        assert!(prefill_pair(&turn_body(SOL, &classic), route(SOL)).is_none());
    }

    const COMPLETED_TURN: &str = concat!(
        "event: response.output_text.delta\n",
        r#"data: {"delta":"done"}"#,
        "\n\n",
        "event: response.completed\n",
        r#"data: {"response":{"status":"completed","usage":{"input_tokens":900,"output_tokens":3,"input_tokens_details":{"cached_tokens":0,"cache_write_tokens":880}}}}"#,
        "\n\n",
    );

    const PREFILL_REPLY: &str = r#"{"model":"gpt-6.1-sol","status":"incomplete","usage":{"input_tokens":900,"output_tokens":16,"input_tokens_details":{"cached_tokens":880}}}"#;

    /// A turn hands back its pair, and the beat posts the stable cut to the
    /// Responses endpoint and reads the reply's usage.
    #[tokio::test]
    async fn a_turn_returns_a_pair_whose_beat_reads_the_turn() {
        let (base, bodies) =
            super::super::serve_recording("/v1/responses", COMPLETED_TURN, PREFILL_REPLY).await;
        let provider =
            OpenAiProvider::new_with_base_url("k".into(), SOL.into(), &base, Vec::new(), false)
                .unwrap();
        let turn = provider
            .chat_responses(
                &tree_turn(),
                &[],
                ModelSelection::model(SOL),
                Some("system"),
                None,
            )
            .await
            .expect("the turn completes");
        let pair = turn
            .prefill
            .expect("a marked GPT-6.1 turn can be prefilled");

        let beat = crate::llm::LlmProvider::prefill(
            &provider,
            &pair.stable,
            crate::llm::metered::CallToken::for_test(),
        )
        .await
        .expect("the beat answers");
        assert_eq!(beat.usage.map(|u| u.cache_read_tokens), Some(880));
        assert_eq!(beat.usage.map(|u| u.output_tokens), Some(16));

        let sent = bodies.lock().unwrap().clone();
        let [_, beat_body] = sent.as_slice() else {
            panic!("expected a turn and a beat, saw {}", sent.len());
        };
        assert_eq!(beat_body, &json(&pair.stable.body));
    }

    /// A history that touches every item kind the Responses wire writes.
    fn every_item_kind() -> Vec<Message> {
        vec![
            user(vec![
                view("[THREAD MEMORY VIEW]\n[0+8] old\n"),
                text("look at this"),
                image("AAAA"),
            ]),
            Message {
                role: "assistant".to_string(),
                content: MessageContent::Blocks(vec![
                    text("checking"),
                    ContentBlock::ToolUse {
                        id: "call_1".to_string(),
                        name: "list_files".to_string(),
                        input: serde_json::json!({"path": "artifacts"}),
                        thought_signature: None,
                    },
                ]),
            },
            user(vec![ContentBlock::ToolResult {
                tool_use_id: "call_1".to_string(),
                content: "a.md".to_string(),
            }]),
        ]
    }

    fn two_tools() -> Vec<ToolDefinition> {
        ["list_files", "request_read"]
            .into_iter()
            .map(|name| ToolDefinition {
                name: name.to_string(),
                description: format!("{name} does a thing"),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {"path": {"type": "string"}},
                }),
            })
            .collect()
    }

    /// A Responses server that records each request's raw bytes and answers
    /// with one completed turn. Returns the base URL a provider takes.
    async fn serve_raw() -> (String, std::sync::Arc<std::sync::Mutex<Vec<String>>>) {
        let bodies = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let record = bodies.clone();
        let app = axum::Router::new().route(
            "/v1/responses",
            axum::routing::post(move |body: String| {
                record.lock().unwrap().push(body);
                async { ([("content-type", "text/event-stream")], COMPLETED_TURN) }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (format!("http://{addr}/v1"), bodies)
    }

    /// The platform body is byte for byte what the build before the ChatGPT
    /// plan endpoint sent: a turn with a forced tool and flex, and an
    /// explicit-mode call. The fixture was written by that build, so
    /// regenerating it to pass is the failure.
    #[tokio::test]
    async fn the_platform_body_is_byte_identical_to_the_pre_plan_build() {
        let (base, bodies) = serve_raw().await;
        let provider =
            OpenAiProvider::new_with_base_url("k".into(), SOL.into(), &base, Vec::new(), false)
                .unwrap();
        let turn = ModelSelection::model(SOL)
            .with_effort(Some("high"))
            .with_forced_tool(Some("request_read"))
            .with_service_tier(Some(crate::llm::ServiceTier::Flex));
        let explicit = ModelSelection::model(SOL).with_cache_mode(CacheMode::Explicit);
        for selection in [turn, explicit] {
            provider
                .chat_responses(
                    &every_item_kind(),
                    &two_tools(),
                    selection,
                    Some("system"),
                    None,
                )
                .await
                .expect("the turn completes");
        }
        let sent: Vec<serde_json::Value> = bodies
            .lock()
            .unwrap()
            .iter()
            .map(|raw| serde_json::from_str(raw).unwrap())
            .collect();
        let raw = bodies.lock().unwrap().clone();
        for (raw, value) in raw.iter().zip(&sent) {
            assert_eq!(raw, &serde_json::to_string(value).unwrap(), "sorted keys");
        }
        let snapshot = serde_json::to_string_pretty(&sent).unwrap();
        assert_eq!(snapshot, include_str!("responses_platform_body.json"));
    }

    // --- The ChatGPT plan endpoint (ADR 0636) ---

    /// A token source that hands out `plan-token-1`, and `plan-token-2` after
    /// a rejection.
    #[derive(Default)]
    struct FakeTokens {
        rejections: std::sync::Mutex<Vec<String>>,
    }

    #[async_trait::async_trait]
    impl crate::llm::chatgpt_plan::ChatGptPlanTokens for FakeTokens {
        async fn access_token(&self) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
            Ok("plan-token-1".to_string())
        }

        async fn after_rejection(
            &self,
            rejected: &str,
        ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
            self.rejections.lock().unwrap().push(rejected.to_string());
            Ok("plan-token-2".to_string())
        }
    }

    /// Fields the plan route refuses or its docs say to omit, plus the two
    /// Lucidos sends on the platform that the probes saw refused. A pinned
    /// wire contract: the literals are the docs' list, less `max_tool_calls`.
    /// That one also names a preference, which ADR 0368 keeps to the catalog,
    /// and no route sends it.
    const PLAN_FORBIDDEN_FIELDS: &[&str] = &[
        "background",
        "conversation",
        "max_output_tokens",
        "metadata",
        "moderation",
        "multi_agent",
        "prompt",
        "prompt_cache_retention",
        "safety_identifier",
        "temperature",
        "top_logprobs",
        "top_p",
        "truncation",
        "user",
        "previous_response_id",
        "service_tier",
        "prompt_cache_options",
    ];

    /// The `unsupported_capability` answer a strict mock gives a body the
    /// plan route would refuse, or `None` for a body it accepts.
    fn plan_refusal(body: &serde_json::Value) -> Option<String> {
        let refused = PLAN_FORBIDDEN_FIELDS
            .iter()
            .find(|field| body.get(**field).is_some())
            .map(|field| field.to_string())
            .or_else(|| {
                body.to_string()
                    .contains("prompt_cache_breakpoint")
                    .then(|| "prompt_cache_breakpoint".to_string())
            })
            .or_else(|| {
                (body["store"] != false || body["stream"] != true).then(|| "store".to_string())
            })?;
        Some(format!(
            r#"{{"error":{{"message":"unsupported","type":"invalid_request_error","param":"{refused}","code":"subscription_sharing_unsupported_capability"}}}}"#
        ))
    }

    type PlanRequests = std::sync::Arc<std::sync::Mutex<Vec<(String, serde_json::Value)>>>;

    /// The plan's model listing, in its own shape: `SOL` listed, one hidden.
    const PLAN_MODELS: &str = r#"{"models":[{"slug":"gpt-6.1-sol","visibility":"list"},{"slug":"gpt-reserve","visibility":"hide"}]}"#;

    /// A plan Responses mock. `answer` gets the request's index and returns a
    /// status and body; a strict check answers first. Records each request's
    /// `Authorization` header and body.
    async fn serve_plan(answer: fn(usize) -> (u16, &'static str)) -> (String, PlanRequests) {
        let requests = PlanRequests::default();
        let record = requests.clone();
        let app = axum::Router::new()
            .route("/v1/models", axum::routing::get(|| async { PLAN_MODELS }))
            .route(
                "/v1/responses",
                axum::routing::post(move |headers: axum::http::HeaderMap, body: String| {
                    let body: serde_json::Value = serde_json::from_str(&body).unwrap();
                    let auth = headers
                        .get("authorization")
                        .and_then(|v| v.to_str().ok())
                        .unwrap_or_default()
                        .to_string();
                    let index = {
                        let mut record = record.lock().unwrap();
                        record.push((auth, body.clone()));
                        record.len() - 1
                    };
                    async move {
                        let (status, reply) = match plan_refusal(&body) {
                            Some(refusal) => (400, refusal),
                            None => {
                                let (status, reply) = answer(index);
                                (status, reply.to_string())
                            }
                        };
                        let kind = if status == 200 {
                            "text/event-stream"
                        } else {
                            "application/json"
                        };
                        (
                            axum::http::StatusCode::from_u16(status).unwrap(),
                            [("content-type", kind), ("x-request-id", "req_mock")],
                            reply,
                        )
                    }
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (format!("http://{addr}/v1"), requests)
    }

    fn plan_provider(base: &str, tokens: std::sync::Arc<FakeTokens>) -> OpenAiProvider {
        let access = crate::llm::chatgpt_plan::ChatGptPlanAccess {
            tokens,
            pause: std::sync::Arc::new(crate::llm::chatgpt_plan::LimitPause::unannounced()),
        };
        OpenAiProvider::new_chatgpt_plan(access, SOL.into(), base).unwrap()
    }

    /// Every option the platform would turn into a refused field.
    fn everything_selected() -> ModelSelection<'static> {
        ModelSelection::model(SOL)
            .with_effort(Some("high"))
            .with_service_tier(Some(crate::llm::ServiceTier::Flex))
            .with_cache_mode(CacheMode::Explicit)
    }

    async fn plan_turn(provider: &OpenAiProvider) -> Result<LlmResponse, String> {
        provider
            .chat_responses(
                &every_item_kind(),
                &two_tools(),
                everything_selected(),
                Some("system"),
                None,
            )
            .await
            .map_err(|e| {
                let plan = e
                    .downcast_ref::<ChatGptPlanError>()
                    .map(|p| p.failure.clone());
                format!("{plan:?}|{e}")
            })
    }

    /// A plan body carries no refused field, keeps the effort and the
    /// required `store` and `stream`, and puts tools in the namespace. A
    /// replayed call names that namespace.
    #[test]
    fn the_plan_body_follows_the_plan_profile() {
        let provider = plan_provider("http://127.0.0.1:9/v1", Default::default());
        let body = provider.build_responses_body(
            SOL,
            &every_item_kind(),
            &two_tools(),
            Some("system"),
            &everything_selected(),
        );
        assert_eq!(plan_refusal(&body), None, "{body:#}");
        assert_eq!(body["reasoning"]["effort"], "high");
        assert_eq!(body["instructions"], "system");
        let tools = body["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0]["type"], "namespace");
        assert_eq!(tools[0]["name"], TOOL_NAMESPACE);
        assert_eq!(tools[0]["tools"].as_array().unwrap().len(), 2);
        let call = body["input"]
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["type"] == "function_call")
            .unwrap();
        assert_eq!(call["namespace"], TOOL_NAMESPACE);
        assert!(body["input"]
            .as_array()
            .unwrap()
            .iter()
            .all(|item| item["role"] != "system"));
    }

    /// A forced tool on the plan route keeps its tools top-level, where the
    /// `tool_choice` can name one, and its replayed calls name no namespace.
    #[test]
    fn a_forced_tool_on_the_plan_route_keeps_tools_top_level() {
        let provider = plan_provider("http://127.0.0.1:9/v1", Default::default());
        let body = provider.build_responses_body(
            SOL,
            &every_item_kind(),
            &two_tools(),
            Some("system"),
            &everything_selected().with_forced_tool(Some("request_read")),
        );
        assert_eq!(plan_refusal(&body), None, "{body:#}");
        let tools = body["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 2);
        assert!(tools.iter().all(|t| t["type"] == "function"), "{body:#}");
        assert!(body["input"]
            .as_array()
            .unwrap()
            .iter()
            .all(|item| item.get("namespace").is_none()));
    }

    /// A plan turn reaches a strict mock with the plan token as its bearer,
    /// and nothing it sends draws a refusal.
    #[tokio::test]
    async fn a_plan_turn_sends_the_plan_token_and_passes_a_strict_mock() {
        let (base, requests) = serve_plan(|_| (200, COMPLETED_TURN)).await;
        let response = plan_turn(&plan_provider(&base, Default::default()))
            .await
            .expect("the strict mock accepts the plan body");
        assert_eq!(response.content.as_deref(), Some("done"));
        assert!(response.prefill.is_none(), "a plan turn offers no beat");
        let requests = requests.lock().unwrap();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].0, "Bearer plan-token-1");
    }

    const LIMIT_BODY: &str = r#"{"error":{"message":"You have hit your usage limit.","type":"usage_limit","param":null,"code":"subscription_sharing_usage_limit_exceeded"}}"#;

    /// A plan 429 is sent once and fails typed, never retried in place.
    #[tokio::test]
    async fn a_plan_limit_is_not_retried_in_place() {
        let (base, requests) = serve_plan(|_| (429, LIMIT_BODY)).await;
        let err = plan_turn(&plan_provider(&base, Default::default()))
            .await
            .unwrap_err();
        assert!(err.starts_with("Some(LimitReached)|"), "{err}");
        assert_eq!(requests.lock().unwrap().len(), 1);
    }

    /// A limit that arrives as `response.failed` mid-stream fails typed too.
    #[tokio::test]
    async fn a_mid_stream_plan_limit_fails_typed() {
        const FAILED: &str = concat!(
            "event: response.failed\n",
            r#"data: {"response":{"status":"failed","error":{"code":"subscription_sharing_usage_limit_exceeded","message":"limit"}}}"#,
            "\n\n",
        );
        let (base, requests) = serve_plan(|_| (200, FAILED)).await;
        let err = plan_turn(&plan_provider(&base, Default::default()))
            .await
            .unwrap_err();
        assert!(err.starts_with("Some(LimitReached)|"), "{err}");
        assert_eq!(requests.lock().unwrap().len(), 1);
    }

    /// After a limit answer the next plan turn fails fast, typed, with no
    /// request sent.
    #[tokio::test]
    async fn a_plan_limit_pauses_the_next_turn_with_no_request() {
        let (base, requests) = serve_plan(|_| (429, LIMIT_BODY)).await;
        let provider = plan_provider(&base, Default::default());
        plan_turn(&provider).await.unwrap_err();
        let err = plan_turn(&provider).await.unwrap_err();
        assert!(err.starts_with("Some(LimitReached)|"), "{err}");
        assert_eq!(requests.lock().unwrap().len(), 1);
    }

    /// A usage limit on the model listing fails typed and starts the pause,
    /// so the next turn asks nothing.
    #[tokio::test]
    async fn a_limit_on_the_model_listing_fails_typed_and_pauses() {
        use std::sync::atomic::{AtomicUsize, Ordering::SeqCst};
        let listed = std::sync::Arc::new(AtomicUsize::new(0));
        let count = listed.clone();
        let app = axum::Router::new().route(
            "/v1/models",
            axum::routing::get(move || {
                count.fetch_add(1, SeqCst);
                async {
                    (
                        axum::http::StatusCode::TOO_MANY_REQUESTS,
                        [("content-type", "application/json")],
                        LIMIT_BODY,
                    )
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let provider = plan_provider(&format!("http://{addr}/v1"), Default::default());

        let err = plan_turn(&provider).await.unwrap_err();
        assert!(err.starts_with("Some(LimitReached)|"), "{err}");
        let again = plan_turn(&provider).await.unwrap_err();
        assert!(again.starts_with("Some(LimitReached)|"), "{again}");
        assert_eq!(listed.load(SeqCst), 1);
    }

    /// A model the plan does not list is refused as not served before any
    /// request spends the plan. So is a hidden one.
    #[tokio::test]
    async fn a_model_the_plan_does_not_list_is_not_served() {
        let (base, requests) = serve_plan(|_| (200, COMPLETED_TURN)).await;
        let provider = plan_provider(&base, Default::default());
        for model in ["gpt-5.4", "gpt-reserve"] {
            let err = provider
                .chat_responses(
                    &every_item_kind(),
                    &[],
                    ModelSelection::model(model),
                    None,
                    None,
                )
                .await
                .unwrap_err();
            assert!(
                err.downcast_ref::<crate::llm::ModelNotServed>().is_some(),
                "{model}: {err}"
            );
            assert!(err.to_string().contains("gpt-6.1-sol"), "{err}");
        }
        assert!(requests.lock().unwrap().is_empty());
    }

    const INVALID_USER_BODY: &str = r#"{"error":{"message":"invalid user","type":"invalid_request_error","param":null,"code":"subscription_sharing_invalid_user"}}"#;

    /// A rejected token is refreshed once, and the retry carries the new one.
    #[tokio::test]
    async fn a_rejected_plan_token_is_refreshed_once() {
        let (base, requests) = serve_plan(|index| match index {
            0 => (401, INVALID_USER_BODY),
            _ => (200, COMPLETED_TURN),
        })
        .await;
        let tokens = std::sync::Arc::new(FakeTokens::default());
        plan_turn(&plan_provider(&base, tokens.clone()))
            .await
            .expect("the refreshed token is accepted");
        let auths: Vec<String> = requests
            .lock()
            .unwrap()
            .iter()
            .map(|(auth, _)| auth.clone())
            .collect();
        assert_eq!(auths, ["Bearer plan-token-1", "Bearer plan-token-2"]);
        assert_eq!(*tokens.rejections.lock().unwrap(), ["plan-token-1"]);
    }

    /// A second rejection after the refresh fails, telling the user to sign
    /// in again.
    #[tokio::test]
    async fn a_second_rejection_fails() {
        let (base, requests) = serve_plan(|_| (401, INVALID_USER_BODY)).await;
        let err = plan_turn(&plan_provider(&base, Default::default()))
            .await
            .unwrap_err();
        assert!(err.starts_with("Some(InvalidUser)|"), "{err}");
        assert!(err.contains("Sign in again"), "{err}");
        assert_eq!(requests.lock().unwrap().len(), 2);
    }

    /// The step table: the platform keeps retrying 429 and 5xx, and the plan
    /// route backs off only on a passing unavailability or a plain 5xx.
    #[test]
    fn the_plan_route_never_backs_off_on_a_429() {
        use HttpFailureStep::*;
        let unavailable = ChatGptPlanFailure::Unavailable;
        let limit = ChatGptPlanFailure::LimitReached;
        let invalid = ChatGptPlanFailure::InvalidUser;
        let cases = [
            (false, 429, None, false, 1, Backoff),
            (false, 500, None, false, 1, Backoff),
            (false, 401, None, false, 1, Fail),
            (false, 429, None, false, 4, Fail),
            (true, 429, Some(&limit), false, 1, Fail),
            (true, 429, None, false, 1, Fail),
            (true, 503, Some(&unavailable), false, 1, Backoff),
            (true, 503, Some(&unavailable), false, 4, Fail),
            (true, 500, None, false, 1, Backoff),
            (true, 401, Some(&invalid), false, 1, Refresh),
            (true, 401, None, false, 1, Refresh),
            (true, 401, Some(&invalid), true, 1, Fail),
        ];
        for (plan, status, failure, refreshed, attempt, step) in cases {
            assert_eq!(
                http_failure_step(plan, status, failure, refreshed, attempt),
                step,
                "plan={plan} status={status} failure={failure:?} refreshed={refreshed}"
            );
        }
    }

    /// A plan provider refuses a keep-alive beat without sending it: a beat
    /// spends the user's weekly limit.
    #[tokio::test]
    async fn a_plan_provider_refuses_a_prefill() {
        let (base, requests) = serve_plan(|_| (200, COMPLETED_TURN)).await;
        let platform = turn_body(SOL, &tree_turn());
        let pair = prefill_pair(&platform, route(SOL)).expect("a platform pair");
        let provider = plan_provider(&base, Default::default());
        let refused = crate::llm::LlmProvider::prefill(
            &provider,
            &pair.stable,
            crate::llm::metered::CallToken::for_test(),
        )
        .await;
        assert!(refused.is_err());
        assert!(requests.lock().unwrap().is_empty());
    }

    /// The first text or tool-call delta stamps the call trace. A chunk that
    /// carries no content does not.
    #[tokio::test]
    async fn a_responses_stream_marks_its_first_content() {
        let feed = |event: &'static str, chunk: &'static str| async move {
            let ((), trace) = crate::llm::call_trace::scoped(async move {
                let mut content = String::new();
                let mut tools: Vec<AccumulatedToolCall> = Vec::new();
                let mut item_id_map: HashMap<String, usize> = HashMap::new();
                let mut meta = StreamMeta::default();
                OpenAiProvider::process_responses_chunk(
                    event,
                    chunk,
                    &mut content,
                    &mut tools,
                    &mut item_id_map,
                    &mut meta,
                )
                .unwrap();
            })
            .await;
            trace.first_delta.is_some()
        };
        assert!(feed("response.output_text.delta", r#"{"delta":"ok"}"#).await);
        assert!(
            feed(
                "response.output_item.added",
                r#"{"item":{"type":"function_call","call_id":"c1","name":"f","id":"i1"}}"#
            )
            .await
        );
        assert!(!feed("response.output_text.delta", r#"{"delta":""}"#).await);
        assert!(!feed("response.created", r#"{"response":{}}"#).await);
    }
}
