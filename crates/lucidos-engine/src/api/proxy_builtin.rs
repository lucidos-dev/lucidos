//! Builtin model-provider proxies.
//!
//! An app calls `lucidos.proxy(<name>).fetch(path, init)` → the engine's
//! `/api/v1/proxy/<name>/<path>` route. `lucidos proxy` and the agent's
//! `proxy_request` tool resolve the same way. When `<name>` has no entry in
//! `data/config/apis.json` but matches a row of [`BUILTIN_PROXIES`], the
//! engine synthesizes the upstream target here: the provider's API base URL
//! plus a server-side auth layer sourced from the engine's OWN provider auth, resolved
//! exactly as the LLM providers resolve it (a stored credential first, then the
//! provider's env fallback), so a workspace never has to duplicate a provider
//! credential into `apis.json`.
//!
//! **A builtin name is not always a model-registry row.** Six are
//! `ProviderKind`s and `typesafe` is not. Jev answers typed questions rather
//! than holding a conversation, so it never enters the model picker
//! (ADR 0220). Nothing here needs a `ProviderKind`: a resolver reads a
//! credential by name and pins a base URL.
//!
//! **Precedence.** `apis.json` is consulted first (`resolve_proxy_target`);
//! this fallback fires only on that 404, so an `apis.json` entry with the same
//! name always overrides the builtin. See `system-knowhow/js-sdk.md`
//! § `lucidos.proxy`.
//!
//! **What is injected.** Only the credential/token the iframe must never see —
//! `Authorization: Bearer …` (openai / openrouter / xai / local / vertex /
//! anthropic-OAuth) or `x-api-key: …` (anthropic API key). Content-Type,
//! `anthropic-version`, and attribution headers stay app-owned.
//!
//! **Vertex** is the one dynamic case: the base URL is the engine-owned prefix
//! `https://<host>/v1/projects/<project>/locations/<region>` (project + region
//! from the engine's boot-resolved Vertex config), so the app sends only the
//! `/publishers/<publisher>/models/<model>:<method>` suffix; the access token is
//! minted/refreshed server-side per request via the shared token cache.

use crate::api::proxy_auth_layer::{AuthLayer, AuthMutation, LayerInput, RetryHint, ScopeBinding};
use crate::api::proxy_static_layers::StaticHeaderLayer;
use crate::core::{preferences::local_base_url_rejection, prefs, AuthType, CredentialStore};
use crate::llm::judgment::{
    TYPESAFE_API_BASE_URL, TYPESAFE_API_KEY_ENV, TYPESAFE_CREDENTIAL_SERVICE,
};
use crate::llm::vertex::{self, TokenCache};
use crate::llm::{
    resolve_anthropic_auth, resolve_bearer_key, resolve_openai_api_key, AnthropicAuth,
    ANTHROPIC_API_BASE_URL, OPENAI_DEFAULT_BASE_URL, OPENROUTER_BASE_URL, XAI_BASE_URL,
};
use async_trait::async_trait;
use axum::http::{HeaderName, StatusCode};
use std::sync::{Arc, LazyLock};

/// A resolved builtin target: the upstream base URL + the pre-built auth
/// pipeline. Bound through [`crate::api::proxy::ScopedPipeline`] before it can
/// be dispatched, exactly as an `apis.json` pipeline is.
type BuiltinTarget = (String, Vec<Arc<dyn AuthLayer>>);

/// Which resolver serves a builtin. An enum, so the match in
/// [`resolve_entry`] proves every catalog row has one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Resolver {
    Anthropic,
    Local,
    OpenAi,
    OpenRouter,
    TypeSafe,
    Vertex,
    XAi,
}

/// One builtin provider proxy.
#[derive(Debug)]
pub(crate) struct BuiltinProxy {
    /// The proxy name a caller addresses, as in `lucidos proxy <name>`.
    pub name: &'static str,
    /// Other spellings of the same service, compared after
    /// [`normalized_service_name`]. `request_credential` reads them.
    pub aliases: &'static [&'static str],
    /// The base an unconfigured row shows. `None` for `vertex`, whose base
    /// comes from the engine's project and region.
    pub default_base_url: Option<&'static str>,
    resolver: Resolver,
}

/// Every builtin provider proxy, in the order the agent's context lists them.
/// The order is fixed so the rendered block is byte-stable across turns.
pub(crate) const BUILTIN_PROXIES: [BuiltinProxy; 7] = [
    BuiltinProxy {
        name: "anthropic",
        aliases: &["claude"],
        default_base_url: Some(ANTHROPIC_API_BASE_URL),
        resolver: Resolver::Anthropic,
    },
    BuiltinProxy {
        name: "local",
        aliases: &[],
        default_base_url: Some(prefs::LOCAL_BASE_URL.default_text()),
        resolver: Resolver::Local,
    },
    BuiltinProxy {
        name: "openai",
        aliases: &["gpt", "chatgpt"],
        default_base_url: Some(OPENAI_DEFAULT_BASE_URL),
        resolver: Resolver::OpenAi,
    },
    BuiltinProxy {
        name: "openrouter",
        aliases: &[],
        default_base_url: Some(OPENROUTER_BASE_URL),
        resolver: Resolver::OpenRouter,
    },
    BuiltinProxy {
        name: TYPESAFE_CREDENTIAL_SERVICE,
        aliases: &["jev"],
        default_base_url: Some(TYPESAFE_API_BASE_URL),
        resolver: Resolver::TypeSafe,
    },
    BuiltinProxy {
        name: "vertex",
        aliases: &["vertexai", "googlevertex"],
        default_base_url: None,
        resolver: Resolver::Vertex,
    },
    BuiltinProxy {
        name: "xai",
        aliases: &["grok"],
        default_base_url: Some(XAI_BASE_URL),
        resolver: Resolver::XAi,
    },
];

/// Model providers with no builtin proxy, each with the reason the agent's
/// context block shows. A test holds every `ProviderKind` to one of the two
/// lists, so a new provider cannot be forgotten.
///
/// `opencode-free` is out by ADR 0104: nothing may build on an anonymous
/// endpoint that can vanish without notice.
pub(crate) const PROVIDERS_WITHOUT_PROXY: [(&str, &str); 1] = [(
    "opencode-free",
    "the keyless free tier serves chat only (ADR 0104)",
)];

/// A service name with case and separators removed, so `Open-AI`, `open_ai`
/// and `openai` compare equal.
pub(crate) fn normalized_service_name(name: &str) -> String {
    name.chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

/// The builtin proxy a service name refers to, by name or alias.
pub(crate) fn builtin_proxy_for_service(service_name: &str) -> Option<&'static BuiltinProxy> {
    let wanted = normalized_service_name(service_name);
    if wanted.is_empty() {
        return None;
    }
    BUILTIN_PROXIES.iter().find(|proxy| {
        std::iter::once(proxy.name)
            .chain(proxy.aliases.iter().copied())
            .any(|spelling| normalized_service_name(spelling) == wanted)
    })
}

