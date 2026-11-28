use crate::llm::model_registry::{
    default_effort, reaches, resolve_generation, resolve_route, vision_flag, Generation,
    ModelRegistry, ProviderKind, RouteEntry, Unconfigured,
};
use crate::llm::not_served::{ModelNotServed, NotServedLedger};
use crate::llm::provider::{
    forward, replace_image_blocks, LlmProvider, LlmResponse, Message, MessageContent,
    ModelSelection, SharedCallback, TokenCallback, ToolDefinition,
};
use crate::llm::reasoning::clamp_effort;
use crate::llm::served_model::served_as_sent;
use async_trait::async_trait;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

type BoxError = Box<dyn std::error::Error + Send + Sync>;

#[derive(Default)]
pub struct RoutingBackends {
    pub vertex: Option<Arc<dyn LlmProvider>>,
    pub openai: Option<Arc<dyn LlmProvider>>,
    pub anthropic: Option<Arc<dyn LlmProvider>>,
    pub openrouter: Option<Arc<dyn LlmProvider>>,
    pub xai: Option<Arc<dyn LlmProvider>>,
    pub opencode_free: Option<Arc<dyn LlmProvider>>,
    pub local: Option<Arc<dyn LlmProvider>>,
    pub claude_gateway: Option<Arc<dyn LlmProvider>>,
    pub chatgpt_plan: Option<Arc<dyn LlmProvider>>,
}

pub struct RoutingProvider {
    backends: RoutingBackends,
    registry: ModelRegistry,
    default_model: String,
    not_served: NotServedLedger,
}

impl RoutingProvider {
    pub fn new(backends: RoutingBackends, registry: ModelRegistry, default_model: String) -> Self {
        Self {
            backends,
            registry,
            default_model,
            not_served: NotServedLedger::default(),
        }
    }

    /// INVARIANT: Route around the not-found answers in `ledger`, and record new ones
    /// there. A shared ledger outlives this router, so a rebuild keeps it.
    pub fn observing(mut self, ledger: NotServedLedger) -> Self {
        self.not_served = ledger;
        self
    }

    fn backend(&self, kind: ProviderKind) -> Option<&dyn LlmProvider> {
        match kind {
            ProviderKind::Vertex => self.backends.vertex.as_deref(),
            ProviderKind::Anthropic => self.backends.anthropic.as_deref(),
            ProviderKind::OpenAi => self.backends.openai.as_deref(),
            ProviderKind::OpenRouter => self.backends.openrouter.as_deref(),
            ProviderKind::XAi => self.backends.xai.as_deref(),
            ProviderKind::OpenCodeFree => self.backends.opencode_free.as_deref(),
            ProviderKind::Local => self.backends.local.as_deref(),
            ProviderKind::ClaudeGateway => self.backends.claude_gateway.as_deref(),
            ProviderKind::ChatGptPlan => self.backends.chatgpt_plan.as_deref(),
        }
    }

    fn is_configured(&self, kind: ProviderKind) -> bool {
        self.backend(kind).is_some()
    }

    /// INVARIANT: Snap a reasoning effort onto the closest tier the route's backend
    /// actually supports.
    ///
    /// **This is the chokepoint** for effort clamping. The registry route
    /// supplies both the backend and wire model before a call is sent. It
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
            // INVARIANT: Not one of our tiers at all, so there is nothing to snap it onto.
            // Send no effort and let the provider default apply, rather than
            // guessing a tier a typo would then be billed for.
            crate::log_legacy!(
                "[Routing] dropping unrecognised reasoning effort '{}' for '{}'; provider default applies",
                effort,
                model
            );
            return None;
        };
        if clamped != effort {
            crate::log_legacy!(
                "[Routing] reasoning effort '{}' is unavailable on '{}'; using closest supported '{}'",
                effort,
                model,
                clamped
            );
        }
        Some(clamped)
    }

    fn messages_for_model(&self, model: &str, mut messages: Vec<Message>) -> Vec<Message> {
        if vision_flag(&self.registry, model) != Some(false) {
            return messages;
        }
        let note = || {
            format!(
                "[Image not sent: {model} does not read images. Turn on its vision flag in Settings → Models to send images.]"
            )
        };
        let withheld: usize = messages
            .iter_mut()
            .filter_map(|message| match &mut message.content {
                MessageContent::Blocks(blocks) => Some(blocks),
                MessageContent::Text(_) => None,
            })
            .map(|blocks| replace_image_blocks(blocks, note))
            .sum();
        if withheld > 0 {
            crate::log_legacy!(
                "[Routing] '{model}' does not read images; withheld {withheld} base64 bytes of images"
            );
        }
        messages
    }

    /// INVARIANT: Resolve which backend serves `model` and what id to send it, or say what
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

struct Resolved {
    model: String,
    generation: Generation,
    route: RouteEntry,
}

impl RoutingProvider {
    fn resolve(&self, selection: &ModelSelection<'_>) -> Result<Resolved, BoxError> {
        self.resolve_refusing(selection, None)
    }

