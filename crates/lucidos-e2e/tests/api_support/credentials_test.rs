//! E2E coverage for credential editing: the full-field update path, the
//! `email_password` sync into `email_accounts` that IMAP/SMTP read from, and
//! the *credential scope* set.
//!
//! The first two tests deliberately POST and PUT the singular `base_url`. That
//! is the permanent back-compat shape, so a script written before the scope
//! became a set keeps working, and this is what proves it.

use crate::support::{base_url, db_url, unique_marker, user_client};
use serde_json::json;
use sqlx::PgPool;

/// A generic (non-email) credential can have every editable field updated, and
/// omitting `auth_value` keeps the stored secret.
#[tokio::test]
async fn update_credential_edits_all_fields_and_keeps_secret_when_omitted() {
    let client = user_client().await;
    let api = base_url();
    let service = unique_marker("e2e-cred");

    // Create.
    let resp = client
        .post(format!("{}/api/v1/credentials", api))
        .json(&json!({
            "service_name": service,
            "base_url": "https://api.old.example.com",
            "auth_type": "api_key",
            "auth_value": "k1",
        }))
        .send()
        .await
        .expect("create failed");
    assert_eq!(resp.status(), 200);
    assert_eq!(
        resp.json::<serde_json::Value>().await.unwrap()["success"],
        true
    );

    // Every verb below addresses the row by id, so resolve it once from the list.
    let id = credential_id(&client, &api, &service).await;

    // Update the scope + auth_type + auth_header + secret, through the legacy
    // singular field.
    let resp = client
        .put(format!("{}/api/v1/credentials", api))
        .query(&[("id", &id)])
        .json(&json!({
            "base_url": "https://api.new.example.com",
            "auth_type": "bearer",
            "auth_header": "X-Api-Key",
            "auth_value": "k2",
        }))
        .send()
        .await
        .expect("update failed");
    assert_eq!(resp.status(), 200);
    assert_eq!(
        resp.json::<serde_json::Value>().await.unwrap()["success"],
        true
    );

    // List reflects the new non-secret fields.
    let listed = find_credential(&client, &api, &service).await;
    assert_eq!(listed["base_urls"], json!(["https://api.new.example.com"]));
    assert_eq!(listed["auth_type"], "bearer");
    assert_eq!(listed["auth_header"], "X-Api-Key");

    // credential-value reflects the new secret.
    assert_eq!(credential_value(&client, &api, &id).await, "k2");

    // Update without auth_value keeps the secret but still edits other fields.
    let resp = client
        .put(format!("{}/api/v1/credentials", api))
        .query(&[("id", &id)])
        .json(&json!({
            "base_url": "https://api.newer.example.com",
            "auth_type": "bearer",
            "auth_header": "X-Api-Key",
        }))
        .send()
        .await
        .expect("update failed");
    assert_eq!(resp.status(), 200);

    let listed = find_credential(&client, &api, &service).await;
    assert_eq!(
        listed["base_urls"],
        json!(["https://api.newer.example.com"])
    );
    assert_eq!(
        credential_value(&client, &api, &id).await,
        "k2",
        "omitting auth_value must keep the stored secret"
    );

    // Cleanup.
    delete_credential(&client, &api, &id).await;
}

