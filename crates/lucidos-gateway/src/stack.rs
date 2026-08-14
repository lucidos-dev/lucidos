//! One workspace stack = the spawned engine process + its health state.
//!
//! Engine bind depends on the build, per ADR 0014's dev runtime topology. See
//! [`spawn_engine`]'s `loopback` argument.
//!
//! Either way engines are spawned **detached**, so a gateway crash or restart
//! does not take them down. A re-adopting gateway reconnects through the
//! pidfile and a health probe. Supervision is health-probe based, so it works
//! identically for spawned and re-adopted engines.

use crate::postgres::PgHandle;
use crate::registry::Workspace;
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::time::{Duration, Instant};

/// Does `path` lie inside a coding-agent worktree — one of the
/// `<workspace>/.lucidos/worktrees/<thread>/` copies the engine creates per
/// coding-agent thread?
///
/// A worktree is a throwaway checkout pinned to one commit, so anything
/// long-lived that resolves into one is frozen at that commit forever. ADR 0021
/// and `docs/plans/2026-07-26-worktree-pinned-stack-guard.md` hold the rest.
/// Keep this in step with `path_is_in_cc_worktree` in `scripts/lib/workspace.sh`.
///
/// A pure path test on purpose: it must stay correct for an ORPHANED worktree
/// whose directory is already gone, which is exactly when it matters most.
/// Matches on the `.lucidos/worktrees` component pair, so a directory merely
/// *named* `worktrees` is not caught.
pub(crate) fn path_is_in_cc_worktree(path: &Path) -> bool {
    let comps: Vec<_> = path
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    comps
        .windows(2)
        .any(|w| w[0] == ".lucidos" && w[1] == "worktrees")
}

// There is deliberately NO `LUCIDOS_ALLOW_WORKTREE_STACK` escape hatch in this
// crate. The opt-out exists only for a session-scoped DIRECT engine. Every path
// in the gateway is by definition the machine-global daemon, so honouring an
// inherited opt-out here would re-open the hole the guards close. See ADR 0021
// § "the opt-out stops at the gateway".

/// Per-workspace health, surfaced on the control API and rendered in the picker.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Health {
    /// Spawned/adopted but not yet answering `/api/v1/health`.
    Booting,
    /// Answering `/api/v1/health`.
    Healthy,
    /// Failed to become healthy after the restart cap — surfaced for manual
    /// retry / view-logs / delete. The gateway stops auto-respawning it.
    Unhealthy,
}

/// Mutable runtime state for one workspace stack. Held behind a `Mutex` in the
/// gateway's stack map; the supervisor loop locks each in turn.
pub struct StackRuntime {
    pub ws: Workspace,
    pub resolved_dir: PathBuf,
    pub pg: PgHandle,
    /// The engine process this gateway spawned. `None` when the engine was
    /// re-adopted (gateway restart) — supervision is health-based, so a missing
    /// `Child` only means "we can't `try_wait` it", not "it's down".
    pub engine: Option<Child>,
    pub health: Health,
    /// Respawn attempts since the stack was last healthy. Caps auto-respawn.
    pub restart_attempts: u32,
    /// Consecutive failed health probes since the stack was last healthy or
    /// respawned. The supervisor requires several before culling an
    /// alive-but-busy engine, so a single slow probe never triggers a respawn.
    /// Reset on a healthy probe and on respawn.
    pub health_misses: u32,
    pub last_spawn: Option<Instant>,
    pub last_error: Option<String>,
    /// Last unread-notification count fetched from this engine (the picker's
    /// per-workspace badge + the aggregate Tauri / gateway-PWA total). `None`
    /// when the engine isn't healthy / hasn't been polled yet — a stopped
    /// workspace contributes no badge. Refreshed by the supervise loop.
    pub last_unread: Option<u64>,
}

/// Serializable status view for the control API / picker.
#[derive(Serialize, Clone, Debug)]
pub struct WorkspaceStatus {
    pub id: String,
    pub name: String,
    pub port: u16,
    pub health: Health,
    /// Whether the gateway auto-starts this workspace on boot (the picker renders
    /// a per-workspace toggle bound to it). Mirrors [`Workspace::autostart`].
    pub autostart: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
    /// Unread-notification count for this workspace's per-row badge. Omitted
    /// (not `0`) when unknown — a stopped/unhealthy/unpolled engine has no
    /// count, so the picker shows no badge rather than a misleading zero.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unread_count: Option<u64>,
}

