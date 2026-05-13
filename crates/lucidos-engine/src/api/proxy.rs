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

use crate::api::proxy_pipeline_config::{LayerConfig, PipelineConfig};
use crate::core::{Credential, CredentialStore};
use axum::body::{Body, Bytes};
use axum::http::{HeaderName, Method};
use std::collections::HashMap;
use std::path::Path as FsPath;
use std::sync::Arc;
use std::time::Duration;

/// Per-API config entry from `data/config/apis.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProxyConfig {
    pub base_url: String,
    /// Pipeline of `AuthLayer` impls applied in order on every outbound
    /// request. The legacy single-variant `ProxyAuth` shape is upgraded
    /// to a pipeline at engine startup by
    /// `proxy_migration::migrate_apis_json_if_needed`.
    #[serde(default)]
    pub auth: Option<PipelineConfig>,
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

pub type ProxyConfigMap = HashMap<String, ProxyConfig>;

const PROXY_CONFIG_REL_PATH: &str = "data/config/apis.json";

/// Shared client — pooled, no proxy, accepts self-signed certs (for local
/// HTTP backends). Mirrors `dev_proxy::CLIENT` so behavior is consistent.
///
/// `redirect(Policy::none())`: signed proxy requests must NOT auto-follow
/// redirects. reqwest would replay the original Authorization header /
/// signed query against whatever host upstream points us at — leaking
/// credentials to a hostile target, or producing an invalid signature
/// for the redirect URL. Engine-side handling lives in
/// `forward_with_redirects`: same-host hops re-run the pipeline against
/// the new path/query; cross-host hops return 502.
static CLIENT: std::sync::LazyLock<reqwest::Client> = std::sync::LazyLock::new(|| {
    reqwest::Client::builder()
        .no_proxy()
        .danger_accept_invalid_certs(true)
        .pool_max_idle_per_host(5)
        .pool_idle_timeout(Duration::from_secs(30))
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(30))
        .redirect(reqwest::redirect::Policy::none())
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
    let configs: ProxyConfigMap = serde_json::from_str(&content)
        .map_err(|e| format!("Failed to parse {}: {}", path.display(), e))?;
    // Walk the pipeline of every provider — validate any ScriptHandshake
    // layer's script path before the engine can be tricked into running an
    // out-of-workspace file. Catches malicious / sloppy `apis.json` edits
    // at startup instead of waiting for the first request.
    for (name, cfg) in &configs {
        let Some(pipeline_cfg) = cfg.auth.as_ref() else {
            continue;
        };
        for layer in &pipeline_cfg.pipeline {
            if let LayerConfig::ScriptHandshake { script, .. } = layer {
                if has_traversal(script) || script.starts_with('/') || script.starts_with('\\') {
                    return Err(format!(
                        "proxy '{}' script path '{}' must be relative under the workspace (no '..', no leading '/' or '\\\\')",
                        name, script
                    ));
                }
            }
        }
    }
    Ok(configs)
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

/// Look up a single credential by service name, mapping missing/empty/db
/// failures to the same `(StatusCode, String)` error shape every auth path
/// uses. The single source of truth for "this credential must exist and be
/// non-empty, or the proxy can't proceed".
pub(crate) async fn fetch_required_credential(
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
    auth_headers: Vec<(HeaderName, HeaderValue)>,
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
    for (name, value) in &auth_headers {
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

/// Re-scan `<workspace>/data/auth-modules/` and atomically swap the
/// engine's compiled-module map. Returns the sorted list of module names
/// now loaded. Shared by:
///  - the HTTP `POST /api/v1/proxy-modules/reload` endpoint,
///  - the `reload_proxy_modules` LLM tool,
///  - the post-install reload triggered when `install_plugin` writes any
///    `auth-modules/` content.
///
/// In-flight pipeline runs holding an `Arc<CompiledModule>` finish on the
/// old module; new runs see the new map.
pub(crate) async fn reload_proxy_modules_into(
    engine: &crate::engine::LucidosEngine,
    workspace_path: &FsPath,
) -> Result<Vec<String>, String> {
    let dir = workspace_path.join("data/auth-modules");
    let new_modules =
        crate::api::proxy_wasm_signer::load_wasm_modules(&dir, engine.wasm_engine())?;
    let mut names: Vec<String> = new_modules.keys().cloned().collect();
    names.sort();
    *engine.proxy_modules().write().await = new_modules;
    log!(
        "[Proxy] reloaded WASM auth modules ({}): {:?}",
        names.len(),
        names
    );
    Ok(names)
}

/// Axum handler: `POST /api/v1/proxy-modules/reload` — re-scan the WASM
/// auth modules directory and swap the engine's compiled-module map
/// atomically. Returns the names now available so the caller (HTTP or
/// LLM tool) can confirm what's loaded.
pub(super) async fn proxy_modules_reload(State(state): State<AppState>) -> Response {
    match reload_proxy_modules_into(&state.engine, &state.workspace_path).await {
        Ok(names) => Json(serde_json::json!({"loaded": names})).into_response(),
        Err(e) => {
            log!("[Proxy] reload of WASM auth modules failed: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("reload failed: {e}"),
            )
                .into_response()
        }
    }
}

/// Maximum total HTTP hops (initial request + redirects) we'll follow on a
/// signed proxy request. Five is the same cap reqwest's default redirect
/// policy uses, but we enforce it ourselves so the auth pipeline gets a
/// chance to re-run on each hop instead of reqwest replaying the original
/// signature against whatever the upstream redirects to.
const MAX_REDIRECT_HOPS: usize = 5;

/// True for the 30x statuses we follow.
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

/// Lowercased host of the URL, or `None` if the URL doesn't parse / has
/// no host. Used to decide whether a redirect target is "the same host"
/// the original request was bound to.
fn host_of(url: &str) -> Option<String> {
    reqwest::Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(|h| h.to_ascii_lowercase()))
}

/// Resolve a `Location` header value (relative or absolute) against the
/// URL we just hit, returning the absolute target URL.
fn resolve_redirect_location(current_url: &str, location: &str) -> Option<reqwest::Url> {
    let base = reqwest::Url::parse(current_url).ok()?;
    base.join(location).ok()
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

    match dispatch_proxy_request(&state.engine, &name, &config, method, path, query, headers, body)
        .await
    {
        Ok(resp) => resp,
        Err((status, msg)) => (status, msg).into_response(),
    }
}

/// Top-level proxy dispatch — exposed so the `proxy_request` LLM tool can
/// share the same code path as the HTTP handler. Builds the per-request
/// pipeline, forwards (with same-host redirect re-signing), and on 401
/// from upstream invalidates opted-in caches and re-runs the SAME layers
/// (the cache invalidation is what changes between attempts).
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
    // Build layers once. The 401-retry path reuses the same layers and
    // just calls invalidate_cache on the opted-in ones — re-fetching
    // every credential and re-instantiating WASM signers would be wasted
    // work.
    let layers = build_pipeline_layers(engine, name, config).await?;

    let (response, outcome) =
        forward_with_redirects(name, &config.base_url, &layers, &method, &path, query.as_deref(), &headers, &body).await?;

    if !crate::api::proxy_pipeline::pipeline_should_retry(&layers, &outcome, response.status()) {
        return Ok(response);
    }
    crate::api::proxy_pipeline::pipeline_invalidate_for_retry(&layers, &outcome).await;
    let (response, _) =
        forward_with_redirects(name, &config.base_url, &layers, &method, &path, query.as_deref(), &headers, &body).await?;
    Ok(response)
}

/// Build the per-request layer pipeline from a `ProxyConfig`. No-op when
/// `config.auth` is `None` (returns an empty `Vec`).
async fn build_pipeline_layers(
    engine: &Arc<crate::engine::LucidosEngine>,
    name: &str,
    config: &ProxyConfig,
) -> Result<Vec<Arc<dyn crate::api::proxy_auth_layer::AuthLayer>>, (StatusCode, String)> {
    let Some(pipeline_cfg) = config.auth.as_ref() else {
        return Ok(Vec::new());
    };
    // Snapshot the module map and drop the read guard before the (async)
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
        proxy_modules: &modules_snapshot,
        wasm_engine: engine.wasm_engine().clone(),
    };
    crate::api::proxy_pipeline_builder::build_pipeline(pipeline_cfg, &ctx).await
}

