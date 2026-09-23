use crate::llm::anthropic::AnthropicProvider;
use crate::llm::model_registry::{
    resolve_route, ModelRegistry, ProviderKind, RouteEntry, Unconfigured,
};
use crate::llm::openai::OpenAiProvider;
use crate::llm::provider::{
    LlmProvider, LlmResponse, Message, ModelSelection, TokenCallback, ToolDefinition,
};
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
    /// xAI, an [`OpenAiProvider`] pointed at `api.x.ai/v1`, serving Grok.
    xai: Option<Arc<OpenAiProvider>>,
    /// OpenCode's keyless free tier, an [`OpenAiProvider`] pointed at
    /// `opencode.ai/zen/v1` with no credential. Present only when the user
    /// turned it on.
    opencode_free: Option<Arc<OpenAiProvider>>,
    /// A generic OpenAI-compatible local server — an [`OpenAiProvider`] pointed
    /// at the configured local base URL (default Ollama).
    local: Option<Arc<OpenAiProvider>>,
    registry: ModelRegistry,
    default_model: String,
}

impl RoutingProvider {
    #[allow(clippy::too_many_arguments)] // one argument per backend, plus the registry and default
    pub fn new(
        vertex: Option<VertexProvider>,
        openai: Option<OpenAiProvider>,
        anthropic: Option<AnthropicProvider>,
        openrouter: Option<OpenAiProvider>,
        xai: Option<OpenAiProvider>,
        opencode_free: Option<OpenAiProvider>,
        local: Option<OpenAiProvider>,
        registry: ModelRegistry,
        default_model: String,
    ) -> Self {
        Self {
            vertex: vertex.map(Arc::new),
            openai: openai.map(Arc::new),
            anthropic: anthropic.map(Arc::new),
            openrouter: openrouter.map(Arc::new),
            xai: xai.map(Arc::new),
            opencode_free: opencode_free.map(Arc::new),
            local: local.map(Arc::new),
            registry,
            default_model,
        }
    }

    /// The backend instance serving `kind`, or `None` when it is not configured.
    ///
    /// The single place that maps a [`ProviderKind`] onto a provider, so
    /// [`Self::is_configured`], [`Self::configured_providers`] and the routing
    /// itself cannot disagree about what this router holds.
    fn backend(&self, kind: ProviderKind) -> Option<&dyn LlmProvider> {
        match kind {
            ProviderKind::Vertex => self.vertex.as_deref().map(|p| p as &dyn LlmProvider),
            ProviderKind::Anthropic => self.anthropic.as_deref().map(|p| p as &dyn LlmProvider),
            ProviderKind::OpenAi => self.openai.as_deref().map(|p| p as &dyn LlmProvider),
            ProviderKind::OpenRouter => self.openrouter.as_deref().map(|p| p as &dyn LlmProvider),
            ProviderKind::XAi => self.xai.as_deref().map(|p| p as &dyn LlmProvider),
            ProviderKind::OpenCodeFree => {
                self.opencode_free.as_deref().map(|p| p as &dyn LlmProvider)
            }
            ProviderKind::Local => self.local.as_deref().map(|p| p as &dyn LlmProvider),
        }
    }

    fn is_configured(&self, kind: ProviderKind) -> bool {
        self.backend(kind).is_some()
    }

