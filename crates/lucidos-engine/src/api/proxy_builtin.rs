//! Builtin model-provider proxies.
//!
//! An app calls `lucidos.proxy(<name>).fetch(path, init)` → the engine's
//! `/api/v1/proxy/<name>/<path>` route. When `<name>` has no entry in
//! `data/config/apis.json` but matches one of the model-registry providers
//! (`vertex`, `openai`, `openrouter`, `xai`, `anthropic`, `local`), the engine
//! synthesizes the upstream target here — the provider's API base URL plus a
//! server-side auth layer sourced from the engine's OWN provider auth, resolved
//! exactly as the LLM providers resolve it (a stored credential first, then the
//! provider's env fallback), so a workspace never has to duplicate a provider
//! credential into `apis.json`.
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

use crate::api::proxy_auth_layer::{AuthLayer, AuthMutation, LayerInput, RetryHint};
use crate::api::proxy_static_layers::StaticHeaderLayer;
use crate::core::{
    AuthType, CredentialStore, PreferenceStore, DEFAULT_LOCAL_BASE_URL, PREF_LOCAL_BASE_URL,
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
/// pipeline. Fed straight into [`crate::api::proxy::dispatch_with_layers`].
type BuiltinTarget = (String, Vec<Arc<dyn AuthLayer>>);

/// Resolve a builtin model-provider proxy target for `name`.
///
/// - `Ok(None)` — `name` is not a builtin provider (caller returns the generic
///   "not configured" 404, so a genuinely unknown name is unchanged).
/// - `Err((404, msg))` — `name` IS a builtin provider but its credential/config
///   is absent; the message names what to configure.
/// - `Ok(Some((base_url, layers)))` — resolved; forward through the layers.
pub(crate) async fn resolve_builtin_provider(
    engine: &Arc<crate::engine::LucidosEngine>,
    name: &str,
) -> Result<Option<BuiltinTarget>, (StatusCode, String)> {
    match name {
        "openai" => resolve_openai(engine.pool()).await.map(Some),
        "openrouter" => resolve_openrouter(engine.pool()).await.map(Some),
        "xai" => resolve_xai(engine.pool()).await.map(Some),
        "anthropic" => resolve_anthropic(engine.pool()).await.map(Some),
        "local" => resolve_local(engine.pool()).await.map(Some),
        "vertex" => resolve_vertex(engine).await.map(Some),
        _ => Ok(None),
    }
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
    let layer = StaticHeaderLayer::bearer("openai".to_string(), key);
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
    let layer = StaticHeaderLayer::bearer("openrouter".to_string(), key);
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
    let layer = StaticHeaderLayer::bearer("xai".to_string(), key);
    Ok((XAI_BASE_URL.to_string(), vec![Arc::new(layer)]))
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
            StaticHeaderLayer::api_key("anthropic".to_string(), "x-api-key", key).map_err(|e| {
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
            )),
            Arc::new(
                StaticHeaderLayer::api_key(
                    "anthropic-oauth-beta".to_string(),
                    "anthropic-beta",
                    crate::llm::anthropic::ANTHROPIC_OAUTH_BETA.to_string(),
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

async fn resolve_local(pool: &sqlx::PgPool) -> Result<BuiltinTarget, (StatusCode, String)> {
    // Opt-in, mirroring `build_local_provider`: only resolve when a base URL
    // (pref or env) or key is configured — otherwise a default localhost
    // backend isn't conjured for a workspace that never asked for one.
    let base_pref = match PreferenceStore::get(pool, PREF_LOCAL_BASE_URL).await {
        Ok(opt) => opt.filter(|s| !s.trim().is_empty()),
        Err(e) => {
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("failed to read local_base_url preference: {e}"),
            ));
        }
    };
    let base_env = std::env::var("LUCIDOS_LOCAL_BASE_URL")
        .ok()
        .filter(|s| !s.trim().is_empty());
    let key = credential_pair(pool, "local")
        .await?
        .map(|(_, v)| v)
        .filter(|s| !s.trim().is_empty())
        .or_else(|| {
            std::env::var("LUCIDOS_LOCAL_API_KEY")
                .ok()
                .filter(|s| !s.trim().is_empty())
        });

    if base_pref.is_none() && base_env.is_none() && key.is_none() {
        return Err(unconfigured_msg(
            "local",
            "a local OpenAI-compatible backend",
            "set local_base_url in Settings → Models → Providers or LUCIDOS_LOCAL_BASE_URL",
        ));
    }

    let base = base_pref
        .or(base_env)
        .unwrap_or_else(|| DEFAULT_LOCAL_BASE_URL.to_string());
    // A keyless local server (Ollama / llama.cpp) gets no auth layer.
    let layers: Vec<Arc<dyn AuthLayer>> = match key {
        Some(k) => vec![Arc::new(StaticHeaderLayer::bearer("local".to_string(), k))],
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
    Ok((base_url, vec![Arc::new(VertexAdcLayer::new(cache))]))
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
}

impl VertexAdcLayer {
    fn new(token_cache: TokenCache) -> Self {
        Self { token_cache }
    }
}

#[async_trait]
impl AuthLayer for VertexAdcLayer {
    fn output_namespace(&self) -> &str {
        "vertex"
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
        let layer = VertexAdcLayer::new(cache.clone());
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

    /// A `local_base_url` preference makes the `local` builtin resolve to that
    /// base; with no key it injects no auth header (keyless local server).
    #[tokio::test]
    async fn resolve_local_uses_pref_base_and_is_keyless() {
        let (pool, db) = setup_test_db().await;
        crate::test_support::seed_preference(
            &pool,
            PREF_LOCAL_BASE_URL,
            "http://localhost:1234/v1",
        )
        .await
        .expect("seed local_base_url pref");

        let target = resolve_local(&pool).await.expect("local resolves");
        assert_eq!(target.0, "http://localhost:1234/v1");
        assert!(
            injected_headers(&target).await.is_empty(),
            "keyless local server must get no auth header"
        );
        teardown_test_db(&db).await;
    }

    /// A `local` credential adds a `Bearer` header on top of the pref base.
    #[tokio::test]
    async fn resolve_local_with_key_injects_bearer() {
        let (pool, db) = setup_test_db().await;
        crate::test_support::seed_preference(
            &pool,
            PREF_LOCAL_BASE_URL,
            "http://localhost:1234/v1",
        )
        .await
        .expect("seed local_base_url pref");
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