impl StackRuntime {
    pub fn status(&self) -> WorkspaceStatus {
        WorkspaceStatus {
            id: self.ws.id.clone(),
            name: self.ws.name.clone(),
            port: self.ws.port,
            health: self.health,
            autostart: self.ws.autostart,
            last_error: self.last_error.clone(),
            unread_count: self.last_unread,
        }
    }
}

/// The workspace-specific variables every spawned engine gets, whatever the
/// runtime posture. Split out of [`spawn_engine`] (which adds the conditional
/// bind/TLS handling on top) so the set is inspectable, and pinned by a test.
///
/// Deliberately NOT here: a model-cache directory. The engine resolves ONE
/// shared per-user cache itself and otherwise inherits whatever the packaged
/// app or the headless service already chose (ADR 0061). Pinning it per
/// workspace here would give every workspace its own copy.
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
        // Identity + callback so the engine's /api/v1/restart can ask the
        // gateway to restart this one stack in place (dev Apply path).
        ("LUCIDOS_WORKSPACE_ID", ws.id.clone().into()),
        ("LUCIDOS_GATEWAY_PORT", gateway_port.to_string().into()),
    ]
}

/// Spawn a workspace engine: detached, pointed at its workspace dir and
/// database, told how to call the gateway back for an in-place restart.
/// Inherits the gateway's environment and overrides the workspace-specific
/// vars. Writes `<dir>/.lucidos/engine.pid` for re-adoption and reclaim.
///
/// `loopback` controls the engine's bind, per ADR 0014's dev runtime topology:
///   * **packaged, `true`**: loopback only, so the gateway is the sole
///     network-facing surface. This is the security posture.
///   * **dev, `false`**: all interfaces on `ws.port`, so `https://localhost:
///     <port>/` reaches the workspace app directly as well as through the
///     gateway. Dev-only convenience, not a relaxation of the posture above.
pub fn spawn_engine(
    engine_bin: &Path,
    ws: &Workspace,
    resolved_dir: &Path,
    database_url: &str,
    gateway_port: u16,
    loopback: bool,
) -> std::io::Result<Child> {
    // The engine writes its pidfile in here, and wants a writable CWD.
    std::fs::create_dir_all(resolved_dir.join(".lucidos"))?;

    let mut cmd = Command::new(engine_bin);
    cmd.current_dir(resolved_dir).envs(engine_env_overrides(
        ws,
        resolved_dir,
        database_url,
        gateway_port,
    ));

    // Never hand a spawned engine a frontend pinned to a coding-agent worktree.
    // This inherit is what makes such a pin self-perpetuating: the worktree's
    // `dist/` reaches every engine the gateway spawns, so the stack keeps
    // serving a frozen build across restarts. Dropping the var makes the engine
    // serve nothing rather than something stale, and a visible failure beats an
    // invisible one. `LUCIDOS_STATIC_DIR` is already optional (ADR 0021).
    if let Some(dir) = std::env::var_os("LUCIDOS_STATIC_DIR") {
        if path_is_in_cc_worktree(Path::new(&dir)) {
            crate::log!(
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
        // Packaged: loopback only. The gateway is the sole network-facing
        // surface and terminates TLS, so the engine serves plain HTTP. Strip
        // any inherited TLS config, or it would serve https and the gateway's
        // http proxy would fail. `LUCIDOS_BIND_LOOPBACK=1` is the engine's
        // `behind_gateway` signal, and the engine also defaults to a loopback
        // bind with `LUCIDOS_BIND_ALL` unset, so this stays loopback-only.
        cmd.env("LUCIDOS_BIND_LOOPBACK", "1")
            .env_remove("LUCIDOS_BIND_ALL")
            .env_remove("LUCIDOS_TLS_CERT")
            .env_remove("LUCIDOS_TLS_KEY");
    } else {
        // Dev: the engine is the direct front on its port. KEEP the inherited
        // TLS so `https://localhost:<port>/` reaches it directly, and the
        // gateway proxies over the matching scheme. Clear the loopback flag so
        // a respawn stays network-capable and not flagged as behind-gateway.
        //
        // Bind defaults to all interfaces, and must say so explicitly because
        // the engine now defaults to loopback. Defer to `network.toml` when it
        // exists, so the engine's own resolver derives the configured bind
        // rather than being masked by BIND_ALL.
        cmd.env_remove("LUCIDOS_BIND_LOOPBACK");
        if crate::net_config::network_toml_exists() {
            cmd.env_remove("LUCIDOS_BIND_ALL");
        } else {
            cmd.env("LUCIDOS_BIND_ALL", "1");
        }
    }

    // Detach into its own session so a gateway death doesn't cascade.
    #[cfg(unix)]
    {
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

    let child = cmd.spawn()?;
    // Not fatal, since the engine is already up and serving, but not silent
    // either. Without the pidfile a re-adopting gateway cannot tell this engine
    // is alive, and `reclaim_stale_engine` cannot free its port on the next
    // respawn. Both failures surface far from here, so name the cause here.
    let pidfile = resolved_dir.join(".lucidos/engine.pid");
    if let Err(e) = std::fs::write(&pidfile, child.id().to_string()) {
        crate::log!(
            "[Gateway] could not write {} for '{}': {e} \
             (re-adoption and stale-engine reclaim will not see this engine)",
            pidfile.display(),
            ws.id
        );
    }
    Ok(child)
}

/// Read the engine pidfile written by [`spawn_engine`].
pub fn read_pidfile(resolved_dir: &Path) -> Option<u32> {
    std::fs::read_to_string(resolved_dir.join(".lucidos/engine.pid"))
        .ok()?
        .trim()
        .parse()
        .ok()
}

/// Whether `pid` names a process that is still RUNNING, reaping it first when
/// it is one of our own children that has already exited.
///
/// A bare `kill(pid, 0)` does NOT answer this. It succeeds for a **zombie**, a
/// process that has exited and lingers in the process table until its parent
/// reaps it. The gateway IS the parent of every engine it spawns. After a self
/// re-exec it holds no `Child` handle for them, so nothing reaps them. A zombie
/// engine then reads as alive forever, and `respawn_decision` never culls an
/// alive engine.
///
/// `waitpid(pid, WNOHANG)` answers and repairs in one call. It is scoped to the
/// single pid, so it can never consume another child's exit status:
///   * `> 0`  the pid was our child, it had exited, it is now REAPED, not alive.
///   * `== 0`  our child and still running, alive.
///   * `< 0`  (`ECHILD`) not our child, so fall back to the existence probe.
///
/// A foreign zombie is indistinguishable in that last branch. But the only
/// zombies the gateway can create are its own children, and the first branch
/// clears those.
#[cfg(unix)]
pub fn pid_is_live(pid: u32) -> bool {
    // Pid 0 is never an engine, and passing it on would be actively harmful.
    // `waitpid(0, ...)` means "any child in MY process group". A corrupt
    // pidfile could then reap a DIFFERENT engine and steal the exit status its
    // `Child` handle waits for. It would read as dead and get culled.
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

/// Wait on a process this gateway forked but holds no [`Child`] for, off the
/// caller's thread, so it cannot linger as a zombie.
///
/// **Signalling an engine is not reaping it.** `reload_gateway` re-execs the
/// image in place, so the pid is unchanged. Every engine the previous image
/// spawned is still a child of this process, while every `Child` handle died
/// with the image. The fresh image re-adopts those engines with `engine: None`,
/// and their teardown then runs through [`reclaim_stale_engine`], which only
/// knows a pid. Without this wait such an engine stays `<defunct>` for the
/// gateway's whole lifetime.
///
/// A dedicated thread rather than the caller's, because a graceful drain takes
/// about ten seconds. A plain `std::thread` rather than `spawn_blocking`, so
/// this stays callable outside a tokio runtime.
///
/// Blocking `waitpid` is safe for a pid that is NOT ours: it returns `ECHILD`
/// at once. It is scoped to the single pid, the same discipline
/// [`pid_is_live`] documents, and it inherits the pid-recycling exposure the
/// `kill` in [`reclaim_stale_engine`] already carries. It is the milder half of
/// that pair.
#[cfg(unix)]
pub fn reap_forked_pid(pid: u32) {
    // Same guard as `pid_is_live`: `waitpid(0, ...)` means "any child in MY
    // process group", which could steal the exit status an engine's `Child`
    // handle is waiting for.
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
            // A signal arriving mid-wait must not abandon the reap; anything
            // else (ECHILD) means there is nothing of ours left to collect.
            if std::io::Error::last_os_error().raw_os_error() != Some(libc::EINTR) {
                return;
            }
        }
    });
}

#[cfg(not(unix))]
pub fn reap_forked_pid(_pid: u32) {}

/// Send SIGUSR1 to a stale engine recorded in the pidfile, so a respawn does
/// not collide on the loopback port. Reap it too, if it is our own child.
/// SIGUSR1 rather than SIGTERM, which the engine ignores.
///
/// The teardown path for an engine the gateway holds no [`Child`] for, and the
/// reap is not optional: see [`reap_forked_pid`] for why such an engine is so
/// often still a child of this process. Best-effort throughout.
pub fn reclaim_stale_engine(resolved_dir: &Path) {
    #[cfg(unix)]
    if let Some(pid) = read_pidfile(resolved_dir) {
        // SAFETY: kill with a valid signal number; an invalid/dead pid just
        // returns ESRCH which we ignore.
        unsafe {
            libc::kill(pid as libc::pid_t, libc::SIGUSR1);
        }
        // Started BEFORE the sleep, so the wait is already in place when the
        // engine exits. The very next spawn overwrites the pidfile, so this is
        // the last moment the pid is known.
        reap_forked_pid(pid);
        // Give it a moment to release the port.
        std::thread::sleep(Duration::from_millis(300));
    }
    #[cfg(not(unix))]
    let _ = resolved_dir;
}

/// Outcome of one health probe. Used to detect `Healthy`, which resets the
/// stack, and to enrich the supervisor's log line.
///
/// The cull decision keys on whether the engine PROCESS is alive, NOT on this
/// outcome. An alive engine is never culled (see `respawn_decision`), and only
/// a process that has exited is respawned.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ProbeOutcome {
    /// 2xx from `/api/v1/health`.
    Healthy,
    /// Connect error that is NOT a timeout (connection refused / reset) — the
    /// engine isn't accepting connections (crashed, or mid-restart). Acted on as
    /// a respawn only when the process is also dead.
    Unreachable,
    /// The probe timed out (connect or read) — the process is (likely) alive
    /// but too busy to answer within the budget. An alive engine is never culled,
    /// so this never respawns on its own.
    Slow,
    /// Any other error, or a non-2xx response. Like `Slow`, never culls an alive
    /// engine.
    Other,
}

