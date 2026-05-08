use crate::llm::openai::OpenAiProvider;
use crate::llm::provider::{LlmProvider, LlmResponse, Message, TokenCallback, ToolDefinition};
use crate::llm::vertex::VertexProvider;
use async_trait::async_trait;
use std::sync::Arc;

/// Routes LLM requests to the correct provider based on model name prefix.
/// Holds both Vertex AI (Claude/Gemini) and OpenAI providers when configured.
pub struct RoutingProvider {
    vertex: Option<Arc<VertexProvider>>,
    openai: Option<Arc<OpenAiProvider>>,
    default_model: String,
}

impl RoutingProvider {
    pub fn new(
        vertex: Option<VertexProvider>,
        openai: Option<OpenAiProvider>,
        default_model: String,
    ) -> Self {
        Self {
            vertex: vertex.map(Arc::new),
            openai: openai.map(Arc::new),
            default_model,
        }
    }

    fn provider_for_model(
        &self,
        model: &str,
    ) -> Result<&dyn LlmProvider, Box<dyn std::error::Error + Send + Sync>> {
        if model.starts_with("gpt-") {
            match &self.openai {
                Some(p) => Ok(p.as_ref()),
                None => Err("OpenAI model requested but OPENAI_API_KEY is not configured".into()),
            }
        } else {
            match &self.vertex {
                Some(p) => Ok(p.as_ref()),
                None => {
                    Err("Vertex AI model requested but VERTEX_PROJECT_ID is not configured".into())
                }
            }
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
}
