use crate::llm::anthropic::AnthropicProvider;
use crate::llm::model_registry::{provider_kind_for, ModelRegistry, ProviderKind};
use crate::llm::openai::OpenAiProvider;
use crate::llm::provider::{LlmProvider, LlmResponse, Message, TokenCallback, ToolDefinition};
use crate::llm::reasoning::clamp_effort;
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

    /// Snap a reasoning effort onto the closest tier the model's resolved
    /// provider actually supports.
    ///
    /// **This is the chokepoint**, and it is here rather than in each provider
    /// because this is the only layer that knows the [`ProviderKind`]: the
    /// OpenAI, OpenRouter and local backends are all the same
    /// [`OpenAiProvider`] struct with a different base URL, so a rule inside it
    /// can only see the model id and cannot tell whose vocabulary applies. It
    /// covers every producer of an effort at once, the chat picker, a trigger's
    /// pinned effort, the `preferences` tool, the HTTP API, and a per-thread
    /// value remembered from a model the thread no longer runs on.
    fn effort_for_model<'a>(&self, model: &str, effort: Option<&'a str>) -> Option<&'a str> {
        let effort = effort?;
        let Some(clamped) = clamp_effort(effort, provider_kind_for(&self.registry, model), model)
        else {
            // Not one of our tiers at all, so there is nothing to snap it onto.
            // Send no effort and let the provider default apply, rather than
            // guessing a tier a typo would then be billed for.
            crate::log!(
                "[Routing] dropping unrecognised reasoning effort '{}' for '{}'; provider default applies",
                effort,
                model
            );
            return None;
        };
        if clamped != effort {
            crate::log!(
                "[Routing] reasoning effort '{}' is unavailable on '{}'; using closest supported '{}'",
                effort,
                model,
                clamped
            );
        }
        Some(clamped)
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
                "Anthropic model requested but no Anthropic credential is configured (Settings → Models → Providers) and ANTHROPIC_API_KEY is not set".into()
            }),
            ProviderKind::OpenRouter => self.openrouter.as_deref().map(|p| p as &dyn LlmProvider).ok_or_else(|| {
                "OpenRouter model requested but no OpenRouter credential is configured (Settings → Models → Providers) and LUCIDOS_OPENROUTER_API_KEY is not set".into()
            }),
            ProviderKind::Local => self.local.as_deref().map(|p| p as &dyn LlmProvider).ok_or_else(|| {
                "Local model requested but the local OpenAI-compatible provider is not configured (Settings → Models → Providers)".into()
            }),
            // The project id is an ALREADY-RESOLVED value (`VERTEX_PROJECT_ID`
            // › ADC `quota_project_id` / gcloud config file › `gcloud config`
            // subprocess), so naming only the env var sends a user who
            // authenticated with ADC to fix the wrong thing.
            ProviderKind::Vertex => self
                .vertex
                .as_deref()
                .map(|p| p as &dyn LlmProvider)
                .ok_or_else(|| "Vertex AI model requested but no Google Cloud project is configured (set VERTEX_PROJECT_ID or run `gcloud auth application-default login`)".into()),
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
        let reasoning_effort = self.effort_for_model(model, reasoning_effort);
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::model_registry::ModelRouting;
    use std::collections::HashMap;
    use std::sync::RwLock;

    /// A provider-less router: the clamp reads only the registry, so no backend
    /// needs configuring to exercise it.
    fn router(rows: &[(&str, ProviderKind)]) -> RoutingProvider {
        let registry: ModelRegistry = Arc::new(RwLock::new(
            rows.iter()
                .map(|(id, provider)| {
                    (
                        id.to_string(),
                        ModelRouting {
                            provider: *provider,
                            context_window: None,
                        },
                    )
                })
                .collect::<HashMap<_, _>>(),
        ));
        RoutingProvider::new(
            None,
            None,
            None,
            None,
            None,
            registry,
            "claude-opus-5@default".to_string(),
        )
    }

    /// The chokepoint. Each model's effort is snapped using the provider its
    /// registry row names, not the shape of its id, so the same effort resolves
    /// differently per backend.
    #[test]
    fn effort_is_clamped_against_the_registry_provider() {
        let router = router(&[
            ("claude-opus-5@default", ProviderKind::Vertex),
            ("gemini-3.5-flash", ProviderKind::Vertex),
            ("gpt-5.6-sol", ProviderKind::OpenAi),
            ("gpt-5.4", ProviderKind::OpenAi),
            ("muse-glimmer:30b-mlx", ProviderKind::Local),
            ("z-ai/glm-5.2", ProviderKind::OpenRouter),
        ]);
        for (model, expected) in [
            ("claude-opus-5@default", "max"),
            ("gemini-3.5-flash", "high"),
            ("gpt-5.6-sol", "max"),
            ("gpt-5.4", "xhigh"),
            ("muse-glimmer:30b-mlx", "high"),
            ("z-ai/glm-5.2", "high"),
        ] {
            assert_eq!(
                router.effort_for_model(model, Some("max")),
                Some(expected),
                "{model}"
            );
        }
    }

    /// The regression, at the layer that prevents it. A local model's turn must
    /// never carry `xhigh`, whichever tier the caller asked for.
    #[test]
    fn a_local_model_never_leaves_the_chokepoint_carrying_xhigh() {
        let router = router(&[("muse-glimmer:30b-mlx", ProviderKind::Local)]);
        for effort in crate::llm::reasoning::EFFORT_LADDER {
            let sent = router.effort_for_model("muse-glimmer:30b-mlx", Some(effort));
            assert_ne!(sent, Some("xhigh"), "asked for {effort}");
        }
    }

    /// A model with no registry row falls back to the same prefix heuristic
    /// routing uses, so its clamp matches the provider it will actually reach.
    #[test]
    fn an_unregistered_model_clamps_against_its_heuristic_provider() {
        let router = router(&[]);
        // `gpt-` → OpenAI, which tops out at xhigh below 5.6.
        assert_eq!(
            router.effort_for_model("gpt-5.4", Some("max")),
            Some("xhigh")
        );
        // Non-fable `claude-` → Vertex Claude, adaptive, so max survives.
        assert_eq!(
            router.effort_for_model("claude-opus-5@default", Some("max")),
            Some("max")
        );
    }

    /// No effort in, no effort out: the clamp must not invent one, or every
    /// caller that deliberately leaves the provider on its own default would
    /// start being told a tier.
    #[test]
    fn no_effort_stays_absent() {
        let router = router(&[("gpt-5.4", ProviderKind::OpenAi)]);
        assert_eq!(router.effort_for_model("gpt-5.4", None), None);
    }

    /// A string that is not one of our tiers is dropped here rather than
    /// guessed at, so the provider applies its own default.
    ///
    /// It really does reach this point: only the `preferences` LLM tool
    /// validates against the ladder, while `PUT /api/v1/preferences` and the
    /// `reasoning_effort` on `POST /api/v1/chat/stream` do not. Before the
    /// clamp existed such a value went to the wire and the provider rejected
    /// it, so the one thing this must NOT do is quietly promote it to a real
    /// tier the user then pays for.
    #[test]
    fn an_unrecognised_effort_is_dropped_not_promoted() {
        let router = router(&[("gpt-5.4", ProviderKind::OpenAi)]);
        for junk in ["", "ultra", "MAX"] {
            assert_eq!(
                router.effort_for_model("gpt-5.4", Some(junk)),
                None,
                "{junk:?}"
            );
        }
    }
}