/// Probe `GET <scheme>://127.0.0.1:<port>/api/v1/health`.
/// `scheme` is `http` for a loopback (packaged) engine, `https` for a dev engine
/// that serves TLS directly on its port.
///
/// Timeout is classified BEFORE connect: a connect-timeout under load means
/// "alive but the accept loop is starved" (busy), not "refused", so it must read
/// as `Slow`. Only a genuine non-timeout connect error is `Unreachable`.
pub async fn probe_health(client: &reqwest::Client, scheme: &str, port: u16) -> ProbeOutcome {
    let url = format!("{scheme}://127.0.0.1:{port}/api/v1/health");
    match client.get(&url).send().await {
        Ok(r) if r.status().is_success() => ProbeOutcome::Healthy,
        Ok(_) => ProbeOutcome::Other,
        Err(e) if e.is_timeout() => ProbeOutcome::Slow,
        Err(e) if e.is_connect() => ProbeOutcome::Unreachable,
        Err(_) => ProbeOutcome::Other,
    }
}

/// A short-timeout client for health probes (distinct from the proxy client,
/// which has no global timeout so SSE can stream).
///
/// Idle pooling is disabled (`pool_max_idle_per_host(0)`) so every 2s probe
/// opens a **fresh** connection. A health probe must verify the engine accepts
/// *new* connections, not that a stale pooled keepalive connection still works.
/// Accepts invalid certs because a dev engine serves its own self-signed cert on
/// its port, probed via `127.0.0.1` (harmless for the plain-http packaged engine).
///
/// The 5s timeout gives a busy engine headroom to answer before the probe is
/// classified `Slow`. The budget can stay modest because a `Slow` outcome never
/// culls a live engine, so a slow probe against a working engine is harmless.
pub fn build_health_client() -> reqwest::Client {
    reqwest::Client::builder()
        .no_proxy()
        .danger_accept_invalid_certs(true)
        .pool_max_idle_per_host(0)
        .timeout(Duration::from_secs(5))
        .build()
        .expect("failed to build gateway health client")
}

