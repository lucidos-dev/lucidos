//! E2E coverage for the Sign in with ChatGPT routes (ADR 0636).
//!
//! This pins what holds without ChatGPT: the signed-out status, the refusal
//! for a browser that cannot follow the loopback redirect, and a sign-out with
//! nothing stored. The sign-in itself is covered against a mock auth server in
//! the engine's own tests, and never spends plan usage.
//!
//! One test, because the sign-in slot is engine-global and parallel cases
//! would race on it.

use crate::support::{base_url, user_client};
use serde_json::json;

async fn status(client: &reqwest::Client, api: &str) -> serde_json::Value {
    let resp = client
        .get(format!("{api}/api/v1/providers/chatgpt-plan"))
        .send()
        .await
        .expect("status request failed");
    assert_eq!(resp.status(), 200);
    resp.json().await.expect("a status body")
}

#[tokio::test]
async fn the_chatgpt_plan_routes_answer_while_signed_out() {
    let client = user_client().await;
    let api = base_url();

    let idle = status(&client, &api).await;
    assert_eq!(
        idle,
        json!({
            "account": null,
            "sign_in_again": false,
            "registered": false,
            "sign_in": { "state": "idle" },
            "paused_until": null,
            "pause_remaining_ms": null,
        })
    );

    // A proxied request without the gateway's local-browser stamp is a remote
    // browser. With a gateway it cannot reach the loopback redirect; without
    // one nothing holds the sign-in port. Either way the start is refused.
    let resp = client
        .post(format!("{api}/api/v1/providers/chatgpt-plan/sign-in"))
        .header("x-forwarded-prefix", "/e2e-test/")
        .json(&json!({}))
        .send()
        .await
        .expect("sign-in request failed");
    assert_eq!(resp.status(), 400);
    let body = resp.text().await.unwrap();
    assert!(body.contains("Sign in with ChatGPT"), "{body}");
    assert!(
        !body.contains("auth.openai.com"),
        "no authorize URL: {body}"
    );
    assert_eq!(
        status(&client, &api).await,
        idle,
        "a refusal starts nothing"
    );

    for query in ["", "?forget_account=true"] {
        let out = client
            .post(format!(
                "{api}/api/v1/providers/chatgpt-plan/sign-out{query}"
            ))
            .json(&json!({}))
            .send()
            .await
            .expect("sign-out request failed");
        assert_eq!(out.status(), 200, "{query}");
        let out: serde_json::Value = out.json().await.unwrap();
        assert_eq!(
            out,
            json!({ "signed_out": false, "account_forgotten": false }),
            "{query}"
        );
        assert_eq!(status(&client, &api).await, idle);
    }
}
