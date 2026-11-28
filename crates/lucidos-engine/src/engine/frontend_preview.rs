use super::LucidosEngine;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::Duration;

const SIDECAR_FILENAME: &str = "frontend-preview.json";

const READY_TIMEOUT: Duration = Duration::from_secs(60);

const READY_POLL: Duration = Duration::from_millis(250);

pub const PREVIEW_WORKSPACE_ID_ENV: &str = "LUCIDOS_FRONTEND_PREVIEW_WORKSPACE_ID";
pub const PREVIEW_SOCKET_ENV: &str = "LUCIDOS_FRONTEND_PREVIEW_SOCKET";

/// The same path is used by the local gateway and any trusted relay.
pub fn preview_path(key: Option<&str>, slug: Option<&str>) -> Result<String, PreviewRefusal> {
    let key = key
        .filter(|key| {
            key.len() == 16
                && key
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        })
        .ok_or(PreviewRefusal::NoGateway)?;
    let slug = slug
        .filter(|slug| lucidos_gateway_constants::is_valid_workspace_id(slug))
        .ok_or(PreviewRefusal::NoGateway)?;
    Ok(format!("/~preview/{key}/{slug}/"))
}

pub fn preview_socket_path(workspace: &Path) -> Result<PathBuf, String> {
    let path = workspace.join(".lucidos/frontend-preview/vite.sock");
    if path.as_os_str().as_encoded_bytes().len() > 103 {
        return Err("The frontend preview socket path exceeds the portable Unix socket limit (103 bytes). Move the workspace to a shorter path to start a preview.".into());
    }
    Ok(path)
}

/// INVARIANT: The running child plus everything needed to describe and reap it.
///
/// The child is NOT `kill_on_drop`: Vite spawns esbuild workers, and
/// `kill_on_drop` reaches only the direct child. Teardown signals the whole
/// process group instead (`isolate_in_process_group` at spawn time).
pub struct RunningPreview {
    pub thread_id: uuid::Uuid,
    pub worktree: PathBuf,
    pub path: String,
    socket: PathBuf,
    pub started_at: chrono::DateTime<chrono::Utc>,
    child: tokio::process::Child,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FrontendPreviewStatus {
    pub running: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<uuid::Uuid>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
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
            path: None,
            started_at: None,
            worktree: None,
        }
    }

    fn of(p: &RunningPreview) -> Self {
        Self {
            running: true,
            thread_id: Some(p.thread_id),
            path: Some(p.path.clone()),
            started_at: Some(p.started_at),
            worktree: Some(p.worktree.display().to_string()),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PreviewSidecar {
    pub pid: u32,
    pub thread_id: uuid::Uuid,
    pub worktree: String,
}

pub fn path_is_inside(parent: &Path, candidate: &Path) -> bool {
    candidate.starts_with(parent) && candidate != parent
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PreviewRefusal {
    Packaged,
    OutsideWorktrees(String),
    NoWorktree(String),
    NotTheLucidosFrontend(String),
    NoNodeModules(String),
    NoGateway,
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
            Self::NoGateway => write!(f, "The frontend preview needs a gateway preview key and workspace slug. Start this engine through its gateway."),
        }
    }
}

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LivenessAction {
    Keep,
    /// INVARIANT: Retire the slot. `kill_group` says whether the child still needs killing:
    /// `false` means it has ALREADY been reaped, so its pid (and with it the
    /// process-group id) may have been recycled and must never be signalled.
    Retire {
        kill_group: bool,
        reason: &'static str,
    },
}

/// INVARIANT: Decide a liveness pass from the two facts the caller looked up.
///
/// `child_exited` is `None` for "still running, or the wait itself failed". A
/// failed wait says nothing about the child, and killing on unknown would tear
/// down a preview the user is looking at, so it is treated as alive: the same
/// direction the unknown-git-state rule takes, keeping the thing that might
/// still be wanted.
pub fn liveness_action(worktree_exists: bool, child_exited: Option<bool>) -> LivenessAction {
    // INVARIANT: The exit check comes FIRST, and it is the one that decides `kill_group`,
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
        return LivenessAction::Retire {
            kill_group: true,
            reason: "stopped, its worktree is gone",
        };
    }
    LivenessAction::Keep
}

