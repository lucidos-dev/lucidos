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
    let registry: serde_json::Value = read.json().await.expect("model registry JSON");
    let models = registry["models"].as_array().expect("models array");
    assert!(
        !models.is_empty(),
        "seeded registry must exercise route metadata"
    );
    for model in models {
        for route in model["routes"].as_array().expect("routes array") {
            let configured = route.get("configured").expect("configured is explicit");
            assert!(configured.is_null() || configured.is_boolean());
            let proxy = route.get("proxy").expect("proxy capability is explicit");
            if !proxy.is_null() {
                let fields = proxy.as_object().expect("proxy capability object");
                assert_eq!(fields.len(), 3, "only public proxy metadata is served");
                assert!(proxy["name"].is_string());
                assert!(proxy["protocol"].is_string());
                let defaults = proxy["request_defaults"]
                    .as_object()
                    .expect("wire defaults");
                assert!(defaults.contains_key(""), "provider default is explicit");
                for template in defaults.values() {
                    assert!(template["body"].is_object());
                    assert!(template["headers"].is_object());
                    assert_eq!(template.as_object().unwrap().len(), 2);
                }
            }
        }
    }

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
