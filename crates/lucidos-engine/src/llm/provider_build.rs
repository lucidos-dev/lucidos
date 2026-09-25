//! Build the engine's active `Arc<dyn LlmProvider>` from current credentials,
//! preferences, and env — the single construction path shared by startup
//! (`main.rs`) and the runtime config subscriber (`spawn_provider_config_subscriber`).
//!
//! The decision of *which* provider to install lives in
//! [`crate::llm::select_provider`] (unit-tested matrix); this module resolves
//! that decision's boolean inputs from the DB + env and maps the chosen
//! [`ProviderSelection`] onto a concrete provider. Factoring it out of `main.rs`
//! means a runtime hot-swap produces a provider byte-identical to a fresh boot.

use crate::core::{
    AuthType, CredentialStore, PreferenceStore, DEFAULT_LOCAL_BASE_URL, PREF_LOCAL_BASE_URL,
    PREF_OPENCODE_FREE_ENABLED, PREF_PROVIDER_ENABLED_ANTHROPIC, PREF_PROVIDER_ENABLED_LOCAL,
    PREF_PROVIDER_ENABLED_OPENAI, PREF_PROVIDER_ENABLED_OPENROUTER, PREF_PROVIDER_ENABLED_VERTEX,
    PREF_PROVIDER_ENABLED_XAI,
};
use crate::llm::web_search::{
    AnthropicServerToolSearch, OpenAiResponsesSearch, VertexGroundingSearch, WebSearchChain,
    WebSearchProvider,
};
use crate::llm::{
    resolve_anthropic_auth, resolve_bearer_key, resolve_openai_api_key, select_provider,
    AnthropicAuth, AnthropicAuthSource, AnthropicProvider, LlmProvider, OpenAiKeySource,
    OpenAiProvider, ProviderSelection, ProviderSelectionInputs, RoutingProvider,
    UnconfiguredProvider, VertexProvider, OPENAI_DEFAULT_BASE_URL, OPENCODE_FREE_BASE_URL,
    OPENROUTER_BASE_URL, XAI_BASE_URL,
};
use sqlx::PgPool;
use std::sync::Arc;

/// Credential service names that, when created/updated/deleted, change which LLM
/// provider is installed. Vertex is env/gcloud-based (no credential) and
/// hot-swaps its region via `spawn_vertex_region_subscriber` instead — so it is
/// deliberately absent here. The credential subscriber filters on this set.
pub const PROVIDER_CREDENTIAL_SERVICES: [&str; 5] =
    ["openai", "anthropic", "openrouter", "xai", "local"];

/// The six per-provider enable switches, in the order [`ProviderSwitches`]
/// declares them.
///
/// [`PROVIDER_PREFERENCE_KEYS`] is built from this list, so the config
/// subscriber cannot end up watching a subset of it. The preference catalog
/// spells the keys itself, and `no_switch_is_agent_settable` below is what
/// holds those two together.
pub(crate) const PROVIDER_ENABLED_KEYS: [&str; 6] = [
    PREF_PROVIDER_ENABLED_VERTEX,
    PREF_PROVIDER_ENABLED_ANTHROPIC,
    PREF_PROVIDER_ENABLED_OPENAI,
    PREF_PROVIDER_ENABLED_OPENROUTER,
    PREF_PROVIDER_ENABLED_XAI,
    PREF_PROVIDER_ENABLED_LOCAL,
];

/// Preference keys that, when changed, change which LLM provider is installed.
/// The same subscriber that watches [`PROVIDER_CREDENTIAL_SERVICES`] filters on
/// this set, so a provider configured by a preference hot-swaps exactly like one
/// configured by a credential. `opencode-free` has no credential at all, and the
/// local base URL used to need a restart. The six per-provider switches are here
/// for the same reason: a switch that needed a restart is a switch the user
/// reads as broken.
pub const PROVIDER_PREFERENCE_KEYS: [&str; 8] = [
    PREF_OPENCODE_FREE_ENABLED,
    PREF_LOCAL_BASE_URL,
    PROVIDER_ENABLED_KEYS[0],
    PROVIDER_ENABLED_KEYS[1],
    PROVIDER_ENABLED_KEYS[2],
    PROVIDER_ENABLED_KEYS[3],
    PROVIDER_ENABLED_KEYS[4],
    PROVIDER_ENABLED_KEYS[5],
];

/// Whether `LUCIDOS_BOOT_WITHOUT_PROVIDER` is truthy — a packaged build lets the
/// engine boot (into `UnconfiguredProvider`) before any provider is configured,
/// instead of the dev/docker fail-fast panic. Read in both `main.rs` (boot) and
/// the subscriber (so a runtime swap-back to unconfigured mirrors boot).
pub fn boot_without_provider_enabled() -> bool {
    std::env::var("LUCIDOS_BOOT_WITHOUT_PROVIDER")
        .map(|v| reads_as_true(&v))
        .unwrap_or(false)
}

/// Whether a preference or env string reads as on. One spelling of truth for
/// every boolean switch this module resolves.
fn reads_as_true(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "yes" | "on"
    )
}

/// Whether a preference string reads as an explicit off. The inverse of
/// [`reads_as_true`] over the same vocabulary, and deliberately NOT its
/// negation: an unrecognised value is neither, which is what lets the
/// per-provider switches below default to on.
fn reads_as_false(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "0" | "false" | "no" | "off"
    )
}

/// The per-provider enable switches, resolved from the `provider_enabled_*`
/// preferences (Settings → Models → Providers).
///
/// A switch is a VETO over a provider that is otherwise configured. It never
/// installs one, and it never touches the credential: turning a provider off is
/// how a user parks a key they still want stored.
///
/// Every field defaults to on, and only an explicit "false" turns one off. A
/// workspace that never opened the page therefore resolves what it always did,
/// and so does a boot that cannot read preferences at all.
#[derive(Debug, Clone, Copy)]
pub struct ProviderSwitches {
    pub vertex: bool,
    pub anthropic: bool,
    pub openai: bool,
    pub openrouter: bool,
    pub xai: bool,
    pub local: bool,
}

impl Default for ProviderSwitches {
    fn default() -> Self {
        Self {
            vertex: true,
            anthropic: true,
            openai: true,
            openrouter: true,
            xai: true,
            local: true,
        }
    }
}

/// One switch's value: on unless the stored preference explicitly says off.
///
/// Shared with `llm::judgment::select`, whose TypeSafe master switch is not one
/// of the six and still has to read the same vocabulary. Two spellings of "off"
/// would make one Settings toggle behave unlike the rest of the page.
pub(crate) fn switch_is_on(stored: Option<&str>) -> bool {
    !stored.is_some_and(reads_as_false)
}

