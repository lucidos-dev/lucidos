//! Env stamping shared by every coding-agent subprocess.
//!
//! Each `AgentRuntime` implementor builds its own CLI command (flags, stdio,
//! protocol) but the *Lucidos* environment contract — workspace resolution,
//! host-process protection, Postgres credentials, subprocess-origin
//! attribution, spawn metadata — is agent-independent. Centralizing it here
//! means a new runtime (Codex, ForgeCode, …) cannot ship without it, and a
//! change to the contract lands in one place.
//!
//! Agent-specific env (CC's `MCP_TIMEOUT`, `CLAUDE_CODE_EFFORT_LEVEL`,
//! `CLAUDECODE` removal) stays in the agent's own `build_command`.

use std::path::Path;

use tokio::io::{AsyncBufReadExt, BufReader};

use super::agent_runtime::SpawnArgs;
use super::lucidos_cli::path_with_prefixes;

/// Stamp the agent-independent Lucidos env contract onto `cmd`.
///
/// Covers, in order:
/// - User-managed env vars (`args.user_env_vars`) — applied first so every
///   engine-owned var below wins a collision. See `SpawnArgs::user_env_vars`.
/// - `LUCIDOS_WORKSPACE` — workspace resolution for the `lucidos` CLI.
/// - Host-process protection (`LUCIDOS_HOST_PID` + friends) — see
///   `api::actor::host_protection_env_vars`.
/// - `PG*` vars so the agent can run `psql -c '…'` bare without leaking the
///   password into persisted tool-call argv — see `core::pg_env_vars`.
/// - Subprocess-origin attribution (`LUCIDOS_AGENT_ORIGIN_TOKEN` +
///   `LUCIDOS_THREAD_ID`) — see `api::actor::subprocess_origin_env_vars`.
/// - `LUCIDOS_EVENT_ID` / `LUCIDOS_REPO` / `LUCIDOS_SESSION_KIND` spawn
///   metadata consumed by `lucidos spawn-thread` and `cc-stop-reminder`.
/// - The compile env from `agent_compile_env`: `RUSTC_WRAPPER` (sccache when
///   on PATH, explicitly empty otherwise) and the agents' own sccache daemon.
/// - `PATH` prefixed with the `lucidos` CLI dir when one was found, AND the
///   bundled Postgres bin dir (`LUCIDOS_PG_BIN_DIR`) when set — a packaged
///   build's `psql` lives there, not on the service manager's minimal PATH,
///   and the `PG*` vars above advertise that bare `psql -c '…'` works.
///   Mirrors `workspace_script_env_vars` (chat bash/python tools).
pub(super) fn apply_lucidos_env(
    cmd: &mut tokio::process::Command,
    args: &SpawnArgs<'_>,
    cli_dir: Option<&Path>,
    log_label: &str,
) {
    // User-managed env vars FIRST so every engine-owned var below overrides on
    // collision (e.g. a user `LUCIDOS_REPO` is replaced by the spawn's repo
    // context). The pairs are already reserved-name-filtered by `env_pairs`.
    crate::core::apply_to_subprocess_env(cmd, args.user_env_vars);
    cmd.env("LUCIDOS_WORKSPACE", args.workspace_path);
    for (key, value) in crate::api::actor::host_protection_env_vars(args.workspace_path) {
        cmd.env(key, value);
    }
    for (key, value) in crate::core::pg_env_vars_cached() {
        cmd.env(key, value);
    }
    // A coding-agent session emits from a spawn tree no task-local reaches.
    // So its chain depth comes from the thread's registration rather than from
    // the ambient scope. That survives a restart too: a session the recovery
    // path respawns re-mints at the depth its re-queued entry registered.
    let chain_depth =
        crate::scheduler::user_tasks::chain_depth_for_thread(args.thread_id).unwrap_or_default();
    // The trigger is `None` here, always, and stated rather than inherited.
    // This spawn starts a coding-agent session, which is work a fire hands off
    // and not the fire itself. A trigger waiting on the session it started must
    // still be woken by it. The depth reaches it, the trigger does not: see
    // ADR 0138 and ADR 0137.
    for (key, value) in
        crate::api::actor::subprocess_origin_env_vars(Some(args.thread_id), chain_depth, None)
    {
        cmd.env(key, value);
    }
    // Read by `lucidos spawn-thread` to default `--caller-event-id` so
    // cross-workspace POSTs from an agent subprocess carry the originating event.
    if let Some(event_id) = args.spawning_event_id {
        cmd.env("LUCIDOS_EVENT_ID", event_id.to_string());
    }
    // Read by `lucidos spawn-thread` to default `--repo` so an agent sidequest
    // is created in the same repo as its caller.
    if let Some(repo_name) = args.repo_name {
        cmd.env("LUCIDOS_REPO", repo_name);
    }
    // Read by `cc-stop-reminder` to gate the AskUserQuestion redirect.
    // Unattended sessions (conflict-resolution) don't set this — they would
    // hang on the redirect waiting for an answer that's not coming.
    // Wire contract: name + value duplicated as `SESSION_KIND_ENV` /
    // `SESSION_KIND_INTERACTIVE` consts in
    // `crates/lucidos-cli/src/cc_stop_reminder.rs`. Keep both in sync.
    if args.interactive {
        cmd.env("LUCIDOS_SESSION_KIND", "interactive");
    }
    // sccache speeds up the heavy lucidos-engine rebuilds an agent session
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
    for (key, value) in agent_compile_env() {
        cmd.env(key, value);
    }
    let prefixes = agent_path_prefixes(
        cli_dir,
        std::env::var_os("LUCIDOS_PG_BIN_DIR").map(std::path::PathBuf::from),
    );
    if !prefixes.is_empty() {
        match path_with_prefixes(&prefixes) {
            Ok(p) => {
                cmd.env("PATH", p);
            }
            Err(e) => {
                crate::log!("[{}] failed to join PATH for agent child: {}", log_label, e);
            }
        }
    }
}

