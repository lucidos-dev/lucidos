use super::*;
use lucidos_gateway_stamp::workspace_client::PEER_SERVED_CLIENT;

const B_SHELL: &str =
    "<!doctype html><html><head></head><body data-peer-client>B's own client</body></html>";
const B_ASSET: &str = "/* B's own client code */";

fn peer_engine(
    shell_fails: bool,
) -> impl FnOnce(Seen, tokio::sync::oneshot::Receiver<()>) -> axum::Router {
    move |seen, _| {
        axum::Router::new().fallback(move |req: Request| {
            if req.uri().path() != "/api/v1/health" {
                seen.lock().unwrap().push(req.uri().to_string());
            }
            async move {
                match req.uri().path() {
                    "/api/v1/health" => axum::Json(serde_json::json!({"workspace_client": lucidos_gateway_stamp::workspace_client::ClientContract::engine()})).into_response(),
                    "/" if shell_fails => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
                    "/" => ([(header::CONTENT_TYPE, "text/html; charset=utf-8")], B_SHELL).into_response(),
                    "/assets/a-build.js" => StatusCode::NOT_FOUND.into_response(),
                    "/assets/moved.js" => Response::builder()
                        .status(StatusCode::TEMPORARY_REDIRECT)
                        .header(header::LOCATION, "/~/api/v1/control/workspaces")
                        .body(Body::empty())
                        .unwrap(),
                    _ => {
                        let mut response =
                            ([(header::CONTENT_TYPE, "application/javascript")], B_ASSET).into_response();
                        for (name, value) in ORIGIN_STATE_HEADERS {
                            response.headers_mut().append(*name, value.parse().unwrap());
                        }
                        response
                    }
                }
            }
        })
    }
}

async fn trusted(shell_fails: bool) -> Paired {
    let p = paired_with_engine(peer_engine(shell_fails)).await;
    p.a.remote_gateways()
        .set_trusted_pairing("cloud", true)
        .unwrap()
        .expect("the pairing exists");
    p
}

async fn shell(p: &Paired) -> String {
    let response = p
        .device_get("/dev@cloud/", &[("accept", "text/html")])
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    String::from_utf8(body_bytes(response).await).unwrap()
}

fn meta(name: &str) -> String {
    format!("name=\"{name}\" content=\"cloud\"")
}

#[tokio::test]
async fn trusted_pairing_serves_bs_own_client_stamped_for_as_origin() {
    let p = trusted(false).await;
    let body = shell(&p).await;
    assert!(body.contains("data-peer-client"), "{body}");
    assert!(!body.contains("data-trusted-client"), "{body}");
    assert!(body.contains("<base href=\"/dev@cloud/\">"), "{body}");
    assert!(body.contains("name=\"lucidos-workspace-id\" content=\"dev@cloud\""));
    assert!(
        body.contains(&meta(crate::server::PEER_CLIENT_META)),
        "{body}"
    );
    assert!(!body.contains(crate::server::PEER_CLIENT_UNAVAILABLE_META));

    let response = p.device_get("/dev@cloud/assets/client.js", &[]).await;
    assert_eq!(response.status(), StatusCode::OK);
    let headers = response.headers().clone();
    assert_eq!(body_bytes(response).await, B_ASSET.as_bytes());
    for (name, _) in ORIGIN_STATE_HEADERS {
        assert!(!headers.contains_key(*name), "{name} reached A's origin");
    }
    assert!(!headers.contains_key(header::CONTENT_SECURITY_POLICY));
}

#[tokio::test]
async fn trusted_pairing_never_runs_a_b_worker() {
    let p = trusted(false).await;
    let response = p.device_get("/dev@cloud/sw.js", &[]).await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let response = p
        .device_get(
            "/dev@cloud/assets/client.js",
            &[("service-worker", "script")],
        )
        .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert!(
        p.seen.lock().unwrap().is_empty(),
        "B was asked for a worker"
    );
}

