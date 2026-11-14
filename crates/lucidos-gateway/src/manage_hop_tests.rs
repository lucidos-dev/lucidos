use super::*;
use crate::auth::{self, PairedDevice};
use crate::relay::tests::{paired, Paired};
use crate::relay::{sign, HEADER_RELAY_CLAIM, HEADER_RELAY_SIGNATURE};
use crate::remote_gateways::tests::{localhost_cert, serve_https};
use serde_json::{json, Value};
use tower::ServiceExt as _;

const NOW: i64 = 1_800_000_000;

fn device() -> AuthenticatedDevice {
    AuthenticatedDevice {
        id: "device-1".into(),
        label: "My iPhone".into(),
    }
}

fn signed(prefix: &str, device: Option<AuthenticatedDevice>, key: &str) -> HeaderMap {
    let raw = RelayClaim {
        issued_at: NOW,
        prefix: prefix.to_string(),
        device,
    }
    .encode();
    let mut headers = HeaderMap::new();
    headers.insert(HEADER_RELAY_CLAIM, raw.parse().unwrap());
    headers.insert(HEADER_RELAY_SIGNATURE, sign(key, &raw).parse().unwrap());
    headers
}

// ── Claims never cross routes ───────────────────────────────────────────────

#[test]
fn a_manage_claim_never_verifies_as_a_workspace_claim() {
    let headers = signed(MANAGE_CLAIM_PREFIX, Some(device()), "key");
    assert!(verify_manage_claim(&headers, "key", NOW).is_ok());
    assert_eq!(
        relay::verify_claim(&headers, "key", "dev", NOW),
        Err(ClaimRefusal::PrefixMismatch)
    );
}

#[test]
fn a_workspace_claim_never_verifies_on_the_manage_hop() {
    let headers = signed("/dev@cloud/", Some(device()), "key");
    assert!(relay::verify_claim(&headers, "key", "dev", NOW).is_ok());
    assert_eq!(
        verify_manage_claim(&headers, "key", NOW),
        Err(ClaimRefusal::PrefixMismatch)
    );
}

#[test]
fn a_manage_claim_must_name_a_device() {
    let headers = signed(MANAGE_CLAIM_PREFIX, None, "key");
    assert_eq!(
        verify_manage_claim(&headers, "key", NOW),
        Err(ClaimRefusal::NoDevice)
    );
}

#[test]
fn a_manage_claim_under_another_credential_is_refused() {
    let headers = signed(MANAGE_CLAIM_PREFIX, Some(device()), "other");
    assert_eq!(
        verify_manage_claim(&headers, "key", NOW),
        Err(ClaimRefusal::BadSignature)
    );
}

// ── The grant ───────────────────────────────────────────────────────────────

#[test]
fn a_pairing_record_from_before_the_grant_reads_as_not_granted() {
    let record = json!({
        "id": "p1",
        "label": "Gateway A",
        "credential_digest": "d",
        "paired_at": "2020-01-01T00:00:00Z",
    });
    let gateway: crate::gateway_protocol::PairedGateway = serde_json::from_value(record).unwrap();
    assert!(!gateway.may_manage);
}

