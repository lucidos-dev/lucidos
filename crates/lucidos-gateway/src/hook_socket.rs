//! The hook socket: a second listener that answers webhook deliveries only.
//!
//! Full design:
//! `docs/plans/2026-08-19-webhooks-and-engines-off-the-network.md`.
//!
//! # Why a separate socket rather than a path
//!
//! This is the one surface a user may deliberately expose to the open
//! internet, with `tailscale funnel`. Funnel maps a port, not a path. So a
//! public caller reaching this listener must be structurally unable to address
//! the control plane or a workspace, whatever it sends.
//!
//! One route, and an explicit 404 for everything else. Nothing here shares a
//! `Router` with the main surface, and the pairing middleware is deliberately
//! absent: a webhook sender holds no device credential.

//! # It forwards, it does not decide
//!
//! Auth is the engine's, because the secret is. This resolves a slug to a
//! loopback engine port and streams the body through untouched. A signature is
//! computed over exact bytes, so any reserialization here would break every
//! signed sender.

use crate::registry::SIGIL;
use crate::server::GatewayState;
use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Router;
use std::net::SocketAddr;
use std::time::Duration;

/// Biggest delivery accepted. Generous for a JSON payload and small enough that
/// a public endpoint cannot be used to fill memory.
const MAX_BODY_BYTES: usize = 1024 * 1024;

/// How long one delivery may take, engine hop included.
const DELIVERY_TIMEOUT: Duration = Duration::from_secs(30);

/// Deliveries in flight at once. A public port needs a ceiling, and a sender
/// that exceeds it gets a 503 it will retry rather than an engine under load.
const MAX_CONCURRENT_DELIVERIES: usize = 16;

/// How far above the gateway's port the hook socket sits.
///
/// Derived rather than fixed, so dev (5251) and packaged (5252) get 5261 and
/// 5262 and coexist exactly as their gateways do. `LUCIDOS_HOOK_PORT` overrides
/// it, and `0` switches the socket off entirely.
const HOOK_PORT_OFFSET: u16 = 10;

/// Resolve the hook port from the environment and the gateway's own port.
///
/// `None` means no hook socket. That is what an explicit `0` asks for, and what
/// an unparseable value gets: a listener on a port nobody meant to open is
/// worse than none.
pub fn hook_port(configured: Option<&str>, gateway_port: u16) -> Option<u16> {
    match configured.map(str::trim) {
        Some(raw) if !raw.is_empty() => match raw.parse::<u16>() {
            Ok(0) => None,
            Ok(port) => Some(port),
            Err(_) => {
                crate::log!("[Hook] LUCIDOS_HOOK_PORT '{raw}' is not a port, so no hook socket");
                None
            }
        },
        _ => gateway_port.checked_add(HOOK_PORT_OFFSET),
    }
}

/// The hook socket's router: one route, and 404 for all else.
pub fn router(state: GatewayState) -> Router {
    let routes = Router::new()
        .route("/:slug/:hook_id", axum::routing::post(deliver))
        // Everything the main surface answers, this one must not. An explicit
        // fallback says so, rather than leaving it to the absence of a route.
        .fallback(|| async { StatusCode::NOT_FOUND })
        // A wrong method on the delivery path answers 404 as well, not 405. On
        // a public socket every refusal should look the same, so probing tells
        // a caller nothing about which paths are real.
        .method_not_allowed_fallback(|| async { StatusCode::NOT_FOUND })
        .with_state(state);
    protect(routes)
}

/// The limits every request to this socket passes through.
///
/// Its own function so a test can drive the same stack over a route that
/// misbehaves on purpose. Verifying panic containment against the real handler
/// would mean giving that handler a way to panic on request.
fn protect(routes: Router) -> Router {
    routes
        .layer(axum::extract::DefaultBodyLimit::max(MAX_BODY_BYTES))
        .layer(tower_http::timeout::TimeoutLayer::with_status_code(
            StatusCode::GATEWAY_TIMEOUT,
            DELIVERY_TIMEOUT,
        ))
        .layer(tower::limit::ConcurrencyLimitLayer::new(
            MAX_CONCURRENT_DELIVERIES,
        ))
        // The hook path shares this process with every workspace's supervisor.
        // An unwinding panic here would take the gateway down with it, so it
        // becomes a 500 instead.
        .layer(tower_http::catch_panic::CatchPanicLayer::new())
}

