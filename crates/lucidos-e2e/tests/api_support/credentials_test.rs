//! E2E coverage for credential editing — the full-field update path and the
//! email_password → `email_accounts` sync that IMAP/SMTP actually read from.

use crate::support::{base_url, db_url, http_client, unique_marker};
use serde_json::json;
use sqlx::PgPool;

/// A generic (non-email) credential can have every editable field updated, and
/// omitting `auth_value` keeps the stored secret.
#[tokio::test]
async fn update_credential_edits_all_fields_and_keeps_secret_when_omitted() {
    let client = http_client();
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

    // Update base_url + auth_type + auth_header + secret.
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
    assert_eq!(listed["base_url"], "https://api.new.example.com");
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
    assert_eq!(listed["base_url"], "https://api.newer.example.com");
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
    let client = http_client();
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
            "base_url": "ignored — derived from smtp_host",
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
    assert_eq!(listed["base_url"], "smtp://smtp.new.example.com");
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

/// Keyed on `id`, like the endpoint. A service name no longer identifies one
/// row: `auth_type` is the discriminator, and an `oauth_client` registration is
/// allowed to share a name with an API key for the same provider.
async fn credential_value(client: &reqwest::Client, api: &str, id: &str) -> String {
    let body: serde_json::Value = client
        .get(format!("{}/api/v1/credential-value", api))
        .query(&[("id", id)])
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
