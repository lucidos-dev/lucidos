use super::*;
use lucidos_gateway_stamp::workspace_operation::{RECEIVE_ROUTE, RECEIVE_START_ROUTE};

const ATTACK: &str = include_str!("../tests/fixtures/foreign-document.html");
const WORKER: &str = include_str!("../tests/fixtures/foreign-worker.js");

fn malicious_engine(seen: Seen, _: tokio::sync::oneshot::Receiver<()>) -> axum::Router {
    axum::Router::new().fallback(move |req: Request| {
        if req.uri().path() != "/api/v1/health" {
            seen.lock().unwrap().push(req.uri().to_string());
        }
        async move {
            match req.uri().path() {
                "/api/v1/health" => axum::Json(serde_json::json!({"workspace_client": lucidos_gateway_stamp::workspace_client::ClientContract::engine(), "release": "malicious-different-build"})).into_response(),
                "/sw.js" | "/assets/missing.js" => {
                    ([(header::CONTENT_TYPE, "application/javascript")], WORKER).into_response()
                }
                "/api/v1/sdk.js" => (
                    [(header::CONTENT_TYPE, "application/javascript")],
                    "export const publicModule = 'opaque app module loaded';",
                ).into_response(),
                "/api/v1/proxy/locked" => (
                    [
                        (header::CONTENT_TYPE, "text/html"),
                        (header::CONTENT_SECURITY_POLICY, "sandbox"),
                    ],
                    ATTACK,
                ).into_response(),
                "/api/v1/files/unchanged.png" => Response::builder()
                    .status(StatusCode::NOT_MODIFIED)
                    .header(header::ETAG, "\"v1\"")
                    .body(Body::empty())
                    .unwrap(),
                "/api/v1/redirect" => {
                    let location = req
                        .uri()
                        .query()
                        .and_then(|q| q.strip_prefix("to="))
                        .map(|value| urlencoding::decode(value).unwrap().into_owned())
                        .unwrap_or_else(|| "/~/api/v1/control/workspaces".into());
                    Response::builder()
                        .status(StatusCode::TEMPORARY_REDIRECT)
                        .header(header::LOCATION, location)
                        .body(Body::empty())
                        .unwrap()
                }
                _ => (
                    [
                        (header::CONTENT_TYPE, "text/html"),
                        (header::REFRESH, "0;url=/~/api/v1/control/workspaces"),
                        (
                            header::LINK,
                            "</~/api/v1/control/workspaces>; rel=preload; as=fetch",
                        ),
                        (
                            header::CONTENT_SECURITY_POLICY,
                            "default-src * 'unsafe-inline'",
                        ),
                        (
                            axum::http::HeaderName::from_static("accept-ch"),
                            "Sec-CH-UA-Platform",
                        ),
                    ],
                    ATTACK,
                )
                    .into_response(),
            }
        }
    })
}

#[tokio::test]
async fn foreign_shell_never_becomes_the_connecting_gateways_frontend() {
    let p = paired_with_engine(malicious_engine).await;
    let response = p
        .device_get("/dev@cloud/", &[("accept", "text/html")])
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = String::from_utf8(body_bytes(response).await).unwrap();
    assert!(!body.contains("foreign-document"), "B supplied A's shell");
    assert!(body.contains("data-trusted-client"), "{body}");
}

#[tokio::test]
async fn direct_foreign_documents_have_an_enforcing_opaque_sandbox() {
    let p = paired_with_engine(malicious_engine).await;
    let response = p
        .device_get("/dev@cloud/api/v1/files/document.html", &[])
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    let policies: Vec<_> = response
        .headers()
        .get_all("content-security-policy")
        .iter()
        .map(|v| v.to_str().unwrap())
        .collect();
    assert!(
        policies
            .iter()
            .any(|policy| policy.split(';').any(|directive| {
                let mut tokens = directive.split_whitespace();
                tokens.next() == Some("sandbox") && !tokens.any(|t| t == "allow-same-origin")
            })),
        "missing enforcing opaque sandbox: {policies:?}"
    );
    assert_eq!(
        response.headers().get("x-content-type-options").unwrap(),
        "nosniff"
    );
    for name in ["refresh", "link", "accept-ch"] {
        assert!(!response.headers().contains_key(name), "{name}");
    }
}