/// Editing an `email_password` credential must propagate the password AND the
/// server settings to the `email_accounts` row — the table IMAP/SMTP read from.
/// This is the regression an email-account edit surfaced.
#[tokio::test]
async fn update_email_credential_syncs_email_accounts_row() {
    let client = user_client().await;
    let api = base_url();
    let pool = PgPool::connect(&db_url()).await.expect("db connect");

    let name = unique_marker("e2e-email");
    // No `email:` prefix: `auth_type` marks it, and the name IS the
    // `email_accounts.name` the sync resolves.
    let service = name.clone();

    // Seed an email account (the row IMAP reads), as `configure_email` would.
    sqlx::query(
        "INSERT INTO email_accounts (name, email_address, imap_host, imap_port, smtp_host, smtp_port, username, password, use_tls, require_send_confirmation) \
         VALUES ($1, $2, 'imap.old.example.com', 993, 'smtp.old.example.com', 587, $2, 'oldpass', true, true)",
    )
    .bind(&name)
    .bind(format!("{}@old.example.com", name))
    .execute(&pool)
    .await
    .expect("seed email account");

    // Create the paired credential via the real create path.
    let resp = client
        .post(format!("{}/api/v1/credentials", api))
        .json(&json!({
            "service_name": service,
            "base_url": "smtp://smtp.old.example.com",
            "auth_type": "email_password",
            "auth_value": "oldpass",
        }))
        .send()
        .await
        .expect("create failed");
    assert_eq!(resp.status(), 200);

    let id = credential_id(&client, &api, &service).await;

    // Edit: new password + new server settings.
    let new_email = format!("{}@new.example.com", name);
    let resp = client
        .put(format!("{}/api/v1/credentials", api))
        .query(&[("id", &id)])
        .json(&json!({
            "base_urls": ["https://ignored.example.com"],
            "auth_type": "email_password",
            "auth_value": "newpass",
            "email": {
                "email_address": new_email,
                "imap_host": "imap.new.example.com",
                "imap_port": 993,
                "smtp_host": "smtp.new.example.com",
                "smtp_port": 465,
                "username": new_email,
                "use_tls": true,
                "require_send_confirmation": false,
            },
        }))
        .send()
        .await
        .expect("update failed");
    assert_eq!(resp.status(), 200);
    assert_eq!(
        resp.json::<serde_json::Value>().await.unwrap()["success"],
        true
    );

    // The email_accounts row IMAP reads must carry the new password + settings.
    let row: (String, String, String, String, i32, bool) = sqlx::query_as(
        "SELECT password, email_address, imap_host, smtp_host, smtp_port, require_send_confirmation \
         FROM email_accounts WHERE name = $1",
    )
    .bind(&name)
    .fetch_one(&pool)
    .await
    .expect("fetch email account");
    assert_eq!(
        row.0, "newpass",
        "email_accounts.password must be updated on edit"
    );
    assert_eq!(row.1, new_email);
    assert_eq!(row.2, "imap.new.example.com");
    assert_eq!(row.3, "smtp.new.example.com");
    assert_eq!(row.4, 465);
    assert!(!row.5, "require_send_confirmation must be updated");

    // The credential's base_url is derived from the SMTP host, and its secret synced.
    let listed = find_credential(&client, &api, &service).await;
    assert_eq!(listed["base_urls"], json!(["smtp://smtp.new.example.com"]));
    assert_eq!(credential_value(&client, &api, &id).await, "newpass");

    // Cleanup.
    delete_credential(&client, &api, &id).await;
    sqlx::query("DELETE FROM email_accounts WHERE name = $1")
        .bind(&name)
        .execute(&pool)
        .await
        .expect("cleanup email account");
}

async fn find_credential(client: &reqwest::Client, api: &str, service: &str) -> serde_json::Value {
    let body: serde_json::Value = client
        .get(format!("{}/api/v1/credentials", api))
        .send()
        .await
        .expect("list failed")
        .json()
        .await
        .expect("invalid json");
    body["credentials"]
        .as_array()
        .expect("credentials array")
        .iter()
        .find(|c| c["service_name"] == service)
        .unwrap_or_else(|| panic!("credential {service} not found in list"))
        .clone()
}

/// The id of the credential with this service name, for the verbs that address
/// an existing row.
async fn credential_id(client: &reqwest::Client, api: &str, service: &str) -> String {
    find_credential(client, api, service).await["id"]
        .as_str()
        .expect("credential id")
        .to_string()
}

/// Mint a one-shot reveal token for `id`. Step one of two (ADR 0117).
async fn reveal_token(client: &reqwest::Client, api: &str, id: &str) -> String {
    let resp = client
        .post(format!("{}/api/v1/credential-reveal-token", api))
        .query(&[("id", id)])
        .send()
        .await
        .expect("mint failed");
    assert_eq!(resp.status().as_u16(), 200, "mint must 200");
    let body: serde_json::Value = resp.json().await.expect("invalid json");
    body["token"].as_str().expect("token").to_string()
}

/// Keyed on `id`, like the endpoint. A service name no longer identifies one
/// row: `auth_type` is the discriminator, and an `oauth_client` registration is
/// allowed to share a name with an API key for the same provider.
async fn credential_value(client: &reqwest::Client, api: &str, id: &str) -> String {
    let token = reveal_token(client, api, id).await;
    let body: serde_json::Value = client
        .get(format!("{}/api/v1/credential-value", api))
        .query(&[("id", id), ("token", token.as_str())])
        .send()
        .await
        .expect("value failed")
        .json()
        .await
        .expect("invalid json");
    body["auth_value"].as_str().expect("auth_value").to_string()
}

