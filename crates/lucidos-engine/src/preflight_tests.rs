use std::sync::atomic::{AtomicUsize, Ordering};

use sqlx::PgPool;

use super::*;
use crate::core::secret_store::key_store::TestKeyStore;
use crate::test_support::{setup_test_db, teardown_test_db, test_db_url};

/// Counts the writes that reach it, and reads like the shared test store.
#[derive(Default)]
struct RecordingKeys {
    writes: AtomicUsize,
}

impl KeyStore for RecordingKeys {
    fn describe(&self, name: &str) -> String {
        TestKeyStore.describe(name)
    }

    fn get(&self, name: &str) -> Result<Option<SecretKey>, BoxError> {
        TestKeyStore.get(name)
    }

    fn put(&self, _name: &str, _key: &SecretKey) -> Result<(), BoxError> {
        self.writes.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    fn delete(&self, _name: &str) -> Result<(), BoxError> {
        self.writes.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

/// A store holding no key, which makes boot mint one.
struct EmptyKeys;

impl KeyStore for EmptyKeys {
    fn describe(&self, name: &str) -> String {
        format!("the empty store ({name})")
    }

    fn get(&self, _name: &str) -> Result<Option<SecretKey>, BoxError> {
        Ok(None)
    }

    fn put(&self, _name: &str, _key: &SecretKey) -> Result<(), BoxError> {
        panic!("a preflight write reached the real key store");
    }

    fn delete(&self, _name: &str) -> Result<(), BoxError> {
        panic!("a preflight delete reached the real key store");
    }
}

#[test]
fn scratch_keys_keep_writes_in_memory() {
    let inner = RecordingKeys::default();
    let store = ScratchKeys::new(&inner);
    let key = SecretKey::generate();
    assert!(store.get("name").unwrap().is_some(), "reads fall through");
    store.put("name", &key).unwrap();
    assert!(store.get("name").unwrap() == Some(key));
    store.delete("name").unwrap();
    assert!(store.get("name").unwrap().is_none());
    assert_eq!(inner.writes.load(Ordering::SeqCst), 0);
}

#[test]
fn the_copy_name_fits_a_database_name_and_keeps_the_marker() {
    assert_eq!(copy_name("lucidos_dev", 42), "lucidos_dev_preflight_42");
    let long = copy_name(&"d".repeat(80), u32::MAX);
    assert!(long.len() <= MAX_DATABASE_NAME_BYTES);
    assert!(long.starts_with(&copy_prefix(&"d".repeat(80))));
    assert!(long.ends_with("_preflight_4294967295"));
}

#[test]
fn like_escape_matches_only_itself() {
    assert_eq!(like_escape("a_b%c\\d"), "a\\_b\\%c\\\\d");
}

/// One digest of the rows a preflight must leave alone.
async fn fingerprint(pool: &PgPool) -> String {
    sqlx::query_scalar(
        "SELECT md5(\
           (SELECT coalesce(string_agg(m::text, ',' ORDER BY m.version), '') FROM _sqlx_migrations m) \
           || (SELECT coalesce(string_agg(m::text, ',' ORDER BY m.id), '') FROM models m))",
    )
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn copies_of(pool: &PgPool, source: &str) -> Vec<String> {
    sqlx::query_scalar("SELECT datname::text FROM pg_database WHERE datname LIKE $1")
        .bind(format!("{}%", like_escape(&copy_prefix(source))))
        .fetch_all(pool)
        .await
        .unwrap()
}

fn version_of(description: &str) -> i64 {
    sqlx::migrate!()
        .iter()
        .find(|m| m.description == description)
        .unwrap_or_else(|| panic!("no migration named {description}"))
        .version
}

/// Preflight fails where boot fails: here, on a missing successor row.
/// It passes once boot would. It drops its copy either way, and writes
/// nothing to the source database or the key store.
#[tokio::test]
async fn preflight_fails_like_boot_and_leaves_the_source_alone() {
    let (pool, db_name) = setup_test_db().await;
    let url = test_db_url(&db_name);
    let scratch = tempfile::tempdir().unwrap();
    let keys = RecordingKeys::default();
    let seed = version_of("seed the successors the retirement migration needs");
    let retire = version_of("retire superseded models and seed their successors");

    // A workspace that applied the seed, lost a successor row, and has not yet
    // applied the retirement: boot raises P0001.
    sqlx::query("DELETE FROM models WHERE id = 'claude-fable-5-1'")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM _sqlx_migrations WHERE version = $1")
        .bind(retire)
        .execute(&pool)
        .await
        .unwrap();
    let before = fingerprint(&pool).await;
    let err = run(&url, scratch.path(), &keys)
        .await
        .expect_err("boot would fail on this data");
    assert!(
        err.to_string()
            .contains("successor claude-fable-5-1 has no models row"),
        "{err}"
    );
    assert_eq!(fingerprint(&pool).await, before);
    assert!(copies_of(&pool, &db_name).await.is_empty());

    // The same workspace with the seed unapplied: boot restores the row first.
    sqlx::query("DELETE FROM _sqlx_migrations WHERE version = $1")
        .bind(seed)
        .execute(&pool)
        .await
        .unwrap();
    let before = fingerprint(&pool).await;
    run(&url, scratch.path(), &keys)
        .await
        .expect("boot succeeds on this data");
    assert_eq!(fingerprint(&pool).await, before);
    assert!(copies_of(&pool, &db_name).await.is_empty());
    assert_eq!(keys.writes.load(Ordering::SeqCst), 0);
    assert_eq!(std::fs::read_dir(scratch.path()).unwrap().count(), 0);

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// A killed preflight leaves its copy behind. The next run drops it.
#[tokio::test]
async fn preflight_drops_the_copy_a_killed_run_left_behind() {
    let (pool, db_name) = setup_test_db().await;
    let url = test_db_url(&db_name);
    let scratch = tempfile::tempdir().unwrap();
    let leftover = format!("{}1", copy_prefix(&db_name));
    sqlx::query(&format!("CREATE DATABASE {}", quote_identifier(&leftover)))
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(copies_of(&pool, &db_name).await, [leftover]);

    run(&url, scratch.path(), &RecordingKeys::default())
        .await
        .expect("boot succeeds on this data");

    assert!(copies_of(&pool, &db_name).await.is_empty());
    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// A database with nothing sealed and no key makes boot mint one. Preflight
/// does the same in memory, and never reaches the real store.
#[tokio::test]
async fn preflight_passes_where_boot_would_mint_a_key() {
    let (pool, db_name) = setup_test_db().await;
    let scratch = tempfile::tempdir().unwrap();
    run(&test_db_url(&db_name), scratch.path(), &EmptyKeys)
        .await
        .expect("boot mints a key here");
    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// A failing row's `detail` can hold an email address or a token. The error
/// preflight returns names the constraint and nothing of the row.
#[tokio::test]
async fn a_preflight_error_carries_no_row_data() {
    let (pool, db_name) = setup_test_db().await;
    sqlx::raw_sql(
        "CREATE TABLE preflight_probe (email text, token text, \
         CONSTRAINT probe_sealed CHECK (token LIKE 'sealed:%'))",
    )
    .execute(&pool)
    .await
    .unwrap();
    let violation = sqlx::query(
        "INSERT INTO preflight_probe VALUES ('person@example.test', 'plain-refresh-token')",
    )
    .execute(&pool)
    .await
    .expect_err("the CHECK refuses a plain token");

    let flattened = readable(Box::new(violation)).to_string();
    assert!(flattened.contains("probe_sealed"), "{flattened}");
    assert!(!flattened.contains("person@example.test"), "{flattened}");
    assert!(!flattened.contains("plain-refresh-token"), "{flattened}");

    pool.close().await;
    teardown_test_db(&db_name).await;
}