/// Read the six switches, defaulting any unreadable one to on. A failed read is
/// logged and treated as absent. Losing a provider over a transient query error
/// is worse than honouring the veto one build late.
pub(crate) async fn read_provider_switches(pool: &PgPool) -> ProviderSwitches {
    async fn one(pool: &PgPool, key: &str) -> bool {
        match PreferenceStore::get(pool, key).await {
            Ok(v) => switch_is_on(v.as_deref()),
            Err(e) => {
                crate::log!("[Startup] Failed to read {} preference: {}", key, e);
                true
            }
        }
    }
    ProviderSwitches {
        vertex: one(pool, PREF_PROVIDER_ENABLED_VERTEX).await,
        anthropic: one(pool, PREF_PROVIDER_ENABLED_ANTHROPIC).await,
        openai: one(pool, PREF_PROVIDER_ENABLED_OPENAI).await,
        openrouter: one(pool, PREF_PROVIDER_ENABLED_OPENROUTER).await,
        xai: one(pool, PREF_PROVIDER_ENABLED_XAI).await,
        local: one(pool, PREF_PROVIDER_ENABLED_LOCAL).await,
    }
}

/// Inputs the active-provider build needs beyond the DB pool. Fixed at boot
/// (default model, mock flag, Vertex project id / region handle / token cache,
/// the shared model→provider registry, and the boot-without-provider gate) and
/// reused verbatim by the runtime credential subscriber so a hot-swap is
/// identical to a fresh boot. `vertex_location`, `vertex_token_cache`, and
/// `model_registry` are shared handles (cloning shares state), so a rebuilt
/// provider tracks live region/registry updates and reuses warm Vertex tokens.
#[derive(Clone)]
pub struct ProviderBuildContext {
    pub default_model: String,
    /// `LUCIDOS_MODEL == "mock"`. When true the subscriber is never spawned;
    /// passing `false` (the subscriber always does) guarantees the build can
    /// never return [`ProviderSelection::Mock`].
    pub model_is_mock: bool,
    pub vertex_project_id: String,
    pub vertex_location: crate::llm::vertex::LocationHandle,
    pub vertex_token_cache: Option<crate::llm::vertex::TokenCache>,
    pub model_registry: crate::llm::model_registry::ModelRegistry,
    pub boot_without_provider: bool,
}

/// What [`build_active_provider`] resolved to.
pub enum ProviderBuildOutcome {
    /// The providers to install. `selection` is carried for logging.
    ///
    /// `web_search` is built and swapped in lockstep with `llm` so adding a
    /// provider credential in Settings enables search without an engine
    /// restart — the same hot-swap guarantee the LLM provider already has.
    Install {
        llm: Arc<dyn LlmProvider>,
        web_search: Arc<WebSearchChain>,
        selection: ProviderSelection,
    },
    /// No real provider and the boot-without-provider gate is off. Boot panics
    /// with the configuration message; the runtime subscriber keeps the current
    /// provider in place (never panics on a credential delete).
    FailFast,
}

/// The direct providers resolved from credentials + env, plus the web-search
/// backends built from the same material.
///
/// Search backends are produced *here*, alongside the credentials they need,
/// rather than reconstructed later from the built providers — those keep their
/// auth private, and adding getters to hand a key back out would widen the
/// surface a secret travels across for no benefit.
struct DirectProviders {
    openai: Option<OpenAiProvider>,
    anthropic: Option<AnthropicProvider>,
    openrouter: Option<OpenAiProvider>,
    xai: Option<OpenAiProvider>,
    opencode_free: Option<OpenAiProvider>,
    local: Option<OpenAiProvider>,
    /// Search backends in chain order — Anthropic before OpenAI. Vertex is
    /// prepended by the caller, which owns the Vertex config.
    search_backends: Vec<Arc<dyn WebSearchProvider>>,
}

/// A credential read that FAILED, as distinct from one that found nothing.
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

/// Read a provider credential as an `(auth_type, auth_value)` pair.
///
/// `Ok(None)` is "nothing stored". `Err` is "could not read", which the caller
/// resolves from the environment while saying so. `display_name` names it.
async fn read_credential_pair(
    pool: &PgPool,
    service: &str,
    display_name: &str,
) -> Result<Option<(AuthType, String)>, CredentialUnreadable> {
    match CredentialStore::get(pool, service).await {
        Ok(cred) => Ok(cred.map(|c| (c.auth_type, c.auth_value))),
        Err(e) => {
            crate::log!(
                "[Startup] WARNING: could not read the {} credential ({}). \
                 Falling back to the environment, which may name a different \
                 account than the stored one.",
                display_name,
                e
            );
            Err(CredentialUnreadable)
        }
    }
}

/// The stored half of the provider material: credentials and preferences.
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
    local_key: Option<String>,
    switches: ProviderSwitches,
}

/// Read every stored credential and preference the build needs, in one place.
async fn read_stored_inputs(pool: &PgPool, mut switches: ProviderSwitches) -> StoredInputs {
    // An unreadable read leaves the field `None`, so the env fallback still
    // applies. `read_credential_pair` has already logged which provider it
    // was and that the account may differ.
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

    // OpenCode Free is a preference, not a credential. Nothing is read from the
    // credential store and nothing is sent as a bearer. An unreadable
    // preference costs the tier, never a key.
    stored.opencode_free_pref = match PreferenceStore::get(pool, PREF_OPENCODE_FREE_ENABLED).await {
        Ok(opt) => opt,
        Err(e) => {
            crate::log!(
                "[Startup] Failed to read opencode_free_enabled preference: {}",
                e
            );
            None
        }
    };

    // Local OpenAI-compatible: the `local_base_url` preference plus an optional
    // `local` credential. Either read failing drops the provider, because the
    // default base URL would send a stored key to another host.
    match PreferenceStore::get(pool, PREF_LOCAL_BASE_URL).await {
        Ok(opt) => stored.local_base = opt,
        Err(e) => {
            crate::log!(
                "[Startup] Failed to read local_base_url preference, omitting local: {}",
                e
            );
            switches.local = false;
        }
    }
    match CredentialStore::get(pool, "local").await {
        Ok(cred) => stored.local_key = cred.map(|c| c.auth_value),
        Err(e) => {
            crate::log!(
                "[Startup] Failed to read local provider credential, omitting local: {}",
                e
            );
            switches.local = false;
        }
    }

    stored.switches = switches;
    stored
}

/// Build a provider only when its switch is on.
///
/// The veto has to gate the BUILDER, not its result. `build_x(..).filter(..)`
/// logs "provider configured" for a provider the router never receives. That
/// is the line a user reads when a switch looks like it did nothing.
fn if_enabled<T>(enabled: bool, build: impl FnOnce() -> Option<T>) -> Option<T> {
    if enabled {
        build()
    } else {
        None
    }
}

