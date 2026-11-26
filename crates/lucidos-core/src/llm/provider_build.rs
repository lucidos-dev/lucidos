use crate::core::prefs::{self, parse_flag, Flag, Pref};
use crate::core::{AuthType, CredentialStore};
use crate::llm::chatgpt_plan::{ChatGptPlanAccess, ChatGptPlanAccountStore};
use crate::llm::claude_gateway::account::ClaudeGatewayAccountStore;
use crate::llm::claude_gateway::{GatewayAccessFactory, GatewayOrigin};
use crate::llm::web_search::{
    AnthropicServerToolSearch, ChatGptPlanSearch, OpenAiResponsesSearch, VertexGroundingSearch,
    WebSearchChain, WebSearchProvider,
};
use crate::llm::{
    resolve_anthropic_auth, resolve_bearer_key, resolve_openai_api_key, select_provider,
    AnthropicAuth, AnthropicAuthSource, AnthropicProvider, LlmProvider, OpenAiKeySource,
    OpenAiProvider, ProviderSelection, ProviderSelectionInputs, RoutingBackends, RoutingProvider,
    UnconfiguredProvider, VertexProvider, OPENAI_DEFAULT_BASE_URL, OPENCODE_FREE_BASE_URL,
    OPENROUTER_BASE_URL, XAI_BASE_URL,
};
use sqlx::PgPool;
use std::sync::Arc;

pub const PROVIDER_CREDENTIAL_SERVICES: [&str; 5] =
    ["openai", "anthropic", "openrouter", "xai", "local"];

pub const PROVIDER_ENABLED_SWITCHES: [&Pref<Flag>; 8] = [
    &prefs::PROVIDER_ENABLED_VERTEX,
    &prefs::PROVIDER_ENABLED_ANTHROPIC,
    &prefs::PROVIDER_ENABLED_OPENAI,
    &prefs::PROVIDER_ENABLED_OPENROUTER,
    &prefs::PROVIDER_ENABLED_XAI,
    &prefs::PROVIDER_ENABLED_LOCAL,
    &prefs::PROVIDER_ENABLED_CLAUDE_GATEWAY,
    &prefs::PROVIDER_ENABLED_CHATGPT_PLAN,
];

/// INVARIANT: Preference keys that, when changed, change which LLM provider is installed.
/// The same subscriber that watches [`PROVIDER_CREDENTIAL_SERVICES`] filters on
/// this set, so a provider configured by a preference hot-swaps exactly like one
/// configured by a credential. `opencode-free` has no credential at all, and the
/// local base URL used to need a restart. The Claude gateway URL decides which
/// sign-in counts. The per-provider switches are here for the same reason: a
/// switch that needed a restart is a switch the user reads as broken.
pub const PROVIDER_PREFERENCE_KEYS: [&str; 11] = [
    prefs::OPENCODE_FREE_ENABLED.key(),
    prefs::LOCAL_BASE_URL.key(),
    prefs::CLAUDE_GATEWAY_URL.key(),
    PROVIDER_ENABLED_SWITCHES[0].key(),
    PROVIDER_ENABLED_SWITCHES[1].key(),
    PROVIDER_ENABLED_SWITCHES[2].key(),
    PROVIDER_ENABLED_SWITCHES[3].key(),
    PROVIDER_ENABLED_SWITCHES[4].key(),
    PROVIDER_ENABLED_SWITCHES[5].key(),
    PROVIDER_ENABLED_SWITCHES[6].key(),
    PROVIDER_ENABLED_SWITCHES[7].key(),
];

/// INVARIANT: The per-provider enable switches, resolved from the `provider_enabled_*`
/// preferences (Settings → Providers).
///
/// A switch is a VETO over a provider that is otherwise configured. It never
/// installs one, and it never touches the credential: turning a provider off is
/// how a user parks a key they still want stored.
///
/// Every field defaults to its catalog default (on), and only an explicit off
/// turns one off. A workspace that never opened the page therefore resolves
/// what it always did, and so does a boot that cannot read preferences at all.
#[derive(Debug, Clone, Copy)]
pub struct ProviderSwitches {
    pub vertex: bool,
    pub anthropic: bool,
    pub openai: bool,
    pub openrouter: bool,
    pub xai: bool,
    pub local: bool,
    pub claude_gateway: bool,
    pub chatgpt_plan: bool,
}

impl Default for ProviderSwitches {
    fn default() -> Self {
        Self {
            vertex: prefs::PROVIDER_ENABLED_VERTEX.default_flag(),
            anthropic: prefs::PROVIDER_ENABLED_ANTHROPIC.default_flag(),
            openai: prefs::PROVIDER_ENABLED_OPENAI.default_flag(),
            openrouter: prefs::PROVIDER_ENABLED_OPENROUTER.default_flag(),
            xai: prefs::PROVIDER_ENABLED_XAI.default_flag(),
            local: prefs::PROVIDER_ENABLED_LOCAL.default_flag(),
            claude_gateway: prefs::PROVIDER_ENABLED_CLAUDE_GATEWAY.default_flag(),
            chatgpt_plan: prefs::PROVIDER_ENABLED_CHATGPT_PLAN.default_flag(),
        }
    }
}

/// INVARIANT: Read the eight switches, defaulting any unreadable one to its default (on).
/// The handle read logs a failed read. Losing a provider over a transient query
/// error is worse than honouring the veto one build late.
pub async fn read_provider_switches(pool: &PgPool) -> ProviderSwitches {
    ProviderSwitches {
        vertex: prefs::PROVIDER_ENABLED_VERTEX.read(pool).await,
        anthropic: prefs::PROVIDER_ENABLED_ANTHROPIC.read(pool).await,
        openai: prefs::PROVIDER_ENABLED_OPENAI.read(pool).await,
        openrouter: prefs::PROVIDER_ENABLED_OPENROUTER.read(pool).await,
        xai: prefs::PROVIDER_ENABLED_XAI.read(pool).await,
        local: prefs::PROVIDER_ENABLED_LOCAL.read(pool).await,
        claude_gateway: prefs::PROVIDER_ENABLED_CLAUDE_GATEWAY.read(pool).await,
        chatgpt_plan: prefs::PROVIDER_ENABLED_CHATGPT_PLAN.read(pool).await,
    }
}

/// INVARIANT: Inputs the active-provider build needs beyond the DB pool. Fixed at boot
/// (default model, mock flag, Vertex project id / region handle / token cache,
/// and the shared model→provider registry) and
/// reused verbatim by the runtime credential subscriber so a hot-swap is
/// identical to a fresh boot. `vertex_location`, `vertex_token_cache`, and
/// `model_registry` are shared handles (cloning shares state), so a rebuilt
/// provider tracks live region/registry updates and reuses warm Vertex tokens.
#[derive(Clone)]
pub struct ProviderBuildContext {
    pub default_model: String,
    /// INVARIANT: `LUCIDOS_MODEL == "mock"`. When true the subscriber is never spawned;
    /// passing `false` (the subscriber always does) guarantees the build can
    /// never return [`ProviderSelection::Mock`].
    pub model_is_mock: bool,
    pub vertex_project_id: String,
    pub vertex_location: crate::llm::vertex::LocationHandle,
    pub vertex_token_cache: Option<crate::llm::vertex::TokenCache>,
    pub model_registry: crate::llm::model_registry::ModelRegistry,
    pub not_served: crate::llm::not_served::NotServedLedger,
    /// INVARIANT: Makes the Claude gateway access. `None` at boot, before the
    /// event bus exists: a signed-in gateway then counts as configured but is
    /// left out, and the engine rebuilds once it can supply one.
    pub claude_gateway_access: Option<GatewayAccessFactory>,
    pub chatgpt_plan_access: Option<ChatGptPlanAccess>,
}

pub struct ProviderBuildOutcome {
    pub llm: Arc<dyn LlmProvider>,
    pub web_search: Arc<WebSearchChain>,
    pub selection: ProviderSelection,
}

struct DirectProviders {
    openai: Option<OpenAiProvider>,
    anthropic: Option<AnthropicProvider>,
    openrouter: Option<OpenAiProvider>,
    xai: Option<OpenAiProvider>,
    opencode_free: Option<OpenAiProvider>,
    local: Option<OpenAiProvider>,
    claude_gateway: Option<AnthropicProvider>,
    claude_gateway_signed_in: bool,
    chatgpt_plan: Option<OpenAiProvider>,
    chatgpt_plan_signed_in: bool,
    /// INVARIANT: Search backends in chain order: Anthropic, OpenAI key, then plan. Vertex is
    /// prepended by the caller, which owns the Vertex config.
    search_backends: Vec<Arc<dyn WebSearchProvider>>,
}

/// INVARIANT: A credential read that FAILED, as distinct from one that found nothing.
///
/// The two must never collapse into `None` unremarked. A failed read that
/// falls through to the environment runs later turns on whatever account that
/// key names. That may not be the stored one.
///
/// It still falls through, and the log line is what makes that safe. Dropping
/// the provider instead was tried and reverted. An engine configured by
/// `ANTHROPIC_API_KEY` alone has no stored credential to be wrong about. One
/// pool error at boot then left it with no provider, and `main` panicked on a
/// key that was set. Loud and working beats silent either way.
struct CredentialUnreadable;

