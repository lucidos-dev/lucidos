use super::*;
use crate::auth;
use axum::body::Body;
use axum::http::{header, StatusCode};
use axum::response::Response;
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

async fn names(response: Response) -> DeviceNamesBody {
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
            id: "paired-1".into(),
            label: "My iPhone".into(),
            credential_digest: auth::digest("cred-phone"),
            paired_at: "2020-01-01T00:00:00Z".into(),
            last_seen_at: None,
        })
        .unwrap();
    state
}

fn identity(body: &DeviceNamesBody, id: &str) -> DeviceIdentity {
    body.devices
        .iter()
        .find(|d| d.id == id)
        .cloned()
        .unwrap_or_else(|| panic!("the gateway names no device {id}"))
}

fn import_body(rows: &[(&str, Option<&str>, Option<&str>)]) -> serde_json::Value {
    serde_json::json!({
        "devices": rows
            .iter()
            .map(|(id, name, label)| serde_json::json!({
                "device_id": id, "name": name, "pairing_label": label,
            }))
            .collect::<Vec<_>>(),
    })
}

const IMPORT: &str = "/~/api/v1/auth/device-names/import";
const LIST: &str = "/~/api/v1/auth/device-names";
/// INVARIANT: Pins the wire limit a gateway accepts for a device name.
const MAX_IMPORTED_FOR_TEST: usize = 80;

#[tokio::test]
async fn names_from_two_workspaces_read_back_from_the_gateway() {
    let state = seeded();
    let first = import_body(&[
        ("paired-1", Some("Kitchen iPad"), Some("My iPhone")),
        ("local-a", Some("Desk Mac"), None),
    ]);
    let second = import_body(&[
        ("local-b", None, Some("Laptop")),
        ("relayed:d1", Some("Studio phone"), None),
        ("unnamed", None, None),
    ]);
    names(call(&state, "POST", IMPORT, Some(first), None).await).await;
    names(call(&state, "POST", IMPORT, Some(second), None).await).await;

    let body = names(call(&state, "GET", LIST, None, None).await).await;
    assert_eq!(
        identity(&body, "paired-1"),
        DeviceIdentity {
            id: "paired-1".into(),
            name: Some("Kitchen iPad".into()),
            label: Some("My iPhone".into()),
        }
    );
    assert_eq!(identity(&body, "local-a").name.as_deref(), Some("Desk Mac"));
    assert_eq!(
        identity(&body, "local-b").name.as_deref(),
        Some("Laptop"),
        "a label the gateway does not know is kept as the name"
    );
    assert_eq!(
        identity(&body, "relayed:d1").name.as_deref(),
        Some("Studio phone")
    );
    assert!(body.devices.iter().all(|d| d.id != "unnamed"));

    let replayed = state.replayed_for_test();
    assert_eq!(replayed.device_identities(), body.devices);
}

#[tokio::test]
async fn the_first_imported_name_wins_and_a_rename_survives_a_late_import() {
    let state = seeded();
    let import = |name: &str| import_body(&[("local-a", Some(name), None)]);
    names(call(&state, "POST", IMPORT, Some(import("Desk Mac")), None).await).await;
    names(call(&state, "POST", IMPORT, Some(import("Old name")), None).await).await;
    let body = names(call(&state, "GET", LIST, None, None).await).await;
    assert_eq!(identity(&body, "local-a").name.as_deref(), Some("Desk Mac"));

    let renamed = serde_json::json!({ "name": "Office Mac" });
    names(
        call(
            &state,
            "PUT",
            "/~/api/v1/auth/devices/local-a/name",
            Some(renamed),
            None,
        )
        .await,
    )
    .await;
    let body = names(call(&state, "POST", IMPORT, Some(import("Desk Mac")), None).await).await;
    assert_eq!(
        identity(&body, "local-a").name.as_deref(),
        Some("Office Mac")
    );
}

#[tokio::test]
async fn a_cleared_name_survives_a_late_import_and_a_long_name_is_cut() {
    let state = seeded();
    let long = "x".repeat(MAX_IMPORTED_FOR_TEST + 20);
    let first = import_body(&[
        ("local-a", Some("Desk Mac"), None),
        ("local-b", Some(&long), None),
    ]);
    names(call(&state, "POST", IMPORT, Some(first), None).await).await;
    let cleared = serde_json::json!({ "name": "" });
    names(
        call(
            &state,
            "PUT",
            "/~/api/v1/auth/devices/local-a/name",
            Some(cleared),
            None,
        )
        .await,
    )
    .await;
    let late = import_body(&[("local-a", Some("Desk Mac"), None)]);
    let body = names(call(&state, "POST", IMPORT, Some(late), None).await).await;
    assert!(
        body.devices
            .iter()
            .all(|d| d.id != "local-a" || d.name.is_none()),
        "a late import never restores a name a user cleared"
    );
    assert_eq!(
        identity(&body, "local-b").name.map(|n| n.chars().count()),
        Some(MAX_IMPORTED_FOR_TEST),
        "a legacy name past the limit is cut, never lost"
    );
    assert_eq!(state.replayed_for_test().device_identities(), body.devices);
}

#[tokio::test]
async fn a_paired_label_never_becomes_a_name() {
    let state = seeded();
    let body = names(
        call(
            &state,
            "POST",
            IMPORT,
            Some(import_body(&[("paired-1", None, Some("Stale label"))])),
            None,
        )
        .await,
    )
    .await;
    assert_eq!(identity(&body, "paired-1").name, None);
    assert_eq!(
        identity(&body, "paired-1").label.as_deref(),
        Some("My iPhone")
    );
}

#[tokio::test]
async fn a_device_may_rename_and_clear_but_never_import() {
    let state = seeded();
    let cookie = format!("{}=cred-phone", state.device_cookie_name());
    let named = names(
        call(
            &state,
            "PUT",
            "/~/api/v1/auth/devices/paired-1/name",
            Some(serde_json::json!({ "name": "  Hall phone  " })),
            Some(&cookie),
        )
        .await,
    )
    .await;
    assert_eq!(
        identity(&named, "paired-1").name.as_deref(),
        Some("Hall phone")
    );
    let cleared = names(
        call(
            &state,
            "PUT",
            "/~/api/v1/auth/devices/paired-1/name",
            Some(serde_json::json!({ "name": " " })),
            Some(&cookie),
        )
        .await,
    )
    .await;
    assert_eq!(identity(&cleared, "paired-1").name, None);

    let refused = call(
        &state,
        "POST",
        IMPORT,
        Some(import_body(&[("local-a", Some("Planted"), None)])),
        Some(&cookie),
    )
    .await;
    assert_eq!(refused.status(), StatusCode::FORBIDDEN);
    assert!(state.directory().device_names.is_empty());
}

#[tokio::test]
async fn revoking_a_device_forgets_its_name() {
    let state = seeded();
    names(
        call(
            &state,
            "PUT",
            "/~/api/v1/auth/devices/paired-1/name",
            Some(serde_json::json!({ "name": "Hall phone" })),
            None,
        )
        .await,
    )
    .await;
    assert!(state.revoke_device("paired-1").unwrap());
    assert!(state.directory().device_identities().is_empty());
}

#[tokio::test]
async fn an_overlong_name_is_refused() {
    let state = seeded();
    let response = call(
        &state,
        "PUT",
        "/~/api/v1/auth/devices/paired-1/name",
        Some(serde_json::json!({ "name": "x".repeat(81) })),
        None,
    )
    .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}