    fn resolve_refusing(
        &self,
        selection: &ModelSelection<'_>,
        also_refused: Option<(ProviderKind, &str)>,
    ) -> Result<Resolved, BoxError> {
        let named = selection.model.unwrap_or(&self.default_model);
        let refused = |provider, model: &str| {
            also_refused == Some((provider, model)) || self.not_served.refused(provider, model)
        };
        let resolved = resolve_generation(&self.registry, named, selection.generation_rule, |m| {
            reaches(
                &self.registry,
                m,
                selection.provider,
                |kind| self.is_configured(kind),
                refused,
            )
        });
        let route = self.route_for(&resolved.model, selection.provider)?;
        Ok(Resolved {
            model: resolved.model,
            generation: resolved.generation,
            route,
        })
    }

    fn log_generation(named: &str, resolved: &Resolved) {
        let model = &resolved.model;
        match &resolved.generation {
            Generation::Named => {}
            Generation::Successor => {
                crate::log_legacy!("[Routing] '{named}' is retired; sending its successor '{model}'")
            }
            Generation::Predecessor => crate::log_legacy!(
                "[Routing] nothing from '{named}' on is reachable; sending the earlier '{model}'"
            ),
            Generation::Held { successor } => crate::log_legacy!(
                "[Routing] '{named}' is retired, but '{successor}' is not reachable; keeping '{named}'"
            ),
        }
    }

    async fn send(
        &self,
        resolved: &Resolved,
        (messages, tools): (Vec<Message>, Vec<ToolDefinition>),
        selection: &ModelSelection<'_>,
        system_prompt: Option<&str>,
        on_token: Option<TokenCallback>,
        call: crate::llm::metered::CallToken,
    ) -> Result<LlmResponse, BoxError> {
        let Resolved { model, route, .. } = resolved;
        crate::llm::metered::check_provider(route.provider).await?;
        let provider = self
            .backend(route.provider)
            .ok_or_else(|| unconfigured_message(route.provider))?;
        let leaf = ModelSelection::model(&route.wire_id)
            .with_effort(self.effort_for_route(model, route, selection.reasoning_effort))
            .with_attempt_timeout(selection.attempt_timeout)
            .with_forced_tool(selection.forced_tool)
            .with_service_tier(selection.service_tier)
            .with_cache_mode(selection.cache_mode);
        let messages = self.messages_for_model(model, messages);
        crate::llm::call_trace::routed_to(route.provider);
        let outcome = provider
            .chat(messages, tools, leaf, system_prompt, on_token, call)
            .await;
        outcome.map_err(|err| self.record_refusal(err, route.provider, model))
    }

    fn record_refusal(&self, err: BoxError, provider: ProviderKind, model: &str) -> BoxError {
        match err.downcast::<ModelNotServed>() {
            Ok(refusal) => {
                self.not_served.mark(provider, model);
                Box::new(refusal.refused_by(provider, model))
            }
            Err(other) => other,
        }
    }
}

/// INVARIANT: What the user has to set up for `kind` to serve a turn.
///
/// The Vertex project id is an ALREADY-RESOLVED value. It comes from
/// `VERTEX_PROJECT_ID`, then the ADC `quota_project_id` or gcloud config file,
/// then a `gcloud config` subprocess. Naming only the env var would send a user
/// who authenticated with ADC to fix the wrong thing.
fn unconfigured_message(kind: ProviderKind) -> &'static str {
    match kind {
        ProviderKind::Vertex => "Vertex AI model requested but no Google Cloud project is configured (set VERTEX_PROJECT_ID or run `gcloud auth application-default login`)",
        ProviderKind::Anthropic => concat!("Anthropic model requested but no Anthropic credential is configured (", providers_settings!(), ") and ANTHROPIC_API_KEY is not set"),
        ProviderKind::OpenAi => concat!("OpenAI model requested but no OpenAI credential is configured (", providers_settings!(), ") and OPENAI_API_KEY is not set"),
        ProviderKind::OpenRouter => concat!("OpenRouter model requested but no OpenRouter credential is configured (", providers_settings!(), ") and LUCIDOS_OPENROUTER_API_KEY is not set"),
        ProviderKind::XAi => concat!("xAI model requested but no xAI credential is configured (", providers_settings!(), ") and LUCIDOS_XAI_API_KEY is not set"),
        ProviderKind::OpenCodeFree => concat!("Free model requested but the keyless OpenCode Free tier is turned off (", providers_settings!(), ")"),
        ProviderKind::Local => concat!("Local model requested but the local OpenAI-compatible provider is not configured (", providers_settings!(), ")"),
        ProviderKind::ClaudeGateway => concat!("Claude gateway model requested but Lucidos is not signed in to a Claude gateway (", providers_settings!(), ")"),
        ProviderKind::ChatGptPlan => concat!("ChatGPT plan model requested but Lucidos is not signed in with ChatGPT (", providers_settings!(), ")"),
    }
}

