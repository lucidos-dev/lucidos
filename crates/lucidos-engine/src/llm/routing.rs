use crate::llm::anthropic::AnthropicProvider;
use crate::llm::model_registry::{
    current_model, default_effort, resolve_route, ModelRegistry, ProviderKind, RouteEntry,
    Unconfigured,
};
use crate::llm::openai::OpenAiProvider;
use crate::llm::provider::{
    LlmProvider, LlmResponse, Message, ModelSelection, TokenCallback, ToolDefinition,
};
use crate::llm::reasoning::clamp_effort;
use crate::llm::served_model::served_as_sent;
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
    ///
    /// No effort runs `model` at its *default effort*, so every caller follows
    /// one rule. A model with none sends none, and its provider decides.
    fn effort_for_route<'a>(
        &self,
        model: &str,
        route: &RouteEntry,
        effort: Option<&'a str>,
    ) -> Option<&'a str> {
        let effort = effort.or_else(|| default_effort(&self.registry, model))?;
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

impl RoutingProvider {
    /// The model a selection is sent as, and the route that serves it. A
    /// retired model is sent as its successor, never as the id a provider
    /// might route elsewhere (ADR 0418).
    fn resolve(
        &self,
        selection: &ModelSelection<'_>,
    ) -> Result<(String, RouteEntry), Box<dyn std::error::Error + Send + Sync>> {
        let model = current_model(
            &self.registry,
            selection.model.unwrap_or(&self.default_model),
        );
        let route = self.route_for(&model, selection.provider)?;
        Ok((model, route))
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
        call: crate::llm::metered::CallToken,
    ) -> Result<LlmResponse, Box<dyn std::error::Error + Send + Sync>> {
        let (model, route) = self.resolve(&selection)?;
        let requested = selection.model.unwrap_or(&self.default_model);
        if requested != model {
            crate::log!("[Routing] '{requested}' is retired; sending its successor '{model}'");
        }
        // `route_for` already refused an unconfigured backend, so the miss here
        // is unreachable. It resolves through the same `backend`, and saying
        // what to configure beats an `expect` if the two ever part company.
        let provider = self
            .backend(route.provider)
            .ok_or_else(|| unconfigured_message(route.provider))?;
        // The leaf serves ONE backend, so it is handed the resolved route: its
        // own wire id, and the effort snapped onto what that backend accepts.
        let resolved = ModelSelection::model(&route.wire_id)
            .with_effort(self.effort_for_route(&model, &route, selection.reasoning_effort))
            .with_attempt_timeout(selection.attempt_timeout);
        let response = provider
            .chat(messages, tools, resolved, system_prompt, on_token, call)
            .await?;
        if let Some(served) = response.served_model.as_deref() {
            if !served_as_sent(&route.wire_id, served) {
                crate::log!(
                    "[Routing] WARNING: sent '{}' to {}, but '{served}' served it",
                    route.wire_id,
                    route.provider.as_str()
                );
            }
        }
        Ok(response)
    }

