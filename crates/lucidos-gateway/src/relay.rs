//! The relay: gateway A serves gateway B's workspaces to A's own devices.
//!
//! A routes `/<slug>@<name>/*` to B's `/~/api/gateway/v1/relay/<slug>/*`. It
//! presents its gateway credential and a claim naming the calling device and
//! the outside prefix, signed with an HMAC keyed by the credential. B checks
//! both, stamps the device under a namespaced id, and serves the request
//! through its ordinary workspace route. ADR 0591 records why this is the one
//! hop that proxies another gateway's workspaces.
//! A serves its own frontend; only B's data and sandboxed content cross this
//! hop into A's browser origin (ADR 0723).
//!
//! Both halves live here: [`relay`] is A's, [`receive`] is B's.

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
use crate::relay_timing::{Label, ReceiveClock, RelayClock};
use crate::server::GatewayState;

/// The relay call, relative to [`GATEWAY_API_PREFIX`].
pub const RELAY_PATH: &str = "/relay";

/// The header carrying A's claim about the calling device.
pub const HEADER_RELAY_CLAIM: &str = "x-lucidos-relay-claim";

/// The header carrying the claim's HMAC, in lowercase hex.
pub const HEADER_RELAY_SIGNATURE: &str = "x-lucidos-relay-signature";

/// Present when A's browser runs on A's machine. Its value is the HMAC of the
/// claim plus [`LOCAL_BROWSER_BINDING`], so it holds for that claim only. A
/// gateway that predates it ignores it, which is why it is not a claim field.
pub const HEADER_RELAY_LOCAL_BROWSER: &str = "x-lucidos-relay-local-browser";

/// Appended to the claim before signing the local-browser stamp, so the stamp's
/// MAC can never equal the claim's own.
const LOCAL_BROWSER_BINDING: &str = "|local-browser";

/// The user A holds the calling device under, as `<id>|<name>|<emails>`,
/// each percent-encoded, the emails comma-joined (ADR 0623). Beside the claim
/// rather than in it, so a gateway that predates users ignores it. An
/// older A sends no emails field.
pub const HEADER_RELAY_USER: &str = "x-lucidos-relay-user";

/// The HMAC of the claim, [`USER_BINDING`] and the user value, so the
/// user holds for that claim only.
pub const HEADER_RELAY_USER_SIGNATURE: &str = "x-lucidos-relay-user-signature";

const USER_BINDING: &str = "|user|";

/// The HMAC of the claim, [`CONTRACT_BINDING`] and the client contracts A
/// negotiated. B recomputes it from its own and refuses a mismatch with 426.
pub const HEADER_RELAY_CONTRACT: &str = "x-lucidos-relay-contract";

const CONTRACT_BINDING: &str = "|contract|";

/// The [`HEADER_RELAY_CONTRACT`] value for `binding` on `claim`.
pub fn sign_contract(credential: &str, claim: &str, binding: &str) -> String {
    sign(credential, &format!("{claim}{CONTRACT_BINDING}{binding}"))
}

/// The pair a gateway older than the user rename sends and reads, signed with
/// [`LEGACY_OPERATOR_BINDING`]. A still sends it and B still reads it, because
/// two machines never update at once (docs/temporary-measures.md, "Old relay
/// user header").
pub const LEGACY_HEADER_RELAY_OPERATOR: &str = "x-lucidos-relay-operator";
pub const LEGACY_HEADER_RELAY_OPERATOR_SIGNATURE: &str = "x-lucidos-relay-operator-signature";
const LEGACY_OPERATOR_BINDING: &str = "|operator|";

/// Every header that carries a relayed user, old names included, so a proxy
/// can strip them all.
pub const RELAY_USER_HEADERS: [&str; 4] = [
    HEADER_RELAY_USER,
    HEADER_RELAY_USER_SIGNATURE,
    LEGACY_HEADER_RELAY_OPERATOR,
    LEGACY_HEADER_RELAY_OPERATOR_SIGNATURE,
];

/// A user as A signs it for B.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelayedUser {
    pub id: String,
    pub name: String,
    /// What lets B tell the user is one of its own: B merges a relayed
    /// user into the local user holding the same email.
    pub emails: Vec<String>,
}

