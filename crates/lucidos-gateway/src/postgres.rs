//! Shared Postgres provisioning for the gateway.
//!
//! ADR 0014 §6/§7 moved the gateway from "one Postgres cluster per workspace"
//! to one shared cluster with one database per workspace:
//!
//! * **Docker** (dev): one durable `lucidos-pg-shared` container/volume.
//! * **Embedded** (packaged): one bundled cluster under `<app-data>/pgdata`.
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

use std::net::ToSocketAddrs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

type BoxError = Box<dyn std::error::Error + Send + Sync>;

const PG_USER: &str = "lucidos";
const PG_PASSWORD: &str = "lucidos";
const PG_ADMIN_DATABASE: &str = "postgres";
const PG_IMAGE: &str = "pgvector/pgvector:pg18";
const SHARED_DOCKER_CONTAINER: &str = "lucidos-pg-shared";
const SHARED_DOCKER_VOLUME: &str = "lucidos-pg-data-shared";

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

/// A handle to a provisioned workspace database, used on workspace delete.
#[derive(Clone, Debug)]
pub enum PgHandle {
    /// No database was provisioned. Used only for failed/unstarted stacks.
    External,
    Docker {
        container: String,
        database: String,
    },
    Embedded {
        bin: PathBuf,
        lib: PathBuf,
        port: u16,
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
                port,
                database,
                ..
            } => {
                let _ = drop_database_embedded(bin, lib, *port, database);
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
pub async fn ensure(
    backend: &PgBackend,
    ws_id: &str,
    app_data: &Path,
    legacy_url: Option<&str>,
) -> Result<Provisioned, BoxError> {
    let database = database_name(ws_id)?;
    match backend {
        PgBackend::Docker => ensure_docker(&database, legacy_url).await,
        PgBackend::Embedded { bin, lib } => {
            ensure_embedded(bin, lib, app_data, &database, legacy_url).await
        }
    }
}

/// Drop a workspace database by id, without requiring a running stack handle.
///
/// This is used by workspace delete for registered-but-stopped workspaces and
/// for unhealthy stacks that failed after database creation but before a
/// [`PgHandle`] was stored. It never creates a missing Docker cluster; if the
/// shared Docker container is absent there is nothing to drop. Embedded clusters
/// are started only when their data directory already exists.
pub async fn teardown_workspace(
    backend: &PgBackend,
    ws_id: &str,
    app_data: &Path,
) -> Result<(), BoxError> {
    let database = database_name(ws_id)?;
    match backend {
        PgBackend::Docker => teardown_docker_workspace(&database).await,
        PgBackend::Embedded { bin, lib } => {
            teardown_embedded_workspace(bin, lib, app_data, &database).await
        }
    }
}

/// Stable workspace database name. Workspace ids are registry slugs
/// (`[a-z0-9-]`); Postgres accepts hyphens in database names when quoted, and
/// URLs accept them in the path.
pub fn database_name(ws_id: &str) -> Result<String, BoxError> {
    if !crate::registry::is_valid_id(ws_id) {
        return Err(format!("invalid workspace id for database name: '{ws_id}'").into());
    }
    Ok(format!("lucidos_{ws_id}"))
}

// ── Docker backend (dev) ────────────────────────────────────────────────────

async fn ensure_docker(database: &str, legacy_url: Option<&str>) -> Result<Provisioned, BoxError> {
    let container = docker_container_name();
    let host_port = ensure_docker_cluster(&container).await?;
    let url = docker_database_url(host_port, database);

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
    let admin_url = docker_database_url(host_port, PG_ADMIN_DATABASE);
    wait_for_pg(&admin_url, Duration::from_secs(30)).await?;
    drop_database_docker(&container, database)
}

fn docker_container_name() -> String {
    std::env::var("LUCIDOS_GATEWAY_PG_CONTAINER")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| SHARED_DOCKER_CONTAINER.to_string())
}

async fn ensure_docker_cluster(container: &str) -> Result<u16, BoxError> {
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
            run_docker(&[
                "run",
                "-d",
                "--name",
                container,
                "--restart",
                "unless-stopped",
                // pgvector builds the HNSW index with parallel maintenance
                // workers backed by POSIX shared memory under /dev/shm. Docker's
                // 64m default overflows ("could not resize shared memory segment
                // ... No space left on device") when migrating/restoring a
                // workspace with a sizeable memory_entries table, aborting the
                // migration. Keep this in lockstep with scripts/lib/workspace.sh
                // (the dev launcher's shared-cluster docker run).
                "--shm-size=1g",
                "-p",
                &format!("127.0.0.1:{port}:5432"),
                "-e",
                &format!("POSTGRES_USER={PG_USER}"),
                "-e",
                &format!("POSTGRES_PASSWORD={PG_PASSWORD}"),
                "-e",
                &format!("POSTGRES_DB={PG_ADMIN_DATABASE}"),
                "-v",
                &format!("{SHARED_DOCKER_VOLUME}:/var/lib/postgresql"),
                PG_IMAGE,
                // ONE shared cluster serves every workspace (ADR 0014 §6/§7) and
                // each engine opens a pool of up to 50 connections
                // (construction.rs). Postgres' default 100 is exhausted by two
                // busy workspaces, so a third fails to provision with "sorry,
                // too many clients already". 500 fits ~10 concurrent engines.
                // Keep in lockstep with scripts/lib/workspace.sh.
                "postgres",
                "-c",
                "max_connections=500",
            ])?;
            port
        }
    };

    let admin_url = docker_database_url(host_port, PG_ADMIN_DATABASE);
    wait_for_pg(&admin_url, Duration::from_secs(90)).await?;
    Ok(host_port)
}