    /// Snap a reasoning effort onto the closest tier the route's backend
    /// actually supports.
    ///
    /// **This is the chokepoint**, and it is here rather than in each provider
    /// because this is the only layer that knows the [`ProviderKind`]: the
    /// OpenAI, OpenRouter and local backends are all the same
    /// [`OpenAiProvider`] struct with a different base URL, so a rule inside it
    /// can only see the model id and cannot tell whose vocabulary applies. It
    /// covers every producer of an effort at once, the chat picker, a trigger's
    /// pinned effort, the `preferences` tool, the HTTP API, and a per-thread
    /// value remembered from a model the thread no longer runs on.
    ///
    /// It reads the ROUTE, so a model served by two backends is clamped against
    /// the one the turn will actually reach.
    fn effort_for_route<'a>(&self, route: &RouteEntry, effort: Option<&'a str>) -> Option<&'a str> {
        let effort = effort?;
        let model = route.wire_id.as_str();
        let Some(clamped) = clamp_effort(effort, route.provider, model) else {
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

    /// Resolve which backend serves `model` and what id to send it, or say what
    /// the user has to configure.
    ///
    /// `chosen` is the turn's own provider pick, which outranks the row's
    /// stored one. Either is honoured or refused, never substituted: a turn
    /// pinned to a parked backend errors rather than leaving for another vendor.
    fn route_for(
        &self,
        model: &str,
        chosen: Option<ProviderKind>,
    ) -> Result<RouteEntry, Box<dyn std::error::Error + Send + Sync>> {
        resolve_route(&self.registry, model, chosen, |kind| {
            self.is_configured(kind)
        })
        .map_err(|Unconfigured(kind)| unconfigured_message(kind).into())
    }
}

/// What the user has to set up for `kind` to serve a turn.
///
/// The Vertex project id is an ALREADY-RESOLVED value. It comes from
/// `VERTEX_PROJECT_ID`, then the ADC `quota_project_id` or gcloud config file,
/// then a `gcloud config` subprocess. Naming only the env var would send a user
/// who authenticated with ADC to fix the wrong thing.
fn unconfigured_message(kind: ProviderKind) -> &'static str {
    match kind {
        ProviderKind::Vertex => "Vertex AI model requested but no Google Cloud project is configured (set VERTEX_PROJECT_ID or run `gcloud auth application-default login`)",
        ProviderKind::Anthropic => "Anthropic model requested but no Anthropic credential is configured (Settings → Models → Providers) and ANTHROPIC_API_KEY is not set",
        ProviderKind::OpenAi => "OpenAI model requested but no OpenAI credential is configured (Settings → Models → Providers) and OPENAI_API_KEY is not set",
        ProviderKind::OpenRouter => "OpenRouter model requested but no OpenRouter credential is configured (Settings → Models → Providers) and LUCIDOS_OPENROUTER_API_KEY is not set",
        ProviderKind::XAi => "xAI model requested but no xAI credential is configured (Settings → Models → Providers) and LUCIDOS_XAI_API_KEY is not set",
        ProviderKind::OpenCodeFree => "Free model requested but the keyless OpenCode Free tier is turned off (Settings → Models → Providers)",
        ProviderKind::Local => "Local model requested but the local OpenAI-compatible provider is not configured (Settings → Models → Providers)",
    }
}

#[async_trait]
impl LlmProvider for RoutingProvider {
    async fn chat(
        &self,
        messages: Vec<Message>,
        tools: Vec<ToolDefinition>,
        selection: ModelSelection<'_>,
        system_prompt: Option<&str>,
        on_token: Option<TokenCallback>,
    ) -> Result<LlmResponse, Box<dyn std::error::Error + Send + Sync>> {
        let model = selection.model.unwrap_or(&self.default_model);
        let route = self.route_for(model, selection.provider)?;
        // `route_for` already refused an unconfigured backend, so the miss here
        // is unreachable. It resolves through the same `backend`, and saying
        // what to configure beats an `expect` if the two ever part company.
        let provider = self
            .backend(route.provider)
            .ok_or_else(|| unconfigured_message(route.provider))?;
        // The leaf serves ONE backend, so it is handed the resolved route: its
        // own wire id, and the effort snapped onto what that backend accepts.
        let resolved = ModelSelection::model(&route.wire_id)
            .with_effort(self.effort_for_route(&route, selection.reasoning_effort));
        provider
            .chat(messages, tools, resolved, system_prompt, on_token)
            .await
    }

    fn default_model(&self) -> &str {
        &self.default_model
    }

