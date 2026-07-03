use crate::llm::anthropic::AnthropicProvider;
use crate::llm::model_registry::{provider_kind_for, ModelRegistry, ProviderKind};
use crate::llm::openai::OpenAiProvider;
use crate::llm::provider::{LlmProvider, LlmResponse, Message, TokenCallback, ToolDefinition};
use crate::llm::vertex::VertexProvider;
use async_trait::async_trait;
use std::sync::Arc;

/// Routes LLM requests to the correct provider for the requested model. The
/// model → provider mapping comes from the database-backed [`ModelRegistry`]
/// (Settings → Models), with a prefix-heuristic fallback for ids not in the
/// table. Holds whichever providers are configured.
pub struct RoutingProvider {
    vertex: Option<Arc<VertexProvider>>,
    openai: Option<Arc<OpenAiProvider>>,
    anthropic: Option<Arc<AnthropicProvider>>,
    /// OpenRouter — an [`OpenAiProvider`] pointed at `openrouter.ai/api/v1`.
    openrouter: Option<Arc<OpenAiProvider>>,
    /// A generic OpenAI-compatible local server — an [`OpenAiProvider`] pointed
    /// at the configured local base URL (default Ollama).
    local: Option<Arc<OpenAiProvider>>,
    registry: ModelRegistry,
    default_model: String,
}

impl RoutingProvider {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        vertex: Option<VertexProvider>,
        openai: Option<OpenAiProvider>,
        anthropic: Option<AnthropicProvider>,
        openrouter: Option<OpenAiProvider>,
        local: Option<OpenAiProvider>,
        registry: ModelRegistry,
        default_model: String,
    ) -> Self {
        Self {
            vertex: vertex.map(Arc::new),
            openai: openai.map(Arc::new),
            anthropic: anthropic.map(Arc::new),
            openrouter: openrouter.map(Arc::new),
            local: local.map(Arc::new),
            registry,
            default_model,
        }
    }

    fn provider_for_model(
        &self,
        model: &str,
    ) -> Result<&dyn LlmProvider, Box<dyn std::error::Error + Send + Sync>> {
        match provider_kind_for(&self.registry, model) {
            ProviderKind::OpenAi => self.openai.as_deref().map(|p| p as &dyn LlmProvider).ok_or_else(|| {
                "OpenAI model requested but no OpenAI credential is configured (Settings → Models → Providers) and OPENAI_API_KEY is not set".into()
            }),
            ProviderKind::Anthropic => self.anthropic.as_deref().map(|p| p as &dyn LlmProvider).ok_or_else(|| {
                "Anthropic model requested but no Anthropic credential is configured (Settings → Models → Providers)".into()
            }),
            ProviderKind::OpenRouter => self.openrouter.as_deref().map(|p| p as &dyn LlmProvider).ok_or_else(|| {
                "OpenRouter model requested but no OpenRouter credential is configured (Settings → Models → Providers) and LUCIDOS_OPENROUTER_API_KEY is not set".into()
            }),
            ProviderKind::Local => self.local.as_deref().map(|p| p as &dyn LlmProvider).ok_or_else(|| {
                "Local model requested but the local OpenAI-compatible provider is not configured (Settings → Models → Providers)".into()
            }),
            ProviderKind::Vertex => self
                .vertex
                .as_deref()
                .map(|p| p as &dyn LlmProvider)
                .ok_or_else(|| "Vertex AI model requested but VERTEX_PROJECT_ID is not configured".into()),
        }
    }
}

#[async_trait]
impl LlmProvider for RoutingProvider {
    async fn chat(
        &self,
        messages: Vec<Message>,
        tools: Vec<ToolDefinition>,
        model_override: Option<&str>,
        system_prompt: Option<&str>,
        on_token: Option<TokenCallback>,
        reasoning_effort: Option<&str>,
    ) -> Result<LlmResponse, Box<dyn std::error::Error + Send + Sync>> {
        let model = model_override.unwrap_or(&self.default_model);
        let provider = self.provider_for_model(model)?;
        provider
            .chat(
                messages,
                tools,
                Some(model),
                system_prompt,
                on_token,
                reasoning_effort,
            )
            .await
    }

    fn default_model(&self) -> &str {
        &self.default_model
    }

    fn configured_providers(&self) -> Option<Vec<ProviderKind>> {
        let mut kinds = Vec::new();
        if self.vertex.is_some() {
            kinds.push(ProviderKind::Vertex);
        }
        if self.anthropic.is_some() {
            kinds.push(ProviderKind::Anthropic);
        }
        if self.openai.is_some() {
            kinds.push(ProviderKind::OpenAi);
        }
        if self.openrouter.is_some() {
            kinds.push(ProviderKind::OpenRouter);
        }
        if self.local.is_some() {
            kinds.push(ProviderKind::Local);
        }
        Some(kinds)
    }
}
