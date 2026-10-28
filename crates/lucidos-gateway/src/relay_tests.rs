use super::*;
use crate::engine_hop::test_engine;
use crate::gateway_protocol::GatewayRedemption;
use crate::remote_gateways::tests::{localhost_cert, serve_https};
use crate::remote_gateways::RemoteGateway;
use std::sync::{Arc, Mutex};
use tower::ServiceExt as _;

const CREDENTIAL: &str = "the-gateway-credential";
const NOW: i64 = 1_800_000_000;

fn device(id: &str) -> AuthenticatedDevice {
    AuthenticatedDevice {
        id: id.to_string(),
        label: "Café | iPhone".to_string(),
    }
}

fn claim(prefix: &str, issued_at: i64, device: Option<AuthenticatedDevice>) -> RelayClaim {
    RelayClaim {
        issued_at,
        prefix: prefix.to_string(),
        device,
    }
}

/// Headers carrying `claim`, signed with `key`.
fn signed(claim: &RelayClaim, key: &str) -> HeaderMap {
    let raw = claim.encode();
    let mut headers = HeaderMap::new();
    headers.insert(HEADER_RELAY_CLAIM, raw.parse().unwrap());
    headers.insert(HEADER_RELAY_SIGNATURE, sign(key, &raw).parse().unwrap());
    headers
}

// ── Routes never collide ────────────────────────────────────────────────────

#[test]
fn a_remote_route_needs_a_slug_and_a_name() {
    assert_eq!(remote_route("dev@cloud"), Some(("dev", "cloud")));
    for not_remote in [
        "dev",
        "dev@",
        "@cloud",
        "@",
        "Dev@cloud",
        "dev@cl@oud",
        "dev@../x",
    ] {
        assert_eq!(remote_route(not_remote), None, "{not_remote}");
    }
}

// ── The claim ───────────────────────────────────────────────────────────────

#[test]
fn a_claim_survives_its_header_encoding() {
    for device in [Some(device("device-1")), None] {
        let original = claim("/dev@cloud/", NOW, device);
        let raw = original.encode();
        assert!(HeaderValue::from_str(&raw).is_ok(), "{raw}");
        assert_eq!(RelayClaim::decode(&raw), Some(original));
    }
    assert_eq!(RelayClaim::decode("v2|1|%2F|a|b"), None);
    assert_eq!(RelayClaim::decode("v1|1|%2F|a|b|extra"), None);
}

#[test]
fn a_valid_claim_verifies() {
    let original = claim("/dev@cloud/", NOW, Some(device("device-1")));
    let headers = signed(&original, CREDENTIAL);
    assert_eq!(
        verify_claim(&headers, CREDENTIAL, "dev", NOW + 30),
        Ok(original)
    );
}

#[test]
fn a_claim_without_its_headers_is_refused() {
    assert_eq!(
        verify_claim(&HeaderMap::new(), CREDENTIAL, "dev", NOW),
        Err(ClaimRefusal::Missing)
    );
}

#[test]
fn a_claim_signed_with_another_credential_is_refused() {
    let headers = signed(&claim("/dev@cloud/", NOW, None), "another-credential");
    assert_eq!(
        verify_claim(&headers, CREDENTIAL, "dev", NOW),
        Err(ClaimRefusal::BadSignature)
    );
}

#[test]
fn a_claim_changed_after_signing_is_refused() {
    let mut headers = signed(
        &claim("/dev@cloud/", NOW, Some(device("device-1"))),
        CREDENTIAL,
    );
    let forged = claim("/dev@cloud/", NOW, Some(device("someone-else"))).encode();
    headers.insert(HEADER_RELAY_CLAIM, forged.parse().unwrap());
    assert_eq!(
        verify_claim(&headers, CREDENTIAL, "dev", NOW),
        Err(ClaimRefusal::BadSignature)
    );
}

