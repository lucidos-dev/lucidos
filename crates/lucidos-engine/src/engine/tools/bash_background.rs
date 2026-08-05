//! In-memory registry for `run_bash_background` tasks. Holds running tasks
//! plus recently-completed ones, so a `bash_output` drain that lands at the
//! moment a task finishes still gets the final tail instead of an error.
//!
//! **Completing a task does not evict it.** The dispatch site reads a
//! [`CompletionRecord`] to emit `BackgroundBashCompleted` and leaves the entry
//! in place; eviction happens later, on the retention policy below. The two
//! used to be one step, which is exactly how a drain arriving at the
//! completion instant found no entry: the watcher removed the task, then built
//! the event, then emitted it, so a drain landing in that window missed the
//! registry AND the not-yet-written event row and surfaced as
//! `unknown task_id`. Five scheduled trigger runs lost a successful result
//! that way between 2026-07-29 and 2026-08-02.
//!
//! Retention is bounded on both axes, and swept lazily on every registry
//! access so no background sweeper task is needed: nothing older than
//! [`FINISHED_RETENTION_SECS`] past its `finished_at` survives, and at most
//! [`MAX_RETAINED_FINISHED`] completed tasks are held at once. Past that
//! window `bash_output` falls back to the persisted `BackgroundBashCompleted`
//! row, which is the durable record.

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

/// How long a completed task stays drainable after `finished_at`.
///
/// The window that has to be covered is "task completes" to "the agent's next
/// `bash_output` call". The completion watcher pushes a wake message to a
/// parked coding agent the instant the task ends, so five minutes is generous
/// by a wide margin. Past it the caller falls back to the persisted
/// `BackgroundBashCompleted` row, which carries the same final output.
///
/// Time, rather than "evict once drained": drain-once ties eviction to a
/// reader's action, so it both leaks tasks nobody drains (a timer would be
/// needed anyway) and reintroduces the original race one drain narrower, since
/// the registry serves concurrent waiters and the second one would find the
/// entry gone. See `two_concurrent_waiters_on_same_task_both_eventually_return`.
pub(super) const FINISHED_RETENTION_SECS: i64 = 300;

/// Ceiling on retained completed tasks, so a long-lived engine that runs
/// thousands of them doesn't accumulate their buffers. Oldest `finished_at`
/// goes first: the newest completions are the ones an agent may still be about
/// to drain.
///
/// Bounding by COUNT rather than by bytes is deliberate. [`Stream`] already
/// caps each buffer at `MAX_BUFFER_BYTES`, so per-task memory is solved; a
/// second byte-budget mechanism layered on top would be two things to reason
/// about instead of one. Worst case here is 16 tasks times two streams at the
/// ~2 MB trim trigger, and only if all sixteen maxed both buffers inside the
/// same five minutes.
pub(super) const MAX_RETAINED_FINISHED: usize = 16;

#[derive(Clone)]
pub struct BackgroundBashRegistry {
    tasks: Arc<Mutex<HashMap<String, BackgroundTask>>>,
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
struct Stream {
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

    /// Bytes not yet handed to a reader, leaving the cursor where it is.
    /// Test-only: lets `wait_for_stdout` poll for a flush without consuming
    /// the output the test is about to drain.
    #[cfg(test)]
    fn peek_unread(&self) -> std::borrow::Cow<'_, str> {
        String::from_utf8_lossy(&self.bytes[self.cursor.min(self.bytes.len())..])
    }

    /// The whole retained buffer and everything the cap ever cut from it.
    /// Used for the final `BackgroundBashCompleted` record. Read-only: it
    /// leaves the cursor alone, so building the completion event cannot
    /// consume the output a pending drain is about to return.
    fn all(&self) -> (String, usize) {
        (
            String::from_utf8_lossy(&self.bytes).to_string(),
            self.trimmed_total,
        )
    }
}

