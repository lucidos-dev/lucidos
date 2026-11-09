use super::*;
use tower::ServiceExt as _;

async fn call(
    state: &GatewayState,
    method: &str,
    path: &str,
    body: Option<serde_json::Value>,
    cookie: Option<&str>,
) -> Response {
    let mut builder = axum::extract::Request::builder()
        .method(method)
        .uri(path)
        .header(header::CONTENT_TYPE, "application/json");
    builder = match cookie {
        Some(cookie) => builder.header(header::COOKIE, cookie),
        None => builder.header(auth::HEADER_LOCAL_TOKEN, "test-local-token"),
    };
    let body = body.map(|b| b.to_string()).unwrap_or_default();
    crate::server::gateway_router(state.clone())
        .oneshot(builder.body(Body::from(body)).unwrap())
        .await
        .unwrap()
}

async fn operators(response: Response) -> OperatorsBody {
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

fn seeded() -> GatewayState {
    let state = GatewayState::for_tests();
    state
        .seed_device_for_test(auth::PairedDevice {
            id: "device-1".into(),
            label: "My iPhone".into(),
            credential_digest: auth::digest("cred-phone"),
            paired_at: "2020-01-01T00:00:00Z".into(),
            last_seen_at: None,
        })
        .unwrap();
    state
}

#[tokio::test]
async fn people_are_added_named_given_emails_and_handed_devices() {
    let state = seeded();
    let body = operators(call(&state, "GET", "/~/api/v1/auth/operators", None, None).await).await;
    assert_eq!(body.operators.len(), 1);
    assert_eq!(body.operators[0].device_ids, vec!["device-1"]);

    let body = operators(
        call(
            &state,
            "POST",
            "/~/api/v1/auth/operators",
            Some(serde_json::json!({ "name": "Sam" })),
            None,
        )
        .await,
    )
    .await;
    let sam = body.operators[1].id.clone();
    let body = operators(
        call(
            &state,
            "POST",
            &format!("/~/api/v1/auth/operators/{sam}/emails"),
            Some(serde_json::json!({ "email": "Sam@Example.com" })),
            None,
        )
        .await,
    )
    .await;
    assert_eq!(body.operators[1].emails, vec!["sam@example.com"]);
    let body = operators(
        call(
            &state,
            "PUT",
            "/~/api/v1/auth/devices/device-1/operator",
            Some(serde_json::json!({ "operator_id": sam })),
            None,
        )
        .await,
    )
    .await;
    assert_eq!(body.operators[1].device_ids, vec!["device-1"]);
    assert_eq!(state.replayed_for_test(), state.directory());
}

#[tokio::test]
async fn a_bad_change_is_the_caller_s_mistake() {
    let state = seeded();
    let response = call(
        &state,
        "POST",
        "/~/api/v1/auth/operators",
        Some(serde_json::json!({ "name": "  " })),
        None,
    )
    .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let response = call(
        &state,
        "PUT",
        "/~/api/v1/auth/devices/device-1/operator",
        Some(serde_json::json!({ "operator_id": "nobody" })),
        None,
    )
    .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn any_paired_device_may_change_people_including_ones_it_is_not() {
    // Identity without authority: nothing branches on who the caller is.
    let state = seeded();
    let cookie = format!("{}=cred-phone", state.device_cookie_name());
    let response = call(
        &state,
        "POST",
        "/~/api/v1/auth/operators",
        Some(serde_json::json!({ "name": "Sam" })),
        Some(&cookie),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn removing_your_own_person_revokes_you_and_clears_your_cookie() {
    let state = seeded();
    let owner = state.directory().operators[0].id.clone();
    let cookie = format!("{}=cred-phone", state.device_cookie_name());
    let response = call(
        &state,
        "DELETE",
        &format!("/~/api/v1/auth/operators/{owner}"),
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let cleared = response
        .headers()
        .get(header::SET_COOKIE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    assert!(cleared.contains("Max-Age=0"), "{cleared}");
    assert!(state.paired_devices().devices.is_empty());
}

#[tokio::test]
async fn a_pairing_code_names_its_person_and_refuses_a_stranger() {
    let state = seeded();
    let response = call(
        &state,
        "POST",
        "/~/api/v1/auth/pairing-code?operator=nobody",
        None,
        None,
    )
    .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    let sam = operators(
        call(
            &state,
            "POST",
            "/~/api/v1/auth/operators",
            Some(serde_json::json!({ "name": "Sam" })),
            None,
        )
        .await,
    )
    .await
    .operators[1]
        .id
        .clone();
    let minted = call(
        &state,
        "POST",
        &format!("/~/api/v1/auth/pairing-code?operator={sam}"),
        None,
        None,
    )
    .await;
    assert_eq!(minted.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(minted.into_body(), usize::MAX)
        .await
        .unwrap();
    let secret = serde_json::from_slice::<serde_json::Value>(&bytes).unwrap()["secret"]
        .as_str()
        .unwrap()
        .to_string();
    state
        .redeem_pairing(
            &secret,
            crate::pairing::PairingChannel::Public,
            Some("Laptop"),
        )
        .unwrap();
    assert_eq!(state.directory().devices_of(&sam).len(), 1);
}
