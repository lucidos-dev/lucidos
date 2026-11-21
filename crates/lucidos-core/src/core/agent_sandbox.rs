//! The OS sandbox around every agent-steered spawn (ADR 0645).
//!
//! A caller holding the pool resolves a [`Sandbox`] once, with
//! [`Sandbox::resolve`]. That is where it fails closed: where this machine
//! cannot sandbox and the owner has not opted out, nothing starts. The spawn
//! site then mints each [`SandboxedCommand`] from it. The backends live in the
//! `lucidos-agent-sandbox` crate. This module decides what they deny.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use lucidos_agent_sandbox::{ps_shim, push_unique, temp_dirs, Availability, Policy};
use sqlx::PgPool;

use crate::core::prefs;
use crate::engine::event_bus::{BusEvent, EventBus, SystemEvent};
use crate::engine::thread_events::permission::{Action, PermissionRow};

type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// Set on every sandboxed child. An engine that finds it already runs inside
/// a Lucidos sandbox, as when an agent starts a test engine.
pub const SANDBOXED_ENV: &str = "LUCIDOS_AGENT_SANDBOXED";

/// Whether agent-steered spawns run sandboxed here, and why not.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "state", rename_all = "kebab-case")]
pub enum SandboxStatus {
    Enforced,
    /// This engine runs inside another Lucidos sandbox that cannot nest
    /// (macOS). Its children stay inside that one, which guards the outer
    /// home, not this engine's own.
    Inherited,
    /// This machine cannot apply the sandbox, so every covered spawn refuses.
    Unavailable {
        reason: String,
    },
    /// The owner turned on `agents_unsandboxed`.
    OwnerOptedOut,
}

/// What this machine can do, before the owner preference.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Machine {
    Available,
    Inherited,
    Unavailable(String),
}

/// The machine fact from a probe result. A failed probe under the marker means
/// an outer seatbelt refused to nest, not a machine without one. Landlock
/// nests, so on Linux the probe passes and the marker never decides.
pub fn machine_from(probe: Availability, marked: bool) -> Machine {
    match probe {
        Availability::Available => Machine::Available,
        Availability::Unavailable(_) if marked => Machine::Inherited,
        Availability::Unavailable(reason) => Machine::Unavailable(reason),
    }
}

/// The status from the machine fact and the owner preference. The preference
/// wins, so an opted-out machine never refuses.
pub fn status_from(machine: &Machine, opted_out: bool) -> SandboxStatus {
    match (opted_out, machine) {
        (true, _) => SandboxStatus::OwnerOptedOut,
        (false, Machine::Available) => SandboxStatus::Enforced,
        (false, Machine::Inherited) => SandboxStatus::Inherited,
        (false, Machine::Unavailable(reason)) => SandboxStatus::Unavailable {
            reason: reason.clone(),
        },
    }
}

/// This machine's status, read fresh from the preference each call.
pub async fn status(pool: &PgPool) -> SandboxStatus {
    let opted_out = prefs::AGENTS_UNSANDBOXED.read(pool).await;
    status_from(&machine().await, opted_out)
}

/// Why this machine cannot sandbox agents, or `None` when it can. A machine
/// fact, unlike [`status`]: the owner preference does not change it.
pub async fn unavailable_reason() -> Option<String> {
    match machine().await {
        Machine::Available | Machine::Inherited => None,
        Machine::Unavailable(reason) => Some(reason),
    }
}

/// Whether this machine's sandbox lets an agent empty a file it protects:
/// Linux below Landlock ABI 3. A machine fact, like [`unavailable_reason`].
pub async fn truncation_unguarded() -> bool {
    matches!(machine().await, Machine::Available) && !lucidos_agent_sandbox::guards_truncation()
}

/// Whether a trigger's write scope leaves metadata outside it open:
/// permissions, owners, times and xattrs. Linux under Landlock alone, where
/// the kernel refuses an unprivileged user namespace. A machine fact.
pub async fn metadata_unguarded() -> bool {
    matches!(machine().await, Machine::Available)
        && lucidos_agent_sandbox::guards_truncation()
        && !tokio::task::spawn_blocking(lucidos_agent_sandbox::guards_metadata)
            .await
            .unwrap_or(false)
}

