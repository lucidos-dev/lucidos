//! Reload-safe hold status and the authenticated manual clear contract.
use crate::support::{base_url, http_client, user_client};

#[tokio::test]
async fn spend_cap_status_and_clear_require_a_known_provider_and_actor() {
    let client = user_client().await;
    let base = format!("{}/api/v1/models/spend-cap-holds", base_url());
    let response = client.get(&base).send().await.unwrap();
    assert_eq!(response.status(), 200);
    let holds: serde_json::Value = response.json().await.unwrap();
    assert!(holds.is_array(), "{holds}");
    for hold in holds.as_array().unwrap() {
        assert!(hold["provider"].is_string());
        assert!(hold["held_until"].is_string());
        assert!(hold["message"].is_string());
    }
    // Clearing an idle provider is idempotent and emits no invented hold end.
    let response = client
        .post(format!("{base}/local/clear"))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let response = client
        .post(format!("{base}/unknown-provider/clear"))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 400);
    let response = http_client()
        .post(format!("{base}/local/clear"))
        .send()
        .await
        .unwrap();
    assert!(matches!(response.status().as_u16(), 401 | 403));
}
