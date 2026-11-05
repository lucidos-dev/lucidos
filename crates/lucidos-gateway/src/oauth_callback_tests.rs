use super::*;
use crate::engine_hop::test_engine;
use axum::routing::get;
use std::sync::{Arc, Mutex};
use tower::ServiceExt as _;

const LOOPBACK_PEER: &str = "127.0.0.1:50000";
const LAN_PEER: &str = "192.168.1.20:50000";

fn peer(remote: &str) -> Peer {
    Peer {
        remote: remote.parse().unwrap(),
        local: bind_addrs()[0],
        public_listener: false,
    }
}

fn callback_request(uri: &str, from: Option<Peer>) -> Request<Body> {
    let mut req = Request::get(uri).body(Body::empty()).unwrap();
    if let Some(from) = from {
        req.extensions_mut().insert(from);
    }
    req
}

/// What a stand-in engine saw of each request: its query and headers.
type Seen = Arc<Mutex<Vec<(String, HeaderMap)>>>;

/// A stand-in engine answering the delivery route with `status` and `page`.
fn engine(status: StatusCode, page: &'static str) -> (tempfile::TempDir, EngineSeen) {
    let dir = test_engine::scratch();
    let seen: Seen = Arc::default();
    let sink = seen.clone();
    let app = Router::new().route(
        &format!("/api/v1{OAUTH_DELIVERY_ROUTE}"),
        get(move |headers: HeaderMap, RawQuery(q): RawQuery| {
            let sink = sink.clone();
            async move {
                sink.lock().unwrap().push((q.unwrap_or_default(), headers));
                (status, page)
            }
        }),
    );
    let address = test_engine::serve(&dir, app);
    (dir, EngineSeen { address, seen })
}

struct EngineSeen {
    address: crate::engine_hop::EngineAddress,
    seen: Seen,
}

/// A stand-in peer gateway on a loopback port: the health probe, and the
/// hand-off route answering `status` with `page`.
async fn peer_gateway(status: StatusCode, page: &'static str) -> (u16, Arc<Mutex<Vec<String>>>) {
    fake_peer(status, page, &[(HANDOFF_ANSWERED_HEADER, "1")]).await
}

/// A peer gateway from before the hand-off route: its picker shell answers
/// the path with a 200.
async fn old_peer_gateway() -> (u16, Arc<Mutex<Vec<String>>>) {
    fake_peer(StatusCode::OK, "<!-- picker shell -->", &[]).await
}

