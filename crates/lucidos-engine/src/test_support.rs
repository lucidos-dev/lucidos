//! Shared helpers for unit tests that need a real Postgres connection.
//! Each call to `setup_test_db` creates a fresh database, runs migrations,
//! and returns the pool plus the database name to pass back to `teardown_test_db`.

#![cfg(test)]

use sqlx::postgres::{PgPool, PgPoolOptions};
use std::path::PathBuf;
use uuid::Uuid;

use crate::engine::event_bus::{BusEvent, EventBus};
use crate::engine::thread_events::{EventChannel, EventMeta, ThreadEvent};

fn admin_url() -> String {
    std::env::var("TEST_DATABASE_URL")
        .unwrap_or_else(|_| "postgres://lucidos:lucidos@localhost:5432/postgres".into())
}

/// Connection URL for a throwaway database created by `setup_test_db`. Lets a
/// test open its own dedicated connection to the same DB (e.g. the startup-lease
/// tests, which contend a Postgres advisory lock across independent connections).
pub fn test_db_url(db_name: &str) -> String {
    admin_url().replace("/postgres", &format!("/{}", db_name))
}

pub async fn setup_test_db() -> (PgPool, String) {
    let base_url = admin_url();
    let db_name = format!(
        "lucidos_test_{}",
        Uuid::new_v4().to_string().replace('-', "")
    );
    let admin_pool = PgPoolOptions::new()
        .max_connections(1)
        .connect(&base_url)
        .await
        .expect("admin connect");
    sqlx::query(&format!("CREATE DATABASE \"{}\"", db_name))
        .execute(&admin_pool)
        .await
        .expect("create db");
    admin_pool.close().await;
    let test_url = test_db_url(&db_name);
    let pool = PgPoolOptions::new()
        .max_connections(5)
        .connect(&test_url)
        .await
        .expect("connect test db");
    sqlx::migrate!().run(&pool).await.expect("migrations");
    (pool, db_name)
}

/// Build a tiny git repo with `main` + initial commit and a worktree on
/// `branch`. Returns `(tmpdir, repo_root, worktree_path)`. Caller must keep
/// `tmpdir` in scope for the test duration — dropping it `rm -rf`'s the repo.
///
/// Shared by tests that need a real on-disk git repo + worktree pair (the
/// `seed_coding_agent_has_diff` tests in `session_seed_tests.rs` and the startup
/// sweep tests in `agent_recovery_tests.rs`). The same boilerplate also lives
/// in git_ops_tests.rs / worktree_cleanup_tests.rs — those older copies are out
/// of scope for this helper, follow-up DRY sweep.
pub async fn make_repo_and_worktree(branch: &str) -> (tempfile::TempDir, PathBuf, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    std::fs::create_dir(&repo).unwrap();

    use crate::engine::git_ops::git_cmd;
    git_cmd(&["init", "-b", "main"], &repo).await.unwrap();
    git_cmd(&["config", "user.email", "test@example.com"], &repo)
        .await
        .unwrap();
    git_cmd(&["config", "user.name", "Test"], &repo)
        .await
        .unwrap();
    std::fs::write(repo.join("seed.txt"), "x").unwrap();
    git_cmd(&["add", "."], &repo).await.unwrap();
    git_cmd(&["commit", "-m", "init"], &repo).await.unwrap();

    let wt = tmp.path().join("wt");
    git_cmd(
        &["worktree", "add", wt.to_str().unwrap(), "-b", branch],
        &repo,
    )
    .await
    .unwrap();

    (tmp, repo, wt)
}

/// Read the `coding_agent_has_diff` projection column for a single thread.
/// Panics if the row is missing — every caller seeds a `SessionStarted` first,
/// so a missing row means the projection write didn't land and the test should
/// fail loudly rather than silently observe `false`.
pub async fn read_coding_agent_has_diff(pool: &PgPool, thread_id: Uuid) -> bool {
    sqlx::query_scalar::<_, bool>(
        "SELECT coding_agent_has_diff FROM thread_summaries WHERE thread_id = $1",
    )
    .bind(thread_id)
    .fetch_one(pool)
    .await
    .unwrap()
}