fn sidecar_path(workspace: &Path) -> PathBuf {
    workspace.join(".lucidos").join(SIDECAR_FILENAME)
}

fn write_sidecar(workspace: &Path, sidecar: &PreviewSidecar) {
    let path = sidecar_path(workspace);
    let json = match serde_json::to_string(sidecar) {
        Ok(j) => j,
        Err(e) => {
            log_legacy!("[FrontendPreview] failed to serialize sidecar: {}", e);
            return;
        }
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Err(e) = std::fs::write(&path, json) {
        log_legacy!(
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
        Err(e) => log_legacy!(
            "[FrontendPreview] failed to remove {}: {}",
            path.display(),
            e
        ),
    }
}

/// INVARIANT: Read + delete the sidecar. Deleted BEFORE parsing so a malformed file is not
/// re-processed on every boot (same reasoning as `supervisor_respawn_sidecar`).
fn take_sidecar(workspace: &Path) -> Option<PreviewSidecar> {
    let path = sidecar_path(workspace);
    let bytes = match std::fs::read(&path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return None,
        Err(e) => {
            log_legacy!("[FrontendPreview] failed to read {}: {}", path.display(), e);
            return None;
        }
    };
    let _ = std::fs::remove_file(&path);
    match serde_json::from_slice::<PreviewSidecar>(&bytes) {
        Ok(s) => Some(s),
        Err(e) => {
            log_legacy!("[FrontendPreview] malformed sidecar discarded: {}", e);
            None
        }
    }
}

/// INVARIANT: A live process's argv as one string, or `None` when it is gone or `ps`
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

/// Remove only a private socket that has no listener. A live collision is never unlinked.
async fn cleanup_unused_socket(socket: &Path) {
    use std::os::unix::fs::{FileTypeExt, MetadataExt};
    let Ok(metadata) = std::fs::symlink_metadata(socket) else {
        return;
    };
    if !metadata.file_type().is_socket() || metadata.uid() != unsafe { libc::geteuid() } {
        return;
    }
    if matches!(
        tokio::time::timeout(READY_POLL, tokio::net::UnixStream::connect(socket)).await,
        Ok(Err(e)) if e.kind() == std::io::ErrorKind::ConnectionRefused
    ) {
        if let Err(e) = std::fs::remove_file(socket) {
            log_legacy!(
                "[FrontendPreview] could not remove stale socket {}: {}",
                socket.display(),
                e
            );
        }
    }
}

async fn wait_until_ready(socket: &Path, path: &str, child: &mut tokio::process::Child) -> bool {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let deadline = tokio::time::Instant::now() + READY_TIMEOUT;
    while tokio::time::Instant::now() < deadline {
        if matches!(child.try_wait(), Ok(Some(_))) {
            return false;
        }
        let probe = async {
            let mut stream = tokio::net::UnixStream::connect(socket).await.ok()?;
            stream
                .write_all(
                    format!("GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
                        .as_bytes(),
                )
                .await
                .ok()?;
            let mut response = [0; 12];
            stream.read_exact(&mut response).await.ok()?;
            (response.starts_with(b"HTTP/1.1 200")).then_some(())
        };
        if matches!(
            tokio::time::timeout(Duration::from_secs(2), probe).await,
            Ok(Some(()))
        ) {
            return true;
        }
        tokio::time::sleep(READY_POLL).await;
    }
    false
}

impl LucidosEngine {
    pub async fn frontend_preview_status(&self) -> FrontendPreviewStatus {
        match self.frontend_preview.lock().await.as_ref() {
            Some(p) => FrontendPreviewStatus::of(p),
            None => FrontendPreviewStatus::stopped(),
        }
    }

    pub async fn start_frontend_preview(
        &self,
        thread_id: uuid::Uuid,
    ) -> Result<FrontendPreviewStatus, String> {
        // INVARIANT: Held for the WHOLE start, not just the slot write. The slot mutex is
        // released between stopping the old preview, binding the socket, spawning and
        // waiting for readiness, so two concurrent starts (two devices, or the
        // CLI alongside a tap) would both pass the socket check, both spawn,
        // and the second would overwrite the first's `RunningPreview`. The child
        // is not `kill_on_drop`, so the displaced Vite would be orphaned with
        // nothing tracking its pid: no slot, no sidecar, no reaper.
        let _lifecycle = self.frontend_preview_lifecycle.lock().await;
        let workspace = self.workspace_path.clone();
        let worktrees = workspace.join(crate::paths::WORKTREES_SUBPATH);
        let worktree =
            crate::engine::session_queries::deterministic_worktree_path(&workspace, thread_id);
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
        let key = std::env::var(lucidos_gateway_stamp::frontend_preview::ENV_PREVIEW_KEY).ok();
        let slug = crate::core::base_path::workspace_id();
        let path = preview_path(key.as_deref(), slug.as_deref()).map_err(|r| r.to_string())?;
        let socket = preview_socket_path(&workspace)?;
        let launcher = worktree.join("crates/lucidos-app/vite/frontendPreviewSocket.mjs");
        if !launcher.is_file() {
            return Err("This branch has no socket preview launcher. Update the branch before previewing it.".into());
        }
        self.stop_frontend_preview_locked().await;
        cleanup_unused_socket(&socket).await;
        if std::fs::symlink_metadata(&socket).is_ok() {
            return Err("The frontend preview socket is already occupied. Stop its owning preview before starting another.".into());
        }
        let sandbox = crate::runtime::agent_sandbox::Sandbox::resolve(self.pool(), &workspace)
            .await
            .map_err(|e| e.to_string())?;
        let mut cmd = sandbox
            .command(Path::new("node"))
            .map_err(|e| format!("Could not prepare Node for the frontend preview: {e}"))?;
        cmd.arg(&launcher)
            .current_dir(worktree.join("crates/lucidos-app"))
            .env(PREVIEW_SOCKET_ENV, &socket)
            .env(
                lucidos_gateway_stamp::frontend_preview::ENV_PREVIEW_KEY,
                key.unwrap(),
            )
            .env(PREVIEW_WORKSPACE_ID_ENV, slug.unwrap())
            .env_remove("LUCIDOS_TLS_CERT")
            .env_remove("LUCIDOS_TLS_KEY")
            .env_remove("LUCIDOS_ATOMIC_DIST")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped());
        crate::runtime::spawn_env::isolate_in_process_group(&mut cmd);

        let mut child = cmd.spawn().map_err(|e| {
            format!(
                "Could not start the frontend preview launcher {}: {e}",
                launcher.display()
            )
        })?;
        let pid = child.id().unwrap_or(0);
        let stderr = child.stderr.take();

        write_sidecar(
            &workspace,
            &PreviewSidecar {
                pid,
                thread_id,
                worktree: worktree.display().to_string(),
            },
        );
        if !wait_until_ready(&socket, &path, &mut child).await {
            let tail = drain_stderr_tail(stderr).await;
            if let Some(live_pid) = child.id() {
                crate::runtime::spawn_env::kill_child_process_group_now(live_pid);
                let _ = child.kill().await;
                let _ = child.wait().await;
            }
            cleanup_unused_socket(&socket).await;
            remove_sidecar(&workspace);
            return Err(format!(
                "Vite did not answer on its private socket within {}s.{}",
                READY_TIMEOUT.as_secs(),
                if tail.is_empty() {
                    String::new()
                } else {
                    format!(" Its last output was: {tail}")
                }
            ));
        }

        // INVARIANT: Keep reading stderr for the preview's whole life, and log what it says.
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
            path,
            socket,
            started_at: chrono::Utc::now(),
            child,
        };
        let status = FrontendPreviewStatus::of(&running);
        *self.frontend_preview.lock().await = Some(running);

        log_legacy!(
            "[FrontendPreview] started for thread {} at {} ({})",
            thread_id,
            status.path.as_deref().unwrap_or_default(),
            worktree.display()
        );
        self.emit_frontend_preview_started(&status).await;
        Ok(status)
    }

    pub async fn stop_frontend_preview(&self) -> FrontendPreviewStatus {
        let _lifecycle = self.frontend_preview_lifecycle.lock().await;
        self.stop_frontend_preview_locked().await
    }

    async fn stop_frontend_preview_locked(&self) -> FrontendPreviewStatus {
        let taken = self.frontend_preview.lock().await.take();
        let Some(running) = taken else {
            remove_sidecar(&self.workspace_path);
            return FrontendPreviewStatus::stopped();
        };
        self.teardown_slot(running, true, "stopped").await
    }

    /// INVARIANT: Kill (or just bury) a taken slot, clear the sidecar, and announce it.
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
        cleanup_unused_socket(&running.socket).await;
        remove_sidecar(&self.workspace_path);
        log_legacy!(
            "[FrontendPreview] {} for thread {} ({})",
            reason,
            running.thread_id,
            running.path
        );
        self.emit_frontend_preview_stopped(running.thread_id).await;
        FrontendPreviewStatus::stopped()
    }

    async fn emit_frontend_preview_started(&self, status: &FrontendPreviewStatus) {
        let (Some(thread_id), Some(path)) = (status.thread_id, status.path.as_ref()) else {
            return;
        };
        self.event_bus
            .emit_or_log(
                crate::engine::event_bus::BusEvent::System(
                    crate::engine::event_bus::SystemEvent::FrontendPreviewStarted {
                        thread_id,
                        path: path.clone(),
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

    pub async fn reconcile_frontend_preview(&self) {
        let _lifecycle = self.frontend_preview_lifecycle.lock().await;
        let mut slot = self.frontend_preview.lock().await;
        let Some(running) = slot.as_mut() else { return };

        // INVARIANT: `try_wait` REAPS the child when it reports an exit, which is exactly
        // why the verdict has to carry whether the group may still be signalled.
        let child_exited = match running.child.try_wait() {
            Ok(Some(status)) => {
                log_legacy!("[FrontendPreview] vite exited on its own: {}", status);
                Some(true)
            }
            Ok(None) => Some(false),
            Err(e) => {
                log_legacy!("[FrontendPreview] could not check on vite: {}", e);
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

/// INVARIANT: Drain what Vite printed before it failed, so the caller can say WHY instead
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

/// INVARIANT: Forward the preview's stderr to the engine log for as long as it runs.
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
            log_legacy!("[FrontendPreview] [{}] {}", thread_id, line);
        }
    }
}

pub fn reap_orphaned_preview(workspace: &Path) {
    let Some(sidecar) = take_sidecar(workspace) else {
        return;
    };
    let self_pid = std::process::id();
    let cmd = process_command_line(sidecar.pid);
    if !sidecar_pid_is_reapable(sidecar.pid, self_pid, cmd.as_deref(), &sidecar.worktree) {
        log_legacy!(
            "[FrontendPreview] not reaping pid {}, it is not the preview we recorded",
            sidecar.pid
        );
        return;
    }
    log_legacy!(
        "[FrontendPreview] reaping orphaned preview pid {}",
        sidecar.pid
    );
    crate::runtime::spawn_env::kill_child_process_group_now(sidecar.pid);
    // INVARIANT: Start removes any stale socket only after the killed listener
    // has gone. Never unlink here: SIGKILL delivery may still be in progress.
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn socket_preview_requires_a_valid_gateway_key_and_workspace_slug() {
        assert_eq!(
            preview_path(Some("0123456789abcdef"), Some("dev")).unwrap(),
            "/~preview/0123456789abcdef/dev/"
        );
        for (key, slug) in [
            (None, Some("dev")),
            (Some("bad"), Some("dev")),
            (Some("0123456789abcdef"), Some("../dev")),
        ] {
            assert!(preview_path(key, slug).is_err());
        }
    }

    #[test]
    fn socket_paths_fail_explicitly_above_the_portable_limit() {
        assert!(preview_socket_path(Path::new("/ws")).is_ok());
        let long = format!("/{}", "x".repeat(104));
        assert!(preview_socket_path(Path::new(&long))
            .unwrap_err()
            .contains("103"));
    }

    #[test]
    fn the_new_launcher_and_legacy_sidecars_remain_reapable() {
        let json = serde_json::json!({"pid": 4242, "port": 6173, "thread_id": uuid::Uuid::nil(), "worktree": WT});
        let legacy: PreviewSidecar = serde_json::from_value(json).unwrap();
        assert_eq!(legacy.pid, 4242);
        assert!(serde_json::to_value(&legacy).unwrap().get("port").is_none());
        assert!(sidecar_pid_is_reapable(
            4242,
            99,
            Some(&format!(
                "node {WT}/crates/lucidos-app/vite/frontendPreviewSocket.mjs"
            )),
            WT
        ));
    }

    #[tokio::test]
    async fn cleanup_preserves_a_live_socket_and_removes_a_dead_one() {
        use std::os::unix::net::UnixListener;
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("vite.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        cleanup_unused_socket(&socket).await;
        assert!(socket.exists());
        drop(listener);
        // INVARIANT: Concurrent process-spawning tests can inherit the listener
        // briefly between fork and exec. A connected socket must stay untouched
        // until those descriptors close, just as on an engine teardown.
        tokio::time::timeout(Duration::from_secs(2), async {
            while socket.exists() {
                cleanup_unused_socket(&socket).await;
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("the dead socket must eventually be removed");
    }

    #[tokio::test]
    async fn cleanup_never_removes_a_regular_file_or_symlink() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("vite.sock");
        std::fs::write(&file, "keep").unwrap();
        cleanup_unused_socket(&file).await;
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "keep");
        let link = dir.path().join("link.sock");
        std::os::unix::fs::symlink(&file, &link).unwrap();
        cleanup_unused_socket(&link).await;
        assert!(link.symlink_metadata().unwrap().file_type().is_symlink());
    }

    #[tokio::test]
    async fn readiness_requires_an_http_response_on_the_socket_path() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("vite.sock");
        let listener = tokio::net::UnixListener::bind(&socket).unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = [0; 512];
            let n = stream.read(&mut request).await.unwrap();
            let request = std::str::from_utf8(&request[..n]).unwrap();
            assert!(request.starts_with("GET /~preview/0123456789abcdef/dev/ HTTP/1.1"));
            assert!(!request.to_lowercase().contains("authorization:"));
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n")
                .await
                .unwrap();
        });
        let mut child = tokio::process::Command::new("sleep")
            .arg("30")
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let ready = wait_until_ready(&socket, "/~preview/0123456789abcdef/dev/", &mut child).await;
        child.kill().await.unwrap();
        child.wait().await.unwrap();
        server.await.unwrap();
        assert!(ready);
    }

    #[tokio::test]
    async fn readiness_stops_immediately_when_the_child_exits() {
        let mut child = tokio::process::Command::new("true").spawn().unwrap();
        child.wait().await.unwrap();
        assert!(!wait_until_ready(Path::new("/nonexistent-preview.sock"), "/", &mut child).await);
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
        assert!(!sidecar_pid_is_reapable(
            4242,
            99,
            Some("node /ws/.lucidos/worktrees/thread-99999999/node_modules/.bin/vite"),
            WT
        ));
    }

    #[test]
    fn the_reaper_refuses_when_the_probe_could_not_run() {
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
        assert_eq!(liveness_action(true, None), LivenessAction::Keep);
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
            thread_id: uuid::Uuid::nil(),
            worktree: WT.to_string(),
        };
        write_sidecar(dir.path(), &s);
        assert_eq!(take_sidecar(dir.path()), Some(s));
        assert!(take_sidecar(dir.path()).is_none());
    }

    fn executable_lines() -> String {
        include_str!("frontend_preview.rs")
            .split("#[cfg(test)]")
            .next()
            .unwrap()
            .lines()
            .filter(|l| !l.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn the_preview_never_touches_the_served_frontend() {
        let code = executable_lines();
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