/// A device on A calls its manage route for `cloud`.
async fn through_a(
    p: &Paired,
    method: &str,
    rest: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut builder = axum::http::Request::builder()
        .method(method)
        .uri(format!("/~/api/v1/remote-gateways/cloud/manage{rest}"))
        .header(
            header::COOKIE,
            format!("{}=cred-abc", p.a.device_cookie_name()),
        );
    let body = match body {
        Some(json) => {
            builder = builder.header(header::CONTENT_TYPE, "application/json");
            Body::from(json.to_string())
        }
        None => Body::empty(),
    };
    let response = crate::server::gateway_router(p.a.clone())
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

fn grant(p: &Paired, may_manage: bool) {
    p.b.paired_gateways()
        .set_may_manage(&p.pairing_id, may_manage)
        .unwrap()
        .expect("the fixture's pairing exists");
}

fn seed_b_device(p: &Paired, id: &str) {
    p.b.seed_device_for_test(PairedDevice {
        id: id.into(),
        label: "B's laptop".into(),
        credential_digest: auth::digest(&format!("cred-{id}")),
        paired_at: "2020-01-01T00:00:00Z".into(),
        last_seen_at: None,
    })
    .unwrap();
}

#[tokio::test]
async fn the_hop_is_refused_until_b_grants_it() {
    let p = paired().await;
    let (status, body) = through_a(&p, "GET", "/users", None).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(body["reason_kind"], REASON_NOT_GRANTED);

    grant(&p, true);
    let (status, body) = through_a(&p, "GET", "/users", None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body["users"].is_array(), "{body}");

    grant(&p, false);
    let (status, _) = through_a(&p, "GET", "/users", None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn a_device_on_a_revokes_a_device_on_b() {
    let p = paired().await;
    grant(&p, true);
    seed_b_device(&p, "b-laptop");
    let (status, body) = through_a(&p, "GET", "/devices", None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body
        .as_array()
        .unwrap()
        .iter()
        .any(|d| d["id"] == "b-laptop"));

    let (status, _) = through_a(&p, "DELETE", "/devices/b-laptop", None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert!(!p
        .b
        .directory()
        .devices
        .devices
        .iter()
        .any(|d| d.id == "b-laptop"));
    // A's own devices are untouched.
    assert!(p
        .a
        .directory()
        .devices
        .devices
        .iter()
        .any(|d| d.id == "device-1"));
}

#[tokio::test]
async fn a_device_on_a_adds_and_renames_a_user_on_b() {
    let p = paired().await;
    grant(&p, true);
    let (status, body) = through_a(&p, "POST", "/users", Some(json!({ "name": "Sam" }))).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let sam = body["users"]
        .as_array()
        .unwrap()
        .iter()
        .find(|u| u["name"] == "Sam")
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    let (status, _) = through_a(
        &p,
        "PUT",
        &format!("/users/{sam}"),
        Some(json!({ "name": "Samantha" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(p.b.directory().user(&sam).unwrap().name, "Samantha");
    assert!(p.a.directory().users.iter().all(|u| u.name != "Samantha"));
}

#[tokio::test]
async fn the_hop_serves_exactly_the_allow_list() {
    let p = paired().await;
    grant(&p, true);
    seed_b_device(&p, "b-laptop");
    let user = p.b.directory().users[0].id.clone();
    let allowed: &[(&str, String, Option<Value>)] = &[
        ("GET", "/devices".into(), None),
        ("GET", "/users".into(), None),
        ("POST", "/users".into(), Some(json!({ "name": "Kim" }))),
        (
            "PUT",
            format!("/users/{user}"),
            Some(json!({ "name": "Ann" })),
        ),
        (
            "POST",
            format!("/users/{user}/emails"),
            Some(json!({ "email": "ann@example.com" })),
        ),
        (
            "DELETE",
            format!("/users/{user}/emails/ann%40example.com"),
            None,
        ),
        (
            "PUT",
            "/devices/b-laptop/user".into(),
            Some(json!({ "user_id": user })),
        ),
        ("DELETE", "/devices/b-laptop".into(), None),
        ("DELETE", "/users/no-such-user".into(), None),
    ];
    for (method, rest, body) in allowed {
        let (status, answer) = through_a(&p, method, rest, body.clone()).await;
        assert!(
            status.is_success() || status == StatusCode::BAD_REQUEST,
            "{method} {rest}: {status} {answer}"
        );
    }
    let refused: &[(&str, &str)] = &[
        ("POST", "/pairing-code"),
        ("POST", "/gateway-pairing-code"),
        ("GET", "/gateways"),
        ("DELETE", "/gateways/x"),
        ("PUT", "/gateways/x/manage"),
        ("GET", "/session"),
        ("POST", "/pair"),
        ("GET", "/control/status"),
        ("GET", "/external-address"),
        ("GET", "/devices/b-laptop"),
        ("PATCH", "/users"),
        ("GET", "/devices/b-laptop/user"),
    ];
    for (method, rest) in refused {
        let (status, answer) = through_a(&p, method, rest, None).await;
        assert!(
            status == StatusCode::NOT_FOUND || status == StatusCode::METHOD_NOT_ALLOWED,
            "{method} {rest}: {status} {answer}"
        );
    }
    assert_eq!(p.b.paired_gateways().list().len(), 1);
}

#[tokio::test]
async fn a_dot_segment_never_reaches_another_call_on_b() {
    let p = paired().await;
    grant(&p, true);
    let (status, _) = through_a(&p, "GET", "/%2e%2e/workspaces", None).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn b_refuses_a_workspace_claim_on_the_manage_hop() {
    let p = paired().await;
    grant(&p, true);
    let raw = RelayClaim {
        issued_at: chrono::Utc::now().timestamp(),
        prefix: "/dev@cloud/".into(),
        device: Some(device()),
    }
    .encode();
    let mut headers = HeaderMap::new();
    headers.insert(HEADER_RELAY_CLAIM, raw.parse().unwrap());
    headers.insert(
        HEADER_RELAY_SIGNATURE,
        sign(&p.credential, &raw).parse().unwrap(),
    );
    headers.insert(HEADER_GATEWAY_CREDENTIAL, p.credential.parse().unwrap());
    let status = reqwest::Client::builder()
        .danger_accept_invalid_certs(true)
        .build()
        .unwrap()
        .get(format!(
            "{}{GATEWAY_API_PREFIX}{MANAGE_PATH}/users",
            p.b_address
        ))
        .headers(headers)
        .header("x-proxy-token", "proxy-secret")
        .send()
        .await
        .unwrap()
        .status();
    assert_eq!(status, 401);
}

#[tokio::test]
async fn an_unpaired_a_reads_as_refused_not_as_signed_out() {
    let p = paired().await;
    assert!(p.b.paired_gateways().unpair(&p.pairing_id).unwrap());
    let (status, body) = through_a(&p, "GET", "/users", None).await;
    assert_eq!(status, StatusCode::BAD_GATEWAY);
    assert_eq!(body["reason_kind"], REASON_REMOTE_REFUSED);
}

#[tokio::test]
async fn a_learns_that_b_was_updated_since_pairing() {
    let p = paired().await;
    grant(&p, true);
    assert!(p.a.remote_gateways().list()[0].gateway_protocol < MANAGE_PROTOCOL);
    let (status, _) = through_a(&p, "GET", "/users", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        p.a.remote_gateways().list()[0].gateway_protocol,
        crate::gateway_protocol::GATEWAY_PROTOCOL
    );
}

#[tokio::test]
async fn an_older_b_answers_a_typed_too_old() {
    let cert = localhost_cert();
    let old_b = axum::Router::new().route(
        crate::peers::GATEWAY_HEALTH_PATH,
        axum::routing::get(|| async {
            axum::Json(json!({ "role": "gateway", "instance_id": "old", "gateway_protocol": 3 }))
        }),
    );
    let port = serve_https(old_b, &cert).await;
    let dir = tempfile::tempdir().unwrap();
    let a = GatewayState::for_tests_with_static_dir(Some(dir.path().to_path_buf()));
    a.peer_connection()
        .trust_for_test(cert.cert.pem().as_bytes());
    a.remote_gateways()
        .add(RemoteGateway {
            name: "cloud".into(),
            address: format!("https://localhost:{port}"),
            pairing_id: "p1".into(),
            credential: "c".into(),
            headers: Default::default(),
            gateway_protocol: 3,
            added_at: "2020-01-01T00:00:00Z".into(),
            rotation: Default::default(),
        })
        .unwrap();
    a.seed_device_for_test(PairedDevice {
        id: "device-1".into(),
        label: "My iPhone".into(),
        credential_digest: auth::digest("cred-abc"),
        paired_at: "2020-01-01T00:00:00Z".into(),
        last_seen_at: None,
    })
    .unwrap();
    let response = crate::server::gateway_router(a.clone())
        .oneshot(
            axum::http::Request::builder()
                .uri("/~/api/v1/remote-gateways/cloud/manage/users")
                .header(
                    header::COOKIE,
                    format!("{}=cred-abc", a.device_cookie_name()),
                )
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CONFLICT);
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["reason_kind"], REASON_TOO_OLD);
}

#[tokio::test]
async fn b_turns_the_grant_on_from_its_paired_gateways_row() {
    let p = paired().await;
    let response = crate::server::gateway_router(p.b.clone())
        .oneshot(
            axum::http::Request::builder()
                .method("PUT")
                .uri(format!("/~/api/v1/auth/gateways/{}/manage", p.pairing_id))
                .header(auth::HEADER_LOCAL_TOKEN, "test-local-token")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(json!({ "may_manage": true }).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(p.b.paired_gateways().list()[0].may_manage);
}

#[test]
fn a_secret_minted_with_the_grant_pairs_with_it() {
    let dir = tempfile::tempdir().unwrap();
    let store =
        crate::gateway_protocol::PairedGatewayStore::load(dir.path().join("p.json")).unwrap();
    let code = store
        .mint_secret(None, true, crate::pairing::Minter::PairedDevice)
        .unwrap();
    store
        .redeem(&code, None, crate::pairing::OwnerGate::NoDevicePaired)
        .unwrap();
    let plain = store
        .mint_secret(None, false, crate::pairing::Minter::PairedDevice)
        .unwrap();
    store
        .redeem(&plain, None, crate::pairing::OwnerGate::NoDevicePaired)
        .unwrap();
    let grants: Vec<bool> = store.list().iter().map(|g| g.may_manage).collect();
    assert_eq!(grants, [true, false]);
}