/// Resolve the direct (OpenAI-wire + Anthropic) providers from credentials +
/// env. `pool == None` means the DB is unavailable (a degraded boot): the env
/// fallbacks (`OPENAI_API_KEY`, `ANTHROPIC_API_KEY`,
/// `LUCIDOS_OPENROUTER_API_KEY`, `LUCIDOS_LOCAL_*`) and the Codex-detected
/// OpenAI key all still apply, but stored credentials and the `local_base_url`
/// preference can't be read. Every field degrades to `None` / an omitted
/// backend on a BUILD error, so the engine still comes up on its other
/// providers. A stored credential that cannot be READ is a different case. It
/// falls through to the env fallback, with a log line: see
/// [`CredentialUnreadable`].
///
/// ONE build path serves both. The degraded boot used to re-spell the seven
/// builders, the search chain and the result literal. That chain's order is a
/// cost decision, so a reorder could have reached one copy only.
///
/// `switches` vetoes a provider the user has switched off, and takes its
/// credential's search backend down with it: a provider that is off must not
/// keep answering `web_search` on the key that is off with it.
async fn resolve_direct_providers(
    pool: Option<&PgPool>,
    default_model: &str,
    registry: &crate::llm::model_registry::ModelRegistry,
    openai_env_key: Option<String>,
    openai_codex_key: Option<String>,
    anthropic_env_key: Option<String>,
    switches: ProviderSwitches,
) -> DirectProviders {
    let StoredInputs {
        anthropic: anthropic_credential,
        openai: openai_credential,
        openrouter: openrouter_credential,
        xai: xai_credential,
        opencode_free_pref,
        local_base,
        local_key,
        switches,
    } = match pool {
        Some(pool) => read_stored_inputs(pool, switches).await,
        // No DB access, but the env-var + Codex fallbacks must still work. The
        // switches live in the preferences table, so on this path they are the
        // caller's all-on default and there is nothing stored to veto.
        None => StoredInputs {
            switches,
            ..StoredInputs::default()
        },
    };

    // Anthropic: a stored `anthropic` credential wins; otherwise the env
    // fallback. Resolved once and held past provider construction so the search
    // backend is built from the same auth (`AnthropicProvider` keeps its copy
    // private), which is also what stops the two disagreeing about which source
    // won. The veto is applied to the resolved AUTH rather than to the built
    // provider, so the search backend below drops with it.
    let anthropic_auth = resolve_anthropic_auth(anthropic_credential, anthropic_env_key)
        .filter(|_| switches.anthropic);
    let anthropic = build_anthropic_provider(anthropic_auth.clone(), default_model);

    // OpenAI: a stored `openai` credential wins; otherwise the env fallback.
    // Resolved once and reused: the provider needs it, and so does the OpenAI
    // search backend. Vetoed at the key, for the reason given above Anthropic's.
    let openai_key = resolve_openai_api_key(openai_credential, openai_env_key, openai_codex_key)
        .filter(|_| switches.openai);
    let openai = build_openai_provider(openai_key.clone(), default_model);

    // OpenRouter and xAI: a stored credential wins; otherwise the env fallback.
    let openrouter = if_enabled(switches.openrouter, || {
        build_openrouter_provider(
            openrouter_credential,
            std::env::var("LUCIDOS_OPENROUTER_API_KEY").ok(),
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

    // Local OpenAI-compatible: base URL from the `local_base_url` pref (env /
    // default applied inside the builder) and an optional `local` credential.
    let local = if_enabled(switches.local, || {
        build_local_provider(local_base, local_key, default_model)
    });

    // Chain order: Anthropic before OpenAI, because Anthropic's server tool has
    // no per-call fee while OpenAI's Responses web search bills per call on top
    // of the tokens the results consume.
    let mut search_backends: Vec<Arc<dyn WebSearchProvider>> =
        anthropic_search_backend(anthropic_auth, registry, default_model)
            .into_iter()
            .collect();
    search_backends.extend(openai_search_backend(openai_key, registry, default_model));

    DirectProviders {
        openai,
        anthropic,
        openrouter,
        xai,
        opencode_free,
        local,
        search_backends,
    }
}

/// Last-resort search model per provider, used when the configured chat model
/// belongs to a *different* provider. Small, current, broadly-available ids —
/// the search call is a short summary over the results, so the cheapest capable
/// model is the right tier. `gpt-5.5` additionally satisfies OpenAI's
/// `uses_responses_api` check, which the `web_search` tool requires.
const ANTHROPIC_FALLBACK_SEARCH_MODEL: &str = "claude-haiku-4-5";
const OPENAI_FALLBACK_SEARCH_MODEL: &str = "gpt-5.5";

/// A model id valid for `provider`, for the one-shot search call.
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

/// The Anthropic search backend for resolved auth, or `None` when Anthropic is
/// not configured. Shared by the DB-up and DB-down paths so both honor the
/// `ANTHROPIC_API_KEY` fallback identically.
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
            crate::log!("[Startup] Failed to build Anthropic search backend: {}", e);
            None
        }
    }
}

/// The model the OpenAI search backend runs on.
///
/// [`search_model_for`]'s value when it satisfies `uses_responses_api`, and
/// the fallback constant otherwise. `web_search` exists ONLY on the Responses
/// API. A reused Chat-Completions chat model such as `gpt-4o` would post the
/// tool to `/responses` for a model the engine says is not served there. An
/// OpenAI-only workspace has nothing behind it in the chain, which leaves the
/// backend dead with no fallback.
fn openai_search_model(
    registry: &crate::llm::model_registry::ModelRegistry,
    chat_model: &str,
) -> String {
    let model = search_model_for(
        registry,
        crate::llm::ProviderKind::OpenAi,
        chat_model,
        OPENAI_FALLBACK_SEARCH_MODEL,
    );
    if crate::llm::openai::uses_responses_api(&model) {
        model
    } else {
        OPENAI_FALLBACK_SEARCH_MODEL.to_string()
    }
}

/// The OpenAI search backend for a resolved key, or `None` when no key is
/// configured. Shared by the DB-up and DB-down paths so both honor the
/// `OPENAI_API_KEY` / Codex-CLI fallbacks identically.
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
            crate::log!("[Startup] Failed to build OpenAI search backend: {}", e);
            None
        }
    }
}

