use crate::support::{base_url, user_client};
use serde_json::{json, Value};

#[tokio::test]
async fn network_config_roundtrips_and_rejects_garbage() {
    let client = user_client().await;
    let api = base_url();

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
    assert!(body
        .as_object()
        .unwrap()
        .contains_key("detected_tailscale_ip"));

    let resp = client
        .put(format!("{}/api/v1/network-config", api))
        .json(&json!({ "engine_bind": "not-an-ip" }))
        .send()
        .await
        .expect("put garbage failed");
    assert_eq!(resp.status(), 400, "garbage bind must be rejected");

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