/// Build the loopback webhook route without letting a decoded path capture
/// become URL syntax.
fn engine_delivery_url(
    scheme: &str,
    port: u16,
    hook_id: &str,
) -> Result<reqwest::Url, &'static str> {
    // The URL standard treats these two complete segments as navigation, and
    // `Url::path_segments_mut` therefore discards them. Refuse them instead of
    // letting caller input remove a segment from the fixed engine route.
    if matches!(hook_id, "." | "..") {
        return Err("hook ID is a URL dot segment");
    }
    let mut url = reqwest::Url::parse(&format!("{scheme}://127.0.0.1:{port}/api/v1/webhooks/"))
        .map_err(|_| "invalid engine URL")?;
    url.path_segments_mut()
        .map_err(|_| "engine URL cannot carry path segments")?
        .pop_if_empty()
        .push(hook_id)
        .push("deliver");
    Ok(url)
}

/// Forward one delivery to the workspace's engine.
async fn deliver(
    State(state): State<GatewayState>,
    Path((slug, hook_id)): Path<(String, String)>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    // The sigil namespace is the gateway's own, and no workspace can be called
    // it (a slug is `[a-z0-9-]`). Refusing it by name as well costs nothing and
    // makes the intent readable.
    if slug.len() == 1 && slug.starts_with(SIGIL) {
        return StatusCode::NOT_FOUND.into_response();
    }
    let Some(port) = state.engine_port(&slug) else {
        // A slug nobody registered and a workspace that is stopped answer
        // alike. Neither can take a delivery, and distinguishing them tells a
        // public caller which workspaces exist.
        return StatusCode::NOT_FOUND.into_response();
    };

    let Ok(url) = engine_delivery_url(state.engine_scheme(), port, &hook_id) else {
        crate::log!("[Hook] '{slug}' delivery could not construct the engine URL");
        return StatusCode::BAD_GATEWAY.into_response();
    };
    let mut request = state.engine_client().post(url);
    // The sender's headers ride along, because the signature was computed over
    // some of them and this listener cannot know which.
    for (name, value) in &headers {
        if !forwarded_to_engine(name.as_str()) {
            continue;
        }
        request = request.header(name.as_str(), value);
    }

    match request.body(body).send().await {
        Ok(upstream) => {
            let status = upstream.status();
            let bytes = upstream.bytes().await.unwrap_or_default();
            (status, Body::from(bytes)).into_response()
        }
        Err(e) => {
            crate::log!("[Hook] '{slug}' delivery could not reach the engine: {e}");
            StatusCode::BAD_GATEWAY.into_response()
        }
    }
}

/// May this header reach the engine from a public caller?
///
/// The default is yes. A signature may cover any header, and this listener
/// holds no webhook config to tell which. Dropped instead is the set that MEANS
/// something to the engine, and is therefore a lie in the hands of whoever
/// posted the delivery. No signature scheme signs one of these, so dropping
/// them breaks no sender.
///
/// Caller passes a lowercase name, which `HeaderName::as_str` guarantees.
fn forwarded_to_engine(name_lower: &str) -> bool {
    // Framing, which reqwest sets for itself off the body it is given.
    let framing = matches!(name_lower, "host" | "content-length");
    // Trust headers. `x-lucidos-local-token` proves a caller is a process on
    // this machine. `x-lucidos-agent-origin-token` attributes a request to a
    // spawning thread. `x-lucidos-target-workspace` can make the engine refuse
    // the delivery outright. A cookie is the gateway's own credential shape.
    let ours = name_lower.starts_with("x-lucidos-") || name_lower == "cookie";
    // Browser-origin metadata, which the engine's same-origin gate reads
    // (`api::browser_origin`). A sender that happens to set `Origin` would
    // otherwise be refused before reaching the webhook route at all.
    let browser =
        matches!(name_lower, "origin" | "referer") || name_lower.starts_with("sec-fetch-");
    !(framing || ours || browser || is_hop_by_hop(name_lower))
}