#[tokio::test]
async fn trusted_pairing_refuses_a_redirecting_client_asset() {
    let p = trusted(false).await;
    let response = p.device_get("/dev@cloud/assets/moved.js", &[]).await;
    assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    assert!(!response.headers().contains_key(header::LOCATION));
}

#[tokio::test]
async fn trusted_pairing_falls_back_to_as_shell_when_bs_shell_fails() {
    let p = trusted(true).await;
    let body = shell(&p).await;
    assert!(body.contains("data-trusted-client"), "{body}");
    assert!(
        body.contains(&meta(crate::server::PEER_CLIENT_UNAVAILABLE_META)),
        "{body}"
    );
}

#[tokio::test]
async fn trusted_pairing_can_fall_back_after_a_successful_preflight() {
    let p = trusted(true).await;
    let preflight = p.device_get("/dev@cloud/api/v1/health", &[]).await;
    assert_eq!(preflight.status(), StatusCode::OK);
    let response = p
        .device_get("/dev@cloud/?preflight=1", &[("sec-fetch-dest", "document")])
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = String::from_utf8(body_bytes(response).await).unwrap();
    assert!(body.contains(&meta(crate::server::PEER_CLIENT_UNAVAILABLE_META)));
}

#[tokio::test]
async fn a_fallback_shell_loads_as_own_assets_that_b_lacks() {
    let p = trusted(true).await;
    let dir = p._dirs[2].path().to_path_buf();
    std::fs::create_dir_all(dir.join("assets")).unwrap();
    std::fs::write(dir.join("assets/a-build.js"), "/* A's build */").unwrap();
    install_client_inventory(&dir, &["index.html", "assets/a-build.js"]);

    let response = p.device_get("/dev@cloud/assets/a-build.js", &[]).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(body_bytes(response).await, b"/* A's build */");
}

fn without_peer_served_client(router: axum::Router) -> axum::Router {
    router.layer(axum::middleware::from_fn(
        |req: Request, next: axum::middleware::Next| async move {
            let health = req.uri().path() == crate::peers::GATEWAY_HEALTH_PATH;
            let response = next.run(req).await;
            if !health {
                return response;
            }
            let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
                .await
                .unwrap();
            let mut body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
            body["workspace_client"]["capabilities"]
                .as_array_mut()
                .unwrap()
                .retain(|c| c != PEER_SERVED_CLIENT);
            axum::Json(body).into_response()
        },
    ))
}

#[tokio::test]
async fn trusted_pairing_falls_back_to_as_shell_when_b_lacks_the_capability() {
    let p = trusted(false).await;
    let older = serve_https(crate::remote_gateways::tests::proxy_guard(
        without_peer_served_client(crate::server::gateway_router(p.b.clone())),
    ));
    let mut remote = p.a.remote_gateways().remove("cloud").unwrap().unwrap();
    remote.address = format!("https://localhost:{}", older.port());
    p.a.remote_gateways().add(remote).unwrap();

    let body = shell(&p).await;
    assert!(body.contains("data-trusted-client"), "{body}");
    assert!(
        body.contains(&meta(crate::server::PEER_CLIENT_UNAVAILABLE_META)),
        "{body}"
    );
    assert!(
        !body.contains(&meta(crate::server::PEER_CLIENT_META)),
        "{body}"
    );
    assert!(
        p.seen.lock().unwrap().is_empty(),
        "A asked an older B for its shell"
    );
}

#[test]
fn the_client_reads_the_metas_the_gateway_stamps() {
    let sdk = include_str!("../../../packages/lucidos-sdk/src/_peerScope.ts");
    assert!(sdk.contains(&format!("'{}'", crate::server::PEER_CLIENT_META)));
    let banner = include_str!("../../lucidos-app/src/components/layout/PeerClientBanner.tsx");
    assert!(banner.contains(&format!(
        "'{}'",
        crate::server::PEER_CLIENT_UNAVAILABLE_META
    )));
    let index = include_str!("../../lucidos-app/index.html");
    let read = format!("meta[name=\"{}\"]", crate::server::PEER_CLIENT_META);
    assert_eq!(
        index.matches(&read).count(),
        3,
        "every boot script scopes its keys"
    );
}