impl RelayedUser {
    pub fn encode(&self) -> String {
        let emails: Vec<String> = self
            .emails
            .iter()
            .map(|e| urlencoding::encode(e).into_owned())
            .collect();
        format!(
            "{}{CLAIM_SEPARATOR}{}{CLAIM_SEPARATOR}{}",
            urlencoding::encode(&self.id),
            urlencoding::encode(&self.name),
            emails.join(",")
        )
    }

    pub fn decode(raw: &str) -> Option<Self> {
        let mut fields = raw.splitn(3, CLAIM_SEPARATOR);
        let id = urlencoding::decode(fields.next()?).ok()?.into_owned();
        let name = urlencoding::decode(fields.next()?).ok()?.into_owned();
        let emails = match fields.next() {
            None | Some("") => Vec::new(),
            Some(joined) => joined
                .split(',')
                .map(|e| urlencoding::decode(e).ok().map(|e| e.trim().to_lowercase()))
                .collect::<Option<Vec<_>>>()?,
        };
        (is_usable_device_id(&id) && !name.trim().is_empty()).then_some(Self { id, name, emails })
    }
}

/// The [`HEADER_RELAY_USER_SIGNATURE`] value for `user` on `claim`.
pub fn sign_user(credential: &str, claim: &str, user: &str) -> String {
    sign(credential, &format!("{claim}{USER_BINDING}{user}"))
}

/// The user A signed for this verified request, if any. A missing or
/// forged signature carries none: only A's signature may name a user.
pub fn signed_user(headers: &HeaderMap, credential: &str) -> Option<RelayedUser> {
    let read = |name: &str| headers.get(name).and_then(|v| v.to_str().ok());
    let claim = read(HEADER_RELAY_CLAIM)?;
    let pairs = [
        (HEADER_RELAY_USER, HEADER_RELAY_USER_SIGNATURE, USER_BINDING),
        (
            LEGACY_HEADER_RELAY_OPERATOR,
            LEGACY_HEADER_RELAY_OPERATOR_SIGNATURE,
            LEGACY_OPERATOR_BINDING,
        ),
    ];
    let (user, signature, binding) = pairs
        .into_iter()
        .find_map(|(value, signature, binding)| Some((read(value)?, read(signature)?, binding)))?;
    let expected = sign(credential, &format!("{claim}{binding}{user}"));
    if !auth::ct_eq(&expected, signature) {
        return None;
    }
    RelayedUser::decode(user)
}

/// The first field of every claim. A new claim shape gets a new version.
const CLAIM_VERSION: &str = "v1";

/// Separates claim fields. `urlencoding` escapes it, so no field contains it.
const CLAIM_SEPARATOR: char = '|';

/// How far a claim's issue time may sit from B's clock, either way.
pub(crate) const CLAIM_MAX_SKEW_SECS: u64 = 300;

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

/// The [`HEADER_RELAY_LOCAL_BROWSER`] value for `claim`.
pub fn sign_local_browser(credential: &str, claim: &str) -> String {
    sign(credential, &format!("{claim}{LOCAL_BROWSER_BINDING}"))
}

/// Does this verified request carry A's stamp that its browser is local to A?
fn stamped_local_browser(headers: &HeaderMap, credential: &str) -> bool {
    let read = |name: &str| headers.get(name).and_then(|v| v.to_str().ok());
    match (read(HEADER_RELAY_CLAIM), read(HEADER_RELAY_LOCAL_BROWSER)) {
        (Some(claim), Some(stamp)) => auth::ct_eq(&sign_local_browser(credential, claim), stamp),
        _ => false,
    }
}

/// Why B refused a relayed request. Each is a 401: A holds a credential, so
/// the reason is for its log, not for a user.
#[derive(Debug, PartialEq, Eq)]
pub enum ClaimRefusal {
    Missing,
    Malformed,
    BadSignature,
    Stale,
    PrefixMismatch,
    BadDeviceId,
    NoDevice,
}

impl ClaimRefusal {
    pub(crate) fn message(&self) -> &'static str {
        match self {
            Self::Missing => "the relayed request carries no claim",
            Self::Malformed => "the relay claim is malformed",
            Self::BadSignature => "the relay claim's signature does not verify",
            Self::Stale => "the relay claim is too old, or from the future",
            Self::PrefixMismatch => "the relay claim names another route",
            Self::BadDeviceId => "the relay claim's device id is not usable",
            Self::NoDevice => "the relay claim names no device",
        }
    }
}