/// Build the layer pipeline + forward + follow same-host redirects (re-
/// running the same `layers` on each hop (HMAC layers sign over the
/// per-hop query, QueryParam appends to the URL — re-running gets the
/// per-hop URL right, while sharing the layers means the script-handshake
/// cache-hit on hop 1 stays a hit on hop 2). Cross-host redirects are
/// refused with 502.
///
/// Returns the final response and the `PipelineOutcome` from the first
/// hop (which is what the 401-retry decision inspects).
#[allow(clippy::too_many_arguments)]
async fn forward_with_redirects(
    name: &str,
    base_url: &str,
    layers: &[Arc<dyn crate::api::proxy_auth_layer::AuthLayer>],
    method: &Method,
    initial_path: &str,
    initial_query: Option<&str>,
    headers: &HeaderMap,
    body: &Bytes,
) -> Result<(Response, crate::api::proxy_pipeline::PipelineOutcome), (StatusCode, String)> {
    // Auth is bound to the initial host — cross-host redirects later are
    // refused with 502.
    let initial_url = build_target_url(base_url, initial_path, initial_query);
    let initial_host = host_of(&initial_url).ok_or_else(|| {
        (
            StatusCode::BAD_GATEWAY,
            format!("proxy '{name}' base_url has no host (cannot bind auth to upstream)"),
        )
    })?;

    let mut current_path = initial_path.to_string();
    let mut current_query: Option<String> = initial_query.map(|s| s.to_string());
    let mut first_outcome: Option<crate::api::proxy_pipeline::PipelineOutcome> = None;
    let mut hops = 0usize;

    loop {
        let target_url = build_target_url(base_url, &current_path, current_query.as_deref());

        let outcome =
            crate::api::proxy_pipeline::run_pipeline(layers, method, &target_url, &[], body)
                .await?;

        let body_for_send = outcome.replace_body.clone().unwrap_or_else(|| body.clone());
        let auth_headers: Vec<(HeaderName, HeaderValue)> = outcome
            .headers
            .iter()
            .filter_map(|(n, v)| HeaderValue::from_str(v).ok().map(|v| (n.clone(), v)))
            .collect();
        // Final URL for this hop = target + pipeline-added query params,
        // URL-encoded. Layers return raw values; engine handles encoding.
        let final_url = merge_query_params(&target_url, &outcome.query);

        let response = forward_request(
            method.clone(),
            &final_url,
            &outcome.log_url,
            headers.clone(),
            auth_headers,
            body_for_send,
        )
        .await;

        if first_outcome.is_none() {
            first_outcome = Some(outcome);
        }

        if !is_redirect_status(response.status()) {
            return Ok((response, first_outcome.unwrap()));
        }

        if hops >= MAX_REDIRECT_HOPS - 1 {
            log!(
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
                format!("proxy '{name}' upstream returned redirect with unparseable Location: {location}"),
            )
        })?;
        let target_host = target
            .host_str()
            .map(|h| h.to_ascii_lowercase())
            .ok_or_else(|| {
                (
                    StatusCode::BAD_GATEWAY,
                    format!("proxy '{name}' upstream redirected to a Location with no host: {location}"),
                )
            })?;
        if target_host != initial_host {
            log!(
                "[Proxy] {} returned redirect to {} but auth was bound to {} — refused",
                name,
                target_host,
                initial_host
            );
            return Err((
                StatusCode::BAD_GATEWAY,
                format!("proxy '{name}' redirected to a different host ({target_host}); refusing to re-sign for an unconfigured upstream"),
            ));
        }
        current_path = target.path().trim_start_matches('/').to_string();
        current_query = target.query().map(|q| q.to_string());
        hops += 1;
    }
}