static MACHINE: OnceLock<Machine> = OnceLock::new();

/// The probe, run once per engine. A kernel or OS cannot gain or lose the
/// sandbox while the engine runs.
pub async fn machine() -> Machine {
    if let Some(known) = MACHINE.get() {
        return known.clone();
    }
    let probed = tokio::task::spawn_blocking(lucidos_agent_sandbox::probe)
        .await
        .unwrap_or_else(|e| Availability::Unavailable(format!("the probe did not finish: {e}")));
    let machine = machine_from(probed, std::env::var_os(SANDBOXED_ENV).is_some());
    MACHINE.get_or_init(|| machine).clone()
}

/// What every covered spawn of one caller runs under. Only [`Sandbox::resolve`]
/// makes one that sandboxes, so a site that takes it cannot skip the gate.
#[derive(Debug, Clone)]
pub struct Sandbox(Mode);

pub use crate::engine::thread_events::SandboxMode;

#[derive(Debug, Clone)]
enum Mode {
    Enforced {
        policy: Arc<Policy>,
        /// Writes go only to roots the policy names (Codex on the exec protocol).
        writes_confined: bool,
    },
    /// Children run bare, inside the outer sandbox this engine inherited.
    Inherited,
    OwnerOptedOut,
    /// Children run bare under a live `run-confinement-tests` permission.
    PermittedOutside,
}

impl Sandbox {
    /// The mode of this resolved value, without reading preferences again.
    pub fn mode(&self) -> SandboxMode {
        match self.0 {
            Mode::Enforced { .. } => SandboxMode::Enforced,
            Mode::Inherited => SandboxMode::Inherited,
            Mode::OwnerOptedOut => SandboxMode::OwnerOptedOut,
            Mode::PermittedOutside => SandboxMode::PermittedOutside,
        }
    }

    /// The policy of this executing process. Bare processes use current hidden
    /// roots for early refusal, but their mode cannot reopen any subtree.
    pub fn policy_for_execution(&self, workspace: &Path) -> Policy {
        match &self.0 {
            Mode::Enforced { policy, .. } => (**policy).clone(),
            _ => policy_for(workspace),
        }
    }

    /// The sandbox for an agent of the workspace at `workspace`. Refuses with
    /// an error naming the preference when this machine cannot apply it.
    pub async fn resolve(pool: &PgPool, workspace: &Path) -> Result<Sandbox, BoxError> {
        resolve_under(pool, NOTICE_BUS.get(), &machine().await, workspace).await
    }

    /// No sandbox, for the confinement tests, which apply their own. Only a live
    /// `run-confinement-tests` permission builds it, and only the confinement
    /// route asks: it is the one bare spawn the engine makes for an agent.
    pub fn outside_for_confinement(permission: &PermissionRow) -> Option<Sandbox> {
        permission
            .is_live_for(Action::RunConfinementTests)
            .then_some(Sandbox(Mode::PermittedOutside))
    }