#[tokio::test]
async fn a_stricter_remote_policy_survives_the_relay() {
    let p = paired_with_engine(malicious_engine).await;
    let response = p.device_get("/dev@cloud/api/v1/proxy/locked", &[]).await;
    assert_eq!(response.status(), StatusCode::OK);
    let policies: Vec<_> = response
        .headers()
        .get_all("content-security-policy")
        .iter()
        .map(|v| v.to_str().unwrap().to_owned())
        .collect();
    assert!(policies.iter().any(|p| p == "sandbox"), "{policies:?}");
    assert!(
        policies.iter().any(|p| p.contains("sandbox allow-scripts")),
        "{policies:?}"
    );
}

#[tokio::test]
async fn a_cache_revalidation_is_not_read_as_a_redirect() {
    let p = paired_with_engine(malicious_engine).await;
    let response = p
        .device_get(
            "/dev@cloud/api/v1/files/unchanged.png",
            &[("if-none-match", "\"v1\"")],
        )
        .await;
    assert_eq!(response.status(), StatusCode::NOT_MODIFIED);
    assert_eq!(response.headers()[header::ETAG], "\"v1\"");
}

#[tokio::test]
async fn remote_content_mounts_remain_relayed_and_sandboxed() {
    let p = paired_with_engine(malicious_engine).await;
    for path in [
        "/dev@cloud/data/artifacts/report.html",
        "/dev@cloud/app/demo",
    ] {
        let response = p.device_get(path, &[]).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert!(response.headers()["content-security-policy"]
            .to_str()
            .unwrap()
            .contains("sandbox allow-scripts"));
        assert!(String::from_utf8(body_bytes(response).await)
            .unwrap()
            .contains("foreign-document"));
    }
}

#[tokio::test]
async fn opaque_apps_can_read_only_public_remote_resources_with_cors() {
    let p = paired_with_engine(malicious_engine).await;
    for path in [
        "/dev@cloud/api/v1/sdk.js",
        "/dev@cloud/api/v1/fonts/example.woff2",
    ] {
        let response = p.device_get(path, &[("origin", "null")]).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()["access-control-allow-origin"], "*");
        assert!(!response
            .headers()
            .contains_key("access-control-allow-credentials"));
        assert!(response.headers()["content-security-policy"]
            .to_str()
            .unwrap()
            .contains("sandbox allow-scripts"));
    }
    let response = p.device_get("/dev@cloud/api/v1/threads", &[]).await;
    assert!(!response
        .headers()
        .contains_key("access-control-allow-origin"));
    let response = p
        .device_get("/dev@cloud/api/v1/threads", &[("origin", "null")])
        .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn remote_oauth_callback_cannot_reintroduce_foreign_html_at_the_gateway_origin() {
    let p = paired_with_engine(malicious_engine).await;
    let response = p
        .device_get(
            "/~/oauth/callback?code=c&state=%2Fdev%40cloud%2F~nonce",
            &[],
        )
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    let page = String::from_utf8(body_bytes(response).await).unwrap();
    assert!(!page.contains("foreign-document"));
    assert!(!page.contains("<script>"));
    assert!(page.contains("Authorization complete"));
}

#[tokio::test]
async fn foreign_workers_and_missing_trusted_assets_never_execute() {
    let p = paired_with_engine(malicious_engine).await;
    for path in ["/dev@cloud/sw.js", "/dev@cloud/assets/missing.js"] {
        let response = p.device_get(path, &[("service-worker", "script")]).await;
        assert!(
            !response.status().is_success(),
            "foreign executable served at {path}"
        );
    }
}

#[tokio::test]
async fn remote_push_worker_is_installed_code_with_one_exact_scope() {
    let p = paired_with_engine(malicious_engine).await;
    std::fs::write(p._dirs[2].path().join("sw.js"), "/* A-owned worker */").unwrap();
    let response = p
        .device_get(
            "/~/sw.js?workspace=dev%40cloud",
            &[("service-worker", "script")],
        )
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["service-worker-allowed"], "/dev@cloud/");
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    assert_eq!(body_bytes(response).await, b"/* A-owned worker */");
    for workspace in ["dev", "../", "dev@cloud/", "dev@cloud@other", ""] {
        let response = p
            .device_get(
                &format!("/~/sw.js?workspace={}", urlencoding::encode(workspace)),
                &[],
            )
            .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{workspace}");
        assert!(!response.headers().contains_key("service-worker-allowed"));
    }
    assert!(p.seen.lock().unwrap().is_empty());
}