async fn read_credential_pair(
    pool: &PgPool,
    service: &str,
    display_name: &str,
) -> Result<Option<(AuthType, String)>, CredentialUnreadable> {
    let read = match CredentialStore::get(pool, service).await {
        Ok(Some(c)) => match c.secret(pool).await {
            Ok(secret) => return Ok(Some((c.auth_type, secret))),
            Err(reason) => reason,
        },
        Ok(None) => return Ok(None),
        Err(e) => e.to_string(),
    };
    crate::log_legacy!(
        "[Startup] WARNING: could not read the {} credential ({}). \
         Falling back to the environment, which may name a different \
         account than the stored one.",
        display_name,
        read
    );
    Err(CredentialUnreadable)
}

/// INVARIANT: The stored half of the provider material: credentials and preferences.
///
/// `switches` arrives as the user's vetoes, and leaves with `local` forced off
/// if its base URL could not be read. Local is the one provider whose address
/// is stored rather than fixed. An unreadable one would send the stored key to
/// the default host instead of the configured one.
///
/// The hosted providers do NOT get that treatment. Their address is fixed, so a
/// failed read costs only the account, which the log line names. A DB-down boot
/// uses [`StoredInputs::default`], every field absent, so the env fallbacks
/// still apply and nothing stored can veto.
#[derive(Default)]
struct StoredInputs {
    anthropic: Option<(AuthType, String)>,
    openai: Option<(AuthType, String)>,
    openrouter: Option<(AuthType, String)>,
    xai: Option<(AuthType, String)>,
    opencode_free_pref: Option<String>,
    local_base: Option<String>,
    local_key: Option<StoredLocalKey>,
    claude_gateway_url: Option<String>,
    claude_gateway_issuer: Option<String>,
    chatgpt_plan_signed_in: bool,
    switches: ProviderSwitches,
}

struct StoredLocalKey {
    value: String,
    base_urls: Vec<String>,
}

async fn read_stored_inputs(pool: &PgPool, mut switches: ProviderSwitches) -> StoredInputs {
    let mut stored = StoredInputs {
        anthropic: read_credential_pair(pool, "anthropic", "Anthropic")
            .await
            .unwrap_or(None),
        openai: read_credential_pair(pool, "openai", "OpenAI")
            .await
            .unwrap_or(None),
        openrouter: read_credential_pair(pool, "openrouter", "OpenRouter")
            .await
            .unwrap_or(None),
        xai: read_credential_pair(pool, "xai", "xAI")
            .await
            .unwrap_or(None),
        ..StoredInputs::default()
    };

    // INVARIANT: OpenCode Free is a preference, not a credential. Nothing is read from the
    // credential store and nothing is sent as a bearer. An unreadable
    // preference costs the tier, never a key.
    stored.opencode_free_pref = prefs::OPENCODE_FREE_ENABLED.stored(pool).await;

    // INVARIANT: Local OpenAI-compatible: the `local_base_url` preference plus an optional
    // `local` credential. Either read failing drops the provider, because the
    // default base URL would send a stored key to another host.
    match prefs::LOCAL_BASE_URL.try_stored(pool).await {
        Ok(opt) => stored.local_base = opt,
        Err(e) => {
            crate::log_legacy!(
                "[Startup] Failed to read local_base_url preference, omitting local: {}",
                e
            );
            switches.local = false;
        }
    }
    match CredentialStore::get(pool, "local").await {
        Ok(Some(c)) => {
            stored.local_key =
                c.secret_or_log(pool, "[Startup]")
                    .await
                    .map(|value| StoredLocalKey {
                        value,
                        base_urls: c.base_urls,
                    })
        }
        Ok(None) => stored.local_key = None,
        Err(e) => {
            crate::log_legacy!(
                "[Startup] Failed to read local provider credential, omitting local: {}",
                e
            );
            switches.local = false;
        }
    }

    // INVARIANT: The Claude gateway: its URL and the stored sign-in's issuer, never its
    // tokens. Either read failing leaves the gateway out, with nothing sent.
    match prefs::CLAUDE_GATEWAY_URL.try_stored(pool).await {
        Ok(url) => stored.claude_gateway_url = url,
        Err(e) => crate::log_legacy!(
            "[Startup] Failed to read claude_gateway_url preference, omitting the Claude gateway: {}",
            e
        ),
    }
    match ClaudeGatewayAccountStore::get(pool).await {
        Ok(account) => stored.claude_gateway_issuer = account.map(|a| a.issuer),
        Err(e) => crate::log_legacy!(
            "[Startup] Failed to read the Claude gateway sign-in, omitting the Claude gateway: {}",
            e
        ),
    }

    match ChatGptPlanAccountStore::is_signed_in(pool).await {
        Ok(signed_in) => stored.chatgpt_plan_signed_in = signed_in,
        Err(e) => crate::log_legacy!(
            "[Startup] Failed to read the ChatGPT plan sign-in, omitting the ChatGPT plan: {}",
            e
        ),
    }

    stored.switches = switches;
    stored
}

fn if_enabled<T>(enabled: bool, build: impl FnOnce() -> Option<T>) -> Option<T> {
    if enabled {
        build()
    } else {
        None
    }
}

#[derive(Default)]
struct EnvFallbacks {
    openai: Option<String>,
    openai_codex: Option<String>,
    anthropic: Option<String>,
}

async fn resolve_direct_providers(
    pool: Option<&PgPool>,
    default_model: &str,
    registry: &crate::llm::model_registry::ModelRegistry,
    env: EnvFallbacks,
    switches: ProviderSwitches,
    claude_gateway_access: Option<&GatewayAccessFactory>,
    chatgpt_plan_access: Option<&ChatGptPlanAccess>,
) -> DirectProviders {
    let EnvFallbacks {
        openai: openai_env_key,
        openai_codex: openai_codex_key,
        anthropic: anthropic_env_key,
    } = env;
    let StoredInputs {
        anthropic: anthropic_credential,
        openai: openai_credential,
        openrouter: openrouter_credential,
        xai: xai_credential,
        opencode_free_pref,
        local_base,
        local_key,
        claude_gateway_url,
        claude_gateway_issuer,
        chatgpt_plan_signed_in,
        switches,
    } = match pool {
        Some(pool) => read_stored_inputs(pool, switches).await,
        None => StoredInputs {
            switches,
            ..StoredInputs::default()
        },
    };

    let anthropic_auth = resolve_anthropic_auth(anthropic_credential, anthropic_env_key)
        .filter(|_| switches.anthropic);
    let anthropic = build_anthropic_provider(anthropic_auth.clone(), default_model);

    let openai_key = resolve_openai_api_key(openai_credential, openai_env_key, openai_codex_key)
        .filter(|_| switches.openai);
    let openai = build_openai_provider(openai_key.clone(), default_model);

    let openrouter = if_enabled(switches.openrouter, || {
        build_openrouter_provider(
            openrouter_credential,
            std::env::var(crate::llm::OPENROUTER_API_KEY_ENV).ok(),
            default_model,
        )
    });
    let xai = if_enabled(switches.xai, || {
        build_xai_provider(
            xai_credential,
            std::env::var("LUCIDOS_XAI_API_KEY").ok(),
            default_model,
        )
    });

    let opencode_free = build_opencode_free_provider(opencode_free_pref, default_model);

    let local = if_enabled(switches.local, || {
        build_local_provider(local_base, local_key, default_model)
    });

    let claude_gateway_origin = if_enabled(switches.claude_gateway, || {
        signed_in_gateway(claude_gateway_url, claude_gateway_issuer)
    });
    let claude_gateway_signed_in = claude_gateway_origin.is_some();
    let claude_gateway = claude_gateway_origin.and_then(|origin| {
        build_claude_gateway_provider(origin, claude_gateway_access, default_model)
    });

    let chatgpt_plan_signed_in = chatgpt_plan_signed_in && switches.chatgpt_plan;
    let chatgpt_plan = if_enabled(chatgpt_plan_signed_in, || {
        build_chatgpt_plan_provider(chatgpt_plan_access, default_model)
    });

    // INVARIANT: Chain order: Anthropic before OpenAI, because Anthropic's server tool has
    // no per-call fee while OpenAI's Responses web search bills per call on top
    // of the tokens the results consume.
    let mut search_backends: Vec<Arc<dyn WebSearchProvider>> =
        anthropic_search_backend(anthropic_auth, registry, default_model)
            .into_iter()
            .collect();
    search_backends.extend(openai_search_backend(openai_key, registry, default_model));
    if chatgpt_plan_signed_in {
        let model = responses_search_model(
            registry,
            crate::llm::ProviderKind::ChatGptPlan,
            default_model,
        );
        if let Some(provider) = build_chatgpt_plan_provider(chatgpt_plan_access, &model) {
            match ChatGptPlanSearch::new(provider, model) {
                Ok(backend) => search_backends.push(Arc::new(backend)),
                Err(error) => {
                    crate::log_legacy!("[Startup] Failed to build ChatGPT plan search: {}", error)
                }
            }
        }
    }

    DirectProviders {
        openai,
        anthropic,
        openrouter,
        xai,
        opencode_free,
        local,
        claude_gateway,
        claude_gateway_signed_in,
        chatgpt_plan,
        chatgpt_plan_signed_in,
        search_backends,
    }
}