/// Append `pairs` (already URL-decoded values) to `url`'s query string,
/// URL-encoding values along the way. The pipeline's `QueryParamLayer`
/// publishes a `log_url_replacement` output with the credential redacted;
/// the runner aggregates that into `PipelineOutcome.log_url`. So this
/// function is concerned only with the FORWARDED URL — it must inject
/// real values, never `REDACTED`.
fn merge_query_params(url: &str, pairs: &[(String, String)]) -> String {
    let mut out = url.to_string();
    for (k, v) in pairs {
        out = append_query_param(&out, k, v);
    }
    out
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

    // Auth header building (Bearer/ApiKey/Basic) moved to AuthLayer impls
    // — see proxy_static_layers tests for equivalent coverage.

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

    // ---- Redirect helpers --------------------------------------------

    #[test]
    fn host_of_extracts_lowercased_host() {
        assert_eq!(
            host_of("https://API.Example.com/path"),
            Some("api.example.com".to_string())
        );
        assert_eq!(
            host_of("http://localhost:8080/x"),
            Some("localhost".to_string())
        );
    }

    #[test]
    fn host_of_returns_none_for_unparseable_url() {
        assert!(host_of("not a url").is_none());
    }

    #[test]
    fn resolve_redirect_handles_absolute_target() {
        let url = resolve_redirect_location(
            "https://example.com/a/b",
            "https://example.com/c/d?q=1",
        )
        .unwrap();
        assert_eq!(url.as_str(), "https://example.com/c/d?q=1");
    }

    #[test]
    fn resolve_redirect_handles_relative_target() {
        let url = resolve_redirect_location("https://example.com/a/b", "/c/d").unwrap();
        assert_eq!(url.as_str(), "https://example.com/c/d");
    }

    #[test]
    fn resolve_redirect_handles_protocol_relative_target() {
        let url = resolve_redirect_location("https://example.com/a", "//other.com/x").unwrap();
        assert_eq!(url.as_str(), "https://other.com/x");
    }

    #[test]
    fn is_redirect_status_covers_30x_we_follow() {
        assert!(is_redirect_status(StatusCode::MOVED_PERMANENTLY));
        assert!(is_redirect_status(StatusCode::FOUND));
        assert!(is_redirect_status(StatusCode::SEE_OTHER));
        assert!(is_redirect_status(StatusCode::TEMPORARY_REDIRECT));
        assert!(is_redirect_status(StatusCode::PERMANENT_REDIRECT));
        // 304 is not really a redirect — we don't follow.
        assert!(!is_redirect_status(StatusCode::NOT_MODIFIED));
        assert!(!is_redirect_status(StatusCode::OK));
        assert!(!is_redirect_status(StatusCode::BAD_GATEWAY));
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

    // Query-string assembly (sign_query_string) used to live here as a
    // standalone helper; with HmacSignedLayer doing the assembly inline,
    // the equivalent tests now live in proxy_hmac_layer (sign_with*).

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

    // Legacy `auth.type` (Bearer/ApiKey/Basic/QueryParam/HmacSigned/
    // ScriptHandshake) parsing tests + ScriptHandshake-cache integration
    // were deleted with the legacy `ProxyAuth` enum. Equivalent coverage:
    //   - on-disk pipeline shape: proxy_pipeline_config tests
    //   - per-layer behavior: proxy_static_layers / proxy_hmac_layer /
    //     proxy_script_layer / proxy_wasm_signer tests
    //   - 401 retry decision: proxy_pipeline retry-truth-table tests
    //   - removed credential_bundle: proxy_migration negative-guard test
    //   - upgrade migration of legacy apis.json: proxy_migration tests

    #[test]
    fn load_config_parses_pipeline_shape_with_static_credential_layer() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg_dir = tmp.path().join("data/config");
        std::fs::create_dir_all(&cfg_dir).unwrap();
        std::fs::write(
            cfg_dir.join("apis.json"),
            r#"{
                "comfort": {
                    "base_url": "https://accsmart.panasonic.com",
                    "auth": {"pipeline": [
                        {"type": "static_credential", "kind": "bearer", "credential": "comfort-cloud"}
                    ]}
                }
            }"#,
        )
        .unwrap();
        let cfg = load_proxy_config(tmp.path()).unwrap();
        let comfort = cfg.get("comfort").unwrap();
        let pipeline = comfort.auth.as_ref().unwrap();
        assert_eq!(pipeline.pipeline.len(), 1);
    }

    #[test]
    fn load_config_rejects_unknown_layer_type_in_pipeline() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg_dir = tmp.path().join("data/config");
        std::fs::create_dir_all(&cfg_dir).unwrap();
        std::fs::write(
            cfg_dir.join("apis.json"),
            r#"{"foo": {"base_url": "https://x", "auth": {"pipeline": [
                {"type": "bearrer", "credential": "k"}
            ]}}}"#,
        )
        .unwrap();
        // Typo `bearrer` must surface at config-load time.
        assert!(load_proxy_config(tmp.path()).is_err());
    }

    #[test]
    fn load_config_rejects_script_handshake_with_traversal_path_in_pipeline() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg_dir = tmp.path().join("data/config");
        std::fs::create_dir_all(&cfg_dir).unwrap();
        std::fs::write(
            cfg_dir.join("apis.json"),
            r#"{"x": {"base_url": "https://x", "auth": {"pipeline": [
                {"type": "script_handshake", "credential": "x", "script": "../../../etc/passwd"}
            ]}}}"#,
        )
        .unwrap();
        assert!(load_proxy_config(tmp.path()).is_err());
    }

    #[test]
    fn load_config_rejects_script_handshake_with_absolute_path_in_pipeline() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg_dir = tmp.path().join("data/config");
        std::fs::create_dir_all(&cfg_dir).unwrap();
        std::fs::write(
            cfg_dir.join("apis.json"),
            r#"{"x": {"base_url": "https://x", "auth": {"pipeline": [
                {"type": "script_handshake", "credential": "x", "script": "/etc/passwd"}
            ]}}}"#,
        )
        .unwrap();
        assert!(load_proxy_config(tmp.path()).is_err());
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
        let shutdown = Arc::new(tokio::sync::Notify::new());
        let shutdown_from_handler = shutdown.clone();
        let app = Router::new().fallback(any(move |req: axum::extract::Request| {
            let slot = slot_clone.clone();
            let shutdown = shutdown_from_handler.clone();
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
                shutdown.notify_one();
                (
                    StatusCode::from_u16(status).unwrap_or(StatusCode::OK),
                    body.to_string(),
                )
            }
        }));

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(async move {
                    shutdown.notified().await;
                })
                .await
                .unwrap();
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
            Vec::new(),
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
        let _ = forward_request(Method::GET, &url, &url, headers, Vec::new(), Bytes::new()).await;
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
        let _ = forward_request(Method::GET, &url, &url, headers, Vec::new(), Bytes::new()).await;
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
    async fn forwards_arbitrary_auth_headers_to_upstream() {
        // Smoke-tests forward_request's auth_headers parameter. The actual
        // header construction lives in BearerLayer / ApiKeyLayer (with
        // their own tests); this confirms forward_request actually puts
        // them on the wire.
        let (base, slot) = spawn_recording_upstream(200, "ok").await;
        let url = format!("{}/x", base);
        let auth_vec: Vec<(HeaderName, HeaderValue)> = vec![
            (
                HeaderName::from_static("authorization"),
                HeaderValue::from_static("Bearer tok-xyz"),
            ),
            (
                HeaderName::from_static("x-api-key"),
                HeaderValue::from_static("secret-key"),
            ),
        ];
        let _ = forward_request(Method::GET, &url, &url, HeaderMap::new(), auth_vec, Bytes::new())
            .await;
        let recorded = slot.lock().unwrap().clone().unwrap();
        let authz = recorded
            .headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case("authorization"))
            .map(|(_, v)| v.as_str());
        let key = recorded
            .headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case("x-api-key"))
            .map(|(_, v)| v.as_str());
        assert_eq!(authz, Some("Bearer tok-xyz"));
        assert_eq!(key, Some("secret-key"));
    }

    #[tokio::test]
    async fn forwards_query_param_auth_to_upstream() {
        let (base, slot) = spawn_recording_upstream(200, "ok").await;
        let url = append_query_param(&format!("{}/v1/items", base), "api-key", "secret-123");
        let _ = forward_request(Method::GET, &url, &url, HeaderMap::new(), Vec::new(), Bytes::new()).await;
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
        let _ = forward_request(Method::GET, &url, &url, HeaderMap::new(), Vec::new(), Bytes::new()).await;
        let recorded = slot.lock().unwrap().clone().unwrap();
        assert_eq!(recorded.query, "limit=10&api-key=secret-123");
    }

    #[tokio::test]
    async fn upstream_5xx_passes_through() {
        let (base, _slot) = spawn_recording_upstream(503, "down").await;
        let url = format!("{}/x", base);
        let resp = forward_request(Method::GET, &url, &url, HeaderMap::new(), Vec::new(), Bytes::new()).await;
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(body_text(resp).await, "down");
    }

    /// Spawn an upstream that responds with `status` + `Location` header.
    /// Used by Phase-8 redirect tests below.
    async fn spawn_redirecting_upstream(status: u16, location: &'static str) -> String {
        let shutdown = Arc::new(tokio::sync::Notify::new());
        let shutdown_from_handler = shutdown.clone();
        let app = Router::new().route(
            "/*path",
            any(move || {
                let location = location;
                let shutdown = shutdown_from_handler.clone();
                async move {
                    let mut resp = (
                        StatusCode::from_u16(status).unwrap_or(StatusCode::FOUND),
                        "redirect",
                    )
                        .into_response();
                    resp.headers_mut().insert(
                        axum::http::header::LOCATION,
                        HeaderValue::from_str(location).unwrap(),
                    );
                    shutdown.notify_one();
                    resp
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(async move {
                    shutdown.notified().await;
                })
                .await
                .unwrap();
        });
        format!("http://{}", addr)
    }


    #[tokio::test]
    async fn forward_request_does_not_auto_follow_30x() {
        // The shared CLIENT sets `redirect(Policy::none())` so signed
        // proxy requests never silently follow upstream redirects without
        // re-running the auth pipeline. Verify by pointing forward_request
        // at a 302 upstream and asserting we get the 302 back instead of
        // the redirect target's body.
        let base = spawn_redirecting_upstream(302, "https://example.invalid/never-fetched").await;
        let url = format!("{}/start", base);
        let resp = forward_request(
            Method::GET,
            &url,
            &url,
            HeaderMap::new(),
            Vec::new(),
            Bytes::new(),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::FOUND);
        assert_eq!(
            resp.headers()
                .get(axum::http::header::LOCATION)
                .and_then(|v| v.to_str().ok()),
            Some("https://example.invalid/never-fetched")
        );
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
            Vec::new(),
            Bytes::new(),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::BAD_GATEWAY);
    }
}