/// Resolve a builtin model-provider proxy target for `name`.
///
/// - `Ok(None)` — `name` is not a builtin provider (caller returns the generic
///   "not configured" 404, so a genuinely unknown name is unchanged).
/// - `Err((404, msg))` — `name` IS a builtin provider but its credential/config
///   is absent; the message names what to configure.
/// - `Ok(Some((base_url, layers)))` — resolved; forward through the layers.
///
/// Matches the exact proxy name only. Aliases serve `request_credential`, never
/// routing.
pub(crate) async fn resolve_builtin_provider(
    engine: &Arc<crate::engine::LucidosEngine>,
    name: &str,
) -> Result<Option<BuiltinTarget>, (StatusCode, String)> {
    match BUILTIN_PROXIES.iter().find(|proxy| proxy.name == name) {
        Some(proxy) => resolve_entry(engine, proxy).await.map(Some),
        None => Ok(None),
    }
}

async fn resolve_entry(
    engine: &Arc<crate::engine::LucidosEngine>,
    proxy: &BuiltinProxy,
) -> Result<BuiltinTarget, (StatusCode, String)> {
    match proxy.resolver {
        Resolver::Anthropic => resolve_anthropic(engine.pool()).await,
        Resolver::Local => resolve_local(engine.pool()).await,
        Resolver::OpenAi => resolve_openai(engine.pool()).await,
        Resolver::OpenRouter => resolve_openrouter(engine.pool()).await,
        Resolver::TypeSafe => resolve_typesafe(engine.pool()).await,
        Resolver::Vertex => resolve_vertex(engine).await,
        Resolver::XAi => resolve_xai(engine.pool()).await,
    }
}

/// Whether a builtin would serve a request right now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum BuiltinProxyState {
    /// It resolves, to this base URL. `injects_auth` is false for a keyless
    /// `local` server, which holds no key a caller could reuse.
    Configured {
        base_url: String,
        injects_auth: bool,
    },
    /// It answers the actionable 404.
    NotConfigured,
    /// The resolver itself failed, so nobody knows. Never read as a no.
    Unknown,
}

/// Ask the proxy's own resolver, so the answer cannot drift from what a call
/// would do. The resolved auth layers are counted, never read.
pub(crate) async fn builtin_proxy_state(
    engine: &Arc<crate::engine::LucidosEngine>,
    proxy: &BuiltinProxy,
) -> BuiltinProxyState {
    match resolve_entry(engine, proxy).await {
        Ok((base_url, layers)) => BuiltinProxyState::Configured {
            base_url,
            injects_auth: !layers.is_empty(),
        },
        Err((StatusCode::NOT_FOUND, _)) => BuiltinProxyState::NotConfigured,
        Err((status, msg)) => {
            crate::log!(
                "[Proxy] Cannot tell whether builtin '{}' is configured ({}): {}",
                proxy.name,
                status,
                msg
            );
            BuiltinProxyState::Unknown
        }
    }
}

/// Every builtin with its state, in catalog order. The checks are independent,
/// so they run concurrently: the chat turn waits for one, not seven.
pub(crate) async fn builtin_proxy_states(
    engine: &Arc<crate::engine::LucidosEngine>,
) -> Vec<(&'static BuiltinProxy, BuiltinProxyState)> {
    futures::future::join_all(
        BUILTIN_PROXIES
            .iter()
            .map(|proxy| async move { (proxy, builtin_proxy_state(engine, proxy).await) }),
    )
    .await
}

/// Whether a base URL already carries the `/v1` version segment, so a caller
/// must not add another.
pub(crate) fn base_includes_v1(base_url: &str) -> bool {
    reqwest::Url::parse(base_url)
        .ok()
        .and_then(|url| url.path_segments().map(|mut s| s.any(|seg| seg == "v1")))
        .unwrap_or(false)
}

/// The agent's context block listing every builtin proxy.
///
/// Deterministic for a given state: catalog order, no timestamps, and no
/// credential, since a state carries only a base URL.
pub(crate) fn render_builtin_proxies_block(
    states: &[(&'static BuiltinProxy, BuiltinProxyState)],
) -> String {
    let mut block = String::from(
        "[BUILTIN PROVIDER PROXIES - engine-owned, NOT in data/config/apis.json]\n\
         Call one with proxy_request(name, path) or `lucidos proxy <name> <path>`. The engine \
         injects the auth. The path is relative to the base, so never repeat a /v1 the base \
         already has: proxy_request(name: 'openai', path: '/models'), not '/v1/models'.\n",
    );
    for (proxy, state) in states {
        let base = match state {
            BuiltinProxyState::Configured { base_url, .. } => Some(base_url.as_str()),
            _ => proxy.default_base_url,
        };
        let base_part = match base {
            Some(url) if base_includes_v1(url) => format!("{url} (base includes /v1)"),
            Some(url) => format!("{url} (base has no /v1)"),
            None => "base set by the engine's Vertex project and region".to_string(),
        };
        let configured = match state {
            BuiltinProxyState::Configured { .. } => "yes",
            BuiltinProxyState::NotConfigured => "no",
            BuiltinProxyState::Unknown => "unknown (the check failed)",
        };
        block.push_str(&format!(
            "  - {}: {base_part}, configured: {configured}\n",
            proxy.name
        ));
    }
    for (name, reason) in PROVIDERS_WITHOUT_PROXY {
        block.push_str(&format!("  - {name}: no proxy, {reason}\n"));
    }
    block.push_str(
        "Never request_credential for a configured one. An unconfigured one is set up in \
         Settings > Models > Providers.\n[END BUILTIN PROVIDER PROXIES]",
    );
    block
}

/// Message for a recognized-but-unconfigured builtin provider. Names the
/// provider, what's missing, and how to fix it — including the `apis.json`
/// override escape hatch.
fn unconfigured_msg(name: &str, missing: &str, how: &str) -> (StatusCode, String) {
    (
        StatusCode::NOT_FOUND,
        format!(
            "proxy '{name}' is a builtin model provider but {missing} is not configured ({how}, or add a '{name}' entry to data/config/apis.json)"
        ),
    )
}

/// The binding for a builtin key whose upstream the engine chose, not a
/// caller. Every provider here but `local` pairs its key with a base URL from
/// this binary or from the engine's boot config.
fn pinned(what: &str, base_url: &str) -> ScopeBinding {
    ScopeBinding::Pinned {
        what: what.to_string(),
        base_url: base_url.to_string(),
    }
}

/// Fetch a stored credential as `(auth_type, auth_value)`. Missing → `None`; a
/// DB read error is a 500 (not a silent skip).
async fn credential_pair(
    pool: &sqlx::PgPool,
    name: &str,
) -> Result<Option<(AuthType, String)>, (StatusCode, String)> {
    match CredentialStore::get(pool, name).await {
        Ok(Some(c)) => Ok(Some((c.auth_type, c.auth_value))),
        Ok(None) => Ok(None),
        Err(e) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("failed to read '{name}' credential: {e}"),
        )),
    }
}