#[tokio::test]
async fn compatible_peer_uses_only_the_installed_client_and_assets() {
    let p = paired_with_engine(malicious_engine).await;
    let dir = p._dirs[2].path();
    std::fs::create_dir(dir.join("assets")).unwrap();
    std::fs::write(
        dir.join("assets/client.js"),
        "/* trusted installed asset */",
    )
    .unwrap();
    install_client_inventory(dir, &["index.html", "assets/client.js"]);
    let response = p.device_get("/dev@cloud/", &[]).await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = String::from_utf8(body_bytes(response).await).unwrap();
    assert!(body.contains("data-trusted-client"));
    assert!(body.contains("<base href=\"/dev@cloud/\">"));
    assert!(body.contains("name=\"lucidos-workspace-id\" content=\"dev@cloud\""));
    assert!(body.contains("name=\"lucidos-remote-client\" content=\"1\""));
    let response = p.device_get("/dev@cloud/assets/client.js", &[]).await;
    assert_eq!(body_bytes(response).await, b"/* trusted installed asset */");
    let response = p.device_get("/dev@cloud/assets/missing.js", &[]).await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert!(p.seen.lock().unwrap().is_empty());
}

#[tokio::test]
async fn private_origin_refusal_precedes_relay_and_device_authorization() {
    let p = paired_with_engine(malicious_engine).await;
    for path in ["/~/api/v1/control/workspaces", "/dev@cloud/api/v1/threads"] {
        for origin in ["null", "https://untrusted.example.test"] {
            let request = Request::builder()
                .method("POST")
                .uri(path)
                .header(header::COOKIE, p.cookie())
                .header(header::ORIGIN, origin)
                .header(header::HOST, "gateway-a.example.test")
                .body(Body::empty())
                .unwrap();
            assert_eq!(p.to_a(request).await.status(), StatusCode::FORBIDDEN);
        }
    }
    assert!(p.seen.lock().unwrap().is_empty());
}

#[tokio::test]
async fn same_origin_browser_writes_survive_the_gateway_hop() {
    let p = paired_with_engine(malicious_engine).await;
    for metadata in [None, Some("same-origin")] {
        let mut request = Request::builder()
            .method("POST")
            .uri("/dev@cloud/api/v1/chat/stream")
            .header(header::COOKIE, p.cookie())
            .header(header::HOST, "gateway-a.example.test")
            .header(header::ORIGIN, "https://gateway-a.example.test");
        if let Some(site) = metadata {
            request = request.header("sec-fetch-site", site);
        }
        assert_eq!(
            p.to_a(request.body(Body::empty()).unwrap()).await.status(),
            StatusCode::OK
        );
    }
    assert_eq!(
        p.seen
            .lock()
            .unwrap()
            .iter()
            .filter(|path| path.as_str() == "/api/v1/chat/stream")
            .count(),
        2
    );
}

#[tokio::test]
async fn http2_writes_and_external_shell_links_keep_their_browser_boundary() {
    let p = paired_with_engine(malicious_engine).await;
    let request = Request::builder()
        .method("POST")
        .uri("https://gateway-a.example.test/dev@cloud/api/v1/chat/stream")
        .version(axum::http::Version::HTTP_2)
        .header(header::COOKIE, p.cookie())
        .header(header::ORIGIN, "https://gateway-a.example.test")
        .header("sec-fetch-site", "same-origin")
        .body(Body::empty())
        .unwrap();
    assert_eq!(p.to_a(request).await.status(), StatusCode::OK);
    for (path, expected) in [
        ("/dev@cloud/", StatusCode::OK),
        ("/dev@cloud/index.html", StatusCode::OK),
        ("/dev@cloud/api/v1/events", StatusCode::FORBIDDEN),
        ("/~/api/v1/control/workspaces", StatusCode::FORBIDDEN),
    ] {
        let response = p
            .device_get(
                path,
                &[
                    ("sec-fetch-site", "cross-site"),
                    ("sec-fetch-mode", "navigate"),
                    ("sec-fetch-dest", "document"),
                ],
            )
            .await;
        assert_eq!(response.status(), expected, "{path}");
    }
}

