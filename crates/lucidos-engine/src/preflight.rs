//! `lucidos-engine preflight`: boot this binary against a throwaway copy of the
//! workspace database, so a build that cannot boot is never published.
//!
//! The copy runs the two steps that fail a boot on user data: the embedded
//! migrator, then the secret-store boot check with its sealing pass. The copy is
//! dropped whatever happens. The source database and the key store are never
//! written (plan `docs/plans/2026-10-18-boot-failure-guards.md`).

use std::collections::HashMap;
use std::path::Path;
use std::str::FromStr;
use std::sync::{Mutex, PoisonError};

use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use sqlx::PgPool;

use crate::core::backup::pg_tool_command;
use crate::core::secret_store::boot::{self, Boot};
use crate::core::secret_store::key_store::KeyStore;
use crate::core::secret_store::SecretKey;

type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// Postgres cuts an identifier at this many bytes.
const MAX_DATABASE_NAME_BYTES: usize = 63;

/// Parallel jobs for the dump and the restore.
const COPY_JOBS: &str = "4";

/// A key store that reads the real one and keeps every write in memory. Boot
/// mints a key for a database with nothing sealed, and a restore parks one.
/// Both run here as in a boot, and the real store stays untouched.
struct ScratchKeys<'a> {
    real: &'a dyn KeyStore,
    /// `None` is a deleted key.
    changes: Mutex<HashMap<String, Option<SecretKey>>>,
}

impl<'a> ScratchKeys<'a> {
    fn new(real: &'a dyn KeyStore) -> Self {
        Self {
            real,
            changes: Mutex::default(),
        }
    }

    fn record(&self, name: &str, key: Option<SecretKey>) {
        let mut changes = self.changes.lock().unwrap_or_else(PoisonError::into_inner);
        changes.insert(name.to_string(), key);
    }
}

impl KeyStore for ScratchKeys<'_> {
    fn describe(&self, name: &str) -> String {
        self.real.describe(name)
    }

    fn get(&self, name: &str) -> Result<Option<SecretKey>, BoxError> {
        let changes = self.changes.lock().unwrap_or_else(PoisonError::into_inner);
        match changes.get(name) {
            Some(change) => Ok(change.clone()),
            None => self.real.get(name),
        }
    }

    fn put(&self, name: &str, key: &SecretKey) -> Result<(), BoxError> {
        self.record(name, Some(key.clone()));
        Ok(())
    }

    fn delete(&self, name: &str) -> Result<(), BoxError> {
        self.record(name, None);
        Ok(())
    }
}

/// What every copy of `source` starts with: the source name, cut so the
/// longest process id still fits, plus a marker.
fn copy_prefix(source: &str) -> String {
    const MARKER: &str = "_preflight_";
    const PID_DIGITS: usize = 10;
    let room = MAX_DATABASE_NAME_BYTES - MARKER.len() - PID_DIGITS;
    format!("{}{MARKER}", &source[..source.floor_char_boundary(room)])
}

fn copy_name(source: &str, pid: u32) -> String {
    format!("{}{pid}", copy_prefix(source))
}

