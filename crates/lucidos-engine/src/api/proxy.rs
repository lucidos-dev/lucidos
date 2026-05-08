//! Generic API proxy for HTTPS-iframe apps that need to call HTTP backends.
//!
//! Apps mount inside an HTTPS iframe (the engine port). The browser blocks
//! direct calls to `http://localhost:5005` from such pages (mixed content),
//! and CORS gets in the way of cross-origin XHR. This module forwards
//! requests through the engine to a configured backend, optionally injecting
//! an auth header sourced from the credential store.
//!
//! Configured via `data/config/apis.json`:
//! ```json
//! {
//!   "sonos":   { "base_url": "http://localhost:5005" },
//!   "comfort": { "base_url": "https://accsmart.panasonic.com",
//!                "auth": { "type": "bearer", "credential": "comfort-cloud" } }
//! }
//! ```

use super::*;

use crate::core::{AuthType, Credential, CredentialStore};
use axum::body::{Body, Bytes};
use axum::http::{HeaderName, Method};
use std::collections::HashMap;
use std::path::Path as FsPath;
use std::time::Duration;

/// Per-API config entry from `data/config/apis.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProxyConfig {
    pub base_url: String,
    #[serde(default)]
    pub auth: Option<ProxyAuth>,
}

/// Auth block — names credentials in the engine credential store and tells
/// the proxy how to attach them to outgoing requests.
///
/// Serialized in `apis.json` with serde's `tag = "type"`, so each variant
/// gets its own JSON shape:
/// - `{"type": "bearer", "credential": "..."}`
/// - `{"type": "api_key", "credential": "...", "header": "X-API-Key"}`
/// - `{"type": "basic", "credential": "..."}`
///
/// Unknown `type` values are rejected at config-load time (typos in
/// `apis.json` fail fast instead of silently producing 401s at request time).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ProxyAuth {
    Bearer {
        /// `service_name` in the credential store.
        credential: String,
    },
    ApiKey {
        credential: String,
        /// Optional header name override (default `Authorization`). Useful
        /// for services that expect e.g. `X-API-Key`.
        #[serde(default)]
        header: Option<String>,
    },
    Basic {
        credential: String,
    },
    /// Inject the credential as a URL query parameter (e.g. Helius's
    /// `?api-key=...` shape). The credential value is URL-encoded and
    /// appended to the existing query string (or starts a new one).
    QueryParam {
        credential: String,
        /// URL query parameter name to inject (e.g. `"api-key"` for Helius).
        param_name: String,
    },
    /// Sign each request with HMAC over its query string. Used by exchanges
    /// like Binance: an API key (`key_credential`) is sent in `key_header`,
    /// and a secret (`secret_credential`) signs the canonical query string.
    /// If `timestamp_param` is set, the current millis-since-epoch is added
    /// to the query before signing (Binance algorithm).
    HmacSigned {
        /// `service_name` in the credential store; auth_value goes into
        /// `key_header`.
        key_credential: String,
        /// `service_name` in the credential store; auth_value used as the
        /// HMAC secret.
        secret_credential: String,
        /// Header name for the API key. Default `"X-API-KEY"`. Set to
        /// `"X-MBX-APIKEY"` for Binance.
        #[serde(default = "default_key_header")]
        key_header: String,
        algorithm: HmacAlgorithm,
        signed_payload: HmacSignedPayload,
        /// Query-string parameter name for the signature. Default
        /// `"signature"`.
        #[serde(default = "default_signature_param")]
        signature_param: String,
        /// If set, current millis-since-epoch is injected as this query
        /// param BEFORE signing (Binance algorithm). Omit for APIs that
        /// don't need timestamping.
        #[serde(default)]
        timestamp_param: Option<String>,
    },
    /// Return the named credentials' values to the caller as a JSON map,
    /// for libraries (e.g. `pcomfortcloud`) that perform their own login
    /// flow. Never injected into outgoing HTTP requests; only callable via
    /// `GET /api/v1/proxy-credentials/<name>` (CLI: `lucidos proxy <name>
    /// --credentials`). The `proxy_request` LLM tool refuses this mode so
    /// raw credentials never reach the model.
    CredentialBundle {
        /// `service_name`s in the credential store. The bundle endpoint
        /// returns a JSON object keyed by these names.
        credentials: Vec<String>,
    },
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
    /// Sign the request's query string (Binance shape).
    QueryString,
}

fn default_signature_param() -> String {
    "signature".to_string()
}

fn default_key_header() -> String {
    "X-API-KEY".to_string()
}

pub type ProxyConfigMap = HashMap<String, ProxyConfig>;

const PROXY_CONFIG_REL_PATH: &str = "data/config/apis.json";

/// Shared client — pooled, no proxy, accepts self-signed certs (for local
/// HTTP backends). Mirrors `dev_proxy::CLIENT` so behavior is consistent.
static CLIENT: std::sync::LazyLock<reqwest::Client> = std::sync::LazyLock::new(|| {
    reqwest::Client::builder()
        .no_proxy()
        .danger_accept_invalid_certs(true)
        .pool_max_idle_per_host(5)
        .pool_idle_timeout(Duration::from_secs(30))
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(30))
        .build()
        .expect("failed to build proxy reqwest client")
});

/// Load proxy config from `<workspace>/data/config/apis.json`.
/// Missing file → empty map (no proxies configured).
pub fn load_proxy_config(workspace_path: &FsPath) -> Result<ProxyConfigMap, String> {
    let path = workspace_path.join(PROXY_CONFIG_REL_PATH);
    if !path.exists() {
        return Ok(ProxyConfigMap::new());
    }
    let content = std::fs::read_to_string(&path)
        .map_err(|e| format!("Failed to read {}: {}", path.display(), e))?;
    serde_json::from_str(&content).map_err(|e| format!("Failed to parse {}: {}", path.display(), e))
}

/// Hop-by-hop headers per RFC 7230 §6.1 — must not be forwarded.
/// Caller MUST pass an already-lowercase string. `HeaderName::as_str()` is
/// guaranteed lowercase by the `http` crate, so callers route through that.
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

/// Headers to strip from the *incoming* request before forwarding upstream.
/// Hop-by-hop + Host (reqwest sets it from the URL) + Cookie/Origin/Referer
/// (these belong to the engine's own origin and would leak browser session
/// context to the upstream).
pub fn should_strip_request_header(name: &HeaderName) -> bool {
    let s = name.as_str();
    is_hop_by_hop(s) || matches!(s, "host" | "cookie" | "origin" | "referer")
}

/// Headers to strip from the *upstream* response before returning to client.
/// Just hop-by-hop — Set-Cookie etc. are fine to pass through.
fn should_strip_response_header(name: &str) -> bool {
    is_hop_by_hop(name)
}