/// B's check of a workspace claim, against the credential the request
/// presented.
///
/// The prefix must be `/<slug>@<name>/` for the slug in the path, so A cannot
/// point B's engine at another workspace's address.
pub fn verify_claim(
    headers: &HeaderMap,
    credential: &str,
    slug: &str,
    now: i64,
) -> Result<RelayClaim, ClaimRefusal> {
    verify_signed_claim(headers, credential, now, |prefix| {
        prefix
            .strip_prefix('/')
            .and_then(|p| p.strip_suffix('/'))
            .and_then(remote_route)
            .is_some_and(|(claimed, _)| claimed == slug)
    })
}

/// The checks every claim kind shares: signature, shape, age and device id.
/// `prefix_fits` decides which route the claim was signed for, so a claim
/// for one route never verifies on another.
pub(crate) fn verify_signed_claim(
    headers: &HeaderMap,
    credential: &str,
    now: i64,
    prefix_fits: impl FnOnce(&str) -> bool,
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
    if !prefix_fits(&claim.prefix) {
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

/// A's half: relay `req`, addressed to `/<slug>@<name>/…`, to remote gateway
/// `name`.
///
/// `enforce` already ran, so the caller is a device A authenticated, or a
/// request A admits without one. The claim says which.
pub async fn relay(state: &GatewayState, slug: &str, name: &str, req: Request) -> Response {
    let route = format!("/{slug}@{name}");
    let peer = req
        .extensions()
        .get::<crate::pairing::Peer>()
        .map(|peer| peer.remote);
    let mut clock = RelayClock::start(req.version(), peer.map(|p| p.port()));
    let flight = state.relay_traffic().enter(&route, peer);
    let response = relay_timed(state, slug, name, &route, req, &mut clock).await;
    crate::relay_timing::timed_response(clock, flight, response)
}

/// [`relay`], marking each stage on `clock` as it passes.
async fn relay_timed(
    state: &GatewayState,
    slug: &str,
    name: &str,
    route: &str,
    req: Request,
    clock: &mut RelayClock,
) -> Response {
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
    let rest = match path_and_query.strip_prefix(route) {
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
    let client = state.peer_connection();
    if crate::remote_browser::worker_request(req.headers()) {
        return (
            StatusCode::FORBIDDEN,
            "remote workspace workers are not allowed",
        )
            .into_response();
    }
    let open_device = navigation
        .then(|| req.extensions().get::<AuthenticatedDevice>().cloned())
        .flatten();
    let probe_key = crate::relay_probe_cache::ProbeKey::new(&remote, slug);
    let run = || probe(state, client, &remote, slug, navigation, open_device, clock);
    let verdict = if navigation {
        run().await
    } else {
        let generation = state.remote_gateways().generation();
        state
            .relay_probes()
            .resolve(&probe_key, generation, run)
            .await
    };
    let verdict = match verdict {
        Ok(verdict) => verdict,
        Err(response) => return response,
    };
    clock.contract = clock.mark();
    if clock.health == Default::default() {
        clock.health = clock.contract;
    }
    let peer = Peer {
        state,
        remote: &remote,
        slug,
        route,
        binding: verdict.binding.as_deref(),
        probe_key: &probe_key,
    };
    let data_path = rest.split('?').next().unwrap_or_default();
    if !crate::remote_browser::is_data_path(data_path) {
        clock.kind = Label("shell");
        if !remote.trusted_pairing {
            return crate::server::serve_remote_client(state, route, rest, req, None).await;
        }
        if !verdict.peer_client {
            let unavailable = Some(remote.name.as_str());
            return crate::server::serve_remote_client(state, route, rest, req, unavailable).await;
        }
        return serve_peer_client(&peer, rest, req).await;
    }
    clock.kind = Label("data");
    let cors_resource = crate::auth_api::is_public_app_asset(req.uri().path())
        || carries_a_remote_frame_capability(&req);
    match peer.send(rest, req, &[]).await {
        Ok(response) => {
            crate::remote_browser::data_response(response, &path_and_query, route, cors_resource)
        }
        Err(response) if navigation && response.status() == StatusCode::BAD_GATEWAY => {
            to_the_list()
        }
        Err(response) => response,
    }
}

/// One relayed workspace on B, as one request reaches it.
struct Peer<'a> {
    state: &'a GatewayState,
    remote: &'a crate::remote_gateways::RemoteGateway,
    slug: &'a str,
    route: &'a str,
    binding: Option<&'a str>,
    probe_key: &'a crate::relay_probe_cache::ProbeKey,
}

impl Peer<'_> {
    /// Send `req` to B for `rest`, signed with the caller's device claim and
    /// the contract binding. `drop` names request headers B must not see.
    async fn send(
        &self,
        rest: &str,
        req: Request,
        drop: &[HeaderName],
    ) -> Result<reqwest::Response, Response> {
        let Peer {
            state,
            remote,
            slug,
            route,
            ..
        } = self;
        let (parts, body) = req.into_parts();
        let device = parts.extensions.get::<AuthenticatedDevice>().cloned();
        let Some((claim, claim_headers)) =
            signed_claim_headers(state, &remote.credential, format!("{route}/"), device)
        else {
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                "the relay claim is not a header value",
            )
                .into_response());
        };
        let local_browser = parts
            .extensions
            .get::<crate::oauth_callback::LocalBrowser>()
            .map(|_| sign_local_browser(&remote.credential, &claim));

        let path = format!("{GATEWAY_API_PREFIX}{RELAY_PATH}/{slug}{rest}");
        let mut headers = HeaderMap::new();
        for (header_name, value) in &parts.headers {
            // A already checked browser origin and the public/capability exemptions.
            // This is now a signed gateway hop, not a browser request at B's origin.
            // Forwarding A's Origin with B's Host makes legitimate writes fail;
            // forwarding an opaque app's metadata also blocks its public resources.
            // B derives device authority only from the signed claim, never cookies.
            let consumed = header_name == header::ORIGIN
                || header_name == header::COOKIE
                || header_name == "sec-fetch-site"
                || drop.contains(header_name);
            if !consumed && !crate::proxy::gateway_owns_header(header_name, false) {
                headers.append(header_name.clone(), value.clone());
            }
        }
        for name in remote.headers.names() {
            headers.remove(name.as_str());
        }
        headers.extend(claim_headers);
        if let Some(stamp) = local_browser.and_then(|s| HeaderValue::from_str(&s).ok()) {
            headers.insert(HEADER_RELAY_LOCAL_BROWSER, stamp);
        }
        let bound = self
            .binding
            .map(|b| sign_contract(&remote.credential, &claim, b));
        if let Some(bound) = bound.and_then(|b| HeaderValue::from_str(&b).ok()) {
            headers.insert(HEADER_RELAY_CONTRACT, bound);
        }

        let client = state.peer_connection();
        let upstream = match client.request(remote, parts.method, &path) {
            Ok(request) => with_body(request.headers(headers), &parts.headers, body),
            Err(reason) => return Err((StatusCode::INTERNAL_SERVER_ERROR, reason).into_response()),
        };
        let response = upstream.send().await;
        if crate::relay_probe_cache::voids_verdict(response.as_ref().ok().map(|r| r.status())) {
            state.relay_probes().evict(self.probe_key);
        }
        response.map_err(|e| {
            let e = e.without_url();
            crate::log_legacy!(
                "[Gateway] relay {} to remote gateway '{}' failed: {}",
                crate::proxy::request_path_for_log(rest),
                remote.name,
                crate::external_address::error_chain(&e)
            );
            (
                StatusCode::BAD_GATEWAY,
                axum::Json(serde_json::json!({
                    "remote_gateway": remote.name,
                    "reason_kind": crate::remote_workspaces::transport_reason(&e),
                    "reason": format!("The remote gateway '{}' could not be reached", remote.name),
                })),
            )
                .into_response()
        })
    }
}

