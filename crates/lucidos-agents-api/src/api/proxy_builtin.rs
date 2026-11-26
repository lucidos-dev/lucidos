mod gateway;

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
use crate::proxy_auth_layer::{AuthLayer, AuthMutation, LayerInput, RetryHint, ScopeBinding};
use async_trait::async_trait;
use axum::http::{HeaderName, StatusCode};
use std::sync::{Arc, LazyLock};

type BuiltinTarget = (String, Vec<Arc<dyn AuthLayer>>);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Resolver {
    Anthropic,
    ClaudeGateway,
    Local,
    OpenAi,
    OpenRouter,
    TypeSafe,
    Vertex,
    XAi,
}

#[derive(Debug)]
pub(crate) struct BuiltinProxy {
    pub name: &'static str,
    pub aliases: &'static [&'static str],
    pub default_base_url: Option<&'static str>,
    resolver: Resolver,
}

/// INVARIANT: Every builtin provider proxy, in the order the agent's context lists them.
/// The order is fixed so the rendered block is byte-stable across turns.
pub(crate) const BUILTIN_PROXIES: [BuiltinProxy; 8] = [
    BuiltinProxy {
        name: "anthropic",
        aliases: &["claude"],
        default_base_url: Some(ANTHROPIC_API_BASE_URL),
        resolver: Resolver::Anthropic,
    },
    BuiltinProxy {
        name: "claude-gateway",
        aliases: &[],
        default_base_url: None,
        resolver: Resolver::ClaudeGateway,
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

pub(crate) const PROVIDERS_WITHOUT_PROXY: [(&str, &str); 2] = [
    (
        "opencode-free",
        "the keyless free tier serves chat only (ADR 0104)",
    ),
    (
        "chatgpt-plan",
        "apps and scripts never spend the user's ChatGPT plan (ADR 0636)",
    ),
];

pub(crate) fn normalized_service_name(name: &str) -> String {
    name.chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

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

pub(crate) fn model_proxy_info(provider: &str, wire_id: &str) -> Option<super::ModelProxyInfo> {
    use super::ModelProxyProtocol;
    use crate::llm::ProviderKind;

    let kind = ProviderKind::from_name(provider)?;
    let proxy = builtin_proxy_for_service(kind.as_str())?;
    let protocol = match kind {
        ProviderKind::Anthropic | ProviderKind::ClaudeGateway => {
            ModelProxyProtocol::AnthropicMessages
        }
        ProviderKind::OpenAi if crate::llm::openai::uses_responses_api(wire_id) => {
            ModelProxyProtocol::OpenaiResponses
        }
        ProviderKind::OpenAi
        | ProviderKind::OpenRouter
        | ProviderKind::XAi
        | ProviderKind::Local => ModelProxyProtocol::OpenaiChatCompletions,
        ProviderKind::Vertex if crate::llm::VertexProvider::is_claude_model(wire_id) => {
            ModelProxyProtocol::VertexAnthropic
        }
        ProviderKind::Vertex => ModelProxyProtocol::VertexGoogle,
        ProviderKind::OpenCodeFree | ProviderKind::ChatGptPlan => return None,
    };
    Some(super::ModelProxyInfo {
        name: proxy.name.to_string(),
        protocol,
        request_defaults: std::iter::once(None)
            .chain(
                crate::llm::reasoning::supported_efforts(kind, wire_id)
                    .iter()
                    .map(|effort| Some(*effort)),
            )
            .map(|effort| {
                let defaults = match protocol {
                    ModelProxyProtocol::AnthropicMessages | ModelProxyProtocol::VertexAnthropic => {
                        crate::llm::anthropic_wire::sdk_request_defaults(
                            wire_id,
                            effort,
                            protocol == ModelProxyProtocol::VertexAnthropic,
                        )
                    }
                    ModelProxyProtocol::VertexGoogle => {
                        crate::llm::vertex::sdk_request_defaults(wire_id, effort)
                    }
                    _ => serde_json::json!({ "body": {}, "headers": {} }),
                };
                (effort.unwrap_or("").to_string(), defaults)
            })
            .collect(),
    })
}

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
        Resolver::ClaudeGateway => gateway::resolve(engine).await,
        Resolver::Local => resolve_local(engine.pool()).await,
        Resolver::OpenAi => resolve_openai(engine.pool()).await,
        Resolver::OpenRouter => resolve_openrouter(engine.pool()).await,
        Resolver::TypeSafe => resolve_typesafe(engine.pool()).await,
        Resolver::Vertex => resolve_vertex(engine).await,
        Resolver::XAi => resolve_xai(engine.pool()).await,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum BuiltinProxyState {
    Configured {
        base_url: String,
        injects_auth: bool,
    },
    NotConfigured,
    Unknown,
}

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
            crate::log_legacy!(
                "[Proxy] Cannot tell whether builtin '{}' is configured ({}): {}",
                proxy.name,
                status,
                msg
            );
            BuiltinProxyState::Unknown
        }
    }
}

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

