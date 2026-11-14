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

use lucidos_agent_sandbox::{Availability, Policy};
use sqlx::PgPool;

use super::spawn_env::AgentSandbox;
use crate::core::prefs;
use crate::engine::event_bus::{BusEvent, EventBus, SystemEvent};

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

static MACHINE: OnceLock<Machine> = OnceLock::new();

/// The probe, run once per engine. A kernel or OS cannot gain or lose the
/// sandbox while the engine runs.
async fn machine() -> Machine {
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
}

impl Sandbox {
    /// The sandbox for an agent of the workspace at `workspace`. Refuses with
    /// an error naming the preference when this machine cannot apply it.
    pub async fn resolve(pool: &PgPool, workspace: &Path) -> Result<Sandbox, BoxError> {
        resolve_under(pool, NOTICE_BUS.get(), &machine().await, workspace).await
    }

    /// No sandbox at all, for tests that spawn a fake.
    #[cfg(test)]
    pub fn unsandboxed() -> Sandbox {
        Sandbox(Mode::OwnerOptedOut)
    }

    #[cfg(test)]
    pub fn enforced_for_test(policy: Policy) -> Sandbox {
        Sandbox(Mode::Enforced {
            policy: Arc::new(policy),
            writes_confined: false,
        })
    }

    /// The inherited mode, for tests of what an active sandbox changes.
    #[cfg(test)]
    pub fn inherited() -> Sandbox {
        Sandbox(Mode::Inherited)
    }

    /// `program` under this sandbox. Add arguments, environment and stdio to
    /// it as to any other command.
    pub fn command(&self, program: impl AsRef<OsStr>) -> std::io::Result<SandboxedCommand> {
        let mut cmd = match &self.0 {
            Mode::Enforced { policy, .. } => {
                tokio::process::Command::from(lucidos_agent_sandbox::command(program, policy)?)
            }
            Mode::Inherited | Mode::OwnerOptedOut => tokio::process::Command::new(program),
        };
        if !matches!(self.0, Mode::OwnerOptedOut) {
            cmd.env(SANDBOXED_ENV, "1");
        }
        Ok(SandboxedCommand(cmd))
    }

    /// This sandbox with writes allowed only under `roots`, the OS temp dirs
    /// and `/dev`. Reads stay as they were.
    pub fn writes_only_under(&self, roots: &[PathBuf]) -> Sandbox {
        match &self.0 {
            Mode::Enforced { policy, .. } => Sandbox(Mode::Enforced {
                policy: Arc::new(confine_writes(policy, roots)),
                writes_confined: true,
            }),
            other => Sandbox(other.clone()),
        }
    }

    /// Whether this sandbox's own policy keeps its children from reading
    /// `path`. An inherited sandbox guards another home, so it says no.
    pub fn hides(&self, path: &Path) -> bool {
        match &self.0 {
            Mode::Enforced { policy, .. } => policy.hides(path),
            Mode::Inherited | Mode::OwnerOptedOut => false,
        }
    }

    /// Whether this sandbox's own policy confines its children.
    pub fn is_enforced(&self) -> bool {
        matches!(self.0, Mode::Enforced { .. })
    }

    /// Whether its children run inside a Lucidos sandbox, this engine's own
    /// or one it inherited. No seatbelt nests inside then.
    pub fn is_active(&self) -> bool {
        !matches!(self.0, Mode::OwnerOptedOut)
    }

    /// The dir to prepend to a child's `PATH` so `ps` resolves to the `lucidos
    /// ps` stand-in, when this sandbox is active on macOS.
    pub fn ps_shim_dir(&self, workspace: &Path, cli_dir: Option<&Path>) -> Option<PathBuf> {
        ps_shim_dir_if(self.is_active(), workspace, cli_dir)
    }

    /// How this sandbox shapes an agent's compile cache (ADR 0567).
    pub(crate) fn compile_cache(&self) -> AgentSandbox {
        match self.0 {
            Mode::Enforced {
                writes_confined: true,
                ..
            } => AgentSandbox::WritesConfined,
            Mode::Enforced { .. } | Mode::Inherited => AgentSandbox::Shared,
            Mode::OwnerOptedOut => AgentSandbox::Unsandboxed,
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
        SandboxStatus::Inherited => Ok(Sandbox(Mode::Inherited)),
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
        SandboxStatus::Enforced => {
            let workspace = workspace.to_path_buf();
            let policy = tokio::task::spawn_blocking(move || {
                prepare_home();
                prepare_workspace(&workspace);
                policy_for(&workspace)
            })
            .await?;
            Ok(Sandbox(Mode::Enforced {
                policy: Arc::new(policy),
                writes_confined: false,
            }))
        }
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
        Err(e) => crate::log!("[AgentSandbox] Refusal notice lookup failed: {e}"),
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
            tap: crate::scheduler::notifications::SettingsPage::PERMISSIONS.tap(),
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
                crate::log!("[AgentSandbox] Could not create ~/{name}: {e}");
            }
        }
    }
}

