//! In-memory registry for `run_bash_background` tasks. Holds only
//! currently-running tasks; the dispatch site emits
//! `BackgroundBashCompleted` and evicts on completion.

use chrono::{DateTime, Utc};
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::sync::Mutex;
use uuid::Uuid;

/// Soft cap on per-stream buffered output. The drain only trims when the
/// buffer reaches `2 * MAX_BUFFER_BYTES`, so the per-chunk amortized cost
/// of `Vec::drain` stays O(1) instead of going quadratic on chatty
/// processes.
const MAX_BUFFER_BYTES: usize = 1024 * 1024;
const TRIM_TRIGGER_BYTES: usize = 2 * MAX_BUFFER_BYTES;

#[derive(Clone)]
pub struct BackgroundBashRegistry {
    tasks: Arc<Mutex<HashMap<String, RunningTask>>>,
}

pub struct RunningTask {
    pub(super) started_at: DateTime<Utc>,
    pub(super) stdout: Vec<u8>,
    pub(super) stderr: Vec<u8>,
    stdout_cursor: usize,
    stderr_cursor: usize,
    pub(super) exit_code: Option<i32>,
    pub(super) timed_out: bool,
    pub(super) killed: bool,
    /// Single source of truth for "has the watchdog finished?".
    /// `None` = still running, `Some(t)` = finished at `t`.
    pub(super) finished_at: Option<DateTime<Utc>>,
    kill_signal: Option<tokio::sync::oneshot::Sender<()>>,
}

impl RunningTask {
    fn is_finished(&self) -> bool {
        self.finished_at.is_some()
    }
}

#[derive(Debug, Clone)]
pub struct OutputSnapshot {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: Option<i32>,
    pub finished: bool,
    pub timed_out: bool,
    pub killed: bool,
}