#[async_trait]
impl LlmProvider for RoutingProvider {
    async fn route_available(
        &self,
        provider: crate::llm::ProviderKind,
        model: &str,
        wire_id: &str,
    ) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
        let Some(backend) = self.backend(provider) else {
            return Ok(false);
        };
        if self.not_served.refused(provider, model) {
            return Ok(false);
        }
        backend.route_available(provider, model, wire_id).await
    }

    fn resolved_provider_kind(
        &self,
        selection: &ModelSelection<'_>,
    ) -> Result<Option<crate::llm::ProviderKind>, Box<dyn std::error::Error + Send + Sync>> {
        Ok(Some(self.resolve(selection)?.route.provider))
    }

    async fn chat(
        &self,
        messages: Vec<Message>,
        tools: Vec<ToolDefinition>,
        selection: ModelSelection<'_>,
        system_prompt: Option<&str>,
        on_token: Option<TokenCallback>,
        call: crate::llm::metered::CallToken,
    ) -> Result<LlmResponse, BoxError> {
        let named = selection.model.unwrap_or(&self.default_model);
        let first = self.resolve(&selection)?;
        Self::log_generation(named, &first);
        let retry_on = self
            .resolve_refusing(&selection, Some((first.route.provider, &first.model)))
            .ok()
            .filter(|next| next.model != first.model);
        let kept = retry_on.as_ref().map(|_| (messages.clone(), tools.clone()));
        let streamed = Arc::new(AtomicBool::new(false));
        let shared: Option<SharedCallback> = on_token.map(|cb| {
            let streamed = streamed.clone();
            Arc::new(move |token: &str| {
                streamed.store(true, Ordering::SeqCst);
                cb(token)
            }) as SharedCallback
        });
        let outcome = self
            .send(
                &first,
                (messages, tools),
                &selection,
                system_prompt,
                forward(&shared),
                call,
            )
            .await;
        let (outcome, resolved) = match (outcome, kept) {
            (Err(err), Some(request))
                if err.is::<ModelNotServed>() && !streamed.load(Ordering::SeqCst) =>
            {
                // INVARIANT: The ledger now holds the refusal, so this resolves afresh.
                let next = self.resolve(&selection)?;
                if next.model == first.model {
                    return Err(err);
                }
                crate::log_legacy!(
                    "[Routing] {} does not serve '{}'; retrying once on '{}'",
                    first.route.provider.as_str(),
                    first.model,
                    next.model
                );
                let outcome = self
                    .send(
                        &next,
                        request,
                        &selection,
                        system_prompt,
                        forward(&shared),
                        call,
                    )
                    .await;
                (outcome, next)
            }
            (outcome, _) => (outcome, first),
        };
        let mut response = outcome?;
        let route = resolved.route;
        response.provider = Some(route.provider);
        if route.provider != ProviderKind::OpenRouter {
            if let Some(usage) = response.usage.as_mut() {
                usage.cache_creation_requested_ttl = None;
            }
        }
        if let Some(pair) = response.prefill.as_mut() {
            pair.routed(route.provider, &resolved.model);
        }
        if let Some(served) = response.served_model.as_deref() {
            if !served_as_sent(&route.wire_id, served) {
                crate::log_legacy!(
                    "[Routing] WARNING: sent '{}' to {}, but '{served}' served it",
                    route.wire_id,
                    route.provider.as_str()
                );
            }
        }
        Ok(response)
    }

    async fn prefill(
        &self,
        body: &crate::llm::prefill::PrefillBody,
        call: crate::llm::metered::CallToken,
    ) -> Result<LlmResponse, BoxError> {
        let kind = body
            .provider
            .ok_or_else(|| crate::llm::prefill::cannot_prefill(None))?;
        let backend = self
            .backend(kind)
            .ok_or_else(|| unconfigured_message(kind))?;
        crate::llm::call_trace::routed_to(kind);
        let mut response = backend.prefill(body, call).await?;
        response.provider = Some(kind);
        Ok(response)
    }

    fn effort_sent(&self, selection: &ModelSelection<'_>) -> Option<String> {
        match self.resolve(selection) {
            Ok(Resolved { model, route, .. }) => {
                self.effort_for_route(&model, &route, selection.reasoning_effort)
            }
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

    fn not_served_ledger(&self) -> Option<NotServedLedger> {
        Some(self.not_served.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::lineage_stub::lineage_router;
    use super::*;
    use crate::llm::model_registry::ModelRouting;
    use crate::llm::{AnthropicProvider, OpenAiProvider, VertexProvider};
    use std::collections::HashMap;
    use std::sync::RwLock;

    fn router(rows: &[(&str, ProviderKind)]) -> RoutingProvider {
        let registry: ModelRegistry = Arc::new(RwLock::new(
            rows.iter()
                .map(|(id, provider)| (id.to_string(), ModelRouting::single(*provider, *id)))
                .collect::<HashMap<_, _>>(),
        ));
        RoutingProvider::new(
            RoutingBackends::default(),
            registry,
            "claude-opus-5".to_string(),
        )
    }

    impl RoutingProvider {
        fn effort_for_model<'a>(&self, model: &str, effort: Option<&'a str>) -> Option<&'a str> {
            let route = resolve_route(&self.registry, model, None, |_| true)
                .expect("every provider counts as configured here");
            self.effort_for_route(model, &route, effort)
        }
    }

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
            ("x-ai/grok-4.6", ProviderKind::OpenRouter),
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

    #[test]
    fn a_free_model_is_hidden_and_actionable_while_the_tier_is_off() {
        let router = router(&[("laguna-s-2.1-free", ProviderKind::OpenCodeFree)]);
        assert_eq!(router.configured_providers(), Some(Vec::new()));
        let Err(err) = router.route_for("laguna-s-2.1-free", None) else {
            panic!("the tier is off, so there is no provider to route to");
        };
        let err = err.to_string();
        assert!(err.contains("OpenCode Free"), "{err}");
        assert!(err.contains(providers_settings!()), "{err}");
    }

    struct NoTokens;

    #[async_trait::async_trait]
    impl crate::llm::claude_gateway::GatewayAccess for NoTokens {
        async fn bearer(&self) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
            Err("not signed in".into())
        }

        async fn renew_rejected(
            &self,
            _rejected: &str,
        ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
            self.bearer().await
        }

        async fn managed_settings(
            &self,
        ) -> Result<
            Option<crate::llm::claude_gateway::gateway_managed_settings::GatewayManagedSettings>,
            Box<dyn std::error::Error + Send + Sync>,
        > {
            Err("not signed in".into())
        }
    }

    fn gateway_only_router() -> RoutingProvider {
        let registry: ModelRegistry = Arc::new(RwLock::new(HashMap::from([(
            "claude-opus-5-5".to_string(),
            ModelRouting {
                routes: vec![
                    RouteEntry::new(ProviderKind::Vertex, "claude-opus-5-5"),
                    RouteEntry::new(ProviderKind::Anthropic, "claude-opus-5-5"),
                    RouteEntry::new(ProviderKind::ClaudeGateway, "claude-opus-5-5"),
                ],
                preferred: None,
                vision: false,
                default_effort: None,
                successor: None,
            },
        )])));
        let gateway = AnthropicProvider::claude_gateway(
            crate::llm::claude_gateway::GatewayOrigin::parse("https://gateway.example").unwrap(),
            Arc::new(NoTokens),
            "claude-opus-5-5".to_string(),
        )
        .expect("build the gateway provider");
        RoutingProvider::new(
            RoutingBackends {
                claude_gateway: Some(Arc::new(gateway)),
                ..RoutingBackends::default()
            },
            registry,
            "claude-opus-5-5".to_string(),
        )
    }

    #[test]
    fn a_claude_model_runs_on_the_gateway_and_a_pin_elsewhere_is_refused() {
        let router = gateway_only_router();
        assert_eq!(
            router.configured_providers(),
            Some(vec![ProviderKind::ClaudeGateway])
        );
        let route = router
            .route_for("claude-opus-5-5", None)
            .expect("the gateway serves it");
        assert_eq!(route.provider, ProviderKind::ClaudeGateway);
        let Err(err) = router.route_for("claude-opus-5-5", Some(ProviderKind::Anthropic)) else {
            panic!("a pin to an unconfigured backend must refuse");
        };
        assert!(err.to_string().contains("Anthropic credential"), "{err}");
    }

    #[test]
    fn a_pin_to_a_gateway_not_signed_in_says_where_to_sign_in() {
        let router = router(&[("claude-opus-5-5", ProviderKind::ClaudeGateway)]);
        let Err(err) = router.route_for("claude-opus-5-5", Some(ProviderKind::ClaudeGateway))
        else {
            panic!("no gateway is configured");
        };
        let err = err.to_string();
        assert!(err.contains("not signed in to a Claude gateway"), "{err}");
        assert!(
            err.contains(crate::llm::claude_gateway::SIGN_IN_LOCATION),
            "{err}"
        );
    }

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
            RoutingBackends {
                anthropic: Some(Arc::new(
                    AnthropicProvider::new(
                        crate::llm::AnthropicAuth::ApiKey("k".to_string()),
                        "claude-opus-5".to_string(),
                    )
                    .expect("build the anthropic provider"),
                )),
                ..RoutingBackends::default()
            },
            registry,
            "claude-opus-5".to_string(),
        );

        let route = router
            .route_for("claude-opus-5", None)
            .expect("anthropic serves it");
        assert_eq!(route.provider, ProviderKind::Anthropic);

        let Err(err) = router.route_for("claude-opus-5", Some(ProviderKind::Vertex)) else {
            panic!("a pin to an unconfigured backend must refuse");
        };
        assert!(err.to_string().contains("Vertex AI"), "{err}");

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
            crate::llm::ProviderKind::OpenAi,
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
            RoutingBackends {
                local: Some(Arc::new(local)),
                ..RoutingBackends::default()
            },
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
        let backoff: std::time::Duration = (1..=crate::llm::MAX_RETRIES)
            .map(|attempt| crate::llm::retry_delay(attempt, 1))
            .sum();
        let bound = backoff + std::time::Duration::from_secs(10);
        let outcome = tokio::time::timeout(bound, call)
            .await
            .expect("each attempt must end at the cap, not the header bound");
        assert!(outcome.is_err(), "the backend never answers");
    }

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
            crate::llm::ProviderKind::OpenAi,
            "k".to_string(),
            "unused".to_string(),
            &format!("http://{addr}/v1"),
            Vec::new(),
            true,
        )
        .expect("build the stub provider");
        (provider, hits)
    }

    #[tokio::test]
    async fn the_response_names_the_route_that_served_it() {
        let (local, _hits) = answering_backend(
            "200 OK",
            concat!(
                r#"data: {"choices":[{"delta":{"content":"hi"},"finish_reason":"stop"}],"#,
                r#""usage":{"prompt_tokens":3,"completion_tokens":1}}"#,
                "\n\ndata: [DONE]\n\n"
            ),
        )
        .await;
        let registry: ModelRegistry = Arc::new(RwLock::new(HashMap::from([(
            "local-model".to_string(),
            ModelRouting::single(ProviderKind::Local, "local-model"),
        )])));
        let router = RoutingProvider::new(
            RoutingBackends {
                local: Some(Arc::new(local)),
                ..RoutingBackends::default()
            },
            registry,
            "local-model".to_string(),
        );
        let response = router
            .chat(
                vec![],
                vec![],
                ModelSelection::model("local-model"),
                None,
                None,
                crate::llm::metered::CallToken::for_test(),
            )
            .await
            .expect("the backend answers");
        assert_eq!(response.provider, Some(ProviderKind::Local));
        assert!(
            response.prefill.is_none(),
            "a local model cannot be prefilled"
        );
    }

    #[tokio::test]
    async fn a_requested_ttl_survives_only_on_a_route_that_honours_it() {
        const MODEL: &str = "anthropic/claude-opus-5-5";
        for (kind, kept) in [
            (ProviderKind::OpenRouter, true),
            (ProviderKind::Local, false),
            (ProviderKind::OpenAi, false),
        ] {
            let (backend, _hits) = answering_backend(
                "200 OK",
                concat!(
                    r#"data: {"choices":[{"delta":{"content":"hi"},"finish_reason":"stop"}],"#,
                    r#""usage":{"prompt_tokens":900,"completion_tokens":1,"#,
                    r#""prompt_tokens_details":{"cached_tokens":0,"cache_write_tokens":880}}}"#,
                    "\n\ndata: [DONE]\n\n"
                ),
            )
            .await;
            let mut backends = RoutingBackends::default();
            match kind {
                ProviderKind::OpenRouter => backends.openrouter = Some(Arc::new(backend)),
                ProviderKind::Local => backends.local = Some(Arc::new(backend)),
                _ => backends.openai = Some(Arc::new(backend)),
            }
            let registry: ModelRegistry = Arc::new(RwLock::new(HashMap::from([(
                MODEL.to_string(),
                ModelRouting::single(kind, MODEL),
            )])));
            let router = RoutingProvider::new(backends, registry, MODEL.to_string());
            let response = router
                .chat(
                    vec![crate::llm::Message {
                        role: "user".to_string(),
                        content: crate::llm::MessageContent::Text("hello".to_string()),
                    }],
                    vec![],
                    ModelSelection::model(MODEL),
                    Some("system"),
                    None,
                    crate::llm::metered::CallToken::for_test(),
                )
                .await
                .expect("the backend answers");
            let expected = kept.then_some(crate::llm::cache_lifetime::CacheLifetime::FiveMinutes);
            assert_eq!(
                response.usage.and_then(|u| u.cache_creation_requested_ttl),
                expected,
                "{kind:?}"
            );
        }
    }

    fn prefill_body(provider: Option<ProviderKind>) -> crate::llm::prefill::PrefillBody {
        crate::llm::prefill::PrefillBody {
            provider,
            model: "local-model".to_string(),
            reasoning_effort: None,
            stable_prefix_fingerprint: None,
            output_cap: Some(0),
            reply: crate::llm::prefill::ReplyShape::Json,
            wire_model: "local-model".to_string(),
            betas: Vec::new(),
            body: bytes::Bytes::from_static(b"{}"),
        }
    }

    #[tokio::test]
    async fn a_beat_goes_only_to_the_backend_that_served_its_turn() {
        let (local, hits) = answering_backend("200 OK", "{}").await;
        let registry: ModelRegistry = Arc::new(RwLock::new(HashMap::new()));
        let router = RoutingProvider::new(
            RoutingBackends {
                local: Some(Arc::new(local)),
                ..RoutingBackends::default()
            },
            registry,
            "local-model".to_string(),
        );
        let beat = |provider| {
            let router = &router;
            async move {
                router
                    .prefill(
                        &prefill_body(provider),
                        crate::llm::metered::CallToken::for_test(),
                    )
                    .await
                    .expect_err("no beat lands here")
                    .to_string()
            }
        };
        assert!(beat(None).await.contains("cannot prefill"));
        assert_eq!(
            beat(Some(ProviderKind::Vertex)).await,
            unconfigured_message(ProviderKind::Vertex)
        );
        assert_eq!(hits.load(std::sync::atomic::Ordering::SeqCst), 0);
        assert!(beat(Some(ProviderKind::Local))
            .await
            .contains("no usage block"));
        assert_eq!(hits.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    async fn recording_openai() -> (OpenAiProvider, tokio::sync::mpsc::UnboundedReceiver<String>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (sent, bodies) = tokio::sync::mpsc::unbounded_channel::<String>();
        tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                let mut request = vec![0u8; 256 * 1024];
                let n = stream.read(&mut request).await.unwrap_or(0);
                let _ = sent.send(String::from_utf8_lossy(&request[..n]).to_string());
                let body = r#"{"error":{"message":"refused for the test"}}"#;
                let reply = format!(
                    "HTTP/1.1 400 Bad Request\r\nContent-Type: application/json\r\n\
                     Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(reply.as_bytes()).await;
            }
        });
        let openai = OpenAiProvider::new_with_base_url(
            crate::llm::ProviderKind::OpenAi,
            "k".to_string(),
            "unused".to_string(),
            &format!("http://{addr}/v1"),
            Vec::new(),
            false,
        )
        .expect("build the stub provider");
        (openai, bodies)
    }

    fn message_with_an_image() -> Message {
        Message {
            role: "user".to_string(),
            content: crate::llm::MessageContent::Blocks(vec![
                crate::llm::ContentBlock::Text {
                    text: "what is in this picture?".to_string(),
                },
                crate::llm::ContentBlock::Image {
                    source_type: "base64".to_string(),
                    media_type: "image/png".to_string(),
                    data: "iVBORw0KGgo".to_string(),
                },
            ]),
        }
    }

    async fn body_sent(
        router: &RoutingProvider,
        bodies: &mut tokio::sync::mpsc::UnboundedReceiver<String>,
        model: &str,
    ) -> String {
        let outcome = router
            .chat(
                vec![message_with_an_image()],
                vec![],
                ModelSelection::model(model),
                None,
                None,
                crate::llm::metered::CallToken::for_test(),
            )
            .await;
        assert!(outcome.is_err(), "the backend refuses");
        bodies.recv().await.expect("the leaf sent a request")
    }

    async fn vision_router(
        rows: &[(&str, bool, Option<&str>)],
    ) -> (
        RoutingProvider,
        tokio::sync::mpsc::UnboundedReceiver<String>,
    ) {
        let (openai, bodies) = recording_openai().await;
        let registry: ModelRegistry = Arc::new(RwLock::new(
            rows.iter()
                .map(|(id, vision, successor)| {
                    let routing = ModelRouting {
                        vision: *vision,
                        successor: successor.map(str::to_string),
                        ..ModelRouting::single(ProviderKind::OpenAi, *id)
                    };
                    (id.to_string(), routing)
                })
                .collect::<HashMap<_, _>>(),
        ));
        let router = RoutingProvider::new(
            RoutingBackends {
                openai: Some(Arc::new(openai)),
                ..RoutingBackends::default()
            },
            registry,
            "gpt-4.1".to_string(),
        );
        (router, bodies)
    }

    fn carries_the_image(request: &str) -> bool {
        request.contains(r#""type":"image_url""#) && request.contains("iVBORw0KGgo")
    }

    #[tokio::test]
    async fn only_a_text_only_row_has_its_images_withheld() {
        let (router, mut bodies) = vision_router(&[
            ("gpt-4.1-looker", true, None),
            ("gpt-4.1-text", false, None),
        ])
        .await;
        for model in ["gpt-4.1-looker", "gpt-4.1"] {
            let request = body_sent(&router, &mut bodies, model).await;
            assert!(carries_the_image(&request), "{model}: {request}");
            assert!(!request.contains("Image not sent"), "{model}: {request}");
        }
        let request = body_sent(&router, &mut bodies, "gpt-4.1-text").await;
        assert!(!request.contains("image_url"), "{request}");
        assert!(!request.contains("iVBORw0KGgo"), "{request}");
        assert!(
            request.contains("[Image not sent: gpt-4.1-text does not read images."),
            "{request}"
        );
        assert!(
            request.contains("what is in this picture?"),
            "the text around the image survives: {request}"
        );
    }

    #[tokio::test]
    async fn the_gate_reads_the_successor_not_the_retired_id() {
        let (router, mut bodies) = vision_router(&[
            ("gpt-4.1-old-text", false, Some("gpt-4.1-new-looker")),
            ("gpt-4.1-new-looker", true, None),
            ("gpt-4.1-old-looker", true, Some("gpt-4.1-new-text")),
            ("gpt-4.1-new-text", false, None),
        ])
        .await;
        let request = body_sent(&router, &mut bodies, "gpt-4.1-old-text").await;
        assert!(carries_the_image(&request), "{request}");
        let request = body_sent(&router, &mut bodies, "gpt-4.1-old-looker").await;
        assert!(!carries_the_image(&request), "{request}");
        assert!(
            request.contains("gpt-4.1-new-text does not read images"),
            "{request}"
        );
    }

    #[tokio::test]
    async fn the_service_tier_and_cache_mode_cross_the_router_to_the_leaf() {
        let (openai, mut bodies) = recording_openai().await;
        let registry: ModelRegistry = Arc::new(RwLock::new(HashMap::from([(
            "gpt-6.1-sol".to_string(),
            ModelRouting::single(ProviderKind::OpenAi, "gpt-6.1-sol"),
        )])));
        let router = RoutingProvider::new(
            RoutingBackends {
                openai: Some(Arc::new(openai)),
                ..RoutingBackends::default()
            },
            registry,
            "gpt-6.1-sol".to_string(),
        );
        let outcome = router
            .chat(
                vec![Message {
                    role: "user".to_string(),
                    content: crate::llm::MessageContent::Text("hi".to_string()),
                }],
                vec![],
                ModelSelection::model("gpt-6.1-sol")
                    .with_service_tier(Some(crate::llm::ServiceTier::Flex))
                    .with_cache_mode(crate::llm::CacheMode::Explicit),
                None,
                None,
                crate::llm::metered::CallToken::for_test(),
            )
            .await;
        assert!(outcome.is_err(), "the backend refuses");
        let request = bodies.recv().await.expect("the leaf sent a request");
        assert!(request.contains(r#""service_tier":"flex""#), "{request}");
        assert!(
            request.contains(r#""prompt_cache_options":{"mode":"explicit"}"#),
            "{request}"
        );
    }

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
            RoutingBackends {
                openrouter: Some(Arc::new(openrouter)),
                local: Some(Arc::new(local)),
                ..RoutingBackends::default()
            },
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

    async fn chat_on(
        router: &RoutingProvider,
        selection: ModelSelection<'_>,
    ) -> Result<LlmResponse, BoxError> {
        let message = Message {
            role: "user".to_string(),
            content: MessageContent::Text("hi".to_string()),
        };
        router
            .chat(
                vec![message],
                vec![],
                selection,
                None,
                None,
                crate::llm::metered::CallToken::for_test(),
            )
            .await
    }

    fn requests(seen: &std::sync::Mutex<Vec<String>>) -> Vec<String> {
        seen.lock().unwrap().clone()
    }

    #[tokio::test]
    async fn a_refused_successor_falls_back_within_the_call_and_is_then_skipped() {
        let (router, seen) = lineage_router(&["new"]).await;
        chat_on(&router, ModelSelection::model("old"))
            .await
            .expect("the retired model serves the retry");
        assert_eq!(requests(&seen), ["new", "old"]);
        chat_on(&router, ModelSelection::model("old"))
            .await
            .expect("the retired model serves");
        assert_eq!(requests(&seen), ["new", "old", "old"]);
        assert!(router
            .not_served_ledger()
            .expect("the router keeps a ledger")
            .refused(ProviderKind::Local, "new"));
    }

    #[tokio::test]
    async fn a_reachable_successor_still_wins() {
        let (router, seen) = lineage_router(&[]).await;
        chat_on(&router, ModelSelection::model("old"))
            .await
            .expect("the successor serves");
        assert_eq!(requests(&seen), ["new"]);
    }

    #[tokio::test]
    async fn only_a_stored_pick_steps_back() {
        let (router, seen) = lineage_router(&["new"]).await;
        let default = ModelSelection::model("new")
            .with_generation_rule(crate::llm::model_registry::GenerationRule::NamedOrNewer);
        let err = chat_on(&router, default)
            .await
            .expect_err("a default does not step back");
        let refusal = err
            .downcast_ref::<ModelNotServed>()
            .expect("the refusal stays typed");
        assert_eq!(refusal.refusal(), Some((ProviderKind::Local, "new")));
        assert_eq!(requests(&seen), ["new"]);
        chat_on(&router, ModelSelection::model("new"))
            .await
            .expect("the stored pick steps back to the retired model");
        assert_eq!(requests(&seen), ["new", "old"]);
    }

    #[tokio::test]
    async fn a_call_retries_at_most_once() {
        let (router, seen) = lineage_router(&["new", "old"]).await;
        let err = chat_on(&router, ModelSelection::model("old"))
            .await
            .expect_err("nothing in the lineage serves");
        let refusal = err
            .downcast_ref::<ModelNotServed>()
            .expect("the refusal stays typed");
        assert_eq!(refusal.refusal(), Some((ProviderKind::Local, "old")));
        assert_eq!(requests(&seen), ["new", "old"]);
    }

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
            RoutingBackends {
                vertex: Some(Arc::new(
                    VertexProvider::new(
                        "my-project".into(),
                        "eu".into(),
                        "gemini-3.8-flash".into(),
                    )
                    .expect("build the vertex provider"),
                )),
                ..RoutingBackends::default()
            },
            registry,
            "claude-opus-5".to_string(),
        );
        let selection = ModelSelection::model("gemini-3.5-flash").with_effort(Some("none"));
        let resolved = router.resolve(&selection).expect("vertex serves it");
        assert_eq!(resolved.model, "gemini-3.8-flash");
        assert_eq!(resolved.route.wire_id, "gemini-3.8-flash");
        assert_eq!(router.effort_sent(&selection).as_deref(), Some("low"));
    }

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
            RoutingBackends {
                openrouter: Some(Arc::new(
                    OpenAiProvider::new_with_base_url(
                        crate::llm::ProviderKind::OpenAi,
                        "k".to_string(),
                        "x".to_string(),
                        crate::llm::OPENROUTER_BASE_URL,
                        Vec::new(),
                        true,
                    )
                    .expect("build the openrouter provider"),
                )),
                ..RoutingBackends::default()
            },
            registry,
            "claude-opus-5-5".to_string(),
        );
        let route = router
            .route_for("claude-opus-5-5", None)
            .expect("openrouter serves it");
        assert_eq!(route.wire_id, "anthropic/claude-opus-5-5");
        assert_eq!(
            router.effort_for_route("claude-opus-5-5", &route, Some("xhigh")),
            Some("high")
        );
    }

    #[test]
    fn an_unregistered_model_clamps_against_its_heuristic_provider() {
        let router = router(&[]);
        assert_eq!(
            router.effort_for_model("gpt-5.4", Some("max")),
            Some("xhigh")
        );
        assert_eq!(
            router.effort_for_model("claude-opus-5", Some("max")),
            Some("max")
        );
    }

    #[test]
    fn the_effort_sent_is_the_routed_one() {
        let local = OpenAiProvider::new_with_base_url(
            crate::llm::ProviderKind::OpenAi,
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
            RoutingBackends {
                local: Some(Arc::new(local)),
                ..RoutingBackends::default()
            },
            registry,
            "local-model".to_string(),
        );
        let sent = |effort| router.effort_sent(&ModelSelection::default().with_effort(effort));
        assert_eq!(sent(None).as_deref(), Some("medium"));
        assert_eq!(sent(Some("max")).as_deref(), Some("high"));
    }

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