#[tokio::test]
async fn preview_html_modules_and_hmr_cross_both_gateways() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let p = trusted(false).await;
    let old = p.a.remote_gateways().list().remove(0);
    let mut known = old.clone();
    known.gateway_id = Some(p.b.gateway_id().into());
    p.a.remote_gateways().replace(&old, known).unwrap();
    let dir = test_engine::scratch();
    let socket = dir.path().join(".lucidos/frontend-preview/vite.sock");
    std::fs::create_dir_all(socket.parent().unwrap()).unwrap();
    let mut ws = crate::registry::Workspace::gateway_provisioned("dev".into(), "Dev".into(), 5173);
    ws.dir = dir.path().to_str().unwrap().into();
    p.b.register_for_test(ws);
    let listener = tokio::net::UnixListener::bind(socket).unwrap();
    let key = lucidos_gateway_stamp::frontend_preview::key(p.b.gateway_id());
    let base = format!("/~preview/{key}/dev/");
    let expected = base.clone();
    let upstream = tokio::spawn(async move {
        for suffix in ["", "@vite/client", "?token=hmr"] {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut head = Vec::new();
            while !head.ends_with(b"\r\n\r\n") {
                head.push(stream.read_u8().await.unwrap());
            }
            let head = String::from_utf8(head).unwrap().to_ascii_lowercase();
            assert!(
                head.starts_with(&format!("get {expected}{suffix} http/1.1")),
                "{head}"
            );
            assert!(head.contains("x-forwarded-prefix: /dev@cloud/"), "{head}");
            assert!(
                !head.contains("cred-abc") && !head.contains("x-lucidos-"),
                "{head}"
            );
            if suffix.starts_with('?') {
                stream.write_all(b"HTTP/1.1 101 Switching Protocols\r\nConnection: Upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Accept: s3pPLMBiTxaQ9kYGzzhZRbK+xOo=\r\n\r\n").await.unwrap();
                let mut echo = [0; 4];
                stream.read_exact(&mut echo).await.unwrap();
                stream.write_all(&echo).await.unwrap();
                let mut end = [0];
                assert_eq!(stream.read(&mut end).await.unwrap(), 0);
            } else {
                stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: 7\r\nSet-Cookie: forged=1\r\n\r\npreview").await.unwrap();
            }
        }
    });
    let check = async {
        for suffix in ["", "@vite/client"] {
            let response = p
                .device_get(
                    &format!("{base}{suffix}"),
                    &[("x-forwarded-prefix", "/forged/")],
                )
                .await;
            assert_eq!(response.status(), StatusCode::OK);
            assert!(!response
                .headers()
                .get_all(header::SET_COOKIE)
                .iter()
                .any(|v| v.to_str().unwrap().contains("forged")));
            assert_eq!(body_bytes(response).await, b"preview");
        }
        let a = serve_https(crate::server::gateway_router(p.a.clone()));
        let origin = format!("https://localhost:{}", a.port());
        let client = lucidos_local_token::client::client_builder()
            .http1_only()
            .build()
            .unwrap();
        let response = client
            .get(format!("{origin}{base}?token=hmr"))
            .header(header::COOKIE, p.cookie())
            .header(header::ORIGIN, &origin)
            .header(header::CONNECTION, "upgrade")
            .header(header::UPGRADE, "websocket")
            .header("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ==")
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::SWITCHING_PROTOCOLS);
        let mut stream = response.upgrade().await.unwrap();
        stream.write_all(b"echo").await.unwrap();
        let mut echoed = [0; 4];
        stream.read_exact(&mut echoed).await.unwrap();
        assert_eq!(&echoed, b"echo");
        let leave =
            p.a.peer_connection()
                .request(
                    &p.a.remote_gateways().list()[0],
                    reqwest::Method::DELETE,
                    &format!(
                        "{GATEWAY_API_PREFIX}{}",
                        crate::gateway_protocol::PAIRING_PATH
                    ),
                )
                .unwrap()
                .send()
                .await
                .unwrap();
        assert_eq!(leave.status(), StatusCode::NO_CONTENT);
        let mut end = [0];
        assert!(matches!(stream.read(&mut end).await, Ok(0) | Err(_)));
        upstream.await.unwrap();
    };
    tokio::time::timeout(std::time::Duration::from_secs(10), check)
        .await
        .unwrap();
}