/// The dirs to prepend to an agent child's PATH: the `lucidos` CLI dir first
/// (so `lucidos …` resolves — its position is load-bearing for the workspace
/// symlink contract), then the bundled Postgres bin dir when the packaged env
/// provides one that exists (so the advertised bare `psql -c '…'` resolves —
/// mirrors `workspace_script_env_vars` for chat bash/python tools). In dev
/// `LUCIDOS_PG_BIN_DIR` is unset ⇒ no PG entry, unchanged behavior. Split from
/// `apply_lucidos_env` so the decision is unit-testable without process env.
fn agent_path_prefixes(
    cli_dir: Option<&Path>,
    pg_bin_dir: Option<std::path::PathBuf>,
) -> Vec<std::path::PathBuf> {
    let mut prefixes: Vec<std::path::PathBuf> = Vec::new();
    if let Some(cli_dir) = cli_dir {
        prefixes.push(cli_dir.to_path_buf());
    }
    if let Some(pg_bin) = pg_bin_dir {
        if pg_bin.is_dir() {
            prefixes.push(pg_bin);
        }
    }
    prefixes
}

/// Resolve a user-configured agent binary path (`SpawnArgs::binary_override`)
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

/// Place a spawned child in its OWN process group (Unix).
///
/// The engine installs a SIGTERM *ignorer* (see `main.rs`) so accidental
/// `kill`s — a CC `Bash`-tool/test that signals the group, an external
/// `kill -TERM -<pgid>`, a terminal/supervisor group signal — can't take the
/// engine down. But an agent child sharing the engine's process group does
/// NOT ignore SIGTERM: `claude`'s Node runtime catches it, runs cleanup, and
/// re-exits `128+15` (`exit=143`), truncating an in-flight streamed response
/// while the engine survives (same pid). `cc_bash_guard.rs` documents the same
/// cascade ("SIGTERM cascades and every concurrent CC dies"). Isolating the
/// child in its own group is the root-cause fix: a signal delivered to the
/// engine's process group can never reach a process in a different group.
///
/// Coding-agent spawns and background tasks reach it through
/// [`spawn_below_engine`]. Three callers use it directly and keep the engine's
/// priority. One is the dev background engine build
/// (`engine::engine_version::run_engine_build`). Its group is the only handle
/// that reaches the `cargo` GRANDCHILD when a coalescing Apply supersedes it
/// (ADR 0304). The others are the Vite preview (`engine::frontend_preview`)
/// and MCP servers (`mcp::client`), whose `npx`/`uvx` launchers fork the real
/// server as a grandchild.
///
/// `process_group(0)` makes the child the leader of a fresh group
/// (`pgid == child pid`). It is applied via `POSIX_SPAWN_SETPGROUP` — no
/// `pre_exec` hook — so the fast `posix_spawn` spawn path is preserved.
/// The engine's deliberate teardown still reaches the whole subtree by
/// signalling the child's group explicitly (see `signal_child_process_group`).
#[cfg(unix)]
pub(crate) fn isolate_in_process_group(cmd: &mut tokio::process::Command) {
    cmd.process_group(0);
}

