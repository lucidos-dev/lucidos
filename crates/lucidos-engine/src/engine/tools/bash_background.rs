//! In-memory registry for `run_bash_background` tasks. Holds only
//! currently-running tasks; the dispatch site emits
//! `BackgroundBashCompleted` and evicts on completion.

use crate::core::shell::{command_shell, TaskOutcome};
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

/// One captured output stream: the retained bytes, how far the reader has
/// consumed, and what the buffer cap threw away.
///
/// The two loss counters exist because a *drain* and the *completion event*
/// ask different questions. A drain shows the window since the last read, so
/// it needs the bytes lost from that window; the completion event shows the
/// whole retained buffer, so it needs everything the cap ever cut. Reporting
/// one where the other belongs understates the loss, and a truncation marker
/// that understates is worse than none — it reads as a bound.
#[derive(Default)]
pub(super) struct Stream {
    bytes: Vec<u8>,
    /// Bytes already handed to a reader.
    cursor: usize,
    /// Not-yet-read bytes the cap discarded since the last drain. Reset by
    /// [`Stream::drain`].
    dropped_unread: usize,
    /// Every byte the cap has discarded, for the lifetime of the task.
    trimmed_total: usize,
}

impl Stream {
    /// Append a chunk, trimming the front once the buffer runs 2× over cap so
    /// the per-byte cost of `Vec::drain` amortizes to O(1) on chatty processes.
    fn push(&mut self, chunk: &[u8]) {
        self.bytes.extend_from_slice(chunk);
        if self.bytes.len() <= TRIM_TRIGGER_BYTES {
            return;
        }
        let drop = self.bytes.len() - MAX_BUFFER_BYTES;
        self.bytes.drain(..drop);
        self.trimmed_total += drop;
        // Anything past the cursor was never delivered to a reader — that
        // part of the trim is real, reportable data loss.
        self.dropped_unread += drop.saturating_sub(self.cursor);
        self.cursor = self.cursor.saturating_sub(drop);
    }

    /// Take everything since the last read, plus the count of unread bytes
    /// lost to trimming in that same span.
    fn drain(&mut self) -> (String, usize) {
        let text = String::from_utf8_lossy(&self.bytes[self.cursor..]).to_string();
        self.cursor = self.bytes.len();
        (text, std::mem::take(&mut self.dropped_unread))
    }

    /// The whole retained buffer and everything the cap ever cut from it.
    /// Used for the final `BackgroundBashCompleted` record.
    pub(super) fn all(&self) -> (String, usize) {
        (
            String::from_utf8_lossy(&self.bytes).to_string(),
            self.trimmed_total,
        )
    }
}