#[test]
fn a_stale_or_future_claim_is_refused() {
    let headers = signed(&claim("/dev@cloud/", NOW, None), CREDENTIAL);
    let skew = CLAIM_MAX_SKEW_SECS as i64;
    for now in [NOW + skew + 1, NOW - skew - 1] {
        assert_eq!(
            verify_claim(&headers, CREDENTIAL, "dev", now),
            Err(ClaimRefusal::Stale)
        );
    }
}

#[test]
fn an_extreme_issue_time_is_stale_rather_than_a_panic() {
    for issued_at in [i64::MIN, i64::MAX] {
        let headers = signed(&claim("/dev@cloud/", issued_at, None), CREDENTIAL);
        assert_eq!(
            verify_claim(&headers, CREDENTIAL, "dev", NOW),
            Err(ClaimRefusal::Stale)
        );
    }
}

#[test]
fn a_claim_naming_another_workspace_is_refused() {
    for prefix in [
        "/other@cloud/",
        "/dev/",
        "/dev@cloud",
        "dev@cloud/",
        "/dev@/",
    ] {
        let headers = signed(&claim(prefix, NOW, None), CREDENTIAL);
        assert_eq!(
            verify_claim(&headers, CREDENTIAL, "dev", NOW),
            Err(ClaimRefusal::PrefixMismatch),
            "{prefix}"
        );
    }
}

#[test]
fn a_device_id_that_would_break_the_namespace_is_refused() {
    for id in ["pairing:device", "a b", &"x".repeat(129)] {
        let headers = signed(&claim("/dev@cloud/", NOW, Some(device(id))), CREDENTIAL);
        assert_eq!(
            verify_claim(&headers, CREDENTIAL, "dev", NOW),
            Err(ClaimRefusal::BadDeviceId),
            "{id}"
        );
    }
}

// ── End to end: A relays to B over https ────────────────────────────────────

/// What B's fake engine saw: each request's target and headers, lowercased.
pub(crate) type Seen = Arc<Mutex<Vec<String>>>;

const PAGE: &str = "<!doctype html><html><head><base href=\"/dev/\"></head><body>B</body></html>";
const GZIP_BYTES: &[u8] = &[0x1f, 0x8b, 0x08, 0x00, 0xde, 0xad, 0xbe, 0xef];

/// Response headers that would set state for A's whole origin.
const ORIGIN_STATE_HEADERS: &[(&str, &str)] = &[
    ("set-cookie", "lucidos_device_x=planted; Path=/"),
    ("clear-site-data", "\"cookies\""),
    ("service-worker-allowed", "/"),
    ("strict-transport-security", "max-age=63072000"),
    ("alt-svc", "h3=\":443\""),
    ("www-authenticate", "Basic realm=\"x\""),
    ("access-control-allow-origin", "*"),
];

/// B's engine for `dev`: records every request, and serves a page, a gzip
/// body, a two-chunk SSE stream held between chunks, and a font.
fn engine(seen: Seen, held: tokio::sync::oneshot::Receiver<()>) -> axum::Router {
    let held = Arc::new(Mutex::new(Some(held)));
    let record = move |req: Request| {
        let mut line = format!("{} {}\n", req.method(), req.uri());
        for (name, value) in req.headers() {
            line.push_str(&format!("{}: {}\n", name, value.to_str().unwrap_or("?")));
        }
        seen.lock().unwrap().push(line.to_lowercase());
        req
    };
    axum::Router::new().fallback(move |req: Request| {
        let req = record(req);
        let held = held.clone();
        async move {
            match req.uri().path() {
                "/" => ([(header::CONTENT_TYPE, "text/html")], PAGE).into_response(),
                "/api/v1/gz" => (
                    [
                        (header::CONTENT_TYPE, "application/json"),
                        (header::CONTENT_ENCODING, "gzip"),
                    ],
                    GZIP_BYTES,
                )
                    .into_response(),
                "/api/v1/events" => {
                    use futures::StreamExt as _;
                    let gate = held.lock().unwrap().take();
                    let first = futures::stream::once(async {
                        Ok::<_, std::io::Error>(axum::body::Bytes::from_static(b"data: one\n\n"))
                    });
                    let second = futures::stream::once(async move {
                        if let Some(gate) = gate {
                            let _ = gate.await;
                        }
                        Ok(axum::body::Bytes::from_static(b"data: two\n\n"))
                    });
                    (
                        [(header::CONTENT_TYPE, "text/event-stream")],
                        Body::from_stream(first.chain(second)),
                    )
                        .into_response()
                }
                "/api/v1/origin-state" => {
                    let mut response = "ok".into_response();
                    for (name, value) in ORIGIN_STATE_HEADERS {
                        response.headers_mut().append(*name, value.parse().unwrap());
                    }
                    response
                }
                _ => "ok".into_response(),
            }
        }
    })
}