    fn effort_sent(&self, selection: &ModelSelection<'_>) -> Option<String> {
        match self.resolve(selection) {
            Ok((model, route)) => self.effort_for_route(&model, &route, selection.reasoning_effort),
            Err(_) => selection.reasoning_effort,
        }
        .map(str::to_string)
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
            self.effort_for_route(model, &route, effort)
        }
    }

    /// The chokepoint. Each model's effort is snapped using the provider its
    /// registry row names, not the shape of its id, so the same effort resolves
    /// differently per backend.
    #[test]
    fn effort_is_clamped_against_the_registry_provider() {
        let router = router(&[
            ("claude-opus-5", ProviderKind::Vertex),
            ("gemini-3.8-flash", ProviderKind::Vertex),
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
            ("gemini-3.8-flash", "high"),
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
                vision: false,
                default_effort: None,
                successor: None,
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
            crate::llm::metered::CallToken::for_test(),
        ));
        let Err(err) = refused else {
            panic!("a turn pinned to an unconfigured backend must refuse");
        };
        assert!(err.to_string().contains("Vertex AI"), "{err}");
    }

    /// An auxiliary call's attempt cap crosses the router and bounds the leaf's
    /// request. The backend accepts and never answers, so without the cap each
    /// attempt would wait out the 120s header bound.
    #[tokio::test]
    async fn the_attempt_cap_crosses_the_router_to_the_leaf() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let mut held = Vec::new();
            while let Ok((stream, _)) = listener.accept().await {
                held.push(stream);
            }
        });
        let local = OpenAiProvider::new_with_base_url(
            String::new(),
            "local-model".to_string(),
            &format!("http://{addr}/v1"),
            Vec::new(),
            true,
        )
        .expect("build the local provider");
        let registry: ModelRegistry = Arc::new(RwLock::new(HashMap::from([(
            "local-model".to_string(),
            ModelRouting::single(ProviderKind::Local, "local-model"),
        )])));
        let router = RoutingProvider::new(
            None,
            None,
            None,
            None,
            None,
            None,
            Some(local),
            registry,
            "local-model".to_string(),
        );
        let cap = std::time::Duration::from_millis(100);
        let call = router.chat(
            vec![],
            vec![],
            ModelSelection::model("local-model").with_attempt_timeout(Some(cap)),
            None,
            None,
            crate::llm::metered::CallToken::for_test(),
        );
        // Every attempt is capped, so the call ends after the retries' backoff.
        let backoff: std::time::Duration = (1..=crate::llm::MAX_RETRIES)
            .map(|attempt| crate::llm::retry_delay(attempt, 1))
            .sum();
        let bound = backoff + std::time::Duration::from_secs(10);
        let outcome = tokio::time::timeout(bound, call)
            .await
            .expect("each attempt must end at the cap, not the header bound");
        assert!(outcome.is_err(), "the backend never answers");
    }

    /// An OpenAI-compatible backend at a local port that answers every request
    /// with `status` and `body`, and counts the requests it saw.
    async fn answering_backend(
        status: &'static str,
        body: &'static str,
    ) -> (OpenAiProvider, Arc<std::sync::atomic::AtomicUsize>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let hits = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let seen = hits.clone();
        tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                seen.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                let mut request = [0u8; 16 * 1024];
                let _ = stream.read(&mut request).await;
                let reply = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\n\
                     Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(reply.as_bytes()).await;
            }
        });
        let provider = OpenAiProvider::new_with_base_url(
            "k".to_string(),
            "unused".to_string(),
            &format!("http://{addr}/v1"),
            Vec::new(),
            true,
        )
        .expect("build the stub provider");
        (provider, hits)
    }

    /// A not-found answer on the first route ends the call there (ADR 0247).
    /// The second route belongs to another vendor, so moving the turn to it
    /// would send the user's data somewhere they never chose.
    #[tokio::test]
    async fn a_not_served_model_never_moves_to_the_next_route() {
        let (openrouter, refused) = answering_backend(
            "404 Not Found",
            r#"{"error":{"message":"No endpoints found for vendor/some-chat.","code":404}}"#,
        )
        .await;
        let (local, untouched) = answering_backend("200 OK", "{}").await;
        let registry: ModelRegistry = Arc::new(RwLock::new(HashMap::from([(
            "some-model".to_string(),
            ModelRouting {
                routes: vec![
                    RouteEntry::new(ProviderKind::OpenRouter, "vendor/some-chat"),
                    RouteEntry::new(ProviderKind::Local, "some-model"),
                ],
                preferred: None,
                vision: false,
                default_effort: None,
                successor: None,
            },
        )])));
        let router = RoutingProvider::new(
            None,
            None,
            None,
            Some(openrouter),
            None,
            None,
            Some(local),
            registry,
            "some-model".to_string(),
        );
        let err = router
            .chat(
                vec![],
                vec![],
                ModelSelection::model("some-model"),
                None,
                None,
                crate::llm::metered::CallToken::for_test(),
            )
            .await
            .expect_err("the first route refused the model");
        assert!(err.is::<crate::llm::ModelNotServed>(), "{err}");
        assert_eq!(refused.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert_eq!(untouched.load(std::sync::atomic::Ordering::SeqCst), 0);
    }

    /// A retired model is sent as its successor, at an effort the successor
    /// takes. Gemini 3.5 Flash took `none`; 3.8 Flash refuses it.
    #[test]
    fn a_retired_model_is_sent_as_its_successor() {
        let registry: ModelRegistry = Arc::new(RwLock::new(HashMap::from([
            (
                "gemini-3.5-flash".to_string(),
                ModelRouting {
                    successor: Some("gemini-3.8-flash".to_string()),
                    ..ModelRouting::single(ProviderKind::Vertex, "gemini-3.5-flash")
                },
            ),
            (
                "gemini-3.8-flash".to_string(),
                ModelRouting::single(ProviderKind::Vertex, "gemini-3.8-flash"),
            ),
        ])));
        let router = RoutingProvider::new(
            Some(
                VertexProvider::new("my-project".into(), "eu".into(), "gemini-3.8-flash".into())
                    .expect("build the vertex provider"),
            ),
            None,
            None,
            None,
            None,
            None,
            None,
            registry,
            "claude-opus-5".to_string(),
        );
        let selection = ModelSelection::model("gemini-3.5-flash").with_effort(Some("none"));
        let (model, route) = router.resolve(&selection).expect("vertex serves it");
        assert_eq!(model, "gemini-3.8-flash");
        assert_eq!(route.wire_id, "gemini-3.8-flash");
        assert_eq!(router.effort_sent(&selection).as_deref(), Some("low"));
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
                vision: false,
                default_effort: None,
                successor: None,
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
        assert_eq!(
            router.effort_for_route("claude-opus-5-5", &route, Some("xhigh")),
            Some("high")
        );
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

    /// The cost record names the tier the router sent: filled from the
    /// default effort, and snapped onto the route.
    #[test]
    fn the_effort_sent_is_the_routed_one() {
        let local = OpenAiProvider::new_with_base_url(
            String::new(),
            "local-model".to_string(),
            "http://127.0.0.1:9/v1",
            Vec::new(),
            true,
        )
        .expect("build the local provider");
        let mut row = ModelRouting::single(ProviderKind::Local, "local-model");
        row.default_effort = Some("medium");
        let registry: ModelRegistry = Arc::new(RwLock::new(HashMap::from([(
            "local-model".to_string(),
            row,
        )])));
        let router = RoutingProvider::new(
            None,
            None,
            None,
            None,
            None,
            None,
            Some(local),
            registry,
            "local-model".to_string(),
        );
        let sent = |effort| router.effort_sent(&ModelSelection::default().with_effort(effort));
        assert_eq!(sent(None).as_deref(), Some("medium"));
        assert_eq!(sent(Some("max")).as_deref(), Some("high"));
    }

    /// No effort runs a model at its default effort. A model with none stays
    /// without one, so its provider decides, rather than the router inventing
    /// a tier.
    #[test]
    fn no_effort_runs_the_default_effort_or_stays_absent() {
        let router = router(&[
            ("gpt-5.4", ProviderKind::OpenAi),
            ("claude-opus-5-5", ProviderKind::Vertex),
        ]);
        router
            .registry
            .write()
            .unwrap()
            .get_mut("claude-opus-5-5")
            .unwrap()
            .default_effort = Some("medium");
        assert_eq!(router.effort_for_model("gpt-5.4", None), None);
        assert_eq!(
            router.effort_for_model("claude-opus-5-5", None),
            Some("medium")
        );
        assert_eq!(
            router.effort_for_model("claude-opus-5-5", Some("max")),
            Some("max")
        );
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