/// Build a copy of `headers` with stripped headers removed.
pub fn filter_request_headers(headers: &HeaderMap) -> HeaderMap {
    let mut filtered = HeaderMap::new();
    for (name, value) in headers {
        if !should_strip_request_header(name) {
            filtered.append(name.clone(), value.clone());
        }
    }
    filtered
}

/// Build the auth header `(name, value)` from credential type + value.
/// Returns `None` for credential types that don't translate to a single header
/// (e.g. `EmailPassword`, `OauthClient`).
pub fn build_auth_header(
    auth_type: AuthType,
    auth_value: &str,
    header_override: Option<&str>,
) -> Option<(HeaderName, HeaderValue)> {
    use base64::Engine as _;
    let header_name_str = header_override.unwrap_or("Authorization");
    let header_name = HeaderName::from_bytes(header_name_str.as_bytes()).ok()?;
    match auth_type {
        AuthType::Bearer => {
            let value = HeaderValue::from_str(&format!("Bearer {}", auth_value)).ok()?;
            Some((header_name, value))
        }
        AuthType::ApiKey => {
            let value = HeaderValue::from_str(auth_value).ok()?;
            Some((header_name, value))
        }
        AuthType::Basic => {
            let encoded = base64::engine::general_purpose::STANDARD.encode(auth_value);
            let value = HeaderValue::from_str(&format!("Basic {}", encoded)).ok()?;
            Some((header_name, value))
        }
        AuthType::Password | AuthType::EmailPassword | AuthType::OauthClient => None,
    }
}

/// Reject `..` traversal segments and backslashes in the proxy path. Without
/// this, a caller can splice `/api/v1/proxy/x/../../admin` and most upstreams
/// normalize the result, escaping any path prefix the operator set in
/// `base_url` (e.g. `https://example.com/safe-prefix`).
pub fn has_traversal(path: &str) -> bool {
    path.split('/').any(|seg| seg == "..") || path.contains('\\')
}

/// Build the upstream URL: `<base_url>/<path>?<query>`. Handles trailing/
/// leading slashes so a `/path` and a `base_url/` don't produce `//`.
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

/// Append `key=urlencoded(value)` to a URL's query string. Preserves any
/// existing query and chooses `?` vs `&` accordingly.
pub(crate) fn append_query_param(url: &str, key: &str, value: &str) -> String {
    let separator = if url.contains('?') { '&' } else { '?' };
    format!("{}{}{}={}", url, separator, key, urlencoding::encode(value),)
}

/// Compute HMAC over `data` with `secret` and return lowercase hex.
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

fn hex_lower(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        use std::fmt::Write as _;
        let _ = write!(s, "{:02x}", b);
    }
    s
}

/// Take an existing query string, optionally append `&<timestamp_param>=<now_ms>`,
/// then compute HMAC over the result and append `&<signature_param>=<hex>`.
/// Returns the final query string (no leading `?`).
///
/// `now_ms` is passed in so tests are deterministic. Production callers pass
/// `chrono::Utc::now().timestamp_millis() as u64`.
pub(crate) fn sign_query_string(
    initial_query: &str,
    secret: &[u8],
    algorithm: HmacAlgorithm,
    timestamp_param: Option<&str>,
    signature_param: &str,
    now_ms: u64,
) -> String {
    let mut query = initial_query.to_string();
    if let Some(ts_param) = timestamp_param {
        if !query.is_empty() {
            query.push('&');
        }
        query.push_str(&format!("{}={}", ts_param, now_ms));
    }
    let sig = compute_hmac_hex(algorithm, secret, query.as_bytes());
    if !query.is_empty() {
        query.push('&');
    }
    query.push_str(&format!("{}={}", signature_param, sig));
    query
}

/// Resolve config name → ProxyConfig. Returns 404 if name not configured.
pub(crate) async fn resolve_proxy_target(
    workspace_path: &FsPath,
    name: &str,
) -> Result<ProxyConfig, (StatusCode, String)> {
    let configs =
        load_proxy_config(workspace_path).map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;
    configs.get(name).cloned().ok_or((
        StatusCode::NOT_FOUND,
        format!("proxy '{}' is not configured", name),
    ))
}

/// Resolve the credential for a config (if any). Returns 502 if the named
/// credential is missing OR has an empty `auth_value` — both leave the
/// proxy unable to inject usable auth, and silently sending `Bearer ` would
/// just produce a confusing upstream 401.
pub(crate) async fn resolve_credential(
    pool: &sqlx::PgPool,
    auth: &ProxyAuth,
) -> Result<Credential, (StatusCode, String)> {
    let name = match auth {
        ProxyAuth::Bearer { credential }
        | ProxyAuth::ApiKey { credential, .. }
        | ProxyAuth::Basic { credential } => credential.as_str(),
        ProxyAuth::QueryParam { .. }
        | ProxyAuth::HmacSigned { .. }
        | ProxyAuth::CredentialBundle { .. } => {
            // These variants are dispatched directly inside `apply_auth` (or, for
            // CredentialBundle, the dedicated handler) and never reach here.
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                "resolve_credential called for an auth variant that does not use a single credential header".to_string(),
            ));
        }
    };
    fetch_required_credential(pool, name).await
}