/// Gateway A, paired with gateway B as `cloud`, B serving `dev`.
pub(crate) struct Paired {
    pub(crate) a: GatewayState,
    pub(crate) b: GatewayState,
    pub(crate) b_address: String,
    pub(crate) credential: String,
    pub(crate) pairing_id: String,
    pub(crate) seen: Seen,
    release: Option<tokio::sync::oneshot::Sender<()>>,
    _dirs: [tempfile::TempDir; 3],
}

pub(crate) async fn paired() -> Paired {
    let seen: Seen = Arc::default();
    let (release, held) = tokio::sync::oneshot::channel();
    let engine_dir = test_engine::scratch();
    let b_dir = tempfile::tempdir().unwrap();
    let b = GatewayState::for_tests_with_static_dir(Some(b_dir.path().to_path_buf()));
    b.set_route_for_test(
        "dev",
        test_engine::serve(&engine_dir, engine(seen.clone(), held)),
    );
    let cert = localhost_cert();
    let port = serve_https(crate::server::gateway_router(b.clone()), &cert).await;
    let b_address = format!("https://localhost:{port}");

    let code = b.paired_gateways().mint_code(None).unwrap();
    let GatewayRedemption::Paired {
        pairing_id,
        credential,
    } = b
        .paired_gateways()
        .redeem(&code, Some("Gateway A"))
        .unwrap()
    else {
        panic!("a fresh code redeems");
    };

    let a_dir = tempfile::tempdir().unwrap();
    let a = GatewayState::for_tests_with_static_dir(Some(a_dir.path().to_path_buf()));
    a.relay_client().trust_for_test(cert.cert.pem().as_bytes());
    a.remote_gateways()
        .add(RemoteGateway {
            name: "cloud".to_string(),
            address: b_address.clone(),
            pairing_id: pairing_id.clone(),
            credential: credential.clone(),
            gateway_protocol: 1,
            added_at: "2020-01-01T00:00:00Z".to_string(),
        })
        .unwrap();
    a.write_paired_devices_for_test(|paired| {
        paired.devices.push(auth::PairedDevice {
            id: "device-1".into(),
            label: "My iPhone".into(),
            credential_digest: auth::digest("cred-abc"),
            paired_at: "2020-01-01T00:00:00Z".into(),
            last_seen_at: None,
        });
    })
    .unwrap();
    Paired {
        a,
        b,
        b_address,
        credential,
        pairing_id,
        seen,
        release: Some(release),
        _dirs: [engine_dir, b_dir, a_dir],
    }
}

impl Paired {
    fn cookie(&self) -> String {
        format!("{}=cred-abc", self.a.device_cookie_name())
    }

    /// A paired device's request to A, plus `extra` headers.
    async fn device_get(&self, path: &str, extra: &[(&str, &str)]) -> Response {
        let mut builder = axum::http::Request::builder()
            .uri(path)
            .header(header::COOKIE, self.cookie());
        for (name, value) in extra {
            builder = builder.header(*name, *value);
        }
        self.to_a(builder.body(Body::empty()).unwrap()).await
    }

    async fn to_a(&self, req: axum::http::Request<Body>) -> Response {
        crate::server::gateway_router(self.a.clone())
            .oneshot(req)
            .await
            .unwrap()
    }