pub(crate) fn base_includes_v1(base_url: &str) -> bool {
    reqwest::Url::parse(base_url)
        .ok()
        .and_then(|url| url.path_segments().map(|mut s| s.any(|seg| seg == "v1")))
        .unwrap_or(false)
}

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
            None if proxy.resolver == Resolver::ClaudeGateway => {
                "base set by the configured Claude gateway".to_string()
            }
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
    block.push_str(concat!(
        "Never request_credential for a configured one. An unconfigured one is set up in ",
        providers_settings!(),
        ".\n[END BUILTIN PROVIDER PROXIES]",
    ));
    block
}

fn unconfigured_msg(name: &str, missing: &str, how: &str) -> (StatusCode, String) {
    (
        StatusCode::NOT_FOUND,
        format!(
            "proxy '{name}' is a builtin model provider but {missing} is not configured ({how}, or add a '{name}' entry to data/config/apis.json)"
        ),
    )
}

fn pinned(what: &str, base_url: &str) -> ScopeBinding {
    ScopeBinding::Pinned {
        what: what.to_string(),
        base_url: base_url.to_string(),
    }
}

async fn credential_pair(
    pool: &sqlx::PgPool,
    name: &str,
) -> Result<Option<(AuthType, String)>, (StatusCode, String)> {
    match CredentialStore::get(pool, name).await {
        Ok(Some(c)) => match c.secret(pool).await {
            Ok(secret) => Ok(Some((c.auth_type, secret))),
            Err(reason) => Err((StatusCode::BAD_GATEWAY, reason)),
        },
        Ok(None) => Ok(None),
        Err(e) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("failed to read '{name}' credential: {e}"),
        )),
    }
}