    /// No sandbox at all, for tests that spawn a fake.
    #[cfg(any(test, feature = "test-support"))]
    pub fn unsandboxed() -> Sandbox {
        Sandbox(Mode::OwnerOptedOut)
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn enforced_for_test(policy: Policy) -> Sandbox {
        Sandbox(Mode::Enforced {
            policy: Arc::new(policy),
            writes_confined: false,
        })
    }

    /// The inherited mode, for tests of what an active sandbox changes.
    #[cfg(any(test, feature = "test-support"))]
    pub fn inherited() -> Sandbox {
        Sandbox(Mode::Inherited)
    }

    /// `program` under this sandbox. Add arguments, environment and stdio to
    /// it as to any other command.
    pub fn command(&self, program: impl AsRef<OsStr>) -> std::io::Result<SandboxedCommand> {
        let mut cmd = match &self.0 {
            Mode::Enforced { policy, .. } => {
                #[cfg(target_os = "macos")]
                if policy.write_scope.is_some() {
                    lucidos_agent_sandbox::seatbelt_log::ensure(
                        LONGEST_CONFINED_COMMAND,
                        seatbelt_stream_problem,
                    );
                }
                tokio::process::Command::from(lucidos_agent_sandbox::command(program, policy)?)
            }
            Mode::Inherited | Mode::OwnerOptedOut | Mode::PermittedOutside => {
                tokio::process::Command::new(program)
            }
        };
        if self.is_active() {
            cmd.env(SANDBOXED_ENV, "1");
        }
        Ok(SandboxedCommand(cmd))
    }

    /// This sandbox with writes allowed only under `roots`, the OS temp dirs
    /// and `/dev`. Reads stay as they were.
    pub fn writes_only_under(&self, roots: &[PathBuf]) -> Sandbox {
        match &self.0 {
            Mode::Enforced { policy, .. } => Sandbox(Mode::Enforced {
                policy: Arc::new(policy.confine_writes(roots)),
                writes_confined: true,
            }),
            other => Sandbox(other.clone()),
        }
    }

    pub fn for_trigger(&self, workspace: &Path, folders: &[PathBuf]) -> Sandbox {
        self.with_trigger_write_scope(
            workspace,
            folders,
            lucidos_agent_sandbox::guards_truncation(),
        )
    }

    fn with_trigger_write_scope(
        &self,
        workspace: &Path,
        folders: &[PathBuf],
        supported: bool,
    ) -> Sandbox {
        let Mode::Enforced {
            policy,
            writes_confined,
        } = &self.0
        else {
            return self.clone();
        };
        if !supported {
            return self.clone();
        }
        let mut policy = (**policy).clone();
        policy.write_scope = Some(
            std::iter::once(
                workspace
                    .canonicalize()
                    .unwrap_or_else(|_| workspace.to_path_buf()),
            )
            .chain(temp_dirs::roots().iter().cloned())
            .chain(folders.iter().cloned())
            // `/dev/shm`, `/dev/fd` and `/dev/stderr` need it, as under
            // `Policy::confine_writes`.
            .chain([PathBuf::from("/dev")])
            .collect(),
        );
        Sandbox(Mode::Enforced {
            policy: Arc::new(policy),
            writes_confined: *writes_confined,
        })
    }

    pub fn has_trigger_write_scope(&self) -> bool {
        matches!(&self.0, Mode::Enforced { policy, .. } if policy.write_scope.is_some())
    }

    /// Best-effort Seatbelt denial for a failed subprocess, from the stream
    /// [`Self::command`] started. Only the launch PID and timestamps match.
    /// A command that succeeded tolerated its refusals, so it never gets one.
    pub async fn seatbelt_write_refusal(
        &self,
        pid: Option<u32>,
        started: chrono::DateTime<chrono::Utc>,
        outcome: &super::shell::TaskOutcome,
    ) -> Option<String> {
        if outcome.is_success() {
            return None;
        }
        #[cfg(target_os = "macos")]
        {
            let pid = pid.filter(|_| self.has_trigger_write_scope())?;
            lucidos_agent_sandbox::seatbelt_log::denial(pid, started).await
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = (pid, started);
            None
        }
    }

    /// Whether this sandbox's own policy keeps its children from reading
    /// `path`. An inherited sandbox guards another home, so it says no.
    pub fn hides(&self, path: &Path) -> bool {
        match &self.0 {
            Mode::Enforced { policy, .. } => policy.hides(path),
            Mode::Inherited | Mode::OwnerOptedOut | Mode::PermittedOutside => false,
        }
    }

    /// Whether this sandbox's own policy confines its children.
    pub fn is_enforced(&self) -> bool {
        matches!(self.0, Mode::Enforced { .. })
    }

    /// Whether its children run inside a Lucidos sandbox, this engine's own
    /// or one it inherited. No seatbelt nests inside then.
    pub fn is_active(&self) -> bool {
        matches!(self.0, Mode::Enforced { .. } | Mode::Inherited)
    }

    /// The dir to prepend to a child's `PATH` so `ps` resolves to the `lucidos
    /// ps` stand-in, when this sandbox is active on macOS.
    pub fn ps_shim_dir(&self, workspace: &Path, cli_dir: Option<&Path>) -> Option<PathBuf> {
        ps_shim_dir_if(self.is_active(), workspace, cli_dir)
    }

    /// How this sandbox shapes an agent's compile cache (ADR 0567).
    pub fn compile_cache(&self) -> AgentSandbox {
        match self.0 {
            Mode::Enforced {
                writes_confined: true,
                ..
            } => AgentSandbox::WritesConfined,
            Mode::Enforced { .. } | Mode::Inherited => AgentSandbox::Shared,
            Mode::OwnerOptedOut | Mode::PermittedOutside => AgentSandbox::Unsandboxed,
        }
    }
}

/// [`Sandbox::resolve`] with the machine fact and the notice bus given.
async fn resolve_under(
    pool: &PgPool,
    bus: Option<&EventBus>,
    machine: &Machine,
    workspace: &Path,
) -> Result<Sandbox, BoxError> {
    let opted_out = prefs::AGENTS_UNSANDBOXED.read(pool).await;
    match status_from(machine, opted_out) {
        SandboxStatus::OwnerOptedOut => Ok(Sandbox(Mode::OwnerOptedOut)),
        SandboxStatus::Unavailable { reason } => {
            if let Some(bus) = bus {
                notify_refusal(pool, bus, &reason).await;
            }
            Err(format!(
                "{} What the sandbox allows: the knowhow system-knowhow/agent-sandbox.",
                refusal(&reason)
            )
            .into())
        }
        status @ (SandboxStatus::Enforced | SandboxStatus::Inherited) => {
            let socket = crate::core::lucidos_database_url_env()
                .map(|(_, url)| database_socket(&url))
                .transpose()?
                .flatten();
            let socket = match socket {
                Some(socket) => Some(
                    verify_database_socket(
                        pool,
                        socket,
                        &crate::core::database_url(),
                        crate::core::database_is_gateway_managed(),
                    )
                    .await?,
                ),
                None => None,
            };
            if matches!(status, SandboxStatus::Inherited) {
                return Ok(Sandbox(Mode::Inherited));
            }
            let workspace = workspace.to_path_buf();
            let policy = tokio::task::spawn_blocking(move || {
                prepare_home();
                prepare_workspace(&workspace);
                policy_for_database(&workspace, socket)
            })
            .await?;
            Ok(Sandbox(Mode::Enforced {
                policy: Arc::new(policy),
                writes_confined: false,
            }))
        }
    }
}

/// The child policy, including the authenticated database handle it is given.
fn policy_for_database(workspace: &Path, socket: Option<VerifiedDatabaseSocket>) -> Policy {
    let mut policy = policy_for(workspace);
    if let Some(socket) = socket {
        push_unique(&mut policy.connectable_sockets, socket.0);
    }
    policy
}

/// The agent URL is authoritative for socket location and port, including relocated homes.
fn database_socket(url: &str) -> Result<Option<PathBuf>, BoxError> {
    let url = lucidos_pg_url::PgUrl::parse(url)?;
    Ok(match url.host {
        lucidos_pg_url::Host::Socket(dir) => Some(dir.join(format!(".s.PGSQL.{}", url.port))),
        lucidos_pg_url::Host::Tcp(_) => None,
    })
}

/// The file check is necessary, but disk rules alone do not prove what Postgres loaded.
async fn local_auth_requires_password(pool: &PgPool) -> Result<bool, BoxError> {
    Ok(sqlx::query_scalar(
        "SELECT NOT EXISTS (SELECT 1 FROM pg_catalog.pg_hba_file_rules WHERE error IS NOT NULL \
         OR (type = 'local' AND (auth_method IS NULL OR auth_method NOT IN \
         ('scram-sha-256', 'md5', 'password', 'reject'))))",
    )
    .fetch_one(pool)
    .await?)
}

struct VerifiedDatabaseSocket(PathBuf);

/// A failed verification refuses the process on Linux too, where socket connects are not gated.
async fn verify_database_socket(
    pool: &PgPool,
    socket: PathBuf,
    admin_url: &str,
    managed: bool,
) -> Result<VerifiedDatabaseSocket, BoxError> {
    if !managed {
        return Err("Lucidos did not start this process: sandboxed Unix database access requires a gateway-managed bundled database. Use Switch to new version for an older engine; external Unix databases require the owner's explicit sandbox opt-out.".into());
    }
    if database_socket(admin_url)? != Some(socket.clone()) {
        return Err("The agent database socket differs from the engine's database socket".into());
    }
    let safe = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        if !local_auth_requires_password(pool).await? {
            return Ok(false);
        }
        password_auth_is_effective(admin_url).await
    })
    .await
    .map_err(|_| -> BoxError {
        "Timed out verifying local database authentication; no agent process was started".into()
    })??;
    if !safe {
        return Err(
            "Lucidos did not start this process: the local database permits passwordless access. \
             Switch older engines to new version, or correct the cluster's authentication."
                .into(),
        );
    }
    Ok(VerifiedDatabaseSocket(socket))
}

