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
                Ok(PgBackend::Embedded { bin, lib })
            }
            other => Err(format!("unknown LUCIDOS_GATEWAY_PG_BACKEND '{other}'").into()),
        }
    }
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

async fn ensure_embedded_cluster(bin: &Path, lib: &Path, data: &Path) -> Result<u16, BoxError> {
    if !data.join("PG_VERSION").exists() {
        std::fs::create_dir_all(data)?;
        let mut cmd = Command::new(bin.join("initdb"));
        with_pg_libpath(&mut cmd, lib);
        cmd.arg("-D").arg(data);
        cmd.args(["-U", PG_USER, "-A", "trust", "--encoding=UTF8"]);
        if !cmd.status()?.success() {
            return Err("initdb failed".into());
        }
    }

    if pg_ctl_status(bin, lib, data) {
        if let Some(port) = read_postmaster_port(data) {
            return Ok(port);
        }
        return Err(format!(
            "embedded Postgres is running at {} but its postmaster.pid has no port",
            data.display()
        )
        .into());
    }

    if data.join("postmaster.pid").exists() {
        // A leftover lock from an unclean stop — clear it before a fresh start.
        let _ = pg_ctl(bin, lib, data, &["-m", "immediate", "stop"]);
    }

    let port = crate::registry::Registry::default().allocate_port()?;
    let log = data.join("server.log");
    let log_s = log.to_string_lossy().to_string();
    let opts = format!("-p {port} -c listen_addresses=127.0.0.1");
    pg_ctl(bin, lib, data, &["-l", &log_s, "-o", &opts, "-w", "start"])?;

    let admin_url = embedded_database_url(port, PG_ADMIN_DATABASE);
    wait_for_pg(&admin_url, Duration::from_secs(30)).await?;
    Ok(port)
}

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
}