async fn resolve_openai(pool: &sqlx::PgPool) -> Result<BuiltinTarget, (StatusCode, String)> {
    let cred = credential_pair(pool, "openai").await?;
    let key = resolve_openai_api_key(
        cred,
        std::env::var(crate::llm::OPENAI_API_KEY_ENV).ok(),
        crate::llm::openai::codex_detect::load(),
    )
    .map(|(k, _)| k);
    let Some(key) = key else {
        return Err(unconfigured_msg(
            "openai",
            "an OpenAI API key",
            concat!(
                "add an 'openai' credential in ",
                providers_settings!(),
                ", set OPENAI_API_KEY"
            ),
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
    let key = resolve_bearer_key(cred, std::env::var(crate::llm::OPENROUTER_API_KEY_ENV).ok());
    let Some(key) = key else {
        return Err(unconfigured_msg(
            "openrouter",
            "an OpenRouter API key",
            concat!(
                "add an 'openrouter' credential in ",
                providers_settings!(),
                ", set LUCIDOS_OPENROUTER_API_KEY"
            ),
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
            concat!(
                "add an 'xai' credential in ",
                providers_settings!(),
                ", set LUCIDOS_XAI_API_KEY"
            ),
        ));
    };
    let layer = StaticHeaderLayer::bearer("xai".to_string(), key, pinned("xai", XAI_BASE_URL));
    Ok((XAI_BASE_URL.to_string(), vec![Arc::new(layer)]))
}

async fn resolve_typesafe(pool: &sqlx::PgPool) -> Result<BuiltinTarget, (StatusCode, String)> {
    let cred = credential_pair(pool, TYPESAFE_CREDENTIAL_SERVICE).await?;
    let key = resolve_bearer_key(cred, std::env::var(TYPESAFE_API_KEY_ENV).ok());
    let Some(key) = key else {
        return Err(unconfigured_msg(
            TYPESAFE_CREDENTIAL_SERVICE,
            "a TypeSafe API key",
            concat!(
                "add a 'typesafe' credential in ",
                providers_settings!(),
                ", set TYPESAFE_API_KEY"
            ),
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
    let auth = resolve_anthropic_auth(cred, std::env::var("ANTHROPIC_API_KEY").ok());
    anthropic_target(auth.map(|(auth, _source)| auth))
}

fn anthropic_target(auth: Option<AnthropicAuth>) -> Result<BuiltinTarget, (StatusCode, String)> {
    let Some(auth) = auth else {
        return Err(unconfigured_msg(
            "anthropic",
            "an Anthropic API key or OAuth token",
            concat!(
                "add an 'anthropic' credential in ",
                providers_settings!(),
                ", set ANTHROPIC_API_KEY"
            ),
        ));
    };
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

enum LocalKeySource {
    Credential,
    Environment(String),
}

async fn resolve_local(pool: &sqlx::PgPool) -> Result<BuiltinTarget, (StatusCode, String)> {
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
                concat!(
                    "set {} in ",
                    providers_settings!(),
                    " or LUCIDOS_LOCAL_BASE_URL"
                ),
                prefs::LOCAL_BASE_URL.key()
            ),
        ));
    }

    let base = base_pref
        .or(base_env)
        .unwrap_or_else(|| prefs::LOCAL_BASE_URL.default_text().to_string());
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
    let cache = engine
        .vertex_token_cache()
        .unwrap_or_else(|| PROXY_VERTEX_TOKEN_CACHE.clone());
    Ok((
        base_url.clone(),
        vec![Arc::new(VertexAdcLayer::new(cache, base_url))],
    ))
}

pub(crate) fn vertex_base_url(project: &str, region: &str) -> String {
    let host = vertex::vertex_host(region);
    format!("https://{host}/v1/projects/{project}/locations/{region}")
}

static PROXY_VERTEX_TOKEN_CACHE: LazyLock<TokenCache> =
    LazyLock::new(|| Arc::new(std::sync::Mutex::new(None)));

struct VertexAdcLayer {
    token_cache: TokenCache,
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

fn is_publisher_model_call(base_url: &str, url: &str) -> bool {
    let Some(rest) = url.strip_prefix(base_url) else {
        return false;
    };
    let path = rest.split_once('?').map_or(rest, |(path, _)| path);
    let segments: Vec<&str> = path.split('/').collect();
    let ["", "publishers", publisher, "models", model_method] = segments.as_slice() else {
        return false;
    };
    let Some((model, method)) = model_method.split_once(':') else {
        return false;
    };
    let is_label = |s: &str, extra: &[u8]| {
        !s.is_empty()
            && s.bytes()
                .all(|b| b.is_ascii_alphanumeric() || extra.contains(&b))
    };
    is_label(publisher, b"-_") && is_label(model, b"-_.@") && is_label(method, b"")
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

    async fn apply(&self, input: &LayerInput<'_>) -> Result<AuthMutation, (StatusCode, String)> {
        if !is_publisher_model_call(&self.base_url, input.url) {
            return Err((
                StatusCode::BAD_REQUEST,
                "proxy 'vertex' forwards only publisher model calls, \
                 /publishers/<publisher>/models/<model>:<method>"
                    .to_string(),
            ));
        }
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
            cache_was_hit: true,
            outputs: serde_json::json!({}),
            ..Default::default()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proxy_auth_layer::BodyView;
    use axum::http::Method;
    use bytes::Bytes;
    use std::collections::HashMap;
    use std::time::Instant;

    fn dummy_input<'a>(
        body: &'a Bytes,
        prior: &'a HashMap<String, serde_json::Value>,
    ) -> LayerInput<'a> {
        input_for("https://example.com/x", body, prior)
    }

    fn input_for<'a>(
        url: &'a str,
        body: &'a Bytes,
        prior: &'a HashMap<String, serde_json::Value>,
    ) -> LayerInput<'a> {
        LayerInput {
            method: &Method::POST,
            url,
            headers: &[],
            body: BodyView::Raw(body),
            prior_layer_outputs: prior,
        }
    }

    const MODEL_CALL: &str = "/publishers/anthropic/models/claude-opus-4-8@default:rawPredict";

    #[test]
    fn vertex_base_url_uses_engine_owned_prefix_per_region() {
        assert_eq!(
            vertex_base_url("my-project", "europe-west1"),
            "https://europe-west1-aiplatform.googleapis.com/v1/projects/my-project/locations/europe-west1"
        );
        assert_eq!(
            vertex_base_url("my-project", "global"),
            "https://aiplatform.googleapis.com/v1/projects/my-project/locations/global"
        );
        assert_eq!(
            vertex_base_url("p", "eu"),
            "https://aiplatform.eu.rep.googleapis.com/v1/projects/p/locations/eu"
        );
    }

    #[test]
    fn vertex_base_url_composes_with_app_supplied_suffix() {
        let base = vertex_base_url("proj", "europe-west1");
        let full = crate::api::proxy::build_target_url(&base, MODEL_CALL, None);
        assert_eq!(
            full,
            "https://europe-west1-aiplatform.googleapis.com/v1/projects/proj/locations/europe-west1/publishers/anthropic/models/claude-opus-4-8@default:rawPredict"
        );
    }

    #[tokio::test]
    async fn vertex_layer_attaches_bearer_from_cached_token() {
        let cache: TokenCache = Arc::new(std::sync::Mutex::new(Some((
            "tok-123".to_string(),
            Instant::now(),
        ))));
        let base = vertex_base_url("proj", "europe-west1");
        let layer = VertexAdcLayer::new(cache.clone(), base.clone());
        let url = crate::api::proxy::build_target_url(&base, MODEL_CALL, None);
        let body = Bytes::new();
        let prior = HashMap::new();
        let m = layer.apply(&input_for(&url, &body, &prior)).await.unwrap();
        assert_eq!(m.add_headers.len(), 1);
        assert_eq!(m.add_headers[0].0.as_str(), "authorization");
        assert_eq!(m.add_headers[0].1, "Bearer tok-123");
        assert!(m.cache_was_hit, "layer opts into the 401 retry path");
        assert_eq!(layer.retry_on_401(), RetryHint::InvalidateAndRetry);

        layer.invalidate_cache().await;
        assert!(
            cache.lock().unwrap().is_none(),
            "cache cleared on invalidate"
        );
    }

    #[tokio::test]
    async fn vertex_layer_refuses_a_non_model_path_before_minting() {
        let base = vertex_base_url("proj", "europe-west1");
        let cache: TokenCache = Arc::new(std::sync::Mutex::new(None));
        let layer = VertexAdcLayer::new(cache.clone(), base.clone());
        let body = Bytes::new();
        let prior = HashMap::new();
        for path in [
            "",
            "/",
            "/customJobs",
            "/endpoints/123",
            "/models/123",
            "/publishers/google/models",
            "/publishers/google/models/gemini-3-flash",
            "/publishers/google/models/gemini-3-flash:",
            "/publishers/google/models/:generateContent",
            "/publishers/google/models/gemini-3-flash:generateContent/extra",
            "/publishers/google/models/gemini%2F3:generateContent",
            "/projects/other/locations/eu/publishers/google/models/m:predict",
        ] {
            let url = crate::api::proxy::build_target_url(&base, path, None);
            let Err((status, message)) = layer.apply(&input_for(&url, &body, &prior)).await else {
                panic!("'{path}' must be refused");
            };
            assert_eq!(status, StatusCode::BAD_REQUEST, "{path}: {message}");
            assert!(message.contains("/publishers/"), "{path}: {message}");
        }
        assert!(cache.lock().unwrap().is_none(), "no token was minted");
    }

    #[tokio::test]
    async fn vertex_layer_serves_publisher_model_calls() {
        let base = vertex_base_url("proj", "europe-west1");
        let cache: TokenCache = Arc::new(std::sync::Mutex::new(Some((
            "tok-123".to_string(),
            Instant::now(),
        ))));
        let layer = VertexAdcLayer::new(cache, base.clone());
        let body = Bytes::new();
        let prior = HashMap::new();
        for (path, query) in [
            (MODEL_CALL, None),
            (
                "/publishers/google/models/gemini-3-flash:streamGenerateContent",
                Some("alt=sse"),
            ),
            (
                "/publishers/google/models/imagen-4.0-generate-001:predict",
                None,
            ),
        ] {
            let url = crate::api::proxy::build_target_url(&base, path, query);
            let m = layer
                .apply(&input_for(&url, &body, &prior))
                .await
                .unwrap_or_else(|e| panic!("'{url}' refused: {e:?}"));
            assert_eq!(m.add_headers[0].1, "Bearer tok-123", "{url}");
        }
    }

    #[test]
    fn vertex_base_url_keeps_a_crafted_region_on_a_vertex_host() {
        for region in ["evil.test#", "evil.test/", "x@evil.test", "a.b"] {
            let base = vertex_base_url("proj", region);
            let url = reqwest::Url::parse(&base).expect("base parses");
            let host = url.host_str().expect("base has a host");
            assert!(vertex::is_vertex_host(host), "{region} named host {host}");
        }
    }

    #[test]
    fn model_proxy_info_exists_exactly_for_catalog_providers() {
        for kind in crate::llm::ProviderKind::ALL {
            let proxied = BUILTIN_PROXIES.iter().any(|p| p.name == kind.as_str());
            let info = model_proxy_info(kind.as_str(), "some-model");
            assert_eq!(info.is_some(), proxied, "{}", kind.as_str());
            if let Some(info) = info {
                assert_eq!(info.name, kind.as_str());
            }
        }
    }

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

    use crate::core::AuthType;
    use crate::test_support::{seed_credential, setup_test_db, teardown_test_db};

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
        assert_eq!(
            target.1[0].scope_bindings(),
            vec![pinned(TYPESAFE_CREDENTIAL_SERVICE, TYPESAFE_API_BASE_URL)]
        );
        teardown_test_db(&db).await;
    }

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

    #[tokio::test]
    async fn a_stored_public_local_base_url_is_not_forwarded() {
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
        teardown_test_db(&db).await;
    }

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
