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
/// - `RUSTC_WRAPPER` — sccache when present on PATH, explicitly empty
///   otherwise (see `sccache_on_path` for why empty ≠ unset).
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
    for (key, value) in args.user_env_vars {
        cmd.env(key, value);
    }
    cmd.env("LUCIDOS_WORKSPACE", args.workspace_path);
    for (key, value) in crate::api::actor::host_protection_env_vars(args.workspace_path) {
        cmd.env(key, value);
    }
    for (key, value) in crate::core::pg_env_vars_cached() {
        cmd.env(key, value);
    }
    for (key, value) in crate::api::actor::subprocess_origin_env_vars(Some(args.thread_id)) {
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
    // a plain (uncached) build. See `sccache_on_path`.
    if sccache_on_path(std::env::var_os("PATH").as_deref()) {
        cmd.env("RUSTC_WRAPPER", "sccache");
    } else {
        cmd.env("RUSTC_WRAPPER", "");
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
/// Coding-agent spawns are the original caller. The dev background engine build
/// (`engine::engine_version::run_engine_build`) uses it for the other half of the
/// same contract: a group is the only handle that reaches the `cargo` GRANDCHILD
/// when a coalescing Apply supersedes the build.
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

/// Signal the agent child's process *group* (negative pid) so the agent AND
/// every descendant it spawned (`Bash` tools, `cargo`/`rustc`, …) are torn
/// down together on a deliberate cancel/shutdown. The child is its own group
/// leader (see `isolate_in_process_group`), so `pgid == child pid`.
///
/// MUST only be called while the child is still unreaped: after `wait()` the
/// pid (and thus the group id) can be recycled, and signalling a recycled
/// group would hit unrelated processes. Best-effort — `ESRCH` (group already
/// gone, or the child was never made a group leader) is ignored.
#[cfg(unix)]
pub(super) fn signal_child_process_group(pid: u32, signal: i32) {
    // SAFETY: `kill(2)` with a negative pid targets the process group and a
    // plain integer signal number; the call has no pointer arguments and is
    // well-defined. The return value is intentionally ignored (best-effort).
    unsafe {
        libc::kill(-(pid as i32), signal);
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
pub(super) async fn graceful_kill_child_process_group(pid: u32, grace: std::time::Duration) {
    signal_child_process_group(pid, libc::SIGTERM);
    tokio::time::sleep(grace).await;
    signal_child_process_group(pid, libc::SIGKILL);
}

#[cfg(not(unix))]
pub(super) async fn graceful_kill_child_process_group(_pid: u32, _grace: std::time::Duration) {}

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

#[cfg(test)]
mod tests {
    use super::*;

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
        // SAFETY: getpgid is async-signal-safe and takes no pointers.
        unsafe { libc::getpgid(pid as i32) }
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