/// Build the active LLM provider from the current credentials/prefs + env,
/// mirroring the startup decision exactly (same [`select_provider`] branches).
/// Used by `main.rs` at boot and by the runtime credential subscriber to
/// hot-swap. `pool == None` is the degraded (DB-down) boot path — see
/// [`resolve_direct_providers`].
///
/// Returns [`ProviderBuildOutcome::FailFast`] (rather than panicking) when no
/// provider is configured and the gate is off, so the caller decides: boot
/// panics, the subscriber keeps the current provider.
///
/// **Mock isolation:** returns [`ProviderSelection::Mock`] iff
/// `ctx.model_is_mock` is true. The subscriber always passes `false`, so a
/// runtime swap can never reach `MockProvider`.
pub async fn build_active_provider(
    pool: Option<&PgPool>,
    ctx: &ProviderBuildContext,
) -> Result<ProviderBuildOutcome, Box<dyn std::error::Error + Send + Sync>> {
    if ctx.model_is_mock {
        return Ok(ProviderBuildOutcome::Install {
            llm: Arc::new(crate::llm::mock::MockProvider::new(
                ctx.default_model.clone(),
            )),
            // Mock is the E2E opt-in; there is no credential to search with, so
            // web_search reports the unconfigured error rather than reaching the
            // network from a test run.
            web_search: Arc::new(WebSearchChain::empty()),
            selection: ProviderSelection::Mock,
        });
    }

    // The user's per-provider switches, read once and applied to every provider
    // this build resolves. Without a pool they are the all-on default, so a
    // degraded boot keeps whatever the env configures.
    let switches = match pool {
        Some(p) => read_provider_switches(p).await,
        None => ProviderSwitches::default(),
    };

    // Vertex (env/gcloud-based, not a credential). Reuse the engine's warm
    // token cache so a rebuild doesn't discard cached access tokens; fall back
    // to a fresh cache only when none exists (project configured but no cache —
    // not reachable for a non-mock boot, defensive).
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

    let openai_env_key = std::env::var("OPENAI_API_KEY").ok();
    // Lowest-precedence OpenAI key: auto-detected from the Codex CLI's auth file
    // (apikey login), the parallel of Vertex reading the gcloud ADC file. Read
    // fresh each build (boot + credential-subscriber hot-swap); never persisted.
    let openai_codex_key = crate::llm::openai::codex_detect::load();
    // The Anthropic env fallback, below a stored `anthropic` credential. Read
    // here (not inside the resolver) so the precedence stays pure over its
    // inputs and unit-testable without touching process env.
    let anthropic_env_key = std::env::var("ANTHROPIC_API_KEY").ok();
    let DirectProviders {
        openai,
        anthropic,
        openrouter,
        xai,
        opencode_free,
        local,
        search_backends,
    } = resolve_direct_providers(
        pool,
        &ctx.default_model,
        &ctx.model_registry,
        openai_env_key,
        openai_codex_key,
        anthropic_env_key,
        switches,
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
        boot_without_provider: ctx.boot_without_provider,
    });

    // Vertex leads the chain so an existing Vertex workspace keeps the exact
    // search behavior it had. Built from the engine's Vertex config rather than
    // from the `vertex` provider above, which is about to be moved into the
    // router — and which carries the chat model, not the grounding one.
    let mut backends: Vec<Arc<dyn WebSearchProvider>> = Vec::new();
    // `vertex_on`, not the project id alone: a switched-off Vertex must not keep
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
            Err(e) => crate::log!("[Startup] Failed to build Vertex search backend: {}", e),
        }
    }
    backends.extend(search_backends);
    let web_search = Arc::new(WebSearchChain::new(backends));
    if web_search.backend_ids().is_empty() {
        crate::log!(
            "[Startup] No web search backend configured — web_search will report how to enable it"
        );
    } else {
        // Log the model per backend: the cross-provider case silently picks a
        // different model than the chat one, and that substitution should be
        // visible when a search misbehaves.
        crate::log!(
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
        // Unreachable: `model_is_mock` is false here, and `select_provider` only
        // returns Mock when that input is true. Guarded above regardless.
        ProviderSelection::Mock => {
            return Ok(ProviderBuildOutcome::Install {
                llm: Arc::new(crate::llm::mock::MockProvider::new(
                    ctx.default_model.clone(),
                )),
                web_search: Arc::new(WebSearchChain::empty()),
                selection: ProviderSelection::Mock,
            });
        }
        ProviderSelection::Real => Arc::new(RoutingProvider::new(
            vertex,
            openai,
            anthropic,
            openrouter,
            xai,
            opencode_free,
            local,
            ctx.model_registry.clone(),
            ctx.default_model.clone(),
        )),
        ProviderSelection::Unconfigured => Arc::new(UnconfiguredProvider::new()),
        ProviderSelection::FailFast => return Ok(ProviderBuildOutcome::FailFast),
    };

    Ok(ProviderBuildOutcome::Install {
        llm,
        web_search,
        selection,
    })
}

/// Build the direct-Anthropic provider from already-resolved auth, logging
/// which source it came from (the source name, never the secret) and degrading
/// to `None` (rather than aborting) if the reqwest client can't be built.
///
/// Takes the resolved auth rather than resolving it, so the caller can hand the
/// same auth to [`anthropic_search_backend`]: resolving twice would risk the
/// provider and the search backend disagreeing about which source won.
fn build_anthropic_provider(
    resolved_auth: Option<(AnthropicAuth, AnthropicAuthSource)>,
    default_model: &str,
) -> Option<AnthropicProvider> {
    let (auth, source) = resolved_auth?;
    match AnthropicProvider::new(auth, default_model.to_string()) {
        Ok(p) => {
            crate::log!(
                "[Startup] Direct Anthropic provider configured (auth from {})",
                source
            );
            Some(p)
        }
        Err(e) => {
            crate::log!("[Startup] Failed to build Anthropic provider: {}", e);
            None
        }
    }
}

/// Build the direct-OpenAI provider from an already-resolved key, logging where
/// the key came from and degrading to `None` (rather than aborting) if the
/// reqwest client can't be built.
///
/// Takes the resolved key rather than resolving it, so the caller can hand the
/// same key to [`openai_search_backend`] — resolving twice would risk the
/// provider and the search backend disagreeing about which key won.
fn build_openai_provider(
    resolved_key: Option<(String, OpenAiKeySource)>,
    default_model: &str,
) -> Option<OpenAiProvider> {
    match resolved_key {
        Some((key, source)) => {
            crate::log!("[Startup] OpenAI provider configured (key from {})", source);
            OpenAiProvider::new(key, default_model.to_string())
                .map_err(|e| crate::log!("[Startup] Failed to build OpenAI provider: {}", e))
                .ok()
        }
        None => None,
    }
}

