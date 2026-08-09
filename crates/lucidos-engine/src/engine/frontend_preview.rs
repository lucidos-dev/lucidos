//! The frontend preview: a Vite dev server the engine supervises, rooted in a
//! coding-agent worktree, so a TypeScript or CSS change is visible in the real
//! app BEFORE Apply.
//! `docs/plans/2026-08-08-frontend-preview-fast-lane.md`.
//!
//! The style remote (`utils/styleOverrides.ts`) retunes a CSS custom property
//! live, but a `.tsx` change has no such channel: the only way to see one is
//! Apply, which merges the branch into `main`. This closes that gap for the one
//! case that matters during a design conversation, the Lucidos frontend itself.
//!
//! **Why the engine owns the process.** A coding-agent session's whole process
//! group is killed when its turn ends, so a `vite` the agent starts dies with
//! the turn and the next turn starts it over. Supervision IS the feature.
//!
//! **Why a separate port rather than a path under the workspace.** The bundle
//! derives `BASE_PATH` from the stamped `<base href>` and then its workspace
//! slug from that (`utils/basePath.ts`); a nested `/preview/<thread>/` prefix
//! fails the slug shape, so `baseContextIsValid()` bounces the app to the
//! picker. Making a prefix work means changing base-path parsing, the API
//! prefix, the service-worker scope, the gateway's routing and the engine's
//! shell stamping. On its own port the bundle takes the `BASE_PATH === ''`
//! branch, which is the already-supported legacy direct-engine mode, and Vite
//! proxies `/api` back here so the page is same-origin with its own API
//! (`vite.config.ts`, gated on `LUCIDOS_FRONTEND_PREVIEW_API_ORIGIN`).
//!
//! **What this is NOT.** It never writes `LUCIDOS_STATIC_DIR` and never swaps
//! the served-frontend handle. ADR 0021 is about a long-lived stack silently
//! rooted in a worktree, where every Apply lands and the served bundle never
//! moves; a preview on its own port, that the user started and can see, is the
//! opposite of a silent pin. ADR 0055 records the distinction.
//!
//! One slot per workspace: starting a preview for another thread replaces the
//! running one. It stops on an explicit stop, on its worktree disappearing, and
//! on engine shutdown. Deliberately no lifetime timer, since a timer that kills
//! the preview while the user is looking at it is worse than a lingering node
//! process, and in dev an engine restart reaps it often enough.

use super::LucidosEngine;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Sidecar recording the running child, so an engine that was SIGKILLed can
/// find and reap the orphan on its next boot. Same shape and purpose as
/// `engine.last-death.json` (`super::supervisor_respawn_sidecar`).
const SIDECAR_FILENAME: &str = "frontend-preview.json";

/// How far to walk forward from the base port when it is busy. A small window:
/// the point is a predictable URL, not finding a port at any cost.
const PORT_WALK_LIMIT: u16 = 10;

/// How long to wait for Vite to answer on its port before giving up. Measured
/// at ~700 ms cold in a worktree; this is headroom for a first run that has to
/// pre-bundle dependencies.
const READY_TIMEOUT: Duration = Duration::from_secs(60);

/// Poll cadence for the readiness probe.
const READY_POLL: Duration = Duration::from_millis(250);

/// Cadence of the worktree-liveness tick. A preview whose worktree was reclaimed
/// is serving a directory that no longer exists.
const LIVENESS_INTERVAL: Duration = Duration::from_secs(60);

/// Env var the preview passes to Vite so its `server.proxy` block sends `/api`,
/// `/app` and `/data` back to this engine. Absent, the block does not exist and
/// a manual `npm run dev` behaves exactly as before.
pub const PREVIEW_API_ORIGIN_ENV: &str = "LUCIDOS_FRONTEND_PREVIEW_API_ORIGIN";

/// Override for the preview's listen port.
pub const PREVIEW_PORT_ENV: &str = "LUCIDOS_FRONTEND_PREVIEW_PORT";

/// The running child plus everything needed to describe and reap it.
///
/// The child is NOT `kill_on_drop`: Vite spawns esbuild workers, and
/// `kill_on_drop` reaches only the direct child. Teardown signals the whole
/// process group instead (`isolate_in_process_group` at spawn time).
pub struct RunningPreview {
    pub thread_id: uuid::Uuid,
    pub worktree: PathBuf,
    pub port: u16,
    pub started_at: chrono::DateTime<chrono::Utc>,
    child: tokio::process::Child,
}

/// What the API and the CLI report. `running: false` carries no other field.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FrontendPreviewStatus {
    pub running: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<uuid::Uuid>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub started_at: Option<chrono::DateTime<chrono::Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub worktree: Option<String>,
}

impl FrontendPreviewStatus {
    pub fn stopped() -> Self {
        Self {
            running: false,
            thread_id: None,
            port: None,
            started_at: None,
            worktree: None,
        }
    }

    fn of(p: &RunningPreview) -> Self {
        Self {
            running: true,
            thread_id: Some(p.thread_id),
            port: Some(p.port),
            started_at: Some(p.started_at),
            worktree: Some(p.worktree.display().to_string()),
        }
    }
}

