//! Generic API proxy for HTTPS-iframe apps that need to call HTTP backends.
//!
//! Apps mount inside an HTTPS iframe (the engine port). The browser blocks
//! direct calls to `http://localhost:5005` from such pages (mixed content),
//! and CORS gets in the way of cross-origin XHR. This module forwards
//! requests through the engine to a configured backend, optionally injecting
//! an auth header sourced from the credential store.
//!
//! Configured via `data/config/apis.json`. `auth` is a *pipeline* of layers
//! (see `proxy_pipeline_config::PipelineConfig`); the legacy single-variant
//! `{"type": "bearer", "credential": …}` shape no longer deserializes and is
//! rewritten in place at startup by
//! `proxy_migration::migrate_apis_json_if_needed`:
//! ```json
//! {
//!   "sonos":   { "base_url": "http://localhost:5005" },
//!   "comfort": { "base_url": "https://accsmart.panasonic.com",
//!                "auth": { "pipeline": [
//!                  { "type": "static_credential",
//!                    "kind": "bearer",
//!                    "credential": "comfort-cloud" }
//!                ] } }
//! }
//! ```

use super::*;

use crate::api::hex::hex_lower;
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

/// One `apis.json` entry the engine refuses to serve, and why.
///
/// `provider` is `None` when the file itself is the problem, so there is
/// no entry to name. Modelling that as an option rather than a sentinel
/// name keeps a real provider called "apis.json" from impersonating it.
#[derive(Debug, Clone, Serialize)]
pub struct RejectedProvider {
    pub provider: Option<String>,
    pub reason: String,
}

impl RejectedProvider {
    /// How this reads to a person: the provider name, or the config path
    /// when the file itself is what failed. Used by the startup log and the
    /// boot notification. `thread-sync.ts` mirrors the same fallback for the
    /// `provider: null` it receives on the wire.
    pub fn label(&self) -> &str {
        self.provider.as_deref().unwrap_or(PROXY_CONFIG_REL_PATH)
    }
}

/// What `apis.json` yielded: the providers the engine will serve, and the
/// entries it refuses.
///
/// Both halves matter, and neither is an error. A refused entry is a
/// configuration mistake the user can fix, reported by name, while every
/// other entry keeps working.
#[derive(Debug, Clone, Default)]
pub struct ProxyConfigLoad {
    pub providers: ProxyConfigMap,
    pub rejected: Vec<RejectedProvider>,
}

impl ProxyConfigLoad {
    /// Every `script_handshake` script these providers name, as
    /// workspace-relative paths (`data/scripts/auth/foo.py`).
    ///
    /// The runner keys approvals that way, and the startup seed needs the same
    /// spelling. A rejected entry contributes nothing: it will never run.
    pub fn handshake_script_paths(&self) -> Vec<String> {
        let mut out: Vec<String> = self
            .providers
            .values()
            .filter_map(|cfg| cfg.auth.as_ref())
            .flat_map(|pipeline| pipeline.pipeline.iter())
            .filter_map(|layer| match layer {
                crate::api::proxy_pipeline_config::LayerConfig::ScriptHandshake {
                    script, ..
                } => Some(format!("data/{}", script.trim_start_matches('/'))),
                _ => None,
            })
            .collect();
        out.sort();
        out.dedup();
        out
    }

    /// Which `base_url` each named credential is used against, across every
    /// entry that names it.
    ///
    /// The startup pass reads this to give a scope to a credential that has
    /// none. One entry naming it is an answer. Two entries disagreeing is not,
    /// which is why the values are a set rather than the first one seen.
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

    /// The reason this name is refused, if it is. A file-level rejection
    /// answers for EVERY name: an unreadable file may have overridden a
    /// builtin, and routing that traffic to the builtin instead would be a
    /// silent change of backend.
    pub fn rejection_for(&self, name: &str) -> Option<&RejectedProvider> {
        self.rejected
            .iter()
            .find(|r| r.provider.is_none() || r.provider.as_deref() == Some(name))
    }
}