#[tokio::test]
async fn http2_remote_migration_redirect_keeps_only_the_path_and_query() {
    let p = paired_with_engine(malicious_engine).await;
    let request = Request::builder()
        .uri("https://gateway-a.example.test/dev@cloud/?thread=demo")
        .version(axum::http::Version::HTTP_2)
        .header(header::COOKIE, p.cookie())
        .header("sec-fetch-mode", "navigate")
        .header("sec-fetch-dest", "document")
        .body(Body::empty())
        .unwrap();
    let response = p.to_a(request).await;
    assert!(response.status().is_redirection());
    let location = response.headers()[header::LOCATION].to_str().unwrap();
    let target =
        urlencoding::decode(location.strip_prefix("/~/remote-client?target=").unwrap()).unwrap();
    assert_eq!(target, "/dev@cloud/?thread=demo");
    assert_eq!(p.device_get(location, &[]).await.status(), StatusCode::OK);
}

#[tokio::test]
async fn remote_migration_marker_reads_all_http2_cookie_fields() {
    let p = paired_with_engine(malicious_engine).await;
    let request = Request::builder()
        .uri("/dev@cloud/")
        .header(header::COOKIE, p.cookie())
        .header(header::COOKIE, "lucidos-remote-client=1")
        .header("sec-fetch-dest", "document")
        .body(Body::empty())
        .unwrap();
    assert_eq!(p.to_a(request).await.status(), StatusCode::OK);
}

#[tokio::test]
async fn remote_health_preserves_the_stopped_workspace_signal() {
    let p = paired_with_engine(malicious_engine).await;
    p.b.register_for_test(crate::registry::Workspace::gateway_provisioned(
        "dev".into(),
        "Dev".into(),
        51996,
    ));
    p.b.stop_workspace("dev", None).await.unwrap();
    let response = p.device_get("/dev@cloud/api/v1/health", &[]).await;
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        response.headers()[crate::server::WORKSPACE_STATE_HEADER],
        crate::server::WORKSPACE_STOPPED
    );
    assert!(
        p.b.engine_address("dev").is_none(),
        "a poll must not start the workspace"
    );
}

#[tokio::test]
async fn scoped_data_redirects_preserve_query_and_fragment() {
    let p = paired_with_engine(malicious_engine).await;
    for target in [
        "files/document.html?q=1#part",
        "/dev@cloud/api/v1/files/document.html?q=1#part",
    ] {
        let response = p
            .device_get(
                &format!(
                    "/dev@cloud/api/v1/redirect?to={}",
                    urlencoding::encode(target)
                ),
                &[],
            )
            .await;
        assert_eq!(response.status(), StatusCode::TEMPORARY_REDIRECT);
        assert_eq!(
            response.headers()[header::LOCATION],
            "/dev@cloud/api/v1/files/document.html?q=1#part"
        );
    }
}

#[tokio::test]
async fn foreign_redirects_cannot_target_the_connecting_gateway() {
    let p = paired_with_engine(malicious_engine).await;
    for target in [
        "/~/api/v1/control/workspaces",
        "/other@sibling/api/v1/threads/list",
        "../../~/api/v1/control/workspaces",
        "/dev@cloud/../other@sibling/api/v1/threads/list",
        "/dev@cloud/%2e%2e/other@sibling/api/v1/threads/list",
        "//localhost/~/api/v1/control/workspaces",
        "https://gateway-a.example.test/~/api/v1/control/workspaces",
        r"/dev@cloud/..\other@sibling/api/v1/threads/list",
    ] {
        let path = format!(
            "/dev@cloud/api/v1/redirect?to={}",
            urlencoding::encode(target)
        );
        let response = p.device_get(&path, &[]).await;
        assert!(
            !response.status().is_redirection() && !response.status().is_success(),
            "unsafe redirect {target} returned {}",
            response.status()
        );
    }
}

