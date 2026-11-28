//! The credential-free hop from the authenticated gateway to a preview.

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use axum::response::{IntoResponse, Response};
use std::path::Path;

use crate::{auth, proxy, server::GatewayState};

pub(crate) fn route(path: &str) -> Option<(&str, &str)> {
    let (key, rest) = path.strip_prefix("/~preview/")?.split_once('/')?;
    let (slug, _) = rest.split_once('/')?;
    (key.len() == 16
        && key
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        && lucidos_gateway_constants::is_valid_workspace_id(slug))
    .then_some((key, slug))
}

fn prepare(
    req: Request<Body>,
    prefix: &str,
    upgrade: bool,
) -> Result<Request<Body>, axum::http::Error> {
    let mut out = Request::builder().method(req.method()).uri(req.uri());
    for (name, value) in req.headers() {
        if name != header::COOKIE
            && name != header::AUTHORIZATION
            && !proxy::gateway_owns_header(name, upgrade)
        {
            out = out.header(name, value);
        }
    }
    out.header(header::HOST, "localhost")
        .header("x-forwarded-prefix", prefix)
        .body(req.into_body())
}

pub(crate) async fn serve(
    state: &GatewayState,
    socket: &Path,
    prefix: &str,
    mut req: Request<Body>,
) -> Response {
    let device = req.extensions().get::<auth::AuthenticatedDevice>().cloned();
    let gateway = req
        .extensions()
        .get::<crate::gateway_protocol::AuthenticatedGateway>()
        .cloned();
    // INVARIANT: Defense in depth for direct dispatch as well as the router's auth layer.
    if device.is_none()
        && req
            .extensions()
            .get::<auth::AuthenticatedLocalProcess>()
            .is_none()
    {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let upgrade = proxy::is_websocket_upgrade(&req);
    if upgrade && proxy::foreign_origin(req.headers()).is_some() {
        return StatusCode::FORBIDDEN.into_response();
    }
    let client_upgrade = if upgrade {
        let Some(on_upgrade) = req.extensions_mut().remove::<hyper::upgrade::OnUpgrade>() else {
            return StatusCode::BAD_REQUEST.into_response();
        };
        Some(on_upgrade)
    } else {
        None
    };
    let Ok(upstream) = prepare(req, prefix, upgrade) else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    let stream = match tokio::net::UnixStream::connect(socket).await {
        Ok(stream) => stream,
        Err(error)
            if error.kind() == std::io::ErrorKind::NotFound
                || error.kind() == std::io::ErrorKind::ConnectionRefused =>
        {
            return (StatusCode::NOT_FOUND, "No frontend preview is running.").into_response();
        }
        Err(error) => {
            crate::log_legacy!("[FrontendPreview] socket connection failed: {error}");
            return StatusCode::BAD_GATEWAY.into_response();
        }
    };
    // INVARIANT: Check the kernel peer before sending any browser data. Do not
    // use engine_hop: it injects credentials which must never reach Vite.
    if !lucidos_engine_socket::peer_is_owner(&stream) {
        return StatusCode::BAD_GATEWAY.into_response();
    }
    let Ok((mut sender, connection)) =
        hyper::client::conn::http1::handshake(hyper_util::rt::TokioIo::new(stream)).await
    else {
        return StatusCode::BAD_GATEWAY.into_response();
    };
    tokio::spawn(async move {
        if let Err(error) = connection.with_upgrades().await {
            crate::log_legacy!("[FrontendPreview] socket transport failed: {error}");
        }
    });
    let Ok(response) = sender.send_request(upstream).await else {
        return StatusCode::BAD_GATEWAY.into_response();
    };
    let switches = upgrade && response.status() == StatusCode::SWITCHING_PROTOCOLS;
    let mut out = Response::builder().status(response.status());
    for (name, value) in response.headers() {
        // A dev server cannot change the gateway's session or its worker scope.
        if name != header::SET_COOKIE
            && name.as_str() != "service-worker-allowed"
            && (switches || !proxy::is_hop_by_hop(name))
        {
            out = out.header(name, value);
        }
    }
    if switches {
        let upstream_upgrade = hyper::upgrade::on(response);
        let splice = tokio::spawn(async move {
            if let Ok((client, upstream)) = tokio::try_join!(
                client_upgrade.expect("an upgrade request has an OnUpgrade"),
                upstream_upgrade
            ) {
                proxy::splice_websocket(
                    &mut hyper_util::rt::TokioIo::new(client),
                    &mut hyper_util::rt::TokioIo::new(upstream),
                )
                .await;
            }
        });
        if let Some(device) = device {
            proxy::track_websocket(splice, state.device_splices(), &device.id, None, |id| {
                match &gateway {
                    Some(gateway) => state.paired_gateways().contains(&gateway.pairing_id),
                    None => state.device_is_paired(id),
                }
            });
        }
        out.body(Body::empty())
            .unwrap_or_else(|_| StatusCode::BAD_GATEWAY.into_response())
    } else {
        out.body(Body::new(response.into_body()))
            .unwrap_or_else(|_| StatusCode::BAD_GATEWAY.into_response())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

    async fn read_head(stream: &mut (impl tokio::io::AsyncRead + Unpin)) -> String {
        let mut bytes = Vec::new();
        while !bytes.ends_with(b"\r\n\r\n") {
            bytes.push(stream.read_u8().await.unwrap());
            assert!(bytes.len() < 16_384);
        }
        String::from_utf8(bytes).unwrap()
    }

    #[tokio::test]
    async fn discovery_advertises_the_same_preview_key_as_the_route() {
        use tower::ServiceExt as _;
        let state = GatewayState::for_tests();
        let key = lucidos_gateway_stamp::frontend_preview::key(state.gateway_id());
        let response = crate::server::gateway_router(state)
            .oneshot(
                Request::builder()
                    .uri(crate::peers::GATEWAY_HEALTH_PATH)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), 16_384)
            .await
            .unwrap();
        let health: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(health["frontend_preview_key"], key);
        assert!(health.get("gateway_id").is_none());
    }

    #[tokio::test]
    async fn hmr_upgrade_crosses_socket_and_strips_credentials() {
        let state = GatewayState::for_tests();
        let dir = crate::engine_hop::test_engine::scratch();
        let socket = dir.path().join("vite.sock");
        let upstream = tokio::net::UnixListener::bind(&socket).unwrap();
        let recorded = tokio::spawn(async move {
            let (mut stream, _) = upstream.accept().await.unwrap();
            let head = read_head(&mut stream).await;
            stream.write_all(b"HTTP/1.1 101 Switching Protocols\r\nConnection: Upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Accept: test\r\n\r\n").await.unwrap();
            let mut payload = [0; 4];
            stream.read_exact(&mut payload).await.unwrap();
            stream.write_all(&payload).await.unwrap();
            head
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let app = axum::Router::new().fallback(move |mut request: Request<Body>| {
            let socket = socket.clone();
            let state = state.clone();
            async move {
                request
                    .extensions_mut()
                    .insert(auth::AuthenticatedLocalProcess);
                serve(&state, &socket, "/demo/", request).await
            }
        });
        let gateway = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let check = async {
            let mut client = tokio::net::TcpStream::connect(("127.0.0.1", port))
                .await
                .unwrap();
            client.write_all(format!("GET /~preview/0123456789abcdef/demo/ HTTP/1.1\r\nHost: localhost:{port}\r\nOrigin: http://localhost:{port}\r\nConnection: Upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Key: test\r\nCookie: session=secret\r\nAuthorization: Bearer secret\r\n\r\n").as_bytes()).await.unwrap();
            let head = read_head(&mut client).await;
            assert!(head.starts_with("HTTP/1.1 101"), "{head}");
            client.write_all(b"echo").await.unwrap();
            let mut echoed = [0; 4];
            client.read_exact(&mut echoed).await.unwrap();
            assert_eq!(&echoed, b"echo");
            let got = recorded.await.unwrap().to_ascii_lowercase();
            assert!(!got.contains("secret"));
            assert!(!got.contains("x-lucidos-"));
            assert!(got.contains("x-forwarded-prefix: /demo/"));
        };
        let result = tokio::time::timeout(std::time::Duration::from_secs(5), check).await;
        gateway.abort();
        result.unwrap();
    }

    #[tokio::test]
    async fn html_and_modules_require_authentication_before_socket_connect() {
        let state = GatewayState::for_tests();
        let socket = Path::new("does-not-exist.sock");
        for suffix in ["", "src/main.ts", "@vite/client"] {
            let request = Request::builder()
                .uri(format!("/~preview/0123456789abcdef/demo/{suffix}"))
                .body(Body::empty())
                .unwrap();
            assert!(!crate::auth_api::is_public_path(request.uri().path()));
            assert_eq!(
                serve(&state, socket, "/demo/", request).await.status(),
                StatusCode::UNAUTHORIZED
            );
        }
    }

    #[tokio::test]
    async fn gateway_auth_layer_covers_preview_html_assets_and_upgrade() {
        use tower::ServiceExt as _;
        let state = GatewayState::for_tests();
        let key = lucidos_gateway_stamp::frontend_preview::key(state.gateway_id());
        let app = crate::server::gateway_router(state);
        for suffix in ["", "@vite/client", "api/v1/sdk.js", "src/main.ts"] {
            for upgrade in [false, true] {
                let mut request = Request::builder().uri(format!("/~preview/{key}/demo/{suffix}"));
                if upgrade {
                    request = request
                        .header("connection", "upgrade")
                        .header("upgrade", "websocket");
                }
                let response = app
                    .clone()
                    .oneshot(request.body(Body::empty()).unwrap())
                    .await
                    .unwrap();
                assert_eq!(
                    response.status(),
                    StatusCode::UNAUTHORIZED,
                    "{suffix} upgrade={upgrade}"
                );
            }
        }
    }

    #[tokio::test]
    async fn foreign_hmr_origin_is_refused_before_socket_connect() {
        let state = GatewayState::for_tests();
        let mut request = Request::builder()
            .uri("/~preview/0123456789abcdef/demo/")
            .header("host", "localhost:5252")
            .header("origin", "https://untrusted.example")
            .header("connection", "upgrade")
            .header("upgrade", "websocket")
            .body(Body::empty())
            .unwrap();
        request
            .extensions_mut()
            .insert(auth::AuthenticatedLocalProcess);
        assert_eq!(
            serve(&state, Path::new("does-not-exist.sock"), "/demo/", request)
                .await
                .status(),
            StatusCode::FORBIDDEN
        );
    }

    #[tokio::test]
    async fn paired_requests_reach_a_real_socket_without_credentials() {
        use tower::ServiceExt as _;
        let state = GatewayState::for_tests();
        let dir = crate::engine_hop::test_engine::scratch();
        let socket = dir.path().join(".lucidos/frontend-preview/vite.sock");
        std::fs::create_dir_all(socket.parent().unwrap()).unwrap();
        let mut workspace =
            crate::registry::Workspace::gateway_provisioned("demo".into(), "Demo".into(), 5173);
        workspace.dir = dir.path().to_str().unwrap().into();
        state.register_for_test(workspace);
        state
            .seed_device_for_test(auth::PairedDevice {
                id: "test-device".into(),
                label: "Test browser".into(),
                credential_digest: auth::digest("secret"),
                paired_at: "2020-01-01T00:00:00Z".into(),
                last_seen_at: None,
            })
            .unwrap();
        let key = lucidos_gateway_stamp::frontend_preview::key(state.gateway_id());
        let listener = tokio::net::UnixListener::bind(&socket).unwrap();
        let upstream = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let head = read_head(&mut stream).await;
            stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nSet-Cookie: session=forged\r\nService-Worker-Allowed: /\r\n\r\nok").await.unwrap();
            head
        });
        let request = Request::builder()
            .uri(format!("/~preview/{key}/demo/@vite/client"))
            .header(
                "cookie",
                format!(
                    "{}=secret",
                    auth::CookieForm::Plain.name(state.device_cookie_name())
                ),
            )
            .header("authorization", "Bearer secret")
            .body(Body::empty())
            .unwrap();
        let response = crate::server::gateway_router(state)
            .oneshot(request)
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert!(!response
            .headers()
            .get_all(header::SET_COOKIE)
            .iter()
            .any(|value| value.to_str().unwrap().contains("forged")));
        assert!(!response.headers().contains_key("service-worker-allowed"));
        assert_eq!(
            axum::body::to_bytes(response.into_body(), 1024)
                .await
                .unwrap(),
            "ok"
        );
        let received = upstream.await.unwrap().to_ascii_lowercase();
        assert!(received.starts_with(&format!("get /~preview/{key}/demo/@vite/client http/1.1")));
        assert!(received.contains("x-forwarded-prefix: /demo/"));
        assert!(!received.contains("secret"));
        assert!(!received.contains("x-lucidos-"));
    }

    #[test]
    fn only_canonical_preview_routes_match() {
        assert_eq!(
            route("/~preview/0123456789abcdef/demo/@vite/client"),
            Some(("0123456789abcdef", "demo"))
        );
        for path in [
            "/~preview/raw-id/demo/",
            "/~preview/0123456789ABCDEF/demo/",
            "/~preview/0123456789abcdef/../",
            "/~preview/0123456789abcdef/demo@peer/",
            "/~preview/0123456789abcdef/demo",
        ] {
            assert_eq!(route(path), None, "{path}");
        }
    }

    #[test]
    fn strips_credentials_and_replaces_prefix_for_http_and_hmr() {
        for upgrade in [false, true] {
            let request = Request::builder()
                .uri("/~preview/0123456789abcdef/demo/src/main.ts?x=1")
                .header("cookie", "session=secret")
                .header("authorization", "Bearer secret")
                .header("x-forwarded-prefix", "/forged/")
                .header("x-lucidos-local-token", "secret")
                .header("x-lucidos-engine-socket-token", "secret")
                .header("connection", "upgrade")
                .header("upgrade", "websocket")
                .body(Body::empty())
                .unwrap();
            let request = prepare(request, "/demo/", upgrade).unwrap();
            for name in [
                "cookie",
                "authorization",
                "x-lucidos-local-token",
                "x-lucidos-engine-socket-token",
            ] {
                assert!(!request.headers().contains_key(name), "{name}");
            }
            assert_eq!(request.headers()["x-forwarded-prefix"], "/demo/");
            assert_eq!(request.headers().contains_key("upgrade"), upgrade);
            assert_eq!(request.uri().query(), Some("x=1"));
        }
    }
}
