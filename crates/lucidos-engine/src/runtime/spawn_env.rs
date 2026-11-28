use std::path::Path;
use std::sync::{Arc, Mutex};

pub use lucidos_build_slot::ENV_CLASS as BUILD_SLOT_CLASS_ENV;
use tokio::io::AsyncReadExt;

use super::agent_run_marker::AGENT_RUN_ENV;
use super::agent_runtime::SpawnArgs;
use super::agent_sandbox::SandboxedCommand;
use super::lucidos_cli::path_with_prefixes;

pub(super) fn apply_lucidos_env(
    cmd: &mut tokio::process::Command,
    args: &SpawnArgs<'_>,
    cli_dir: Option<&Path>,
    sandbox: AgentSandbox,
    log_label: &str,
) {
    // INVARIANT: User-managed env vars FIRST so every engine-owned var below overrides on
    // collision (e.g. a user `LUCIDOS_REPO` is replaced by the spawn's repo
    // context). The pairs are already reserved-name-filtered by `env_pairs`.
    crate::core::apply_to_subprocess_env(
        cmd,
        args.user_env_vars,
        crate::core::InheritedSecrets::Keep,
    );
    cmd.env(
        lucidos_build_slot::ENV_CLASS,
        args.build_slot_class.as_str(),
    );
    cmd.env("LUCIDOS_WORKSPACE", args.workspace_path);
    for (key, value) in crate::core::subprocess_origin::host_protection_env_vars() {
        cmd.env(key, value);
    }
    for (key, value) in engine_credential_env(args.thread_id) {
        cmd.env(key, value);
    }
    if let Some(event_id) = args.spawning_event_id {
        cmd.env("LUCIDOS_EVENT_ID", event_id.to_string());
    }
    if let Some(repo_name) = args.repo_name {
        cmd.env("LUCIDOS_REPO", repo_name);
    }
    // INVARIANT: Read by `cc-stop-reminder` to gate the AskUserQuestion redirect.
    // Only sessions that can ask the owner ordinary questions get a redirect.
    // Wire contract: name + value duplicated as `SESSION_KIND_ENV` /
    // `SESSION_KIND_INTERACTIVE` consts in
    // `crates/lucidos-cli/src/cc_stop_reminder.rs`. Keep both in sync.
    if args.interactive {
        cmd.env("LUCIDOS_SESSION_KIND", "interactive");
    } else {
        cmd.env_remove("LUCIDOS_SESSION_KIND");
    }
    cmd.env(AGENT_RUN_ENV, args.agent_run.marker().value());
    // INVARIANT: sccache speeds up the heavy lucidos-engine rebuilds an agent session
    // triggers, but it's a dev-machine optimization (installed by
    // scripts/lib/preflight.sh), not a runtime dependency of the shipped
    // engine binary. Use the wrapper only when sccache is actually on PATH —
    // otherwise a session on a host without it (one-click install, CI, an
    // external repo on a contributor's laptop) would hard-fail every cargo
    // build/test it runs with `process didn't exit successfully: sccache`.
    // The absent branch sets an EMPTY value rather than leaving the var
    // unset: the Lucidos repo's tracked .cargo/config.toml sets
    // `build.rustc-wrapper = "sccache"`, which cargo falls back to when
    // RUSTC_WRAPPER is unset — only an explicit empty value overrides it to
    // a plain (uncached) build. See `sccache_on_path` and `agent_compile_env`.
    for (key, value) in agent_compile_env(sandbox, args.worktree_path) {
        cmd.env(key, value);
    }
    let prefixes = agent_path_prefixes(
        cli_dir,
        args.sandbox.ps_shim_dir(args.workspace_path, cli_dir),
        std::env::var_os("LUCIDOS_PG_BIN_DIR").map(std::path::PathBuf::from),
    );
    if !prefixes.is_empty() {
        match path_with_prefixes(&prefixes) {
            Ok(p) => {
                cmd.env("PATH", p);
            }
            Err(e) => {
                crate::log_legacy!("[{}] failed to join PATH for agent child: {}", log_label, e);
            }
        }
    }
}

fn engine_credential_env(thread_id: uuid::Uuid) -> Vec<(String, String)> {
    let mut env: Vec<(String, String)> = crate::core::lucidos_database_url_env()
        .into_iter()
        .collect();
    // INVARIANT: A coding-agent session emits from a spawn tree no task-local reaches.
    // So its chain depth comes from the thread's registration rather than from
    // the ambient scope. That survives a restart too: a session the recovery
    // path respawns re-mints at the depth its re-queued entry registered.
    let chain_depth =
        crate::scheduler::user_tasks::chain_depth_for_thread(thread_id).unwrap_or_default();
    env.extend(
        crate::core::subprocess_origin::subprocess_origin_env_vars(
            Some(thread_id),
            chain_depth,
            None,
        )
        .into_iter()
        .map(|(key, value)| (key.to_string(), value)),
    );
    env
}

/// INVARIANT: The secret values a coding agent's session env holds, for masking its
/// stored tool results: the user's own secret-shaped variables, the engine's
/// credentials, and what the agent inherits from the engine.
pub fn session_secret_values(
    thread_id: uuid::Uuid,
    user_env_vars: &[(String, String)],
) -> Vec<String> {
    let mut env = user_env_vars.to_vec();
    env.extend(engine_credential_env(thread_id));
    crate::core::subprocess_secret_values(&env, crate::core::inherited_secret_env())
}

/// INVARIANT: The dirs to prepend to an agent child's PATH: the `lucidos` CLI dir first
/// (so `lucidos …` resolves — its position is load-bearing for the workspace
/// symlink contract), then the `ps` shim dir of a macOS sandbox, then the
/// bundled Postgres bin dir when the packaged env
/// provides one that exists, so `psql "$LUCIDOS_DATABASE_URL"` resolves. It
/// mirrors `workspace_script_env_vars` for chat bash/python tools. In dev
/// `LUCIDOS_PG_BIN_DIR` is unset ⇒ no PG entry, unchanged behavior. Split from
/// `apply_lucidos_env` so the decision is unit-testable without process env.
pub(crate) fn agent_path_prefixes(
    cli_dir: Option<&Path>,
    ps_shim_dir: Option<std::path::PathBuf>,
    pg_bin_dir: Option<std::path::PathBuf>,
) -> Vec<std::path::PathBuf> {
    let mut prefixes: Vec<std::path::PathBuf> = Vec::new();
    if let Some(cli_dir) = cli_dir {
        prefixes.push(cli_dir.to_path_buf());
    }
    prefixes.extend(ps_shim_dir);
    if let Some(pg_bin) = pg_bin_dir {
        if pg_bin.is_dir() {
            prefixes.push(pg_bin);
        }
    }
    prefixes
}