/// INVARIANT: Last-resort search model per provider, used when the configured chat model
/// belongs to a *different* provider. Small, current, broadly-available ids —
/// the search call is a short summary over the results, so the cheapest capable
/// model is the right tier. `gpt-6.2-sol` additionally satisfies OpenAI's
/// `uses_responses_api` check, which the `web_search` tool requires.
const ANTHROPIC_FALLBACK_SEARCH_MODEL: &str = "claude-haiku-5-5";
const OPENAI_FALLBACK_SEARCH_MODEL: &str = "gpt-6.2-sol";

/// INVARIANT: A model id valid for `provider`, for the one-shot search call.
///
/// Prefers the configured chat model, which is known to work for this user and
/// keeps search on the tier they picked. **But only when that model has a route
/// to `provider`.** The entire point of the chain is that search can run
/// on a provider the user is *not* chatting with, and in that case the chat
/// model id is meaningless to the search provider: handing OpenRouter's
/// `z-ai/glm-5.2` to Anthropic's Messages API is a hard rejection, which would
/// make the advertised fallback fail every time it was actually needed.
///
/// It asks whether a route EXISTS, rather than which one would serve. A Claude
/// model listing Vertex first still has an Anthropic route, and that route's
/// own id is what Anthropic's search backend should be sent.
///
/// The reused chat model has its `[1m]` suffix stripped. That suffix is a
/// Lucidos convention selecting the 1M-context beta, not part of any real model
/// id: the chat path strips it in `build_claude_request` and sends the beta as a
/// header instead. The one-shot search call has no such opt-in. Leaving the
/// suffix on sends `claude-fable-5[1m]` (a seeded, direct-Anthropic builtin)
/// verbatim to `/v1/messages`, which rejects it and drops the backend out of the
/// chain. The route is resolved on the FULL id, because that is how the registry
/// rows are keyed.
fn search_model_for(
    registry: &crate::llm::model_registry::ModelRegistry,
    provider: crate::llm::ProviderKind,
    chat_model: &str,
    fallback: &str,
) -> String {
    match crate::llm::model_registry::route_on(registry, chat_model, provider) {
        Some(route) => crate::llm::anthropic_wire::parse_context_suffix(&route.wire_id)
            .0
            .to_string(),
        None => fallback.to_string(),
    }
}

fn anthropic_search_backend(
    resolved_auth: Option<(AnthropicAuth, AnthropicAuthSource)>,
    registry: &crate::llm::model_registry::ModelRegistry,
    default_model: &str,
) -> Option<Arc<dyn WebSearchProvider>> {
    let (auth, _source) = resolved_auth?;
    let model = search_model_for(
        registry,
        crate::llm::ProviderKind::Anthropic,
        default_model,
        ANTHROPIC_FALLBACK_SEARCH_MODEL,
    );
    match AnthropicServerToolSearch::new(auth, model) {
        Ok(b) => Some(Arc::new(b) as Arc<dyn WebSearchProvider>),
        Err(e) => {
            crate::log_legacy!("[Startup] Failed to build Anthropic search backend: {}", e);
            None
        }
    }
}

fn openai_search_model(
    registry: &crate::llm::model_registry::ModelRegistry,
    chat_model: &str,
) -> String {
    responses_search_model(registry, crate::llm::ProviderKind::OpenAi, chat_model)
}

fn responses_search_model(
    registry: &crate::llm::model_registry::ModelRegistry,
    provider: crate::llm::ProviderKind,
    chat_model: &str,
) -> String {
    let model = search_model_for(registry, provider, chat_model, OPENAI_FALLBACK_SEARCH_MODEL);
    if crate::llm::openai::uses_responses_api(&model) {
        model
    } else {
        OPENAI_FALLBACK_SEARCH_MODEL.to_string()
    }
}

fn openai_search_backend(
    resolved_key: Option<(String, OpenAiKeySource)>,
    registry: &crate::llm::model_registry::ModelRegistry,
    default_model: &str,
) -> Option<Arc<dyn WebSearchProvider>> {
    let (key, _source) = resolved_key?;
    let model = openai_search_model(registry, default_model);
    match OpenAiResponsesSearch::new(key, model, OPENAI_DEFAULT_BASE_URL) {
        Ok(b) => Some(Arc::new(b) as Arc<dyn WebSearchProvider>),
        Err(e) => {
            crate::log_legacy!("[Startup] Failed to build OpenAI search backend: {}", e);
            None
        }
    }
}

/// INVARIANT: Build the active LLM provider from the current credentials/prefs + env,
/// mirroring the startup decision exactly (same [`select_provider`] branches).
/// Used by `main.rs` at boot and by the runtime credential subscriber to
/// hot-swap. `pool == None` is the degraded (DB-down) boot path — see
/// [`resolve_direct_providers`].
///
/// With no provider configured, installs the unconfigured sentinel.
///
/// **Mock isolation:** returns [`ProviderSelection::Mock`] iff
/// `ctx.model_is_mock` is true. The subscriber always passes `false`, so a
/// runtime swap can never reach `MockProvider`.
pub async fn build_active_provider(
    pool: Option<&PgPool>,
    ctx: &ProviderBuildContext,
) -> Result<ProviderBuildOutcome, Box<dyn std::error::Error + Send + Sync>> {
    if ctx.model_is_mock {
        return Ok(ProviderBuildOutcome {
            llm: Arc::new(crate::llm::mock::MockProvider::new(
                ctx.default_model.clone(),
            )),
            web_search: Arc::new(WebSearchChain::empty()),
            selection: ProviderSelection::Mock,
        });
    }

    let switches = match pool {
        Some(p) => read_provider_switches(p).await,
        None => ProviderSwitches::default(),
    };

    let vertex_on = !ctx.vertex_project_id.is_empty() && switches.vertex;
    let vertex = if vertex_on {
        let cache = ctx
            .vertex_token_cache
            .clone()
            .unwrap_or_else(|| Arc::new(std::sync::Mutex::new(None)));
        Some(VertexProvider::with_location_handle(
            ctx.vertex_project_id.clone(),
            ctx.vertex_location.clone(),
            ctx.default_model.clone(),
            cache,
        )?)
    } else {
        None
    };

    let env = EnvFallbacks {
        openai: std::env::var(crate::llm::OPENAI_API_KEY_ENV).ok(),
        // INVARIANT: Lowest-precedence OpenAI key: auto-detected from the Codex CLI's auth
        // file, the parallel of Vertex reading the gcloud ADC file. Read fresh
        // each build (boot + credential-subscriber hot-swap); never persisted.
        openai_codex: crate::llm::openai::codex_detect::load(),
        anthropic: std::env::var("ANTHROPIC_API_KEY").ok(),
    };
    let DirectProviders {
        openai,
        anthropic,
        openrouter,
        xai,
        opencode_free,
        local,
        claude_gateway,
        claude_gateway_signed_in,
        chatgpt_plan,
        chatgpt_plan_signed_in,
        search_backends,
    } = resolve_direct_providers(
        pool,
        &ctx.default_model,
        &ctx.model_registry,
        env,
        switches,
        ctx.claude_gateway_access.as_ref(),
        ctx.chatgpt_plan_access.as_ref(),
    )
    .await;

    let selection = select_provider(ProviderSelectionInputs {
        model_is_mock: false,
        has_vertex: vertex.is_some(),
        has_openai: openai.is_some(),
        has_anthropic: anthropic.is_some(),
        has_openrouter: openrouter.is_some(),
        has_xai: xai.is_some(),
        has_opencode_free: opencode_free.is_some(),
        has_local: local.is_some(),
        has_claude_gateway: claude_gateway_signed_in,
        has_chatgpt_plan: chatgpt_plan_signed_in,
    });

    // INVARIANT: Vertex leads the chain so an existing Vertex workspace keeps the exact
    // search behavior it had. Built from the engine's Vertex config rather than
    // from the `vertex` provider above, which is about to be moved into the
    // router — and which carries the chat model, not the grounding one.
    let mut backends: Vec<Arc<dyn WebSearchProvider>> = Vec::new();
    // INVARIANT: `vertex_on`, not the project id alone: a switched-off Vertex must not keep
    // grounding web searches on the credentials it was switched off with.
    if vertex_on {
        let cache = ctx
            .vertex_token_cache
            .clone()
            .unwrap_or_else(|| Arc::new(std::sync::Mutex::new(None)));
        match VertexGroundingSearch::new(
            ctx.vertex_project_id.clone(),
            ctx.vertex_location.clone(),
            cache,
        ) {
            Ok(b) => backends.push(Arc::new(b)),
            Err(e) => crate::log_legacy!("[Startup] Failed to build Vertex search backend: {}", e),
        }
    }
    backends.extend(search_backends);
    let web_search = Arc::new(WebSearchChain::new(backends));
    if web_search.backend_ids().is_empty() {
        crate::log_legacy!(
            "[Startup] No web search backend configured — web_search will report how to enable it"
        );
    } else {
        crate::log_legacy!(
            "[Startup] Web search backends: {}",
            web_search
                .backend_models()
                .iter()
                .map(|(id, model)| match model {
                    Some(m) => format!("{id} ({m})"),
                    None => id.to_string(),
                })
                .collect::<Vec<_>>()
                .join(" → ")
        );
    }

    let llm: Arc<dyn LlmProvider> = match selection {
        ProviderSelection::Mock => {
            return Ok(ProviderBuildOutcome {
                llm: Arc::new(crate::llm::mock::MockProvider::new(
                    ctx.default_model.clone(),
                )),
                web_search: Arc::new(WebSearchChain::empty()),
                selection: ProviderSelection::Mock,
            });
        }
        ProviderSelection::Real => Arc::new(
            RoutingProvider::new(
                RoutingBackends {
                    vertex: vertex.map(|provider| Arc::new(provider) as Arc<dyn LlmProvider>),
                    openai: openai.map(|provider| Arc::new(provider) as Arc<dyn LlmProvider>),
                    anthropic: anthropic.map(|provider| Arc::new(provider) as Arc<dyn LlmProvider>),
                    openrouter: openrouter
                        .map(|provider| Arc::new(provider) as Arc<dyn LlmProvider>),
                    xai: xai.map(|provider| Arc::new(provider) as Arc<dyn LlmProvider>),
                    opencode_free: opencode_free
                        .map(|provider| Arc::new(provider) as Arc<dyn LlmProvider>),
                    local: local.map(|provider| Arc::new(provider) as Arc<dyn LlmProvider>),
                    claude_gateway: claude_gateway
                        .map(|provider| Arc::new(provider) as Arc<dyn LlmProvider>),
                    chatgpt_plan: chatgpt_plan
                        .map(|provider| Arc::new(provider) as Arc<dyn LlmProvider>),
                },
                ctx.model_registry.clone(),
                ctx.default_model.clone(),
            )
            .observing(ctx.not_served.clone()),
        ),
        ProviderSelection::Unconfigured => Arc::new(UnconfiguredProvider::new()),
    };

    Ok(ProviderBuildOutcome {
        llm,
        web_search,
        selection,
    })
}