    fn last_seen(&self) -> Option<String> {
        self.seen.lock().unwrap().last().cloned()
    }
}

async fn body_bytes(response: Response) -> Vec<u8> {
    axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap()
        .to_vec()
}

#[tokio::test]
async fn a_paired_device_reaches_bs_engine_under_a_namespaced_id_and_the_outside_prefix() {
    let p = paired().await;
    let response = p
        .device_get("/dev@cloud/api/v1/threads/list?limit=5", &[])
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(body_bytes(response).await, b"ok");
    let seen = p.last_seen().expect("B's engine was reached");
    assert!(
        seen.starts_with("get /api/v1/threads/list?limit=5\n"),
        "{seen}"
    );
    let device = format!("x-lucidos-device-id: {}:device-1\n", p.pairing_id).to_lowercase();
    assert!(seen.contains(&device), "{seen}");
    assert!(
        seen.contains("x-lucidos-device-label: my%20iphone\n"),
        "{seen}"
    );
    assert!(seen.contains("x-forwarded-prefix: /dev@cloud/\n"), "{seen}");
    for never in [
        HEADER_GATEWAY_CREDENTIAL,
        HEADER_RELAY_CLAIM,
        HEADER_RELAY_SIGNATURE,
    ] {
        assert!(!seen.contains(never), "{never} reached the engine: {seen}");
    }
    assert!(!seen.contains(&p.credential.to_lowercase()), "{seen}");
}

#[tokio::test]
async fn client_sent_device_prefix_and_relay_headers_are_stripped_on_both_hops() {
    let p = paired().await;
    let response = p
        .device_get(
            "/dev@cloud/api/v1/threads/list",
            &[
                ("x-lucidos-device-id", "forged-device"),
                ("x-lucidos-device-label", "Forged"),
                ("x-forwarded-prefix", "/elsewhere/"),
                (HEADER_RELAY_CLAIM, "v1|0|%2F|forged|forged"),
                (HEADER_RELAY_SIGNATURE, "00"),
                (HEADER_GATEWAY_CREDENTIAL, "forged-credential"),
            ],
        )
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    let seen = p.last_seen().unwrap();
    assert!(!seen.contains("forged"), "{seen}");
    assert!(!seen.contains("/elsewhere/"), "{seen}");
    assert_eq!(seen.matches("x-lucidos-device-id:").count(), 1, "{seen}");
    assert_eq!(seen.matches("x-forwarded-prefix:").count(), 1, "{seen}");
}

#[tokio::test]
async fn as_cookie_never_reaches_b() {
    let p = paired().await;
    let response = p.device_get("/dev@cloud/api/v1/threads/list", &[]).await;
    assert_eq!(response.status(), StatusCode::OK);
    let seen = p.last_seen().unwrap();
    assert!(!seen.contains("cred-abc"), "{seen}");
    assert!(!seen.contains("cookie:"), "{seen}");
}

#[tokio::test]
async fn bs_response_cannot_set_state_on_as_origin() {
    let p = paired().await;
    let response = p.device_get("/dev@cloud/api/v1/origin-state", &[]).await;
    assert_eq!(response.status(), StatusCode::OK);
    // A may refresh its own device cookie, so the check is B's values.
    for (name, value) in ORIGIN_STATE_HEADERS {
        let arrived = response
            .headers()
            .get_all(*name)
            .iter()
            .any(|v| v.as_bytes() == value.as_bytes());
        assert!(!arrived, "{name} reached A's origin");
    }
    assert!(response.headers().contains_key(header::CONTENT_TYPE));
}

#[test]
fn the_relay_drops_origin_state_headers_and_keeps_the_rest() {
    for (name, _) in ORIGIN_STATE_HEADERS {
        let name = axum::http::HeaderName::from_static(name);
        assert!(!super::relays_response_header(&name), "{name}");
    }
    for kept in ["content-type", "cache-control", "etag", "x-request-id"] {
        let name = axum::http::HeaderName::from_static(kept);
        assert!(super::relays_response_header(&name), "{kept}");
    }
}