/// Build the OpenRouter provider — an [`OpenAiProvider`] pinned to OpenRouter's
/// OpenAI-compatible Chat Completions endpoint — from a stored `openrouter`
/// credential (Settings → Models → Providers) with the `LUCIDOS_OPENROUTER_API_KEY` env
/// var as a fallback. Sends OpenRouter's optional `HTTP-Referer` / `X-Title`
/// attribution headers. `None` when no key is configured.
fn build_openrouter_provider(
    credential: Option<(AuthType, String)>,
    env_key: Option<String>,
    default_model: &str,
) -> Option<OpenAiProvider> {
    build_bearer_openai_compatible(
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

/// Build the xAI provider: an [`OpenAiProvider`] pinned to xAI's
/// OpenAI-compatible Chat Completions endpoint, serving Grok. The key comes
/// from a stored `xai` credential (Settings → Models → Providers), with
/// `LUCIDOS_XAI_API_KEY` as the fallback. `None` when neither is configured.
///
/// Sends no extra headers: `HTTP-Referer` / `X-Title` are OpenRouter's own
/// attribution convention, and xAI has no counterpart.
fn build_xai_provider(
    credential: Option<(AuthType, String)>,
    env_key: Option<String>,
    default_model: &str,
) -> Option<OpenAiProvider> {
    build_bearer_openai_compatible(
        "xAI",
        XAI_BASE_URL,
        credential,
        env_key,
        default_model,
        Vec::new(),
    )
}

/// The shared body behind [`build_openrouter_provider`] and
/// [`build_xai_provider`]: a hosted OpenAI-compatible backend authenticated by a
/// single bearer key, differing only in base URL, attribution headers and the
/// name in the startup log.
///
/// `force_chat_completions` is always true here. Neither service implements
/// OpenAI's Responses API, so a `gpt-5`-shaped id served by one must still take
/// the Chat Completions path.
///
/// The named wrappers stay, because each carries what is provider-specific:
/// which env var falls back, and why the headers are there or absent.
/// [`build_local_provider`] deliberately does NOT route through here: it
/// resolves its own base URL, tolerates an empty key, and logs that base.
fn build_bearer_openai_compatible(
    provider_label: &str,
    base_url: &str,
    credential: Option<(AuthType, String)>,
    env_key: Option<String>,
    default_model: &str,
    extra_headers: Vec<(String, String)>,
) -> Option<OpenAiProvider> {
    let key = resolve_bearer_key(credential, env_key)?;
    match OpenAiProvider::new_with_base_url(
        key,
        default_model.to_string(),
        base_url,
        extra_headers,
        true,
    ) {
        Ok(p) => {
            crate::log!("[Startup] {} provider configured", provider_label);
            Some(p)
        }
        Err(e) => {
            crate::log!(
                "[Startup] Failed to build {} provider: {}",
                provider_label,
                e
            );
            None
        }
    }
}

/// The headers the keyless provider sends on every request.
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
        (
            "User-Agent".to_string(),
            format!("Lucidos/{}", env!("CARGO_PKG_VERSION")),
        ),
    ]
}

/// Build the keyless OpenCode Free provider, or `None` when the tier is off.
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
        .map(|v| reads_as_true(&v))
        .or_else(|| {
            std::env::var("LUCIDOS_OPENCODE_FREE")
                .ok()
                .map(|v| reads_as_true(&v))
        })
        .unwrap_or(false);
    if !enabled {
        return None;
    }
    match OpenAiProvider::new_with_base_url(
        String::new(),
        default_model.to_string(),
        OPENCODE_FREE_BASE_URL,
        opencode_free_headers(),
        true,
    ) {
        Ok(p) => {
            crate::log!("[Startup] OpenCode Free provider configured (keyless)");
            Some(p)
        }
        Err(e) => {
            crate::log!("[Startup] Failed to build OpenCode Free provider: {}", e);
            None
        }
    }
}