pub struct RunningTask {
    pub(super) started_at: DateTime<Utc>,
    pub(super) stdout: Stream,
    pub(super) stderr: Stream,
    /// How the child ended. `None` until the watchdog writes it, in the same
    /// locked block as `finished_at` — so `finished_at.is_some()` and
    /// `outcome.is_some()` are always in step, and a still-running task can
    /// never present a status at all (as opposed to presenting a `0`).
    pub(super) outcome: Option<TaskOutcome>,
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
    /// Signals the final `finished_at` write, and ONLY that — a buffered
    /// chunk deliberately does not wake the waiter. `bash_output(wait_secs=N)`
    /// means "block up to N seconds"; waking on the first byte made the
    /// wait a no-op for any chatty task (a cargo build, `notarytool`, an
    /// npm install all emit output every few hundred ms), which is exactly
    /// the sleep-poll burn this was built to end — one release thread spent
    /// 172 `bash_output` calls in 20 minutes, each returning in 2-3 s.
    /// The watchdog uses `notify_waiters`, which wakes every parked waiter
    /// but stores no permit. Waiters close that gap themselves by registering
    /// before re-reading `finished_at` — see `read_output_in_memory_wait`.
    pub(super) finish_notify: Arc<Notify>,
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
    /// Unread bytes the buffer cap discarded within this window. Non-zero
    /// only for a task chatty enough to overrun the cap between two drains —
    /// which a full-budget `wait_secs` block makes possible where a
    /// wake-on-every-chunk drain never could. The truncation marker adds
    /// these in, so it can't quietly report less loss than occurred.
    pub stdout_dropped: usize,
    pub stderr_dropped: usize,
    /// How the child ended, or `None` while it is still running. Carrying the
    /// typed outcome rather than a loose `exit_code: Option<i32>` is what makes
    /// "exited 0" and "we don't know" un-confusable — callers project it to the
    /// wire via `TaskOutcome::{exit_code, signal, describe}` instead of each
    /// inventing their own fallback.
    pub outcome: Option<TaskOutcome>,
    pub finished: bool,
    pub timed_out: bool,
    pub killed: bool,
    /// Wall-clock seconds since the task was spawned (its total runtime once
    /// finished). An LLM has no clock of its own: given only a stream of
    /// drains it infers elapsed time from how long it *asked* to wait, and
    /// reports "roughly 20 minutes in Apple's queue" 90 seconds in. This is
    /// the ground truth that makes that guess unnecessary.
    pub elapsed_secs: i64,
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
    ) -> Result<
        (String, tokio::sync::oneshot::Receiver<()>),
        Box<dyn std::error::Error + Send + Sync>,
    > {
        let task_id = Uuid::new_v4().to_string();

        // Built through `command_shell()` so the command runs under `pipefail`
        // — without it a `… | tee build.log` reports tee's 0 and a failing
        // build reaches the LLM as a clean success. See `core::shell`.
        let mut cmd = command_shell().command(command);
        cmd.current_dir(cwd)
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
                    stdout: Stream::default(),
                    stderr: Stream::default(),
                    outcome: None,
                    timed_out: false,
                    killed: false,
                    finished_at: None,
                    thread_id,
                    kill_signal: Some(kill_tx),
                    finish_notify: Arc::new(Notify::new()),
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
            // Every arm reaps the child and classifies the REAL status. The
            // timeout and kill arms used to throw it away and report `None`,
            // which left "the watchdog SIGKILLed it" indistinguishable from
            // "wait() failed". `child.kill()` already awaits the child, and
            // tokio caches the status, so the follow-up `wait()` returns it.
            let outcome: TaskOutcome = tokio::select! {
                exit = child.wait() => TaskOutcome::from_wait(exit),
                _ = &mut timeout_fut => {
                    timed_out = true;
                    let _ = child.kill().await;
                    TaskOutcome::from_wait(child.wait().await)
                }
                _ = kill_rx => {
                    killed = true;
                    let _ = child.kill().await;
                    TaskOutcome::from_wait(child.wait().await)
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
                task.outcome = Some(outcome);
                task.timed_out = timed_out;
                task.killed = killed;
                task.finished_at = Some(Utc::now());
                task.kill_signal = None;
                // Wakes every parked waiter. A finish that lands before a
                // waiter registers is NOT lost: `read_output_in_memory_wait`
                // registers first and re-checks `is_finished()` under the
                // lock afterwards, so each waiter either sees the flag or is
                // already parked to be woken here. That re-check is what makes
                // this correct for N waiters — a `notify_one` permit would
                // rescue exactly one of them and strand the rest for their
                // full budget on an already-finished task.
                task.finish_notify.notify_waiters();
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
    /// With `wait > ZERO`, blocks server-side for the FULL `wait` unless
    /// the task finishes first, then drains everything that accumulated
    /// and returns. Returns immediately only when the task is already
    /// finished or `wait == ZERO`. On timeout returns whatever's there
    /// (possibly empty stdout/stderr with `finished=false`) — same shape,
    /// no error.
    ///
    /// **Buffered output is not a reason to cut the wait short.** That was
    /// the original behaviour and it made `wait_secs` a no-op for exactly
    /// the tasks it exists for: anything chatty (a cargo build, `notarytool`,
    /// an npm install) has new bytes within milliseconds, so every
    /// `wait_secs=120` returned instantly and the agent re-polled forever.
    /// A single release thread logged 172 `bash_output` calls in 20 minutes,
    /// 51 of them 2 seconds apart. "Block up to N seconds" now means it.
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
        // First check under the lock: is the task already finished (or is
        // this the legacy non-blocking drain)? If so, drain and return.
        let finish_notify = {
            let mut tasks = self.tasks.lock().await;
            let task = tasks.get_mut(task_id)?;
            if task.is_finished() || wait.is_zero() {
                return Some(drain_snapshot(task));
            }
            task.finish_notify.clone()
        };
        // Register BEFORE re-checking, then re-check under the lock. The
        // watchdog fires `notify_waiters`, which has no permit to fall back
        // on, so a finish landing in the gap above would be lost to a waiter
        // that hasn't registered yet. Registering first and then re-reading
        // the flag leaves no gap: the finish is either already visible here
        // or still to come, and by then we are parked to receive it. Every
        // concurrent waiter runs this, so all of them are covered — a
        // single-permit wake would rescue one and strand the others for
        // their full budget on a task that is already done.
        let notified = finish_notify.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        {
            let mut tasks = self.tasks.lock().await;
            let task = tasks.get_mut(task_id)?;
            if task.is_finished() {
                return Some(drain_snapshot(task));
            }
        }
        // Park until the watchdog signals finish, or the budget runs out.
        // On timeout, fall through and return whatever accumulated.
        let _ = tokio::time::timeout(wait, notified).await;
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
    let (stdout, stdout_dropped) = task.stdout.drain();
    let (stderr, stderr_dropped) = task.stderr.drain();
    // Measure to `finished_at` once the task is done so a late drain reports
    // the task's runtime, not "how long ago it was spawned".
    let until = task.finished_at.unwrap_or_else(Utc::now);
    OutputSnapshot {
        stdout,
        stderr,
        stdout_dropped,
        stderr_dropped,
        outcome: task.outcome,
        finished: task.is_finished(),
        timed_out: task.timed_out,
        killed: task.killed,
        elapsed_secs: (until - task.started_at).num_seconds().max(0),
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
                if is_stderr {
                    &mut task.stderr
                } else {
                    &mut task.stdout
                }
                .push(&buf[..n]);
                // Deliberately no notify here: a buffered chunk must not end
                // a `bash_output(wait_secs=N)` block. See `finish_notify`.
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
        let snap = reg
            .read_output_in_memory_wait(&task_id, Duration::ZERO)
            .await
            .expect("snapshot");
        assert!(snap.stdout.contains("hi"), "stdout was: {:?}", snap.stdout);
        assert_eq!(snap.outcome, Some(TaskOutcome::Exited(0)));
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
        let first = reg
            .read_output_in_memory_wait(&task_id, Duration::ZERO)
            .await
            .expect("first");
        assert!(
            first.stdout.contains('a'),
            "first stdout: {:?}",
            first.stdout
        );
        assert!(
            !first.stdout.contains('b'),
            "first stdout leaked b: {:?}",
            first.stdout
        );
        assert!(!first.finished);

        // Wait for finish, then drain again — only "b" should remain.
        let finished = reg.wait_for_finish(&task_id, Duration::from_secs(3)).await;
        assert!(finished);
        let second = reg
            .read_output_in_memory_wait(&task_id, Duration::ZERO)
            .await
            .expect("second");
        assert!(
            second.stdout.contains('b'),
            "second stdout: {:?}",
            second.stdout
        );
        assert!(
            !second.stdout.contains('a'),
            "second stdout returned a again: {:?}",
            second.stdout
        );
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

        let snap = reg
            .read_output_in_memory_wait(&task_id, Duration::ZERO)
            .await
            .expect("snapshot");
        assert!(snap.finished);
        assert!(snap.killed, "killed flag should be true");
        assert!(!snap.timed_out);
        // The kill arm used to discard the reaped status and report `None`.
        // It now records the SIGKILL it actually sent, so the summary can say
        // *how* the child died rather than just that the engine asked it to.
        assert_eq!(snap.outcome, Some(TaskOutcome::Signaled(9)));
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

        let snap = reg
            .read_output_in_memory_wait(&task_id, Duration::ZERO)
            .await
            .expect("snapshot");
        assert!(snap.finished);
        assert!(snap.timed_out, "timed_out flag should be true");
        assert!(!snap.killed);
        // Same as the kill path: the watchdog's own SIGKILL is now recorded
        // instead of being dropped on the floor.
        assert_eq!(snap.outcome, Some(TaskOutcome::Signaled(9)));
        assert_eq!(
            snap.outcome.unwrap().exit_code(),
            None,
            "a timed-out task must never present an exit code"
        );
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
        assert!(taken.stdout.all().0.contains("done"));

        // Second read should now miss — task evicted.
        assert!(reg
            .read_output_in_memory_wait(&task_id, Duration::ZERO)
            .await
            .is_none());
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
        let out = taken.stdout.all().0;
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
    async fn wait_returns_immediately_when_task_already_finished() {
        // Finished is the ONE reason to cut a wait short — there is
        // nothing left to wait for. Buffered output is not (see
        // `wait_holds_full_budget_while_output_keeps_arriving`).
        let reg = BackgroundBashRegistry::new();
        let (task_id, _finish_rx) = reg
            .spawn("echo eager", 5, std::path::Path::new("/tmp"), &[], None)
            .await
            .expect("spawn");
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
            "a finished task must not hold the wait (took {:?})",
            elapsed
        );
    }

    #[tokio::test]
    async fn wait_holds_full_budget_while_output_keeps_arriving() {
        // THE regression test for the polling storm. A chatty task emits
        // something every ~100 ms; the old wake-on-chunk wait returned in
        // milliseconds, so `wait_secs=120` was a no-op and the agent
        // re-polled hundreds of times (172 calls in one release thread,
        // 51 of them 2 s apart). The wait must now hold its full budget
        // and hand back everything that accumulated in one go.
        let reg = BackgroundBashRegistry::new();
        let (task_id, _finish_rx) = reg
            .spawn(
                "for i in $(seq 1 100); do echo line$i; sleep 0.1; done",
                30,
                std::path::Path::new("/tmp"),
                &[],
                None,
            )
            .await
            .expect("spawn");

        let start = std::time::Instant::now();
        let snap = reg
            .read_output_in_memory_wait(&task_id, Duration::from_millis(700))
            .await
            .expect("snapshot");
        let elapsed = start.elapsed();
        assert!(
            elapsed >= Duration::from_millis(650),
            "chatty output must NOT cut the wait short (took {:?})",
            elapsed
        );
        assert!(
            !snap.finished,
            "task is still looping — finished must be false"
        );
        // One call collected the whole window, not just the first line.
        let lines = snap.stdout.lines().filter(|l| !l.is_empty()).count();
        assert!(
            lines >= 3,
            "one drain should return the whole window, got {} line(s): {:?}",
            lines,
            snap.stdout
        );

        reg.kill(&task_id).await;
    }

    #[test]
    fn stream_reports_unread_bytes_the_cap_discarded() {
        // Reachable only now that a wait holds its full budget: a task chatty
        // enough to overrun the 2 MB buffer between two drains loses the
        // un-read prefix. Silent loss is the bug — the drain has to hand the
        // count up so the truncation marker states the real gap.
        let mut s = Stream::default();
        s.push(&vec![b'a'; TRIM_TRIGGER_BYTES + 4096]);

        let (text, dropped) = s.drain();
        assert!(dropped > 0, "the trim discarded unread bytes, unreported");
        assert_eq!(
            dropped + text.len(),
            TRIM_TRIGGER_BYTES + 4096,
            "every byte written is either returned or counted as dropped"
        );
        // Reset per drain — the next window reports only its own loss.
        s.push(b"quiet");
        assert_eq!(s.drain(), ("quiet".to_string(), 0));
    }

    #[test]
    fn stream_trim_of_already_read_bytes_is_not_unread_loss() {
        // Trimming bytes the reader already consumed costs nothing, and
        // counting them would overstate — a marker crying loss on every
        // long-running task teaches the LLM to ignore it.
        let mut s = Stream::default();
        s.push(&vec![b'a'; TRIM_TRIGGER_BYTES]);
        let (first, dropped) = s.drain();
        assert_eq!(dropped, 0, "no trim yet at exactly the trigger size");
        assert_eq!(first.len(), TRIM_TRIGGER_BYTES);

        // Now overrun. Everything trimmed is behind the cursor.
        s.push(&vec![b'b'; 4096]);
        let (second, dropped) = s.drain();
        assert_eq!(
            second.len(),
            4096,
            "the new bytes survive: {}",
            second.len()
        );
        assert_eq!(dropped, 0, "already-read bytes are not a reportable loss");
        // The lifetime total still records the trim, for the final record.
        assert!(s.all().1 > 0, "trimmed_total must track every cut byte");
    }

    #[tokio::test]
    async fn wait_reports_elapsed_task_runtime() {
        // The model has no clock. Without `elapsed_secs` it infers time
        // from what it ASKED to wait and narrates "roughly 20 minutes in"
        // 90 seconds into a build. A running task reports its age; a
        // finished one reports its total runtime, not time-since-spawn.
        let reg = BackgroundBashRegistry::new();
        let (task_id, _finish_rx) = reg
            .spawn("sleep 1.2", 30, std::path::Path::new("/tmp"), &[], None)
            .await
            .expect("spawn");

        let running = reg
            .read_output_in_memory_wait(&task_id, Duration::from_millis(1100))
            .await
            .expect("snapshot");
        assert!(!running.finished);
        assert!(
            running.elapsed_secs >= 1,
            "a task alive for >1 s must report it, got {}",
            running.elapsed_secs
        );

        assert!(reg.wait_for_finish(&task_id, Duration::from_secs(3)).await);
        tokio::time::sleep(Duration::from_millis(600)).await;
        let done = reg
            .read_output_in_memory_wait(&task_id, Duration::ZERO)
            .await
            .expect("snapshot");
        assert!(done.finished);
        assert!(
            done.elapsed_secs <= 2,
            "a finished task reports its RUNTIME, not time since spawn, got {}",
            done.elapsed_secs
        );
    }

    #[tokio::test]
    async fn wait_wakes_when_task_finishes_with_empty_output() {
        // A task that produces NO output during the wait should still
        // wake the waiter when it finishes — otherwise the LLM would
        // sit on the full wait_secs ceiling for every silent-then-done
        // task. Finishing is now the ONLY early wake (a chunk no longer
        // ends a wait), so this is the whole early-return path. It is
        // durable across the lock-drop/await gap because the waiter
        // registers before re-reading `finished_at`.
        let reg = BackgroundBashRegistry::new();
        let (task_id, _finish_rx) = reg
            .spawn("sleep 0.3", 5, std::path::Path::new("/tmp"), &[], None)
            .await
            .expect("spawn");

        let start = std::time::Instant::now();
        let snap = reg
            .read_output_in_memory_wait(&task_id, Duration::from_secs(3))
            .await
            .expect("snapshot");
        let elapsed = start.elapsed();
        assert!(snap.finished, "wait must wake when the task finishes");
        assert_eq!(snap.outcome, Some(TaskOutcome::Exited(0)));
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
        assert!(
            snap.outcome.is_none(),
            "a running task has no outcome at all — not even a placeholder"
        );
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
    /// parallel with an in-flight drain. Pin the documented contract: BOTH
    /// waiters wake on finish and neither deadlocks; the first to take the
    /// lock after the wake gets the bytes, the other sees `finished=true`
    /// with an empty window. `notify_waiters` stores no permit, so this
    /// only holds because each waiter registers before re-reading
    /// `finished_at` — with a single-permit wake instead, one waiter would
    /// return and the other would sit out its whole budget on a task that
    /// had already finished.
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

        let started = std::time::Instant::now();
        let (snap_a, snap_b) = tokio::join!(h_a, h_b);
        let elapsed = started.elapsed();
        let snap_a = snap_a.unwrap().expect("waiter A returned None");
        let snap_b = snap_b.unwrap().expect("waiter B returned None");

        // BOTH return on the finish, not just whichever won a single
        // permit. The task ends at ~0.3 s against a 3 s budget, so a
        // stranded waiter shows up as an elapsed near the full ceiling.
        assert!(
            elapsed < Duration::from_secs(2),
            "both waiters must wake on finish, not sit out the budget (took {:?})",
            elapsed
        );
        assert!(
            snap_a.finished && snap_b.finished,
            "both waiters must observe the finish: a={} b={}",
            snap_a.finished,
            snap_b.finished
        );
        // At least one saw the output; the other may see an empty window
        // because its peer drained the bytes first.
        let combined = format!("{}{}", snap_a.stdout, snap_b.stdout);
        assert!(
            combined.contains("first") || combined.contains("second"),
            "at least one waiter must see output across the race: a={:?} b={:?}",
            snap_a.stdout,
            snap_b.stdout
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
            .spawn(
                "echo a; sleep 30",
                60,
                std::path::Path::new("/tmp"),
                &[],
                None,
            )
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

    // -----------------------------------------------------------------
    // Exit-status fidelity.
    //
    // The 2026-07-26 nightly hit the same defect four times in one
    // pipeline: a background task that really exited 101 (clippy) or 1
    // (e2e) was reported as "exit code 0". Every step had to write the
    // real status into a sidecar `.ec` file and cross-check it. These
    // tests pin the statuses that were being lost, so the sidecar
    // workaround never has to be load-bearing again.
    // -----------------------------------------------------------------

    /// Convenience: run to completion and hand back the final snapshot.
    async fn run_to_completion(command: &str, timeout_secs: u64) -> OutputSnapshot {
        let reg = BackgroundBashRegistry::new();
        let (task_id, _finish_rx) = reg
            .spawn(
                command,
                timeout_secs,
                std::path::Path::new("/tmp"),
                &[],
                None,
            )
            .await
            .expect("spawn");
        assert!(
            reg.wait_for_finish(&task_id, Duration::from_secs(20)).await,
            "task did not finish: {command}"
        );
        reg.read_output_in_memory_wait(&task_id, Duration::ZERO)
            .await
            .expect("snapshot")
    }

    #[tokio::test]
    async fn plain_exit_101_is_reported_as_101() {
        let snap = run_to_completion("echo lints; exit 101", 10).await;
        assert_eq!(snap.outcome, Some(TaskOutcome::Exited(101)));
        assert_eq!(snap.outcome.unwrap().exit_code(), Some(101));
        assert_eq!(snap.outcome.unwrap().describe(), "exit code 101");
    }

    #[tokio::test]
    async fn plain_exit_1_is_reported_as_1() {
        let snap = run_to_completion("echo fail >&2; exit 1", 10).await;
        assert_eq!(snap.outcome, Some(TaskOutcome::Exited(1)));
    }

    /// THE regression. `cargo clippy … 2>&1 | tee build.log` exits 101 in
    /// its first stage and 0 in its last. Before the `pipefail` shell, the
    /// registry recorded tee's 0 and every downstream surface reported a
    /// clean build. Asserting 101 here is the whole point of the change.
    #[tokio::test]
    async fn pipeline_exit_101_is_not_masked_by_tee() {
        let snap = run_to_completion("sh -c 'echo lints; exit 101' 2>&1 | tee /dev/null", 10).await;
        assert_eq!(
            snap.outcome,
            Some(TaskOutcome::Exited(101)),
            "a failing stage piped into tee must not be reported as tee's 0"
        );
        assert_ne!(
            snap.outcome.unwrap().exit_code(),
            Some(0),
            "the masking trap is back: a 101 build read as a clean 0"
        );
    }

    /// Same trap, the other status the nightly observed (e2e exited 1).
    #[tokio::test]
    async fn pipeline_exit_1_is_not_masked_by_tee() {
        let snap = run_to_completion("sh -c 'echo fail; exit 1' 2>&1 | tee /dev/null", 10).await;
        assert_eq!(snap.outcome, Some(TaskOutcome::Exited(1)));
    }

    /// A failing stage keeps its own status through the succeeding stages
    /// after it, rather than degrading to a generic "something failed".
    /// (`pipefail` reports the rightmost *failing* stage — here there is only
    /// one, so it is the one that surfaces. See
    /// `core::shell::tests::pipefail_reports_the_rightmost_failing_stage_not_the_first`
    /// for the multi-failure case.)
    #[tokio::test]
    async fn pipeline_preserves_the_failing_stages_own_status() {
        let snap = run_to_completion("sh -c 'exit 42' | cat | cat", 10).await;
        assert_eq!(snap.outcome, Some(TaskOutcome::Exited(42)));
    }

    /// A fully-successful pipeline still reports 0 — pipefail must not
    /// invent failures.
    #[tokio::test]
    async fn successful_pipeline_still_reports_zero() {
        let snap = run_to_completion("echo hi | cat | cat", 10).await;
        assert_eq!(snap.outcome, Some(TaskOutcome::Exited(0)));
        assert!(snap.outcome.unwrap().is_success());
    }

    /// A child that dies on a signal must report the signal — never 0, and
    /// never a bare number that reads like a normal exit.
    #[tokio::test]
    async fn signal_death_reports_sigkill_not_zero() {
        let snap = run_to_completion("echo bye; kill -9 $$", 10).await;
        assert_eq!(snap.outcome, Some(TaskOutcome::Signaled(9)));
        let outcome = snap.outcome.unwrap();
        assert_eq!(outcome.exit_code(), None, "a signal death has no exit code");
        assert_eq!(outcome.signal(), Some(9));
        assert_eq!(outcome.describe(), "killed by SIGKILL (signal 9)");
        assert!(!outcome.is_success());
        // Not killed/timed_out: the ENGINE didn't end this task, the child
        // did. Conflating the two would tell the LLM the engine intervened.
        assert!(!snap.killed);
        assert!(!snap.timed_out);
    }

    #[tokio::test]
    async fn signal_death_reports_sigsegv() {
        let snap = run_to_completion("sh -c 'kill -SEGV $$'", 10).await;
        assert_eq!(snap.outcome, Some(TaskOutcome::Signaled(11)));
        assert_eq!(
            snap.outcome.unwrap().describe(),
            "killed by SIGSEGV (signal 11)"
        );
    }

    /// The SIGPIPE case `pipefail` newly makes visible: a producer killed
    /// because its consumer closed the pipe. It must not read as success, and
    /// the `141` must be readable as what it is rather than as a mystery
    /// number. Note the shape — the shell exits 141 *normally*, so this is
    /// `Exited(141)`, not `Signaled(13)`; only the shell's own death would be
    /// a signal to us.
    #[tokio::test]
    async fn sigpipe_producer_is_named_not_silently_successful() {
        let snap = run_to_completion("yes | head -1 >/dev/null", 10).await;
        let outcome = snap.outcome.expect("finished task has an outcome");
        assert!(
            !outcome.is_success(),
            "pipefail must surface the SIGPIPE'd producer, got {outcome:?}"
        );
        assert_eq!(
            outcome.describe(),
            "exit code 141 (probable SIGPIPE)",
            "the 141 must be decoded, not left as a bare number"
        );
    }
}
