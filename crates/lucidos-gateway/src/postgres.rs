//! Shared Postgres provisioning for the gateway.
//!
//! ADR 0014 §6/§7 moved the gateway from "one Postgres cluster per workspace"
//! to one shared cluster with one database per workspace:
//!
//! * **Docker** (dev): one durable `lucidos-pg-shared` container/volume, on
//!   loopback TCP with a minted password.
//! * **Embedded** (packaged): one bundled cluster under `<app-data>/pgdata`.
//!   It has no TCP listener. Its one socket sits in the owner-only cluster
//!   socket directory `<run directory>/pg-<digest>` (ADR 0452). The
//!   superuser signs in with a password kept beside the socket, which an
//!   agent's sandbox cannot read (ADR 0645).
//! * Each workspace gets its own database, `lucidos_<workspace-id>`, and the
//!   engine still receives a normal single-tenant `DATABASE_URL`.
//! * A legacy registry `database_url` is treated only as a migration source. If
//!   the shared workspace database does not exist yet, the gateway attempts a
//!   `pg_dump`/`pg_restore` migration from that URL; if the shared database
//!   already exists, the legacy URL is ignored so decommissioned old clusters do
//!   not break future starts.
//!
//! The gateway deliberately does not link sqlx (ADR 0014 §1). All database
//! management is done with PostgreSQL command-line tools and readiness probes.

use crate::cleanup_process::CleanupCommand;
use std::net::ToSocketAddrs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use lucidos_local_token::SHARED_PG_PASSWORD_FILE;
use lucidos_pg_url::{redact, Host, PgUrl, PG_MAX_IDENTIFIER_BYTES};

type BoxError = Box<dyn std::error::Error + Send + Sync>;

// The dev scripts start this same cluster, so `scripts/lib/workspace_constants.sh`
// carries a copy of each `pub(crate)` value below. `value_pins_tests` pins them.
pub(crate) const PG_USER: &str = "lucidos";
const PG_ADMIN_DATABASE: &str = "postgres";
// Server-side bounds also stop SQL inside Docker if its CLI disconnects.
const PG_BULK_OPTIONS: &str = "-c statement_timeout=0 -c lock_timeout=10000";
const PG_QUERY_OPTIONS: &str = "-c statement_timeout=60000 -c lock_timeout=10000";
pub(crate) const PG_IMAGE: &str = "pgvector/pgvector:pg18";
pub(crate) const SHARED_DOCKER_CONTAINER: &str = "lucidos-pg-shared";
pub(crate) const SHARED_DOCKER_VOLUME: &str = "lucidos-pg-data-shared";

/// The Docker cluster's `/dev/shm`. pgvector builds the HNSW index with
/// parallel maintenance workers backed by POSIX shared memory there. Docker's
/// 64m default overflows on a workspace with a sizeable memory_entries table.
/// The migration or restore then aborts with "could not resize shared memory
/// segment ... No space left on device".
pub(crate) const PG_SHM_SIZE: &str = "1g";

/// ONE shared cluster serves every workspace (ADR 0014 §6/§7), and each engine
/// opens a pool of up to 50 connections (construction.rs). Postgres' default
/// 100 is exhausted by two busy workspaces, so a third fails to provision with
/// "sorry, too many clients already". 500 fits about 10 concurrent engines.
pub(crate) const PG_MAX_CONNECTIONS: u32 = 500;

/// Which shared-cluster backend the gateway uses. Selected once from the
/// environment at gateway start (`LUCIDOS_GATEWAY_PG_BACKEND`): dev → Docker,
/// packaged → Embedded.
#[derive(Clone, Debug)]
pub enum PgBackend {
    Docker,
    Embedded { bin: PathBuf, lib: PathBuf },
}

impl PgBackend {
    /// Resolve the backend from the environment. `LUCIDOS_GATEWAY_PG_BACKEND` is
    /// `docker` (default) or `embedded`; embedded reads `LUCIDOS_PG_BIN_DIR` and
    /// `LUCIDOS_PG_LIB_DIR` for the bundled binaries.
    pub fn from_env() -> Result<Self, BoxError> {
        match std::env::var("LUCIDOS_GATEWAY_PG_BACKEND")
            .unwrap_or_else(|_| "docker".to_string())
            .to_ascii_lowercase()
            .as_str()
        {
            "docker" => Ok(PgBackend::Docker),
            "embedded" => {
                let bin = std::env::var_os("LUCIDOS_PG_BIN_DIR")
                    .map(PathBuf::from)
                    .ok_or("LUCIDOS_PG_BIN_DIR required for embedded Postgres backend")?;
                let lib = std::env::var_os("LUCIDOS_PG_LIB_DIR")
                    .map(PathBuf::from)
                    .ok_or("LUCIDOS_PG_LIB_DIR required for embedded Postgres backend")?;
                // The vars being SET is not enough — a mis-staged / quarantined
                // `postgres/` resource makes the first `<bin>/postgres --version`
                // (or `initdb`) fail with a raw `os error 2` (no path), which then
                // surfaces as the boot-splash hang. Validate the bundled tree
                // exists here so the failure is a path-bearing boot error instead.
                validate_embedded_pg_dirs(&bin, &lib)?;
                Ok(PgBackend::Embedded { bin, lib })
            }
            other => Err(format!("unknown LUCIDOS_GATEWAY_PG_BACKEND '{other}'").into()),
        }
    }
}

/// Validate the bundled relocatable-PostgreSQL tree referenced by
/// `LUCIDOS_PG_BIN_DIR` (`<postgres>/bin`) + `LUCIDOS_PG_LIB_DIR`
/// (`<postgres>/lib`): the `postgres` server binary must exist, `lib` must be a
/// dir, and the sibling `share/` (the bootstrap `postgres.bki`, timezone data,
/// and config templates `initdb` resolves via `bin/../share`) must exist —
/// otherwise first-ever `initdb` fails with PG's opaque "could not find … share".
/// Path-bearing errors so a packaging regression is diagnosable from the boot log.
fn validate_embedded_pg_dirs(bin: &Path, lib: &Path) -> Result<(), BoxError> {
    let postgres = bin.join("postgres");
    if !postgres.is_file() {
        return Err(format!(
            "bundled Postgres server binary not found at {} (LUCIDOS_PG_BIN_DIR)",
            postgres.display()
        )
        .into());
    }
    if !lib.is_dir() {
        return Err(format!(
            "bundled Postgres lib dir not found at {} (LUCIDOS_PG_LIB_DIR)",
            lib.display()
        )
        .into());
    }
    // `share/` is resolved relative to the binary (`bin/../share`); the bundle
    // stages bin/lib/share together, so derive it from the bin dir's parent.
    if let Some(share) = bin.parent().map(|p| p.join("share")) {
        if !share.is_dir() {
            return Err(format!(
                "bundled Postgres share dir not found at {} (relocatable PG needs bin/../share \
                 for initdb)",
                share.display()
            )
            .into());
        }
    }
    Ok(())
}

/// Whether a provisioning failure can plausibly clear on its own.
///
/// The gateway supervisor reads this to decide between retrying the workspace
/// and latching it as dead. Getting it wrong in the `Terminal` direction is the
/// expensive mistake: it makes a workspace permanently unopenable for the
/// lifetime of the gateway process, which is why [`ProvisionError`]'s
/// `From<BoxError>` conversion defaults every unclassified failure to
/// `Transient`. Retrying is bounded and backed off, so a wrong `Transient`
/// costs a couple of minutes and then says the same thing.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ProvisionErrorKind {
    /// An environment condition that can clear with nobody doing anything: the
    /// Docker daemon still starting (the 2026-08-03 login race), a cluster that
    /// has not finished coming up, an image pull failing on a flaky network.
    Transient,
    /// A condition no retry can change: an invalid workspace id, a `docker`
    /// command that is not installed, a mis-staged bundled Postgres tree, a
    /// data dir from another Postgres major.
    Terminal,
}

/// A provisioning failure plus the verdict on whether retrying it is worth
/// anything. `message` is user-facing: it reaches the boot splash and the
/// picker's health-dot tooltip, not just the log.
#[derive(Debug)]
pub struct ProvisionError {
    pub kind: ProvisionErrorKind,
    pub message: String,
}

impl ProvisionError {
    /// A failure worth retrying (see [`ProvisionErrorKind::Transient`]).
    pub fn transient(message: impl Into<String>) -> Self {
        Self {
            kind: ProvisionErrorKind::Transient,
            message: message.into(),
        }
    }

    /// A failure retrying cannot fix (see [`ProvisionErrorKind::Terminal`]).
    pub fn terminal(message: impl Into<String>) -> Self {
        Self {
            kind: ProvisionErrorKind::Terminal,
            message: message.into(),
        }
    }
}

impl std::fmt::Display for ProvisionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ProvisionError {}

/// Every unclassified provisioning failure is [`ProvisionErrorKind::Transient`].
/// This is what makes the classification safe to add incrementally: a `?` on any
/// helper that has not been classified yet gets the retryable verdict, and only
/// the provably-hopeless sites opt into `Terminal`.
impl From<BoxError> for ProvisionError {
    fn from(e: BoxError) -> Self {
        Self::transient(e.to_string())
    }
}

/// A handle to a provisioned workspace database, used on workspace delete.
#[derive(Clone, Debug)]
pub enum PgHandle {
    /// The gateway provisioned no database here. Three cases: a stack whose
    /// provisioning failed, one that never started, and an adopted external
    /// engine, whose database came from whoever launched it.
    External,
    Docker {
        container: String,
        database: String,
    },
    Embedded {
        bin: PathBuf,
        lib: PathBuf,
        socket_dir: PathBuf,
        database: String,
    },
}

impl PgHandle {
    /// Drop this workspace's database on workspace delete. Best-effort. The
    /// shared cluster/container is intentionally left running for peer
    /// workspaces.
    pub fn teardown(&self) {
        match self {
            PgHandle::External => {}
            PgHandle::Docker {
                container,
                database,
            } => {
                let _ = drop_database_docker(container, database);
            }
            PgHandle::Embedded {
                bin,
                lib,
                socket_dir,
                database,
            } => {
                let _ = drop_database_embedded(bin, lib, socket_dir, database);
            }
        }
    }
}

/// The outcome of [`ensure`]: the single-tenant engine URL plus the delete
/// handle for this workspace database.
pub struct Provisioned {
    pub database_url: String,
    pub handle: PgHandle,
}

/// Ensure the shared cluster is running and this workspace's database exists.
///
/// `legacy_url`, when present, is a migration source from the old
/// per-workspace-cluster topology. The old cluster is never modified or stopped;
/// the gateway dumps it into the shared database and then starts the engine on
/// the shared URL. If the shared database already exists, the legacy URL is not
/// consulted, so an explicit later decommission is safe.
///
/// Failures carry a [`ProvisionErrorKind`] so the caller can tell an environment
/// condition that will clear (Docker still starting) from one that never will
/// (an invalid workspace id). See ADR 0014, 2026-08-03 addendum.
///
/// `auth` applies to the embedded cluster only. The Docker cluster always
/// takes its password.
pub async fn ensure(
    backend: &PgBackend,
    ws_id: &str,
    app_data: &Path,
    legacy_url: Option<&str>,
    auth: SuperuserAuth,
) -> Result<Provisioned, ProvisionError> {
    // A malformed workspace id cannot become valid by waiting.
    let database = database_name(ws_id).map_err(|e| ProvisionError::terminal(e.to_string()))?;
    match backend {
        PgBackend::Docker => ensure_docker(&database, legacy_url).await,
        PgBackend::Embedded { bin, lib } => {
            ensure_embedded(bin, lib, app_data, &database, legacy_url, auth).await
        }
    }
}

/// Drop a workspace database by id, without requiring a running stack handle.
///
/// This is used by workspace delete for registered-but-stopped workspaces and
/// for unhealthy stacks that failed after database creation but before a
/// [`PgHandle`] was stored. It never creates a missing Docker cluster; if the
/// shared Docker container is absent there is nothing to drop. Embedded clusters
/// must already be running; teardown never starts or provisions them.
pub async fn teardown_workspace(
    backend: &PgBackend,
    ws_id: &str,
    app_data: &Path,
) -> Result<(), BoxError> {
    teardown_named_database(backend, &database_name(ws_id)?, app_data).await
}

/// Delete exactly an observed cleanup database, without deriving a workspace id.
pub async fn teardown_named_database(
    backend: &PgBackend,
    database: &str,
    app_data: &Path,
) -> Result<(), BoxError> {
    validate_cleanup_database(database)?;
    match backend {
        PgBackend::Docker => teardown_docker_workspace(database).await,
        PgBackend::Embedded { bin, lib } => {
            teardown_embedded_workspace(bin, lib, app_data, database).await
        }
    }
}

/// Stable workspace database name. Workspace ids are registry slugs
/// (`[a-z0-9-]`); Postgres accepts hyphens in database names when quoted, and
/// URLs accept them in the path.
pub fn database_name(ws_id: &str) -> Result<String, BoxError> {
    lucidos_gateway_constants::workspace_database_name(ws_id)
}

/// Cleanup names come from the cluster inventory, including legacy names that
/// are not registry slugs. Preserve them exactly and quote at the SQL boundary.
/// PostgreSQL truncates identifiers beyond 63 bytes, so reject those outright.
pub(crate) fn validate_cleanup_database(database: &str) -> Result<(), BoxError> {
    if !database.starts_with("lucidos_")
        || database.len() <= "lucidos_".len()
        || database.len() > 63
        || database.contains('\0')
    {
        return Err("invalid cleanup database name".into());
    }
    Ok(())
}

/// Write a custom-format recovery dump without provisioning or changing a cluster.
pub fn cleanup_dump(
    backend: &PgBackend,
    app_data: &Path,
    id: &str,
    path: &Path,
) -> Result<(), BoxError> {
    cleanup_dump_named(backend, app_data, &database_name(id)?, path)
}

