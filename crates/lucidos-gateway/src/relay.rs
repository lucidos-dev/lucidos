//! The relay: gateway A serves gateway B's workspaces to A's own devices.
//!
//! A routes `/<slug>@<name>/*` to B's `/~/api/gateway/v1/relay/<slug>/*`. It
//! presents its gateway credential and a claim naming the calling device and
//! the outside prefix, signed with an HMAC keyed by the credential. B checks
//! both, stamps the device under a namespaced id, and serves the request
//! through its ordinary workspace route. ADR 0591 records why this is the one
//! hop that proxies another gateway's workspaces.
//!
//! Both halves live here: [`relay`] is A's, [`receive`] is B's.

use std::time::Duration;

use axum::body::Body;
use axum::extract::{OriginalUri, Request, State};
use axum::http::{header, HeaderMap, HeaderName, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Extension;
use futures::TryStreamExt as _;
use hmac::{Hmac, Mac};
use sha2::Sha256;

use crate::auth::{self, AuthenticatedDevice};
use crate::gateway_protocol::{
    AuthenticatedGateway, GATEWAY_API_PREFIX, HEADER_GATEWAY_CREDENTIAL,
};
use crate::server::GatewayState;

/// The relay call, relative to [`GATEWAY_API_PREFIX`].
pub const RELAY_PATH: &str = "/relay";

/// The header carrying A's claim about the calling device.
pub const HEADER_RELAY_CLAIM: &str = "x-lucidos-relay-claim";

/// The header carrying the claim's HMAC, in lowercase hex.
pub const HEADER_RELAY_SIGNATURE: &str = "x-lucidos-relay-signature";

/// The first field of every claim. A new claim shape gets a new version.
const CLAIM_VERSION: &str = "v1";

/// Separates claim fields. `urlencoding` escapes it, so no field contains it.
const CLAIM_SEPARATOR: char = '|';

/// How far a claim's issue time may sit from B's clock, either way.
const CLAIM_MAX_SKEW_SECS: u64 = 300;

/// How long A waits to connect to B. No total timeout: an SSE stream is open
/// for as long as the page is.
const RELAY_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// Split a route segment `<slug>@<name>` into its halves.
///
/// `None` unless both halves are valid ids. `@` is never part of a slug, so a
/// `None` here falls through to the local lookup, which misses.
pub fn remote_route(segment: &str) -> Option<(&str, &str)> {
    let (slug, name) = segment.split_once(lucidos_gateway_constants::REMOTE_ROUTE_SEPARATOR)?;
    (crate::registry::is_valid_id(slug) && crate::registry::is_valid_id(name))
        .then_some((slug, name))
}

/// Does A pass this request to a remote gateway for B to verify?
///
/// A frame capability on a remote route is B's to verify, since B's engine
/// minted it and only B holds the key. So A checks its shape and nothing else:
/// a read-only, non-upgrade request with no dot segment. B then refuses a
/// capability that does not verify, because the claim carries no device.
pub fn carries_a_remote_frame_capability(req: &Request) -> bool {
    let path = req.uri().path();
    let remote = path
        .strip_prefix('/')
        .and_then(|p| p.split('/').next())
        .and_then(remote_route)
        .is_some();
    remote
        && crate::auth_api::path_carries_frame_capability(path)
        && !crate::auth_api::carries_a_dot_segment(path)
        && lucidos_frame_capability::method_is_read_only(req.method().as_str())
        && !req.headers().contains_key(header::UPGRADE)
}

/// What A says about one relayed request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelayClaim {
    pub issued_at: i64,
    /// Where the browser sees the workspace on A: `/<slug>@<name>/`.
    pub prefix: String,
    /// The device A authenticated. `None` for what A admits without one.
    pub device: Option<AuthenticatedDevice>,
}

impl RelayClaim {
    /// The claim as one header value: version, issue time, prefix, device id
    /// and label, each percent-encoded.
    pub fn encode(&self) -> String {
        let (id, label) = match &self.device {
            Some(d) => (d.id.as_str(), d.label.as_str()),
            None => ("", ""),
        };
        [
            CLAIM_VERSION.to_string(),
            self.issued_at.to_string(),
            urlencoding::encode(&self.prefix).into_owned(),
            urlencoding::encode(id).into_owned(),
            urlencoding::encode(label).into_owned(),
        ]
        .join(&CLAIM_SEPARATOR.to_string())
    }

