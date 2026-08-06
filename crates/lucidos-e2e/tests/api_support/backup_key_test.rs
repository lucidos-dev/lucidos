//! E2E coverage for the backup-key HTTP contract.
//!
//! Pins the behavior behind the user-reported confusion: pressing "Show backup
//! key" surfaced a "New backup key generated" toast even though earlier backups
//! existed. The fix splits the surface so a *reveal* (GET) never mints a key and
//! *generation* (POST) is explicit and idempotent — so the key that protects
//! existing backups can never be silently overwritten.
//!
//! The engine reads the key file fresh per request (no in-memory cache), so
//! manipulating `<workspace>/.lucidos/backup.key` takes effect immediately
//! without an engine restart. A drop guard restores the original key (or its
//! absence) so the test is hermetic even on panic.

use crate::support::{base_url, http_client, workspace_path};
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
    let client = http_client();
    let key_url = format!("{}/api/v1/backup/key", base_url());
    let exists_url = format!("{}/api/v1/backup/key/exists", base_url());

    // --- Start from a known-absent state ---
    _guard.remove();

    // exists probe reports false WITHOUT minting a key.
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
    let reveal_absent = client.get(&key_url).send().await.expect("reveal failed");
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
    let gen = client.post(&key_url).send().await.expect("generate failed");
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
    let reveal = client.get(&key_url).send().await.expect("reveal failed");
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

    // --- Generate again: idempotent, NEVER overwrites ---
    // This is the core guarantee the user asked about: pressing the button a
    // second time (or a scheduled backup racing it) must not replace the key.
    let regen = client
        .post(&key_url)
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
}
