//! E2E coverage for the backup key HTTP contract.
//!
//! Backups are sealed to a *backup recipient*, an age public key. Lucidos
//! mints the pair, shows the backup key once, and seals nothing with it until
//! the user pastes the key back. So no backup ever starts under a key nobody
//! saved, and the engine never stores the backup key itself.
//!
//! A workspace from before keeps its legacy symmetric key until the switch.
//! The reveal of that key stays behind a one-shot token and refuses an app
//! `Referer` (ADR 0117). The switch deletes it.
//!
//! The e2e engine is no app bundle, so its key store is the key file under
//! `~/.lucidos/secret-keys` (ADR 0509). A drop guard restores the original
//! legacy key (or its absence) so the test is hermetic even on panic.

use crate::support::{base_url, db_url, user_client};
use lucidos_engine::core::backup::crypto;
use lucidos_engine::core::secret_store::key_store::{
    backup_key_name, default_key_dir, FileKeyStore, KeyStore as _,
};
use lucidos_engine::core::secret_store::{boot::database_name, SecretKey};
use serde_json::{json, Value};
use sqlx::PgPool;

/// Restores the workspace's legacy backup key to its pre-test state on drop.
struct LegacyKeyGuard {
    store: FileKeyStore,
    name: String,
    original: Option<SecretKey>,
}

impl LegacyKeyGuard {
    fn capture() -> Self {
        let store = FileKeyStore::new(default_key_dir().expect("a key directory"));
        let name = backup_key_name(&database_name(&db_url()).expect("a database name"));
        let original = store.get(&name).expect("a readable backup key");
        Self {
            store,
            name,
            original,
        }
    }

    fn remove(&self) {
        self.store
            .delete(&self.name)
            .expect("delete the legacy key");
    }

    fn put(&self, key: &SecretKey) {
        self.store.put(&self.name, key).expect("store a legacy key");
    }

    fn stored(&self) -> Option<SecretKey> {
        self.store.get(&self.name).expect("a readable backup key")
    }
}

impl Drop for LegacyKeyGuard {
    fn drop(&mut self) {
        let _ = match &self.original {
            Some(key) => self.store.put(&self.name, key),
            None => self.store.delete(&self.name),
        };
    }
}

async fn pool() -> PgPool {
    PgPool::connect(&db_url())
        .await
        .expect("Failed to connect to database")
}

async fn forget_recipient() {
    let pool = pool().await;
    sqlx::query("DELETE FROM backup_recipient")
        .execute(&pool)
        .await
        .expect("clear the backup recipient");
    pool.close().await;
}

async fn recipient_status(client: &reqwest::Client, api: &str) -> Value {
    let resp = client
        .get(format!("{api}/api/v1/backup/recipient"))
        .send()
        .await
        .expect("recipient status failed");
    assert_eq!(resp.status().as_u16(), 200);
    resp.json().await.expect("recipient status JSON")
}

async fn post_json(client: &reqwest::Client, url: &str, body: Value) -> reqwest::Response {
    client
        .post(url)
        .json(&body)
        .send()
        .await
        .expect("POST failed")
}

async fn put_schedule(client: &reqwest::Client, api: &str, schedule: &str) -> reqwest::Response {
    client
        .put(format!("{api}/api/v1/backup/schedule"))
        .json(&json!({ "provider": "dropbox", "schedule": schedule }))
        .send()
        .await
        .expect("PUT /backup/schedule failed")
}

async fn reason(resp: reqwest::Response) -> String {
    let body: Value = resp.json().await.expect("an error body");
    body["reason"].as_str().unwrap_or_default().to_string()
}

