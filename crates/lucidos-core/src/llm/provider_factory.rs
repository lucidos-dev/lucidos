use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde_json::{Map, Value};
use sqlx::PgPool;
use std::sync::Arc;

use super::chatgpt_plan_pause::LimitPause;
use super::vertex_auth::TokenCache;
use super::vertex_defaults::LocationHandle;
use super::{LlmProvider, ProviderSelection, WebSearchChain, ANTHROPIC_API_BASE_URL};

type BoxError = Box<dyn std::error::Error + Send + Sync>;

#[async_trait]
pub trait ProviderFactory: Send + Sync {
    async fn build_active(
        &self,
        pool: Option<&PgPool>,
        context: &ProviderBuildContext,
    ) -> Result<ProviderBuildOutcome, BoxError>;

    async fn build_image(
        &self,
        pool: &PgPool,
        openai_api_key: Option<&str>,
        vertex_project_id: &str,
        vertex_location: &LocationHandle,
        vertex_token_cache: &Option<TokenCache>,
    ) -> Option<Arc<dyn ImageProvider>>;
}

/// INVARIANT: Boot and the configuration subscriber build with the same context.
/// Clones share the live Vertex region, token cache, model registry and not-served ledger.
#[derive(Clone)]
pub struct ProviderBuildContext {
    pub default_model: String,
    /// INVARIANT: `LUCIDOS_MODEL == "mock"`. When true the subscriber is never spawned;
    /// passing `false` (the subscriber always does) guarantees the build can
    /// never return [`ProviderSelection::Mock`].
    pub model_is_mock: bool,
    pub vertex_project_id: String,
    pub vertex_location: LocationHandle,
    pub vertex_token_cache: Option<TokenCache>,
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

#[async_trait::async_trait]
pub trait GatewayAccess: Send + Sync {
    async fn bearer(&self) -> Result<String, Box<dyn std::error::Error + Send + Sync>>;

    async fn renew_rejected(
        &self,
        rejected: &str,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>>;

    async fn managed_settings(
        &self,
    ) -> Result<Option<GatewayManagedSettings>, Box<dyn std::error::Error + Send + Sync>>;
}

pub type GatewayAccessFactory = Arc<dyn Fn(GatewayOrigin) -> Arc<dyn GatewayAccess> + Send + Sync>;

/// INVARIANT: A validated Claude gateway origin, such as `https://gateway.example`.
///
/// The only way to name a gateway host. It uses `https`, or plain `http` on a
/// loopback host for tests. It carries no path, query or credentials. It is
/// never the direct Anthropic host, so a gateway bearer cannot reach it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GatewayOrigin(String);

impl GatewayOrigin {
    pub fn parse(url: &str) -> Result<Self, String> {
        let url = url.trim();
        let refuse = |why: &str| Err(format!("Claude gateway URL '{url}' {why}"));
        let Ok(parsed) = reqwest::Url::parse(url) else {
            return refuse("is not a URL");
        };
        let Some(host) = parsed.host_str() else {
            return refuse("has no host");
        };
        let host = host.to_ascii_lowercase();
        match parsed.scheme() {
            "https" => {}
            "http" if is_loopback(&host) => {}
            _ => return refuse("must use https"),
        }
        if !parsed.username().is_empty() || parsed.password().is_some() {
            return refuse("must not carry credentials");
        }
        if !matches!(parsed.path(), "" | "/") || parsed.query().is_some() {
            return refuse("must be an origin, with no path or query");
        }
        if parsed.fragment().is_some() {
            return refuse("must not carry a fragment");
        }
        if Some(host.as_str()) == anthropic_api_host().as_deref() {
            return refuse("is the direct Anthropic API, not a gateway");
        }
        let port = parsed.port().map(|p| format!(":{p}")).unwrap_or_default();
        Ok(Self(format!("{}://{host}{port}", parsed.scheme())))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn api_base_url(&self) -> String {
        format!("{}/v1", self.0)
    }

    /// INVARIANT: Whether `url` lies on this origin, so a token sent there stays on the
    /// gateway.
    pub fn owns(&self, url: &str) -> bool {
        let Ok(parsed) = reqwest::Url::parse(url) else {
            return false;
        };
        let Some(host) = parsed.host_str() else {
            return false;
        };
        let port = parsed.port().map(|p| format!(":{p}")).unwrap_or_default();
        let origin = format!("{}://{}{port}", parsed.scheme(), host.to_ascii_lowercase());
        origin == self.0
    }
}

impl std::fmt::Display for GatewayOrigin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

pub(crate) fn is_loopback(host: &str) -> bool {
    let bare = host.trim_start_matches('[').trim_end_matches(']');
    bare == "localhost"
        || bare
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
}

fn anthropic_api_host() -> Option<String> {
    reqwest::Url::parse(ANTHROPIC_API_BASE_URL)
        .ok()?
        .host_str()
        .map(str::to_ascii_lowercase)
}

#[derive(Clone, Default, PartialEq)]
pub struct GatewayManagedSettings(Map<String, Value>);

impl std::fmt::Debug for GatewayManagedSettings {
    /// INVARIANT: Names the keys only: an `env` value can be a collector's credential.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_set().entries(self.0.keys()).finish()
    }
}

impl GatewayManagedSettings {
    pub fn from_document(document: &Value) -> Self {
        Self(document.as_object().cloned().unwrap_or_default())
    }