/// Look up a single credential by service name, mapping missing/empty/db
/// failures to the same `(StatusCode, String)` error shape every auth path
/// uses. The single source of truth for "this credential must exist and be
/// non-empty, or the proxy can't proceed".
async fn fetch_required_credential(
    pool: &sqlx::PgPool,
    name: &str,
) -> Result<Credential, (StatusCode, String)> {
    match CredentialStore::get(pool, name).await {
        Ok(Some(c)) if c.auth_value.is_empty() => Err((
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

/// Look up a single credential's `auth_value`. Same error shapes as
/// `resolve_credential`, but for callers that only need the secret string.
pub(crate) async fn lookup_credential_value(
    pool: &sqlx::PgPool,
    name: &str,
) -> Result<String, (StatusCode, String)> {
    fetch_required_credential(pool, name)
        .await
        .map(|c| c.auth_value)
}

/// Resolved pieces every auth-injection step needs at request build time —
/// the final URL (with any query-param injection already applied), an
/// optional header tuple to attach, and a redacted form of the URL safe
/// for use in log lines (so credentials embedded in the query string don't
/// leak via `[Proxy LLM] GET ... → ?api-key=SECRET`).
pub struct ResolvedAuth {
    pub url: String,
    pub log_url: String,
    pub header: Option<(HeaderName, HeaderValue)>,
}

/// Apply an auth config to a base URL+path+query, returning the final URL
/// and optional auth header. Header-style auth (`Bearer`, `ApiKey`, `Basic`)
/// leaves the URL alone and returns the header. URL-style auth (`QueryParam`)
/// folds the credential into the URL and returns no header. The
/// `credential_bundle` variant is request-time invalid (it's only callable
/// via the dedicated `/api/v1/proxy-credentials/:name` endpoint) and returns
/// 400 here.
pub(crate) async fn apply_auth(
    pool: &sqlx::PgPool,
    auth: Option<&ProxyAuth>,
    base_url: &str,
    path: &str,
    query: Option<&str>,
) -> Result<ResolvedAuth, (StatusCode, String)> {
    let url = build_target_url(base_url, path, query);
    let Some(auth) = auth else {
        return Ok(ResolvedAuth {
            log_url: url.clone(),
            url,
            header: None,
        });
    };
    match auth {
        ProxyAuth::Bearer { .. } | ProxyAuth::ApiKey { .. } | ProxyAuth::Basic { .. } => {
            let cred = resolve_credential(pool, auth).await?;
            let header_override = match auth {
                ProxyAuth::ApiKey { header, .. } => header.as_deref(),
                _ => None,
            };
            let header = build_auth_header(cred.auth_type, &cred.auth_value, header_override);
            Ok(ResolvedAuth {
                log_url: url.clone(),
                url,
                header,
            })
        }
        ProxyAuth::QueryParam {
            credential,
            param_name,
        } => {
            let value = lookup_credential_value(pool, credential).await?;
            let url_with_cred = append_query_param(&url, param_name, &value);
            // Logged URL replaces the credential value with `REDACTED` so it
            // doesn't leak into engine logs or aggregations.
            let log_url = append_query_param(&url, param_name, "REDACTED");
            Ok(ResolvedAuth {
                url: url_with_cred,
                log_url,
                header: None,
            })
        }
        ProxyAuth::HmacSigned {
            key_credential,
            secret_credential,
            key_header,
            algorithm,
            signed_payload: HmacSignedPayload::QueryString,
            signature_param,
            timestamp_param,
        } => {
            apply_hmac_signed(
                pool,
                base_url,
                path,
                query,
                key_credential,
                secret_credential,
                key_header,
                *algorithm,
                signature_param,
                timestamp_param.as_deref(),
            )
            .await
        }
        ProxyAuth::CredentialBundle { .. } => Err((
            StatusCode::BAD_REQUEST,
            "credential_bundle proxies do not inject HTTP auth; use GET /api/v1/proxy-credentials/<name>".to_string(),
        )),
    }
}

/// Sign the request's query string with HMAC and return the final URL plus
/// the API-key header. Lifted out of `apply_auth`'s match so that arm reads
/// as one dispatch instead of forty lines of inline logic.
#[allow(clippy::too_many_arguments)]
async fn apply_hmac_signed(
    pool: &sqlx::PgPool,
    base_url: &str,
    path: &str,
    query: Option<&str>,
    key_credential: &str,
    secret_credential: &str,
    key_header: &str,
    algorithm: HmacAlgorithm,
    signature_param: &str,
    timestamp_param: Option<&str>,
) -> Result<ResolvedAuth, (StatusCode, String)> {
    let key = lookup_credential_value(pool, key_credential).await?;
    let secret = lookup_credential_value(pool, secret_credential).await?;
    let now_ms = u64::try_from(chrono::Utc::now().timestamp_millis()).unwrap_or(0);
    let signed_query = sign_query_string(
        query.unwrap_or(""),
        secret.as_bytes(),
        algorithm,
        timestamp_param,
        signature_param,
        now_ms,
    );
    let url = build_target_url(base_url, path, Some(&signed_query));
    let header_name = HeaderName::from_bytes(key_header.as_bytes()).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("invalid key_header '{}': {}", key_header, e),
        )
    })?;
    let header_value = HeaderValue::from_str(&key).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!(
                "credential '{}' is not a valid HTTP header value: {}",
                key_credential, e
            ),
        )
    })?;
    // Signature is the HMAC output (a hex digest) — knowing it reveals
    // nothing about the secret, so it's safe to log alongside the URL.
    Ok(ResolvedAuth {
        log_url: url.clone(),
        url,
        header: Some((header_name, header_value)),
    })
}

