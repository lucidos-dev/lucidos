//! The loopback OAuth callback: the gateway owns the port and routes by `state`.
//!
//! No engine binds `14981` while a gateway runs, because the flow's engine may
//! run on another install. This gateway binds it for its whole life and routes
//! each callback by the prefix the engine wrote into `state` (`/dev/~<nonce>`).
//!
//! - Only `code`, `state`, `error`, `error_description` and `client_id` cross.
//!   The engine checks the nonce and renders the page. This module only routes.
//! - A `/dev@cloud/~<nonce>` callback crosses the relay as a device-less claim.
//! - With no own engine claiming it, the callback goes to each same-machine peer
//!   at [`HANDOFF_PATH`]. A gateway that lost the bind keeps retrying.
//! - Only a [`LocalBrowser`] gets the loopback URI. Others use [`web_callback`].
//!
//! Why: ADR 0622.

use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr};
use std::time::Duration;

use axum::body::Body;
use axum::extract::{RawQuery, State};
use axum::http::{HeaderMap, Method, Request, StatusCode};
use axum::response::{Html, IntoResponse, Response};
use axum::{Extension, Router};
pub use lucidos_gateway_constants::HEADER_LOCAL_BROWSER;
use lucidos_gateway_constants::{
    OAUTH_CALLBACK_PATH, OAUTH_CALLBACK_PORT, OAUTH_DELIVERY_ROUTE, OAUTH_STATE_PREFIX_SEPARATOR,
};

use crate::engine_hop::{self, Credential};
use crate::pairing::{is_forwarded, Peer};
use crate::server::GatewayState;

/// Where a peer gateway on this machine takes a handed-off callback. Served on
/// the main listener to loopback callers only, and never handed on again.
pub const HANDOFF_PATH: &str = "/~/oauth/handoff";

/// Marks a response as the hand-off route's own. A gateway that predates the
/// route answers its path with the picker shell, which is no claim.
pub const HANDOFF_ANSWERED_HEADER: &str = "x-lucidos-oauth-handoff";

/// The callback fields a provider sends, and the only ones forwarded. Sign in
/// with ChatGPT returns the issued `client_id` beside the code (ADR 0636).
const FORWARDED_FIELDS: [&str; 5] = ["code", "state", "error", "error_description", "client_id"];

/// How long the engine may take to settle a delivered callback.
const ENGINE_TIMEOUT: Duration = Duration::from_secs(10);

/// How long a remote gateway may take, its own engine wait included.
const RELAY_TIMEOUT: Duration = Duration::from_secs(15);

/// The largest page accepted from an engine.
const MAX_PAGE_BYTES: usize = 64 * 1024;

/// Callbacks in flight at once. A user finishes one authorization at a time.
const MAX_CONCURRENT_CALLBACKS: usize = 16;

/// How often a lost bind is retried. One syscall, so a short wait costs
/// nothing and a quitting owner is replaced within seconds.
const BIND_RETRY: Duration = Duration::from_secs(2);

/// The two addresses the callback port binds. Every loopback host form a
/// redirect URI may name resolves to one of them.
pub fn bind_addrs() -> [SocketAddr; 2] {
    [
        SocketAddr::from((Ipv4Addr::LOCALHOST, OAUTH_CALLBACK_PORT)),
        SocketAddr::from((Ipv6Addr::LOCALHOST, OAUTH_CALLBACK_PORT)),
    ]
}

/// The callback port's router: one route, and 404 for all else.
pub fn router(state: GatewayState) -> Router {
    Router::new()
        .route(OAUTH_CALLBACK_PATH, axum::routing::get(callback))
        .fallback(|| async { StatusCode::NOT_FOUND })
        .method_not_allowed_fallback(|| async { StatusCode::NOT_FOUND })
        .with_state(state)
        .layer(tower::limit::ConcurrencyLimitLayer::new(
            MAX_CONCURRENT_CALLBACKS,
        ))
        .layer(tower_http::catch_panic::CatchPanicLayer::new())
}

