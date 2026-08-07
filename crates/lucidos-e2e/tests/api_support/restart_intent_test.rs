use crate::support::{base_url, http_client};

/// `POST /api/v1/internal/restart-intent` is how the workspace gateway tells an
/// engine that a human asked for the teardown it is about to signal, so the
/// picker's Restart / Stop settles in-flight threads at `paused` with
/// "Paused by restart" instead of the crash-shaped `failed` / "Response
/// interrupted" they used to get.
///
/// The two refusals are the interesting half and are asserted first, because
/// they have no side effect: only the accepted call stashes anything.
#[tokio::test]
async fn restart_intent_requires_a_device_and_refuses_a_proxied_caller() {
    let client = http_client();
    let url = format!("{}/api/v1/internal/restart-intent", base_url());

    // No device to name. `user_actor_resolved` would fall back to an `Api`
    // actor, which is not the switch fingerprint: it would resume nothing while
    // replacing the honest System attribution. Absent attribution stays absent.
    let no_device = client
        .post(&url)
        .send()
        .await
        .expect("request to /api/v1/internal/restart-intent failed");
    assert_eq!(
        no_device.status(),
        400,
        "a caller with no device id must be refused, not stashed as an Api actor"
    );

    // Arrived through the gateway proxy, i.e. from a browser. The gateway
    // strips a client-supplied `x-forwarded-prefix` and injects its own, so this
    // header's presence is a forge-proof "a page sent this". A page must not be
    // able to set the engine's restart actor: that is what defeats the
    // crash-loop protection behind cause-gated resume.
    let proxied = client
        .post(&url)
        .header("x-forwarded-prefix", "/e2e-test/")
        .header("x-lucidos-device-id", "e2e-restart-intent-device")
        .send()
        .await
        .expect("request to /api/v1/internal/restart-intent failed");
    assert_eq!(
        proxied.status(),
        403,
        "a request carrying the gateway's forwarded prefix must be refused"
    );

    // The real call: direct to the engine's own port, naming a device. The only
    // effect is an in-memory stash the next teardown reads, so on this
    // disposable workspace it changes nothing any later test observes.
    let accepted = client
        .post(&url)
        .header("x-lucidos-device-id", "e2e-restart-intent-device")
        .send()
        .await
        .expect("request to /api/v1/internal/restart-intent failed");
    assert_eq!(
        accepted.status(),
        204,
        "a direct call naming a device must be accepted"
    );

    // It stashes and returns. Nothing was respawned, so the engine is still here
    // to answer, which is what keeps it from recursing with `/api/v1/restart`
    // (whose respawn is what makes the gateway call this route in the first
    // place).
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