#[tokio::test]
async fn the_page_comes_back_byte_for_byte() {
    let p = paired().await;
    let response = p
        .device_get("/dev@cloud/", &[("accept", "text/html")])
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(body_bytes(response).await, PAGE.as_bytes());
}

#[tokio::test]
async fn a_compressed_body_crosses_untouched() {
    let p = paired().await;
    let response = p
        .device_get("/dev@cloud/api/v1/gz", &[("accept-encoding", "gzip")])
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[header::CONTENT_ENCODING], "gzip");
    assert_eq!(body_bytes(response).await, GZIP_BYTES);
    assert!(p.last_seen().unwrap().contains("accept-encoding: gzip"));
}

#[tokio::test]
async fn an_event_stream_flows_live_chunk_by_chunk() {
    use futures::StreamExt as _;
    let mut p = paired().await;
    let response = p.device_get("/dev@cloud/api/v1/events", &[]).await;
    assert_eq!(response.status(), StatusCode::OK);
    let mut body = response.into_body().into_data_stream();
    let first = body.next().await.expect("a first chunk").unwrap();
    assert_eq!(&first[..], b"data: one\n\n");
    p.release.take().unwrap().send(()).unwrap();
    let second = body.next().await.expect("a second chunk").unwrap();
    assert_eq!(&second[..], b"data: two\n\n");
}

#[tokio::test]
async fn a_posted_body_streams_through() {
    let p = paired().await;
    let req = axum::http::Request::builder()
        .method("POST")
        .uri("/dev@cloud/api/v1/threads")
        .header(header::COOKIE, p.cookie())
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::CONTENT_LENGTH, "2")
        .body(Body::from("{}"))
        .unwrap();
    let response = p.to_a(req).await;
    assert_eq!(response.status(), StatusCode::OK);
    let seen = p.last_seen().unwrap();
    assert!(seen.starts_with("post /api/v1/threads\n"), "{seen}");
    assert!(seen.contains("content-length: 2\n"), "{seen}");
}

/// "Switch to new version" in a relayed workspace is B's own page posting
/// B's restart route. B's engine names the tap's device, so the restart
/// counts as user-initiated there.
#[tokio::test]
async fn switch_reaches_bs_engine_as_the_tapping_device() {
    let p = paired().await;
    let req = axum::http::Request::builder()
        .method("POST")
        .uri("/dev@cloud/api/v1/restart")
        .header(header::COOKIE, p.cookie())
        .header("x-lucidos-device-id", "device-1")
        .body(Body::empty())
        .unwrap();
    let response = p.to_a(req).await;
    assert_eq!(response.status(), StatusCode::OK);
    let seen = p.last_seen().unwrap();
    assert!(seen.starts_with("post /api/v1/restart\n"), "{seen}");
    let device = format!("x-lucidos-device-id: {}:device-1\n", p.pairing_id).to_lowercase();
    assert!(seen.contains(&device), "{seen}");
    assert_eq!(seen.matches("x-lucidos-device-id:").count(), 1, "{seen}");
}

#[tokio::test]
async fn the_bare_route_redirects_to_its_trailing_slash() {
    let p = paired().await;
    let response = p.device_get("/dev@cloud", &[]).await;
    assert_eq!(response.status(), StatusCode::TEMPORARY_REDIRECT);
    assert_eq!(response.headers()[header::LOCATION], "/dev@cloud/");
}

#[tokio::test]
async fn an_unknown_remote_gateway_is_a_404_and_reaches_nothing() {
    let p = paired().await;
    let response = p
        .device_get("/dev@elsewhere/api/v1/threads/list", &[])
        .await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert!(p.last_seen().is_none());
}

