//! One workspace stack = the spawned engine process + its health state.
//!
//! Engine bind depends on the build (ADR 0014 "Dev runtime topology"): **packaged**
//! engines bind **loopback only** so the gateway is the sole network-facing
//! surface (the engine defaults to loopback; `LUCIDOS_BIND_LOOPBACK=1` is also
//! set as its `behind_gateway` signal); **dev** engines bind all interfaces on
//! their own port (`LUCIDOS_BIND_ALL=1`) so the app is reachable directly there
//! too (see `spawn_engine`'s `loopback` arg). Either way engines are spawned
//! **detached** (`setsid`) so a
//! gateway crash/restart doesn't take them down; a re-adopting gateway reconnects
//! via the pidfile + a health probe (engine-statelessness). Supervision is
//! health-probe based, so it works identically for spawned + re-adopted engines.

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
/// A worktree is a throwaway checkout pinned to one commit. Anything long-lived
/// that resolves into one (an engine/gateway binary, a served `dist/`) is frozen
/// at that commit forever, which on 2026-07-26 made every frontend-only Apply
/// silently serve a stale build. The bash side has the same predicate
/// (`path_is_in_cc_worktree` in `scripts/lib/workspace.sh`) — keep the two in
/// step; see `docs/plans/2026-07-26-worktree-pinned-stack-guard.md`.
///
/// A pure path test on purpose: it must stay correct for an ORPHANED worktree
/// whose directory is already gone, which is exactly when it matters most.
/// Matches on the `.lucidos/worktrees` component pair so a directory merely
/// *named* `worktrees` (or a `~/worktrees/lucidos` checkout) is not caught.
pub(crate) fn path_is_in_cc_worktree(path: &Path) -> bool {
    let comps: Vec<_> = path
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    comps
        .windows(2)
        .any(|w| w[0] == ".lucidos" && w[1] == "worktrees")
}

// NOTE: there is deliberately NO `LUCIDOS_ALLOW_WORKTREE_STACK` escape hatch in
// this crate. The opt-out exists only for a session-scoped DIRECT engine (the
// e2e harness, which calls `start_engine` and never starts a gateway). Every
// path in the gateway is by definition the machine-global daemon — it outlives
// the shell that launched it and propagates its env into every engine it spawns
// — so honouring an inherited opt-out here would re-open exactly the 2026-07-26
// hole the guards close. See ADR 0021 § "the opt-out stops at the gateway".

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
    /// Consecutive failed health probes since the stack was last healthy (or
    /// last respawned). The supervisor requires several of these before culling
    /// an alive-but-busy engine, so a transient load spike (a single slow probe)
    /// never triggers a respawn — the root cause of the 2026-06-24 respawn
    /// storm. Reset on a healthy probe and on respawn.
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