    fn configured_providers(&self) -> Option<Vec<ProviderKind>> {
        Some(
            ProviderKind::ALL
                .into_iter()
                .filter(|kind| self.is_configured(*kind))
                .collect(),
        )
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
                .map(|(id, provider)| (id.to_string(), ModelRouting::single(*provider, *id)))
                .collect::<HashMap<_, _>>(),
        ));
        RoutingProvider::new(
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            registry,
            "claude-opus-5".to_string(),
        )
    }

    impl RoutingProvider {
        /// Clamp `effort` for `model` against the route the registry names,
        /// ignoring what this router has configured.
        ///
        /// The clamp itself reads only the route, so the tests below exercise it
        /// on a provider-less router. `chat` resolves against the configured set
        /// first, and refuses before reaching the clamp when nothing serves.
        fn effort_for_model<'a>(&self, model: &str, effort: Option<&'a str>) -> Option<&'a str> {
            let route = resolve_route(&self.registry, model, None, |_| true)
                .expect("every provider counts as configured here");
            self.effort_for_route(&route, effort)
        }
    }

    /// The chokepoint. Each model's effort is snapped using the provider its
    /// registry row names, not the shape of its id, so the same effort resolves
    /// differently per backend.
    #[test]
    fn effort_is_clamped_against_the_registry_provider() {
        let router = router(&[
            ("claude-opus-5", ProviderKind::Vertex),
            ("gemini-3.5-flash", ProviderKind::Vertex),
            ("gpt-5.6-sol", ProviderKind::OpenAi),
            ("gpt-5.4", ProviderKind::OpenAi),
            ("muse-glimmer:30b-mlx", ProviderKind::Local),
            ("z-ai/glm-5.2", ProviderKind::OpenRouter),
            ("grok-4.6", ProviderKind::XAi),
            // Grok through OpenRouter is a different row on a different
            // backend, and must keep resolving there.
            ("x-ai/grok-4.6", ProviderKind::OpenRouter),
            // The free tier splits per model: Ox Alpha keeps `max`, and every
            // other free id snaps to the conservative ceiling.
            ("x-preview-f-free", ProviderKind::OpenCodeFree),
            ("laguna-s-2.1-free", ProviderKind::OpenCodeFree),
        ]);
        for (model, expected) in [
            ("claude-opus-5", "max"),
            ("gemini-3.5-flash", "high"),
            ("gpt-5.6-sol", "max"),
            ("gpt-5.4", "xhigh"),
            ("muse-glimmer:30b-mlx", "high"),
            ("z-ai/glm-5.2", "high"),
            ("grok-4.6", "high"),
            ("x-ai/grok-4.6", "high"),
            ("x-preview-f-free", "max"),
            ("laguna-s-2.1-free", "high"),
        ] {
            assert_eq!(
                router.effort_for_model(model, Some("max")),
                Some(expected),
                "{model}"
            );
        }
    }

    /// The regression, at the layer that prevents it. A turn on a server that is
    /// not OpenAI's must never carry the OpenAI-proprietary `xhigh`, whichever
    /// tier the caller asked for. The local model is the case that failed; xAI
    /// is the same shape, an OpenAI-compatible third party.
    #[test]
    fn a_third_party_model_never_leaves_the_chokepoint_carrying_xhigh() {
        let router = router(&[
            ("muse-glimmer:30b-mlx", ProviderKind::Local),
            ("grok-4.6", ProviderKind::XAi),
        ]);
        for model in ["muse-glimmer:30b-mlx", "grok-4.6"] {
            for effort in crate::llm::reasoning::EFFORT_LADDER {
                let sent = router.effort_for_model(model, Some(effort));
                assert_ne!(sent, Some("xhigh"), "{model} asked for {effort}");
            }
        }
    }

    /// With the free tier off, its models are neither offered nor silently
    /// routed somewhere else. The picker filters on `configured_providers`, and
    /// a turn that gets through anyway is told where the switch is.
    #[test]
    fn a_free_model_is_hidden_and_actionable_while_the_tier_is_off() {
        let router = router(&[("laguna-s-2.1-free", ProviderKind::OpenCodeFree)]);
        assert_eq!(router.configured_providers(), Some(Vec::new()));
        let Err(err) = router.route_for("laguna-s-2.1-free", None) else {
            panic!("the tier is off, so there is no provider to route to");
        };
        let err = err.to_string();
        assert!(err.contains("OpenCode Free"), "{err}");
        assert!(err.contains("Settings → Models → Providers"), "{err}");
    }

    /// The honoured-or-refused rule, at the layer that enforces it.
    ///
    /// A model routed to both Vertex and Anthropic, with only Anthropic
    /// configured, runs on Anthropic. Pin it to Vertex and the turn is REFUSED,
    /// naming Vertex, rather than quietly leaving for the other vendor.
    #[test]
    fn a_pinned_provider_is_honoured_or_refused_but_never_substituted() {
        let registry: ModelRegistry = Arc::new(RwLock::new(HashMap::from([(
            "claude-opus-5".to_string(),
            ModelRouting {
                routes: vec![
                    RouteEntry::new(ProviderKind::Vertex, "claude-opus-5"),
                    RouteEntry::new(ProviderKind::Anthropic, "claude-opus-5"),
                ],
                preferred: None,
            },
        )])));
        let router = RoutingProvider::new(
            None,
            None,
            Some(
                AnthropicProvider::new(
                    crate::llm::AnthropicAuth::ApiKey("k".to_string()),
                    "claude-opus-5".to_string(),
                )
                .expect("build the anthropic provider"),
            ),
            None,
            None,
            None,
            None,
            registry,
            "claude-opus-5".to_string(),
        );

        // No pick: the first CONFIGURED route wins, so Vertex being listed
        // first does not strand a workspace holding only an Anthropic key.
        let route = router
            .route_for("claude-opus-5", None)
            .expect("anthropic serves it");
        assert_eq!(route.provider, ProviderKind::Anthropic);

        // Pinned to the parked backend: refused, and the message names it.
        let Err(err) = router.route_for("claude-opus-5", Some(ProviderKind::Vertex)) else {
            panic!("a pin to an unconfigured backend must refuse");
        };
        assert!(err.to_string().contains("Vertex AI"), "{err}");

        // The same refusal through `chat`, fed the owned selection a turn
        // resolves and stamps. That is the seam a turn crosses, so a provider
        // dropped on the way would run this turn on Anthropic instead.
        let resolved = crate::core::ResolvedModelSelection {
            model: Some("claude-opus-5".to_string()),
            reasoning_effort: None,
            provider: Some("vertex".to_string()),
        };
        let refused = futures::executor::block_on(router.chat(
            vec![],
            vec![],
            resolved.as_selection(),
            None,
            None,
        ));
        let Err(err) = refused else {
            panic!("a turn pinned to an unconfigured backend must refuse");
        };
        assert!(err.to_string().contains("Vertex AI"), "{err}");
    }

    /// The wire carries the ROUTE's id, not the row's. That is what lets a row
    /// keep one identity while a backend spelling it differently still works.
    #[test]
    fn the_route_decides_the_id_on_the_wire() {
        let registry: ModelRegistry = Arc::new(RwLock::new(HashMap::from([(
            "claude-opus-5-5".to_string(),
            ModelRouting {
                routes: vec![RouteEntry::new(
                    ProviderKind::OpenRouter,
                    "anthropic/claude-opus-5-5",
                )],
                preferred: None,
            },
        )])));
        let router = RoutingProvider::new(
            None,
            None,
            None,
            Some(
                OpenAiProvider::new_with_base_url(
                    "k".to_string(),
                    "x".to_string(),
                    crate::llm::OPENROUTER_BASE_URL,
                    Vec::new(),
                    true,
                )
                .expect("build the openrouter provider"),
            ),
            None,
            None,
            None,
            registry,
            "claude-opus-5-5".to_string(),
        );
        let route = router
            .route_for("claude-opus-5-5", None)
            .expect("openrouter serves it");
        assert_eq!(route.wire_id, "anthropic/claude-opus-5-5");
        // And the effort clamp reads that id's backend, so OpenAI's own
        // `xhigh` is never offered on a third-party server.
        assert_eq!(router.effort_for_route(&route, Some("xhigh")), Some("high"));
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
            router.effort_for_model("claude-opus-5", Some("max")),
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
