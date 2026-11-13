use super::*;
use crate::auth;
use crate::gateway_protocol::GatewayRedemption;
use crate::gateway_protocol::HEADER_GATEWAY_CREDENTIAL;
use crate::registry::Workspace;
use crate::remote_gateways::tests::{localhost_cert, serve_https};
use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use serde_json::{json, Value};
use tower::ServiceExt as _;

const DEVICE_CREDENTIAL: &str = "cred-abc";

/// Gateway B over https with one workspace, and gateway A paired with it as
/// `cloud`.
struct Pair {
    a: GatewayState,
    b: GatewayState,
    b_address: String,
    credential: String,
    pairing_id: String,
    _dirs: [tempfile::TempDir; 2],
}

async fn pair() -> Pair {
    let b_dir = tempfile::tempdir().unwrap();
    let b = GatewayState::for_tests_with_static_dir(Some(b_dir.path().to_path_buf()));
    b.register_for_test(Workspace::gateway_provisioned(
        "dev".to_string(),
        "Dev".to_string(),
        40_123,
    ));
    let cert = localhost_cert();
    let port = serve_https(
        crate::remote_gateways::tests::proxy_guard(crate::server::gateway_router(b.clone())),
        &cert,
    )
    .await;
    let b_address = format!("https://localhost:{port}");
    let code = b
        .paired_gateways()
        .mint_secret(None, false, crate::pairing::Minter::PairedDevice)
        .unwrap();
    let GatewayRedemption::Paired {
        pairing_id,
        credential,
    } = b
        .paired_gateways()
        .redeem(&code, None, crate::pairing::OwnerGate::NoDevicePaired)
        .unwrap()
    else {
        panic!("a fresh code redeems");
    };

    let a_dir = tempfile::tempdir().unwrap();
    let a = GatewayState::for_tests_with_static_dir(Some(a_dir.path().to_path_buf()));
    a.peer_connection()
        .trust_for_test(cert.cert.pem().as_bytes());
    add_remote(&a, "cloud", &b_address, &credential);
    a.seed_device_for_test(auth::PairedDevice {
        id: "device-1".into(),
        label: "My iPhone".into(),
        credential_digest: auth::digest(DEVICE_CREDENTIAL),
        paired_at: "2020-01-01T00:00:00Z".into(),
        last_seen_at: None,
    })
    .unwrap();
    Pair {
        a,
        b,
        b_address,
        credential,
        pairing_id,
        _dirs: [b_dir, a_dir],
    }
}

fn add_remote(a: &GatewayState, name: &str, address: &str, credential: &str) {
    a.remote_gateways()
        .add(RemoteGateway {
            name: name.to_string(),
            address: address.to_string(),
            pairing_id: "pairing".to_string(),
            credential: credential.to_string(),
            headers: crate::remote_gateways::tests::proxy_headers(),
            gateway_protocol: 1,
            added_at: "2020-01-01T00:00:00Z".to_string(),
            rotation: Default::default(),
        })
        .unwrap();
}