fn build_anthropic_provider(
    resolved_auth: Option<(AnthropicAuth, AnthropicAuthSource)>,
    default_model: &str,
) -> Option<AnthropicProvider> {
    let (auth, source) = resolved_auth?;
    match AnthropicProvider::new(auth, default_model.to_string()) {
        Ok(p) => {
            crate::log_legacy!(
                "[Startup] Direct Anthropic provider configured (auth from {})",
                source
            );
            Some(p)
        }
        Err(e) => {
            crate::log_legacy!("[Startup] Failed to build Anthropic provider: {}", e);
            None
        }
    }
}

/// INVARIANT: The configured Claude gateway's origin, when Lucidos is signed in to it.
///
/// The `claude_gateway_url` preference must parse as a gateway origin, and the
/// stored sign-in must come from that origin. A sign-in to another gateway is
/// left out rather than presented: its token belongs to its issuer only.
fn signed_in_gateway(url: Option<String>, issuer: Option<String>) -> Option<GatewayOrigin> {
    let url = url.filter(|url| !url.trim().is_empty())?;
    let origin = match GatewayOrigin::parse(&url) {
        Ok(origin) => origin,
        Err(reason) => {
            crate::log_legacy!("[Startup] Omitting the Claude gateway: {}", reason);
            return None;
        }
    };
    match issuer {
        Some(issuer) if issuer == origin.as_str() => Some(origin),
        Some(issuer) => {
            crate::log_legacy!(
                "[Startup] Omitting the Claude gateway: signed in to {}, not the configured {}",
                issuer,
                origin
            );
            None
        }
        None => {
            crate::log_legacy!(
                "[Startup] Claude gateway {} is configured but not signed in to",
                origin
            );
            None
        }
    }
}

fn build_claude_gateway_provider(
    origin: GatewayOrigin,
    access: Option<&GatewayAccessFactory>,
    default_model: &str,
) -> Option<AnthropicProvider> {
    let Some(access) = access else {
        crate::log_legacy!(
            "[Startup] Claude gateway {} joins once the engine's event bus exists",
            origin
        );
        return None;
    };
    match AnthropicProvider::claude_gateway(
        origin.clone(),
        access(origin.clone()),
        default_model.to_string(),
    ) {
        Ok(p) => {
            crate::log_legacy!("[Startup] Claude gateway provider configured ({})", origin);
            Some(p)
        }
        Err(e) => {
            crate::log_legacy!(
                "[Startup] Failed to build the Claude gateway provider: {}",
                e
            );
            None
        }
    }
}

fn build_chatgpt_plan_provider(
    access: Option<&ChatGptPlanAccess>,
    default_model: &str,
) -> Option<OpenAiProvider> {
    let Some(access) = access else {
        crate::log_legacy!("[Startup] The ChatGPT plan joins once the engine's event bus exists");
        return None;
    };
    match OpenAiProvider::new_chatgpt_plan(
        access.clone(),
        default_model.to_string(),
        OPENAI_DEFAULT_BASE_URL,
    ) {
        Ok(p) => {
            crate::log_legacy!("[Startup] ChatGPT plan provider configured");
            Some(p)
        }
        Err(e) => {
            crate::log_legacy!("[Startup] Failed to build the ChatGPT plan provider: {}", e);
            None
        }
    }
}

fn build_openai_provider(
    resolved_key: Option<(String, OpenAiKeySource)>,
    default_model: &str,
) -> Option<OpenAiProvider> {
    match resolved_key {
        Some((key, source)) => {
            crate::log_legacy!("[Startup] OpenAI provider configured (key from {})", source);
            OpenAiProvider::new(key, default_model.to_string())
                .map_err(|e| crate::log_legacy!("[Startup] Failed to build OpenAI provider: {}", e))
                .ok()
        }
        None => None,
    }
}

fn build_openrouter_provider(
    credential: Option<(AuthType, String)>,
    env_key: Option<String>,
    default_model: &str,
) -> Option<OpenAiProvider> {
    build_bearer_openai_compatible(
        crate::llm::ProviderKind::OpenRouter,
        "OpenRouter",
        OPENROUTER_BASE_URL,
        credential,
        env_key,
        default_model,
        vec![
            (
                "HTTP-Referer".to_string(),
                "https://lucidos.dev".to_string(),
            ),
            ("X-Title".to_string(), "Lucidos".to_string()),
        ],
    )
}

fn build_xai_provider(
    credential: Option<(AuthType, String)>,
    env_key: Option<String>,
    default_model: &str,
) -> Option<OpenAiProvider> {
    build_bearer_openai_compatible(
        crate::llm::ProviderKind::XAi,
        "xAI",
        XAI_BASE_URL,
        credential,
        env_key,
        default_model,
        Vec::new(),
    )
}

fn build_bearer_openai_compatible(
    provider_kind: crate::llm::ProviderKind,
    provider_label: &str,
    base_url: &str,
    credential: Option<(AuthType, String)>,
    env_key: Option<String>,
    default_model: &str,
    extra_headers: Vec<(String, String)>,
) -> Option<OpenAiProvider> {
    let key = resolve_bearer_key(credential, env_key)?;
    match OpenAiProvider::new_with_base_url(
        provider_kind,
        key,
        default_model.to_string(),
        base_url,
        extra_headers,
        true,
    ) {
        Ok(p) => {
            crate::log_legacy!("[Startup] {} provider configured", provider_label);
            Some(p)
        }
        Err(e) => {
            crate::log_legacy!(
                "[Startup] Failed to build {} provider: {}",
                provider_label,
                e
            );
            None
        }
    }
}

/// INVARIANT: The headers the keyless provider sends on every request.
///
/// Attribution follows OpenRouter's convention, which the relay honours. The
/// User-Agent names Lucidos: some free models are gated to another client's
/// User-Agent, and impersonating it is not an option we take. Split out from
/// the builder so the list itself is unit-testable.
fn opencode_free_headers() -> Vec<(String, String)> {
    vec![
        (
            "HTTP-Referer".to_string(),
            "https://lucidos.dev".to_string(),
        ),
        ("X-Title".to_string(), "Lucidos".to_string()),
        ("User-Agent".to_string(), crate::llm::lucidos_user_agent()),
    ]
}

/// INVARIANT: Build the keyless OpenCode Free provider, or `None` when the tier is off.
///
/// Opt-in through the `opencode_free_enabled` preference, with
/// `LUCIDOS_OPENCODE_FREE` as the launch env fallback. Off by default, so a
/// workspace that never asked for it is unaffected.
///
/// Three things are deliberate. The key is empty, so no `Authorization` header
/// is sent: the relay rejects a bearer it does not recognise. The attribution
/// headers follow OpenRouter's convention, which the relay honours. The
/// User-Agent names Lucidos, because some free models are gated to another
/// client's User-Agent and impersonating it is not an option we take.
fn build_opencode_free_provider(
    enabled_pref: Option<String>,
    default_model: &str,
) -> Option<OpenAiProvider> {
    let enabled = enabled_pref
        .map(|v| prefs::OPENCODE_FREE_ENABLED.resolve(Some(v.as_str())))
        .or_else(|| {
            std::env::var("LUCIDOS_OPENCODE_FREE")
                .ok()
                .map(|v| parse_flag(&v) == Some(true))
        })
        .unwrap_or_else(|| prefs::OPENCODE_FREE_ENABLED.default_flag());
    if !enabled {
        return None;
    }
    match OpenAiProvider::new_with_base_url(
        crate::llm::ProviderKind::OpenCodeFree,
        String::new(),
        default_model.to_string(),
        OPENCODE_FREE_BASE_URL,
        opencode_free_headers(),
        true,
    ) {
        Ok(p) => {
            crate::log_legacy!("[Startup] OpenCode Free provider configured (keyless)");
            Some(p)
        }
        Err(e) => {
            crate::log_legacy!("[Startup] Failed to build OpenCode Free provider: {}", e);
            None
        }
    }
}