/// On-disk sidecar. Every field is needed by the reaper's safety check, so none
/// is optional.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PreviewSidecar {
    pub pid: u32,
    pub port: u16,
    pub thread_id: uuid::Uuid,
    pub worktree: String,
}

// ---------------------------------------------------------------------------
// Pure helpers (unit-tested without a filesystem, a port or a process)
// ---------------------------------------------------------------------------

/// Base port for the preview: the explicit override, else 1000 above the
/// engine's own API port. 1000 clears the workspace port bands entirely
/// (`scripts/lib/ports.sh` spaces API at 3000+offset and Vite at 5173+offset,
/// so an offset would have to exceed 1000 to collide).
pub fn preview_port_base(port_override: Option<&str>, api_port: u16) -> u16 {
    port_override
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .and_then(|s| s.parse::<u16>().ok())
        .filter(|p| *p > 0)
        .unwrap_or_else(|| api_port.saturating_add(1000))
}

/// First free port at or after `base`, within [`PORT_WALK_LIMIT`]. `is_free` is
/// injected so the walk is testable without binding anything.
pub fn select_free_port(base: u16, is_free: impl Fn(u16) -> bool) -> Option<u16> {
    (0..PORT_WALK_LIMIT)
        .filter_map(|i| base.checked_add(i))
        .find(|p| is_free(*p))
}

/// Is `candidate` inside `parent`? Both are compared as-is: the caller derives
/// `candidate` from a `Uuid`, so this is defense in depth against a future
/// caller that derives it from something else.
pub fn path_is_inside(parent: &Path, candidate: &Path) -> bool {
    candidate.starts_with(parent) && candidate != parent
}

/// Why a thread cannot host a frontend preview. Each variant's message names the
/// thing that is missing, per the "never a bare generic" error rule.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PreviewRefusal {
    Packaged,
    OutsideWorktrees(String),
    NoWorktree(String),
    NotTheLucidosFrontend(String),
    NoNodeModules(String),
}

impl std::fmt::Display for PreviewRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Packaged => write!(
                f,
                "The frontend preview is a development affordance and is not available in a packaged install."
            ),
            Self::OutsideWorktrees(p) => write!(
                f,
                "Refusing to preview {p}: it is not inside this workspace's .lucidos/worktrees/."
            ),
            Self::NoWorktree(p) => write!(
                f,
                "No coding-agent worktree at {p}. This thread has none, or it was already reclaimed."
            ),
            Self::NotTheLucidosFrontend(p) => write!(
                f,
                "The worktree at {p} has no crates/lucidos-app/package.json, so it is not a Lucidos-source worktree (an app or external-repo thread has no frontend to preview)."
            ),
            Self::NoNodeModules(p) => write!(
                f,
                "The worktree at {p} has no node_modules/.bin/vite, so its dependencies were never provisioned."
            ),
        }
    }
}

/// Decide, from four filesystem answers the caller looked up, whether this
/// worktree may host a preview. Split from the I/O so every refusal is testable.
pub fn classify_worktree(
    packaged: bool,
    worktrees_dir: &Path,
    worktree: &Path,
    worktree_exists: bool,
    has_frontend_package_json: bool,
    has_vite_binary: bool,
) -> Result<(), PreviewRefusal> {
    let shown = worktree.display().to_string();
    if packaged {
        return Err(PreviewRefusal::Packaged);
    }
    if !path_is_inside(worktrees_dir, worktree) {
        return Err(PreviewRefusal::OutsideWorktrees(shown));
    }
    if !worktree_exists {
        return Err(PreviewRefusal::NoWorktree(shown));
    }
    if !has_frontend_package_json {
        return Err(PreviewRefusal::NotTheLucidosFrontend(shown));
    }
    if !has_vite_binary {
        return Err(PreviewRefusal::NoNodeModules(shown));
    }
    Ok(())
}

/// May the startup reaper kill this recorded pid?
///
/// The recorded pid is stale by construction (the engine that wrote it is gone)
/// and pids are recycled, so a bare `kill` is how a reaper takes out an
/// unrelated process. The command line is the evidence: we spawn Vite by its
/// ABSOLUTE path inside the worktree, so a process that is really ours carries
/// both `vite` and that directory in its argv. Anything else is somebody else's.
///
/// Mirrors the host-kill guard of ADR 0025: never signal ourselves, and never
/// signal on a probe that could not run (`command_line: None` means the process
/// is gone or `ps` failed, and neither authorizes a kill).
pub fn sidecar_pid_is_reapable(
    pid: u32,
    self_pid: u32,
    command_line: Option<&str>,
    worktree: &str,
) -> bool {
    if pid == 0 || pid == self_pid {
        return false;
    }
    let Some(cmd) = command_line else {
        return false;
    };
    cmd.contains("vite") && cmd.contains(worktree)
}

/// What one liveness pass decided about a running slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LivenessAction {
    /// The preview is still real. Leave it alone.
    Keep,
    /// Retire the slot. `kill_group` says whether the child still needs killing:
    /// `false` means it has ALREADY been reaped, so its pid (and with it the
    /// process-group id) may have been recycled and must never be signalled.
    Retire {
        kill_group: bool,
        reason: &'static str,
    },
}

