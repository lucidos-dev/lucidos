use super::*;

use crate::api::proxy_pipeline_config::{LayerConfig, PipelineConfig};
use crate::api::proxy_timeout::CallBudget;
use crate::core::hex::hex_lower;
use crate::core::proxy_timeout_bounds::MAX_SECS;
use crate::core::url_scope::{check_path_within, path_is_within, PathEscape, MAX_DECODE_ROUNDS};
use crate::core::{Credential, CredentialStore};
use axum::body::{Body, Bytes};
use axum::http::{HeaderName, Method};
use std::collections::HashMap;
use std::path::Path as FsPath;
use std::sync::Arc;
use std::time::Duration;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProxyConfig {
    pub base_url: String,
    #[serde(default)]
    pub auth: Option<PipelineConfig>,
    #[serde(default)]
    pub insecure_transport: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_secs: Option<f64>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum HmacAlgorithm {
    Sha256,
    Sha512,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum HmacSignedPayload {
    QueryString,
}

pub type ProxyConfigMap = HashMap<String, ProxyConfig>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transport {
    Verified,
    Unverified,
}

fn proxy_client_options() -> reqwest::ClientBuilder {
    reqwest::Client::builder()
        .no_proxy()
        .pool_max_idle_per_host(5)
        .pool_idle_timeout(Duration::from_secs(30))
        .connect_timeout(Duration::from_secs(5))
        .redirect(reqwest::redirect::Policy::none())
}

static VERIFIED_CLIENT: std::sync::LazyLock<reqwest::Client> = std::sync::LazyLock::new(|| {
    proxy_client_options()
        .build()
        .expect("failed to build proxy reqwest client")
});

static UNVERIFIED_CLIENT: std::sync::LazyLock<reqwest::Client> = std::sync::LazyLock::new(|| {
    proxy_client_options()
        .danger_accept_invalid_certs(true)
        .build()
        .expect("failed to build insecure proxy reqwest client")
});

fn client_for(transport: Transport) -> &'static reqwest::Client {
    match transport {
        Transport::Verified => &VERIFIED_CLIENT,
        Transport::Unverified => &UNVERIFIED_CLIENT,
    }
}

pub use crate::engine::provider_rejection::RejectedProvider;
use crate::engine::provider_rejection::PROXY_CONFIG_REL_PATH;

#[derive(Debug, Clone)]
pub struct InsecureTransport {
    pub provider: String,
    pub reason: String,
}

#[derive(Debug, Clone, Default)]
pub struct ProxyConfigLoad {
    pub providers: ProxyConfigMap,
    pub rejected: Vec<RejectedProvider>,
    pub insecure: Vec<InsecureTransport>,
}

fn only<T>(values: impl IntoIterator<Item = T>) -> Option<T> {
    let mut it = values.into_iter();
    match (it.next(), it.next()) {
        (Some(single), None) => Some(single),
        _ => None,
    }
}

impl ProxyConfigLoad {
    pub fn handshake_script_paths(&self) -> Vec<String> {
        self.handshake_seed_entries()
            .into_iter()
            .map(|entry| entry.path)
            .collect()
    }

    pub fn handshake_seed_entries(&self) -> Vec<crate::core::handshake_approvals::SeedEntry> {
        use crate::api::proxy_pipeline_config::LayerConfig;
        use crate::core::handshake_approvals::{config_path_key, injected_secrets, SeedEntry};
        use std::collections::{BTreeMap, BTreeSet};

        type Seen = (BTreeSet<String>, BTreeSet<BTreeSet<String>>);
        let mut seen: BTreeMap<String, Seen> = BTreeMap::new();
        for cfg in self.providers.values() {
            for layer in cfg.auth.iter().flat_map(|p| &p.pipeline) {
                if let LayerConfig::ScriptHandshake {
                    script,
                    credential,
                    oauth_providers,
                } = layer
                {
                    let entry = seen.entry(config_path_key(script)).or_default();
                    entry.0.insert(cfg.base_url.clone());
                    entry.1.insert(injected_secrets(
                        credential.as_deref(),
                        oauth_providers.iter().map(String::as_str),
                    ));
                }
            }
        }
        seen.into_iter()
            .map(|(path, (urls, injected))| SeedEntry {
                path,
                base_url: only(urls.into_iter().filter(|u| !u.trim().is_empty())),
                injects: only(injected.into_iter().filter(|set| !set.is_empty())),
            })
            .collect()
    }

    pub fn credential_scopes(
        &self,
    ) -> std::collections::BTreeMap<String, std::collections::BTreeSet<String>> {
        use crate::api::proxy_pipeline_config::LayerConfig;
        let mut out: std::collections::BTreeMap<String, std::collections::BTreeSet<String>> =
            Default::default();
        for cfg in self.providers.values() {
            let Some(pipeline) = cfg.auth.as_ref() else {
                continue;
            };
            for layer in &pipeline.pipeline {
                let names: Vec<&String> = match layer {
                    LayerConfig::StaticCredential { credential, .. } => vec![credential],
                    LayerConfig::ScriptHandshake { credential, .. } => credential.iter().collect(),
                    LayerConfig::HmacSigned {
                        key_credential,
                        secret_credential,
                        ..
                    } => vec![key_credential, secret_credential],
                    LayerConfig::WasmSigner {
                        credential_handles, ..
                    } => credential_handles.iter().map(|h| &h.credential).collect(),
                };
                for name in names {
                    out.entry(name.clone())
                        .or_default()
                        .insert(cfg.base_url.clone());
                }
            }
        }
        out
    }

    pub fn rejection_for(&self, name: &str) -> Option<&RejectedProvider> {
        self.rejected
            .iter()
            .find(|r| r.provider.is_none() || r.provider.as_deref() == Some(name))
    }
}

pub fn load_proxy_config(workspace_path: &FsPath) -> ProxyConfigLoad {
    let path = workspace_path.join(PROXY_CONFIG_REL_PATH);
    if !path.exists() {
        return ProxyConfigLoad::default();
    }
    let file_level = |reason: String| ProxyConfigLoad {
        rejected: vec![RejectedProvider {
            provider: None,
            reason,
        }],
        ..Default::default()
    };
    let content = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(e) => return file_level(format!("could not be read: {e}")),
    };
    let raw: HashMap<String, serde_json::Value> = match serde_json::from_str(&content) {
        Ok(m) => m,
        Err(e) => return file_level(format!("is not usable: {e}")),
    };

    let mut load = ProxyConfigLoad::default();
    for (name, entry) in raw {
        match parse_provider(&name, &entry) {
            Ok(cfg) => {
                if let Some(reason) = insecure_transport_reason(&cfg) {
                    load.insecure.push(InsecureTransport {
                        provider: name.clone(),
                        reason,
                    });
                }
                load.providers.insert(name, cfg);
            }
            Err(reason) => load.rejected.push(RejectedProvider {
                provider: Some(name),
                reason,
            }),
        }
    }
    load.rejected.sort_by(|a, b| a.provider.cmp(&b.provider));
    load.insecure.sort_by(|a, b| a.provider.cmp(&b.provider));
    load
}

