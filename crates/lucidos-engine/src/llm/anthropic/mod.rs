//! Direct Anthropic provider — hits `api.anthropic.com/v1/messages`.
//!
//! Shares the entire wire format (request building, cache-control, SSE parsing)
//! with the Vertex Claude path via [`crate::llm::anthropic_wire`]; this module
//! only owns the direct transport: the endpoint, the auth headers, and the
//! retry loop. Wired into [`crate::llm::routing::RoutingProvider`] for any model
//! the [`crate::llm::model_registry`] maps to `ProviderKind::Anthropic`.

use crate::llm::provider::{LlmProvider, LlmResponse, Message, TokenCallback, ToolDefinition};
use async_trait::async_trait;
use std::time::Duration;

/// `pub(crate)` so `llm::web_search::anthropic` can share this module's auth
/// header and API-version constants instead of restating them.
pub(crate) mod chat;

/// Beta flag that authorizes a Claude subscription OAuth bearer token to call
/// the Messages API. Required alongside `Authorization: Bearer` for OAuth auth —
/// shared by the direct provider (`chat.rs`) and the builtin `anthropic` proxy
/// (`api::proxy_builtin`), so an OAuth credential works through either path.
pub const ANTHROPIC_OAUTH_BETA: &str = "oauth-2025-04-20";

/// Authentication for the direct Anthropic API. The two kinds map to different
/// HTTP headers (see `chat.rs`):
/// - [`AnthropicAuth::ApiKey`] → `x-api-key` (pay-per-token).
/// - [`AnthropicAuth::OAuthBearer`] → `Authorization: Bearer` + the
///   `anthropic-beta: oauth-2025-04-20` header (Claude subscription).
#[derive(Clone)]
pub enum AnthropicAuth {
    ApiKey(String),
    /// v1: no auto-refresh. Subscription OAuth tokens are short-lived; when one
    /// expires the call 401s and the UI tells the user to re-paste a fresh token
    /// in Settings → Providers. Auto-refresh is a follow-up.
    OAuthBearer(String),
}

pub struct AnthropicProvider {
    auth: AnthropicAuth,
    model: String,
    /// Client without a per-request timeout — streaming applies per-chunk
    /// timeouts inside `parse_claude_stream` instead.
    streaming_client: reqwest::Client,
}

impl AnthropicProvider {
    /// Build the provider; returns `Err` if the reqwest builder rejects the
    /// configuration so the engine can fail at startup with a logged reason.
    pub fn new(
        auth: AnthropicAuth,
        model: String,
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let streaming_client = reqwest::Client::builder()
            .pool_idle_timeout(Duration::from_secs(30))
            .build()?;
        Ok(Self {
            auth,
            model,
            streaming_client,
        })
    }
}

#[async_trait]
impl LlmProvider for AnthropicProvider {
    fn default_model(&self) -> &str {
        &self.model
    }

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
        self.chat_anthropic(
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