async fn send(state: &GatewayState, req: Request<Body>) -> (StatusCode, Value) {
    let response = crate::server::gateway_router(state.clone())
        .oneshot(req)
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

/// A paired device asking A for the remote workspaces.
async fn listed_on_a(p: &Pair) -> (StatusCode, Value) {
    let cookie = format!("{}={DEVICE_CREDENTIAL}", p.a.device_cookie_name());
    let req = Request::builder()
        .uri("/~/api/v1/remote-workspaces")
        .header(header::COOKIE, cookie)
        .body(Body::empty())
        .unwrap();
    send(&p.a, req).await
}

#[tokio::test]
async fn remote_counts_match_the_namespaced_inbox_instead_of_the_shared_count() {
    let p = pair().await;
    let reader = crate::relay::namespaced_device_id(&p.pairing_id, "device-1");
    let _engine = crate::notification_count_tests::install_inbox(&p.b, reader.clone()).await;
    let (status, rows) = listed_on_a(&p).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(rows[0]["workspaces"][0]["unread_count"], 0);
    assert_eq!(p.b.list_status().await[0].unread_count, Some(12));
    assert!(p
        .b
        .directory()
        .relayed
        .iter()
        .any(|d| d.device_id == reader));
}

#[tokio::test]
async fn a_failed_remote_inbox_probe_keeps_the_workspace_but_omits_its_shared_count() {
    let p = pair().await;
    let _engine = crate::notification_count_tests::install_inbox(&p.b, "unused".into()).await;
    let dead = crate::engine_hop::test_engine::scratch();
    p.b.set_route_for_test("dev", crate::engine_hop::test_engine::unreachable(&dead));
    let (status, rows) = listed_on_a(&p).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(rows[0]["state"], "reachable");
    assert_eq!(rows[0]["workspaces"][0]["id"], "dev");
    assert!(rows[0]["workspaces"][0].get("unread_count").is_none());
}

#[tokio::test]
async fn b_lists_its_workspaces_to_a_paired_gateway_without_ports() {
    let p = pair().await;
    let req = |credential: Option<&str>| {
        let mut builder = Request::builder().uri(format!("{GATEWAY_API_PREFIX}{WORKSPACES_PATH}"));
        if let Some(credential) = credential {
            builder = builder.header(HEADER_GATEWAY_CREDENTIAL, credential);
        }
        builder.body(Body::empty()).unwrap()
    };
    let (status, body) = send(&p.b, req(Some(&p.credential))).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body,
        json!([{ "id": "dev", "name": "Dev", "health": "unhealthy", "last_error": "not started" }])
    );

    for refused in [None, Some("not-the-credential")] {
        let (status, _) = send(&p.b, req(refused)).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{refused:?}");
    }
}

#[tokio::test]
async fn a_lists_each_remote_gateways_workspaces_and_names_the_one_it_cannot_reach() {
    let p = pair().await;
    // A port nothing listens on: bind one, then let it go.
    let closed = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    add_remote(&p.a, "gone", &format!("https://localhost:{closed}"), "x");

    let (status, body) = listed_on_a(&p).await;
    assert_eq!(status, StatusCode::OK);
    let listings = body.as_array().expect("a list");
    assert!(listings[0]["last_seen_at"].is_string());
    assert_eq!(listings[1]["reason_kind"], "refused");
    assert_eq!(
        listings[0]["workspaces"],
        json!([{ "id": "dev", "name": "Dev", "health": "unhealthy", "last_error": "not started" }])
    );
    assert_eq!(listings[1]["gateway"], "gone");
    assert_eq!(listings[1]["state"], "unreachable");
    let reason = listings[1]["reason"].as_str().unwrap();
    assert!(
        reason.starts_with("could not reach https://localhost:"),
        "{reason}"
    );
}

#[tokio::test]
async fn an_unpaired_b_reads_as_a_revoked_grant() {
    let p = pair().await;
    assert!(p.b.paired_gateways().unpair(&p.pairing_id).unwrap());
    let (_, body) = listed_on_a(&p).await;
    assert_eq!(body[0]["state"], "unreachable");
    assert_eq!(body[0]["reason_kind"], "revoked");
    let reason = body[0]["reason"].as_str().unwrap();
    assert_eq!(
        reason,
        format!(
            "{} no longer grants this gateway access. Remove it here and pair again",
            p.b_address
        )
    );
}