/// Fetch a running engine's unread-notification count via the count-only probe
/// (`?limit=0`) the engine already exposes — reused so the gateway needs no new
/// engine endpoint. Best-effort: any transport / non-2xx / parse failure returns
/// `None`, so a slow or just-restarted engine simply shows no badge that tick.
///
/// This HTTP read is the ONLY count path: the gateway deliberately holds no DB
/// handle (ADR 0014 §1). So a STOPPED workspace contributes nothing to the
/// per-row badge or the aggregate total, which is the settled behaviour.
/// `text()` and a manual parse avoid pulling reqwest's `json` feature.
pub async fn fetch_unread_count(client: &reqwest::Client, scheme: &str, port: u16) -> Option<u64> {
    let url = format!("{scheme}://127.0.0.1:{port}/api/v1/notifications?limit=0");
    let resp = client.get(&url).send().await.ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let body = resp.text().await.ok()?;
    let json: serde_json::Value = serde_json::from_str(&body).ok()?;
    json.get("unread_count")?.as_u64()
}

/// Header the engine reads the originating device off. Mirrors
/// `api::actor::HEADER_DEVICE_ID` in `lucidos-engine`, which the gateway cannot
/// depend on. Rename one and the other must follow in lockstep, the same rule
/// as the CLI's copy of the token header.
pub const HEADER_DEVICE_ID: &str = "x-lucidos-device-id";

