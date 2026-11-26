use crate::support::{base_url, unique_marker, user_client};
use serde_json::{json, Value};

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

    let redirect = body["default_redirect_uri"].as_str().unwrap_or_default();
    assert!(
        redirect.contains("/oauth/callback"),
        "expected the loopback callback URI, got {redirect:?}"
    );
}

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
    assert_eq!(request["defaults"]["auth_url"], known["auth_url"]);
    assert_eq!(request["defaults"]["token_url"], known["token_url"]);
    assert_eq!(request["base_urls"], serde_json::json!([known["base_url"]]));
}

#[tokio::test]
async fn an_endpointless_client_reopens_the_form_instead_of_failing_later() {
    let client = user_client().await;
    let api = base_url();

    let known = first_known_provider(&client, &api).await;
    let base_provider = known["id"].as_str().unwrap();
    let provider = unique_marker("e2e-oauth").to_lowercase();

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

    let existing = request["existing_credential_id"].as_str();
    assert!(
        existing.is_some_and(|s| !s.is_empty()),
        "the repair must name the credential it updates: {request}"
    );

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

    assert_ne!(provider, base_provider);

    let id = existing.unwrap();
    client
        .delete(format!("{}/api/v1/credentials", api))
        .query(&[("id", id)])
        .send()
        .await
        .expect("delete failed");
}

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

#[tokio::test]
async fn saving_a_client_with_a_cleartext_token_url_is_refused() {
    const SECRET: &str = "e2e-client-secret-value";
    let client = user_client().await;
    let api = base_url();
    let provider = unique_marker("e2e-oauth-https").to_lowercase();
    let client_json = |token_url: &str| {
        json!({
            "client_id": "abc",
            "client_secret": SECRET,
            "auth_url": "https://oauth.example.com/authorize",
            "token_url": token_url,
        })
        .to_string()
    };
    let assert_refused = |body: &Value| {
        assert_eq!(body["success"], false, "{body}");
        let error = body["error"].as_str().unwrap_or_default();
        assert!(error.contains(&provider), "names the credential: {error}");
        assert!(error.contains("token_url"), "names the field: {error}");
        assert!(error.contains("https"), "says what to fix: {error}");
        assert!(!error.contains(SECRET), "echoes no secret: {error}");
    };

    let create = |auth_value: String| {
        client
            .post(format!("{}/api/v1/credentials", api))
            .json(&json!({
                "service_name": provider,
                "base_url": "https://api.example.com",
                "auth_type": "oauth_client",
                "auth_value": auth_value,
            }))
            .send()
    };
    let refused: Value = create(client_json("http://oauth.example.com/token"))
        .await
        .expect("create failed")
        .json()
        .await
        .expect("create is not JSON");
    assert_refused(&refused);

    let saved: Value = create(client_json("https://oauth.example.com/token"))
        .await
        .expect("create failed")
        .json()
        .await
        .expect("create is not JSON");
    assert_eq!(saved["success"], true, "https saves: {saved}");

    let listed: Value = client
        .get(format!("{}/api/v1/credentials", api))
        .send()
        .await
        .expect("list failed")
        .json()
        .await
        .expect("list is not JSON");
    let id = listed["credentials"]
        .as_array()
        .expect("credentials array")
        .iter()
        .find(|c| c["service_name"] == provider.as_str())
        .and_then(|c| c["id"].as_str())
        .expect("the https client was stored")
        .to_string();

    let edited: Value = client
        .put(format!("{}/api/v1/credentials", api))
        .query(&[("id", id.as_str())])
        .json(&json!({
            "auth_type": "oauth_client",
            "auth_value": client_json("http://oauth.example.com/token"),
        }))
        .send()
        .await
        .expect("update failed")
        .json()
        .await
        .expect("update is not JSON");
    assert_refused(&edited);

    client
        .delete(format!("{}/api/v1/credentials", api))
        .query(&[("id", id.as_str())])
        .send()
        .await
        .expect("delete failed");
}