/// The largest shell A reads from B to stamp.
const MAX_PEER_SHELL_BYTES: usize = 4 * 1024 * 1024;

/// Request headers that would let B answer the shell with anything but a
/// complete, uncompressed document A can stamp.
fn shell_fetch_drops() -> [HeaderName; 5] {
    [
        header::ACCEPT_ENCODING,
        header::IF_NONE_MATCH,
        header::IF_MODIFIED_SINCE,
        header::RANGE,
        header::IF_RANGE,
    ]
}

/// Trusted pairing: B's own client, at A's origin. A worker script never
/// comes from B, and A keeps its own manifest.
async fn serve_peer_client(peer: &Peer<'_>, rest: &str, req: Request) -> Response {
    let (state, route, name) = (peer.state, peer.route, peer.remote.name.as_str());
    if req.method() != axum::http::Method::GET && req.method() != axum::http::Method::HEAD {
        return StatusCode::METHOD_NOT_ALLOWED.into_response();
    }
    let path = rest
        .split('?')
        .next()
        .unwrap_or_default()
        .trim_start_matches('/');
    if path == "manifest.json" {
        return crate::server::serve_remote_client(state, route, rest, req, None).await;
    }
    if path == "sw.js" || path.split('/').any(|part| part.starts_with('.')) {
        return StatusCode::NOT_FOUND.into_response();
    }
    if !(path.is_empty() || path == "index.html") {
        let method = req.method().clone();
        return match peer.send(rest, req, &[]).await {
            Ok(response) if response.status() == StatusCode::NOT_FOUND => {
                own_client_asset(peer, rest, method).await
            }
            Ok(response) => peer_client_response(response),
            Err(response) => response,
        };
    }
    if let Some(redirect) = crate::server::remote_client_migration(&req) {
        return redirect;
    }
    let head = req.method() == axum::http::Method::HEAD;
    match peer_shell(peer, rest, req).await {
        Some(html) => {
            let meta = format!(
                "<meta name=\"{}\" content=\"{name}\">",
                crate::server::PEER_CLIENT_META
            );
            let response = crate::server::remote_shell_response(
                &html,
                &crate::server::remote_shell_tags(state, route, &meta),
            );
            if head {
                let (parts, _) = response.into_parts();
                return Response::from_parts(parts, Body::empty());
            }
            response
        }
        None => crate::server::installed_remote_shell(state, route, Some(name)),
    }
}

