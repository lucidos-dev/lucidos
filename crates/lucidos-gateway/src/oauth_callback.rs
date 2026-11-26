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

pub const HANDOFF_PATH: &str = "/~/oauth/handoff";

pub const HANDOFF_ANSWERED_HEADER: &str = "x-lucidos-oauth-handoff";

const FORWARDED_FIELDS: [&str; 5] = ["code", "state", "error", "error_description", "client_id"];

const ENGINE_TIMEOUT: Duration = lucidos_engine_socket::OAUTH_CALLBACK_HOP_TIMEOUT;

const RELAY_TIMEOUT: Duration = ENGINE_TIMEOUT.saturating_add(Duration::from_secs(5));

pub(crate) const HANDOFF_TIMEOUT: Duration = RELAY_TIMEOUT.saturating_add(Duration::from_secs(2));

const MAX_PAGE_BYTES: usize = 64 * 1024;

const MAX_CONCURRENT_CALLBACKS: usize = 16;

const BIND_RETRY: Duration = Duration::from_secs(2);

pub fn bind_addrs() -> [SocketAddr; 2] {
    [
        SocketAddr::from((Ipv4Addr::LOCALHOST, OAUTH_CALLBACK_PORT)),
        SocketAddr::from((Ipv6Addr::LOCALHOST, OAUTH_CALLBACK_PORT)),
    ]
}

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

pub async fn serve(addr: SocketAddr, state: GatewayState, handle: axum_server::Handle) {
    let mut reported: Option<std::io::ErrorKind> = None;
    loop {
        match std::net::TcpListener::bind(addr) {
            Ok(listener) => {
                crate::log_legacy!("[OAuth] callback listener on http://{addr}");
                reported = None;
                let serving = crate::server::serve_listener(
                    listener,
                    router(state.clone()),
                    handle.clone(),
                    lucidos_gateway_tls::ListenerTls::Plain,
                );
                match serving.await {
                    Ok(()) => return,
                    Err(e) => {
                        crate::log_legacy!("[OAuth] callback listener on {addr} stopped ({e})")
                    }
                }
            }
            Err(e) => {
                if reported != Some(e.kind()) {
                    reported = Some(e.kind());
                    crate::log_legacy!(
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

#[derive(Clone, Copy, Debug)]
pub struct LocalBrowser;

pub fn stamp_locality(req: &mut axum::extract::Request) {
    if from_this_machine(req.extensions().get::<Peer>(), req.headers()) {
        req.extensions_mut().insert(LocalBrowser);
    }
}

pub fn is_relayed_callback(method: &Method, rest: &str) -> bool {
    method == Method::GET && rest == delivery_path()
}

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

fn from_this_machine(peer: Option<&Peer>, headers: &HeaderMap) -> bool {
    peer.is_some_and(|peer| {
        !peer.public_listener && !is_forwarded(headers) && peer.is_same_machine()
    })
}

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
    crate::log_legacy!("[OAuth] no workspace on this machine claimed a callback");
    Delivery::Unclaimed.into_answer()
}

enum Delivery {
    Answered { status: StatusCode, page: String },
    Unclaimed,
}

impl Delivery {
    fn into_answer(self) -> (StatusCode, String) {
        match self {
            Self::Answered { status, page } => (status, page),
            Self::Unclaimed => (StatusCode::NOT_FOUND, failure_page()),
        }
    }

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

fn delivery_path() -> String {
    format!("/api/v1{OAUTH_DELIVERY_ROUTE}")
}

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
        crate::log_legacy!(
            "[OAuth] remote gateway '{name}' took over {RELAY_TIMEOUT:?} to take a callback"
        );
        return Delivery::Unclaimed;
    };
    let status = response.status();
    if !matches!(
        status,
        StatusCode::OK | StatusCode::BAD_REQUEST | StatusCode::NOT_FOUND
    ) {
        crate::log_legacy!(
            "[OAuth] remote gateway '{name}' did not take a callback for '{slug}' ({status})"
        );
    }
    match status {
        StatusCode::OK => Delivery::Answered {
            status,
            page: "<!doctype html><meta charset=\"utf-8\"><title>Authorization complete</title><p>Authorization complete. Return to Lucidos to continue.</p>".into(),
        },
        StatusCode::BAD_REQUEST => Delivery::Answered { status, page: failure_page() },
        _ => Delivery::Unclaimed,
    }
}

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
            crate::log_legacy!("[OAuth] '{slug}' could not take a callback: {e}");
            return Delivery::Unclaimed;
        }
        Err(_) => {
            crate::log_legacy!("[OAuth] '{slug}' took over {ENGINE_TIMEOUT:?} to take a callback");
            return Delivery::Unclaimed;
        }
    };
    let status = upstream.status();
    Delivery::read(status, Body::new(upstream.into_body())).await
}

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

fn query_value(query: &str, field: &str) -> Option<String> {
    let url = reqwest::Url::parse(&format!("http://callback/?{query}")).ok()?;
    url.query_pairs()
        .find(|(key, _)| key == field)
        .map(|(_, value)| value.into_owned())
}

#[derive(Debug, PartialEq, Eq)]
enum StateRoute<'a> {
    Local(&'a str),
    Remote { slug: &'a str, name: &'a str },
}

fn state_route(state: &str) -> Option<StateRoute<'_>> {
    let (prefix, _nonce) = state.rsplit_once(OAUTH_STATE_PREFIX_SEPARATOR)?;
    let segment = prefix.strip_prefix('/')?.strip_suffix('/')?;
    if let Some((slug, name)) = crate::relay::remote_route(segment) {
        return Some(StateRoute::Remote { slug, name });
    }
    lucidos_gateway_constants::is_valid_workspace_id(segment).then_some(StateRoute::Local(segment))
}

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