#[cfg(any(test, feature = "test-support"))]
pub mod lineage_stub {
    use super::*;
    use crate::llm::model_registry::ModelRouting;
    use crate::llm::OpenAiProvider;
    use std::collections::HashMap;
    use std::sync::RwLock;

    pub(crate) async fn lineage_backend(
        refused: &'static [&'static str],
    ) -> (OpenAiProvider, Arc<std::sync::Mutex<Vec<String>>>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
        let log = seen.clone();
        tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                let mut request = vec![0u8; 64 * 1024];
                let n = stream.read(&mut request).await.unwrap_or(0);
                let request = String::from_utf8_lossy(&request[..n]).to_string();
                let model = request
                    .split(r#""model":""#)
                    .nth(1)
                    .and_then(|rest| rest.split('"').next())
                    .unwrap_or_default()
                    .to_string();
                let (status, body) = match refused.contains(&model.as_str()) {
                    true => (
                        "404 Not Found",
                        format!(r#"{{"error":{{"message":"model \"{model}\" not found"}}}}"#),
                    ),
                    false => (
                        "200 OK",
                        concat!(
                            r#"data: {"choices":[{"delta":{"content":"hi"},"finish_reason":"stop"}],"#,
                            r#""usage":{"prompt_tokens":3,"completion_tokens":1}}"#,
                            "\n\ndata: [DONE]\n\n"
                        )
                        .to_string(),
                    ),
                };
                log.lock().unwrap().push(model);
                let reply = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\n\
                     Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(reply.as_bytes()).await;
            }
        });
        let provider = OpenAiProvider::new_with_base_url(
            crate::llm::ProviderKind::OpenAi,
            String::new(),
            "unused".to_string(),
            &format!("http://{addr}/v1"),
            Vec::new(),
            true,
        )
        .expect("build the stub provider");
        (provider, seen)
    }

    pub async fn lineage_router(
        refused: &'static [&'static str],
    ) -> (RoutingProvider, Arc<std::sync::Mutex<Vec<String>>>) {
        let (local, seen) = lineage_backend(refused).await;
        let router = RoutingProvider::new(
            RoutingBackends {
                local: Some(Arc::new(local)),
                ..RoutingBackends::default()
            },
            lineage_registry(),
            "old".to_string(),
        );
        (router, seen)
    }

    pub fn lineage_registry() -> ModelRegistry {
        Arc::new(RwLock::new(HashMap::from([
            (
                "old".to_string(),
                ModelRouting {
                    successor: Some("new".to_string()),
                    ..ModelRouting::single(ProviderKind::Local, "old")
                },
            ),
            (
                "new".to_string(),
                ModelRouting::single(ProviderKind::Local, "new"),
            ),
        ])))
    }
}

#[cfg(test)]
#[path = "routing_plan_tests.rs"]
mod plan_tests;
