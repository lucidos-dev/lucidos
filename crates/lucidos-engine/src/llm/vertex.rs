use crate::llm::provider::{
    ContentBlock, LlmProvider, LlmResponse, Message, MessageContent, TokenCallback, ToolCall,
    ToolDefinition,
};
use async_trait::async_trait;
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;

/// Beta header enabling 1M token context window for Claude models on Vertex AI.
const ANTHROPIC_BETA_1M_CONTEXT: &str = "context-1m-2025-08-07";

/// Per-chunk timeout for Claude SSE streams (seconds).
const CLAUDE_STREAM_CHUNK_TIMEOUT_SECS: u64 = 300;

/// Shared access token cache for all VertexProvider instances in the same project.
/// gcloud tokens are project-scoped, so one cache serves all models.
pub type TokenCache = Arc<std::sync::Mutex<Option<(String, std::time::Instant)>>>;

/// Shared handle to the current Vertex AI region. Updated in place by the
/// engine when `vertex_region` changes; provider clones read the new value
/// on their next API call.
pub type LocationHandle = Arc<std::sync::RwLock<String>>;

/// Construct a `LocationHandle` from an initial region string.
pub fn location_handle(initial: String) -> LocationHandle {
    Arc::new(std::sync::RwLock::new(initial))
}

/// Snapshot the current region. Returns `"global"` if the lock is poisoned —
/// poisoning means a writer panicked mid-update, which should never happen
/// (writers only call `*guard = new`), but readers still shouldn't take down
/// every LLM and image call if it does.
pub fn read_location(handle: &LocationHandle) -> String {
    handle.read().map(|g| g.clone()).unwrap_or_else(|e| {
        crate::log!("[Vertex] location lock poisoned, falling back to global: {}", e);
        "global".to_string()
    })
}

pub fn vertex_host(location: &str) -> String {
    if location == "global" {
        "aiplatform.googleapis.com".to_string()
    } else {
        format!("{}-aiplatform.googleapis.com", location)
    }
}

/// Get a cached gcloud access token, refreshing only when expired (50 min TTL).
/// Shared by VertexProvider and VertexImagenProvider.
pub fn get_cached_access_token(
    cache: &TokenCache,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    const TOKEN_TTL: Duration = Duration::from_secs(3000);

    {
        let guard = cache
            .lock()
            .map_err(|e| format!("Token cache mutex poisoned: {}", e))?;
        if let Some((ref token, ref fetched_at)) = *guard {
            if fetched_at.elapsed() < TOKEN_TTL {
                return Ok(token.clone());
            }
        }
    }

    let output = Command::new("gcloud")
        .args(["auth", "application-default", "print-access-token"])
        .output()?;

    if output.status.success() {
        let token = String::from_utf8(output.stdout)?.trim().to_string();
        let mut guard = cache
            .lock()
            .map_err(|e| format!("Token cache mutex poisoned: {}", e))?;
        *guard = Some((token.clone(), std::time::Instant::now()));
        Ok(token)
    } else {
        Err(format!(
            "Failed to get access token: {}",
            String::from_utf8_lossy(&output.stderr)
        )
        .into())
    }
}

#[derive(Clone)]
pub struct VertexProvider {
    project_id: String,
    location: LocationHandle,
    model: String,
    client: reqwest::Client,
    /// Client without per-request timeout, used for Claude streaming where
    /// we apply per-chunk timeouts instead.
    streaming_client: reqwest::Client,
    /// Shared cached access token with expiry (tokens last 3600s, refresh at 3000s)
    token_cache: TokenCache,
}

impl VertexProvider {
    pub fn new(project_id: String, location: String, model: String) -> Self {
        Self::with_location_handle(
            project_id,
            location_handle(location),
            model,
            Arc::new(std::sync::Mutex::new(None)),
        )
    }