pub fn cleanup_dump_named(
    backend: &PgBackend,
    app_data: &Path,
    database: &str,
    path: &Path,
) -> Result<(), BoxError> {
    validate_cleanup_database(database)?;
    let mut command = match backend {
        PgBackend::Docker => {
            let mut command = Command::new("docker");
            command.args([
                "exec",
                "-e",
                &format!("PGOPTIONS={PG_BULK_OPTIONS}"),
                &docker_container_name(),
                "pg_dump",
                "-U",
                PG_USER,
            ]);
            command
        }
        PgBackend::Embedded { bin, lib } => {
            let socket_dir = embedded_socket_dir(&embedded_data_dir(app_data))?;
            let mut command = embedded_client(bin, lib, "pg_dump", &socket_dir)?;
            command.env("PGOPTIONS", PG_BULK_OPTIONS);
            command
        }
    };
    let file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?;
    command.args(["-Fc", "-d", database]);
    let output = command.cleanup_output_to(file.try_clone()?)?;
    if !output.status.success() || file.metadata()?.len() == 0 {
        return Err(format!(
            "Recovery dump failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }
    file.sync_all()?;
    Ok(())
}

fn cleanup_query(backend: &PgBackend, app_data: &Path, sql: &str) -> Result<String, BoxError> {
    match backend {
        PgBackend::Docker => psql_query_docker(&docker_container_name(), PG_ADMIN_DATABASE, sql),
        PgBackend::Embedded { bin, lib } => {
            let socket = embedded_socket_dir(&embedded_data_dir(app_data))?;
            psql_query_embedded(bin, lib, &socket, PG_ADMIN_DATABASE, sql)
        }
    }
}

/// A successful catalog observation only. Errors never prove that DROP finished.
pub(crate) fn cleanup_database_exists(
    backend: &PgBackend,
    app_data: &Path,
    database: &str,
) -> Result<bool, BoxError> {
    cleanup_database_exists_with(database, |sql| cleanup_query(backend, app_data, sql))
}

fn cleanup_database_exists_with(
    database: &str,
    query: impl FnOnce(&str) -> Result<String, BoxError>,
) -> Result<bool, BoxError> {
    validate_cleanup_database(database)?;
    let result = query(&format!(
        "SELECT count(*) FROM pg_database WHERE datname={}",
        sql_string_literal(database)
    ))?;
    match result.trim() {
        "0" => Ok(false),
        "1" => Ok(true),
        _ => Err("invalid database existence observation".into()),
    }
}

#[cfg(test)]
mod cleanup_existence_tests {
    use super::*;

    #[test]
    fn only_successful_exact_catalog_answers_prove_absence() {
        for answer in ["0", " 0\n"] {
            assert!(!cleanup_database_exists_with("lucidos_odd'name", |sql| {
                assert_eq!(
                    sql,
                    "SELECT count(*) FROM pg_database WHERE datname='lucidos_odd''name'"
                );
                Ok(answer.into())
            })
            .unwrap());
        }
        assert!(cleanup_database_exists_with("lucidos_scratch", |_| Ok("1".into())).unwrap());
        for answer in ["", "2", "unknown"] {
            assert!(
                cleanup_database_exists_with("lucidos_scratch", |_| Ok(answer.into())).is_err()
            );
        }
        assert!(cleanup_database_exists_with("lucidos_scratch", |_| Err(
            "catalog unavailable".into()
        ))
        .is_err());
        assert!(cleanup_database_exists_with("postgres", |_| panic!(
            "invalid target must not query"
        ))
        .is_err());
    }
}

pub fn cleanup_mark_original(
    backend: &PgBackend,
    app_data: &Path,
    id: &str,
    operation: &str,
) -> Result<(), BoxError> {
    cleanup_mark_named_original(backend, app_data, &database_name(id)?, operation)
}

pub fn cleanup_mark_named_original(
    backend: &PgBackend,
    app_data: &Path,
    database: &str,
    operation: &str,
) -> Result<(), BoxError> {
    cleanup_mark_original_with(database, operation, |sql| {
        cleanup_query(backend, app_data, sql)
    })
}

fn cleanup_mark_original_with(
    database: &str,
    operation: &str,
    mut query: impl FnMut(&str) -> Result<String, BoxError>,
) -> Result<(), BoxError> {
    validate_cleanup_database(database)?;
    validate_cleanup_operation(operation)?;
    if query(&format!(
        "SELECT 1 FROM pg_database WHERE datname={}",
        sql_string_literal(database)
    ))?
    .trim()
        != "1"
    {
        return Ok(());
    }
    query(&format!(
        "COMMENT ON DATABASE {} IS {}",
        quote_ident(database),
        sql_string_literal(&format!("lucidos-trash:{operation}:deleting"))
    ))?;
    Ok(())
}

fn validate_cleanup_operation(operation: &str) -> Result<(), BoxError> {
    if operation.len() != 32 || !operation.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("invalid Trash operation".into());
    }
    Ok(())
}

/// A present original is reusable only with this journal's deletion marker.
/// Absence means the drop committed and the dump must be restored instead.
fn cleanup_original_with(
    target: &str,
    operation: Option<&str>,
    mut query: impl FnMut(&str) -> Result<String, BoxError>,
) -> Result<bool, BoxError> {
    let Some(operation) = operation else {
        return Ok(false);
    };
    validate_cleanup_operation(operation)?;
    if query(&format!(
        "SELECT 1 FROM pg_database WHERE datname={}",
        sql_string_literal(target)
    ))?
    .trim()
        != "1"
    {
        return Ok(false);
    }
    let marker = query(&format!(
        "SELECT COALESCE(shobj_description(oid, 'pg_database'), '') FROM pg_database WHERE datname={}",
        sql_string_literal(target)
    ))?;
    Ok(marker.trim() == format!("lucidos-trash:{operation}:deleting"))
}

/// Restore a Trash dump into an operation-specific staging database. A durable
/// database comment distinguishes a completed rename from an unrelated target
/// after a gateway crash. Never uses --clean or drops the destination database.
pub fn cleanup_restore(
    backend: &PgBackend,
    app_data: &Path,
    id: &str,
    operation: &str,
    dump: &Path,
    deletion_operation: Option<&str>,
) -> Result<(), BoxError> {
    cleanup_restore_named(
        backend,
        app_data,
        &database_name(id)?,
        operation,
        dump,
        deletion_operation,
    )
}

pub fn cleanup_restore_named(
    backend: &PgBackend,
    app_data: &Path,
    target: &str,
    operation: &str,
    dump: &Path,
    deletion_operation: Option<&str>,
) -> Result<(), BoxError> {
    validate_cleanup_database(target)?;
    if cleanup_original_with(target, deletion_operation, |sql| {
        cleanup_query(backend, app_data, sql)
    })? {
        return Ok(());
    }
    cleanup_restore_with(
        target,
        operation,
        |sql| cleanup_query(backend, app_data, sql),
        |database| {
            let mut command = match backend {
                PgBackend::Docker => {
                    let mut command = Command::new("docker");
                    command.args([
                        "exec",
                        "-i",
                        "-e",
                        &format!("PGOPTIONS={PG_BULK_OPTIONS}"),
                        &docker_container_name(),
                        "pg_restore",
                        "-U",
                        PG_USER,
                    ]);
                    command
                }
                PgBackend::Embedded { bin, lib } => {
                    let socket_dir = embedded_socket_dir(&embedded_data_dir(app_data))?;
                    let mut command = embedded_client(bin, lib, "pg_restore", &socket_dir)?;
                    command.env("PGOPTIONS", PG_BULK_OPTIONS);
                    command
                }
            };
            command.args([
                "--no-owner",
                "--no-privileges",
                "--exit-on-error",
                "-d",
                database,
            ]);
            command.stdin(std::fs::File::open(dump)?);
            run_cleanup_bulk_command(command, "restore Trash recovery dump")
        },
    )
}

/// Explicit recovery abandons an unmarked staging name; never mutates that database.
/// A published target or an owned staging database must use ordinary restore.
pub(crate) fn cleanup_rotate_restore(
    backend: &PgBackend,
    home: &Path,
    target: &str,
    operation: &str,
    replacement: &str,
) -> Result<(), BoxError> {
    cleanup_rotate_restore_with(target, operation, replacement, |sql| {
        cleanup_query(backend, home, sql)
    })
}

fn cleanup_rotate_restore_with(
    target: &str,
    operation: &str,
    replacement: &str,
    mut query: impl FnMut(&str) -> Result<String, BoxError>,
) -> Result<(), BoxError> {
    for identity in [operation, replacement] {
        if identity.len() != 32 || !identity.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err("invalid Trash restore operation".into());
        }
    }
    if operation == replacement {
        return Err("restore identity did not change".into());
    }
    if cleanup_database_exists_with(target, &mut query)? {
        return Err("restore target is occupied; retry ordinary restore".into());
    }
    let staging = format!("lucidos_restore_{operation}");
    if !cleanup_database_exists_with(&staging, &mut query)? {
        return Err("restore staging is absent; retry ordinary restore".into());
    }
    // JSON distinguishes an actually empty comment from a malformed query reply.
    let marker = query(&format!("SELECT to_json(COALESCE(shobj_description(oid, 'pg_database'), '')) FROM pg_database WHERE datname={}", sql_string_literal(&staging)))?;
    if !serde_json::from_str::<String>(marker.trim())?.is_empty() {
        return Err("restore staging has an ownership marker; keep its recovery identity".into());
    }
    if cleanup_database_exists_with(&format!("lucidos_restore_{replacement}"), &mut query)? {
        return Err("replacement restore staging is occupied".into());
    }
    Ok(())
}

#[cfg(test)]
mod restore_rotation_tests {
    use super::*;
    #[test]
    fn rotation_only_observes_absent_target_and_unmarked_staging() {
        for answers in [
            vec!["0", "1", "\"\"", "0"],
            vec!["1"],
            vec!["0", "0"],
            vec!["0", "1", "\"lucidos-trash:owned:ready\""],
            vec!["0", "1", "\"\"", "1"],
            vec![""],
            vec!["0", "1", ""],
        ] {
            let expected = answers == vec!["0", "1", "\"\"", "0"];
            let mut answers = answers.into_iter();
            let result = cleanup_rotate_restore_with(
                "lucidos_scratch",
                &"a".repeat(32),
                &"b".repeat(32),
                |sql| {
                    assert!(sql.starts_with("SELECT "));
                    Ok(answers.next().unwrap().into())
                },
            );
            assert_eq!(result.is_ok(), expected);
        }
        assert!(cleanup_rotate_restore_with(
            "lucidos_scratch",
            &"a".repeat(32),
            &"b".repeat(32),
            |_| Err("offline".into())
        )
        .is_err());
    }
}

fn cleanup_restore_with(
    target: &str,
    operation: &str,
    mut query: impl FnMut(&str) -> Result<String, BoxError>,
    restore: impl FnOnce(&str) -> Result<(), BoxError>,
) -> Result<(), BoxError> {
    validate_cleanup_database(target)?;
    // The journal allocates and persists this random operation identity before
    // calling us. Restrict it to an identifier-sized, unambiguous alphabet.
    if operation.len() != 32 || !operation.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("invalid Trash restore operation".into());
    }
    let staging = format!("lucidos_restore_{operation}");
    let preparing = format!("lucidos-trash:{operation}:preparing");
    let ready = format!("lucidos-trash:{operation}:ready");
    let inspect = |name: &str| {
        format!(
        "SELECT COALESCE(shobj_description(oid, 'pg_database'), '') FROM pg_database WHERE datname={}",
        sql_string_literal(name)
    )
    };
    // Return existence separately: a database with an empty comment is occupied.
    let exists = |name: &str| {
        format!(
            "SELECT 1 FROM pg_database WHERE datname={}",
            sql_string_literal(name)
        )
    };
    if query(&exists(target))?.trim() == "1" {
        if query(&inspect(target))?.trim() == ready {
            return Ok(());
        }
        return Err("restore destination database is occupied".into());
    }
    let staged = query(&exists(&staging))?.trim() == "1";
    let mut staged_ready = false;
    if staged {
        let marker = query(&inspect(&staging))?;
        if marker.trim() == ready {
            staged_ready = true;
        } else if marker.trim() == preparing {
            // A failed pg_restore may have committed a prefix. Only our marked
            // staging database can be discarded and rebuilt on retry.
            query(&format!("DROP DATABASE {}", quote_ident(&staging)))?;
        } else {
            return Err("restore staging database is occupied".into());
        }
    }
    if !staged_ready {
        query(&format!(
            "CREATE DATABASE {} OWNER {PG_USER}",
            quote_ident(&staging)
        ))?;
        query(&format!(
            "COMMENT ON DATABASE {} IS {}",
            quote_ident(&staging),
            sql_string_literal(&preparing)
        ))?;
        restore(&staging)?;
        query(&format!(
            "COMMENT ON DATABASE {} IS {}",
            quote_ident(&staging),
            sql_string_literal(&ready)
        ))?;
    }
    // ALTER DATABASE refuses a destination created since our observation. Its
    // database comment moves with it, making a lost response safe to retry.
    query(&format!(
        "ALTER DATABASE {} RENAME TO {}",
        quote_ident(&staging),
        quote_ident(target)
    ))?;
    Ok(())
}

/// Observation only: never creates a cluster, database, or workspace.
pub fn cleanup_observations(
    backend: &PgBackend,
    app_data: &Path,
) -> Result<Vec<crate::cleanup::DatabaseObservation>, BoxError> {
    cleanup_observations_with(|database: &str, sql: &str| match backend {
        PgBackend::Docker => psql_query_docker(&docker_container_name(), database, sql),
        PgBackend::Embedded { bin, lib } => {
            let socket = embedded_socket_dir(&embedded_data_dir(app_data))?;
            psql_query_embedded(bin, lib, &socket, database, sql)
        }
    })
}

fn cleanup_observations_with(
    mut query: impl FnMut(&str, &str) -> Result<String, BoxError>,
) -> Result<Vec<crate::cleanup::DatabaseObservation>, BoxError> {
    let json = query(PG_ADMIN_DATABASE, "SELECT coalesce(json_agg(json_build_object('name',datname,'bytes',pg_database_size(oid),'last_event',null,'warning',null)), '[]'::json) FROM pg_database WHERE left(datname,8) = 'lucidos_'")?;
    let mut observations: Vec<crate::cleanup::DatabaseObservation> = serde_json::from_str(&json)?;
    for observation in &mut observations {
        match query(
            &observation.name,
            "SELECT to_json(max(created)) FROM events",
        ) {
            Ok(value) => match serde_json::from_str(&value) {
                Ok(last_event) => observation.last_event = last_event,
                Err(e) => observation.warning = Some(format!("Activity unreadable: {e}")),
            },
            Err(e) => observation.warning = Some(format!("Activity unavailable: {e}")),
        }
    }
    Ok(observations)
}

use lucidos_gateway_constants::WORKSPACE_DATABASE_PREFIX as DATABASE_PREFIX;

/// The longest id a NEW workspace may take. Past it, Postgres cuts the database
/// name: two long ids sharing a prefix then share one database, and an id past
/// 64 bytes fails `is_valid_id` and cannot be deleted. Every path that mints an
/// id checks this. `is_valid_id` keeps 64, so an id registered before stays
/// addressable.
pub const MAX_NEW_WORKSPACE_ID_BYTES: usize = PG_MAX_IDENTIFIER_BYTES - DATABASE_PREFIX.len();

// ── Docker backend (dev) ────────────────────────────────────────────────────

async fn ensure_docker(
    database: &str,
    legacy_url: Option<&str>,
) -> Result<Provisioned, ProvisionError> {
    let container = docker_container_name();
    let password = shared_pg_password()?;
    let host_port = ensure_docker_cluster(&container, &password).await?;
    let url = docker_database_url(host_port, database, &password);

    if database_exists_docker(&container, database)? {
        verify_database_docker(&container, database)?;
    } else if let Some(source) = legacy_url {
        migrate_legacy_url_to_target(
            source,
            &url,
            Toolchain::Host,
            || create_database_docker(&container, database),
            || drop_database_docker(&container, database),
            || verify_database_docker(&container, database),
        )?;
    } else {
        create_database_docker(&container, database)?;
        verify_database_docker(&container, database)?;
    }

    wait_for_pg(&url, Duration::from_secs(30)).await?;
    Ok(Provisioned {
        database_url: url,
        handle: PgHandle::Docker {
            container,
            database: database.to_string(),
        },
    })
}

async fn teardown_docker_workspace(database: &str) -> Result<(), BoxError> {
    let container = docker_container_name();
    let host_port = match docker_container_state(&container)? {
        ContainerState::Running => docker_published_port(&container)?,
        ContainerState::Stopped => {
            run_docker(&["start", &container])?;
            docker_published_port(&container)?
        }
        ContainerState::Absent => return Ok(()),
    };
    let admin_url = docker_database_url(host_port, PG_ADMIN_DATABASE, &shared_pg_password()?);
    wait_for_pg(&admin_url, Duration::from_secs(30)).await?;
    drop_database_docker(&container, database)
}

fn docker_container_name() -> String {
    std::env::var("LUCIDOS_GATEWAY_PG_CONTAINER")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| SHARED_DOCKER_CONTAINER.to_string())
}

/// Whether the Docker daemon is answering right now.
#[derive(Debug, PartialEq, Eq)]
enum DaemonState {
    /// The daemon answered the probe.
    Ready,
    /// The CLI ran but could not reach the daemon: Docker Desktop still
    /// starting, the socket absent, the daemon stopped. Carries the CLI's own
    /// stderr for the log (never for classification).
    Unreachable(String),
    /// The `docker` executable is not on this process's PATH.
    CliMissing,
}

/// The Docker daemon's user-facing state when it is not up yet. A boot-splash
/// sentence, not a CLI error: the stderr goes to the gateway log.
const DOCKER_NOT_RUNNING: &str = "The Docker daemon is not running yet.";
/// No `docker` on PATH. Waiting cannot fix this one, so it latches.
const DOCKER_CLI_MISSING: &str =
    "Docker is not installed, or the docker command is not on the PATH this gateway was \
     launched with.";

/// Ask the Docker CLI whether the daemon answers.
///
/// Provisioning checks the daemon first so an unavailable daemon gets the
/// boot-splash retry verdict rather than an inventory or launch error. Container
/// inventory also refuses failed observations independently, since teardown
/// calls it without this provisioning probe.
///
/// `docker version --format {{.Server.Version}}` is the probe because its
/// **exit status** answers the question: the server half of `docker version` can
/// only be filled in by a daemon that responded. Classification therefore never
/// reads an error message, which would break the moment Docker rewords one.
fn docker_daemon_state() -> DaemonState {
    classify_daemon_probe(
        Command::new("docker")
            .args(["version", "--format", "{{.Server.Version}}"])
            .output(),
    )
}