/// One background task: running, or completed and retained for a late drain.
///
/// Named for what it is rather than for the state it spends most of its life
/// in. It was called `RunningTask` while completion and eviction were the same
/// step, which made "finished" a state the type barely occupied; retention
/// promotes it to a first-class one.
struct BackgroundTask {
    started_at: DateTime<Utc>,
    stdout: Stream,
    stderr: Stream,
    /// How the child ended. `None` until the watchdog writes it, in the same
    /// locked block as `finished_at` — so `finished_at.is_some()` and
    /// `outcome.is_some()` are always in step, and a still-running task can
    /// never present a status at all (as opposed to presenting a `0`).
    outcome: Option<TaskOutcome>,
    timed_out: bool,
    killed: bool,
    /// Single source of truth for "has the watchdog finished?", and the clock
    /// the retention sweep reads. `None` = still running, `Some(t)` = finished
    /// at `t`, drainable until `t + FINISHED_RETENTION_SECS`.
    finished_at: Option<DateTime<Utc>>,
    /// Whether someone has taken this task's [`CompletionRecord`] yet, which
    /// in production means the watcher is about to persist
    /// `BackgroundBashCompleted`. Until then the completion exists nowhere but
    /// here, so the retention CAP must leave the entry alone: dropping it
    /// would lose the durable record entirely. Expiry ignores this flag, so
    /// the exemption cannot pin memory. See [`sweep_finished`].
    completion_recorded: bool,
    /// Thread that spawned this task. `None` for tests and engine-internal
    /// callers with no owning thread. Drives `has_running_for_thread` so the
    /// agent-session idle handler can keep CC alive while bg bash is still
    /// running for its thread — without this, a CC that idled mid-/harden
    /// waiting on its own `run_bash_background` tests was killed and the
    /// change auto-proposed as "done", which caused premature Apply + harden-
    /// from-scratch on click.
    thread_id: Option<Uuid>,
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
    finish_notify: Arc<Notify>,
}

impl BackgroundTask {
    fn is_finished(&self) -> bool {
        self.finished_at.is_some()
    }

    /// True once the retention window has closed on a completed task. Always
    /// false while it is running: an unfinished task is live state, never a
    /// retention candidate, however long it has been going.
    fn is_expired(&self, now: DateTime<Utc>) -> bool {
        self.finished_at
            .is_some_and(|at| (now - at).num_seconds() > FINISHED_RETENTION_SECS)
    }
}

/// The final state of a completed task, read for the
/// `BackgroundBashCompleted` event. Owned rather than a borrow so the dispatch
/// site can build and emit the event without holding the registry lock, and
/// deliberately NOT a removal: reading the final state and evicting the entry
/// used to be the same call, which is what broke a drain arriving at the
/// completion instant.
///
/// The two `*_dropped` counts are lifetime totals from [`Stream::all`] (every
/// byte the buffer cap ever cut), not the per-window count a drain reports.
#[derive(Debug, Clone)]
pub struct CompletionRecord {
    pub started_at: DateTime<Utc>,
    pub finished_at: DateTime<Utc>,
    /// A finished task always has an outcome (the watchdog writes it in the
    /// same locked block as `finished_at`), so this is not optional. A status
    /// the engine could not obtain is [`TaskOutcome::Unknown`], which renders
    /// as words rather than as a `0`.
    pub outcome: TaskOutcome,
    pub timed_out: bool,
    pub killed: bool,
    pub stdout: String,
    pub stdout_dropped: usize,
    pub stderr: String,
    pub stderr_dropped: usize,
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

