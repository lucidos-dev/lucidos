//! E2E coverage for the per-workspace engine Network access endpoint
//! (`GET`/`PUT /api/v1/network-config`).
//!
//! The PUT writes only this workspace's `network_bind` preference (isolated to
//! the workspace DB), so it never mutates the machine's `~/.lucidos/network.toml`
//! — that file (the gateway bind + the engine-inherit toggle) is owned by the
//! gateway control plane, not this endpoint. The test resets to `loopback` at the
//! end so the e2e workspace is left at the safe default.

use crate::support::{base_url, user_client};
use serde_json::{json, Value};

#[tokio::test]
async fn network_config_roundtrips_and_rejects_garbage() {
    let client = user_client().await;
    let api = base_url();

    // GET has the expected shape.
    let resp = client
        .get(format!("{}/api/v1/network-config", api))
        .send()
        .await
        .expect("get failed");
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert!(
        body.get("engine_bind").and_then(Value::as_str).is_some(),
        "engine_bind missing: {body}"
    );
    assert!(body.get("inherit").and_then(Value::as_bool).is_some());
    assert!(body.get("gateway_bind").and_then(Value::as_str).is_some());
    // Present as null or a string — just assert the key exists (best-effort hint).
    assert!(body
        .as_object()
        .unwrap()
        .contains_key("detected_tailscale_ip"));

    // PUT garbage → 400 (server-side validation, fail-safe).
    let resp = client
        .put(format!("{}/api/v1/network-config", api))
        .json(&json!({ "engine_bind": "not-an-ip" }))
        .send()
        .await
        .expect("put garbage failed");
    assert_eq!(resp.status(), 400, "garbage bind must be rejected");

    // PUT a valid IP → success; GET reflects it.
    let resp = client
        .put(format!("{}/api/v1/network-config", api))
        .json(&json!({ "engine_bind": "100.64.0.1" }))
        .send()
        .await
        .expect("put ip failed");
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.json::<Value>().await.unwrap()["success"], true);

    let body: Value = client
        .get(format!("{}/api/v1/network-config", api))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(body["engine_bind"], "100.64.0.1");

    // Reset to the safe default so the e2e workspace is left at loopback.
    let resp = client
        .put(format!("{}/api/v1/network-config", api))
        .json(&json!({ "engine_bind": "loopback" }))
        .send()
        .await
        .expect("reset failed");
    assert_eq!(resp.status(), 200);
    let body: Value = client
        .get(format!("{}/api/v1/network-config", api))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(body["engine_bind"], "loopback");
}