async fn delete_credential(client: &reqwest::Client, api: &str, id: &str) {
    let resp = client
        .delete(format!("{}/api/v1/credentials", api))
        .query(&[("id", id)])
        .send()
        .await
        .expect("delete failed");
    assert_eq!(resp.status(), 200);
}

/// A credential's scope is a set, over the wire and from the CLI.
///
/// Three things at once, because they are one flow. A create declares two
/// hosts. `PUT /api/v1/credential-base-urls` replaces the set without naming
/// the auth type or the auth header, which is what `lucidos credentials
/// set-base-urls` calls. A scope that is not a URL with a host is refused where
/// the user typed it.
#[tokio::test]
async fn a_credential_carries_and_edits_a_set_of_base_urls() {
    let client = user_client().await;
    let api = base_url();
    let service = unique_marker("e2e-scope");

    let resp = client
        .post(format!("{}/api/v1/credentials", api))
        .json(&json!({
            "service_name": service,
            "base_urls": ["https://api.binance.test", "https://fapi.binance.test"],
            "auth_type": "api_key",
            "auth_header": "X-MBX-APIKEY",
            "auth_value": "the-hmac-key",
        }))
        .send()
        .await
        .expect("create failed");
    assert_eq!(resp.status(), 200);

    let listed = find_credential(&client, &api, &service).await;
    assert_eq!(
        listed["base_urls"],
        json!(["https://api.binance.test", "https://fapi.binance.test"])
    );
    let id = credential_id(&client, &api, &service).await;

    // The narrow verb replaces the set and leaves everything else alone.
    let resp = client
        .put(format!("{}/api/v1/credential-base-urls", api))
        .query(&[("id", &id)])
        .json(&json!({
            "base_urls": ["https://api.binance.test", "https://dapi.binance.test"],
        }))
        .send()
        .await
        .expect("set-base-urls failed");
    assert_eq!(resp.status(), 200);
    assert_eq!(
        resp.json::<serde_json::Value>().await.unwrap()["success"],
        true
    );

    let listed = find_credential(&client, &api, &service).await;
    assert_eq!(
        listed["base_urls"],
        json!(["https://api.binance.test", "https://dapi.binance.test"])
    );
    assert_eq!(
        listed["auth_header"], "X-MBX-APIKEY",
        "widening a scope must not clobber the auth header"
    );
    assert_eq!(credential_value(&client, &api, &id).await, "the-hmac-key");

    // A host-less value is refused here, not silently at the proxy gate.
    let resp = client
        .put(format!("{}/api/v1/credential-base-urls", api))
        .query(&[("id", &id)])
        .json(&json!({ "base_urls": ["api.binance.test"] }))
        .send()
        .await
        .expect("request failed");
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["success"], false);
    assert!(
        body["error"].as_str().unwrap_or_default().contains("host"),
        "the refusal must say what is wrong: {body}"
    );

    // An edit naming NO scope field keeps the stored set. Both fields are
    // optional so the two spellings can coexist, so absence would otherwise
    // deserialize as an empty set and refuse the credential everywhere.
    let resp = client
        .put(format!("{}/api/v1/credentials", api))
        .query(&[("id", &id)])
        .json(&json!({ "auth_type": "api_key", "auth_header": "X-Changed" }))
        .send()
        .await
        .expect("update failed");
    assert_eq!(resp.status(), 200);
    let listed = find_credential(&client, &api, &service).await;
    assert_eq!(
        listed["base_urls"],
        json!(["https://api.binance.test", "https://dapi.binance.test"]),
        "an edit that named no scope must not empty it"
    );
    assert_eq!(listed["auth_header"], "X-Changed", "the edit still landed");

    delete_credential(&client, &api, &id).await;
}