fn insecure_transport_reason(cfg: &ProxyConfig) -> Option<String> {
    if cfg.insecure_transport {
        return Some(format!(
            "insecure_transport is on, so the engine will not check {}'s certificate \
             and will send credentials over plain http",
            cfg.base_url
        ));
    }
    let carries_auth = cfg.auth.as_ref().is_some_and(|p| !p.pipeline.is_empty());
    if carries_auth && plaintext_to_a_public_host(&cfg.base_url) {
        return Some(format!(
            "it attaches credentials over plain http to {}, which is not loopback. \
             Every call answers 502 until the base_url uses https, or the entry \
             sets \"insecure_transport\": true",
            cfg.base_url,
        ));
    }
    None
}

fn plaintext_to_a_public_host(base_url: &str) -> bool {
    let Ok(url) = reqwest::Url::parse(base_url.trim()) else {
        return false;
    };
    url.scheme() == "http" && !host_is_loopback(url.host_str().unwrap_or_default())
}

fn host_is_loopback(host: &str) -> bool {
    let host = host.to_ascii_lowercase();
    if host == "localhost" {
        return true;
    }
    host.trim_start_matches('[')
        .trim_end_matches(']')
        .parse::<std::net::IpAddr>()
        .is_ok_and(|ip| ip.is_loopback())
}

fn parse_provider(name: &str, entry: &serde_json::Value) -> Result<ProxyConfig, String> {
    let cfg: ProxyConfig = serde_json::from_value(entry.clone()).map_err(|e| {
        entry
            .get("auth")
            .and_then(|auth| super::proxy_migration::legacy_rejection(name, auth))
            .unwrap_or_else(|| format!("provider '{name}': {e}"))
    })?;
    if let Some(reason) = cfg
        .timeout_secs
        .and_then(|secs| super::proxy_timeout::range_rejection("timeout_secs", secs))
    {
        return Err(format!("provider '{name}': {reason}"));
    }
    for layer in cfg.auth.iter().flat_map(|p| &p.pipeline) {
        if let LayerConfig::ScriptHandshake { script, .. } = layer {
            if let Some(reason) = super::proxy_script_runner::script_path_rejection(script) {
                return Err(format!("proxy '{name}' {reason}"));
            }
        }
    }
    Ok(cfg)
}

fn is_hop_by_hop(name_lower: &str) -> bool {
    matches!(
        name_lower,
        "connection"
            | "keep-alive"
            | "proxy-authenticate"
            | "proxy-authorization"
            | "te"
            | "trailer"
            | "transfer-encoding"
            | "upgrade"
    )
}

pub fn should_strip_request_header(name: &HeaderName) -> bool {
    let s = name.as_str();
    let ours = s.starts_with("x-lucidos-");
    let forwarding = matches!(s, "x-forwarded-prefix" | "x-forwarded-host");
    is_hop_by_hop(s)
        || ours
        || forwarding
        || matches!(
            s,
            "host" | "content-length" | "cookie" | "origin" | "referer"
        )
}

fn forwards_response_header(name: &str) -> bool {
    let standard = matches!(
        name,
        "accept-patch"
            | "accept-post"
            | "accept-ranges"
            | "age"
            | "allow"
            | "cache-control"
            | "content-digest"
            | "content-disposition"
            | "content-encoding"
            | "content-language"
            | "content-length"
            | "content-location"
            | "content-range"
            | "content-type"
            | "date"
            | "deprecation"
            | "digest"
            | "etag"
            | "expires"
            | "last-modified"
            | "link"
            | "location"
            | "pragma"
            | "ratelimit"
            | "ratelimit-limit"
            | "ratelimit-policy"
            | "ratelimit-remaining"
            | "ratelimit-reset"
            | "repr-digest"
            | "request-id"
            | "retry-after"
            | "server"
            | "sunset"
            | "traceparent"
            | "tracestate"
            | "vary"
            | "warning"
    );
    let browser_reads_it = matches!(
        name,
        "x-content-security-policy"
            | "x-content-security-policy-report-only"
            | "x-content-type-options"
            | "x-dns-prefetch-control"
            | "x-download-options"
            | "x-frame-options"
            | "x-permitted-cross-domain-policies"
            | "x-ua-compatible"
            | "x-webkit-csp"
            | "x-webkit-csp-report-only"
            | "x-xss-protection"
    );
    let ours = name.starts_with("x-lucidos-") || name.starts_with("x-forwarded-");
    let extension = name.starts_with("x-") && !browser_reads_it && !ours;
    let provider = ["anthropic-", "openai-"]
        .iter()
        .any(|prefix| name.starts_with(prefix));
    standard || extension || provider
}

pub(super) const PROXIED_DOCUMENT_CSP: &str = "sandbox";

