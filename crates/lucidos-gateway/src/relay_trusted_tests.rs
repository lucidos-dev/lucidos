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