/// Spawn a workspace engine: detached, pointed at its workspace dir + database,
/// told how to call the gateway back for an in-place restart. Inherits the
/// gateway's environment (so `LUCIDOS_STATIC_DIR` — the engine serves the built
/// `dist/` directly, ADR 0014 §4/§5 — plus `LUCIDOS_MODEL`, `VERTEX_*`, etc. flow
/// through) and overrides the workspace-specific vars. Writes
/// `<dir>/.lucidos/engine.pid` for re-adoption + reclaim.
///
/// `loopback` controls the engine's bind (ADR 0014 "Dev runtime topology"):
///   * **packaged → `true`**: the engine binds loopback only; the gateway is the
///     sole network-facing surface (the security posture).
///   * **dev → `false`**: the engine binds all interfaces on `ws.port` (the
///     user-facing port), so `https://localhost:<port>/` reaches the workspace
///     app DIRECTLY (base `/`) *in addition* to `…/<slug>/` through the gateway.
///     This is dev-only convenience, not a relaxation of the packaged posture.
pub fn spawn_engine(
    engine_bin: &Path,
    ws: &Workspace,
    resolved_dir: &Path,
    database_url: &str,
    gateway_port: u16,
    loopback: bool,
) -> std::io::Result<Child> {
    // fastembed caches its model relative to CWD by default; pin it under the
    // workspace's ephemeral .lucidos/ and give the engine a writable CWD.
    let fastembed_cache = resolved_dir.join(".lucidos/fastembed");
    std::fs::create_dir_all(&fastembed_cache)?;
    std::fs::create_dir_all(resolved_dir.join(".lucidos"))?;

    let mut cmd = Command::new(engine_bin);
    cmd.current_dir(resolved_dir)
        .env("LUCIDOS_WORKSPACE", resolved_dir)
        .env("DATABASE_URL", database_url)
        .env("LUCIDOS_API_PORT", ws.port.to_string())
        // Identity + callback so the engine's /api/v1/restart can ask the
        // gateway to restart this one stack in place (dev Apply path).
        .env("LUCIDOS_WORKSPACE_ID", &ws.id)
        .env("LUCIDOS_GATEWAY_PORT", gateway_port.to_string())
        .env("FASTEMBED_CACHE_DIR", &fastembed_cache);

    // Never hand a spawned engine a frontend pinned to a coding-agent worktree.
    // This inherit is precisely what made the 2026-07-26 pin self-perpetuating:
    // a gateway launched from a worktree passed that worktree's dist/ into every
    // engine it spawned, so the stack kept serving a frozen build across restarts
    // and every frontend-only Apply silently did nothing. Dropping the var makes
    // the engine serve nothing rather than something stale — a visible failure
    // beats an invisible one, and `LUCIDOS_STATIC_DIR` is already optional
    // (headless engines run without it).
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
        // Packaged: loopback only — the gateway is the sole network-facing
        // surface and terminates TLS, so the engine serves plain HTTP on
        // loopback. Strip any inherited TLS config (else it would serve https
        // and the gateway's http proxy would fail). `LUCIDOS_BIND_LOOPBACK=1`
        // is the engine's `behind_gateway` signal; the engine ALSO defaults to a
        // loopback bind when `LUCIDOS_BIND_ALL` is unset (which it is here), so
        // this stays loopback-only.
        cmd.env("LUCIDOS_BIND_LOOPBACK", "1")
            .env_remove("LUCIDOS_BIND_ALL")
            .env_remove("LUCIDOS_TLS_CERT")
            .env_remove("LUCIDOS_TLS_KEY");
    } else {
        // Dev: the engine is the direct front on its port. KEEP the inherited
        // TLS (LUCIDOS_TLS_CERT/KEY) so `https://localhost:<port>/` reaches it
        // directly; the gateway proxies to it over the matching scheme. Clear
        // the loopback flag so a respawn stays network-capable and not flagged
        // as behind-gateway.
        //
        // Bind: default to all interfaces (the engine defaults to loopback now,
        // so this must be explicit) — BUT defer to ~/.lucidos/network.toml when
        // it exists, so the engine's own resolver derives the configured bind
        // (`[engine] inherit` → the gateway bind, else this workspace's own
        // `network_bind` preference) instead of being masked by BIND_ALL.
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
    let _ = std::fs::write(
        resolved_dir.join(".lucidos/engine.pid"),
        child.id().to_string(),
    );
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

/// Send SIGUSR1 (the engine's graceful-stop signal — it ignores SIGTERM) to a
/// stale engine recorded in the pidfile, so a respawn doesn't collide on the
/// loopback port. Best-effort.
pub fn reclaim_stale_engine(resolved_dir: &Path) {
    #[cfg(unix)]
    if let Some(pid) = read_pidfile(resolved_dir) {
        // SAFETY: kill with a valid signal number; an invalid/dead pid just
        // returns ESRCH which we ignore.
        unsafe {
            libc::kill(pid as libc::pid_t, libc::SIGUSR1);
        }
        // Give it a moment to release the port.
        std::thread::sleep(Duration::from_millis(300));
    }
    #[cfg(not(unix))]
    let _ = resolved_dir;
}

/// Outcome of one health probe. Used to detect `Healthy` (which resets the
/// stack) and to enrich the supervisor's log line. The cull decision keys on
/// whether the engine PROCESS is alive, NOT on this outcome — an alive engine is
/// never culled (see `respawn_decision`); only a process that has exited is
/// respawned. The variants remain for the `Healthy` check and observability.
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
/// classified `Slow` (a response that arrives in <5s is healthy). The budget can
/// stay modest because a `Slow` outcome never culls a live engine at all — only a
/// process that has actually EXITED is respawned (see `respawn_decision` in
/// `server.rs`), so a slow probe against a working engine is harmless.
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
/// handle (ADR 0014 §1), so a STOPPED workspace contributes nothing to the
/// per-row badge or the aggregate total — the settled "running workspaces only"
/// behavior. `text()` + manual parse avoids pulling reqwest's `json` feature.
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::Workspace;

    /// Regression cover for the 2026-07-26 incident: the live stack was running
    /// out of an ORPHANED coding-agent worktree, so it served a frozen `dist/`
    /// and every frontend-only Apply silently did nothing.
    #[test]
    fn detects_coding_agent_worktree_paths() {
        for p in [
            "/Users/me/workspaces/dev/.lucidos/worktrees/thread-abc",
            "/Users/me/workspaces/dev/.lucidos/worktrees/thread-abc/crates/lucidos-app/dist",
            "/Users/me/workspaces/dev/.lucidos/worktrees/thread-abc/target/debug/lucidos-engine",
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
}
