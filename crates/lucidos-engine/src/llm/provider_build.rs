//! Build the engine's active `Arc<dyn LlmProvider>` from current credentials,
//! preferences, and env — the single construction path shared by startup
//! (`main.rs`) and the runtime credential subscriber (`spawn_provider_credential_subscriber`).
//!
//! The decision of *which* provider to install lives in
//! [`crate::llm::select_provider`] (unit-tested matrix); this module resolves
//! that decision's boolean inputs from the DB + env and maps the chosen
//! [`ProviderSelection`] onto a concrete provider. Factoring it out of `main.rs`
//! means a runtime hot-swap produces a provider byte-identical to a fresh boot.

use crate::core::{
    AuthType, CredentialStore, PreferenceStore, DEFAULT_LOCAL_BASE_URL, PREF_LOCAL_BASE_URL,
};
use crate::llm::web_search::{
    AnthropicServerToolSearch, OpenAiResponsesSearch, VertexGroundingSearch, WebSearchChain,
    WebSearchProvider,
};
use crate::llm::{
    resolve_bearer_key, resolve_openai_api_key, select_provider, AnthropicAuth, AnthropicProvider,
    LlmProvider, OpenAiKeySource, OpenAiProvider, ProviderSelection, ProviderSelectionInputs,
    RoutingProvider, UnconfiguredProvider, VertexProvider, OPENAI_DEFAULT_BASE_URL,
    OPENROUTER_BASE_URL,
};
use sqlx::PgPool;
use std::sync::Arc;

/// Credential service names that, when created/updated/deleted, change which LLM
/// provider is installed. Vertex is env/gcloud-based (no credential) and
/// hot-swaps its region via `spawn_vertex_region_subscriber` instead — so it is
/// deliberately absent here. The credential subscriber filters on this set.
pub const PROVIDER_CREDENTIAL_SERVICES: [&str; 4] = ["openai", "anthropic", "openrouter", "local"];