fn build_local_provider(
    base_url_pref: Option<String>,
    stored_key: Option<StoredLocalKey>,
    default_model: &str,
) -> Option<OpenAiProvider> {
    let base_pref = base_url_pref.filter(|s| !s.trim().is_empty());
    let base_env = std::env::var("LUCIDOS_LOCAL_BASE_URL")
        .ok()
        .filter(|s| !s.trim().is_empty());
    let stored_key = stored_key.filter(|k| !k.value.trim().is_empty());
    let env_key = std::env::var("LUCIDOS_LOCAL_API_KEY")
        .ok()
        .filter(|s| !s.trim().is_empty());

    if base_pref.is_none() && base_env.is_none() && stored_key.is_none() && env_key.is_none() {
        return None;
    }
    // INVARIANT: The write path refuses such a value, but one stored before that check
    // must not reach the provider either. Dropped, never swapped for the env
    // or default host, so the user sees local stop rather than move.
    if let Some(reason) = base_pref
        .as_deref()
        .and_then(crate::core::preferences::local_base_url_rejection)
    {
        crate::log_legacy!("[Startup] Omitting the local provider: {}", reason);
        return None;
    }

    // INVARIANT: An env key is pinned to the env base URL, or the default, never to the
    // preference. Same pairing as `api::proxy_builtin::resolve_local`.
    let env_key_host = base_env
        .clone()
        .unwrap_or_else(|| prefs::LOCAL_BASE_URL.default_text().to_string());
    let base = base_pref
        .or(base_env)
        .unwrap_or_else(|| prefs::LOCAL_BASE_URL.default_text().to_string());
    let key = local_key_in_scope(&base, stored_key, env_key.map(|k| (k, env_key_host)));
    match OpenAiProvider::new_with_base_url(
        crate::llm::ProviderKind::Local,
        key.unwrap_or_default(),
        default_model.to_string(),
        &base,
        Vec::new(),
        true,
    ) {
        Ok(p) => {
            crate::log_legacy!(
                "[Startup] Local OpenAI-compatible provider configured (base {})",
                base
            );
            Some(p)
        }
        Err(e) => {
            crate::log_legacy!("[Startup] Failed to build local provider: {}", e);
            None
        }
    }
}