/// Build the local OpenAI-compatible provider (Ollama / LM Studio / vLLM /
/// llama.cpp). Opt-in: only built when the user signalled local use via the
/// `local_base_url` preference (`base_url_pref`), a `local` credential
/// (`api_key_cred`), or the `LUCIDOS_LOCAL_BASE_URL` / `LUCIDOS_LOCAL_API_KEY`
/// env vars — otherwise `None`, so a default localhost backend isn't conjured
/// for users who never asked for it (and the "no provider configured" guard
/// stays honest). The base URL resolves pref → env → [`DEFAULT_LOCAL_BASE_URL`];
/// the key is optional (the `Authorization` header is omitted when empty).
fn build_local_provider(
    base_url_pref: Option<String>,
    api_key_cred: Option<String>,
    default_model: &str,
) -> Option<OpenAiProvider> {
    let base_pref = base_url_pref.filter(|s| !s.trim().is_empty());
    let base_env = std::env::var("LUCIDOS_LOCAL_BASE_URL")
        .ok()
        .filter(|s| !s.trim().is_empty());
    let key = api_key_cred.filter(|s| !s.trim().is_empty()).or_else(|| {
        std::env::var("LUCIDOS_LOCAL_API_KEY")
            .ok()
            .filter(|s| !s.trim().is_empty())
    });

    if base_pref.is_none() && base_env.is_none() && key.is_none() {
        return None;
    }

    let base = base_pref
        .or(base_env)
        .unwrap_or_else(|| DEFAULT_LOCAL_BASE_URL.to_string());
    match OpenAiProvider::new_with_base_url(
        key.unwrap_or_default(),
        default_model.to_string(),
        &base,
        Vec::new(),
        true,
    ) {
        Ok(p) => {
            crate::log!(
                "[Startup] Local OpenAI-compatible provider configured (base {})",
                base
            );
            Some(p)
        }
        Err(e) => {
            crate::log!("[Startup] Failed to build local provider: {}", e);
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{
        delete_credential, seed_credential, seed_preference, setup_test_db, teardown_test_db,
    };

    /// Build a non-mock context with no Vertex and the boot-without-provider gate
    /// on — the packaged first-run shape. `model_is_mock: false` is the load-
    /// bearing bit: it proves the rebuild can never reach `MockProvider`.
    fn unconfigured_ctx(boot_without_provider: bool) -> ProviderBuildContext {
        ProviderBuildContext {
            default_model: crate::core::DEFAULT_CHAT_MODEL.to_string(),
            model_is_mock: false,
            vertex_project_id: String::new(),
            vertex_location: crate::llm::vertex::location_handle("europe-west1".to_string()),
            vertex_token_cache: None,
            model_registry: crate::llm::model_registry::empty(),
            boot_without_provider,
        }
    }

    fn install(outcome: ProviderBuildOutcome) -> (Arc<dyn LlmProvider>, ProviderSelection) {
        match outcome {
            ProviderBuildOutcome::Install { llm, selection, .. } => (llm, selection),
            ProviderBuildOutcome::FailFast => panic!("expected Install, got FailFast"),
        }
    }

    /// The web-search chain from an `Install` outcome.
    fn installed_search(outcome: ProviderBuildOutcome) -> Arc<WebSearchChain> {
        match outcome {
            ProviderBuildOutcome::Install { web_search, .. } => web_search,
            ProviderBuildOutcome::FailFast => panic!("expected Install, got FailFast"),
        }
    }

    /// Whether an ambient provider source could pre-configure a provider in this
    /// process: the Anthropic / OpenAI / OpenRouter / xAI / local env fallbacks, OR a
    /// Codex CLI `apikey` login on disk (`${CODEX_HOME:-~/.codex}/auth.json`),
    /// which the OpenAI builder honors as its lowest-precedence fallback. On a
    /// dev shell or CI runner that has any of these, the "no provider
    /// configured" assertions below are not meaningful; seeding an `anthropic`
    /// credential still selects `Real` either way, so that half of each test
    /// runs unconditionally.
    ///
    /// Every non-credential source belongs here, not just the obvious env vars.
    /// A developer logged into Codex, or one exporting `ANTHROPIC_API_KEY`,
    /// would otherwise see `build_active_provider` return `Real` while this gate
    /// reported false, breaking the `Unconfigured`/`FailFast` assertions.
    fn ambient_provider_env() -> bool {
        std::env::var("ANTHROPIC_API_KEY").is_ok()
            || std::env::var("OPENAI_API_KEY").is_ok()
            || std::env::var("LUCIDOS_OPENROUTER_API_KEY").is_ok()
            || std::env::var("LUCIDOS_XAI_API_KEY").is_ok()
            || std::env::var("LUCIDOS_LOCAL_BASE_URL").is_ok()
            || std::env::var("LUCIDOS_LOCAL_API_KEY").is_ok()
            || std::env::var("LUCIDOS_OPENCODE_FREE").is_ok()
            || crate::llm::openai::codex_detect::load().is_some()
    }

    /// The core fix: with the boot-without-provider gate on, no credentials →
    /// `Unconfigured`; adding the first provider credential → `Real` (configured,
    /// NOT mock); removing it → back to `Unconfigured`. Mirrors what the runtime
    /// credential subscriber does, proving the rebuild swaps the active provider
    /// without a restart.
    #[tokio::test]
    async fn rebuild_swaps_unconfigured_to_real_and_back() {
        let (pool, db) = setup_test_db().await;
        let ctx = unconfigured_ctx(true);
        let ambient = ambient_provider_env();

        // No credentials → Unconfigured (only when no ambient env provider).
        if !ambient {
            let (provider, selection) =
                install(build_active_provider(Some(&pool), &ctx).await.unwrap());
            assert_eq!(selection, ProviderSelection::Unconfigured);
            assert!(
                !provider.is_configured(),
                "no provider configured → llm_configured() must be false"
            );
            // Unconfigured reports an empty set (not None) so the picker filters
            // to nothing rather than skipping the filter.
            assert_eq!(provider.configured_providers(), Some(Vec::new()));
        }

        // Add the first provider credential (anthropic — env-independent).
        seed_credential(
            &pool,
            "anthropic",
            "https://api.anthropic.com",
            crate::core::AuthType::ApiKey,
            "sk-ant-test",
        )
        .await;

        // Rebuild → Real, configured, and NEVER mock (model_is_mock is false).
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
        // The real provider reports its live backends — at least anthropic.
        assert!(
            provider
                .configured_providers()
                .expect("routing provider reports a set")
                .contains(&crate::llm::ProviderKind::Anthropic),
            "configured_providers must include the anthropic backend just added"
        );

        // Remove it → back to Unconfigured (only meaningful with no ambient env).
        // Through the test helper, not the store directly: the store's delete
        // needs an EventBus for its CredentialDeleted emit, and `llm/` must not
        // name `crate::engine` (llm_does_not_depend_on_engine).
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

    /// The ids of the resolved search backends, in chain order.
    fn backend_ids(providers: &DirectProviders) -> Vec<&'static str> {
        providers.search_backends.iter().map(|b| b.id()).collect()
    }

    /// `ANTHROPIC_API_KEY` alone (no stored credential) configures the direct
    /// Anthropic provider AND its search backend. The search half is the one
    /// that silently goes missing: the auth is deliberately held past provider
    /// construction so both are built from it, and an env path that reached only
    /// the provider would give an env-configured user chat with no Anthropic
    /// search backend.
    ///
    /// The env value is passed as an argument, never exported, so this cannot
    /// race the rest of the binary.
    #[tokio::test]
    async fn anthropic_env_key_alone_builds_the_provider_and_its_search_backend() {
        let (pool, db) = setup_test_db().await;
        let registry = crate::llm::model_registry::empty();
        let resolved = resolve_direct_providers(
            Some(&pool),
            crate::core::DEFAULT_CHAT_MODEL,
            &registry,
            None,
            None,
            Some("sk-ant-env".to_string()),
            ProviderSwitches::default(),
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

    /// The degraded (DB-down) boot honors the env fallback too, exactly as the
    /// OpenAI env + Codex fallbacks already do there. Before this, that path
    /// hardcoded `anthropic: None`, so a DB outage silently dropped Anthropic
    /// even for a user whose key was in the environment all along.
    ///
    /// Needs no database (`pool == None`) and no process env: both key sources
    /// are arguments, which makes the with/without pair deterministic.
    #[tokio::test]
    async fn db_down_boot_honors_the_anthropic_env_key() {
        let registry = crate::llm::model_registry::empty();

        let without = resolve_direct_providers(
            None,
            crate::core::DEFAULT_CHAT_MODEL,
            &registry,
            None,
            None,
            None,
            ProviderSwitches::default(),
        )
        .await;
        assert!(
            without.anthropic.is_none(),
            "no credential and no env key means no Anthropic, even degraded"
        );
        assert!(!backend_ids(&without).contains(&"anthropic-server-tool"));

        let with = resolve_direct_providers(
            None,
            crate::core::DEFAULT_CHAT_MODEL,
            &registry,
            None,
            None,
            Some("sk-ant-env".to_string()),
            ProviderSwitches::default(),
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

    /// With the gate OFF and nothing configured, the rebuild reports `FailFast`
    /// (boot panics; the runtime subscriber keeps the current provider) — it must
    /// NOT silently install a provider. Skipped when ambient env would configure
    /// one (then `Real` is correct, not `FailFast`).
    #[tokio::test]
    async fn no_provider_gate_off_is_failfast() {
        if ambient_provider_env() {
            return;
        }
        let (pool, db) = setup_test_db().await;
        let ctx = unconfigured_ctx(false);
        assert!(
            matches!(
                build_active_provider(Some(&pool), &ctx).await.unwrap(),
                ProviderBuildOutcome::FailFast
            ),
            "no provider + gate off must be FailFast, never a silent install"
        );
        teardown_test_db(&db).await;
    }

    /// A resolved Vertex project (from env / ADC / gcloud config — here injected
    /// directly) builds the Vertex backend and reports it as configured, with no
    /// credential and regardless of ambient env. Proves the build wiring; the
    /// ADC token-minting itself is covered by `vertex::adc` unit tests + smoke.
    #[tokio::test]
    async fn resolved_vertex_project_builds_vertex_backend() {
        let (pool, db) = setup_test_db().await;
        let ctx = ProviderBuildContext {
            vertex_project_id: "my-gcp-project".to_string(),
            vertex_token_cache: Some(std::sync::Arc::new(std::sync::Mutex::new(None))),
            ..unconfigured_ctx(true)
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

    /// A stored `xai` credential alone configures the provider and reports it,
    /// so the picker stops hiding every Grok row as "not set up".
    ///
    /// This also pins the ARGUMENT ORDER into `RoutingProvider::new`, whose
    /// `openrouter`, `xai` and `local` arguments are three consecutive
    /// `Option<OpenAiProvider>`. Swap two and the compiler stays silent, but the
    /// kind reported here is the wrong one.
    #[tokio::test]
    async fn an_xai_credential_configures_and_reports_the_xai_backend() {
        let (pool, db) = setup_test_db().await;
        let ctx = unconfigured_ctx(true);
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

    /// The keyless tier is installed by a preference alone, with no credential
    /// anywhere. It is also the one provider that must never be reachable by
    /// accident, so the off case is asserted in the same test.
    #[tokio::test]
    async fn the_free_tier_is_configured_by_its_preference_and_never_a_credential() {
        let (pool, db) = setup_test_db().await;
        let ctx = unconfigured_ctx(true);

        // Default (unset) leaves it out of the configured set.
        let (provider, _) = install(build_active_provider(Some(&pool), &ctx).await.unwrap());
        assert!(
            !provider
                .configured_providers()
                .expect("a provider set is reported")
                .contains(&crate::llm::ProviderKind::OpenCodeFree),
            "the free tier must be off until the user turns it on"
        );

        seed_preference(&pool, PREF_OPENCODE_FREE_ENABLED, "true")
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

        // No credential service exists for it, so nothing can be stored and
        // nothing can be sent as a bearer.
        assert!(
            !PROVIDER_CREDENTIAL_SERVICES.contains(&"opencode-free"),
            "the keyless tier must have no credential service"
        );
        teardown_test_db(&db).await;
    }

    /// Lucidos identifies itself and nobody else. The relay gates `big-pickle`
    /// on the OpenCode CLI's own User-Agent, so the tempting fix for a 429 is
    /// to borrow it. This pins the honest header instead, and pins that the
    /// attribution list carries no credential.
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

    /// Web search resolves over the CONFIGURED PROVIDER SET, not the chat
    /// model's provider.
    ///
    /// This is the fallback that keeps OpenRouter and local-endpoint users from
    /// dead-ending: neither exposes a web search tool, so a chain derived from
    /// the chat model would leave them with nothing. Chatting on an OpenRouter
    /// model while holding an Anthropic credential must still yield a backend.
    #[tokio::test]
    async fn search_chain_ignores_the_chat_models_provider() {
        let (pool, db) = setup_test_db().await;
        let ctx = ProviderBuildContext {
            // Routes to OpenRouter, which has no search tool of its own.
            default_model: "z-ai/glm-5.2".to_string(),
            ..unconfigured_ctx(true)
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
        // Presence is not enough — the backend must also be given a model
        // Anthropic can actually serve. Handing it the OpenRouter chat model id
        // would make every cross-provider fallback fail at the API, i.e. exactly
        // when the fallback matters.
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

    /// The chat model IS reused when it belongs to the provider — that keeps
    /// search on the tier the user chose in the ordinary single-provider case,
    /// and is why this isn't just a hardcoded model everywhere.
    #[test]
    fn search_model_prefers_the_chat_model_when_it_matches_the_provider() {
        let registry = crate::llm::model_registry::empty();
        // `provider_kind_for` falls back to a prefix heuristic for ids with no
        // registry row: `claude-*` → Vertex, `gpt-*` → OpenAI.
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

    /// …but the `[1m]` suffix comes off first. It is a Lucidos marker for the
    /// 1M-context beta, not part of a real model id, and the one-shot search
    /// call has no beta to attach it to. `claude-fable-5[1m]` is a seeded
    /// builtin that routes to direct Anthropic, so before this the whole
    /// Anthropic search backend 404'd out of the chain for anyone using it.
    #[test]
    fn search_model_strips_the_lucidos_only_1m_suffix() {
        let registry = crate::llm::model_registry::empty();
        // `claude-fable*` routes to Anthropic through the prefix heuristic, so
        // this is the reuse branch, suffix and all.
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
        // A bare id is untouched.
        assert_eq!(
            search_model_for(
                &registry,
                crate::llm::ProviderKind::Anthropic,
                "claude-fable-5",
                ANTHROPIC_FALLBACK_SEARCH_MODEL
            ),
            "claude-fable-5"
        );
        // A point release keeps its trailing `-1` when the suffix comes off.
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

    /// …and is replaced when it does not. Regression for the cross-provider bug:
    /// every backend was handed the global chat model, so an OpenRouter or
    /// Vertex chat model was sent to Anthropic / OpenAI and rejected.
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

    /// A reused chat model must ALSO run on the Responses API.
    ///
    /// `web_search` exists nowhere else, so a `models` row like `gpt-4o` would
    /// post the tool to `/responses` for a model the engine's own predicate
    /// rejects. OpenAI is last in the chain, so there is nothing after it.
    #[test]
    fn a_chat_completions_chat_model_never_becomes_the_search_model() {
        let registry = crate::llm::model_registry::empty();
        assert_eq!(
            openai_search_model(&registry, "gpt-4o"),
            OPENAI_FALLBACK_SEARCH_MODEL,
            "gpt-4o routes to OpenAI but not to the Responses API"
        );
        // A Responses-API chat model is still reused, which is the whole point
        // of preferring the tier the user picked.
        assert_eq!(openai_search_model(&registry, "gpt-5.6-sol"), "gpt-5.6-sol");
        // Another provider's model still falls back, as before.
        assert_eq!(
            openai_search_model(&registry, "claude-opus-5"),
            OPENAI_FALLBACK_SEARCH_MODEL
        );
    }

    /// The veto gates the BUILDER, not its result.
    ///
    /// Filtering afterwards logs "provider configured" for a provider the
    /// router never receives. That is the line a user reads when a switch
    /// looks like it did nothing.
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

    /// A credential the engine cannot READ omits its provider, rather than
    /// falling through to the env key.
    ///
    /// Take a workspace with a stored `openai` credential for one account and
    /// `OPENAI_API_KEY` for another. One transient pool error at boot would
    /// otherwise put every later turn on the second account.
    #[tokio::test]
    async fn an_unreadable_credential_still_boots_on_the_environment_key() {
        let (pool, db) = setup_test_db().await;
        let registry = crate::llm::model_registry::empty();

        let readable = resolve_direct_providers(
            Some(&pool),
            crate::core::DEFAULT_CHAT_MODEL,
            &registry,
            None,
            None,
            Some("sk-ant-env".to_string()),
            ProviderSwitches::default(),
        )
        .await;
        assert!(
            readable.anthropic.is_some(),
            "the env key alone configures the provider while the read works"
        );

        // Make every credential read fail, exactly as a pool error does.
        sqlx::query("DROP TABLE credentials CASCADE")
            .execute(&pool)
            .await
            .unwrap();

        let unreadable = resolve_direct_providers(
            Some(&pool),
            crate::core::DEFAULT_CHAT_MODEL,
            &registry,
            None,
            None,
            Some("sk-ant-env".to_string()),
            ProviderSwitches::default(),
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

    /// The OpenAI fallback must satisfy `uses_responses_api`, because the
    /// `web_search` tool only exists on the Responses API — a Chat-Completions
    /// id would make the backend dead on arrival.
    ///
    /// Calls the real predicate rather than re-spelling it. A copy of the rule
    /// keeps passing after the rule itself narrows, which is exactly the drift
    /// this test exists to catch.
    #[test]
    fn openai_fallback_search_model_routes_to_the_responses_api() {
        assert!(
            crate::llm::openai::uses_responses_api(OPENAI_FALLBACK_SEARCH_MODEL),
            "{OPENAI_FALLBACK_SEARCH_MODEL} must route to the Responses API"
        );
    }

    /// Vertex leads the chain whenever it is configured, so an existing Vertex
    /// workspace keeps the exact search behavior it had before the chain
    /// existed. Asserts position, not set membership — ambient env may add an
    /// OpenAI backend on a developer machine.
    #[tokio::test]
    async fn vertex_leads_the_chain_when_configured() {
        let (pool, db) = setup_test_db().await;
        let ctx = ProviderBuildContext {
            vertex_project_id: "my-gcp-project".to_string(),
            vertex_token_cache: Some(std::sync::Arc::new(std::sync::Mutex::new(None))),
            ..unconfigured_ctx(true)
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

    /// No search-capable provider → an empty chain that reports how to enable
    /// search, rather than a silently broken tool. `local` is deliberate: it is
    /// a configured LLM provider that offers no search tool, so it proves the
    /// chain tracks search capability rather than mere provider presence.
    #[tokio::test]
    async fn local_only_workspace_gets_an_empty_chain() {
        if ambient_provider_env() {
            // A developer machine with ANTHROPIC_API_KEY / OPENAI_API_KEY / a
            // Codex login would add a real backend and make this assertion
            // meaningless.
            return;
        }
        let (pool, db) = setup_test_db().await;
        let ctx = unconfigured_ctx(true);
        seed_credential(
            &pool,
            "local",
            DEFAULT_LOCAL_BASE_URL,
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
        let msg = chain.search("q", 5).await.unwrap_err().to_string();
        assert!(msg.contains("Settings → Models → Providers"), "{msg}");
        teardown_test_db(&db).await;
    }

    // ---- Per-provider enable switches -------------------------------------

    /// Absent means enabled, and only an explicit off turns a switch off. This
    /// is the back-compat guarantee: a workspace that never opened the page has
    /// no rows at all, and must resolve every provider it always did.
    #[test]
    fn a_switch_is_on_unless_it_explicitly_says_off() {
        assert!(switch_is_on(None), "absent must mean enabled");
        for on in ["true", "TRUE", " on ", "1", "yes"] {
            assert!(switch_is_on(Some(on)), "{on} must read as enabled");
        }
        for off in ["false", "FALSE", " off ", "0", "no"] {
            assert!(!switch_is_on(Some(off)), "{off} must read as disabled");
        }
        // Neither vocabulary. Enabled, because a garbled value must not be the
        // thing that quietly removes a working provider.
        assert!(switch_is_on(Some("maybe")));
        assert!(switch_is_on(Some("")));
    }

    /// The default is every switch on, which is what a DB-down boot resolves.
    #[test]
    fn default_switches_are_all_on() {
        let s = ProviderSwitches::default();
        assert!(s.vertex && s.anthropic && s.openai && s.openrouter && s.xai && s.local);
    }

    /// A switched-off provider leaves the router AND takes its search backend
    /// with it, while its stored credential stays exactly where it was. That
    /// pairing is the whole point: the switch parks a key, it does not spend it.
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
            crate::core::DEFAULT_CHAT_MODEL,
            &registry,
            None,
            None,
            None,
            ProviderSwitches::default(),
        )
        .await;
        assert!(
            on.anthropic.is_some(),
            "the seeded credential configures it"
        );
        assert!(backend_ids(&on).contains(&"anthropic-server-tool"));

        let off = resolve_direct_providers(
            Some(&pool),
            crate::core::DEFAULT_CHAT_MODEL,
            &registry,
            None,
            None,
            None,
            ProviderSwitches {
                anthropic: false,
                ..ProviderSwitches::default()
            },
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

    /// The switch is a veto, never an installer: turning one on for a provider
    /// with no credential and no env key configures nothing.
    #[tokio::test]
    async fn a_switch_never_installs_an_unconfigured_provider() {
        let (pool, db) = setup_test_db().await;
        let registry = crate::llm::model_registry::empty();
        seed_preference(&pool, PREF_PROVIDER_ENABLED_OPENROUTER, "true")
            .await
            .unwrap();
        let resolved = resolve_direct_providers(
            Some(&pool),
            crate::core::DEFAULT_CHAT_MODEL,
            &registry,
            None,
            None,
            None,
            read_provider_switches(&pool).await,
        )
        .await;
        assert!(
            resolved.openrouter.is_none() || std::env::var("LUCIDOS_OPENROUTER_API_KEY").is_ok(),
            "an on switch with no key must configure nothing"
        );
        teardown_test_db(&db).await;
    }

    /// The stored preferences reach the resolved switches, key by key. Reads
    /// them through the real store so a misspelled constant cannot pass.
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
                }
            ),
            "a workspace with no rows must resolve every switch on"
        );

        for key in PROVIDER_ENABLED_KEYS {
            seed_preference(&pool, key, "false").await.unwrap();
        }
        let s = read_provider_switches(&pool).await;
        assert!(
            !s.vertex && !s.anthropic && !s.openai && !s.openrouter && !s.xai && !s.local,
            "every key must reach its own field: {s:?}"
        );
        teardown_test_db(&db).await;
    }

    /// A switch that needed a restart is a switch the user reads as broken, so
    /// the config subscriber has to watch all six.
    #[test]
    fn every_switch_hot_swaps() {
        for key in PROVIDER_ENABLED_KEYS {
            assert!(
                PROVIDER_PREFERENCE_KEYS.contains(&key),
                "{key} must be watched by the provider config subscriber"
            );
        }
    }

    /// The frontend mirrors this watch set, to re-probe `/health` on the same
    /// preferences that rebuild the provider here. A key added on this side and
    /// forgotten there leaves the model picker offering a provider the engine
    /// has already dropped, with nothing failing.
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

    /// The agent must not be able to switch a provider off. Asserted from this
    /// side, over the same key list the build reads, so a renamed constant
    /// cannot leave a settable key behind in the catalog.
    #[test]
    fn no_switch_is_agent_settable() {
        for key in PROVIDER_ENABLED_KEYS {
            assert!(
                crate::core::preference_catalog::INTERNAL_KEYS
                    .iter()
                    .any(|(k, _)| *k == key),
                "{key} must be an INTERNAL_KEY: the provider it switches off may \
                 be the one answering the turn"
            );
        }
    }
}