/// Load proxy config from `<workspace>/data/config/apis.json`.
/// Missing file means no proxies are configured, and yields an empty load.
///
/// **Never fails.** One malformed entry used to fail the whole file, and
/// `main.rs` turned that into a boot abort. So a single bad provider took a
/// workspace offline until somebody edited JSON by hand. Every entry is now
/// parsed on its own: the good ones load, the bad ones come back in
/// `rejected` with a reason, and the caller decides how loudly to say so.
/// See `docs/plans/2026-08-26-apis-json-must-not-kill-the-engine-boot.md`.
pub fn load_proxy_config(workspace_path: &FsPath) -> ProxyConfigLoad {
    let path = workspace_path.join(PROXY_CONFIG_REL_PATH);
    if !path.exists() {
        return ProxyConfigLoad::default();
    }
    let file_level = |reason: String| ProxyConfigLoad {
        providers: ProxyConfigMap::new(),
        rejected: vec![RejectedProvider {
            provider: None,
            reason,
        }],
    };
    let content = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(e) => return file_level(format!("could not be read: {e}")),
    };
    // Deserialize the outer object only, so one unparseable entry costs
    // nothing but itself. A root that is not an object has no entries to
    // salvage and is reported whole.
    let raw: HashMap<String, serde_json::Value> = match serde_json::from_str(&content) {
        Ok(m) => m,
        Err(e) => return file_level(format!("is not usable: {e}")),
    };

    let mut load = ProxyConfigLoad::default();
    for (name, entry) in raw {
        match parse_provider(&name, &entry) {
            Ok(cfg) => {
                load.providers.insert(name, cfg);
            }
            Err(reason) => load.rejected.push(RejectedProvider {
                provider: Some(name),
                reason,
            }),
        }
    }
    // Stable order so a log line, an event payload and a test all read the
    // same way regardless of the hash map's iteration order.
    load.rejected.sort_by(|a, b| a.provider.cmp(&b.provider));
    load
}