    pub fn with_location_handle(
        project_id: String,
        location: LocationHandle,
        model: String,
        token_cache: TokenCache,
    ) -> Self {
        Self {
            project_id,
            location,
            model,
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(900))
                .pool_idle_timeout(Duration::from_secs(30))
                .build()
                .expect("Failed to build HTTP client"),
            streaming_client: reqwest::Client::builder()
                .pool_idle_timeout(Duration::from_secs(30))
                .build()
                .expect("Failed to build streaming HTTP client"),
            token_cache,
        }
    }

    /// Snapshot the current region. URL builders call this per-request.
    fn current_location(&self) -> String {
        read_location(&self.location)
    }

    fn is_claude_model(model: &str) -> bool {
        model.starts_with("claude")
    }

    /// Strip `[1m]` suffix from model ID, returning (base_model, is_1m_context).
    fn parse_context_suffix(model: &str) -> (&str, bool) {
        if let Some(base) = model.strip_suffix("[1m]") {
            (base, true)
        } else {
            (model, false)
        }
    }

    /// Models that support extended thinking (reasoning goes to dedicated blocks
    /// instead of polluting the text response).
    fn supports_extended_thinking(model: &str) -> bool {
        model.contains("claude-3-7-sonnet")
            || model.contains("claude-sonnet-4")
            || model.contains("claude-opus-4")
    }

    /// Opus 4.7+ only supports adaptive thinking (no budget_tokens, no
    /// temperature/top_p/top_k). Effort is controlled via output_config.effort.
    fn requires_adaptive_thinking(model: &str) -> bool {
        model.contains("claude-opus-4-7")
    }

    fn endpoint_for_model(&self, model: &str) -> String {
        let location = self.current_location();
        let host = vertex_host(&location);
        if Self::is_claude_model(model) {
            format!(
                "https://{}/v1/projects/{}/locations/{}/publishers/anthropic/models/{}:streamRawPredict",
                host, self.project_id, location, model
            )
        } else if model.starts_with("gemini-3") {
            format!(
                "https://aiplatform.googleapis.com/v1/projects/{}/locations/global/publishers/google/models/{}:generateContent",
                self.project_id, model
            )
        } else {
            format!(
                "https://{}/v1/projects/{}/locations/{}/publishers/google/models/{}:generateContent",
                host, self.project_id, location, model
            )
        }
    }

    fn get_access_token(&self) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        get_cached_access_token(&self.token_cache)
    }

    /// Invalidate the cached token and refresh it. Returns the new token on success.
    /// `retried_auth` is flipped to true to prevent infinite retry loops.
    fn handle_auth_refresh(&self, model: &str, retried_auth: &mut bool) -> Option<String> {
        *retried_auth = true;
        if let Ok(mut cache) = self.token_cache.lock() {
            *cache = None;
        }
        match get_cached_access_token(&self.token_cache) {
            Ok(new_token) => {
                log!("[{}] HTTP 401, refreshed access token and retrying", model);
                Some(new_token)
            }
            Err(e) => {
                log!("[{}] HTTP 401 and failed to refresh token: {}", model, e);
                None
            }
        }
    }

    /// Send an HTTP request with retry on retryable status codes and network errors.
    /// On 401, invalidates the cached token, refreshes it, and retries once.
    async fn request_with_retry(
        &self,
        model: &str,
        url: &str,
        access_token: &str,
        body: &impl Serialize,
    ) -> Result<(reqwest::StatusCode, String), Box<dyn std::error::Error + Send + Sync>> {
        let mut attempt = 0u32;
        let mut token = access_token.to_string();
        let mut retried_auth = false;
        loop {
            attempt += 1;

            let response = match self
                .client
                .post(url)
                .header("Authorization", format!("Bearer {}", token))
                .header("Content-Type", "application/json")
                .json(body)
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

            let status = response.status();
            let response_body = response.text().await?;

            if status.as_u16() == 401 && !retried_auth {
                if let Some(new_token) = self.handle_auth_refresh(model, &mut retried_auth) {
                    token = new_token;
                    continue;
                }
                return Ok((status, response_body));
            }

            if super::should_retry_http(status.as_u16(), &response_body, attempt) {
                let delay = super::retry_delay(attempt, 1);
                super::log_retry(model, &format!("HTTP {}", status), attempt, delay);
                tokio::time::sleep(delay).await;
                continue;
            }

            return Ok((status, response_body));
        }
    }

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
                        ContentBlock::ToolUse { id, name, input } => serde_json::json!({
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

    async fn chat_claude(
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
        // through this gate, even ones that bypass the agentic loop.
        let stubs = crate::engine::validate_tool_use_pairing(&mut messages);
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
                let budget = match effort {
                    "low" => 4096,
                    "medium" => 8192,
                    "high" => 16384,
                    "xhigh" => 24576,
                    "max" => 32768,
                    _ => 16384,
                };
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

        let mut access_token = self.get_access_token()?;
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

                if status.as_u16() == 401 && !retried_auth {
                    if let Some(new_token) = self.handle_auth_refresh(model, &mut retried_auth) {
                        access_token = new_token;
                        continue;
                    }
                    return Err(format!("Claude API error ({}): {}", status, error_body).into());
                }

                if super::should_retry_http(status.as_u16(), &error_body, attempt) {
                    let delay = super::retry_delay(attempt, 1);
                    super::log_retry(model, &format!("HTTP {}", status), attempt, delay);
                    tokio::time::sleep(delay).await;
                    continue;
                }

                return Err(super::with_retry_context(
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

                    if super::is_retryable_error(&err_str) && attempt <= super::MAX_RETRIES {
                        let delay = super::retry_delay(attempt, 2); // longer for stream errors
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
                Err(_) => return Err("Claude stream timed out (no data for 5 minutes)".into()),
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
                            // Extract the delta from the last text block
                            for block in blocks.iter().rev() {
                                if let AccumulatedBlock::Text(t) = block {
                                    let delta_start = t.len() - (new_text_len - prev_text_len);
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
                    "thinking" | "redacted_thinking" => AccumulatedBlock::Thinking(
                        block["thinking"].as_str().unwrap_or("").to_string(),
                    ),
                    _ => AccumulatedBlock::Thinking(String::new()),
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
                        _ => {} // signature_delta, etc. — discard
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
                    meta.input_tokens = Some(total as u32);
                }
                if cache_write > 0 {
                    meta.cache_creation_tokens = Some(cache_write as u32);
                }
                if cache_read > 0 {
                    meta.cache_read_tokens = Some(cache_read as u32);
                }
            }
            "message_delta" => {
                if let Some(sr) = data["delta"]["stop_reason"].as_str() {
                    meta.stop_reason = Some(sr.to_string());
                }
                if let Some(ot) = data["usage"]["output_tokens"].as_u64() {
                    meta.output_tokens = Some(ot as u32);
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

    /// Search the web using Gemini's Google Search grounding.
    /// Returns a formatted string with the grounded answer and numbered source list.
    pub async fn search_with_grounding(
        &self,
        query: &str,
        max_results: usize,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        let model = "gemini-2.5-flash-lite";
        let request = serde_json::json!({
            "system_instruction": {
                "parts": [{"text": "You are a web search assistant. Search the web thoroughly and return comprehensive, detailed results with sources. Focus on finding and presenting relevant information from search results rather than answering from your own knowledge."}]
            },
            "contents": [{
                "role": "user",
                "parts": [{"text": query}]
            }],
            "tools": [{"google_search": {}}]
        });

        let access_token = self.get_access_token()?;
        let url = self.endpoint_for_model(model);

        let (status, body) = self
            .request_with_retry(model, &url, &access_token, &request)
            .await?;

        if !status.is_success() {
            return Err(format!("Gemini search API error ({}): {}", status, body).into());
        }

        let parsed: serde_json::Value = serde_json::from_str(&body)
            .map_err(|e| format!("Failed to parse Gemini search response: {}", e))?;

        // Extract the grounded text answer
        let answer = parsed["candidates"][0]["content"]["parts"]
            .as_array()
            .and_then(|parts| {
                parts
                    .iter()
                    .find_map(|p| p["text"].as_str().map(|s| s.to_string()))
            })
            .unwrap_or_default();

        // Extract grounding chunks (sources with URL and title)
        let metadata = &parsed["candidates"][0]["groundingMetadata"];
        let chunks = metadata["groundingChunks"]
            .as_array()
            .cloned()
            .unwrap_or_default();

        let mut sources = Vec::new();
        for (i, chunk) in chunks.iter().enumerate() {
            if i >= max_results {
                break;
            }
            let title = chunk["web"]["title"].as_str().unwrap_or("Untitled");
            let uri = chunk["web"]["uri"].as_str().unwrap_or("");
            if !uri.is_empty() {
                sources.push(format!("{}. {}\n   {}", i + 1, title, uri));
            }
        }

        if answer.is_empty() && sources.is_empty() {
            return Ok(format!("No search results found for: {}", query));
        }

        let mut result = String::new();
        if !answer.is_empty() {
            result.push_str(&answer);
        }
        if !sources.is_empty() {
            if !result.is_empty() {
                result.push_str("\n\n");
            }
            result.push_str("Sources:\n\n");
            result.push_str(&sources.join("\n\n"));
        }

        Ok(result)
    }

    async fn chat_gemini(
        &self,
        messages: Vec<Message>,
        tools: Vec<ToolDefinition>,
        model: &str,
        system_prompt: Option<&str>,
        on_token: Option<TokenCallback>,
        reasoning_effort: Option<&str>,
    ) -> Result<LlmResponse, Box<dyn std::error::Error + Send + Sync>> {
        let contents: Vec<VertexContent> = messages
            .into_iter()
            .map(|m| VertexContent {
                role: if m.role == "user" {
                    "user".to_string()
                } else {
                    "model".to_string()
                },
                parts: message_content_to_parts(m.content),
            })
            .collect();

        let vertex_tools = if tools.is_empty() {
            None
        } else {
            Some(vec![VertexTool {
                function_declarations: tools
                    .into_iter()
                    .map(|t| VertexFunction {
                        name: t.name,
                        description: t.description,
                        parameters: t.parameters,
                    })
                    .collect(),
            }])
        };

        let system_inst = system_prompt.map(|s| VertexSystemInstruction {
            parts: vec![VertexPart {
                text: Some(s.to_string()),
                inline_data: None,
            }],
        });

        let generation_config = if model.starts_with("gemini-3") {
            let effort = reasoning_effort.unwrap_or("high");
            if effort == "none" {
                Some(VertexGenerationConfig {
                    thinking_config: VertexThinkingConfig { thinking_budget: 0 },
                })
            } else {
                let budget = match effort {
                    "low" => 4096,
                    "medium" => 8192,
                    "high" => 16384,
                    "xhigh" => 24576,
                    "max" => 32768,
                    _ => 16384,
                };
                Some(VertexGenerationConfig {
                    thinking_config: VertexThinkingConfig {
                        thinking_budget: budget,
                    },
                })
            }
        } else {
            None
        };

        let request = VertexRequest {
            system_instruction: system_inst,
            contents,
            tools: vertex_tools,
            generation_config,
        };

        let access_token = self.get_access_token()?;

        let (status, body) = self
            .request_with_retry(
                model,
                &self.endpoint_for_model(model),
                &access_token,
                &request,
            )
            .await?;

        if !status.is_success() {
            log!("[Vertex] Gemini API error ({}): {}", status, body);
            return Err(format!("Gemini API error ({}): {}", status, body).into());
        }

        let parsed: VertexResponse = match serde_json::from_str(&body) {
            Ok(p) => p,
            Err(e) => {
                log!("[Vertex] Failed to parse Gemini response: {}\nBody: {}", e, body);
                return Err(format!("Failed to parse Gemini response: {}", e).into());
            }
        };

        let mut content = None;
        let mut tool_calls = Vec::new();

        if let Some(candidates) = parsed.candidates {
            if let Some(candidate) = candidates.first() {
                for part in &candidate.content.parts {
                    // Skip thinking parts — internal reasoning, not shown to user
                    if part.thought {
                        continue;
                    }
                    if let Some(text) = &part.text {
                        content = Some(text.clone());
                    }
                    if let Some(fc) = &part.function_call {
                        tool_calls.push(ToolCall {
                            id: uuid::Uuid::new_v4().to_string(),
                            name: fc.name.clone(),
                            arguments: fc.args.clone(),
                        });
                    }
                }
            }
        }

        // Gemini uses non-streaming requests, so emit the full text at once
        if let (Some(cb), Some(text)) = (&on_token, &content) {
            cb(text);
        }

        // TODO: capture Gemini finishReason + usage if empty completions surface here
        Ok(LlmResponse {
            content,
            tool_calls,
            stop_reason: None,
            output_tokens: None,
            input_tokens: None,
            cache_creation_tokens: None,
            cache_read_tokens: None,
            thinking_chars: None,
        })
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
    Some(serde_json::Value::Array(vec![text_block_with_cache_control(
        s.to_string(),
    )]))
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
}

// ===== Gemini/Vertex request/response types =====

#[derive(Serialize)]
struct VertexRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    system_instruction: Option<VertexSystemInstruction>,
    contents: Vec<VertexContent>,
    tools: Option<Vec<VertexTool>>,
    #[serde(skip_serializing_if = "Option::is_none", rename = "generationConfig")]
    generation_config: Option<VertexGenerationConfig>,
}

#[derive(Serialize)]
struct VertexGenerationConfig {
    #[serde(rename = "thinkingConfig")]
    thinking_config: VertexThinkingConfig,
}

#[derive(Serialize)]
struct VertexThinkingConfig {
    #[serde(rename = "thinkingBudget")]
    thinking_budget: u32,
}

#[derive(Serialize)]
struct VertexSystemInstruction {
    parts: Vec<VertexPart>,
}

#[derive(Serialize)]
struct VertexContent {
    role: String,
    parts: Vec<VertexPart>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct VertexPart {
    #[serde(skip_serializing_if = "Option::is_none")]
    text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    inline_data: Option<VertexInlineData>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct VertexInlineData {
    mime_type: String,
    data: String,
}

/// Convert a MessageContent into Gemini-format parts.
/// Text blocks become text parts, Image blocks become inlineData parts.
fn message_content_to_parts(content: MessageContent) -> Vec<VertexPart> {
    match content {
        MessageContent::Text(s) => vec![VertexPart {
            text: Some(s),
            inline_data: None,
        }],
        MessageContent::Blocks(blocks) => {
            blocks
                .into_iter()
                .filter_map(|block| match block {
                    ContentBlock::Text { text } => Some(VertexPart {
                        text: Some(text),
                        inline_data: None,
                    }),
                    ContentBlock::Image {
                        media_type, data, ..
                    } => Some(VertexPart {
                        text: None,
                        inline_data: Some(VertexInlineData {
                            mime_type: media_type,
                            data,
                        }),
                    }),
                    // Tool blocks are not used in Gemini's content parts
                    _ => None,
                })
                .collect()
        }
    }
}

#[derive(Serialize)]
struct VertexTool {
    function_declarations: Vec<VertexFunction>,
}

#[derive(Serialize)]
struct VertexFunction {
    name: String,
    description: String,
    parameters: serde_json::Value,
}

#[derive(Deserialize)]
struct VertexResponse {
    candidates: Option<Vec<VertexCandidate>>,
}

#[derive(Deserialize)]
struct VertexCandidate {
    content: VertexResponseContent,
}

#[derive(Deserialize)]
struct VertexResponseContent {
    #[serde(default)]
    parts: Vec<VertexResponsePart>,
}

#[derive(Deserialize)]
struct VertexResponsePart {
    text: Option<String>,
    /// When true, this part contains internal reasoning (Gemini thinking mode).
    #[serde(default)]
    thought: bool,
    #[serde(rename = "functionCall")]
    function_call: Option<VertexFunctionCall>,
}

#[derive(Deserialize)]
struct VertexFunctionCall {
    name: String,
    args: serde_json::Value,
}

#[async_trait]
impl LlmProvider for VertexProvider {
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
        if Self::is_claude_model(model) {
            self.chat_claude(
                messages,
                tools,
                model,
                system_prompt,
                on_token,
                reasoning_effort,
            )
            .await
        } else {
            self.chat_gemini(
                messages,
                tools,
                model,
                system_prompt,
                on_token,
                reasoning_effort,
            )
            .await
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::provider::{ContentBlock, MessageContent};

    #[test]
    fn message_content_to_parts_text_only() {
        let content = MessageContent::Text("hello".to_string());
        let parts = message_content_to_parts(content);
        assert_eq!(parts.len(), 1);
        assert_eq!(parts[0].text.as_deref(), Some("hello"));
        assert!(parts[0].inline_data.is_none());
    }

    #[test]
    fn message_content_to_parts_with_image() {
        let content = MessageContent::Blocks(vec![
            ContentBlock::Text {
                text: "describe this".to_string(),
            },
            ContentBlock::Image {
                source_type: "base64".to_string(),
                media_type: "image/jpeg".to_string(),
                data: "abc123".to_string(),
            },
        ]);
        let parts = message_content_to_parts(content);
        assert_eq!(parts.len(), 2);
        // First part is text
        assert_eq!(parts[0].text.as_deref(), Some("describe this"));
        assert!(parts[0].inline_data.is_none());
        // Second part is image
        assert!(parts[1].text.is_none());
        let inline = parts[1].inline_data.as_ref().unwrap();
        assert_eq!(inline.mime_type, "image/jpeg");
        assert_eq!(inline.data, "abc123");
    }

    #[test]
    fn message_content_to_parts_serializes_correctly() {
        let content = MessageContent::Blocks(vec![
            ContentBlock::Text {
                text: "look at this".to_string(),
            },
            ContentBlock::Image {
                source_type: "base64".to_string(),
                media_type: "image/png".to_string(),
                data: "AAAA".to_string(),
            },
        ]);
        let parts = message_content_to_parts(content);
        let json = serde_json::to_value(&parts).unwrap();
        let arr = json.as_array().unwrap();

        // Text part: only "text" field, no "inlineData"
        assert_eq!(arr[0]["text"], "look at this");
        assert!(arr[0].get("inlineData").is_none());

        // Image part: only "inlineData" field, no "text"
        assert!(arr[1].get("text").is_none());
        assert_eq!(arr[1]["inlineData"]["mimeType"], "image/png");
        assert_eq!(arr[1]["inlineData"]["data"], "AAAA");
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
        let value = VertexProvider::message_content_to_claude_value(&content);
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
        let value = VertexProvider::message_content_to_claude_value(&content);
        let arr = value.as_array().unwrap();
        assert_eq!(arr.len(), 2);
        assert_eq!(arr[0]["type"], "text");
        assert_eq!(arr[0]["text"], "describe this");
        assert_eq!(arr[1]["type"], "image");
    }

    #[test]
    fn message_content_to_parts_skips_tool_blocks() {
        let content = MessageContent::Blocks(vec![
            ContentBlock::Text {
                text: "hi".to_string(),
            },
            ContentBlock::ToolUse {
                id: "t1".to_string(),
                name: "search".to_string(),
                input: serde_json::json!({}),
            },
            ContentBlock::ToolResult {
                tool_use_id: "t1".to_string(),
                content: "result".to_string(),
            },
        ]);
        let parts = message_content_to_parts(content);
        assert_eq!(parts.len(), 1);
        assert_eq!(parts[0].text.as_deref(), Some("hi"));
    }

    #[test]
    fn endpoint_global_location_no_region_prefix() {
        let provider = VertexProvider::new(
            "my-project".into(),
            "global".into(),
            "claude-opus-4-6".into(),
        );
        let url = provider.endpoint_for_model("claude-opus-4-6");
        assert!(
            url.starts_with("https://aiplatform.googleapis.com/"),
            "global should not have region prefix: {}",
            url
        );
        assert!(
            url.contains("/locations/global/"),
            "path should still use locations/global: {}",
            url
        );
    }

    #[test]
    fn endpoint_regional_location_has_prefix() {
        let provider = VertexProvider::new(
            "my-project".into(),
            "europe-west1".into(),
            "claude-opus-4-6".into(),
        );
        let url = provider.endpoint_for_model("claude-opus-4-6");
        assert!(
            url.starts_with("https://europe-west1-aiplatform.googleapis.com/"),
            "regional should have prefix: {}",
            url
        );
        assert!(
            url.contains("/locations/europe-west1/"),
            "path should use the region: {}",
            url
        );
    }

    #[test]
    fn endpoint_gemini_global_location() {
        let provider = VertexProvider::new(
            "my-project".into(),
            "global".into(),
            "gemini-2.5-flash".into(),
        );
        let url = provider.endpoint_for_model("gemini-2.5-flash");
        assert!(
            url.starts_with("https://aiplatform.googleapis.com/"),
            "global gemini should not have region prefix: {}",
            url
        );
        assert!(
            url.contains("/locations/global/"),
            "path should use locations/global: {}",
            url
        );
    }

    #[test]
    fn endpoint_gemini3_always_global() {
        let provider = VertexProvider::new(
            "my-project".into(),
            "us-central1".into(),
            "gemini-3-flash-preview".into(),
        );
        let url = provider.endpoint_for_model("gemini-3-flash-preview");
        assert!(
            url.starts_with("https://aiplatform.googleapis.com/"),
            "gemini-3 always uses global host: {}",
            url
        );
        assert!(
            url.contains("/locations/global/"),
            "gemini-3 always uses locations/global: {}",
            url
        );
    }

    #[test]
    fn parse_context_suffix_strips_1m() {
        assert_eq!(
            VertexProvider::parse_context_suffix("claude-opus-4-6[1m]"),
            ("claude-opus-4-6", true)
        );
        assert_eq!(
            VertexProvider::parse_context_suffix("claude-sonnet-4-6[1m]"),
            ("claude-sonnet-4-6", true)
        );
    }

    #[test]
    fn parse_context_suffix_preserves_base_model() {
        assert_eq!(
            VertexProvider::parse_context_suffix("claude-opus-4-6"),
            ("claude-opus-4-6", false)
        );
        assert_eq!(
            VertexProvider::parse_context_suffix("gemini-2.5-pro"),
            ("gemini-2.5-pro", false)
        );
    }

    #[test]
    fn endpoint_reflects_live_location_handle_updates() {
        let handle = location_handle("europe-west1".into());
        let provider = VertexProvider::with_location_handle(
            "my-project".into(),
            handle.clone(),
            "claude-opus-4-6".into(),
            Arc::new(std::sync::Mutex::new(None)),
        );

        let before = provider.endpoint_for_model("claude-opus-4-6");
        assert!(
            before.contains("europe-west1") && before.contains("/locations/europe-west1/"),
            "initial URL must reflect handle's starting region, got: {}",
            before
        );

        *handle.write().unwrap() = "us-central1".into();

        let after = provider.endpoint_for_model("claude-opus-4-6");
        assert!(
            after.contains("us-central1") && after.contains("/locations/us-central1/"),
            "URL must pick up updated handle without rebuilding the provider, got: {}",
            after
        );
        assert!(
            !after.contains("europe-west1"),
            "URL must not still contain the old region, got: {}",
            after
        );
    }

    #[test]
    fn process_sse_captures_input_tokens_from_message_start() {
        // Anthropic streams `message_start` early in every response with the
        // exact prompt-token cost. Capturing it lets the UI replace the
        // chars/4 estimate (which over-counts base64 image bytes by orders
        // of magnitude) with the real number.
        let mut blocks = Vec::new();
        let mut meta = TurnMeta::default();
        let event = r#"{"type":"message_start","message":{"id":"msg_x","type":"message","role":"assistant","content":[],"model":"claude-opus-4-7","stop_reason":null,"stop_sequence":null,"usage":{"input_tokens":4321,"cache_creation_input_tokens":1000,"cache_read_input_tokens":500,"output_tokens":1}}}"#;

        VertexProvider::process_sse_data(event, &mut blocks, &mut meta).unwrap();

        // Real prompt size = uncached input + cache writes + cache reads
        // (everything the model actually processed). 4321 + 1000 + 500 = 5821.
        assert_eq!(meta.input_tokens, Some(5821));
        // Cache breakdown survives separately so the modal can show hit rate.
        assert_eq!(meta.cache_creation_tokens, Some(1000));
        assert_eq!(meta.cache_read_tokens, Some(500));
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
    fn cache_control_serializes_into_wire_format() {
        // End-to-end: build a request the way chat_claude does, serialize it,
        // and check cache_control lands on tools[-1], the system block, and
        // messages[-1]'s last content block.
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
            anthropic_version: "vertex-2023-10-16".into(),
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
        assert_eq!(last_blocks.last().unwrap()["cache_control"]["type"], "ephemeral");
    }
}