/// Decide a liveness pass from the two facts the caller looked up.
///
/// `child_exited` is `None` for "still running, or the wait itself failed". A
/// failed wait says nothing about the child, and killing on unknown would tear
/// down a preview the user is looking at, so it is treated as alive: the same
/// direction the unknown-git-state rule takes, keeping the thing that might
/// still be wanted.
pub fn liveness_action(worktree_exists: bool, child_exited: Option<bool>) -> LivenessAction {
    // The exit check comes FIRST, and it is the one that decides `kill_group`,
    // because it is the safety-critical fact: learning it reaped the child, so
    // from here on the pid may belong to somebody else. Both conditions can hold
    // at once (a Discard removes the tree and kills the session's processes),
    // and in that overlap the reaped child still wins the kill decision.
    if child_exited == Some(true) {
        return LivenessAction::Retire {
            kill_group: false,
            reason: if worktree_exists {
                "stopped, vite exited"
            } else {
                "stopped, its worktree is gone and vite exited with it"
            },
        };
    }
    if !worktree_exists {
        // Vite is still up, serving a directory that no longer exists, and its
        // pid is still ours, so its group does need killing.
        return LivenessAction::Retire {
            kill_group: true,
            reason: "stopped, its worktree is gone",
        };
    }
    LivenessAction::Keep
}

/// The origin Vite proxies `/api`, `/app` and `/data` to: this engine, on
/// loopback, on whichever scheme it actually serves.
pub fn engine_api_origin(scheme: &str, api_port: u16) -> String {
    format!("{scheme}://127.0.0.1:{api_port}")
}

/// The preview's URL as seen by whoever asked, built by swapping the port of
/// the `Host` they reached us on.
///
/// The engine cannot know this on its own: the same workspace is `localhost`
/// from the laptop and a Tailscale name from the phone, and a `localhost` link
/// handed to a phone is dead. So the answer is a function of the requester's
/// own `Host`, computed per request, and the SSE event carries only the port so
/// the page can do the same from its `location`.
///
/// `None` for a missing or bracket-mismatched `Host`; the caller reports the
/// port instead of guessing a hostname.
pub fn preview_url_for_host(host: Option<&str>, scheme: &str, port: u16) -> Option<String> {
    let host = host.map(str::trim).filter(|h| !h.is_empty())?;
    // An IPv6 literal is bracketed and its address is full of colons, so the
    // port separator is the colon AFTER the closing bracket, not the first one.
    let hostname = if let Some(rest) = host.strip_prefix('[') {
        // `rest` is offset by the `[`, so the closing bracket sits at
        // `close + 1` in `host` and the literal ends one past it.
        let close = rest.find(']')?;
        &host[..close + 2]
    } else {
        host.split(':').next()?
    };
    if hostname.is_empty() {
        return None;
    }
    Some(format!("{scheme}://{hostname}:{port}/"))
}

// ---------------------------------------------------------------------------
// Filesystem + process glue
// ---------------------------------------------------------------------------

fn sidecar_path(workspace: &Path) -> PathBuf {
    workspace.join(".lucidos").join(SIDECAR_FILENAME)
}

fn write_sidecar(workspace: &Path, sidecar: &PreviewSidecar) {
    let path = sidecar_path(workspace);
    let json = match serde_json::to_string(sidecar) {
        Ok(j) => j,
        Err(e) => {
            log!("[FrontendPreview] failed to serialize sidecar: {}", e);
            return;
        }
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Err(e) = std::fs::write(&path, json) {
        log!(
            "[FrontendPreview] failed to write {}: {}",
            path.display(),
            e
        );
    }
}

fn remove_sidecar(workspace: &Path) {
    let path = sidecar_path(workspace);
    match std::fs::remove_file(&path) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => log!(
            "[FrontendPreview] failed to remove {}: {}",
            path.display(),
            e
        ),
    }
}

/// Read + delete the sidecar. Deleted BEFORE parsing so a malformed file is not
/// re-processed on every boot (same reasoning as `supervisor_respawn_sidecar`).
fn take_sidecar(workspace: &Path) -> Option<PreviewSidecar> {
    let path = sidecar_path(workspace);
    let bytes = match std::fs::read(&path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return None,
        Err(e) => {
            log!("[FrontendPreview] failed to read {}: {}", path.display(), e);
            return None;
        }
    };
    let _ = std::fs::remove_file(&path);
    match serde_json::from_slice::<PreviewSidecar>(&bytes) {
        Ok(s) => Some(s),
        Err(e) => {
            log!("[FrontendPreview] malformed sidecar discarded: {}", e);
            None
        }
    }
}

/// A live process's argv as one string, or `None` when it is gone or `ps`
/// failed. The distinction matters: `None` must never be read as "not ours" and
/// must never be read as "ours" either, so [`sidecar_pid_is_reapable`] refuses
/// on it.
fn process_command_line(pid: u32) -> Option<String> {
    let out = std::process::Command::new("ps")
        .args(["-o", "command=", "-p", &pid.to_string()])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let cmd = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if cmd.is_empty() {
        None
    } else {
        Some(cmd)
    }
}

fn port_is_free(port: u16) -> bool {
    std::net::TcpListener::bind(("0.0.0.0", port)).is_ok()
}

