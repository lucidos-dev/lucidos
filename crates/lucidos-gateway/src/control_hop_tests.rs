use super::*;
use crate::auth::{self, PairedDevice};
use crate::gateway_protocol::{HopGrant, HopGrants, PairedGateway};
use crate::relay::tests::{paired, Paired};
use crate::relay::{sign, HEADER_RELAY_CLAIM, HEADER_RELAY_SIGNATURE};
use lucidos_local_token::test_support::serve_https;
use serde_json::{json, Value};
use tower::ServiceExt as _;

const NOW: i64 = 1_800_000_000;

fn device() -> AuthenticatedDevice {
    AuthenticatedDevice {
        id: "device-1".into(),
        label: "My iPhone".into(),
    }
}

fn signed(prefix: &str, key: &str) -> HeaderMap {
    let raw = RelayClaim {
        issued_at: NOW,
        prefix: prefix.to_string(),
        device: Some(device()),
    }
    .encode();
    let mut headers = HeaderMap::new();
    headers.insert(HEADER_RELAY_CLAIM, raw.parse().unwrap());
    headers.insert(HEADER_RELAY_SIGNATURE, sign(key, &raw).parse().unwrap());
    headers
}

fn grant(p: &Paired, grant: HopGrant, on: bool) {
    p.b.paired_gateways()
        .set_grant(&p.pairing_id, grant, on)
        .unwrap()
        .expect("the fixture's pairing exists");
}

struct Answer {
    status: StatusCode,
    headers: HeaderMap,
    body: Value,
}

/// A device on A calls its control route for `cloud`.
async fn through_a(
    a: &GatewayState,
    method: &str,
    rest: &str,
    headers: &[(&str, &str)],
    body: Body,
) -> Answer {
    let mut builder = axum::http::Request::builder()
        .method(method)
        .uri(format!("/~/api/v1/remote-gateways/cloud/control{rest}"))
        .header(
            header::COOKIE,
            format!("{}=cred-abc", a.device_cookie_name()),
        );
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    let response = crate::server::gateway_router(a.clone())
        .oneshot(builder.body(body).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    Answer {
        status,
        headers,
        body: serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    }
}

async fn get(a: &GatewayState, rest: &str) -> Answer {
    through_a(a, "GET", rest, &[], Body::empty()).await
}

async fn post_json(a: &GatewayState, rest: &str, body: Value) -> Answer {
    through_a(
        a,
        "POST",
        rest,
        &[("content-type", "application/json")],
        Body::from(body.to_string()),
    )
    .await
}

fn header_of<'a>(answer: &'a Answer, name: &str) -> &'a str {
    answer
        .headers
        .get(name)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
}

// ── I7: without the grant, A reaches nothing new ────────────────────────────

#[test]
fn a_pairing_record_from_before_the_control_grant_reads_as_not_granted() {
    let record = json!({
        "id": "p1",
        "label": "Gateway A",
        "credential_digest": "d",
        "paired_at": "2020-01-01T00:00:00Z",
        "may_manage": true,
    });
    let gateway: PairedGateway = serde_json::from_value(record).unwrap();
    assert!(gateway.may_manage);
    assert!(!gateway.may_control);
}