#[tokio::test]
#[ignore = "requires the provisioned Playwright browsers; invoke by name"]
async fn remote_browser_origin_boundary() {
    let p = paired_with_engine(malicious_engine).await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = format!("http://localhost:{}", listener.local_addr().unwrap().port());
    let router = crate::server::gateway_router(p.a.clone()).route(
        "/dev@cloud/legacy-worker.js",
        axum::routing::get(|| async {
            ([(header::CONTENT_TYPE, "application/javascript")], WORKER)
        }),
    );
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(100),
        tokio::process::Command::new("node")
            .arg(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/tests/fixtures/browser-isolation.mjs"
            ))
            .arg(&address)
            .arg(p.a.device_cookie_name())
            .arg("cred-abc")
            .kill_on_drop(true)
            .output(),
    )
    .await;
    server.abort();
    let output = result
        .expect("browser probes finish in 100 seconds")
        .expect("start node");
    assert!(
        output.status.success(),
        "browser isolation failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        p.a.remote_gateways()
            .list()
            .iter()
            .all(|g| g.return_access.is_none()),
        "foreign browser content must not enable agent return access"
    );
    assert!(
        !p.seen
            .lock()
            .unwrap()
            .iter()
            .any(|path| [RECEIVE_ROUTE, RECEIVE_START_ROUTE]
                .iter()
                .any(|route| path.ends_with(route))),
        "foreign browser content must not dispatch a workspace operation"
    );
}

#[tokio::test]
async fn gateway_support_does_not_certify_an_old_running_engine() {
    let p =
        paired_with_engine(|_, _| axum::Router::new().fallback(|| async { "old engine frontend" }))
            .await;
    for path in [
        "/dev@cloud/",
        "/dev@cloud/api/v1/threads/list",
        "/dev@cloud/data/report.html",
    ] {
        let response = p
            .device_get(
                path,
                &[("sec-fetch-dest", "empty"), ("accept", "text/html")],
            )
            .await;
        assert_eq!(response.status(), StatusCode::UPGRADE_REQUIRED, "{path}");
        let body: serde_json::Value = serde_json::from_slice(&body_bytes(response).await).unwrap();
        assert_eq!(body["error"], "workspace_client_mismatch");
        assert_eq!(body["behind"], "remote-engine");
        assert!(body["theirs"].is_null());
    }
}

