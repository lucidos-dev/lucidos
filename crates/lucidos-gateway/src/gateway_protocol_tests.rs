use super::*;
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
    let mut builder = HttpRequest::builder().method(method).uri(path);
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
    let code = state.paired_gateways().mint_code(None).unwrap();
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
        .mint_code(Some("cloud box".to_string()))
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
    let device_code = state.mint_pairing_code(None).unwrap();
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
    let gateway_code = state.paired_gateways().mint_code(None).unwrap();
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
    let code = state.paired_gateways().mint_code(None).unwrap();
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
    let device = match state.redeem_pairing_code(&state.mint_pairing_code(None).unwrap(), None) {
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
    assert!(auth::valid_pairing_code(body["code"].as_str().unwrap()).is_some());
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
    let code = store.mint_code(None).unwrap();
    {
        let _locked = ReadOnlyDir::new(dir.path());
        assert!(store.redeem(&code, Some("A")).is_err());
    }
    assert!(store.list().is_empty());
}
