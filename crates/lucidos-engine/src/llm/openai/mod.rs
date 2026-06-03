//! OpenAI provider.
//!
//! This module owns the shared `OpenAiProvider` struct, the streamed-tool /
//! per-turn meta types, the shared `build_llm_response`, and the `LlmProvider`
//! dispatch. The two request paths live in child modules:
//!
//! - [`chat`] — Chat Completions API (non-codex models).
//! - [`responses`] — Responses API (GPT-5+ / codex models).
//!
//! Splitting is purely structural — `OpenAiProvider` stays reachable at
//! `crate::llm::openai::OpenAiProvider` (and re-exported from `crate::llm`).


use crate::llm::provider::{
    LlmProvider, LlmResponse, Message, TokenCallback, ToolCall, ToolDefinition,
};
use async_trait::async_trait;
use std::time::Duration;

mod chat;
mod responses;

const CHUNK_TIMEOUT_SECS: u64 = 300;
const DEFAULT_MAX_COMPLETION_TOKENS: u32 = 16384;

/// GPT-5+ and Codex models use the Responses API, all others use Chat Completions.
fn uses_responses_api(model: &str) -> bool {
    model.contains("codex") || model.starts_with("gpt-5")
}

/// Map the unified `reasoning_effort` string to OpenAI's vocabulary. Today
/// only `"max" → "xhigh"` needs translating; every other value (`low`,
/// `medium`, `high`, …) passes through unchanged. Centralised so the Chat
/// Completions builder and the Responses builder cannot drift.
fn openai_reasoning_effort(effort: &str) -> &str {
    if effort == "max" {
        "xhigh"
    } else {
        effort
    }
}

pub struct OpenAiProvider {
    api_key: String,
    model: String,
    /// Client without per-request timeout, used for streaming where
    /// we apply per-chunk timeouts instead.
    streaming_client: reqwest::Client,
}

impl OpenAiProvider {
    /// Build the provider; returns `Err` if the reqwest builder rejects the
    /// configuration so the engine can fail at startup with a logged reason.
    pub fn new(
        api_key: String,
        model: String,
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let streaming_client = reqwest::Client::builder()
            .pool_idle_timeout(Duration::from_secs(30))
            .build()?;
        Ok(Self {
            api_key,
            model,
            streaming_client,
        })
    }

    /// Build an LlmResponse from accumulated text content, tool calls, and
    /// the per-stream meta captured from `finish_reason` / `usage` deltas.
    fn build_llm_response(
        content: String,
        tool_call_map: Vec<AccumulatedToolCall>,
        meta: StreamMeta,
    ) -> LlmResponse {
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
                    // OpenAI doesn't surface Gemini-style thought signatures.
                    thought_signature: None,
                }
            })
            .collect();

        LlmResponse {
            content: final_content,
            tool_calls,
            stop_reason: meta.stop_reason,
            output_tokens: meta.output_tokens,
            input_tokens: meta.input_tokens,
            // OpenAI's usage block doesn't separate cache writes the way
            // Anthropic's does; cache_read is the only cached-tokens count
            // they expose (via `prompt_tokens_details.cached_tokens`), and
            // we leave cache_creation_tokens unset to honor that distinction.
            cache_creation_tokens: None,
            cache_read_tokens: meta.cache_read_tokens,
            thinking_chars: None,
            unknown_sse_dropped: 0,
        }
    }
}

/// Intermediate state for accumulating a streamed tool call.
struct AccumulatedToolCall {
    id: String,
    name: String,
    arguments_json: String,
}

/// Per-turn metadata accumulated from streamed chunks. Both OpenAI APIs
/// surface `finish_reason` / `status` and token usage near the end of the
/// stream rather than per-chunk; the parser stashes them here so
/// `build_llm_response` can populate `LlmResponse.{stop_reason,
/// input_tokens, output_tokens, cache_read_tokens}` in one place.
#[derive(Default)]
struct StreamMeta {
    stop_reason: Option<String>,
    input_tokens: Option<u32>,
    output_tokens: Option<u32>,
    cache_read_tokens: Option<u32>,
}

impl StreamMeta {
    /// Pull `prompt_tokens` / `completion_tokens` / cached-tokens out of a
    /// Chat Completions `usage` object. Above-u32 token counts indicate a
    /// corrupt upstream block — clamp rather than panic so a single bad
    /// chunk can't tank the stream.
    fn absorb_chat_usage(&mut self, usage: &serde_json::Value) {
        if let Some(prompt) = usage.get("prompt_tokens").and_then(|v| v.as_u64()) {
            self.input_tokens = Some(crate::llm::clamp_provider_token_count(prompt, "OpenAI"));
        }
        if let Some(completion) = usage.get("completion_tokens").and_then(|v| v.as_u64()) {
            self.output_tokens = Some(crate::llm::clamp_provider_token_count(completion, "OpenAI"));
        }
        if let Some(cached) = usage
            .pointer("/prompt_tokens_details/cached_tokens")
            .and_then(|v| v.as_u64())
        {
            if cached > 0 {
                self.cache_read_tokens = Some(crate::llm::clamp_provider_token_count(cached, "OpenAI"));
            }
        }
    }

    /// Pull usage + finish reason out of a Responses API `response.completed`
    /// payload (the terminal `data: { response: { ... } }`). Responses API
    /// uses `input_tokens` / `output_tokens` (not `prompt_/completion_`) and
    /// reports completion shape via `status` + optional
    /// `incomplete_details.reason`.
    fn absorb_responses_completion(&mut self, resp: &serde_json::Value) {
        if let Some(usage) = resp.get("usage") {
            if let Some(input) = usage.get("input_tokens").and_then(|v| v.as_u64()) {
                self.input_tokens = Some(crate::llm::clamp_provider_token_count(input, "OpenAI"));
            }
            if let Some(output) = usage.get("output_tokens").and_then(|v| v.as_u64()) {
                self.output_tokens = Some(crate::llm::clamp_provider_token_count(output, "OpenAI"));
            }
            if let Some(cached) = usage
                .pointer("/input_tokens_details/cached_tokens")
                .and_then(|v| v.as_u64())
            {
                if cached > 0 {
                    self.cache_read_tokens = Some(crate::llm::clamp_provider_token_count(cached, "OpenAI"));
                }
            }
        }
        // For incomplete responses the reason lives at
        // `incomplete_details.reason` (e.g. "max_output_tokens",
        // "content_filter"). For normal completions the top-level `status`
        // is "completed" — keep it so empty-completion debugging shows
        // *something* in the field rather than None.
        if let Some(reason) = resp
            .pointer("/incomplete_details/reason")
            .and_then(|r| r.as_str())
        {
            self.stop_reason = Some(reason.to_string());
        } else if self.stop_reason.is_none() {
            if let Some(status) = resp.get("status").and_then(|s| s.as_str()) {
                self.stop_reason = Some(status.to_string());
            }
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Above-u32 token counts must clamp rather than panic — a single
    /// corrupt upstream usage block shouldn't tank the stream.
    #[test]
    fn absurd_token_count_clamps_to_u32_max() {
        let mut meta = StreamMeta::default();
        let absurd = serde_json::json!({
            "prompt_tokens": u64::MAX,
            "completion_tokens": (u32::MAX as u64) + 1,
        });
        meta.absorb_chat_usage(&absurd);
        assert_eq!(meta.input_tokens, Some(u32::MAX));
        assert_eq!(meta.output_tokens, Some(u32::MAX));
    }
}