/// `text` as a literal `LIKE` pattern.
fn like_escape(text: &str) -> String {
    text.replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

fn quote_identifier(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

/// Copy the database at `database_url`, boot the copy, and drop it.
///
/// `scratch_dir` holds the dump for the length of the copy. `Err` says why a
/// boot of this binary on this data would fail. It carries the server's
/// primary message only, never a `Debug` form that would quote a failing row.
pub async fn run(
    database_url: &str,
    scratch_dir: &Path,
    store: &dyn KeyStore,
) -> Result<(), BoxError> {
    copy_and_boot(database_url, scratch_dir, store)
        .await
        .map_err(readable)
}

/// `e` as its `Display` text, which for a database error is the server's
/// primary message.
fn readable(e: BoxError) -> BoxError {
    e.to_string().into()
}

async fn copy_and_boot(
    database_url: &str,
    scratch_dir: &Path,
    store: &dyn KeyStore,
) -> Result<(), BoxError> {
    let source = boot::database_name(database_url)?;
    let copy = copy_name(&source, std::process::id());
    let admin = PgPoolOptions::new()
        .max_connections(1)
        .connect(database_url)
        .await?;
    drop_leftover_copies(&admin, &source).await?;
    sqlx::query(&format!("CREATE DATABASE {}", quote_identifier(&copy)))
        .execute(&admin)
        .await?;
    log!("[Preflight] Copying {source} to {copy}");
    let outcome = boot_copy(database_url, &source, &copy, scratch_dir, store).await;
    if let Err(e) = drop_database(&admin, &copy).await {
        log!("[Preflight] Could not drop {copy}: {e}");
    }
    admin.close().await;
    outcome
}

/// Drop the copies a killed preflight left behind. The build lock lets one
/// preflight run per checkout, so every copy found is stale.
async fn drop_leftover_copies(admin: &PgPool, source: &str) -> Result<(), BoxError> {
    let leftovers: Vec<String> =
        sqlx::query_scalar("SELECT datname::text FROM pg_database WHERE datname LIKE $1")
            .bind(format!("{}%", like_escape(&copy_prefix(source))))
            .fetch_all(admin)
            .await?;
    for name in leftovers {
        log!("[Preflight] Dropping {name}, left by an earlier run");
        drop_database(admin, &name).await?;
    }
    Ok(())
}

async fn drop_database(admin: &PgPool, name: &str) -> Result<(), BoxError> {
    sqlx::query(&format!(
        "DROP DATABASE IF EXISTS {} WITH (FORCE)",
        quote_identifier(name)
    ))
    .execute(admin)
    .await?;
    Ok(())
}

async fn boot_copy(
    database_url: &str,
    source: &str,
    copy: &str,
    scratch_dir: &Path,
    store: &dyn KeyStore,
) -> Result<(), BoxError> {
    let dump_dir = tempfile::Builder::new()
        .prefix("lucidos-preflight-")
        .tempdir_in(scratch_dir)?;
    let archive = dump_dir.path().join("dump");
    run_pg_tool("pg_dump", database_url, None, |cmd| {
        cmd.args(["--format=directory", "--no-owner", "--no-acl", "--jobs"])
            .arg(COPY_JOBS)
            .arg("--file")
            .arg(&archive);
    })
    .await?;
    run_pg_tool("pg_restore", database_url, Some(copy), |cmd| {
        cmd.args(["--no-owner", "--no-acl", "--jobs"])
            .arg(COPY_JOBS)
            .arg("--dbname")
            .arg(copy)
            .arg(&archive);
    })
    .await?;
    drop(dump_dir);

    let pool = PgPoolOptions::new()
        .max_connections(2)
        .connect_with(PgConnectOptions::from_str(database_url)?.database(copy))
        .await?;
    let outcome = migrate_and_unseal(&pool, source, store).await;
    pool.close().await;
    outcome
}

/// The steps a boot runs on user data, in the order it runs them.
async fn migrate_and_unseal(
    pool: &PgPool,
    source: &str,
    store: &dyn KeyStore,
) -> Result<(), BoxError> {
    sqlx::migrate!()
        .run(pool)
        .await
        .map_err(|e| format!("migrations failed: {e}"))?;
    match boot::boot(pool, source, &ScratchKeys::new(store)).await? {
        Boot::Ready(_) => Ok(()),
        Boot::Refused(message) => Err(message.into()),
    }
}

/// Run a Postgres client tool against `database_url`, or against `database`
/// on the same server, and fail with the first line of its stderr. Later
/// lines can quote a row.
async fn run_pg_tool(
    name: &str,
    database_url: &str,
    database: Option<&str>,
    configure: impl FnOnce(&mut std::process::Command),
) -> Result<(), BoxError> {
    let mut cmd = pg_tool_command(name, database_url)?;
    if let Some(database) = database {
        cmd.env("PGDATABASE", database);
    }
    configure(&mut cmd);
    let output = tokio::process::Command::from(cmd).output().await?;
    if output.status.success() {
        return Ok(());
    }
    Err(format!(
        "{name} failed with {}: {}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
            .lines()
            .next()
            .unwrap_or_default()
    )
    .into())
}

#[cfg(test)]
#[path = "preflight_tests.rs"]
mod tests;