/// INVARIANT: Resolve a user-configured agent binary path (`SpawnArgs::binary_override`)
/// to a spawnable path, FAILING with an error that names the preference when
/// the path doesn't resolve to an executable file. A configured-but-wrong path
/// must surface to the user — silently falling back to probing would mask the
/// typo and spawn a different binary than the one they asked for.
pub(super) fn resolve_binary_override(
    path: &str,
    agent_label: &str,
    pref_key: &str,
) -> Result<std::path::PathBuf, Box<dyn std::error::Error + Send + Sync>> {
    let p = std::path::PathBuf::from(path);
    if !p.is_file() {
        return Err(format!(
            "configured {agent_label} binary '{path}' does not exist or is not a file — \
             fix or clear the '{pref_key}' setting (Settings → Coding Agents)"
        )
        .into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let executable = std::fs::metadata(&p)
            .map(|m| m.permissions().mode() & 0o111 != 0)
            .unwrap_or(false);
        if !executable {
            return Err(format!(
                "configured {agent_label} binary '{path}' is not executable — \
                 fix or clear the '{pref_key}' setting (Settings → Coding Agents)"
            )
            .into());
        }
    }
    Ok(p)
}

#[cfg(unix)]
pub fn isolate_in_process_group(cmd: &mut tokio::process::Command) {
    cmd.process_group(0);
}

#[cfg(not(unix))]
pub fn isolate_in_process_group(_cmd: &mut tokio::process::Command) {}

#[cfg(unix)]
const BELOW_ENGINE_NICE: i32 = 5;

#[cfg(unix)]
const MAX_NICE: i32 = 19;

pub fn spawn_below_engine(cmd: &mut SandboxedCommand) -> std::io::Result<tokio::process::Child> {
    isolate_in_process_group(cmd);
    let child = cmd.spawn()?;
    #[cfg(unix)]
    if let Some(pid) = child.id() {
        if let Err(e) = lower_group_below_engine(pid) {
            crate::log_legacy!("[SpawnEnv] Could not lower priority of process group {pid}: {e}");
        }
    }
    Ok(child)
}

#[cfg(unix)]
fn lower_group_below_engine(pgid: u32) -> std::io::Result<()> {
    // SAFETY: getpriority takes no pointers. PRIO_PROCESS with who = 0 names
    // this process, which always exists, so a -1 is a real nice value.
    let engine_nice = unsafe { libc::getpriority(libc::PRIO_PROCESS, 0) };
    let target = (engine_nice + BELOW_ENGINE_NICE).min(MAX_NICE);
    // SAFETY: setpriority takes no pointers and only adjusts scheduling state.
    if unsafe { libc::setpriority(libc::PRIO_PGRP, pgid, target) } == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(unix)]
fn process_group_target(pid: u32) -> Option<i32> {
    match i32::try_from(pid) {
        Ok(0) => None,
        Ok(p) => Some(-p),
        Err(_) => None,
    }
}

/// INVARIANT: Signal the agent child's process *group* (negative pid) so the agent AND
/// every descendant it spawned (`Bash` tools, `cargo`/`rustc`, …) are torn
/// down together on a deliberate cancel/shutdown. The child is its own group
/// leader (see `isolate_in_process_group`), so `pgid == child pid`.
///
/// MUST only be called while the child is still unreaped: after `wait()` the
/// pid (and thus the group id) can be recycled, and signalling a recycled
/// group would hit unrelated processes. Best-effort — `ESRCH` (group already
/// gone, or the child was never made a group leader) is ignored.
///
/// A pid [`process_group_target`] refuses is logged and signals nothing.
#[cfg(unix)]
pub(super) fn signal_child_process_group(pid: u32, signal: i32) {
    let Some(target) = process_group_target(pid) else {
        crate::log_legacy!("[SpawnEnv] Refusing to signal process group for pid {pid}");
        return;
    };
    // SAFETY: `kill(2)` with a negative pid targets the process group and a
    // plain integer signal number; the call has no pointer arguments and is
    // well-defined. The return value is intentionally ignored (best-effort).
    unsafe {
        libc::kill(target, signal);
    }
}

#[cfg(not(unix))]
pub(super) fn signal_child_process_group(_pid: u32, _signal: i32) {}

/// INVARIANT: Gracefully tear down an agent child's whole process group, then force what
/// ignores it. Sends the group SIGTERM (CATCHABLE — a descendant such as a
/// Playwright test runner handles it and closes the browsers it tracks; those
/// browsers `setsid`-DETACH into their own group, so a group signal can't reach
/// them directly — only the runner's own teardown reaps them), waits `grace` for
/// that teardown to run, then SIGKILLs the group to force anything that ignored
/// the SIGTERM.
///
/// A bare SIGKILL (the previous behavior) gave the runner no chance to close its
/// detached browsers — they orphaned and piled up (the 2026-06-24 WebKit
/// pile-up that, combined with an over-eager gateway respawn, fed a respawn
/// storm). macOS has no `PR_SET_PDEATHSIG`/cgroup guarantee, so graceful-first +
/// a content-matching reaper backstop (`scripts/lib/webkit_reaper.sh`) is the
/// accepted shape; this makes graceful teardown the PRIMARY mechanism.
///
/// `grace` is a fixed wait, not a liveness poll: the group LEADER (the agent
/// child) is the engine's direct child and becomes an unreaped zombie the instant
/// it exits — until the caller `wait()`s it — so a `kill(-pgid, 0)` poll would
/// never see the group empty and couldn't end early. This runs in the detached
/// `driver_task`, off the user-visible cancel path, so the wait costs no
/// interactive latency.
///
/// Best-effort, and MUST only be called while the child is still unreaped — after
/// `wait()` the pid (hence the group id) can be recycled and signalling a
/// recycled group would hit unrelated processes (see `signal_child_process_group`).
#[cfg(unix)]
pub(crate) async fn graceful_kill_child_process_group(pid: u32, grace: std::time::Duration) {
    graceful_kill_child_process_group_or_sooner(pid, grace, std::future::pending()).await;
}

#[cfg(not(unix))]
pub(crate) async fn graceful_kill_child_process_group(_pid: u32, _grace: std::time::Duration) {}

#[cfg(unix)]
pub(crate) async fn graceful_kill_child_process_group_or_sooner(
    pid: u32,
    grace: std::time::Duration,
    sooner: impl std::future::Future<Output = ()>,
) {
    signal_child_process_group(pid, libc::SIGTERM);
    tokio::select! {
        _ = tokio::time::sleep(grace) => {}
        _ = sooner => {}
    }
    signal_child_process_group(pid, libc::SIGKILL);
}

#[cfg(not(unix))]
pub(crate) async fn graceful_kill_child_process_group_or_sooner(
    _pid: u32,
    _grace: std::time::Duration,
    _sooner: impl std::future::Future<Output = ()>,
) {
}

/// INVARIANT: SIGKILL a child's whole process group, synchronously.
///
/// The `Drop`-safe sibling of [`graceful_kill_child_process_group`]: a drop
/// handler cannot await, so a caller tearing a subtree down from `Drop` (the dev
/// background engine build, cancelled by an explicit Rebuild) gets the force half
/// only. That is the right trade for a build: `cargo` is crash-safe by
/// construction (atomic renames plus fingerprints), so it has no teardown worth
/// granting a SIGTERM grace, and the alternative is leaving it compiling against
/// the shared `target/` with nothing left to reap it. Prefer the graceful
/// variant wherever the caller CAN await.
///
/// Same reaping caveat as [`signal_child_process_group`]: only call it while the
/// child is still unreaped, or a recycled pid makes this signal an unrelated
/// group.
pub fn kill_child_process_group_now(pid: u32) {
    signal_child_process_group(pid, KILL_SIGNAL);
}

pub(crate) fn terminate_child_process_group(pid: u32) {
    signal_child_process_group(pid, TERM_SIGNAL);
}

/// INVARIANT: SIGUSR2 a child's whole process group, and return at once: the engine is
/// going away and asks it to detach. Not SIGHUP, which a supervisor may have
/// ignored for the whole tree, and bash cannot trap a signal ignored at start.
/// Same reaping caveat as [`kill_child_process_group_now`].
pub(crate) fn detach_child_process_group(pid: u32) {
    signal_child_process_group(pid, DETACH_SIGNAL);
}

#[cfg(unix)]
pub(crate) fn process_group_of(pid: u32) -> Option<u32> {
    let pid = i32::try_from(pid).ok().filter(|p| *p > 0)?;
    // SAFETY: getpgid takes no pointers and only reads process state.
    let group = unsafe { libc::getpgid(pid) };
    u32::try_from(group).ok()
}

#[cfg(not(unix))]
pub(crate) fn process_group_of(_pid: u32) -> Option<u32> {
    None
}

#[cfg(unix)]
const KILL_SIGNAL: i32 = libc::SIGKILL;
#[cfg(not(unix))]
const KILL_SIGNAL: i32 = 9;
#[cfg(unix)]
const TERM_SIGNAL: i32 = libc::SIGTERM;
#[cfg(not(unix))]
const TERM_SIGNAL: i32 = 15;
#[cfg(unix)]
const DETACH_SIGNAL: i32 = libc::SIGUSR2;
#[cfg(not(unix))]
const DETACH_SIGNAL: i32 = 12;

pub(crate) const STDERR_TAIL_BYTES: usize = 4096;

/// INVARIANT: How much raw stderr the reader holds before masking. The extra half lets a
/// secret that straddles the 4 KB cut be masked whole, then cut.
const STDERR_BUFFER_BYTES: usize = 2 * STDERR_TAIL_BYTES;

const STDERR_SETTLE: std::time::Duration = std::time::Duration::from_millis(250);

/// INVARIANT: The last [`STDERR_TAIL_BYTES`] of a child's stderr, read while the child
/// runs. An unread pipe blocks the child once the OS buffer (about 64 KB)
/// fills, so every driver reads continuously instead of after teardown.
pub(crate) struct StderrTail {
    bytes: Arc<Mutex<Vec<u8>>>,
    secrets: Vec<String>,
    reader: tokio::task::JoinHandle<()>,
}

impl StderrTail {
    pub(crate) fn spawn(mut stderr: tokio::process::ChildStderr, secrets: Vec<String>) -> Self {
        let bytes = Arc::new(Mutex::new(Vec::new()));
        let sink = bytes.clone();
        let reader = tokio::spawn(async move {
            let mut chunk = [0u8; 4096];
            loop {
                match stderr.read(&mut chunk).await {
                    Ok(0) | Err(_) => break,
                    Ok(n) => push_capped(&mut sink.lock().unwrap(), &chunk[..n]),
                }
            }
        });
        Self {
            bytes,
            secrets,
            reader,
        }
    }

    pub(crate) async fn finish(self) -> String {
        let Self {
            bytes,
            secrets,
            mut reader,
        } = self;
        if tokio::time::timeout(STDERR_SETTLE, &mut reader)
            .await
            .is_err()
        {
            reader.abort();
        }
        let text = tail_text(&bytes.lock().unwrap(), &secrets);
        text
    }
}

pub(crate) fn command_secret_values(cmd: &tokio::process::Command) -> Vec<String> {
    let env: Vec<(String, String)> = cmd
        .as_std()
        .get_envs()
        .filter_map(|(k, v)| Some((k.to_str()?.to_string(), v?.to_str()?.to_string())))
        .collect();
    crate::core::subprocess_secret_values(&env, crate::core::inherited_secret_env())
}

fn push_capped(tail: &mut Vec<u8>, chunk: &[u8]) {
    tail.extend_from_slice(chunk);
    if tail.len() > STDERR_BUFFER_BYTES {
        let excess = tail.len() - STDERR_BUFFER_BYTES;
        tail.drain(..excess);
    }
}

/// INVARIANT: Decode a captured buffer: masked first, then cut to the last
/// [`STDERR_TAIL_BYTES`] on a character boundary, and trimmed.
///
/// Masking comes before the cut so no secret loses its prefix to the cut. The
/// buffer's own front may start inside a character, and those bytes are
/// skipped rather than shown as a replacement mark.
fn tail_text(buffer: &[u8], secrets: &[String]) -> String {
    let start = buffer
        .iter()
        .position(|b| b & 0xC0 != 0x80)
        .unwrap_or(buffer.len());
    let text = String::from_utf8_lossy(&buffer[start..]);
    let masked = crate::core::redact_subprocess_output(&text, secrets);
    let mut cut = masked.len().saturating_sub(STDERR_TAIL_BYTES);
    while !masked.is_char_boundary(cut) {
        cut += 1;
    }
    masked[cut..].trim().to_string()
}

pub(crate) fn find_on_path(
    file_name: &std::ffi::OsStr,
    path_var: Option<&std::ffi::OsStr>,
) -> Option<std::path::PathBuf> {
    let path_var = path_var?;
    std::env::split_paths(path_var)
        .map(|dir| dir.join(file_name))
        .find(|candidate| candidate.is_file())
}

pub(super) fn sccache_on_path(path_var: Option<&std::ffi::OsStr>) -> bool {
    let exe = format!("sccache{}", std::env::consts::EXE_SUFFIX);
    find_on_path(std::ffi::OsStr::new(&exe), path_var).is_some()
}

const AGENT_SCCACHE_CACHE_SIZE: &str = "50G";

struct SccacheDaemon {
    port: &'static str,
    dir_name: &'static str,
}

const UNSANDBOXED_AGENTS_SCCACHE: SccacheDaemon = SccacheDaemon {
    port: "4227",
    dir_name: "sccache-agents",
};

/// INVARIANT: The daemon of agents in the shared sandbox. It starts inside that sandbox,
/// so it runs every client's `rustc` under the policy the client has.
const SANDBOXED_AGENTS_SCCACHE: SccacheDaemon = SccacheDaemon {
    port: "4228",
    dir_name: "sccache-agents-sandboxed",
};

pub use crate::core::agent_sandbox::AgentSandbox;

pub(crate) fn agent_compile_env_from(
    sandbox: AgentSandbox,
    path_var: Option<&std::ffi::OsStr>,
    cache_root: Option<&Path>,
) -> Vec<(&'static str, String)> {
    let daemon = match sandbox {
        AgentSandbox::WritesConfined => None,
        AgentSandbox::Shared => Some(SANDBOXED_AGENTS_SCCACHE),
        AgentSandbox::Unsandboxed => Some(UNSANDBOXED_AGENTS_SCCACHE),
    };
    let Some(daemon) = daemon.filter(|_| sccache_on_path(path_var)) else {
        return vec![("RUSTC_WRAPPER", String::new())];
    };
    let mut env = vec![("RUSTC_WRAPPER", "sccache".to_string())];
    if let Some(root) = cache_root {
        let dir = root.join(daemon.dir_name);
        env.push(("SCCACHE_SERVER_PORT", daemon.port.to_string()));
        env.push(("SCCACHE_DIR", dir.to_string_lossy().into_owned()));
        env.push(("SCCACHE_CACHE_SIZE", AGENT_SCCACHE_CACHE_SIZE.to_string()));
    }
    env
}

pub(crate) fn agent_compile_env(
    sandbox: AgentSandbox,
    worktree: &Path,
) -> Vec<(&'static str, String)> {
    let cache_root = crate::paths::user_cache_root();
    let mut env = agent_compile_env_from(
        sandbox,
        std::env::var_os("PATH").as_deref(),
        cache_root.as_deref(),
    );
    env.extend(super::compile_mode::env_for_worktree(
        worktree,
        cache_root.as_deref(),
    ));
    env
}

#[cfg(test)]
mod tests {
    use super::super::agent_run_marker::AgentRun;
    use super::*;
    use tokio::io::{AsyncBufReadExt, BufReader};

    #[cfg(unix)]
    #[test]
    fn process_group_target_refuses_pid_zero() {
        assert_eq!(process_group_target(0), None, "pid 0 is our own group");
        assert_eq!(
            process_group_target(u32::MAX),
            None,
            "a pid past i32::MAX cannot be negated into a group id"
        );
        assert_eq!(
            process_group_target(4321),
            Some(-4321),
            "a real child pid still targets its own group"
        );
    }

    #[test]
    fn the_tail_keeps_the_last_bytes_not_the_first() {
        let mut tail = Vec::new();
        push_capped(&mut tail, &vec![b'a'; STDERR_BUFFER_BYTES]);
        push_capped(&mut tail, b"the end");
        assert_eq!(tail.len(), STDERR_BUFFER_BYTES);
        let text = tail_text(&tail, &[]);
        assert_eq!(text.len(), STDERR_TAIL_BYTES);
        assert!(text.ends_with("the end"));
        assert!(text.starts_with('a'));
    }

    #[test]
    fn a_secret_that_straddles_the_cut_is_masked_whole() {
        let secret = "tok-0123456789";
        let mut tail = Vec::new();
        push_capped(&mut tail, "x".repeat(100).as_bytes());
        push_capped(&mut tail, secret.as_bytes());
        push_capped(&mut tail, "y".repeat(STDERR_TAIL_BYTES - 5).as_bytes());
        let text = tail_text(&tail, &[secret.to_string()]);
        assert!(!text.contains("56789"), "no suffix of the secret survives");
        assert!(text.len() <= STDERR_TAIL_BYTES);
    }

    #[test]
    fn a_cap_inside_a_multibyte_character_drops_the_partial_character() {
        let mut tail = Vec::new();
        push_capped(&mut tail, "é".repeat(STDERR_TAIL_BYTES / 2).as_bytes());
        push_capped(&mut tail, b"!");
        let text = tail_text(&tail, &[]);
        assert!(text.ends_with('!'));
        assert!(
            !text.contains('\u{FFFD}'),
            "a cut character is dropped, not shown as a mark"
        );
        assert!(text.starts_with('é'));
    }

    #[test]
    fn a_planted_secret_and_a_database_password_are_masked() {
        let secrets = vec!["tok-0123456789".to_string()];
        let text = tail_text(
            b"auth failed for tok-0123456789 via postgres://user:hunter22@db.example.com/app\n",
            &secrets,
        );
        assert!(!text.contains("tok-0123456789"));
        assert!(!text.contains("hunter22"));
        assert!(text.contains("[REDACTED]"));
    }

    #[test]
    fn an_empty_tail_is_an_empty_string() {
        assert_eq!(tail_text(b"  \n", &[]), "");
    }

    #[test]
    fn command_secret_values_reads_the_env_the_command_was_given() {
        let mut cmd = tokio::process::Command::new("true");
        cmd.env("LUCIDOS_DATABASE_URL", "postgres://u:pw-secret@h/db");
        cmd.env("HARMLESS", "visible-value");
        let secrets = command_secret_values(&cmd);
        assert!(secrets.iter().any(|v| v.contains("pw-secret")));
        assert!(!secrets.iter().any(|v| v == "visible-value"));
    }

    #[test]
    fn session_secret_values_cover_the_agent_credentials_and_user_secrets() {
        crate::test_support::install_agent_database_url();
        crate::core::subprocess_origin::init_agent_origin_secret(
            "spawn-env-test-secret".to_string(),
        );
        let thread_id = uuid::Uuid::new_v4();
        let user_env = vec![
            ("GITHUB_TOKEN".to_string(), "user-row-gh-value".to_string()),
            ("EDITOR".to_string(), "vim-but-long-enough".to_string()),
        ];
        let secrets = session_secret_values(thread_id, &user_env);
        let token =
            crate::core::subprocess_origin::subprocess_origin_env_vars(Some(thread_id), 0, None)
                .into_iter()
                .find(|(key, _)| *key == crate::core::subprocess_origin::ENV_AGENT_ORIGIN_TOKEN)
                .map(|(_, value)| value)
                .expect("an origin token is minted");
        assert!(secrets.contains(&token));
        assert!(secrets.contains(&"user-row-gh-value".to_string()));
        assert!(!secrets.contains(&"vim-but-long-enough".to_string()));
        let (_, handle) = crate::core::lucidos_database_url_env().expect("a handle is installed");
        assert!(secrets.contains(&handle));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_child_writing_200_kb_to_stderr_is_never_blocked() {
        let mut cmd = tokio::process::Command::new("sh");
        cmd.arg("-c")
            .arg("yes x | head -c 200000 >&2; echo last-words >&2; exit 3")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped());
        let mut child = cmd.spawn().unwrap();
        let tail = StderrTail::spawn(child.stderr.take().unwrap(), Vec::new());
        let status = tokio::time::timeout(std::time::Duration::from_secs(10), child.wait())
            .await
            .expect("the child must not block on a full stderr pipe")
            .unwrap();
        assert_eq!(status.code(), Some(3));
        let text = tail.finish().await;
        assert!(text.ends_with("last-words"), "got {text:?}");
        assert!(text.len() <= STDERR_TAIL_BYTES);
    }

    fn spawn_args_for(thread_id: uuid::Uuid, workspace: &Path) -> SpawnArgs<'_> {
        SpawnArgs {
            worktree_path: workspace,
            coding_agent_kind: Default::default(),
            workspace_path: workspace,
            allowed_tools: None,
            system_prompt: None,
            session_context: None,
            resume_session_id: None,
            model: None,
            reasoning_effort: None,
            thread_id,
            spawning_event_id: None,
            repo_name: None,
            interactive: false,
            build_slot_class: lucidos_build_slot::SlotClass::Ordinary,
            user_env_vars: &[],
            account_pin: None,
            binary_override: None,
            permission_mode: None,
            additional_directories: &[],
            auto_compact: None,
            keep_alive: false,
            agent_run: AgentRun::unrecorded(),
            sandbox: crate::runtime::agent_sandbox::Sandbox::unsandboxed(),
        }
    }

    mod guard_tests {
        use super::*;

        #[test]
        fn coding_agent_environment_handles_stay_reachable_in_the_sandbox() {
            crate::test_support::install_agent_database_url();
            let workspace = tempfile::tempdir().unwrap();
            let args = spawn_args_for(uuid::Uuid::new_v4(), workspace.path());
            let mut cmd = tokio::process::Command::new("true");
            apply_lucidos_env(&mut cmd, &args, None, AgentSandbox::Shared, "SpawnEnvTest");
            let env = cmd.as_std().get_envs().filter_map(|(key, value)| {
                value.map(|value| {
                    (
                        key.to_string_lossy().into_owned(),
                        value.to_string_lossy().into_owned(),
                    )
                })
            });
            crate::runtime::agent_sandbox::env_guard_tests::assert_spawn_env_reachable(
                workspace.path(),
                env,
            );
        }
    }

    #[test]
    fn engine_build_class_overrides_workspace_values() {
        let workspace = tempfile::TempDir::new().unwrap();
        let mut args = spawn_args_for(uuid::Uuid::new_v4(), workspace.path());
        let user_env = vec![(lucidos_build_slot::ENV_CLASS.into(), "priority".into())];
        args.user_env_vars = &user_env;
        args.build_slot_class = lucidos_build_slot::SlotClass::Background;
        let mut cmd = tokio::process::Command::new("true");
        apply_lucidos_env(
            &mut cmd,
            &args,
            None,
            AgentSandbox::Unsandboxed,
            "SpawnEnvTest",
        );
        let class = cmd
            .as_std()
            .get_envs()
            .find(|(key, _)| *key == lucidos_build_slot::ENV_CLASS)
            .and_then(|(_, value)| value);
        assert_eq!(class, Some(std::ffi::OsStr::new("background")));
    }

    #[test]
    fn the_shared_env_contract_never_sets_the_auto_compact_window() {
        use crate::engine::auto_compact_trial::{decide, TrialMode, AUTO_COMPACT_WINDOW_ENV};
        let workspace = tempfile::TempDir::new().expect("tempdir");
        let mut args = spawn_args_for(uuid::Uuid::new_v4(), workspace.path());
        args.auto_compact = decide(TrialMode::On, None, uuid::Uuid::new_v4());
        let mut cmd = tokio::process::Command::new("true");
        apply_lucidos_env(
            &mut cmd,
            &args,
            None,
            AgentSandbox::Unsandboxed,
            "SpawnEnvTest",
        );
        assert!(
            !cmd.as_std()
                .get_envs()
                .any(|(key, value)| key == AUTO_COMPACT_WINDOW_ENV && value.is_some()),
            "a Codex spawn must not get the window"
        );
    }

    #[test]
    fn an_agent_spawn_gets_the_read_only_handle_and_no_libpq_defaults() {
        crate::test_support::install_agent_database_url();
        let workspace = tempfile::TempDir::new().expect("tempdir");
        let args = spawn_args_for(uuid::Uuid::new_v4(), workspace.path());
        let mut cmd = tokio::process::Command::new("true");
        apply_lucidos_env(
            &mut cmd,
            &args,
            None,
            AgentSandbox::Unsandboxed,
            "SpawnEnvTest",
        );
        let envs: std::collections::HashMap<String, Option<String>> = cmd
            .as_std()
            .get_envs()
            .map(|(key, value)| {
                (
                    key.to_string_lossy().into_owned(),
                    value.map(|v| v.to_string_lossy().into_owned()),
                )
            })
            .collect();
        for name in [
            "PGUSER",
            "PGPASSWORD",
            "PGHOST",
            "PGPORT",
            "PGDATABASE",
            "DATABASE_URL",
        ] {
            assert!(!envs.contains_key(name), "{name} reached the agent");
        }
        let handle = envs
            .get(crate::core::LUCIDOS_DATABASE_URL)
            .cloned()
            .flatten()
            .expect("the agent gets the database handle");
        assert!(
            handle.ends_with(lucidos_pg_url::READ_ONLY_OPTION),
            "{handle}"
        );
    }

    #[tokio::test]
    async fn every_agent_spawn_carries_the_marker_of_its_run() {
        let workspace = tempfile::TempDir::new().expect("tempdir");
        let args = spawn_args_for(uuid::Uuid::new_v4(), workspace.path());

        let mut cmd = tokio::process::Command::new("true");
        apply_lucidos_env(
            &mut cmd,
            &args,
            None,
            AgentSandbox::Unsandboxed,
            "SpawnEnvTest",
        );
        let stamped = cmd
            .as_std()
            .get_envs()
            .find(|(key, _)| *key == AGENT_RUN_ENV)
            .and_then(|(_, value)| value?.to_str().map(str::to_string));

        assert_eq!(stamped.as_deref(), Some(args.agent_run.marker().value()));
    }

    #[tokio::test]
    async fn a_coding_agent_spawn_carries_no_trigger_claim() {
        use crate::core::subprocess_origin::{
            init_agent_origin_secret, subprocess_origin, SubprocessOrigin, ENV_AGENT_ORIGIN_TOKEN,
            HEADER_AGENT_ORIGIN_TOKEN,
        };
        init_agent_origin_secret("spawn-env-test-secret".to_string());
        let thread_id = uuid::Uuid::new_v4();
        let workspace = tempfile::TempDir::new().expect("tempdir");
        let args = spawn_args_for(thread_id, workspace.path());

        let mut cmd = tokio::process::Command::new("true");
        crate::scheduler::user_tasks::ACTIVE_TRIGGER_ID
            .scope("the-fire-that-started-the-session".to_string(), async {
                apply_lucidos_env(
                    &mut cmd,
                    &args,
                    None,
                    AgentSandbox::Unsandboxed,
                    "SpawnEnvTest",
                );
            })
            .await;

        let token = cmd
            .as_std()
            .get_envs()
            .find(|(key, _)| *key == std::ffi::OsStr::new(ENV_AGENT_ORIGIN_TOKEN))
            .and_then(|(_, value)| value)
            .and_then(|value| value.to_str())
            .expect("every agent spawn carries an origin token")
            .to_string();

        let mut headers = axum::http::HeaderMap::new();
        headers.insert(HEADER_AGENT_ORIGIN_TOKEN, token.parse().expect("header"));
        let SubprocessOrigin::Subprocess {
            source_thread_id,
            emitting_trigger_id,
            ..
        } = subprocess_origin(&headers)
        else {
            panic!("every agent spawn authenticates as a subprocess");
        };
        assert_eq!(source_thread_id, Some(thread_id));
        assert_eq!(
            emitting_trigger_id, None,
            "a trigger must still be woken by the session it started"
        );
    }

    #[test]
    fn agent_path_prefixes_pg_bin_alone_still_contributes() {
        let pg = tempfile::TempDir::new().expect("tempdir");
        assert_eq!(
            agent_path_prefixes(None, None, Some(pg.path().to_path_buf())),
            vec![pg.path().to_path_buf()]
        );
    }

    #[test]
    fn sccache_on_path_true_when_binary_present() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let bin = tmp
            .path()
            .join(format!("sccache{}", std::env::consts::EXE_SUFFIX));
        std::fs::write(&bin, b"#!/bin/sh\nexit 0\n").expect("write fake sccache");
        let path_var = std::env::join_paths([tmp.path()]).expect("join_paths");
        assert!(
            sccache_on_path(Some(path_var.as_os_str())),
            "sccache present on PATH must be detected"
        );
    }

    #[test]
    fn sccache_on_path_false_when_absent() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let path_var = std::env::join_paths([tmp.path()]).expect("join_paths");
        assert!(
            !sccache_on_path(Some(path_var.as_os_str())),
            "missing sccache must not be reported as present"
        );
    }

    fn path_with_fake_sccache(dir: &Path) -> std::ffi::OsString {
        std::fs::write(
            dir.join(format!("sccache{}", std::env::consts::EXE_SUFFIX)),
            b"#!/bin/sh\nexit 0\n",
        )
        .expect("write fake sccache");
        std::env::join_paths([dir]).expect("join_paths")
    }

    #[test]
    fn without_sccache_the_wrapper_is_empty_and_no_daemon_is_named() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let path_var = std::env::join_paths([tmp.path()]).expect("join_paths");
        assert_eq!(
            agent_compile_env_from(
                AgentSandbox::Unsandboxed,
                Some(&path_var),
                Some(Path::new("/tmp/cache"))
            ),
            vec![("RUSTC_WRAPPER", String::new())],
        );
    }

    #[test]
    fn a_write_confined_agent_reaches_no_daemon() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let path_var = path_with_fake_sccache(tmp.path());
        assert_eq!(
            agent_compile_env_from(
                AgentSandbox::WritesConfined,
                Some(&path_var),
                Some(Path::new("/tmp/cache/lucidos"))
            ),
            vec![("RUSTC_WRAPPER", String::new())],
        );
    }

    #[test]
    fn a_sandboxed_agent_compiles_through_the_sandboxed_daemon() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let path_var = path_with_fake_sccache(tmp.path());
        let env = agent_compile_env_from(
            AgentSandbox::Shared,
            Some(&path_var),
            Some(Path::new("/tmp/cache/lucidos")),
        );
        let unsandboxed = agent_compile_env_from(
            AgentSandbox::Unsandboxed,
            Some(&path_var),
            Some(Path::new("/tmp/cache/lucidos")),
        );
        for key in ["SCCACHE_SERVER_PORT", "SCCACHE_DIR"] {
            let value = |env: &Vec<(&'static str, String)>| {
                env.iter().find(|(k, _)| *k == key).map(|(_, v)| v.clone())
            };
            assert!(value(&env).is_some(), "{key}: {env:?}");
            assert_ne!(value(&env), value(&unsandboxed), "{key}");
        }
    }

    #[test]
    fn with_sccache_agents_compile_through_their_own_daemon_and_cache() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let path_var = path_with_fake_sccache(tmp.path());
        assert_eq!(
            agent_compile_env_from(
                AgentSandbox::Unsandboxed,
                Some(&path_var),
                Some(Path::new("/tmp/cache/lucidos"))
            ),
            vec![
                ("RUSTC_WRAPPER", "sccache".to_string()),
                ("SCCACHE_SERVER_PORT", "4227".to_string()),
                (
                    "SCCACHE_DIR",
                    "/tmp/cache/lucidos/sccache-agents".to_string()
                ),
                ("SCCACHE_CACHE_SIZE", "50G".to_string()),
            ],
        );
    }

    #[test]
    fn with_no_cache_root_the_wrapper_goes_alone() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let path_var = path_with_fake_sccache(tmp.path());
        assert_eq!(
            agent_compile_env_from(AgentSandbox::Unsandboxed, Some(&path_var), None),
            vec![("RUSTC_WRAPPER", "sccache".to_string())],
        );
    }

    #[test]
    fn sccache_on_path_false_when_path_unset() {
        assert!(!sccache_on_path(None), "absent PATH must yield false");
    }

    #[test]
    fn agent_path_prefixes_cli_dir_only_in_dev() {
        let cli = std::path::Path::new("/opt/lucidos/bin");
        assert_eq!(
            agent_path_prefixes(Some(cli), None, None),
            vec![cli.to_path_buf()]
        );
    }

    #[test]
    fn agent_path_prefixes_appends_existing_pg_bin_after_cli_dir() {
        let cli = std::path::Path::new("/opt/lucidos/bin");
        let pg = tempfile::TempDir::new().expect("tempdir");
        assert_eq!(
            agent_path_prefixes(Some(cli), None, Some(pg.path().to_path_buf())),
            vec![cli.to_path_buf(), pg.path().to_path_buf()]
        );
    }

    #[test]
    fn agent_path_prefixes_puts_the_ps_shim_between_the_cli_and_postgres() {
        let cli = std::path::Path::new("/opt/lucidos/bin");
        let shim = std::path::PathBuf::from("/w/.lucidos/sandbox-bin");
        let pg = tempfile::TempDir::new().expect("tempdir");
        assert_eq!(
            agent_path_prefixes(Some(cli), Some(shim.clone()), Some(pg.path().to_path_buf())),
            vec![cli.to_path_buf(), shim, pg.path().to_path_buf()]
        );
    }

    fn path_of(cmd: &tokio::process::Command) -> Option<std::ffi::OsString> {
        cmd.as_std()
            .get_envs()
            .find(|(key, _)| *key == "PATH")
            .and_then(|(_, value)| value.map(std::ffi::OsString::from))
    }

    #[test]
    fn an_unsandboxed_agent_or_a_linux_one_gets_no_ps_shim_on_its_path() {
        let workspace = tempfile::TempDir::new().expect("tempdir");
        let cli = tempfile::TempDir::new().expect("tempdir");
        std::fs::write(
            cli.path().join(super::super::lucidos_cli::LUCIDOS_BIN_NAME),
            "",
        )
        .expect("fake cli");
        let mut unsandboxed = spawn_args_for(uuid::Uuid::new_v4(), workspace.path());
        let mut inherited = spawn_args_for(uuid::Uuid::new_v4(), workspace.path());
        unsandboxed.sandbox = crate::runtime::agent_sandbox::Sandbox::unsandboxed();
        inherited.sandbox = crate::runtime::agent_sandbox::Sandbox::inherited();

        let mut opted_out = tokio::process::Command::new("true");
        apply_lucidos_env(
            &mut opted_out,
            &unsandboxed,
            Some(cli.path()),
            AgentSandbox::Unsandboxed,
            "SpawnEnvTest",
        );
        let mut active = tokio::process::Command::new("true");
        apply_lucidos_env(
            &mut active,
            &inherited,
            Some(cli.path()),
            AgentSandbox::Shared,
            "SpawnEnvTest",
        );

        let shim = workspace
            .path()
            .join(crate::runtime::agent_sandbox::PS_SHIM_SUBPATH);
        let on_path = |cmd: &tokio::process::Command| {
            path_of(cmd).is_some_and(|path| std::env::split_paths(&path).any(|dir| dir == shim))
        };
        assert!(!on_path(&opted_out));
        assert_eq!(on_path(&active), cfg!(target_os = "macos"));
        assert!(shim.join("ps").is_file() == cfg!(target_os = "macos"));
    }

    #[test]
    fn agent_path_prefixes_ignores_missing_pg_bin_dir() {
        let missing = std::path::PathBuf::from("/nonexistent/postgres/bin");
        assert!(agent_path_prefixes(None, None, Some(missing)).is_empty());
    }

    #[cfg(unix)]
    fn pgid_of(pid: u32) -> i32 {
        process_group_of(pid).map_or(-1, |g| g as i32)
    }

    #[cfg(unix)]
    #[test]
    fn a_pid_that_names_no_process_has_no_group() {
        assert_eq!(process_group_of(0), None, "0 is not a process");
        assert_eq!(process_group_of(u32::MAX), None, "past i32::MAX");
        assert!(process_group_of(std::process::id()).is_some());
    }

    #[cfg(unix)]
    fn spawn_sleeper(
        configure: impl FnOnce(&mut tokio::process::Command),
    ) -> tokio::process::Child {
        let mut cmd = tokio::process::Command::new("sleep");
        cmd.arg("60");
        configure(&mut cmd);
        cmd.kill_on_drop(true).spawn().expect("spawn sleeper")
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn isolated_child_leaves_the_engine_process_group() {
        let mut child = spawn_sleeper(isolate_in_process_group);
        let pid = child.id().expect("child has a pid");
        let child_pgid = pgid_of(pid);
        let own_pgid = pgid_of(std::process::id());

        assert_eq!(
            child_pgid, pid as i32,
            "isolated child must be its own process-group leader"
        );
        assert_ne!(
            child_pgid, own_pgid,
            "isolated child must NOT share the engine/test process group"
        );

        let _ = child.start_kill();
        let _ = child.wait().await;
    }

    fn test_command(program: &str) -> SandboxedCommand {
        crate::runtime::agent_sandbox::Sandbox::unsandboxed()
            .command(program)
            .expect("unsandboxed command")
    }

    #[cfg(unix)]
    fn nice_of(pid: u32) -> i32 {
        // SAFETY: getpriority takes no pointers and only reads process state.
        unsafe { libc::getpriority(libc::PRIO_PROCESS, pid) }
    }

    #[cfg(unix)]
    fn below_engine() -> i32 {
        (nice_of(0) + BELOW_ENGINE_NICE).min(MAX_NICE)
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_child_spawned_below_the_engine_leads_its_group_at_lower_priority() {
        let mut cmd = test_command("sleep");
        cmd.arg("60").kill_on_drop(true);
        let mut child = spawn_below_engine(&mut cmd).expect("spawn");
        let pid = child.id().expect("child has a pid");

        assert_eq!(pgid_of(pid), pid as i32, "the renice names this group");
        assert_eq!(nice_of(pid), below_engine());

        let _ = child.start_kill();
        let _ = child.wait().await;
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_grandchild_runs_below_the_engine_too() {
        use tokio::io::AsyncWriteExt;

        let mut cmd = test_command("sh");
        cmd.args(["-c", "read _; sleep 60 & echo $!; wait"])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .kill_on_drop(true);
        let mut child = spawn_below_engine(&mut cmd).expect("spawn");
        let pgid = child.id().expect("child has a pid");
        let mut stdin = child.stdin.take().expect("piped stdin");
        stdin.write_all(b"\n").await.expect("release the shell");
        let mut line = String::new();
        BufReader::new(child.stdout.take().expect("piped stdout"))
            .read_line(&mut line)
            .await
            .expect("read grandchild pid");
        let grandchild: u32 = line.trim().parse().expect("grandchild pid");
        let grandchild_nice = nice_of(grandchild);
        signal_child_process_group(pgid, KILL_SIGNAL);
        let _ = child.wait().await;

        assert_eq!(grandchild_nice, below_engine());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn lowering_a_group_that_is_gone_is_an_error_not_a_panic() {
        let mut child = tokio::process::Command::new("true")
            .process_group(0)
            .spawn()
            .expect("spawn");
        let pid = child.id().expect("child has a pid");
        let _ = child.wait().await;

        assert!(lower_group_below_engine(pid).is_err());
    }

    #[tokio::test]
    async fn a_missing_binary_still_fails_the_spawn() {
        let mut cmd = test_command("/nonexistent/lucidos-test-binary");
        assert!(spawn_below_engine(&mut cmd).is_err());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn sigterm_to_engine_group_spares_isolated_child() {
        let mut leader = spawn_sleeper(isolate_in_process_group);
        let leader_pid = leader.id().expect("leader pid");
        let engine_group = leader_pid;

        let mut victim = spawn_sleeper(|cmd| {
            cmd.process_group(engine_group as i32);
        });
        let victim_pid = victim.id().expect("victim pid");

        let mut survivor = spawn_sleeper(isolate_in_process_group);

        assert_eq!(
            pgid_of(victim_pid),
            engine_group as i32,
            "victim must share the synthetic engine group"
        );
        assert_ne!(
            pgid_of(survivor.id().expect("survivor pid")),
            engine_group as i32,
            "survivor must be isolated from the engine group"
        );

        signal_child_process_group(engine_group, libc::SIGTERM);

        let victim_died = tokio::time::timeout(std::time::Duration::from_secs(5), victim.wait())
            .await
            .is_ok();
        assert!(
            victim_died,
            "a child sharing the engine process group must be killed by the group SIGTERM"
        );
        assert!(
            survivor.try_wait().expect("try_wait survivor").is_none(),
            "an isolated child must survive a SIGTERM aimed at the engine process group"
        );

        let _ = leader.start_kill();
        let _ = survivor.start_kill();
        let _ = leader.wait().await;
        let _ = survivor.wait().await;
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn graceful_kill_reaps_a_normal_group() {
        let mut child = spawn_sleeper(isolate_in_process_group);
        let pid = child.id().expect("child pid");
        graceful_kill_child_process_group(pid, std::time::Duration::from_millis(200)).await;
        let reaped = tokio::time::timeout(std::time::Duration::from_secs(2), child.wait())
            .await
            .is_ok();
        assert!(
            reaped,
            "graceful_kill must terminate a normal process group"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn graceful_kill_force_kills_a_sigterm_ignoring_group() {
        let mut cmd = tokio::process::Command::new("perl");
        cmd.arg("-e").arg(r#"$SIG{TERM}="IGNORE"; sleep 600"#);
        isolate_in_process_group(&mut cmd);
        cmd.kill_on_drop(true);
        let Ok(mut child) = cmd.spawn() else {
            crate::log_legacy!(
                "[SpawnEnv] SKIP graceful_kill_force_kills_a_sigterm_ignoring_group: perl unavailable"
            );
            return;
        };
        let pid = child.id().expect("child pid");
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
        graceful_kill_child_process_group(pid, std::time::Duration::from_millis(300)).await;
        let reaped = tokio::time::timeout(std::time::Duration::from_secs(2), child.wait())
            .await
            .is_ok();
        assert!(
            reaped,
            "a SIGTERM-ignoring group leader must be SIGKILLed after the grace"
        );
    }
}