/// Poll the preview's own port until it answers. The self-signed dev cert is
/// accepted here for the same reason every other intra-host hop accepts it.
async fn wait_until_ready(scheme: &str, port: u16) -> bool {
    let client = match reqwest::Client::builder()
        .danger_accept_invalid_certs(true)
        .no_proxy()
        .timeout(Duration::from_secs(5))
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            log!("[FrontendPreview] could not build probe client: {}", e);
            return false;
        }
    };
    let url = format!("{scheme}://127.0.0.1:{port}/");
    let deadline = tokio::time::Instant::now() + READY_TIMEOUT;
    while tokio::time::Instant::now() < deadline {
        if let Ok(resp) = client.get(&url).send().await {
            if resp.status().is_success() {
                return true;
            }
        }
        tokio::time::sleep(READY_POLL).await;
    }
    false
}

// ---------------------------------------------------------------------------
// Engine methods
// ---------------------------------------------------------------------------

impl LucidosEngine {
    /// Current preview state.
    pub async fn frontend_preview_status(&self) -> FrontendPreviewStatus {
        match self.frontend_preview.lock().await.as_ref() {
            Some(p) => FrontendPreviewStatus::of(p),
            None => FrontendPreviewStatus::stopped(),
        }
    }

    /// Start (or replace) the frontend preview for `thread_id`.
    ///
    /// Returns the running status, or a message naming exactly what was refused.
    /// Replacing is deliberate: one slot per workspace (see the module docs), so
    /// pointing the preview at a different thread is one call rather than a
    /// stop followed by a start.
    pub async fn start_frontend_preview(
        &self,
        thread_id: uuid::Uuid,
    ) -> Result<FrontendPreviewStatus, String> {
        // Held for the WHOLE start, not just the slot write. The slot mutex is
        // released between stopping the old preview, taking a port, spawning and
        // waiting for readiness, so two concurrent starts (two devices, or the
        // CLI alongside a tap) would both pass the free-port check, both spawn,
        // and the second would overwrite the first's `RunningPreview`. The child
        // is not `kill_on_drop`, so the displaced Vite would be orphaned with
        // nothing tracking its pid: no slot, no sidecar, no reaper.
        let _lifecycle = self.frontend_preview_lifecycle.lock().await;
        let workspace = self.workspace_path.clone();
        let worktrees = workspace.join(".lucidos").join("worktrees");
        let worktree =
            super::agent_session::resume::deterministic_worktree_path(&workspace, thread_id);
        let vite_bin = worktree.join("node_modules").join(".bin").join("vite");

        classify_worktree(
            crate::runtime::is_packaged(),
            &worktrees,
            &worktree,
            worktree.is_dir(),
            worktree
                .join("crates")
                .join("lucidos-app")
                .join("package.json")
                .is_file(),
            vite_bin.exists(),
        )
        .map_err(|r| r.to_string())?;

        // Replace any running preview BEFORE taking a port, so restarting the
        // same thread's preview reuses its port instead of walking past itself.
        // The `_locked` variant, because this call already holds the lifecycle
        // lock the public one takes.
        self.stop_frontend_preview_locked().await;

        let api_port = std::env::var("LUCIDOS_API_PORT")
            .ok()
            .and_then(|s| s.trim().parse::<u16>().ok())
            .unwrap_or(3000);
        let base = preview_port_base(std::env::var(PREVIEW_PORT_ENV).ok().as_deref(), api_port);
        let port = select_free_port(base, port_is_free).ok_or_else(|| {
            format!(
                "No free port for the frontend preview in {}..{}. Set {PREVIEW_PORT_ENV} to choose one.",
                base,
                base.saturating_add(PORT_WALK_LIMIT - 1),
            )
        })?;

        let scheme = crate::net_config::tls_scheme();
        let mut cmd = tokio::process::Command::new(&vite_bin);
        cmd.current_dir(worktree.join("crates").join("lucidos-app"))
            .env("VITE_PORT", port.to_string())
            .env(PREVIEW_API_ORIGIN_ENV, engine_api_origin(scheme, api_port))
            // The build-watch's staging dance is a `vite build` concern and must
            // not follow us into `vite serve`.
            .env_remove("LUCIDOS_ATOMIC_DIST")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped());
        crate::runtime::spawn_env::isolate_in_process_group(&mut cmd);

        let mut child = cmd
            .spawn()
            .map_err(|e| format!("Could not start Vite at {}: {e}", vite_bin.display()))?;
        let pid = child.id().unwrap_or(0);
        let stderr = child.stderr.take();

        if !wait_until_ready(scheme, port).await {
            let tail = drain_stderr_tail(stderr).await;
            crate::runtime::spawn_env::kill_child_process_group_now(pid);
            let _ = child.kill().await;
            return Err(format!(
                "Vite did not answer on port {port} within {}s.{}",
                READY_TIMEOUT.as_secs(),
                if tail.is_empty() {
                    String::new()
                } else {
                    format!(" Its last output was: {tail}")
                }
            ));
        }