/// Whether `LUCIDOS_BOOT_WITHOUT_PROVIDER` is truthy — a packaged build lets the
/// engine boot (into `UnconfiguredProvider`) before any provider is configured,
/// instead of the dev/docker fail-fast panic. Read in both `main.rs` (boot) and
/// the subscriber (so a runtime swap-back to unconfigured mirrors boot).
pub fn boot_without_provider_enabled() -> bool {
    std::env::var("LUCIDOS_BOOT_WITHOUT_PROVIDER")
        .map(|v| {
            matches!(
                v.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
        .unwrap_or(false)
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
    local: Option<OpenAiProvider>,
    /// Search backends in chain order — Anthropic before OpenAI. Vertex is
    /// prepended by the caller, which owns the Vertex config.
    search_backends: Vec<Arc<dyn WebSearchProvider>>,
}

/// Resolve the direct (OpenAI-wire + Anthropic) providers from credentials +
/// env. `pool == None` means the DB is unavailable (a degraded boot) — the env
/// fallbacks (`OPENAI_API_KEY`, `LUCIDOS_OPENROUTER_API_KEY`,
/// `LUCIDOS_LOCAL_*`) and the Codex-detected OpenAI key still apply, but stored
/// credentials and the `local_base_url` preference can't be read (so no direct
/// Anthropic). Every field degrades to `None` / an omitted backend on any
/// read/build error so the engine still comes up on its other providers.
async fn resolve_direct_providers(
    pool: Option<&PgPool>,
    default_model: &str,
    registry: &crate::llm::model_registry::ModelRegistry,
    openai_env_key: Option<String>,
    openai_codex_key: Option<String>,
) -> DirectProviders {
    let Some(pool) = pool else {
        // No DB access, but the env-var + Codex fallbacks must still work.
        let openai_key = resolve_openai_api_key(None, openai_env_key, openai_codex_key);
        let openai = build_openai_provider(openai_key.clone(), default_model);
        let openrouter = build_openrouter_provider(
            None,
            std::env::var("LUCIDOS_OPENROUTER_API_KEY").ok(),
            default_model,
        );
        let local = build_local_provider(None, None, default_model);
        return DirectProviders {
            openai,
            anthropic: None,
            openrouter,
            local,
            search_backends: openai_search_backend(openai_key, registry, default_model)
                .into_iter()
                .collect(),
        };
    };

    // Held past provider construction so the Anthropic search backend can be
    // built from the same auth (`AnthropicProvider` keeps its copy private).
    let anthropic_auth = match CredentialStore::get(pool, "anthropic").await {
        Ok(Some(cred)) => match cred.auth_type {
            AuthType::ApiKey => Some(AnthropicAuth::ApiKey(cred.auth_value)),
            AuthType::Bearer => Some(AnthropicAuth::OAuthBearer(cred.auth_value)),
            other => {
                crate::log!(
                    "[Startup] Anthropic credential auth_type {} unsupported (expected api_key or bearer) — direct Anthropic disabled",
                    other
                );
                None
            }
        },
        Ok(None) => None,
        Err(e) => {
            crate::log!("[Startup] Failed to read Anthropic credential: {}", e);
            None
        }
    };
    let anthropic = anthropic_auth.clone().and_then(|a| {
        match AnthropicProvider::new(a, default_model.to_string()) {
            Ok(p) => {
                crate::log!("[Startup] Direct Anthropic provider configured");
                Some(p)
            }
            Err(e) => {
                crate::log!("[Startup] Failed to build Anthropic provider: {}", e);
                None
            }
        }
    });

    // OpenAI: a stored `openai` credential wins; otherwise the env fallback.
    let openai_credential = match CredentialStore::get(pool, "openai").await {
        Ok(Some(cred)) => Some((cred.auth_type, cred.auth_value)),
        Ok(None) => None,
        Err(e) => {
            crate::log!("[Startup] Failed to read OpenAI credential: {}", e);
            None
        }
    };
    // Resolved once and reused: the provider needs it, and so does the OpenAI
    // search backend.
    let openai_key = resolve_openai_api_key(openai_credential, openai_env_key, openai_codex_key);
    let openai = build_openai_provider(openai_key.clone(), default_model);

    // OpenRouter: a stored `openrouter` credential wins; otherwise the env fallback.
    let openrouter_credential = match CredentialStore::get(pool, "openrouter").await {
        Ok(Some(cred)) => Some((cred.auth_type, cred.auth_value)),
        Ok(None) => None,
        Err(e) => {
            crate::log!("[Startup] Failed to read OpenRouter credential: {}", e);
            None
        }
    };
    let openrouter = build_openrouter_provider(
        openrouter_credential,
        std::env::var("LUCIDOS_OPENROUTER_API_KEY").ok(),
        default_model,
    );

    // Local OpenAI-compatible: base URL from the `local_base_url` pref (env /
    // default applied inside the builder) and an optional `local` credential.
    let local_base_pref = match PreferenceStore::get(pool, PREF_LOCAL_BASE_URL).await {
        Ok(opt) => opt,
        Err(e) => {
            crate::log!("[Startup] Failed to read local_base_url preference: {}", e);
            None
        }
    };
    let local_key = match CredentialStore::get(pool, "local").await {
        Ok(Some(cred)) => Some(cred.auth_value),
        Ok(None) => None,
        Err(e) => {
            crate::log!("[Startup] Failed to read local provider credential: {}", e);
            None
        }
    };
    let local = build_local_provider(local_base_pref, local_key, default_model);

    // Chain order: Anthropic before OpenAI, because Anthropic's server tool has
    // no per-call fee while OpenAI's Responses web search bills per call on top
    // of the tokens the results consume.
    let mut search_backends: Vec<Arc<dyn WebSearchProvider>> = Vec::new();
    if let Some(auth) = anthropic_auth {
        let model = search_model_for(
            registry,
            crate::llm::ProviderKind::Anthropic,
            default_model,
            ANTHROPIC_FALLBACK_SEARCH_MODEL,
        );
        match AnthropicServerToolSearch::new(auth, model) {
            Ok(b) => search_backends.push(Arc::new(b)),
            Err(e) => crate::log!("[Startup] Failed to build Anthropic search backend: {}", e),
        }
    }
    search_backends.extend(openai_search_backend(openai_key, registry, default_model));

    DirectProviders {
        openai,
        anthropic,
        openrouter,
        local,
        search_backends,
    }
}

/// Last-resort search model per provider, used when the configured chat model
/// belongs to a *different* provider. Small, current, broadly-available ids —
/// the search call is a short summary over the results, so the cheapest capable
/// model is the right tier. `gpt-5.5` additionally satisfies OpenAI's
/// `uses_responses_api` prefix check, which the `web_search` tool requires.
const ANTHROPIC_FALLBACK_SEARCH_MODEL: &str = "claude-haiku-4-5";
const OPENAI_FALLBACK_SEARCH_MODEL: &str = "gpt-5.5";

/// A model id valid for `provider`, for the one-shot search call.
///
/// Prefers the configured chat model — it is known to work for this user and
/// keeps search on the tier they picked — **but only when that model actually
/// routes to `provider`**. The entire point of the chain is that search can run
/// on a provider the user is *not* chatting with, and in that case the chat
/// model id is meaningless to the search provider: handing OpenRouter's
/// `z-ai/glm-5.2` to Anthropic's Messages API is a hard rejection, which would
/// make the advertised fallback fail every time it was actually needed.
fn search_model_for(
    registry: &crate::llm::model_registry::ModelRegistry,
    provider: crate::llm::ProviderKind,
    chat_model: &str,
    fallback: &str,
) -> String {
    if crate::llm::model_registry::provider_kind_for(registry, chat_model) == provider {
        chat_model.to_string()
    } else {
        fallback.to_string()
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
    let model = search_model_for(
        registry,
        crate::llm::ProviderKind::OpenAi,
        default_model,
        OPENAI_FALLBACK_SEARCH_MODEL,
    );
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

    // Vertex (env/gcloud-based, not a credential). Reuse the engine's warm
    // token cache so a rebuild doesn't discard cached access tokens; fall back
    // to a fresh cache only when none exists (project configured but no cache —
    // not reachable for a non-mock boot, defensive).
    let vertex = if !ctx.vertex_project_id.is_empty() {
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
    let DirectProviders {
        openai,
        anthropic,
        openrouter,
        local,
        search_backends,
    } = resolve_direct_providers(
        pool,
        &ctx.default_model,
        &ctx.model_registry,
        openai_env_key,
        openai_codex_key,
    )
    .await;

    let selection = select_provider(ProviderSelectionInputs {
        model_is_mock: false,
        has_vertex: vertex.is_some(),
        has_openai: openai.is_some(),
        has_anthropic: anthropic.is_some(),
        has_openrouter: openrouter.is_some(),
        has_local: local.is_some(),
        boot_without_provider: ctx.boot_without_provider,
    });

    // Vertex leads the chain so an existing Vertex workspace keeps the exact
    // search behavior it had. Built from the engine's Vertex config rather than
    // from the `vertex` provider above, which is about to be moved into the
    // router — and which carries the chat model, not the grounding one.
    let mut backends: Vec<Arc<dyn WebSearchProvider>> = Vec::new();
    if !ctx.vertex_project_id.is_empty() {
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
    let key = resolve_bearer_key(credential, env_key)?;
    let extra_headers = vec![
        (
            "HTTP-Referer".to_string(),
            "https://lucidos.dev".to_string(),
        ),
        ("X-Title".to_string(), "Lucidos".to_string()),
    ];
    match OpenAiProvider::new_with_base_url(
        key,
        default_model.to_string(),
        OPENROUTER_BASE_URL,
        extra_headers,
        true,
    ) {
        Ok(p) => {
            crate::log!("[Startup] OpenRouter provider configured");
            Some(p)
        }
        Err(e) => {
            crate::log!("[Startup] Failed to build OpenRouter provider: {}", e);
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
    use crate::test_support::{setup_test_db, teardown_test_db};

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
    /// process: the OpenAI / OpenRouter / local env fallbacks, OR a Codex CLI
    /// `apikey` login on disk (`${CODEX_HOME:-~/.codex}/auth.json`), which the
    /// OpenAI builder now honors as its lowest-precedence fallback. On a dev
    /// shell or CI runner that has any of these, the "no provider configured"
    /// assertions below are not meaningful — the `anthropic`-credential
    /// transition (env- and file-independent) still is. (Anthropic has no env
    /// fallback in `resolve_direct_providers`.) The Codex source must be included
    /// here, not just the env vars: a developer logged into Codex would otherwise
    /// see `build_active_provider` return `Real` while this gate reported false,
    /// breaking the `Unconfigured`/`FailFast` assertions.
    fn ambient_provider_env() -> bool {
        std::env::var("OPENAI_API_KEY").is_ok()
            || std::env::var("LUCIDOS_OPENROUTER_API_KEY").is_ok()
            || std::env::var("LUCIDOS_LOCAL_BASE_URL").is_ok()
            || std::env::var("LUCIDOS_LOCAL_API_KEY").is_ok()
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
        crate::core::CredentialStore::upsert(
            &pool,
            "anthropic",
            "https://api.anthropic.com",
            crate::core::AuthType::ApiKey,
            "sk-ant-test",
            None,
            None,
        )
        .await
        .expect("upsert anthropic credential");

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
        crate::core::CredentialStore::delete(&pool, "anthropic")
            .await
            .expect("delete anthropic credential");
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
        crate::core::CredentialStore::upsert(
            &pool,
            "anthropic",
            "https://api.anthropic.com",
            crate::core::AuthType::ApiKey,
            "sk-ant-test",
            None,
            None,
        )
        .await
        .expect("upsert anthropic credential");

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

    /// The OpenAI fallback must satisfy `uses_responses_api`, because the
    /// `web_search` tool only exists on the Responses API — a Chat-Completions
    /// id would make the backend dead on arrival.
    #[test]
    fn openai_fallback_search_model_routes_to_the_responses_api() {
        assert!(
            OPENAI_FALLBACK_SEARCH_MODEL.starts_with("gpt-5")
                || OPENAI_FALLBACK_SEARCH_MODEL.contains("codex"),
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
        crate::core::CredentialStore::upsert(
            &pool,
            "anthropic",
            "https://api.anthropic.com",
            crate::core::AuthType::ApiKey,
            "sk-ant-test",
            None,
            None,
        )
        .await
        .expect("upsert anthropic credential");

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
            // A developer machine with OPENAI_API_KEY / a Codex login would add
            // a real backend and make this assertion meaningless.
            return;
        }
        let (pool, db) = setup_test_db().await;
        let ctx = unconfigured_ctx(true);
        crate::core::CredentialStore::upsert(
            &pool,
            "local",
            DEFAULT_LOCAL_BASE_URL,
            crate::core::AuthType::ApiKey,
            "local-key",
            None,
            None,
        )
        .await
        .expect("upsert local credential");

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
}