#[tokio::test]
async fn an_unreachable_remote_gateway_sends_a_page_load_to_the_list() {
    let p = paired().await;
    let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = format!("https://localhost:{}", closed.local_addr().unwrap().port());
    drop(closed);
    p.a.remote_gateways()
        .add(RemoteGateway {
            name: "down".to_string(),
            address,
            pairing_id: p.pairing_id.clone(),
            credential: p.credential.clone(),
            gateway_protocol: 1,
            added_at: "2020-01-01T00:00:00Z".to_string(),
        })
        .unwrap();
    let page = p
        .device_get("/dev@down/", &[("sec-fetch-dest", "document")])
        .await;
    assert_eq!(page.status(), StatusCode::TEMPORARY_REDIRECT);
    assert_eq!(page.headers()[header::LOCATION], "/~/?pick");
    let call = p.device_get("/dev@down/api/v1/threads/list", &[]).await;
    assert_eq!(call.status(), StatusCode::BAD_GATEWAY);
}

#[tokio::test]
async fn a_dot_segment_never_reaches_another_gateway_call() {
    let p = paired().await;
    for path in [
        "/dev@cloud/../../~/api/gateway/v1/pairing",
        "/dev@cloud/%2e%2e/%2e%2e/~/api/gateway/v1/pairing",
    ] {
        let response = p.device_get(path, &[]).await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{path}");
    }
    assert_eq!(p.b.paired_gateways().list().len(), 1, "the pairing stands");
}

#[tokio::test]
async fn an_unpaired_caller_on_a_never_reaches_b() {
    let p = paired().await;
    let req = axum::http::Request::builder()
        .uri("/dev@cloud/api/v1/threads/list")
        .body(Body::empty())
        .unwrap();
    assert_eq!(p.to_a(req).await.status(), StatusCode::UNAUTHORIZED);
    assert!(p.last_seen().is_none());
}

#[tokio::test]
async fn a_local_workspace_of_the_same_name_keeps_its_route() {
    let p = paired().await;
    let local_seen: Seen = Arc::default();
    let (_release, held) = tokio::sync::oneshot::channel();
    let local_dir = test_engine::scratch();
    p.a.set_route_for_test(
        "dev",
        test_engine::serve(&local_dir, engine(local_seen.clone(), held)),
    );

    let response = p.device_get("/dev/api/v1/threads/list", &[]).await;
    assert_eq!(response.status(), StatusCode::OK);
    let local = local_seen.lock().unwrap().last().cloned().unwrap();
    assert!(local.contains("x-forwarded-prefix: /dev/\n"), "{local}");
    assert!(p.last_seen().is_none(), "the local route stayed local");

    p.device_get("/dev@cloud/api/v1/threads/list", &[]).await;
    assert!(p.last_seen().is_some(), "the remote route went to B");
    assert_eq!(local_seen.lock().unwrap().len(), 1);
}

// ── B's side: only paired gateways, revocation, one-way ─────────────────────

/// A request straight to B's relay call, as some caller might send it.
async fn to_b(p: &Paired, path: &str, headers: HeaderMap) -> reqwest::StatusCode {
    let client = reqwest::Client::builder()
        .danger_accept_invalid_certs(true)
        .build()
        .unwrap();
    client
        .get(format!(
            "{}{GATEWAY_API_PREFIX}{RELAY_PATH}{path}",
            p.b_address
        ))
        .headers(headers)
        .send()
        .await
        .unwrap()
        .status()
}

fn credential_and_claim(credential: &str, claim: &RelayClaim, key: &str) -> HeaderMap {
    let mut headers = signed(claim, key);
    headers.insert(HEADER_GATEWAY_CREDENTIAL, credential.parse().unwrap());
    headers
}

fn fresh(device: Option<AuthenticatedDevice>) -> RelayClaim {
    claim("/dev@cloud/", chrono::Utc::now().timestamp(), device)
}