fn local_key_in_scope(
    base: &str,
    stored_key: Option<StoredLocalKey>,
    env_key: Option<(String, String)>,
) -> Option<String> {
    match (stored_key, env_key) {
        (Some(stored), _) => {
            if crate::core::credential_scope_covers(&stored.base_urls, base) {
                Some(stored.value)
            } else {
                crate::log_legacy!(
                    concat!(
                        "[Startup] Not sending the stored 'local' key to {}: its scope is {:?}. \
                         Re-save the key in ",
                        providers_settings!(),
                        " to scope it to this URL"
                    ),
                    base,
                    stored.base_urls
                );
                None
            }
        }
        (None, Some((key, host))) => {
            if crate::core::credential_scope_covers(std::slice::from_ref(&host), base) {
                Some(key)
            } else {
                crate::log_legacy!(
                    "[Startup] Not sending LUCIDOS_LOCAL_API_KEY to {}: it is pinned to {}",
                    base,
                    host
                );
                None
            }
        }
        (None, None) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{
        delete_credential, seed_chatgpt_plan_sign_in, seed_claude_gateway_sign_in, seed_credential,
        seed_preference, setup_test_db, teardown_test_db,
    };

    fn unconfigured_ctx() -> ProviderBuildContext {
        ProviderBuildContext {
            default_model: prefs::CHAT_MODEL.default_text().to_string(),
            model_is_mock: false,
            vertex_project_id: String::new(),
            vertex_location: crate::llm::vertex::location_handle("europe-west1".to_string()),
            vertex_token_cache: None,
            model_registry: crate::llm::model_registry::empty(),
            not_served: Default::default(),
            claude_gateway_access: None,
            chatgpt_plan_access: None,
        }
    }

    fn install(outcome: ProviderBuildOutcome) -> (Arc<dyn LlmProvider>, ProviderSelection) {
        (outcome.llm, outcome.selection)
    }

    fn installed_search(outcome: ProviderBuildOutcome) -> Arc<WebSearchChain> {
        outcome.web_search
    }

    fn stored_local_key(scope: &[&str]) -> Option<StoredLocalKey> {
        Some(StoredLocalKey {
            value: "local-secret".to_string(),
            base_urls: scope.iter().map(|s| s.to_string()).collect(),
        })
    }

    #[test]
    fn a_stored_local_key_is_sent_inside_its_scope() {
        let key = local_key_in_scope(
            "http://localhost:11434/v1",
            stored_local_key(&["http://localhost:11434/v1"]),
            None,
        );
        assert_eq!(key.as_deref(), Some("local-secret"));
    }

    #[test]
    fn a_rewritten_base_url_does_not_carry_the_stored_local_key() {
        let key = local_key_in_scope(
            "https://attacker.example/v1",
            stored_local_key(&["http://localhost:11434/v1"]),
            None,
        );
        assert_eq!(key, None);
    }

    #[test]
    fn an_unscoped_stored_local_key_is_not_sent() {
        let key = local_key_in_scope("http://localhost:11434/v1", stored_local_key(&[]), None);
        assert_eq!(key, None);
    }

    #[test]
    fn an_env_local_key_is_pinned_to_its_own_host() {
        let env = || {
            Some((
                "env-secret".to_string(),
                prefs::LOCAL_BASE_URL.default_text().to_string(),
            ))
        };
        assert_eq!(
            local_key_in_scope(prefs::LOCAL_BASE_URL.default_text(), None, env()).as_deref(),
            Some("env-secret")
        );
        assert_eq!(
            local_key_in_scope("https://attacker.example/v1", None, env()),
            None
        );
    }

    #[test]
    fn a_stored_public_local_base_url_builds_no_provider() {
        let model = prefs::CHAT_MODEL.default_text();
        assert!(
            build_local_provider(Some("https://attacker.example/v1".into()), None, model).is_none()
        );
        assert!(build_local_provider(
            Some("https://attacker.example/v1".into()),
            stored_local_key(&["https://attacker.example/v1"]),
            model
        )
        .is_none());
        assert!(
            build_local_provider(Some("http://127.0.0.1:1234/v1".into()), None, model).is_some()
        );
    }

    fn ambient_provider_env() -> bool {
        std::env::var("ANTHROPIC_API_KEY").is_ok()
            || std::env::var(crate::llm::OPENAI_API_KEY_ENV).is_ok()
            || std::env::var(crate::llm::OPENROUTER_API_KEY_ENV).is_ok()
            || std::env::var("LUCIDOS_XAI_API_KEY").is_ok()
            || std::env::var("LUCIDOS_LOCAL_BASE_URL").is_ok()
            || std::env::var("LUCIDOS_LOCAL_API_KEY").is_ok()
            || std::env::var("LUCIDOS_OPENCODE_FREE").is_ok()
            || crate::llm::openai::codex_detect::load().is_some()
    }

    #[tokio::test]
    async fn rebuild_swaps_unconfigured_to_real_and_back() {
        let (pool, db) = setup_test_db().await;
        let ctx = unconfigured_ctx();
        let ambient = ambient_provider_env();

        if !ambient {
            let (provider, selection) =
                install(build_active_provider(Some(&pool), &ctx).await.unwrap());
            assert_eq!(selection, ProviderSelection::Unconfigured);
            assert!(
                !provider.is_configured(),
                "no provider configured → llm_configured() must be false"
            );
            assert_eq!(provider.configured_providers(), Some(Vec::new()));
        }

        seed_credential(
            &pool,
            "anthropic",
            "https://api.anthropic.com",
            crate::core::AuthType::ApiKey,
            "sk-ant-test",
        )
        .await;

        let (provider, selection) =
            install(build_active_provider(Some(&pool), &ctx).await.unwrap());
        assert_eq!(
            selection,
            ProviderSelection::Real,
            "a provider credential must select the real RoutingProvider"
        );
        assert_ne!(
            selection,
            ProviderSelection::Mock,
            "must never swap to mock"
        );
        assert!(
            provider.is_configured(),
            "after adding a credential, llm_configured() must flip true"
        );
        assert!(
            provider
                .configured_providers()
                .expect("routing provider reports a set")
                .contains(&crate::llm::ProviderKind::Anthropic),
            "configured_providers must include the anthropic backend just added"
        );

        delete_credential(&pool, "anthropic").await;
        if !ambient {
            let (provider, selection) =
                install(build_active_provider(Some(&pool), &ctx).await.unwrap());
            assert_eq!(
                selection,
                ProviderSelection::Unconfigured,
                "removing the last credential must swap back to unconfigured"
            );
            assert!(!provider.is_configured());
        }

        teardown_test_db(&db).await;
    }

    fn backend_ids(providers: &DirectProviders) -> Vec<&'static str> {
        providers.search_backends.iter().map(|b| b.id()).collect()
    }

    #[tokio::test]
    async fn anthropic_env_key_alone_builds_the_provider_and_its_search_backend() {
        let (pool, db) = setup_test_db().await;
        let registry = crate::llm::model_registry::empty();
        let resolved = resolve_direct_providers(
            Some(&pool),
            prefs::CHAT_MODEL.default_text(),
            &registry,
            EnvFallbacks {
                anthropic: Some("sk-ant-env".to_string()),
                ..EnvFallbacks::default()
            },
            ProviderSwitches::default(),
            None,
            None,
        )
        .await;
        assert!(
            resolved.anthropic.is_some(),
            "ANTHROPIC_API_KEY must configure the direct Anthropic provider"
        );
        assert!(
            backend_ids(&resolved).contains(&"anthropic-server-tool"),
            "the same env-resolved auth must reach the search backend: {:?}",
            backend_ids(&resolved)
        );
        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn db_down_boot_honors_the_anthropic_env_key() {
        let registry = crate::llm::model_registry::empty();

        let without = resolve_direct_providers(
            None,
            prefs::CHAT_MODEL.default_text(),
            &registry,
            EnvFallbacks::default(),
            ProviderSwitches::default(),
            None,
            None,
        )
        .await;
        assert!(
            without.anthropic.is_none(),
            "no credential and no env key means no Anthropic, even degraded"
        );
        assert!(!backend_ids(&without).contains(&"anthropic-server-tool"));

        let with = resolve_direct_providers(
            None,
            prefs::CHAT_MODEL.default_text(),
            &registry,
            EnvFallbacks {
                anthropic: Some("sk-ant-env".to_string()),
                ..EnvFallbacks::default()
            },
            ProviderSwitches::default(),
            None,
            None,
        )
        .await;
        assert!(
            with.anthropic.is_some(),
            "the env fallback must survive a DB-down boot"
        );
        assert_eq!(
            backend_ids(&with),
            vec!["anthropic-server-tool"],
            "and must carry its search backend with it"
        );
    }

    #[tokio::test]
    async fn no_provider_boots_unconfigured_without_opt_in() {
        let (pool, db) = setup_test_db().await;
        for pref in PROVIDER_ENABLED_SWITCHES {
            seed_preference(&pool, pref.key(), "false").await.unwrap();
        }
        seed_preference(&pool, prefs::OPENCODE_FREE_ENABLED.key(), "false")
            .await
            .unwrap();
        let (provider, selection) = install(
            build_active_provider(Some(&pool), &unconfigured_ctx())
                .await
                .unwrap(),
        );
        assert_eq!(selection, ProviderSelection::Unconfigured);
        assert!(!provider.is_configured());
        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn resolved_vertex_project_builds_vertex_backend() {
        let (pool, db) = setup_test_db().await;
        let ctx = ProviderBuildContext {
            vertex_project_id: "my-gcp-project".to_string(),
            vertex_token_cache: Some(std::sync::Arc::new(std::sync::Mutex::new(None))),
            ..unconfigured_ctx()
        };
        let (provider, selection) =
            install(build_active_provider(Some(&pool), &ctx).await.unwrap());
        assert_eq!(selection, ProviderSelection::Real);
        assert!(
            provider
                .configured_providers()
                .expect("routing provider reports a set")
                .contains(&crate::llm::ProviderKind::Vertex),
            "a resolved Vertex project must build + report the vertex backend"
        );
        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn an_xai_credential_configures_and_reports_the_xai_backend() {
        let (pool, db) = setup_test_db().await;
        let ctx = unconfigured_ctx();
        seed_credential(
            &pool,
            "xai",
            crate::llm::XAI_BASE_URL,
            crate::core::AuthType::ApiKey,
            "xai-test",
        )
        .await;

        let (provider, selection) =
            install(build_active_provider(Some(&pool), &ctx).await.unwrap());
        assert_eq!(selection, ProviderSelection::Real);
        assert!(
            provider
                .configured_providers()
                .expect("routing provider reports a set")
                .contains(&crate::llm::ProviderKind::XAi),
            "an xai credential must build + report the xai backend"
        );
        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn the_free_tier_is_configured_by_its_preference_and_never_a_credential() {
        let (pool, db) = setup_test_db().await;
        let ctx = unconfigured_ctx();

        let (provider, _) = install(build_active_provider(Some(&pool), &ctx).await.unwrap());
        assert!(
            !provider
                .configured_providers()
                .expect("a provider set is reported")
                .contains(&crate::llm::ProviderKind::OpenCodeFree),
            "the free tier must be off until the user turns it on"
        );

        seed_preference(&pool, prefs::OPENCODE_FREE_ENABLED.key(), "true")
            .await
            .unwrap();
        let (provider, selection) =
            install(build_active_provider(Some(&pool), &ctx).await.unwrap());
        assert_eq!(selection, ProviderSelection::Real);
        assert!(
            provider
                .configured_providers()
                .expect("routing provider reports a set")
                .contains(&crate::llm::ProviderKind::OpenCodeFree),
            "the preference alone must build and report the keyless backend"
        );

        assert!(
            !PROVIDER_CREDENTIAL_SERVICES.contains(&"opencode-free"),
            "the keyless tier must have no credential service"
        );
        teardown_test_db(&db).await;
    }

    #[test]
    fn the_keyless_headers_name_lucidos_and_carry_no_credential() {
        let headers = opencode_free_headers();
        let ua = headers
            .iter()
            .find(|(n, _)| n == "User-Agent")
            .map(|(_, v)| v.as_str())
            .expect("the keyless provider states a User-Agent");
        assert!(ua.starts_with("Lucidos/"), "{ua}");
        assert!(
            !headers.iter().any(|(n, _)| n == "Authorization"),
            "the relay rejects a bearer, so none may be built"
        );
        for (name, value) in &headers {
            let lower = value.to_ascii_lowercase();
            assert!(
                !lower.contains("opencode") && !lower.contains("hermes"),
                "{name} names another client: {value}"
            );
        }
    }

    #[tokio::test]
    async fn search_chain_ignores_the_chat_models_provider() {
        let (pool, db) = setup_test_db().await;
        let ctx = ProviderBuildContext {
            default_model: "z-ai/glm-5.2".to_string(),
            ..unconfigured_ctx()
        };
        seed_credential(
            &pool,
            "anthropic",
            "https://api.anthropic.com",
            crate::core::AuthType::ApiKey,
            "sk-ant-test",
        )
        .await;

        let chain = installed_search(build_active_provider(Some(&pool), &ctx).await.unwrap());
        assert!(
            chain.backend_ids().contains(&"anthropic-server-tool"),
            "an Anthropic credential must supply search even when chat routes elsewhere: {:?}",
            chain.backend_ids()
        );
        let model = chain
            .backend_models()
            .into_iter()
            .find(|(id, _)| *id == "anthropic-server-tool")
            .and_then(|(_, model)| model.map(str::to_string))
            .expect("the Anthropic backend reports its model");
        assert_ne!(
            model, "z-ai/glm-5.2",
            "the OpenRouter chat model must not be sent to Anthropic"
        );
        assert!(
            model.starts_with("claude-"),
            "the Anthropic backend needs an Anthropic model, got {model:?}"
        );
        teardown_test_db(&db).await;
    }

    #[test]
    fn search_model_prefers_the_chat_model_when_it_matches_the_provider() {
        let registry = crate::llm::model_registry::empty();
        assert_eq!(
            search_model_for(
                &registry,
                crate::llm::ProviderKind::OpenAi,
                "gpt-5.6-sol",
                OPENAI_FALLBACK_SEARCH_MODEL
            ),
            "gpt-5.6-sol"
        );
    }

    #[test]
    fn search_model_strips_the_lucidos_only_1m_suffix() {
        let registry = crate::llm::model_registry::empty();
        assert_eq!(
            search_model_for(
                &registry,
                crate::llm::ProviderKind::Anthropic,
                "claude-fable-5[1m]",
                ANTHROPIC_FALLBACK_SEARCH_MODEL
            ),
            "claude-fable-5",
            "the [1m] marker must never reach a provider API"
        );
        assert_eq!(
            search_model_for(
                &registry,
                crate::llm::ProviderKind::Anthropic,
                "claude-fable-5",
                ANTHROPIC_FALLBACK_SEARCH_MODEL
            ),
            "claude-fable-5"
        );
        assert_eq!(
            search_model_for(
                &registry,
                crate::llm::ProviderKind::Anthropic,
                "claude-fable-5-1[1m]",
                ANTHROPIC_FALLBACK_SEARCH_MODEL
            ),
            "claude-fable-5-1"
        );
    }

    #[test]
    fn search_model_falls_back_when_the_chat_model_is_another_providers() {
        let registry = crate::llm::model_registry::empty();
        for (provider, chat_model, expected) in [
            (
                crate::llm::ProviderKind::Anthropic,
                "z-ai/glm-5.2",
                ANTHROPIC_FALLBACK_SEARCH_MODEL,
            ),
            (
                crate::llm::ProviderKind::OpenAi,
                "claude-opus-5",
                OPENAI_FALLBACK_SEARCH_MODEL,
            ),
        ] {
            let fallback = if provider == crate::llm::ProviderKind::Anthropic {
                ANTHROPIC_FALLBACK_SEARCH_MODEL
            } else {
                OPENAI_FALLBACK_SEARCH_MODEL
            };
            assert_eq!(
                search_model_for(&registry, provider, chat_model, fallback),
                expected,
                "{chat_model} must not be sent to {provider:?}"
            );
        }
    }

    #[test]
    fn a_chat_completions_chat_model_never_becomes_the_search_model() {
        let registry = crate::llm::model_registry::empty();
        assert_eq!(
            openai_search_model(&registry, "gpt-4o"),
            OPENAI_FALLBACK_SEARCH_MODEL,
            "gpt-4o routes to OpenAI but not to the Responses API"
        );
        assert_eq!(openai_search_model(&registry, "gpt-5.6-sol"), "gpt-5.6-sol");
        assert_eq!(
            openai_search_model(&registry, "claude-opus-5"),
            OPENAI_FALLBACK_SEARCH_MODEL
        );
    }

    #[test]
    fn a_switched_off_provider_is_never_built() {
        let mut built = false;
        let vetoed = if_enabled(false, || {
            built = true;
            Some(())
        });
        assert!(vetoed.is_none());
        assert!(!built, "a vetoed provider must not be constructed");

        let mut built = false;
        let allowed = if_enabled(true, || {
            built = true;
            Some(())
        });
        assert!(allowed.is_some());
        assert!(built, "an enabled provider still gets built");
    }

    #[tokio::test]
    async fn an_unreadable_credential_still_boots_on_the_environment_key() {
        let (pool, db) = setup_test_db().await;
        let registry = crate::llm::model_registry::empty();

        let readable = resolve_direct_providers(
            Some(&pool),
            prefs::CHAT_MODEL.default_text(),
            &registry,
            EnvFallbacks {
                anthropic: Some("sk-ant-env".to_string()),
                ..EnvFallbacks::default()
            },
            ProviderSwitches::default(),
            None,
            None,
        )
        .await;
        assert!(
            readable.anthropic.is_some(),
            "the env key alone configures the provider while the read works"
        );

        sqlx::query("DROP TABLE credentials CASCADE")
            .execute(&pool)
            .await
            .unwrap();

        let unreadable = resolve_direct_providers(
            Some(&pool),
            prefs::CHAT_MODEL.default_text(),
            &registry,
            EnvFallbacks {
                anthropic: Some("sk-ant-env".to_string()),
                ..EnvFallbacks::default()
            },
            ProviderSwitches::default(),
            None,
            None,
        )
        .await;
        assert!(
            unreadable.anthropic.is_some(),
            "an unreadable read must not cost the engine a provider the env configures"
        );
        assert!(
            backend_ids(&unreadable).contains(&"anthropic-server-tool"),
            "and its search backend comes with it: {:?}",
            backend_ids(&unreadable)
        );

        teardown_test_db(&db).await;
    }

    #[test]
    fn openai_fallback_search_model_routes_to_the_responses_api() {
        assert!(
            crate::llm::openai::uses_responses_api(OPENAI_FALLBACK_SEARCH_MODEL),
            "{OPENAI_FALLBACK_SEARCH_MODEL} must route to the Responses API"
        );
    }

    #[tokio::test]
    async fn vertex_leads_the_chain_when_configured() {
        let (pool, db) = setup_test_db().await;
        let ctx = ProviderBuildContext {
            vertex_project_id: "my-gcp-project".to_string(),
            vertex_token_cache: Some(std::sync::Arc::new(std::sync::Mutex::new(None))),
            ..unconfigured_ctx()
        };
        seed_credential(
            &pool,
            "anthropic",
            "https://api.anthropic.com",
            crate::core::AuthType::ApiKey,
            "sk-ant-test",
        )
        .await;

        let chain = installed_search(build_active_provider(Some(&pool), &ctx).await.unwrap());
        let ids = chain.backend_ids();
        assert_eq!(
            ids.first(),
            Some(&"vertex-grounding"),
            "Vertex must lead the chain: {ids:?}"
        );
        assert!(
            ids.contains(&"anthropic-server-tool"),
            "Anthropic must still be present as the fallback: {ids:?}"
        );
        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn local_only_workspace_gets_an_empty_chain() {
        if ambient_provider_env() {
            return;
        }
        let (pool, db) = setup_test_db().await;
        let ctx = unconfigured_ctx();
        seed_credential(
            &pool,
            "local",
            prefs::LOCAL_BASE_URL.default_text(),
            crate::core::AuthType::ApiKey,
            "local-key",
        )
        .await;

        let chain = installed_search(build_active_provider(Some(&pool), &ctx).await.unwrap());
        assert!(
            chain.backend_ids().is_empty(),
            "a local-only workspace has no search-capable provider: {:?}",
            chain.backend_ids()
        );
        let msg = chain
            .search("q", 5, crate::llm::metered::CallToken::for_test())
            .await
            .err()
            .expect("an empty chain errors")
            .to_string();
        assert!(msg.contains(providers_settings!()), "{msg}");
        teardown_test_db(&db).await;
    }

    const GATEWAY: &str = "https://gateway.example";

    struct FixedTokens;

    #[async_trait::async_trait]
    impl crate::llm::claude_gateway::GatewayAccess for FixedTokens {
        async fn bearer(&self) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
            Ok("gw-token".to_string())
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
            Ok(None)
        }
    }

    fn gateway_ctx() -> ProviderBuildContext {
        ProviderBuildContext {
            claude_gateway_access: Some(Arc::new(|_| Arc::new(FixedTokens))),
            ..unconfigured_ctx()
        }
    }

    async fn configured(
        pool: &PgPool,
        ctx: &ProviderBuildContext,
    ) -> Vec<crate::llm::ProviderKind> {
        let (provider, _) = install(build_active_provider(Some(pool), ctx).await.unwrap());
        provider
            .configured_providers()
            .expect("a provider set is reported")
    }

    #[tokio::test]
    async fn a_signed_in_claude_gateway_configures_and_reports_its_backend() {
        let (pool, db) = setup_test_db().await;
        seed_preference(&pool, prefs::CLAUDE_GATEWAY_URL.key(), GATEWAY)
            .await
            .unwrap();
        seed_claude_gateway_sign_in(&pool, GATEWAY).await;

        let outcome = build_active_provider(Some(&pool), &gateway_ctx())
            .await
            .unwrap();
        let ProviderBuildOutcome {
            llm,
            web_search,
            selection,
        } = outcome;
        assert_eq!(selection, ProviderSelection::Real);
        let providers = llm.configured_providers().expect("a provider set");
        assert!(providers.contains(&crate::llm::ProviderKind::ClaudeGateway));
        if !ambient_provider_env() {
            assert_eq!(providers, vec![crate::llm::ProviderKind::ClaudeGateway]);
            assert!(web_search.backend_ids().is_empty());
        }
        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn a_claude_gateway_missing_any_half_is_left_out() {
        let (pool, db) = setup_test_db().await;
        let ctx = gateway_ctx();
        let gateway = crate::llm::ProviderKind::ClaudeGateway;
        seed_preference(&pool, prefs::CLAUDE_GATEWAY_URL.key(), GATEWAY)
            .await
            .unwrap();
        assert!(
            !configured(&pool, &ctx).await.contains(&gateway),
            "not signed in"
        );

        seed_claude_gateway_sign_in(&pool, "https://other-gateway.example").await;
        assert!(
            !configured(&pool, &ctx).await.contains(&gateway),
            "signed in to another gateway"
        );

        seed_claude_gateway_sign_in(&pool, GATEWAY).await;
        assert!(configured(&pool, &ctx).await.contains(&gateway));

        seed_preference(&pool, prefs::PROVIDER_ENABLED_CLAUDE_GATEWAY.key(), "false")
            .await
            .unwrap();
        assert!(
            !configured(&pool, &ctx).await.contains(&gateway),
            "switched off"
        );

        seed_preference(&pool, prefs::PROVIDER_ENABLED_CLAUDE_GATEWAY.key(), "true")
            .await
            .unwrap();
        seed_preference(
            &pool,
            prefs::CLAUDE_GATEWAY_URL.key(),
            "http://gateway.example",
        )
        .await
        .unwrap();
        assert!(
            !configured(&pool, &ctx).await.contains(&gateway),
            "a plain http gateway URL"
        );
        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn a_signed_in_gateway_counts_at_boot_before_it_can_be_built() {
        if ambient_provider_env() {
            return;
        }
        let (pool, db) = setup_test_db().await;
        seed_preference(&pool, prefs::CLAUDE_GATEWAY_URL.key(), GATEWAY)
            .await
            .unwrap();
        seed_claude_gateway_sign_in(&pool, GATEWAY).await;

        let (provider, selection) = install(
            build_active_provider(Some(&pool), &unconfigured_ctx())
                .await
                .unwrap(),
        );
        assert_eq!(selection, ProviderSelection::Real);
        assert_eq!(provider.configured_providers(), Some(Vec::new()));
        teardown_test_db(&db).await;
    }

    struct FixedPlanTokens;

    #[async_trait::async_trait]
    impl crate::llm::chatgpt_plan::ChatGptPlanTokens for FixedPlanTokens {
        async fn access_token(&self) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
            Ok("plan-token".into())
        }

        async fn after_rejection(
            &self,
            _rejected: &str,
        ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
            Ok("plan-token".into())
        }
    }

    fn plan_ctx() -> ProviderBuildContext {
        ProviderBuildContext {
            chatgpt_plan_access: Some(ChatGptPlanAccess {
                tokens: Arc::new(FixedPlanTokens),
                pause: Arc::new(crate::llm::chatgpt_plan::LimitPause::unannounced()),
            }),
            ..unconfigured_ctx()
        }
    }

    #[tokio::test]
    async fn a_chatgpt_plan_sign_in_configures_only_its_backend() {
        let (pool, db) = setup_test_db().await;
        seed_chatgpt_plan_sign_in(&pool, "plan-token").await;
        let ProviderBuildOutcome {
            llm,
            web_search,
            selection,
        } = build_active_provider(Some(&pool), &plan_ctx())
            .await
            .unwrap();
        assert_eq!(selection, ProviderSelection::Real);
        let providers = llm.configured_providers().expect("a provider set");
        assert!(providers.contains(&crate::llm::ProviderKind::ChatGptPlan));
        if !ambient_provider_env() {
            assert_eq!(providers, vec![crate::llm::ProviderKind::ChatGptPlan]);
            assert_eq!(web_search.backend_ids(), vec!["chatgpt-plan-responses"]);
        }
        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn a_chatgpt_plan_without_a_sign_in_or_switched_off_is_left_out() {
        let (pool, db) = setup_test_db().await;
        let ctx = plan_ctx();
        let plan = crate::llm::ProviderKind::ChatGptPlan;
        assert!(
            !configured(&pool, &ctx).await.contains(&plan),
            "not signed in"
        );
        assert!(
            !installed_search(build_active_provider(Some(&pool), &ctx).await.unwrap())
                .backend_ids()
                .contains(&"chatgpt-plan-responses")
        );
        seed_chatgpt_plan_sign_in(&pool, "plan-token").await;
        assert!(configured(&pool, &ctx).await.contains(&plan));
        seed_preference(&pool, prefs::PROVIDER_ENABLED_CHATGPT_PLAN.key(), "false")
            .await
            .unwrap();
        assert!(
            !configured(&pool, &ctx).await.contains(&plan),
            "switched off"
        );
        assert!(
            !installed_search(build_active_provider(Some(&pool), &ctx).await.unwrap())
                .backend_ids()
                .contains(&"chatgpt-plan-responses")
        );
        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn a_chatgpt_plan_sign_in_counts_at_boot_before_it_can_be_built() {
        if ambient_provider_env() {
            return;
        }
        let (pool, db) = setup_test_db().await;
        seed_chatgpt_plan_sign_in(&pool, "plan-token").await;
        let (provider, selection) = install(
            build_active_provider(Some(&pool), &unconfigured_ctx())
                .await
                .unwrap(),
        );
        assert_eq!(selection, ProviderSelection::Real);
        assert_eq!(provider.configured_providers(), Some(Vec::new()));
        teardown_test_db(&db).await;
    }

    #[test]
    fn a_switch_is_on_unless_it_explicitly_says_off() {
        for switch in PROVIDER_ENABLED_SWITCHES {
            let key = switch.key();
            assert!(switch.resolve(None), "{key}: absent must mean enabled");
            for on in ["true", "TRUE", " on ", "1", "yes"] {
                assert!(switch.resolve(Some(on)), "{key}: {on} must read as enabled");
            }
            for off in ["false", "FALSE", " off ", "0", "no"] {
                assert!(
                    !switch.resolve(Some(off)),
                    "{key}: {off} must read as disabled"
                );
            }
            assert!(switch.resolve(Some("maybe")), "{key}");
            assert!(switch.resolve(Some("")), "{key}");
        }
    }

    #[test]
    fn default_switches_are_all_on() {
        let s = ProviderSwitches::default();
        assert!(s.vertex && s.anthropic && s.openai && s.openrouter && s.xai && s.local);
    }

    #[tokio::test]
    async fn switching_anthropic_off_drops_it_and_keeps_the_credential() {
        let (pool, db) = setup_test_db().await;
        let registry = crate::llm::model_registry::empty();
        seed_credential(
            &pool,
            "anthropic",
            "https://api.anthropic.com",
            crate::core::AuthType::ApiKey,
            "sk-ant-test",
        )
        .await;

        let on = resolve_direct_providers(
            Some(&pool),
            prefs::CHAT_MODEL.default_text(),
            &registry,
            EnvFallbacks::default(),
            ProviderSwitches::default(),
            None,
            None,
        )
        .await;
        assert!(
            on.anthropic.is_some(),
            "the seeded credential configures it"
        );
        assert!(backend_ids(&on).contains(&"anthropic-server-tool"));

        let off = resolve_direct_providers(
            Some(&pool),
            prefs::CHAT_MODEL.default_text(),
            &registry,
            EnvFallbacks::default(),
            ProviderSwitches {
                anthropic: false,
                ..ProviderSwitches::default()
            },
            None,
            None,
        )
        .await;
        assert!(off.anthropic.is_none(), "the switch must veto the provider");
        assert!(
            !backend_ids(&off).contains(&"anthropic-server-tool"),
            "and must take its search backend down with it: {:?}",
            backend_ids(&off)
        );

        assert!(
            crate::core::CredentialStore::get(&pool, "anthropic")
                .await
                .unwrap()
                .is_some(),
            "switching a provider off must never delete the key"
        );
        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn a_switch_never_installs_an_unconfigured_provider() {
        let (pool, db) = setup_test_db().await;
        let registry = crate::llm::model_registry::empty();
        seed_preference(&pool, prefs::PROVIDER_ENABLED_OPENROUTER.key(), "true")
            .await
            .unwrap();
        let resolved = resolve_direct_providers(
            Some(&pool),
            prefs::CHAT_MODEL.default_text(),
            &registry,
            EnvFallbacks::default(),
            read_provider_switches(&pool).await,
            None,
            None,
        )
        .await;
        assert!(
            resolved.openrouter.is_none()
                || std::env::var(crate::llm::OPENROUTER_API_KEY_ENV).is_ok(),
            "an on switch with no key must configure nothing"
        );
        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn stored_preferences_resolve_to_the_switches() {
        let (pool, db) = setup_test_db().await;
        assert!(
            matches!(
                read_provider_switches(&pool).await,
                ProviderSwitches {
                    vertex: true,
                    anthropic: true,
                    openai: true,
                    openrouter: true,
                    xai: true,
                    local: true,
                    claude_gateway: true,
                    chatgpt_plan: true,
                }
            ),
            "a workspace with no rows must resolve every switch on"
        );

        for switch in PROVIDER_ENABLED_SWITCHES {
            seed_preference(&pool, switch.key(), "false").await.unwrap();
        }
        let s = read_provider_switches(&pool).await;
        assert!(
            !s.vertex
                && !s.anthropic
                && !s.openai
                && !s.openrouter
                && !s.xai
                && !s.local
                && !s.claude_gateway
                && !s.chatgpt_plan,
            "every key must reach its own field: {s:?}"
        );
        teardown_test_db(&db).await;
    }

    #[test]
    fn every_switch_hot_swaps() {
        for switch in PROVIDER_ENABLED_SWITCHES {
            let key = switch.key();
            assert!(
                PROVIDER_PREFERENCE_KEYS.contains(&key),
                "{key} must be watched by the provider config subscriber"
            );
        }
    }

    #[test]
    fn the_frontend_mirrors_the_provider_preference_keys() {
        let ts = include_str!("../../../lucidos-app/src/store/actions/entityReferences.ts");
        for key in PROVIDER_PREFERENCE_KEYS {
            assert!(
                ts.contains(&format!("'{key}'")),
                "{key} rebuilds the provider but entityReferences.ts does not \
                 re-probe on it"
            );
        }
    }

    #[test]
    fn no_switch_is_agent_settable() {
        for switch in PROVIDER_ENABLED_SWITCHES {
            assert!(
                !matches!(switch.spec.access, prefs::PrefAccess::Agent),
                "{} must not be agent-settable",
                switch.key()
            );
        }
    }
    #[tokio::test]
    async fn plan_search_is_last_after_other_direct_backends() {
        let (pool, db) = setup_test_db().await;
        seed_chatgpt_plan_sign_in(&pool, "plan-token").await;
        let registry = crate::llm::model_registry::empty();
        let ctx = plan_ctx();
        let direct = resolve_direct_providers(
            Some(&pool),
            "gpt-6.1-sol",
            &registry,
            EnvFallbacks {
                openai: Some("platform-key".into()),
                anthropic: Some("anthropic-key".into()),
                ..Default::default()
            },
            ProviderSwitches::default(),
            None,
            ctx.chatgpt_plan_access.as_ref(),
        )
        .await;
        assert_eq!(
            backend_ids(&direct),
            [
                "anthropic-server-tool",
                "openai-responses",
                "chatgpt-plan-responses"
            ]
        );
        teardown_test_db(&db).await;
    }

    #[test]
    fn plan_search_uses_a_responses_model_from_its_own_route() {
        let registry = crate::llm::model_registry::empty();
        for model in ["gpt-4o", "claude-opus-5"] {
            assert_eq!(
                responses_search_model(&registry, crate::llm::ProviderKind::ChatGptPlan, model),
                OPENAI_FALLBACK_SEARCH_MODEL
            );
        }
    }
}