fn docker_database_url(port: u16, database: &str) -> String {
    format!("postgres://{PG_USER}:{PG_PASSWORD}@127.0.0.1:{port}/{database}")
}

enum ContainerState {
    Running,
    Stopped,
    Absent,
}

fn docker_container_state(container: &str) -> Result<ContainerState, BoxError> {
    let out = Command::new("docker")
        .args(["inspect", "-f", "{{.State.Running}}", container])
        .output()?;
    if !out.status.success() {
        return Ok(ContainerState::Absent);
    }
    let running = String::from_utf8_lossy(&out.stdout).trim() == "true";
    Ok(if running {
        ContainerState::Running
    } else {
        ContainerState::Stopped
    })
}

/// Read the loopback host port Docker published for the container's 5432.
fn docker_published_port(container: &str) -> Result<u16, BoxError> {
    let out = Command::new("docker")
        .args(["port", container, "5432/tcp"])
        .output()?;
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
            "exec", container, "psql", "-U", PG_USER, "-d", db, "-tAX", "-c", sql,
        ])
        .output()?;
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
    psql_query_docker(container, PG_ADMIN_DATABASE, &drop).map(|_| ())
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
) -> Result<Provisioned, BoxError> {
    let data = app_data.join("pgdata");
    let port = ensure_embedded_cluster(bin, lib, &data).await?;
    let url = embedded_database_url(port, database);

    if database_exists_embedded(bin, lib, port, database)? {
        verify_database_embedded(bin, lib, port, database)?;
    } else if let Some(source) = legacy_url {
        migrate_legacy_url_to_target(
            source,
            &url,
            Toolchain::Bundled {
                bin: bin.to_path_buf(),
                lib: lib.to_path_buf(),
            },
            || create_database_embedded(bin, lib, port, database),
            || drop_database_embedded(bin, lib, port, database),
            || verify_database_embedded(bin, lib, port, database),
        )?;
    } else {
        create_database_embedded(bin, lib, port, database)?;
        verify_database_embedded(bin, lib, port, database)?;
    }

    wait_for_pg(&url, Duration::from_secs(30)).await?;
    Ok(Provisioned {
        database_url: url,
        handle: PgHandle::Embedded {
            bin: bin.to_path_buf(),
            lib: lib.to_path_buf(),
            port,
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
    let data = app_data.join("pgdata");
    if !data.join("PG_VERSION").exists() {
        return Ok(());
    }
    let port = ensure_embedded_cluster(bin, lib, &data).await?;
    drop_database_embedded(bin, lib, port, database)
}

/// Ensure a usable embedded cluster exists at `data` and return its port.
///
/// The lifecycle is **self-healing**, so a stale/orphaned cluster (e.g. left by
/// a previous app version after an auto-update) can never block a fresh start:
///
///  * **Foreign-major data dir** — the bundled binaries physically cannot open a
///    data dir from a different PostgreSQL major (the catalog format differs), so
///    we tear down any orphan running on it, **move the old dir aside** (renamed,
///    never deleted — recoverable), then `initdb` a fresh cluster in its place.
///  * **Adopt only when safe** — a reported-running cluster is adopted ONLY when
///    a real connection succeeds AND its major version matches the bundled
///    binaries (not a blind `pg_ctl status` adopt). A wedged/unreachable/foreign
///    cluster is torn down and restarted instead.
///  * **Stale lock** — a leftover `postmaster.pid` from an unclean stop is
///    cleared via the same robust teardown before a fresh start.
///
/// Teardown only ever targets THIS data dir (`pg_ctl -D <data>`) or the single
/// PID recorded in its `postmaster.pid` — never a broad kill, so concurrent
/// workspace engines and other-version bundles on the machine are untouched.
async fn ensure_embedded_cluster(bin: &Path, lib: &Path, data: &Path) -> Result<u16, BoxError> {
    let bundled = bundled_major(bin, lib)?;

    // Foreign-major data dir: cannot be opened by the bundled binaries. Stop any
    // orphan on it, move it aside (recoverable), then fall through to a fresh
    // initdb. The common (same-major) case is left intact for a lossless restart.
    if should_recreate_foreign(read_data_dir_major(data), bundled) {
        let data_major = read_data_dir_major(data).unwrap_or(0); // present by definition here
        crate::log!(
            "[Gateway] embedded Postgres data dir {} is v{} but bundled binaries are v{} — recreating (old dir moved aside)",
            data.display(),
            data_major,
            bundled
        );
        stop_cluster_robustly(bin, lib, data);
        match move_data_dir_aside(data, data_major) {
            Ok(moved) => crate::log!(
                "[Gateway] moved foreign-version data dir to {}",
                moved.display()
            ),
            Err(e) => {
                return Err(format!("could not move foreign-version data dir aside: {e}").into())
            }
        }
    }

    // First run (or post-move-aside): initialize a fresh cluster.
    if !data.join("PG_VERSION").exists() {
        initdb_cluster(bin, lib, data)?;
    }

    // Adopt an already-running cluster ONLY if it is reachable AND version-matched.
    if pg_ctl_status(bin, lib, data) {
        match read_postmaster_port(data) {
            Some(port) if should_adopt(probe_running_major(bin, lib, port), bundled) => {
                crate::log!(
                    "[Gateway] adopting healthy embedded Postgres v{} on :{}",
                    bundled,
                    port
                );
                return Ok(port);
            }
            Some(port) => crate::log!(
                "[Gateway] embedded Postgres reports running on :{} but is unreachable or version-mismatched — tearing down",
                port
            ),
            None => crate::log!(
                "[Gateway] embedded Postgres reports running but postmaster.pid has no port — tearing down"
            ),
        }
        stop_cluster_robustly(bin, lib, data);
    } else if data.join("postmaster.pid").exists() {
        // Not running, but a leftover lock from an unclean stop — clear it.
        stop_cluster_robustly(bin, lib, data);
    }

    // Start a fresh postmaster on a freshly allocated port. A new port sidesteps
    // any lingering holder from a partially-failed teardown.
    let port = crate::registry::Registry::default().allocate_port()?;
    let log = data.join("server.log");
    let log_s = log.to_string_lossy().to_string();
    let opts = format!("-p {port} -c listen_addresses=127.0.0.1");
    pg_ctl(bin, lib, data, &["-l", &log_s, "-o", &opts, "-w", "start"])?;

    let admin_url = embedded_database_url(port, PG_ADMIN_DATABASE);
    wait_for_pg(&admin_url, Duration::from_secs(30)).await?;
    Ok(port)
}

/// `initdb` a fresh cluster at `data` (loopback trust auth, UTF-8). Used on first
/// run and after a foreign-version data dir is moved aside.
fn initdb_cluster(bin: &Path, lib: &Path, data: &Path) -> Result<(), BoxError> {
    std::fs::create_dir_all(data)?;
    let mut cmd = Command::new(bin.join("initdb"));
    with_pg_libpath(&mut cmd, lib);
    cmd.arg("-D").arg(data);
    cmd.args(["-U", PG_USER, "-A", "trust", "--encoding=UTF8"]);
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

/// Probe a running cluster on `port` with a real query (`SHOW server_version_num`).
/// Returns its major version, or `None` when unreachable — so a single call
/// proves BOTH reachability and version.
fn probe_running_major(bin: &Path, lib: &Path, port: u16) -> Option<u16> {
    parse_pg_major(
        &psql_query_embedded(bin, lib, port, PG_ADMIN_DATABASE, "SHOW server_version_num").ok()?,
    )
}

/// Whether a reachable running cluster should be adopted: reachable AND its major
/// matches the bundled binaries. `None` (unreachable) never adopts.
fn should_adopt(reachable_major: Option<u16>, bundled_major: u16) -> bool {
    reachable_major == Some(bundled_major)
}

/// Whether an on-disk data dir must be recreated: it exists AND its major differs
/// from the bundled binaries. An uninitialized dir (`None`) is NOT foreign.
fn should_recreate_foreign(data_major: Option<u16>, bundled_major: u16) -> bool {
    data_major.is_some_and(|m| m != bundled_major)
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

/// Move a foreign-version data dir aside (rename, never delete) so a fresh
/// cluster can `initdb` in its place while the old data stays recoverable.
fn move_data_dir_aside(data: &Path, data_major: u16) -> Result<PathBuf, BoxError> {
    let ts = chrono::Utc::now().format("%Y%m%d-%H%M%S").to_string();
    let base = data
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("pgdata");
    let aside = data.with_file_name(foreign_aside_dir_name(base, data_major, &ts));
    std::fs::rename(data, &aside)?;
    Ok(aside)
}

/// Sibling name for a moved-aside foreign-version data dir.
fn foreign_aside_dir_name(base: &str, major: u16, ts: &str) -> String {
    format!("{base}.foreign-{major}-{ts}")
}

/// Stop the cluster at `data` as robustly as possible, only ever targeting THIS
/// data dir or the single PID recorded in its `postmaster.pid`. Best-effort:
/// graceful (`-m fast`) → immediate (`-m immediate`) → kill the recorded PID →
/// remove the stale lock file. Every fallback is logged; never a broad kill.
fn stop_cluster_robustly(bin: &Path, lib: &Path, data: &Path) {
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
    // Remove the stale lock so a fresh start isn't blocked by it.
    let lock = data.join("postmaster.pid");
    if lock.exists() {
        if let Err(e) = std::fs::remove_file(&lock) {
            crate::log!("[Gateway] could not remove stale {}: {}", lock.display(), e);
        }
    }
}

/// First line of `postmaster.pid` — the postmaster PID.
fn read_postmaster_pid(data: &Path) -> Option<i32> {
    std::fs::read_to_string(data.join("postmaster.pid"))
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

fn embedded_database_url(port: u16, database: &str) -> String {
    // Trust auth on loopback (matches desktop.rs) — no password in the URL.
    format!("postgres://{PG_USER}@127.0.0.1:{port}/{database}")
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

fn read_postmaster_port(data: &Path) -> Option<u16> {
    let text = std::fs::read_to_string(data.join("postmaster.pid")).ok()?;
    text.lines().nth(3)?.trim().parse().ok()
}

fn psql_query_embedded(
    bin: &Path,
    lib: &Path,
    port: u16,
    db: &str,
    sql: &str,
) -> Result<String, BoxError> {
    let mut cmd = Command::new(bin.join("psql"));
    with_pg_libpath(&mut cmd, lib);
    cmd.args([
        "-h",
        "127.0.0.1",
        "-p",
        &port.to_string(),
        "-U",
        PG_USER,
        "-d",
        db,
        "-tAX",
        "-c",
        sql,
    ]);
    let out = cmd.output()?;
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
    port: u16,
    database: &str,
) -> Result<bool, BoxError> {
    let sql = format!(
        "SELECT 1 FROM pg_database WHERE datname={}",
        sql_string_literal(database)
    );
    Ok(!psql_query_embedded(bin, lib, port, PG_ADMIN_DATABASE, &sql)?.is_empty())
}

fn create_database_embedded(
    bin: &Path,
    lib: &Path,
    port: u16,
    database: &str,
) -> Result<(), BoxError> {
    let sql = format!("CREATE DATABASE {} OWNER {PG_USER}", quote_ident(database));
    psql_query_embedded(bin, lib, port, PG_ADMIN_DATABASE, &sql).map(|_| ())
}

fn drop_database_embedded(
    bin: &Path,
    lib: &Path,
    port: u16,
    database: &str,
) -> Result<(), BoxError> {
    let terminate = format!(
        "SELECT pg_terminate_backend(pid) FROM pg_stat_activity WHERE datname={} AND pid <> pg_backend_pid()",
        sql_string_literal(database)
    );
    let drop = format!("DROP DATABASE IF EXISTS {}", quote_ident(database));
    let _ = psql_query_embedded(bin, lib, port, PG_ADMIN_DATABASE, &terminate);
    psql_query_embedded(bin, lib, port, PG_ADMIN_DATABASE, &drop).map(|_| ())
}

fn verify_database_embedded(
    bin: &Path,
    lib: &Path,
    port: u16,
    database: &str,
) -> Result<(), BoxError> {
    let got = psql_query_embedded(bin, lib, port, database, "SELECT 1")?;
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
    let source = PgUrlParts::parse(source_url)?;
    let target = PgUrlParts::parse(target_url)?;
    let dump_path = std::env::temp_dir().join(format!(
        "lucidos-gateway-migrate-{}-{}.dump",
        std::process::id(),
        target.database
    ));
    let dump_guard = DumpGuard(dump_path.clone());

    let mut dump = tools.command("pg_dump");
    source.apply_env(&mut dump);
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
    target.apply_env(&mut restore);
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

    drop(dump_guard);
    Ok(())
}

struct DumpGuard(PathBuf);

impl Drop for DumpGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn run_command(mut cmd: Command, label: &str) -> Result<(), BoxError> {
    let out = cmd.output()?;
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

#[derive(Debug, PartialEq, Eq)]
struct PgUrlParts {
    user: Option<String>,
    password: Option<String>,
    host: String,
    port: u16,
    database: String,
}

impl PgUrlParts {
    fn parse(url: &str) -> Result<Self, BoxError> {
        let rest = url.split("://").nth(1).unwrap_or(url);
        let without_query = rest.split('?').next().unwrap_or(rest);
        let (authority, path) = without_query
            .split_once('/')
            .ok_or_else(|| format!("postgres URL has no database path: '{url}'"))?;
        let (user, password, host_port) = match authority.rsplit_once('@') {
            Some((auth, host_port)) => {
                let (user, password) = match auth.split_once(':') {
                    Some((u, p)) => (Some(u.to_string()), Some(p.to_string())),
                    None => (Some(auth.to_string()), None),
                };
                (user, password, host_port)
            }
            None => (None, None, authority),
        };
        let (host, port) = match host_port.rsplit_once(':') {
            Some((host, port)) => (host.to_string(), port.parse::<u16>()?),
            None => (host_port.to_string(), 5432),
        };
        if host.is_empty() || path.is_empty() {
            return Err(format!("invalid postgres URL: '{url}'").into());
        }
        Ok(Self {
            user,
            password,
            host,
            port,
            database: path.to_string(),
        })
    }

    fn apply_env(&self, cmd: &mut Command) {
        cmd.env("PGHOST", &self.host)
            .env("PGPORT", self.port.to_string())
            .env("PGDATABASE", &self.database);
        if let Some(user) = &self.user {
            cmd.env("PGUSER", user);
        }
        if let Some(password) = &self.password {
            cmd.env("PGPASSWORD", password);
        }
    }
}

fn quote_ident(ident: &str) -> String {
    format!("\"{}\"", ident.replace('"', "\"\""))
}

fn sql_string_literal(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

// ── Readiness ───────────────────────────────────────────────────────────────

/// Poll until the cluster's `host:port` accepts a TCP connection, or the
/// deadline passes. A TCP connect proves the postmaster is listening; the engine
/// then runs its own pooled connect + migrations (which retry/fail loudly).
async fn wait_for_pg(url: &str, timeout: Duration) -> Result<(), BoxError> {
    let (host, port) = parse_host_port(url)
        .ok_or_else(|| format!("could not parse host:port from database_url '{url}'"))?;
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

/// Extract `host` and `port` from a `postgres://[user[:pass]@]host[:port]/db`
/// URL. Defaults the port to 5432 when omitted.
fn parse_host_port(url: &str) -> Option<(String, u16)> {
    let rest = url.split("://").nth(1).unwrap_or(url);
    let after_auth = rest.rsplit('@').next().unwrap_or(rest);
    let authority = after_auth.split(['/', '?']).next().unwrap_or(after_auth);
    if authority.is_empty() {
        return None;
    }
    match authority.rsplit_once(':') {
        Some((host, port)) => {
            let port = port.parse::<u16>().ok()?;
            Some((host.to_string(), port))
        }
        None => Some((authority.to_string(), 5432)),
    }
}

#[cfg(test)]
mod tests {
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

    #[test]
    fn parse_host_port_variants() {
        assert_eq!(
            parse_host_port("postgres://lucidos:lucidos@127.0.0.1:5599/lucidos"),
            Some(("127.0.0.1".to_string(), 5599))
        );
        assert_eq!(
            parse_host_port("postgres://lucidos@localhost/lucidos"),
            Some(("localhost".to_string(), 5432))
        );
        assert_eq!(
            parse_host_port("postgres://localhost:5432/db?sslmode=disable"),
            Some(("localhost".to_string(), 5432))
        );
        assert_eq!(
            parse_host_port("not-a-url"),
            Some(("not-a-url".to_string(), 5432))
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
    fn sql_quoting_handles_workspace_database_names() {
        assert_eq!(quote_ident("lucidos_e2e-test"), "\"lucidos_e2e-test\"");
        assert_eq!(sql_string_literal("lucidos_e2e-test"), "'lucidos_e2e-test'");
    }

    #[test]
    fn parses_pg_url_into_env_parts_without_query() {
        assert_eq!(
            PgUrlParts::parse(
                "postgres://lucidos:secret@127.0.0.1:5544/lucidos_dev?sslmode=disable"
            )
            .unwrap(),
            PgUrlParts {
                user: Some("lucidos".into()),
                password: Some("secret".into()),
                host: "127.0.0.1".into(),
                port: 5544,
                database: "lucidos_dev".into(),
            }
        );
        assert_eq!(
            PgUrlParts::parse("postgres://lucidos@localhost/lucidos").unwrap(),
            PgUrlParts {
                user: Some("lucidos".into()),
                password: None,
                host: "localhost".into(),
                port: 5432,
                database: "lucidos".into(),
            }
        );
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

    #[test]
    fn should_recreate_foreign_only_for_a_mismatched_existing_dir() {
        // Same major → keep (lossless restart of the existing data dir).
        assert!(!should_recreate_foreign(Some(18), 18));
        // Foreign major → recreate.
        assert!(should_recreate_foreign(Some(17), 18));
        // Uninitialized dir (first run) → not foreign, just initdb.
        assert!(!should_recreate_foreign(None, 18));
    }

    #[test]
    fn foreign_aside_dir_name_is_stable_and_recoverable() {
        assert_eq!(
            foreign_aside_dir_name("pgdata", 17, "20260622-120000"),
            "pgdata.foreign-17-20260622-120000"
        );
    }

    #[test]
    fn read_data_dir_major_and_postmaster_pid_parse_real_files() {
        let dir = std::env::temp_dir().join(format!("lucidos-pgmeta-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        // No files yet → None (first run / no running cluster).
        assert_eq!(read_data_dir_major(&dir), None);
        assert_eq!(read_postmaster_pid(&dir), None);

        std::fs::write(dir.join("PG_VERSION"), "18\n").unwrap();
        assert_eq!(read_data_dir_major(&dir), Some(18));

        // postmaster.pid: line 1 = pid, line 2 = data dir, line 4 = port.
        std::fs::write(
            dir.join("postmaster.pid"),
            "4242\n/some/data\n1700000000\n5599\n127.0.0.1\n",
        )
        .unwrap();
        assert_eq!(read_postmaster_pid(&dir), Some(4242));
        assert_eq!(read_postmaster_port(&dir), Some(5599));

        let _ = std::fs::remove_dir_all(&dir);
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

    /// A unique throwaway parent dir + its `pgdata` child, so a moved-aside
    /// sibling lands inside the same parent and cleanup removes everything.
    fn gated_temp(tag: &str) -> (PathBuf, PathBuf) {
        let parent = std::env::temp_dir().join(format!("{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&parent);
        std::fs::create_dir_all(&parent).unwrap();
        let data = parent.join("pgdata");
        (parent, data)
    }

    /// Stale lock (unclean stop) must recover by restarting the SAME data dir —
    /// no wipe — with the prior data intact.
    #[tokio::test]
    async fn gated_stale_lock_recovers_and_preserves_data() {
        let Some((bin, lib)) = gated_pg_dirs() else {
            eprintln!(
                "SKIP gated_stale_lock_recovers_and_preserves_data: no LUCIDOS_PG_BIN_DIR/LIB"
            );
            return;
        };
        let (parent, data) = gated_temp("lucidos-pg-stale");

        // First start: real cluster + a marker row in a workspace database.
        let port = ensure_embedded_cluster(&bin, &lib, &data).await.unwrap();
        create_database_embedded(&bin, &lib, port, "lucidos_gated").unwrap();
        psql_query_embedded(
            &bin,
            &lib,
            port,
            "lucidos_gated",
            "CREATE TABLE t(x int); INSERT INTO t VALUES (7)",
        )
        .unwrap();

        // Simulate an orphan / unclean stop: SIGKILL the postmaster, leaving its
        // stale postmaster.pid behind.
        let pid = read_postmaster_pid(&data).expect("postmaster.pid");
        #[cfg(unix)]
        // SAFETY: SIGKILL to this test cluster's own recorded postmaster pid.
        unsafe {
            libc::kill(pid as libc::pid_t, libc::SIGKILL);
        }
        std::thread::sleep(Duration::from_millis(300));

        // Recover: restart the SAME data dir (crash recovery), row survives.
        let port2 = ensure_embedded_cluster(&bin, &lib, &data).await.unwrap();
        let got =
            psql_query_embedded(&bin, &lib, port2, "lucidos_gated", "SELECT x FROM t").unwrap();
        assert_eq!(got.trim(), "7", "data must survive a stale-lock recovery");
        // The data dir was NOT moved aside.
        assert!(
            !sibling_foreign_dir_exists(&parent),
            "same-version recovery must not move the data dir aside"
        );

        stop_cluster_robustly(&bin, &lib, &data);
        let _ = std::fs::remove_dir_all(&parent);
    }

    /// A foreign-major data dir must NOT be adopted: tear down, move the old dir
    /// aside (recoverable, never deleted), initdb a fresh bundled-version cluster.
    #[tokio::test]
    async fn gated_foreign_version_recreates_and_moves_old_dir_aside() {
        let Some((bin, lib)) = gated_pg_dirs() else {
            eprintln!("SKIP gated_foreign_version_recreates_and_moves_old_dir_aside: no LUCIDOS_PG_BIN_DIR/LIB");
            return;
        };
        let (parent, data) = gated_temp("lucidos-pg-foreign");

        // Init a real cluster, stop it, then forge a foreign-major PG_VERSION.
        ensure_embedded_cluster(&bin, &lib, &data).await.unwrap();
        stop_cluster_robustly(&bin, &lib, &data);
        let bundled = bundled_major(&bin, &lib).unwrap();
        std::fs::write(data.join("PG_VERSION"), format!("{}\n", bundled - 1)).unwrap();

        // Recreate path: move the foreign dir aside + initdb fresh + start.
        let _port = ensure_embedded_cluster(&bin, &lib, &data).await.unwrap();

        assert!(
            sibling_foreign_dir_exists(&parent),
            "foreign-version data dir must be moved aside, not deleted"
        );
        assert_eq!(
            read_data_dir_major(&data),
            Some(bundled),
            "the recreated data dir must be the bundled major version"
        );

        stop_cluster_robustly(&bin, &lib, &data);
        let _ = std::fs::remove_dir_all(&parent);
    }

    /// Whether a `*.foreign-*` moved-aside sibling exists under `parent`.
    fn sibling_foreign_dir_exists(parent: &Path) -> bool {
        std::fs::read_dir(parent)
            .map(|rd| {
                rd.flatten()
                    .any(|e| e.file_name().to_string_lossy().contains(".foreign-"))
            })
            .unwrap_or(false)
    }
}