async fn resolve_openai(pool: &sqlx::PgPool) -> Result<BuiltinTarget, (StatusCode, String)> {
    let cred = credential_pair(pool, "openai").await?;
    // Same resolution order as the OpenAI LLM provider: credential → env → Codex.
    let key = resolve_openai_api_key(
        cred,
        std::env::var("OPENAI_API_KEY").ok(),
        crate::llm::openai::codex_detect::load(),
    )
    .map(|(k, _)| k);
    let Some(key) = key else {
        return Err(unconfigured_msg(
            "openai",
            "an OpenAI API key",
            "add an 'openai' credential in Settings → Models → Providers, set OPENAI_API_KEY",
        ));
    };
    let layer = StaticHeaderLayer::bearer(
        "openai".to_string(),
        key,
        pinned("openai", OPENAI_DEFAULT_BASE_URL),
    );
    Ok((OPENAI_DEFAULT_BASE_URL.to_string(), vec![Arc::new(layer)]))
}

async fn resolve_openrouter(pool: &sqlx::PgPool) -> Result<BuiltinTarget, (StatusCode, String)> {
    let cred = credential_pair(pool, "openrouter").await?;
    let key = resolve_bearer_key(cred, std::env::var("LUCIDOS_OPENROUTER_API_KEY").ok());
    let Some(key) = key else {
        return Err(unconfigured_msg(
            "openrouter",
            "an OpenRouter API key",
            "add an 'openrouter' credential in Settings → Models → Providers, set LUCIDOS_OPENROUTER_API_KEY",
        ));
    };
    let layer = StaticHeaderLayer::bearer(
        "openrouter".to_string(),
        key,
        pinned("openrouter", OPENROUTER_BASE_URL),
    );
    Ok((OPENROUTER_BASE_URL.to_string(), vec![Arc::new(layer)]))
}

async fn resolve_xai(pool: &sqlx::PgPool) -> Result<BuiltinTarget, (StatusCode, String)> {
    let cred = credential_pair(pool, "xai").await?;
    let key = resolve_bearer_key(cred, std::env::var("LUCIDOS_XAI_API_KEY").ok());
    let Some(key) = key else {
        return Err(unconfigured_msg(
            "xai",
            "an xAI API key",
            "add an 'xai' credential in Settings → Models → Providers, set LUCIDOS_XAI_API_KEY",
        ));
    };
    let layer = StaticHeaderLayer::bearer("xai".to_string(), key, pinned("xai", XAI_BASE_URL));
    Ok((XAI_BASE_URL.to_string(), vec![Arc::new(layer)]))
}

/// TypeSafe's System One endpoint, for an app that wants a typed judgment.
///
/// Reads the same credential and the same env var as
/// [`crate::llm::judgment::select`], through the same helper. So the proxy and
/// the engine's own judgment calls cannot disagree about which key is in
/// effect. Both constants come from that module rather than being spelled
/// again here.
async fn resolve_typesafe(pool: &sqlx::PgPool) -> Result<BuiltinTarget, (StatusCode, String)> {
    let cred = credential_pair(pool, TYPESAFE_CREDENTIAL_SERVICE).await?;
    let key = resolve_bearer_key(cred, std::env::var(TYPESAFE_API_KEY_ENV).ok());
    let Some(key) = key else {
        return Err(unconfigured_msg(
            TYPESAFE_CREDENTIAL_SERVICE,
            "a TypeSafe API key",
            "add a 'typesafe' credential in Settings → Models → Providers, set TYPESAFE_API_KEY",
        ));
    };
    let layer = StaticHeaderLayer::bearer(
        TYPESAFE_CREDENTIAL_SERVICE.to_string(),
        key,
        pinned(TYPESAFE_CREDENTIAL_SERVICE, TYPESAFE_API_BASE_URL),
    );
    Ok((TYPESAFE_API_BASE_URL.to_string(), vec![Arc::new(layer)]))
}

async fn resolve_anthropic(pool: &sqlx::PgPool) -> Result<BuiltinTarget, (StatusCode, String)> {
    let cred = credential_pair(pool, "anthropic").await?;
    // Same resolution order as the Anthropic LLM provider: credential → env.
    // A credential whose auth_type carries no usable Anthropic auth is logged
    // and skipped inside the resolver, so it falls through to the env var here
    // exactly as it does at provider-build time.
    let auth = resolve_anthropic_auth(cred, std::env::var("ANTHROPIC_API_KEY").ok());
    anthropic_target(auth.map(|(auth, _source)| auth))
}

/// Shape the `anthropic` builtin target from already-resolved auth. Split from
/// the credential/env read above so the per-auth-kind header shaping is
/// testable without mutating process env.
fn anthropic_target(auth: Option<AnthropicAuth>) -> Result<BuiltinTarget, (StatusCode, String)> {
    let Some(auth) = auth else {
        return Err(unconfigured_msg(
            "anthropic",
            "an Anthropic API key or OAuth token",
            "add an 'anthropic' credential in Settings → Models → Providers, set ANTHROPIC_API_KEY",
        ));
    };
    // API keys go on `x-api-key`; OAuth subscription tokens on
    // `Authorization: Bearer`. Mirrors `anthropic::chat::auth_header`.
    let layers: Vec<Arc<dyn AuthLayer>> = match auth {
        AnthropicAuth::ApiKey(key) => vec![Arc::new(
            StaticHeaderLayer::api_key(
                "anthropic".to_string(),
                "x-api-key",
                key,
                pinned("anthropic", ANTHROPIC_API_BASE_URL),
            )
            .map_err(|e| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("failed to build anthropic auth header: {e}"),
                )
            })?,
        )],
        // An OAuth subscription token ALSO requires the `anthropic-beta` OAuth
        // companion header — the direct provider adds it via
        // `anthropic_beta_header`. It's part of what makes OAuth auth work, and
        // the app can't add it (it doesn't know the credential is OAuth), so the
        // engine injects it here too.
        AnthropicAuth::OAuthBearer(token) => vec![
            Arc::new(StaticHeaderLayer::bearer(
                "anthropic-auth".to_string(),
                token,
                pinned("anthropic", ANTHROPIC_API_BASE_URL),
            )),
            Arc::new(
                StaticHeaderLayer::api_key(
                    "anthropic-oauth-beta".to_string(),
                    "anthropic-beta",
                    crate::llm::anthropic::ANTHROPIC_OAUTH_BETA.to_string(),
                    // A protocol constant, not a secret. Pinned all the same,
                    // so no layer is exempt from declaring where it may go.
                    pinned("anthropic", ANTHROPIC_API_BASE_URL),
                )
                .map_err(|e| {
                    (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        format!("failed to build anthropic beta header: {e}"),
                    )
                })?,
            ),
        ],
    };
    Ok((ANTHROPIC_API_BASE_URL.to_string(), layers))
}

