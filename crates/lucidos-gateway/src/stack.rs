use crate::engine_hop::{self, Credential, EngineAddress};
use crate::postgres::PgHandle;
use crate::registry::Workspace;
use crate::row_subscription::RowSubscription;
use lucidos_gateway_stamp::{StampKey, ENV_STAMP_KEY_ON_STDIN, ENV_STARTED_BY_GATEWAY};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

pub(crate) fn path_is_in_cc_worktree(path: &Path) -> bool {
    let comps: Vec<_> = path
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    comps
        .windows(2)
        .any(|w| w[0] == ".lucidos" && w[1] == "worktrees")
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Health {
    Restoring,
    Booting,
    Healthy,
    Unhealthy,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum EngineKeeper {
    Gateway,
    External,
}

pub struct StackRuntime {
    pub ws: Workspace,
    pub resolved_dir: PathBuf,
    pub pg: PgHandle,
    pub engine: Option<Child>,
    pub keeper: EngineKeeper,
    pub health: Health,
    pub restart_attempts: u32,
    pub health_misses: u32,
    pub last_spawn: Option<Instant>,
    pub last_error: Option<String>,
    pub last_unread: Option<u64>,
    pub last_successful_backup: Option<LastSuccessfulBackup>,
    pub row_subscription: RowSubscription,
}

impl StackRuntime {
    pub fn engine_address(&self) -> EngineAddress {
        EngineAddress::for_workspace(&self.resolved_dir, self.ws.port)
    }

    pub fn blank_row(&mut self) {
        self.last_unread = None;
        self.last_successful_backup = None;
        self.row_subscription.forget_devices();
    }

    pub fn stop_row_subscription(&mut self) {
        self.row_subscription.stop();
        self.blank_row();
    }

    pub fn keep_row_subscription(
        &mut self,
        event_filter: bool,
        stack: &std::sync::Arc<tokio::sync::Mutex<StackRuntime>>,
        engine: &EngineAddress,
    ) {
        if self.row_subscription.note_engine_stream(event_filter) {
            crate::log!(
                "[Gateway] the engine of '{}' is older than the row stream: its picker row shows \
                 no badge and no backup line until it restarts",
                lucidos_diagnostic_log::private(self.ws.id.clone())
            );
        }
        if !event_filter {
            self.stop_row_subscription();
        } else if !self.row_subscription.is_live() {
            self.blank_row();
            self.row_subscription
                .start(std::sync::Arc::downgrade(stack), engine.clone());
        }
    }
}

#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
pub struct LastSuccessfulBackup {
    pub at: Option<chrono::DateTime<chrono::Utc>>,
    pub stale: bool,
    #[serde(skip)]
    pub stale_at: Option<chrono::DateTime<chrono::Utc>>,
    pub configured: bool,
}

impl LastSuccessfulBackup {
    pub fn as_of(mut self, now: chrono::DateTime<chrono::Utc>) -> Self {
        self.stale = self.stale || self.stale_at.is_some_and(|at| now > at);
        self
    }
}

#[derive(Serialize, Clone, Debug)]
pub struct WorkspaceStatus {
    pub id: String,
    pub name: String,
    pub path: PathBuf,
    pub port: u16,
    pub health: Health,
    pub autostart: bool,
    pub origin: crate::registry::WorkspaceOrigin,
    pub machine_made: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unread_count: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_successful_backup: Option<LastSuccessfulBackup>,
    /// An interrupted delete holds this workspace until Trash finishes or
    /// restores it, so the picker points there instead of offering a restart.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub delete_interrupted: bool,
}

impl WorkspaceStatus {
    pub fn without_engine(
        ws: &Workspace,
        path: PathBuf,
        health: Health,
        last_error: Option<String>,
    ) -> Self {
        WorkspaceStatus {
            id: ws.id.clone(),
            name: ws.name.clone(),
            path,
            port: ws.port,
            health,
            autostart: ws.autostart,
            origin: ws.cleanup.origin,
            machine_made: ws.cleanup.origin.is_machine_made(),
            last_error,
            unread_count: None,
            last_successful_backup: None,
            delete_interrupted: false,
        }
    }
}

impl StackRuntime {
    pub fn status(&self) -> WorkspaceStatus {
        WorkspaceStatus {
            id: self.ws.id.clone(),
            name: self.ws.name.clone(),
            path: self.resolved_dir.clone(),
            port: self.ws.port,
            health: self.health,
            autostart: self.ws.autostart,
            origin: self.ws.cleanup.origin,
            machine_made: self.ws.cleanup.origin.is_machine_made(),
            last_error: self.last_error.clone(),
            unread_count: self.last_unread,
            last_successful_backup: self
                .last_successful_backup
                .map(|line| line.as_of(chrono::Utc::now())),
            delete_interrupted: false,
        }
    }
}

fn engine_env_overrides(
    ws: &Workspace,
    resolved_dir: &Path,
    database_url: &str,
    gateway_port: u16,
) -> Vec<(&'static str, std::ffi::OsString)> {
    vec![
        ("LUCIDOS_WORKSPACE", resolved_dir.as_os_str().to_os_string()),
        ("DATABASE_URL", database_url.into()),
        ("LUCIDOS_API_PORT", ws.port.to_string().into()),
        ("LUCIDOS_WORKSPACE_ID", ws.id.clone().into()),
        (
            "LUCIDOS_WORKSPACE_ORIGIN",
            ws.cleanup.origin.as_str().into(),
        ),
        ("LUCIDOS_GATEWAY_PORT", gateway_port.to_string().into()),
        (
            "LUCIDOS_HOOK_PORT",
            crate::hook_socket::resolved_hook_port(gateway_port)
                .unwrap_or(0)
                .to_string()
                .into(),
        ),
    ]
}

fn preview_tls_handoff(
    leaf: Option<&lucidos_loopback_ca::PreviewLeaf>,
    user_cert: Option<&str>,
    user_key: Option<&str>,
) -> [(&'static str, Option<String>); 4] {
    use lucidos_local_token::loopback_cert::{
        PREVIEW_TLS_CERT_ENV, PREVIEW_TLS_KEY_ENV, PREVIEW_USER_TLS_CERT_ENV,
        PREVIEW_USER_TLS_KEY_ENV,
    };
    let user = match (user_cert, user_key) {
        (Some(c), Some(k)) if crate::net_config::serves_tls(user_cert, user_key) => {
            Some((c.trim().to_string(), k.trim().to_string()))
        }
        _ => None,
    };
    let (user_cert, user_key) = user.unzip();
    let path = |p: &Path| p.display().to_string();
    [
        (PREVIEW_TLS_CERT_ENV, leaf.map(|l| path(&l.cert))),
        (PREVIEW_TLS_KEY_ENV, leaf.map(|l| path(&l.key))),
        (PREVIEW_USER_TLS_CERT_ENV, user_cert),
        (PREVIEW_USER_TLS_KEY_ENV, user_key),
    ]
}

pub struct EngineLaunch<'a> {
    pub loopback: bool,
    pub proto: &'a str,
    pub disposable: bool,
    pub managed_database: bool,
    pub preview_leaf: Option<&'a lucidos_loopback_ca::PreviewLeaf>,
}

fn managed_database_handoff(cmd: &mut Command, managed: bool) {
    cmd.env_remove("LUCIDOS_MANAGED_DATABASE");
    if managed {
        cmd.env("LUCIDOS_MANAGED_DATABASE", "1");
    }
}

pub fn spawn_engine(
    engine_bin: &Path,
    ws: &Workspace,
    resolved_dir: &Path,
    database_url: &str,
    gateway_port: u16,
    launch: EngineLaunch<'_>,
) -> std::io::Result<SpawnedEngine> {
    let EngineLaunch {
        loopback,
        proto,
        disposable,
        managed_database,
        preview_leaf,
    } = launch;
    let stamp_key = StampKey::generate()?;
    crate::diagnostic_log::secret(stamp_key.handover_line().trim().to_owned());
    let state_dir = resolved_dir.join(".lucidos");
    std::fs::create_dir_all(&state_dir)?;

    crate::file_backup::exclude(&state_dir, "workspace state dir");

    let mut cmd = Command::new(engine_bin);
    managed_database_handoff(&mut cmd, managed_database);
    cmd.current_dir(resolved_dir).envs(engine_env_overrides(
        ws,
        resolved_dir,
        database_url,
        gateway_port,
    ));
    for (name, value) in preview_tls_handoff(
        preview_leaf,
        std::env::var("LUCIDOS_TLS_CERT").ok().as_deref(),
        std::env::var("LUCIDOS_TLS_KEY").ok().as_deref(),
    ) {
        match value {
            Some(v) => cmd.env(name, v),
            None => cmd.env_remove(name),
        };
    }

    if let Some(dir) = std::env::var_os("LUCIDOS_STATIC_DIR") {
        if !disposable && path_is_in_cc_worktree(Path::new(&dir)) {
            crate::log_legacy!(
                "[Gateway] refusing to pass worktree-pinned LUCIDOS_STATIC_DIR to engine \
                 '{}': {} — relaunch the stack from the real checkout \
                 (see docs/plans/2026-07-26-worktree-pinned-stack-guard.md)",
                ws.id,
                Path::new(&dir).display()
            );
            cmd.env_remove("LUCIDOS_STATIC_DIR");
        }
    }
    if loopback {
        cmd.env("LUCIDOS_BIND_LOOPBACK", "1")
            .env_remove("LUCIDOS_BIND_ALL")
            .env_remove("LUCIDOS_TLS_CERT")
            .env_remove("LUCIDOS_TLS_KEY");
    } else {
        cmd.env_remove("LUCIDOS_BIND_LOOPBACK");
        if crate::net_config::network_toml_exists() {
            cmd.env_remove("LUCIDOS_BIND_ALL");
        } else {
            cmd.env("LUCIDOS_BIND_ALL", "1");
        }
    }

    #[cfg(unix)]
    if !disposable {
        use std::os::unix::process::CommandExt;
        // SAFETY: setsid is async-signal-safe and takes no pointers. Failure
        // (already a session leader) is harmless — ignore the result.
        unsafe {
            cmd.pre_exec(|| {
                libc::setsid();
                Ok(())
            });
        }
    }

    cmd.env(ENV_STAMP_KEY_ON_STDIN, "1").stdin(Stdio::piped());
    cmd.env(ENV_STARTED_BY_GATEWAY, "1");
    let mut child = cmd.spawn()?;
    let stamp_key = hand_over_stamp_key(&mut child, stamp_key);
    publish_ports_file(resolved_dir, ws, ws.port, proto);
    Ok(SpawnedEngine { child, stamp_key })
}

#[derive(Debug)]
pub struct SpawnedEngine {
    pub child: Child,
    pub stamp_key: Option<StampKey>,
}

fn hand_over_stamp_key(child: &mut Child, key: StampKey) -> Option<StampKey> {
    use std::io::Write as _;
    let Some(mut stdin) = child.stdin.take() else {
        crate::log_legacy!(
            "[Gateway] engine pid {} has no stdin pipe for its stamp key",
            child.id()
        );
        return None;
    };
    if let Err(e) = stdin.write_all(key.handover_line().as_bytes()) {
        crate::log_legacy!(
            "[Gateway] could not hand engine pid {} its stamp key: {e}",
            child.id()
        );
        return None;
    }
    Some(key)
}

pub fn publish_ports_file(resolved_dir: &Path, ws: &Workspace, api_port: u16, proto: &str) {
    let path = resolved_dir.join(".lucidos/ports");
    let existing = std::fs::read_to_string(&path).ok();
    let contents = merge_ports_file(existing.as_deref(), api_port, proto);
    if let Err(e) = std::fs::write(&path, contents) {
        crate::log_legacy!(
            "[Gateway] could not write {} for '{}': {e} \
             (cross-workspace spawns aimed at this workspace will not resolve it)",
            path.display(),
            ws.id
        );
    }
}

fn merge_ports_file(existing: Option<&str>, api_port: u16, proto: &str) -> String {
    let api_line = format!("API_PORT={api_port}");
    let proto_line = format!("PROTO={proto}");
    let Some(existing) = existing else {
        return format!("{api_line}\n{proto_line}\n");
    };
    let mut out = Vec::new();
    let (mut saw_api, mut saw_proto) = (false, false);
    for line in existing.lines() {
        if line.starts_with("API_PORT=") {
            saw_api = true;
            out.push(api_line.clone());
        } else if line.starts_with("PROTO=") {
            saw_proto = true;
            out.push(proto_line.clone());
        } else {
            out.push(line.to_string());
        }
    }
    if !saw_api {
        out.push(api_line);
    }
    if !saw_proto {
        out.push(proto_line);
    }
    out.push(String::new());
    out.join("\n")
}

pub fn live_engine_pid(resolved_dir: &Path) -> Option<u32> {
    lucidos_owner_lock::engine::live_pid(resolved_dir)
}

pub fn adopted_engine_alive(resolved_dir: &Path) -> bool {
    let named = std::fs::read_to_string(lucidos_owner_lock::engine::pid_path(resolved_dir))
        .ok()
        .and_then(|raw| raw.trim().parse::<u32>().ok());
    named.is_some_and(pid_is_live) && live_engine_pid(resolved_dir) == named
}

#[cfg(unix)]
pub fn pid_is_live(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    let mut status: libc::c_int = 0;
    // SAFETY: `waitpid` with an explicit pid and `WNOHANG` never blocks, only
    // ever touches that one pid, and writes into a stack local we own.
    let waited = unsafe { libc::waitpid(pid as libc::pid_t, &mut status, libc::WNOHANG) };
    if waited > 0 {
        return false;
    }
    if waited == 0 {
        return true;
    }
    // SAFETY: signal 0 performs existence/permission checks without delivering
    // a signal; returns 0 iff the process exists.
    unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
}

#[cfg(not(unix))]
pub fn pid_is_live(_pid: u32) -> bool {
    true
}

#[cfg(unix)]
pub fn reap_forked_pid(pid: u32) {
    if pid == 0 {
        return;
    }
    std::thread::spawn(move || {
        let mut status: libc::c_int = 0;
        loop {
            // SAFETY: an explicit pid, and a stack local we own for the status.
            // Blocks until that one pid exits, or returns ECHILD at once if it
            // is not our child.
            let waited = unsafe { libc::waitpid(pid as libc::pid_t, &mut status, 0) };
            if waited >= 0 {
                return;
            }
            if std::io::Error::last_os_error().raw_os_error() != Some(libc::EINTR) {
                return;
            }
        }
    });
}

#[cfg(not(unix))]
pub fn reap_forked_pid(_pid: u32) {}

pub fn reclaim_stale_engine(resolved_dir: &Path) {
    #[cfg(unix)]
    if let Some(pid) = live_engine_pid(resolved_dir) {
        let Some(signal_pid) = single_process_pid(pid) else {
            crate::log_legacy!(
                "[Gateway] ignoring pidfile in {} naming pid {pid}, which is no single process",
                resolved_dir.display()
            );
            return;
        };
        if let Some(exe) = other_program_at_pid(signal_pid, ENGINE_EXECUTABLE_NAME) {
            crate::log_legacy!(
                "[Gateway] pidfile in {} names pid {pid}, now running {}; not signalling it",
                resolved_dir.display(),
                exe.display()
            );
            return;
        }
        // SAFETY: kill with a valid signal number on a positive pid that is
        // not known to run anything but the engine. A dead pid returns ESRCH.
        unsafe {
            libc::kill(signal_pid, libc::SIGUSR1);
        }
        reap_forked_pid(pid);
    }
    #[cfg(not(unix))]
    let _ = resolved_dir;
}

#[cfg(unix)]
pub(crate) fn single_process_pid(pid: u32) -> Option<libc::pid_t> {
    libc::pid_t::try_from(pid).ok().filter(|p| *p > 0)
}

#[cfg(unix)]
const ENGINE_EXECUTABLE_NAME: &str = "lucidos-engine";

#[cfg(unix)]
pub(crate) fn other_program_at_pid(pid: libc::pid_t, expected: &str) -> Option<PathBuf> {
    pid_executable(pid).filter(|exe| !executable_is_named(exe, expected))
}

#[cfg(unix)]
fn executable_is_named(exe: &Path, expected: &str) -> bool {
    let Some(name) = exe.file_name().and_then(|n| n.to_str()) else {
        return false;
    };
    name.strip_suffix(" (deleted)").unwrap_or(name) == expected
}

#[cfg(target_os = "macos")]
fn pid_executable(pid: libc::pid_t) -> Option<PathBuf> {
    use std::os::unix::ffi::OsStrExt;
    let mut buf = [0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
    // SAFETY: the buffer is writable and its length is passed alongside it.
    let len = unsafe { libc::proc_pidpath(pid, buf.as_mut_ptr().cast(), buf.len() as u32) };
    (len > 0).then(|| PathBuf::from(std::ffi::OsStr::from_bytes(&buf[..len as usize])))
}

#[cfg(all(unix, not(target_os = "macos")))]
fn pid_executable(pid: libc::pid_t) -> Option<PathBuf> {
    std::fs::read_link(format!("/proc/{pid}/exe")).ok()
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ProbeOutcome {
    Healthy,
    Unreachable,
    Slow,
    Other,
    Foreign,
}

pub async fn probe_health(engine: &EngineAddress, workspace: &Path) -> ProbeOutcome {
    probe_engine(engine, workspace).await.outcome
}

const PROBE_TIMEOUT: Duration = Duration::from_secs(5);

async fn get_body(
    engine: &EngineAddress,
    path: &str,
    budget: Duration,
) -> Result<(axum::http::StatusCode, String), ProbeOutcome> {
    let exchange = async {
        let req = axum::http::Request::get(path)
            .body(axum::body::Body::empty())
            .map_err(|_| ProbeOutcome::Other)?;
        let resp = engine_hop::send(engine, req, Credential::None)
            .await
            .map_err(|e| {
                if e.is_foreign() {
                    crate::log_legacy!("[Gateway] engine probe {path} at {engine} refused: {e}");
                    ProbeOutcome::Foreign
                } else if e.is_unreachable() {
                    ProbeOutcome::Unreachable
                } else {
                    ProbeOutcome::Other
                }
            })?;
        let status = resp.status();
        let body = axum::body::to_bytes(axum::body::Body::new(resp.into_body()), usize::MAX)
            .await
            .map_err(|_| ProbeOutcome::Other)?;
        Ok((status, String::from_utf8_lossy(&body).into_owned()))
    };
    tokio::time::timeout(budget, exchange)
        .await
        .unwrap_or(Err(ProbeOutcome::Slow))
}

async fn get_success_body(engine: &EngineAddress, path: &str) -> Option<String> {
    match get_body(engine, path, PROBE_TIMEOUT).await {
        Ok((status, body)) if status.is_success() => Some(body),
        _ => None,
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct EngineProbe {
    pub outcome: ProbeOutcome,
    pub database: DatabaseHealth,
    pub started_by_gateway: bool,
    pub calls_gateway_over_tls: bool,
    pub event_filter: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DatabaseHealth {
    Reachable,
    NotAnswering,
    PoolExhausted,
}

pub async fn probe_engine(engine: &EngineAddress, workspace: &Path) -> EngineProbe {
    let outcome = |outcome| EngineProbe {
        outcome,
        database: DatabaseHealth::Reachable,
        started_by_gateway: false,
        calls_gateway_over_tls: false,
        event_filter: false,
    };
    match get_body(engine, "/api/v1/health", PROBE_TIMEOUT).await {
        Ok((status, body)) if status.is_success() => match own_health_body(&body, workspace) {
            Some(health) => EngineProbe {
                outcome: ProbeOutcome::Healthy,
                database: database_health(&health),
                started_by_gateway: health.get("started_by_gateway")
                    == Some(&serde_json::Value::Bool(true)),
                calls_gateway_over_tls: calls_gateway_over_tls(&health),
                event_filter: health.get(lucidos_gateway_stamp::row_stream::HEALTH_FIELD)
                    == Some(&serde_json::Value::Bool(true)),
            },
            None => outcome(ProbeOutcome::Foreign),
        },
        Ok(_) => outcome(ProbeOutcome::Other),
        Err(failed) => outcome(failed),
    }
}

fn own_health_body(body: &str, workspace: &Path) -> Option<serde_json::Value> {
    let health: serde_json::Value = serde_json::from_str(body).ok()?;
    let reported = Path::new(health.get("workspace_path")?.as_str()?);
    let same = match (reported.canonicalize(), workspace.canonicalize()) {
        (Ok(reported), Ok(workspace)) => reported == workspace,
        _ => reported == workspace,
    };
    same.then_some(health)
}

fn database_health(health: &serde_json::Value) -> DatabaseHealth {
    let flag = |key: &str| health.get(key).and_then(serde_json::Value::as_bool);
    match (flag("database_reachable"), flag("database_pool_exhausted")) {
        (Some(false), Some(true)) => DatabaseHealth::PoolExhausted,
        (Some(false), _) => DatabaseHealth::NotAnswering,
        _ => DatabaseHealth::Reachable,
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct LiveEngine {
    pub started_at: Option<chrono::DateTime<chrono::Utc>>,
}

pub async fn probe_live_engine(engine: &EngineAddress, workspace: &Path) -> Option<LiveEngine> {
    let health = own_health_body(
        &get_success_body(engine, "/api/v1/health").await?,
        workspace,
    )?;
    Some(LiveEngine {
        started_at: started_at_in(&health),
    })
}

fn started_at_in(health: &serde_json::Value) -> Option<chrono::DateTime<chrono::Utc>> {
    let stamp = health.get("started_at")?.as_str()?;
    Some(chrono::DateTime::parse_from_rfc3339(stamp).ok()?.into())
}

fn calls_gateway_over_tls(health: &serde_json::Value) -> bool {
    use lucidos_gateway_stamp::workspace_client::{ClientContract, LOOPBACK_TLS};
    health
        .get("workspace_client")
        .and_then(|c| serde_json::from_value::<ClientContract>(c.clone()).ok())
        .is_some_and(|contract| contract.has(LOOPBACK_TLS))
}

pub async fn fetch_unread_count(engine: &EngineAddress) -> Option<u64> {
    let body = get_success_body(engine, "/api/v1/notifications?limit=0").await?;
    let json: serde_json::Value = serde_json::from_str(&body).ok()?;
    json.get("unread_count")?.as_u64()
}

pub async fn engine_is_idle(engine: &EngineAddress) -> Option<bool> {
    let body = get_success_body(engine, "/api/v1/threads/list?status=running&limit=1").await?;
    no_thread_listed(&body)
}

fn no_thread_listed(body: &str) -> Option<bool> {
    let json: serde_json::Value = serde_json::from_str(body).ok()?;
    Some(json.as_array()?.is_empty())
}

pub async fn fetch_last_successful_backup(engine: &EngineAddress) -> Option<LastSuccessfulBackup> {
    let body = get_success_body(engine, "/api/v1/backup/last-successful").await?;
    parse_last_successful_backup(&body)
}

fn parse_last_successful_backup(body: &str) -> Option<LastSuccessfulBackup> {
    let json: serde_json::Value = serde_json::from_str(body).ok()?;
    let at = match json.get("at") {
        None | Some(serde_json::Value::Null) => None,
        Some(v) => Some(
            chrono::DateTime::parse_from_rfc3339(v.as_str()?)
                .ok()?
                .with_timezone(&chrono::Utc),
        ),
    };
    let stale_at = json
        .get("stale_at")
        .and_then(serde_json::Value::as_str)
        .and_then(|v| chrono::DateTime::parse_from_rfc3339(v).ok())
        .map(|t| t.with_timezone(&chrono::Utc));
    Some(LastSuccessfulBackup {
        at,
        stale: json.get("stale")?.as_bool()?,
        configured: json.get("configured")?.as_bool()?,
        stale_at,
    })
}

pub const HEADER_DEVICE_ID: &str = "x-lucidos-device-id";

pub const HEADER_DEVICE_LABEL: &str = "x-lucidos-device-label";

const RESTART_INTENT_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RestartIntentNotify {
    Skipped,
    Delivered,
    Failed,
}

pub async fn notify_restart_intent(
    engine: &EngineAddress,
    signer: Option<(&str, &StampKey)>,
    device_id: Option<&str>,
) -> RestartIntentNotify {
    let Some(device_id) = device_id.map(str::trim).filter(|d| !d.is_empty()) else {
        return RestartIntentNotify::Skipped;
    };
    let mut req = axum::http::Request::post("/api/v1/internal/restart-intent")
        .header(HEADER_DEVICE_ID, device_id);
    if let Some((workspace_id, key)) = signer {
        req = req.header(
            lucidos_gateway_stamp::HEADER_GATEWAY_STAMP,
            key.sign_now(lucidos_gateway_stamp::Claim::Device {
                workspace_id,
                device_id,
            }),
        );
    }
    let Ok(req) = req.body(axum::body::Body::empty()) else {
        return RestartIntentNotify::Failed;
    };
    let sent = tokio::time::timeout(
        RESTART_INTENT_TIMEOUT,
        engine_hop::send(engine, req, Credential::None),
    )
    .await;
    match sent {
        Ok(Ok(r)) if r.status().is_success() => RestartIntentNotify::Delivered,
        Ok(Ok(r)) => {
            crate::log_legacy!(
                "[Gateway] restart intent to {engine} returned {}",
                r.status()
            );
            RestartIntentNotify::Failed
        }
        Ok(Err(e)) => {
            crate::log_legacy!("[Gateway] restart intent to {engine} failed: {e}");
            RestartIntentNotify::Failed
        }
        Err(_) => {
            crate::log_legacy!(
                "[Gateway] restart intent to {engine} timed out after {RESTART_INTENT_TIMEOUT:?}"
            );
            RestartIntentNotify::Failed
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine_hop::test_engine;
    use crate::registry::Workspace;

    #[test]
    fn only_an_explicit_false_reads_as_an_unreachable_database() {
        use DatabaseHealth::*;
        let table = [
            (r#"{"database_reachable":false}"#, NotAnswering),
            (
                r#"{"database_reachable":false,"database_pool_exhausted":false}"#,
                NotAnswering,
            ),
            (
                r#"{"database_reachable":false,"database_pool_exhausted":true}"#,
                PoolExhausted,
            ),
            (r#"{"database_reachable":true}"#, Reachable),
            (
                r#"{"database_reachable":true,"database_pool_exhausted":true}"#,
                Reachable,
            ),
            (r#"{"status":"ok"}"#, Reachable),
        ];
        for (body, report) in table {
            let health = serde_json::from_str(body).unwrap();
            assert_eq!(database_health(&health), report, "{body}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_running_child_is_live() {
        let mut child = std::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .expect("spawn sleep");
        assert!(pid_is_live(child.id()), "a running child must read as live");
        assert!(
            matches!(child.try_wait(), Ok(None)),
            "probing must not consume the child's exit status"
        );
        let _ = child.kill();
        let _ = child.wait();
    }

    #[cfg(unix)]
    fn wait_until_defunct(pid: u32) {
        loop {
            // SAFETY: zero initializes the output buffer; waitid writes it and
            // waits only for this test's child. WNOWAIT preserves its status.
            let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
            let result = unsafe {
                libc::waitid(
                    libc::P_PID,
                    pid as libc::id_t,
                    &mut info,
                    libc::WEXITED | libc::WNOWAIT,
                )
            };
            if result == 0 {
                return;
            }
            let error = std::io::Error::last_os_error();
            if error.kind() != std::io::ErrorKind::Interrupted {
                panic!("could not observe exited child: {error}");
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn an_unreaped_exited_child_is_a_zombie_and_not_live() {
        let pid = std::process::Command::new("true")
            .spawn()
            .expect("spawn true")
            .id();

        wait_until_defunct(pid);
        // SAFETY: signal 0 is an existence check only. This is the premise of
        // the whole helper: the old probe accepted this pid as alive.
        assert!(
            unsafe { libc::kill(pid as libc::pid_t, 0) == 0 },
            "premise gone: kill(pid, 0) no longer accepts a zombie"
        );

        assert!(
            !pid_is_live(pid),
            "a zombie must not read as live (kill -0 says it does)"
        );
        let mut status: libc::c_int = 0;
        // SAFETY: WNOHANG never blocks; the pid is expected to be reaped already.
        let waited = unsafe { libc::waitpid(pid as libc::pid_t, &mut status, libc::WNOHANG) };
        assert!(waited < 0, "the probe must have reaped the zombie");
    }

    #[cfg(unix)]
    #[test]
    fn pid_zero_is_never_live_and_never_reaps() {
        let mut bystander = std::process::Command::new("true")
            .spawn()
            .expect("spawn true");
        let pid = bystander.id();
        wait_until_defunct(pid);

        assert!(!pid_is_live(0), "pid 0 must never read as live");

        match bystander.try_wait() {
            Ok(Some(_)) => {}
            other => panic!("pid 0 probe consumed another child's status: {other:?}"),
        }
        let _ = bystander.wait();
        let _ = pid;
    }

    #[cfg(unix)]
    #[test]
    fn only_a_positive_pid_is_a_kill_target() {
        assert_eq!(single_process_pid(0), None);
        assert_eq!(single_process_pid(u32::MAX), None);
        assert_eq!(single_process_pid(1 << 31), None);
        assert_eq!(single_process_pid(4242), Some(4242));
    }

    #[cfg(unix)]
    #[test]
    fn a_program_is_recognised_by_its_file_name_alone() {
        let engine = ENGINE_EXECUTABLE_NAME;
        assert!(executable_is_named(
            Path::new("/opt/x/runtime/lucidos-engine"),
            engine
        ));
        assert!(executable_is_named(
            Path::new("/tmp/lucidos-engine (deleted)"),
            engine
        ));
        assert!(!executable_is_named(Path::new("/bin/sleep"), engine));
        assert!(!executable_is_named(
            Path::new("/tmp/lucidos-engine-old"),
            engine
        ));
    }

    #[cfg(unix)]
    #[test]
    fn a_reused_pid_is_named_by_what_it_now_runs() {
        let mut child = std::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .expect("spawn sleep");
        let pid = single_process_pid(child.id()).expect("a positive pid");
        let exe = pid_executable(pid);
        let other = other_program_at_pid(pid, ENGINE_EXECUTABLE_NAME);
        let name = exe
            .as_deref()
            .and_then(Path::file_name)
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .to_string();
        let itself = other_program_at_pid(pid, &name);
        let _ = child.kill();
        let _ = child.wait();
        assert!(exe.is_some(), "a live child's executable must be readable");
        assert_eq!(other, exe);
        assert_eq!(itself, None);
    }

    #[cfg(unix)]
    #[test]
    fn a_gone_pid_is_not_live() {
        let mut child = std::process::Command::new("true")
            .spawn()
            .expect("spawn true");
        let pid = child.id();
        let _ = child.wait();
        assert!(!pid_is_live(pid), "a reaped pid must not read as live");
    }

    #[test]
    fn detects_coding_agent_worktree_paths() {
        for p in [
            "/Users/me/workspaces/dev/.lucidos/worktrees/thread-abc",
            "/Users/me/workspaces/dev/.lucidos/worktrees/thread-abc/crates/lucidos-app/dist",
            "/Users/me/workspaces/dev/.lucidos/worktrees/thread-abc/target/debug/lucidos-engine",
            "/Users/me/workspaces/dev/.lucidos/worktrees/thread-abc/.launch/debug/plain/lucidos-engine",
        ] {
            assert!(
                path_is_in_cc_worktree(Path::new(p)),
                "should be flagged as a worktree path: {p}"
            );
        }
    }

    #[test]
    fn leaves_real_checkout_paths_alone() {
        for p in [
            "/Users/me/projects/lucidos",
            "/Users/me/projects/lucidos/crates/lucidos-app/dist",
            "/Users/me/projects/lucidos/target/debug/lucidos-engine",
            "/Users/me/worktrees/lucidos/crates/lucidos-app/dist",
            "/Users/me/projects/lucidos/.lucidos/served-frontend/0",
        ] {
            assert!(
                !path_is_in_cc_worktree(Path::new(p)),
                "should NOT be flagged as a worktree path: {p}"
            );
        }
    }

    #[test]
    fn requires_adjacent_lucidos_worktrees_components() {
        assert!(!path_is_in_cc_worktree(Path::new(
            "/w/.lucidos/cache/worktrees/thread-abc"
        )));
        assert!(path_is_in_cc_worktree(Path::new("/w/.lucidos/worktrees")));
    }

    #[test]
    fn does_not_require_the_path_to_exist() {
        assert!(path_is_in_cc_worktree(Path::new(
            "/definitely/not/here/.lucidos/worktrees/thread-gone/crates"
        )));
    }

    #[test]
    fn workspace_status_serializes_autostart() {
        let ws = Workspace {
            id: "dev".into(),
            name: "Dev".into(),
            dir: "/ws/dev".into(),
            port: 5173,
            database_url: None,
            cleanup: Default::default(),
            autostart: true,
        };
        let stack = StackRuntime {
            ws,
            resolved_dir: PathBuf::from("/ws/dev"),
            pg: PgHandle::External,
            engine: None,
            keeper: EngineKeeper::Gateway,
            health: Health::Healthy,
            restart_attempts: 0,
            health_misses: 0,
            last_spawn: None,
            last_error: None,
            last_unread: None,
            last_successful_backup: None,
            row_subscription: Default::default(),
        };
        let json = serde_json::to_value(stack.status()).unwrap();
        assert_eq!(json["autostart"], serde_json::json!(true));
        assert_eq!(json["health"], serde_json::json!("healthy"));
        assert_eq!(json["id"], serde_json::json!("dev"));
        assert!(json.get("unread_count").is_none());
    }

    async fn capturing_engine() -> (
        tempfile::TempDir,
        EngineAddress,
        std::sync::Arc<tokio::sync::Mutex<String>>,
    ) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let dir = test_engine::scratch();
        let (listener, engine) = test_engine::bind(&dir);
        let captured = std::sync::Arc::new(tokio::sync::Mutex::new(String::new()));
        let c = captured.clone();
        tokio::spawn(async move {
            if let Ok((mut sock, _)) = listener.accept().await {
                let mut buf = vec![0u8; 4096];
                let n = sock.read(&mut buf).await.unwrap_or(0);
                *c.lock().await = String::from_utf8_lossy(&buf[..n]).into_owned();
                let _ = sock
                    .write_all(b"HTTP/1.1 204 No Content\r\nConnection: close\r\n\r\n")
                    .await;
                let _ = sock.flush().await;
            }
        });
        (dir, engine, captured)
    }

    #[tokio::test]
    async fn a_named_device_reaches_the_engines_restart_intent_route() {
        let (_dir, engine, captured) = capturing_engine().await;
        let key = StampKey::generate().unwrap();
        let outcome =
            notify_restart_intent(&engine, Some(("dev", &key)), Some("picker-device")).await;

        assert_eq!(outcome, RestartIntentNotify::Delivered);
        let got = captured.lock().await.to_lowercase();
        assert!(
            got.starts_with("post /api/v1/internal/restart-intent"),
            "must POST the engine's restart-intent route; engine saw:\n{got}"
        );
        assert!(
            got.contains("x-lucidos-device-id: picker-device"),
            "the device the picker named must ride the request, or the engine \
             refuses it and the restart stays unattributed; engine saw:\n{got}"
        );
        let stamp = got
            .lines()
            .find_map(|l| l.strip_prefix("x-lucidos-gateway-stamp: "))
            .expect("the device is stamped, or the engine reads it as unattested");
        assert!(key.verify_now(
            stamp.trim(),
            lucidos_gateway_stamp::Claim::Device {
                workspace_id: "dev",
                device_id: "picker-device",
            }
        ));
    }

    #[tokio::test]
    async fn the_notify_is_aimed_at_the_stack_being_signalled_and_no_other() {
        let (_target_dir, target, target_saw) = capturing_engine().await;
        let (_bystander_dir, bystander, bystander_saw) = capturing_engine().await;

        notify_restart_intent(&target, None, Some("d1")).await;

        assert!(
            !target_saw.lock().await.is_empty(),
            "the target was notified"
        );
        assert!(
            bystander_saw.lock().await.is_empty(),
            "a peer workspace's engine must not see a restart it was not part of"
        );
        assert_ne!(target, bystander);
    }

    #[tokio::test]
    async fn no_device_means_no_call_at_all() {
        let (_dir, engine, captured) = capturing_engine().await;

        for absent in [None, Some(""), Some("   ")] {
            assert_eq!(
                notify_restart_intent(&engine, None, absent).await,
                RestartIntentNotify::Skipped,
                "no device to name means no notify, not an empty one"
            );
        }
        assert!(
            captured.lock().await.is_empty(),
            "the engine must not have been contacted at all"
        );
    }

    #[tokio::test]
    async fn an_unreachable_engine_fails_the_notify_and_nothing_else() {
        let dir = test_engine::scratch();
        let started = Instant::now();
        let outcome =
            notify_restart_intent(&test_engine::unreachable(&dir), None, Some("d1")).await;

        assert_eq!(outcome, RestartIntentNotify::Failed);
        assert!(
            started.elapsed() < RESTART_INTENT_TIMEOUT * 2,
            "the notify must give up well inside its own budget, not hold the \
             restart open: took {:?}",
            started.elapsed()
        );
    }

    #[test]
    fn workspace_status_serializes_unread_count_when_known() {
        let ws = Workspace {
            id: "dev".into(),
            name: "Dev".into(),
            dir: "/ws/dev".into(),
            port: 5173,
            database_url: None,
            cleanup: Default::default(),
            autostart: false,
        };
        let stack = StackRuntime {
            ws,
            resolved_dir: PathBuf::from("/ws/dev"),
            pg: PgHandle::External,
            engine: None,
            keeper: EngineKeeper::Gateway,
            health: Health::Healthy,
            restart_attempts: 0,
            health_misses: 0,
            last_spawn: None,
            last_error: None,
            last_unread: Some(4),
            last_successful_backup: None,
            row_subscription: Default::default(),
        };
        let json = serde_json::to_value(stack.status()).unwrap();
        assert_eq!(json["unread_count"], serde_json::json!(4));
        assert!(json.get("last_successful_backup").is_none());
    }

    fn healthy_stack(backup: Option<LastSuccessfulBackup>) -> StackRuntime {
        StackRuntime {
            ws: Workspace {
                id: "dev".into(),
                name: "Dev".into(),
                dir: "/ws/dev".into(),
                port: 5173,
                database_url: None,
                cleanup: Default::default(),
                autostart: false,
            },
            resolved_dir: PathBuf::from("/ws/dev"),
            pg: PgHandle::External,
            engine: None,
            keeper: EngineKeeper::Gateway,
            health: Health::Healthy,
            restart_attempts: 0,
            health_misses: 0,
            last_spawn: None,
            last_error: None,
            last_unread: None,
            last_successful_backup: backup,
            row_subscription: Default::default(),
        }
    }

    #[test]
    fn workspace_status_serializes_the_backup_line_when_known() {
        let at = chrono::DateTime::parse_from_rfc3339("2026-08-26T02:00:07Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        let stack = healthy_stack(Some(LastSuccessfulBackup {
            at: Some(at),
            stale: false,
            configured: true,
            stale_at: None,
        }));
        let json = serde_json::to_value(stack.status()).unwrap();
        let line = &json["last_successful_backup"];
        assert_eq!(line["stale"], serde_json::json!(false));
        assert_eq!(line["configured"], serde_json::json!(true));
        assert_eq!(
            chrono::DateTime::parse_from_rfc3339(line["at"].as_str().unwrap())
                .unwrap()
                .timestamp(),
            at.timestamp(),
        );
    }

    #[test]
    fn a_workspace_that_never_backed_up_still_reports_a_line() {
        let stack = healthy_stack(Some(LastSuccessfulBackup {
            at: None,
            stale: true,
            configured: false,
            stale_at: None,
        }));
        let json = serde_json::to_value(stack.status()).unwrap();
        let line = &json["last_successful_backup"];
        assert_eq!(line["at"], serde_json::Value::Null);
        assert_eq!(line["stale"], serde_json::json!(true));
        assert_eq!(line["configured"], serde_json::json!(false));
    }

    #[test]
    fn idleness_is_an_empty_running_list_and_nothing_else() {
        assert_eq!(no_thread_listed("[]"), Some(true));
        assert_eq!(no_thread_listed(r#"[{"thread_id":"t1"}]"#), Some(false));
        assert_eq!(no_thread_listed(r#"{"error":"nope"}"#), None);
        assert_eq!(no_thread_listed("not json"), None);
    }

    #[test]
    fn a_backup_answer_parses_with_and_without_a_timestamp() {
        let dated = parse_last_successful_backup(
            r#"{"at":"2026-08-26T02:00:07Z","stale":false,"configured":true}"#,
        )
        .expect("a dated answer");
        assert_eq!(dated.at.unwrap().to_rfc3339(), "2026-08-26T02:00:07+00:00");
        assert!(!dated.stale);
        assert!(dated.configured);

        let never = parse_last_successful_backup(r#"{"at":null,"stale":true,"configured":true}"#)
            .expect("never backed up is an answer");
        assert!(never.at.is_none());
        assert!(never.stale);
    }

    #[test]
    fn a_backup_line_turns_stale_at_the_engines_instant() {
        let line = parse_last_successful_backup(
            r#"{"at":"2026-08-26T02:00:07Z","stale":false,"configured":true,
                "stale_at":"2026-08-27T08:00:07Z"}"#,
        )
        .expect("a dated answer");
        let stale_at = line.stale_at.expect("the engine named the instant");
        assert!(!line.as_of(stale_at).stale);
        assert!(line.as_of(stale_at + chrono::Duration::seconds(1)).stale);
        let json = serde_json::to_value(line).unwrap();
        assert!(json.get("stale_at").is_none(), "{json}");

        let now = chrono::Utc::now();
        for stale in [true, false] {
            let body = format!(r#"{{"at":null,"stale":{stale},"configured":true}}"#);
            let line = parse_last_successful_backup(&body).unwrap();
            assert_eq!(line.stale_at, None);
            assert_eq!(line.as_of(now).stale, stale);
        }
        let garbled = parse_last_successful_backup(
            r#"{"at":null,"stale":false,"configured":true,"stale_at":"soon"}"#,
        )
        .expect("an unreadable instant does not discard the reading");
        assert_eq!(garbled.stale_at, None);
    }

    #[test]
    fn an_unreadable_backup_answer_is_no_answer() {
        for body in [
            "not json",
            r#"{"at":"yesterday","stale":false,"configured":true}"#,
            r#"{"at":null,"configured":true}"#,
            r#"{"at":null,"stale":true}"#,
            r#"{"at":null,"stale":"no","configured":true}"#,
        ] {
            assert!(
                parse_last_successful_backup(body).is_none(),
                "expected no answer from {body}",
            );
        }
    }

    #[test]
    fn a_health_payload_names_when_the_engine_started() {
        let body =
            serde_json::json!({"status":"ok","started_at":"2026-08-22T09:34:40.135361+00:00"});
        let started = started_at_in(&body).expect("an engine reports its start time");
        assert_eq!(started.to_rfc3339(), "2026-08-22T09:34:40.135361+00:00");
    }

    #[test]
    fn an_unreadable_payload_names_no_start_time() {
        for body in [
            r#"{"status":"ok"}"#,
            r#"{"started_at":42}"#,
            r#"{"started_at":"yesterday"}"#,
        ] {
            let health = serde_json::from_str(body).unwrap();
            assert!(started_at_in(&health).is_none(), "must not parse: {body}");
        }
    }

    #[tokio::test]
    async fn a_socket_nothing_listens_on_holds_no_engine() {
        let dir = test_engine::scratch();
        assert_eq!(
            probe_live_engine(&test_engine::unreachable(&dir), Path::new("/ws/a")).await,
            None
        );
        assert_eq!(
            probe_health(&test_engine::unreachable(&dir), Path::new("/ws/a")).await,
            ProbeOutcome::Unreachable
        );
    }

    #[tokio::test]
    async fn a_strangers_socket_probes_foreign_and_hears_nothing() {
        let dir = test_engine::scratch();
        let (engine, recorded) = test_engine::record_one(&dir);
        assert_eq!(
            probe_health(&engine.held_by_a_stranger(), Path::new("/ws/a")).await,
            ProbeOutcome::Foreign
        );
        assert!(recorded.await.expect("recorder").is_empty());

        let dir = test_engine::scratch();
        let (engine, recorded) = test_engine::record_one(&dir);
        assert_eq!(
            probe_live_engine(&engine.held_by_a_stranger(), Path::new("/ws/a")).await,
            None
        );
        assert!(recorded.await.expect("recorder").is_empty());
    }

    #[tokio::test]
    async fn an_answer_that_names_no_workspace_is_no_engine_of_ours() {
        let (_dir, engine, _saw) = capturing_engine().await;
        assert_eq!(probe_live_engine(&engine, Path::new("/ws/a")).await, None);
    }

    fn engine_answering(body: String) -> (tempfile::TempDir, EngineAddress) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let dir = test_engine::scratch();
        let (listener, engine) = test_engine::bind(&dir);
        tokio::spawn(async move {
            while let Ok((mut sock, _)) = listener.accept().await {
                let mut buf = [0u8; 1024];
                let _ = sock.read(&mut buf).await;
                let head = format!(
                    "HTTP/1.1 200 OK\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                    body.len()
                );
                let _ = sock.write_all(head.as_bytes()).await;
                let _ = sock.write_all(body.as_bytes()).await;
            }
        });
        (dir, engine)
    }

    #[tokio::test]
    async fn a_probe_answered_by_another_workspace_is_foreign() {
        let root = tempfile::TempDir::new().unwrap();
        let (ours, theirs) = (root.path().join("ours"), root.path().join("theirs"));
        std::fs::create_dir_all(&ours).unwrap();
        std::fs::create_dir_all(&theirs).unwrap();
        let body =
            |dir: &Path| serde_json::json!({"status": "ok", "workspace_path": dir}).to_string();

        let (_stranger_dir, stranger) = engine_answering(body(&theirs));
        assert_eq!(probe_health(&stranger, &ours).await, ProbeOutcome::Foreign);
        let (_own_dir, own) = engine_answering(body(&ours));
        assert_eq!(probe_health(&own, &ours).await, ProbeOutcome::Healthy);
    }

    #[tokio::test]
    async fn a_probe_reads_whether_a_gateway_spawned_the_engine() {
        let root = tempfile::TempDir::new().unwrap();
        let ours = root.path().join("ours");
        std::fs::create_dir_all(&ours).unwrap();
        for (said, expected) in [(Some(true), true), (Some(false), false), (None, false)] {
            let mut body = serde_json::json!({"status": "ok", "workspace_path": &ours});
            if let Some(said) = said {
                body["started_by_gateway"] = serde_json::json!(said);
            }
            let (_dir, engine) = engine_answering(body.to_string());
            let probe = probe_engine(&engine, &ours).await;
            assert_eq!(probe.outcome, ProbeOutcome::Healthy);
            assert_eq!(probe.started_by_gateway, expected, "{body}");
        }
    }

    #[tokio::test]
    async fn a_probe_reads_whether_the_engine_serves_the_row_stream() {
        let root = tempfile::TempDir::new().unwrap();
        let ours = root.path().join("ours");
        std::fs::create_dir_all(&ours).unwrap();
        let field = lucidos_gateway_stamp::row_stream::HEALTH_FIELD;
        let cases = [
            (Some(serde_json::json!(true)), true),
            (Some(serde_json::json!(false)), false),
            (Some(serde_json::json!("true")), false),
            (None, false),
        ];
        for (said, expected) in cases {
            let mut body = serde_json::json!({"status": "ok", "workspace_path": &ours});
            if let Some(said) = said {
                body[field] = said;
            }
            let (_dir, engine) = engine_answering(body.to_string());
            let probe = probe_engine(&engine, &ours).await;
            assert_eq!(probe.outcome, ProbeOutcome::Healthy);
            assert_eq!(probe.event_filter, expected, "{body}");
        }
    }

    #[tokio::test]
    async fn a_probe_reads_whether_the_engine_calls_its_gateway_over_tls() {
        use lucidos_gateway_stamp::workspace_client::ClientContract;
        let root = tempfile::TempDir::new().unwrap();
        let ours = root.path().join("ours");
        std::fs::create_dir_all(&ours).unwrap();
        let older = serde_json::json!({"min": 1, "max": 1, "capabilities": ["workspace-api-v1"]});
        let cases = [
            (Some(serde_json::json!(ClientContract::engine())), true),
            (Some(older), false),
            (None, false),
        ];
        for (contract, expected) in cases {
            let mut body = serde_json::json!({"status": "ok", "workspace_path": &ours});
            if let Some(contract) = contract {
                body["workspace_client"] = contract;
            }
            let (_dir, engine) = engine_answering(body.to_string());
            let probe = probe_engine(&engine, &ours).await;
            assert_eq!(probe.calls_gateway_over_tls, expected, "{body}");
        }
    }

    #[test]
    fn a_health_body_is_ours_only_when_it_names_our_workspace() {
        let root = tempfile::TempDir::new().unwrap();
        let ours = root.path().join("ours");
        let theirs = root.path().join("theirs");
        std::fs::create_dir_all(&ours).unwrap();
        std::fs::create_dir_all(&theirs).unwrap();
        let body =
            |dir: &Path| serde_json::json!({"status": "ok", "workspace_path": dir}).to_string();
        assert!(own_health_body(&body(&ours), &ours).is_some());
        assert!(own_health_body(&body(&ours.join(".")), &ours).is_some());
        assert!(own_health_body(&body(&theirs), &ours).is_none());
        assert!(own_health_body(r#"{"status":"ok"}"#, &ours).is_none());
        assert!(own_health_body("not json", &ours).is_none());
    }

    #[test]
    fn a_spawned_engine_inherits_the_model_cache_instead_of_getting_its_own() {
        let ws = Workspace {
            id: "dev".into(),
            name: "Dev".into(),
            dir: "/ws/dev".into(),
            port: 5173,
            database_url: None,
            cleanup: Default::default(),
            autostart: false,
        };
        let overrides =
            engine_env_overrides(&ws, Path::new("/ws/dev"), "postgres://local/dev", 5251);

        let keys: Vec<&str> = overrides.iter().map(|(k, _)| *k).collect();
        for cache_var in ["FASTEMBED_CACHE_DIR", "HF_HOME"] {
            assert!(
                !keys.contains(&cache_var),
                "{cache_var} must be inherited, not set per workspace: {keys:?}"
            );
        }
        assert_eq!(
            keys,
            [
                "LUCIDOS_WORKSPACE",
                "DATABASE_URL",
                "LUCIDOS_API_PORT",
                "LUCIDOS_WORKSPACE_ID",
                "LUCIDOS_WORKSPACE_ORIGIN",
                "LUCIDOS_GATEWAY_PORT",
                "LUCIDOS_HOOK_PORT",
            ],
            "the workspace-specific set changed: add the new variable here deliberately"
        );
    }

    #[test]
    fn a_spawned_engine_gets_its_registered_origin() {
        use crate::registry::WorkspaceOrigin;
        for origin in [
            WorkspaceOrigin::User,
            WorkspaceOrigin::Unknown,
            WorkspaceOrigin::Agent,
            WorkspaceOrigin::Demo,
            WorkspaceOrigin::Eval,
            WorkspaceOrigin::E2e,
        ] {
            let ws = Workspace {
                id: "sample".into(),
                name: "Sample".into(),
                dir: "/ws/sample".into(),
                port: 5173,
                database_url: None,
                cleanup: crate::registry::WorkspaceCleanup {
                    origin,
                    ..Default::default()
                },
                autostart: false,
            };
            let overrides = engine_env_overrides(
                &ws,
                Path::new("/ws/sample"),
                "postgres://local/sample",
                5251,
            );
            let handed = overrides
                .iter()
                .find(|(key, _)| *key == "LUCIDOS_WORKSPACE_ORIGIN")
                .unwrap();
            let wire = serde_json::to_value(origin).unwrap();
            assert_eq!(handed.1.to_str().unwrap(), wire.as_str().unwrap());
        }
    }

    #[test]
    fn a_spawned_engine_is_told_the_port_its_deliveries_arrive_on() {
        let ws = Workspace {
            id: "dev".into(),
            name: "Dev".into(),
            dir: "/ws/dev".into(),
            port: 5173,
            database_url: None,
            cleanup: Default::default(),
            autostart: false,
        };
        let overrides =
            engine_env_overrides(&ws, Path::new("/ws/dev"), "postgres://local/dev", 5251);
        let handed = overrides
            .iter()
            .find(|(k, _)| *k == "LUCIDOS_HOOK_PORT")
            .map(|(_, v)| v.to_string_lossy().to_string())
            .expect("the engine is handed a hook port");

        assert_eq!(
            handed,
            crate::hook_socket::resolved_hook_port(5251)
                .unwrap_or(0)
                .to_string(),
            "the engine must be handed the port this gateway itself resolved"
        );
    }

    #[test]
    fn the_preview_gets_its_leaf_and_the_user_cert_under_names_the_engine_never_serves_with() {
        let leaf = lucidos_loopback_ca::PreviewLeaf {
            cert: PathBuf::from("/home/user/.lucidos/preview-tls/dev/preview-cert.pem"),
            key: PathBuf::from("/home/user/.lucidos/preview-tls/dev/preview-key.pem"),
        };
        let handed = preview_tls_handoff(
            Some(&leaf),
            Some(" /certs/cert.pem "),
            Some("/certs/key.pem\n"),
        );
        assert_eq!(
            handed,
            [
                (
                    "LUCIDOS_PREVIEW_TLS_CERT",
                    Some("/home/user/.lucidos/preview-tls/dev/preview-cert.pem".to_string())
                ),
                (
                    "LUCIDOS_PREVIEW_TLS_KEY",
                    Some("/home/user/.lucidos/preview-tls/dev/preview-key.pem".to_string())
                ),
                (
                    "LUCIDOS_PREVIEW_USER_TLS_CERT",
                    Some("/certs/cert.pem".to_string())
                ),
                (
                    "LUCIDOS_PREVIEW_USER_TLS_KEY",
                    Some("/certs/key.pem".to_string())
                ),
            ]
        );
        assert!(handed.iter().all(|(k, _)| !k.starts_with("LUCIDOS_TLS_")));
    }

    #[test]
    fn a_missing_leaf_or_user_cert_clears_its_names() {
        for (cert, key) in [
            (None, None),
            (Some("/certs/cert.pem"), None),
            (Some("/certs/cert.pem"), Some("  ")),
            (Some(""), Some("/certs/key.pem")),
        ] {
            assert!(
                preview_tls_handoff(None, cert, key)
                    .iter()
                    .all(|(_, value)| value.is_none()),
                "{cert:?} / {key:?}"
            );
        }
    }

    #[test]
    fn publishing_ports_keeps_the_keys_the_dev_scripts_own() {
        let merged = merge_ports_file(
            Some("API_PORT=5173\nVITE_PORT=5173\nPG_PORT=5432\nDATABASE_URL=postgres://x\n"),
            5999,
            "http",
        );
        assert_eq!(
            merged,
            "API_PORT=5999\nVITE_PORT=5173\nPG_PORT=5432\nDATABASE_URL=postgres://x\nPROTO=http\n"
        );
    }

    #[test]
    fn publishing_ports_writes_both_keys_when_there_is_no_file() {
        assert_eq!(
            merge_ports_file(None, 42000, "http"),
            "API_PORT=42000\nPROTO=http\n"
        );
    }

    #[test]
    fn publishing_ports_corrects_a_stale_file() {
        assert_eq!(
            merge_ports_file(Some("API_PORT=41000\nPROTO=https\n"), 42000, "http"),
            "API_PORT=42000\nPROTO=http\n"
        );
    }

    #[test]
    fn disposable_engine_remains_in_the_owned_gateway_group() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let script = tmp.path().join("engine");
        std::fs::write(&script, "#!/bin/sh\nread -r key\nexec /bin/sleep 30\n").unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();
        let ws = Workspace::gateway_provisioned("smoke".into(), "Smoke".into(), 42000);
        let mut spawned = spawn_engine(
            &script,
            &ws,
            tmp.path(),
            "postgres://local/smoke",
            5252,
            EngineLaunch {
                loopback: true,
                proto: "http",
                disposable: true,
                managed_database: false,
                preview_leaf: None,
            },
        )
        .unwrap();
        let group = unsafe { libc::getpgid(spawned.child.id() as i32) };
        let _ = spawned.child.kill();
        spawned.child.wait().unwrap();
        assert_eq!(group, unsafe { libc::getpgrp() });
    }

    #[test]
    fn database_handoff_removes_inherited_provenance_for_external_launches() {
        let mut command = Command::new("/usr/bin/env");
        command.env("LUCIDOS_MANAGED_DATABASE", "1");
        managed_database_handoff(&mut command, false);
        let output = command.output().unwrap();
        assert!(output.status.success());
        assert!(!String::from_utf8(output.stdout)
            .unwrap()
            .lines()
            .any(|line| line.starts_with("LUCIDOS_MANAGED_DATABASE=")));
    }

    #[test]
    fn real_engine_spawn_hands_off_provenance_only_for_managed_databases() {
        use std::os::unix::fs::PermissionsExt;
        for managed in [false, true] {
            let root = tempfile::tempdir().unwrap();
            let script = root.path().join("engine");
            std::fs::write(&script, "#!/bin/sh\nread -r key\nprintf '%s' \"${LUCIDOS_MANAGED_DATABASE-absent}\" > managed.txt\n").unwrap();
            std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();
            let ws = Workspace::gateway_provisioned("fixture".into(), "Fixture".into(), 42000);
            let mut spawned = spawn_engine(
                &script,
                &ws,
                root.path(),
                "postgres://local/fixture",
                5252,
                EngineLaunch {
                    loopback: true,
                    proto: "http",
                    disposable: true,
                    managed_database: managed,
                    preview_leaf: None,
                },
            )
            .unwrap();
            assert!(spawned.child.wait().unwrap().success());
            assert_eq!(
                std::fs::read_to_string(root.path().join("managed.txt")).unwrap(),
                if managed { "1" } else { "absent" }
            );
        }
    }

    #[test]
    fn a_spawn_publishes_the_ports_file_and_leaves_the_pidfile_to_the_engine() {
        let tmp = tempfile::TempDir::new().unwrap();
        let dir = tmp.path();
        let ws = Workspace {
            id: "smoke".into(),
            name: "Smoke".into(),
            dir: dir.display().to_string(),
            port: 42000,
            database_url: None,
            cleanup: Default::default(),
            autostart: false,
        };

        let mut spawned = spawn_engine(
            Path::new("/usr/bin/true"),
            &ws,
            dir,
            "postgres://local/smoke",
            5252,
            EngineLaunch {
                loopback: true,
                proto: "http",
                disposable: false,
                managed_database: false,
                preview_leaf: None,
            },
        )
        .expect("stand-in engine spawns");
        let _ = spawned.child.wait();

        assert_eq!(
            std::fs::read_to_string(dir.join(".lucidos/ports")).unwrap(),
            "API_PORT=42000\nPROTO=http\n"
        );
        assert!(
            !lucidos_owner_lock::engine::pid_path(dir).exists(),
            "the spawner named a process that does not own the workspace yet"
        );
    }

    #[cfg(unix)]
    #[test]
    fn the_stamp_key_crosses_stdin_and_never_the_environment() {
        use std::os::unix::fs::PermissionsExt as _;
        let tmp = tempfile::TempDir::new().unwrap();
        let dir = tmp.path();
        let script = dir.join("stand-in-engine.sh");
        std::fs::write(
            &script,
            "#!/bin/sh\nenv > \"$LUCIDOS_WORKSPACE/env.txt\"\nhead -n 1 > \"$LUCIDOS_WORKSPACE/stdin.txt\"\n",
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        let ws = Workspace {
            id: "stamped".into(),
            name: "Stamped".into(),
            dir: dir.display().to_string(),
            port: 42001,
            database_url: None,
            cleanup: Default::default(),
            autostart: false,
        };

        let mut spawned = spawn_engine(
            &script,
            &ws,
            dir,
            "postgres://local/stamped",
            5252,
            EngineLaunch {
                loopback: true,
                proto: "http",
                disposable: false,
                managed_database: false,
                preview_leaf: None,
            },
        )
        .expect("stand-in engine spawns");
        assert!(spawned.child.wait().unwrap().success());

        let line = std::fs::read_to_string(dir.join("stdin.txt")).unwrap();
        let received = StampKey::read_handover(line.as_bytes()).expect("the key arrived on stdin");
        let claim = lucidos_gateway_stamp::Claim::Device {
            workspace_id: "stamped",
            device_id: "d1",
        };
        assert!(
            received.verify_now(
                &spawned
                    .stamp_key
                    .as_ref()
                    .expect("the handover succeeded")
                    .sign_now(claim),
                claim
            ),
            "the engine received the key the gateway keeps"
        );

        let env = std::fs::read_to_string(dir.join("env.txt")).unwrap();
        assert!(env
            .lines()
            .any(|l| l == format!("{ENV_STAMP_KEY_ON_STDIN}=1")));
        assert!(env
            .lines()
            .any(|l| l == format!("{ENV_STARTED_BY_GATEWAY}=1")));
        assert!(
            !env.contains(line.trim()),
            "the stamp key leaked into the engine's environment"
        );
    }
}