        // Keep reading stderr for the preview's whole life, and log what it says.
        //
        // Not optional tidiness: dropping `ChildStderr` closes the read end of
        // the pipe, so the first thing Vite writes to stderr afterwards (a type
        // error, a failed hot update, any warning) gets EPIPE, and node treats
        // an EPIPE on `process.stderr` as fatal. The preview would then die
        // exactly when it had something to say. Draining also puts Vite's own
        // diagnostics in the engine log, which is where a user asking "why did
        // my preview stop reloading" is looked up.
        if let Some(stderr) = stderr {
            tokio::spawn(log_stderr_lines(stderr, thread_id));
        }

        let running = RunningPreview {
            thread_id,
            worktree: worktree.clone(),
            port,
            started_at: chrono::Utc::now(),
            child,
        };
        let status = FrontendPreviewStatus::of(&running);
        write_sidecar(
            &workspace,
            &PreviewSidecar {
                pid,
                port,
                thread_id,
                worktree: worktree.display().to_string(),
            },
        );
        *self.frontend_preview.lock().await = Some(running);

        log!(
            "[FrontendPreview] started for thread {} on port {} ({})",
            thread_id,
            port,
            worktree.display()
        );
        self.emit_frontend_preview_started(thread_id, port).await;
        Ok(status)
    }

    /// Stop the preview if one is running. Idempotent, and safe to call on a
    /// shutdown path.
    pub async fn stop_frontend_preview(&self) -> FrontendPreviewStatus {
        let _lifecycle = self.frontend_preview_lifecycle.lock().await;
        self.stop_frontend_preview_locked().await
    }

    /// [`stop_frontend_preview`](Self::stop_frontend_preview) for a caller that
    /// already holds the lifecycle lock.
    async fn stop_frontend_preview_locked(&self) -> FrontendPreviewStatus {
        let taken = self.frontend_preview.lock().await.take();
        let Some(running) = taken else {
            // Even with no slot, clear a sidecar left by a previous process so a
            // later boot does not chase a pid we already know nothing about.
            remove_sidecar(&self.workspace_path);
            return FrontendPreviewStatus::stopped();
        };
        self.teardown_slot(running, true, "stopped").await
    }

    /// Kill (or just bury) a taken slot, clear the sidecar, and announce it.
    ///
    /// `kill_group` is `false` for a child that has ALREADY been reaped, which is
    /// the liveness tick's case: `try_wait` reaps on success, and after that the
    /// pid, hence the process-group id, can be recycled, so signalling it would
    /// hit an unrelated group (the same caveat `spawn_env` documents for every
    /// group signal).
    async fn teardown_slot(
        &self,
        mut running: RunningPreview,
        kill_group: bool,
        reason: &str,
    ) -> FrontendPreviewStatus {
        if kill_group {
            if let Some(pid) = running.child.id() {
                crate::runtime::spawn_env::kill_child_process_group_now(pid);
            }
            let _ = running.child.kill().await;
            let _ = running.child.wait().await;
        }
        remove_sidecar(&self.workspace_path);
        log!(
            "[FrontendPreview] {} for thread {} (port {})",
            reason,
            running.thread_id,
            running.port
        );
        self.emit_frontend_preview_stopped(running.thread_id).await;
        FrontendPreviewStatus::stopped()
    }

    async fn emit_frontend_preview_started(&self, thread_id: uuid::Uuid, port: u16) {
        self.event_bus
            .emit_or_log(
                crate::engine::event_bus::BusEvent::System(
                    crate::engine::event_bus::SystemEvent::FrontendPreviewStarted {
                        thread_id,
                        port,
                        sent_at_ms: crate::engine::now_epoch_millis(),
                    },
                ),
                "[FrontendPreview] FrontendPreviewStarted",
            )
            .await;
    }

    async fn emit_frontend_preview_stopped(&self, thread_id: uuid::Uuid) {
        self.event_bus
            .emit_or_log(
                crate::engine::event_bus::BusEvent::System(
                    crate::engine::event_bus::SystemEvent::FrontendPreviewStopped {
                        thread_id,
                        sent_at_ms: crate::engine::now_epoch_millis(),
                    },
                ),
                "[FrontendPreview] FrontendPreviewStopped",
            )
            .await;
    }

    /// Reap a preview an earlier engine process left behind, then start the
    /// liveness tick. Called once from startup.
    pub fn init_frontend_preview(self: &std::sync::Arc<Self>) {
        if crate::runtime::is_packaged() {
            return;
        }
        reap_orphaned_preview(&self.workspace_path);
        let engine = self.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(LIVENESS_INTERVAL).await;
                engine.reconcile_frontend_preview().await;
            }
        });
    }

    /// One liveness pass: retire a slot whose preview is no longer real.
    ///
    /// Two ways it stops being real. **The worktree is gone** (reclaimed, or the
    /// change discarded), leaving Vite serving a directory that no longer
    /// exists. Or **Vite exited on its own** after passing the readiness probe: a
    /// config error, an OOM, a `kill` from outside. Without the second check the
    /// slot stays `running` forever and the UI keeps offering an Open link to a
    /// dead port.
    ///
    /// The two are torn down differently, and [`liveness_action`] is where that
    /// distinction is decided and tested.
    async fn reconcile_frontend_preview(&self) {
        let _lifecycle = self.frontend_preview_lifecycle.lock().await;
        let mut slot = self.frontend_preview.lock().await;
        let Some(running) = slot.as_mut() else { return };

        // `try_wait` REAPS the child when it reports an exit, which is exactly
        // why the verdict has to carry whether the group may still be signalled.
        let child_exited = match running.child.try_wait() {
            Ok(Some(status)) => {
                log!("[FrontendPreview] vite exited on its own: {}", status);
                Some(true)
            }
            Ok(None) => Some(false),
            Err(e) => {
                log!("[FrontendPreview] could not check on vite: {}", e);
                None
            }
        };

        let LivenessAction::Retire { kill_group, reason } =
            liveness_action(running.worktree.is_dir(), child_exited)
        else {
            return;
        };

        let Some(running) = slot.take() else { return };
        drop(slot);
        self.teardown_slot(running, kill_group, reason).await;
    }
}

