use crate::engine_hop::{self, Credential};
use crate::registry::SIGIL;
use crate::server::GatewayState;
use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Router;
use std::net::SocketAddr;
use std::time::Duration;

const MAX_BODY_BYTES: usize = 1024 * 1024;

const DELIVERY_TIMEOUT: Duration = Duration::from_secs(30);

const MAX_CONCURRENT_DELIVERIES: usize = 16;

const HOOK_PORT_OFFSET: u16 = 10;

pub fn hook_port(configured: Option<&str>, gateway_port: u16) -> Option<u16> {
    crate::net_config::side_port(
        "LUCIDOS_HOOK_PORT",
        configured,
        gateway_port,
        HOOK_PORT_OFFSET,
    )
}

pub fn resolved_hook_port(gateway_port: u16) -> Option<u16> {
    hook_port(
        std::env::var("LUCIDOS_HOOK_PORT").ok().as_deref(),
        gateway_port,
    )
}

pub fn router(state: GatewayState) -> Router {
    let routes = Router::new()
        .route("/:slug/:hook_id", axum::routing::post(deliver))
        .fallback(|| async { StatusCode::NOT_FOUND })
        .method_not_allowed_fallback(|| async { StatusCode::NOT_FOUND })
        .with_state(state);
    protect(routes)
}

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
        .layer(tower_http::catch_panic::CatchPanicLayer::new())
}

fn engine_delivery_path(hook_id: &str) -> Result<String, &'static str> {
    if matches!(hook_id, "." | "..") {
        return Err("hook ID is a URL dot segment");
    }
    let mut url =
        reqwest::Url::parse("http://engine/api/v1/webhooks/").map_err(|_| "invalid engine URL")?;
    url.path_segments_mut()
        .map_err(|_| "engine URL cannot carry path segments")?
        .pop_if_empty()
        .push(hook_id)
        .push("deliver");
    Ok(url.path().to_string())
}

async fn deliver(
    State(state): State<GatewayState>,
    Path((slug, hook_id)): Path<(String, String)>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    if slug.len() == 1 && slug.starts_with(SIGIL) {
        return StatusCode::NOT_FOUND.into_response();
    }
    let Some(engine) = state.engine_address(&slug) else {
        return StatusCode::NOT_FOUND.into_response();
    };

    let Ok(path) = engine_delivery_path(&hook_id) else {
        crate::log_legacy!("[Hook] '{slug}' delivery could not construct the engine URL");
        return StatusCode::BAD_GATEWAY.into_response();
    };
    let mut request = axum::http::Request::post(path);
    for (name, value) in &headers {
        if !forwarded_to_engine(name.as_str()) {
            continue;
        }
        request = request.header(name.as_str(), value);
    }
    let Ok(request) = request.body(Body::from(body)) else {
        crate::log_legacy!("[Hook] '{slug}' delivery could not build the engine request");
        return StatusCode::BAD_GATEWAY.into_response();
    };

    let credential = Credential::HookSocket(state.hook_socket_token());
    match engine_hop::send(&engine, request, credential).await {
        Ok(upstream) => {
            let status = upstream.status();
            let bytes = axum::body::to_bytes(Body::new(upstream.into_body()), usize::MAX)
                .await
                .unwrap_or_default();
            (status, Body::from(bytes)).into_response()
        }
        Err(e) => {
            crate::log_legacy!("[Hook] '{slug}' delivery could not reach the engine: {e}");
            StatusCode::BAD_GATEWAY.into_response()
        }
    }
}

fn forwarded_to_engine(name_lower: &str) -> bool {
    let framing = matches!(name_lower, "host" | "content-length");
    let ours = name_lower.starts_with("x-lucidos-") || name_lower == "cookie";
    let browser =
        matches!(name_lower, "origin" | "referer") || name_lower.starts_with("sec-fetch-");
    let forwarding = matches!(name_lower, "x-forwarded-prefix" | "x-forwarded-host");
    !(framing || ours || browser || forwarding || is_hop_by_hop(name_lower))
}

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

pub fn bind_addr(configured: Option<&str>, port: u16) -> SocketAddr {
    crate::net_config::side_bind_addr("LUCIDOS_HOOK_BIND_ADDR", configured, port)
}

