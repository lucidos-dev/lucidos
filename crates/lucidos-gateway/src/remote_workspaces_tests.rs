use super::*;
use crate::gateway_protocol::GatewayRedemption;
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
    let port = serve_https(crate::server::gateway_router(b.clone()), &cert).await;
    let b_address = format!("https://localhost:{port}");
    let code = b.paired_gateways().mint_code(None).unwrap();
    let GatewayRedemption::Paired {
        pairing_id,
        credential,
    } = b.paired_gateways().redeem(&code, None).unwrap()
    else {
        panic!("a fresh code redeems");
    };

    let a_dir = tempfile::tempdir().unwrap();
    let a = GatewayState::for_tests_with_static_dir(Some(a_dir.path().to_path_buf()));
    a.relay_client().trust_for_test(cert.cert.pem().as_bytes());
    add_remote(&a, "cloud", &b_address, &credential);
    a.write_paired_devices_for_test(|paired| {
        paired.devices.push(auth::PairedDevice {
            id: "device-1".into(),
            label: "My iPhone".into(),
            credential_digest: auth::digest(DEVICE_CREDENTIAL),
            paired_at: "2020-01-01T00:00:00Z".into(),
            last_seen_at: None,
        });
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
            gateway_protocol: 1,
            added_at: "2020-01-01T00:00:00Z".to_string(),
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
    assert_eq!(
        listings[0],
        json!({
            "gateway": "cloud",
            "state": "reachable",
            "workspaces": [{ "id": "dev", "name": "Dev", "health": "unhealthy", "last_error": "not started" }],
        })
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