/// Create [`AGENT_WRITABLE_SUBPATHS`] in `workspace`. Landlock opens a folder
/// that exists at spawn, and the read-only `.lucidos/` refuses a new one.
fn prepare_workspace(workspace: &Path) {
    for writable in AGENT_WRITABLE_SUBPATHS {
        if let Err(e) = std::fs::create_dir_all(workspace.join(writable)) {
            crate::log!("[AgentSandbox] Could not create {writable}: {e}");
        }
    }
}

/// What the sandbox protects on this machine, for an agent of `workspace`.
pub fn policy_for(workspace: &Path) -> Policy {
    let data_dirs = crate::paths::gateway_data_dirs();
    let mut workspaces = crate::paths::registered_workspace_dirs(&data_dirs);
    workspaces.push(workspace.to_path_buf());
    let mut policy = policy_from(&PolicyInputs {
        lucidos_home: lucidos_home::path(),
        data_dirs,
        workspaces,
    });
    // An engine on a moved private subtree, such as an e2e engine, hides it
    // from its own agents as well.
    if let Some(moved) = lucidos_home::private::relocated() {
        push(&mut policy.hidden, moved);
    }
    policy
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

/// macOS runs `/bin/ps` setuid, which its sandbox refuses. A sandboxed agent
/// gets the `lucidos ps` stand-in in its place, and nobody else does.
fn ps_shim_wanted(sandbox_active: bool, macos: bool) -> bool {
    sandbox_active && macos
}

fn ps_shim_script(cli: &Path) -> String {
    let quoted = cli.display().to_string().replace('\'', r"'\''");
    format!("#!/bin/sh\nexec '{quoted}' ps \"$@\"\n")
}

/// The dir holding the `ps` shim that runs the CLI in `cli_dir`, written fresh
/// so a moved CLI never leaves it stale. `None` where the machine or the
/// sandbox mode wants no shim, and when the shim cannot be written: an agent
/// then runs the system `ps`.
fn ps_shim_dir_if(
    sandbox_active: bool,
    workspace: &Path,
    cli_dir: Option<&Path>,
) -> Option<PathBuf> {
    if !ps_shim_wanted(sandbox_active, cfg!(target_os = "macos")) {
        return None;
    }
    let cli = cli_dir?.join(super::lucidos_cli::LUCIDOS_BIN_NAME);
    if !cli.is_file() {
        return None;
    }
    let dir = workspace.join(PS_SHIM_SUBPATH);
    match write_ps_shim(&dir, &ps_shim_script(&cli)) {
        Ok(()) => Some(dir),
        Err(e) => {
            crate::log!("[AgentSandbox] Could not write the ps shim in {dir:?}: {e}");
            None
        }
    }
}

/// Write `script` as `dir/ps`, executable, through a rename so a spawn racing
/// this one never runs a half-written file.
fn write_ps_shim(dir: &Path, script: &str) -> std::io::Result<()> {
    let shim = dir.join("ps");
    if std::fs::read_to_string(&shim).is_ok_and(|current| current == script) {
        return Ok(());
    }
    std::fs::create_dir_all(dir)?;
    let temp = dir.join(format!("ps.{}.tmp", uuid::Uuid::new_v4()));
    std::fs::write(&temp, script)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&temp, std::fs::Permissions::from_mode(0o755))?;
    }
    std::fs::rename(&temp, &shim)
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
    let mut policy = Policy::default();
    if let Some(home) = &inputs.lucidos_home {
        push(
            &mut policy.hidden,
            home.join(lucidos_home::private::DIR_NAME),
        );
        for name in lucidos_home::private::legacy_entries() {
            push(&mut policy.hidden, home.join(name));
        }
    }
    let mut workspaces = inputs.workspaces.clone();
    for data in &inputs.data_dirs {
        push(&mut policy.hidden, data.clone());
        let open = data.join("workspaces");
        if let Ok(entries) = std::fs::read_dir(&open) {
            workspaces.extend(entries.flatten().map(|entry| entry.path()));
        }
        push(&mut policy.open_within_hidden, open);
    }
    for ws in workspaces {
        push(&mut policy.read_only, ws.join(".lucidos"));
        for writable in AGENT_WRITABLE_SUBPATHS {
            push(&mut policy.writable_within_read_only, ws.join(writable));
        }
    }
    policy
}

/// `policy` with every write refused outside `roots`, the OS temp dirs and
/// `/dev`. Each open folder inside a hidden root turns read-only too, so the
/// reopening grants reads only.
pub fn confine_writes(policy: &Policy, roots: &[PathBuf]) -> Policy {
    let mut confined = policy.clone();
    push(&mut confined.read_only, PathBuf::from("/"));
    for open in &policy.open_within_hidden {
        push(&mut confined.read_only, open.clone());
    }
    confined.writable_within_read_only.clear();
    let tmpdir = std::env::var_os("TMPDIR")
        .filter(|dir| !dir.is_empty())
        .map(PathBuf::from);
    let temp = ["/tmp", "/var/tmp", "/dev"].map(PathBuf::from);
    for root in roots.iter().cloned().chain(tmpdir).chain(temp) {
        push(&mut confined.writable_within_read_only, root);
    }
    confined
}

fn push(paths: &mut Vec<PathBuf>, path: PathBuf) {
    if !paths.contains(&path) {
        paths.push(path);
    }
}

#[cfg(test)]
#[path = "agent_sandbox_tests.rs"]
mod tests;