#[tokio::test]
async fn the_control_hop_is_refused_until_b_grants_it() {
    let p = paired().await;
    p.b.register_for_test(crate::registry::Workspace::gateway_provisioned(
        "dev".to_string(),
        "Dev".to_string(),
        40_123,
    ));
    let refused = get(&p.a, "/workspaces").await;
    assert_eq!(refused.status, StatusCode::FORBIDDEN, "{}", refused.body);
    assert_eq!(refused.body["reason_kind"], CONTROL.not_granted_kind);

    grant(&p, HopGrant::Manage, true);
    let manage_only = get(&p.a, "/workspaces").await;
    assert_eq!(
        manage_only.status,
        StatusCode::FORBIDDEN,
        "the manage grant does not open the control hop"
    );

    grant(&p, HopGrant::Control, true);
    let listed = get(&p.a, "/workspaces").await;
    assert_eq!(listed.status, StatusCode::OK, "{}", listed.body);
    let rows = listed.body.as_array().expect("a list of workspaces");
    assert!(rows.iter().any(|w| w["id"] == "dev"), "{}", listed.body);
    for row in rows {
        assert!(
            row.get("path").is_none() && row.get("port").is_none(),
            "{row}"
        );
    }

    grant(&p, HopGrant::Control, false);
    assert_eq!(get(&p.a, "/workspaces").await.status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn the_control_grant_does_not_open_the_manage_hop() {
    let p = paired().await;
    grant(&p, HopGrant::Control, true);
    let response = crate::server::gateway_router(p.a.clone())
        .oneshot(
            axum::http::Request::builder()
                .uri("/~/api/v1/remote-gateways/cloud/manage/users")
                .header(
                    header::COOKIE,
                    format!("{}=cred-abc", p.a.device_cookie_name()),
                )
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

// ── I8: the allow-list and nothing else ─────────────────────────────────────

#[tokio::test]
async fn a_restore_upload_runs_through_the_hop() {
    let p = paired().await;
    grant(&p, HopGrant::Control, true);
    let chunk = b"backup bytes";
    let length = chunk.len().to_string();
    let created = through_a(
        &p.a,
        "POST",
        "/restore-uploads",
        &[
            ("tus-resumable", crate::restore_upload::TUS_VERSION),
            ("upload-length", &length),
            ("upload-metadata", "filename YmFja3VwLnRhci5hZ2U="),
        ],
        Body::empty(),
    )
    .await;
    assert_eq!(created.status, StatusCode::CREATED, "{}", created.body);
    let location = header_of(&created, "location");
    let own_root = "/~/api/v1/remote-gateways/cloud/control/restore-uploads/";
    let id = location
        .strip_prefix(own_root)
        .unwrap_or_else(|| panic!("A's own path, not B's: {location}"))
        .to_string();
    assert_eq!(header_of(&created, "tus-resumable"), "1.0.0");

    let tus = [("tus-resumable", crate::restore_upload::TUS_VERSION)];
    let patched = through_a(
        &p.a,
        "PATCH",
        &format!("/restore-uploads/{id}"),
        &[
            tus[0],
            ("content-type", "application/offset+octet-stream"),
            ("upload-offset", "0"),
        ],
        Body::from(chunk.to_vec()),
    )
    .await;
    assert_eq!(patched.status, StatusCode::NO_CONTENT, "{}", patched.body);
    assert_eq!(header_of(&patched, "upload-offset"), length);

    let probed = through_a(
        &p.a,
        "HEAD",
        &format!("/restore-uploads/{id}"),
        &tus,
        Body::empty(),
    )
    .await;
    assert_eq!(probed.status, StatusCode::OK);
    assert_eq!(header_of(&probed, "upload-offset"), length);
    assert_eq!(header_of(&probed, "upload-length"), length);
    assert!(!header_of(&probed, "upload-expires").is_empty());

    let removed = through_a(
        &p.a,
        "DELETE",
        &format!("/restore-uploads/{id}"),
        &tus,
        Body::empty(),
    )
    .await;
    assert_eq!(removed.status, StatusCode::NO_CONTENT);
    assert!(
        p.b.restore_uploads().claim(&id, 0).is_err(),
        "B dropped the session"
    );
}

#[tokio::test]
async fn the_control_hop_serves_exactly_the_allow_list() {
    let p = paired().await;
    p.b.register_for_test(crate::registry::Workspace::gateway_provisioned(
        "dev".to_string(),
        "Dev".to_string(),
        40_123,
    ));
    grant(&p, HopGrant::Control, true);
    grant(&p, HopGrant::Manage, true);

    // Each allowed call reaches its handler: a refusal here comes from the
    // handler's own check, never from the router.
    assert_eq!(get(&p.a, "/restore-status").await.status, StatusCode::OK);
    let created = post_json(&p.a, "/workspaces", json!({ "name": " " })).await;
    assert_eq!(created.status, StatusCode::BAD_REQUEST, "{}", created.body);
    let commit = post_json(
        &p.a,
        "/workspaces/restore",
        json!({ "upload": "00000000000000000000000000000000", "key": "k" }),
    )
    .await;
    assert_eq!(commit.status, StatusCode::NOT_FOUND, "{}", commit.body);
    for rest in [
        "/workspaces/no-such-ws/start",
        "/workspaces/no-such-ws/stop",
    ] {
        let answer = through_a(&p.a, "POST", rest, &[], Body::empty()).await;
        assert!(
            answer.status.is_client_error() && answer.body["error"].is_string(),
            "{rest}: {} {}",
            answer.status,
            answer.body
        );
    }

    let refused: &[(&str, &str)] = &[
        ("DELETE", "/workspaces/dev"),
        ("POST", "/workspaces/dev/rename"),
        ("POST", "/workspaces/dev/restart"),
        ("POST", "/workspaces/dev/autostart"),
        ("POST", "/workspaces/dev/snapshot-restore"),
        ("POST", "/workspaces/dev/boot-phase"),
        ("POST", "/workspaces/adopt"),
        ("PUT", "/workspace-order"),
        ("GET", "/trash"),
        ("POST", "/trash/dev/restore"),
        ("POST", "/trash/empty-batches"),
        ("POST", "/workspace-delete-batches"),
        ("POST", "/cleanup/batches"),
        ("GET", "/cleanup"),
        ("GET", "/gateway/status"),
        ("POST", "/gateway/reload"),
        ("POST", "/gateway/check-updates"),
        ("GET", "/engine-builds"),
        ("POST", "/engine-builds/switch-running"),
        ("PUT", "/release-check"),
        ("GET", "/network-config"),
        ("PUT", "/external-address"),
        ("GET", "/tls"),
        ("GET", "/tailnet-status"),
        ("GET", "/diagnostic-log"),
        ("GET", "/unread-total"),
        ("GET", "/installs"),
        ("POST", "/broker/workspaces"),
        ("GET", "/users"),
        ("GET", "/devices"),
        ("POST", "/pairing-code"),
        ("POST", "/gateway-pairing-code"),
        ("GET", "/gateways"),
        ("PUT", "/gateways/x/control"),
        ("PUT", "/workspaces"),
    ];
    for (method, rest) in refused {
        let answer = through_a(&p.a, method, rest, &[], Body::empty()).await;
        assert!(
            answer.status == StatusCode::NOT_FOUND
                || answer.status == StatusCode::METHOD_NOT_ALLOWED,
            "{method} {rest}: {} {}",
            answer.status,
            answer.body
        );
    }
    let registry = p.b.registry_snapshot_for_test();
    let dev = registry.workspaces.iter().find(|w| w.id == "dev");
    assert_eq!(
        dev.map(|w| w.name.as_str()),
        Some("Dev"),
        "B's workspace is untouched"
    );
    assert_eq!(p.b.paired_gateways().list().len(), 1);
}

#[tokio::test]
async fn a_dot_segment_never_reaches_another_call_on_b() {
    let p = paired().await;
    grant(&p, HopGrant::Control, true);
    let answer = get(&p.a, "/workspaces/%2e%2e/gateway/status").await;
    assert_eq!(answer.status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn a_body_over_one_chunk_is_refused_on_a() {
    let p = paired().await;
    grant(&p, HopGrant::Control, true);
    let over = vec![0u8; CONTROL.max_body_bytes + 1];
    let answer = through_a(&p.a, "PATCH", "/restore-uploads/x", &[], Body::from(over)).await;
    assert_eq!(answer.status, StatusCode::PAYLOAD_TOO_LARGE);
}

#[test]
fn the_body_cap_holds_the_largest_chunk_the_client_sends() {
    let client = include_str!("../../lucidos-app/src/api/client/resumableUpload.ts");
    assert!(
        client.contains("export const FIRST_CHUNK_BYTES = 32 * 1024 * 1024;"),
        "MAX_CHUNK_BYTES restates the client's FIRST_CHUNK_BYTES"
    );
    assert_eq!(MAX_CHUNK_BYTES, 32 * 1024 * 1024);
    assert!(CONTROL.max_body_bytes > MAX_CHUNK_BYTES);
}

// ── Claims never cross hops ─────────────────────────────────────────────────

#[test]
fn a_control_claim_verifies_on_no_other_route() {
    let headers = signed(CONTROL.claim_prefix, "key");
    assert!(CONTROL.verify_claim(&headers, "key", NOW).is_ok());
    assert_eq!(
        MANAGE.verify_claim(&headers, "key", NOW),
        Err(ClaimRefusal::PrefixMismatch)
    );
    assert_eq!(
        relay::verify_claim(&headers, "key", "dev", NOW),
        Err(ClaimRefusal::PrefixMismatch)
    );
}

#[test]
fn no_other_claim_verifies_on_the_control_hop() {
    for prefix in [MANAGE.claim_prefix, "/dev@cloud/"] {
        let headers = signed(prefix, "key");
        assert_eq!(
            CONTROL.verify_claim(&headers, "key", NOW),
            Err(ClaimRefusal::PrefixMismatch),
            "{prefix}"
        );
    }
}

#[tokio::test]
async fn b_refuses_a_manage_claim_on_the_control_hop() {
    let p = paired().await;
    grant(&p, HopGrant::Control, true);
    grant(&p, HopGrant::Manage, true);
    let raw = RelayClaim {
        issued_at: chrono::Utc::now().timestamp(),
        prefix: MANAGE.claim_prefix.into(),
        device: Some(device()),
    }
    .encode();
    let status = lucidos_local_token::client::client_builder()
        .build()
        .unwrap()
        .get(format!(
            "{}{GATEWAY_API_PREFIX}{CONTROL_PATH}/workspaces",
            p.b_address
        ))
        .header(HEADER_RELAY_CLAIM, raw.clone())
        .header(HEADER_RELAY_SIGNATURE, sign(&p.credential, &raw))
        .header(HEADER_GATEWAY_CREDENTIAL, p.credential.clone())
        .header("x-proxy-token", "proxy-secret")
        .send()
        .await
        .unwrap()
        .status();
    assert_eq!(status, 401);
}

// ── I10: B records who acted ────────────────────────────────────────────────

#[test]
fn b_names_the_device_and_the_gateway_for_each_change() {
    let line = attribution(
        &CONTROL,
        &Method::POST,
        "/workspaces",
        "My iPhone",
        "Gateway A",
    )
    .expect("a create is a change");
    assert_eq!(
        line,
        "[Gateway] control POST /workspaces by 'My iPhone' via 'Gateway A'"
    );
    for method in [Method::GET, Method::HEAD, Method::PATCH] {
        assert_eq!(
            attribution(&CONTROL, &method, "/restore-uploads/x", "d", "g"),
            None,
            "{method}"
        );
    }
}

// ── I11 and I9: B's answer never acts on A's browser ────────────────────────

#[tokio::test]
async fn an_unpaired_a_reads_as_refused_not_as_signed_out() {
    let p = paired().await;
    assert!(p.b.paired_gateways().unpair(&p.pairing_id).unwrap());
    let answer = get(&p.a, "/workspaces").await;
    assert_eq!(answer.status, StatusCode::BAD_GATEWAY);
    assert_eq!(answer.body["reason_kind"], REASON_REMOTE_REFUSED);
}

/// A gateway A paired with a fake B that serves `b` at protocol `protocol`.
fn a_paired_with(b: axum::Router, protocol: u32) -> (tempfile::TempDir, GatewayState) {
    let port = serve_https(b).port();
    let dir = tempfile::tempdir().unwrap();
    let a = GatewayState::for_tests_with_static_dir(Some(dir.path().to_path_buf()));
    a.remote_gateways()
        .add(RemoteGateway {
            gateway_id: None,
            name: "cloud".into(),
            address: format!("https://localhost:{port}"),
            pairing_id: "p1".into(),
            credential: "c".into(),
            headers: Default::default(),
            gateway_protocol: protocol,
            added_at: "2020-01-01T00:00:00Z".into(),
            rotation: Default::default(),
            return_access: None,
            trusted_pairing: false,
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
    (dir, a)
}

#[tokio::test]
async fn a_refuses_html_or_script_from_b() {
    for content_type in ["text/html; charset=utf-8", "application/javascript"] {
        let b = axum::Router::new().route(
            &format!("{GATEWAY_API_PREFIX}{CONTROL_PATH}/workspaces"),
            axum::routing::get(move || async move {
                (
                    [(header::CONTENT_TYPE, content_type)],
                    "<script>alert(1)</script>",
                )
            }),
        );
        let (_dir, a) = a_paired_with(b, CONTROL_PROTOCOL);
        let answer = get(&a, "/workspaces").await;
        assert_eq!(answer.status, StatusCode::BAD_GATEWAY, "{content_type}");
        assert_eq!(answer.body["reason_kind"], REASON_UNEXPECTED_ANSWER);
    }
}

#[tokio::test]
async fn a_keeps_a_proxy_html_413_as_a_json_413() {
    let b = axum::Router::new().route(
        &format!("{GATEWAY_API_PREFIX}{CONTROL_PATH}/workspaces"),
        axum::routing::get(|| async {
            (
                StatusCode::PAYLOAD_TOO_LARGE,
                [(header::CONTENT_TYPE, "text/html")],
                "<html>413 Request Entity Too Large</html>",
            )
        }),
    );
    let (_dir, a) = a_paired_with(b, CONTROL_PROTOCOL);
    let answer = get(&a, "/workspaces").await;
    assert_eq!(answer.status, StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(answer.body["reason_kind"], REASON_UNEXPECTED_ANSWER);
    assert!(!answer.body["error"].as_str().unwrap().contains("<html>"));
}

#[tokio::test]
async fn a_passes_json_from_b_with_nosniff() {
    let b = axum::Router::new().route(
        &format!("{GATEWAY_API_PREFIX}{CONTROL_PATH}/workspaces"),
        axum::routing::get(|| async { axum::Json(json!([])) }),
    );
    let (_dir, a) = a_paired_with(b, CONTROL_PROTOCOL);
    let answer = get(&a, "/workspaces").await;
    assert_eq!(answer.status, StatusCode::OK);
    assert_eq!(header_of(&answer, "x-content-type-options"), "nosniff");
}

#[tokio::test]
async fn a_b_that_serves_only_the_manage_hop_answers_a_typed_too_old() {
    let b = axum::Router::new().route(
        crate::peers::GATEWAY_HEALTH_PATH,
        axum::routing::get(|| async {
            axum::Json(json!({ "role": "gateway", "instance_id": "old", "gateway_protocol": 5 }))
        }),
    );
    let (_dir, a) = a_paired_with(b, 5);
    let answer = get(&a, "/workspaces").await;
    assert_eq!(answer.status, StatusCode::CONFLICT);
    assert_eq!(answer.body["reason_kind"], REASON_TOO_OLD);
}

// ── B's own switch ──────────────────────────────────────────────────────────

#[tokio::test]
async fn b_turns_the_control_grant_on_from_its_paired_gateways_row() {
    let p = paired().await;
    let response = crate::server::gateway_router(p.b.clone())
        .oneshot(
            axum::http::Request::builder()
                .method("PUT")
                .uri(format!("/~/api/v1/auth/gateways/{}/control", p.pairing_id))
                .header(auth::HEADER_LOCAL_TOKEN, "test-local-token")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(json!({ "may_control": true }).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let row: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(row["may_control"], true);
    assert_eq!(row["may_manage"], false);
    assert!(p.b.paired_gateways().list()[0].may_control);
}

#[test]
fn a_secret_minted_with_the_control_grant_pairs_with_it() {
    let dir = tempfile::tempdir().unwrap();
    let store =
        crate::gateway_protocol::PairedGatewayStore::load(dir.path().join("p.json")).unwrap();
    for grants in [
        HopGrants {
            manage: false,
            control: true,
            admin: false,
        },
        HopGrants::default(),
    ] {
        let code = store
            .mint_secret(None, grants, crate::pairing::Minter::PairedDevice)
            .unwrap();
        store
            .redeem(&code, None, crate::pairing::OwnerGate::NoDevicePaired)
            .unwrap();
    }
    let grants: Vec<(bool, bool)> = store
        .list()
        .iter()
        .map(|g| (g.may_manage, g.may_control))
        .collect();
    assert_eq!(grants, [(false, true), (false, false)]);
}