#[tokio::test]
async fn a_public_asset_mismatch_does_not_disclose_contract_details() {
    let p =
        paired_with_engine(|_, _| axum::Router::new().fallback(|| async { "old engine" })).await;
    let response = p
        .to_a(
            Request::builder()
                .uri("/dev@cloud/api/v1/sdk.js")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(response.status(), StatusCode::UPGRADE_REQUIRED);
    let body: serde_json::Value = serde_json::from_slice(&body_bytes(response).await).unwrap();
    assert_eq!(
        body,
        serde_json::json!({"error": "workspace_client_mismatch"})
    );
}

const PAGE_LOAD: &[(&str, &str)] = &[("sec-fetch-dest", "document")];

#[tokio::test]
async fn running_engine_contract_is_checked_fresh_and_rejects_malformed_support() {
    let health = Arc::new(Mutex::new(serde_json::json!({
        "workspace_client": lucidos_gateway_stamp::workspace_client::ClientContract::engine(),
        "release": "different-engine-build",
        "workspace_path": "/private/workspace",
    })));
    let reply = health.clone();
    let p = paired_with_engine(move |_, _| {
        axum::Router::new().fallback(move || {
            let reply = reply.lock().unwrap().clone();
            async move { axum::Json(reply) }
        })
    })
    .await;
    assert_eq!(
        p.device_get("/dev@cloud/", PAGE_LOAD).await.status(),
        StatusCode::OK
    );
    let version = lucidos_gateway_stamp::workspace_client::VERSION;
    for (invalid, side, range) in [
        (serde_json::json!({}), "remote-engine", String::new()),
        (
            serde_json::json!("not a health object"),
            "remote-engine",
            String::new(),
        ),
        (
            serde_json::json!({"workspace_client": {"min": version + 1, "max": version + 3, "capabilities": ["workspace-api-v1"]}}),
            "this-gateway",
            format!("{}-{}", version + 1, version + 3),
        ),
        (
            serde_json::json!({"workspace_client": {"min": 0, "max": version - 1, "capabilities": []}}),
            "remote-engine",
            format!("0-{}", version - 1),
        ),
        (
            serde_json::json!({"workspace_client": lucidos_gateway_stamp::workspace_client::ClientContract::gateway()}),
            "remote-engine",
            format!("{version}-{version}"),
        ),
        (
            serde_json::json!({"padding": "x".repeat(crate::remote_browser::MAX_CLIENT_HEALTH_BYTES + 1), "workspace_client": lucidos_gateway_stamp::workspace_client::ClientContract::engine()}),
            "remote-engine",
            String::new(),
        ),
    ] {
        *health.lock().unwrap() = invalid;
        let response = p.device_get("/dev@cloud/", PAGE_LOAD).await;
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        let location = response.headers()[header::LOCATION].to_str().unwrap();
        assert!(location.starts_with("/~/?pick&"));
        assert!(location.contains(&format!("behind={side}&")));
        assert!(location.contains(&format!("theirs={range}&")));
    }
    *health.lock().unwrap() = serde_json::json!({"workspace_client": {
        "min": 0, "max": version + 1, "capabilities": ["workspace-api-v1", "future-addition"]
    }});
    assert_eq!(
        p.device_get("/dev@cloud/", PAGE_LOAD).await.status(),
        StatusCode::OK
    );
}

#[tokio::test]
async fn installed_inventory_refuses_unlisted_or_changed_assets() {
    let p = paired_with_engine(malicious_engine).await;
    let dir = p._dirs[2].path();
    std::fs::write(dir.join("stale.js"), "old installed code").unwrap();
    assert_eq!(
        p.device_get("/dev@cloud/stale.js", &[]).await.status(),
        StatusCode::NOT_FOUND
    );
    install_client_inventory(dir, &["index.html", "stale.js"]);
    std::fs::write(dir.join("stale.js"), "changed installed code").unwrap();
    assert_eq!(
        p.device_get("/dev@cloud/stale.js", &[]).await.status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
    std::fs::write(dir.join("index.html"), "changed shell").unwrap();
    assert_eq!(
        p.device_get("/dev@cloud/", &[]).await.status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
    std::fs::remove_file(dir.join("workspace-client-assets.json")).unwrap();
    assert_eq!(
        p.device_get("/dev@cloud/", &[]).await.status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
    assert!(p.seen.lock().unwrap().is_empty());
}

#[tokio::test]
async fn engine_contract_probe_requires_pairing_and_signature_and_projects_only_contract() {
    let p = paired_with_engine(|_, _| {
        axum::Router::new().fallback(|| async {
            axum::Json(serde_json::json!({
                "workspace_client": lucidos_gateway_stamp::workspace_client::ClientContract::engine(),
                "workspace_path": "/private/workspace", "private_configuration": "never returned",
            }))
        })
    }).await;
    for (credential, signature, foreign_origin, status) in [
        (false, false, false, StatusCode::UNAUTHORIZED),
        (true, false, false, StatusCode::UNAUTHORIZED),
        (false, false, true, StatusCode::UNAUTHORIZED),
        (true, true, false, StatusCode::OK),
        (true, true, true, StatusCode::OK),
    ] {
        let mut request =
            Request::builder().uri(format!("{GATEWAY_API_PREFIX}{RELAY_PATH}/dev/~client"));
        if credential {
            request = request.header(HEADER_GATEWAY_CREDENTIAL, &p.credential);
        }
        if foreign_origin {
            request = request
                .header(header::HOST, "gateway-b.example.test")
                .header(header::ORIGIN, "https://gateway-a.example.test");
        }
        let mut request = request.body(Body::empty()).unwrap();
        if signature {
            request.headers_mut().extend(signed(
                &claim("/dev@cloud/", chrono::Utc::now().timestamp(), None),
                &p.credential,
            ));
        }
        let response = crate::server::gateway_router(p.b.clone())
            .oneshot(request)
            .await
            .unwrap();
        assert_eq!(response.status(), status);
        let body = String::from_utf8(body_bytes(response).await).unwrap();
        assert!(!body.contains("private"), "{body}");
        if status == StatusCode::OK {
            let contract: serde_json::Value = serde_json::from_str(&body).unwrap();
            assert_eq!(contract.as_object().unwrap().len(), 3);
        }
    }
}

#[tokio::test]
async fn browser_shell_requires_trusted_retirement_entry_and_validates_its_destination() {
    let p = paired_with_engine(malicious_engine).await;
    let req = Request::builder()
        .uri("/dev@cloud/?thread=example")
        .header(header::COOKIE, p.cookie())
        .header(header::ACCEPT, "text/html")
        .body(Body::empty())
        .unwrap();
    let response = p.to_a(req).await;
    assert!(response.status().is_redirection());
    assert_eq!(
        response.headers()[header::LOCATION],
        "/~/remote-client?target=%2Fdev%40cloud%2F%3Fthread%3Dexample"
    );
    for target in [
        "https://outside.example/",
        "//outside.example/",
        "/local/",
        "/~/",
        "/dev@cloud/api/v1/private",
        "/dev@cloud/../local/",
    ] {
        let response = p
            .device_get(
                &format!("/~/remote-client?target={}", urlencoding::encode(target)),
                &[],
            )
            .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{target}");
    }
    let response = p
        .device_get(
            "/~/remote-client?target=%2Fdev%40cloud%2F%3Fthread%3Dexample",
            &[],
        )
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    let html = String::from_utf8(body_bytes(response).await).unwrap();
    assert!(html.contains("lucidosRemoteClientReady.then"));
    assert!(html.contains("getRegistrations"));
    assert!(!html.contains("foreign-document"));
}

#[tokio::test]
async fn opening_probe_requires_a_signed_device_and_unknown_targets_do_not_start() {
    let p = paired_with_engine(malicious_engine).await;
    for (device, expected) in [
        (None, StatusCode::FORBIDDEN),
        (Some(device("browser")), StatusCode::NOT_FOUND),
    ] {
        let mut request = Request::builder()
            .uri(format!(
                "{GATEWAY_API_PREFIX}{RELAY_PATH}/absent/~client/open"
            ))
            .header(HEADER_GATEWAY_CREDENTIAL, &p.credential)
            .body(Body::empty())
            .unwrap();
        request.headers_mut().extend(signed(
            &claim("/absent@cloud/", chrono::Utc::now().timestamp(), device),
            &p.credential,
        ));
        assert_eq!(
            to_b(&p, "/absent/~client/open", request.headers().clone()).await,
            expected
        );
    }
}

#[tokio::test]
async fn a_page_load_of_a_workspace_b_no_longer_has_goes_to_the_list() {
    let p = paired_with_engine(malicious_engine).await;
    let page = p
        .device_get("/absent@cloud/", &[("sec-fetch-dest", "document")])
        .await;
    assert_eq!(page.status(), StatusCode::TEMPORARY_REDIRECT);
    assert_eq!(page.headers()[header::LOCATION], "/~/?pick");
}

#[tokio::test]
#[ignore = "run through scripts/e2e-api.sh with the production client and engine"]
async fn remote_production_client_continuity() {
    let workspace =
        std::path::PathBuf::from(std::env::var("E2E_WORKSPACE").expect("use scripts/e2e-api.sh"));
    let key_text = std::fs::read_to_string(workspace.join(".lucidos/e2e-stamp-key")).unwrap();
    let mut lines = key_text.lines();
    let key =
        lucidos_gateway_stamp::StampKey::read_handover(lines.next().unwrap().as_bytes()).unwrap();
    let slug = lines.next().unwrap();
    assert!(
        slug.starts_with("e2e-test"),
        "only a disposable e2e workspace"
    );
    let token = lucidos_local_token::read().expect("the disposable engine's frame-capability key");
    let p = paired_with_engine_token(malicious_engine, &token).await;
    p.b.set_route_for_test(
        slug,
        crate::engine_hop::EngineAddress::at(
            lucidos_engine_socket::path_for(&workspace).unwrap(),
            std::env::var("VITE_PORT").unwrap().parse().unwrap(),
        ),
    );
    p.b.set_stamp_key_for_test(slug, key);
    fn copy_tree(source: &std::path::Path, destination: &std::path::Path) {
        std::fs::create_dir_all(destination).unwrap();
        for entry in std::fs::read_dir(source).unwrap() {
            let entry = entry.unwrap();
            let target = destination.join(entry.file_name());
            if entry.file_type().unwrap().is_dir() {
                copy_tree(&entry.path(), &target);
            } else {
                std::fs::copy(entry.path(), target).unwrap();
            }
        }
    }
    copy_tree(
        &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../lucidos-app/dist"),
        p._dirs[2].path(),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://localhost:{}", listener.local_addr().unwrap().port());
    let (disconnect, _) = tokio::sync::broadcast::channel::<()>(1);
    let signal = disconnect.clone();
    let router = crate::server::gateway_router(p.a.clone())
        .route(
            "/__fixture/disconnect-streams",
            axum::routing::post(move || {
                let signal = signal.clone();
                async move {
                    let _ = signal.send(());
                    StatusCode::NO_CONTENT
                }
            }),
        )
        .layer(axum::middleware::from_fn(
            move |req: Request, next: axum::middleware::Next| {
                let stream_request = req.uri().path().ends_with("/api/v1/events");
                let mut disconnected = disconnect.subscribe();
                async move {
                    let response = next.run(req).await;
                    if !stream_request {
                        return response;
                    }
                    let (parts, body) = response.into_parts();
                    let stream =
                        futures::StreamExt::take_until(body.into_data_stream(), async move {
                            let _ = disconnected.recv().await;
                        });
                    Response::from_parts(parts, Body::from_stream(stream))
                }
            },
        ));
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(180),
        tokio::process::Command::new("node")
            .arg(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/tests/fixtures/production-client.mjs"
            ))
            .arg(&origin)
            .arg(p.a.device_cookie_name())
            .arg("cred-abc")
            .arg(format!("/{slug}@cloud"))
            .arg(p._dirs[2].path())
            .kill_on_drop(true)
            .output(),
    )
    .await;
    server.abort();
    let output = result
        .expect("production browser checks finish in 180 seconds")
        .unwrap();
    assert!(
        output.status.success(),
        "production client continuity failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[tokio::test]
async fn preview_without_trust_never_contacts_the_peer() {
    let p = paired().await;
    let old = p.a.remote_gateways().list().remove(0);
    let mut known = old.clone();
    known.gateway_id = Some(p.b.gateway_id().into());
    let calls = Arc::new(AtomicUsize::new(0));
    let counted = calls.clone();
    let peer = serve_https(axum::Router::new().fallback(move || {
        counted.fetch_add(1, Ordering::SeqCst);
        async { StatusCode::INTERNAL_SERVER_ERROR }
    }));
    known.address = format!("https://localhost:{}", peer.port());
    p.a.remote_gateways().replace(&old, known).unwrap();
    let key = lucidos_gateway_stamp::frontend_preview::key(p.b.gateway_id());
    for suffix in ["", "@vite/client"] {
        let response = p
            .device_get(&format!("/~preview/{key}/dev/{suffix}"), &[])
            .await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        let body = String::from_utf8(body_bytes(response).await).unwrap();
        assert!(
            body.contains("cloud") && body.contains("trusted pairing"),
            "{body}"
        );
    }
    assert!(p.seen.lock().unwrap().is_empty());
    let response = p
        .device_get(
            &format!("/~preview/{key}/dev/"),
            &[("connection", "upgrade"), ("upgrade", "websocket")],
        )
        .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert_eq!(
        calls.load(Ordering::SeqCst),
        0,
        "even probes must stay local"
    );
    assert!(p.b.directory().relayed.is_empty());
}

#[tokio::test]
async fn unknown_preview_key_is_an_explained_refusal() {
    let p = paired().await;
    let response = p.device_get("/~preview/0123456789abcdef/dev/", &[]).await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let body = String::from_utf8(body_bytes(response).await).unwrap();
    assert!(
        body.contains("re-pair") && body.contains("update"),
        "{body}"
    );
    assert!(p.seen.lock().unwrap().is_empty());
}