/// An asset B lacks comes from A's installed client. A fallback shell is A's
/// own build, so its hashed bundle names exist only here.
async fn own_client_asset(peer: &Peer<'_>, rest: &str, method: axum::http::Method) -> Response {
    let Ok(req) = Request::builder().method(method).body(Body::empty()) else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    crate::server::serve_remote_client(peer.state, peer.route, rest, req, None).await
}

/// B's shell document, or `None` when B did not send a complete HTML page
/// within the peer timeout.
async fn peer_shell(peer: &Peer<'_>, rest: &str, req: Request) -> Option<String> {
    tokio::time::timeout(
        crate::peer_connection::PEER_TIMEOUT,
        read_peer_shell(peer, rest, req),
    )
    .await
    .ok()
    .flatten()
}

async fn read_peer_shell(peer: &Peer<'_>, rest: &str, req: Request) -> Option<String> {
    let mut req = req;
    *req.method_mut() = axum::http::Method::GET;
    let response = peer.send(rest, req, &shell_fetch_drops()).await.ok()?;
    let html = response.status() == StatusCode::OK
        && response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v.starts_with("text/html"))
        && !response.headers().contains_key(header::CONTENT_ENCODING);
    if !html {
        crate::log_legacy!(
            "[Gateway] remote gateway '{}' sent no client shell ({}); serving this gateway's",
            peer.remote.name,
            response.status()
        );
        return None;
    }
    let mut stream = response.bytes_stream();
    let mut bytes = Vec::new();
    while let Some(chunk) = stream.try_next().await.ok()? {
        if bytes.len() + chunk.len() > MAX_PEER_SHELL_BYTES {
            return None;
        }
        bytes.extend_from_slice(&chunk);
    }
    String::from_utf8(bytes).ok()
}

/// One of B's client assets, at A's origin. Unlike `data_response` it runs
/// unsandboxed, so it keeps no redirect and no header that sets origin state.
fn peer_client_response(response: reqwest::Response) -> Response {
    if response.status().is_redirection() && response.status() != StatusCode::NOT_MODIFIED {
        return (
            StatusCode::BAD_GATEWAY,
            "the remote client asset answered with a redirect",
        )
            .into_response();
    }
    let mut out = streamed_response(response);
    out.headers_mut().remove(header::LOCATION);
    out.headers_mut().insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    out
}