/// The pure half of [`docker_daemon_state`]: exit status and spawn-error kind in,
/// verdict out. Split so the classification is unit-tested without a daemon.
fn classify_daemon_probe(probe: std::io::Result<std::process::Output>) -> DaemonState {
    match probe {
        Ok(out) if out.status.success() => DaemonState::Ready,
        Ok(out) => {
            DaemonState::Unreachable(String::from_utf8_lossy(&out.stderr).trim().to_string())
        }
        // The binary itself is missing, which is a different problem from a
        // daemon that has not started, and is not one that fixes itself.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => DaemonState::CliMissing,
        Err(e) => DaemonState::Unreachable(e.to_string()),
    }
}

async fn ensure_docker_cluster(container: &str, password: &str) -> Result<u16, ProvisionError> {
    // Establish that the daemon is up BEFORE interpreting any container state:
    // every `docker` call below reads a down daemon as some other failure.
    match docker_daemon_state() {
        DaemonState::Ready => {}
        DaemonState::Unreachable(detail) => {
            crate::log!("[Gateway] Docker daemon not reachable yet: {}", detail);
            return Err(ProvisionError::transient(DOCKER_NOT_RUNNING));
        }
        DaemonState::CliMissing => return Err(ProvisionError::terminal(DOCKER_CLI_MISSING)),
    }

    let host_port = match docker_container_state(container)? {
        ContainerState::Running => docker_published_port(container)?,
        ContainerState::Stopped => {
            run_docker(&["start", container])?;
            docker_published_port(container)?
        }
        ContainerState::Absent => {
            let port = std::env::var("LUCIDOS_GATEWAY_PG_PORT")
                .ok()
                .and_then(|p| p.parse::<u16>().ok())
                .unwrap_or(crate::registry::Registry::default().allocate_port()?);
            let mut run = Command::new("docker");
            run.args(docker_run_args(container, port))
                .env("POSTGRES_PASSWORD", password);
            run_command(run, "docker run shared Postgres")?;
            port
        }
    };

    let admin_url = docker_database_url(host_port, PG_ADMIN_DATABASE, password);
    wait_for_pg(&admin_url, Duration::from_secs(90)).await?;
    set_password_docker(container, password)?;
    Ok(host_port)
}

/// The dev cluster's password: `~/.lucidos/` [`SHARED_PG_PASSWORD_FILE`],
/// minted on first use. See that constant for why dev needs one.
fn shared_pg_password() -> Result<String, BoxError> {
    Ok(lucidos_local_token::ensure_named(SHARED_PG_PASSWORD_FILE)?)
}

/// `docker run` for the shared cluster. It names `POSTGRES_PASSWORD` without a
/// value, so Docker reads it from the environment. The secret stays out of
/// argv, where `ps` shows it to every local user.
fn docker_run_args(container: &str, port: u16) -> Vec<String> {
    [
        "run",
        "-d",
        "--name",
        container,
        "--restart",
        "unless-stopped",
        &format!("--shm-size={PG_SHM_SIZE}"),
        "-p",
        &format!("127.0.0.1:{port}:5432"),
        "-e",
        &format!("POSTGRES_USER={PG_USER}"),
        "-e",
        "POSTGRES_PASSWORD",
        "-e",
        &format!("POSTGRES_DB={PG_ADMIN_DATABASE}"),
        "-v",
        &format!("{SHARED_DOCKER_VOLUME}:/var/lib/postgresql"),
        PG_IMAGE,
        "postgres",
        "-c",
        &format!("max_connections={PG_MAX_CONNECTIONS}"),
    ]
    .map(str::to_string)
    .to_vec()
}