impl Default for BackgroundBashRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl BackgroundBashRegistry {
    pub fn new() -> Self {
        BackgroundBashRegistry {
            tasks: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Spawn a child process. Inserts the task into the registry before
    /// returning the task_id, eliminating the spawn/poll race a follow-up
    /// `bash_output` would otherwise hit. The returned receiver fires
    /// when the watchdog marks the task finished — the dispatch site uses
    /// it to emit `BackgroundBashCompleted` and call `take_finished` for
    /// eviction.
    pub async fn spawn(
        &self,
        command: &str,
        timeout_secs: u64,
        cwd: &Path,
        env: &[(String, String)],
    ) -> Result<(String, tokio::sync::oneshot::Receiver<()>), Box<dyn std::error::Error + Send + Sync>>
    {
        let task_id = Uuid::new_v4().to_string();

        let mut cmd = tokio::process::Command::new("/bin/sh");
        cmd.args(["-c", command])
            .current_dir(cwd)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);
        for (k, v) in env {
            cmd.env(k, v);
        }

        let mut child = cmd.spawn()?;
        let stdout = child.stdout.take().expect("piped stdout");
        let stderr = child.stderr.take().expect("piped stderr");

        let (kill_tx, kill_rx) = tokio::sync::oneshot::channel();
        let (finish_tx, finish_rx) = tokio::sync::oneshot::channel::<()>();

        {
            let mut tasks = self.tasks.lock().await;
            tasks.insert(
                task_id.clone(),
                RunningTask {
                    started_at: Utc::now(),
                    stdout: Vec::new(),
                    stderr: Vec::new(),
                    stdout_cursor: 0,
                    stderr_cursor: 0,
                    exit_code: None,
                    timed_out: false,
                    killed: false,
                    finished_at: None,
                    kill_signal: Some(kill_tx),
                },
            );
        }

        let stdout_drain = tokio::spawn(drain_pipe(
            self.tasks.clone(),
            task_id.clone(),
            stdout,
            false,
        ));
        let stderr_drain = tokio::spawn(drain_pipe(
            self.tasks.clone(),
            task_id.clone(),
            stderr,
            true,
        ));

        let tasks = self.tasks.clone();
        let id = task_id.clone();
        tokio::spawn(async move {
            let timeout_fut = tokio::time::sleep(Duration::from_secs(timeout_secs));
            tokio::pin!(timeout_fut);
            let mut timed_out = false;
            let mut killed = false;
            let exit_code: Option<i32> = tokio::select! {
                exit = child.wait() => match exit {
                    Ok(status) => status.code(),
                    Err(_) => None,
                },
                _ = &mut timeout_fut => {
                    let _ = child.kill().await;
                    timed_out = true;
                    let _ = child.wait().await;
                    None
                }
                _ = kill_rx => {
                    let _ = child.kill().await;
                    killed = true;
                    let _ = child.wait().await;
                    None
                }
            };
            // Wait for both drain tasks to flush remaining buffered bytes
            // and hit EOF before signaling finish — otherwise the
            // dispatch-site watcher could evict the registry entry while
            // the drains are still racing the kernel pipe, losing the
            // tail of stdout/stderr after a kill or timeout. Bound the
            // join with a deadline so a stuck drain task never wedges
            // the watchdog (the OS pipe should always close after wait,
            // but we don't want that assumption to deadlock production).
            let drain_deadline = tokio::time::sleep(Duration::from_secs(2));
            tokio::select! {
                _ = async { let _ = tokio::join!(stdout_drain, stderr_drain); } => {}
                _ = drain_deadline => {}
            }
            let mut t = tasks.lock().await;
            if let Some(task) = t.get_mut(&id) {
                task.exit_code = exit_code;
                task.timed_out = timed_out;
                task.killed = killed;
                task.finished_at = Some(Utc::now());
                task.kill_signal = None;
            }
            // Ignore send error: receiver drop just means nobody's listening.
            let _ = finish_tx.send(());
        });

        Ok((task_id, finish_rx))
    }

    /// Drain the in-memory buffer for a running or recently-finished task.
    /// Returns `None` when the task is unknown (caller should fall back to
    /// the event-store query). The cursor advances by the bytes returned,
    /// so the next call only sees newly-written output.
    pub async fn read_output_in_memory(&self, task_id: &str) -> Option<OutputSnapshot> {
        let mut tasks = self.tasks.lock().await;
        let task = tasks.get_mut(task_id)?;
        let new_stdout = String::from_utf8_lossy(&task.stdout[task.stdout_cursor..]).to_string();
        let new_stderr = String::from_utf8_lossy(&task.stderr[task.stderr_cursor..]).to_string();
        task.stdout_cursor = task.stdout.len();
        task.stderr_cursor = task.stderr.len();
        Some(OutputSnapshot {
            stdout: new_stdout,
            stderr: new_stderr,
            exit_code: task.exit_code,
            finished: task.is_finished(),
            timed_out: task.timed_out,
            killed: task.killed,
        })
    }

    /// Cancel a running task. Returns `false` if the task is unknown or
    /// already finished.
    pub async fn kill(&self, task_id: &str) -> bool {
        let mut tasks = self.tasks.lock().await;
        let Some(task) = tasks.get_mut(task_id) else {
            return false;
        };
        if task.is_finished() {
            return false;
        }
        if let Some(tx) = task.kill_signal.take() {
            let _ = tx.send(());
            true
        } else {
            false
        }
    }

    /// Remove and return a finished task's full state. Returns `None` if
    /// the task is missing or still running. Used by the engine-side
    /// watcher right before emitting `BackgroundBashCompleted`.
    pub async fn take_finished(&self, task_id: &str) -> Option<RunningTask> {
        let mut tasks = self.tasks.lock().await;
        if !tasks.get(task_id).is_some_and(|t| t.is_finished()) {
            return None;
        }
        tasks.remove(task_id)
    }

    /// Test helper: poll-wait until the task is marked finished (or
    /// timeout). Production callers don't need this — the dispatch site
    /// uses an event-emitting watcher that drives `take_finished`.
    #[cfg(test)]
    pub async fn wait_for_finish(&self, task_id: &str, timeout: Duration) -> bool {
        let deadline = std::time::Instant::now() + timeout;
        loop {
            {
                let tasks = self.tasks.lock().await;
                if tasks.get(task_id).is_some_and(|t| t.is_finished()) {
                    return true;
                }
            }
            if std::time::Instant::now() >= deadline {
                return false;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }
}

async fn drain_pipe<R: AsyncRead + Unpin + Send + 'static>(
    tasks: Arc<Mutex<HashMap<String, RunningTask>>>,
    task_id: String,
    mut pipe: R,
    is_stderr: bool,
) {
    let mut buf = [0u8; 8192];
    loop {
        match pipe.read(&mut buf).await {
            Ok(0) => return, // EOF — child closed pipe
            Ok(n) => {
                let mut t = tasks.lock().await;
                let Some(task) = t.get_mut(&task_id) else {
                    return;
                };
                let target = if is_stderr {
                    &mut task.stderr
                } else {
                    &mut task.stdout
                };
                target.extend_from_slice(&buf[..n]);
                // Trim only when 2x over cap so the O(N) drain amortizes
                // to O(1) per byte for chatty processes.
                if target.len() > TRIM_TRIGGER_BYTES {
                    let drop = target.len() - MAX_BUFFER_BYTES;
                    target.drain(..drop);
                    let cursor = if is_stderr {
                        &mut task.stderr_cursor
                    } else {
                        &mut task.stdout_cursor
                    };
                    *cursor = cursor.saturating_sub(drop);
                }
            }
            Err(_) => return,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn spawn_returns_task_id_and_output_is_drainable() {
        let reg = BackgroundBashRegistry::new();
        let (task_id, _finish_rx) = reg
            .spawn("echo hi", 5, std::path::Path::new("/tmp"), &[])
            .await
            .expect("spawn");
        let finished = reg.wait_for_finish(&task_id, Duration::from_secs(3)).await;
        assert!(finished, "task did not finish in time");
        let snap = reg.read_output_in_memory(&task_id).await.expect("snapshot");
        assert!(snap.stdout.contains("hi"), "stdout was: {:?}", snap.stdout);
        assert_eq!(snap.exit_code, Some(0));
        assert!(snap.finished);
    }

    #[tokio::test]
    async fn drain_returns_only_new_output_each_call() {
        let reg = BackgroundBashRegistry::new();
        let (task_id, _finish_rx) = reg
            .spawn(
                "echo a; sleep 0.3; echo b",
                5,
                std::path::Path::new("/tmp"),
                &[],
            )
            .await
            .expect("spawn");

        // Wait for "a" to flush.
        tokio::time::sleep(Duration::from_millis(150)).await;
        let first = reg.read_output_in_memory(&task_id).await.expect("first");
        assert!(first.stdout.contains('a'), "first stdout: {:?}", first.stdout);
        assert!(!first.stdout.contains('b'), "first stdout leaked b: {:?}", first.stdout);
        assert!(!first.finished);

        // Wait for finish, then drain again — only "b" should remain.
        let finished = reg.wait_for_finish(&task_id, Duration::from_secs(3)).await;
        assert!(finished);
        let second = reg.read_output_in_memory(&task_id).await.expect("second");
        assert!(second.stdout.contains('b'), "second stdout: {:?}", second.stdout);
        assert!(!second.stdout.contains('a'), "second stdout returned a again: {:?}", second.stdout);
        assert!(second.finished);
    }

    #[tokio::test]
    async fn kill_terminates_running_task() {
        let reg = BackgroundBashRegistry::new();
        let (task_id, _finish_rx) = reg
            .spawn("sleep 30", 60, std::path::Path::new("/tmp"), &[])
            .await
            .expect("spawn");

        // Give the watchdog a moment to start.
        tokio::time::sleep(Duration::from_millis(100)).await;

        let killed = reg.kill(&task_id).await;
        assert!(killed, "kill should return true for a running task");

        let finished = reg.wait_for_finish(&task_id, Duration::from_secs(3)).await;
        assert!(finished, "killed task did not transition to finished");

        let snap = reg.read_output_in_memory(&task_id).await.expect("snapshot");
        assert!(snap.finished);
        assert!(snap.killed, "killed flag should be true");
        assert!(!snap.timed_out);
    }

    #[tokio::test]
    async fn kill_returns_false_for_unknown_task() {
        let reg = BackgroundBashRegistry::new();
        assert!(!reg.kill("does-not-exist").await);
    }

    #[tokio::test]
    async fn timeout_kills_long_running_task() {
        let reg = BackgroundBashRegistry::new();
        let (task_id, _finish_rx) = reg
            .spawn("sleep 30", 1, std::path::Path::new("/tmp"), &[])
            .await
            .expect("spawn");

        let finished = reg.wait_for_finish(&task_id, Duration::from_secs(3)).await;
        assert!(finished, "timeout did not fire");

        let snap = reg.read_output_in_memory(&task_id).await.expect("snapshot");
        assert!(snap.finished);
        assert!(snap.timed_out, "timed_out flag should be true");
        assert!(!snap.killed);
    }

    #[tokio::test]
    async fn take_finished_evicts_completed_task() {
        let reg = BackgroundBashRegistry::new();
        let (task_id, _finish_rx) = reg
            .spawn("echo done", 5, std::path::Path::new("/tmp"), &[])
            .await
            .expect("spawn");
        let finished = reg.wait_for_finish(&task_id, Duration::from_secs(3)).await;
        assert!(finished);

        let taken = reg.take_finished(&task_id).await.expect("take_finished");
        assert!(taken.is_finished());
        assert!(taken.stdout.windows(4).any(|w| w == b"done"));

        // Second read should now miss — task evicted.
        assert!(reg.read_output_in_memory(&task_id).await.is_none());
        assert!(reg.take_finished(&task_id).await.is_none());
    }

    #[tokio::test]
    async fn killed_task_preserves_output_written_before_kill() {
        // Regression for the drain-vs-evict race: the watchdog now
        // joins the drain tasks before signaling finish, so output
        // written before kill is still available in the final snapshot
        // even if the kill arrives moments later.
        let reg = BackgroundBashRegistry::new();
        let (task_id, _finish_rx) = reg
            .spawn(
                "echo hello-before-kill; sleep 30",
                60,
                std::path::Path::new("/tmp"),
                &[],
            )
            .await
            .expect("spawn");

        // Wait for the echo to flush, then kill mid-sleep.
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(reg.kill(&task_id).await);

        let finished = reg.wait_for_finish(&task_id, Duration::from_secs(8)).await;
        assert!(finished, "task did not finish within 8s after kill");

        let taken = reg
            .take_finished(&task_id)
            .await
            .expect("take_finished after kill");
        let out = String::from_utf8_lossy(&taken.stdout);
        assert!(
            out.contains("hello-before-kill"),
            "stdout from before kill was lost: {:?}",
            out
        );
        assert!(taken.killed);
    }

    #[tokio::test]
    async fn take_finished_returns_none_while_running() {
        let reg = BackgroundBashRegistry::new();
        let (task_id, _finish_rx) = reg
            .spawn("sleep 5", 60, std::path::Path::new("/tmp"), &[])
            .await
            .expect("spawn");
        // Don't wait — task is still running.
        assert!(reg.take_finished(&task_id).await.is_none());
        // Clean up so the spawned sleep doesn't outlive the test thread.
        reg.kill(&task_id).await;
    }
}