/// B's health and engine contract probes for one relayed request. `Ok` holds
/// what A learned about B, or `Err` the response that ends the request.
async fn probe(
    state: &GatewayState,
    client: &crate::peer_connection::PeerConnection,
    remote: &crate::remote_gateways::RemoteGateway,
    slug: &str,
    navigation: bool,
    open_device: Option<AuthenticatedDevice>,
    clock: &mut RelayClock,
) -> Result<crate::relay_probe_cache::Verdict, Response> {
    let to_the_list = || redirect(&format!("/{}/?pick", crate::registry::SIGIL));
    let route = format!("/{slug}@{}", remote.name);
    let health = match client.client_health(&remote.address, &remote.headers).await {
        Ok(health) => health,
        Err((reason_kind, reason)) => {
            if navigation {
                return Err(to_the_list());
            }
            return Err((
                StatusCode::BAD_GATEWAY,
                axum::Json(serde_json::json!({
                    "remote_gateway": remote.name,
                    "reason_kind": reason_kind,
                    "reason": reason,
                })),
            )
                .into_response());
        }
    };
    clock.health = clock.mark();
    let Some(gateway) = crate::remote_browser::compatible(&health) else {
        return Err(crate::remote_browser::upgrade_required());
    };
    // Never infer the running engine's API from its gateway's release.
    let open = open_device.is_some();
    let Some((_, contract_headers)) =
        signed_claim_headers(state, &remote.credential, format!("{route}/"), open_device)
    else {
        return Err(StatusCode::INTERNAL_SERVER_ERROR.into_response());
    };
    let suffix = if open { "/open" } else { "" };
    let contract_path = format!("{GATEWAY_API_PREFIX}{RELAY_PATH}/{slug}/~client{suffix}");
    let contract_request = match client.request(remote, axum::http::Method::GET, &contract_path) {
        Ok(request) => request.headers(contract_headers),
        Err(reason) => return Err((StatusCode::BAD_GATEWAY, reason).into_response()),
    };
    let contract_result = tokio::time::timeout(crate::peer_connection::PEER_TIMEOUT, async {
        let response = contract_request
            .send()
            .await
            .map_err(|_| StatusCode::BAD_GATEWAY)?;
        if response.status() == StatusCode::SERVICE_UNAVAILABLE
            && response
                .headers()
                .get(crate::server::WORKSPACE_STATE_HEADER)
                .is_some_and(|value| value == crate::server::WORKSPACE_STOPPED)
        {
            return Ok(crate::remote_browser::EngineClientProbe::Stopped);
        }
        if !response.status().is_success() {
            return Err(match response.status() {
                StatusCode::UNAUTHORIZED
                | StatusCode::FORBIDDEN
                | StatusCode::NOT_FOUND
                | StatusCode::SERVICE_UNAVAILABLE
                | StatusCode::UPGRADE_REQUIRED => response.status(),
                _ => StatusCode::BAD_GATEWAY,
            });
        }
        let starting = response.status() == StatusCode::ACCEPTED;
        // Bound untrusted health bodies, including streamed/chunked responses.
        let mut stream = response.bytes_stream();
        let mut bytes = Vec::new();
        while let Some(chunk) = stream
            .try_next()
            .await
            .map_err(|_| StatusCode::BAD_GATEWAY)?
        {
            if bytes.len() + chunk.len() > crate::remote_browser::MAX_CLIENT_HEALTH_BYTES {
                return Err(StatusCode::UPGRADE_REQUIRED);
            }
            bytes.extend_from_slice(&chunk);
        }
        if starting {
            if !open {
                return Err(StatusCode::BAD_GATEWAY);
            }
            return serde_json::from_slice::<crate::remote_browser::RemoteStartup>(&bytes)
                .map(|startup| crate::remote_browser::EngineClientProbe::Starting(startup.label))
                .map_err(|_| StatusCode::BAD_GATEWAY);
        }
        serde_json::from_slice::<crate::remote_browser::ClientContract>(&bytes)
            .map(crate::remote_browser::EngineClientProbe::Ready)
            .map_err(|_| StatusCode::UPGRADE_REQUIRED)
    })
    .await;
    Err(match contract_result {
        Ok(Ok(crate::remote_browser::EngineClientProbe::Stopped)) => (
            StatusCode::SERVICE_UNAVAILABLE,
            [(
                crate::server::WORKSPACE_STATE_HEADER,
                crate::server::WORKSPACE_STOPPED,
            )],
            "remote workspace engine is stopped",
        )
            .into_response(),
        Ok(Ok(crate::remote_browser::EngineClientProbe::Starting(label))) => {
            crate::proxy::starting_page(&label)
        }
        Ok(Ok(crate::remote_browser::EngineClientProbe::Ready(engine)))
            if engine.supports_engine() =>
        {
            use lucidos_gateway_stamp::workspace_client::{
                CLAIM_BOUND_CONTRACT, PEER_SERVED_CLIENT,
            };
            let checks = gateway.has(CLAIM_BOUND_CONTRACT);
            return Ok(crate::relay_probe_cache::Verdict {
                binding: checks.then(|| crate::remote_browser::contract_binding(&gateway, &engine)),
                peer_client: gateway.has(PEER_SERVED_CLIENT),
            });
        }
        Ok(Ok(_)) | Ok(Err(StatusCode::UPGRADE_REQUIRED)) => {
            crate::remote_browser::upgrade_required()
        }
        Ok(Err(_)) | Err(_) if navigation => to_the_list(),
        Ok(Err(status)) => (
            status,
            "remote workspace client negotiation was refused or unavailable",
        )
            .into_response(),
        Err(_) => (
            StatusCode::GATEWAY_TIMEOUT,
            "remote workspace client negotiation timed out",
        )
            .into_response(),
    })
}