/// One pass over the setup contract, in a single test so the assertions do
/// not race each other on the shared recipient and legacy key.
#[tokio::test]
async fn backups_wait_for_the_paste_back_and_never_store_the_backup_key() {
    let _lock = crate::support::backup_key_lock().lock().await;
    let legacy = LegacyKeyGuard::capture();
    legacy.remove();
    forget_recipient().await;
    let client = user_client().await;
    let api = base_url();
    let setup_url = format!("{api}/api/v1/backup/recipient/setup");
    let confirm_url = format!("{api}/api/v1/backup/recipient/confirm");
    let verify_url = format!("{api}/api/v1/backup/recipient/verify");

    let status = recipient_status(&client, &api).await;
    assert_eq!(status["recipient"], Value::Null);
    assert_eq!(status["pending"], json!(false));
    assert_eq!(status["legacy_key"], json!(false));

    // Nothing can seal yet, so neither a schedule nor a manual backup starts.
    let refused = put_schedule(&client, &api, "0 0 3 * * *").await;
    assert_eq!(refused.status().as_u16(), 409);
    assert_eq!(reason(refused).await, "backups-not-set-up");
    let refused = post_json(
        &client,
        &format!("{api}/api/v1/backup"),
        json!({ "provider": "dropbox" }),
    )
    .await;
    assert_eq!(refused.status().as_u16(), 409);
    assert_eq!(reason(refused).await, "backups-not-set-up");

    // Setup shows a backup key once.
    let resp = post_json(&client, &setup_url, json!({})).await;
    assert_eq!(resp.status().as_u16(), 200);
    let backup_key = resp.json::<Value>().await.unwrap()["backup_key"]
        .as_str()
        .expect("a backup key")
        .to_string();
    assert!(backup_key.starts_with("AGE-SECRET-KEY-1"));

    // A pending pair still seals nothing: the gate holds until the paste-back.
    let status = recipient_status(&client, &api).await;
    assert_eq!(status["pending"], json!(true));
    assert_eq!(status["recipient"], Value::Null);
    let refused = put_schedule(&client, &api, "0 0 3 * * *").await;
    assert_eq!(refused.status().as_u16(), 409);

    // A different key, or no key at all, does not pass the gate.
    let (other, _) = crypto::generate_key_pair();
    let resp = post_json(
        &client,
        &confirm_url,
        json!({ "backup_key": crypto::backup_key_text(&other) }),
    )
    .await;
    assert_eq!(resp.status().as_u16(), 400);
    assert_eq!(reason(resp).await, "key-mismatch");
    let resp = post_json(&client, &confirm_url, json!({ "backup_key": "not a key" })).await;
    assert_eq!(resp.status().as_u16(), 400);
    assert_eq!(
        recipient_status(&client, &api).await["recipient"],
        Value::Null
    );

    // The right key passes, and now a schedule may turn on.
    let resp = post_json(&client, &confirm_url, json!({ "backup_key": backup_key })).await;
    assert_eq!(resp.status().as_u16(), 200);
    let status: Value = resp.json().await.unwrap();
    let expected = crypto::parse_backup_key(&backup_key)
        .unwrap()
        .to_public()
        .to_string();
    assert_eq!(status["recipient"], json!(expected));
    assert_eq!(status["pending"], json!(false));
    let resp = put_schedule(&client, &api, "0 0 3 * * *").await;
    assert_eq!(resp.status().as_u16(), 200);
    put_schedule(&client, &api, "off").await;

    // Verify tells the held key from another, and forgets both.
    let resp = post_json(&client, &verify_url, json!({ "backup_key": backup_key })).await;
    assert_eq!(resp.json::<Value>().await.unwrap()["matches"], json!(true));
    let resp = post_json(
        &client,
        &verify_url,
        json!({ "backup_key": crypto::backup_key_text(&other) }),
    )
    .await;
    assert_eq!(resp.json::<Value>().await.unwrap()["matches"], json!(false));

    assert_no_backup_key_is_stored().await;
    assert_an_app_document_cannot_change_the_recipient(&client, &api).await;
}

/// Setup, confirm and verify leave no backup key in any table or event.
async fn assert_no_backup_key_is_stored() {
    let pool = pool().await;
    let rows: Vec<String> = sqlx::query_scalar(
        "SELECT row_to_json(b)::text FROM backup_recipient b \
         UNION ALL SELECT payload::text FROM events",
    )
    .fetch_all(&pool)
    .await
    .expect("query failed");
    pool.close().await;
    assert!(
        rows.iter().all(|row| !row.contains("AGE-SECRET-KEY")),
        "a backup key reached the database"
    );
}

async fn assert_an_app_document_cannot_change_the_recipient(client: &reqwest::Client, api: &str) {
    let app_referer = format!("{api}/app/habit-tracker/");
    for path in ["setup", "confirm", "verify"] {
        let resp = client
            .post(format!("{api}/api/v1/backup/recipient/{path}"))
            .json(&json!({ "backup_key": "x" }))
            .header("sec-fetch-site", "same-origin")
            .header("referer", &app_referer)
            .send()
            .await
            .expect("app-origin request failed");
        assert_eq!(
            resp.status().as_u16(),
            403,
            "{path} must refuse an app-iframe Referer"
        );
    }
}