/// Where the `local` key came from, and therefore what binds it.
///
/// `local` is the one builtin whose upstream a preference names, and
/// `prefs::LOCAL_BASE_URL` moves without the key being re-saved. So the key's own
/// source has to name the host, never the preference (ADR 0144 decision 4).
enum LocalKeySource {
    /// A stored `local` credential. Its `base_url` is the scope, and Settings
    /// can correct it.
    Credential,
    /// `LUCIDOS_LOCAL_API_KEY`. Nothing in the database scopes it, so process
    /// env has to name the host too.
    Environment(String),
}

/// The upstream the builtin `local` provider resolves to: the preference,
/// then `LUCIDOS_LOCAL_BASE_URL`, then the built-in default.
///
/// `None` when the preference read failed, so a caller cannot mistake a broken
/// database for a configured host. `None` too when the preference names a host
/// off the user's own network, which the provider refuses. The boot pass reads
/// this to give an unscoped `local` credential the scope it needs (ADR 0144).
pub async fn local_upstream_base_url(pool: &sqlx::PgPool) -> Option<String> {
    let base_pref = prefs::LOCAL_BASE_URL
        .try_stored(pool)
        .await
        .ok()?
        .filter(|s| !s.trim().is_empty());
    if let Some(reason) = base_pref.as_deref().and_then(local_base_url_rejection) {
        crate::log!("[Proxy] Not scoping the 'local' key: {}", reason);
        return None;
    }
    Some(
        base_pref
            .or_else(|| {
                std::env::var("LUCIDOS_LOCAL_BASE_URL")
                    .ok()
                    .filter(|s| !s.trim().is_empty())
            })
            .unwrap_or_else(|| prefs::LOCAL_BASE_URL.default_text().to_string()),
    )
}

async fn resolve_local(pool: &sqlx::PgPool) -> Result<BuiltinTarget, (StatusCode, String)> {
    // Reads the preference and the env var itself rather than calling
    // `local_upstream_base_url`: the two halves are needed apart, for the
    // unconfigured check below and for the env key's own pairing. Keep the
    // pref-then-env-then-default order in step with that function.
    //
    // Opt-in, mirroring `build_local_provider`: only resolve when a base URL
    // (pref or env) or key is configured — otherwise a default localhost
    // backend isn't conjured for a workspace that never asked for one.
    let base_pref = match prefs::LOCAL_BASE_URL.try_stored(pool).await {
        Ok(opt) => opt.filter(|s| !s.trim().is_empty()),
        Err(e) => {
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                format!(
                    "failed to read {} preference: {e}",
                    prefs::LOCAL_BASE_URL.key()
                ),
            ));
        }
    };
    // A keyless call attaches no layer, so the proxy's scope gate never sees
    // it. This check is what keeps the route from forwarding anywhere.
    if let Some(reason) = base_pref.as_deref().and_then(local_base_url_rejection) {
        return Err((StatusCode::BAD_GATEWAY, format!("proxy 'local': {reason}")));
    }
    let base_env = std::env::var("LUCIDOS_LOCAL_BASE_URL")
        .ok()
        .filter(|s| !s.trim().is_empty());
    let key = match credential_pair(pool, "local")
        .await?
        .map(|(_, v)| v)
        .filter(|s| !s.trim().is_empty())
    {
        Some(v) => Some((v, LocalKeySource::Credential)),
        None => std::env::var("LUCIDOS_LOCAL_API_KEY")
            .ok()
            .filter(|s| !s.trim().is_empty())
            .map(|v| {
                // The env pairing, decided here rather than at the base URL
                // below: an env key follows the env base URL, or the default.
                let host = base_env
                    .clone()
                    .unwrap_or_else(|| prefs::LOCAL_BASE_URL.default_text().to_string());
                (v, LocalKeySource::Environment(host))
            }),
    };

    if base_pref.is_none() && base_env.is_none() && key.is_none() {
        return Err(unconfigured_msg(
            "local",
            "a local OpenAI-compatible backend",
            &format!(
                "set {} in Settings → Models → Providers or LUCIDOS_LOCAL_BASE_URL",
                prefs::LOCAL_BASE_URL.key()
            ),
        ));
    }

    let base = base_pref
        .or(base_env)
        .unwrap_or_else(|| prefs::LOCAL_BASE_URL.default_text().to_string());
    // A keyless local server (Ollama / llama.cpp) gets no auth layer.
    let layers: Vec<Arc<dyn AuthLayer>> = match key {
        Some((k, source)) => {
            let binding = match source {
                LocalKeySource::Credential => ScopeBinding::StoredCredential("local".to_string()),
                LocalKeySource::Environment(host) => pinned("LUCIDOS_LOCAL_API_KEY", &host),
            };
            vec![Arc::new(StaticHeaderLayer::bearer(
                "local".to_string(),
                k,
                binding,
            ))]
        }
        None => Vec::new(),
    };
    Ok((base, layers))
}

async fn resolve_vertex(
    engine: &Arc<crate::engine::LucidosEngine>,
) -> Result<BuiltinTarget, (StatusCode, String)> {
    let project = engine.vertex_project_id().trim().to_string();
    if project.is_empty() {
        return Err(unconfigured_msg(
            "vertex",
            "a Google Cloud project",
            "set VERTEX_PROJECT_ID or run `gcloud auth application-default login`",
        ));
    }
    let region = vertex::read_location(engine.vertex_location());
    let base_url = vertex_base_url(&project, &region);
    // Reuse the engine's warm token cache when Vertex is the active LLM provider
    // (project non-empty ⇒ a cache was built at boot); fall back to a shared
    // process-wide cache defensively so proxied requests still share tokens.
    let cache = engine
        .vertex_token_cache()
        .unwrap_or_else(|| PROXY_VERTEX_TOKEN_CACHE.clone());
    Ok((
        base_url.clone(),
        vec![Arc::new(VertexAdcLayer::new(cache, base_url))],
    ))
}

/// Engine-owned Vertex AI URL prefix. The app supplies only the
/// `/publishers/<publisher>/models/<model>:<method>` suffix; the engine fills
/// project + region so neither ever has to live in the app or the workspace's
/// `apis.json`.
pub(crate) fn vertex_base_url(project: &str, region: &str) -> String {
    let host = vertex::vertex_host(region);
    format!("https://{host}/v1/projects/{project}/locations/{region}")
}

/// Fallback Vertex token cache for the defensive case where the engine has a
/// configured project but no boot-built cache. Process-wide so proxied requests
/// still share warm access tokens.
static PROXY_VERTEX_TOKEN_CACHE: LazyLock<TokenCache> =
    LazyLock::new(|| Arc::new(std::sync::Mutex::new(None)));

