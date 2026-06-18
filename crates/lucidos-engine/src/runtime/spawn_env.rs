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
use super::lucidos_cli::path_with_prefix;

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
/// - `PATH` prefixed with the `lucidos` CLI dir when one was found.
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
    if let Some(cli_dir) = cli_dir {
        match path_with_prefix(cli_dir) {
            Ok(p) => {
                cmd.env("PATH", p);
            }
            Err(e) => {
                crate::log!("[{}] failed to join PATH for lucidos CLI: {}", log_label, e);
            }
        }
    }
}

/// Place the spawned agent child in its OWN process group (Unix).
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
/// `process_group(0)` makes the child the leader of a fresh group
/// (`pgid == child pid`). It is applied via `POSIX_SPAWN_SETPGROUP` — no
/// `pre_exec` hook — so the fast `posix_spawn` spawn path is preserved.
/// The engine's deliberate teardown still reaches the whole subtree by
/// signalling the child's group explicitly (see `signal_child_process_group`).
#[cfg(unix)]
pub(super) fn isolate_in_process_group(cmd: &mut tokio::process::Command) {
    cmd.process_group(0);
}

#[cfg(not(unix))]
pub(super) fn isolate_in_process_group(_cmd: &mut tokio::process::Command) {}

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

/// Whether `sccache` is resolvable as an executable on `path_var`.
///
/// `path_var` is injected to keep this pure and unit-testable; production
/// passes `std::env::var_os("PATH")`. The child process inherits a superset
/// of the engine's PATH (see `path_with_prefix`), so the engine's own PATH is
/// the correct probe for what cargo will resolve at build time.
pub(super) fn sccache_on_path(path_var: Option<&std::ffi::OsStr>) -> bool {
    let Some(path_var) = path_var else {
        return false;
    };
    let exe = format!("sccache{}", std::env::consts::EXE_SUFFIX);
    std::env::split_paths(path_var).any(|dir| dir.join(&exe).is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn spawn_sleeper(configure: impl FnOnce(&mut tokio::process::Command)) -> tokio::process::Child {
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
            pgid_of(victim_pid), engine_group as i32,
            "victim must share the synthetic engine group"
        );
        assert_ne!(
            pgid_of(survivor.id().expect("survivor pid")), engine_group as i32,
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
}