/// A workspace with only a legacy key keeps backing up with it, can reveal it
/// behind a one-shot token, and loses it at the switch.
#[tokio::test]
async fn a_legacy_key_seals_until_the_switch_and_reveals_behind_a_token() {
    let _lock = crate::support::backup_key_lock().lock().await;
    let legacy = LegacyKeyGuard::capture();
    forget_recipient().await;
    let key = SecretKey::generate();
    legacy.put(&key);
    let client = user_client().await;
    let api = base_url();
    let key_url = format!("{api}/api/v1/backup/key");

    let status = recipient_status(&client, &api).await;
    assert_eq!(status["legacy_key"], json!(true));
    assert_eq!(status["recipient"], Value::Null);

    // A pending switch never pauses backups.
    let resp = put_schedule(&client, &api, "0 0 3 * * *").await;
    assert_eq!(resp.status().as_u16(), 200);
    put_schedule(&client, &api, "off").await;

    // The reveal spends a token once and returns the key.
    let token = key_token(&client, &api).await;
    let reveal = client
        .get(&key_url)
        .query(&[("token", token.as_str())])
        .send()
        .await
        .expect("reveal failed");
    assert_eq!(reveal.status().as_u16(), 200);
    assert_eq!(
        reveal.json::<Value>().await.unwrap()["key"],
        json!(key.to_base64())
    );
    let replay = client
        .get(&key_url)
        .query(&[("token", token.as_str())])
        .send()
        .await
        .expect("replay failed");
    assert_eq!(replay.status().as_u16(), 403, "a reveal token is one-shot");
    assert!(!replay.text().await.unwrap().contains(&key.to_base64()));

    let bare = client.get(&key_url).send().await.expect("bare read failed");
    assert_eq!(bare.status().as_u16(), 403);
    assert!(bare
        .text()
        .await
        .unwrap()
        .contains("/api/v1/backup/key/reveal-token"));

    let app_referer = format!("{api}/app/habit-tracker/");
    for (method, url) in [
        ("POST", format!("{api}/api/v1/backup/key/reveal-token")),
        ("GET", key_url.clone()),
    ] {
        let req = if method == "POST" {
            client.post(&url)
        } else {
            client.get(&url)
        };
        let resp = req
            .query(&[("token", "anything")])
            .header("sec-fetch-site", "same-origin")
            .header("referer", &app_referer)
            .send()
            .await
            .expect("app-origin request failed");
        assert_eq!(resp.status().as_u16(), 403, "{method} {url}");
    }
    assert_the_reveal_is_on_the_record(&key.to_base64()).await;

    // The switch: an own recipient takes over and the legacy key is gone.
    let (_, own) = crypto::generate_key_pair();
    let resp = client
        .put(format!("{api}/api/v1/backup/recipient"))
        .json(&json!({ "recipient": own.to_string() }))
        .send()
        .await
        .expect("own recipient failed");
    assert_eq!(resp.status().as_u16(), 200);
    let status: Value = resp.json().await.unwrap();
    assert_eq!(status["recipient"], json!(own.to_string()));
    assert_eq!(status["legacy_key"], json!(false));
    assert_eq!(legacy.stored(), None);

    let token = key_token(&client, &api).await;
    let gone = client
        .get(&key_url)
        .query(&[("token", token)])
        .send()
        .await
        .expect("reveal failed");
    assert_eq!(gone.status().as_u16(), 404);
}

/// Mint a one-shot reveal token for the legacy key. Step one of two.
async fn key_token(client: &reqwest::Client, api: &str) -> String {
    let resp = client
        .post(format!("{api}/api/v1/backup/key/reveal-token"))
        .send()
        .await
        .expect("mint failed");
    assert_eq!(resp.status().as_u16(), 200, "mint must 200");
    let body: Value = resp.json().await.expect("invalid json");
    assert!(body["expires_in_secs"].as_u64().unwrap_or(0) > 0);
    body["token"].as_str().expect("token").to_string()
}

/// The reveal is on the record, and no row carries the key.
async fn assert_the_reveal_is_on_the_record(key: &str) {
    let pool = pool().await;
    let rows: Vec<Value> = sqlx::query_scalar(
        "SELECT payload FROM events WHERE event_type = 'BackupKeyRevealed' \
         ORDER BY sequence DESC LIMIT 8",
    )
    .fetch_all(&pool)
    .await
    .expect("query failed");
    pool.close().await;
    assert!(!rows.is_empty(), "the reveal owes a row");
    for row in &rows {
        assert!(!row.to_string().contains(key), "{row}");
    }
}