/// Hold one callback address for the life of the process.
///
/// Another install's gateway may hold it. Retrying means this gateway takes
/// over when that one quits. Logs once per state change, not per attempt.
pub async fn serve(addr: SocketAddr, state: GatewayState, handle: axum_server::Handle) {
    let mut reported: Option<std::io::ErrorKind> = None;
    loop {
        match std::net::TcpListener::bind(addr) {
            Ok(listener) => {
                crate::log!("[OAuth] callback listener on http://{addr}");
                reported = None;
                let serving = crate::server::serve_listener(
                    listener,
                    router(state.clone()),
                    handle.clone(),
                    crate::tls::ListenerTls::Plain,
                );
                // `Ok` is the shutdown handle firing.
                match serving.await {
                    Ok(()) => return,
                    Err(e) => crate::log!("[OAuth] callback listener on {addr} stopped ({e})"),
                }
            }
            Err(e) => {
                if reported != Some(e.kind()) {
                    reported = Some(e.kind());
                    crate::log!(
                        "[OAuth] cannot bind the callback port {addr} ({e}); retrying every {}s. \
                         Another Lucidos install on this machine may hold it, and hands \
                         callbacks on to this one",
                        BIND_RETRY.as_secs()
                    );
                }
            }
        }
        tokio::time::sleep(BIND_RETRY).await;
    }
}

/// A browser's callback on the loopback port.
async fn callback(
    State(state): State<GatewayState>,
    peer: Option<Extension<Peer>>,
    headers: HeaderMap,
    RawQuery(query): RawQuery,
) -> Response {
    if !from_this_machine(peer.as_deref(), &headers) {
        return StatusCode::NOT_FOUND.into_response();
    }
    let query = forwarded_query(query.as_deref().unwrap_or(""));
    let (status, page) = route_callback(&state, &query, || {
        crate::peers::peer_gateway_ports(&state.install_inventory(), state.gateway_port())
    })
    .await;
    (status, Html(page)).into_response()
}

/// A peer gateway's hand-off. Routed to this gateway's own workspaces only,
/// local or remote, so a callback never bounces between installs.
pub async fn handoff(
    State(state): State<GatewayState>,
    peer: Option<Extension<Peer>>,
    headers: HeaderMap,
    RawQuery(query): RawQuery,
) -> Response {
    if !from_this_machine(peer.as_deref(), &headers) {
        return StatusCode::NOT_FOUND.into_response();
    }
    let query = forwarded_query(query.as_deref().unwrap_or(""));
    let (status, page) = deliver_to_own(&state, &query).await.into_answer();
    (status, [(HANDOFF_ANSWERED_HEADER, "1")], Html(page)).into_response()
}

/// A browser's callback on this gateway's own origin, through a *web redirect
/// URI*. Public, since the provider sends the browser here with no credential.
///
/// The URI names this gateway, so the `state` must name one of its own
/// workspaces, local or remote. It is never handed to a peer install.
///
/// Anyone can call it, and each call may wait on a relay, so it takes at most
/// [`MAX_CONCURRENT_CALLBACKS`] at once, as the callback port does.
pub async fn web_callback(
    State(state): State<GatewayState>,
    RawQuery(query): RawQuery,
) -> Response {
    static IN_FLIGHT: tokio::sync::Semaphore =
        tokio::sync::Semaphore::const_new(MAX_CONCURRENT_CALLBACKS);
    let Ok(_permit) = IN_FLIGHT.try_acquire() else {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };
    let query = forwarded_query(query.as_deref().unwrap_or(""));
    let (status, page) = deliver_to_own(&state, &query).await.into_answer();
    (status, Html(page)).into_response()
}

/// Marks a request whose browser runs on this gateway's machine. The proxy
/// turns it into [`HEADER_LOCAL_BROWSER`], and the relay into a signed header.
#[derive(Clone, Copy, Debug)]
pub struct LocalBrowser;

/// Mark `req` as a [`LocalBrowser`] when it came straight from this machine.
///
/// Called where a browser's request enters, never on the relay's receiving
/// side: there the TCP peer is the other gateway.
pub fn stamp_locality(req: &mut axum::extract::Request) {
    if from_this_machine(req.extensions().get::<Peer>(), req.headers()) {
        req.extensions_mut().insert(LocalBrowser);
    }
}

/// Is this relayed request, past `/relay/<slug>`, the callback delivery? The
/// one engine route a remote gateway reaches with a device-less claim.
pub fn is_relayed_callback(method: &Method, rest: &str) -> bool {
    method == Method::GET && rest == delivery_path()
}

/// B's half of a remote workspace's callback: deliver it to `slug`'s engine.
///
/// `prefix` is the verified claim's, `/<slug>@<name>/`. The `state` must carry
/// that same prefix, so A cannot route one workspace's code to another.
pub async fn deliver_relayed(
    state: &GatewayState,
    slug: &str,
    prefix: &str,
    raw_query: Option<&str>,
) -> Response {
    let query = forwarded_query(raw_query.unwrap_or(""));
    let names_prefix = query_value(&query, "state")
        .and_then(|s| {
            s.rsplit_once(OAUTH_STATE_PREFIX_SEPARATOR)
                .map(|(p, _)| p == prefix)
        })
        .unwrap_or(false);
    let delivery = if names_prefix {
        deliver_to_engine(state, slug, &query).await
    } else {
        Delivery::Unclaimed
    };
    let (status, page) = delivery.into_answer();
    (status, Html(page)).into_response()
}