/// Drain what Vite printed before it failed, so the caller can say WHY instead
/// of "it did not start". Bounded in time and size: the pipe stays open through
/// esbuild workers that inherited it, so an unbounded read never returns.
async fn drain_stderr_tail(stderr: Option<tokio::process::ChildStderr>) -> String {
    use tokio::io::AsyncReadExt;
    let Some(mut stderr) = stderr else {
        return String::new();
    };
    let mut buf = vec![0u8; 2048];
    match tokio::time::timeout(Duration::from_millis(500), stderr.read(&mut buf)).await {
        Ok(Ok(n)) if n > 0 => String::from_utf8_lossy(&buf[..n]).trim().to_string(),
        _ => String::new(),
    }
}

/// Forward the preview's stderr to the engine log for as long as it runs.
///
/// Two jobs. Vite's diagnostics (a type error, a failed hot update) become
/// greppable next to everything else the engine logs. And, less obviously, the
/// pipe stays OPEN: dropping `ChildStderr` closes the read end, and node treats
/// the resulting EPIPE on `process.stderr` as fatal, so the preview would die on
/// the first thing it tried to report. Ends by itself at EOF when Vite exits.
async fn log_stderr_lines(stderr: tokio::process::ChildStderr, thread_id: uuid::Uuid) {
    use tokio::io::AsyncBufReadExt;
    let mut lines = tokio::io::BufReader::new(stderr).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        let line = line.trim();
        if !line.is_empty() {
            log!("[FrontendPreview] [{}] {}", thread_id, line);
        }
    }
}