    pub fn env(&self, key: &str) -> Option<&str> {
        self.0.get("env")?.get(key)?.as_str()
    }

    pub fn available_models(&self) -> Option<Vec<String>> {
        let entries: Vec<String> = self
            .0
            .get("availableModels")?
            .as_array()?
            .iter()
            .filter_map(|entry| entry.as_str().map(str::to_string))
            .collect();
        (!entries.is_empty()).then_some(entries)
    }

    pub fn served_ids(&self) -> Option<Vec<String>> {
        let mut ids: Vec<String> = Vec::new();
        for entry in self.available_models()? {
            let id = strip_bracket_suffix(&entry);
            if !id.contains('@') && !ids.iter().any(|seen| seen == id) {
                ids.push(id.to_string());
            }
        }
        (!ids.is_empty()).then_some(ids)
    }

    pub fn reported_model(&self, model: &str) -> String {
        let id = strip_bracket_suffix(model);
        let prefix = format!("{id}@");
        self.available_models()
            .into_iter()
            .flatten()
            .find_map(|entry| {
                let alias = strip_bracket_suffix(&entry).strip_prefix(&prefix)?;
                (!alias.is_empty()).then(|| format!("{id}-{alias}"))
            })
            .unwrap_or_else(|| id.to_string())
    }

    pub fn telemetry_requested(&self) -> bool {
        self.env("CLAUDE_CODE_ENABLE_TELEMETRY") == Some("1")
            && self.env("OTEL_LOGS_EXPORTER") == Some("otlp")
    }
}

fn strip_bracket_suffix(entry: &str) -> &str {
    match entry.strip_suffix(']').and_then(|rest| rest.rfind('[')) {
        Some(open) => &entry[..open],
        None => entry,
    }
}

/// INVARIANT: What every plan provider build shares, so a rebuild keeps the refresh
/// lock and the limit pause.
#[derive(Clone)]
pub struct ChatGptPlanAccess {
    pub tokens: Arc<dyn ChatGptPlanTokens>,
    pub pause: Arc<LimitPause>,
}

#[async_trait::async_trait]
pub trait ChatGptPlanTokens: Send + Sync {
    async fn access_token(&self) -> Result<String, Box<dyn std::error::Error + Send + Sync>>;

    async fn after_rejection(
        &self,
        rejected: &str,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>>;
}

#[derive(Debug, Clone, Copy)]
pub enum ImageSize {
    Square,
    Landscape,
    Portrait,
    Auto,
}

impl ImageSize {
    pub fn parse_size(s: &str) -> Self {
        match s {
            "square" => ImageSize::Square,
            "landscape" => ImageSize::Landscape,
            "portrait" => ImageSize::Portrait,
            _ => ImageSize::Auto,
        }
    }
}

pub struct ImageResult {
    pub bytes: Vec<u8>,
    pub mime_type: String,
    pub input_tokens: Option<u32>,
    pub output_tokens: Option<u32>,
    /// INVARIANT: The model id that served this call, for `ContextCaptured.model`, which
    /// means the serving model everywhere it appears. It rides on the result
    /// rather than the provider because Imagen picks its model per call:
    /// generating and editing hit different ones.
    pub model: String,
}

#[async_trait]
pub trait ImageProvider: Send + Sync {
    async fn generate(
        &self,
        prompt: &str,
        input_images: Vec<Vec<u8>>,
        size: ImageSize,
        call: crate::llm::metered::CallToken,
    ) -> Result<ImageResult, Box<dyn std::error::Error + Send + Sync>>;

    fn supports_multi_image(&self) -> bool;

    fn name(&self) -> &str;
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ClaudeGatewayAccountInfo {
    pub issuer: String,
    pub email: Option<String>,
    pub signed_in_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ChatGptPlanAccountInfo {
    pub email: Option<String>,
    pub signed_in_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize)]
pub struct SignOutOutcome {
    pub signed_out: bool,
    pub account_forgotten: bool,
}