fn confine_proxied_response(headers: &mut HeaderMap) {
    headers.insert(
        axum::http::header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    if may_render_as_document(headers) {
        headers.insert(
            axum::http::header::CONTENT_SECURITY_POLICY,
            HeaderValue::from_static(PROXIED_DOCUMENT_CSP),
        );
    }
}

fn may_render_as_document(headers: &HeaderMap) -> bool {
    let mut types = Vec::new();
    for value in headers.get_all(axum::http::header::CONTENT_TYPE) {
        let Ok(value) = value.to_str() else {
            return true;
        };
        types.extend(value.split(',').filter(|t| !t.trim().is_empty()));
    }
    types.is_empty()
        || types
            .into_iter()
            .any(crate::api::file_response::is_active_document)
}

pub const PROXY_ORIGIN_HEADER: &str = "x-lucidos-proxied";

pub(crate) async fn refuse_proxied_request(
    request: axum::extract::Request,
    next: Next,
) -> Response {
    if !request.headers().contains_key(PROXY_ORIGIN_HEADER) {
        return next.run(request).await;
    }
    log_legacy!(
        "[Proxy] Refusing {} {}: it came through a credentialed proxy",
        request.method(),
        request.uri().path()
    );
    super::error::ApiError::new(
        StatusCode::FORBIDDEN,
        "A request forwarded by a Lucidos proxy may not call a Lucidos engine.",
    )
    .into_response()
}

pub fn filter_request_headers(headers: &HeaderMap) -> HeaderMap {
    let mut filtered = HeaderMap::new();
    for (name, value) in headers {
        if !should_strip_request_header(name) {
            filtered.append(name.clone(), value.clone());
        }
    }
    filtered
}

fn browser_proxy_request_allowed(headers: &HeaderMap) -> bool {
    crate::api::browser_origin::browser_request_allowed(headers)
}

fn forbidden_cross_origin_proxy_response() -> Response {
    (
        StatusCode::FORBIDDEN,
        "cross-origin browser requests are not allowed for credentialed proxy routes",
    )
        .into_response()
}

pub fn request_path_has_traversal(path: &str) -> bool {
    path.split('/').any(segment_is_parent) || path.contains('\\')
}

fn segment_is_parent(segment: &str) -> bool {
    segment.to_ascii_lowercase().replace("%2e", ".") == ".."
}

pub fn build_target_url(base_url: &str, path: &str, query: Option<&str>) -> String {
    let base = base_url.trim_end_matches('/');
    let path_part = if path.is_empty() {
        String::new()
    } else if path.starts_with('/') {
        path.to_string()
    } else {
        format!("/{}", path)
    };
    match query {
        Some(q) if !q.is_empty() => format!("{}{}?{}", base, path_part, q),
        _ => format!("{}{}", base, path_part),
    }
}

pub(crate) fn build_contained_target_url(
    base_url: &str,
    path: &str,
    query: Option<&str>,
) -> Result<String, String> {
    let candidate = build_target_url(base_url, path, query);
    let Ok(base) = reqwest::Url::parse(base_url.trim()) else {
        return Err("base_url does not parse".to_string());
    };
    let Ok(target) = reqwest::Url::parse(&candidate) else {
        return Err("upstream URL does not parse".to_string());
    };

    if target.scheme() != base.scheme()
        || target.host_str() != base.host_str()
        || target.port_or_known_default() != base.port_or_known_default()
    {
        return Err("upstream URL leaves the configured origin".to_string());
    }

    let base_path = base.path().trim_end_matches('/');
    match check_path_within(base_path, &target) {
        Ok(()) => Ok(candidate),
        Err(PathEscape::Parsed(path)) => Err(format!(
            "upstream path '{path}' escapes the configured base path '{base_path}'"
        )),
        Err(PathEscape::Decoded(path)) => Err(format!(
            "upstream path '{path}' escapes the configured base path '{base_path}' \
             once encoded separators are decoded"
        )),
        Err(PathEscape::TooDeep) => Err(format!(
            "upstream path nests more than {MAX_DECODE_ROUNDS} layers of percent-encoding"
        )),
    }
}

fn redirect_path_within_base(base_url: &str, target_path: &str) -> Option<String> {
    let base = reqwest::Url::parse(base_url.trim()).ok()?;
    let base_path = base.path().trim_end_matches('/');
    if !path_is_within(base_path, target_path) {
        return None;
    }
    Some(
        target_path
            .strip_prefix(base_path)
            .unwrap_or(target_path)
            .to_string(),
    )
}

pub(crate) fn append_query_param(url: &str, key: &str, value: &str) -> String {
    let separator = if url.contains('?') { '&' } else { '?' };
    format!("{}{}{}={}", url, separator, key, urlencoding::encode(value),)
}

pub(crate) fn redacted_query_log_url(url: &str, key: &str) -> String {
    const REDACTED: &str = "REDACTED";
    let masked = match url.split_once('?') {
        None => url.to_string(),
        Some((base, query)) => {
            let pairs: Vec<String> = query
                .split('&')
                .map(|pair| match pair.split_once('=') {
                    Some((name, _)) if name == key => format!("{key}={REDACTED}"),
                    _ => pair.to_string(),
                })
                .collect();
            format!("{base}?{}", pairs.join("&"))
        }
    };
    append_query_param(&masked, key, REDACTED)
}

pub(crate) fn compute_hmac_hex(algorithm: HmacAlgorithm, secret: &[u8], data: &[u8]) -> String {
    use hmac::{Hmac, Mac};
    use sha2::{Sha256, Sha512};
    match algorithm {
        HmacAlgorithm::Sha256 => {
            let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(secret)
                .expect("HMAC-Sha256 accepts any key length");
            mac.update(data);
            hex_lower(&mac.finalize().into_bytes())
        }
        HmacAlgorithm::Sha512 => {
            let mut mac = <Hmac<Sha512> as Mac>::new_from_slice(secret)
                .expect("HMAC-Sha512 accepts any key length");
            mac.update(data);
            hex_lower(&mac.finalize().into_bytes())
        }
    }
}

pub(crate) async fn resolve_proxy_target(
    workspace_path: &FsPath,
    name: &str,
) -> Result<ProxyConfig, (StatusCode, String)> {
    let load = load_proxy_config(workspace_path);
    if let Some(cfg) = load.providers.get(name) {
        return Ok(cfg.clone());
    }
    if let Some(rejected) = load.rejection_for(name) {
        return Err((
            StatusCode::BAD_GATEWAY,
            format!(
                "proxy '{}' is configured but unusable: {}",
                name, rejected.reason
            ),
        ));
    }
    Err((
        StatusCode::NOT_FOUND,
        format!("proxy '{}' is not configured", name),
    ))
}

pub(crate) async fn fetch_required_credential(
    pool: &sqlx::PgPool,
    name: &str,
) -> Result<Credential, (StatusCode, String)> {
    let found = match CredentialStore::get(pool, name).await {
        Ok(Some(c)) => Ok(Some(c)),
        Ok(None) => {
            CredentialStore::get_typed(pool, name, crate::core::AuthType::OauthClient).await
        }
        Err(e) => Err(e),
    };
    match found {
        Ok(Some(c)) if c.stored_value().is_empty() => Err((
            StatusCode::BAD_GATEWAY,
            format!("proxy auth credential '{}' has an empty value", name),
        )),
        Ok(Some(c)) => Ok(c),
        Ok(None) => Err((
            StatusCode::BAD_GATEWAY,
            format!(
                "proxy auth refers to credential '{}' which is not in the store",
                name
            ),
        )),
        Err(e) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("failed to read credential '{}': {}", name, e),
        )),
    }
}

