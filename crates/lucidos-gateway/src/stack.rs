//! One workspace stack = the spawned engine process + its health state.
//!
//! Engine bind depends on the build (ADR 0014 "Dev runtime topology"): **packaged**
//! engines bind **loopback only** (`LUCIDOS_BIND_LOOPBACK=1`) so the gateway is
//! the sole network-facing surface; **dev** engines bind all interfaces on their
//! own port so the app is reachable directly there too (see `spawn_engine`'s
//! `loopback` arg). Either way engines are spawned **detached** (`setsid`) so a
//! gateway crash/restart doesn't take them down; a re-adopting gateway reconnects
//! via the pidfile + a health probe (engine-statelessness). Supervision is
//! health-probe based, so it works identically for spawned + re-adopted engines.

use crate::postgres::PgHandle;
use crate::registry::Workspace;
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::time::{Duration, Instant};

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
    pub last_spawn: Option<Instant>,
    pub last_error: Option<String>,
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
    if loopback {
        // Packaged: loopback only — the gateway is the sole network-facing
        // surface and terminates TLS, so the engine serves plain HTTP on
        // loopback. Strip any inherited TLS config (else it would serve https
        // and the gateway's http proxy would fail).
        cmd.env("LUCIDOS_BIND_LOOPBACK", "1")
            .env_remove("LUCIDOS_TLS_CERT")
            .env_remove("LUCIDOS_TLS_KEY");
    } else {
        // Dev: the engine is the direct front on its port. Bind all interfaces
        // and KEEP the inherited TLS (LUCIDOS_TLS_CERT/KEY) so
        // `https://localhost:<port>/` reaches it directly; the gateway proxies
        // to it over the matching scheme. Clear the loopback flag so a respawn
        // stays network-bound.
        cmd.env_remove("LUCIDOS_BIND_LOOPBACK");
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

/// Probe `GET <scheme>://127.0.0.1:<port>/api/v1/health`. `true` on a 2xx.
/// `scheme` is `http` for a loopback (packaged) engine, `https` for a dev engine
/// that serves TLS directly on its port.
pub async fn probe_health(client: &reqwest::Client, scheme: &str, port: u16) -> bool {
    let url = format!("{scheme}://127.0.0.1:{port}/api/v1/health");
    matches!(client.get(&url).send().await, Ok(r) if r.status().is_success())
}

/// A short-timeout client for health probes (distinct from the proxy client,
/// which has no global timeout so SSE can stream).
///
/// Idle pooling is disabled (`pool_max_idle_per_host(0)`) so every 2s probe
/// opens a **fresh** connection. A health probe must verify the engine accepts
/// *new* connections, not that a stale pooled keepalive connection still works.
/// Accepts invalid certs because a dev engine serves its own self-signed cert on
/// its port, probed via `127.0.0.1` (harmless for the plain-http packaged engine).
pub fn build_health_client() -> reqwest::Client {
    reqwest::Client::builder()
        .no_proxy()
        .danger_accept_invalid_certs(true)
        .pool_max_idle_per_host(0)
        .timeout(Duration::from_secs(3))
        .build()
        .expect("failed to build gateway health client")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::Workspace;

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
            last_spawn: None,
            last_error: None,
        };
        let json = serde_json::to_value(stack.status()).unwrap();
        assert_eq!(json["autostart"], serde_json::json!(true));
        assert_eq!(json["health"], serde_json::json!("healthy"));
        assert_eq!(json["id"], serde_json::json!("dev"));
    }
}