/// `upstream` carrying `body`, as the client's `headers` declare it.
///
/// A body streams through, never buffered. A request that declares none
/// sends none, so a GET does not go out chunked.
pub(crate) fn with_body(
    upstream: reqwest::RequestBuilder,
    headers: &HeaderMap,
    body: Body,
) -> reqwest::RequestBuilder {
    if let Some(length) = declared_body_length(headers) {
        upstream
            .header(header::CONTENT_LENGTH, length)
            .body(reqwest::Body::wrap_stream(body.into_data_stream()))
    } else if headers.contains_key(header::TRANSFER_ENCODING) {
        upstream.body(reqwest::Body::wrap_stream(body.into_data_stream()))
    } else {
        upstream
    }
}

/// B's answer for the client, streamed, with only the headers
/// [`relays_response_header`] lets through.
pub(crate) fn streamed_response(response: reqwest::Response) -> Response {
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

/// The claim A signs for `device` at `prefix`, and the headers that carry it:
/// the claim, its signature, and the signed user when the device has one.
/// `None` when a value cannot be a header.
pub(crate) fn signed_claim_headers(
    state: &GatewayState,
    credential: &str,
    prefix: String,
    device: Option<AuthenticatedDevice>,
) -> Option<(String, HeaderMap)> {
    let user = device.as_ref().and_then(|d| {
        let directory = state.directory();
        let user = directory.user(directory.user_of_device(&d.id)?)?;
        Some(
            RelayedUser {
                id: user.id.clone(),
                name: user.name.clone(),
                emails: user.emails.clone(),
            }
            .encode(),
        )
    });
    let claim = RelayClaim {
        issued_at: chrono::Utc::now().timestamp(),
        prefix,
        device,
    }
    .encode();
    let mut headers = HeaderMap::new();
    headers.insert(HEADER_RELAY_CLAIM, HeaderValue::from_str(&claim).ok()?);
    headers.insert(
        HEADER_RELAY_SIGNATURE,
        HeaderValue::from_str(&sign(credential, &claim)).ok()?,
    );
    if let Some(value) = user {
        let signature = sign_user(credential, &claim, &value);
        let legacy_signature = sign(
            credential,
            &format!("{claim}{LEGACY_OPERATOR_BINDING}{value}"),
        );
        if let (Ok(value), Ok(signature), Ok(legacy_signature)) = (
            HeaderValue::from_str(&value),
            HeaderValue::from_str(&signature),
            HeaderValue::from_str(&legacy_signature),
        ) {
            headers.insert(HEADER_RELAY_USER, value.clone());
            headers.insert(HEADER_RELAY_USER_SIGNATURE, signature);
            headers.insert(LEGACY_HEADER_RELAY_OPERATOR, value);
            headers.insert(LEGACY_HEADER_RELAY_OPERATOR_SIGNATURE, legacy_signature);
        }
    }
    Some((claim, headers))
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
    let mut clock = ReceiveClock::start();
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
            crate::log_legacy!(
                "[Gateway] refused a request relayed by '{}': {}",
                gateway.label,
                refusal.message()
            );
            return refuse(refusal.message());
        }
    };
    clock.verify = clock.mark();
    // This signed pairing-only probe exposes the engine contract, never its
    // health body (which includes private workspace paths and configuration).
    if rest == "/~client" || rest == "/~client/open" {
        if req.method() != axum::http::Method::GET || original.query().is_some() {
            return StatusCode::BAD_REQUEST.into_response();
        }
        let open = rest == "/~client/open";
        if open && claim.device.is_none() {
            return StatusCode::FORBIDDEN.into_response();
        }
        let response = crate::server::engine_client_contract(&state, slug, open).await;
        clock.contract = clock.mark();
        return clock.finish(response);
    }
    if let Some(bound) = req.headers().get(HEADER_RELAY_CONTRACT) {
        let raw_claim = req
            .headers()
            .get(HEADER_RELAY_CLAIM)
            .and_then(|v| v.to_str().ok());
        let signed = |binding: &str| {
            raw_claim.is_some_and(|c| {
                let expected = sign_contract(credential, c, binding);
                bound.to_str().is_ok_and(|b| auth::ct_eq(&expected, b))
            })
        };
        if let Some(refusal) = crate::server::bound_contract_refusal(&state, slug, signed).await {
            return refusal;
        }
    }
    if claim.device.is_none() && crate::oauth_callback::is_relayed_callback(req.method(), &rest) {
        return crate::oauth_callback::deliver_relayed(
            &state,
            slug,
            &claim.prefix,
            original.query(),
        )
        .await;
    }

    let local_path = match original.query() {
        Some(query) => format!("/{slug}{rest}?{query}"),
        None => format!("/{slug}{rest}"),
    };
    let local_browser = stamped_local_browser(req.headers(), credential);
    let user = signed_user(req.headers(), credential);
    let (mut parts, body) = req.into_parts();
    let Ok(uri) = local_path.parse() else {
        return (StatusCode::BAD_REQUEST, "malformed relayed path").into_response();
    };
    parts.uri = uri;
    parts.extensions.remove::<AuthenticatedDevice>();
    parts
        .extensions
        .remove::<crate::oauth_callback::LocalBrowser>();
    if local_browser {
        parts.extensions.insert(crate::oauth_callback::LocalBrowser);
    }
    let device = claim.device.map(|d| AuthenticatedDevice {
        id: namespaced_device_id(&gateway.pairing_id, &d.id),
        label: d.label,
    });
    let has_device = device.is_some();
    if let (Some(device), Some(user)) = (&device, user) {
        record_relayed_user(&state, &gateway, device, user);
    }
    if let Some(device) = device {
        parts.extensions.insert(device);
    }
    let local = Request::from_parts(parts, body);
    // A claim with no device reaches only what B admits without one: the OAuth
    // callback above, public app assets, and a frame capability. B, not A,
    // verifies a frame capability: B's engine minted it.
    if !has_device
        && !crate::auth_api::is_public_app_asset(local.uri().path())
        && !crate::auth_api::frame_capability_admits(&state, &local)
    {
        return refuse("a relayed request with no device reaches only public app assets");
    }
    let response = crate::server::serve_workspace(&state, slug, &claim.prefix, local).await;
    clock.engine_first_byte = clock.mark();
    clock.finish(response)
}