/// Probe a fresh connection with a random incorrect password, never the engine's password.
async fn password_auth_is_effective(admin_url: &str) -> Result<bool, BoxError> {
    use sqlx::{ConnectOptions as _, Connection as _};
    let url = lucidos_pg_url::PgUrl::parse(admin_url)
        .map_err(|_| "The database authentication probe URL is invalid")?;
    let user = url
        .user
        .as_deref()
        .ok_or("The database authentication probe needs an explicit role")?;
    let host = match &url.host {
        lucidos_pg_url::Host::Socket(dir) => dir.to_string_lossy().into_owned(),
        lucidos_pg_url::Host::Tcp(host) => host.clone(),
    };
    let wrong_password = format!("lucidos-auth-check-{}", lucidos_local_token::mint_hex(32)?);
    let options = sqlx::postgres::PgConnectOptions::new()
        .host(&host)
        .port(url.port)
        .username(user)
        .database(&url.database)
        .password(&wrong_password)
        .ssl_mode(sqlx::postgres::PgSslMode::Disable)
        .application_name("lucidos-auth-check")
        .disable_statement_logging();
    match sqlx::PgConnection::connect_with(&options).await {
        Ok(connection) => {
            connection.close().await.map_err(|_| "Could not close the database authentication probe")?;
            Ok(false)
        }
        Err(sqlx::Error::Database(error)) if error.code().as_deref() == Some("28P01") => Ok(true),
        Err(_) => Err("Could not verify password rejection from the local database; no agent process was started".into()),
    }
}