#[tokio::test]
async fn b_refuses_a_relay_without_a_current_pairings_credential() {
    let p = paired().await;
    let path = "/dev/api/v1/threads/list";
    let good = fresh(Some(device("device-1")));
    assert_eq!(to_b(&p, path, signed(&good, &p.credential)).await, 401);
    let wrong = credential_and_claim("wrong-credential", &good, "wrong-credential");
    assert_eq!(to_b(&p, path, wrong).await, 401);
    assert!(p.last_seen().is_none());
    let right = credential_and_claim(&p.credential, &good, &p.credential);
    assert_eq!(to_b(&p, path, right).await, 200);
}

#[tokio::test]
async fn b_refuses_a_forged_or_stale_claim_under_a_good_credential() {
    let p = paired().await;
    let path = "/dev/api/v1/threads/list";
    let forged = credential_and_claim(&p.credential, &fresh(None), "not-the-credential");
    assert_eq!(to_b(&p, path, forged).await, 401);
    let old = claim("/dev@cloud/", chrono::Utc::now().timestamp() - 3600, None);
    let stale = credential_and_claim(&p.credential, &old, &p.credential);
    assert_eq!(to_b(&p, path, stale).await, 401);
    assert!(p.last_seen().is_none());
}

#[tokio::test]
async fn unpair_on_b_refuses_the_very_next_relayed_request() {
    let p = paired().await;
    let path = "/dev@cloud/api/v1/threads/list";
    assert_eq!(p.device_get(path, &[]).await.status(), StatusCode::OK);
    assert!(p.b.paired_gateways().unpair(&p.pairing_id).unwrap());
    p.seen.lock().unwrap().clear();
    assert_eq!(
        p.device_get(path, &[]).await.status(),
        StatusCode::UNAUTHORIZED
    );
    assert!(p.last_seen().is_none());
}

#[tokio::test]
async fn a_claim_with_no_device_reaches_only_public_app_assets() {
    let p = paired().await;
    let anonymous = credential_and_claim(&p.credential, &fresh(None), &p.credential);
    assert_eq!(
        to_b(&p, "/dev/api/v1/threads/list", anonymous.clone()).await,
        401
    );
    assert!(p.last_seen().is_none());
    assert_eq!(
        to_b(&p, "/dev/api/v1/fonts/inter.woff2", anonymous).await,
        200
    );
    assert!(p
        .last_seen()
        .unwrap()
        .starts_with("get /api/v1/fonts/inter.woff2\n"));
}

#[tokio::test]
async fn the_relay_is_one_way_a_credential_opens_nothing_on_its_holder() {
    let p = paired().await;
    // A holds the credential. Presented to A, it is no device and no gateway.
    for path in [
        "/dev@cloud/api/v1/threads/list",
        "/~/api/gateway/v1/pairing",
        "/~/api/gateway/v1/relay/dev/api/v1/threads/list",
        "/~/api/v1/remote-gateways",
    ] {
        let mut req = axum::http::Request::builder()
            .uri(path)
            .body(Body::empty())
            .unwrap();
        *req.headers_mut() = credential_and_claim(&p.credential, &fresh(None), &p.credential);
        assert_eq!(
            p.to_a(req).await.status(),
            StatusCode::UNAUTHORIZED,
            "{path}"
        );
    }
    assert!(p.last_seen().is_none());
}

#[test]
fn a_remote_frame_capability_is_passed_on_by_shape_alone() {
    let req = |method: &str, path: &str| {
        axum::http::Request::builder()
            .method(method)
            .uri(path)
            .body(Body::empty())
            .unwrap()
    };
    let token = "AAAA";
    let cap = format!("/dev@cloud/~cap/{token}/api/v1/apps/x/index.html");
    assert!(carries_a_remote_frame_capability(&req("GET", &cap)));
    assert!(!carries_a_remote_frame_capability(&req("POST", &cap)));
    let local = format!("/dev/~cap/{token}/api/v1/apps/x/index.html");
    assert!(!carries_a_remote_frame_capability(&req("GET", &local)));
    let dotted = format!("/dev@cloud/~cap/{token}/../../~/api/v1/remote-gateways");
    assert!(!carries_a_remote_frame_capability(&req("GET", &dotted)));
}
