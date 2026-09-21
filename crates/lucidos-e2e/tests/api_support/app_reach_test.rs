//! The engine's own answer to a request an app stamped itself on.
//!
//! The host bridge is the boundary (ADR 0231). This is the second answer, and
//! it is the half a unit test cannot show: the layer only sees a route once
//! axum has matched one, so the gate has to be exercised over real HTTP.

use crate::support::{base_url, user_client};

const APP_ID_HEADER: &str = "x-lucidos-app-id";

#[tokio::test]
async fn a_stamped_call_to_a_host_route_is_refused() {
    let client = user_client().await;
    let res = client
        .get(format!("{}/api/v1/credentials", base_url()))
        .header(APP_ID_HEADER, "habit-tracker")
        .send()
        .await
        .expect("request to /api/v1/credentials failed");
    assert_eq!(
        res.status(),
        403,
        "an app-stamped call to a host-only route must be refused"
    );
    let body = res.text().await.unwrap_or_default();
    assert!(
        body.contains("An app may not call"),
        "the refusal must say what it refused, got: {body}"
    );
}

#[tokio::test]
async fn the_same_route_answers_the_shell() {
    // No stamp, so the gate stands aside. This is what makes the test above a
    // statement about the stamp rather than about the route being broken.
    let client = user_client().await;
    let res = client
        .get(format!("{}/api/v1/credentials", base_url()))
        .send()
        .await
        .expect("request to /api/v1/credentials failed");
    assert_eq!(res.status(), 200, "the shell reads its own credential list");
}

#[tokio::test]
async fn a_stamped_call_to_an_app_route_goes_through() {
    let client = user_client().await;
    let res = client
        .get(format!("{}/api/v1/env-vars", base_url()))
        .header(APP_ID_HEADER, "habit-tracker")
        .send()
        .await
        .expect("request to /api/v1/env-vars failed");
    assert_eq!(
        res.status(),
        200,
        "env-vars is app-reachable, and the stamp must not change that"
    );
}

#[tokio::test]
async fn a_stamped_call_is_refused_per_method() {
    // The model registry reads, and never writes, from an app.
    let client = user_client().await;
    let read = client
        .get(format!("{}/api/v1/models", base_url()))
        .header(APP_ID_HEADER, "habit-tracker")
        .send()
        .await
        .expect("request to /api/v1/models failed");
    assert_eq!(read.status(), 200, "an app may read the model registry");

    let write = client
        .delete(format!("{}/api/v1/models?id=whatever", base_url()))
        .header(APP_ID_HEADER, "habit-tracker")
        .send()
        .await
        .expect("request to /api/v1/models failed");
    assert_eq!(
        write.status(),
        403,
        "an app may not write the model registry, even though it may read it"
    );
}

#[tokio::test]
async fn an_unmatched_route_still_answers_404() {
    // A stamped request to nothing is a 404, not a refusal. Answering with a
    // refusal would point the caller at the wrong problem.
    let client = user_client().await;
    let res = client
        .get(format!("{}/api/v1/no-such-route", base_url()))
        .header(APP_ID_HEADER, "habit-tracker")
        .send()
        .await
        .expect("request to /api/v1/no-such-route failed");
    assert_eq!(res.status(), 404);
}
