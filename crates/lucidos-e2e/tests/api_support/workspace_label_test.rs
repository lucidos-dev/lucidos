//! `GET /api/v1/workspace-label`: the display name a direct-port page asks for.
//!
//! The e2e engine is launched directly by `scripts/lib/e2e.sh`, not spawned by
//! the gateway, so it is exactly the shape that has no registry row of its own
//! to consult. That makes this suite the right place to pin the DEGRADATION,
//! which is the half a unit test cannot reach: the route must answer 200 with a
//! null label, not 404 and not a 500. A frontend that got either would log a
//! warning on every boot, and an engine that erred here would put an unreachable
//! gateway on the app's startup path.
//!
//! **Both roads to that null are real here, which is why the assertion holds
//! however the suite was started.** From a clean shell the gateway identity vars
//! are simply unset. From inside a coding-agent session they are NOT: a
//! directly-launched engine inherits its launcher's whole environment, and this
//! suite genuinely picked up `LUCIDOS_WORKSPACE_ID=dev` that way and answered
//! with the `dev` workspace's label until the handler started checking the slug
//! against the port it actually serves on. That check is what keeps this test
//! honest rather than accidentally passing.
//!
//! The populated case lives in the Rust unit tests beside the handler
//! (`api/workspace_label.rs`), which drive the projection off a real listing
//! without needing a gateway in the harness.

use crate::support::{base_url, http_client};

#[tokio::test]
async fn workspace_label_is_null_for_an_engine_with_no_registry_row_of_its_own() {
    let client = http_client();
    let url = format!("{}/api/v1/workspace-label", base_url());

    let resp = client
        .get(&url)
        .send()
        .await
        .expect("Workspace label request failed");
    assert_eq!(
        resp.status(),
        200,
        "the label route answers even with no gateway"
    );

    let body: serde_json::Value = resp.json().await.expect("Invalid JSON");
    // The key must be PRESENT and null, not absent: the client reads
    // `body.label`, and an engine that answered `{}` would look identical to one
    // that answered a label of `undefined`.
    assert!(
        body.get("label").is_some(),
        "response carries a `label` key: {body}"
    );
    assert!(
        body["label"].is_null(),
        "this engine holds no registry row, so it has no name to report, got {}",
        body["label"]
    );
}

#[tokio::test]
async fn workspace_label_never_leaks_the_workspace_listing() {
    // The engine resolves the label from the gateway's control listing, which
    // names every workspace on the machine. Only this workspace's own name may
    // cross the boundary, so the response is one key and nothing else.
    let client = http_client();
    let body: serde_json::Value = client
        .get(format!("{}/api/v1/workspace-label", base_url()))
        .send()
        .await
        .expect("Workspace label request failed")
        .json()
        .await
        .expect("Invalid JSON");

    let obj = body.as_object().expect("an object response");
    assert_eq!(
        obj.keys().collect::<Vec<_>>(),
        vec!["label"],
        "exactly one key, so no listing row can ride along: {body}"
    );
}
