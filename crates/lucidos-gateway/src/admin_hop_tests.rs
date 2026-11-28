use super::*;
use crate::auth;
use crate::gateway_protocol::{HopGrant, PairedGateway};
use crate::relay::tests::{paired, Paired};
use serde_json::{json, Value};
use tower::ServiceExt as _;

async fn through_a(
    p: &Paired,
    hop: &Hop,
    method: &str,
    rest: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut builder = axum::http::Request::builder()
        .method(method)
        .uri(format!("/~/api/v1/remote-gateways/cloud{}{rest}", hop.path))
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

fn grant(p: &Paired, grant: HopGrant, on: bool) {
    p.b.paired_gateways()
        .set_grant(&p.pairing_id, grant, on)
        .unwrap()
        .expect("the fixture's pairing exists");
}

/// Reads only, plus the mint: a refusal must come before any handler, and
/// no test may write this machine's network settings.
const ADMIN_ROUTES: &[(&str, &str)] = &[
    ("GET", "/control/gateway/status"),
    ("GET", "/control/installs"),
    ("GET", "/control/engine-builds"),
    ("GET", "/control/network-config"),
    ("GET", "/control/external-address"),
    ("GET", "/control/tls"),
    ("GET", "/control/tailnet-status"),
    ("GET", "/auth/devices"),
    ("GET", "/auth/users"),
    ("GET", "/auth/gateways"),
    ("POST", "/auth/gateway-pairing-code"),
];

#[test]
fn a_pairing_record_from_before_the_admin_grant_keeps_its_grants() {
    let record = json!({
        "id": "p1",
        "label": "Gateway A",
        "credential_digest": "d",
        "paired_at": "2020-01-01T00:00:00Z",
        "may_manage": true,
        "may_control": false,
    });
    let gateway: PairedGateway = serde_json::from_value(record).unwrap();
    assert!(!gateway.may_admin);
    assert!(gateway.may_manage);
    assert!(!gateway.may_control);
}

#[tokio::test]
async fn every_admin_route_is_refused_without_the_admin_grant() {
    let p = paired().await;
    for grants in [&[][..], &[HopGrant::Manage], &[HopGrant::Control]] {
        for g in [HopGrant::Manage, HopGrant::Control] {
            grant(&p, g, grants.contains(&g));
        }
        for (method, rest) in ADMIN_ROUTES {
            let (status, body) = through_a(&p, &ADMIN, method, rest, None).await;
            assert_eq!(status, StatusCode::FORBIDDEN, "{grants:?} {method} {rest}");
            assert_eq!(body["reason_kind"], ADMIN.not_granted_kind, "{rest}");
        }
    }
    assert_eq!(p.b.paired_gateways().list().len(), 1);
}

#[tokio::test]
async fn the_admin_grant_alone_reaches_the_manage_and_control_hops() {
    let p = paired().await;
    grant(&p, HopGrant::Admin, true);
    let (status, body) = through_a(&p, &MANAGE, "GET", "/users", None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, body) = through_a(&p, &CONTROL, "GET", "/workspaces", None).await;
    assert_eq!(status, StatusCode::OK, "{body}");

    grant(&p, HopGrant::Admin, false);
    let (status, _) = through_a(&p, &MANAGE, "GET", "/users", None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn the_admin_hop_serves_its_allow_list_and_no_grant_switch() {
    let p = paired().await;
    grant(&p, HopGrant::Admin, true);
    for rest in [
        "/control/gateway/status",
        "/control/engine-builds",
        "/control/tls",
        "/auth/devices",
        "/auth/users",
        "/auth/gateways",
    ] {
        let (status, body) = through_a(&p, &ADMIN, "GET", rest, None).await;
        assert_eq!(status, StatusCode::OK, "{rest}: {body}");
    }
    let id = p.pairing_id.clone();
    let refused = [
        ("PUT", format!("/auth/gateways/{id}/manage")),
        ("PUT", format!("/auth/gateways/{id}/control")),
        ("PUT", format!("/auth/gateways/{id}/admin")),
        ("PUT", format!("/auth/gateways/{id}/remote-starts")),
        ("POST", "/auth/pairing-code".into()),
        ("GET", "/auth/pairing-requests".into()),
        ("GET", "/auth/session".into()),
        ("GET", "/control/workspaces".into()),
        ("POST", "/control/workspaces/x/start".into()),
        ("GET", "/control/diagnostic-log".into()),
        ("GET", "/control/broker".into()),
        ("GET", "/remote-gateways".into()),
    ];
    for (method, rest) in &refused {
        let body = Some(json!({ "may_admin": true, "may_manage": true, "may_control": true }));
        let (status, answer) = through_a(&p, &ADMIN, method, rest, body).await;
        assert!(
            status == StatusCode::NOT_FOUND || status == StatusCode::METHOD_NOT_ALLOWED,
            "{method} {rest}: {status} {answer}"
        );
    }
    let row = &p.b.paired_gateways().list()[0];
    assert!(row.may_admin && !row.may_manage && !row.may_control);
}

#[tokio::test]
async fn a_hop_mint_never_grants_anything() {
    let p = paired().await;
    grant(&p, HopGrant::Admin, true);
    for ticks in ["admin=true", "manage=true", "control=true"] {
        let rest = format!("/auth/gateway-pairing-code?{ticks}");
        let (status, body) = through_a(&p, &ADMIN, "POST", &rest, None).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{ticks}: {body}");
    }

    let (status, body) = through_a(&p, &ADMIN, "POST", "/auth/gateway-pairing-code", None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let code = body["code"].as_str().unwrap();
    let redeemed =
        p.b.paired_gateways()
            .redeem(
                code,
                Some("Gateway C"),
                crate::pairing::OwnerGate::DevicePaired { asking_as: "C" },
            )
            .unwrap();
    assert!(
        matches!(
            redeemed,
            crate::gateway_protocol::GatewayRedemption::Paired { .. }
        ),
        "a hop mint needs no Allow on B: {redeemed:?}"
    );
    let c =
        p.b.paired_gateways()
            .list()
            .into_iter()
            .find(|g| g.label == "Gateway C")
            .unwrap();
    assert!(!c.may_manage && !c.may_control && !c.may_admin);
}

#[test]
fn a_hop_mint_names_the_gateway_it_came_through() {
    let line = attribution(
        &ADMIN,
        &Method::POST,
        "/~/api/gateway/v1/admin/auth/gateway-pairing-code",
        "My iPhone",
        "Gateway A",
    )
    .unwrap();
    assert!(line.contains("admin POST"), "{line}");
    assert!(line.contains("by 'My iPhone' via 'Gateway A'"), "{line}");
}

#[tokio::test]
async fn only_b_turns_the_admin_grant_on() {
    let p = paired().await;
    let response = crate::server::gateway_router(p.b.clone())
        .oneshot(
            axum::http::Request::builder()
                .method("PUT")
                .uri(format!("/~/api/v1/auth/gateways/{}/admin", p.pairing_id))
                .header(auth::HEADER_LOCAL_TOKEN, "test-local-token")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(json!({ "may_admin": true }).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(p.b.paired_gateways().list()[0].may_admin);

    let unauthenticated = crate::server::gateway_router(p.b.clone())
        .oneshot(
            axum::http::Request::builder()
                .method("PUT")
                .uri(format!("/~/api/v1/auth/gateways/{}/admin", p.pairing_id))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(json!({ "may_admin": false }).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(unauthenticated.status(), StatusCode::UNAUTHORIZED);
    assert!(p.b.paired_gateways().list()[0].may_admin);
}

#[tokio::test]
async fn a_hop_caller_names_the_device_as_the_relay_does() {
    async fn echo(axum::Extension(caller): axum::Extension<HopCaller>) -> String {
        caller.device_id
    }
    let receiver = Router::new()
        .route("/probe", axum::routing::post(echo))
        .route_layer(middleware::from_fn_with_state(&ADMIN, require_grant));
    let raw = RelayClaim {
        issued_at: chrono::Utc::now().timestamp(),
        prefix: ADMIN.claim_prefix.to_string(),
        device: Some(AuthenticatedDevice {
            id: "device-1".into(),
            label: "My iPhone".into(),
        }),
    }
    .encode();
    let mut request = axum::http::Request::post("/probe")
        .header(crate::relay::HEADER_RELAY_CLAIM, &raw)
        .header(
            crate::relay::HEADER_RELAY_SIGNATURE,
            crate::relay::sign("key", &raw),
        )
        .header(HEADER_GATEWAY_CREDENTIAL, "key")
        .body(Body::empty())
        .unwrap();
    request
        .extensions_mut()
        .insert(crate::gateway_protocol::AuthenticatedGateway {
            pairing_id: "pairing-1".into(),
            label: "Gateway A".into(),
            may_manage: false,
            may_control: false,
            may_admin: true,
            remote_starts: Default::default(),
        });
    let response = receiver.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(
        bytes,
        crate::relay::namespaced_device_id("pairing-1", "device-1").as_bytes()
    );
}
