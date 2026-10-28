use super::*;
use crate::relay::tests::{paired, Paired};
use crate::remote_gateways::tests::{localhost_cert, serve_https};
use axum::http::HeaderMap;
use tower::ServiceExt as _;

fn engine_nudges(p: &Paired) -> Vec<String> {
    p.seen
        .lock()
        .unwrap()
        .iter()
        .filter(|line| line.starts_with(&format!("post {ENGINE_NUDGE_PATH}\n")))
        .cloned()
        .collect()
}

async fn post_to(state: &GatewayState, path: &str, headers: HeaderMap) -> StatusCode {
    let mut req = Request::post(path).body(Body::empty()).unwrap();
    *req.headers_mut() = headers;
    crate::server::gateway_router(state.clone())
        .oneshot(req)
        .await
        .unwrap()
        .status()
}

fn credential(value: &str) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(HEADER_GATEWAY_CREDENTIAL, value.parse().unwrap());
    headers
}

/// A landed: B's engine hears it, as the gateway, with B's local token.
#[tokio::test]
async fn a_landing_on_a_wakes_every_engine_b_routes() {
    let p = paired().await;
    let answers = nudge_remote_gateways(&p.a).await;
    assert_eq!(answers, [("cloud".to_string(), Ok(NudgeAnswer::Delivered))]);
    let nudges = engine_nudges(&p);
    assert_eq!(nudges.len(), 1, "{nudges:?}");
    let token = format!("{}: {}\n", auth::HEADER_LOCAL_TOKEN, p.b.local_token()).to_lowercase();
    assert!(nudges[0].contains(&token), "{}", nudges[0]);
}

/// Only a current pairing's credential nudges B.
#[tokio::test]
async fn b_takes_a_nudge_only_from_a_paired_gateway() {
    let p = paired().await;
    let path = format!("{GATEWAY_API_PREFIX}{NUDGE_PATH}");
    for refused in [HeaderMap::new(), credential("not-the-credential")] {
        assert_eq!(
            post_to(&p.b, &path, refused).await,
            StatusCode::UNAUTHORIZED
        );
    }
    assert!(engine_nudges(&p).is_empty());
    assert_eq!(
        post_to(&p.b, &path, credential(&p.credential)).await,
        StatusCode::ACCEPTED
    );
    assert_eq!(engine_nudges(&p).len(), 1);
}

/// The nudge follows the pairing: B's credential nudges nothing on A.
#[tokio::test]
async fn the_nudge_is_one_way() {
    let p = paired().await;
    let path = format!("{GATEWAY_API_PREFIX}{NUDGE_PATH}");
    assert_eq!(
        post_to(&p.a, &path, credential(&p.credential)).await,
        StatusCode::UNAUTHORIZED
    );
}

/// An older B lacks the call. A hears a 404 and shows nobody an error.
#[tokio::test]
async fn an_older_gateway_without_the_nudge_is_skipped_quietly() {
    let p = paired().await;
    let cert = localhost_cert();
    let port = serve_https(axum::Router::new(), &cert).await;
    p.a.relay_client()
        .trust_for_test(cert.cert.pem().as_bytes());
    p.a.remote_gateways().remove("cloud").unwrap();
    p.a.remote_gateways()
        .add(RemoteGateway {
            name: "older".to_string(),
            address: format!("https://localhost:{port}"),
            pairing_id: p.pairing_id.clone(),
            credential: p.credential.clone(),
            gateway_protocol: 1,
            added_at: "2020-01-01T00:00:00Z".to_string(),
        })
        .unwrap();
    let answers = nudge_remote_gateways(&p.a).await;
    assert_eq!(
        answers,
        [("older".to_string(), Ok(NudgeAnswer::Unsupported))]
    );
}

/// A's engine reports a landing on the control plane, as a local process.
#[tokio::test]
async fn an_engine_reports_a_landing_with_the_local_token() {
    let p = paired().await;
    let path = format!("/~/api/v1/control{MAIN_LANDED_PATH}");
    assert_eq!(
        post_to(&p.a, &path, HeaderMap::new()).await,
        StatusCode::UNAUTHORIZED
    );
    let mut local = HeaderMap::new();
    local.insert(auth::HEADER_LOCAL_TOKEN, p.a.local_token().parse().unwrap());
    assert_eq!(post_to(&p.a, &path, local).await, StatusCode::ACCEPTED);
}

async fn paired_answer(state: &GatewayState) -> bool {
    let mut req = Request::get(format!("/~/api/v1/control{GATEWAY_PAIRINGS_PATH}"))
        .body(Body::empty())
        .unwrap();
    req.headers_mut().insert(
        auth::HEADER_LOCAL_TOKEN,
        state.local_token().parse().unwrap(),
    );
    let response = crate::server::gateway_router(state.clone())
        .oneshot(req)
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
    body["paired"].as_bool().unwrap()
}

/// Either side of a pairing reads as paired. A lone gateway does not.
#[tokio::test]
async fn both_ends_of_a_pairing_answer_paired() {
    assert!(!paired_answer(&GatewayState::for_tests()).await);
    let p = paired().await;
    assert!(paired_answer(&p.a).await, "A added a remote gateway");
    assert!(paired_answer(&p.b).await, "A redeemed B's code");
    p.a.remote_gateways().remove("cloud").unwrap();
    assert!(!paired_answer(&p.a).await, "A removed its only one");
}
