use super::*;
use crate::pairing;
use axum::body::Body;
use axum::http::Request as HttpRequest;
use serde_json::{json, Value};
use tower::ServiceExt as _;

/// A gateway with a store of its own, so parallel tests never share a file.
fn gateway() -> (GatewayState, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let state = GatewayState::for_tests_with_static_dir(Some(dir.path().to_path_buf()));
    (state, dir)
}

/// Send one request through the router the process serves.
async fn call(
    state: &GatewayState,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: Option<Value>,
) -> (StatusCode, Value) {
    call_from(state, None, method, path, headers, body).await
}

/// [`call`], as if the acceptor had stamped `peer` on the request.
async fn call_from(
    state: &GatewayState,
    peer: Option<pairing::Peer>,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut builder = HttpRequest::builder().method(method).uri(path);
    if let Some(peer) = peer {
        builder = builder.extension(peer);
    }
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    let body = match body {
        Some(json) => {
            builder = builder.header("content-type", "application/json");
            Body::from(json.to_string())
        }
        None => Body::empty(),
    };
    let response = crate::server::gateway_router(state.clone())
        .oneshot(builder.body(body).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

const LOCAL: (&str, &str) = (auth::HEADER_LOCAL_TOKEN, "test-local-token");

fn pair_path() -> String {
    format!("{GATEWAY_API_PREFIX}{PAIR_PATH}")
}

fn pairing_path() -> String {
    format!("{GATEWAY_API_PREFIX}{PAIRING_PATH}")
}

/// Mint a gateway code on `state` and redeem it as a remote gateway would.
async fn pair_a_gateway(state: &GatewayState) -> PairAnswer {
    let code = state.paired_gateways().mint_secret(None).unwrap();
    let (status, body) = call(
        state,
        "POST",
        &pair_path(),
        &[],
        Some(json!({ "code": code, "label": "Lucidos on a" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    serde_json::from_value(body).unwrap()
}

#[test]
fn the_namespace_is_matched_on_whole_segments() {
    assert!(is_gateway_api_path("/~/api/gateway"));
    assert!(is_gateway_api_path("/~/api/gateway/v1/pair"));
    assert!(is_gateway_api_path("/~/api/gateway/v2/anything"));
    assert!(!is_gateway_api_path("/~/api/gatewayx/v1/pair"));
    assert!(!is_gateway_api_path("/~/api/v1/auth/pair"));
    assert!(!is_gateway_api_path("/dev/~/api/gateway/v1/pair"));
}

#[tokio::test]
async fn health_names_the_gateway_protocol() {
    let (state, _dir) = gateway();
    let (status, body) = call(&state, "GET", crate::peers::GATEWAY_HEALTH_PATH, &[], None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["gateway_protocol"], json!(GATEWAY_PROTOCOL));
}

#[tokio::test]
async fn a_gateway_code_pairs_a_gateway_that_can_then_name_its_pairing() {
    let (state, _dir) = gateway();
    let answer = pair_a_gateway(&state).await;
    assert_eq!(answer.gateway_protocol, GATEWAY_PROTOCOL);

    let (status, body) = call(
        &state,
        "GET",
        &pairing_path(),
        &[(HEADER_GATEWAY_CREDENTIAL, &answer.credential)],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["pairing_id"], json!(answer.pairing_id));
    assert_eq!(body["label"], json!("Lucidos on a"));
}

#[tokio::test]
async fn the_label_on_the_code_wins_over_the_one_the_gateway_offers() {
    let (state, _dir) = gateway();
    let code = state
        .paired_gateways()
        .mint_secret(Some("cloud box".to_string()))
        .unwrap();
    let (status, _) = call(
        &state,
        "POST",
        &pair_path(),
        &[],
        Some(json!({ "code": code, "label": "Lucidos on a" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(state.paired_gateways().list()[0].label, "cloud box");
}

#[tokio::test]
async fn a_device_code_never_pairs_a_gateway() {
    let (state, _dir) = gateway();
    let device_code = state.mint_pairing(None).unwrap().secret;
    let (status, _) = call(
        &state,
        "POST",
        &pair_path(),
        &[],
        Some(json!({ "code": device_code })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(state.paired_gateways().list().is_empty());

    // Still a good device code: the gateway path never spent it.
    let (status, _) = call(
        &state,
        "POST",
        "/~/api/v1/auth/pair",
        &[],
        Some(json!({ "code": device_code })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn a_gateway_code_never_pairs_a_device() {
    let (state, _dir) = gateway();
    let gateway_code = state.paired_gateways().mint_secret(None).unwrap();
    let (status, _) = call(
        &state,
        "POST",
        "/~/api/v1/auth/pair",
        &[],
        Some(json!({ "code": gateway_code })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(state.paired_devices().devices.is_empty());

    let (status, _) = call(
        &state,
        "POST",
        &pair_path(),
        &[],
        Some(json!({ "code": gateway_code })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "the device path never spent it");
}

#[tokio::test]
async fn a_gateway_code_works_once() {
    let (state, _dir) = gateway();
    let code = state.paired_gateways().mint_secret(None).unwrap();
    let path = pair_path();
    let pair = || call(&state, "POST", &path, &[], Some(json!({ "code": code })));
    assert_eq!(pair().await.0, StatusCode::OK);
    assert_eq!(pair().await.0, StatusCode::BAD_REQUEST);
    assert_eq!(state.paired_gateways().list().len(), 1);
}

#[tokio::test]
async fn gateway_calls_refuse_anything_but_a_current_gateway_credential() {
    let (state, _dir) = gateway();
    pair_a_gateway(&state).await;
    let device = match state.redeem_pairing(
        &state.mint_pairing(None).unwrap().secret,
        crate::pairing::PairingChannel::Public,
        None,
    ) {
        Ok(crate::server::PairingOutcome::Paired(credential)) => credential,
        _ => panic!("the device did not pair"),
    };
    let device_cookie = format!("{}={device}", state.device_cookie_name());

    for headers in [
        vec![],
        vec![(HEADER_GATEWAY_CREDENTIAL, "not-a-credential")],
        vec![("cookie", device_cookie.as_str())],
        vec![(HEADER_GATEWAY_CREDENTIAL, device.as_str())],
        vec![LOCAL],
    ] {
        let (status, _) = call(&state, "GET", &pairing_path(), &headers, None).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{headers:?}");
    }
}

#[tokio::test]
async fn a_gateway_credential_never_authorizes_a_device_route() {
    let (state, _dir) = gateway();
    let answer = pair_a_gateway(&state).await;
    let as_cookie = format!("{}={}", state.device_cookie_name(), answer.credential);
    for path in [
        "/~/api/v1/auth/devices",
        "/~/api/v1/auth/gateways",
        "/~/api/v1/remote-gateways",
        "/~/api/v1/control/workspaces",
    ] {
        for header in [
            (HEADER_GATEWAY_CREDENTIAL, answer.credential.as_str()),
            ("cookie", as_cookie.as_str()),
        ] {
            let (status, _) = call(&state, "GET", path, &[header], None).await;
            assert_eq!(status, StatusCode::UNAUTHORIZED, "{path} {}", header.0);
        }
    }
}

#[tokio::test]
async fn unpair_on_b_refuses_the_very_next_call() {
    let (state, _dir) = gateway();
    let answer = pair_a_gateway(&state).await;
    let (status, _) = call(
        &state,
        "DELETE",
        &format!("/~/api/v1/auth/gateways/{}", answer.pairing_id),
        &[LOCAL],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let (status, _) = call(
        &state,
        "GET",
        &pairing_path(),
        &[(HEADER_GATEWAY_CREDENTIAL, &answer.credential)],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn a_gateway_may_end_its_own_pairing() {
    let (state, _dir) = gateway();
    let answer = pair_a_gateway(&state).await;
    let (status, _) = call(
        &state,
        "DELETE",
        &pairing_path(),
        &[(HEADER_GATEWAY_CREDENTIAL, &answer.credential)],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert!(state.paired_gateways().list().is_empty());
}

#[tokio::test]
async fn b_lists_its_paired_gateways_without_credentials() {
    let (state, _dir) = gateway();
    let answer = pair_a_gateway(&state).await;
    let (status, body) = call(&state, "GET", "/~/api/v1/auth/gateways", &[LOCAL], None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body[0]["id"], json!(answer.pairing_id));
    assert!(body[0].get("credential_digest").is_none());
    assert!(!body.to_string().contains(&answer.credential));
}

#[tokio::test]
async fn minting_a_gateway_code_needs_a_paired_caller() {
    let (state, _dir) = gateway();
    let (status, _) = call(
        &state,
        "POST",
        "/~/api/v1/auth/gateway-pairing-code",
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let (status, body) = call(
        &state,
        "POST",
        "/~/api/v1/auth/gateway-pairing-code",
        &[LOCAL],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(crate::pairing::valid_pairing_secret(
        body["code"].as_str().unwrap(),
        crate::pairing::PairingKind::Gateway
    )
    .is_some());
}

#[tokio::test]
async fn the_store_keeps_a_digest_never_the_credential() {
    let (state, dir) = gateway();
    let answer = pair_a_gateway(&state).await;
    let file = std::fs::read_to_string(paired_gateways_path(&dir.path().join("state"))).unwrap();
    assert!(!file.contains(&answer.credential));
    assert!(file.contains(&auth::digest(&answer.credential)));
}

#[tokio::test]
async fn an_unrouted_gateway_call_is_a_404_not_the_picker() {
    let (state, _dir) = gateway();
    for path in [
        "/~/api/gateway/v1/nope",
        "/~/api/gateway/v2/pairing",
        "/~/api/gateway",
    ] {
        let (status, body) = call(&state, "GET", path, &[], None).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{path}");
        assert!(body["error"].is_string(), "{path} answered {body}");
    }
}

/// A redemption whose save fails pairs nobody. The caller never got the
/// credential, so a pairing kept in memory could only confuse the list.
#[test]
fn a_failed_save_records_no_pairing() {
    use crate::remote_gateways::tests::ReadOnlyDir;
    let dir = tempfile::tempdir().unwrap();
    let store = PairedGatewayStore::load(paired_gateways_path(dir.path())).unwrap();
    let code = store.mint_secret(None).unwrap();
    {
        let _locked = ReadOnlyDir::new(dir.path());
        assert!(store.redeem(&code, Some("A")).is_err());
    }
    assert!(store.list().is_empty());
}

// ── Pairing secrets and channels ────────────────────────────────────────────

const DEVICE_PAIR: &str = "/~/api/v1/auth/pair";

fn peer_at(remote: &str, public_listener: bool) -> Option<pairing::Peer> {
    Some(pairing::Peer {
        remote: remote.parse().unwrap(),
        local: "127.0.0.1:5252".parse().unwrap(),
        public_listener,
    })
}

fn lan_peer(last: u8) -> Option<pairing::Peer> {
    Some(pairing::Peer {
        remote: format!("192.168.1.{last}:40000").parse().unwrap(),
        local: "192.168.1.5:5252".parse().unwrap(),
        public_listener: false,
    })
}

async fn mint_device(state: &GatewayState) -> (String, String) {
    let (status, body) = call(state, "POST", "/~/api/v1/auth/pairing-code", &[LOCAL], None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    (
        body["code"].as_str().unwrap().to_string(),
        body["secret"].as_str().unwrap().to_string(),
    )
}

fn wrong_short(code: &str) -> &'static str {
    if code == "00000000" {
        "00000001"
    } else {
        "00000000"
    }
}

/// Invariant: a request from 127.0.0.1 carrying a tunnel header is refused a
/// typed code, and so is any request on the public listener.
#[tokio::test]
async fn a_typed_code_through_a_tunnel_is_refused_and_spends_nothing() {
    let (state, _dir) = gateway();
    let (code, secret) = mint_device(&state).await;
    let loopback = peer_at("127.0.0.1:50000", false);
    for header in ["cf-connecting-ip", "x-forwarded-for", "forwarded", "cf-ray"] {
        let (status, body) = call_from(
            &state,
            loopback,
            "POST",
            DEVICE_PAIR,
            &[("host", "127.0.0.1:5252"), (header, "203.0.113.9")],
            Some(json!({ "code": code })),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{header}");
        assert_eq!(
            body["error"],
            json!(crate::auth_api::PUBLIC_SHORT_CODE_REFUSAL)
        );
    }
    let (status, _) = call_from(
        &state,
        peer_at("127.0.0.1:50000", true),
        "POST",
        DEVICE_PAIR,
        &[("host", "127.0.0.1:5282")],
        Some(json!({ "code": code })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "the public listener");
    assert!(state.paired_devices().devices.is_empty());

    let (status, _) = call_from(
        &state,
        loopback,
        "POST",
        DEVICE_PAIR,
        &[
            ("host", "lucidos.example.com"),
            ("cf-connecting-ip", "203.0.113.9"),
        ],
        Some(json!({ "code": secret })),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "the long secret pairs through a tunnel"
    );
}

/// Invariant: a raw forward to the main port, which a caller can give any
/// `Host`, is refused a typed code. This machine pairs by the secret instead.
#[tokio::test]
async fn a_typed_code_pairs_on_the_lan_and_never_from_loopback() {
    let (state, _dir) = gateway();
    let (code, _) = mint_device(&state).await;
    let (status, body) = call_from(
        &state,
        lan_peer(20),
        "POST",
        DEVICE_PAIR,
        &[("host", "192.168.1.5:5252")],
        Some(json!({ "code": code })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let (code, secret) = mint_device(&state).await;
    let loopback = peer_at("127.0.0.1:50000", false);
    let (status, body) = call_from(
        &state,
        loopback,
        "POST",
        DEVICE_PAIR,
        &[("host", "localhost:5252")],
        Some(json!({ "code": code })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(
        body["error"],
        json!(crate::auth_api::PUBLIC_SHORT_CODE_REFUSAL)
    );
    let (status, body) = call_from(
        &state,
        loopback,
        "POST",
        DEVICE_PAIR,
        &[("host", "localhost:5252")],
        Some(json!({ "code": secret })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

/// Invariant: a junk flood from X cannot block a correct typed code from Y.
#[tokio::test]
async fn one_lan_device_flooding_costs_another_nothing() {
    let (state, _dir) = gateway();
    let (code, _) = mint_device(&state).await;
    let mut throttled = false;
    for _ in 0..200 {
        let (status, _) = call_from(
            &state,
            lan_peer(66),
            "POST",
            DEVICE_PAIR,
            &[],
            Some(json!({ "code": wrong_short(&code) })),
        )
        .await;
        throttled |= status == StatusCode::TOO_MANY_REQUESTS;
    }
    assert!(throttled, "the flooding device met its own cap");
    let (status, body) = call_from(
        &state,
        lan_peer(7),
        "POST",
        DEVICE_PAIR,
        &[],
        Some(json!({ "code": code })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

/// Invariant: a flood of wrong secrets from many sources cannot stop a correct
/// long secret, for device and gateway pairing alike. The full 100,000-guess
/// run is `pairing::tests`; this drives the routes the process serves.
#[tokio::test]
async fn a_flood_through_the_routes_never_refuses_a_correct_secret() {
    let (state, _dir) = gateway();
    let (code, device_secret) = mint_device(&state).await;
    let gateway_secret = state.paired_gateways().mint_secret(None).unwrap();
    for n in 0..1_000u32 {
        let remote = format!("10.0.{}.{}:4000", n / 250, n % 250);
        let peer = Some(pairing::Peer {
            remote: remote.parse().unwrap(),
            local: "10.0.9.9:5252".parse().unwrap(),
            public_listener: n % 2 == 0,
        });
        let guess = if n % 2 == 0 {
            json!({ "code": wrong_short(&code) })
        } else {
            json!({ "code": format!("ldp_{:026}", n).replace('0', "a").replace('1', "b") })
        };
        let (status, _) = call_from(&state, peer, "POST", DEVICE_PAIR, &[], Some(guess)).await;
        assert_ne!(status, StatusCode::OK);
        let gateway_guess = json!({ "code": format!("lgp_{:026}", n).replace('0', "a") });
        let (status, _) =
            call_from(&state, peer, "POST", &pair_path(), &[], Some(gateway_guess)).await;
        assert_eq!(
            status,
            StatusCode::BAD_REQUEST,
            "never a 429 on the gateway route"
        );
    }
    let (status, _) = call(
        &state,
        "POST",
        DEVICE_PAIR,
        &[],
        Some(json!({ "code": device_secret })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "the device secret paired");
    let (status, _) = call(
        &state,
        "POST",
        &pair_path(),
        &[],
        Some(json!({ "code": gateway_secret })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "the gateway secret paired");
}

/// Invariant: with nothing pending, a flood counts nothing.
#[tokio::test]
async fn a_flood_before_any_code_is_minted_costs_the_first_code_nothing() {
    let (state, _dir) = gateway();
    for _ in 0..100 {
        let (status, _) = call_from(
            &state,
            lan_peer(9),
            "POST",
            DEVICE_PAIR,
            &[],
            Some(json!({ "code": "00000000" })),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }
    let (code, _) = mint_device(&state).await;
    let (status, body) = call_from(
        &state,
        lan_peer(9),
        "POST",
        DEVICE_PAIR,
        &[],
        Some(json!({ "code": code })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

#[tokio::test]
async fn the_qr_carries_the_long_secret_never_the_short_code() {
    let (state, _dir) = gateway();
    let (status, body) = call(
        &state,
        "POST",
        "/~/api/v1/auth/pairing-code?origin=https://mac.example.ts.net",
        &[LOCAL],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let url = body["pair_url"].as_str().unwrap();
    assert!(url.ends_with(body["secret"].as_str().unwrap()), "{url}");
    assert!(!url.contains(body["code"].as_str().unwrap()), "{url}");
}

#[test]
fn rotation_survives_reload_and_first_new_use_revokes_overlap() {
    let dir = tempfile::tempdir().unwrap();
    let path = paired_gateways_path(dir.path());
    let store = PairedGatewayStore::load(path.clone()).unwrap();
    let code = store.mint_secret(None).unwrap();
    let GatewayRedemption::Paired { credential, .. } = store.redeem(&code, None).unwrap() else {
        panic!()
    };
    let headers = |value: &str| {
        let mut headers = HeaderMap::new();
        headers.insert(HEADER_GATEWAY_CREDENTIAL, value.parse().unwrap());
        headers
    };
    let candidate = auth::mint_credential().unwrap();
    store.rotate(&headers(&credential), &candidate).unwrap();
    // Lost responses can repeat the same intent without creating another key.
    store.rotate(&headers(&credential), &candidate).unwrap();
    let store = PairedGatewayStore::load(path.clone()).unwrap();
    assert!(store.authorize(&headers(&credential)).unwrap().is_some());
    assert!(store.authorize(&headers(&candidate)).unwrap().is_some());
    let store = PairedGatewayStore::load(path).unwrap();
    assert!(store.authorize(&headers(&credential)).unwrap().is_none());
    assert!(store.authorize(&headers(&candidate)).unwrap().is_some());
    assert!(store.list()[0].last_used_at.is_some());
}

#[test]
fn rotation_expiry_revocation_and_persistence_failure_fail_closed() {
    let dir = tempfile::tempdir().unwrap();
    let path = paired_gateways_path(dir.path());
    let store = PairedGatewayStore::load(path.clone()).unwrap();
    let code = store.mint_secret(None).unwrap();
    let GatewayRedemption::Paired {
        credential,
        pairing_id,
    } = store.redeem(&code, None).unwrap()
    else {
        panic!()
    };
    let headers = |value: &str| {
        let mut headers = HeaderMap::new();
        headers.insert(HEADER_GATEWAY_CREDENTIAL, value.parse().unwrap());
        headers
    };
    let candidate = auth::mint_credential().unwrap();
    assert!(!store.rotate(&headers("wrong"), &candidate).unwrap());
    assert!(store.rotate(&headers(&credential), "short").is_err());
    assert!(store.rotate(&headers(&credential), &candidate).unwrap());
    assert!(store
        .rotate(&headers(&credential), &auth::mint_credential().unwrap())
        .is_err());
    store
        .write(|data| {
            data.gateways[0]
                .previous_credential
                .as_mut()
                .unwrap()
                .expires_at = 0
        })
        .unwrap();
    assert!(store.authorize(&headers(&credential)).unwrap().is_none());
    // Old overlap expiry cannot strand A: its durable candidate still works.
    assert!(store.authorize(&headers(&candidate)).unwrap().is_some());
    store
        .write(|data| data.gateways[0].last_used_at = None)
        .unwrap();
    let before = store.list();
    std::fs::remove_file(&path).unwrap();
    std::fs::create_dir(&path).unwrap();
    assert!(store
        .rotate(&headers(&candidate), &auth::mint_credential().unwrap())
        .is_err());
    assert!(store.authorize(&headers(&candidate)).is_err());
    assert_eq!(store.list(), before);
    std::fs::remove_dir(&path).unwrap();
    assert!(store.unpair(&pairing_id).unwrap());
    assert!(!store
        .rotate(&headers(&candidate), &auth::mint_credential().unwrap())
        .unwrap());
}

#[tokio::test]
async fn rotation_route_authenticates_without_consuming_the_overlap() {
    let (state, _dir) = gateway();
    let code = state.paired_gateways().mint_secret(None).unwrap();
    let GatewayRedemption::Paired { credential, .. } =
        state.paired_gateways().redeem(&code, None).unwrap()
    else {
        panic!()
    };
    let candidate = auth::mint_credential().unwrap();
    let path = format!("{GATEWAY_API_PREFIX}{ROTATE_PATH}");
    for _ in 0..2 {
        let (status, _) = call(
            &state,
            "POST",
            &path,
            &[(HEADER_GATEWAY_CREDENTIAL, &credential)],
            Some(json!({"candidate":candidate})),
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT);
    }
    let (status, _) = call(
        &state,
        "POST",
        &path,
        &[],
        Some(json!({"candidate":candidate})),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[test]
fn recent_gateway_use_does_not_rewrite_the_store() {
    let dir = tempfile::tempdir().unwrap();
    let path = paired_gateways_path(dir.path());
    let store = PairedGatewayStore::load(path.clone()).unwrap();
    let code = store.mint_secret(None).unwrap();
    let GatewayRedemption::Paired { credential, .. } = store.redeem(&code, None).unwrap() else {
        panic!()
    };
    let mut headers = HeaderMap::new();
    headers.insert(HEADER_GATEWAY_CREDENTIAL, credential.parse().unwrap());
    assert!(store.authorize(&headers).unwrap().is_some());
    let before = store.list();
    std::fs::remove_file(&path).unwrap();
    std::fs::create_dir(&path).unwrap();
    assert!(store.authorize(&headers).unwrap().is_some());
    assert_eq!(store.list(), before);
}