#[cfg(not(unix))]
pub(crate) fn isolate_in_process_group(_cmd: &mut tokio::process::Command) {}

/// How far below the engine an agent's process tree runs. A build slot inside
/// the tree adds its own +10 on top, so agent builds land at 15 (ADR 0341).
#[cfg(unix)]
const BELOW_ENGINE_NICE: i32 = 5;

/// The weakest priority a process can hold. The kernel clamps past it anyway.
#[cfg(unix)]
const MAX_NICE: i32 = 19;

/// Spawn `cmd` as its own process group, then lower that group below the
/// engine, so the engine keeps answering clients while agents build (ADR 0341).
///
/// Agent sessions, side questions, model probes and background tasks spawn
/// through here. The engine's own Apply rebuild must not: the user waits on it
/// (ADR 0304).
///
/// The renice runs in the parent after spawn, because a `pre_exec` hook would
/// cost the `posix_spawn` path. It names the group, not the pid: Linux nice is
/// per thread, and the group reaches every thread and process present. A
/// failed renice is logged and the child returned, never a failed spawn.
pub(crate) fn spawn_below_engine(
    cmd: &mut tokio::process::Command,
) -> std::io::Result<tokio::process::Child> {
    isolate_in_process_group(cmd);
    let child = cmd.spawn()?;
    #[cfg(unix)]
    if let Some(pid) = child.id() {
        if let Err(e) = lower_group_below_engine(pid) {
            crate::log!("[SpawnEnv] Could not lower priority of process group {pid}: {e}");
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

/// The `kill(2)` first argument for a child's process group, or `None` when
/// there is no group safe to name.
///
/// `kill(0, sig)` signals the CALLER's own process group. A pid of 0 would
/// therefore turn a child teardown into killing this engine and every
/// workspace process it spawned. A caller reaches 0 through `child.id()`
/// returning `None`, where there is no group to signal anyway. Refusing here
/// covers every call site (CLAUDE.md "Never kill broadly", ADR 0025).
#[cfg(unix)]
fn process_group_target(pid: u32) -> Option<i32> {
    match i32::try_from(pid) {
        Ok(0) => None,
        Ok(p) => Some(-p),
        // A pid past `i32::MAX` cannot be negated into a group id.
        Err(_) => None,
    }
}

/// Signal the agent child's process *group* (negative pid) so the agent AND
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
        crate::log!("[SpawnEnv] Refusing to signal process group for pid {pid}");
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

/// Gracefully tear down an agent child's whole process group, then force what
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

/// [`graceful_kill_child_process_group`], with a grace that `sooner` can cut
/// short. The background-task registry needs it: an engine teardown arriving
/// mid-grace has only `REAP_WAIT` to reap, so it cannot sit out the grace.
/// Same reaping caveat: only while the child is unreaped.
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

/// SIGKILL a child's whole process group, synchronously.
///
/// The `Drop`-safe sibling of [`graceful_kill_child_process_group`]: a drop
/// handler cannot await, so a caller tearing a subtree down from `Drop` (the dev
/// background engine build, cancelled by a coalescing Apply) gets the force half
/// only. That is the right trade for a build: `cargo` is crash-safe by
/// construction (atomic renames plus fingerprints), so it has no teardown worth
/// granting a SIGTERM grace, and the alternative is leaving it compiling against
/// the shared `target/` with nothing left to reap it. Prefer the graceful
/// variant wherever the caller CAN await.
///
/// Same reaping caveat as [`signal_child_process_group`]: only call it while the
/// child is still unreaped, or a recycled pid makes this signal an unrelated
/// group.
pub(crate) fn kill_child_process_group_now(pid: u32) {
    signal_child_process_group(pid, KILL_SIGNAL);
}

/// The process group `pid` belongs to, or `None` when the process is gone or
/// the pid cannot be named. A read, never a signal.
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

/// `SIGKILL`, named so the non-Unix build has something to compile against.
#[cfg(unix)]
const KILL_SIGNAL: i32 = libc::SIGKILL;
#[cfg(not(unix))]
const KILL_SIGNAL: i32 = 9;

/// Drain remaining stderr from an agent child (up to 4 KB).
///
/// Per-line timeout — 100 ms of stderr silence means we're done. A
/// wall-clock timeout would block the post-loop cleanup whenever a
/// grandchild (e.g., rustc) inherited stderr and kept the fd open after the
/// agent's death: read_line on an inherited-but-silent fd is Pending
/// forever. Per-line bounding returns the moment the agent's own stderr is
/// drained, even if grandchildren hold the fd.
pub(super) async fn drain_stderr(
    stderr_reader: &mut BufReader<tokio::process::ChildStderr>,
) -> String {
    let mut output = String::with_capacity(4096);
    let mut line = String::new();
    loop {
        match tokio::time::timeout(
            std::time::Duration::from_millis(100),
            stderr_reader.read_line(&mut line),
        )
        .await
        {
            Ok(Ok(0)) | Ok(Err(_)) | Err(_) => break,
            Ok(Ok(_)) => {
                output.push_str(&line);
                line.clear();
                if output.len() > 4096 {
                    break;
                }
            }
        }
    }
    output
}

/// Resolve `file_name` as an executable file on `path_var` — the ONE PATH
/// walk behind [`sccache_on_path`], `resolve_lucidos_binary_in` (claude_code),
/// and `detect_agent_binary` (runtime), emulating `Command::spawn`'s lookup.
/// `file_name` is the FINAL on-disk name — callers append
/// `std::env::consts::EXE_SUFFIX` themselves where it applies (bundled names
/// like `LUCIDOS_BIN_NAME` already carry it).
pub(crate) fn find_on_path(
    file_name: &std::ffi::OsStr,
    path_var: Option<&std::ffi::OsStr>,
) -> Option<std::path::PathBuf> {
    let path_var = path_var?;
    std::env::split_paths(path_var)
        .map(|dir| dir.join(file_name))
        .find(|candidate| candidate.is_file())
}

/// Whether `sccache` is resolvable as an executable on `path_var`.
///
/// `path_var` is injected to keep this pure and unit-testable; production
/// passes `std::env::var_os("PATH")`. The child process inherits a superset
/// of the engine's PATH (see `path_with_prefixes`), so the engine's own PATH is
/// the correct probe for what cargo will resolve at build time.
pub(super) fn sccache_on_path(path_var: Option<&std::ffi::OsStr>) -> bool {
    let exe = format!("sccache{}", std::env::consts::EXE_SUFFIX);
    find_on_path(std::ffi::OsStr::new(&exe), path_var).is_some()
}

/// Port of the agents' own sccache daemon. The default daemon on 4226 is left
/// to the Apply rebuild and to terminals (ADR 0343).
const AGENT_SCCACHE_PORT: &str = "4227";

/// Directory under [`crate::paths::user_cache_root`] holding the agents' cache.
/// Two daemons never share one disk cache: sccache does not lock it.
const AGENT_SCCACHE_DIR_NAME: &str = "sccache-agents";

/// The compile env a coding agent's builds get. The agent spawn and an agent's
/// background task both take it, so a build behaves the same in both.
///
/// With `sccache` on PATH, builds go to the agents' own daemon. That daemon
/// inherits the priority of the agent that starts it, so agent `rustc` runs
/// below the engine (ADR 0343). Without `sccache`, `RUSTC_WRAPPER` is empty,
/// never unset: see the note in `apply_lucidos_env`. Without a cache root, the
/// wrapper goes alone, to the default daemon.
pub(crate) fn agent_compile_env_from(
    path_var: Option<&std::ffi::OsStr>,
    cache_root: Option<&Path>,
) -> Vec<(&'static str, String)> {
    if !sccache_on_path(path_var) {
        return vec![("RUSTC_WRAPPER", String::new())];
    }
    let mut env = vec![("RUSTC_WRAPPER", "sccache".to_string())];
    if let Some(root) = cache_root {
        let dir = root.join(AGENT_SCCACHE_DIR_NAME);
        env.push(("SCCACHE_SERVER_PORT", AGENT_SCCACHE_PORT.to_string()));
        env.push(("SCCACHE_DIR", dir.to_string_lossy().into_owned()));
    }
    env
}

/// [`agent_compile_env_from`] over the engine's own PATH and cache root.
pub(crate) fn agent_compile_env() -> Vec<(&'static str, String)> {
    agent_compile_env_from(
        std::env::var_os("PATH").as_deref(),
        crate::paths::user_cache_root().as_deref(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A pid of 0 must never reach `kill`. `kill(0, sig)` signals the caller's
    /// OWN process group, so one `unwrap_or(0)` upstream turns a child
    /// teardown into killing this engine and every process it spawned. The
    /// predicate is tested rather than the call, because proving the unfixed
    /// behaviour would take the test runner down with it.
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

    // ── The trigger claim stops at a handoff (ADR 0137) ────────────────────
    // A coding-agent spawn runs INSIDE the fire's `ACTIVE_TRIGGER_ID` scope.
    // A mint site reading that scope would hand the session the fire's own
    // trigger. The trigger would then never be woken by the session it
    // started, and the work would stall with no error. The spawn states
    // `None` instead.

    fn spawn_args_for(thread_id: uuid::Uuid, workspace: &Path) -> SpawnArgs<'_> {
        SpawnArgs {
            worktree_path: workspace,
            coding_agent_kind: Default::default(),
            workspace_path: workspace,
            allowed_tools: None,
            system_prompt: None,
            resume_session_id: None,
            model: None,
            reasoning_effort: None,
            thread_id,
            spawning_event_id: None,
            repo_name: None,
            interactive: false,
            user_env_vars: &[],
            account_pin: None,
            binary_override: None,
            permission_mode: None,
            additional_directories: &[],
        }
    }

    /// Guards the exclusion, so widening it fails here rather than in a
    /// workspace whose trigger silently stopped firing.
    #[tokio::test]
    async fn a_coding_agent_spawn_carries_no_trigger_claim() {
        use crate::api::actor::{
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
                apply_lucidos_env(&mut cmd, &args, None, "SpawnEnvTest");
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
        // Packaged with a mis-staged CLI: psql must still resolve — the PG dir
        // is independent of the CLI dir's presence.
        let pg = tempfile::TempDir::new().expect("tempdir");
        assert_eq!(
            agent_path_prefixes(None, Some(pg.path().to_path_buf())),
            vec![pg.path().to_path_buf()]
        );
    }

    #[test]
    fn sccache_on_path_true_when_binary_present() {
        // A `sccache` executable on the probed PATH → wrapper is safe to set.
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
        // PATH dir exists but has no sccache → must NOT claim it's present,
        // otherwise the wrapper is set and cargo hard-fails.
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
            agent_compile_env_from(Some(&path_var), Some(Path::new("/tmp/cache"))),
            vec![("RUSTC_WRAPPER", String::new())],
        );
    }

    /// The daemon an agent starts inherits the agent's lower priority. Only
    /// its own port keeps the Apply rebuild off it (ADR 0343).
    #[test]
    fn with_sccache_agents_compile_through_their_own_daemon_and_cache() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let path_var = path_with_fake_sccache(tmp.path());
        assert_eq!(
            agent_compile_env_from(Some(&path_var), Some(Path::new("/tmp/cache/lucidos"))),
            vec![
                ("RUSTC_WRAPPER", "sccache".to_string()),
                ("SCCACHE_SERVER_PORT", "4227".to_string()),
                (
                    "SCCACHE_DIR",
                    "/tmp/cache/lucidos/sccache-agents".to_string()
                ),
            ],
        );
    }

    #[test]
    fn with_no_cache_root_the_wrapper_goes_alone() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let path_var = path_with_fake_sccache(tmp.path());
        assert_eq!(
            agent_compile_env_from(Some(&path_var), None),
            vec![("RUSTC_WRAPPER", "sccache".to_string())],
        );
    }

    #[test]
    fn sccache_on_path_false_when_path_unset() {
        // No PATH at all → can't resolve anything → false (degrade to plain build).
        assert!(!sccache_on_path(None), "absent PATH must yield false");
    }

    // ── Agent-child PATH prefixes (agent_path_prefixes) ────────────────────
    // Packaged builds: bare `psql` inside a CC/Codex session must resolve to
    // the bundled Postgres (LUCIDOS_PG_BIN_DIR) — the PG* env vars advertise
    // it. Dev (no env var) stays cli-dir-only.

    #[test]
    fn agent_path_prefixes_cli_dir_only_in_dev() {
        // Dev: LUCIDOS_PG_BIN_DIR unset → only the CLI dir, unchanged behavior.
        let cli = std::path::Path::new("/opt/lucidos/bin");
        assert_eq!(
            agent_path_prefixes(Some(cli), None),
            vec![cli.to_path_buf()]
        );
    }

    #[test]
    fn agent_path_prefixes_appends_existing_pg_bin_after_cli_dir() {
        // Packaged: a real LUCIDOS_PG_BIN_DIR joins the PATH prefix AFTER the
        // CLI dir (the CLI dir's first position is load-bearing).
        let cli = std::path::Path::new("/opt/lucidos/bin");
        let pg = tempfile::TempDir::new().expect("tempdir");
        assert_eq!(
            agent_path_prefixes(Some(cli), Some(pg.path().to_path_buf())),
            vec![cli.to_path_buf(), pg.path().to_path_buf()]
        );
    }

    #[test]
    fn agent_path_prefixes_ignores_missing_pg_bin_dir() {
        // A set-but-absent dir (mis-staged runtime) must not poison PATH.
        let missing = std::path::PathBuf::from("/nonexistent/postgres/bin");
        assert!(agent_path_prefixes(None, Some(missing)).is_empty());
    }

    // ── Process-group isolation ────────────────────────────────────────────
    // Regression for the stray-SIGTERM truncation bug: an agent child sharing
    // the engine's process group is killed (exit=143) by a group-wide SIGTERM
    // the engine itself ignores. Isolation puts the child in its own group.

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

    /// `isolate_in_process_group` makes the child a group leader of its own
    /// fresh group (`pgid == pid`), distinct from the engine/test group.
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

    // ── Running below the engine (ADR 0341) ────────────────────────────────

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
        let mut cmd = tokio::process::Command::new("sleep");
        cmd.arg("60").kill_on_drop(true);
        let mut child = spawn_below_engine(&mut cmd).expect("spawn");
        let pid = child.id().expect("child has a pid");

        assert_eq!(pgid_of(pid), pid as i32, "the renice names this group");
        assert_eq!(nice_of(pid), below_engine());

        let _ = child.start_kill();
        let _ = child.wait().await;
    }

    /// The renice must reach what the agent spawns, not just the agent: a
    /// `make lint` under a session is the load this exists for.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_grandchild_runs_below_the_engine_too() {
        let mut cmd = tokio::process::Command::new("sh");
        cmd.args(["-c", "sleep 60 & echo $!; wait"])
            .stdout(std::process::Stdio::piped())
            .kill_on_drop(true);
        let mut child = spawn_below_engine(&mut cmd).expect("spawn");
        let pgid = child.id().expect("child has a pid");
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
        let mut cmd = tokio::process::Command::new("/nonexistent/lucidos-test-binary");
        assert!(spawn_below_engine(&mut cmd).is_err());
    }

    /// The end-to-end property: a SIGTERM delivered to the engine's process
    /// group kills a child that shares it but NOT one isolated by the fix.
    ///
    /// Built without touching the test runner's own group (a process-group
    /// signal from `cargo test` would hit every concurrently-running test): a
    /// synthetic `leader` creates a throwaway group standing in for "the
    /// engine's group", a `victim` joins it (pre-fix behavior), and a
    /// `survivor` is isolated by `isolate_in_process_group` (post-fix). The
    /// SIGTERM is aimed only at the synthetic group.
    #[cfg(unix)]
    #[tokio::test]
    async fn sigterm_to_engine_group_spares_isolated_child() {
        // Synthetic stand-in for the engine: its own fresh process group.
        let mut leader = spawn_sleeper(isolate_in_process_group);
        let leader_pid = leader.id().expect("leader pid");
        let engine_group = leader_pid; // pgid == leader pid (it is the leader)

        // Pre-fix: a child that SHARES the engine's process group.
        let mut victim = spawn_sleeper(|cmd| {
            cmd.process_group(engine_group as i32);
        });
        let victim_pid = victim.id().expect("victim pid");

        // Post-fix: a child isolated into its own group.
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

        // SIGTERM the engine group only — never the test runner's group.
        signal_child_process_group(engine_group, libc::SIGTERM);

        // The victim (shares the group) must die; the survivor must live.
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

    // ── Graceful group teardown (graceful_kill_child_process_group) ─────────
    // Best-practice fix for orphaned Playwright browsers: SIGTERM the group
    // (lets a runner close its detached browsers), grace, then SIGKILL. Both
    // tests use their OWN isolated group so the signals never reach the test
    // runner's group.

    /// Terminates a normal (SIGTERM-respecting) group.
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

    /// The SIGKILL fallback is load-bearing: a group leader that IGNORES SIGTERM
    /// must still be reaped after the grace. `perl` ignores TERM and sleeps;
    /// skipped if perl isn't on the host (the property is platform-independent).
    #[cfg(unix)]
    #[tokio::test]
    async fn graceful_kill_force_kills_a_sigterm_ignoring_group() {
        let mut cmd = tokio::process::Command::new("perl");
        cmd.arg("-e").arg(r#"$SIG{TERM}="IGNORE"; sleep 600"#);
        isolate_in_process_group(&mut cmd);
        cmd.kill_on_drop(true);
        let Ok(mut child) = cmd.spawn() else {
            crate::log!(
                "[SpawnEnv] SKIP graceful_kill_force_kills_a_sigterm_ignoring_group: perl unavailable"
            );
            return;
        };
        let pid = child.id().expect("child pid");
        // Let perl install its SIGTERM-ignore handler before we signal.
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