/// Did this request come straight from a process on this machine?
///
/// A [`Peer::is_same_machine`] caller with no forwarding header. A tunnel or
/// reverse proxy on this machine also connects locally, and the header gives
/// it away.
fn from_this_machine(peer: Option<&Peer>, headers: &HeaderMap) -> bool {
    peer.is_some_and(|peer| {
        !peer.public_listener && !is_forwarded(headers) && peer.is_same_machine()
    })
}

/// Route a callback: this gateway's own engines first, then each peer.
///
/// Stops at the first answer that is not a 404, so a code reaches at most one
/// engine that accepts it. `peer_ports` runs only when no own engine claims it.
async fn route_callback(
    state: &GatewayState,
    query: &str,
    peer_ports: impl FnOnce() -> Vec<u16>,
) -> (StatusCode, String) {
    if let Delivery::Answered { status, page } = deliver_to_own(state, query).await {
        return (status, page);
    }
    let client = crate::peers::probe_client();
    for port in peer_ports() {
        match crate::peers::hand_off_oauth_callback(&client, port, query).await {
            Some((404, _)) | None => {}
            Some((status, page)) => {
                let status = StatusCode::from_u16(status).unwrap_or(StatusCode::BAD_GATEWAY);
                return (status, page);
            }
        }
    }
    crate::log!("[OAuth] no workspace on this machine claimed a callback");
    Delivery::Unclaimed.into_answer()
}

/// What this gateway's own engines made of a callback.
enum Delivery {
    /// An engine owned the `state`, and rendered this page.
    Answered { status: StatusCode, page: String },
    /// No engine here took it. Every such answer is the same [`failure_page`],
    /// so a public caller cannot tell a workspace that exists from one that
    /// does not.
    Unclaimed,
}

impl Delivery {
    /// The status and page for the browser. Unclaimed is a 404.
    fn into_answer(self) -> (StatusCode, String) {
        match self {
            Self::Answered { status, page } => (status, page),
            Self::Unclaimed => (StatusCode::NOT_FOUND, failure_page()),
        }
    }

    /// Read an engine's answer, relayed or not.
    ///
    /// The delivery route answers 200 or 400 for a `state` it owns, and 404 for
    /// one it does not. Anything else is a refusal or a restart before the
    /// route ran, so the callback may still belong to a peer.
    async fn read(status: StatusCode, body: Body) -> Self {
        if !matches!(status, StatusCode::OK | StatusCode::BAD_REQUEST) {
            return Self::Unclaimed;
        }
        let page = axum::body::to_bytes(body, MAX_PAGE_BYTES)
            .await
            .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
            .unwrap_or_else(|_| failure_page());
        Self::Answered { status, page }
    }
}

/// The engine's delivery route, as a path.
fn delivery_path() -> String {
    format!("/api/v1{OAUTH_DELIVERY_ROUTE}")
}

/// Deliver a callback to the workspace its `state` prefix names: a local
/// engine, or a remote gateway's workspace over the relay.
async fn deliver_to_own(state: &GatewayState, query: &str) -> Delivery {
    let Some(target) = query_value(query, "state") else {
        return Delivery::Unclaimed;
    };
    match state_route(&target) {
        Some(StateRoute::Local(slug)) => deliver_to_engine(state, slug, query).await,
        Some(StateRoute::Remote { slug, name }) => {
            deliver_to_remote(state, slug, name, query).await
        }
        None => Delivery::Unclaimed,
    }
}

