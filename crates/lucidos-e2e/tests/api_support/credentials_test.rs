use crate::support::{base_url, db_url, unique_marker, user_client};
use serde_json::json;
use sqlx::PgPool;

#[tokio::test]
async fn update_credential_edits_all_fields_and_keeps_secret_when_omitted() {
    let client = user_client().await;
    let api = base_url();
    let service = unique_marker("e2e-cred");

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

    let id = credential_id(&client, &api, &service).await;

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

    let listed = find_credential(&client, &api, &service).await;
    assert_eq!(listed["base_urls"], json!(["https://api.new.example.com"]));
    assert_eq!(listed["auth_type"], "bearer");
    assert_eq!(listed["auth_header"], "X-Api-Key");

    assert_eq!(credential_value(&client, &api, &id).await, "k2");

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

    delete_credential(&client, &api, &id).await;
}

#[tokio::test]
async fn update_email_credential_syncs_email_accounts_row() {
    let client = user_client().await;
    let api = base_url();
    let pool = PgPool::connect(&db_url()).await.expect("db connect");

    let name = unique_marker("e2e-email");
    let service = name.clone();

    sqlx::query(
        "INSERT INTO email_accounts (name, email_address, imap_host, imap_port, smtp_host, smtp_port, username, use_tls, require_send_confirmation) \
         VALUES ($1, $2, 'imap.old.example.com', 993, 'smtp.old.example.com', 587, $2, true, true)",
    )
    .bind(&name)
    .bind(format!("{}@old.example.com", name))
    .execute(&pool)
    .await
    .expect("seed email account");

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

    let row: (String, String, String, i32, bool) = sqlx::query_as(
        "SELECT email_address, imap_host, smtp_host, smtp_port, require_send_confirmation \
         FROM email_accounts WHERE name = $1",
    )
    .bind(&name)
    .fetch_one(&pool)
    .await
    .expect("fetch email account");
    assert_eq!(row.0, new_email);
    assert_eq!(row.1, "imap.new.example.com");
    assert_eq!(row.2, "smtp.new.example.com");
    assert_eq!(row.3, 465);
    assert!(!row.4, "require_send_confirmation must be updated");

    let listed = find_credential(&client, &api, &service).await;
    assert_eq!(listed["base_urls"], json!(["smtp://smtp.new.example.com"]));
    assert_eq!(credential_value(&client, &api, &id).await, "newpass");

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

async fn credential_id(client: &reqwest::Client, api: &str, service: &str) -> String {
    find_credential(client, api, service).await["id"]
        .as_str()
        .expect("credential id")
        .to_string()
}

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

#[tokio::test]
async fn events_query_masks_a_stored_credential_in_an_old_row() {
    let client = user_client().await;
    let api = base_url();
    let service = unique_marker("e2e-mask");
    let secret = unique_marker("opaque-stored-value");
    let resp = client
        .post(format!("{}/api/v1/credentials", api))
        .json(&json!({
            "service_name": service,
            "base_url": "https://api.example.com",
            "auth_type": "api_key",
            "auth_value": secret,
        }))
        .send()
        .await
        .expect("create failed");
    assert_eq!(resp.status(), 200);
    let id = credential_id(&client, &api, &service).await;

    let pool = PgPool::connect(&db_url())
        .await
        .expect("Failed to connect to database");
    let thread_id = uuid::Uuid::new_v4();
    sqlx::query(
        "INSERT INTO events (id, event_type, payload, created, aggregate_id, aggregate, thread_id) \
         VALUES ($1, 'BackgroundBashCompleted', $2, NOW(), $3::text, 'thread', $3)",
    )
    .bind(uuid::Uuid::new_v4())
    .bind(json!({ "task_id": "task-old", "stdout": format!("key={secret}"), "stderr": "" }))
    .bind(thread_id)
    .execute(&pool)
    .await
    .expect("seed BackgroundBashCompleted");
    pool.close().await;

    let resp = client
        .get(format!("{}/api/v1/events/query", api))
        .query(&[("thread_id", thread_id.to_string())])
        .send()
        .await
        .expect("query failed");
    assert_eq!(resp.status(), 200);
    let body = resp.text().await.expect("body");
    assert!(!body.contains(&secret), "credential replayed: {body}");
    assert!(
        body.contains("key=[REDACTED]"),
        "row missing or unmasked: {body}"
    );

    delete_credential(&client, &api, &id).await;
}