pub(crate) async fn lookup_credential_value(
    pool: &sqlx::PgPool,
    name: &str,
) -> Result<String, (StatusCode, String)> {
    fetch_required_credential(pool, name)
        .await?
        .secret(pool)
        .await
        .map_err(|reason| (StatusCode::BAD_GATEWAY, reason))
}

pub(crate) async fn check_credential_scope(
    pool: &sqlx::PgPool,
    name: &str,
    base_url: &str,
) -> Result<(), (StatusCode, String)> {
    let credential = fetch_required_credential(pool, name).await?;
    if credential.base_urls.is_empty() {
        return Err((
            StatusCode::BAD_GATEWAY,
            format!(
                "credential '{}' has no base URL, so it will not be sent anywhere. \
                 Set its base URL in Settings to the API it belongs to",
                name
            ),
        ));
    }
    if !crate::core::credential_scope_covers(&credential.base_urls, base_url) {
        return Err((
            StatusCode::BAD_GATEWAY,
            format!(
                "credential '{}' is scoped to {} and will not be sent to {}. \
                 If that host really belongs to this credential, add it under \
                 Settings, Credentials, or with `lucidos credentials set-base-urls`",
                name,
                credential.base_urls.join(", "),
                base_url,
            ),
        ));
    }
    Ok(())
}

pub(crate) struct ScopedPipeline {
    base_url: String,
    layers: Vec<Arc<dyn crate::proxy_auth_layer::AuthLayer>>,
    transport: Transport,
}

impl std::fmt::Debug for ScopedPipeline {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ScopedPipeline")
            .field("base_url", &self.base_url)
            .field("layers", &self.layers.len())
            .field("transport", &self.transport)
            .finish()
    }
}

pub(crate) struct ScopeContext<'a> {
    pub pool: &'a sqlx::PgPool,
    pub workspace_path: &'a FsPath,
    pub event_bus: &'a crate::engine::event_bus::EventBus,
}

impl<'a> ScopeContext<'a> {
    pub(crate) fn from_engine(engine: &'a Arc<crate::engine::LucidosEngine>) -> Self {
        Self {
            pool: engine.pool(),
            workspace_path: engine.workspace_path(),
            event_bus: &engine.event_bus,
        }
    }
}

impl ScopedPipeline {
    pub(crate) async fn bind(
        ctx: &ScopeContext<'_>,
        name: &str,
        base_url: String,
        layers: Vec<Arc<dyn crate::proxy_auth_layer::AuthLayer>>,
        insecure_transport: bool,
    ) -> Result<Self, (StatusCode, String)> {
        if !crate::core::credentials::url_names_a_host(&base_url) {
            return Err((
                StatusCode::BAD_GATEWAY,
                format!("proxy '{name}' base_url is not a URL with a host: {base_url}"),
            ));
        }
        if !layers.is_empty() && !insecure_transport && plaintext_to_a_public_host(&base_url) {
            return Err((
                StatusCode::BAD_GATEWAY,
                format!(
                    "proxy '{name}' would send credentials over plain http to {base_url}, \
                     where anyone on the path reads them. Use https, or accept it by \
                     setting \"insecure_transport\": true on a '{name}' entry in \
                     {PROXY_CONFIG_REL_PATH}"
                ),
            ));
        }
        for layer in &layers {
            for binding in layer.scope_bindings() {
                enforce_binding(ctx, name, &binding, &base_url).await?;
            }
        }
        Ok(Self {
            base_url,
            layers,
            transport: match insecure_transport {
                true => Transport::Unverified,
                false => Transport::Verified,
            },
        })
    }
}

async fn enforce_binding(
    ctx: &ScopeContext<'_>,
    name: &str,
    binding: &crate::proxy_auth_layer::ScopeBinding,
    base_url: &str,
) -> Result<(), (StatusCode, String)> {
    use crate::proxy_auth_layer::ScopeBinding;
    match binding {
        ScopeBinding::StoredCredential(credential) => {
            check_credential_scope(ctx.pool, credential, base_url).await
        }
        ScopeBinding::Pinned {
            what,
            base_url: scope,
        } => {
            if crate::core::credentials::credential_base_url_matches(scope, base_url) {
                return Ok(());
            }
            Err((
                StatusCode::BAD_GATEWAY,
                format!(
                    "proxy '{name}': {what} belongs to {scope} and will not be sent to {base_url}"
                ),
            ))
        }
        ScopeBinding::HandshakeScript(script) => {
            check_handshake_scope(ctx, name, script, base_url).await
        }
        ScopeBinding::HandshakeInjects { script, injects } => {
            check_handshake_injects(ctx, name, script, injects).await
        }
    }
}