/// Hop-by-hop headers per RFC 7230 §6.1.
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

/// The address the hook socket binds.
///
/// Loopback only, always. `tailscale funnel` proxies from this machine, so it
/// reaches loopback, while nothing else on the network can address the socket
/// directly. That is the same reasoning that makes a loopback peer address
/// prove nothing about a caller (ADR 0094).
pub fn bind_addr(port: u16) -> SocketAddr {
    SocketAddr::from((std::net::Ipv4Addr::LOCALHOST, port))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt as _;

    #[test]
    fn the_hook_port_derives_from_the_gateway_port() {
        assert_eq!(hook_port(None, 5251), Some(5261));
        assert_eq!(hook_port(None, 5252), Some(5262));
        assert_eq!(hook_port(Some(""), 5251), Some(5261));
        assert_eq!(hook_port(Some("  "), 5251), Some(5261));
    }

    #[test]
    fn an_explicit_port_wins_and_zero_switches_the_socket_off() {
        assert_eq!(hook_port(Some("9000"), 5251), Some(9000));
        assert_eq!(hook_port(Some(" 9000 "), 5251), Some(9000));
        assert_eq!(hook_port(Some("0"), 5251), None);
    }

    #[test]
    fn a_value_that_is_not_a_port_opens_nothing() {
        // A listener on a port nobody meant to open is worse than no listener.
        assert_eq!(hook_port(Some("nonsense"), 5251), None);
        assert_eq!(hook_port(Some("70000"), 5251), None);
    }

    #[test]
    fn the_socket_binds_loopback_whatever_the_port() {
        assert_eq!(bind_addr(5261).to_string(), "127.0.0.1:5261");
        assert!(bind_addr(9000).ip().is_loopback());
    }

    #[test]
    fn a_hook_id_stays_inside_one_engine_url_segment() {
        for (hook_id, encoded) in [
            ("../chat?", "..%2Fchat%3F"),
            ("a/b", "a%2Fb"),
            ("a?b", "a%3Fb"),
            ("a#b", "a%23b"),
            ("%2F", "%252F"),
        ] {
            let url = engine_delivery_url("http", 5252, hook_id).unwrap();
            assert_eq!(
                url.as_str(),
                format!("http://127.0.0.1:5252/api/v1/webhooks/{encoded}/deliver")
            );
            assert_eq!(url.host_str(), Some("127.0.0.1"));
            assert_eq!(url.port(), Some(5252));
            assert_eq!(url.query(), None);
            assert_eq!(url.fragment(), None);
        }
        for hook_id in [".", ".."] {
            assert!(engine_delivery_url("http", 5252, hook_id).is_err());
        }
    }

    #[test]
    fn an_ordinary_hook_id_keeps_the_webhook_delivery_route() {
        let url =
            engine_delivery_url("https", 5252, "01990ce2-fbed-7fd1-85de-dbc8000418c8").unwrap();
        assert_eq!(
            url.as_str(),
            "https://127.0.0.1:5252/api/v1/webhooks/01990ce2-fbed-7fd1-85de-dbc8000418c8/deliver"
        );
    }

    /// The invariant this socket exists for: nothing but a delivery answers.
    ///
    /// Driven through the real `Router`, so it covers the route table rather
    /// than a restatement of it. Every case uses a slug the empty test registry
    /// does not know, since a registered one would reach the engine hop and
    /// fail there instead.
    #[tokio::test]
    async fn no_gateway_or_workspace_path_answers_on_the_hook_socket() {
        let state = crate::server::GatewayState::for_tests();
        for (method, path) in [
            ("GET", "/~/api/v1/control/workspaces"),
            ("POST", "/~/api/v1/control/workspaces"),
            ("GET", "/~/api/v1/health"),
            ("POST", "/~/api/v1/auth/pair"),
            ("GET", "/~/"),
            ("GET", "/dev/api/v1/threads/list"),
            ("POST", "/dev/api/v1/chat/stream"),
            ("GET", "/dev/app/habit-tracker/"),
            ("GET", "/dev/"),
            ("GET", "/"),
            // The delivery route's own shape, on every other method.
            ("GET", "/dev/some-hook-id"),
            ("PUT", "/dev/some-hook-id"),
            ("DELETE", "/dev/some-hook-id"),
        ] {
            let response = router(state.clone())
                .oneshot(
                    Request::builder()
                        .method(method)
                        .uri(path)
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(
                response.status(),
                StatusCode::NOT_FOUND,
                "{method} {path} must not answer on the hook socket"
            );
        }
    }

    #[tokio::test]
    async fn a_delivery_to_an_unknown_workspace_is_a_flat_404() {
        // Not a 502, and not a message naming what is registered. A public
        // caller learns nothing about which workspaces exist.
        let state = crate::server::GatewayState::for_tests();
        let response = router(state)
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/no-such-workspace/abc")
                    .body(Body::from("{}"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[test]
    fn a_signed_senders_headers_reach_the_engine() {
        // The default, and the reason the filter is a denylist: any of these
        // may be the header a signature covers.
        for name in [
            "authorization",
            "content-type",
            "user-agent",
            "x-hub-signature-256",
            "x-slack-signature",
            "x-slack-request-timestamp",
            "stripe-signature",
            "x-github-event",
            "x-request-id",
        ] {
            assert!(forwarded_to_engine(name), "{name} must reach the engine");
        }
    }

    #[test]
    fn a_public_caller_cannot_hand_the_engine_a_trust_header() {
        // Every one of these MEANS something to the engine, and on this
        // listener it is written by whoever posted the delivery.
        for name in [
            "x-lucidos-local-token",
            "x-lucidos-agent-origin-token",
            "x-lucidos-device-id",
            "x-lucidos-target-workspace",
            "x-lucidos-anything-added-later",
            "cookie",
        ] {
            assert!(!forwarded_to_engine(name), "{name} must be dropped");
        }
    }

    #[test]
    fn browser_origin_metadata_is_dropped_so_a_delivery_is_not_refused_by_the_gate() {
        // The engine's same-origin gate reads these. A sender that sets Origin
        // would be 403'd before its delivery reached the webhook route.
        for name in ["origin", "referer", "sec-fetch-site", "sec-fetch-mode"] {
            assert!(!forwarded_to_engine(name), "{name} must be dropped");
        }
    }

    #[test]
    fn framing_and_hop_by_hop_headers_stay_behind() {
        for name in ["host", "content-length", "connection", "transfer-encoding"] {
            assert!(!forwarded_to_engine(name), "{name} must be dropped");
        }
    }

    #[tokio::test]
    async fn a_panic_on_the_hook_path_becomes_a_500_and_the_process_lives() {
        // The hook socket shares this process with every workspace's
        // supervisor, and the gateway unwinds rather than aborting. So a
        // malformed delivery reaching a panic must not take the machine's only
        // gateway down with it.
        async fn boom() -> StatusCode {
            panic!("a malformed delivery")
        }
        let exploding = Router::new().route("/boom", axum::routing::get(boom));
        let response = protect(exploding)
            .oneshot(Request::builder().uri("/boom").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[tokio::test]
    async fn a_body_over_the_cap_is_refused_before_the_engine_hop() {
        // The handler reads the body, exactly as `deliver` does. That is what
        // the limit applies to: an extractor, not the route.
        async fn read_body(body: axum::body::Bytes) -> String {
            format!("read {} bytes", body.len())
        }
        let oversized = Router::new().route("/x/y", axum::routing::post(read_body));
        let response = protect(oversized)
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/x/y")
                    .body(Body::from(vec![b'a'; MAX_BODY_BYTES + 1]))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    }

    #[tokio::test]
    async fn the_sigil_namespace_is_refused_by_name_too() {
        let state = crate::server::GatewayState::for_tests();
        let response = router(state)
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/{SIGIL}/abc"))
                    .body(Body::from("{}"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }
}
