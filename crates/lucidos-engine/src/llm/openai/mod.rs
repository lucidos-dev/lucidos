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


use crate::core::AuthType;
use crate::llm::provider::{
    LlmProvider, LlmResponse, Message, TokenCallback, ToolCall, ToolDefinition,
};
use async_trait::async_trait;
use std::time::Duration;

mod chat;
mod responses;

const CHUNK_TIMEOUT_SECS: u64 = 300;
const DEFAULT_MAX_COMPLETION_TOKENS: u32 = 16384;

/// Where the OpenAI API key the engine builds [`OpenAiProvider`] from came
/// from — surfaced in the startup log so an operator can tell whether the
/// stored credential or the env fallback is in effect.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenAiKeySource {
    /// A credential stored in Settings → Providers (service name `openai`).
    Credential,
    /// The `OPENAI_API_KEY` launch environment variable (the fallback).
    Env,
}

impl std::fmt::Display for OpenAiKeySource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Credential => "stored credential (Settings → Providers)",
            Self::Env => "OPENAI_API_KEY",
        })
    }
}

/// Resolve the OpenAI API key used to construct [`OpenAiProvider`].
///
/// A usable stored `openai` credential (Settings → Providers) takes precedence
/// so the engine no longer needs `OPENAI_API_KEY` in its launch environment;
/// the env var stays as a fallback to preserve existing deployments. Only
/// `api_key` / `bearer` credentials carry a usable OpenAI key — any other
/// `auth_type` (e.g. a `password` JSON blob) is ignored with a log line and the
/// resolver falls through to the env var. Blank/whitespace values on either
/// side are treated as absent. Returns `None` when neither is configured, in
/// which case OpenAI models surface a clear error from `RoutingProvider`.
pub fn resolve_openai_api_key(
    credential: Option<(AuthType, String)>,
    env_key: Option<String>,
) -> Option<(String, OpenAiKeySource)> {
    if let Some((auth_type, value)) = credential {
        match auth_type {
            AuthType::ApiKey | AuthType::Bearer => {
                let trimmed = value.trim();
                if !trimmed.is_empty() {
                    return Some((trimmed.to_string(), OpenAiKeySource::Credential));
                }
            }
            other => {
                log!(
                    "[Startup] OpenAI credential auth_type {} unsupported (expected api_key or bearer) — ignoring it and falling back to OPENAI_API_KEY",
                    other
                );
            }
        }
    }
    if let Some(value) = env_key {
        let trimmed = value.trim();
        if !trimmed.is_empty() {
            return Some((trimmed.to_string(), OpenAiKeySource::Env));
        }
    }
    None
}

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

    fn default_model(&self) -> &str {
        &self.model
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

    /// A stored `openai` credential wins over the env var so users can configure
    /// OpenAI entirely from Settings → Providers without OPENAI_API_KEY.
    #[test]
    fn stored_credential_takes_precedence_over_env() {
        let resolved = resolve_openai_api_key(
            Some((AuthType::ApiKey, "sk-stored".to_string())),
            Some("sk-env".to_string()),
        );
        assert_eq!(
            resolved,
            Some(("sk-stored".to_string(), OpenAiKeySource::Credential))
        );
    }

    /// With no credential the OPENAI_API_KEY env fallback is preserved.
    #[test]
    fn falls_back_to_env_when_no_credential() {
        let resolved = resolve_openai_api_key(None, Some("sk-env".to_string()));
        assert_eq!(resolved, Some(("sk-env".to_string(), OpenAiKeySource::Env)));
    }

    /// A `bearer` credential carries a usable OpenAI key just like `api_key`.
    #[test]
    fn bearer_credential_is_accepted() {
        let resolved =
            resolve_openai_api_key(Some((AuthType::Bearer, "sk-bearer".to_string())), None);
        assert_eq!(
            resolved,
            Some(("sk-bearer".to_string(), OpenAiKeySource::Credential))
        );
    }

    /// A non-key auth_type (e.g. a `password` JSON blob) is not a usable OpenAI
    /// key — ignore it and fall back to the env var rather than feeding garbage
    /// into the provider.
    #[test]
    fn unsupported_auth_type_falls_back_to_env() {
        let resolved = resolve_openai_api_key(
            Some((AuthType::Password, r#"{"username":"a","password":"b"}"#.to_string())),
            Some("sk-env".to_string()),
        );
        assert_eq!(resolved, Some(("sk-env".to_string(), OpenAiKeySource::Env)));
    }

    /// A blank/whitespace credential value is treated as absent so a stray empty
    /// row doesn't shadow a real env key.
    #[test]
    fn blank_credential_falls_back_to_env() {
        let resolved = resolve_openai_api_key(
            Some((AuthType::ApiKey, "   ".to_string())),
            Some("sk-env".to_string()),
        );
        assert_eq!(resolved, Some(("sk-env".to_string(), OpenAiKeySource::Env)));
    }

    /// Neither configured (or both blank) → None, so OpenAI models surface a
    /// clear "not configured" error instead of constructing a dead provider.
    #[test]
    fn none_configured_returns_none() {
        assert_eq!(resolve_openai_api_key(None, None), None);
        assert_eq!(
            resolve_openai_api_key(
                Some((AuthType::ApiKey, String::new())),
                Some("  ".to_string())
            ),
            None
        );
    }
}
