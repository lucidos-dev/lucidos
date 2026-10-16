//! E2E coverage for the backup-key HTTP contract.
//!
//! Pins the behavior behind the user-reported confusion: pressing "Show backup
//! key" surfaced a "New backup key generated" toast even though earlier backups
//! existed. The fix splits the surface so a *reveal* (GET) never mints a key and
//! *generation* (POST) is explicit and idempotent — so the key that protects
//! existing backups can never be silently overwritten.
//!
//! It also pins the guard in front of both. The key decrypts every archive this
//! workspace uploaded. App UIs are same-origin with the engine, so the reveal
//! used to be one bare GET away from any installed app. Both key-bearing routes
//! now spend a one-shot token and refuse an app `Referer` (ADR 0117).
//!
//! The engine reads the key file fresh per request (no in-memory cache), so
//! manipulating `<workspace>/.lucidos/backup.key` takes effect immediately
//! without an engine restart. A drop guard restores the original key (or its
//! absence) so the test is hermetic even on panic.

use crate::support::{base_url, db_url, user_client, workspace_path};
use sqlx::PgPool;
use std::path::PathBuf;

/// Restores the workspace's backup key file to its pre-test state on drop, so
/// this test never strands the shared e2e workspace with a different key.
struct KeyFileGuard {
    path: PathBuf,
    original: Option<Vec<u8>>,
}

impl KeyFileGuard {
    fn capture() -> Self {
        let path = workspace_path().join(".lucidos").join("backup.key");
        let original = std::fs::read(&path).ok();
        Self { path, original }
    }