    pub fn decode(raw: &str) -> Option<Self> {
        let mut fields = raw.split(CLAIM_SEPARATOR);
        if fields.next()? != CLAIM_VERSION {
            return None;
        }
        let issued_at = fields.next()?.parse().ok()?;
        let prefix = urlencoding::decode(fields.next()?).ok()?.into_owned();
        let id = urlencoding::decode(fields.next()?).ok()?.into_owned();
        let label = urlencoding::decode(fields.next()?).ok()?.into_owned();
        if fields.next().is_some() {
            return None;
        }
        let device = (!id.is_empty()).then_some(AuthenticatedDevice { id, label });
        Some(Self {
            issued_at,
            prefix,
            device,
        })
    }
}

/// The HMAC-SHA256 of `claim`, keyed by `credential`, in lowercase hex.
pub fn sign(credential: &str, claim: &str) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(credential.as_bytes())
        .expect("HMAC takes a key of any length");
    mac.update(claim.as_bytes());
    mac.finalize()
        .into_bytes()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Why B refused a relayed request. Each is a 401: A holds a credential, so
/// the reason is for its log, not for a person.
#[derive(Debug, PartialEq, Eq)]
pub enum ClaimRefusal {
    Missing,
    Malformed,
    BadSignature,
    Stale,
    PrefixMismatch,
    BadDeviceId,
}

impl ClaimRefusal {
    fn message(&self) -> &'static str {
        match self {
            Self::Missing => "the relayed request carries no claim",
            Self::Malformed => "the relay claim is malformed",
            Self::BadSignature => "the relay claim's signature does not verify",
            Self::Stale => "the relay claim is too old, or from the future",
            Self::PrefixMismatch => "the relay claim names another workspace",
            Self::BadDeviceId => "the relay claim's device id is not usable",
        }
    }
}

/// B's check of a claim, against the credential the request presented.
///
/// The prefix must be `/<slug>@<name>/` for the slug in the path, so A cannot
/// point B's engine at another workspace's address.
pub fn verify_claim(
    headers: &HeaderMap,
    credential: &str,
    slug: &str,
    now: i64,
) -> Result<RelayClaim, ClaimRefusal> {
    let read = |name: &str| headers.get(name).and_then(|v| v.to_str().ok());
    let (Some(raw), Some(signature)) = (read(HEADER_RELAY_CLAIM), read(HEADER_RELAY_SIGNATURE))
    else {
        return Err(ClaimRefusal::Missing);
    };
    if !auth::ct_eq(&sign(credential, raw), signature) {
        return Err(ClaimRefusal::BadSignature);
    }
    let claim = RelayClaim::decode(raw).ok_or(ClaimRefusal::Malformed)?;
    if now.abs_diff(claim.issued_at) > CLAIM_MAX_SKEW_SECS {
        return Err(ClaimRefusal::Stale);
    }
    let names_this_slug = claim
        .prefix
        .strip_prefix('/')
        .and_then(|p| p.strip_suffix('/'))
        .and_then(remote_route)
        .is_some_and(|(claimed, _)| claimed == slug);
    if !names_this_slug {
        return Err(ClaimRefusal::PrefixMismatch);
    }
    if claim
        .device
        .as_ref()
        .is_some_and(|d| !is_usable_device_id(&d.id))
    {
        return Err(ClaimRefusal::BadDeviceId);
    }
    Ok(claim)
}

/// A device id B can namespace: the characters A's own ids use, and no `:`,
/// which separates the pairing id from the device id.
fn is_usable_device_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// The device id B's engine sees for a relayed device.
pub fn namespaced_device_id(pairing_id: &str, device_id: &str) -> String {
    format!("{pairing_id}:{device_id}")
}

/// A's client for relayed requests: `https` only, certificates checked, no
/// redirects, no decompression, and no total timeout. A call that a person
/// waits on sets its own timeout per request.
pub struct RelayClient {
    http: std::sync::RwLock<reqwest::Client>,
}

impl RelayClient {
    pub fn new() -> Result<Self, String> {
        Ok(Self {
            http: std::sync::RwLock::new(build_client(None)?),
        })
    }

    /// Also trust `pem`, for a test's self-signed B.
    #[cfg(test)]
    pub fn trust_for_test(&self, pem: &[u8]) {
        let root = reqwest::Certificate::from_pem(pem).unwrap();
        *self.http.write().unwrap() = build_client(Some(root)).unwrap();
    }

    pub(crate) fn client(&self) -> Option<reqwest::Client> {
        self.http.read().ok().map(|c| c.clone())
    }
}