/// Forward a request to the configured upstream and return the upstream
/// response (headers + body + status). Pure with respect to AppState: takes
/// only the data it needs, so the integration tests can spin up a tiny axum
/// server and exercise this directly.
///
/// `log_url` is what gets written to logs and error responses on failure;
/// for `query_param` auth this MUST be a redacted form of `target_url`
/// (otherwise the credential leaks through the logs and the upstream-error
/// response body). For everything else, callers pass `target_url` for both.
pub async fn forward_request(
    method: Method,
    target_url: &str,
    log_url: &str,
    request_headers: HeaderMap,
    auth: Option<(HeaderName, HeaderValue)>,
    body: Bytes,
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
    let mut builder = CLIENT.request(req_method, target_url);

    let filtered = filter_request_headers(&request_headers);
    for (name, value) in &filtered {
        // Pass raw bytes so non-ASCII header values (rare but valid) survive.
        builder = builder.header(name.as_str(), value.as_bytes());
    }
    if let Some((name, value)) = auth {
        builder = builder.header(name.as_str(), value.as_bytes());
    }
    if !body.is_empty() {
        builder = builder.body(body);
    }

    match builder.send().await {
        Ok(resp) => {
            let status =
                StatusCode::from_u16(resp.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
            let resp_headers = resp.headers().clone();
            let resp_body = resp.bytes().await.unwrap_or_default();
            let mut response = Response::builder().status(status);
            for (name, value) in &resp_headers {
                if !should_strip_response_header(name.as_str()) {
                    response = response.header(name, value);
                }
            }
            response
                .body(Body::from(resp_body))
                .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
        }
        Err(e) => {
            // `reqwest::Error`'s Display and Debug impls embed the request URL,
            // which for `query_param` auth contains the credential. Strip it
            // before logging or surfacing the error to the caller.
            let is_timeout = e.is_timeout();
            let safe_e = e.without_url();
            log!("[Proxy] forward to {} failed: {}", log_url, safe_e);
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

/// Axum handler: `/api/v1/proxy/:name/*path`.
pub(super) async fn proxy_handler(
    State(state): State<AppState>,
    Path((name, path)): Path<(String, String)>,
    req: axum::extract::Request,
) -> Response {
    proxy_handle_inner(state, name, path, req).await
}

/// Axum handler: `/api/v1/proxy/:name` (no path component).
pub(super) async fn proxy_handler_root(
    State(state): State<AppState>,
    Path(name): Path<String>,
    req: axum::extract::Request,
) -> Response {
    proxy_handle_inner(state, name, String::new(), req).await
}

/// Pure logic for the credential-bundle endpoint — given a resolved config
/// and a pool, produce the JSON map (or an HTTP error). Lifted out of the
/// handler so the variant guard is testable without a full `AppState`.
pub(crate) async fn build_credential_bundle(
    pool: &sqlx::PgPool,
    config: &ProxyConfig,
    name: &str,
) -> Result<serde_json::Map<String, serde_json::Value>, (StatusCode, String)> {
    let credentials = match &config.auth {
        Some(ProxyAuth::CredentialBundle { credentials }) => credentials,
        _ => {
            return Err((
                StatusCode::BAD_REQUEST,
                format!("proxy '{}' is not configured as credential_bundle", name),
            ));
        }
    };
    let mut out = serde_json::Map::with_capacity(credentials.len());
    for cred_name in credentials {
        let value = lookup_credential_value(pool, cred_name).await?;
        out.insert(cred_name.clone(), serde_json::Value::String(value));
    }
    Ok(out)
}

/// Axum handler: `GET /api/v1/proxy-credentials/:name`.
///
/// Returns a JSON object `{"<service_name>": "<auth_value>", ...}` for
/// proxies configured with `auth.type == "credential_bundle"`. For other
/// proxy types (or missing proxies), returns 4xx. The credential value
/// never goes through the LLM path — this endpoint is only reachable
/// directly (CLI, scripts).
pub(super) async fn proxy_credentials_handler(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Response {
    let config = match resolve_proxy_target(&state.workspace_path, &name).await {
        Ok(c) => c,
        Err((status, msg)) => return (status, msg).into_response(),
    };
    match build_credential_bundle(&state.pool, &config, &name).await {
        Ok(map) => (
            StatusCode::OK,
            [(axum::http::header::CONTENT_TYPE, "application/json")],
            serde_json::Value::Object(map).to_string(),
        )
            .into_response(),
        Err((status, msg)) => (status, msg).into_response(),
    }
}

async fn proxy_handle_inner(
    state: AppState,
    name: String,
    path: String,
    req: axum::extract::Request,
) -> Response {
    if has_traversal(&path) {
        return (
            StatusCode::BAD_REQUEST,
            "proxy path may not contain '..' or backslash segments".to_string(),
        )
            .into_response();
    }
    let config = match resolve_proxy_target(&state.workspace_path, &name).await {
        Ok(c) => c,
        Err((status, msg)) => return (status, msg).into_response(),
    };

    let method = req.method().clone();
    let query = req.uri().query().map(|s| s.to_string());
    let headers = req.headers().clone();
    let body = match axum::body::to_bytes(req.into_body(), 100 * 1024 * 1024).await {
        Ok(b) => b,
        Err(_) => {
            return (StatusCode::BAD_REQUEST, "failed to read request body").into_response();
        }
    };

    let resolved = match apply_auth(
        &state.pool,
        config.auth.as_ref(),
        &config.base_url,
        &path,
        query.as_deref(),
    )
    .await
    {
        Ok(r) => r,
        Err((status, msg)) => return (status, msg).into_response(),
    };
    forward_request(
        method,
        &resolved.url,
        &resolved.log_url,
        headers,
        resolved.header,
        body,
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderName;
    use axum::routing::any;
    use axum::Router;

    fn hm(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (n, v) in pairs {
            h.append(
                HeaderName::from_bytes(n.as_bytes()).unwrap(),
                HeaderValue::from_str(v).unwrap(),
            );
        }
        h
    }

    fn name(s: &str) -> HeaderName {
        HeaderName::from_bytes(s.as_bytes()).unwrap()
    }

    // ---- Header stripping --------------------------------------------------

    #[test]
    fn strips_hop_by_hop_headers() {
        for h in [
            "Connection",
            "Keep-Alive",
            "Proxy-Authenticate",
            "Proxy-Authorization",
            "TE",
            "Trailer",
            "Transfer-Encoding",
            "Upgrade",
        ] {
            assert!(
                should_strip_request_header(&name(h)),
                "expected {} to be stripped",
                h
            );
        }
    }

    #[test]
    fn strips_host_cookie_origin_referer() {
        for h in ["Host", "Cookie", "Origin", "Referer"] {
            assert!(
                should_strip_request_header(&name(h)),
                "expected {} to be stripped",
                h
            );
        }
    }

    #[test]
    fn strip_check_is_case_insensitive() {
        assert!(should_strip_request_header(&name("cookie")));
        assert!(should_strip_request_header(&name("HOST")));
        assert!(should_strip_request_header(&name("connection")));
    }

    #[test]
    fn keeps_safe_headers() {
        for h in [
            "Content-Type",
            "Accept",
            "Accept-Language",
            "User-Agent",
            "X-Custom-Header",
        ] {
            assert!(
                !should_strip_request_header(&name(h)),
                "expected {} to be kept",
                h
            );
        }
    }

    #[test]
    fn filter_request_headers_drops_stripped_keeps_others() {
        let input = hm(&[
            ("Content-Type", "application/json"),
            ("Cookie", "session=abc"),
            ("Host", "engine.example.com"),
            ("Origin", "https://engine.example.com"),
            ("Referer", "https://engine.example.com/"),
            ("Accept", "application/json"),
            ("X-Custom", "ok"),
        ]);
        let out = filter_request_headers(&input);
        assert!(out.contains_key("content-type"));
        assert!(out.contains_key("accept"));
        assert!(out.contains_key("x-custom"));
        assert!(!out.contains_key("cookie"));
        assert!(!out.contains_key("host"));
        assert!(!out.contains_key("origin"));
        assert!(!out.contains_key("referer"));
    }

    // ---- Auth header building ---------------------------------------------

    #[test]
    fn bearer_builds_authorization_header() {
        let (n, v) = build_auth_header(AuthType::Bearer, "tok-xyz", None).unwrap();
        assert_eq!(n.as_str(), "authorization");
        assert_eq!(v.to_str().unwrap(), "Bearer tok-xyz");
    }

    #[test]
    fn api_key_default_authorization_header() {
        let (n, v) = build_auth_header(AuthType::ApiKey, "raw-key", None).unwrap();
        assert_eq!(n.as_str(), "authorization");
        assert_eq!(v.to_str().unwrap(), "raw-key");
    }

    #[test]
    fn api_key_custom_header_override() {
        let (n, v) = build_auth_header(AuthType::ApiKey, "raw-key", Some("X-API-Key")).unwrap();
        assert_eq!(n.as_str(), "x-api-key");
        assert_eq!(v.to_str().unwrap(), "raw-key");
    }

    #[test]
    fn basic_auth_base64_encodes_user_password() {
        let (n, v) = build_auth_header(AuthType::Basic, "alice:s3cret", None).unwrap();
        assert_eq!(n.as_str(), "authorization");
        // base64("alice:s3cret") = "YWxpY2U6czNjcmV0"
        assert_eq!(v.to_str().unwrap(), "Basic YWxpY2U6czNjcmV0");
    }

    #[test]
    fn unsupported_auth_types_return_none() {
        assert!(build_auth_header(AuthType::EmailPassword, "x", None).is_none());
        assert!(build_auth_header(AuthType::OauthClient, "x", None).is_none());
        assert!(build_auth_header(AuthType::Password, "x", None).is_none());
    }

    // ---- Path traversal ---------------------------------------------------

    #[test]
    fn has_traversal_flags_dot_dot_segments() {
        assert!(has_traversal("../etc/passwd"));
        assert!(has_traversal("foo/../bar"));
        assert!(has_traversal("a/b/../../c"));
        assert!(has_traversal("/.."));
    }

    #[test]
    fn has_traversal_flags_backslashes() {
        assert!(has_traversal("foo\\..\\bar"));
        assert!(has_traversal("a\\b"));
    }

    #[test]
    fn has_traversal_passes_normal_paths() {
        assert!(!has_traversal(""));
        assert!(!has_traversal("/Spisestua/play"));
        assert!(!has_traversal("api/v1/items?id=42"));
        // A literal segment that *contains* `..` but isn't `..` is fine.
        assert!(!has_traversal("foo..bar"));
    }

    // ---- URL building -----------------------------------------------------

    #[test]
    fn build_url_handles_no_trailing_no_leading() {
        assert_eq!(
            build_target_url("http://localhost:5005", "Spisestua/play", None),
            "http://localhost:5005/Spisestua/play"
        );
    }

    #[test]
    fn build_url_handles_trailing_and_leading_slashes() {
        assert_eq!(
            build_target_url("http://localhost:5005/", "/Spisestua/play", None),
            "http://localhost:5005/Spisestua/play"
        );
    }

    #[test]
    fn build_url_empty_path_omits_slash() {
        assert_eq!(
            build_target_url("http://localhost:5005", "", None),
            "http://localhost:5005"
        );
    }

    #[test]
    fn build_url_includes_query_string() {
        assert_eq!(
            build_target_url(
                "http://api.example.com",
                "/v1/items",
                Some("limit=10&page=2")
            ),
            "http://api.example.com/v1/items?limit=10&page=2"
        );
    }

    #[test]
    fn build_url_ignores_empty_query() {
        assert_eq!(
            build_target_url("http://api.example.com", "/v1/items", Some("")),
            "http://api.example.com/v1/items"
        );
    }

    // ---- Query-param appending --------------------------------------------

    #[test]
    fn append_query_param_to_url_with_no_existing_query() {
        let url = append_query_param("https://api.example.com/v1/x", "api-key", "secret-123");
        assert_eq!(url, "https://api.example.com/v1/x?api-key=secret-123");
    }

    #[test]
    fn append_query_param_to_url_with_existing_query() {
        let url = append_query_param(
            "https://api.example.com/v1/x?limit=10&page=2",
            "api-key",
            "secret-123",
        );
        assert_eq!(
            url,
            "https://api.example.com/v1/x?limit=10&page=2&api-key=secret-123"
        );
    }

    #[test]
    fn append_query_param_url_encodes_value_with_special_chars() {
        let url = append_query_param("https://x/y", "k", "a&b=c d");
        assert_eq!(url, "https://x/y?k=a%26b%3Dc%20d");
    }

    #[test]
    fn append_query_param_redacted_form_does_not_contain_credential() {
        // Mirrors what apply_auth does for ProxyAuth::QueryParam to build
        // log_url: same key, value replaced with REDACTED. Guards against
        // the credential leaking into log lines.
        let base = "https://api.example.com/v1/x";
        let real = append_query_param(base, "api-key", "actual-secret-value");
        let redacted = append_query_param(base, "api-key", "REDACTED");
        assert!(real.contains("actual-secret-value"));
        assert!(!redacted.contains("actual-secret-value"));
        assert_eq!(redacted, "https://api.example.com/v1/x?api-key=REDACTED");
    }

    // ---- HMAC signing ------------------------------------------------------

    #[test]
    fn hmac_sha256_known_vector() {
        // RFC 4231 test case 1: key = 0x0b * 20, data = "Hi There"
        let key = [0x0bu8; 20];
        let sig = compute_hmac_hex(HmacAlgorithm::Sha256, &key, b"Hi There");
        assert_eq!(
            sig,
            "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7"
        );
    }

    #[test]
    fn hmac_sha512_known_vector() {
        // RFC 4231 test case 1
        let key = [0x0bu8; 20];
        let sig = compute_hmac_hex(HmacAlgorithm::Sha512, &key, b"Hi There");
        assert_eq!(
            sig,
            "87aa7cdea5ef619d4ff0b4241a1d6cb02379f4e2ce4ec2787ad0b30545e17cdedaa833b7d6b8a702038b274eaea3f4e4be9d914eeb61f1702e696c203a126854"
        );
    }

    #[test]
    fn hmac_query_string_appends_timestamp_then_signature() {
        let signed = sign_query_string(
            "symbol=BTCUSDT&side=BUY",
            b"secret",
            HmacAlgorithm::Sha256,
            Some("timestamp"),
            "signature",
            1_700_000_000_000,
        );
        assert!(
            signed.starts_with("symbol=BTCUSDT&side=BUY&timestamp=1700000000000&signature="),
            "got: {}",
            signed
        );
        let sig_part = signed.rsplit("signature=").next().unwrap();
        assert_eq!(sig_part.len(), 64, "sha256 hex should be 64 chars");
    }

    #[test]
    fn hmac_query_string_skips_timestamp_when_unset() {
        let signed = sign_query_string(
            "a=1&b=2",
            b"secret",
            HmacAlgorithm::Sha256,
            None,
            "signature",
            1_700_000_000_000,
        );
        assert!(signed.starts_with("a=1&b=2&signature="), "got: {}", signed);
        assert!(!signed.contains("timestamp="));
    }

    #[test]
    fn hmac_query_string_handles_empty_initial_query() {
        let signed = sign_query_string(
            "",
            b"secret",
            HmacAlgorithm::Sha256,
            Some("timestamp"),
            "signature",
            1_700_000_000_000,
        );
        // No leading `&` when the initial query was empty.
        assert!(
            signed.starts_with("timestamp=1700000000000&signature="),
            "got: {}",
            signed
        );
    }

    #[test]
    fn hmac_signature_matches_binance_known_example() {
        // Binance's published worked example (SIGNED endpoint test):
        //   secret = "NhqPtmdSJYdKjVHjA7PZj4Mge3R5YNiP1e3UZjInClVN65XAbvqqM6A7H5fATj0j"
        //   query  = "symbol=LTCBTC&side=BUY&type=LIMIT&timeInForce=GTC&quantity=1&price=0.1&recvWindow=5000&timestamp=1499827319559"
        //   sig    = "c8db56825ae71d6d79447849e617115f4a920fa2acdcab2b053c4b2838bd6b71"
        let secret = b"NhqPtmdSJYdKjVHjA7PZj4Mge3R5YNiP1e3UZjInClVN65XAbvqqM6A7H5fATj0j";
        let query = "symbol=LTCBTC&side=BUY&type=LIMIT&timeInForce=GTC&quantity=1&price=0.1&recvWindow=5000&timestamp=1499827319559";
        let sig = compute_hmac_hex(HmacAlgorithm::Sha256, secret, query.as_bytes());
        assert_eq!(
            sig,
            "c8db56825ae71d6d79447849e617115f4a920fa2acdcab2b053c4b2838bd6b71"
        );
    }

    // ---- Config loader ----------------------------------------------------

    #[test]
    fn load_config_returns_empty_when_file_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = load_proxy_config(tmp.path()).unwrap();
        assert!(cfg.is_empty());
    }

    #[test]
    fn load_config_parses_basic_shape() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg_dir = tmp.path().join("data/config");
        std::fs::create_dir_all(&cfg_dir).unwrap();
        std::fs::write(
            cfg_dir.join("apis.json"),
            r#"{"sonos": {"base_url": "http://localhost:5005"}}"#,
        )
        .unwrap();
        let cfg = load_proxy_config(tmp.path()).unwrap();
        assert_eq!(cfg.len(), 1);
        let sonos = cfg.get("sonos").unwrap();
        assert_eq!(sonos.base_url, "http://localhost:5005");
        assert!(sonos.auth.is_none());
    }

    #[test]
    fn load_config_parses_auth_block() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg_dir = tmp.path().join("data/config");
        std::fs::create_dir_all(&cfg_dir).unwrap();
        std::fs::write(
            cfg_dir.join("apis.json"),
            r#"{
                "comfort": {
                    "base_url": "https://accsmart.panasonic.com",
                    "auth": {
                        "type": "bearer",
                        "credential": "comfort-cloud"
                    }
                }
            }"#,
        )
        .unwrap();
        let cfg = load_proxy_config(tmp.path()).unwrap();
        let comfort = cfg.get("comfort").unwrap();
        match comfort.auth.as_ref().unwrap() {
            ProxyAuth::Bearer { credential } => assert_eq!(credential, "comfort-cloud"),
            other => panic!("expected Bearer, got {:?}", other),
        }
    }

    #[test]
    fn load_config_rejects_unknown_auth_type() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg_dir = tmp.path().join("data/config");
        std::fs::create_dir_all(&cfg_dir).unwrap();
        std::fs::write(
            cfg_dir.join("apis.json"),
            r#"{"foo": {"base_url": "https://x", "auth":
                {"type": "bearrer", "credential": "foo-key"}}}"#,
        )
        .unwrap();
        // Typo `bearrer` must surface at config-load time, not silently parse
        // as some default.
        assert!(load_proxy_config(tmp.path()).is_err());
    }

    #[test]
    fn load_config_parses_hmac_signed_auth() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg_dir = tmp.path().join("data/config");
        std::fs::create_dir_all(&cfg_dir).unwrap();
        std::fs::write(
            cfg_dir.join("apis.json"),
            r#"{"binance": {"base_url": "https://api.binance.com", "auth": {
                "type": "hmac_signed",
                "key_credential": "binance-key",
                "secret_credential": "binance-secret",
                "key_header": "X-MBX-APIKEY",
                "algorithm": "sha256",
                "signed_payload": "query_string",
                "signature_param": "signature",
                "timestamp_param": "timestamp"
            }}}"#,
        )
        .unwrap();
        let cfg = load_proxy_config(tmp.path()).unwrap();
        match cfg.get("binance").unwrap().auth.as_ref().unwrap() {
            ProxyAuth::HmacSigned {
                key_credential,
                secret_credential,
                key_header,
                algorithm,
                signed_payload,
                signature_param,
                timestamp_param,
            } => {
                assert_eq!(key_credential, "binance-key");
                assert_eq!(secret_credential, "binance-secret");
                assert_eq!(key_header, "X-MBX-APIKEY");
                assert_eq!(*algorithm, HmacAlgorithm::Sha256);
                assert_eq!(*signed_payload, HmacSignedPayload::QueryString);
                assert_eq!(signature_param, "signature");
                assert_eq!(timestamp_param.as_deref(), Some("timestamp"));
            }
            other => panic!("expected HmacSigned, got {:?}", other),
        }
    }

    #[test]
    fn load_config_hmac_signed_uses_defaults_for_optional_fields() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg_dir = tmp.path().join("data/config");
        std::fs::create_dir_all(&cfg_dir).unwrap();
        std::fs::write(
            cfg_dir.join("apis.json"),
            r#"{"x": {"base_url": "https://x", "auth": {
                "type": "hmac_signed",
                "key_credential": "k",
                "secret_credential": "s",
                "algorithm": "sha512",
                "signed_payload": "query_string"
            }}}"#,
        )
        .unwrap();
        let cfg = load_proxy_config(tmp.path()).unwrap();
        match cfg.get("x").unwrap().auth.as_ref().unwrap() {
            ProxyAuth::HmacSigned {
                key_header,
                algorithm,
                signature_param,
                timestamp_param,
                ..
            } => {
                assert_eq!(key_header, "X-API-KEY");
                assert_eq!(*algorithm, HmacAlgorithm::Sha512);
                assert_eq!(signature_param, "signature");
                assert!(timestamp_param.is_none());
            }
            other => panic!("expected HmacSigned, got {:?}", other),
        }
    }

    #[test]
    fn load_config_rejects_invalid_hmac_algorithm() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg_dir = tmp.path().join("data/config");
        std::fs::create_dir_all(&cfg_dir).unwrap();
        std::fs::write(
            cfg_dir.join("apis.json"),
            r#"{"x": {"base_url": "https://x", "auth": {
                "type": "hmac_signed",
                "key_credential": "k",
                "secret_credential": "s",
                "algorithm": "md5",
                "signed_payload": "query_string"
            }}}"#,
        )
        .unwrap();
        assert!(load_proxy_config(tmp.path()).is_err());
    }

    #[test]
    fn load_config_parses_credential_bundle_auth() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg_dir = tmp.path().join("data/config");
        std::fs::create_dir_all(&cfg_dir).unwrap();
        std::fs::write(
            cfg_dir.join("apis.json"),
            r#"{"comfort_creds": {"base_url": "", "auth": {
                "type": "credential_bundle",
                "credentials": ["comfort_username", "comfort_password"]
            }}}"#,
        )
        .unwrap();
        let cfg = load_proxy_config(tmp.path()).unwrap();
        match cfg.get("comfort_creds").unwrap().auth.as_ref().unwrap() {
            ProxyAuth::CredentialBundle { credentials } => {
                assert_eq!(
                    credentials,
                    &vec![
                        "comfort_username".to_string(),
                        "comfort_password".to_string(),
                    ]
                );
            }
            other => panic!("expected CredentialBundle, got {:?}", other),
        }
    }

    #[test]
    fn load_config_credential_bundle_allows_empty_base_url() {
        // Bundle proxies don't make HTTP requests, so base_url is meaningless
        // and the schema should still parse with an empty string.
        let tmp = tempfile::tempdir().unwrap();
        let cfg_dir = tmp.path().join("data/config");
        std::fs::create_dir_all(&cfg_dir).unwrap();
        std::fs::write(
            cfg_dir.join("apis.json"),
            r#"{"x": {"base_url": "", "auth": {"type": "credential_bundle", "credentials": ["a"]}}}"#,
        )
        .unwrap();
        assert!(load_proxy_config(tmp.path()).is_ok());
    }

    #[tokio::test]
    async fn build_credential_bundle_rejects_non_bundle_config() {
        // Lazy pool — the helper short-circuits before touching the DB.
        let pool = sqlx::PgPool::connect_lazy("postgres://invalid").expect("lazy pool");
        let config = ProxyConfig {
            base_url: "https://x".to_string(),
            auth: Some(ProxyAuth::Bearer {
                credential: "x".to_string(),
            }),
        };
        let err = build_credential_bundle(&pool, &config, "x")
            .await
            .expect_err("non-bundle config should error");
        assert_eq!(err.0, StatusCode::BAD_REQUEST);
        assert!(err.1.contains("credential_bundle"));
    }

    #[tokio::test]
    async fn build_credential_bundle_rejects_unauthenticated_config() {
        let pool = sqlx::PgPool::connect_lazy("postgres://invalid").expect("lazy pool");
        let config = ProxyConfig {
            base_url: "https://x".to_string(),
            auth: None,
        };
        let err = build_credential_bundle(&pool, &config, "x")
            .await
            .expect_err("unauthenticated config should error");
        assert_eq!(err.0, StatusCode::BAD_REQUEST);
    }

    #[test]
    fn load_config_parses_query_param_auth() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg_dir = tmp.path().join("data/config");
        std::fs::create_dir_all(&cfg_dir).unwrap();
        std::fs::write(
            cfg_dir.join("apis.json"),
            r#"{"helius": {"base_url": "https://mainnet.helius-rpc.com", "auth":
                {"type": "query_param", "credential": "helius-key", "param_name": "api-key"}}}"#,
        )
        .unwrap();
        let cfg = load_proxy_config(tmp.path()).unwrap();
        match cfg.get("helius").unwrap().auth.as_ref().unwrap() {
            ProxyAuth::QueryParam {
                credential,
                param_name,
            } => {
                assert_eq!(credential, "helius-key");
                assert_eq!(param_name, "api-key");
            }
            other => panic!("expected QueryParam, got {:?}", other),
        }
    }

    #[test]
    fn load_config_parses_header_override() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg_dir = tmp.path().join("data/config");
        std::fs::create_dir_all(&cfg_dir).unwrap();
        std::fs::write(
            cfg_dir.join("apis.json"),
            r#"{"foo": {"base_url": "https://x", "auth":
                {"type": "api_key", "credential": "foo-key", "header": "X-API-Key"}}}"#,
        )
        .unwrap();
        let cfg = load_proxy_config(tmp.path()).unwrap();
        match cfg.get("foo").unwrap().auth.as_ref().unwrap() {
            ProxyAuth::ApiKey { credential, header } => {
                assert_eq!(credential, "foo-key");
                assert_eq!(header.as_deref(), Some("X-API-Key"));
            }
            other => panic!("expected ApiKey, got {:?}", other),
        }
    }

    #[test]
    fn load_config_invalid_json_returns_err() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg_dir = tmp.path().join("data/config");
        std::fs::create_dir_all(&cfg_dir).unwrap();
        std::fs::write(cfg_dir.join("apis.json"), "{ not json").unwrap();
        assert!(load_proxy_config(tmp.path()).is_err());
    }

    // ---- Integration tests with a tiny upstream server --------------------
    //
    // For these, we spin up an axum server on 127.0.0.1:0 (random port) and
    // exercise `forward_request` directly — no AppState/database needed.

    use std::sync::Arc;
    use std::sync::Mutex;

    /// Records what the upstream observed for assertion.
    #[derive(Default, Clone)]
    struct UpstreamRecord {
        method: String,
        path: String,
        query: String,
        body: Vec<u8>,
        headers: Vec<(String, String)>,
    }

    type RecordSlot = Arc<Mutex<Option<UpstreamRecord>>>;

    /// Spawn an upstream that records the incoming request and replies with
    /// `status` and `body`. Returns `(base_url, slot)`.
    async fn spawn_recording_upstream(status: u16, body: &'static str) -> (String, RecordSlot) {
        let slot: RecordSlot = Arc::new(Mutex::new(None));
        let slot_clone = slot.clone();
        let app = Router::new().fallback(any(move |req: axum::extract::Request| {
            let slot = slot_clone.clone();
            async move {
                let method = req.method().to_string();
                let path = req.uri().path().to_string();
                let query = req.uri().query().unwrap_or("").to_string();
                let headers: Vec<(String, String)> = req
                    .headers()
                    .iter()
                    .map(|(n, v)| (n.as_str().to_string(), v.to_str().unwrap_or("").to_string()))
                    .collect();
                let req_body = axum::body::to_bytes(req.into_body(), 1024 * 1024)
                    .await
                    .unwrap_or_default()
                    .to_vec();
                *slot.lock().unwrap() = Some(UpstreamRecord {
                    method,
                    path,
                    query,
                    body: req_body,
                    headers,
                });
                (
                    StatusCode::from_u16(status).unwrap_or(StatusCode::OK),
                    body.to_string(),
                )
            }
        }));

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        (format!("http://{}", addr), slot)
    }

    /// Read the full body of a `Response` into a `String`.
    async fn body_text(resp: Response) -> String {
        let bytes = axum::body::to_bytes(resp.into_body(), 1024 * 1024)
            .await
            .unwrap();
        String::from_utf8_lossy(&bytes).to_string()
    }

    async fn run_method_test(method: Method, body: &str) {
        let (base, slot) = spawn_recording_upstream(200, "ok").await;
        let url = format!("{}/path/sub", base);
        let resp = forward_request(
            method.clone(),
            &url,
            &url,
            HeaderMap::new(),
            None,
            Bytes::copy_from_slice(body.as_bytes()),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
        let recorded = slot.lock().unwrap().clone().expect("upstream not called");
        assert_eq!(recorded.method, method.as_str());
        assert_eq!(recorded.path, "/path/sub");
        assert_eq!(recorded.body, body.as_bytes());
    }

    #[tokio::test]
    async fn forwards_get() {
        run_method_test(Method::GET, "").await;
    }

    #[tokio::test]
    async fn forwards_post_with_body() {
        run_method_test(Method::POST, r#"{"hello":"world"}"#).await;
    }

    #[tokio::test]
    async fn forwards_put_with_body() {
        run_method_test(Method::PUT, "raw bytes").await;
    }

    #[tokio::test]
    async fn forwards_delete() {
        run_method_test(Method::DELETE, "").await;
    }

    #[tokio::test]
    async fn forwards_patch_with_body() {
        run_method_test(Method::PATCH, "patch-body").await;
    }

    #[tokio::test]
    async fn upstream_does_not_see_stripped_headers() {
        let (base, slot) = spawn_recording_upstream(200, "ok").await;
        let url = format!("{}/x", base);
        let headers = hm(&[
            ("Cookie", "session=abc"),
            ("Origin", "https://engine.local"),
            ("Referer", "https://engine.local/"),
            ("X-Keep-Me", "yes"),
        ]);
        let _ = forward_request(Method::GET, &url, &url, headers, None, Bytes::new()).await;
        let recorded = slot.lock().unwrap().clone().unwrap();
        let observed: Vec<&str> = recorded.headers.iter().map(|(n, _)| n.as_str()).collect();
        assert!(!observed.iter().any(|n| n.eq_ignore_ascii_case("cookie")));
        assert!(!observed.iter().any(|n| n.eq_ignore_ascii_case("origin")));
        assert!(!observed.iter().any(|n| n.eq_ignore_ascii_case("referer")));
        assert!(observed.iter().any(|n| n.eq_ignore_ascii_case("x-keep-me")));
    }

    #[tokio::test]
    async fn upstream_does_not_see_host_header_from_engine() {
        // We forward the request with no Host header — reqwest sets one from
        // the URL (i.e. the upstream's host, not the engine's). This proves
        // a Host: engine.example.com sent by the browser doesn't bleed through.
        let (base, slot) = spawn_recording_upstream(200, "ok").await;
        let url = format!("{}/x", base);
        let headers = hm(&[("Host", "engine.example.com")]);
        let _ = forward_request(Method::GET, &url, &url, headers, None, Bytes::new()).await;
        let recorded = slot.lock().unwrap().clone().unwrap();
        let host = recorded
            .headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case("host"))
            .map(|(_, v)| v.as_str())
            .unwrap_or("");
        // The upstream's Host header (set by reqwest) should be the upstream's
        // address — it must not be `engine.example.com`.
        assert_ne!(host, "engine.example.com");
    }

    #[tokio::test]
    async fn injects_bearer_auth_header() {
        let (base, slot) = spawn_recording_upstream(200, "ok").await;
        let url = format!("{}/x", base);
        let auth = build_auth_header(AuthType::Bearer, "tok-xyz", None);
        let _ = forward_request(Method::GET, &url, &url, HeaderMap::new(), auth, Bytes::new()).await;
        let recorded = slot.lock().unwrap().clone().unwrap();
        let authz = recorded
            .headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case("authorization"))
            .map(|(_, v)| v.as_str())
            .expect("authorization header missing");
        assert_eq!(authz, "Bearer tok-xyz");
    }

    #[tokio::test]
    async fn injects_api_key_with_custom_header() {
        let (base, slot) = spawn_recording_upstream(200, "ok").await;
        let url = format!("{}/x", base);
        let auth = build_auth_header(AuthType::ApiKey, "secret-key", Some("X-API-Key"));
        let _ = forward_request(Method::GET, &url, &url, HeaderMap::new(), auth, Bytes::new()).await;
        let recorded = slot.lock().unwrap().clone().unwrap();
        let key = recorded
            .headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case("x-api-key"))
            .map(|(_, v)| v.as_str())
            .expect("x-api-key header missing");
        assert_eq!(key, "secret-key");
    }

    #[tokio::test]
    async fn forwards_query_param_auth_to_upstream() {
        let (base, slot) = spawn_recording_upstream(200, "ok").await;
        let url = append_query_param(&format!("{}/v1/items", base), "api-key", "secret-123");
        let _ = forward_request(Method::GET, &url, &url, HeaderMap::new(), None, Bytes::new()).await;
        let recorded = slot.lock().unwrap().clone().unwrap();
        assert_eq!(recorded.path, "/v1/items");
        assert_eq!(recorded.query, "api-key=secret-123");
    }

    #[tokio::test]
    async fn forwards_query_param_auth_preserves_existing_query() {
        let (base, slot) = spawn_recording_upstream(200, "ok").await;
        let url = append_query_param(
            &format!("{}/v1/items?limit=10", base),
            "api-key",
            "secret-123",
        );
        let _ = forward_request(Method::GET, &url, &url, HeaderMap::new(), None, Bytes::new()).await;
        let recorded = slot.lock().unwrap().clone().unwrap();
        assert_eq!(recorded.query, "limit=10&api-key=secret-123");
    }

    #[tokio::test]
    async fn upstream_5xx_passes_through() {
        let (base, _slot) = spawn_recording_upstream(503, "down").await;
        let url = format!("{}/x", base);
        let resp = forward_request(Method::GET, &url, &url, HeaderMap::new(), None, Bytes::new()).await;
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(body_text(resp).await, "down");
    }

    #[tokio::test]
    async fn upstream_unreachable_returns_502() {
        // Bind a port and drop the listener — the OS keeps the address
        // briefly unreachable, but on most OSes the connect call fails fast
        // with ECONNREFUSED. We pick port 1 (privileged, never bound by us).
        let resp = forward_request(
            Method::GET,
            "http://127.0.0.1:1/nope",
            "http://127.0.0.1:1/nope",
            HeaderMap::new(),
            None,
            Bytes::new(),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::BAD_GATEWAY);
    }
}