async fn fake_peer(
    status: StatusCode,
    page: &'static str,
    headers: &'static [(&'static str, &'static str)],
) -> (u16, Arc<Mutex<Vec<String>>>) {
    let headers: Vec<_> = headers.to_vec();
    let seen: Arc<Mutex<Vec<String>>> = Arc::default();
    let sink = seen.clone();
    let app = Router::new()
        .route(
            crate::peers::GATEWAY_HEALTH_PATH,
            get(|| async { axum::Json(serde_json::json!({ "role": "gateway" })) }),
        )
        .route(
            HANDOFF_PATH,
            get(move |RawQuery(q): RawQuery| {
                let sink = sink.clone();
                let mut response = (status, page).into_response();
                for (name, value) in &headers {
                    response.headers_mut().insert(*name, value.parse().unwrap());
                }
                async move {
                    sink.lock().unwrap().push(q.unwrap_or_default());
                    response
                }
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    (port, seen)
}

#[test]
fn only_the_four_callback_fields_are_forwarded() {
    let query = forwarded_query(
        "code=4%2F0Ab&state=%2Fdev%2F~n0nce&scope=email&error=&\
         error_description=a+b&code_verifier=leak&session=cookie&code=second",
    );
    let url = reqwest::Url::parse(&format!("http://x/?{query}")).unwrap();
    let pairs: Vec<(String, String)> = url.query_pairs().into_owned().collect();
    assert_eq!(
        pairs,
        [
            ("code".into(), "4/0Ab".into()),
            ("state".into(), "/dev/~n0nce".into()),
            ("error".into(), String::new()),
            ("error_description".into(), "a b".into()),
        ]
    );
}

#[test]
fn a_forwarded_code_crosses_byte_for_byte() {
    for code in ["a+b", "a%2Bb", "a%20b", "4/0Ab~x"] {
        let raw = format!("x=1&code={code}&state=%2Fdev%2F~n");
        assert_eq!(
            forwarded_query(&raw),
            format!("code={code}&state=%2Fdev%2F~n")
        );
    }
    // A key that only starts with a field's name is not that field.
    assert_eq!(forwarded_query("codex=leak&code=c"), "code=c");
}

#[test]
fn a_state_routes_to_a_local_or_a_remote_workspace() {
    assert_eq!(state_route("/dev/~n0nce"), Some(StateRoute::Local("dev")));
    assert_eq!(
        state_route("/my-ws/~n0nce"),
        Some(StateRoute::Local("my-ws"))
    );
    assert_eq!(
        state_route("/dev@cloud/~n0nce"),
        Some(StateRoute::Remote {
            slug: "dev",
            name: "cloud"
        })
    );
    for state in [
        "n0nce",
        "/../~n0nce",
        "/dev/sub/~n0nce",
        "dev~n0nce",
        "/~/~n0nce",
        "/dev@/~n0nce",
        "/@cloud/~n0nce",
        "/dev@cloud@x/~n0nce",
    ] {
        assert_eq!(state_route(state), None, "{state}");
    }
}

#[test]
fn only_a_direct_caller_on_this_machine_is_from_this_machine() {
    let empty = HeaderMap::new();
    assert!(from_this_machine(Some(&peer(LOOPBACK_PEER)), &empty));
    assert!(from_this_machine(Some(&peer("[::1]:50000")), &empty));
    assert!(!from_this_machine(Some(&peer(LAN_PEER)), &empty));
    assert!(!from_this_machine(None, &empty));

    // A browser here that opened this machine's own tailnet or LAN address.
    let tailnet: SocketAddr = "100.64.0.5:8443".parse().unwrap();
    let own_address = Peer {
        remote: "100.64.0.5:50000".parse().unwrap(),
        local: tailnet,
        public_listener: false,
    };
    assert!(from_this_machine(Some(&own_address), &empty));
    let other_tailnet_host = Peer {
        remote: "100.64.0.9:50000".parse().unwrap(),
        ..own_address
    };
    assert!(!from_this_machine(Some(&other_tailnet_host), &empty));

    let mut public = peer(LOOPBACK_PEER);
    public.public_listener = true;
    assert!(!from_this_machine(Some(&public), &empty));

    // A tunnel on this machine connects over loopback too.
    let mut tunnelled = HeaderMap::new();
    tunnelled.insert("x-forwarded-for", "203.0.113.9".parse().unwrap());
    assert!(!from_this_machine(Some(&peer(LOOPBACK_PEER)), &tunnelled));
}

#[tokio::test]
async fn a_callback_from_off_this_machine_reaches_no_engine() {
    let state = GatewayState::for_tests();
    let dir = test_engine::scratch();
    let (address, recorded) = test_engine::record_one(&dir);
    state.set_route_for_test("dev", address);

    for from in [Some(peer(LAN_PEER)), None] {
        let response = router(state.clone())
            .oneshot(callback_request(
                "/oauth/callback?code=c&state=%2Fdev%2F~n",
                from,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }
    drop(state);
    // Nothing connected, so the recorder still waits. Nothing was sent.
    assert!(!recorded.is_finished());
    recorded.abort();
}

#[tokio::test]
async fn the_callback_port_answers_nothing_else() {
    let state = GatewayState::for_tests();
    for path in [
        "/",
        "/~/api/v1/health",
        "/~/api/v1/control/workspaces",
        HANDOFF_PATH,
        "/dev/api/v1/threads/list",
    ] {
        let response = router(state.clone())
            .oneshot(callback_request(path, Some(peer(LOOPBACK_PEER))))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "{path}");
    }
}

#[tokio::test]
async fn a_local_callback_reaches_its_engine_with_the_four_fields_only() {
    let state = GatewayState::for_tests();
    let (_dir, engine) = engine(StatusCode::OK, "<p>done</p>");
    state.set_route_for_test("dev", engine.address);

    let mut req = callback_request(
        "/oauth/callback?code=c0de&state=%2Fdev%2F~n0nce&code_verifier=leak",
        Some(peer(LOOPBACK_PEER)),
    );
    req.headers_mut()
        .insert("cookie", "lucidos_device=secret".parse().unwrap());
    req.headers_mut()
        .insert(crate::auth::HEADER_LOCAL_TOKEN, "forged".parse().unwrap());
    let response = router(state).oneshot(req).await.unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let page = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(&page[..], b"<p>done</p>");

    let seen = engine.seen.lock().unwrap();
    let (query, headers) = &seen[0];
    assert_eq!(query, "code=c0de&state=%2Fdev%2F~n0nce");
    assert!(headers.get("cookie").is_none());
    assert!(headers.get("x-forwarded-prefix").is_none());
    assert_eq!(
        headers.get(crate::auth::HEADER_LOCAL_TOKEN).unwrap(),
        "test-local-token"
    );
}

#[tokio::test]
async fn an_unclaimed_callback_stops_at_the_first_peer_that_claims_it() {
    let state = GatewayState::for_tests();
    let (_dir, engine) = engine(StatusCode::NOT_FOUND, "engine: not mine");
    state.set_route_for_test("dev", engine.address);
    let (declines, declined) = peer_gateway(StatusCode::NOT_FOUND, "peer: not mine").await;
    let (old, asked_old) = old_peer_gateway().await;
    let (claims, claimed) = peer_gateway(StatusCode::OK, "peer: done").await;
    let (later, never) = peer_gateway(StatusCode::OK, "late: done").await;

    let query = forwarded_query("code=c0de&state=%2Fdev%2F~n0nce&extra=x");
    let (status, page) =
        route_callback(&state, &query, || vec![declines, old, claims, later]).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(page, "peer: done");
    assert_eq!(engine.seen.lock().unwrap().len(), 1);
    assert_eq!(*declined.lock().unwrap(), std::slice::from_ref(&query));
    assert_eq!(asked_old.lock().unwrap().len(), 1, "an old peer is asked");
    assert_eq!(*claimed.lock().unwrap(), [query]);
    assert!(
        never.lock().unwrap().is_empty(),
        "a code goes to one engine"
    );
}

#[tokio::test]
async fn a_callback_nobody_claims_shows_the_gateway_failure_page() {
    let state = GatewayState::for_tests();
    let (declines, _) = peer_gateway(StatusCode::NOT_FOUND, "peer: not mine").await;
    let query = forwarded_query("code=c&state=%2Fnobody%2F~n");
    let (status, page) = route_callback(&state, &query, || vec![declines]).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(page, failure_page());

    let (status, page) = route_callback(&state, &query, Vec::new).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(page, failure_page());
}

#[tokio::test]
async fn an_engine_that_refuses_before_the_route_runs_hands_on_to_peers() {
    for refusal in [StatusCode::FORBIDDEN, StatusCode::SERVICE_UNAVAILABLE] {
        let state = GatewayState::for_tests();
        let (_dir, engine) = engine(refusal, "engine: restarting");
        state.set_route_for_test("dev", engine.address);
        let (claims, claimed) = peer_gateway(StatusCode::OK, "peer: done").await;

        let query = forwarded_query("code=c0de&state=%2Fdev%2F~n0nce");
        let (status, page) = route_callback(&state, &query, || vec![claims]).await;

        assert_eq!(status, StatusCode::OK, "{refusal}");
        assert_eq!(page, "peer: done");
        assert_eq!(claimed.lock().unwrap().len(), 1);
    }
}

#[tokio::test]
async fn a_claiming_engine_ends_the_route_before_any_peer() {
    let state = GatewayState::for_tests();
    let (_dir, engine) = engine(StatusCode::BAD_REQUEST, "engine: provider error");
    state.set_route_for_test("dev", engine.address);
    let query = forwarded_query("error=access_denied&state=%2Fdev%2F~n");
    let (status, page) = route_callback(&state, &query, || {
        panic!("a claimed callback must not look for peers")
    })
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(page, "engine: provider error");
}

/// The hand-off route on the main listener: loopback only, own engines only.
#[tokio::test]
async fn a_handoff_is_loopback_only_and_never_handed_on() {
    let state = GatewayState::for_tests();
    let (_dir, engine) = engine(StatusCode::OK, "engine: done");
    state.set_route_for_test("dev", engine.address);
    let gateway = crate::server::gateway_router(state);
    let uri = format!("{HANDOFF_PATH}?code=c&state=%2Fdev%2F~n");

    let refused = gateway
        .clone()
        .oneshot(callback_request(&uri, Some(peer(LAN_PEER))))
        .await
        .unwrap();
    assert_eq!(refused.status(), StatusCode::NOT_FOUND);
    assert!(engine.seen.lock().unwrap().is_empty());

    let accepted = gateway
        .clone()
        .oneshot(callback_request(&uri, Some(peer(LOOPBACK_PEER))))
        .await
        .unwrap();
    assert_eq!(accepted.status(), StatusCode::OK);
    assert_eq!(engine.seen.lock().unwrap().len(), 1);

    let unclaimed = gateway
        .oneshot(callback_request(
            &format!("{HANDOFF_PATH}?code=c&state=%2Fnobody%2F~n"),
            Some(peer(LOOPBACK_PEER)),
        ))
        .await
        .unwrap();
    assert_eq!(unclaimed.status(), StatusCode::NOT_FOUND);
}

// ── A remote workspace: the callback crosses the relay ─────────────────────

#[tokio::test]
async fn a_remote_callback_crosses_the_relay_device_less_with_the_four_fields_only() {
    let p = crate::relay::tests::paired().await;
    let mut req = callback_request(
        "/oauth/callback?code=c0de&state=%2Fdev%40cloud%2F~n0nce&code_verifier=leak",
        Some(peer(LOOPBACK_PEER)),
    );
    req.headers_mut()
        .insert("cookie", "lucidos_device=secret".parse().unwrap());
    let response = router(p.a.clone()).oneshot(req).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    // B's stand-in engine lowercases what it saw.
    let seen = p.seen.lock().unwrap().last().cloned().unwrap();
    let (target, headers) = seen.split_once('\n').unwrap();
    assert_eq!(
        target,
        "get /api/v1/internal/oauth/callback?code=c0de&state=%2fdev%40cloud%2f~n0nce"
    );
    let token = crate::auth::HEADER_LOCAL_TOKEN.to_lowercase();
    assert!(headers.contains(&format!("{token}: test-local-token")));
    for absent in ["cookie", "x-forwarded-prefix", "x-lucidos-device-id"] {
        assert!(
            !headers.lines().any(|l| l.starts_with(absent)),
            "{absent} reached B's engine"
        );
    }
}

#[tokio::test]
async fn a_callback_for_an_unknown_remote_gateway_is_unclaimed() {
    let p = crate::relay::tests::paired().await;
    let query = forwarded_query("code=c&state=%2Fdev%40nowhere%2F~n");
    let (status, page) = route_callback(&p.a, &query, Vec::new).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(page, failure_page());
    assert!(p.seen.lock().unwrap().is_empty());
}

#[test]
fn the_callback_port_binds_both_loopback_families() {
    let [v4, v6] = bind_addrs();
    assert_eq!(v4.to_string(), "127.0.0.1:14981");
    assert_eq!(v6.to_string(), "[::1]:14981");
}

// ── Locality and the web redirect route ────────────────────────────────────

/// A gateway with one paired device and a stand-in `dev` engine that records
/// every request's headers.
fn workspace_gateway() -> (GatewayState, tempfile::TempDir, Seen) {
    let state = GatewayState::for_tests();
    state
        .write_paired_devices_for_test(|paired| {
            paired.devices.push(crate::auth::PairedDevice {
                id: "device-1".into(),
                label: "My iPhone".into(),
                credential_digest: crate::auth::digest("cred-abc"),
                paired_at: "2020-01-01T00:00:00Z".into(),
                last_seen_at: None,
            });
        })
        .unwrap();
    let dir = test_engine::scratch();
    let seen: Seen = Arc::default();
    let sink = seen.clone();
    let app = Router::new().fallback(move |headers: HeaderMap, RawQuery(q): RawQuery| {
        let sink = sink.clone();
        async move {
            sink.lock().unwrap().push((q.unwrap_or_default(), headers));
            "ok"
        }
    });
    state.set_route_for_test("dev", test_engine::serve(&dir, app));
    (state, dir, seen)
}

async fn device_request(state: &GatewayState, from: Option<Peer>, extra: &[(&'static str, &str)]) {
    let mut req = callback_request("/dev/api/v1/threads/list", from);
    let cookie = format!("{}=cred-abc", state.device_cookie_name());
    req.headers_mut().insert("cookie", cookie.parse().unwrap());
    for (name, value) in extra {
        req.headers_mut().insert(*name, value.parse().unwrap());
    }
    let response = crate::server::gateway_router(state.clone())
        .oneshot(req)
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}

fn last_stamp(seen: &Seen) -> Option<String> {
    let seen = seen.lock().unwrap();
    let (_, headers) = seen.last().expect("the engine was reached");
    assert!(headers.get_all(HEADER_LOCAL_BROWSER).iter().count() <= 1);
    headers
        .get(HEADER_LOCAL_BROWSER)
        .map(|v| v.to_str().unwrap().to_string())
}

#[tokio::test]
async fn only_a_browser_on_this_machine_is_stamped_local() {
    let (state, _dir, seen) = workspace_gateway();
    device_request(&state, Some(peer(LOOPBACK_PEER)), &[]).await;
    assert_eq!(last_stamp(&seen).as_deref(), Some("1"));

    device_request(&state, Some(peer(LAN_PEER)), &[]).await;
    assert_eq!(last_stamp(&seen), None);

    // A tunnel on this machine connects over loopback, and says so.
    device_request(
        &state,
        Some(peer(LOOPBACK_PEER)),
        &[("x-forwarded-for", "203.0.113.9")],
    )
    .await;
    assert_eq!(last_stamp(&seen), None);
}

#[tokio::test]
async fn a_client_sent_local_browser_stamp_never_reaches_the_engine() {
    let (state, _dir, seen) = workspace_gateway();
    device_request(&state, Some(peer(LAN_PEER)), &[(HEADER_LOCAL_BROWSER, "1")]).await;
    assert_eq!(last_stamp(&seen), None);
    device_request(&state, None, &[(HEADER_LOCAL_BROWSER, "1")]).await;
    assert_eq!(last_stamp(&seen), None);
}

#[tokio::test]
async fn a_web_callback_on_the_public_origin_reaches_its_engine_without_a_credential() {
    let state = GatewayState::for_tests();
    let (_dir, engine) = engine(StatusCode::OK, "<p>done</p>");
    state.set_route_for_test("dev", engine.address);

    let req = callback_request(
        "/~/oauth/callback?code=c0de&state=%2Fdev%2F~n0nce&code_verifier=leak",
        Some(peer(LAN_PEER)),
    );
    let response = crate::server::gateway_router(state)
        .oneshot(req)
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let seen = engine.seen.lock().unwrap();
    assert_eq!(seen[0].0, "code=c0de&state=%2Fdev%2F~n0nce");
}

#[tokio::test]
async fn a_web_callback_is_never_handed_to_a_peer_install() {
    let state = GatewayState::for_tests();
    let response = crate::server::gateway_router(state)
        .oneshot(callback_request(
            "/~/oauth/callback?code=c&state=%2Fother%2F~n",
            Some(peer(LAN_PEER)),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_web_callback_does_not_tell_a_workspace_that_exists_from_one_that_does_not() {
    let state = GatewayState::for_tests();
    let (_dir, engine) = engine(StatusCode::NOT_FOUND, "engine: no such flow");
    state.set_route_for_test("dev", engine.address);

    let mut answers = Vec::new();
    for slug in ["dev", "nobody"] {
        let response = crate::server::gateway_router(state.clone())
            .oneshot(callback_request(
                &format!("/~/oauth/callback?code=c&state=%2F{slug}%2F~guess"),
                Some(peer(LAN_PEER)),
            ))
            .await
            .unwrap();
        let status = response.status();
        let page = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        answers.push((status, page));
    }
    assert_eq!(engine.seen.lock().unwrap().len(), 1, "the engine was asked");
    assert_eq!(answers[0], answers[1]);
    assert_eq!(answers[0].0, StatusCode::NOT_FOUND);
}