fn build_client(extra_root: Option<reqwest::Certificate>) -> Result<reqwest::Client, String> {
    let mut builder = reqwest::Client::builder()
        .https_only(true)
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(RELAY_CONNECT_TIMEOUT);
    if let Some(root) = extra_root {
        builder = builder.add_root_certificate(root);
    }
    builder
        .build()
        .map_err(|e| format!("could not start the relay client: {e}"))
}

/// A's half: relay `req`, addressed to `/<slug>@<name>/…`, to remote gateway
/// `name`.
///
/// `enforce` already ran, so the caller is a device A authenticated, or a
/// request A admits without one. The claim says which.
pub async fn relay(state: &GatewayState, slug: &str, name: &str, req: Request) -> Response {
    let route = format!("/{slug}@{name}");
    // A page load that B cannot serve goes back to the workspace list, which
    // says what is wrong with B. A remembered remote workspace then never
    // traps a launch on an error page.
    let navigation = crate::auth_api::is_top_level_navigation(req.headers());
    let to_the_list = || redirect(&format!("/{}/?pick", crate::registry::SIGIL));
    let Some(remote) = state
        .remote_gateways()
        .list()
        .into_iter()
        .find(|g| g.name == name)
    else {
        if navigation {
            return to_the_list();
        }
        return (
            StatusCode::NOT_FOUND,
            format!("no remote gateway called '{name}'"),
        )
            .into_response();
    };
    let path_and_query = req
        .uri()
        .path_and_query()
        .map(|pq| pq.as_str())
        .unwrap_or("/")
        .to_string();
    let rest = match path_and_query.strip_prefix(&route) {
        Some(rest) if rest.starts_with('/') => rest,
        _ => return redirect(&format!("{route}/")),
    };
    // A URL parser collapses `..`, so `/dev@cloud/../../~/api/gateway/v1/…`
    // would reach another of B's gateway calls with A's credential.
    if crate::auth_api::carries_a_dot_segment(req.uri().path()) {
        return (
            StatusCode::BAD_REQUEST,
            "a relayed path cannot hold a dot segment",
        )
            .into_response();
    }
    if crate::proxy::is_websocket_upgrade(&req) {
        return (
            StatusCode::NOT_IMPLEMENTED,
            "a workspace on a remote gateway cannot open a WebSocket through this gateway yet",
        )
            .into_response();
    }
    let Some(client) = state.relay_client().client() else {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            "the relay client is unavailable",
        )
            .into_response();
    };
    let (parts, body) = req.into_parts();
    let claim = RelayClaim {
        issued_at: chrono::Utc::now().timestamp(),
        prefix: format!("{route}/"),
        device: parts.extensions.get::<AuthenticatedDevice>().cloned(),
    }
    .encode();
    let signature = sign(&remote.credential, &claim);

    let url = format!(
        "{}{GATEWAY_API_PREFIX}{RELAY_PATH}/{slug}{rest}",
        remote.address
    );
    let origin_vouched = crate::proxy::vouches_for_origin(&parts.headers);
    let mut headers = HeaderMap::new();
    for (header_name, value) in &parts.headers {
        // A's cookies are A's credentials. B authenticates the hop by the claim.
        let consumed =
            (origin_vouched && header_name == header::ORIGIN) || header_name == header::COOKIE;
        if !consumed && !crate::proxy::gateway_owns_header(header_name, false) {
            headers.append(header_name.clone(), value.clone());
        }
    }
    let (Some(credential), Ok(claim), Ok(signature)) = (
        auth::sensitive_credential(&remote.credential),
        HeaderValue::from_str(&claim),
        HeaderValue::from_str(&signature),
    ) else {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            "the relay claim is not a header value",
        )
            .into_response();
    };
    headers.insert(HEADER_GATEWAY_CREDENTIAL, credential);
    headers.insert(HEADER_RELAY_CLAIM, claim);
    headers.insert(HEADER_RELAY_SIGNATURE, signature);

    let mut upstream = client.request(parts.method, url).headers(headers);
    // A body streams through, never buffered. A request that declares none
    // sends none, so a GET does not go out chunked.
    if let Some(length) = declared_body_length(&parts.headers) {
        upstream = upstream
            .header(header::CONTENT_LENGTH, length)
            .body(reqwest::Body::wrap_stream(body.into_data_stream()));
    } else if parts.headers.contains_key(header::TRANSFER_ENCODING) {
        upstream = upstream.body(reqwest::Body::wrap_stream(body.into_data_stream()));
    }
    let response = match upstream.send().await {
        Ok(r) => r,
        Err(e) => {
            crate::log!(
                "[Gateway] relay {} to remote gateway '{}' failed: {}",
                path_and_query,
                name,
                crate::external_address::error_chain(&e)
            );
            if navigation {
                return to_the_list();
            }
            return (
                StatusCode::BAD_GATEWAY,
                format!("the remote gateway '{name}' could not be reached"),
            )
                .into_response();
        }
    };
    let mut out = Response::builder().status(response.status());
    for (header_name, value) in response.headers() {
        if relays_response_header(header_name) {
            out = out.header(header_name, value);
        }
    }
    out.body(Body::from_stream(
        response.bytes_stream().map_err(std::io::Error::other),
    ))
    .unwrap_or_else(|_| StatusCode::BAD_GATEWAY.into_response())
}

