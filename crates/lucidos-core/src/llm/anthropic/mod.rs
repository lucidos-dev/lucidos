pub use super::provider_credentials::{resolve_anthropic_auth, AnthropicAuth, AnthropicAuthSource};
use crate::llm::claude_gateway::served_models::ServedModels;
use crate::llm::claude_gateway::{GatewayAccess, GatewayOrigin};
use crate::llm::provider::{
    LlmProvider, LlmResponse, Message, ModelSelection, TokenCallback, ToolDefinition,
};
use async_trait::async_trait;
use std::sync::Arc;
use std::time::Duration;

pub(crate) mod chat;

pub const ANTHROPIC_OAUTH_BETA: &str = "oauth-2025-04-20";

pub(crate) enum Endpoint {
    Direct(AnthropicAuth),
    ClaudeGateway {
        origin: GatewayOrigin,
        access: Arc<dyn GatewayAccess>,
        served: ServedModels,
    },
}

pub struct AnthropicProvider {
    endpoint: Endpoint,
    model: String,
    /// INVARIANT: Client without a per-request timeout — streaming applies per-chunk
    /// timeouts inside `parse_claude_stream` instead.
    streaming_client: reqwest::Client,
}

impl AnthropicProvider {
    pub fn new(
        auth: AnthropicAuth,
        model: String,
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        Self::with_endpoint(Endpoint::Direct(auth), model)
    }

    /// INVARIANT: Build the provider for a Claude gateway. Every request asks `access`
    /// for its bearer, so a refreshed token takes effect on the next call. The
    /// gateway's model listing is read on first use.
    pub fn claude_gateway(
        origin: GatewayOrigin,
        access: Arc<dyn GatewayAccess>,
        model: String,
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let served = ServedModels::new(origin.clone());
        Self::with_endpoint(
            Endpoint::ClaudeGateway {
                origin,
                access,
                served,
            },
            model,
        )
    }

    fn with_endpoint(
        endpoint: Endpoint,
        model: String,
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let streaming_client = reqwest::Client::builder()
            .pool_idle_timeout(Duration::from_secs(30))
            .build()?;
        Ok(Self {
            endpoint,
            model,
            streaming_client,
        })
    }
}

#[async_trait]
impl LlmProvider for AnthropicProvider {
    async fn route_available(
        &self,
        _provider: crate::llm::ProviderKind,
        _model: &str,
        wire_id: &str,
    ) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
        let Endpoint::ClaudeGateway { access, served, .. } = &self.endpoint else {
            return Ok(true);
        };
        let policy = access.managed_settings().await?;
        match served
            .wire_id(wire_id, access.as_ref(), policy.as_ref())
            .await
        {
            Ok(_) => Ok(true),
            Err(e) if e.is::<crate::llm::ModelNotServed>() => Ok(false),
            Err(e) => Err(e),
        }
    }

    fn resolved_provider_kind(
        &self,
        _selection: &ModelSelection<'_>,
    ) -> Result<Option<crate::llm::ProviderKind>, Box<dyn std::error::Error + Send + Sync>> {
        Ok(Some(self.endpoint.provider_kind()))
    }

    fn default_model(&self) -> &str {
        &self.model
    }

    async fn chat(
        &self,
        messages: Vec<Message>,
        tools: Vec<ToolDefinition>,
        selection: ModelSelection<'_>,
        system_prompt: Option<&str>,
        on_token: Option<TokenCallback>,
        _call: crate::llm::metered::CallToken,
    ) -> Result<LlmResponse, Box<dyn std::error::Error + Send + Sync>> {
        self.chat_anthropic(messages, tools, selection, system_prompt, on_token)
            .await
    }

    async fn prefill(
        &self,
        body: &crate::llm::prefill::PrefillBody,
        _call: crate::llm::metered::CallToken,
    ) -> Result<LlmResponse, Box<dyn std::error::Error + Send + Sync>> {
        self.prefill_anthropic(body).await
    }
}
