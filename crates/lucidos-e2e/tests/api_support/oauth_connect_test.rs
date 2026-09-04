//! E2E coverage for the Connect flow's HTTP surface: the *OAuth provider
//! registry* endpoint, and what `reauthorize` answers for a registration that
//! is missing or that cannot drive a flow.
//!
//! The second half is the reported bug's whole shape. A user typed a provider
//! name, pressed Connect, was handed a credential form with an empty endpoint
//! section, saved a client with no `auth_url` because nothing enforced one, and
//! got "Missing auth_url in OAuth credentials" on the next press: an error one
//! screen away from its cause, with no way forward.

use crate::support::{base_url, unique_marker, user_client};
use serde_json::{json, Value};

/// The registry the Connect form autofills from.
#[tokio::test]
async fn known_providers_are_served_with_the_redirect_uri_to_register() {
    let client = user_client().await;
    let api = base_url();

    let body: Value = client
        .get(format!("{}/api/v1/oauth/known-providers", api))
        .send()
        .await
        .expect("known-providers failed")
        .json()
        .await
        .expect("known-providers is not JSON");

    let providers = body["providers"]
        .as_array()
        .expect("known-providers must answer a providers array, even when empty");
    assert!(
        !providers.is_empty(),
        "the shipped registry must list providers, or the Accounts page has no quick buttons"
    );
    for p in providers {
        for field in ["id", "label", "base_url", "auth_url", "token_url"] {
            assert!(
                p[field].as_str().is_some_and(|s| !s.trim().is_empty()),
                "provider row is missing {field}, so the form cannot prefill around it: {p}"
            );
        }
    }

    // The form offers this for copying into the provider's console, where it has
    // to be registered character for character. The engine owns the port and
    // path, so it is the engine that states it.
    let redirect = body["default_redirect_uri"].as_str().unwrap_or_default();
    assert!(
        redirect.contains("/oauth/callback"),
        "expected the loopback callback URI, got {redirect:?}"
    );
}

/// No registration yet: the form opens PREFILLED for a provider the registry
/// knows, so the user enters only a Client ID.
#[tokio::test]
async fn connecting_a_known_provider_with_no_client_prefills_its_endpoints() {
    let client = user_client().await;
    let api = base_url();

    let known = first_known_provider(&client, &api).await;
    let id = known["id"].as_str().unwrap().to_string();

    let body: Value = client
        .post(format!("{}/api/v1/oauth/reauthorize", api))
        .json(&json!({ "provider": id, "scopes": "openid email profile" }))
        .send()
        .await
        .expect("reauthorize failed")
        .json()
        .await
        .expect("reauthorize is not JSON");

    assert_eq!(
        body["success"], false,
        "no client is registered yet: {body}"
    );
    let request = &body["credential_request"];
    assert_eq!(request["auth_type"], "oauth_client");
    assert_eq!(request["service"], id);
    // The whole point of the registry. Before it, this block was absent and the
    // modal made the user type a provider's own URLs by hand.
    assert_eq!(request["defaults"]["auth_url"], known["auth_url"]);
    assert_eq!(request["defaults"]["token_url"], known["token_url"]);
    assert_eq!(request["base_url"], known["base_url"]);
}

/// A registration that exists but cannot drive a flow reopens the SAME form,
/// prefilled, targeted at the row it must repair.
#[tokio::test]
async fn an_endpointless_client_reopens_the_form_instead_of_failing_later() {
    let client = user_client().await;
    let api = base_url();

    let known = first_known_provider(&client, &api).await;
    let base_provider = known["id"].as_str().unwrap();
    // A derived name, so this test owns its own credential row and cannot
    // disturb a real one. It resolves to no registry row on purpose.
    let provider = unique_marker("e2e-oauth").to_lowercase();

    // Exactly the credential the old form allowed: a client id, no endpoints.
    let resp = client
        .post(format!("{}/api/v1/credentials", api))
        .json(&json!({
            "service_name": provider,
            "base_url": "https://api.example.com",
            "auth_type": "oauth_client",
            "auth_value": r#"{"client_id":"abc"}"#,
        }))
        .send()
        .await
        .expect("create failed");
    assert_eq!(resp.status(), 200);

    let body: Value = client
        .post(format!("{}/api/v1/oauth/reauthorize", api))
        .json(&json!({ "provider": provider, "scopes": "openid email profile" }))
        .send()
        .await
        .expect("reauthorize failed")
        .json()
        .await
        .expect("reauthorize is not JSON");

    assert_eq!(body["success"], false);
    let request = &body["credential_request"];
    assert!(
        !request.is_null(),
        "an incomplete client must reopen the form, not fail with a bare toast: {body}"
    );

    // Targeted at the existing row. Creating instead would make a SECOND
    // oauth_client for one provider, and a name plus an auth type is the
    // credential's identity, so that pair is a duplicate.
    let existing = request["existing_credential_id"].as_str();
    assert!(
        existing.is_some_and(|s| !s.is_empty()),
        "the repair must name the credential it updates: {request}"
    );

    // Says what was wrong, and keeps the value the user already supplied.
    let missing: Vec<&str> = request["missing"]
        .as_array()
        .expect("a repair names the missing fields")
        .iter()
        .map(|v| v.as_str().unwrap_or_default())
        .collect();
    assert!(
        missing.contains(&"auth_url"),
        "expected auth_url in {missing:?}"
    );
    assert!(
        missing.contains(&"token_url"),
        "expected token_url in {missing:?}"
    );
    assert_eq!(request["defaults"]["client_id"], "abc");

    // An unknown provider has no row to prefill from, which is the whole reason
    // the form asks which known provider a derived name runs on.
    assert_ne!(provider, base_provider);

    // Clean up: registry-style rows are shared state within a run.
    let id = existing.unwrap();
    client
        .delete(format!("{}/api/v1/credentials", api))
        .query(&[("id", id)])
        .send()
        .await
        .expect("delete failed");
}

/// The first row of the shipped registry, whatever it is. Naming a provider here
/// would put a provider-specific expectation in the test suite, which is the
/// thing the registry exists to avoid.
async fn first_known_provider(client: &reqwest::Client, api: &str) -> Value {
    let body: Value = client
        .get(format!("{}/api/v1/oauth/known-providers", api))
        .send()
        .await
        .expect("known-providers failed")
        .json()
        .await
        .expect("known-providers is not JSON");
    body["providers"]
        .as_array()
        .and_then(|p| p.first())
        .cloned()
        .expect("the shipped registry must list at least one provider")
}
