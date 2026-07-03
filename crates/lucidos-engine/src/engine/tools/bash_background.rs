//! In-memory registry for `run_bash_background` tasks. Holds only
//! currently-running tasks; the dispatch site emits
//! `BackgroundBashCompleted` and evicts on completion.

use chrono::{DateTime, Utc};
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::sync::{Mutex, Notify};
use uuid::Uuid;

/// Maximum `wait_secs` honored by `bash_output(wait_secs=…)`. Keeps a
/// poorly-prompted agent from pinning a model turn for half an hour on
/// what is conceptually a non-blocking drain. 120 s is long enough for a
/// short-running bg task to finish during the wait without the LLM-side
/// HTTP timeout firing.
pub const BASH_OUTPUT_MAX_WAIT_SECS: u32 = 120;

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
    /// Thread that spawned this task. `None` for tests and engine-internal
    /// callers with no owning thread. Drives `has_running_for_thread` so the
    /// agent-session idle handler can keep CC alive while bg bash is still
    /// running for its thread — without this, a CC that idled mid-/harden
    /// waiting on its own `run_bash_background` tests was killed and the
    /// change auto-proposed as "done", which caused premature Apply + harden-
    /// from-scratch on click.
    pub(super) thread_id: Option<Uuid>,
    kill_signal: Option<tokio::sync::oneshot::Sender<()>>,
    /// Signals every new buffered chunk AND the final finished_at write.
    /// `read_output_in_memory_wait` clones this and awaits it under a
    /// timeout so an LLM doing `bash_output(task_id, wait_secs=N)` wakes
    /// the moment new output arrives instead of having to sleep-poll
    /// from a fresh `run_python` (the antipattern this notifier exists
    /// to kill). Both drain and watchdog use notify_one — its permit
    /// semantics close the race where a wake that fires between the
    /// caller's lock-drop and the future's first poll would otherwise
    /// be lost (notify_waiters has no permit, so it requires a waiter
    /// to be already registered).
    pub(super) notify: Arc<Notify>,
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
    ///
    /// `thread_id` records the spawning thread so `has_running_for_thread`
    /// can answer "does this thread still have unfinished background bash?".
    /// Production callers (the `run_bash_background` LLM tool) always pass
    /// `Some(thread_id)`; tests typically pass `None`.
    pub async fn spawn(
        &self,
        command: &str,
        timeout_secs: u64,
        cwd: &Path,
        env: &[(String, String)],
        thread_id: Option<Uuid>,
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
                    thread_id,
                    kill_signal: Some(kill_tx),
                    notify: Arc::new(Notify::new()),
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
                // notify_one (not notify_waiters) so the wake is durable:
                // notify_one stores a permit if no waiter is yet
                // registered, and the next `notified().await` consumes
                // it. notify_waiters has no permit semantics — a wake
                // that fires between a caller's lock-drop (line above
                // the await in read_output_in_memory_wait) and the
                // future's first poll would be lost, blocking the LLM
                // for the full wait_secs on a task that already
                // finished. The chat-agent path has exactly one waiter
                // per task, so notify_one is sufficient AND race-free;
                // additional waiters (rare — concurrent introspection)
                // fall back to the under-lock is_finished() check on
                // their next call.
                task.notify.notify_one();
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
    ///
    /// With `wait > ZERO`, blocks server-side until new output arrives
    /// OR the task finishes OR the wait elapses, then drains and returns.
    /// Returns immediately when (a) output is already buffered, (b) the
    /// task is already finished, or (c) `wait == ZERO`. On timeout returns
    /// whatever's there (possibly empty stdout/stderr with `finished=false`)
    /// — same shape, no error.
    ///
    /// The wait path exists so the LLM-facing `bash_output(wait_secs=…)`
    /// can replace the antipattern of "spawn `run_python_background`,
    /// then poll by issuing a fresh `run_python` containing
    /// `time.sleep(N)`". The chat agent observed in `dev` workspace
    /// burned 5+ wasted tool calls per backtest waiting like this;
    /// server-side blocking collapses the cycle to one drain call.
    pub async fn read_output_in_memory_wait(
        &self,
        task_id: &str,
        wait: Duration,
    ) -> Option<OutputSnapshot> {
        // First check under the lock: is there already output / has the
        // task finished? If so, drain and return without waiting.
        let notify = {
            let mut tasks = self.tasks.lock().await;
            let task = tasks.get_mut(task_id)?;
            let has_new =
                task.stdout_cursor < task.stdout.len() || task.stderr_cursor < task.stderr.len();
            if has_new || task.is_finished() || wait.is_zero() {
                return Some(drain_snapshot(task));
            }
            task.notify.clone()
        };
        // Wait for the next notify_one — fires both from drain (a
        // chunk arrived) and from the watchdog (task finished). Both
        // paths use notify_one so the wake is durable across the
        // lock-drop/await gap (notify_one stores a permit; the next
        // notified().await consumes it). On timeout, fall through and
        // return whatever's there.
        let _ = tokio::time::timeout(wait, notify.notified()).await;
        let mut tasks = self.tasks.lock().await;
        let task = tasks.get_mut(task_id)?;
        Some(drain_snapshot(task))
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

    /// True iff the registry holds at least one unfinished task that was
    /// spawned for this thread. The agent-session idle handler reads this
    /// to decide whether to skip the propose-and-terminate dance at idle —
    /// a CC subprocess that idled mid-/harden waiting on its own
    /// `run_bash_background` tests must stay alive until the tests finish,
    /// otherwise the change auto-proposes as "done" (premature Apply) and
    /// a click triggers a from-scratch harden in a fresh worktree.
    pub async fn has_running_for_thread(&self, thread_id: Uuid) -> bool {
        let tasks = self.tasks.lock().await;
        tasks
            .values()
            .any(|t| t.thread_id == Some(thread_id) && !t.is_finished())
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

/// Drain the per-task buffers since the previous cursor and advance
/// the cursors. Shared by zero-wait and wait paths in
/// `read_output_in_memory_wait` so cursor semantics live in exactly
/// one place.
fn drain_snapshot(task: &mut RunningTask) -> OutputSnapshot {
    let new_stdout = String::from_utf8_lossy(&task.stdout[task.stdout_cursor..]).to_string();
    let new_stderr = String::from_utf8_lossy(&task.stderr[task.stderr_cursor..]).to_string();
    task.stdout_cursor = task.stdout.len();
    task.stderr_cursor = task.stderr.len();
    OutputSnapshot {
        stdout: new_stdout,
        stderr: new_stderr,
        exit_code: task.exit_code,
        finished: task.is_finished(),
        timed_out: task.timed_out,
        killed: task.killed,
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
                task.notify.notify_one();
            }
            // Pipe read error — the child closed its end or the OS dropped
            // the fd. The reader task is done; the watchdog will surface the
            // exit code (or timeout) as the user-visible signal.
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
            .spawn("echo hi", 5, std::path::Path::new("/tmp"), &[], None)
            .await
            .expect("spawn");
        let finished = reg.wait_for_finish(&task_id, Duration::from_secs(3)).await;
        assert!(finished, "task did not finish in time");
        let snap = reg.read_output_in_memory_wait(&task_id, Duration::ZERO).await.expect("snapshot");
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
                None,
            )
            .await
            .expect("spawn");

        // Wait for "a" to flush.
        tokio::time::sleep(Duration::from_millis(150)).await;
        let first = reg.read_output_in_memory_wait(&task_id, Duration::ZERO).await.expect("first");
        assert!(first.stdout.contains('a'), "first stdout: {:?}", first.stdout);
        assert!(!first.stdout.contains('b'), "first stdout leaked b: {:?}", first.stdout);
        assert!(!first.finished);

        // Wait for finish, then drain again — only "b" should remain.
        let finished = reg.wait_for_finish(&task_id, Duration::from_secs(3)).await;
        assert!(finished);
        let second = reg.read_output_in_memory_wait(&task_id, Duration::ZERO).await.expect("second");
        assert!(second.stdout.contains('b'), "second stdout: {:?}", second.stdout);
        assert!(!second.stdout.contains('a'), "second stdout returned a again: {:?}", second.stdout);
        assert!(second.finished);
    }

    #[tokio::test]
    async fn kill_terminates_running_task() {
        let reg = BackgroundBashRegistry::new();
        let (task_id, _finish_rx) = reg
            .spawn("sleep 30", 60, std::path::Path::new("/tmp"), &[], None)
            .await
            .expect("spawn");

        // Give the watchdog a moment to start.
        tokio::time::sleep(Duration::from_millis(100)).await;

        let killed = reg.kill(&task_id).await;
        assert!(killed, "kill should return true for a running task");

        let finished = reg.wait_for_finish(&task_id, Duration::from_secs(3)).await;
        assert!(finished, "killed task did not transition to finished");

        let snap = reg.read_output_in_memory_wait(&task_id, Duration::ZERO).await.expect("snapshot");
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
            .spawn("sleep 30", 1, std::path::Path::new("/tmp"), &[], None)
            .await
            .expect("spawn");

        let finished = reg.wait_for_finish(&task_id, Duration::from_secs(3)).await;
        assert!(finished, "timeout did not fire");

        let snap = reg.read_output_in_memory_wait(&task_id, Duration::ZERO).await.expect("snapshot");
        assert!(snap.finished);
        assert!(snap.timed_out, "timed_out flag should be true");
        assert!(!snap.killed);
    }

    #[tokio::test]
    async fn take_finished_evicts_completed_task() {
        let reg = BackgroundBashRegistry::new();
        let (task_id, _finish_rx) = reg
            .spawn("echo done", 5, std::path::Path::new("/tmp"), &[], None)
            .await
            .expect("spawn");
        let finished = reg.wait_for_finish(&task_id, Duration::from_secs(3)).await;
        assert!(finished);

        let taken = reg.take_finished(&task_id).await.expect("take_finished");
        assert!(taken.is_finished());
        assert!(taken.stdout.windows(4).any(|w| w == b"done"));

        // Second read should now miss — task evicted.
        assert!(reg.read_output_in_memory_wait(&task_id, Duration::ZERO).await.is_none());
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
                None,
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
            .spawn("sleep 5", 60, std::path::Path::new("/tmp"), &[], None)
            .await
            .expect("spawn");
        // Don't wait — task is still running.
        assert!(reg.take_finished(&task_id).await.is_none());
        // Clean up so the spawned sleep doesn't outlive the test thread.
        reg.kill(&task_id).await;
    }

    /// `has_running_for_thread` is the core hook the agent-session idle
    /// handler uses to decide whether to skip propose+terminate. The pre-
    /// fix bug was the engine treating a CC that idled while its own
    /// `run_bash_background` tests were still running as "CC is done":
    /// the change auto-proposed, CC was killed, and an Apply click then
    /// fell through `apply_now`'s stale-session fallback into a fresh
    /// /harden-from-scratch in a `harden-*` worktree. With per-task
    /// thread_id tracking, the idle handler can ask the registry "does
    /// my thread still have unfinished bg bash?" and keep CC alive +
    /// suppress the propose until the bash actually completes.
    #[tokio::test]
    async fn has_running_for_thread_tracks_unfinished_tasks_per_thread() {
        let reg = BackgroundBashRegistry::new();
        let my_thread = Uuid::new_v4();
        let other_thread = Uuid::new_v4();

        assert!(
            !reg.has_running_for_thread(my_thread).await,
            "empty registry must report no running tasks for any thread"
        );

        let (task_id, _finish_rx) = reg
            .spawn(
                "sleep 30",
                60,
                std::path::Path::new("/tmp"),
                &[],
                Some(my_thread),
            )
            .await
            .expect("spawn");

        assert!(
            reg.has_running_for_thread(my_thread).await,
            "running task spawned for my_thread must be visible to has_running_for_thread"
        );
        assert!(
            !reg.has_running_for_thread(other_thread).await,
            "running task spawned for my_thread must NOT leak to other_thread"
        );

        // Kill + drain → registry evicts → has_running_for_thread flips back.
        assert!(reg.kill(&task_id).await);
        let finished = reg.wait_for_finish(&task_id, Duration::from_secs(3)).await;
        assert!(finished);
        let _ = reg.take_finished(&task_id).await;

        assert!(
            !reg.has_running_for_thread(my_thread).await,
            "evicted task must NOT keep registering as running"
        );
    }

    /// Tasks spawned without a thread_id (test fixtures, engine-internal
    /// jobs) must never appear under `has_running_for_thread` for any
    /// thread — otherwise a stray engine job would falsely keep an
    /// unrelated CC alive.
    #[tokio::test]
    async fn has_running_for_thread_ignores_tasks_with_no_thread_id() {
        let reg = BackgroundBashRegistry::new();
        let some_thread = Uuid::new_v4();

        let (task_id, _finish_rx) = reg
            .spawn("sleep 30", 60, std::path::Path::new("/tmp"), &[], None)
            .await
            .expect("spawn");

        assert!(
            !reg.has_running_for_thread(some_thread).await,
            "task with thread_id=None must NOT register under any thread"
        );

        reg.kill(&task_id).await;
    }

    // -----------------------------------------------------------------
    // read_output_in_memory_wait — `bash_output(wait_secs)` backbone.
    //
    // These tests pin the four behaviours the chat-agent side of the
    // sleep-poll fix depends on. They use the in-memory registry
    // directly (no tool dispatch) so a regression shows up here long
    // before it reaches the LLM-facing tool description.
    // -----------------------------------------------------------------

    #[tokio::test]
    async fn wait_returns_immediately_when_output_already_buffered() {
        let reg = BackgroundBashRegistry::new();
        let (task_id, _finish_rx) = reg
            .spawn("echo eager", 5, std::path::Path::new("/tmp"), &[], None)
            .await
            .expect("spawn");
        // Let the echo flush.
        let finished = reg.wait_for_finish(&task_id, Duration::from_secs(3)).await;
        assert!(finished);

        let start = std::time::Instant::now();
        let snap = reg
            .read_output_in_memory_wait(&task_id, Duration::from_secs(30))
            .await
            .expect("snapshot");
        let elapsed = start.elapsed();
        assert!(snap.stdout.contains("eager"), "stdout: {:?}", snap.stdout);
        assert!(snap.finished);
        assert!(
            elapsed < Duration::from_millis(500),
            "wait should not block when output is already buffered (took {:?})",
            elapsed
        );
    }

    #[tokio::test]
    async fn wait_wakes_when_output_arrives_during_wait() {
        let reg = BackgroundBashRegistry::new();
        // Stay alive long enough that the wait blocks on an empty
        // buffer, then emit something the wait must wake on.
        let (task_id, _finish_rx) = reg
            .spawn(
                "sleep 0.3; echo arrived; sleep 5",
                30,
                std::path::Path::new("/tmp"),
                &[],
                None,
            )
            .await
            .expect("spawn");

        let start = std::time::Instant::now();
        let snap = reg
            .read_output_in_memory_wait(&task_id, Duration::from_secs(3))
            .await
            .expect("snapshot");
        let elapsed = start.elapsed();
        assert!(
            snap.stdout.contains("arrived"),
            "stdout should contain woken-up output: {:?}",
            snap.stdout
        );
        assert!(
            !snap.finished,
            "task is still sleeping — finished must be false"
        );
        assert!(
            elapsed < Duration::from_secs(2),
            "wait should wake on output, not run the full timeout (took {:?})",
            elapsed
        );

        // Cleanup so the trailing sleep doesn't outlive the test.
        reg.kill(&task_id).await;
    }

    #[tokio::test]
    async fn wait_wakes_when_task_finishes_with_empty_output() {
        // A task that produces NO output during the wait should still
        // wake the waiter when it finishes — otherwise the LLM would
        // sit on the full wait_secs ceiling for every silent-then-done
        // task. This is the notify_one-on-finish path; the simplify
        // pass switched from notify_waiters to notify_one specifically
        // so this case is durable across the lock-drop/await gap.
        let reg = BackgroundBashRegistry::new();
        let (task_id, _finish_rx) = reg
            .spawn(
                "sleep 0.3",
                5,
                std::path::Path::new("/tmp"),
                &[],
                None,
            )
            .await
            .expect("spawn");

        let start = std::time::Instant::now();
        let snap = reg
            .read_output_in_memory_wait(&task_id, Duration::from_secs(3))
            .await
            .expect("snapshot");
        let elapsed = start.elapsed();
        assert!(snap.finished, "wait must wake when the task finishes");
        assert_eq!(snap.exit_code, Some(0));
        assert!(
            elapsed < Duration::from_secs(2),
            "wait should wake on finish, not run the full timeout (took {:?})",
            elapsed
        );
    }

    #[tokio::test]
    async fn wait_timeout_returns_empty_snapshot_without_error() {
        // A task with no output during the wait window must NOT error —
        // the LLM should get back a normal empty snapshot with
        // finished=false so it knows to keep polling (or give up).
        let reg = BackgroundBashRegistry::new();
        let (task_id, _finish_rx) = reg
            .spawn("sleep 30", 60, std::path::Path::new("/tmp"), &[], None)
            .await
            .expect("spawn");

        let start = std::time::Instant::now();
        let snap = reg
            .read_output_in_memory_wait(&task_id, Duration::from_millis(300))
            .await
            .expect("snapshot");
        let elapsed = start.elapsed();
        assert!(snap.stdout.is_empty(), "stdout: {:?}", snap.stdout);
        assert!(snap.stderr.is_empty(), "stderr: {:?}", snap.stderr);
        assert!(!snap.finished, "task is still sleeping");
        assert!(snap.exit_code.is_none());
        assert!(
            elapsed >= Duration::from_millis(250),
            "wait should hold the full window when nothing arrives (took {:?})",
            elapsed
        );
        assert!(
            elapsed < Duration::from_secs(2),
            "wait should not overshoot its budget (took {:?})",
            elapsed
        );

        reg.kill(&task_id).await;
    }

    /// Two concurrent `bash_output(wait_secs)` callers on the same
    /// task_id. The chat-agent doesn't normally do this, but a
    /// recovery sweep or a manual introspection probe could land in
    /// parallel with an in-flight drain. Pin the documented contract:
    /// both waiters eventually wake (either on a chunk, on finish, or
    /// on timeout) and never deadlock; the first to acquire the lock
    /// after wake gets that round's bytes, the other re-enters and
    /// sees `finished=true` once the task ends.
    #[tokio::test]
    async fn two_concurrent_waiters_on_same_task_both_eventually_return() {
        let reg = BackgroundBashRegistry::new();
        let (task_id, _finish_rx) = reg
            .spawn(
                "echo first; sleep 0.3; echo second",
                10,
                std::path::Path::new("/tmp"),
                &[],
                None,
            )
            .await
            .expect("spawn");

        let reg_a = reg.clone();
        let task_a = task_id.clone();
        let h_a = tokio::spawn(async move {
            reg_a
                .read_output_in_memory_wait(&task_a, Duration::from_secs(3))
                .await
        });
        let reg_b = reg.clone();
        let task_b = task_id.clone();
        let h_b = tokio::spawn(async move {
            reg_b
                .read_output_in_memory_wait(&task_b, Duration::from_secs(3))
                .await
        });

        let (snap_a, snap_b) = tokio::join!(h_a, h_b);
        let snap_a = snap_a.unwrap().expect("waiter A returned None");
        let snap_b = snap_b.unwrap().expect("waiter B returned None");

        // Neither waiter deadlocked / hung — both returned within the
        // 3 s ceiling. At least one of them saw output; the other may
        // have seen empty (its peer drained first) and would return
        // on the next chunk / finish.
        let combined = format!("{}{}", snap_a.stdout, snap_b.stdout);
        assert!(
            combined.contains("first") || combined.contains("second"),
            "at least one waiter must see output across the race: a={:?} b={:?}",
            snap_a.stdout, snap_b.stdout
        );
    }

    #[tokio::test]
    async fn wait_zero_matches_legacy_non_blocking_drain() {
        // wait=ZERO must be a pure non-blocking drain — same shape,
        // same cursor advance, no awaiting. This pins the back-compat
        // path the existing callers (bash_output without wait_secs)
        // depend on.
        let reg = BackgroundBashRegistry::new();
        let (task_id, _finish_rx) = reg
            .spawn("echo a; sleep 30", 60, std::path::Path::new("/tmp"), &[], None)
            .await
            .expect("spawn");

        // Wait for "a" to flush into the buffer.
        tokio::time::sleep(Duration::from_millis(150)).await;

        let start = std::time::Instant::now();
        let snap = reg
            .read_output_in_memory_wait(&task_id, Duration::ZERO)
            .await
            .expect("snapshot");
        let elapsed = start.elapsed();
        assert!(snap.stdout.contains('a'), "stdout: {:?}", snap.stdout);
        assert!(!snap.finished);
        assert!(
            elapsed < Duration::from_millis(100),
            "wait=ZERO must not block (took {:?})",
            elapsed
        );

        // Second drain returns empty — cursor advanced like the
        // non-wait path does.
        let snap2 = reg
            .read_output_in_memory_wait(&task_id, Duration::ZERO)
            .await
            .expect("snapshot 2");
        assert!(snap2.stdout.is_empty(), "stdout 2: {:?}", snap2.stdout);

        reg.kill(&task_id).await;
    }
}