/// A command that may run agent-steered code. Only [`Sandbox::command`] mints
/// one, so a spawn site that takes it cannot skip the sandbox.
#[derive(Debug)]
pub struct SandboxedCommand(tokio::process::Command);

impl std::ops::Deref for SandboxedCommand {
    type Target = tokio::process::Command;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl std::ops::DerefMut for SandboxedCommand {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

const SETTING: &str = "Settings → Permissions → Sandbox agents";

fn refusal(reason: &str) -> String {
    let key = prefs::AGENTS_UNSANDBOXED.spec.key;
    format!(
        "Lucidos did not start this process: agents run in an OS sandbox, and this machine \
         cannot apply it ({reason}). To run agents without it, the owner turns off {SETTING} \
         (preference {key}=true)."
    )
}

/// The bus refusal notices go out on, set once the engine has one.
static NOTICE_BUS: OnceLock<EventBus> = OnceLock::new();

/// Let refusal notices reach the owner. Engine construction calls this.
pub fn install_notice_bus(bus: &EventBus) {
    NOTICE_BUS.get_or_init(|| bus.clone());
}

/// Fixed namespace for the refusal notice id, so one reason notifies once per
/// workspace, across restarts.
const REFUSAL_NOTICE_NAMESPACE: uuid::Uuid = uuid::uuid!("4f0c6a52-9d1e-4b7a-8a51-6c2b1f0e9a37");

async fn notify_refusal(pool: &PgPool, bus: &EventBus, reason: &str) {
    let id = uuid::Uuid::new_v5(&REFUSAL_NOTICE_NAMESPACE, reason.as_bytes()).to_string();
    let exists = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM events WHERE event_type = 'NotificationCreated' \
         AND payload->'data'->>'id' = $1)",
    )
    .bind(&id)
    .fetch_one(pool)
    .await;
    match exists {
        Ok(true) => return,
        Ok(false) => {}
        Err(e) => crate::log_legacy!("[AgentSandbox] Refusal notice lookup failed: {e}"),
    }
    bus.emit_or_log(
        BusEvent::System(SystemEvent::NotificationCreated {
            id,
            title: "Agents cannot run sandboxed here".to_string(),
            message: refusal(reason),
            task_id: None,
            app_id: None,
            thread_id: None,
            event_id: None,
            tap: crate::engine::thread_events::notification::SettingsPage::PERMISSIONS.tap(),
            actor: None,
            audience: None,
            push_group: None,
        }),
        "[AgentSandbox] NotificationCreated",
    )
    .await;
}