#[tokio::test]
async fn stopped_remote_preview_has_an_explanatory_page() {
    let p = trusted(false).await;
    let old = p.a.remote_gateways().list().remove(0);
    let mut known = old.clone();
    known.gateway_id = Some(p.b.gateway_id().into());
    p.a.remote_gateways().replace(&old, known).unwrap();
    let key = lucidos_gateway_stamp::frontend_preview::key(p.b.gateway_id());
    let response = p.device_get(&format!("/~preview/{key}/dev/"), &[]).await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let body = String::from_utf8(body_bytes(response).await).unwrap();
    assert!(
        body.contains("cloud") && body.contains("re-pair") && body.contains("running"),
        "{body}"
    );
}

#[tokio::test]
async fn old_peer_cannot_substitute_its_spa_for_a_preview() {
    let p = trusted(false).await;
    let old = p.a.remote_gateways().list().remove(0);
    let mut known = old.clone();
    known.gateway_id = Some(p.b.gateway_id().into());
    let calls = Arc::new(AtomicUsize::new(0));
    let counted = calls.clone();
    let peer = serve_https(
        axum::Router::new()
            .route(
                crate::peers::GATEWAY_HEALTH_PATH,
                axum::routing::get(|| async {
                    axum::Json(serde_json::json!({"gateway_protocol": 1}))
                }),
            )
            .fallback(move || {
                counted.fetch_add(1, Ordering::SeqCst);
                async { B_SHELL }
            }),
    );
    known.address = format!("https://localhost:{}", peer.port());
    p.a.remote_gateways().replace(&old, known).unwrap();
    let key = lucidos_gateway_stamp::frontend_preview::key(p.b.gateway_id());
    let response = p.device_get(&format!("/~preview/{key}/dev/"), &[]).await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let body = String::from_utf8(body_bytes(response).await).unwrap();
    assert!(body.contains("update") && !body.contains("data-peer-client"));
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn received_preview_is_bound_to_the_local_key_workspace_and_device() {
    let p = trusted(false).await;
    let key = lucidos_gateway_stamp::frontend_preview::key(p.b.gateway_id());
    for (path, expected) in [
        (format!("/~preview/{key}/dev/"), StatusCode::UNAUTHORIZED),
        (format!("/~preview/{key}/other/"), StatusCode::NOT_FOUND),
        (
            "/~preview/0123456789abcdef/dev/".into(),
            StatusCode::NOT_FOUND,
        ),
    ] {
        let headers = signed(
            &claim("/dev@cloud/", chrono::Utc::now().timestamp(), None),
            &p.credential,
        );
        let response =
            p.a.peer_connection()
                .request(
                    &p.a.remote_gateways().list()[0],
                    reqwest::Method::GET,
                    &format!("{GATEWAY_API_PREFIX}{RELAY_PATH}/dev{path}"),
                )
                .unwrap()
                .headers(headers)
                .send()
                .await
                .unwrap();
        assert_eq!(response.status(), expected, "{path}");
    }
    assert!(p.seen.lock().unwrap().is_empty());
}