    fn remove(&self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

impl Drop for KeyFileGuard {
    fn drop(&mut self) {
        match &self.original {
            Some(bytes) => {
                let _ = std::fs::write(&self.path, bytes);
            }
            None => {
                let _ = std::fs::remove_file(&self.path);
            }
        }
    }
}

fn decode_key_len(b64: &str) -> usize {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD
        .decode(b64.trim())
        .expect("key must be valid base64")
        .len()
}

/// Mint a one-shot reveal token for the backup key. Step one of two.
///
/// Takes no id: a workspace has exactly one backup key, so the token is bound
/// to that subject rather than to a row.
async fn key_token(client: &reqwest::Client, api: &str) -> String {
    let resp = client
        .post(format!("{}/api/v1/backup/key/reveal-token", api))
        .send()
        .await
        .expect("mint failed");
    assert_eq!(resp.status().as_u16(), 200, "mint must 200");
    let body: serde_json::Value = resp.json().await.expect("invalid json");
    assert!(
        body["expires_in_secs"].as_u64().unwrap_or(0) > 0,
        "the mint must tell the caller how long it has"
    );
    body["token"].as_str().expect("token").to_string()
}

/// One end-to-end pass over the whole contract, in a single test so the
/// assertions don't race each other on the shared key file.
///
/// The lock covers the OTHER file that touches the key: enabling a backup
/// schedule calls `crypto::ensure_key`, so `backup_schedule_test` would re-mint
/// the key this test has deliberately removed, and the failure would land here.
#[tokio::test]
async fn backup_key_reveal_is_read_only_and_generate_never_overwrites() {
    let _lock = crate::support::backup_key_lock().lock().await;
    let _guard = KeyFileGuard::capture();
    let client = user_client().await;
    let api = base_url();
    let key_url = format!("{}/api/v1/backup/key", api);
    let exists_url = format!("{}/api/v1/backup/key/exists", api);

    // --- Start from a known-absent state ---
    _guard.remove();

    // exists probe reports false WITHOUT minting a key, and needs no token:
    // it reveals nothing, and the page calls it on load to label its button.
    let exists: serde_json::Value = client
        .get(&exists_url)
        .send()
        .await
        .expect("exists probe failed")
        .json()
        .await
        .expect("exists JSON");
    assert_eq!(exists["exists"], serde_json::json!(false));

    // GET reveal on an absent key is a clean 404 — it must NOT generate one.
    let reveal_absent = client
        .get(&key_url)
        .query(&[("token", key_token(&client, &api).await)])
        .send()
        .await
        .expect("reveal failed");
    assert_eq!(
        reveal_absent.status().as_u16(),
        404,
        "revealing an absent key must 404, never silently generate"
    );

    // The probe still reports absent — the failed reveal created nothing.
    let still_absent: serde_json::Value = client
        .get(&exists_url)
        .send()
        .await
        .expect("exists probe failed")
        .json()
        .await
        .expect("exists JSON");
    assert_eq!(
        still_absent["exists"],
        serde_json::json!(false),
        "a 404 reveal must not have minted a key"
    );

    // --- Generate: the explicit, only user-facing mint path ---
    let gen = client
        .post(&key_url)
        .query(&[("token", key_token(&client, &api).await)])
        .send()
        .await
        .expect("generate failed");
    assert_eq!(gen.status().as_u16(), 200);
    let gen_body: serde_json::Value = gen.json().await.expect("generate JSON");
    let key1 = gen_body["key"].as_str().expect("key string").to_string();
    assert_eq!(
        gen_body["is_new"],
        serde_json::json!(true),
        "first generate must report is_new"
    );
    assert_eq!(decode_key_len(&key1), 32, "AES-256 key must be 32 bytes");

    // exists now reports true.
    let exists_after: serde_json::Value = client
        .get(&exists_url)
        .send()
        .await
        .expect("exists probe failed")
        .json()
        .await
        .expect("exists JSON");
    assert_eq!(exists_after["exists"], serde_json::json!(true));

    // --- Reveal: read-only, returns the SAME key, is_new = false ---
    let token = key_token(&client, &api).await;
    let reveal = client
        .get(&key_url)
        .query(&[("token", token.as_str())])
        .send()
        .await
        .expect("reveal failed");
    assert_eq!(reveal.status().as_u16(), 200);
    let reveal_body: serde_json::Value = reveal.json().await.expect("reveal JSON");
    assert_eq!(
        reveal_body["key"].as_str().unwrap(),
        key1,
        "reveal must return the existing key unchanged"
    );
    assert_eq!(
        reveal_body["is_new"],
        serde_json::json!(false),
        "a reveal never reports is_new"
    );

    // A token spends exactly once, so a replay reaches no key.
    let replay = client
        .get(&key_url)
        .query(&[("token", token.as_str())])
        .send()
        .await
        .expect("replay failed");
    assert_eq!(
        replay.status().as_u16(),
        403,
        "a reveal token is one-shot; a replay must be refused"
    );
    assert!(
        !replay.text().await.unwrap().contains(&key1),
        "a refusal must never carry the key"
    );

    // --- Generate again: idempotent, NEVER overwrites ---
    // This is the core guarantee the user asked about: pressing the button a
    // second time (or a scheduled backup racing it) must not replace the key.
    let regen = client
        .post(&key_url)
        .query(&[("token", key_token(&client, &api).await)])
        .send()
        .await
        .expect("re-generate failed");
    assert_eq!(regen.status().as_u16(), 200);
    let regen_body: serde_json::Value = regen.json().await.expect("re-generate JSON");
    assert_eq!(
        regen_body["key"].as_str().unwrap(),
        key1,
        "generating again must return the SAME key — never overwrite it"
    );
    assert_eq!(
        regen_body["is_new"],
        serde_json::json!(false),
        "an existing key must report is_new = false on a repeat generate"
    );

    assert_key_needs_a_token(&client, &key_url, &key1).await;
    assert_an_app_document_is_refused(&client, &api, &key_url).await;
    assert_the_reveals_are_on_the_record(&key1).await;
}

/// Neither key-bearing route answers without a live token, and neither refusal
/// carries the key. The generate route is included because it is idempotent: on
/// a workspace that already has a key it hands back the same plaintext.
async fn assert_key_needs_a_token(client: &reqwest::Client, key_url: &str, key: &str) {
    for method in ["GET", "POST"] {
        let req = if method == "GET" {
            client.get(key_url)
        } else {
            client.post(key_url)
        };
        let resp = req.send().await.expect("token-less request failed");
        assert_eq!(
            resp.status().as_u16(),
            403,
            "{method} with no token must not reach the key file"
        );
        let body = resp.text().await.unwrap();
        assert!(!body.contains(key), "a refusal must never carry the key");
        assert!(
            body.contains("/api/v1/backup/key/reveal-token"),
            "the refusal must name the route that mints: {body}"
        );
    }
}

/// Regression: an app iframe cannot read the key.
///
/// App UIs load at `/app/<id>/` on the engine's own origin, so `Sec-Fetch-Site`
/// reads `same-origin` for them exactly as for the Settings page. The `Referer`
/// is the only thing that differs, and all three routes now read it.
async fn assert_an_app_document_is_refused(client: &reqwest::Client, api: &str, key_url: &str) {
    let app_referer = format!("{}/app/habit-tracker/", api);
    let mint_url = format!("{}/api/v1/backup/key/reveal-token", api);
    for (method, url) in [
        ("POST", mint_url.as_str()),
        ("GET", key_url),
        ("POST", key_url),
    ] {
        let req = if method == "POST" {
            client.post(url)
        } else {
            client.get(url)
        };
        let resp = req
            .query(&[("token", "anything")])
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
}

/// Every successful reveal above is on the record, and no row carries the key.
/// An app that does defeat the origin check cannot do it quietly.
async fn assert_the_reveals_are_on_the_record(key: &str) {
    let pool = PgPool::connect(&db_url())
        .await
        .expect("Failed to connect to database");
    let rows: Vec<serde_json::Value> = sqlx::query_scalar(
        "SELECT payload FROM events WHERE event_type = 'BackupKeyRevealed' \
         ORDER BY sequence DESC LIMIT 8",
    )
    .fetch_all(&pool)
    .await
    .expect("query failed");
    pool.close().await;

    assert!(
        rows.len() >= 3,
        "the generate, the reveal and the re-generate each owe a row: {rows:?}"
    );
    for row in &rows {
        assert!(
            row["data"]["minted"].is_boolean(),
            "the row must say whether this call created the key: {row}"
        );
        assert!(
            !row.to_string().contains(key),
            "the audit row must never carry the key: {row}"
        );
    }
    assert!(
        rows.iter().any(|r| r["data"]["minted"] == true),
        "the first generate minted the key and must say so: {rows:?}"
    );
}