    /// Take the registry lock, applying the retention policy on the way in.
    ///
    /// Every public method goes through here, which is what makes the sweep
    /// lazy: retention needs no background task and no timer, and there is
    /// exactly one place where "how long a completed task lives" is decided.
    /// `drain_pipe` deliberately does NOT use it: that path locks per 8 KB
    /// chunk on a hot loop and only ever touches its own still-running task.
    async fn locked(&self) -> tokio::sync::MutexGuard<'_, HashMap<String, BackgroundTask>> {
        let mut tasks = self.tasks.lock().await;
        sweep_finished(&mut tasks);
        tasks
    }

    /// Spawn a child process. Inserts the task into the registry before
    /// returning the task_id, eliminating the spawn/poll race a follow-up
    /// `bash_output` would otherwise hit. The returned receiver fires
    /// when the watchdog marks the task finished, and the dispatch site uses
    /// it to read a [`CompletionRecord`] and emit `BackgroundBashCompleted`.
    /// That read does NOT evict: the entry stays drainable for
    /// [`FINISHED_RETENTION_SECS`] afterwards.
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
            let mut tasks = self.locked().await;
            tasks.insert(
                task_id.clone(),
                BackgroundTask {
                    started_at: Utc::now(),
                    stdout: Stream::default(),
                    stderr: Stream::default(),
                    outcome: None,
                    timed_out: false,
                    killed: false,
                    finished_at: None,
                    completion_recorded: false,
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
            // and hit EOF before signaling finish. Otherwise the completion
            // record the dispatch-site watcher reads (and the first drain
            // after it) would be built while the pipe readers are still
            // racing the kernel, losing the tail of stdout/stderr after a
            // kill or timeout: `finished_at` is the flag every reader gates
            // on, so it must not be set until the bytes are in. Bound the
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
    /// Returns `None` when the task is unknown, which now means one of two
    /// things: it never existed, or its retention window has closed. Either
    /// way the caller falls back to the event-store query, which serves the
    /// second case from the persisted `BackgroundBashCompleted` row. The
    /// cursor advances by the bytes returned, so the next call only sees
    /// newly-written output.
    ///
    /// **A completed task is served here, not from the event store**, for as
    /// long as it is retained. That is the whole point of retention: a drain
    /// landing at the completion instant used to find nothing here and no
    /// event row yet either, and surfaced to the agent as `unknown task_id`
    /// even though the work had succeeded.
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
            let mut tasks = self.locked().await;
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
            let mut tasks = self.locked().await;
            let task = tasks.get_mut(task_id)?;
            if task.is_finished() {
                return Some(drain_snapshot(task));
            }
        }
        // Park until the watchdog signals finish, or the budget runs out.
        // On timeout, fall through and return whatever accumulated. The task
        // cannot have been swept from under us here: the sweep only touches
        // finished tasks, and one that finishes during the wait has a
        // `finished_at` of a moment ago.
        let _ = tokio::time::timeout(wait, notified).await;
        let mut tasks = self.locked().await;
        let task = tasks.get_mut(task_id)?;
        Some(drain_snapshot(task))
    }

    /// Cancel a running task. Returns `false` if the task is unknown or
    /// already finished, retained completions included: retention makes a
    /// finished task readable, never killable.
    pub async fn kill(&self, task_id: &str) -> bool {
        let mut tasks = self.locked().await;
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
    ///
    /// Retained completions are invisible here, and must stay that way: the
    /// filter is on `!is_finished()`, not on presence in the map. A retained
    /// task counted as running would pin a coding-agent session open for the
    /// whole retention window after its work was already done.
    pub async fn has_running_for_thread(&self, thread_id: Uuid) -> bool {
        let tasks = self.locked().await;
        tasks
            .values()
            .any(|t| t.thread_id == Some(thread_id) && !t.is_finished())
    }

    /// Read a finished task's final state, for the `BackgroundBashCompleted`
    /// event. Returns `None` if the task is missing or still running.
    ///
    /// **Reads, never removes.** This was `take_finished`, which did
    /// `tasks.remove` and so made emitting the completion event and evicting
    /// the entry the same step. The dispatch site's order is read, build,
    /// emit, so a drain arriving anywhere in that span found neither the
    /// registry entry (already gone) nor the event row (not yet written) and
    /// reached the agent as `unknown task_id`. Eviction is now the retention
    /// sweep's job alone.
    ///
    /// Taking the record is what makes the entry a candidate for the sweep's
    /// cap: until the caller has this, the task's completion exists nowhere
    /// durable, so the cap must not drop it. See [`sweep_finished`].
    pub async fn completion_record(&self, task_id: &str) -> Option<CompletionRecord> {
        let mut tasks = self.locked().await;
        let task = tasks.get_mut(task_id)?;
        let finished_at = task.finished_at?;
        task.completion_recorded = true;
        let (stdout, stdout_dropped) = task.stdout.all();
        let (stderr, stderr_dropped) = task.stderr.all();
        Some(CompletionRecord {
            started_at: task.started_at,
            finished_at,
            // A finished task always has an outcome (written in the same
            // locked block as `finished_at`). Defend anyway rather than
            // unwrapping: `Unknown` is the honest reading, and it renders as
            // words, not as a `0`.
            outcome: task.outcome.unwrap_or(TaskOutcome::Unknown),
            timed_out: task.timed_out,
            killed: task.killed,
            stdout,
            stdout_dropped,
            stderr,
            stderr_dropped,
        })
    }

    /// Test helper: poll-wait until the task's not-yet-read stdout contains
    /// `needle` (or timeout). Peeks past the cursor instead of draining, so a
    /// test that then drains still sees the bytes.
    ///
    /// Use this instead of sleeping before asserting on a subprocess's first
    /// flush. A sleep long enough on an idle machine is not long enough under
    /// the full suite, where thousands of tests contend for the CPU: three
    /// tests here failed on exactly that race on 2026-08-05 and passed in
    /// isolation seconds later. Polling also fixes the other direction, since
    /// a fixed sleep can overshoot a later write the test asserts is absent.
    #[cfg(test)]
    pub async fn wait_for_stdout(&self, task_id: &str, needle: &str, timeout: Duration) -> bool {
        let deadline = std::time::Instant::now() + timeout;
        loop {
            {
                let tasks = self.tasks.lock().await;
                if tasks
                    .get(task_id)
                    .is_some_and(|t| t.stdout.peek_unread().contains(needle))
                {
                    return true;
                }
            }
            if std::time::Instant::now() >= deadline {
                return false;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    /// Test helper: poll-wait until the task is marked finished (or
    /// timeout). Production callers don't need this: the dispatch site uses
    /// an event-emitting watcher driven by the spawn-time finish receiver.
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

    /// Test helper: shift a task's timestamps `by` seconds into the past, so
    /// the retention window can be exercised against the real
    /// `FINISHED_RETENTION_SECS` without sleeping five minutes. Moves
    /// `started_at` with `finished_at` so the task still looks like it ran for
    /// as long as it did. Takes the raw lock, not `locked()`, so backdating
    /// past the window doesn't sweep the entry before the caller can observe
    /// the next access doing it.
    #[cfg(test)]
    pub async fn backdate_for_test(&self, task_id: &str, by: i64) {
        let shift = chrono::Duration::seconds(by);
        let mut tasks = self.tasks.lock().await;
        let Some(task) = tasks.get_mut(task_id) else {
            return;
        };
        task.started_at -= shift;
        task.finished_at = task.finished_at.map(|at| at - shift);
    }

    /// Test helper: how many completed tasks the registry is currently
    /// retaining, for the cap assertion.
    #[cfg(test)]
    pub async fn retained_finished_count(&self) -> usize {
        self.locked()
            .await
            .values()
            .filter(|t| t.is_finished())
            .count()
    }
}

/// Apply the retention policy: drop every completed task past
/// [`FINISHED_RETENTION_SECS`], then, if more than
/// [`MAX_RETAINED_FINISHED`] *recorded* completions remain, drop the oldest
/// until the count is back at the cap.
///
/// Two kinds of entry are never touched, for the same reason: they are not
/// retention candidates yet.
///
/// - **Running tasks**, however long they have been going. They are live
///   state.
/// - **Completions whose [`CompletionRecord`] has not been read**, on the CAP
///   pass. That read is what the watcher does immediately before emitting
///   `BackgroundBashCompleted`, so evicting one first would leave the task
///   with no durable record at all: the watcher finds nothing, emits nothing,
///   and every later `bash_output` reports an unknown task. That is the exact
///   loss this whole change exists to prevent, so a burst of more than
///   `MAX_RETAINED_FINISHED` simultaneous completions must overshoot the cap
///   briefly rather than drop one.
///
/// The EXPIRY pass is deliberately unconditional, which is what keeps the
/// overshoot bounded: an unread completion is a transient (the watcher is a
/// spawned task already parked on the finish signal, so it reads within
/// microseconds), and a five-minute window is many orders of magnitude longer
/// than that. Gating expiry on the read too would pin an entry forever
/// whenever a watcher never runs, and unbounded memory is the worse failure.
fn sweep_finished(tasks: &mut HashMap<String, BackgroundTask>) {
    let now = Utc::now();
    tasks.retain(|_, t| !t.is_expired(now));

    // Count before collecting. This runs on every registry lock, so the
    // common case (nothing over the cap) must not clone an id per completion
    // and sort them just to discover there was nothing to do.
    let excess = tasks
        .values()
        .filter(|t| t.completion_recorded)
        .count()
        .saturating_sub(MAX_RETAINED_FINISHED);
    if excess == 0 {
        return;
    }
    let mut recorded: Vec<(DateTime<Utc>, String)> = tasks
        .iter()
        .filter(|(_, t)| t.completion_recorded)
        .filter_map(|(id, t)| t.finished_at.map(|at| (at, id.clone())))
        .collect();
    // Oldest completion first: the newest are the ones an agent may still be
    // about to drain, so they are the last to go.
    recorded.sort_unstable_by(|a, b| a.0.cmp(&b.0));
    for (_, id) in recorded.into_iter().take(excess) {
        tasks.remove(&id);
    }
}

/// Drain the per-task buffers since the previous cursor and advance
/// the cursors. Shared by zero-wait and wait paths in
/// `read_output_in_memory_wait` so cursor semantics live in exactly
/// one place.
fn drain_snapshot(task: &mut BackgroundTask) -> OutputSnapshot {
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
    tasks: Arc<Mutex<HashMap<String, BackgroundTask>>>,
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

        // Wait for "a" to flush. Polled, not slept: a fixed wait is both too
        // short under suite load (no "a" yet) and too long on a slow scheduler
        // (the 0.3s "b" arrives too, which the next assert forbids).
        assert!(
            reg.wait_for_stdout(&task_id, "a", Duration::from_secs(5))
                .await,
            "the first echo never flushed",
        );
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

        // Ceiling generously above the 1 s spawn timeout, and still well below
        // the 30 s the task would run un-killed: a broken timeout leaves it
        // alive at 20 s and the assert fires. A tight 3 s ceiling measured host
        // spawn latency instead, and failed under a loaded full-suite run.
        let finished = reg.wait_for_finish(&task_id, Duration::from_secs(20)).await;
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

    /// THE regression. A `bash_output` drain that lands at the moment a task
    /// completes must return the final tail with `finished: true`, not
    /// `unknown task_id`. Observed 5 times in 7 days against two scheduled
    /// triggers: the task had both a `BackgroundBashStarted` and a
    /// `BackgroundBashCompleted` with `exit_code: 0`, and the drain error
    /// carried the completion timestamp to the second. Successful background
    /// work was silently discarded and the trigger carried on without it.
    ///
    /// This replays the completion watcher's exact order: the watchdog marks
    /// the task finished, the watcher reads the final state for
    /// `BackgroundBashCompleted`, and only then does the drain arrive.
    #[tokio::test]
    async fn drain_after_completion_event_returns_the_final_tail() {
        let reg = BackgroundBashRegistry::new();
        let (task_id, _finish_rx) = reg
            .spawn(
                "echo the-result",
                5,
                std::path::Path::new("/tmp"),
                &[],
                None,
            )
            .await
            .expect("spawn");
        assert!(reg.wait_for_finish(&task_id, Duration::from_secs(3)).await);

        // What `spawn_bash_completion_watcher` does before it emits.
        let record = reg
            .completion_record(&task_id)
            .await
            .expect("completion record for a finished task");
        assert!(record.stdout.contains("the-result"));
        assert_eq!(record.outcome, TaskOutcome::Exited(0));

        // The drain that used to arrive one instant too late.
        let snap = reg
            .read_output_in_memory_wait(&task_id, Duration::ZERO)
            .await
            .expect("a completed task must still be drainable");
        assert!(snap.finished, "a completed task must report finished");
        assert!(
            snap.stdout.contains("the-result"),
            "the final tail was lost: {:?}",
            snap.stdout
        );
        assert_eq!(snap.outcome, Some(TaskOutcome::Exited(0)));
        assert!(!snap.timed_out);
        assert!(!snap.killed);
    }

    /// The drain cursor is not reset by retention: a second drain inside the
    /// window sees an empty window, still flagged `finished`. Without this the
    /// registry would replay the whole buffer on every poll, which is the
    /// context bloat the drain semantics exist to avoid.
    #[tokio::test]
    async fn repeat_drain_within_retention_is_empty_but_still_finished() {
        let reg = BackgroundBashRegistry::new();
        let (task_id, _finish_rx) = reg
            .spawn("echo once", 5, std::path::Path::new("/tmp"), &[], None)
            .await
            .expect("spawn");
        assert!(reg.wait_for_finish(&task_id, Duration::from_secs(3)).await);

        let first = reg
            .read_output_in_memory_wait(&task_id, Duration::ZERO)
            .await
            .expect("first drain");
        assert!(first.stdout.contains("once"));

        let second = reg
            .read_output_in_memory_wait(&task_id, Duration::ZERO)
            .await
            .expect("a retained task stays drainable");
        assert!(
            second.stdout.is_empty(),
            "a repeat drain must not replay: {:?}",
            second.stdout
        );
        assert!(second.finished);
        assert_eq!(second.outcome, Some(TaskOutcome::Exited(0)));
    }

    /// Retention is bounded in time. Past the grace window the entry is swept
    /// on the next registry access, and `bash_output` routes to the persisted
    /// `BackgroundBashCompleted` row instead.
    #[tokio::test]
    async fn finished_task_is_evicted_after_the_grace_window() {
        let reg = BackgroundBashRegistry::new();
        let (task_id, _finish_rx) = reg
            .spawn("echo stale", 5, std::path::Path::new("/tmp"), &[], None)
            .await
            .expect("spawn");
        assert!(reg.wait_for_finish(&task_id, Duration::from_secs(3)).await);
        assert!(reg
            .read_output_in_memory_wait(&task_id, Duration::ZERO)
            .await
            .is_some());

        // One second past the window, so the boundary itself stays retained.
        reg.backdate_for_test(&task_id, FINISHED_RETENTION_SECS + 1)
            .await;

        assert!(
            reg.read_output_in_memory_wait(&task_id, Duration::ZERO)
                .await
                .is_none(),
            "an expired task must be swept so the caller falls back to the event store"
        );
        assert!(reg.completion_record(&task_id).await.is_none());
    }

    /// Retention is bounded in count too, so a long-lived engine that runs
    /// thousands of background tasks doesn't accumulate their buffers. The
    /// oldest completions go first, because the newest are the ones an agent
    /// might still be about to drain.
    #[tokio::test]
    async fn retained_finished_tasks_are_capped_keeping_the_newest() {
        let reg = BackgroundBashRegistry::new();
        let mut ids = Vec::new();
        for i in 0..(MAX_RETAINED_FINISHED + 4) {
            let (task_id, _finish_rx) = reg
                .spawn(
                    &format!("echo task{i}"),
                    5,
                    std::path::Path::new("/tmp"),
                    &[],
                    None,
                )
                .await
                .expect("spawn");
            assert!(reg.wait_for_finish(&task_id, Duration::from_secs(5)).await);
            // Stand in for the completion watcher, which takes the record and
            // persists `BackgroundBashCompleted`. Only a recorded completion
            // is a cap candidate, so a test that skipped this would exercise
            // the exemption instead of the cap.
            assert!(reg.completion_record(&task_id).await.is_some());
            // Order the completions deterministically: `finished_at` is written
            // by the watchdog at real wall-clock time, and several short echoes
            // can land inside the same clock tick. Earliest-spawned reads as
            // oldest, and every offset stays inside the retention window so
            // this test measures the cap and nothing else.
            let seconds_ago = 64 - ids.len() as i64;
            assert!(seconds_ago > 0 && seconds_ago < FINISHED_RETENTION_SECS);
            reg.backdate_for_test(&task_id, seconds_ago).await;
            ids.push(task_id);
        }

        assert_eq!(
            reg.retained_finished_count().await,
            MAX_RETAINED_FINISHED,
            "retention must stop at the cap"
        );
        for old in &ids[..4] {
            assert!(
                reg.read_output_in_memory_wait(old, Duration::ZERO)
                    .await
                    .is_none(),
                "the oldest completions are the ones evicted at the cap"
            );
        }
        for recent in &ids[4..] {
            assert!(
                reg.read_output_in_memory_wait(recent, Duration::ZERO)
                    .await
                    .is_some(),
                "the newest completions are the ones an agent may still drain"
            );
        }
    }

    /// The cap must never drop a completion whose watcher has not yet taken
    /// its record. That entry is the ONLY copy: the watcher reads the record
    /// and then emits `BackgroundBashCompleted`, so evicting it first leaves
    /// the watcher with nothing to emit, no durable row, and every later
    /// `bash_output` reporting an unknown task. It would reintroduce the exact
    /// silent loss this change exists to remove, just through the cap instead
    /// of through completion-as-eviction.
    ///
    /// Reachable whenever more than `MAX_RETAINED_FINISHED` tasks complete in
    /// a burst before their watchers are scheduled, so the cap has to overshoot
    /// rather than drop one. Expiry still bounds the overshoot.
    #[tokio::test]
    async fn cap_never_drops_a_completion_whose_record_was_not_taken() {
        let reg = BackgroundBashRegistry::new();
        let mut ids = Vec::new();
        for i in 0..(MAX_RETAINED_FINISHED + 4) {
            let (task_id, _finish_rx) = reg
                .spawn(
                    &format!("echo burst{i}"),
                    5,
                    std::path::Path::new("/tmp"),
                    &[],
                    None,
                )
                .await
                .expect("spawn");
            assert!(reg.wait_for_finish(&task_id, Duration::from_secs(5)).await);
            // Deliberately do NOT take the record: every watcher is still
            // waiting to be scheduled.
            ids.push(task_id);
        }

        assert_eq!(
            reg.retained_finished_count().await,
            MAX_RETAINED_FINISHED + 4,
            "the cap must overshoot rather than evict an unrecorded completion"
        );
        for id in &ids {
            assert!(
                reg.completion_record(id).await.is_some(),
                "every watcher must still find its task to emit BackgroundBashCompleted"
            );
        }
        // Once every record is taken, the cap applies again on the next access.
        assert_eq!(reg.retained_finished_count().await, MAX_RETAINED_FINISHED);
    }

    /// Retention must not make a finished task read as running. The
    /// agent-session idle handler keeps a coding agent alive while this is
    /// true, so a retained task counted as running would pin the session open
    /// for the whole grace window.
    #[tokio::test]
    async fn has_running_for_thread_is_false_for_a_retained_finished_task() {
        let reg = BackgroundBashRegistry::new();
        let my_thread = Uuid::new_v4();
        let (task_id, _finish_rx) = reg
            .spawn(
                "echo done",
                5,
                std::path::Path::new("/tmp"),
                &[],
                Some(my_thread),
            )
            .await
            .expect("spawn");
        assert!(reg.wait_for_finish(&task_id, Duration::from_secs(3)).await);

        assert!(
            !reg.has_running_for_thread(my_thread).await,
            "a finished task must not register as running, retained or not"
        );
        // And it is still there, which is the whole point of retention.
        assert!(reg
            .read_output_in_memory_wait(&task_id, Duration::ZERO)
            .await
            .is_some());
    }

    #[tokio::test]
    async fn completion_record_does_not_evict_the_task() {
        let reg = BackgroundBashRegistry::new();
        let (task_id, _finish_rx) = reg
            .spawn("echo done", 5, std::path::Path::new("/tmp"), &[], None)
            .await
            .expect("spawn");
        let finished = reg.wait_for_finish(&task_id, Duration::from_secs(3)).await;
        assert!(finished);

        let record = reg
            .completion_record(&task_id)
            .await
            .expect("completion record");
        assert!(record.stdout.contains("done"));

        // Reading the final state must NOT be the same step as removing the
        // entry: that coupling is what made a drain at the completion instant
        // fail. Reading it twice is fine, and the task stays drainable.
        assert!(reg.completion_record(&task_id).await.is_some());
        assert!(reg
            .read_output_in_memory_wait(&task_id, Duration::ZERO)
            .await
            .is_some());
    }

    /// The completion record reads the whole retained buffer via
    /// `Stream::all`, which must not move the drain cursor. If it did, the
    /// watcher would consume the output a pending drain is waiting for and the
    /// agent would see an empty tail for work that produced plenty.
    #[tokio::test]
    async fn completion_record_does_not_consume_the_drain_cursor() {
        let reg = BackgroundBashRegistry::new();
        let (task_id, _finish_rx) = reg
            .spawn("echo payload", 5, std::path::Path::new("/tmp"), &[], None)
            .await
            .expect("spawn");
        assert!(reg.wait_for_finish(&task_id, Duration::from_secs(3)).await);

        assert!(reg
            .completion_record(&task_id)
            .await
            .expect("completion record")
            .stdout
            .contains("payload"));

        let snap = reg
            .read_output_in_memory_wait(&task_id, Duration::ZERO)
            .await
            .expect("drain");
        assert!(
            snap.stdout.contains("payload"),
            "building the completion event ate the pending drain: {:?}",
            snap.stdout
        );
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

        // Wait for the echo to flush, then kill mid-sleep. Polled, not slept:
        // under suite load a fixed wait kills before the first line lands, and
        // the test then blames the kill for losing output that never arrived.
        assert!(
            reg.wait_for_stdout(&task_id, "hello-before-kill", Duration::from_secs(5))
                .await,
            "the echo never flushed, so this test cannot say what the kill did",
        );
        assert!(reg.kill(&task_id).await);

        let finished = reg.wait_for_finish(&task_id, Duration::from_secs(8)).await;
        assert!(finished, "task did not finish within 8s after kill");

        let record = reg
            .completion_record(&task_id)
            .await
            .expect("completion record after kill");
        assert!(
            record.stdout.contains("hello-before-kill"),
            "stdout from before kill was lost: {:?}",
            record.stdout
        );
        assert!(record.killed);
    }

    #[tokio::test]
    async fn completion_record_returns_none_while_running() {
        let reg = BackgroundBashRegistry::new();
        let (task_id, _finish_rx) = reg
            .spawn("sleep 5", 60, std::path::Path::new("/tmp"), &[], None)
            .await
            .expect("spawn");
        // Don't wait — task is still running.
        assert!(reg.completion_record(&task_id).await.is_none());
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

        // Kill → the task is finished → has_running_for_thread flips back,
        // whether or not the entry is still retained for a late drain.
        assert!(reg.kill(&task_id).await);
        let finished = reg.wait_for_finish(&task_id, Duration::from_secs(3)).await;
        assert!(finished);

        assert!(
            !reg.has_running_for_thread(my_thread).await,
            "a finished task must NOT keep registering as running"
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

        // Same widening as the two siblings above: a 1.8 s margin over a 1.2 s
        // sleep is a host-speed measurement, not an assertion about this code.
        assert!(reg.wait_for_finish(&task_id, Duration::from_secs(20)).await);
        let at_finish = reg
            .read_output_in_memory_wait(&task_id, Duration::ZERO)
            .await
            .expect("snapshot");
        assert!(at_finish.finished);

        // The invariant is that the number FREEZES at completion, so assert it
        // stops moving rather than bounding it against wall-clock. An absolute
        // ceiling (this once read `<= 2` for a 1.2 s sleep) measures the host's
        // spawn latency as much as the code: on a machine running another test
        // suite the shell spawn alone ate the margin and the task genuinely ran
        // ~6 s, failing a test that had found no bug.
        tokio::time::sleep(Duration::from_millis(600)).await;
        let later = reg
            .read_output_in_memory_wait(&task_id, Duration::ZERO)
            .await
            .expect("snapshot");
        assert_eq!(
            later.elapsed_secs, at_finish.elapsed_secs,
            "a finished task reports its RUNTIME, frozen at completion, not a \
             time-since-spawn that keeps counting"
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

        // "Woke early" is proven by the GAP between the wake and the ceiling,
        // not by an absolute duration: a wide ceiling with a much lower bound
        // still fails a wait that ran to timeout, while a tight pair (3 s
        // ceiling, 2 s bound) just measures how loaded the host is. That pair
        // failed under a full-suite run for a 0.3 s task.
        let start = std::time::Instant::now();
        let snap = reg
            .read_output_in_memory_wait(&task_id, Duration::from_secs(30))
            .await
            .expect("snapshot");
        let elapsed = start.elapsed();
        assert!(snap.finished, "wait must wake when the task finishes");
        assert_eq!(snap.outcome, Some(TaskOutcome::Exited(0)));
        assert!(
            elapsed < Duration::from_secs(15),
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

        // Wait for "a" to flush into the buffer. Polled, not slept: this test
        // then measures that a ZERO wait does not block, which is only a
        // meaningful measurement once there is something to return.
        assert!(
            reg.wait_for_stdout(&task_id, "a", Duration::from_secs(5))
                .await,
            "the echo never flushed",
        );

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
