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

/// Auth block — names a credential in the engine credential store and
/// optionally overrides the header name (for `api_key`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProxyAuth {
    /// Auth scheme. Serde's `snake_case` rename matches the JSON spelling
    /// (`"bearer"`, `"api_key"`, `"basic"`) and rejects typos at config-load
    /// time instead of failing silently at request time.
    #[serde(rename = "type")]
    pub auth_type: AuthType,
    /// The `service_name` in the credential store.
    pub credential: String,
    /// Optional header name override (default `Authorization`). Useful for
    /// `api_key` services that expect e.g. `X-API-Key`.
    #[serde(default)]
    pub header: Option<String>,
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
    serde_json::from_str(&content)
        .map_err(|e| format!("Failed to parse {}: {}", path.display(), e))
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

/// Resolve config name → ProxyConfig. Returns 404 if name not configured.
pub(crate) async fn resolve_proxy_target(
    workspace_path: &FsPath,
    name: &str,
) -> Result<ProxyConfig, (StatusCode, String)> {
    let configs = load_proxy_config(workspace_path)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;
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
    match CredentialStore::get(pool, &auth.credential).await {
        Ok(Some(c)) if c.auth_value.is_empty() => Err((
            StatusCode::BAD_GATEWAY,
            format!(
                "proxy auth credential '{}' has an empty value",
                auth.credential
            ),
        )),
        Ok(Some(c)) => Ok(c),
        Ok(None) => Err((
            StatusCode::BAD_GATEWAY,
            format!(
                "proxy auth refers to credential '{}' which is not in the store",
                auth.credential
            ),
        )),
        Err(e) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("failed to read credential '{}': {}", auth.credential, e),
        )),
    }
}

/// Forward a request to the configured upstream and return the upstream
/// response (headers + body + status). Pure with respect to AppState: takes
/// only the data it needs, so the integration tests can spin up a tiny axum
/// server and exercise this directly.
pub async fn forward_request(
    method: Method,
    target_url: &str,
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
            log!("[Proxy] forward to {} failed: {:?}", target_url, e);
            if e.is_timeout() {
                (StatusCode::GATEWAY_TIMEOUT, format!("upstream timeout: {}", e)).into_response()
            } else {
                (StatusCode::BAD_GATEWAY, format!("upstream error: {}", e)).into_response()
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

    let auth_header = if let Some(auth) = &config.auth {
        let cred = match resolve_credential(&state.pool, auth).await {
            Ok(c) => c,
            Err((status, msg)) => return (status, msg).into_response(),
        };
        build_auth_header(cred.auth_type, &cred.auth_value, auth.header.as_deref())
    } else {
        None
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

    let target_url = build_target_url(&config.base_url, &path, query.as_deref());
    forward_request(method, &target_url, headers, auth_header, body).await
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
            build_target_url("http://api.example.com", "/v1/items", Some("limit=10&page=2")),
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
        let auth = comfort.auth.as_ref().unwrap();
        assert_eq!(auth.auth_type, AuthType::Bearer);
        assert_eq!(auth.credential, "comfort-cloud");
        assert!(auth.header.is_none());
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
        let auth = cfg.get("foo").unwrap().auth.as_ref().unwrap();
        assert_eq!(auth.header.as_deref(), Some("X-API-Key"));
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
        let _ = forward_request(Method::GET, &url, headers, None, Bytes::new()).await;
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
        let _ = forward_request(Method::GET, &url, headers, None, Bytes::new()).await;
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
        let _ = forward_request(Method::GET, &url, HeaderMap::new(), auth, Bytes::new()).await;
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
        let _ = forward_request(Method::GET, &url, HeaderMap::new(), auth, Bytes::new()).await;
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
    async fn upstream_5xx_passes_through() {
        let (base, _slot) = spawn_recording_upstream(503, "down").await;
        let url = format!("{}/x", base);
        let resp = forward_request(Method::GET, &url, HeaderMap::new(), None, Bytes::new()).await;
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
            HeaderMap::new(),
            None,
            Bytes::new(),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::BAD_GATEWAY);
    }
}
