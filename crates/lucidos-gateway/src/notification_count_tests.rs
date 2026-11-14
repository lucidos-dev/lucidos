use axum::body::Body;
use axum::http::{header, HeaderMap, Request, StatusCode};
use axum::{routing::get, Json, Router};
use serde_json::{json, Value};
use tower::ServiceExt as _;

use crate::auth;
use crate::engine_hop::test_engine;
use crate::server::GatewayState;

/// An engine whose shared unread count differs from each authenticated inbox.
pub(crate) async fn install_inbox(state: &GatewayState, first_device: String) -> tempfile::TempDir {
    let dir = test_engine::scratch();
    let key = lucidos_gateway_stamp::StampKey::generate().unwrap();
    state.set_stamp_key_for_test("dev", key);
    let key = state.stamp_key("dev").unwrap();
    let app = Router::new().route(
        "/api/v1/notifications",
        get(move |headers: HeaderMap| {
            let key = key.clone();
            let first_device = first_device.clone();
            async move {
                let id = headers
                    .get(crate::stack::HEADER_DEVICE_ID)
                    .and_then(|v| v.to_str().ok());
                let count = match id {
                    None => 12,
                    Some(id) => {
                        let stamp = headers
                            .get(lucidos_gateway_stamp::HEADER_GATEWAY_STAMP)
                            .and_then(|v| v.to_str().ok())
                            .expect("an authenticated device probe is stamped");
                        assert!(key.verify_now(
                            stamp,
                            lucidos_gateway_stamp::Claim::Device {
                                workspace_id: "dev",
                                device_id: id,
                            }
                        ));
                        if id == first_device {
                            0
                        } else {
                            3
                        }
                    }
                };
                Json(json!({"notifications": [], "unread_count": count, "has_more": false}))
            }
        }),
    );
    state.set_route_for_test("dev", test_engine::serve(&dir, app));
    state.install_notification_stack_for_test(dir.path()).await;
    dir
}

pub(crate) async fn listing(state: &GatewayState, credential: &str) -> Value {
    let response = crate::server::gateway_router(state.clone())
        .oneshot(
            Request::builder()
                .uri("/~/api/v1/control/workspaces")
                .header(
                    header::COOKIE,
                    format!("{}={credential}", state.device_cookie_name()),
                )
                .header(crate::stack::HEADER_DEVICE_ID, "spoofed-device")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    serde_json::from_slice(
        &axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap()
}

fn seed_device(state: &GatewayState, id: &str, credential: &str) {
    state
        .seed_device_for_test(auth::PairedDevice {
            id: id.into(),
            label: id.into(),
            credential_digest: auth::digest(credential),
            paired_at: "2020-01-01T00:00:00Z".into(),
            last_seen_at: None,
        })
        .unwrap();
}

#[tokio::test]
async fn local_listing_uses_each_authenticated_inbox_without_changing_the_shared_cache() {
    let root = tempfile::tempdir().unwrap();
    let state = GatewayState::for_tests_with_static_dir(Some(root.path().to_path_buf()));
    seed_device(&state, "first", "first-secret");
    seed_device(&state, "second", "second-secret");
    let _engine = install_inbox(&state, "first".into()).await;
    let (first, second) = tokio::join!(
        listing(&state, "first-secret"),
        listing(&state, "second-secret")
    );
    assert_eq!(first["workspaces"][0]["unread_count"], 0);
    assert_eq!(second["workspaces"][0]["unread_count"], 3);
    assert_eq!(state.list_status().await[0].unread_count, Some(12));
}

#[tokio::test]
async fn a_failed_personal_probe_omits_the_count_instead_of_using_the_shared_cache() {
    let root = tempfile::tempdir().unwrap();
    let state = GatewayState::for_tests_with_static_dir(Some(root.path().to_path_buf()));
    seed_device(&state, "first", "first-secret");
    let _engine = install_inbox(&state, "first".into()).await;
    let dead = test_engine::scratch();
    state.set_route_for_test("dev", test_engine::unreachable(&dead));
    let rows = listing(&state, "first-secret").await;
    assert!(rows["workspaces"][0].get("unread_count").is_none());
    assert_eq!(rows["workspaces"][0]["health"], "healthy");
}

async fn total(
    state: &GatewayState,
    query: &str,
    credential: Option<&str>,
    local: bool,
) -> (StatusCode, Value) {
    let mut request = Request::builder()
        .uri(format!("/~/api/v1/control/unread-total{query}"))
        .header(crate::stack::HEADER_DEVICE_ID, "second");
    if let Some(credential) = credential {
        request = request.header(
            header::COOKIE,
            format!("{}={credential}", state.device_cookie_name()),
        );
    }
    if local {
        request = request.header(auth::HEADER_LOCAL_TOKEN, "test-local-token");
    }
    let response = crate::server::gateway_router(state.clone())
        .oneshot(request.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, serde_json::from_slice(&body).unwrap_or(Value::Null))
}

#[tokio::test]
async fn background_totals_select_only_known_recipients_and_keep_desktop_aggregate() {
    let root = tempfile::tempdir().unwrap();
    let state = GatewayState::for_tests_with_static_dir(Some(root.path().to_path_buf()));
    seed_device(&state, "first", "first-secret");
    seed_device(&state, "second", "second-secret");
    let _engine = install_inbox(&state, "first".into()).await;
    for (query, expected) in [
        ("?recipient-device-id=first", json!(0)),
        ("?recipient-device-id=second", json!(3)),
        ("?recipient-device-id=revoked", Value::Null),
        ("?recipient-device-id=", Value::Null),
        // This fixture's supervisor socket is unreachable; the desktop sum stays zero.
        ("", json!(0)),
    ] {
        let (status, body) = total(&state, query, None, true).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["total"], expected, "{query}");
    }
    let dead = test_engine::scratch();
    state.set_route_for_test("dev", test_engine::unreachable(&dead));
    assert_eq!(
        total(&state, "?recipient-device-id=first", None, true)
            .await
            .1["total"],
        Value::Null
    );
}

#[tokio::test]
async fn browsers_cannot_select_background_recipients_or_use_the_shared_total() {
    let root = tempfile::tempdir().unwrap();
    let state = GatewayState::for_tests_with_static_dir(Some(root.path().to_path_buf()));
    seed_device(&state, "first", "first-secret");
    let _engine = install_inbox(&state, "first".into()).await;
    assert_eq!(
        total(&state, "", Some("first-secret"), false).await.1["total"],
        0
    );
    assert_eq!(
        total(
            &state,
            "?recipient-device-id=second",
            Some("first-secret"),
            false
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        total(&state, "?recipient-device-id=first", None, false)
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn keyless_engines_never_supply_a_personal_background_count() {
    let root = tempfile::tempdir().unwrap();
    let state = GatewayState::for_tests_with_static_dir(Some(root.path().to_path_buf()));
    seed_device(&state, "first", "first-secret");
    state.install_notification_stack_for_test(root.path()).await;
    assert_eq!(
        total(&state, "?recipient-device-id=first", None, true)
            .await
            .1["total"],
        Value::Null
    );
}