/// Set the role's password to the minted secret, on every start.
///
/// A cluster created before the secret existed still holds the old public
/// password, and the image applies `POSTGRES_PASSWORD` only on first init. The
/// statement goes in on stdin, so the secret never reaches argv.
fn set_password_docker(container: &str, password: &str) -> Result<(), BoxError> {
    use std::io::Write as _;
    use std::process::Stdio;
    let mut child = Command::new("docker")
        .args([
            "exec",
            "-i",
            container,
            "psql",
            "-U",
            PG_USER,
            "-d",
            PG_ADMIN_DATABASE,
        ])
        .args(["-X", "-q", "-v", "ON_ERROR_STOP=1"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()?;
    child
        .stdin
        .take()
        .ok_or("docker exec psql has no stdin")?
        .write_all(alter_role_password_sql(password).as_bytes())?;
    let out = child.wait_with_output()?;
    if !out.status.success() {
        return Err(format!(
            "setting the shared Postgres password failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )
        .into());
    }
    Ok(())
}

fn alter_role_password_sql(password: &str) -> String {
    format!(
        "ALTER ROLE {} PASSWORD {};\n",
        quote_ident(PG_USER),
        sql_string_literal(password)
    )
}

fn docker_database_url(port: u16, database: &str, password: &str) -> String {
    let password = urlencoding::encode(password);
    format!("postgres://{PG_USER}:{password}@127.0.0.1:{port}/{database}")
}

#[derive(Debug, PartialEq, Eq)]
enum ContainerState {
    Running,
    Stopped,
    Absent,
}

fn docker_container_state(container: &str) -> Result<ContainerState, BoxError> {
    let out = Command::new("docker")
        .args([
            "container",
            "ls",
            "--all",
            "--format",
            "{{.Names}}\t{{.State}}",
            "--filter",
            &format!("name={container}"),
        ])
        .cleanup_output()?;
    classify_container_state(container, out)
}

fn classify_container_state(
    container: &str,
    out: std::process::Output,
) -> Result<ContainerState, BoxError> {
    if !out.status.success() {
        return Err(format!(
            "could not observe Docker containers: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )
        .into());
    }
    // A successful inventory, rather than an inspect error, establishes absence.
    // Docker's name filter is a substring match, so compare exact names here.
    let text = std::str::from_utf8(&out.stdout)?;
    let mut result = ContainerState::Absent;
    let mut found = false;
    for line in text.lines() {
        let (name, state) = line
            .split_once('\t')
            .ok_or("invalid Docker container observation")?;
        if name != container {
            continue;
        }
        if found {
            return Err("duplicate Docker container observation".into());
        }
        found = true;
        result = match state {
            "running" => ContainerState::Running,
            "created" | "exited" => ContainerState::Stopped,
            _ => return Err(format!("Docker container is unavailable: {state}").into()),
        };
    }
    Ok(result)
}

/// Read the loopback host port Docker published for the container's 5432.
fn docker_published_port(container: &str) -> Result<u16, BoxError> {
    let out = Command::new("docker")
        .args(["port", container, "5432/tcp"])
        .cleanup_output()?;
    if !out.status.success() {
        return Err(format!(
            "docker port {container} failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )
        .into());
    }
    let text = String::from_utf8_lossy(&out.stdout);
    text.lines()
        .filter_map(|l| l.rsplit(':').next())
        .find_map(|p| p.trim().parse::<u16>().ok())
        .ok_or_else(|| format!("could not parse published port from: {text}").into())
}

fn run_docker(args: &[&str]) -> Result<(), BoxError> {
    let out = Command::new("docker").args(args).output()?;
    if !out.status.success() {
        return Err(format!(
            "docker {} failed: {}",
            args.first().copied().unwrap_or(""),
            String::from_utf8_lossy(&out.stderr).trim()
        )
        .into());
    }
    Ok(())
}

fn psql_query_docker(container: &str, db: &str, sql: &str) -> Result<String, BoxError> {
    let out = Command::new("docker")
        .args([
            "exec",
            "-e",
            &format!("PGOPTIONS={PG_QUERY_OPTIONS}"),
            container,
            "psql",
            "-X",
            "-v",
            "ON_ERROR_STOP=1",
            "-U",
            PG_USER,
            "-d",
            db,
            "-tAX",
            "-c",
            sql,
        ])
        .cleanup_output()?;
    if !out.status.success() {
        return Err(format!(
            "docker exec psql failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )
        .into());
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn database_exists_docker(container: &str, database: &str) -> Result<bool, BoxError> {
    let sql = format!(
        "SELECT 1 FROM pg_database WHERE datname={}",
        sql_string_literal(database)
    );
    Ok(!psql_query_docker(container, PG_ADMIN_DATABASE, &sql)?.is_empty())
}

fn create_database_docker(container: &str, database: &str) -> Result<(), BoxError> {
    let sql = format!("CREATE DATABASE {} OWNER {PG_USER}", quote_ident(database));
    psql_query_docker(container, PG_ADMIN_DATABASE, &sql).map(|_| ())
}

fn drop_database_docker(container: &str, database: &str) -> Result<(), BoxError> {
    let terminate = format!(
        "SELECT pg_terminate_backend(pid) FROM pg_stat_activity WHERE datname={} AND pid <> pg_backend_pid()",
        sql_string_literal(database)
    );
    let drop = format!("DROP DATABASE IF EXISTS {}", quote_ident(database));
    let _ = psql_query_docker(container, PG_ADMIN_DATABASE, &terminate);
    psql_query_docker(container, PG_ADMIN_DATABASE, &drop)?;
    psql_query_docker(container, PG_ADMIN_DATABASE, &drop_agent_role(database)).map(|_| ())
}

/// The database's agent role goes with it. The engine created it at boot.
fn drop_agent_role(database: &str) -> String {
    format!(
        "DROP ROLE IF EXISTS {}",
        quote_ident(&lucidos_pg_url::agent_role(database))
    )
}

fn verify_database_docker(container: &str, database: &str) -> Result<(), BoxError> {
    let got = psql_query_docker(container, database, "SELECT 1")?;
    if got.trim() == "1" {
        Ok(())
    } else {
        Err(format!("verification query for database '{database}' returned '{got}'").into())
    }
}

// ── Embedded backend (packaged) ─────────────────────────────────────────────

async fn ensure_embedded(
    bin: &Path,
    lib: &Path,
    app_data: &Path,
    database: &str,
    legacy_url: Option<&str>,
    auth: SuperuserAuth,
) -> Result<Provisioned, ProvisionError> {
    let data = embedded_data_dir(app_data);
    // The socket directory is named after the canonical data dir, so the data
    // dir must exist first. initdb takes an existing empty directory.
    std::fs::create_dir_all(&data)
        .map_err(|e| ProvisionError::transient(format!("cannot create {}: {e}", data.display())))?;
    let socket_dir = embedded_socket_dir(&data)?;
    ensure_embedded_cluster(bin, lib, &data, &socket_dir, auth)?;
    let url = embedded_database_url(&socket_dir, database, &embedded_passfile(&socket_dir)?);

    if database_exists_embedded(bin, lib, &socket_dir, database)? {
        verify_database_embedded(bin, lib, &socket_dir, database)?;
    } else if let Some(source) = legacy_url {
        migrate_legacy_url_to_target(
            source,
            &url,
            Toolchain::Bundled {
                bin: bin.to_path_buf(),
                lib: lib.to_path_buf(),
            },
            || create_database_embedded(bin, lib, &socket_dir, database),
            || drop_database_embedded(bin, lib, &socket_dir, database),
            || verify_database_embedded(bin, lib, &socket_dir, database),
        )?;
    } else {
        create_database_embedded(bin, lib, &socket_dir, database)?;
        verify_database_embedded(bin, lib, &socket_dir, database)?;
    }

    Ok(Provisioned {
        database_url: url,
        handle: PgHandle::Embedded {
            bin: bin.to_path_buf(),
            lib: lib.to_path_buf(),
            socket_dir,
            database: database.to_string(),
        },
    })
}

async fn teardown_embedded_workspace(
    bin: &Path,
    lib: &Path,
    app_data: &Path,
    database: &str,
) -> Result<(), BoxError> {
    teardown_embedded_workspace_with(bin, lib, app_data, database, embedded_socket_dir)
}

fn teardown_embedded_workspace_with(
    bin: &Path,
    lib: &Path,
    app_data: &Path,
    database: &str,
    socket: impl FnOnce(&Path) -> Result<PathBuf, ProvisionError>,
) -> Result<(), BoxError> {
    let data = embedded_data_dir(app_data);
    // Cleanup must observe the existing cluster, never provision or repair it.
    // A missing version file does not establish database absence: a repair
    // could `initdb` an empty cluster and "delete" from it while the original
    // database still exists elsewhere. An unavailable cluster leaves the recovery journal for a later retry.
    let socket_dir = socket(&data).map_err(|e| -> BoxError { Box::new(e) })?;
    drop_database_embedded(bin, lib, &socket_dir, database)
}

/// Ensure a usable embedded cluster runs at `data`, reachable on `socket_dir`
/// alone.
///
/// The cluster has no TCP listener. Its one socket sits in `socket_dir`, which
/// only this user can enter. Role [`PG_USER`] signs in with the password kept
/// beside it ([`embedded_password`]), so this user's sandboxed agents cannot.
///
/// Both auth files and the password are set before every start. So a cluster
/// an older build made with trust or peer auth converges here, data intact.
/// An adopted cluster converges in place, with a reload.
///
/// The lifecycle is **self-healing** where healing loses nothing, so a stale or
/// orphaned same-major cluster never blocks a fresh start:
///
///  * **Foreign-major data dir**: the bundled binaries cannot open a data dir
///    from another PostgreSQL major, and no cross-major path exists. So this
///    refuses it, terminally, and leaves it exactly as found (ADR 0626).
///  * **Adopt only when safe** — a reported-running cluster is adopted ONLY when
///    a real connection on `socket_dir` succeeds AND its major version matches
///    the bundled binaries. A cluster an older build started answers on TCP and
///    in `/tmp`, never here, so it is stopped and restarted like a wedged one.
///  * **Stale lock** — a leftover `postmaster.pid` from an unclean stop is
///    cleared via the same robust teardown before a fresh start.
///
/// Teardown only ever targets THIS data dir (`pg_ctl -D <data>`) or the single
/// PID recorded in its `postmaster.pid` — never a broad kill, so concurrent
/// workspace engines and other-version bundles on the machine are untouched.
fn ensure_embedded_cluster(
    bin: &Path,
    lib: &Path,
    data: &Path,
    socket_dir: &Path,
    auth: SuperuserAuth,
) -> Result<(), ProvisionError> {
    let bundled = bundled_major(bin, lib)?;

    // Only a dir with no PG_VERSION gets initdb, and the major check below
    // never refuses one.
    if !data.join("PG_VERSION").exists() {
        initdb_cluster(bin, lib, data)?;
    }

    // The `credentials` table sits in here in plaintext (ADR 0153). A
    // file-level copy of a live cluster is a torn read anyway, and the copy
    // that restores is the workspace's own `pg_dump`-backed backup. This runs
    // on every start, so a cluster older than the ADR converges on its next.
    // It sets an attribute and reads no data, so a refused dir gets it too.
    crate::file_backup::exclude(data, "Postgres data dir");

    // This check runs before any stop or start on `data`.
    if let Some(data_major) = read_data_dir_major(data).filter(|&m| m != bundled) {
        let refusal = foreign_major_refusal(data, data_major, bundled);
        crate::log!("[Gateway] {}", refusal.message);
        return Err(refusal);
    }

    // Adopt an already-running cluster ONLY if it is reachable AND version-matched.
    if pg_ctl_status(bin, lib, data) {
        if should_adopt(probe_running_major(bin, lib, socket_dir), bundled) {
            crate::log!(
                "[Gateway] adopting healthy embedded Postgres v{} on {}",
                bundled,
                socket_dir.display()
            );
            return converge_running_auth(bin, lib, data, socket_dir, auth);
        }
        crate::log!(
            "[Gateway] embedded Postgres reports running but does not answer on {} with v{} — tearing down",
            socket_dir.display(),
            bundled
        );
        stop_cluster_robustly(bin, lib, data);
    } else if data.join("postmaster.pid").exists() {
        // Not running, but a leftover lock from an unclean stop — clear it.
        stop_cluster_robustly(bin, lib, data);
    }

    // The cluster socket directory outlives a reboot, unlike `/tmp`, so the
    // lock Postgres keeps beside its socket can be stale the same way.
    remove_lock_naming_another_program(&embedded_socket_lock(socket_dir));
    set_password_single_user(bin, lib, data, &embedded_password(socket_dir)?)?;
    write_auth_files(data, auth)?;
    let log = data.join("server.log");
    let log_s = log.to_string_lossy().to_string();
    let opts = embedded_start_options(socket_dir);
    // `-w` returns once the postmaster accepts connections, so no poll follows.
    pg_ctl(bin, lib, data, &["-l", &log_s, "-o", &opts, "-w", "start"]).map_err(|e| {
        ProvisionError::transient(format!("{e}. The server log is {}", log.display()))
    })?;
    // Single-user mode reports a failed statement on stderr, not in its exit
    // status. A sign-in with the password is the proof it took.
    psql_query_embedded(bin, lib, socket_dir, PG_ADMIN_DATABASE, "SELECT 1")
        .map(|_| ())
        .map_err(|e| {
            ProvisionError::transient(format!(
                "embedded Postgres started but refused its superuser password: {e}"
            ))
        })
}

/// Set [`PG_USER`]'s password while the cluster at `data` is stopped, through
/// single-user mode. It needs no sign-in, so it works whatever the auth files
/// said before. The statement goes on stdin, never argv.
fn set_password_single_user(
    bin: &Path,
    lib: &Path,
    data: &Path,
    password: &str,
) -> Result<(), ProvisionError> {
    use std::io::Write as _;
    let failed = |e: &dyn std::fmt::Display| {
        ProvisionError::transient(format!(
            "setting the embedded Postgres password failed: {e}"
        ))
    };
    let mut cmd = Command::new(bin.join("postgres"));
    with_pg_libpath(&mut cmd, lib);
    cmd.arg("--single")
        .arg("-D")
        .arg(data)
        .arg(PG_ADMIN_DATABASE);
    cmd.stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped());
    let mut child = cmd.spawn().map_err(|e| failed(&e))?;
    let mut stdin = child.stdin.take().ok_or_else(|| failed(&"no stdin"))?;
    let statement = format!("{}\n", alter_role_password_sql(password));
    stdin
        .write_all(statement.as_bytes())
        .map_err(|e| failed(&e))?;
    std::mem::drop(stdin);
    let out = child.wait_with_output().map_err(|e| failed(&e))?;
    if !out.status.success() {
        return Err(failed(&String::from_utf8_lossy(&out.stderr).trim()));
    }
    Ok(())
}

/// Bring an adopted cluster to `auth`: set the password over a connection the
/// rules in force admit, then rewrite the auth files and reload if they differ.
fn converge_running_auth(
    bin: &Path,
    lib: &Path,
    data: &Path,
    socket_dir: &Path,
    auth: SuperuserAuth,
) -> Result<(), ProvisionError> {
    let sql = alter_role_password_sql(&embedded_password(socket_dir)?);
    psql_query_embedded(bin, lib, socket_dir, PG_ADMIN_DATABASE, &sql).map_err(|e| {
        ProvisionError::transient(format!(
            "setting the embedded Postgres password failed: {e}"
        ))
    })?;
    let hba = std::fs::read_to_string(data.join("pg_hba.conf")).unwrap_or_default();
    if hba == pg_hba_conf(auth) {
        return Ok(());
    }
    write_auth_files(data, auth)?;
    pg_ctl(bin, lib, data, &["reload"])
        .map_err(|e| ProvisionError::transient(format!("reloading embedded Postgres: {e}")))
}

/// The embedded cluster's port. Each cluster has its own socket directory, so
/// the port need not be unique. libpq's default lets a bare `psql -h <dir>` in.
const EMBEDDED_PORT: u16 = lucidos_pg_url::DEFAULT_PORT;

/// The name of the cluster socket directory under the run directory.
const EMBEDDED_SOCKET_PREFIX: &str = "pg";

/// The superuser password's file, in the cluster socket directory.
const EMBEDDED_PASSWORD_FILE: &str = "superuser-password";

/// The libpq password file for the superuser, beside the password.
const EMBEDDED_PASSFILE: &str = "superuser.pgpass";

/// The superuser password of the cluster whose socket sits in `socket_dir`,
/// minted on first use.
///
/// Kept beside the socket it opens, so every cluster has its own, and a
/// test's cluster never touches the machine's. The socket directory is in the
/// private subtree, which an agent's sandbox denies (ADR 0645).
fn embedded_password(socket_dir: &Path) -> Result<String, ProvisionError> {
    lucidos_local_token::ensure_at(&socket_dir.join(EMBEDDED_PASSWORD_FILE)).map_err(|e| {
        ProvisionError::transient(format!(
            "the embedded Postgres password in {} cannot be read: {e}",
            socket_dir.display()
        ))
    })
}

/// The libpq password file signing [`PG_USER`] in to the cluster on
/// `socket_dir`, rewritten from the password whenever it differs.
///
/// Clients get its path, never the password. On macOS any same-user process
/// reads another's environment, sandboxed or not (ADR 0645).
fn embedded_passfile(socket_dir: &Path) -> Result<PathBuf, ProvisionError> {
    let path = socket_dir.join(EMBEDDED_PASSFILE);
    let line = lucidos_pg_url::passfile_line(PG_USER, &embedded_password(socket_dir)?);
    if std::fs::read_to_string(&path).ok().as_deref() == Some(line.as_str()) {
        return Ok(path);
    }
    let write = || -> std::io::Result<()> {
        let mut staged = tempfile::NamedTempFile::new_in(socket_dir)?;
        std::io::Write::write_all(&mut staged, line.as_bytes())?;
        staged.persist(&path).map_err(|e| e.error)?;
        Ok(())
    };
    write()
        .map_err(|e| ProvisionError::transient(format!("cannot write {}: {e}", path.display())))?;
    Ok(path)
}

/// The socket file Postgres binds in the cluster socket directory.
fn embedded_socket_name() -> String {
    format!(".s.PGSQL.{EMBEDDED_PORT}")
}

/// The lock file Postgres keeps beside its socket in `socket_dir`.
fn embedded_socket_lock(socket_dir: &Path) -> PathBuf {
    socket_dir.join(format!("{}.lock", embedded_socket_name()))
}

/// The cluster socket directory for the data dir `data`, which must exist:
/// `<run directory>/pg-<digest>`, owner-only.
fn embedded_socket_dir(data: &Path) -> Result<PathBuf, ProvisionError> {
    lucidos_engine_socket::owner_only_subdir(EMBEDDED_SOCKET_PREFIX, data, &embedded_socket_name())
        .map_err(|e| {
            let message = format!("the Postgres socket directory cannot be used: {e}");
            match e.kind() {
                // A loose or foreign directory, or a path past the kernel
                // limit, stays that way until a person changes it.
                std::io::ErrorKind::PermissionDenied | std::io::ErrorKind::InvalidInput => {
                    ProvisionError::terminal(message)
                }
                _ => ProvisionError::transient(message),
            }
        })
        .and_then(require_utf8_socket_dir)
}

/// Refuse a socket directory that is not UTF-8. The start options and the URL
/// are both text, so a lossy copy would name a directory that does not exist.
fn require_utf8_socket_dir(dir: PathBuf) -> Result<PathBuf, ProvisionError> {
    if dir.to_str().is_some() {
        return Ok(dir);
    }
    Err(ProvisionError::terminal(format!(
        "the Postgres socket directory {} is not valid UTF-8. A UTF-8 HOME fixes it.",
        dir.display()
    )))
}

/// The postmaster options `pg_ctl -o` passes on. No TCP listener, and one
/// owner-only socket in `socket_dir`.
///
/// `pg_ctl` hands the string to `/bin/sh`, so each argument is shell-quoted.
/// The directory is also double-quoted inside the setting, which Postgres
/// reads as a comma-separated list.
fn embedded_start_options(socket_dir: &Path) -> String {
    let dir = socket_dir.to_string_lossy().replace('"', "\"\"");
    [
        "-p".to_string(),
        EMBEDDED_PORT.to_string(),
        "-c".to_string(),
        "listen_addresses=".to_string(),
        "-c".to_string(),
        format!("unix_socket_directories=\"{dir}\""),
        "-c".to_string(),
        "unix_socket_permissions=0700".to_string(),
    ]
    .iter()
    .map(|arg| shell_quote(arg))
    .collect::<Vec<_>>()
    .join(" ")
}

/// `arg` as one `/bin/sh` word.
fn shell_quote(arg: &str) -> String {
    format!("'{}'", arg.replace('\'', "'\\''"))
}

/// How role [`PG_USER`] signs in to the embedded cluster.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SuperuserAuth {
    /// With [`embedded_password`] alone. An agent's subprocess runs as this OS
    /// user, so peer auth would admit it (ADR 0645).
    Password,
    /// By peer, as this OS user, while an engine an older build started may
    /// still run. Its URL holds no password. The password is set anyway, and
    /// peer auth admits a client presenting it. Registry row: "Peer auth kept
    /// while adopted engines run" in `docs/temporary-measures.md`.
    PeerForAdoptedEngines,
}

/// Rewrite `pg_hba.conf` and `pg_ident.conf` for `auth`, over the socket
/// alone. An agent role always signs in with its password. Runs before every
/// start, and before a reload of an adopted cluster.
fn write_auth_files(data: &Path, auth: SuperuserAuth) -> Result<(), ProvisionError> {
    let ident = match auth {
        SuperuserAuth::Password => AUTH_FILE_HEADER.to_string(),
        SuperuserAuth::PeerForAdoptedEngines => pg_ident_conf(&os_user_name()?)?,
    };
    for (name, body) in [("pg_hba.conf", pg_hba_conf(auth)), ("pg_ident.conf", ident)] {
        let path = data.join(name);
        std::fs::write(&path, body).map_err(|e| {
            ProvisionError::transient(format!("cannot write {}: {e}", path.display()))
        })?;
    }
    Ok(())
}

const AUTH_FILE_HEADER: &str =
    "# Written by the Lucidos gateway before every start. Edits here are lost.\n";

/// An agent role (ADR 0509) signs in with the password its engine minted at
/// boot, and [`PG_USER`] as `auth` says.
fn pg_hba_conf(auth: SuperuserAuth) -> String {
    let agents = lucidos_pg_url::AGENT_ROLE_GROUP;
    let superuser = match auth {
        SuperuserAuth::Password => "scram-sha-256".to_string(),
        SuperuserAuth::PeerForAdoptedEngines => format!("peer map={PEER_MAP}"),
    };
    format!(
        "{AUTH_FILE_HEADER}local all +{agents} scram-sha-256\n\
         local all {PG_USER} {superuser}\n"
    )
}

/// The `pg_ident.conf` map that names who may connect as [`PG_USER`] by peer.
const PEER_MAP: &str = "owner";

/// The ident map: `os_user` connects as [`PG_USER`], and nobody else does.
/// Quoted, so a name with a space stays one token.
fn pg_ident_conf(os_user: &str) -> Result<String, ProvisionError> {
    if os_user.is_empty() || os_user.contains(['"', '\n', '\r']) {
        return Err(ProvisionError::terminal(format!(
            "the OS user name {os_user:?} cannot be written to pg_ident.conf"
        )));
    }
    Ok(format!(
        "{AUTH_FILE_HEADER}{PEER_MAP} \"{os_user}\" {PG_USER}\n"
    ))
}

/// The name of this process's effective user, as Postgres's peer auth reads
/// it: `getpwuid` on the uid the kernel reports for the client.
fn os_user_name() -> Result<String, ProvisionError> {
    // SAFETY: `geteuid` takes no arguments, cannot fail and touches no memory.
    let uid = unsafe { libc::geteuid() };
    let mut buf = vec![0 as libc::c_char; 16 * 1024];
    // SAFETY: an all-zero `passwd` is a valid value for getpwuid_r to fill.
    let mut entry: libc::passwd = unsafe { std::mem::zeroed() };
    let mut found: *mut libc::passwd = std::ptr::null_mut();
    // SAFETY: every pointer is valid for the call, and `buf.len()` bounds `buf`.
    let rc = unsafe { libc::getpwuid_r(uid, &mut entry, buf.as_mut_ptr(), buf.len(), &mut found) };
    if rc != 0 || found.is_null() {
        return Err(ProvisionError::terminal(format!(
            "uid {uid} has no user name in the user database (getpwuid_r returned {rc}), so \
             Postgres peer auth cannot admit it"
        )));
    }
    // SAFETY: on success `pw_name` points at a NUL-terminated string in `buf`.
    let name = unsafe { std::ffi::CStr::from_ptr(entry.pw_name) };
    Ok(name.to_string_lossy().into_owned())
}

/// `initdb` a fresh cluster at `data` (UTF-8, every sign-in rejected), on
/// first run only. [`write_auth_files`] opens it before the first start.
fn initdb_cluster(bin: &Path, lib: &Path, data: &Path) -> Result<(), BoxError> {
    std::fs::create_dir_all(data)?;
    let mut cmd = Command::new(bin.join("initdb"));
    with_pg_libpath(&mut cmd, lib);
    cmd.arg("-D").arg(data);
    cmd.args([
        "-U",
        PG_USER,
        "--auth-local=reject",
        "--auth-host=reject",
        "--encoding=UTF8",
    ]);
    if !cmd.status()?.success() {
        return Err("initdb failed".into());
    }
    Ok(())
}

/// Major version of the bundled PostgreSQL binaries (`postgres --version`).
fn bundled_major(bin: &Path, lib: &Path) -> Result<u16, BoxError> {
    let mut cmd = Command::new(bin.join("postgres"));
    with_pg_libpath(&mut cmd, lib);
    cmd.arg("--version");
    let out = cmd.output()?;
    if !out.status.success() {
        return Err(format!(
            "postgres --version failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )
        .into());
    }
    let text = String::from_utf8_lossy(&out.stdout);
    parse_pg_major(&text).ok_or_else(|| {
        format!(
            "could not parse bundled Postgres version from '{}'",
            text.trim()
        )
        .into()
    })
}

/// Major version recorded in the data dir's `PG_VERSION` file (the authoritative
/// on-disk catalog major), or `None` when the dir is not yet initialized.
fn read_data_dir_major(data: &Path) -> Option<u16> {
    parse_pg_major(&std::fs::read_to_string(data.join("PG_VERSION")).ok()?)
}

/// Probe a running cluster on `socket_dir` with a real query
/// (`SHOW server_version_num`). Returns its major version, or `None` when
/// unreachable — so a single call proves BOTH reachability and version.
fn probe_running_major(bin: &Path, lib: &Path, socket_dir: &Path) -> Option<u16> {
    let version = psql_query_embedded(
        bin,
        lib,
        socket_dir,
        PG_ADMIN_DATABASE,
        "SHOW server_version_num",
    );
    parse_pg_major(&version.ok()?)
}

/// Whether a reachable running cluster should be adopted: reachable AND its major
/// matches the bundled binaries. `None` (unreachable) never adopts.
fn should_adopt(reachable_major: Option<u16>, bundled_major: u16) -> bool {
    reachable_major == Some(bundled_major)
}

/// The terminal refusal for a data dir from another Postgres major. Its message
/// reaches the boot splash, so it says what to do and that nothing was lost.
/// Newer data came from a newer Lucidos release, which the user can reinstall.
/// Older data did not, since no shipped release bundled a major below 18.
fn foreign_major_refusal(data: &Path, data_major: u16, bundled_major: u16) -> ProvisionError {
    let way_back = if data_major > bundled_major {
        format!(
            "Install the Lucidos release that bundles Postgres {data_major} to open \
             your workspaces again."
        )
    } else {
        format!(
            "No Lucidos release bundles Postgres {data_major}. Restore your workspaces \
             from a backup, or dump this data with Postgres {data_major}'s own tools."
        )
    };
    ProvisionError::terminal(format!(
        "The Postgres data at {} is version {data_major}, but this Lucidos bundles \
         Postgres {bundled_major}. Lucidos cannot move data between Postgres major \
         versions yet, so it left the data untouched. {way_back}",
        data.display()
    ))
}

/// Parse a PostgreSQL major version from any of the shapes we read:
/// `postgres (PostgreSQL) 18.4` → 18, a `PG_VERSION` body `18` → 18, and a
/// `server_version_num` `180004` → 18 (values ≥ 10000 are the numeric form).
fn parse_pg_major(s: &str) -> Option<u16> {
    let digits: String = s
        .chars()
        .skip_while(|c| !c.is_ascii_digit())
        .take_while(|c| c.is_ascii_digit())
        .collect();
    let n: u32 = digits.parse().ok()?;
    let major = if n >= 10_000 { n / 10_000 } else { n };
    u16::try_from(major).ok()
}

/// Stop the cluster at `data` as robustly as possible, only ever targeting THIS
/// data dir or the single PID recorded in its `postmaster.pid`. Best-effort:
/// graceful (`-m fast`) → immediate (`-m immediate`) → kill the recorded PID →
/// remove the stale lock file. Every fallback is logged; never a broad kill.
pub(crate) fn stop_cluster_robustly(bin: &Path, lib: &Path, data: &Path) {
    // An unclean stop (a crash, a power loss) leaves `postmaster.pid` behind,
    // and after a reboot its pid may belong to an unrelated app. `pg_ctl` and
    // the fallback below would both signal it, so drop the lock instead.
    let postmaster_pid = data.join("postmaster.pid");
    if remove_lock_naming_another_program(&postmaster_pid) {
        return;
    }
    if pg_ctl(bin, lib, data, &["-m", "fast", "-w", "stop"]).is_ok() {
        return;
    }
    if pg_ctl(bin, lib, data, &["-m", "immediate", "-w", "stop"]).is_ok() {
        return;
    }
    // pg_ctl could not stop it (foreign binaries, or a wedged/half-dead
    // postmaster). Fall back to signalling the single PID from postmaster.pid.
    if let Some(pid) = read_postmaster_pid(data) {
        crate::log!(
            "[Gateway] pg_ctl stop failed for {} — killing recorded postmaster pid {}",
            data.display(),
            pid
        );
        kill_pid_graceful_then_force(pid);
    }
    remove_stale_lock(&postmaster_pid);
}

/// Remove `lock`, a Postgres lock file, when the pid in it provably runs some
/// other program. Returns whether it did.
///
/// An unclean stop leaves the lock behind, and after a reboot its pid may
/// belong to an unrelated app of this user. Postgres then refuses to start, and
/// `pg_ctl` would signal that app.
fn remove_lock_naming_another_program(lock: &Path) -> bool {
    let Some(exe) = lock_names_another_program(lock) else {
        return false;
    };
    crate::log!(
        "[Gateway] {} names a pid now running {}, so it is stale; removing it",
        lock.display(),
        exe.display()
    );
    remove_stale_lock(lock);
    true
}

/// Remove a stale lock file, so a fresh start is not blocked by it.
fn remove_stale_lock(lock: &Path) {
    if lock.exists() {
        if let Err(e) = std::fs::remove_file(lock) {
            crate::log!("[Gateway] could not remove stale {}: {}", lock.display(), e);
        }
    }
}

/// The program the lock file `lock` names, when that is provably not Postgres.
#[cfg(unix)]
fn lock_names_another_program(lock: &Path) -> Option<PathBuf> {
    let pid = u32::try_from(read_lock_pid(lock)?).ok()?;
    crate::stack::other_program_at_pid(crate::stack::single_process_pid(pid)?, "postgres")
}

#[cfg(not(unix))]
fn lock_names_another_program(_lock: &Path) -> Option<PathBuf> {
    None
}

/// The embedded cluster's data directory, shared by every workspace.
pub(crate) fn embedded_data_dir(app_data: &Path) -> PathBuf {
    app_data.join("pgdata")
}

/// First line of `postmaster.pid` — the postmaster PID.
pub(crate) fn read_postmaster_pid(data: &Path) -> Option<i32> {
    read_lock_pid(&data.join("postmaster.pid"))
}

/// First line of a Postgres lock file: the pid of the process holding it.
fn read_lock_pid(lock: &Path) -> Option<i32> {
    std::fs::read_to_string(lock)
        .ok()?
        .lines()
        .next()?
        .trim()
        .parse()
        .ok()
}

/// SIGTERM a single PID, wait briefly, then SIGKILL if still alive. Targets only
/// the given PID (read from this cluster's postmaster.pid); never a broad kill.
#[cfg(unix)]
fn kill_pid_graceful_then_force(pid: i32) {
    // A corrupt lock naming 0 or a negative pid would make `kill` signal a
    // whole process group, or every process this user owns.
    if pid <= 0 {
        return;
    }
    // SAFETY: kill with a valid signal; a dead/foreign pid just returns ESRCH/EPERM.
    unsafe {
        libc::kill(pid as libc::pid_t, libc::SIGTERM);
    }
    for _ in 0..20 {
        // SAFETY: signal 0 performs an existence check without delivering a signal.
        if unsafe { libc::kill(pid as libc::pid_t, 0) } != 0 {
            return; // exited
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    // SAFETY: still alive after the grace window — force-kill the single pid.
    unsafe {
        libc::kill(pid as libc::pid_t, libc::SIGKILL);
    }
}

#[cfg(not(unix))]
fn kill_pid_graceful_then_force(_pid: i32) {}

/// The engine's URL for `database` on the socket in `socket_dir`. The port is
/// spelled out, since sqlx and libpq would otherwise take an inherited
/// `PGPORT`. Everything sits in the query, since sqlx refuses an empty host.
///
/// It names the `passfile`, never the password, because the URL reaches the
/// engine through its environment. The engine reads the file at boot.
fn embedded_database_url(socket_dir: &Path, database: &str, passfile: &Path) -> String {
    let enc = |path: &Path| urlencoding::encode(&path.to_string_lossy()).into_owned();
    format!(
        "postgres:///{database}?host={}&port={EMBEDDED_PORT}&user={PG_USER}&passfile={}",
        enc(socket_dir),
        enc(passfile)
    )
}

/// The bundled client `tool`, signed in to the cluster on `socket_dir` as
/// [`PG_USER`] through its password file, never `PGPASSWORD`.
fn embedded_client(
    bin: &Path,
    lib: &Path,
    tool: &str,
    socket_dir: &Path,
) -> Result<Command, ProvisionError> {
    let mut cmd = Command::new(bin.join(tool));
    with_pg_libpath(&mut cmd, lib);
    cmd.env_remove("PGPASSWORD");
    cmd.env("PGPASSFILE", embedded_passfile(socket_dir)?);
    cmd.arg("-h").arg(socket_dir);
    cmd.args(["-p", &EMBEDDED_PORT.to_string(), "-U", PG_USER]);
    Ok(cmd)
}

fn pg_ctl(bin: &Path, lib: &Path, data: &Path, extra: &[&str]) -> Result<(), BoxError> {
    let mut cmd = Command::new(bin.join("pg_ctl"));
    with_pg_libpath(&mut cmd, lib);
    cmd.arg("-D").arg(data);
    cmd.args(extra);
    let status = cmd.status()?;
    if !status.success() {
        return Err(format!("pg_ctl {extra:?} exited with {status}").into());
    }
    Ok(())
}

fn pg_ctl_status(bin: &Path, lib: &Path, data: &Path) -> bool {
    let mut cmd = Command::new(bin.join("pg_ctl"));
    with_pg_libpath(&mut cmd, lib);
    cmd.arg("-D").arg(data).arg("status");
    matches!(cmd.output(), Ok(out) if out.status.success())
}

fn psql_query_embedded(
    bin: &Path,
    lib: &Path,
    socket_dir: &Path,
    db: &str,
    sql: &str,
) -> Result<String, BoxError> {
    let mut cmd = embedded_client(bin, lib, "psql", socket_dir)?;
    cmd.env("PGOPTIONS", PG_QUERY_OPTIONS);
    cmd.args(["-X", "-v", "ON_ERROR_STOP=1", "-d", db, "-tAX", "-c", sql]);
    let out = cmd.cleanup_output()?;
    if !out.status.success() {
        return Err(format!(
            "psql failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )
        .into());
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn database_exists_embedded(
    bin: &Path,
    lib: &Path,
    socket_dir: &Path,
    database: &str,
) -> Result<bool, BoxError> {
    let sql = format!(
        "SELECT 1 FROM pg_database WHERE datname={}",
        sql_string_literal(database)
    );
    Ok(!psql_query_embedded(bin, lib, socket_dir, PG_ADMIN_DATABASE, &sql)?.is_empty())
}

fn create_database_embedded(
    bin: &Path,
    lib: &Path,
    socket_dir: &Path,
    database: &str,
) -> Result<(), BoxError> {
    let sql = format!("CREATE DATABASE {} OWNER {PG_USER}", quote_ident(database));
    psql_query_embedded(bin, lib, socket_dir, PG_ADMIN_DATABASE, &sql).map(|_| ())
}

fn drop_database_embedded(
    bin: &Path,
    lib: &Path,
    socket_dir: &Path,
    database: &str,
) -> Result<(), BoxError> {
    let terminate = format!(
        "SELECT pg_terminate_backend(pid) FROM pg_stat_activity WHERE datname={} AND pid <> pg_backend_pid()",
        sql_string_literal(database)
    );
    let drop = format!("DROP DATABASE IF EXISTS {}", quote_ident(database));
    let _ = psql_query_embedded(bin, lib, socket_dir, PG_ADMIN_DATABASE, &terminate);
    psql_query_embedded(bin, lib, socket_dir, PG_ADMIN_DATABASE, &drop)?;
    psql_query_embedded(
        bin,
        lib,
        socket_dir,
        PG_ADMIN_DATABASE,
        &drop_agent_role(database),
    )
    .map(|_| ())
}

fn verify_database_embedded(
    bin: &Path,
    lib: &Path,
    socket_dir: &Path,
    database: &str,
) -> Result<(), BoxError> {
    let got = psql_query_embedded(bin, lib, socket_dir, database, "SELECT 1")?;
    if got.trim() == "1" {
        Ok(())
    } else {
        Err(format!("verification query for database '{database}' returned '{got}'").into())
    }
}

fn with_pg_libpath(cmd: &mut Command, lib: &Path) {
    cmd.env("DYLD_LIBRARY_PATH", lib);
    cmd.env("LD_LIBRARY_PATH", lib);
}

// ── Legacy URL migration ───────────────────────────────────────────────────

#[derive(Clone)]
enum Toolchain {
    Host,
    Bundled { bin: PathBuf, lib: PathBuf },
}

impl Toolchain {
    fn command(&self, name: &str) -> Command {
        match self {
            Toolchain::Host => Command::new(name),
            Toolchain::Bundled { bin, lib } => {
                let mut cmd = Command::new(bin.join(name));
                with_pg_libpath(&mut cmd, lib);
                cmd
            }
        }
    }
}

fn migrate_legacy_url_to_target<C, D, V>(
    source_url: &str,
    target_url: &str,
    tools: Toolchain,
    create_target: C,
    drop_target: D,
    verify_target: V,
) -> Result<(), BoxError>
where
    C: FnOnce() -> Result<(), BoxError>,
    D: Fn() -> Result<(), BoxError>,
    V: Fn() -> Result<(), BoxError>,
{
    let source = PgUrl::parse(source_url)?;
    let target = PgUrl::parse(target_url)?;
    // pg_dump writes a plaintext dump under the umask, so it goes in an
    // owner-only directory that is removed with the dump on every return.
    let dump_dir = tempfile::Builder::new()
        .prefix("lucidos-gateway-migrate-")
        .tempdir()?;
    let dump_path = dump_dir.path().join(format!("{}.dump", target.database));

    let mut dump = tools.command("pg_dump");
    apply_pg_env(&mut dump, &source);
    dump.args(["-Fc", "-f"]).arg(&dump_path);
    run_command(dump, "pg_dump legacy workspace database")?;

    let mut list = tools.command("pg_restore");
    list.arg("-l").arg(&dump_path);
    let toc = command_output(list, "pg_restore -l legacy dump")?;
    if toc.trim().is_empty() {
        return Err("legacy database dump verification failed: empty TOC".into());
    }

    create_target()?;
    let mut restore = tools.command("pg_restore");
    apply_pg_env(&mut restore, &target);
    restore.args(["--no-owner", "--no-privileges", "--exit-on-error"]);
    restore.arg("-d").arg(&target.database);
    restore.arg(&dump_path);
    if let Err(e) = run_command(restore, "pg_restore legacy workspace database") {
        let _ = drop_target();
        return Err(e);
    }
    if let Err(e) = verify_target() {
        let _ = drop_target();
        return Err(e);
    }

    Ok(())
}

fn run_command(mut cmd: Command, label: &str) -> Result<(), BoxError> {
    check_command_output(cmd.output()?, label)
}

fn run_cleanup_bulk_command(mut cmd: Command, label: &str) -> Result<(), BoxError> {
    check_command_output(cmd.cleanup_bulk_output()?, label)
}

fn check_command_output(out: std::process::Output, label: &str) -> Result<(), BoxError> {
    if !out.status.success() {
        return Err(format!(
            "{label} failed with {}: {}",
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        )
        .into());
    }
    Ok(())
}

fn command_output(mut cmd: Command, label: &str) -> Result<String, BoxError> {
    let out = cmd.output()?;
    if !out.status.success() {
        return Err(format!(
            "{label} failed with {}: {}",
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        )
        .into());
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

/// Point a PostgreSQL client tool at `url` through libpq's `PG*` env, so the
/// password stays out of argv.
fn apply_pg_env(cmd: &mut Command, url: &PgUrl) {
    for (key, value) in url.env() {
        cmd.env(key, value);
    }
}

fn quote_ident(ident: &str) -> String {
    format!("\"{}\"", ident.replace('"', "\"\""))
}

fn sql_string_literal(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

// ── Readiness ───────────────────────────────────────────────────────────────

/// Poll until the Docker cluster's `host:port` accepts a TCP connection, or the
/// deadline passes. A TCP connect proves the postmaster is listening; the engine
/// then runs its own pooled connect + migrations (which retry/fail loudly).
/// The embedded cluster needs none: `pg_ctl -w start` returns once it is ready.
async fn wait_for_pg(url: &str, timeout: Duration) -> Result<(), BoxError> {
    let parsed = PgUrl::parse(url)?;
    let Host::Tcp(host) = parsed.host else {
        return Err(format!("'{}' names a Unix socket, not a TCP server", redact(url)).into());
    };
    let port = parsed.port;
    let deadline = Instant::now() + timeout;
    loop {
        match tcp_connect(&host, port).await {
            Ok(()) => return Ok(()),
            Err(e) if Instant::now() >= deadline => {
                return Err(
                    format!("Postgres at {host}:{port} not reachable within timeout: {e}").into(),
                );
            }
            Err(_) => {}
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

/// One TCP connect attempt with a short per-attempt timeout, off the async
/// reactor's critical path via `spawn_blocking` (DNS + connect can block).
async fn tcp_connect(host: &str, port: u16) -> Result<(), String> {
    let host = host.to_string();
    tokio::task::spawn_blocking(move || {
        let addr = (host.as_str(), port)
            .to_socket_addrs()
            .map_err(|e| e.to_string())?
            .next()
            .ok_or_else(|| format!("no address for {host}:{port}"))?;
        std::net::TcpStream::connect_timeout(&addr, Duration::from_secs(3))
            .map(|_| ())
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[cfg(test)]
mod tests {
    #[test]
    fn cleanup_inventory_uses_configured_docker_cluster() {
        use std::os::unix::fs::PermissionsExt;
        if std::env::var_os("CLEANUP_CONTAINER_TEST_CHILD").is_some() {
            let rows =
                super::cleanup_observations(&super::PgBackend::Docker, std::path::Path::new("."))
                    .unwrap();
            assert!(rows.is_empty());
            return;
        }
        let temp = tempfile::tempdir().unwrap();
        let docker = temp.path().join("docker");
        let log = temp.path().join("arguments");
        std::fs::write(
            &docker,
            "#!/bin/sh\nprintf '%s\n' \"$@\" > \"$CLEANUP_CONTAINER_TEST_LOG\"\nprintf '[]'\n",
        )
        .unwrap();
        std::fs::set_permissions(&docker, std::fs::Permissions::from_mode(0o700)).unwrap();
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "postgres::tests::cleanup_inventory_uses_configured_docker_cluster",
            ])
            .env("CLEANUP_CONTAINER_TEST_CHILD", "1")
            .env("CLEANUP_CONTAINER_TEST_LOG", &log)
            .env("PATH", temp.path())
            .env("LUCIDOS_GATEWAY_PG_CONTAINER", "configured-cleanup-cluster")
            .status()
            .unwrap();
        assert!(status.success());
        let arguments = std::fs::read_to_string(log).unwrap();
        assert!(arguments
            .lines()
            .any(|arg| arg == "configured-cleanup-cluster"));
        assert!(!arguments
            .lines()
            .any(|arg| arg == super::SHARED_DOCKER_CONTAINER));
    }

    #[test]
    fn named_cleanup_accepts_exact_observed_names_without_slug_conversion() {
        for name in [
            "lucidos_sample",
            "lucidos_old_workspace",
            "lucidos_Mixed.Name",
        ] {
            assert!(super::validate_cleanup_database(name).is_ok(), "{name}");
        }
        for name in ["postgres", "template0", "lucidos_", "lucidos_bad\0name"] {
            assert!(super::validate_cleanup_database(name).is_err(), "{name:?}");
        }
        assert!(super::validate_cleanup_database(&format!("lucidos_{}", "x".repeat(56))).is_err());
    }

    #[test]
    fn named_cleanup_rejects_invalid_targets_before_querying() {
        let operation = "0123456789abcdef0123456789abcdef";
        assert!(
            super::cleanup_mark_original_with("postgres", operation, |_| {
                panic!("invalid database must never reach SQL")
            })
            .is_err()
        );
        assert!(super::cleanup_restore_with(
            "template1",
            operation,
            |_| { panic!("invalid database must never reach SQL") },
            |_| panic!("invalid database must never restore")
        )
        .is_err());
    }

    #[test]
    fn embedded_teardown_does_not_repair_or_start_foreign_major_cluster() {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir().unwrap();
        let data = root.path().join("pgdata");
        std::fs::create_dir(&data).unwrap();
        std::fs::write(data.join("PG_VERSION"), "1\n").unwrap();
        let psql = root.path().join("psql");
        // Only the SQL client exists. Any lifecycle probe, init or start fails.
        std::fs::write(&psql, "#!/bin/sh\nprintf '%s\\n' \"$@\" >> \"$0.calls\"\n").unwrap();
        std::fs::set_permissions(&psql, std::fs::Permissions::from_mode(0o700)).unwrap();
        super::teardown_embedded_workspace_with(
            root.path(),
            root.path(),
            root.path(),
            "lucidos_sample",
            |_| Ok(root.path().join("socket")),
        )
        .unwrap();
        assert_eq!(
            std::fs::read_to_string(data.join("PG_VERSION")).unwrap(),
            "1\n"
        );
        let calls = std::fs::read_to_string(root.path().join("psql.calls")).unwrap();
        assert!(calls.contains("DROP DATABASE IF EXISTS \"lucidos_sample\""));
        assert!(calls.contains("DROP ROLE IF EXISTS"));
    }

    #[test]
    fn embedded_teardown_missing_cluster_is_not_successful_database_deletion() {
        let root = tempfile::tempdir().unwrap();
        let error = super::teardown_embedded_workspace_with(
            root.path(),
            root.path(),
            root.path(),
            "lucidos_sample",
            |_| Ok(root.path().join("socket")),
        )
        .expect_err("an unavailable cluster cannot establish database absence");
        assert!(!error.to_string().is_empty());
        assert!(!root.path().join("pgdata").exists());
    }

    #[test]
    fn trash_mark_original_handles_missing_database_without_creating_one() {
        let mut statements = Vec::new();
        super::cleanup_mark_original_with(
            "lucidos_sample",
            "0123456789abcdef0123456789abcdef",
            |sql| {
                statements.push(sql.to_owned());
                Ok(String::new())
            },
        )
        .unwrap();
        assert_eq!(statements.len(), 1);
        assert!(statements[0].starts_with("SELECT 1"));
    }

    #[test]
    fn trash_mark_original_requires_observation_and_valid_operation() {
        let mut statements = Vec::new();
        super::cleanup_mark_original_with(
            "lucidos_sample",
            "0123456789abcdef0123456789abcdef",
            |sql| {
                statements.push(sql.to_owned());
                Ok("1".into())
            },
        )
        .unwrap();
        assert_eq!(statements.len(), 2);
        assert!(statements[0].starts_with("SELECT 1"));
        assert!(statements[1].starts_with("COMMENT ON DATABASE"));
        assert!(
            super::cleanup_mark_original_with("lucidos_sample", "invalid", |_| panic!(
                "invalid operation must not query"
            ))
            .is_err()
        );
        assert!(super::cleanup_mark_original_with(
            "lucidos_sample",
            "0123456789abcdef0123456789abcdef",
            |_| Err("unavailable".into())
        )
        .is_err());
    }

    #[test]
    fn trash_moved_reuses_only_operation_marked_original() {
        let token = "0123456789abcdef0123456789abcdef";
        for (marker, expected) in [
            (format!("lucidos-trash:{token}:deleting"), true),
            ("unrelated".into(), false),
            (String::new(), false),
        ] {
            let result = super::cleanup_original_with("lucidos_sample", Some(token), |sql| {
                assert!(sql.starts_with("SELECT"));
                Ok(if sql.starts_with("SELECT 1") {
                    "1".into()
                } else {
                    marker.clone()
                })
            });
            assert_eq!(result.is_ok_and(|v| v), expected);
        }
        assert!(!super::cleanup_original_with(
            "lucidos_sample",
            Some(token),
            |_| Ok(String::new())
        )
        .unwrap());
        assert!(
            !super::cleanup_original_with("lucidos_sample", None, |_| panic!(
                "no marker must not authorize reuse"
            ))
            .unwrap()
        );
    }

    #[test]
    fn trash_restore_refuses_occupied_target_without_mutation() {
        let mut sql = Vec::new();
        let result = super::cleanup_restore_with(
            "lucidos_sample",
            "0123456789abcdef0123456789abcdef",
            |query| {
                sql.push(query.to_owned());
                Ok(if query.starts_with("SELECT 1") {
                    "1"
                } else {
                    "unrelated"
                }
                .into())
            },
            |_| panic!("must not restore over an existing database"),
        );
        assert!(result.is_err());
        assert!(sql.iter().all(|q| q.starts_with("SELECT")));
    }

    #[test]
    fn trash_restore_recognizes_crash_after_publish() {
        let operation = "0123456789abcdef0123456789abcdef";
        super::cleanup_restore_with(
            "lucidos_sample",
            operation,
            |query| {
                assert!(query.starts_with("SELECT"));
                Ok(if query.starts_with("SELECT 1") {
                    "1".into()
                } else {
                    format!("lucidos-trash:{operation}:ready")
                })
            },
            |_| panic!("published restore must not run twice"),
        )
        .unwrap();
    }

    #[test]
    fn trash_restore_failure_never_publishes_or_drops_original() {
        let mut sql = Vec::new();
        let result = super::cleanup_restore_with(
            "lucidos_sample",
            "0123456789abcdef0123456789abcdef",
            |query| {
                sql.push(query.to_owned());
                Ok(String::new())
            },
            |database| {
                assert!(database.starts_with("lucidos_restore_"));
                Err("injected restore failure".into())
            },
        );
        assert!(result.is_err());
        assert!(!sql
            .iter()
            .any(|q| q.starts_with("ALTER") || q.starts_with("DROP")));
        assert!(!sql.iter().any(|q| q.contains(":ready")));
    }

    #[test]
    fn trash_restore_retry_rebuilds_only_its_marked_staging_database() {
        use std::cell::RefCell;
        let calls = RefCell::new(Vec::new());
        let operation = "0123456789abcdef0123456789abcdef";
        let mut responses = [
            String::new(),
            "1".into(),
            format!("lucidos-trash:{operation}:preparing"),
        ]
        .into_iter();
        super::cleanup_restore_with(
            "lucidos_sample",
            operation,
            |sql| {
                calls.borrow_mut().push(sql.to_owned());
                Ok(responses.next().unwrap_or_default())
            },
            |_| {
                calls.borrow_mut().push("RESTORE".into());
                Ok(())
            },
        )
        .unwrap();
        let calls = calls.into_inner();
        assert!(calls[3].starts_with("DROP DATABASE \"lucidos_restore_"));
        assert!(calls[4].starts_with("CREATE DATABASE"));
        assert!(calls[5].contains(":preparing"));
        assert_eq!(calls[6], "RESTORE");
        assert!(calls[7].contains(":ready"));
        assert!(calls[8].starts_with("ALTER DATABASE"));
        assert!(calls[8].ends_with("RENAME TO \"lucidos_sample\""));
    }

    #[test]
    fn trash_restore_ready_staging_is_published_without_reloading_dump() {
        let operation = "0123456789abcdef0123456789abcdef";
        let mut responses = [
            String::new(),
            "1".into(),
            format!("lucidos-trash:{operation}:ready"),
        ]
        .into_iter();
        let mut calls = Vec::new();
        super::cleanup_restore_with(
            "lucidos_sample",
            operation,
            |sql| {
                calls.push(sql.to_owned());
                Ok(responses.next().unwrap_or_default())
            },
            |_| panic!("already restored into staging"),
        )
        .unwrap();
        assert_eq!(calls.len(), 4);
        assert!(calls[3].starts_with("ALTER DATABASE"));
    }

    #[test]
    fn trash_restore_unmarked_staging_is_never_dropped() {
        let mut responses = ["", "1", ""].into_iter();
        let mut calls = Vec::new();
        assert!(super::cleanup_restore_with(
            "lucidos_sample",
            "0123456789abcdef0123456789abcdef",
            |sql| {
                calls.push(sql.to_owned());
                Ok(responses.next().unwrap_or_default().into())
            },
            |_| panic!("unmarked staging is not ours")
        )
        .is_err());
        assert!(calls.iter().all(|sql| sql.starts_with("SELECT")));
    }

    #[test]
    fn cleanup_observation_queries_are_read_only_and_preserve_unknown_activity() {
        let observations = super::cleanup_observations_with(|database, sql| {
            assert!(sql.starts_with("SELECT "));
            if database == "postgres" {
                Ok(
                    r#"[{"name":"lucidos_sample","bytes":4096,"last_event":null,"warning":null}]"#
                        .into(),
                )
            } else {
                Err("activity unavailable".into())
            }
        })
        .unwrap();
        assert_eq!(observations[0].bytes, 4096);
        assert!(observations[0].last_event.is_none());
        assert!(observations[0].warning.is_some());
    }
    use super::*;

    /// Build a fake relocatable-PG tree (bin/postgres, lib/, share/) under a temp
    /// dir; returns (root, bin, lib).
    fn fake_pg_tree() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let root = tempfile::tempdir().unwrap();
        let bin = root.path().join("bin");
        let lib = root.path().join("lib");
        let share = root.path().join("share");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::create_dir_all(&lib).unwrap();
        std::fs::create_dir_all(&share).unwrap();
        std::fs::write(bin.join("postgres"), b"").unwrap();
        (root, bin, lib)
    }

    #[test]
    fn validate_embedded_pg_dirs_accepts_a_complete_tree() {
        let (_root, bin, lib) = fake_pg_tree();
        assert!(validate_embedded_pg_dirs(&bin, &lib).is_ok());
    }

    #[test]
    fn validate_embedded_pg_dirs_rejects_missing_server_binary() {
        let (_root, bin, lib) = fake_pg_tree();
        std::fs::remove_file(bin.join("postgres")).unwrap();
        let err = validate_embedded_pg_dirs(&bin, &lib).expect_err("missing postgres must error");
        assert!(err.to_string().contains("postgres"), "{err}");
    }

    #[test]
    fn validate_embedded_pg_dirs_rejects_missing_lib() {
        let (_root, bin, lib) = fake_pg_tree();
        std::fs::remove_dir_all(&lib).unwrap();
        let err = validate_embedded_pg_dirs(&bin, &lib).expect_err("missing lib must error");
        assert!(err.to_string().contains("lib"), "{err}");
    }

    #[test]
    fn validate_embedded_pg_dirs_rejects_missing_share() {
        // The relocatable-PG share/ dependency (#11): initdb resolves bin/../share.
        let (root, bin, lib) = fake_pg_tree();
        std::fs::remove_dir_all(root.path().join("share")).unwrap();
        let err = validate_embedded_pg_dirs(&bin, &lib).expect_err("missing share must error");
        assert!(err.to_string().contains("share"), "{err}");
    }

    // ── Provisioning-failure classification ──────────────────────────────────
    // A workspace whose provisioning failed once is retried unless the failure
    // is provably hopeless (ADR 0014, 2026-08-03). These pin which is which, and
    // pin that the Docker verdict comes from an EXIT STATUS rather than the text
    // of an error message.

    /// Build a fake `docker version` result with the given exit code.
    fn probe_exit(code: i32, stderr: &str) -> std::io::Result<std::process::Output> {
        use std::os::unix::process::ExitStatusExt;
        Ok(std::process::Output {
            // `from_raw` takes a wait(2) status: the exit code sits in the high
            // byte, so code 1 is 0x0100.
            status: std::process::ExitStatus::from_raw(code << 8),
            stdout: Vec::new(),
            stderr: stderr.as_bytes().to_vec(),
        })
    }

    #[test]
    fn container_observation_never_turns_failure_into_absence() {
        assert!(classify_container_state("sample", probe_exit(1, "unreachable").unwrap()).is_err());
        let classify = |stdout: &str| {
            let mut output = probe_exit(0, "").unwrap();
            output.stdout = stdout.as_bytes().to_vec();
            classify_container_state("sample", output)
        };
        assert_eq!(classify("").unwrap(), ContainerState::Absent);
        assert_eq!(
            classify("peer\trunning\nsample\trunning\n").unwrap(),
            ContainerState::Running
        );
        assert_eq!(
            classify("sample\texited\n").unwrap(),
            ContainerState::Stopped
        );
        assert_eq!(
            classify("sample-other\trunning\n").unwrap(),
            ContainerState::Absent
        );
        assert!(classify("sample\tunexpected\n").is_err());
        assert!(classify("malformed").is_err());
        assert!(classify("sample\trunning\nsample\texited\n").is_err());
    }

    #[test]
    fn daemon_probe_reads_the_exit_status_not_the_message() {
        // Exit 0: the daemon filled in the server half of `docker version`.
        assert_eq!(classify_daemon_probe(probe_exit(0, "")), DaemonState::Ready);
        // Non-zero: unreachable, whatever the wording. The stderr is carried for
        // the log only, so a Docker release that rewords it changes nothing.
        assert_eq!(
            classify_daemon_probe(probe_exit(1, "failed to connect to the docker API")),
            DaemonState::Unreachable("failed to connect to the docker API".to_string())
        );
        assert_eq!(
            classify_daemon_probe(probe_exit(1, "some future wording nobody predicted")),
            DaemonState::Unreachable("some future wording nobody predicted".to_string())
        );
    }

    #[test]
    fn missing_docker_binary_is_distinct_from_a_down_daemon() {
        // Spawn-level NotFound means no `docker` on PATH, which waiting cannot
        // fix; anything else the spawn can fail with is treated as unreachable.
        assert_eq!(
            classify_daemon_probe(Err(std::io::Error::from(std::io::ErrorKind::NotFound))),
            DaemonState::CliMissing
        );
        assert_eq!(
            classify_daemon_probe(Err(std::io::Error::from(
                std::io::ErrorKind::PermissionDenied
            ))),
            DaemonState::Unreachable(
                std::io::Error::from(std::io::ErrorKind::PermissionDenied).to_string()
            )
        );
    }

    #[test]
    fn unclassified_provisioning_failures_default_to_transient() {
        // The safety property behind incremental classification: a `?` on any
        // helper that has not been classified is retryable, so no unclassified
        // failure can latch a workspace dead.
        let e: ProvisionError = (Box::from("psql failed: connection reset") as BoxError).into();
        assert_eq!(e.kind, ProvisionErrorKind::Transient);
        assert_eq!(e.to_string(), "psql failed: connection reset");
        assert_eq!(
            ProvisionError::terminal("no docker").kind,
            ProvisionErrorKind::Terminal
        );
    }

    #[tokio::test]
    async fn invalid_workspace_id_is_terminal() {
        // A malformed id cannot become valid by waiting, so it must NOT spend
        // the retry budget. Reached before any backend call, so no daemon or
        // bundled cluster is needed.
        // Matched rather than `expect_err`, which would need `Provisioned: Debug`
        // and so put a password-bearing database URL one derive away from a log.
        let Err(err) = ensure(
            &PgBackend::Docker,
            "../bad",
            Path::new("/tmp"),
            None,
            SuperuserAuth::Password,
        )
        .await
        else {
            panic!("an invalid workspace id must fail");
        };
        assert_eq!(err.kind, ProvisionErrorKind::Terminal);
        assert!(err.to_string().contains("../bad"), "{err}");
    }

    /// The real `docker` CLI, pointed at a socket that does not exist, must
    /// classify as `Unreachable` (the 2026-08-03 login race). Skips when the CLI
    /// is absent, like the gated embedded-cluster tests below. `DOCKER_HOST` is
    /// set on THIS command only, never on the process env, so a parallel test
    /// cannot see it.
    #[test]
    fn gated_real_docker_cli_with_no_daemon_is_unreachable() {
        let probe = Command::new("docker")
            .env("DOCKER_HOST", "unix:///tmp/lucidos-no-such-docker.sock")
            .args(["version", "--format", "{{.Server.Version}}"])
            .output();
        if matches!(&probe, Err(e) if e.kind() == std::io::ErrorKind::NotFound) {
            eprintln!("SKIP gated_real_docker_cli_with_no_daemon_is_unreachable: no docker CLI");
            return;
        }
        assert!(
            matches!(classify_daemon_probe(probe), DaemonState::Unreachable(_)),
            "an absent daemon socket must classify as unreachable"
        );
    }

    /// The secret reaches `docker run` through the environment. A value in
    /// argv would show in `ps` to every local user. The PGDATA mount stays a
    /// named volume at the parent PG 18 relocated it under.
    #[test]
    fn docker_run_names_the_password_and_never_its_value() {
        let args = docker_run_args(SHARED_DOCKER_CONTAINER, 5435);
        let password_args: Vec<&String> = args
            .iter()
            .filter(|a| a.starts_with("POSTGRES_PASSWORD"))
            .collect();
        assert_eq!(password_args, ["POSTGRES_PASSWORD"]);
        assert!(args.contains(&"127.0.0.1:5435:5432".to_string()));
        assert!(args.contains(&format!("{SHARED_DOCKER_VOLUME}:/var/lib/postgresql")));
    }

    #[test]
    fn the_password_statement_quotes_its_secret() {
        assert_eq!(
            alter_role_password_sql("a'b"),
            "ALTER ROLE \"lucidos\" PASSWORD 'a''b';\n"
        );
    }

    #[test]
    fn the_dev_url_carries_the_secret_percent_encoded() {
        let url = docker_database_url(5435, "lucidos_dev", "p@ss/w");
        assert_eq!(
            url,
            "postgres://lucidos:p%40ss%2Fw@127.0.0.1:5435/lucidos_dev"
        );
        assert_eq!(
            PgUrl::parse(&url).unwrap().password.as_deref(),
            Some("p@ss/w")
        );
    }

    #[test]
    fn database_name_preserves_slug_shape_under_lucidos_prefix() {
        assert_eq!(database_name("default").unwrap(), "lucidos_default");
        assert_eq!(database_name("e2e-test").unwrap(), "lucidos_e2e-test");
        assert!(database_name("../bad").is_err());
        assert!(database_name("Upper").is_err());
    }

    #[test]
    fn a_new_workspace_id_at_the_limit_keeps_its_whole_database_name() {
        let longest = "a".repeat(MAX_NEW_WORKSPACE_ID_BYTES);
        assert_eq!(
            database_name(&longest).unwrap().len(),
            PG_MAX_IDENTIFIER_BYTES
        );
    }

    #[test]
    fn sql_quoting_handles_workspace_database_names() {
        assert_eq!(quote_ident("lucidos_e2e-test"), "\"lucidos_e2e-test\"");
        assert_eq!(sql_string_literal("lucidos_e2e-test"), "'lucidos_e2e-test'");
    }

    #[test]
    fn parse_pg_major_handles_every_shape_we_read() {
        // `postgres --version` output.
        assert_eq!(parse_pg_major("postgres (PostgreSQL) 18.4"), Some(18));
        assert_eq!(parse_pg_major("postgres (PostgreSQL) 17.2\n"), Some(17));
        // `PG_VERSION` file body.
        assert_eq!(parse_pg_major("18\n"), Some(18));
        assert_eq!(parse_pg_major("17"), Some(17));
        // `SHOW server_version_num` (numeric form, ≥ 10000 → divide).
        assert_eq!(parse_pg_major("180004"), Some(18));
        assert_eq!(parse_pg_major("170002"), Some(17));
        // Garbage / empty.
        assert_eq!(parse_pg_major(""), None);
        assert_eq!(parse_pg_major("not a version"), None);
    }

    #[test]
    fn should_adopt_only_when_reachable_and_version_matched() {
        // Reachable + same major → adopt.
        assert!(should_adopt(Some(18), 18));
        // Reachable but foreign major → do not adopt.
        assert!(!should_adopt(Some(17), 18));
        // Unreachable (probe returned None) → never adopt.
        assert!(!should_adopt(None, 18));
    }

    /// A `bin` dir whose every Postgres tool appends its name and args to
    /// `calls`. Only `postgres --version` answers, as v18.
    fn logging_pg_bin(dir: &Path) -> PathBuf {
        let bin = dir.join("bin");
        std::fs::create_dir(&bin).unwrap();
        for tool in ["postgres", "initdb", "pg_ctl", "psql"] {
            let path = bin.join(tool);
            std::fs::write(
                &path,
                format!(
                    "#!/bin/sh\necho \"{tool} $*\" >> \"$(dirname \"$0\")/calls\"\n\
                     [ \"{tool} $1\" = \"postgres --version\" ] && echo 'postgres (PostgreSQL) 18.4'\n\
                     exit 0\n"
                ),
            )
            .unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        bin
    }

    /// No cross-major path exists, so a data dir from another major is refused
    /// and left exactly as found: no stop, no rename, no `initdb` (ADR 0626).
    #[test]
    fn ensure_embedded_cluster_refuses_a_foreign_major_data_dir_and_touches_nothing() {
        for (data_major, way_back) in [(17, "from a backup"), (19, "bundles Postgres 19")] {
            let root = tempfile::tempdir().unwrap();
            let bin = logging_pg_bin(root.path());
            let data = root.path().join("pgdata");
            std::fs::create_dir(&data).unwrap();
            std::fs::write(data.join("PG_VERSION"), format!("{data_major}\n")).unwrap();
            std::fs::write(data.join("postmaster.pid"), "4242\n").unwrap();

            let err = ensure_embedded_cluster(
                &bin,
                &bin,
                &data,
                &root.path().join("socket"),
                SuperuserAuth::Password,
            )
            .expect_err("a foreign-major data dir must be refused");

            assert_eq!(err.kind, ProvisionErrorKind::Terminal, "{data_major}");
            for needle in [
                format!("version {data_major}"),
                "Postgres 18".to_string(),
                data.display().to_string(),
                "untouched".to_string(),
                way_back.to_string(),
            ] {
                assert!(
                    err.message.contains(&needle),
                    "{needle:?} missing: {}",
                    err.message
                );
            }
            assert_eq!(
                std::fs::read_to_string(data.join("PG_VERSION")).unwrap(),
                format!("{data_major}\n")
            );
            assert!(data.join("postmaster.pid").exists(), "no teardown ran");
            assert!(
                !sibling_foreign_dir_exists(root.path()),
                "nothing moved aside"
            );
            assert_eq!(
                std::fs::read_to_string(bin.join("calls")).unwrap(),
                "postgres --version\n",
                "only the version probe may run"
            );
        }
    }

    #[test]
    fn read_data_dir_major_and_postmaster_pid_parse_real_files() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();

        // No files yet → None (first run / no running cluster).
        assert_eq!(read_data_dir_major(dir), None);
        assert_eq!(read_postmaster_pid(dir), None);

        std::fs::write(dir.join("PG_VERSION"), "18\n").unwrap();
        assert_eq!(read_data_dir_major(dir), Some(18));

        // postmaster.pid: line 1 = pid, line 2 = data dir, line 4 = port.
        std::fs::write(
            dir.join("postmaster.pid"),
            "4242\n/some/data\n1700000000\n5599\n127.0.0.1\n",
        )
        .unwrap();
        assert_eq!(read_postmaster_pid(dir), Some(4242));
    }

    // ── The socket-only embedded cluster, without binaries ───────────────────

    /// A directory with a space (`Application Support`) and an `@` (a home
    /// named after an email address) survives the URL both ways.
    #[test]
    fn the_embedded_url_names_the_socket_directory_and_its_passfile() {
        let dir = Path::new("/Users/me.x@example.com/.lucidos/run/pg x");
        let passfile = dir.join("pass&file=x");
        let url = embedded_database_url(dir, "lucidos_myws", &passfile);
        let parsed = PgUrl::parse(&url).unwrap();
        assert_eq!(parsed.host, Host::Socket(dir.to_path_buf()));
        assert_eq!(parsed.port, EMBEDDED_PORT);
        assert_eq!(parsed.user.as_deref(), Some(PG_USER));
        assert_eq!(parsed.password, None);
        assert_eq!(parsed.passfile(), Some(passfile));
        assert_eq!(parsed.database, "lucidos_myws");
    }

    /// The URL an engine boots with resolves to the cluster's password, and
    /// the file is owner-only.
    #[test]
    fn the_passfile_signs_in_with_the_cluster_password() {
        let dir = tempfile::tempdir().unwrap();
        let passfile = embedded_passfile(dir.path()).unwrap();
        let url = embedded_database_url(dir.path(), "lucidos_myws", &passfile);
        let (inlined, _) = lucidos_pg_url::inline_passfile(&url).unwrap();
        assert_eq!(
            PgUrl::parse(&inlined).unwrap().password,
            Some(embedded_password(dir.path()).unwrap())
        );
        let mode = std::fs::metadata(&passfile).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        assert_eq!(embedded_passfile(dir.path()).unwrap(), passfile);
    }

    /// `pg_ctl` runs `-o` through `/bin/sh`, so the real shell reads the
    /// options back here. A quote or space in `HOME` must not split a word.
    #[test]
    fn the_start_options_survive_the_shell_with_any_directory() {
        let dir = Path::new("/home/u'x \"y\"/.lucidos/run/pg-0123");
        let script = format!("printf '%s\\n' {}", embedded_start_options(dir));
        let out = Command::new("/bin/sh")
            .arg("-c")
            .arg(&script)
            .output()
            .unwrap();
        assert!(out.status.success(), "{script}");
        let words: Vec<String> = String::from_utf8(out.stdout)
            .unwrap()
            .lines()
            .map(str::to_string)
            .collect();
        assert_eq!(
            words,
            [
                "-p",
                &EMBEDDED_PORT.to_string(),
                "-c",
                "listen_addresses=",
                "-c",
                "unix_socket_directories=\"/home/u'x \"\"y\"\"/.lucidos/run/pg-0123\"",
                "-c",
                "unix_socket_permissions=0700",
            ]
        );
    }

    fn rules(text: &str) -> Vec<&str> {
        text.lines().filter(|l| !l.starts_with('#')).collect()
    }

    /// The literals pin the file format Postgres reads, so they are spelled out.
    /// No peer rule: it would admit an agent's subprocess (ADR 0645).
    #[test]
    fn the_auth_files_admit_password_holders_over_the_socket_alone() {
        assert_eq!(
            rules(&pg_hba_conf(SuperuserAuth::Password)),
            [
                "local all +lucidos_agents scram-sha-256",
                "local all lucidos scram-sha-256"
            ]
        );
    }

    /// While adopted engines run, the superuser keeps the previous release's
    /// peer rule, for this OS user alone. Agent roles still need a password.
    #[test]
    fn the_kept_peer_rule_admits_one_os_user() {
        assert_eq!(
            rules(&pg_hba_conf(SuperuserAuth::PeerForAdoptedEngines)),
            [
                "local all +lucidos_agents scram-sha-256",
                "local all lucidos peer map=owner"
            ]
        );
        assert_eq!(
            rules(&pg_ident_conf("me x").unwrap()),
            ["owner \"me x\" lucidos"]
        );
    }

    #[test]
    fn a_user_name_that_would_break_the_ident_file_is_refused() {
        for bad in ["", "a\"b", "a\nowner root lucidos"] {
            let err = pg_ident_conf(bad).expect_err(bad);
            assert_eq!(err.kind, ProvisionErrorKind::Terminal, "{bad:?}");
        }
    }

    /// Postgres names the peer by `getpwuid`, as `id -un` does.
    #[test]
    fn the_os_user_name_is_the_one_id_reports() {
        let out = Command::new("id").arg("-un").output().unwrap();
        assert!(out.status.success());
        assert_eq!(
            os_user_name().unwrap(),
            String::from_utf8(out.stdout).unwrap().trim()
        );
    }

    /// Each cluster mints its own password, beside its socket, at 0600.
    #[test]
    fn the_password_is_minted_once_beside_the_socket() {
        let dir = tempfile::tempdir().unwrap();
        let first = embedded_password(dir.path()).unwrap();
        assert_eq!(embedded_password(dir.path()).unwrap(), first);
        let file = dir.path().join(EMBEDDED_PASSWORD_FILE);
        let mode = std::fs::metadata(&file).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        let other = tempfile::tempdir().unwrap();
        assert_ne!(embedded_password(other.path()).unwrap(), first);
    }

    #[test]
    fn a_socket_directory_that_is_not_utf8_is_refused() {
        use std::os::unix::ffi::OsStrExt as _;
        let bad = PathBuf::from(std::ffi::OsStr::from_bytes(b"/home/u/\xff/run/pg-0123"));
        let err = require_utf8_socket_dir(bad).expect_err("not UTF-8");
        assert_eq!(err.kind, ProvisionErrorKind::Terminal);
        assert!(err.to_string().contains("UTF-8"), "{err}");
        let good = PathBuf::from("/home/u/.lucidos/run/pg-0123");
        assert_eq!(require_utf8_socket_dir(good.clone()).unwrap(), good);
    }

    /// A crash leaves the socket lock behind, and the directory outlives a
    /// reboot. A lock naming a live program that is not Postgres is stale.
    #[test]
    fn a_socket_lock_naming_another_program_is_removed() {
        let dir = tempfile::tempdir().unwrap();
        let lock = embedded_socket_lock(dir.path());
        // This test binary is alive and is not Postgres.
        std::fs::write(&lock, format!("{}\n/data\n", std::process::id())).unwrap();
        assert!(remove_lock_naming_another_program(&lock));
        assert!(!lock.exists());
    }

    /// A pid nobody holds is Postgres's own case: it removes such a lock
    /// itself. The gateway only removes what it can prove is stale.
    #[test]
    fn a_socket_lock_naming_no_live_program_is_left_to_postgres() {
        let dir = tempfile::tempdir().unwrap();
        let lock = embedded_socket_lock(dir.path());
        std::fs::write(&lock, "999999999\n/data\n").unwrap();
        assert!(!remove_lock_naming_another_program(&lock));
        assert!(lock.exists());
    }

    // ── Gated embedded-cluster integration tests ─────────────────────────────
    //
    // These drive `ensure_embedded_cluster` against REAL bundled PostgreSQL
    // binaries, so they need `LUCIDOS_PG_BIN_DIR` + `LUCIDOS_PG_LIB_DIR` (the
    // relocatable bundle, as set in packaged builds / `build-dmg.sh`). The
    // standard test env (`test-engine.sh`'s Docker PG) ships no such binaries, so
    // they GRACEFULLY SKIP when the env is absent — never red the suite — the same
    // resilience pattern as the `real-embedder-tests` gate. Run them with the
    // binaries present to exercise the lifecycle wiring end-to-end.

    /// Bundled PG dirs for the gated tests, or `None` to skip.
    fn gated_pg_dirs() -> Option<(PathBuf, PathBuf)> {
        let bin = std::env::var_os("LUCIDOS_PG_BIN_DIR")?;
        let lib = std::env::var_os("LUCIDOS_PG_LIB_DIR")?;
        Some((PathBuf::from(bin), PathBuf::from(lib)))
    }

    /// A throwaway cluster: a parent dir holding `pgdata`, so any sibling a
    /// repair might create lands inside it, and a cluster socket directory.
    ///
    /// The run directory stands in for the real one. It sits in `/tmp` to
    /// fit `sun_path`. Its name holds a space and a quote, which the shell and
    /// Postgres's directory list must both carry through.
    struct GatedCluster {
        parent: tempfile::TempDir,
        _run: tempfile::TempDir,
        data: PathBuf,
        socket_dir: PathBuf,
    }

    fn gated_cluster() -> GatedCluster {
        let parent = tempfile::tempdir().unwrap();
        let data = parent.path().join("pgdata");
        std::fs::create_dir_all(&data).unwrap();
        let run = tempfile::Builder::new()
            .prefix("lpg o'x ")
            .tempdir_in("/tmp")
            .unwrap();
        std::fs::set_permissions(run.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let socket_dir = lucidos_engine_socket::test_support::owner_only_subdir_in(
            run.path(),
            EMBEDDED_SOCKET_PREFIX,
            &data,
            &embedded_socket_name(),
        )
        .unwrap();
        GatedCluster {
            parent,
            _run: run,
            data,
            socket_dir,
        }
    }

    use std::os::unix::fs::PermissionsExt as _;

    /// Line `n` (from 1) of `postmaster.pid`.
    fn postmaster_pid_line(data: &Path, n: usize) -> String {
        let text = std::fs::read_to_string(data.join("postmaster.pid")).unwrap();
        text.lines().nth(n - 1).unwrap_or("").trim().to_string()
    }

    /// Whether `pid` holds any TCP socket, by `lsof`. `None` without `lsof`.
    fn process_has_tcp_socket(pid: i32) -> Option<bool> {
        let out = Command::new("lsof")
            .args(["-nP", "-a", "-p", &pid.to_string(), "-iTCP"])
            .output()
            .ok()?;
        Some(!String::from_utf8_lossy(&out.stdout).trim().is_empty())
    }

    /// I1, I2 and I3: no TCP listener, one owner-only socket in the cluster
    /// socket directory, and auth files that admit this OS user alone.
    #[test]
    fn gated_the_cluster_listens_on_its_owner_only_socket_alone() {
        let Some((bin, lib)) = gated_pg_dirs() else {
            eprintln!("SKIP gated_the_cluster_listens_on_its_owner_only_socket_alone: no LUCIDOS_PG_BIN_DIR/LIB");
            return;
        };
        let c = gated_cluster();
        ensure_embedded_cluster(&bin, &lib, &c.data, &c.socket_dir, SuperuserAuth::Password)
            .unwrap();

        let show = |setting: &str| {
            psql_query_embedded(
                &bin,
                &lib,
                &c.socket_dir,
                PG_ADMIN_DATABASE,
                &format!("SHOW {setting}"),
            )
            .unwrap()
        };
        assert_eq!(show("listen_addresses"), "");
        assert_eq!(postmaster_pid_line(&c.data, 6), "", "no TCP listen address");
        let pid = read_postmaster_pid(&c.data).unwrap();
        if let Some(has_tcp) = process_has_tcp_socket(pid) {
            assert!(!has_tcp, "the postmaster holds a TCP socket");
        }

        assert_eq!(
            PathBuf::from(postmaster_pid_line(&c.data, 5)),
            c.socket_dir,
            "the one socket directory is the cluster socket directory, not /tmp"
        );
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&c.socket_dir), 0o700);
        assert_eq!(mode(&c.socket_dir.join(embedded_socket_name())), 0o700);

        let hba = std::fs::read_to_string(c.data.join("pg_hba.conf")).unwrap();
        assert_eq!(hba, pg_hba_conf(SuperuserAuth::Password));
        let ident = std::fs::read_to_string(c.data.join("pg_ident.conf")).unwrap();
        assert_eq!(ident, AUTH_FILE_HEADER);
        assert_refuses_the_passwordless(&bin, &lib, &c.socket_dir);

        stop_cluster_robustly(&bin, &lib, &c.data);
    }

    /// The socket invariant (ADR 0645): this OS user, presenting no password,
    /// does not get a session as [`PG_USER`]. An agent's subprocess is
    /// exactly that caller.
    fn assert_refuses_the_passwordless(bin: &Path, lib: &Path, socket_dir: &Path) {
        let out = passwordless_sign_in(bin, lib, socket_dir);
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(!out.status.success(), "a passwordless session was granted");
        assert!(stderr.contains("password"), "{stderr}");
    }

    /// Whether this OS user, presenting no password, gets a session.
    fn admits_the_passwordless(bin: &Path, lib: &Path, socket_dir: &Path) -> bool {
        passwordless_sign_in(bin, lib, socket_dir).status.success()
    }

    /// `psql` as [`PG_USER`] with no password from anywhere, never prompting.
    fn passwordless_sign_in(bin: &Path, lib: &Path, socket_dir: &Path) -> std::process::Output {
        let mut psql = Command::new(bin.join("psql"));
        with_pg_libpath(&mut psql, lib);
        psql.env_remove("PGPASSWORD")
            .env("PGPASSFILE", "/dev/null")
            .stdin(std::process::Stdio::null())
            .arg("-h")
            .arg(socket_dir)
            .args(["-p", &EMBEDDED_PORT.to_string(), "-U", PG_USER])
            .args(["-w", "-d", PG_ADMIN_DATABASE, "-tAXc", "SELECT 1"]);
        psql.output().unwrap()
    }

    /// A cluster an older build left running with peer auth is adopted. It
    /// keeps peer auth while adopted engines run, then converges in place:
    /// password auth, no restart, its data intact.
    #[test]
    fn gated_a_running_peer_cluster_converges_in_place() {
        let Some((bin, lib)) = gated_pg_dirs() else {
            eprintln!(
                "SKIP gated_a_running_peer_cluster_converges_in_place: no LUCIDOS_PG_BIN_DIR/LIB"
            );
            return;
        };
        let c = gated_cluster();
        let mut initdb = Command::new(bin.join("initdb"));
        with_pg_libpath(&mut initdb, &lib);
        initdb.arg("-D").arg(&c.data);
        initdb.args(["-U", PG_USER, "--auth-local=peer", "--encoding=UTF8"]);
        assert!(
            initdb.output().unwrap().status.success(),
            "old-style initdb"
        );
        // The previous release wrote exactly the kept peer rule.
        write_auth_files(&c.data, SuperuserAuth::PeerForAdoptedEngines).unwrap();
        let log = c.data.join("server.log").to_string_lossy().to_string();
        let opts = embedded_start_options(&c.socket_dir);
        pg_ctl(
            &bin,
            &lib,
            &c.data,
            &["-l", &log, "-o", &opts, "-w", "start"],
        )
        .unwrap();
        let pid_before = read_postmaster_pid(&c.data).unwrap();
        psql_query_embedded(
            &bin,
            &lib,
            &c.socket_dir,
            PG_ADMIN_DATABASE,
            "CREATE DATABASE lucidos_gated",
        )
        .unwrap();

        // An adopted engine's passwordless URL keeps working.
        let keep = SuperuserAuth::PeerForAdoptedEngines;
        ensure_embedded_cluster(&bin, &lib, &c.data, &c.socket_dir, keep).unwrap();
        assert!(admits_the_passwordless(&bin, &lib, &c.socket_dir));

        ensure_embedded_cluster(&bin, &lib, &c.data, &c.socket_dir, SuperuserAuth::Password)
            .unwrap();

        assert_eq!(
            read_postmaster_pid(&c.data),
            Some(pid_before),
            "adopted, not restarted"
        );
        assert_refuses_the_passwordless(&bin, &lib, &c.socket_dir);
        assert!(database_exists_embedded(&bin, &lib, &c.socket_dir, "lucidos_gated").unwrap());

        stop_cluster_robustly(&bin, &lib, &c.data);
    }

    /// I4: a cluster an older build made (trust auth, loopback TCP, still
    /// running) converges on the next ensure, its data intact.
    #[test]
    fn gated_an_old_trust_cluster_converges_with_its_data() {
        let Some((bin, lib)) = gated_pg_dirs() else {
            eprintln!("SKIP gated_an_old_trust_cluster_converges_with_its_data: no LUCIDOS_PG_BIN_DIR/LIB");
            return;
        };
        let c = gated_cluster();
        let mut initdb = Command::new(bin.join("initdb"));
        with_pg_libpath(&mut initdb, &lib);
        initdb.arg("-D").arg(&c.data);
        initdb.args(["-U", PG_USER, "-A", "trust", "--encoding=UTF8"]);
        assert!(
            initdb.output().unwrap().status.success(),
            "old-style initdb"
        );

        let old_port = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let old_sockets = tempfile::Builder::new()
            .prefix("lpg-old")
            .tempdir_in("/tmp")
            .unwrap();
        let opts = format!(
            "-p {old_port} -c listen_addresses=127.0.0.1 -k {}",
            old_sockets.path().display()
        );
        let log = c.data.join("server.log").to_string_lossy().to_string();
        pg_ctl(
            &bin,
            &lib,
            &c.data,
            &["-l", &log, "-o", &opts, "-w", "start"],
        )
        .unwrap();
        let old_psql = |db: &str, sql: &str| {
            let mut cmd = Command::new(bin.join("psql"));
            with_pg_libpath(&mut cmd, &lib);
            cmd.args([
                "-h",
                "127.0.0.1",
                "-p",
                &old_port.to_string(),
                "-U",
                PG_USER,
            ]);
            cmd.args(["-d", db, "-tAX", "-c", sql]);
            assert!(cmd.output().unwrap().status.success(), "{sql}");
        };
        old_psql(PG_ADMIN_DATABASE, "CREATE DATABASE lucidos_gated");
        old_psql(
            "lucidos_gated",
            "CREATE TABLE t(x int); INSERT INTO t VALUES (7)",
        );

        ensure_embedded_cluster(&bin, &lib, &c.data, &c.socket_dir, SuperuserAuth::Password)
            .unwrap();

        let got = psql_query_embedded(
            &bin,
            &lib,
            &c.socket_dir,
            "lucidos_gated",
            "SELECT x FROM t",
        )
        .unwrap();
        assert_eq!(got, "7", "the old cluster's data survives");
        let hba = std::fs::read_to_string(c.data.join("pg_hba.conf")).unwrap();
        assert!(!hba.contains("trust"), "{hba}");
        assert_refuses_the_passwordless(&bin, &lib, &c.socket_dir);
        assert!(
            std::net::TcpStream::connect(("127.0.0.1", old_port)).is_err(),
            "the old TCP listener is gone"
        );
        assert!(
            !sibling_foreign_dir_exists(c.parent.path()),
            "the data dir stays put"
        );

        stop_cluster_robustly(&bin, &lib, &c.data);
    }

    /// Stale lock (unclean stop) must recover by restarting the SAME data dir —
    /// no wipe — with the prior data intact.
    #[test]
    fn gated_stale_lock_recovers_and_preserves_data() {
        let Some((bin, lib)) = gated_pg_dirs() else {
            eprintln!(
                "SKIP gated_stale_lock_recovers_and_preserves_data: no LUCIDOS_PG_BIN_DIR/LIB"
            );
            return;
        };
        let c = gated_cluster();
        let socket = &c.socket_dir;

        // First start: real cluster + a marker row in a workspace database.
        ensure_embedded_cluster(&bin, &lib, &c.data, socket, SuperuserAuth::Password).unwrap();
        create_database_embedded(&bin, &lib, socket, "lucidos_gated").unwrap();
        psql_query_embedded(
            &bin,
            &lib,
            socket,
            "lucidos_gated",
            "CREATE TABLE t(x int); INSERT INTO t VALUES (7)",
        )
        .unwrap();

        // Simulate an orphan / unclean stop: SIGKILL the postmaster, leaving its
        // stale postmaster.pid behind.
        let pid = read_postmaster_pid(&c.data).expect("postmaster.pid");
        // SAFETY: SIGKILL to this test cluster's own recorded postmaster pid.
        unsafe {
            libc::kill(pid as libc::pid_t, libc::SIGKILL);
        }
        std::thread::sleep(Duration::from_millis(300));

        // Recover: restart the SAME data dir (crash recovery), row survives.
        ensure_embedded_cluster(&bin, &lib, &c.data, socket, SuperuserAuth::Password).unwrap();
        let got =
            psql_query_embedded(&bin, &lib, socket, "lucidos_gated", "SELECT x FROM t").unwrap();
        assert_eq!(got.trim(), "7", "data must survive a stale-lock recovery");
        // The data dir was NOT moved aside.
        assert!(
            !sibling_foreign_dir_exists(c.parent.path()),
            "same-version recovery must not move the data dir aside"
        );

        stop_cluster_robustly(&bin, &lib, &c.data);
    }

    /// After an unclean stop and a reboot, both lock files can name a pid the
    /// OS has handed to another program of this user. The cluster still starts.
    #[test]
    fn gated_locks_naming_a_reused_pid_do_not_block_the_next_start() {
        let Some((bin, lib)) = gated_pg_dirs() else {
            eprintln!("SKIP gated_locks_naming_a_reused_pid_do_not_block_the_next_start: no LUCIDOS_PG_BIN_DIR/LIB");
            return;
        };
        let c = gated_cluster();
        ensure_embedded_cluster(&bin, &lib, &c.data, &c.socket_dir, SuperuserAuth::Password)
            .unwrap();
        let pid = read_postmaster_pid(&c.data).expect("postmaster.pid");
        // SAFETY: SIGKILL to this test cluster's own recorded postmaster pid.
        unsafe {
            libc::kill(pid as libc::pid_t, libc::SIGKILL);
        }
        std::thread::sleep(Duration::from_millis(300));

        // A `sleep` stands in for the app that took the pid. Not this test
        // binary: `pg_ctl` names its caller as the postmaster's grandparent,
        // and Postgres ignores a lock naming that pid.
        let mut app = Command::new("sleep").arg("60").spawn().unwrap();
        let reused = app.id().to_string();
        for lock in [
            c.data.join("postmaster.pid"),
            embedded_socket_lock(&c.socket_dir),
        ] {
            let text = std::fs::read_to_string(&lock).unwrap();
            let rest: Vec<&str> = text.lines().skip(1).collect();
            std::fs::write(&lock, format!("{reused}\n{}\n", rest.join("\n"))).unwrap();
        }

        ensure_embedded_cluster(&bin, &lib, &c.data, &c.socket_dir, SuperuserAuth::Password)
            .unwrap();
        let got = psql_query_embedded(&bin, &lib, &c.socket_dir, PG_ADMIN_DATABASE, "SELECT 1");
        let _ = app.kill();
        let _ = app.wait();
        assert_eq!(got.unwrap(), "1");

        stop_cluster_robustly(&bin, &lib, &c.data);
    }

    /// A real cluster forged to a foreign major is refused, stays in place with
    /// its data, and opens again once its major matches.
    #[test]
    fn gated_foreign_version_is_refused_and_left_in_place() {
        let Some((bin, lib)) = gated_pg_dirs() else {
            eprintln!("SKIP gated_foreign_version_is_refused_and_left_in_place: no LUCIDOS_PG_BIN_DIR/LIB");
            return;
        };
        let c = gated_cluster();

        ensure_embedded_cluster(&bin, &lib, &c.data, &c.socket_dir, SuperuserAuth::Password)
            .unwrap();
        psql_query_embedded(
            &bin,
            &lib,
            &c.socket_dir,
            PG_ADMIN_DATABASE,
            "CREATE DATABASE kept",
        )
        .unwrap();
        stop_cluster_robustly(&bin, &lib, &c.data);
        let bundled = bundled_major(&bin, &lib).unwrap();
        std::fs::write(c.data.join("PG_VERSION"), format!("{}\n", bundled - 1)).unwrap();

        let err =
            ensure_embedded_cluster(&bin, &lib, &c.data, &c.socket_dir, SuperuserAuth::Password)
                .unwrap_err();
        assert_eq!(err.kind, ProvisionErrorKind::Terminal);
        assert!(
            !sibling_foreign_dir_exists(c.parent.path()),
            "nothing moved aside"
        );
        assert!(!pg_ctl_status(&bin, &lib, &c.data), "nothing started");

        std::fs::write(c.data.join("PG_VERSION"), format!("{bundled}\n")).unwrap();
        ensure_embedded_cluster(&bin, &lib, &c.data, &c.socket_dir, SuperuserAuth::Password)
            .unwrap();
        let kept = psql_query_embedded(
            &bin,
            &lib,
            &c.socket_dir,
            PG_ADMIN_DATABASE,
            "SELECT count(*) FROM pg_database WHERE datname = 'kept'",
        );
        stop_cluster_robustly(&bin, &lib, &c.data);
        assert_eq!(kept.unwrap(), "1", "the original cluster's data survived");
    }

    /// Whether a `*.foreign-*` sibling of the data dir exists under `parent`.
    fn sibling_foreign_dir_exists(parent: &Path) -> bool {
        std::fs::read_dir(parent)
            .map(|rd| {
                rd.flatten()
                    .any(|e| e.file_name().to_string_lossy().contains(".foreign-"))
            })
            .unwrap_or(false)
    }
}