/// Auth layer that mints/refreshes a Vertex AI OAuth access token server-side
/// and attaches it as `Authorization: Bearer <token>`. Opts into the 401
/// invalidate-and-retry so an expired cached token is cleared and re-minted
/// once — mirroring `VertexProvider`'s own 401 handling.
struct VertexAdcLayer {
    token_cache: TokenCache,
    /// The Vertex URL prefix this token was minted for. `vertex_base_url`
    /// derives it from the engine's project and region, and `vertex_host`
    /// refuses a region that could name another host.
    base_url: String,
}

impl VertexAdcLayer {
    fn new(token_cache: TokenCache, base_url: String) -> Self {
        Self {
            token_cache,
            base_url,
        }
    }
}

#[async_trait]
impl AuthLayer for VertexAdcLayer {
    fn output_namespace(&self) -> &str {
        "vertex"
    }

    fn scope_bindings(&self) -> Vec<ScopeBinding> {
        vec![pinned("the Vertex access token", &self.base_url)]
    }

    fn retry_on_401(&self) -> RetryHint {
        RetryHint::InvalidateAndRetry
    }

    async fn invalidate_cache(&self) {
        if let Ok(mut guard) = self.token_cache.lock() {
            *guard = None;
        }
    }

    async fn apply(&self, _input: &LayerInput<'_>) -> Result<AuthMutation, (StatusCode, String)> {
        let token = vertex::get_cached_access_token(&self.token_cache)
            .await
            .map_err(|e| {
                (
                    StatusCode::BAD_GATEWAY,
                    format!("failed to mint Vertex access token: {e}"),
                )
            })?;
        Ok(AuthMutation {
            add_headers: vec![(
                HeaderName::from_static("authorization"),
                format!("Bearer {token}"),
            )],
            // We can't distinguish a warm-cache hit from a fresh mint through
            // `get_cached_access_token`, so opt every apply into the retry path:
            // a 401 always invalidates + re-mints once. The single wasted retry
            // when a freshly-minted token 401s is bounded to one extra request
            // (the same one-shot `VertexProvider` does).
            cache_was_hit: true,
            outputs: serde_json::json!({}),
            ..Default::default()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::proxy_auth_layer::BodyView;
    use axum::http::Method;
    use bytes::Bytes;
    use std::collections::HashMap;
    use std::time::Instant;

    fn dummy_input<'a>(
        body: &'a Bytes,
        prior: &'a HashMap<String, serde_json::Value>,
    ) -> LayerInput<'a> {
        LayerInput {
            method: &Method::POST,
            url: "https://example.com/x",
            headers: &[],
            body: BodyView::Raw(body),
            prior_layer_outputs: prior,
        }
    }

    #[test]
    fn vertex_base_url_uses_engine_owned_prefix_per_region() {
        // Regional → {region}-aiplatform host + locations/{region}.
        assert_eq!(
            vertex_base_url("my-project", "europe-west1"),
            "https://europe-west1-aiplatform.googleapis.com/v1/projects/my-project/locations/europe-west1"
        );
        // global → default host.
        assert_eq!(
            vertex_base_url("my-project", "global"),
            "https://aiplatform.googleapis.com/v1/projects/my-project/locations/global"
        );
        // multi-region → dedicated .rep host (the 404-avoidance case).
        assert_eq!(
            vertex_base_url("p", "eu"),
            "https://aiplatform.eu.rep.googleapis.com/v1/projects/p/locations/eu"
        );
    }

    /// A base URL + an app suffix compose into the full Vertex predict URL —
    /// the app never has to know project/region.
    #[test]
    fn vertex_base_url_composes_with_app_supplied_suffix() {
        let base = vertex_base_url("proj", "europe-west1");
        let full = crate::api::proxy::build_target_url(
            &base,
            "/publishers/anthropic/models/claude-opus-4-8@default:rawPredict",
            None,
        );
        assert_eq!(
            full,
            "https://europe-west1-aiplatform.googleapis.com/v1/projects/proj/locations/europe-west1/publishers/anthropic/models/claude-opus-4-8@default:rawPredict"
        );
    }

    /// The Vertex layer attaches `Authorization: Bearer <token>` from the token
    /// cache. Seeding the cache with a fresh token exercises the header shaping
    /// without minting a real ADC token.
    #[tokio::test]
    async fn vertex_layer_attaches_bearer_from_cached_token() {
        let cache: TokenCache = Arc::new(std::sync::Mutex::new(Some((
            "tok-123".to_string(),
            Instant::now(),
        ))));
        let layer = VertexAdcLayer::new(cache.clone(), "https://x.googleapis.com".to_string());
        let body = Bytes::new();
        let prior = HashMap::new();
        let m = layer.apply(&dummy_input(&body, &prior)).await.unwrap();
        assert_eq!(m.add_headers.len(), 1);
        assert_eq!(m.add_headers[0].0.as_str(), "authorization");
        assert_eq!(m.add_headers[0].1, "Bearer tok-123");
        assert!(m.cache_was_hit, "layer opts into the 401 retry path");
        assert_eq!(layer.retry_on_401(), RetryHint::InvalidateAndRetry);

        // invalidate_cache clears the cached token so the next apply re-mints.
        layer.invalidate_cache().await;
        assert!(
            cache.lock().unwrap().is_none(),
            "cache cleared on invalidate"
        );
    }

    // ---- The catalog and the agent's context block --------------------------

    /// Every model provider is a builtin proxy or names its reason for not
    /// being one. A provider added to the enum fails here until it is either.
    #[test]
    fn every_model_provider_is_a_builtin_proxy_or_says_why_not() {
        for kind in crate::llm::ProviderKind::ALL {
            let name = kind.as_str();
            let proxied = BUILTIN_PROXIES.iter().any(|p| p.name == name);
            let excluded = PROVIDERS_WITHOUT_PROXY.iter().any(|(n, _)| *n == name);
            assert!(
                proxied != excluded,
                "provider '{name}' must be on exactly one list (proxied {proxied}, excluded {excluded})"
            );
        }
        for (name, reason) in PROVIDERS_WITHOUT_PROXY {
            assert!(
                crate::llm::ProviderKind::from_name(name).is_some(),
                "'{name}' is not a provider, so its exclusion is stale"
            );
            assert!(!reason.trim().is_empty(), "'{name}' needs a reason");
        }
    }

    /// No two rows share a spelling, or `request_credential` would map one
    /// service name to two proxies.
    #[test]
    fn builtin_names_and_aliases_are_unique() {
        let mut seen = std::collections::HashSet::new();
        for proxy in &BUILTIN_PROXIES {
            for spelling in std::iter::once(proxy.name).chain(proxy.aliases.iter().copied()) {
                assert!(
                    seen.insert(normalized_service_name(spelling)),
                    "'{spelling}' appears twice in the catalog"
                );
            }
        }
    }

    #[test]
    fn a_service_name_maps_to_its_builtin_by_name_or_alias() {
        let name = |s: &str| builtin_proxy_for_service(s).map(|p| p.name);
        for spelling in [
            "openai", "OpenAI", "open-ai", "open_ai", "Open AI", "gpt", "ChatGPT",
        ] {
            assert_eq!(name(spelling), Some("openai"), "{spelling}");
        }
        assert_eq!(name("Claude"), Some("anthropic"));
        assert_eq!(name("x-ai"), Some("xai"));
        assert_eq!(name("grok"), Some("xai"));
        assert_eq!(name("open-router"), Some("openrouter"));
        assert_eq!(name("vertex-ai"), Some("vertex"));
        assert_eq!(name("jev"), Some(TYPESAFE_CREDENTIAL_SERVICE));
        for unrelated in ["oura", "github", "", "--", "openai-realtime-relay"] {
            assert_eq!(name(unrelated), None, "{unrelated}");
        }
    }

    #[test]
    fn base_includes_v1_reads_the_path_segments() {
        assert!(base_includes_v1(OPENAI_DEFAULT_BASE_URL));
        assert!(base_includes_v1(OPENROUTER_BASE_URL));
        assert!(base_includes_v1(&vertex_base_url("p", "europe-west1")));
        assert!(!base_includes_v1("https://api.example.com"));
        assert!(!base_includes_v1("https://api.example.com/v10"));
        assert!(!base_includes_v1("not a url"));
    }

    /// Every default base the catalog ships already ends in the version
    /// segment. That is the quirk the block warns about.
    #[test]
    fn every_shipped_default_base_includes_v1() {
        for proxy in &BUILTIN_PROXIES {
            if let Some(base) = proxy.default_base_url {
                assert!(base_includes_v1(base), "{}: {base}", proxy.name);
            }
        }
    }

    fn states_with(configured: &[(&str, &str)]) -> Vec<(&'static BuiltinProxy, BuiltinProxyState)> {
        BUILTIN_PROXIES
            .iter()
            .map(|proxy| {
                let state = configured
                    .iter()
                    .find(|(name, _)| *name == proxy.name)
                    .map(|(_, base)| BuiltinProxyState::Configured {
                        base_url: base.to_string(),
                        injects_auth: true,
                    })
                    .unwrap_or(BuiltinProxyState::NotConfigured);
                (proxy, state)
            })
            .collect()
    }

    #[test]
    fn the_block_lists_every_builtin_in_catalog_order() {
        let block =
            render_builtin_proxies_block(&states_with(&[("openai", OPENAI_DEFAULT_BASE_URL)]));
        assert!(block.starts_with("[BUILTIN PROVIDER PROXIES"), "{block}");
        assert!(block.contains("NOT in data/config/apis.json"), "{block}");
        assert!(block.ends_with("[END BUILTIN PROVIDER PROXIES]"), "{block}");
        let mut last = 0;
        for proxy in &BUILTIN_PROXIES {
            let at = block
                .find(&format!("  - {}: ", proxy.name))
                .unwrap_or_else(|| panic!("{} missing: {block}", proxy.name));
            assert!(at > last, "{} is out of order: {block}", proxy.name);
            last = at;
        }
        assert!(
            block.contains(
                "  - openai: https://api.openai.com/v1 (base includes /v1), configured: yes\n"
            ),
            "{block}"
        );
        assert!(
            block.contains("  - xai: https://api.x.ai/v1 (base includes /v1), configured: no\n"),
            "{block}"
        );
        assert!(
            block.contains(
                "  - vertex: base set by the engine's Vertex project and region, configured: no\n"
            ),
            "{block}"
        );
        assert!(
            block.contains("path: '/models'"),
            "the worked example: {block}"
        );
        assert!(
            block.contains("  - opencode-free: no proxy, "),
            "an excluded provider says so rather than vanishing: {block}"
        );
    }

    /// Same state, same bytes, so the block never churns the prompt.
    #[test]
    fn the_block_is_deterministic() {
        let states = states_with(&[("anthropic", ANTHROPIC_API_BASE_URL)]);
        assert_eq!(
            render_builtin_proxies_block(&states),
            render_builtin_proxies_block(&states)
        );
    }

    #[test]
    fn a_configured_base_wins_over_the_default_and_an_unknown_says_so() {
        let mut states = states_with(&[
            ("local", "http://localhost:1234/v1"),
            ("vertex", &vertex_base_url("proj", "europe-west1")),
        ]);
        states[0].1 = BuiltinProxyState::Unknown;
        let block = render_builtin_proxies_block(&states);
        assert!(
            block.contains(
                "  - local: http://localhost:1234/v1 (base includes /v1), configured: yes\n"
            ),
            "{block}"
        );
        assert!(
            block.contains(
                "/projects/proj/locations/europe-west1 (base includes /v1), configured: yes"
            ),
            "{block}"
        );
        assert!(
            block.contains("  - anthropic: https://api.anthropic.com/v1 (base includes /v1), configured: unknown"),
            "an unknown is never rendered as a no: {block}"
        );
    }

    // ---- DB-backed resolver tests (need Postgres via test-engine.sh) --------

    use crate::core::AuthType;
    use crate::test_support::{seed_credential, setup_test_db, teardown_test_db};

    /// Run a resolved target's layers over a dummy request and collect the
    /// injected header pairs (name lowercased by `HeaderName`, value).
    async fn injected_headers(target: &BuiltinTarget) -> Vec<(String, String)> {
        let body = Bytes::new();
        let prior = HashMap::new();
        let input = dummy_input(&body, &prior);
        let mut out = Vec::new();
        for layer in &target.1 {
            let m = layer.apply(&input).await.unwrap();
            for (n, v) in m.add_headers {
                out.push((n.as_str().to_string(), v));
            }
        }
        out
    }

    /// A seeded `openai` credential resolves to the OpenAI API root with a
    /// `Bearer` header — the credential wins over any ambient env/Codex key, so
    /// this is deterministic in CI.
    #[tokio::test]
    async fn resolve_openai_injects_bearer_from_credential() {
        let (pool, db) = setup_test_db().await;
        seed_credential(
            &pool,
            "openai",
            OPENAI_DEFAULT_BASE_URL,
            AuthType::ApiKey,
            "sk-test-openai",
        )
        .await;

        let target = resolve_openai(&pool).await.expect("openai resolves");
        assert_eq!(target.0, OPENAI_DEFAULT_BASE_URL);
        assert_eq!(
            injected_headers(&target).await,
            vec![(
                "authorization".to_string(),
                "Bearer sk-test-openai".to_string()
            )]
        );
        teardown_test_db(&db).await;
    }

    /// A seeded `openrouter` credential resolves to the OpenRouter API root with
    /// a `Bearer` header.
    #[tokio::test]
    async fn resolve_openrouter_injects_bearer_from_credential() {
        let (pool, db) = setup_test_db().await;
        seed_credential(
            &pool,
            "openrouter",
            OPENROUTER_BASE_URL,
            AuthType::Bearer,
            "sk-or-test",
        )
        .await;

        let target = resolve_openrouter(&pool)
            .await
            .expect("openrouter resolves");
        assert_eq!(target.0, OPENROUTER_BASE_URL);
        assert_eq!(
            injected_headers(&target).await,
            vec![("authorization".to_string(), "Bearer sk-or-test".to_string())]
        );
        teardown_test_db(&db).await;
    }

    /// A seeded `xai` credential resolves to xAI's API root with a `Bearer`
    /// header. So an app calls Grok through the proxy, and the workspace never
    /// re-enters the key in `apis.json`.
    #[tokio::test]
    async fn resolve_xai_injects_bearer_from_credential() {
        let (pool, db) = setup_test_db().await;
        seed_credential(&pool, "xai", XAI_BASE_URL, AuthType::ApiKey, "xai-test").await;

        let target = resolve_xai(&pool).await.expect("xai resolves");
        assert_eq!(target.0, XAI_BASE_URL);
        assert_eq!(
            injected_headers(&target).await,
            vec![("authorization".to_string(), "Bearer xai-test".to_string())]
        );
        teardown_test_db(&db).await;
    }

    /// A seeded `typesafe` credential resolves to the System One root with a
    /// `Bearer` header. So an app asks Jev a typed question through the proxy,
    /// and never holds the key itself.
    #[tokio::test]
    async fn resolve_typesafe_injects_bearer_from_credential() {
        let (pool, db) = setup_test_db().await;
        seed_credential(
            &pool,
            TYPESAFE_CREDENTIAL_SERVICE,
            TYPESAFE_API_BASE_URL,
            AuthType::ApiKey,
            "ts-test",
        )
        .await;

        let target = resolve_typesafe(&pool).await.expect("typesafe resolves");
        assert_eq!(target.0, TYPESAFE_API_BASE_URL);
        assert_eq!(
            injected_headers(&target).await,
            vec![("authorization".to_string(), "Bearer ts-test".to_string())]
        );
        // The gate every credential-bearing arm passes (ADR 0144 decision 4).
        // The upstream is a constant in this binary, so it pins to that
        // constant and no API caller can point the key somewhere else.
        assert_eq!(
            target.1[0].scope_bindings(),
            vec![pinned(TYPESAFE_CREDENTIAL_SERVICE, TYPESAFE_API_BASE_URL)]
        );
        teardown_test_db(&db).await;
    }

    /// The credential wins over the env var, which is the order
    /// `judgment::select::api_key` reads them in. A proxy finding a key the
    /// judgment path does not, or the reverse, is the drift this pins.
    #[tokio::test]
    async fn the_typesafe_proxy_prefers_the_credential_over_the_env_var() {
        let (pool, db) = setup_test_db().await;
        seed_credential(
            &pool,
            TYPESAFE_CREDENTIAL_SERVICE,
            TYPESAFE_API_BASE_URL,
            AuthType::ApiKey,
            "ts-stored",
        )
        .await;

        let target = resolve_typesafe(&pool).await.expect("typesafe resolves");
        assert_eq!(
            injected_headers(&target).await,
            vec![("authorization".to_string(), "Bearer ts-stored".to_string())]
        );
        teardown_test_db(&db).await;
    }

    /// `typesafe` is a recognized builtin, so an unconfigured one is a 404
    /// naming what to set rather than the generic unknown-proxy message.
    /// Skipped when the launch environment supplies a key, since the case
    /// cannot arise there.
    #[tokio::test]
    async fn an_unconfigured_typesafe_proxy_names_what_to_set() {
        if std::env::var(TYPESAFE_API_KEY_ENV).is_ok() {
            return;
        }
        let (pool, db) = setup_test_db().await;

        let Err((status, message)) = resolve_typesafe(&pool).await else {
            panic!("no credential and no env var must not resolve");
        };
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert!(message.contains("typesafe"), "{message}");
        assert!(message.contains("TYPESAFE_API_KEY"), "{message}");
        assert!(message.contains("apis.json"), "{message}");
        teardown_test_db(&db).await;
    }

    /// An `anthropic` API-key credential is injected on `x-api-key` (not
    /// `Authorization`), mirroring the Anthropic LLM path. The credential wins
    /// over any ambient `ANTHROPIC_API_KEY`, so this is deterministic in CI.
    #[tokio::test]
    async fn resolve_anthropic_api_key_injects_x_api_key() {
        let (pool, db) = setup_test_db().await;
        seed_credential(
            &pool,
            "anthropic",
            ANTHROPIC_API_BASE_URL,
            AuthType::ApiKey,
            "sk-ant-test",
        )
        .await;

        let target = resolve_anthropic(&pool).await.expect("anthropic resolves");
        assert_eq!(target.0, ANTHROPIC_API_BASE_URL);
        assert_eq!(
            injected_headers(&target).await,
            vec![("x-api-key".to_string(), "sk-ant-test".to_string())]
        );
        teardown_test_db(&db).await;
    }

    /// An `anthropic` OAuth (Bearer) credential injects both `Authorization:
    /// Bearer` AND the required `anthropic-beta` OAuth companion header — the
    /// app can't add the latter, so the engine must.
    #[tokio::test]
    async fn resolve_anthropic_oauth_injects_bearer_and_beta_header() {
        let (pool, db) = setup_test_db().await;
        seed_credential(
            &pool,
            "anthropic",
            ANTHROPIC_API_BASE_URL,
            AuthType::Bearer,
            "oauth-token-xyz",
        )
        .await;

        let target = resolve_anthropic(&pool).await.expect("anthropic resolves");
        let headers = injected_headers(&target).await;
        assert!(
            headers.contains(&(
                "authorization".to_string(),
                "Bearer oauth-token-xyz".to_string()
            )),
            "must inject the OAuth bearer token: {headers:?}"
        );
        assert!(
            headers.contains(&(
                "anthropic-beta".to_string(),
                crate::llm::anthropic::ANTHROPIC_OAUTH_BETA.to_string()
            )),
            "must inject the OAuth beta companion header: {headers:?}"
        );
        teardown_test_db(&db).await;
    }

    /// With no stored credential, an `ANTHROPIC_API_KEY` in the environment
    /// resolves the builtin proxy and is injected on `x-api-key` (an exported
    /// key is a pay-per-token API key, never a subscription token). Driven
    /// through the resolver + target shaping rather than by mutating process
    /// env, which would race every other test in the binary.
    #[tokio::test]
    async fn anthropic_env_key_resolves_and_injects_x_api_key() {
        let auth = resolve_anthropic_auth(None, Some("sk-ant-env".to_string()))
            .map(|(auth, _source)| auth);
        let target = anthropic_target(auth).expect("the env key resolves the builtin");
        assert_eq!(target.0, ANTHROPIC_API_BASE_URL);
        assert_eq!(
            injected_headers(&target).await,
            vec![("x-api-key".to_string(), "sk-ant-env".to_string())]
        );
    }

    /// A recognized-but-unconfigured builtin returns an actionable 404 that
    /// names the provider and the `apis.json` escape hatch. Skipped when
    /// `ANTHROPIC_API_KEY` is exported: the proxy honors that fallback, so
    /// resolving is then correct and "no credential → 404" is not.
    #[tokio::test]
    async fn resolve_anthropic_unconfigured_is_actionable_404() {
        if std::env::var("ANTHROPIC_API_KEY").is_ok() {
            return;
        }
        let (pool, db) = setup_test_db().await;
        let err = match resolve_anthropic(&pool).await {
            Ok(_) => panic!("no anthropic credential must be unconfigured"),
            Err(e) => e,
        };
        assert_eq!(err.0, StatusCode::NOT_FOUND);
        assert!(err.1.contains("builtin model provider"), "msg: {}", err.1);
        assert!(err.1.contains("apis.json"), "msg: {}", err.1);
        teardown_test_db(&db).await;
    }

    /// A `prefs::LOCAL_BASE_URL` preference makes the `local` builtin resolve to that
    /// base; with no key it injects no auth header (keyless local server).
    #[tokio::test]
    async fn resolve_local_uses_pref_base_and_is_keyless() {
        let (pool, db) = setup_test_db().await;
        crate::test_support::seed_preference(
            &pool,
            prefs::LOCAL_BASE_URL.key(),
            "http://localhost:1234/v1",
        )
        .await
        .expect("seed the local base URL pref");

        let target = resolve_local(&pool).await.expect("local resolves");
        assert_eq!(target.0, "http://localhost:1234/v1");
        assert!(
            injected_headers(&target).await.is_empty(),
            "keyless local server must get no auth header"
        );
        assert!(
            target.1.is_empty(),
            "no layer at all, which is what reports `injects_auth: false` to request_credential"
        );
        teardown_test_db(&db).await;
    }

    /// A keyless call attaches no layer, so the scope gate never runs for it.
    /// A public host stored before the write path checked it must therefore
    /// be refused here, or the route forwards anywhere and hands back the body.
    /// The boot pass must not scope the `local` key to that host either.
    #[tokio::test]
    async fn a_stored_public_local_base_url_is_not_forwarded_or_scoped() {
        let (pool, db) = setup_test_db().await;
        crate::test_support::seed_preference(
            &pool,
            prefs::LOCAL_BASE_URL.key(),
            "https://attacker.example/v1",
        )
        .await
        .expect("seed the local base URL pref");

        let err = match resolve_local(&pool).await {
            Err(e) => e,
            Ok(target) => panic!("forwarded to {}", target.0),
        };
        assert_eq!(err.0, StatusCode::BAD_GATEWAY);
        assert!(err.1.contains("attacker.example"), "msg: {}", err.1);
        assert_eq!(local_upstream_base_url(&pool).await, None);
        teardown_test_db(&db).await;
    }

    /// A `local` credential adds a `Bearer` header on top of the pref base.
    #[tokio::test]
    async fn resolve_local_with_key_injects_bearer() {
        let (pool, db) = setup_test_db().await;
        crate::test_support::seed_preference(
            &pool,
            prefs::LOCAL_BASE_URL.key(),
            "http://localhost:1234/v1",
        )
        .await
        .expect("seed the local base URL pref");
        seed_credential(
            &pool,
            "local",
            "http://localhost:1234/v1",
            AuthType::Bearer,
            "local-key",
        )
        .await;

        let target = resolve_local(&pool).await.expect("local resolves");
        assert_eq!(
            injected_headers(&target).await,
            vec![("authorization".to_string(), "Bearer local-key".to_string())]
        );
        teardown_test_db(&db).await;
    }

    /// FINDING 2, at the source. A stored `local` credential is bound by its
    /// own `base_url`, so Settings can correct it and a rewritten preference
    /// cannot speak for it. Every other builtin pins to a base URL no API
    /// caller can write.
    #[tokio::test]
    async fn every_builtin_layer_declares_what_binds_it() {
        let (pool, db) = setup_test_db().await;
        crate::test_support::seed_preference(
            &pool,
            prefs::LOCAL_BASE_URL.key(),
            "http://localhost:1234/v1",
        )
        .await
        .expect("seed the local base URL pref");
        seed_credential(
            &pool,
            "local",
            "http://localhost:1234/v1",
            AuthType::Bearer,
            "local-key",
        )
        .await;
        let target = resolve_local(&pool).await.expect("local resolves");
        assert_eq!(
            target.1[0].scope_bindings(),
            vec![ScopeBinding::StoredCredential("local".to_string())],
            "a stored local key follows the credential's own scope"
        );

        seed_credential(
            &pool,
            "openai",
            OPENAI_DEFAULT_BASE_URL,
            AuthType::ApiKey,
            "sk-test-openai",
        )
        .await;
        let target = resolve_openai(&pool).await.expect("openai resolves");
        assert_eq!(
            target.1[0].scope_bindings(),
            vec![pinned("openai", OPENAI_DEFAULT_BASE_URL)],
            "a constant upstream is pinned to that constant"
        );

        pool.close().await;
        teardown_test_db(&db).await;
    }

    /// An env-supplied key has no row to scope it, so the env base URL binds
    /// it. Driven through the layer shaping, because mutating process env would
    /// race every other test in the binary.
    #[tokio::test]
    async fn an_env_local_key_is_pinned_to_the_env_base_url() {
        let layer = StaticHeaderLayer::bearer(
            "local".to_string(),
            "env-key".to_string(),
            pinned(
                "LUCIDOS_LOCAL_API_KEY",
                prefs::LOCAL_BASE_URL.default_text(),
            ),
        );
        assert_eq!(
            layer.scope_bindings(),
            vec![ScopeBinding::Pinned {
                what: "LUCIDOS_LOCAL_API_KEY".to_string(),
                base_url: prefs::LOCAL_BASE_URL.default_text().to_string(),
            }]
        );
    }

    /// Precedence: an `apis.json` entry with the same name as a builtin wins —
    /// `resolve_proxy_target` returns it, so the builtin fallback is never
    /// reached (the handler consults `apis.json` first).
    #[tokio::test]
    async fn apis_json_entry_overrides_builtin() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg_dir = tmp.path().join("data/config");
        std::fs::create_dir_all(&cfg_dir).unwrap();
        std::fs::write(
            cfg_dir.join("apis.json"),
            r#"{"openai": {"base_url": "http://openai.override.test"}}"#,
        )
        .unwrap();

        let cfg = crate::api::proxy::resolve_proxy_target(tmp.path(), "openai")
            .await
            .expect("apis.json openai entry must resolve, overriding the builtin");
        assert_eq!(cfg.base_url, "http://openai.override.test");
    }
}