async fn check_handshake_injects(
    ctx: &ScopeContext<'_>,
    name: &str,
    script: &str,
    injects: &std::collections::BTreeSet<String>,
) -> Result<(), (StatusCode, String)> {
    use crate::core::handshake_approvals::{self, BindOutcome};
    let workspace = ctx.workspace_path;
    if !handshake_approvals::injects_are_recordable(injects) {
        return Err((
            StatusCode::BAD_GATEWAY,
            format!(
                "proxy '{name}': a credential or OAuth provider named with a comma or a \
                 space cannot be recorded against the handshake script '{script}'. \
                 Rename it in Settings, and name the new one in {PROXY_CONFIG_REL_PATH}"
            ),
        ));
    }
    let refuse = |recorded: &std::collections::BTreeSet<String>| {
        let list = |set: &std::collections::BTreeSet<String>| {
            set.iter().cloned().collect::<Vec<_>>().join(", ")
        };
        (
            StatusCode::BAD_GATEWAY,
            format!(
                "proxy '{name}': the handshake script '{script}' is approved to receive {}, \
                 so it will not receive {}. Put the entry back, or edit the injects column \
                 in .lucidos/approved-handshake-scripts",
                list(recorded),
                list(injects)
            ),
        )
    };
    if let Some(recorded) = handshake_approvals::injects_for(workspace, script) {
        return match recorded == *injects {
            true => Ok(()),
            false => Err(refuse(&recorded)),
        };
    }
    // INVARIANT: Trust on first sight, as the scope does. Two concurrent first requests
    // race here, so the outcome carries whichever set won and this one is
    // checked against it.
    match handshake_approvals::bind_injects_if_absent(workspace, script, injects) {
        Ok(BindOutcome::Bound) => {
            log_legacy!(
                "[Proxy] bound handshake script {} to inject {:?} (ADR 0144)",
                script,
                injects
            );
            ctx.event_bus
                .emit_or_log(
                    crate::engine::event_bus::BusEvent::System(
                        crate::engine::event_bus::SystemEvent::HandshakeScriptInjectsBound {
                            path: script.to_string(),
                            injects: injects.iter().cloned().collect(),
                            actor: None,
                        },
                    ),
                    "[Proxy] HandshakeScriptInjectsBound",
                )
                .await;
            Ok(())
        }
        Ok(BindOutcome::AlreadyBound(recorded)) => match recorded == *injects {
            true => Ok(()),
            false => Err(refuse(&recorded)),
        },
        Ok(BindOutcome::NotBindable) => Ok(()),
        Err(e) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("could not bind what handshake script '{script}' may receive: {e}"),
        )),
    }
}

async fn check_handshake_scope(
    ctx: &ScopeContext<'_>,
    name: &str,
    script: &str,
    base_url: &str,
) -> Result<(), (StatusCode, String)> {
    use crate::core::handshake_approvals::{self, BindOutcome};
    let workspace = ctx.workspace_path;
    let refuse = |scope: &str| {
        (
            StatusCode::BAD_GATEWAY,
            format!(
                "proxy '{name}': the handshake script '{script}' mints a token for {scope}, \
                 so it will not mint one for {base_url}. Point the entry back at {scope}, \
                 or edit the base_url column in .lucidos/approved-handshake-scripts"
            ),
        )
    };
    if let Some(scope) = handshake_approvals::scope_for(workspace, script) {
        return match crate::core::credentials::credential_base_url_matches(&scope, base_url) {
            true => Ok(()),
            false => Err(refuse(&scope)),
        };
    }
    if !handshake_approvals::scope_is_recordable(base_url.trim()) {
        return Err((
            StatusCode::BAD_GATEWAY,
            format!(
                "proxy '{name}': a base_url containing whitespace cannot be recorded against \
                 the handshake script '{script}'. Fix the base_url in \
                 {PROXY_CONFIG_REL_PATH}"
            ),
        ));
    }
    // INVARIANT: Trust on first sight. Two concurrent first requests race here, so the
    // outcome carries whichever scope won and this one is checked against it.
    match handshake_approvals::bind_scope_if_absent(workspace, script, base_url) {
        Ok(BindOutcome::Bound) => {
            log_legacy!(
                "[Proxy] bound handshake script {} to {} (ADR 0144)",
                script,
                base_url
            );
            ctx.event_bus
                .emit_or_log(
                    crate::engine::event_bus::BusEvent::System(
                        crate::engine::event_bus::SystemEvent::HandshakeScriptScopeBound {
                            path: script.to_string(),
                            base_url: base_url.to_string(),
                            actor: None,
                        },
                    ),
                    "[Proxy] HandshakeScriptScopeBound",
                )
                .await;
            Ok(())
        }
        Ok(BindOutcome::AlreadyBound(scope)) => {
            match crate::core::credentials::credential_base_url_matches(&scope, base_url) {
                true => Ok(()),
                false => Err(refuse(&scope)),
            }
        }
        Ok(BindOutcome::NotBindable) => Ok(()),
        Err(e) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("could not bind handshake script '{script}' to {base_url}: {e}"),
        )),
    }
}