/// How long the restart-intent notify may take before it is abandoned. Short on
/// purpose: this sits in front of a user-visible Restart click, and the restart
/// proceeds regardless. Well clear of a loopback round-trip to a healthy
/// engine, which is sub-millisecond.
///
/// The whole budget is only ever spent on an engine that is NOT answering, and
/// the caller holds that stack's lock while it waits. A picker poll can
/// therefore stall behind it. Accepted rather than optimised away, because
/// `respawn_stack` runs on the very next line and holds the same lock for
/// longer. Skipping the notify for an `Unhealthy` stack is worse: a merely BUSY
/// engine misses health probes without being dead, and its in-flight threads
/// are exactly what the attribution is for.
const RESTART_INTENT_TIMEOUT: Duration = Duration::from_secs(2);

/// What a restart-intent notify did. Returned rather than logged and dropped,
/// so the call sites and the tests can tell the three outcomes apart. **No
/// caller may treat any of them as a reason not to restart.**
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RestartIntentNotify {
    /// No device asked, so there is nothing to attribute and no call was made.
    /// This is the supervisor's health respawn, `stop.sh`'s curl, the dev
    /// launcher: every teardown that is genuinely not a user action.
    Skipped,
    /// The engine acknowledged. Its next teardown will be device-attributed.
    Delivered,
    /// The engine could not be reached, was too slow, or refused. The restart
    /// goes ahead and its threads settle the way they did before this existed.
    Failed,
}