pub fn resolved_bind_addr(port: u16) -> SocketAddr {
    bind_addr(
        std::env::var("LUCIDOS_HOOK_BIND_ADDR").ok().as_deref(),
        port,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt as _;

    #[test]
    fn the_hook_port_derives_from_the_gateway_port() {
        use lucidos_installs::{DEFAULT_DEV_GATEWAY_PORT as DEV, DEFAULT_GATEWAY_PORT as PACKAGED};
        assert_eq!(hook_port(None, DEV), Some(5261));
        assert_eq!(hook_port(None, PACKAGED), Some(5262));
        assert_eq!(hook_port(Some(""), DEV), Some(5261));
        assert_eq!(hook_port(Some("  "), DEV), Some(5261));
    }

    #[test]
    fn an_explicit_port_wins_and_zero_switches_the_socket_off() {
        assert_eq!(hook_port(Some("9000"), 5251), Some(9000));
        assert_eq!(hook_port(Some(" 9000 "), 5251), Some(9000));
        assert_eq!(hook_port(Some("0"), 5251), None);
    }

    #[test]
    fn a_value_that_is_not_a_port_opens_nothing() {
        assert_eq!(hook_port(Some("nonsense"), 5251), None);
        assert_eq!(hook_port(Some("70000"), 5251), None);
    }

    #[test]
    fn the_socket_binds_loopback_unless_an_address_is_set() {
        assert_eq!(bind_addr(None, 5261).to_string(), "127.0.0.1:5261");
        assert!(bind_addr(Some("nonsense"), 9000).ip().is_loopback());
        assert_eq!(bind_addr(Some("0.0.0.0"), 5262).to_string(), "0.0.0.0:5262");
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
            let path = engine_delivery_path(hook_id).unwrap();
            assert_eq!(path, format!("/api/v1/webhooks/{encoded}/deliver"));
            assert!(!path.contains(['?', '#']), "{path}");
        }
        for hook_id in [".", ".."] {
            assert!(engine_delivery_path(hook_id).is_err());
        }
    }

    #[test]
    fn an_ordinary_hook_id_keeps_the_webhook_delivery_route() {
        let path = engine_delivery_path("01990ce2-fbed-7fd1-85de-dbc8000418c8").unwrap();
        assert_eq!(
            path,
            "/api/v1/webhooks/01990ce2-fbed-7fd1-85de-dbc8000418c8/deliver"
        );
    }

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
        for name in [
            "x-lucidos-local-token",
            "x-lucidos-hook-socket-token",
            "x-lucidos-agent-origin-token",
            "x-lucidos-device-id",
            "x-lucidos-target-workspace",
            "x-lucidos-anything-added-later",
            "cookie",
        ] {
            assert!(!forwarded_to_engine(name), "{name} must be dropped");
        }
        for name in [
            crate::auth::HEADER_LOCAL_TOKEN,
            crate::auth::HEADER_HOOK_SOCKET_TOKEN,
        ] {
            assert!(!forwarded_to_engine(name), "{name} must be dropped");
        }
    }

    #[test]
    fn a_public_caller_cannot_claim_the_delivery_came_through_the_proxy() {
        for name in ["x-forwarded-prefix", "x-forwarded-host"] {
            assert!(!forwarded_to_engine(name), "{name} must be dropped");
        }
    }

    async fn capturing_engine() -> (
        tempfile::TempDir,
        crate::engine_hop::EngineAddress,
        std::sync::Arc<tokio::sync::Mutex<String>>,
    ) {
        use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
        let dir = crate::engine_hop::test_engine::scratch();
        let (listener, engine) = crate::engine_hop::test_engine::bind(&dir);
        let captured = std::sync::Arc::new(tokio::sync::Mutex::new(String::new()));
        let sink = captured.clone();
        tokio::spawn(async move {
            if let Ok((mut sock, _)) = listener.accept().await {
                let mut buf = vec![0u8; 8192];
                let n = sock.read(&mut buf).await.unwrap_or(0);
                *sink.lock().await = String::from_utf8_lossy(&buf[..n]).into_owned();
                let _ = sock
                    .write_all(
                        b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok",
                    )
                    .await;
                let _ = sock.flush().await;
            }
        });
        (dir, engine, captured)
    }

    #[tokio::test]
    async fn a_delivery_to_a_strangers_socket_sends_nothing() {
        let state = crate::server::GatewayState::for_tests();
        let dir = crate::engine_hop::test_engine::scratch();
        let (engine, recorded) = crate::engine_hop::test_engine::record_one(&dir);
        state.set_route_for_test("dev", engine.held_by_a_stranger());

        let response = router(state)
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/dev/abc")
                    .body(Body::from("{\"hello\":\"world\"}"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
        assert!(
            recorded.await.expect("recorder").is_empty(),
            "a stranger's socket must read zero bytes"
        );
    }

    #[tokio::test]
    async fn a_delivery_carries_the_hook_socket_scope_and_nothing_wider() {
        let state = crate::server::GatewayState::for_tests();
        let (_dir, engine, captured) = capturing_engine().await;
        state.set_route_for_test("dev", engine);

        let response = router(state)
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/dev/abc")
                    .header("x-hub-signature-256", "sha256=deadbeef")
                    .header(crate::auth::HEADER_LOCAL_TOKEN, "sender-forged-full-token")
                    .body(Body::from("{\"hello\":\"world\"}"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let got = captured.lock().await.to_lowercase();
        assert!(
            got.contains(&format!(
                "{}: test-hook-socket-token",
                crate::auth::HEADER_HOOK_SOCKET_TOKEN
            )),
            "the delivery must present the hook socket token; engine saw:\n{got}"
        );
        assert!(
            !got.contains("sender-forged-full-token"),
            "a sender must not hand the engine full authority; engine saw:\n{got}"
        );
        assert!(
            got.contains("x-hub-signature-256: sha256=deadbeef"),
            "a signature header must survive the hop; engine saw:\n{got}"
        );
        assert!(
            got.contains("{\"hello\":\"world\"}"),
            "the body must reach the engine byte for byte; engine saw:\n{got}"
        );
    }

    #[test]
    fn browser_origin_metadata_is_dropped_so_a_delivery_is_not_refused_by_the_gate() {
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