/// A create that names no scope field is refused, rather than storing a
/// credential the proxy can only ever refuse. An empty list is a real answer
/// and has to be written down.
#[tokio::test]
async fn creating_a_credential_without_a_scope_field_is_refused() {
    let client = user_client().await;
    let api = base_url();
    let service = unique_marker("e2e-noscope");

    let resp = client
        .post(format!("{}/api/v1/credentials", api))
        .json(&json!({
            "service_name": service,
            "auth_type": "api_key",
            "auth_value": "k",
        }))
        .send()
        .await
        .expect("create failed");
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["success"], false, "{body}");
    assert!(
        body["error"]
            .as_str()
            .unwrap_or_default()
            .contains("base_urls"),
        "the refusal must name the field: {body}"
    );

    // An explicit empty list IS accepted: that is what a `secret` carries.
    let resp = client
        .post(format!("{}/api/v1/credentials", api))
        .json(&json!({
            "service_name": service,
            "base_urls": [],
            "auth_type": "secret",
            "auth_value": "shared-secret",
        }))
        .send()
        .await
        .expect("create failed");
    assert_eq!(resp.status(), 200);
    assert_eq!(
        resp.json::<serde_json::Value>().await.unwrap()["success"],
        true
    );
    let listed = find_credential(&client, &api, &service).await;
    assert_eq!(listed["base_urls"], json!([]));

    delete_credential(&client, &api, &credential_id(&client, &api, &service).await).await;
}

/// Regression: the plaintext is not one bare GET away, and not reachable from
/// an app.
///
/// `GET /api/v1/credential-value?id=<uuid>` returned the stored secret to any
/// caller. App UIs are same-origin, so an installed app's JS could list the
/// credentials and read every one. That is exactly what the credentialed proxy
/// exists to prevent.
#[tokio::test]
async fn a_credential_value_needs_a_token_and_a_non_app_origin() {
    let client = user_client().await;
    let api = base_url();
    let service = unique_marker("e2e-reveal");

    let resp = client
        .post(format!("{}/api/v1/credentials", api))
        .json(&json!({
            "service_name": service,
            "base_url": "https://api.example.com",
            "auth_type": "api_key",
            "auth_value": "s3cret",
        }))
        .send()
        .await
        .expect("create failed");
    assert_eq!(resp.status(), 200);
    let id = credential_id(&client, &api, &service).await;

    // No token: refused, and the body carries no secret.
    let resp = client
        .get(format!("{}/api/v1/credential-value", api))
        .query(&[("id", id.as_str())])
        .send()
        .await
        .expect("token-less read failed");
    assert_eq!(
        resp.status().as_u16(),
        403,
        "a request with no `token` must not reach the credential store"
    );
    assert!(
        !resp.text().await.unwrap().contains("s3cret"),
        "a refusal must never carry the value"
    );

    // A minted token spends exactly once.
    let token = reveal_token(&client, &api, &id).await;
    let value_url = format!("{}/api/v1/credential-value", api);
    let first = client
        .get(&value_url)
        .query(&[("id", id.as_str()), ("token", token.as_str())])
        .send()
        .await
        .expect("first read failed");
    assert_eq!(first.status().as_u16(), 200);
    assert_eq!(
        first.json::<serde_json::Value>().await.unwrap()["auth_value"],
        "s3cret"
    );

    let replay = client
        .get(&value_url)
        .query(&[("id", id.as_str()), ("token", token.as_str())])
        .send()
        .await
        .expect("replay failed");
    assert_eq!(
        replay.status().as_u16(),
        403,
        "a reveal token is one-shot; a replay must be refused"
    );

    // An app document is refused at both steps, token or no token.
    let app_referer = format!("{}/app/habit-tracker/", api);
    for (method, url) in [
        ("POST", format!("{}/api/v1/credential-reveal-token", api)),
        ("GET", value_url.clone()),
    ] {
        let req = if method == "POST" {
            client.post(&url)
        } else {
            client.get(&url)
        };
        let resp = req
            .query(&[("id", id.as_str()), ("token", "anything")])
            .header("sec-fetch-site", "same-origin")
            .header("referer", &app_referer)
            .send()
            .await
            .expect("app-origin request failed");
        assert_eq!(
            resp.status().as_u16(),
            403,
            "{method} {url} must refuse an app-iframe Referer"
        );
    }

    // The successful read is on the record, naming the service, never the value.
    let pool = PgPool::connect(&db_url())
        .await
        .expect("Failed to connect to database");
    let payload: Option<serde_json::Value> = sqlx::query_scalar(
        "SELECT payload FROM events \
         WHERE event_type = 'CredentialRevealed' AND aggregate_id = $1 \
         ORDER BY sequence DESC LIMIT 1",
    )
    .bind(&service)
    .fetch_optional(&pool)
    .await
    .expect("query failed");
    let payload = payload.expect("a reveal must persist a CredentialRevealed row");
    assert_eq!(payload["data"]["service_name"], service);
    assert!(
        !payload.to_string().contains("s3cret"),
        "the audit row must never carry the value: {payload}"
    );
    pool.close().await;

    delete_credential(&client, &api, &id).await;
}