/// One provider's entry, or the reason the engine will not serve it.
fn parse_provider(name: &str, entry: &serde_json::Value) -> Result<ProxyConfig, String> {
    let cfg: ProxyConfig = serde_json::from_value(entry.clone()).map_err(|e| {
        // A legacy `auth` block reaching here is one the startup migration
        // could not rewrite. Serde's "missing field `pipeline`" says nothing
        // a user can act on, and the translator's own words do.
        entry
            .get("auth")
            .and_then(|auth| super::proxy_migration::legacy_rejection(name, auth))
            .unwrap_or_else(|| format!("provider '{name}': {e}"))
    })?;
    // Walk the pipeline and validate any ScriptHandshake layer's script
    // path, before the engine can be tricked into running an out-of-workspace
    // file. The rule itself lives with the spawn, in
    // `proxy_script_runner::script_path_rejection`, so the two cannot disagree.
    for layer in cfg.auth.iter().flat_map(|p| &p.pipeline) {
        if let LayerConfig::ScriptHandshake { script, .. } = layer {
            if let Some(reason) = super::proxy_script_runner::script_path_rejection(script) {
                return Err(format!("proxy '{name}' {reason}"));
            }
        }
    }
    Ok(cfg)
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

/// Browser-origin guard for credential-injecting proxy HTTP routes.
///
/// The policy lives in [`crate::api::browser_origin`], which also layers it over
/// the whole `/api/v1` surface. This call stays for two reasons. The outer layer
/// is skipped under `LUCIDOS_PERMISSIVE_CORS`, and a route resolving a
/// credential must refuse a foreign page even then. It also runs before the
/// credential lookup, rather than only before routing.
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

/// Reject `..` traversal segments and backslashes in the upstream REQUEST path.
/// Without this, a caller can splice `/api/v1/proxy/x/../../admin` and most
/// upstreams normalize the result, escaping any path prefix the operator set in
/// `base_url` (e.g. `https://example.com/safe-prefix`).
///
/// A URL path, not a filesystem path, which is why it is looser than
/// `is_path_traversal`: a leading `/` is normal here, and a segment that merely
/// contains `..` (`foo..bar`) is a legitimate resource name. The name states
/// its domain, so nobody reaches for it where a file path is meant. That is how
/// the `script_handshake` guard once ended up weaker than its sibling.
///
/// Compares DECODED segments, not the literal `..`. Axum decodes the wildcard
/// capture exactly once, so `%252e%252e` arrives here as `%2e%2e`, which the
/// URL parser still normalizes into a parent segment.
/// [`build_contained_target_url`] is the structural guard; this one turns the
/// same input into an early, clear 400.
pub fn request_path_has_traversal(path: &str) -> bool {
    path.split('/').any(segment_is_parent) || path.contains('\\')
}

/// True when one path segment normalizes to `..`.
///
/// Mirrors the WHATWG double-dot rule the `url` crate implements: either dot
/// may be written literally or as `%2e`, in any case. A single-dot segment is
/// deliberately not matched, because it cannot leave a prefix.
fn segment_is_parent(segment: &str) -> bool {
    segment.to_ascii_lowercase().replace("%2e", ".") == ".."
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

/// Build the upstream URL, and prove it stayed inside `base_url`'s path prefix.
///
/// [`build_target_url`] concatenates, and the URL parser then NORMALIZES.
/// Normalization is where a prefix escapes: the parser reads `%2e%2e` as a
/// parent segment, so a double-encoded path survives Axum's single decode and
/// pops the operator's prefix during parse. The request stays on the configured
/// origin and reaches an endpoint outside the prefix, carrying the proxy's
/// credentials.
///
/// So the check runs on the PARSED url, the only place the real path is known.
/// It therefore holds whatever encoding produced the path, where a blocklist of
/// dot spellings only holds until the next one. Same shape as
/// `CredentialStore`'s `credential_base_url_matches`.
///
/// Returns the ORIGINAL concatenation on success, never the normalized string:
/// signing layers hash the URL they are handed, so re-encoding it here would
/// change signatures on the accepted path.
///
/// See `docs/plans/2026-08-25-oauth-host-classification-and-proxy-prefix-containment.md`.
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
    if !path_is_within(base_path, target.path()) {
        // Path only. A query string can carry a credential.
        return Err(format!(
            "upstream path '{}' escapes the configured base path '{base_path}'",
            target.path()
        ));
    }

    // Then every deeper reading, one per decode layer an upstream might apply.
    // `%2f` is not a separator to the URL parser, so `%2e%2e%2fadmin` parses as
    // one contained segment and forwards verbatim. An upstream that decodes it
    // reads `../admin` and leaves the prefix anyway, with our credentials on
    // the request. Checking the readings refuses that, while a blanket ban on
    // `%2f` would break a legitimate encoded slash inside a segment.
    for reading in decoded_readings(path) {
        let probe = build_target_url(base_url, &reading, query);
        let Ok(probe) = reqwest::Url::parse(&probe) else {
            return Err("upstream URL does not parse with separators decoded".to_string());
        };
        if !path_is_within(base_path, probe.path()) {
            return Err(format!(
                "upstream path '{}' escapes the configured base path '{base_path}' \
                 once encoded separators are decoded",
                probe.path()
            ));
        }
    }

    Ok(candidate)
}

/// True when `target_path` is `base_path` itself or sits under it.
///
/// An empty `base_path` means the operator configured no prefix, so nothing is
/// out of bounds. The boundary is a segment, never a string prefix, so
/// `/safe-prefix-evil` does not count as inside `/safe-prefix`.
fn path_is_within(base_path: &str, target_path: &str) -> bool {
    base_path.is_empty()
        || target_path == base_path
        || target_path
            .strip_prefix(base_path)
            .is_some_and(|rest| rest.starts_with('/'))
}

/// Every deeper reading of `path`, one per decode layer. For the containment
/// probe above, never for sending.
///
/// A round rewrites the separator encodings into `/`, then unwraps one layer of
/// escaping. So `%252f` becomes `%2f`, then `/`, and an upstream stack that
/// decodes twice is covered as well as one that decodes once. Chasing a fixed
/// depth would just move the bypass one `%25` deeper.
///
/// Terminates because every rewrite maps three characters to one, so each round
/// is strictly shorter than the last. A backslash maps to `/` because the URL
/// standard treats it as a separator. That is also why
/// [`request_path_has_traversal`] rejects a literal one.
fn decoded_readings(path: &str) -> Vec<String> {
    let mut readings = Vec::new();
    let mut current = path.to_string();
    loop {
        let next = current
            .replace("%2f", "/")
            .replace("%2F", "/")
            .replace("%5c", "/")
            .replace("%5C", "/")
            .replace("%25", "%");
        if next == current {
            return readings;
        }
        readings.push(next.clone());
        current = next;
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
    let load = load_proxy_config(workspace_path);
    if let Some(cfg) = load.providers.get(name) {
        return Ok(cfg.clone());
    }
    // A refused entry is NOT "not configured", and the difference decides
    // where the request goes. Only a 404 falls through to the builtin
    // provider of the same name. Answering 404 here would quietly send this
    // traffic to a different backend than the one that was configured.
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

/// Look up a single credential by service name, mapping missing/empty/db
/// failures to the same `(StatusCode, String)` error shape every auth path
/// uses. The single source of truth for "this credential must exist and be
/// non-empty, or the proxy can't proceed".
///
/// Resolves an OAuth client registration too, which `CredentialStore::get` is
/// deliberately blind to. An `apis.json` entry may name one explicitly (a
/// `script_handshake` layer whose script does its own token exchange), and it is
/// named rather than merely present, so the ambiguity `get`'s exclusion exists
/// to prevent does not arise here.
///
/// **Temporary measure**, registered in `docs/temporary-measures.md` under
/// "`oauth:` prefix stripped from a caller-supplied credential name". An older
/// config, written before the prefix migration renamed the credential, spells
/// that name `oauth:<provider>`, which is now stored as just `<provider>`. The
/// fallback keeps those entries working: without it a live `apis.json` 502s on
/// every request the moment the prefix migration runs, and `data/config/` is
/// user data no DB migration can rewrite.
pub(crate) async fn fetch_required_credential(
    pool: &sqlx::PgPool,
    name: &str,
) -> Result<Credential, (StatusCode, String)> {
    let found = match CredentialStore::get(pool, name).await {
        Ok(Some(c)) => Ok(Some(c)),
        Ok(None) => {
            // Normalize ONLY a name that actually carries the legacy prefix.
            // Running every miss through `client_provider_name` would also
            // lowercase it, so a config naming `Stripe` (no such credential)
            // would silently resolve an unrelated `stripe` OAuth registration
            // and the proxy would send a `{client_id, ...}` blob as its auth
            // header. A miss must stay a miss unless the name is one of the two
            // spellings of the same thing.
            let lookup = if name.trim().to_lowercase().starts_with("oauth:") {
                crate::core::oauth::client_provider_name(name)
            } else {
                name.to_string()
            };
            CredentialStore::get_typed(pool, &lookup, crate::core::AuthType::OauthClient).await
        }
        Err(e) => Err(e),
    };
    match found {
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

/// Refuse to present `name` to a provider whose `base_url` its scope does not
/// cover (ADR 0144).
///
/// `data/config/apis.json` is writable over the API, so `base_url` is caller
/// data. Without this, an entry naming `github` and pointing at an attacker
/// host makes the engine attach that credential and forward it. The rule is
/// already applied to git: `core::git_auth` re-checks the same predicate on
/// every credential callback, so a redirect cannot carry a secret off.
///
/// A credential with no scope is refused rather than presented anywhere. The
/// startup pass infers a scope for one that predates this rule, so what reaches
/// here unscoped is a row nothing in `apis.json` explains.
pub(crate) async fn check_credential_scope(
    pool: &sqlx::PgPool,
    name: &str,
    base_url: &str,
) -> Result<(), (StatusCode, String)> {
    let credential = fetch_required_credential(pool, name).await?;
    if credential.base_url.trim().is_empty() {
        return Err((
            StatusCode::BAD_GATEWAY,
            format!(
                "credential '{}' has no base_url, so it will not be sent anywhere. \
                 Set its base_url in Settings to the API it belongs to",
                name
            ),
        ));
    }
    if !crate::core::credentials::credential_base_url_matches(&credential.base_url, base_url) {
        return Err((
            StatusCode::BAD_GATEWAY,
            format!(
                "credential '{}' is scoped to {} and will not be sent to {}",
                name, credential.base_url, base_url
            ),
        ));
    }
    Ok(())
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
            // A body read that fails mid-stream (connection dropped, the
            // client's 30s timeout expiring during the body) must NOT be
            // reported as the upstream's own status with an empty body: the
            // calling app would read a 200 with no data as a successful empty
            // result. Surface it as the gateway error it is. Same URL-stripping
            // as the send-failure arm below, for the same credential reason.
            let resp_body = match resp.bytes().await {
                Ok(b) => b,
                Err(e) => {
                    let safe_e = e.without_url();
                    log!(
                        "[Proxy] reading upstream body from {} failed: {}",
                        log_url,
                        safe_e
                    );
                    return (
                        StatusCode::BAD_GATEWAY,
                        format!("upstream body read failed: {}", safe_e),
                    )
                        .into_response();
                }
            };
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
    let new_modules = crate::api::proxy_wasm_signer::load_wasm_modules(&dir, engine.wasm_engine())?;
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
    if !browser_proxy_request_allowed(&headers) {
        return forbidden_cross_origin_proxy_response();
    }
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

/// The full origin a signed request is bound to: `(scheme, host, port)`.
/// Used to refuse re-signing after a redirect that crosses the origin —
/// host-only matching would re-send the credential after a `https → http`
/// scheme downgrade (credential over plaintext) or a hop to a *different
/// port* on the same host (e.g. an internal admin service). `None` when
/// the URL doesn't parse or has no host. Scheme and host are lowercased;
/// port falls back to the scheme's default so `https://h` and
/// `https://h:443` compare equal.
fn origin_of(url: &str) -> Option<(String, String, Option<u16>)> {
    let u = reqwest::Url::parse(url).ok()?;
    let host = u.host_str()?.to_ascii_lowercase();
    Some((
        u.scheme().to_ascii_lowercase(),
        host,
        u.port_or_known_default(),
    ))
}

/// Resolve a `Location` header value (relative or absolute) against the
/// URL we just hit, returning the absolute target URL.
fn resolve_redirect_location(current_url: &str, location: &str) -> Option<reqwest::Url> {
    let base = reqwest::Url::parse(current_url).ok()?;
    base.join(location).ok()
}

/// A resolved proxy target. `apis.json` entries build their auth layers per
/// request from `config.auth`; a builtin model-provider target carries its
/// base URL + pre-built auth layers (sourced from the engine's own provider
/// credentials — see [`crate::api::proxy_builtin`]).
enum ResolvedProxy {
    Config(ProxyConfig),
    Builtin {
        base_url: String,
        layers: Vec<Arc<dyn crate::api::proxy_auth_layer::AuthLayer>>,
    },
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
    // `apis.json` is resolved first, so an entry with the same name overrides
    // the builtin. A builtin model-provider proxy (openai/openrouter/anthropic/
    // vertex/local) fills the 404 gap when no entry exists.
    let resolved = match resolve_proxy_target(&state.workspace_path, &name).await {
        Ok(config) => ResolvedProxy::Config(config),
        Err((StatusCode::NOT_FOUND, generic)) => {
            match crate::api::proxy_builtin::resolve_builtin_provider(&state.engine, &name).await {
                Ok(Some((base_url, layers))) => ResolvedProxy::Builtin { base_url, layers },
                // Not a builtin either — the original "not configured" 404.
                Ok(None) => return (StatusCode::NOT_FOUND, generic).into_response(),
                // A recognized builtin whose credential/config is absent.
                Err((status, msg)) => return (status, msg).into_response(),
            }
        }
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

    let result = match resolved {
        ResolvedProxy::Config(config) => {
            dispatch_proxy_request(
                &state.engine,
                &name,
                &config,
                method,
                path,
                query,
                headers,
                body,
            )
            .await
        }
        ResolvedProxy::Builtin { base_url, layers } => {
            dispatch_with_layers(&name, &base_url, layers, method, path, query, headers, body).await
        }
    };
    match result {
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
    dispatch_with_layers(
        name,
        &config.base_url,
        layers,
        method,
        path,
        query,
        headers,
        body,
    )
    .await
}

/// Forward through a pre-built layer pipeline (with same-host redirect
/// re-signing) and drive the one-shot 401 invalidate-and-retry. Shared by the
/// `apis.json` path ([`dispatch_proxy_request`], which builds `layers` from a
/// `ProxyConfig`) and the builtin-provider path
/// ([`crate::api::proxy_builtin::resolve_builtin_provider`], which builds
/// `layers` from the engine's own model-provider credentials) so the forward +
/// redirect + retry semantics stay identical for both.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn dispatch_with_layers(
    name: &str,
    base_url: &str,
    layers: Vec<Arc<dyn crate::api::proxy_auth_layer::AuthLayer>>,
    method: Method,
    path: String,
    query: Option<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, (StatusCode, String)> {
    let (response, outcome) = forward_with_redirects(
        name,
        base_url,
        &layers,
        &method,
        &path,
        query.as_deref(),
        &headers,
        &body,
    )
    .await?;

    if !crate::api::proxy_pipeline::pipeline_should_retry(&layers, &outcome, response.status()) {
        return Ok(response);
    }
    crate::api::proxy_pipeline::pipeline_invalidate_for_retry(&layers, &outcome).await;
    let (response, _) = forward_with_redirects(
        name,
        base_url,
        &layers,
        &method,
        &path,
        query.as_deref(),
        &headers,
        &body,
    )
    .await?;
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
        base_url: &config.base_url,
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
    // Auth is bound to the initial origin (scheme + host + port) — a
    // redirect that crosses ANY of those later is refused with 502.
    // Host-only binding would re-sign + re-send the credential after a
    // `https → http` scheme downgrade (credential over plaintext) or a
    // hop to a different port on the same host.
    let initial_url = build_target_url(base_url, initial_path, initial_query);
    let initial_origin = origin_of(&initial_url).ok_or_else(|| {
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
        // Per hop, so it also catches a same-origin `Location` that leaves the
        // prefix. The origin check below cannot see that: scheme, host and port
        // all still match.
        let target_url =
            build_contained_target_url(base_url, &current_path, current_query.as_deref()).map_err(
                |reason| {
                    log!("[Proxy] {} refused an upstream URL: {}", name, reason);
                    // Hop 0 is the caller's own path, so that is a 400. A later
                    // hop is the upstream misdirecting us, which is a 502.
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
        let target_origin = origin_of(target.as_str()).ok_or_else(|| {
            (
                StatusCode::BAD_GATEWAY,
                format!(
                    "proxy '{name}' upstream redirected to a Location with no host: {location}"
                ),
            )
        })?;
        if target_origin != initial_origin {
            // Names only — no full URLs in the log (a Location can carry a
            // credential in its query string).
            log!(
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