/// Tell one workspace's engine that a HUMAN asked for the teardown it is about
/// to be signalled for, and which device they were on. Called immediately
/// before `stop_engine_process` on the control-plane restart path, and nowhere
/// else.
///
/// The engine cannot work this out for itself: `SIGUSR1` carries no sender.
/// Without this call the picker's Restart is indistinguishable from a crash,
/// and its in-flight threads settle at `failed` with no auto-resume. See the
/// engine's `restart_intent` handler.
///
/// **Best effort, and bounded.** A failure here costs attribution on one
/// restart, while blocking the restart would cost the user the thing they
/// clicked. So every failure mode collapses to [`RestartIntentNotify`].
///
/// **`None` means skip, not "unknown device".** The engine refuses a call with
/// no device anyway, so not calling is both cheaper and the same answer.
///
/// Scheme comes from the caller's `GatewayState::engine_scheme()`. The gateway
/// spawned this engine and decided its TLS, so it is not guessing.
pub async fn notify_restart_intent(
    client: &reqwest::Client,
    scheme: &str,
    port: u16,
    device_id: Option<&str>,
) -> RestartIntentNotify {
    let Some(device_id) = device_id.map(str::trim).filter(|d| !d.is_empty()) else {
        return RestartIntentNotify::Skipped;
    };
    let url = format!("{scheme}://127.0.0.1:{port}/api/v1/internal/restart-intent");
    let sent = client
        .post(&url)
        .header(HEADER_DEVICE_ID, device_id)
        .timeout(RESTART_INTENT_TIMEOUT)
        .send()
        .await;
    match sent {
        Ok(r) if r.status().is_success() => RestartIntentNotify::Delivered,
        Ok(r) => {
            crate::log!(
                "[Gateway] restart intent to :{port} returned {}",
                r.status()
            );
            RestartIntentNotify::Failed
        }
        Err(e) => {
            crate::log!("[Gateway] restart intent to :{port} failed: {e}");
            RestartIntentNotify::Failed
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::Workspace;

    /// A child that is still running is live, and probing it must NOT reap or
    /// otherwise disturb it: the very next `try_wait` has to still work.
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

    /// Block until `pid` is an observably exited-but-unreaped child (`ps` state
    /// `Z`), returning false if it never gets there. `ps` state is the portable
    /// read: macOS reserves `WNOWAIT` for `waitid`, there is no /proc here, and
    /// `try_wait` would REAP the child, which is the very state under test.
    ///
    /// A poll rather than a fixed sleep, because these tests share a machine
    /// with the whole suite. A sleep long enough to be reliable under load is a
    /// sleep the whole suite pays on every run.
    #[cfg(unix)]
    fn wait_until_defunct(pid: u32) -> bool {
        for _ in 0..200 {
            let state = std::process::Command::new("ps")
                .args(["-o", "state=", "-p", &pid.to_string()])
                .output()
                .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
                .unwrap_or_default();
            if state.starts_with('Z') {
                return true;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        false
    }

    /// The bug this whole helper exists for: an exited child that nobody has
    /// waited on is a ZOMBIE, and `kill(pid, 0)` still succeeds for it. It must
    /// read as dead, and the probe must reap it so it stops existing at all.
    #[cfg(unix)]
    #[test]
    fn an_unreaped_exited_child_is_a_zombie_and_not_live() {
        // Dropping a `Child` deliberately does NOT wait, so letting the handle
        // fall out of scope leaves exactly the state a re-exec'd gateway is in:
        // our own child, exited, and nobody reaping it.
        let pid = std::process::Command::new("true")
            .spawn()
            .expect("spawn true")
            .id();

        assert!(
            wait_until_defunct(pid),
            "fixture never became an observable zombie"
        );
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
        // And it is gone now, not merely reported dead.
        let mut status: libc::c_int = 0;
        // SAFETY: WNOHANG never blocks; the pid is expected to be reaped already.
        let waited = unsafe { libc::waitpid(pid as libc::pid_t, &mut status, libc::WNOHANG) };
        assert!(waited < 0, "the probe must have reaped the zombie");
    }

    /// Pid 0 must be rejected before it reaches `waitpid`, where it would mean
    /// "any child in my process group" and could reap an unrelated engine.
    #[cfg(unix)]
    #[test]
    fn pid_zero_is_never_live_and_never_reaps() {
        let mut bystander = std::process::Command::new("true")
            .spawn()
            .expect("spawn true");
        let pid = bystander.id();
        // Wait for it to actually exit, so it is exactly the kind of child
        // `waitpid(0, …)` would grab if the guard were missing. Polled, not
        // slept: a busy machine can leave a just-spawned `true` unscheduled past
        // any fixed delay, and the test would then pass for the wrong reason.
        assert!(
            wait_until_defunct(pid),
            "bystander never exited, so the guard would not be under test"
        );

        assert!(!pid_is_live(0), "pid 0 must never read as live");

        // The bystander's exit status is still ours to collect.
        match bystander.try_wait() {
            Ok(Some(_)) => {}
            other => panic!("pid 0 probe consumed another child's status: {other:?}"),
        }
        let _ = bystander.wait();
        let _ = pid;
    }

    /// A pid that is not ours and not running (already reaped by its own parent,
    /// or never existed) is not live either.
    #[cfg(unix)]
    #[test]
    fn a_gone_pid_is_not_live() {
        let mut child = std::process::Command::new("true")
            .spawn()
            .expect("spawn true");
        let pid = child.id();
        let _ = child.wait(); // reaped here, so the pid is fully gone
        assert!(!pid_is_live(pid), "a reaped pid must not read as live");
    }

    /// Regression cover: a live stack running out of an ORPHANED coding-agent
    /// worktree serves a frozen `dist/`, so every frontend-only Apply silently
    /// does nothing.
    #[test]
    fn detects_coding_agent_worktree_paths() {
        for p in [
            "/Users/me/workspaces/dev/.lucidos/worktrees/thread-abc",
            "/Users/me/workspaces/dev/.lucidos/worktrees/thread-abc/crates/lucidos-app/dist",
            "/Users/me/workspaces/dev/.lucidos/worktrees/thread-abc/target/debug/lucidos-engine",
            // The PUBLISHED launch path (ADR 0063), which is what a worktree
            // build actually produces and therefore what this guard has to
            // catch. It moved out of `target/`, and the refusal must not have
            // moved with it.
            "/Users/me/workspaces/dev/.lucidos/worktrees/thread-abc/.launch/debug/plain/lucidos-engine",
        ] {
            assert!(
                path_is_in_cc_worktree(Path::new(p)),
                "should be flagged as a worktree path: {p}"
            );
        }
    }

    /// Must not fire on a real checkout — including paths that merely contain
    /// the word "worktrees", which a naive substring match would catch.
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

    /// The pair must be adjacent — `.lucidos/<x>/worktrees` is not a CC worktree.
    #[test]
    fn requires_adjacent_lucidos_worktrees_components() {
        assert!(!path_is_in_cc_worktree(Path::new(
            "/w/.lucidos/cache/worktrees/thread-abc"
        )));
        assert!(path_is_in_cc_worktree(Path::new("/w/.lucidos/worktrees")));
    }

    /// The predicate must not touch the filesystem: an orphaned worktree's
    /// directory is often already gone, and that is exactly when the guard has
    /// to keep firing.
    #[test]
    fn does_not_require_the_path_to_exist() {
        assert!(path_is_in_cc_worktree(Path::new(
            "/definitely/not/here/.lucidos/worktrees/thread-gone/crates"
        )));
    }

    /// The picker binds its per-workspace auto-start toggle to this field, so the
    /// status JSON must ALWAYS carry `autostart` (no skip).
    #[test]
    fn workspace_status_serializes_autostart() {
        let ws = Workspace {
            id: "dev".into(),
            name: "Dev".into(),
            dir: "/ws/dev".into(),
            port: 5173,
            database_url: None,
            autostart: true,
        };
        let stack = StackRuntime {
            ws,
            resolved_dir: PathBuf::from("/ws/dev"),
            pg: PgHandle::External,
            engine: None,
            health: Health::Healthy,
            restart_attempts: 0,
            health_misses: 0,
            last_spawn: None,
            last_error: None,
            last_unread: None,
        };
        let json = serde_json::to_value(stack.status()).unwrap();
        assert_eq!(json["autostart"], serde_json::json!(true));
        assert_eq!(json["health"], serde_json::json!("healthy"));
        assert_eq!(json["id"], serde_json::json!("dev"));
        // Unknown count → field omitted (no misleading zero badge).
        assert!(json.get("unread_count").is_none());
    }

    // ── Restart intent ───────────────────────────────────────────────────────
    //
    // The notify that turns a picker Restart from something the engine cannot
    // distinguish from a crash into a user action it attributes to a device.
    // Exercised against a mock engine on a real socket, because the thing worth
    // pinning is the wire: the method, the path and the device header.

    /// A one-shot engine that records the raw request it was sent, then 204s.
    async fn capturing_engine() -> (u16, std::sync::Arc<tokio::sync::Mutex<String>>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .unwrap();
        let port = listener.local_addr().unwrap().port();
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
        (port, captured)
    }

    /// A free port nothing is listening on, for the unreachable-engine case.
    async fn dead_port() -> u16 {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        port
    }

    #[tokio::test]
    async fn a_named_device_reaches_the_engines_restart_intent_route() {
        let (port, captured) = capturing_engine().await;
        let outcome =
            notify_restart_intent(&build_health_client(), "http", port, Some("picker-device"))
                .await;

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
    }

    #[tokio::test]
    async fn the_notify_is_aimed_at_the_stack_being_signalled_and_no_other() {
        // Cross-workspace isolation: restarting one workspace must not touch
        // another. The port is the only thing that selects a target, so a second
        // engine on a second port must see nothing at all.
        let (target, target_saw) = capturing_engine().await;
        let (bystander, bystander_saw) = capturing_engine().await;

        notify_restart_intent(&build_health_client(), "http", target, Some("d1")).await;

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
        // This is what keeps every non-user teardown honest: the supervisor's
        // health respawn, `stop.sh`'s curl, the dev launcher. None of them names
        // a device, so none of them can make a crash look like a user restart.
        let (port, captured) = capturing_engine().await;

        for absent in [None, Some(""), Some("   ")] {
            assert_eq!(
                notify_restart_intent(&build_health_client(), "http", port, absent).await,
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
        // The restart proceeds whatever happens here, so the only thing this
        // must do is come back, promptly, with a verdict the caller can log. An
        // engine that is already gone (or wedged) is the common case: the user
        // is restarting it for a reason.
        let port = dead_port().await;
        let started = Instant::now();
        let outcome = notify_restart_intent(&build_health_client(), "http", port, Some("d1")).await;

        assert_eq!(outcome, RestartIntentNotify::Failed);
        assert!(
            started.elapsed() < RESTART_INTENT_TIMEOUT * 2,
            "the notify must give up well inside its own budget, not hold the \
             restart open: took {:?}",
            started.elapsed()
        );
    }

    /// A polled count surfaces as `unread_count` for the picker's per-row badge.
    #[test]
    fn workspace_status_serializes_unread_count_when_known() {
        let ws = Workspace {
            id: "dev".into(),
            name: "Dev".into(),
            dir: "/ws/dev".into(),
            port: 5173,
            database_url: None,
            autostart: false,
        };
        let stack = StackRuntime {
            ws,
            resolved_dir: PathBuf::from("/ws/dev"),
            pg: PgHandle::External,
            engine: None,
            health: Health::Healthy,
            restart_attempts: 0,
            health_misses: 0,
            last_spawn: None,
            last_error: None,
            last_unread: Some(4),
        };
        let json = serde_json::to_value(stack.status()).unwrap();
        assert_eq!(json["unread_count"], serde_json::json!(4));
    }

    /// A spawned engine must INHERIT its model cache location rather than be
    /// given a per-workspace one (ADR 0061). Pinning `FASTEMBED_CACHE_DIR` or
    /// `HF_HOME` here would leave a private copy under every workspace, and
    /// override the shared directory the packaged app already set.
    #[test]
    fn a_spawned_engine_inherits_the_model_cache_instead_of_getting_its_own() {
        let ws = Workspace {
            id: "dev".into(),
            name: "Dev".into(),
            dir: "/ws/dev".into(),
            port: 5173,
            database_url: None,
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
                "LUCIDOS_GATEWAY_PORT",
            ],
            "the workspace-specific set changed: add the new variable here deliberately"
        );
    }
}