#[tokio::test]
async fn only_a_paired_device_reads_the_remote_workspaces() {
    let p = pair().await;
    let req = Request::builder()
        .uri("/~/api/v1/remote-workspaces")
        .body(Body::empty())
        .unwrap();
    let (status, _) = send(&p.a, req).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn bs_credential_lists_nothing_on_a() {
    let p = pair().await;
    let req = Request::builder()
        .uri("/~/api/v1/remote-workspaces")
        .header(HEADER_GATEWAY_CREDENTIAL, &p.credential)
        .body(Body::empty())
        .unwrap();
    let (status, _) = send(&p.a, req).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn proxy_headers_can_be_replaced_without_repairing_and_require_a_device() {
    let p = pair().await;
    let update = |authenticated: bool| {
        let mut req = Request::builder()
            .method("PUT")
            .uri("/~/api/v1/remote-gateways/cloud")
            .header(header::CONTENT_TYPE, "application/json");
        if authenticated {
            req = req.header(
                header::COOKIE,
                format!("{}={DEVICE_CREDENTIAL}", p.a.device_cookie_name()),
            );
        }
        req.body(Body::from(r#"{"headers":{"X-Token":"new-proxy-secret"}}"#))
            .unwrap()
    };
    let (status, _) = send(&p.a, update(false)).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, row) = send(&p.a, update(true)).await;
    assert_eq!(status, StatusCode::OK, "{row}");
    assert_eq!(row["header_names"], json!(["x-token"]));
    assert!(!row.to_string().contains("new-proxy-secret"));
    assert_eq!(p.a.remote_gateways().list()[0].credential, p.credential);
    let cookie = format!("{}={DEVICE_CREDENTIAL}", p.a.device_cookie_name());
    let req = Request::builder()
        .method("PUT")
        .uri("/~/api/v1/remote-gateways/cloud")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::COOKIE, cookie)
        .body(Body::from(r#"{"headers":{}}"#))
        .unwrap();
    let (status, row) = send(&p.a, req).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(row["header_names"], json!([]));
}

#[tokio::test]
async fn revoked_gateway_keeps_its_last_success_timestamp() {
    let p = pair().await;
    let (_, first) = listed_on_a(&p).await;
    assert!(first[0]["last_seen_at"].is_string());
    p.b.paired_gateways().unpair(&p.pairing_id).unwrap();
    let (_, second) = listed_on_a(&p).await;
    assert_eq!(second[0]["last_seen_at"], first[0]["last_seen_at"]);
    assert_eq!(second[0]["reason_kind"], "revoked");
}

#[tokio::test]
async fn b_does_not_list_its_own_remote_gateways() {
    let p = pair().await;
    add_remote(&p.b, "elsewhere", "https://example.invalid", "unused");
    let (_, body) = listed_on_a(&p).await;
    assert_eq!(body[0]["workspaces"].as_array().unwrap().len(), 1);
    assert_eq!(body[0]["workspaces"][0]["id"], "dev");
}

#[tokio::test]
async fn a_remote_that_never_completes_tls_times_out() {
    let p = pair().await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = format!(
        "https://127.0.0.1:{}",
        listener.local_addr().unwrap().port()
    );
    let task = tokio::spawn(async move {
        let (_socket, _) = listener.accept().await.unwrap();
        std::future::pending::<()>().await;
    });
    add_remote(&p.a, "silent", &address, "unused");
    let (_, body) = listed_on_a(&p).await;
    task.abort();
    let silent = body
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["gateway"] == "silent")
        .unwrap();
    assert_eq!(silent["reason_kind"], "timeout");
}

#[tokio::test]
async fn newer_additive_protocol_can_still_list_workspaces() {
    let p = pair().await;
    let mut remote = p.a.remote_gateways().remove("cloud").unwrap().unwrap();
    remote.gateway_protocol = crate::gateway_protocol::GATEWAY_PROTOCOL + 1;
    p.a.remote_gateways().add(remote).unwrap();
    let (_, body) = listed_on_a(&p).await;
    assert_eq!(body[0]["state"], "reachable");
}

#[tokio::test]
async fn removing_gateway_forgets_last_seen() {
    let p = pair().await;
    listed_on_a(&p).await;
    p.a.remote_gateways().remove("cloud").unwrap();
    assert!(p.a.remote_gateways().last_seen.lock().unwrap().is_empty());
}