/// Emit `SessionStarted` on the `CodingAgent` channel so the projection
/// upserts an `is_coding_agent=true, state='active'` row in `thread_summaries`. Pass
/// `repo_id = Some(...)` to stamp `cc_repo_id` on the projection row (the
/// external-repo branch); `None` covers the in-workspace common case.
///
/// `session_id` is a fixed sentinel — no test reads it back. The branch and
/// repo_id are the per-test inputs that actually matter.
pub async fn start_cc_session(
    bus: &EventBus,
    thread_id: Uuid,
    branch: &str,
    repo_id: Option<String>,
) {
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::SessionStarted {
            coding_agent: crate::runtime::CodingAgent::ClaudeCode,
            session_id: "test-session".into(),
            branch: branch.into(),
            repo_id,
            coding_agent_kind: Default::default(),
            coding_agent_folder: String::new(),
            app_id: None,
        },
        meta: EventMeta {
            channel: Some(EventChannel::ClaudeCode),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();
}

/// Seed a credential for a test that only cares that one exists.
///
/// The store's mutators take the `EventBus` because a credential write and its
/// `Credential{Created,Updated}` announcement are one operation (see
/// `CredentialStore`'s type doc). A fixture has no bus of its own and no
/// interest in the event, so this builds a throwaway one against the test
/// database rather than pushing that ceremony into every seeding call site.
/// Tests that assert ON the event build their own bus and call the store
/// directly.
pub async fn seed_credential(
    pool: &PgPool,
    service_name: &str,
    base_url: &str,
    auth_type: crate::core::AuthType,
    auth_value: &str,
) {
    let (bus, _callback_rx) = EventBus::new(pool.clone());
    crate::core::CredentialStore::upsert(
        pool,
        &bus,
        service_name,
        base_url,
        auth_type,
        auth_value,
        None,
        None,
        None,
    )
    .await
    .unwrap_or_else(|e| panic!("seed credential {service_name}: {e}"));
}

/// Delete a credential in a test.
///
/// Exists so callers outside the engine layer do not have to name
/// `crate::engine::event_bus` to build a bus for the store's
/// `CredentialDeleted` emit: `llm/*.rs` is forbidden from depending on
/// `crate::engine` (see `llm::validate::tests::llm_does_not_depend_on_engine`).
/// Still takes a NAME rather than the store's id, because that is what a
/// fixture has in hand. It resolves the id first, so a caller does not have to
/// thread one through just to clean up after itself.
pub async fn delete_credential(pool: &PgPool, service_name: &str) {
    let (bus, _callback_rx) = EventBus::new(pool.clone());
    let Some(id) = sqlx::query_scalar::<_, uuid::Uuid>(
        "SELECT id FROM credentials WHERE service_name = $1 LIMIT 1",
    )
    .bind(service_name)
    .fetch_optional(pool)
    .await
    .unwrap_or_else(|e| panic!("look up credential {service_name}: {e}")) else {
        return;
    };
    crate::core::CredentialStore::delete(pool, &bus, id, None)
        .await
        .unwrap_or_else(|e| panic!("delete credential {service_name}: {e}"));
}

/// Register a device and give it a display name, for a test that needs one to
/// exist (actor resolution, presence, device-scoped preferences).
///
/// Same rationale as [`seed_credential`]: the store's mutators own their
/// `Device{Registered,Renamed}` emits, and a fixture has no bus of its own.
pub async fn seed_device(pool: &PgPool, id: &str, user_agent: Option<&str>, name: Option<&str>) {
    let (bus, _callback_rx) = EventBus::new(pool.clone());
    crate::core::DeviceStore::register(pool, &bus, id, user_agent, None)
        .await
        .unwrap_or_else(|e| panic!("seed device {id}: {e}"));
    if name.is_some() {
        crate::core::DeviceStore::rename(pool, &bus, id, name, None)
            .await
            .unwrap_or_else(|e| panic!("name device {id}: {e}"));
    }
}

/// Seed a connected OAuth account for a test that needs one to exist (token
/// resolution, provider routing, scope merging).
///
/// Argument order mirrors `OAuthStore::connect` minus the bus and actor, which
/// a fixture has no opinion about. Same rationale as [`seed_credential`].
#[allow(clippy::too_many_arguments)] // mirrors the store's column list
pub async fn seed_oauth_account(
    pool: &PgPool,
    provider: &str,
    email: Option<&str>,
    display_name: Option<&str>,
    access_token: &str,
    refresh_token: Option<&str>,
    token_expiry: Option<chrono::DateTime<chrono::Utc>>,
    scopes: &str,
) -> Result<uuid::Uuid, sqlx::Error> {
    seed_oauth_account_with_desired(
        pool,
        provider,
        email,
        display_name,
        access_token,
        refresh_token,
        token_expiry,
        scopes,
        scopes,
    )
    .await
}

/// Seed an account whose GRANTED scopes differ from the set it was ASKED for.
///
/// The shape a provider produces when it refuses part of a request (a Dropbox
/// app whose Permissions tab has not enabled a scope). [`seed_oauth_account`]
/// cannot express it: it seeds granted and desired identically, which is the
/// ordinary case and says nothing about recovery.
#[allow(clippy::too_many_arguments)] // mirrors the store's column list
pub async fn seed_oauth_account_with_desired(
    pool: &PgPool,
    provider: &str,
    email: Option<&str>,
    display_name: Option<&str>,
    access_token: &str,
    refresh_token: Option<&str>,
    token_expiry: Option<chrono::DateTime<chrono::Utc>>,
    scopes: &str,
    desired_scopes: &str,
) -> Result<uuid::Uuid, sqlx::Error> {
    let (bus, _callback_rx) = EventBus::new(pool.clone());
    crate::core::OAuthStore::connect(
        pool,
        &bus,
        provider,
        email,
        display_name,
        access_token,
        refresh_token,
        token_expiry,
        scopes,
        desired_scopes,
        None,
    )
    .await
}

/// Seed a global preference for a test that just needs one stored.
///
/// Same rationale as [`seed_credential`]: `PreferenceStore`'s writers announce
/// `PreferencesChanged`, and a fixture has no bus of its own. Returns the
/// store's `Result` so existing call sites keep their `.unwrap()`. Tests that
/// assert ON the announcement build a bus and call the store directly.
pub async fn seed_preference(pool: &PgPool, key: &str, value: &str) -> Result<(), sqlx::Error> {
    let (bus, _callback_rx) = EventBus::new(pool.clone());
    crate::core::PreferenceStore::set(pool, &bus, key, value, None).await
}

/// [`seed_preference`], scoped to one device.
pub async fn seed_preference_for_device(
    pool: &PgPool,
    key: &str,
    value: &str,
    device_id: &str,
) -> Result<(), sqlx::Error> {
    let (bus, _callback_rx) = EventBus::new(pool.clone());
    crate::core::PreferenceStore::set_for_device(pool, &bus, key, value, device_id, None).await
}

/// An `EventBus` backed by a pool that never dials, for tests whose subject is
/// a rejection or a filesystem effect and that never reach an emit.
///
/// `connect_lazy` builds the pool without touching the network, so this stays a
/// fast no-DB test. If a test using it DOES reach an emit, `emit_or_log` logs
/// the connection failure rather than panicking, which is the wrong shape for
/// an assertion: a test that cares about the event wants a real
/// `setup_test_db`.
pub fn offline_event_bus() -> EventBus {
    let pool = PgPoolOptions::new()
        .connect_lazy("postgres://lucidos:lucidos@127.0.0.1:1/offline")
        .expect("lazy pool");
    let (bus, _callback_rx) = EventBus::new(pool);
    bus
}

pub async fn teardown_test_db(db_name: &str) {
    let base_url = admin_url();
    let admin_pool = PgPoolOptions::new()
        .max_connections(1)
        .connect(&base_url)
        .await
        .expect("admin connect");
    let _ = sqlx::query(&format!(
        "SELECT pg_terminate_backend(pid) FROM pg_stat_activity WHERE datname = '{}'",
        db_name
    ))
    .execute(&admin_pool)
    .await;
    let _ = sqlx::query(&format!("DROP DATABASE IF EXISTS \"{}\"", db_name))
        .execute(&admin_pool)
        .await;
    admin_pool.close().await;
}

/// Reading the engine's own sources, for the repo's source-scan tripwires
/// (`announced_surfaces_tests`, the trigger write chokepoint guard).
///
/// Rust is not parsed. These helpers only locate the *production* text of each
/// file so a scan can look for a structure the codebase forbids. Test modules
/// are excluded by path convention and each file is truncated at its first
/// top-level `#[cfg(test)]`, because a fixture doing the forbidden thing is
/// setup rather than a real call site.
pub mod source_scan {
    use std::path::{Path, PathBuf};

    /// `crates/lucidos-engine/src`.
    pub fn src_root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("src")
    }

    /// A path convention test module.
    pub fn is_test_path(rel: &str) -> bool {
        rel.split('/').any(|part| {
            let base = part.strip_suffix(".rs").unwrap_or(part);
            base == "tests" || base == "bin" || base.ends_with("_test") || base.ends_with("_tests")
        })
    }

    /// Read a source file with its inline test module cut off. Inline
    /// `mod tests` sits at the end of the file by convention, so truncating at
    /// the first top-level `#[cfg(test)]` drops it whole.
    pub fn read_production_source(path: &Path) -> String {
        let text = std::fs::read_to_string(path)
            .unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
        match text.find("\n#[cfg(test)]") {
            Some(idx) => text[..idx].to_string(),
            None => text,
        }
    }

    /// Every non-test engine source, as `(path relative to src/, production text)`.
    pub fn production_sources() -> Vec<(String, String)> {
        let root = src_root();
        let mut out = Vec::new();
        let mut stack = vec![root.clone()];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).expect("read_dir").flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                    continue;
                }
                if path.extension().and_then(|e| e.to_str()) != Some("rs") {
                    continue;
                }
                let rel = path
                    .strip_prefix(&root)
                    .expect("under src")
                    .to_string_lossy()
                    .replace('\\', "/");
                if is_test_path(&rel) {
                    continue;
                }
                let text = read_production_source(&path);
                out.push((rel, text));
            }
        }
        out.sort();
        out
    }
}