/// INVARIANT: Forward a request to the configured upstream and return the upstream
/// response (headers + body + status). Pure with respect to AppState: takes
/// only the data it needs, so the integration tests can spin up a tiny axum
/// server and exercise this directly.
///
/// `log_url` is what gets written to logs and error responses on failure;
/// for `query_param` auth this MUST be a redacted form of `target_url`
/// (otherwise the credential leaks through the logs and the upstream-error
/// response body). For everything else, callers pass `target_url` for both.
#[allow(clippy::too_many_arguments)]
pub async fn forward_request(
    method: Method,
    target_url: &str,
    log_url: &str,
    request_headers: HeaderMap,
    auth_headers: Vec<(HeaderName, HeaderValue)>,
    body: Bytes,
    transport: Transport,
    timeout: Duration,
) -> Response {
    let req_method = match reqwest::Method::from_bytes(method.as_str().as_bytes()) {
        Ok(m) => m,
        Err(_) => {
            return (
                StatusCode::BAD_REQUEST,
                format!("invalid HTTP method: {}", method.as_str()),
            )
                .into_response();
        }
    };
    let mut builder = client_for(transport)
        .request(req_method, target_url)
        .timeout(timeout);

    let mut filtered = filter_request_headers(&request_headers);
    for (name, _) in &auth_headers {
        filtered.remove(name);
    }
    for (name, value) in &filtered {
        builder = builder.header(name.as_str(), value.as_bytes());
    }
    for (name, value) in &auth_headers {
        builder = builder.header(name.as_str(), value.as_bytes());
    }
    builder = builder.header(PROXY_ORIGIN_HEADER, "1");
    if !body.is_empty() {
        builder = builder.body(body);
    }

    match builder.send().await {
        Ok(resp) => {
            let status =
                StatusCode::from_u16(resp.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
            let resp_headers = resp.headers().clone();
            let resp_body = match resp.bytes().await {
                Ok(b) => b,
                Err(e) => {
                    let is_timeout = e.is_timeout();
                    let safe_e = e.without_url();
                    log_legacy!(
                        "[Proxy] reading upstream body from {} failed: {}",
                        log_url,
                        safe_e
                    );
                    let (status, what) = match is_timeout {
                        true => (StatusCode::GATEWAY_TIMEOUT, "upstream timeout"),
                        false => (StatusCode::BAD_GATEWAY, "upstream body read failed"),
                    };
                    return (status, format!("{what}: {safe_e}")).into_response();
                }
            };
            let mut response = Response::new(Body::from(resp_body));
            *response.status_mut() = status;
            for (name, value) in &resp_headers {
                if forwards_response_header(name.as_str()) {
                    response.headers_mut().append(name, value.clone());
                }
            }
            confine_proxied_response(response.headers_mut());
            super::internal_faults::external(response)
        }
        Err(e) => {
            let is_timeout = e.is_timeout();
            let safe_e = e.without_url();
            log_legacy!("[Proxy] forward to {} failed: {}", log_url, safe_e);
            if is_timeout {
                (
                    StatusCode::GATEWAY_TIMEOUT,
                    format!("upstream timeout: {}", safe_e),
                )
                    .into_response()
            } else {
                (
                    StatusCode::BAD_GATEWAY,
                    format!("upstream error: {}", safe_e),
                )
                    .into_response()
            }
        }
    }
}

pub(super) async fn proxy_handler(
    State(state): State<AppState>,
    Path((name, path)): Path<(String, String)>,
    req: axum::extract::Request,
) -> Response {
    proxy_handle_inner(state, name, path, req).await
}

pub(super) async fn proxy_handler_root(
    State(state): State<AppState>,
    Path(name): Path<String>,
    req: axum::extract::Request,
) -> Response {
    proxy_handle_inner(state, name, String::new(), req).await
}

pub(crate) async fn reload_proxy_modules_into(
    engine: &crate::engine::LucidosEngine,
    workspace_path: &FsPath,
) -> Result<Vec<String>, String> {
    let dir = workspace_path.join("data/auth-modules");
    let new_modules =
        crate::engine::proxy_wasm_signer::load_wasm_modules(&dir, engine.wasm_engine())?;
    let mut names: Vec<String> = new_modules.keys().cloned().collect();
    names.sort();
    *engine.proxy_modules().write().await = new_modules;
    log_legacy!(
        "[Proxy] reloaded WASM auth modules ({}): {:?}",
        names.len(),
        names
    );
    Ok(names)
}

pub(super) async fn proxy_modules_reload(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Response {
    if !browser_proxy_request_allowed(&headers) {
        return forbidden_cross_origin_proxy_response();
    }
    let actor = match crate::api::actor::require_user_actor_response(&headers, &state.pool).await {
        Ok(a) => a,
        Err(resp) => return resp,
    };
    match reload_proxy_modules_into(&state.engine, &state.workspace_path).await {
        Ok(names) => {
            let count = names.len();
            state
                .engine
                .event_bus
                .emit_or_log(
                    crate::engine::event_bus::BusEvent::System(
                        crate::engine::event_bus::SystemEvent::ProxyModulesReloaded {
                            count,
                            names: names.clone(),
                            actor: Some(actor),
                        },
                    ),
                    "[Proxy] ProxyModulesReloaded",
                )
                .await;
            Json(serde_json::json!({"loaded": names})).into_response()
        }
        Err(e) => {
            log_legacy!("[Proxy] reload of WASM auth modules failed: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("reload failed: {e}"),
            )
                .into_response()
        }
    }
}

const MAX_REDIRECT_HOPS: usize = 5;

fn is_redirect_status(status: StatusCode) -> bool {
    matches!(
        status,
        StatusCode::MOVED_PERMANENTLY
            | StatusCode::FOUND
            | StatusCode::SEE_OTHER
            | StatusCode::TEMPORARY_REDIRECT
            | StatusCode::PERMANENT_REDIRECT
    )
}

fn redirect_downgrades_to_get(status: StatusCode, method: &Method) -> bool {
    match status {
        StatusCode::SEE_OTHER => *method != Method::HEAD,
        StatusCode::MOVED_PERMANENTLY | StatusCode::FOUND => *method == Method::POST,
        _ => false,
    }
}

fn origin_of(url: &str) -> Option<(String, String, Option<u16>)> {
    let u = reqwest::Url::parse(url).ok()?;
    let host = u.host_str()?.to_ascii_lowercase();
    Some((
        u.scheme().to_ascii_lowercase(),
        host,
        u.port_or_known_default(),
    ))
}

fn resolve_redirect_location(current_url: &str, location: &str) -> Option<reqwest::Url> {
    let base = reqwest::Url::parse(current_url).ok()?;
    base.join(location).ok()
}

pub(crate) enum ResolvedProxy {
    Config(ProxyConfig),
    Builtin {
        base_url: String,
        layers: Vec<Arc<dyn crate::proxy_auth_layer::AuthLayer>>,
    },
}

pub(crate) async fn resolve_named_proxy(
    engine: &Arc<crate::engine::LucidosEngine>,
    workspace_path: &std::path::Path,
    name: &str,
) -> Result<ResolvedProxy, (StatusCode, String)> {
    match resolve_proxy_target(workspace_path, name).await {
        Ok(config) => Ok(ResolvedProxy::Config(config)),
        Err((StatusCode::NOT_FOUND, generic)) => {
            match crate::api::proxy_builtin::resolve_builtin_provider(engine, name).await? {
                Some((base_url, layers)) => Ok(ResolvedProxy::Builtin { base_url, layers }),
                None => Err((StatusCode::NOT_FOUND, generic)),
            }
        }
        Err(e) => Err(e),
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn dispatch_resolved(
    engine: &Arc<crate::engine::LucidosEngine>,
    name: &str,
    resolved: ResolvedProxy,
    method: Method,
    path: String,
    query: Option<String>,
    mut headers: HeaderMap,
    body: Bytes,
    owner: crate::api::proxy_cost::CostOwner,
) -> Result<Response, (StatusCode, String)> {
    use crate::api::proxy_cost::{prepare_request, record_model_call, Upstream};
    use crate::api::proxy_speech::{ask_for_usage, finish};
    let base_url = match &resolved {
        ResolvedProxy::Config(config) => config.base_url.as_str(),
        ResolvedProxy::Builtin { base_url, .. } => base_url.as_str(),
    };
    let (body, speech) = ask_for_usage(base_url, &path, body)?;
    let response = match resolved {
        ResolvedProxy::Config(config) => {
            let upstream = Upstream::Configured {
                base_url: &config.base_url,
            };
            prepare_request(&upstream, &mut headers);
            let response = dispatch_proxy_request(
                engine,
                name,
                &config,
                method,
                path.clone(),
                query,
                headers,
                body.clone(),
            )
            .await?;
            record_model_call(engine, owner.clone(), upstream, &path, &body, response).await
        }
        ResolvedProxy::Builtin { base_url, layers } => {
            let ctx = ScopeContext::from_engine(engine);
            let scoped = ScopedPipeline::bind(&ctx, name, base_url, layers, false).await?;
            let timeout = crate::api::proxy_timeout::resolve(ctx.pool, None).await?;
            prepare_request(&Upstream::Builtin { name }, &mut headers);
            let response = dispatch_scoped(
                name,
                &scoped,
                timeout,
                method,
                path.clone(),
                query,
                headers,
                body.clone(),
            )
            .await?;
            record_model_call(
                engine,
                owner.clone(),
                Upstream::Builtin { name },
                &path,
                &body,
                response,
            )
            .await
        }
    };
    Ok(finish(engine, owner, speech, &body, response).await)
}

async fn proxy_handle_inner(
    state: AppState,
    name: String,
    path: String,
    req: axum::extract::Request,
) -> Response {
    if !browser_proxy_request_allowed(req.headers()) {
        return forbidden_cross_origin_proxy_response();
    }
    if request_path_has_traversal(&path) {
        return (
            StatusCode::BAD_REQUEST,
            "proxy path may not contain '..' or backslash segments".to_string(),
        )
            .into_response();
    }
    let resolved = match resolve_named_proxy(&state.engine, &state.workspace_path, &name).await {
        Ok(resolved) => resolved,
        Err((status, msg)) => return (status, msg).into_response(),
    };

    let method = req.method().clone();
    let query = req.uri().query().map(|s| s.to_string());
    let headers = req.headers().clone();
    let owner = crate::api::proxy_cost::cost_owner(&headers);
    let body = match axum::body::to_bytes(req.into_body(), 100 * 1024 * 1024).await {
        Ok(b) => b,
        Err(_) => {
            return (StatusCode::BAD_REQUEST, "failed to read request body").into_response();
        }
    };

    match dispatch_resolved(
        &state.engine,
        &name,
        resolved,
        method,
        path,
        query,
        headers,
        body,
        owner,
    )
    .await
    {
        Ok(resp) => resp,
        Err((status, msg)) => (status, msg).into_response(),
    }
}

#[allow(clippy::too_many_arguments)]
pub async fn dispatch_proxy_request(
    engine: &Arc<crate::engine::LucidosEngine>,
    name: &str,
    config: &ProxyConfig,
    method: Method,
    path: String,
    query: Option<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, (StatusCode, String)> {
    let layers = build_pipeline_layers(engine, name, config).await?;
    let scoped = ScopedPipeline::bind(
        &ScopeContext::from_engine(engine),
        name,
        config.base_url.clone(),
        layers,
        config.insecure_transport,
    )
    .await?;
    let timeout = crate::api::proxy_timeout::resolve(engine.pool(), config.timeout_secs).await?;
    dispatch_scoped(name, &scoped, timeout, method, path, query, headers, body).await
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn dispatch_scoped(
    name: &str,
    scoped: &ScopedPipeline,
    timeout: Duration,
    method: Method,
    path: String,
    query: Option<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, (StatusCode, String)> {
    let budget = CallBudget::start(timeout);
    let (response, outcome) = forward_with_redirects(
        name,
        scoped,
        &budget,
        &method,
        &path,
        query.as_deref(),
        &headers,
        &body,
    )
    .await?;

    if !crate::api::proxy_pipeline::pipeline_should_retry(
        &scoped.layers,
        &outcome,
        response.status(),
    ) {
        return Ok(response);
    }
    crate::api::proxy_pipeline::pipeline_invalidate_for_retry(&scoped.layers, &outcome).await;
    let (response, _) = forward_with_redirects(
        name,
        scoped,
        &budget,
        &method,
        &path,
        query.as_deref(),
        &headers,
        &body,
    )
    .await?;
    Ok(response)
}

async fn build_pipeline_layers(
    engine: &Arc<crate::engine::LucidosEngine>,
    name: &str,
    config: &ProxyConfig,
) -> Result<Vec<Arc<dyn crate::proxy_auth_layer::AuthLayer>>, (StatusCode, String)> {
    let Some(pipeline_cfg) = config.auth.as_ref() else {
        return Ok(Vec::new());
    };
    // INVARIANT: Snapshot the module map and drop the read guard before the (async)
    // credential lookups inside `build_pipeline`. Holding the guard
    // across DB IO would let a slow lookup block the
    // `proxy_modules_reload` writer (tokio's RwLock is write-preferring,
    // so a queued writer also parks new readers). Values are
    // `Arc<CompiledModule>` — cloning the map is N Arc bumps.
    let modules_snapshot = engine.proxy_modules().read().await.clone();
    let ctx = crate::api::proxy_pipeline_builder::PipelineBuildContext {
        pool: engine.pool().clone(),
        workspace_path: Arc::new(engine.workspace_path().to_path_buf()),
        token_cache: engine.proxy_token_cache_arc(),
        proxy_name: name,
        base_url: &config.base_url,
        proxy_modules: &modules_snapshot,
        wasm_engine: engine.wasm_engine().clone(),
    };
    crate::api::proxy_pipeline_builder::build_pipeline(pipeline_cfg, &ctx).await
}

#[allow(clippy::too_many_arguments)]
async fn forward_with_redirects(
    name: &str,
    scoped: &ScopedPipeline,
    budget: &CallBudget,
    method: &Method,
    initial_path: &str,
    initial_query: Option<&str>,
    headers: &HeaderMap,
    body: &Bytes,
) -> Result<(Response, crate::api::proxy_pipeline::PipelineOutcome), (StatusCode, String)> {
    let mut injected = crate::api::proxy_redaction::InjectedSecrets::default();
    let forwarded = follow_redirects(
        name,
        scoped,
        budget,
        method,
        initial_path,
        initial_query,
        headers,
        body,
        &mut injected,
    )
    .await;
    match forwarded {
        Ok((response, outcome)) => Ok((injected.redact_response(response).await?, outcome)),
        Err((status, message)) => Err((status, injected.redact_text(&message))),
    }
}

#[allow(clippy::too_many_arguments)]
async fn follow_redirects(
    name: &str,
    scoped: &ScopedPipeline,
    budget: &CallBudget,
    method: &Method,
    initial_path: &str,
    initial_query: Option<&str>,
    headers: &HeaderMap,
    body: &Bytes,
    injected: &mut crate::api::proxy_redaction::InjectedSecrets,
) -> Result<(Response, crate::api::proxy_pipeline::PipelineOutcome), (StatusCode, String)> {
    let base_url = scoped.base_url.as_str();
    let layers = scoped.layers.as_slice();
    let initial_url = build_target_url(base_url, initial_path, initial_query);
    let initial_origin = origin_of(&initial_url).ok_or_else(|| {
        (
            StatusCode::BAD_GATEWAY,
            format!("proxy '{name}' base_url has no host (cannot bind auth to upstream)"),
        )
    })?;

    let mut current_path = initial_path.to_string();
    let mut current_query: Option<String> = initial_query.map(|s| s.to_string());
    let mut current_method = method.clone();
    let mut current_body = body.clone();
    let mut first_outcome: Option<crate::api::proxy_pipeline::PipelineOutcome> = None;
    let mut hops = 0usize;

    let out_of_time = || {
        log_legacy!("[Proxy] {} ran out of its {}s call budget", name, MAX_SECS);
        (
            StatusCode::GATEWAY_TIMEOUT,
            format!("proxy '{name}' ran past the {MAX_SECS}s limit on one proxied call"),
        )
    };

    loop {
        let target_url =
            build_contained_target_url(base_url, &current_path, current_query.as_deref()).map_err(
                |reason| {
                    log_legacy!("[Proxy] {} refused an upstream URL: {}", name, reason);
                    let status = if hops == 0 {
                        StatusCode::BAD_REQUEST
                    } else {
                        StatusCode::BAD_GATEWAY
                    };
                    (
                        status,
                        format!("proxy '{name}' refused the upstream URL: {reason}"),
                    )
                },
            )?;

        let call_left = budget.time_left().ok_or_else(out_of_time)?;
        let outcome = tokio::time::timeout(
            call_left,
            crate::api::proxy_pipeline::run_pipeline(
                layers,
                &current_method,
                &target_url,
                &[],
                &current_body,
            ),
        )
        .await
        .map_err(|_| out_of_time())??;
        injected.add_hop(&outcome);

        let body_for_send = outcome
            .replace_body
            .clone()
            .unwrap_or_else(|| current_body.clone());
        let auth_headers: Vec<(HeaderName, HeaderValue)> = outcome
            .headers
            .iter()
            .map(|(header, value)| {
                HeaderValue::from_str(value)
                    .map(|sendable| (header.clone(), sendable))
                    .map_err(|_| {
                        (
                            StatusCode::BAD_GATEWAY,
                            format!(
                                "proxy '{name}' produced an unsendable value for header '{}'",
                                header.as_str()
                            ),
                        )
                    })
            })
            .collect::<Result<Vec<_>, _>>()?;
        let final_url = merge_query_params(&target_url, &outcome.query);
        let request_timeout = budget.next_request().ok_or_else(out_of_time)?;

        let response = forward_request(
            current_method.clone(),
            &final_url,
            &outcome.log_url,
            injected.request_headers(headers),
            auth_headers,
            body_for_send,
            scoped.transport,
            request_timeout,
        )
        .await;

        if first_outcome.is_none() {
            first_outcome = Some(outcome);
        }

        if !is_redirect_status(response.status()) {
            return Ok((response, first_outcome.unwrap()));
        }

        if hops >= MAX_REDIRECT_HOPS - 1 {
            log_legacy!(
                "[Proxy] {} hit redirect-hop cap ({}); refusing to follow further",
                name,
                MAX_REDIRECT_HOPS
            );
            return Err((
                StatusCode::BAD_GATEWAY,
                format!("proxy '{name}' redirect chain exceeded {MAX_REDIRECT_HOPS} hops"),
            ));
        }

        let Some(location) = response
            .headers()
            .get(axum::http::header::LOCATION)
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string())
        else {
            return Ok((response, first_outcome.unwrap()));
        };
        let target = resolve_redirect_location(&final_url, &location).ok_or_else(|| {
            (
                StatusCode::BAD_GATEWAY,
                format!("proxy '{name}' upstream returned redirect with unparseable Location"),
            )
        })?;
        let target_origin = origin_of(target.as_str()).ok_or_else(|| {
            (
                StatusCode::BAD_GATEWAY,
                format!("proxy '{name}' upstream redirected to a Location with no host"),
            )
        })?;
        if target_origin != initial_origin {
            // INVARIANT: Names only — no full URLs in the log (a Location can carry a
            // credential in its query string).
            log_legacy!(
                "[Proxy] {} returned redirect to origin {}://{}:{:?} but auth was bound to {}://{}:{:?} — refused",
                name,
                target_origin.0,
                target_origin.1,
                target_origin.2,
                initial_origin.0,
                initial_origin.1,
                initial_origin.2
            );
            return Err((
                StatusCode::BAD_GATEWAY,
                format!(
                    "proxy '{name}' redirected to a different origin ({}://{}); refusing to re-sign for an unconfigured upstream",
                    target_origin.0, target_origin.1
                ),
            ));
        }
        current_path = redirect_path_within_base(base_url, target.path()).ok_or_else(|| {
            log_legacy!(
                "[Proxy] {} refused a redirect to '{}', outside its base path",
                name,
                target.path()
            );
            (
                StatusCode::BAD_GATEWAY,
                format!(
                    "proxy '{name}' redirected to '{}', outside the configured base path",
                    target.path()
                ),
            )
        })?;
        current_query = target.query().map(|q| q.to_string());
        if redirect_downgrades_to_get(response.status(), &current_method) {
            current_method = Method::GET;
            current_body = Bytes::new();
        }
        hops += 1;
    }
}

fn merge_query_params(url: &str, pairs: &[(String, String)]) -> String {
    let mut out = url.to_string();
    for (k, v) in pairs {
        out = append_query_param(&out, k, v);
    }
    out
}

pub(super) fn router() -> Router<AppState> {
    Router::new()
        .route("/proxy/:name", any(proxy_handler_root))
        .route("/proxy/:name/", any(proxy_handler_root))
        .route("/proxy/:name/*path", any(proxy_handler))
        .route("/proxy-modules/reload", post(proxy_modules_reload))
}

#[cfg(test)]
#[path = "proxy_tests.rs"]
mod tests;
