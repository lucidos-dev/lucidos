use super::*;
use axum::body::Body;
use axum::http::StatusCode;
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

async fn users(response: Response) -> UsersBody {
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

async fn wire(response: Response) -> serde_json::Value {
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

#[tokio::test]
async fn an_old_page_can_manage_users_and_read_device_membership() {
    let state = seeded();
    let body = wire(
        call(
            &state,
            "POST",
            "/~/api/v1/auth/operators",
            Some(serde_json::json!({"name": "Sam"})),
            None,
        )
        .await,
    )
    .await;
    assert!(body.get("users").is_none());
    let sam = body["operators"][1]["id"].as_str().unwrap();
    let body = wire(
        call(
            &state,
            "PUT",
            "/~/api/v1/auth/devices/device-1/operator",
            Some(serde_json::json!({"operator_id": sam})),
            None,
        )
        .await,
    )
    .await;
    assert_eq!(body["operators"][1]["device_ids"][0], "device-1");
    let devices = wire(call(&state, "GET", "/~/api/v1/auth/devices", None, None).await).await;
    assert_eq!(devices[0]["operator_id"], sam);
    assert_eq!(devices[0]["user_id"], sam);
    let users = wire(call(&state, "GET", "/~/api/v1/auth/users", None, None).await).await;
    assert_eq!(users["users"][1]["device_ids"][0], "device-1");
    assert!(users.get("operators").is_none());
}

#[tokio::test]
async fn an_old_engine_reads_relayed_identity_under_its_wire_names() {
    let state = seeded();
    state
        .record(|d| {
            Ok(d.relayed_seen(crate::gateway_event_log::RelayedDevice {
                device_id: "pair:device".into(),
                user_id: "pair:user".into(),
                name: "Sam".into(),
                via: "Laptop".into(),
                emails: vec!["sam@example.com".into()],
            })
            .into_iter()
            .collect())
        })
        .unwrap()
        .unwrap();
    let old = wire(call(&state, "GET", "/~/api/v1/auth/operators", None, None).await).await;
    assert_eq!(old["relayed"][0]["operator_id"], "pair:user");
    assert!(old["relayed"][0].get("user_id").is_none());
    assert_eq!(old["relayed"][0]["emails"][0], "sam@example.com");
}

#[tokio::test]
async fn removing_your_own_user_through_the_old_route_keeps_its_cookie_clear() {
    let state = seeded();
    let owner = state.directory().users[0].id.clone();
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
    assert!(response.headers()[header::SET_COOKIE]
        .to_str()
        .unwrap()
        .contains("Max-Age=0"));
    let old = wire(response).await;
    assert!(old["operators"].as_array().unwrap().is_empty());
    assert!(state.paired_devices().devices.is_empty());
}

#[tokio::test]
async fn old_and_dual_pairing_queries_keep_the_selected_user() {
    for query in ["operator={id}", "user={id}&operator={id}"] {
        let state = seeded();
        state.record(|d| d.add_user("Sam")).unwrap().unwrap();
        let sam = state.directory().users[1].id.clone();
        let cookie = format!("{}=cred-phone", state.device_cookie_name());
        let query = query.replace("{id}", &sam);
        let body = wire(
            call(
                &state,
                "POST",
                &format!("/~/api/v1/auth/pairing-code?{query}"),
                None,
                Some(&cookie),
            )
            .await,
        )
        .await;
        let outcome = state
            .redeem_pairing(
                body["secret"].as_str().unwrap(),
                crate::pairing::PairingChannel::Public,
                Some("Laptop"),
            )
            .unwrap();
        assert!(matches!(outcome, crate::server::PairingOutcome::Paired(_)));
        assert_eq!(state.directory().devices_of(&sam).len(), 1);
    }
}

#[tokio::test]
async fn conflicting_old_and_new_identity_fields_are_refused() {
    let state = seeded();
    let response = call(
        &state,
        "POST",
        "/~/api/v1/auth/pairing-code?user=one&operator=two",
        None,
        None,
    )
    .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let response = call(
        &state,
        "PUT",
        "/~/api/v1/auth/devices/device-1/user",
        Some(serde_json::json!({"user_id":"one", "operator_id":"two"})),
        None,
    )
    .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn the_legacy_user_routes_keep_the_auth_plane_gate() {
    let state = seeded();
    let response = crate::server::gateway_router(state)
        .oneshot(
            axum::extract::Request::builder()
                .uri("/~/api/v1/auth/operators")
                .header(auth::HEADER_LOCAL_TOKEN, "test-local-token")
                .header(header::HOST, "localhost")
                .header(header::REFERER, "http://localhost/dev/app/example/")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn users_are_added_named_given_emails_and_handed_devices() {
    let state = seeded();
    let body = users(call(&state, "GET", "/~/api/v1/auth/users", None, None).await).await;
    assert_eq!(body.users.len(), 1);
    assert_eq!(body.users[0].device_ids, vec!["device-1"]);

    let body = users(
        call(
            &state,
            "POST",
            "/~/api/v1/auth/users",
            Some(serde_json::json!({ "name": "Sam" })),
            None,
        )
        .await,
    )
    .await;
    let sam = body.users[1].id.clone();
    let body = users(
        call(
            &state,
            "POST",
            &format!("/~/api/v1/auth/users/{sam}/emails"),
            Some(serde_json::json!({ "email": "Sam@Example.com" })),
            None,
        )
        .await,
    )
    .await;
    assert_eq!(body.users[1].emails, vec!["sam@example.com"]);
    let body = users(
        call(
            &state,
            "PUT",
            "/~/api/v1/auth/devices/device-1/user",
            Some(serde_json::json!({ "user_id": sam })),
            None,
        )
        .await,
    )
    .await;
    assert_eq!(body.users[1].device_ids, vec!["device-1"]);
    assert_eq!(state.replayed_for_test(), state.directory());
}

#[tokio::test]
async fn a_bad_change_is_the_caller_s_mistake() {
    let state = seeded();
    let response = call(
        &state,
        "POST",
        "/~/api/v1/auth/users",
        Some(serde_json::json!({ "name": "  " })),
        None,
    )
    .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let response = call(
        &state,
        "PUT",
        "/~/api/v1/auth/devices/device-1/user",
        Some(serde_json::json!({ "user_id": "nobody" })),
        None,
    )
    .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn any_paired_device_may_change_users_including_ones_it_is_not() {
    // Identity without authority: nothing branches on who the caller is.
    let state = seeded();
    let cookie = format!("{}=cred-phone", state.device_cookie_name());
    let response = call(
        &state,
        "POST",
        "/~/api/v1/auth/users",
        Some(serde_json::json!({ "name": "Sam" })),
        Some(&cookie),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn removing_your_own_user_revokes_you_and_clears_your_cookie() {
    let state = seeded();
    let owner = state.directory().users[0].id.clone();
    let cookie = format!("{}=cred-phone", state.device_cookie_name());
    let response = call(
        &state,
        "DELETE",
        &format!("/~/api/v1/auth/users/{owner}"),
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
async fn a_pairing_code_names_its_user_and_refuses_a_stranger() {
    let state = seeded();
    let response = call(
        &state,
        "POST",
        "/~/api/v1/auth/pairing-code?user=nobody",
        None,
        None,
    )
    .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    let sam = users(
        call(
            &state,
            "POST",
            "/~/api/v1/auth/users",
            Some(serde_json::json!({ "name": "Sam" })),
            None,
        )
        .await,
    )
    .await
    .users[1]
        .id
        .clone();
    // The paired phone mints, so the redeem needs no Allow.
    let cookie = format!("{}=cred-phone", state.device_cookie_name());
    let minted = call(
        &state,
        "POST",
        &format!("/~/api/v1/auth/pairing-code?user={sam}"),
        None,
        Some(&cookie),
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