/// Kill a preview child an earlier engine process left running, if the recorded
/// pid still looks like ours. Always consumes the sidecar.
fn reap_orphaned_preview(workspace: &Path) {
    let Some(sidecar) = take_sidecar(workspace) else {
        return;
    };
    let self_pid = std::process::id();
    let cmd = process_command_line(sidecar.pid);
    if !sidecar_pid_is_reapable(sidecar.pid, self_pid, cmd.as_deref(), &sidecar.worktree) {
        log!(
            "[FrontendPreview] not reaping pid {}, it is not the preview we recorded",
            sidecar.pid
        );
        return;
    }
    log!(
        "[FrontendPreview] reaping orphaned preview pid {} (port {})",
        sidecar.pid,
        sidecar.port
    );
    crate::runtime::spawn_env::kill_child_process_group_now(sidecar.pid);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn port_base_prefers_the_override_and_ignores_junk() {
        assert_eq!(preview_port_base(Some("7000"), 5173), 7000);
        assert_eq!(preview_port_base(Some("  7000 "), 5173), 7000);
        assert_eq!(preview_port_base(None, 5173), 6173);
        assert_eq!(preview_port_base(Some(""), 5173), 6173);
        assert_eq!(preview_port_base(Some("not-a-port"), 5173), 6173);
        assert_eq!(preview_port_base(Some("0"), 5173), 6173);
    }

    #[test]
    fn port_base_saturates_instead_of_wrapping() {
        // A wrap would land the preview on a privileged port.
        assert_eq!(preview_port_base(None, 65_000), u16::MAX);
    }

    #[test]
    fn free_port_walks_forward_then_gives_up() {
        assert_eq!(select_free_port(6173, |_| true), Some(6173));
        assert_eq!(select_free_port(6173, |p| p >= 6175), Some(6175));
        assert_eq!(select_free_port(6173, |_| false), None);
        // The walk is bounded: a port past the window is never chosen.
        assert_eq!(
            select_free_port(6173, |p| p == 6173 + PORT_WALK_LIMIT),
            None
        );
    }

    #[test]
    fn path_containment_rejects_the_parent_itself_and_siblings() {
        let parent = Path::new("/ws/.lucidos/worktrees");
        assert!(path_is_inside(
            parent,
            Path::new("/ws/.lucidos/worktrees/thread-abc")
        ));
        assert!(!path_is_inside(parent, parent));
        assert!(!path_is_inside(
            parent,
            Path::new("/ws/.lucidos/other/thread-abc")
        ));
        assert!(!path_is_inside(parent, Path::new("/etc")));
    }

    fn worktrees() -> PathBuf {
        PathBuf::from("/ws/.lucidos/worktrees")
    }
    fn wt() -> PathBuf {
        PathBuf::from("/ws/.lucidos/worktrees/thread-abc12345")
    }

    #[test]
    fn a_packaged_engine_refuses_before_looking_at_anything_else() {
        let err = classify_worktree(true, &worktrees(), &wt(), true, true, true).unwrap_err();
        assert_eq!(err, PreviewRefusal::Packaged);
    }

    #[test]
    fn a_path_outside_the_worktrees_dir_is_refused() {
        let outside = PathBuf::from("/etc");
        let err = classify_worktree(false, &worktrees(), &outside, true, true, true).unwrap_err();
        assert!(matches!(err, PreviewRefusal::OutsideWorktrees(_)));
    }

    #[test]
    fn a_reclaimed_worktree_is_refused_by_name() {
        let err = classify_worktree(false, &worktrees(), &wt(), false, true, true).unwrap_err();
        match err {
            PreviewRefusal::NoWorktree(p) => assert!(p.contains("thread-abc12345")),
            other => panic!("expected NoWorktree, got {other:?}"),
        }
    }

    #[test]
    fn an_app_worktree_has_no_frontend_to_preview() {
        // An app coding-agent thread gets the same deterministic path, sparse-
        // checked-out to `data/apps/<id>`, so `crates/lucidos-app` is absent.
        let err = classify_worktree(false, &worktrees(), &wt(), true, false, true).unwrap_err();
        assert!(matches!(err, PreviewRefusal::NotTheLucidosFrontend(_)));
    }

    #[test]
    fn a_worktree_without_provisioned_dependencies_is_refused() {
        let err = classify_worktree(false, &worktrees(), &wt(), true, true, false).unwrap_err();
        assert!(matches!(err, PreviewRefusal::NoNodeModules(_)));
    }

    #[test]
    fn a_lucidos_worktree_is_accepted() {
        assert!(classify_worktree(false, &worktrees(), &wt(), true, true, true).is_ok());
    }

    #[test]
    fn every_refusal_names_the_path_it_refused() {
        let p = "/ws/.lucidos/worktrees/thread-abc12345";
        for refusal in [
            PreviewRefusal::OutsideWorktrees(p.into()),
            PreviewRefusal::NoWorktree(p.into()),
            PreviewRefusal::NotTheLucidosFrontend(p.into()),
            PreviewRefusal::NoNodeModules(p.into()),
        ] {
            assert!(
                refusal.to_string().contains(p),
                "refusal must name the path: {refusal:?}"
            );
        }
    }

    const WT: &str = "/ws/.lucidos/worktrees/thread-abc12345";

    #[test]
    fn the_reaper_kills_only_a_vite_rooted_in_the_recorded_worktree() {
        let ours = format!("node {WT}/node_modules/.bin/vite --port 6173");
        assert!(sidecar_pid_is_reapable(4242, 99, Some(&ours), WT));
    }

    #[test]
    fn the_reaper_refuses_a_recycled_pid_running_something_else() {
        assert!(!sidecar_pid_is_reapable(
            4242,
            99,
            Some("/usr/bin/node /some/other/server.js"),
            WT
        ));
        // Vite, but somebody else's worktree.
        assert!(!sidecar_pid_is_reapable(
            4242,
            99,
            Some("node /ws/.lucidos/worktrees/thread-99999999/node_modules/.bin/vite"),
            WT
        ));
    }

    #[test]
    fn the_reaper_refuses_when_the_probe_could_not_run() {
        // ADR 0025 and the unknown-git-state rule: no answer is never a yes.
        assert!(!sidecar_pid_is_reapable(4242, 99, None, WT));
    }

    #[test]
    fn the_reaper_never_signals_this_process_or_pid_zero() {
        let ours = format!("node {WT}/node_modules/.bin/vite");
        assert!(!sidecar_pid_is_reapable(99, 99, Some(&ours), WT));
        assert!(!sidecar_pid_is_reapable(0, 99, Some(&ours), WT));
    }

    #[test]
    fn a_live_preview_with_a_live_worktree_is_left_alone() {
        assert_eq!(liveness_action(true, Some(false)), LivenessAction::Keep);
    }

    #[test]
    fn a_vite_that_exited_is_retired_without_signalling_its_group() {
        // `try_wait` reaped the child to learn this, so the pid is free to be
        // recycled and signalling its group would hit an unrelated process
        // (the ADR 0025 hazard, and the reason the verdict carries the flag).
        match liveness_action(true, Some(true)) {
            LivenessAction::Retire { kill_group, .. } => assert!(
                !kill_group,
                "an already-reaped child must never have its group signalled"
            ),
            other => panic!("expected Retire, got {other:?}"),
        }
    }

    #[test]
    fn a_reclaimed_worktree_retires_a_live_preview_and_kills_it() {
        // The opposite case: vite is alive and serving a directory that is gone,
        // so the group is still ours to signal and still needs signalling.
        match liveness_action(false, Some(false)) {
            LivenessAction::Retire { kill_group, reason } => {
                assert!(kill_group);
                assert!(reason.contains("worktree"), "unhelpful reason: {reason}");
            }
            other => panic!("expected Retire, got {other:?}"),
        }
    }

    #[test]
    fn a_reaped_child_is_never_signalled_even_when_the_worktree_is_also_gone() {
        // Both conditions hold after a Discard, which removes the tree AND kills
        // the session's processes. The reaped child wins: "the worktree is gone"
        // is a reason to retire, never a licence to signal a pid that is no
        // longer ours.
        match liveness_action(false, Some(true)) {
            LivenessAction::Retire { kill_group, reason } => {
                assert!(!kill_group, "a reaped pid must not be signalled");
                assert!(reason.contains("worktree"), "unhelpful reason: {reason}");
            }
            other => panic!("expected Retire, got {other:?}"),
        }
    }

    #[test]
    fn a_check_that_could_not_run_leaves_the_preview_alone() {
        // Unknown is not a yes: tearing down on a failed wait would kill a
        // preview the user is looking at, and the next tick asks again.
        assert_eq!(liveness_action(true, None), LivenessAction::Keep);
    }

    #[test]
    fn the_proxy_origin_follows_the_engines_own_scheme() {
        assert_eq!(engine_api_origin("https", 5173), "https://127.0.0.1:5173");
        assert_eq!(engine_api_origin("http", 3000), "http://127.0.0.1:3000");
    }

    #[test]
    fn the_preview_url_keeps_the_host_the_requester_used() {
        // The whole point: a phone on Tailscale must not be handed localhost.
        assert_eq!(
            preview_url_for_host(Some("my-laptop.tailnet.ts.net:5173"), "https", 6173).as_deref(),
            Some("https://my-laptop.tailnet.ts.net:6173/")
        );
        assert_eq!(
            preview_url_for_host(Some("localhost:5173"), "https", 6173).as_deref(),
            Some("https://localhost:6173/")
        );
        // A Host with no port at all (default 80/443) still yields a hostname.
        assert_eq!(
            preview_url_for_host(Some("lucidos.local"), "http", 6173).as_deref(),
            Some("http://lucidos.local:6173/")
        );
    }

    #[test]
    fn an_ipv6_host_keeps_its_brackets_and_loses_only_the_port() {
        // The address is full of colons, so splitting on the FIRST one would
        // yield "[" and build a URL nothing can resolve.
        assert_eq!(
            preview_url_for_host(Some("[::1]:5173"), "https", 6173).as_deref(),
            Some("https://[::1]:6173/")
        );
        assert_eq!(
            preview_url_for_host(Some("[fd7a:115c::1]"), "https", 6173).as_deref(),
            Some("https://[fd7a:115c::1]:6173/")
        );
    }

    #[test]
    fn an_unusable_host_yields_no_url_rather_than_a_guess() {
        assert_eq!(preview_url_for_host(None, "https", 6173), None);
        assert_eq!(preview_url_for_host(Some("   "), "https", 6173), None);
        assert_eq!(preview_url_for_host(Some(":5173"), "https", 6173), None);
        // Unclosed bracket: malformed, so there is no hostname to extract.
        assert_eq!(preview_url_for_host(Some("[::1"), "https", 6173), None);
    }

    #[test]
    fn a_stopped_status_carries_nothing_else() {
        let json = serde_json::to_value(FrontendPreviewStatus::stopped()).unwrap();
        assert_eq!(json, serde_json::json!({ "running": false }));
    }

    #[test]
    fn the_sidecar_round_trips() {
        let s = PreviewSidecar {
            pid: 4242,
            port: 6173,
            thread_id: uuid::Uuid::nil(),
            worktree: WT.to_string(),
        };
        let back: PreviewSidecar =
            serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
        assert_eq!(s, back);
    }

    #[test]
    fn taking_an_absent_sidecar_is_not_an_error() {
        let dir = tempfile::tempdir().unwrap();
        assert!(take_sidecar(dir.path()).is_none());
    }

    #[test]
    fn a_sidecar_is_consumed_even_when_it_is_malformed() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".lucidos")).unwrap();
        let path = sidecar_path(dir.path());
        std::fs::write(&path, "{ not json").unwrap();
        assert!(take_sidecar(dir.path()).is_none());
        assert!(
            !path.exists(),
            "a malformed sidecar must not be re-processed on the next boot"
        );
    }

    #[test]
    fn a_written_sidecar_is_read_back_then_gone() {
        let dir = tempfile::tempdir().unwrap();
        let s = PreviewSidecar {
            pid: 7,
            port: 6173,
            thread_id: uuid::Uuid::nil(),
            worktree: WT.to_string(),
        };
        write_sidecar(dir.path(), &s);
        assert_eq!(take_sidecar(dir.path()), Some(s));
        assert!(take_sidecar(dir.path()).is_none());
    }

    /// ADR 0021: the preview must never become the workspace's serving path.
    /// A source scan rather than a behavioral test, because the failure it
    /// guards is a future edit adding the pin, not a branch in today's code.
    #[test]
    fn the_preview_never_touches_the_served_frontend() {
        let src = include_str!("frontend_preview.rs");
        // Two exclusions, both deliberate. This test's own body names every
        // forbidden symbol, and so does the module's prose, which has to say
        // what the preview is NOT for the reader to trust it. Only executable
        // lines are scanned.
        let code: String = src
            .split("#[cfg(test)]")
            .next()
            .unwrap()
            .lines()
            .filter(|l| !l.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n");
        for forbidden in [
            "LUCIDOS_STATIC_DIR",
            "init_served_frontend",
            "served_frontend",
        ] {
            assert!(
                !code.contains(forbidden),
                "the frontend preview must not reference {forbidden} (ADR 0021)"
            );
        }
    }
}
