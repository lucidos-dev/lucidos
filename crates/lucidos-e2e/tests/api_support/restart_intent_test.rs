use crate::support::{as_stamped_device, base_url, http_client, register_device};

const DEVICE: &str = "e2e-restart-intent-device";

#[tokio::test]
async fn restart_intent_requires_a_device_and_refuses_a_proxied_caller() {
    let client = http_client();
    let url = format!("{}/api/v1/internal/restart-intent", base_url());
    register_device(DEVICE).await;

    let no_device = client
        .post(&url)
        .send()
        .await
        .expect("request to /api/v1/internal/restart-intent failed");
    assert_eq!(
        no_device.status(),
        401,
        "a caller with no identity must be refused, not stashed as an Api actor"
    );

    let proxied = client
        .post(&url)
        .header("x-forwarded-prefix", "/e2e-test/")
        .header("x-lucidos-device-id", DEVICE)
        .send()
        .await
        .expect("request to /api/v1/internal/restart-intent failed");
    assert_eq!(
        proxied.status(),
        403,
        "a request carrying the gateway's forwarded prefix must be refused"
    );

    let accepted = as_stamped_device(client.post(&url), DEVICE)
        .send()
        .await
        .expect("request to /api/v1/internal/restart-intent failed");
    assert_eq!(
        accepted.status(),
        204,
        "a direct call naming a device must be accepted"
    );

    let health = client
        .get(format!("{}/api/v1/health", base_url()))
        .send()
        .await
        .expect("health request failed");
    assert!(
        health.status().is_success(),
        "restart-intent must not restart anything"
    );
}
