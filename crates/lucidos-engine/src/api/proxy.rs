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

use crate::api::proxy_hex::hex_lower;
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
/// HTTP backends).
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
/// `fetch_required_credential`, but for callers that only need the secret string.
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
pub(super) async fn proxy_modules_reload(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Response {
    match reload_proxy_modules_into(&state.engine, &state.workspace_path).await {
        Ok(names) => {
            let count = names.len();
            state
                .engine
                .event_bus
                .emit_user_system(
                    &headers,
                    &state.pool,
                    "[Proxy] ProxyModulesReloaded",
                    |actor| crate::engine::event_bus::SystemEvent::ProxyModulesReloaded {
                        count,
                        names: names.clone(),
                        actor,
                    },
                )
                .await;
            Json(serde_json::json!({"loaded": names})).into_response()
        }
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

/// Routes for the generic API proxy — forwards to a backend configured in
/// `data/config/apis.json`. Two root routes so callers can hit
/// `/proxy/sonos` (no trailing path) as well as `/proxy/sonos/play/2`.
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