/// Folders in `$HOME` that common tools write beneath. Landlock lets a child
/// write inside a folder that existed at spawn, never create one directly in
/// `$HOME` (decision 11 of
/// `docs/plans/2026-11-10-os-sandbox-for-agent-subprocesses.md`).
#[cfg(target_os = "linux")]
const HOME_FOLDERS: [&str; 3] = [".cache", ".config", ".local"];

/// Create [`HOME_FOLDERS`] before a sandboxed child needs them.
fn prepare_home() {
    #[cfg(target_os = "linux")]
    if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
        for name in HOME_FOLDERS {
            if let Err(e) = std::fs::create_dir_all(home.join(name)) {
                crate::log_legacy!("[AgentSandbox] Could not create ~/{name}: {e}");
            }
        }
    }
}

/// Create [`AGENT_WRITABLE_SUBPATHS`] in `workspace`. Landlock opens a folder
/// that exists at spawn, and the read-only `.lucidos/` refuses a new one.
fn prepare_workspace(workspace: &Path) {
    for writable in AGENT_WRITABLE_SUBPATHS {
        if let Err(e) = std::fs::create_dir_all(workspace.join(writable)) {
            crate::log_legacy!("[AgentSandbox] Could not create {writable}: {e}");
        }
    }
}

/// What the sandbox protects on this machine, for an agent of `workspace`.
pub fn policy_for(workspace: &Path) -> Policy {
    let data_dirs = crate::paths::gateway_data_dirs();
    let mut workspaces = crate::paths::registered_workspace_dirs(&data_dirs);
    workspaces.push(workspace.to_path_buf());
    let mut policy = Policy::for_topology(&data_dirs, &workspaces, &AGENT_WRITABLE_SUBPATHS);
    match lucidos_home::private::effective_paths(workspace) {
        Ok(paths) => {
            policy.hidden.splice(..0, paths.built_in);
            policy.private = paths.configured;
        }
        Err(error) => {
            crate::log_legacy!("[AgentSandbox] Cannot load private paths: {error}");
            policy.private.push(PathBuf::from("/"));
        }
    }
    policy
}

/// The same protected paths apply when the engine opens a file for an agent.
pub fn private_file_check(workspace: &Path) -> impl Fn(&Path) -> Result<(), String> + use<> {
    let policy = policy_for(workspace).resolved();
    move |path| {
        if policy.hides_resolved(path) {
            Err("Access to a private path is blocked.".into())
        } else {
            Ok(())
        }
    }
}

/// What an agent writes inside a workspace's `.lucidos/`: its worktrees, the
/// python venv (`runtime/`), and the scratch trees its tools run in.
pub const AGENT_WRITABLE_SUBPATHS: [&str; 5] = [
    crate::paths::WORKTREES_SUBPATH,
    ".lucidos/runtime",
    ".lucidos/tmp",
    ".lucidos/exhaust",
    ".lucidos/staging",
];

/// Where the engine writes the `ps` shim. It sits in the read-only
/// `.lucidos/` and not in [`AGENT_WRITABLE_SUBPATHS`], so an agent cannot
/// replace what its own `ps` runs.
pub const PS_SHIM_SUBPATH: &str = ".lucidos/sandbox-bin";