/// Does a header from B's response reach the browser?
///
/// The browser reads it on A's origin. So a header that sets origin-wide state
/// would reach A's device cookie, A's storage or A's service-worker scope.
pub(crate) fn relays_response_header(name: &HeaderName) -> bool {
    let n = name.as_str();
    !crate::proxy::is_hop_by_hop(name)
        && !n.starts_with("access-control-")
        && !matches!(
            n,
            "set-cookie"
                | "set-cookie2"
                | "clear-site-data"
                | "service-worker-allowed"
                | "strict-transport-security"
                | "alt-svc"
                | "www-authenticate"
        )
}

/// A positive `Content-Length`, as sent.
fn declared_body_length(headers: &HeaderMap) -> Option<u64> {
    headers
        .get(header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.trim().parse::<u64>().ok())
        .filter(|&n| n > 0)
}

fn redirect(location: &str) -> Response {
    Response::builder()
        .status(StatusCode::TEMPORARY_REDIRECT)
        .header(header::LOCATION, location)
        .body(Body::empty())
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

/// B's half: serve a request A relayed, behind `require_gateway`.
///
/// The claim is checked against the credential the request presented, which
/// `require_gateway` already matched to a current pairing. The request then
/// becomes `/<slug>/…` and takes B's ordinary workspace route.
pub async fn receive(
    State(state): State<GatewayState>,
    Extension(gateway): Extension<AuthenticatedGateway>,
    OriginalUri(original): OriginalUri,
    req: Request,
) -> Response {
    let refuse = |message: &str| {
        (
            StatusCode::UNAUTHORIZED,
            axum::Json(serde_json::json!({ "error": message })),
        )
            .into_response()
    };
    let relay_root = format!("{GATEWAY_API_PREFIX}{RELAY_PATH}/");
    let Some(target) = original.path().strip_prefix(&relay_root) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let (slug, rest) = match target.split_once('/') {
        Some((slug, rest)) => (slug, format!("/{rest}")),
        None => (target, String::new()),
    };
    if !crate::registry::is_valid_id(slug) {
        return StatusCode::NOT_FOUND.into_response();
    }
    let credential = req
        .headers()
        .get(HEADER_GATEWAY_CREDENTIAL)
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .unwrap_or_default();
    let claim = match verify_claim(
        req.headers(),
        credential,
        slug,
        chrono::Utc::now().timestamp(),
    ) {
        Ok(claim) => claim,
        Err(refusal) => {
            crate::log!(
                "[Gateway] refused a request relayed by '{}': {}",
                gateway.label,
                refusal.message()
            );
            return refuse(refusal.message());
        }
    };

    let local_path = match original.query() {
        Some(query) => format!("/{slug}{rest}?{query}"),
        None => format!("/{slug}{rest}"),
    };
    let (mut parts, body) = req.into_parts();
    let Ok(uri) = local_path.parse() else {
        return (StatusCode::BAD_REQUEST, "malformed relayed path").into_response();
    };
    parts.uri = uri;
    parts.extensions.remove::<AuthenticatedDevice>();
    let device = claim.device.map(|d| AuthenticatedDevice {
        id: namespaced_device_id(&gateway.pairing_id, &d.id),
        label: d.label,
    });
    let has_device = device.is_some();
    if let Some(device) = device {
        parts.extensions.insert(device);
    }
    let local = Request::from_parts(parts, body);
    // A claim with no device reaches only what B admits without one. B, not
    // A, verifies a frame capability: B's engine minted it.
    if !has_device
        && !crate::auth_api::is_public_app_asset(local.uri().path())
        && !crate::auth_api::frame_capability_admits(&state, &local)
    {
        return refuse("a relayed request with no device reaches only public app assets");
    }
    crate::server::serve_workspace(&state, slug, &claim.prefix, local).await
}

#[cfg(test)]
#[path = "relay_tests.rs"]
pub(crate) mod tests;