/// Remember the user A signed for a relayed device, when it changed, so
/// B's engines can name the user. A failed write costs only the display.
fn record_relayed_user(
    state: &GatewayState,
    gateway: &AuthenticatedGateway,
    device: &AuthenticatedDevice,
    user: RelayedUser,
) {
    let seen = crate::gateway_event_log::RelayedDevice {
        device_id: device.id.clone(),
        user_id: namespaced_device_id(&gateway.pairing_id, &user.id),
        name: user.name,
        via: gateway.label.clone(),
        emails: user.emails,
    };
    if state.directory().relayed.contains(&seen) {
        return;
    }
    // Judged under the log lock: an unpair that already ran has forgotten
    // this pairing's users, so a request still in flight must not add one.
    let pairing_id = &gateway.pairing_id;
    let recorded = state.record(|directory| {
        let paired = state
            .paired_gateways()
            .list()
            .iter()
            .any(|g| &g.id == pairing_id);
        Ok(paired
            .then(|| directory.relayed_seen(seen))
            .flatten()
            .into_iter()
            .collect())
    });
    if let Err(e) = recorded {
        crate::log_legacy!("[Gateway] could not record a relayed device's user: {e}");
    }
}

#[cfg(test)]
#[path = "relay_tests.rs"]
pub(crate) mod tests;