/// The dir holding the `ps` shim that runs the CLI in `cli_dir`, written fresh
/// so a moved CLI never leaves it stale. `None` where the machine or the
/// sandbox mode wants no shim, and when the shim cannot be written: an agent
/// then runs the system `ps`.
fn ps_shim_dir_if(
    sandbox_active: bool,
    workspace: &Path,
    cli_dir: Option<&Path>,
) -> Option<PathBuf> {
    if !ps_shim::wanted(sandbox_active, cfg!(target_os = "macos")) {
        return None;
    }
    let cli = cli_dir?.join(LUCIDOS_BIN_NAME);
    if !cli.is_file() {
        return None;
    }
    let dir = workspace.join(PS_SHIM_SUBPATH);
    match ps_shim::write(&dir, &ps_shim::script(&cli)) {
        Ok(()) => Some(dir),
        Err(e) => {
            crate::log_legacy!("[AgentSandbox] Could not write the ps shim in {dir:?}: {e}");
            None
        }
    }
}

/// [`Sandbox::ps_shim_dir`] for a caller with no [`Sandbox`], such as the env
/// of a script. It reads the status the same way a spawn would.
pub async fn ps_shim_dir_for_workspace(
    pool: &PgPool,
    workspace: &Path,
    cli_dir: Option<&Path>,
) -> Option<PathBuf> {
    let active = matches!(
        status(pool).await,
        SandboxStatus::Enforced | SandboxStatus::Inherited
    );
    ps_shim_dir_if(active, workspace, cli_dir)
}

/// The machine facts a policy is built from.
pub struct PolicyInputs {
    pub lucidos_home: Option<PathBuf>,
    pub data_dirs: Vec<PathBuf>,
    pub workspaces: Vec<PathBuf>,
}

/// The policy over `inputs`:
///
/// - **Hidden:** the private subtree, each private entry an older home keeps
///   at its old path, and every gateway data dir.
/// - **Open inside a data dir:** its `workspaces/`, where workspaces live.
/// - **Read-only:** each workspace's `.lucidos/`, which holds its grant files,
///   except [`AGENT_WRITABLE_SUBPATHS`].
pub fn policy_from(inputs: &PolicyInputs) -> Policy {
    let mut policy = Policy::for_topology(
        &inputs.data_dirs,
        &inputs.workspaces,
        &AGENT_WRITABLE_SUBPATHS,
    );
    policy.hidden.extend(lucidos_home::private::built_in_paths(
        inputs.lucidos_home.as_deref(),
    ));
    policy
}

/// A tool call or a trigger script, with a minute of slack.
#[cfg(target_os = "macos")]
const LONGEST_CONFINED_COMMAND: std::time::Duration =
    std::time::Duration::from_secs(crate::llm::tools::MAX_TIMEOUT_SECS + 60);

#[cfg(target_os = "macos")]
fn seatbelt_stream_problem(problem: lucidos_agent_sandbox::seatbelt_log::StreamProblem) {
    use lucidos_agent_sandbox::seatbelt_log::StreamProblem;
    match problem {
        StreamProblem::NotRenewed => {
            crate::log!("[Sandbox] The Seatbelt denial stream could not renew")
        }
        StreamProblem::NotStarted => crate::log!(
            "[Sandbox] The Seatbelt denial stream did not start, so folder grants rely on command output"
        ),
    }
}

#[cfg(test)]
#[path = "../runtime/agent_sandbox_tests.rs"]
mod tests;

#[cfg(any(test, feature = "test-support"))]
#[path = "../runtime/agent_sandbox_env_tests.rs"]
pub mod env_guard_tests;

/// How the OS confines an agent's processes, as its compile cache sees it.
/// Read from [`Sandbox::compile_cache`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentSandbox {
    Unsandboxed,
    /// The policy every sandboxed agent shares.
    Shared,
    /// Writes confined to the agent's own roots (Codex on the exec protocol).
    WritesConfined,
}

#[cfg(windows)]
pub(crate) const LUCIDOS_BIN_NAME: &str = "lucidos.exe";
#[cfg(not(windows))]
pub const LUCIDOS_BIN_NAME: &str = "lucidos";
