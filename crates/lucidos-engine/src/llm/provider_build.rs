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
use crate::llm::{
    resolve_bearer_key, resolve_openai_api_key, select_provider, AnthropicAuth, AnthropicProvider,
    LlmProvider, OpenAiProvider, ProviderSelection, ProviderSelectionInputs, RoutingProvider,
    UnconfiguredProvider, VertexProvider,
};
use sqlx::PgPool;
use std::sync::Arc;

/// OpenRouter's OpenAI-compatible base URL.
const OPENROUTER_BASE_URL: &str = "https://openrouter.ai/api/v1";

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
        .map(|v| matches!(v.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on"))
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
    /// A provider to install (Mock / Real / Unconfigured). The selection is
    /// carried for logging.
    Install(Arc<dyn LlmProvider>, ProviderSelection),
    /// No real provider and the boot-without-provider gate is off. Boot panics
    /// with the configuration message; the runtime subscriber keeps the current
    /// provider in place (never panics on a credential delete).
    FailFast,
}

/// Resolve the direct (OpenAI-wire + Anthropic) providers from credentials +
/// env. `pool == None` means the DB is unavailable (a degraded boot) — the env
/// fallbacks (`OPENAI_API_KEY`, `LUCIDOS_OPENROUTER_API_KEY`,
/// `LUCIDOS_LOCAL_*`) still apply, but stored credentials and the
/// `local_base_url` preference can't be read (so no direct Anthropic).
/// Returns `(openai, anthropic, openrouter, local)`; each degrades to `None` on
/// any read/build error so the engine still comes up on its other providers.
async fn resolve_direct_providers(
    pool: Option<&PgPool>,
    default_model: &str,
    openai_env_key: Option<String>,
) -> (
    Option<OpenAiProvider>,
    Option<AnthropicProvider>,
    Option<OpenAiProvider>,
    Option<OpenAiProvider>,
) {
    let Some(pool) = pool else {
        // No DB access, but the env-var fallbacks must still work.
        let openai = build_openai_provider(None, openai_env_key, default_model);
        let openrouter = build_openrouter_provider(
            None,
            std::env::var("LUCIDOS_OPENROUTER_API_KEY").ok(),
            default_model,
        );
        let local = build_local_provider(None, None, default_model);
        return (openai, None, openrouter, local);
    };

    let anthropic = match CredentialStore::get(pool, "anthropic").await {
        Ok(Some(cred)) => {
            let auth = match cred.auth_type {
                AuthType::ApiKey => Some(AnthropicAuth::ApiKey(cred.auth_value)),
                AuthType::Bearer => Some(AnthropicAuth::OAuthBearer(cred.auth_value)),
                other => {
                    crate::log!(
                        "[Startup] Anthropic credential auth_type {} unsupported (expected api_key or bearer) — direct Anthropic disabled",
                        other
                    );
                    None
                }
            };
            match auth.and_then(|a| {
                AnthropicProvider::new(a, default_model.to_string())
                    .map_err(|e| crate::log!("[Startup] Failed to build Anthropic provider: {}", e))
                    .ok()
            }) {
                Some(p) => {
                    crate::log!("[Startup] Direct Anthropic provider configured");
                    Some(p)
                }
                None => None,
            }
        }
        Ok(None) => None,
        Err(e) => {
            crate::log!("[Startup] Failed to read Anthropic credential: {}", e);
            None
        }
    };

    // OpenAI: a stored `openai` credential wins; otherwise the env fallback.
    let openai_credential = match CredentialStore::get(pool, "openai").await {
        Ok(Some(cred)) => Some((cred.auth_type, cred.auth_value)),
        Ok(None) => None,
        Err(e) => {
            crate::log!("[Startup] Failed to read OpenAI credential: {}", e);
            None
        }
    };
    let openai = build_openai_provider(openai_credential, openai_env_key, default_model);

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

    (openai, anthropic, openrouter, local)
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
        return Ok(ProviderBuildOutcome::Install(
            Arc::new(crate::llm::mock::MockProvider::new(ctx.default_model.clone())),
            ProviderSelection::Mock,
        ));
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
    let (openai, anthropic, openrouter, local) =
        resolve_direct_providers(pool, &ctx.default_model, openai_env_key).await;

    let selection = select_provider(ProviderSelectionInputs {
        model_is_mock: false,
        has_vertex: vertex.is_some(),
        has_openai: openai.is_some(),
        has_anthropic: anthropic.is_some(),
        has_openrouter: openrouter.is_some(),
        has_local: local.is_some(),
        boot_without_provider: ctx.boot_without_provider,
    });

    let provider: Arc<dyn LlmProvider> = match selection {
        // Unreachable: `model_is_mock` is false here, and `select_provider` only
        // returns Mock when that input is true. Guarded above regardless.
        ProviderSelection::Mock => {
            return Ok(ProviderBuildOutcome::Install(
                Arc::new(crate::llm::mock::MockProvider::new(ctx.default_model.clone())),
                ProviderSelection::Mock,
            ));
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

    Ok(ProviderBuildOutcome::Install(provider, selection))
}

/// Build the direct-OpenAI provider from a resolved key, logging where the key
/// came from and degrading to `None` (rather than aborting) if the reqwest
/// client can't be built. Shared by both branches of [`resolve_direct_providers`]
/// so the DB-up and DB-down paths honor the `OPENAI_API_KEY` fallback identically.
fn build_openai_provider(
    credential: Option<(AuthType, String)>,
    env_key: Option<String>,
    default_model: &str,
) -> Option<OpenAiProvider> {
    match resolve_openai_api_key(credential, env_key) {
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
        ("HTTP-Referer".to_string(), "https://lucidos.dev".to_string()),
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
            ProviderBuildOutcome::Install(p, s) => (p, s),
            ProviderBuildOutcome::FailFast => panic!("expected Install, got FailFast"),
        }
    }

    /// Whether ambient provider env vars (`OPENAI_API_KEY`, OpenRouter, local)
    /// could pre-configure a provider in this process. The OpenAI/OpenRouter/
    /// local builders honor env fallbacks, so on a dev shell that has them set
    /// the "no provider configured" assertions below are not meaningful — the
    /// `anthropic`-credential transition (env-independent) still is. (Anthropic
    /// has no env fallback in `resolve_direct_providers`.)
    fn ambient_provider_env() -> bool {
        std::env::var("OPENAI_API_KEY").is_ok()
            || std::env::var("LUCIDOS_OPENROUTER_API_KEY").is_ok()
            || std::env::var("LUCIDOS_LOCAL_BASE_URL").is_ok()
            || std::env::var("LUCIDOS_LOCAL_API_KEY").is_ok()
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
        assert_ne!(selection, ProviderSelection::Mock, "must never swap to mock");
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
}