/// Relay a callback to remote gateway `name` as a claim with no device.
///
/// B delivers it to its engine for `slug`. Anything but an engine's answer is
/// B refusing or unreachable, and counts as unclaimed.
async fn deliver_to_remote(state: &GatewayState, slug: &str, name: &str, query: &str) -> Delivery {
    if !state
        .remote_gateways()
        .list()
        .iter()
        .any(|g| g.name == name)
    {
        return Delivery::Unclaimed;
    }
    let route = format!("/{slug}@{name}{}?{query}", delivery_path());
    let Ok(request) = Request::get(route).body(Body::empty()) else {
        return Delivery::Unclaimed;
    };
    let relayed = tokio::time::timeout(
        RELAY_TIMEOUT,
        crate::relay::relay(state, slug, name, request),
    )
    .await;
    let Ok(response) = relayed else {
        crate::log!(
            "[OAuth] remote gateway '{name}' took over {RELAY_TIMEOUT:?} to take a callback"
        );
        return Delivery::Unclaimed;
    };
    let status = response.status();
    if !matches!(
        status,
        StatusCode::OK | StatusCode::BAD_REQUEST | StatusCode::NOT_FOUND
    ) {
        crate::log!(
            "[OAuth] remote gateway '{name}' did not take a callback for '{slug}' ({status})"
        );
    }
    Delivery::read(status, response.into_body()).await
}

/// Deliver a callback to this gateway's engine for `slug`.
async fn deliver_to_engine(state: &GatewayState, slug: &str, query: &str) -> Delivery {
    let Some(engine) = state.engine_address(slug) else {
        return Delivery::Unclaimed;
    };
    let Ok(request) = Request::get(format!("{}?{query}", delivery_path())).body(Body::empty())
    else {
        return Delivery::Unclaimed;
    };
    let sent = tokio::time::timeout(
        ENGINE_TIMEOUT,
        engine_hop::send(&engine, request, Credential::Local(state.local_token())),
    )
    .await;
    let upstream = match sent {
        Ok(Ok(upstream)) => upstream,
        Ok(Err(e)) => {
            crate::log!("[OAuth] '{slug}' could not take a callback: {e}");
            return Delivery::Unclaimed;
        }
        Err(_) => {
            crate::log!("[OAuth] '{slug}' took over {ENGINE_TIMEOUT:?} to take a callback");
            return Delivery::Unclaimed;
        }
    };
    let status = upstream.status();
    Delivery::read(status, Body::new(upstream.into_body())).await
}

/// The callback query, cut down to [`FORWARDED_FIELDS`].
///
/// Each pair crosses byte for byte, since the engine decodes a code with `+`
/// literal and a re-encode would change it. The first `<field>=` pair wins,
/// matched as the engine matches it.
pub fn forwarded_query(raw: &str) -> String {
    FORWARDED_FIELDS
        .iter()
        .filter_map(|field| {
            raw.split('&').find(|pair| {
                pair.strip_prefix(field)
                    .is_some_and(|value| value.starts_with('='))
            })
        })
        .collect::<Vec<_>>()
        .join("&")
}

/// The first value of `field` in a form-encoded query, decoded.
fn query_value(query: &str, field: &str) -> Option<String> {
    let url = reqwest::Url::parse(&format!("http://callback/?{query}")).ok()?;
    url.query_pairs()
        .find(|(key, _)| key == field)
        .map(|(_, value)| value.into_owned())
}

/// Where a `state` prefix points.
#[derive(Debug, PartialEq, Eq)]
enum StateRoute<'a> {
    /// `/dev/~<nonce>`: a workspace this gateway serves.
    Local(&'a str),
    /// `/dev@cloud/~<nonce>`: workspace `slug` on remote gateway `name`.
    Remote { slug: &'a str, name: &'a str },
}

/// Read the route from a `state`. `None` when it carries no valid prefix.
fn state_route(state: &str) -> Option<StateRoute<'_>> {
    let (prefix, _nonce) = state.rsplit_once(OAUTH_STATE_PREFIX_SEPARATOR)?;
    let segment = prefix.strip_prefix('/')?.strip_suffix('/')?;
    if let Some((slug, name)) = crate::relay::remote_route(segment) {
        return Some(StateRoute::Remote { slug, name });
    }
    lucidos_gateway_constants::is_valid_workspace_id(segment).then_some(StateRoute::Local(segment))
}

/// The page a browser gets when no engine on this machine claimed its
/// callback, and none rendered a page of its own.
fn failure_page() -> String {
    "<!DOCTYPE html><html lang=\"en\"><head><meta charset=\"utf-8\">\
     <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
     <title>Lucidos</title><style>:root{color-scheme:dark}\
     body{font:1rem/1.5 system-ui,sans-serif;max-width:32rem;margin:4rem auto;padding:0 1.5rem}\
     </style></head><body><h1>Authorization failed</h1>\
     <p>No Lucidos workspace on this computer was waiting for this sign-in. \
     Nothing was connected. Return to Lucidos and start the connection again.</p>\
     </body></html>"
        .to_string()
}

#[cfg(test)]
#[path = "oauth_callback_tests.rs"]
mod tests;
