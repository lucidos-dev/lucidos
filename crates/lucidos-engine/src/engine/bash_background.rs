//! INVARIANT: In-memory registry for `run_bash_background` tasks. Holds running tasks
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
//!
//! **Exactly one completion per task**, gated by
//! [`BackgroundTask::completion_claimed`]. Two emitters can reach a task: its
//! own watchdog, and the teardown sweep in `bash_background_recovery`. Both
//! claim under the registry lock, and the loser writes nothing.

use crate::core::shell::{command_shell, TaskOutcome};
use crate::runtime::agent_sandbox::Sandbox;
use chrono::{DateTime, Utc};
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::sync::{watch, Mutex, Notify};
use uuid::Uuid;

pub use crate::llm::tools::BASH_OUTPUT_MAX_WAIT_SECS;

const MAX_BUFFER_BYTES: usize = 1024 * 1024;
const TRIM_TRIGGER_BYTES: usize = 2 * MAX_BUFFER_BYTES;

pub(super) const FINISHED_RETENTION_SECS: i64 = 300;

pub(super) const MAX_RETAINED_FINISHED: usize = 16;

const COMMAND_PREFIX_BYTES: usize = 200;

/// INVARIANT: Cut a command to what a `BackgroundBashCompleted` carries.
///
/// One function rather than two slices against a shared constant, because the
/// constant is not the part that can go wrong. `bash_background_recovery`
/// rebuilds this field from the `Started` row, and a hand-written
/// `&s[..COMMAND_PREFIX_BYTES]` beside it would panic on a multi-byte boundary.
pub(super) fn command_prefix(command: &str) -> String {
    command[..command.floor_char_boundary(COMMAND_PREFIX_BYTES)].to_string()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompletionDelivery {
    EventWait,
    /// INVARIANT: The owner polls with `lucidos background-task wait`, so no engine wait
    /// arms for it while the owner's turn runs. A wake after the agent already
    /// read the result would re-open the thread for nothing. Once the turn
    /// ends cleanly, nobody polls, and the turn tail arms for it.
    Poll,
}

#[derive(Debug, Clone, Copy)]
pub struct TaskSpec<'a> {
    pub timeout_secs: u64,
    /// INVARIANT: The agent's name for the work, shown on the wait.
    pub description: Option<&'a str>,
    pub delivery: CompletionDelivery,
    /// INVARIANT: The chat agent's tasks strip the engine's inherited secrets. A coding
    /// agent's keep them, as its own shell does.
    pub inherited: crate::core::InheritedSecrets<'a>,
}

#[derive(Clone)]
pub struct BackgroundBashRegistry {
    tasks: Arc<Mutex<HashMap<String, BackgroundTask>>>,
    unread_results: Arc<std::sync::Mutex<HashMap<Uuid, Vec<UnreadResult>>>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnreadResult {
    pub task_id: String,
    pub description: Option<String>,
    /// INVARIANT: Redacted and cut by [`command_prefix`]: safe to show the user.
    pub command: String,
}

type UnreadResults = HashMap<Uuid, Vec<UnreadResult>>;

fn unread(map: &std::sync::Mutex<UnreadResults>) -> std::sync::MutexGuard<'_, UnreadResults> {
    map.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn forget_unread(results: &mut UnreadResults, thread_id: Uuid, task_ids: &[&str]) {
    if let Some(list) = results.get_mut(&thread_id) {
        list.retain(|r| !task_ids.contains(&r.task_id.as_str()));
        if list.is_empty() {
            results.remove(&thread_id);
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stop {
    /// INVARIANT: SIGTERM, then SIGKILL after `GROUP_TEARDOWN_GRACE`. A trap can release
    /// a lock, and a Playwright runner can close the browsers it detached.
    Graceful,
    Immediate,
}

/// INVARIANT: One captured output stream: the retained bytes, how far the reader has
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
    cursor: usize,
    dropped_unread: usize,
    trimmed_total: usize,
}

impl Stream {
    fn push(&mut self, chunk: &[u8]) {
        self.bytes.extend_from_slice(chunk);
        if self.bytes.len() <= TRIM_TRIGGER_BYTES {
            return;
        }
        let drop = self.bytes.len() - MAX_BUFFER_BYTES;
        self.bytes.drain(..drop);
        self.trimmed_total += drop;
        self.dropped_unread += drop.saturating_sub(self.cursor);
        self.cursor = self.cursor.saturating_sub(drop);
    }

    fn drain(&mut self) -> (String, usize) {
        let text = String::from_utf8_lossy(&self.bytes[self.cursor..]).to_string();
        self.cursor = self.bytes.len();
        (text, std::mem::take(&mut self.dropped_unread))
    }

    #[cfg(test)]
    fn peek_unread(&self) -> std::borrow::Cow<'_, str> {
        String::from_utf8_lossy(&self.bytes[self.cursor.min(self.bytes.len())..])
    }

    fn window(&self, bytes: usize) -> Vec<u8> {
        self.bytes[self.bytes.len().saturating_sub(bytes)..].to_vec()
    }

    fn all(&self) -> (String, usize) {
        (
            String::from_utf8_lossy(&self.bytes).to_string(),
            self.trimmed_total,
        )
    }
}

struct BackgroundTask {
    started_at: DateTime<Utc>,
    command: String,
    description: Option<String>,
    /// INVARIANT: The secret VALUES this task's environment carries, from
    /// [`crate::core::subprocess_secret_values`]. Both exits redact against it.
    /// So a child that echoes its own environment cannot put a credential in
    /// the model's context or in the persisted `BackgroundBashCompleted`.
    ///
    /// Captured at spawn rather than passed to each reader, for the reason
    /// stated on the command above. A reader handed both the raw bytes and the
    /// secrets to hide is one transposition away from storing them.
    secrets: Vec<String>,
    stdout: Stream,
    stderr: Stream,
    /// INVARIANT: How the child ended. `None` until the watchdog writes it, in the same
    /// locked block as `finished_at` — so `finished_at.is_some()` and
    /// `outcome.is_some()` are always in step, and a still-running task can
    /// never present a status at all (as opposed to presenting a `0`).
    outcome: Option<TaskOutcome>,
    timed_out: bool,
    /// INVARIANT: A stop's signal reached a live child. Set from [`end_task`], never from
    /// the outcome: a graceful stop can end in a trap's clean exit.
    killed: bool,
    /// INVARIANT: The engine is stopping and took this task down as part of stopping.
    ///
    /// Set by [`BackgroundBashRegistry::hand_over_at_teardown`] in the same locked
    /// block that fires the kill, so the outcome the watchdog then reaps is
    /// already labelled. It outranks `killed` on the emitted event: both are
    /// our signal, and only this one says the work was not called off.
    abandoned: bool,
    finished_at: Option<DateTime<Utc>>,
    /// INVARIANT: Whether an emitter has claimed the right to write this task's
    /// `BackgroundBashCompleted`. **The one-shot gate that makes exactly one
    /// terminal event per task true.**
    ///
    /// Two callers race for it, both through
    /// [`BackgroundBashRegistry::completion_record`] and so both under this
    /// lock: the task's own watchdog watcher, and the teardown sweep once its
    /// kill has been reaped. Whichever wins builds the same event from the same
    /// record, and the loser writes nothing.
    ///
    /// It also exempts the entry from the retention CAP while it is false,
    /// because until someone claims it the completion exists nowhere but here.
    /// Expiry ignores the flag, so the exemption cannot pin memory. See
    /// [`sweep_finished`].
    completion_claimed: bool,
    delivery: CompletionDelivery,
    thread_id: Option<Uuid>,
    timing: watch::Receiver<TaskTiming>,
    kill_signal: Option<tokio::sync::oneshot::Sender<Stop>>,
    cut_grace: Arc<Notify>,
    /// INVARIANT: Signals the final `finished_at` write, and ONLY that — a buffered
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

#[derive(Clone, Copy)]
struct TaskTiming {
    deadline: DateTime<Utc>,
    queued: bool,
}

struct BuildSlotQueue {
    listener: tokio::net::UnixListener,
    _directory: tempfile::TempDir,
}

impl BuildSlotQueue {
    fn bind(command: &mut tokio::process::Command) -> Option<Self> {
        command.env_remove(lucidos_build_slot::ENV_WAIT_SOCKET);
        // INVARIANT: Unix socket paths have a short platform limit.
        let directory = tempfile::Builder::new()
            .prefix("lucidos-slot-")
            .tempdir_in("/tmp")
            .ok()?;
        let path = directory.path().join("wait");
        let listener = tokio::net::UnixListener::bind(&path).ok()?;
        command.env(lucidos_build_slot::ENV_WAIT_SOCKET, path);
        Some(Self {
            listener,
            _directory: directory,
        })
    }
}

const BACKGROUND_TASK_CAP: Duration = Duration::from_secs(24 * 60 * 60);

async fn build_slot_watchdog(
    queue: Option<BuildSlotQueue>,
    started: tokio::time::Instant,
    started_at: DateTime<Utc>,
    budget: Duration,
    timing: watch::Sender<TaskTiming>,
) {
    let Some(queue) = queue else {
        tokio::time::sleep_until(started + budget).await;
        return;
    };
    let cap = started + BACKGROUND_TASK_CAP.max(budget);
    let mut run_deadline = started + budget;
    let mut leases = tokio::task::JoinSet::new();
    let mut queued_at: Option<tokio::time::Instant> = None;
    let mut accept_after = started;
    loop {
        let deadline = if queued_at.is_some() {
            cap
        } else {
            run_deadline
        };
        if tokio::time::Instant::now() >= cap {
            return;
        }
        tokio::select! {
            biased;
            Some(_) = leases.join_next(), if !leases.is_empty() => {
                if leases.is_empty() {
                    let queued = queued_at.take().expect("active queue lease");
                    run_deadline = (run_deadline + queued.elapsed()).min(cap);
                    timing.send_replace(TaskTiming {
                        deadline: started_at + (run_deadline - started),
                        queued: false,
                    });
                }
            }
            _ = tokio::time::sleep_until(deadline) => return,
            result = async {
                if accept_after > tokio::time::Instant::now() {
                    tokio::time::sleep_until(accept_after).await;
                }
                queue.listener.accept().await
            } => {
                let Ok((mut stream, _)) = result else {
                    accept_after = tokio::time::Instant::now() + Duration::from_secs(1);
                    continue;
                };
                if queued_at.is_none() {
                    queued_at = Some(tokio::time::Instant::now());
                    timing.send_replace(TaskTiming {
                        deadline: started_at + (cap - started),
                        queued: true,
                    });
                }
                leases.spawn(async move { let _ = stream.read(&mut [0]).await; });
            }
        }
    }
}

impl BackgroundTask {
    #[cfg(test)]
    fn for_test() -> Self {
        BackgroundTask {
            started_at: Utc::now(),
            command: "cargo build".to_string(),
            description: None,
            secrets: Vec::new(),
            stdout: Stream::default(),
            stderr: Stream::default(),
            outcome: None,
            timed_out: false,
            killed: false,
            abandoned: false,
            finished_at: None,
            completion_claimed: false,
            delivery: CompletionDelivery::EventWait,
            thread_id: None,
            timing: watch::channel(TaskTiming {
                deadline: Utc::now(),
                queued: false,
            })
            .1,
            kill_signal: None,
            cut_grace: Arc::new(Notify::new()),
            finish_notify: Arc::new(Notify::new()),
        }
    }

    fn is_finished(&self) -> bool {
        self.finished_at.is_some()
    }

    fn stop_requested(&self) -> bool {
        !self.is_finished() && self.kill_signal.is_none()
    }

    fn is_expired(&self, now: DateTime<Utc>) -> bool {
        self.finished_at
            .is_some_and(|at| (now - at).num_seconds() > FINISHED_RETENTION_SECS)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunningTaskHandle {
    pub task_id: String,
    pub description: Option<String>,
    /// INVARIANT: Redacted and cut by [`command_prefix`]: safe to show the user.
    pub command: String,
    /// INVARIANT: When the watchdog kills the child if it has not exited. The task cannot
    /// outlive this, so a wait armed to at least this instant cannot be
    /// outlived by the task either.
    pub watchdog_deadline: DateTime<Utc>,
    pub queued_for_build_slot: bool,
    /// INVARIANT: A stop was already requested, so the engine must not arm a wait for it.
    /// Without this, `stop X; run Y` inside the stop's grace re-armed a wait
    /// on X, and X's own killed completion woke the thread that stopped it.
    pub stop_requested: bool,
    /// INVARIANT: The owner polls for this task's result, so no engine wait arms for it.
    pub polled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskProgress {
    pub what: String,
    pub running_secs: i64,
    pub latest_output: String,
}

const PROGRESS_TAIL_BYTES: usize = 1_500;

/// INVARIANT: The raw window a progress tail is redacted over. Wider than the tail, so
/// a secret cut at the window's front edge falls outside the tail kept.
const PROGRESS_WINDOW_BYTES: usize = 16 * 1024;

/// INVARIANT: The right to stop one task, taken by [`BackgroundBashRegistry::begin_stop`]
/// and not yet used.
///
/// Dropping it unsent still stops the task, at once rather than gracefully:
/// the watchdog reads a closed channel as [`Stop::Immediate`].
#[derive(Debug)]
pub struct PendingStop {
    owner: Option<Uuid>,
    signal: tokio::sync::oneshot::Sender<Stop>,
}

impl PendingStop {
    pub fn owner(&self) -> Option<Uuid> {
        self.owner
    }

    pub fn send(self) {
        let _ = self.signal.send(Stop::Graceful);
    }
}

/// INVARIANT: One task the teardown is responsible for recording, as
/// [`BackgroundBashRegistry::hand_over_at_teardown`] hands it over.
///
/// Not "abandoned": most were, but a task an earlier `bash_kill` already ended
/// is in here too, and still needs its completion written before the process
/// goes. Only what the caller needs to go back for the record once the watchdog
/// has reaped it. Nothing about the outcome, because there is not one yet.
#[derive(Debug, Clone)]
pub struct TeardownTask {
    pub thread_id: Uuid,
    pub task_id: String,
}

/// INVARIANT: The final state of a completed task, read for the
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
    pub command: String,
    /// INVARIANT: A finished task always has an outcome (the watchdog writes it in the
    /// same locked block as `finished_at`), so this is not optional. A status
    /// the engine could not obtain is [`TaskOutcome::Unknown`], which renders
    /// as words rather than as a `0`.
    pub outcome: TaskOutcome,
    pub timed_out: bool,
    pub killed: bool,
    pub abandoned: bool,
    pub stdout: String,
    pub stdout_dropped: usize,
    pub stderr: String,
    pub stderr_dropped: usize,
}

#[derive(Debug, Clone)]
pub struct OutputSnapshot {
    pub stdout: String,
    pub stderr: String,
    pub stdout_dropped: usize,
    pub stderr_dropped: usize,
    pub outcome: Option<TaskOutcome>,
    pub finished: bool,
    pub timed_out: bool,
    pub killed: bool,
    /// INVARIANT: The engine's own shutdown ended this task. Derived by [`ending`], the
    /// same function the persisted row uses, because the two are one tool call
    /// to the LLM and must never disagree. Reachable in-memory as well as from
    /// the event store: a drain landing after the teardown's kill, but inside
    /// the retention window, reads a task this process abandoned.
    pub abandoned: bool,
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
            unread_results: Arc::new(std::sync::Mutex::new(HashMap::new())),
        }
    }

    async fn locked(&self) -> tokio::sync::MutexGuard<'_, HashMap<String, BackgroundTask>> {
        let mut tasks = self.tasks.lock().await;
        sweep_finished(&mut tasks);
        tasks
    }

    #[cfg(any(test, feature = "test-support"))]
    pub async fn spawn(
        &self,
        command: &str,
        timeout_secs: u64,
        cwd: &Path,
        env: &[(String, String)],
        thread_id: Option<Uuid>,
        description: Option<&str>,
    ) -> Result<
        (String, tokio::sync::oneshot::Receiver<()>),
        Box<dyn std::error::Error + Send + Sync>,
    > {
        let spec = TaskSpec {
            timeout_secs,
            description,
            delivery: CompletionDelivery::EventWait,
            inherited: crate::core::InheritedSecrets::Keep,
        };
        self.spawn_task(&Sandbox::unsandboxed(), command, cwd, env, thread_id, spec)
            .await
    }

    /// INVARIANT: Spawn a child process. Inserts the task into the registry before
    /// returning the task_id, eliminating the spawn/poll race a follow-up
    /// `bash_output` would otherwise hit. The returned receiver fires
    /// when the watchdog marks the task finished, and the dispatch site uses
    /// it to read a [`CompletionRecord`] and emit `BackgroundBashCompleted`.
    /// That read does NOT evict: the entry stays drainable for
    /// [`FINISHED_RETENTION_SECS`] afterwards.
    ///
    /// `thread_id` records the spawning thread so `has_running_for_thread`
    /// can answer "does this thread still have unfinished background bash?".
    /// Production callers (`LucidosEngine::start_background_task`) always pass
    /// `Some(thread_id)`; tests typically pass `None`.
    ///
    /// A blank `spec.description` counts as none, so the wait falls back to the
    /// command rather than to nothing. `spec.delivery` is set in the same
    /// locked insert that makes the task visible, so no wait can arm for a
    /// polled task in between.
    pub async fn spawn_task(
        &self,
        sandbox: &Sandbox,
        command: &str,
        cwd: &Path,
        env: &[(String, String)],
        thread_id: Option<Uuid>,
        spec: TaskSpec<'_>,
    ) -> Result<
        (String, tokio::sync::oneshot::Receiver<()>),
        Box<dyn std::error::Error + Send + Sync>,
    > {
        let TaskSpec {
            timeout_secs,
            description,
            delivery,
            inherited,
        } = spec;
        let task_id = Uuid::new_v4().to_string();

        let mut cmd = command_shell().command(sandbox, command)?;
        cmd.current_dir(cwd)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);
        crate::core::apply_to_subprocess_env(&mut cmd, env, inherited);

        let queue = BuildSlotQueue::bind(&mut cmd);
        let started = tokio::time::Instant::now();
        let started_at = Utc::now();
        let budget = Duration::from_secs(timeout_secs);
        let (timing_tx, timing) = watch::channel(TaskTiming {
            deadline: started_at + budget,
            queued: false,
        });
        let mut child = crate::runtime::spawn_env::spawn_below_engine(&mut cmd)?;
        // INVARIANT: Read now: `id()` goes `None` once the child is reaped, and the group
        // signals below must never outlive the reap.
        let pid = child.id();
        let stdout = child.stdout.take().expect("piped stdout");
        let stderr = child.stderr.take().expect("piped stderr");

        let (kill_tx, kill_rx) = tokio::sync::oneshot::channel();
        let cut_grace = Arc::new(Notify::new());
        let (finish_tx, finish_rx) = tokio::sync::oneshot::channel::<()>();

        let safe_prefix = command_prefix(&crate::core::redact_postgres_secrets(command));
        let secrets =
            crate::core::subprocess_secret_values(env, crate::core::inherited_secret_env());

        {
            let mut tasks = self.locked().await;
            tasks.insert(
                task_id.clone(),
                BackgroundTask {
                    started_at,
                    command: safe_prefix,
                    description: description
                        .map(str::trim)
                        .filter(|d| !d.is_empty())
                        .map(String::from),
                    secrets,
                    stdout: Stream::default(),
                    stderr: Stream::default(),
                    outcome: None,
                    timed_out: false,
                    killed: false,
                    abandoned: false,
                    finished_at: None,
                    completion_claimed: false,
                    delivery,
                    thread_id,
                    timing,
                    kill_signal: Some(kill_tx),
                    cut_grace: cut_grace.clone(),
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
        let unread_results = self.unread_results.clone();
        let id = task_id.clone();
        let awake = crate::core::keep_awake::hold(
            crate::core::keep_awake::Work::BackgroundTask,
            task_id.clone(),
        );
        tokio::spawn(async move {
            let _awake = awake;
            let timeout_fut = build_slot_watchdog(queue, started, started_at, budget, timing_tx);
            tokio::pin!(timeout_fut);
            let mut timed_out = false;
            let mut killed = false;
            let outcome: TaskOutcome = tokio::select! {
                exit = child.wait() => TaskOutcome::from_wait(exit),
                _ = &mut timeout_fut => {
                    let (outcome, signalled) =
                        end_task(&mut child, pid, Stop::Graceful, &cut_grace).await;
                    timed_out = signalled;
                    outcome
                }
                stop = kill_rx => {
                    let stop = stop.unwrap_or(Stop::Immediate);
                    let (outcome, signalled) = end_task(&mut child, pid, stop, &cut_grace).await;
                    killed = signalled;
                    outcome
                }
            };
            // INVARIANT: Wait for both drain tasks to flush remaining buffered bytes
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
                let stop_taken = task.kill_signal.take().is_none();
                // INVARIANT: Wakes every parked waiter. A finish that lands before a
                // waiter registers is NOT lost: `read_output_in_memory_wait`
                // registers first and re-checks `is_finished()` under the
                // lock afterwards, so each waiter either sees the flag or is
                // already parked to be woken here. That re-check is what makes
                // this correct for N waiters — a `notify_one` permit would
                // rescue exactly one of them and strand the rest for their
                // full budget on an already-finished task.
                task.finish_notify.notify_waiters();
                // INVARIANT: Under the task lock, so a drain that sees the task finished
                // also sees it unread, and its read removes it.
                // Before the completion is emitted, so the wait that delivers
                // it always finds the entry to remove.
                let unread_result = task.delivery == CompletionDelivery::Poll && !stop_taken;
                if let Some(owner) = task.thread_id.filter(|_| unread_result) {
                    unread(&unread_results)
                        .entry(owner)
                        .or_default()
                        .push(UnreadResult {
                            task_id: id.clone(),
                            description: task.description.clone(),
                            command: task.command.clone(),
                        });
                }
            }
            let _ = finish_tx.send(());
        });

        Ok((task_id, finish_rx))
    }

    /// INVARIANT: Drain the in-memory buffer for a running or recently-finished task.
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
        let finish_notify = {
            let mut tasks = self.locked().await;
            let task = tasks.get_mut(task_id)?;
            if task.is_finished() || wait.is_zero() {
                return Some(drain_snapshot(task));
            }
            task.finish_notify.clone()
        };
        // INVARIANT: Register BEFORE re-checking, then re-check under the lock. The
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
        let _ = tokio::time::timeout(wait, notified).await;
        let mut tasks = self.locked().await;
        let task = tasks.get_mut(task_id)?;
        Some(drain_snapshot(task))
    }

    /// INVARIANT: Take the right to stop a running task, without stopping it yet. `None`
    /// if the task is unknown, already finished, or already being stopped:
    /// retention makes a finished task readable, never stoppable.
    ///
    /// Two steps, so a thread stopping its own task can stand down its waits
    /// on it BEFORE the signal goes out. A child can exit on SIGTERM and emit
    /// its completion within milliseconds. A stand-down after the signal
    /// would race the very delivery it exists to prevent. From here on the
    /// task is *stop requested*, so no engine wait arms for it
    /// ([`RunningTaskHandle::stop_requested`]).
    pub async fn begin_stop(&self, task_id: &str) -> Option<PendingStop> {
        let mut tasks = self.locked().await;
        let task = tasks.get_mut(task_id)?;
        if task.is_finished() {
            return None;
        }
        let signal = task.kill_signal.take()?;
        Some(PendingStop {
            owner: task.thread_id,
            signal,
        })
    }

    #[cfg(test)]
    pub async fn kill(&self, task_id: &str) -> bool {
        match self.begin_stop(task_id).await {
            Some(stop) => {
                stop.send();
                true
            }
            None => false,
        }
    }

    pub async fn has_running_for_thread(&self, thread_id: Uuid) -> bool {
        let tasks = self.locked().await;
        tasks
            .values()
            .any(|t| t.thread_id == Some(thread_id) && !t.is_finished())
    }

    pub async fn running_for_thread(&self, thread_id: Uuid) -> Vec<RunningTaskHandle> {
        let tasks = self.locked().await;
        tasks
            .iter()
            .filter(|(_, t)| t.thread_id == Some(thread_id) && !t.is_finished())
            .map(|(task_id, t)| {
                let timing = *t.timing.borrow();
                RunningTaskHandle {
                    task_id: task_id.clone(),
                    description: t.description.clone(),
                    command: t.command.clone(),
                    watchdog_deadline: timing.deadline,
                    queued_for_build_slot: timing.queued,
                    stop_requested: t.stop_requested(),
                    polled: t.delivery == CompletionDelivery::Poll,
                }
            })
            .collect()
    }

    pub async fn progress_for_thread(&self, thread_id: Uuid) -> Vec<TaskProgress> {
        let windows: Vec<_> = {
            let tasks = self.locked().await;
            tasks
                .values()
                .filter(|t| t.thread_id == Some(thread_id) && !t.is_finished())
                .filter(|t| !t.stop_requested())
                .map(|t| {
                    (
                        t.description.clone().unwrap_or_else(|| t.command.clone()),
                        (Utc::now() - t.started_at).num_seconds().max(0),
                        t.stdout.window(PROGRESS_WINDOW_BYTES),
                        t.stderr.window(PROGRESS_WINDOW_BYTES),
                        t.secrets.clone(),
                    )
                })
                .collect()
        };
        windows
            .into_iter()
            .map(|(what, running_secs, stdout, stderr, secrets)| {
                let output = [stdout, stderr]
                    .iter()
                    .map(|raw| String::from_utf8_lossy(raw))
                    .filter(|text| !text.trim().is_empty())
                    .collect::<Vec<_>>()
                    .join("\n");
                let output = crate::core::redact_subprocess_output(&output, &secrets);
                let cut =
                    output.ceil_char_boundary(output.len().saturating_sub(PROGRESS_TAIL_BYTES));
                TaskProgress {
                    what,
                    running_secs,
                    latest_output: output[cut..].to_string(),
                }
            })
            .collect()
    }

    pub async fn is_running(&self, task_id: &str) -> bool {
        let tasks = self.locked().await;
        tasks.get(task_id).is_some_and(|t| !t.is_finished())
    }

    pub async fn spawned_by(&self, task_id: &str, thread_id: Uuid) -> Option<bool> {
        let tasks = self.locked().await;
        tasks.get(task_id).map(|t| t.thread_id == Some(thread_id))
    }

    pub async fn kill_for_thread(&self, thread_id: Uuid) -> usize {
        let mut tasks = self.locked().await;
        unread(&self.unread_results).remove(&thread_id);
        tasks
            .values_mut()
            .filter(|t| t.thread_id == Some(thread_id) && !t.is_finished())
            .filter_map(|t| t.kill_signal.take())
            .map(|tx| {
                let _ = tx.send(Stop::Graceful);
            })
            .count()
    }

    pub fn unread_results_for_thread(&self, thread_id: Uuid) -> Vec<UnreadResult> {
        unread(&self.unread_results)
            .get(&thread_id)
            .cloned()
            .unwrap_or_default()
    }

    pub fn mark_result_read(&self, reader: Uuid, task_id: &str) {
        forget_unread(&mut unread(&self.unread_results), reader, &[task_id]);
    }

    /// INVARIANT: Hand the teardown every task whose completion nobody has written yet,
    /// killing the ones still running. Called once, from graceful teardown.
    ///
    /// **Unclaimed, not unfinished.** A task that reached `finished_at` just
    /// before the signal has a watcher that may never be scheduled again.
    /// Filtering on "still running" dropped it, so the next boot reported a
    /// success as an engine loss. It needs recording, not killing.
    ///
    /// **Kills rather than merely recording**, so "this did not finish" is true
    /// when the caller writes it. The caller's own docs carry that argument.
    ///
    /// **Marks only a task whose kill signal it actually takes**, but returns
    /// every one regardless. An earlier `bash_kill` has spent that signal and
    /// owns the ending under its own name. The task still needs recording. A
    /// task with no owning thread is the one real skip.
    ///
    /// The mark is not the last word: [`ending`] drops it unless our signal is
    /// what ended the task. The caller then takes the record through
    /// [`Self::completion_record`], where the one-shot gate lives.
    pub async fn hand_over_at_teardown(&self) -> Vec<TeardownTask> {
        let mut tasks = self.locked().await;
        tasks
            .iter_mut()
            .filter(|(_, t)| !t.completion_claimed)
            .filter_map(|(task_id, t)| {
                let thread_id = t.thread_id?;
                if !t.is_finished() {
                    if let Some(tx) = t.kill_signal.take() {
                        t.abandoned = true;
                        let _ = tx.send(Stop::Immediate);
                    }
                    // INVARIANT: A stop or a timeout already in its grace no longer hears
                    // the channel, and the grace would outlast `REAP_WAIT`.
                    t.cut_grace.notify_one();
                }
                Some(TeardownTask {
                    thread_id,
                    task_id: task_id.clone(),
                })
            })
            .collect()
    }

    /// INVARIANT: Block until this task's watchdog has written `finished_at`, or `wait`
    /// runs out. `false` means it is still running, or is gone entirely.
    ///
    /// The teardown's join. Same register-then-recheck shape as
    /// [`Self::read_output_in_memory_wait`], for the same reason: the watchdog
    /// fires `notify_waiters`, which leaves no permit for a waiter that has not
    /// registered yet.
    pub async fn wait_until_finished(&self, task_id: &str, wait: Duration) -> bool {
        let finish_notify = {
            let tasks = self.locked().await;
            let Some(task) = tasks.get(task_id) else {
                return false;
            };
            if task.is_finished() {
                return true;
            }
            task.finish_notify.clone()
        };
        let notified = finish_notify.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        {
            let tasks = self.locked().await;
            if tasks.get(task_id).is_some_and(|t| t.is_finished()) {
                return true;
            }
        }
        let _ = tokio::time::timeout(wait, notified).await;
        let tasks = self.locked().await;
        tasks.get(task_id).is_some_and(|t| t.is_finished())
    }

    /// INVARIANT: Read a finished task's final state, for the `BackgroundBashCompleted`
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
    /// **One-shot.** It claims [`BackgroundTask::completion_claimed`], and a
    /// second call returns `None` even though the entry is still there. The
    /// caller writes `BackgroundBashCompleted` from what this returns. Handing
    /// it out twice would put two terminal events on one task, and
    /// `bash_output` reads the last row. The other claimant is
    /// [`Self::hand_over_at_teardown`], whose caller records what it killed.
    ///
    /// The claim is also what makes the entry a candidate for the sweep's cap:
    /// until someone has it, the task's completion exists nowhere durable, so
    /// the cap must not drop it. See [`sweep_finished`].
    pub async fn completion_record(&self, task_id: &str) -> Option<CompletionRecord> {
        let mut tasks = self.locked().await;
        let task = tasks.get_mut(task_id)?;
        let finished_at = task.finished_at?;
        if task.completion_claimed {
            return None;
        }
        task.completion_claimed = true;
        let outcome = task.outcome.unwrap_or(TaskOutcome::Unknown);
        let (abandoned, killed) = ending(&*task);
        let (stdout, stdout_dropped) = task.stdout.all();
        let (stderr, stderr_dropped) = task.stderr.all();
        // INVARIANT: The caller writes this straight into `BackgroundBashCompleted`, which
        // is permanent. Redact over the whole retained buffer, so no drain
        // boundary can split a token here. `Stream::push` can still have
        // trimmed the front mid-token on a chatty task, which no call site can
        // undo. `core::subprocess_secret_values` records both gaps.
        let stdout = crate::core::redact_subprocess_output(&stdout, &task.secrets);
        let stderr = crate::core::redact_subprocess_output(&stderr, &task.secrets);
        Some(CompletionRecord {
            started_at: task.started_at,
            finished_at,
            command: task.command.clone(),
            outcome,
            timed_out: task.timed_out,
            killed,
            abandoned,
            stdout,
            stdout_dropped,
            stderr,
            stderr_dropped,
        })
    }

    /// INVARIANT: Hand a claim back, for an emitter whose write failed.
    ///
    /// The claim is a promise to persist `BackgroundBashCompleted`, not proof
    /// that it happened. An emitter that took it and then failed leaves nothing
    /// durable, and holding the claim tells the teardown the task is settled.
    /// The boot sweep would then report a success as an engine loss.
    ///
    /// Safe to call for a task that is gone: the entry may have been swept.
    pub async fn release_completion_claim(&self, task_id: &str) {
        if let Some(task) = self.locked().await.get_mut(task_id) {
            task.completion_claimed = false;
        }
    }

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

    #[cfg(any(test, feature = "test-support"))]
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

    #[cfg(test)]
    pub async fn retained_finished_count(&self) -> usize {
        self.locked()
            .await
            .values()
            .filter(|t| t.is_finished())
            .count()
    }
}

/// INVARIANT: Which of the two engine-side endings this was: `(abandoned, killed)`.
///
/// Decided once, here, so no reader re-derives the precedence. Both come down
/// the same channel, and `BackgroundTask::abandoned` is the only record of who
/// sent the signal.
///
/// **Neither is claimed unless our signal reached a live child**, which is
/// exactly `BackgroundTask::killed` (see [`end_task`]). The outcome cannot
/// decide it: a graceful stop can end in a trap's `exit 143` or even `exit 0`.
/// Three other endings reach here with a signal sent or in flight, and each
/// keeps its own name.
///
/// - A normal exit just before the signal. `end_task` finds the child already
///   gone and reports its verdict, so a release that exited 0 really shipped.
/// - The watchdog's own budget, which never takes the kill signal and so is
///   still markable. It reports `timed_out`.
/// - A signal from outside, a SIGSEGV say, which must keep its own number.
fn ending(task: &BackgroundTask) -> (bool, bool) {
    (
        task.killed && task.abandoned,
        task.killed && !task.abandoned,
    )
}

/// INVARIANT: End a task the watchdog was told to stop, and reap it. Returns the outcome
/// and whether our signal reached a live child.
///
/// A child that already exited keeps its own verdict and is not signalled:
/// `try_wait` reaps it, and a reaped pid must never be named again. Otherwise
/// the signals go to the whole process group while the leader is unreaped,
/// per `spawn_env::signal_child_process_group`. `cut_grace` ends a graceful
/// stop's grace early, for a teardown that cannot wait it out.
async fn end_task(
    child: &mut tokio::process::Child,
    pid: Option<u32>,
    stop: Stop,
    cut_grace: &Notify,
) -> (TaskOutcome, bool) {
    if let Ok(Some(status)) = child.try_wait() {
        return (TaskOutcome::from_wait(Ok(status)), false);
    }
    if let Some(pid) = pid {
        match stop {
            Stop::Graceful => {
                crate::runtime::spawn_env::graceful_kill_child_process_group_or_sooner(
                    pid,
                    crate::runtime::claude_code::GROUP_TEARDOWN_GRACE,
                    cut_grace.notified(),
                )
                .await
            }
            Stop::Immediate => crate::runtime::spawn_env::kill_child_process_group_now(pid),
        }
    }
    let _ = child.kill().await;
    (TaskOutcome::from_wait(child.wait().await), true)
}

/// INVARIANT: Apply the retention policy: drop every completed task past
/// [`FINISHED_RETENTION_SECS`], then, if more than
/// [`MAX_RETAINED_FINISHED`] *recorded* completions remain, drop the oldest
/// until the count is back at the cap.
///
/// Two kinds of entry are never touched, for the same reason: they are not
/// retention candidates yet.
///
/// - **Running tasks**, however long they have been going. They are live
///   state.
/// - **Completions nobody has claimed** ([`BackgroundTask::completion_claimed`]),
///   on the CAP pass. That claim is what the watcher takes immediately before
///   emitting `BackgroundBashCompleted`, so evicting one first would leave the
///   task with no durable record at all: the watcher finds nothing, emits
///   nothing, and every later `bash_output` reports an unknown task. That is the
///   exact loss this whole change exists to prevent, so a burst of more than
///   `MAX_RETAINED_FINISHED` simultaneous completions must overshoot the cap
///   briefly rather than drop one.
///
/// The EXPIRY pass is deliberately unconditional, and it is the only thing
/// bounding the overshoot. An unclaimed completion is USUALLY a transient: the
/// watcher is a spawned task already parked on the finish signal, so it claims
/// within microseconds. A claim handed back by a failed emit is not, and a
/// database outage hands back every one for as long as it lasts. Expiry still
/// caps that at the retention window. Gating expiry on the claim too would pin
/// an entry forever, and unbounded memory is the worse failure.
fn sweep_finished(tasks: &mut HashMap<String, BackgroundTask>) {
    let now = Utc::now();
    tasks.retain(|_, t| !t.is_expired(now));

    let excess = tasks
        .values()
        .filter(|t| t.completion_claimed)
        .count()
        .saturating_sub(MAX_RETAINED_FINISHED);
    if excess == 0 {
        return;
    }
    let mut recorded: Vec<(DateTime<Utc>, String)> = tasks
        .iter()
        .filter(|(_, t)| t.completion_claimed)
        .filter_map(|(id, t)| t.finished_at.map(|at| (at, id.clone())))
        .collect();
    // INVARIANT: Oldest completion first: the newest are the ones an agent may still be
    // about to drain, so they are the last to go.
    recorded.sort_unstable_by(|a, b| a.0.cmp(&b.0));
    for (_, id) in recorded.into_iter().take(excess) {
        tasks.remove(&id);
    }
}

fn drain_snapshot(task: &mut BackgroundTask) -> OutputSnapshot {
    let (stdout, stdout_dropped) = task.stdout.drain();
    let (stderr, stderr_dropped) = task.stderr.drain();
    let stdout = crate::core::redact_subprocess_output(&stdout, &task.secrets);
    let stderr = crate::core::redact_subprocess_output(&stderr, &task.secrets);
    let until = task.finished_at.unwrap_or_else(Utc::now);
    let (abandoned, killed) = ending(task);
    OutputSnapshot {
        stdout,
        stderr,
        stdout_dropped,
        stderr_dropped,
        outcome: task.outcome,
        finished: task.is_finished(),
        timed_out: task.timed_out,
        killed,
        abandoned,
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
            Ok(0) => return,
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
                // INVARIANT: Deliberately no notify here: a buffered chunk must not end
                // a `bash_output(wait_secs=N)` block. See `finish_notify`.
            }
            Err(_) => return,
        }
    }
}

pub fn finalize_drain(text: &str, already_dropped: usize) -> String {
    let sanitized = crate::core::sanitize_for_jsonb(text);
    truncate_output_tail(
        &sanitized,
        crate::core::shell::MAX_OUTPUT_BYTES,
        already_dropped,
    )
}

pub fn truncate_output_tail(s: &str, max: usize, already_dropped: usize) -> String {
    if s.len() <= max && already_dropped == 0 {
        return s.to_string();
    }
    let start = if s.len() > max {
        s.ceil_char_boundary(s.len() - max)
    } else {
        0
    };
    format!(
        "[truncated — {} earlier bytes dropped, showing the most recent {} of {} total]\n...{}",
        already_dropped + start,
        s.len() - start,
        already_dropped + s.len(),
        &s[start..]
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::time::{advance, Instant};

    const TEST_RUN_BUDGET: Duration = Duration::from_secs(5);

    fn queued_watchdog() -> (
        std::os::unix::net::UnixStream,
        Instant,
        DateTime<Utc>,
        watch::Receiver<TaskTiming>,
        tokio::task::JoinHandle<()>,
    ) {
        let mut command = tokio::process::Command::new("true");
        let queue = BuildSlotQueue::bind(&mut command).expect("build-slot listener");
        let address = queue.listener.local_addr().unwrap();
        let lease =
            std::os::unix::net::UnixStream::connect(address.as_pathname().unwrap()).unwrap();
        let started = Instant::now();
        let started_at = Utc::now();
        let (tx, rx) = watch::channel(TaskTiming {
            deadline: started_at + TEST_RUN_BUDGET,
            queued: false,
        });
        let watchdog = tokio::spawn(build_slot_watchdog(
            Some(queue),
            started,
            started_at,
            TEST_RUN_BUDGET,
            tx,
        ));
        (lease, started, started_at, rx, watchdog)
    }

    async fn wait_for_queue_state(timing: &watch::Receiver<TaskTiming>, queued: bool) {
        let started = std::time::Instant::now();
        while timing.borrow().queued != queued {
            assert!(
                started.elapsed() < Duration::from_secs(5),
                "missing queue transition"
            );
            tokio::task::yield_now().await;
        }
    }

    #[tokio::test(start_paused = true)]
    async fn a_build_slot_lease_pauses_the_run_clock_until_grant() {
        let (lease, started, started_at, timing, watchdog) = queued_watchdog();
        wait_for_queue_state(&timing, true).await;
        assert_eq!(timing.borrow().deadline, started_at + BACKGROUND_TASK_CAP);

        let waited = Duration::from_secs(300);
        advance(waited).await;
        assert!(!watchdog.is_finished());
        drop(lease);
        wait_for_queue_state(&timing, false).await;
        assert_eq!(
            timing.borrow().deadline,
            started_at + waited + TEST_RUN_BUDGET
        );
        watchdog.await.unwrap();
        assert!(started.elapsed() >= waited + TEST_RUN_BUDGET);
    }

    #[tokio::test(start_paused = true)]
    async fn a_queued_watchdog_still_ends_at_the_absolute_cap() {
        let (_held_lease, started, _, timing, watchdog) = queued_watchdog();
        wait_for_queue_state(&timing, true).await;
        advance(BACKGROUND_TASK_CAP - Duration::from_secs(1)).await;
        assert!(!watchdog.is_finished());
        watchdog.await.unwrap();
        assert!(started.elapsed() >= BACKGROUND_TASK_CAP);
    }

    #[tokio::test]
    async fn spawn_returns_task_id_and_output_is_drainable() {
        let reg = BackgroundBashRegistry::new();
        let (task_id, _finish_rx) = reg
            .spawn("echo hi", 5, std::path::Path::new("/tmp"), &[], None, None)
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

    const SECRET: &str = "sk-test-abcdef0123456789";

    fn secret_env() -> Vec<(String, String)> {
        vec![("CRED_DEMO_TOKEN".to_string(), SECRET.to_string())]
    }

    #[tokio::test]
    async fn a_background_drain_redacts_the_injected_credential() {
        let reg = BackgroundBashRegistry::new();
        let (task_id, _finish_rx) = reg
            .spawn(
                "echo \"$CRED_DEMO_TOKEN\"",
                5,
                std::path::Path::new("/tmp"),
                &secret_env(),
                None,
                None,
            )
            .await
            .expect("spawn");
        assert!(reg.wait_for_finish(&task_id, Duration::from_secs(5)).await);

        let snap = reg
            .read_output_in_memory_wait(&task_id, Duration::ZERO)
            .await
            .expect("snapshot");
        assert!(
            !snap.stdout.contains(SECRET),
            "the drain handed the model a live credential: {:?}",
            snap.stdout
        );
        assert!(
            snap.stdout.contains("[REDACTED]"),
            "the secret was neither redacted nor echoed: {:?}",
            snap.stdout
        );
    }

    #[tokio::test]
    async fn the_persisted_completion_record_redacts_the_injected_credential() {
        let reg = BackgroundBashRegistry::new();
        let (task_id, _finish_rx) = reg
            .spawn(
                "echo \"$CRED_DEMO_TOKEN\" >&2; echo \"$CRED_DEMO_TOKEN\"",
                5,
                std::path::Path::new("/tmp"),
                &secret_env(),
                None,
                None,
            )
            .await
            .expect("spawn");
        assert!(reg.wait_for_finish(&task_id, Duration::from_secs(5)).await);

        let record = reg
            .completion_record(&task_id)
            .await
            .expect("completion record for a finished task");
        assert!(
            !record.stdout.contains(SECRET) && !record.stderr.contains(SECRET),
            "BackgroundBashCompleted would store a live credential: {:?} / {:?}",
            record.stdout,
            record.stderr
        );
        assert!(record.stdout.contains("[REDACTED]"));
        assert!(record.stderr.contains("[REDACTED]"));
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
                None,
            )
            .await
            .expect("spawn");

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

        let finished = reg.wait_for_finish(&task_id, Duration::from_secs(60)).await;
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
            .spawn(
                "sleep 30",
                60,
                std::path::Path::new("/tmp"),
                &[],
                None,
                None,
            )
            .await
            .expect("spawn");

        tokio::time::sleep(Duration::from_millis(100)).await;

        let killed = reg.kill(&task_id).await;
        assert!(killed, "kill should return true for a running task");

        let finished = reg.wait_for_finish(&task_id, Duration::from_secs(20)).await;
        assert!(finished, "killed task did not transition to finished");

        let snap = reg
            .read_output_in_memory_wait(&task_id, Duration::ZERO)
            .await
            .expect("snapshot");
        assert!(snap.finished);
        assert!(snap.killed, "killed flag should be true");
        assert!(!snap.timed_out);
        assert_eq!(snap.outcome, Some(TaskOutcome::Signaled(15)));
    }

    async fn hold_released(task_id: &str) -> bool {
        for _ in 0..100 {
            if !crate::core::keep_awake::is_held(task_id) {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        false
    }

    #[tokio::test]
    async fn a_task_keeps_the_computer_awake_until_it_ends() {
        let reg = BackgroundBashRegistry::new();
        let tmp = std::path::Path::new("/tmp");
        for command in ["sleep 0.3", "sleep 0.3; exit 3"] {
            let (task_id, finish_rx) = reg.spawn(command, 60, tmp, &[], None, None).await.unwrap();
            assert!(crate::core::keep_awake::is_held(&task_id), "{command}");
            finish_rx.await.unwrap();
            assert!(hold_released(&task_id).await, "{command} leaked its hold");
        }

        let (task_id, finish_rx) = reg
            .spawn("sleep 30", 60, tmp, &[], None, None)
            .await
            .unwrap();
        assert!(crate::core::keep_awake::is_held(&task_id));
        assert!(reg.kill(&task_id).await);
        finish_rx.await.unwrap();
        assert!(
            hold_released(&task_id).await,
            "a killed task leaked its hold"
        );
    }

    #[tokio::test]
    async fn kill_returns_false_for_unknown_task() {
        let reg = BackgroundBashRegistry::new();
        assert!(!reg.kill("does-not-exist").await);
    }

    #[cfg(unix)]
    fn pid_alive(pid: i32) -> bool {
        // SAFETY: signal 0 only checks that the pid exists and may be signalled.
        unsafe { libc::kill(pid, 0) == 0 }
    }

    #[cfg(unix)]
    async fn pid_gone_within(pid: i32, budget: Duration) -> bool {
        let deadline = tokio::time::Instant::now() + budget;
        while tokio::time::Instant::now() < deadline {
            if !pid_alive(pid) {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        !pid_alive(pid)
    }

    #[cfg(unix)]
    async fn spawn_with_grandchild(
        reg: &BackgroundBashRegistry,
        dir: &Path,
        timeout_secs: u64,
        thread_id: Option<Uuid>,
    ) -> (String, i32) {
        let pidfile = dir.join("grandchild.pid");
        let command = format!("sleep 300 & echo $! > {}; wait", pidfile.display());
        let (task_id, _finish_rx) = reg
            .spawn(&command, timeout_secs, dir, &[], thread_id, None)
            .await
            .expect("spawn");
        let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
        loop {
            if let Some(pid) = std::fs::read_to_string(&pidfile)
                .ok()
                .and_then(|s| s.trim().parse::<i32>().ok())
            {
                assert!(pid_alive(pid), "the grandchild died before the test began");
                return (task_id, pid);
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "the grandchild never wrote its pid"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    #[tokio::test]
    #[cfg(unix)]
    async fn kill_ends_the_whole_process_group() {
        let dir = tempfile::tempdir().expect("tempdir");
        let reg = BackgroundBashRegistry::new();
        let (task_id, grandchild) = spawn_with_grandchild(&reg, dir.path(), 600, None).await;

        assert!(reg.kill(&task_id).await);
        assert!(reg.wait_for_finish(&task_id, Duration::from_secs(20)).await);
        assert!(
            pid_gone_within(grandchild, Duration::from_secs(5)).await,
            "the stop left the task's grandchild running"
        );
    }

    #[tokio::test]
    #[cfg(unix)]
    async fn timeout_ends_the_whole_process_group() {
        let dir = tempfile::tempdir().expect("tempdir");
        let reg = BackgroundBashRegistry::new();
        let (task_id, grandchild) = spawn_with_grandchild(&reg, dir.path(), 5, None).await;

        assert!(reg.wait_for_finish(&task_id, Duration::from_secs(30)).await);
        assert!(
            pid_gone_within(grandchild, Duration::from_secs(5)).await,
            "the timeout left the task's grandchild running"
        );
    }

    #[tokio::test]
    #[cfg(unix)]
    async fn kill_for_thread_ends_the_whole_process_group() {
        let dir = tempfile::tempdir().expect("tempdir");
        let reg = BackgroundBashRegistry::new();
        let thread = Uuid::new_v4();
        let (task_id, grandchild) =
            spawn_with_grandchild(&reg, dir.path(), 600, Some(thread)).await;

        assert_eq!(reg.kill_for_thread(thread).await, 1);
        assert!(reg.wait_for_finish(&task_id, Duration::from_secs(20)).await);
        assert!(pid_gone_within(grandchild, Duration::from_secs(5)).await);
    }

    #[tokio::test]
    #[cfg(unix)]
    async fn teardown_ends_the_whole_process_group_without_a_grace() {
        let dir = tempfile::tempdir().expect("tempdir");
        let reg = BackgroundBashRegistry::new();
        let (task_id, grandchild) =
            spawn_with_grandchild(&reg, dir.path(), 600, Some(Uuid::new_v4())).await;

        assert_eq!(reg.hand_over_at_teardown().await.len(), 1);
        assert!(
            reg.wait_until_finished(&task_id, Duration::from_secs(2))
                .await,
            "the teardown's kill must reap well inside its 3 s budget"
        );
        assert!(pid_gone_within(grandchild, Duration::from_secs(5)).await);
    }

    #[tokio::test]
    #[cfg(unix)]
    async fn teardown_cuts_short_a_stop_already_in_its_grace() {
        let dir = tempfile::tempdir().expect("tempdir");
        let ready = dir.path().join("ready");
        let command = format!("trap '' TERM; touch {}; sleep 300 & wait", ready.display());
        let reg = BackgroundBashRegistry::new();
        let (task_id, _finish_rx) = reg
            .spawn(&command, 600, dir.path(), &[], Some(Uuid::new_v4()), None)
            .await
            .expect("spawn");
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        while !ready.exists() {
            assert!(
                tokio::time::Instant::now() < deadline,
                "trap never installed"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(reg.kill(&task_id).await);
        tokio::time::sleep(Duration::from_millis(200)).await;

        assert_eq!(reg.hand_over_at_teardown().await.len(), 1);
        assert!(
            reg.wait_until_finished(&task_id, Duration::from_secs(2))
                .await,
            "the teardown sat out the stop's grace"
        );
        let record = reg.completion_record(&task_id).await.expect("record");
        assert!(record.killed, "the stop still owns this ending");
        assert!(!record.abandoned);
    }

    #[tokio::test]
    #[cfg(unix)]
    async fn kill_lets_a_term_trap_run_first() {
        let dir = tempfile::tempdir().expect("tempdir");
        let marker = dir.path().join("trapped");
        let ready = dir.path().join("ready");
        let command = format!(
            "trap 'echo yes > {}; exit 0' TERM; touch {}; sleep 300 & wait",
            marker.display(),
            ready.display()
        );
        let reg = BackgroundBashRegistry::new();
        let (task_id, _finish_rx) = reg
            .spawn(&command, 600, dir.path(), &[], None, None)
            .await
            .expect("spawn");
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        while !ready.exists() {
            assert!(
                tokio::time::Instant::now() < deadline,
                "trap never installed"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }

        assert!(reg.kill(&task_id).await);
        assert!(reg.wait_for_finish(&task_id, Duration::from_secs(20)).await);
        assert!(
            marker.exists(),
            "the stop never gave the TERM trap a chance"
        );
        let snap = reg
            .read_output_in_memory_wait(&task_id, Duration::ZERO)
            .await
            .expect("snapshot");
        assert_eq!(snap.outcome, Some(TaskOutcome::Exited(0)));
        assert!(
            snap.killed,
            "the trap's clean exit must not hide that a stop ended the task"
        );
    }

    #[tokio::test]
    #[cfg(unix)]
    async fn kill_still_ends_a_command_that_ignores_term() {
        let dir = tempfile::tempdir().expect("tempdir");
        let ready = dir.path().join("ready");
        let command = format!("trap '' TERM; touch {}; sleep 300 & wait", ready.display());
        let reg = BackgroundBashRegistry::new();
        let (task_id, _finish_rx) = reg
            .spawn(&command, 600, dir.path(), &[], None, None)
            .await
            .expect("spawn");
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        while !ready.exists() {
            assert!(
                tokio::time::Instant::now() < deadline,
                "trap never installed"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }

        assert!(reg.kill(&task_id).await);
        assert!(
            reg.wait_for_finish(&task_id, Duration::from_secs(20)).await,
            "the SIGKILL after the grace never came"
        );
        let snap = reg
            .read_output_in_memory_wait(&task_id, Duration::ZERO)
            .await
            .expect("snapshot");
        assert_eq!(snap.outcome, Some(TaskOutcome::Signaled(9)));
    }

    #[tokio::test]
    #[cfg(unix)]
    async fn a_task_that_exits_on_its_own_leaves_its_detached_child_alone() {
        let reg = BackgroundBashRegistry::new();
        let (task_id, _finish_rx) = reg
            .spawn(
                "sleep 30 > /dev/null 2>&1 & echo $!",
                60,
                std::path::Path::new("/tmp"),
                &[],
                None,
                None,
            )
            .await
            .expect("spawn");
        assert!(reg.wait_for_finish(&task_id, Duration::from_secs(10)).await);
        let snap = reg
            .read_output_in_memory_wait(&task_id, Duration::ZERO)
            .await
            .expect("snapshot");
        let detached: i32 = snap.stdout.trim().parse().expect("the sleeper's pid");

        let survived = pid_alive(detached);
        // SAFETY: plain signal to a pid this test started.
        unsafe {
            libc::kill(detached, libc::SIGKILL);
        }
        assert!(survived, "a natural exit must not signal the task's group");
    }

    #[tokio::test]
    async fn timeout_kills_long_running_task() {
        let reg = BackgroundBashRegistry::new();
        let (task_id, _finish_rx) = reg
            .spawn("sleep 30", 1, std::path::Path::new("/tmp"), &[], None, None)
            .await
            .expect("spawn");

        let finished = reg.wait_for_finish(&task_id, Duration::from_secs(20)).await;
        assert!(finished, "timeout did not fire");

        let snap = reg
            .read_output_in_memory_wait(&task_id, Duration::ZERO)
            .await
            .expect("snapshot");
        assert!(snap.finished);
        assert!(snap.timed_out, "timed_out flag should be true");
        assert!(!snap.killed);
        assert_eq!(snap.outcome, Some(TaskOutcome::Signaled(15)));
        assert_eq!(
            snap.outcome.unwrap().exit_code(),
            None,
            "a timed-out task must never present an exit code"
        );
    }

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
                None,
            )
            .await
            .expect("spawn");
        assert!(reg.wait_for_finish(&task_id, Duration::from_secs(3)).await);

        let record = reg
            .completion_record(&task_id)
            .await
            .expect("completion record for a finished task");
        assert!(record.stdout.contains("the-result"));
        assert_eq!(record.outcome, TaskOutcome::Exited(0));

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

    #[tokio::test]
    async fn progress_reads_the_latest_output_without_draining_it() {
        let reg = BackgroundBashRegistry::new();
        let thread_id = Uuid::new_v4();
        let (task_id, _finish_rx) = reg
            .spawn(
                "echo step 3/5: linking; sleep 3",
                10,
                std::path::Path::new("/tmp"),
                &[],
                Some(thread_id),
                Some("the release build"),
            )
            .await
            .expect("spawn");
        assert!(
            reg.wait_for_stdout(&task_id, "linking", Duration::from_secs(30))
                .await
        );

        let progress = reg.progress_for_thread(thread_id).await;
        assert_eq!(progress.len(), 1);
        assert_eq!(progress[0].what, "the release build");
        assert!(progress[0].latest_output.contains("step 3/5: linking"));
        assert!(reg.progress_for_thread(Uuid::new_v4()).await.is_empty());

        let drained = reg
            .read_output_in_memory_wait(&task_id, Duration::ZERO)
            .await
            .expect("a running task is drainable");
        assert!(drained.stdout.contains("step 3/5: linking"));
    }

    #[tokio::test]
    async fn repeat_drain_within_retention_is_empty_but_still_finished() {
        let reg = BackgroundBashRegistry::new();
        let (task_id, _finish_rx) = reg
            .spawn(
                "echo once",
                5,
                std::path::Path::new("/tmp"),
                &[],
                None,
                None,
            )
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

    #[tokio::test]
    async fn finished_task_is_evicted_after_the_grace_window() {
        let reg = BackgroundBashRegistry::new();
        let (task_id, _finish_rx) = reg
            .spawn(
                "echo stale",
                5,
                std::path::Path::new("/tmp"),
                &[],
                None,
                None,
            )
            .await
            .expect("spawn");
        assert!(reg.wait_for_finish(&task_id, Duration::from_secs(3)).await);
        assert!(reg
            .read_output_in_memory_wait(&task_id, Duration::ZERO)
            .await
            .is_some());

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
                    None,
                )
                .await
                .expect("spawn");
            assert!(reg.wait_for_finish(&task_id, Duration::from_secs(5)).await);
            assert!(reg.completion_record(&task_id).await.is_some());
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
                    None,
                )
                .await
                .expect("spawn");
            assert!(reg.wait_for_finish(&task_id, Duration::from_secs(5)).await);
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
        assert_eq!(reg.retained_finished_count().await, MAX_RETAINED_FINISHED);
    }

    #[tokio::test]
    async fn is_running_is_false_for_a_retained_finished_task() {
        let reg = BackgroundBashRegistry::new();
        let (task_id, _finish_rx) = reg
            .spawn(
                "sleep 30",
                60,
                std::path::Path::new("/tmp"),
                &[],
                None,
                None,
            )
            .await
            .expect("spawn");
        assert!(
            reg.is_running(&task_id).await,
            "a task this process owns must block the sweep from settling it"
        );

        assert!(reg.kill(&task_id).await);
        assert!(reg.wait_for_finish(&task_id, Duration::from_secs(20)).await);
        assert!(!reg.is_running(&task_id).await);
        assert!(!reg.is_running("never-spawned").await);
    }

    #[tokio::test]
    async fn hand_over_at_teardown_kills_the_child_and_keeps_what_it_wrote() {
        let reg = BackgroundBashRegistry::new();
        let my_thread = Uuid::new_v4();
        let (task_id, _finish_rx) = reg
            .spawn(
                "echo mid-flight; sleep 300",
                600,
                std::path::Path::new("/tmp"),
                &[],
                Some(my_thread),
                None,
            )
            .await
            .expect("spawn");
        assert!(
            reg.wait_for_stdout(&task_id, "mid-flight", Duration::from_secs(5))
                .await,
            "the echo never flushed, so this test cannot say what teardown would keep",
        );

        let killed = reg.hand_over_at_teardown().await;
        assert_eq!(killed.len(), 1, "killed: {killed:?}");
        assert_eq!(killed[0].thread_id, my_thread);
        assert_eq!(killed[0].task_id, task_id);

        assert!(
            reg.wait_until_finished(&task_id, Duration::from_secs(30))
                .await,
            "hand_over_at_teardown must kill, not merely mark",
        );

        let record = reg.completion_record(&task_id).await.expect("record");
        assert_eq!(record.command, "echo mid-flight; sleep 300");
        assert!(record.abandoned);
        assert!(
            !record.killed,
            "`abandoned` outranks `killed`: nobody called bash_kill"
        );
        let crate::engine::thread_events::ThreadEvent::BackgroundBashCompleted { stderr, .. } =
            crate::engine::bash_background_recovery::completion_event(
                task_id.clone(),
                record.clone(),
            )
        else {
            panic!("expected a completion");
        };
        assert!(stderr.contains("the engine was shutting down"), "{stderr}");
        assert!(
            stderr.contains("may still be running"),
            "a group kill cannot reach a detached session: {stderr}"
        );
        assert!(
            record.stdout.contains("mid-flight"),
            "the output written before the kill was lost: {:?}",
            record.stdout
        );
    }

    #[test]
    fn ending_names_the_decider_of_a_signal_that_landed() {
        let cases = [
            ((true, true), (true, false)),
            ((false, true), (false, true)),
            ((true, false), (false, false)),
            ((false, false), (false, false)),
        ];
        for ((abandoned_mark, landed), want) in cases {
            let mut task = BackgroundTask::for_test();
            task.abandoned = abandoned_mark;
            task.killed = landed;
            assert_eq!(ending(&task), want, "mark={abandoned_mark} landed={landed}");
        }
    }

    #[tokio::test]
    async fn end_task_keeps_the_verdict_of_a_child_that_already_exited() {
        let dir = tempfile::tempdir().expect("tempdir");
        let done = dir.path().join("done");
        let mut cmd = command_shell()
            .command(
                &Sandbox::unsandboxed(),
                &format!("touch {}; exit 3", done.display()),
            )
            .expect("unsandboxed command");
        crate::runtime::spawn_env::isolate_in_process_group(&mut cmd);
        let mut child = cmd.spawn().expect("spawn");
        let pid = child.id();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        while !done.exists() {
            assert!(
                tokio::time::Instant::now() < deadline,
                "the child never ran"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        tokio::time::sleep(Duration::from_millis(300)).await;

        let (outcome, signalled) = end_task(&mut child, pid, Stop::Graceful, &Notify::new()).await;
        assert_eq!(outcome, TaskOutcome::Exited(3));
        assert!(
            !signalled,
            "a stop must not claim work that had already ended"
        );
    }

    #[tokio::test]
    async fn a_task_already_killed_by_the_agent_stays_killed() {
        let reg = BackgroundBashRegistry::new();
        let (task_id, _finish_rx) = reg
            .spawn(
                "sleep 300",
                600,
                std::path::Path::new("/tmp"),
                &[],
                Some(Uuid::new_v4()),
                None,
            )
            .await
            .expect("spawn");

        assert!(reg.kill(&task_id).await);
        let handles = reg.hand_over_at_teardown().await;
        assert_eq!(
            handles.len(),
            1,
            "the teardown still has to record it before the process goes"
        );
        assert!(reg.wait_for_finish(&task_id, Duration::from_secs(30)).await);

        let record = reg.completion_record(&task_id).await.expect("record");
        assert!(record.killed, "bash_kill ended it, and says so");
        assert!(!record.abandoned);
    }

    #[tokio::test]
    async fn a_timed_out_task_is_not_stolen_by_the_shutdown() {
        let reg = BackgroundBashRegistry::new();
        let (task_id, _finish_rx) = reg
            .spawn(
                "sleep 300",
                1,
                std::path::Path::new("/tmp"),
                &[],
                Some(Uuid::new_v4()),
                None,
            )
            .await
            .expect("spawn");
        assert!(reg.wait_for_finish(&task_id, Duration::from_secs(30)).await);

        assert_eq!(reg.hand_over_at_teardown().await.len(), 1);

        let record = reg.completion_record(&task_id).await.expect("record");
        assert!(record.timed_out, "its own budget ended it, and says so");
        assert!(!record.abandoned);
        assert_eq!(record.outcome, TaskOutcome::Signaled(15));
    }

    #[tokio::test]
    async fn only_one_of_the_two_emitters_gets_the_record() {
        let reg = BackgroundBashRegistry::new();
        let (task_id, _finish_rx) = reg
            .spawn(
                "echo nearly; sleep 0.2",
                60,
                std::path::Path::new("/tmp"),
                &[],
                Some(Uuid::new_v4()),
                None,
            )
            .await
            .expect("spawn");
        assert!(reg.wait_for_finish(&task_id, Duration::from_secs(60)).await);

        assert!(reg.completion_record(&task_id).await.is_some(), "the first");
        assert!(
            reg.completion_record(&task_id).await.is_none(),
            "the second must write nothing"
        );
        assert!(reg
            .read_output_in_memory_wait(&task_id, Duration::ZERO)
            .await
            .is_some());
    }

    #[tokio::test]
    async fn hand_over_at_teardown_takes_the_unclaimed_and_skips_the_thread_less() {
        let reg = BackgroundBashRegistry::new();
        let (orphan, _rx1) = reg
            .spawn(
                "sleep 30",
                60,
                std::path::Path::new("/tmp"),
                &[],
                None,
                None,
            )
            .await
            .expect("spawn");
        let (done, _rx2) = reg
            .spawn(
                "echo bye",
                5,
                std::path::Path::new("/tmp"),
                &[],
                Some(Uuid::new_v4()),
                None,
            )
            .await
            .expect("spawn");
        assert!(reg.wait_for_finish(&done, Duration::from_secs(5)).await);

        let handed = reg.hand_over_at_teardown().await;
        let ids: Vec<&str> = handed.iter().map(|t| t.task_id.as_str()).collect();
        assert_eq!(ids, vec![done.as_str()], "handed: {handed:?}");
        assert!(
            reg.is_running(&orphan).await,
            "a thread-less task must be left running, not killed for nothing"
        );

        let record = reg.completion_record(&done).await.expect("record");
        assert!(
            !record.abandoned,
            "a task that finished on its own is not abandoned"
        );
        assert_eq!(
            record.outcome,
            TaskOutcome::Exited(0),
            "its real verdict is what the teardown records"
        );

        reg.kill(&orphan).await;
    }

    #[tokio::test]
    async fn a_released_claim_makes_the_task_recordable_again() {
        let reg = BackgroundBashRegistry::new();
        let (task_id, _finish_rx) = reg
            .spawn(
                "echo written",
                5,
                std::path::Path::new("/tmp"),
                &[],
                Some(Uuid::new_v4()),
                None,
            )
            .await
            .expect("spawn");
        assert!(reg.wait_for_finish(&task_id, Duration::from_secs(8)).await);

        assert!(reg.completion_record(&task_id).await.is_some());
        assert!(reg.hand_over_at_teardown().await.is_empty());

        reg.release_completion_claim(&task_id).await;

        assert_eq!(reg.hand_over_at_teardown().await.len(), 1);
        let record = reg.completion_record(&task_id).await.expect("record");
        assert!(record.stdout.contains("written"));
    }

    #[tokio::test]
    async fn releasing_a_claim_on_a_missing_task_is_a_no_op() {
        let reg = BackgroundBashRegistry::new();
        reg.release_completion_claim("never-spawned").await;
    }

    #[tokio::test]
    async fn hand_over_at_teardown_skips_a_claimed_completion() {
        let reg = BackgroundBashRegistry::new();
        let (task_id, _finish_rx) = reg
            .spawn(
                "echo claimed",
                5,
                std::path::Path::new("/tmp"),
                &[],
                Some(Uuid::new_v4()),
                None,
            )
            .await
            .expect("spawn");
        assert!(reg.wait_for_finish(&task_id, Duration::from_secs(8)).await);
        assert!(reg.completion_record(&task_id).await.is_some());

        assert!(reg.hand_over_at_teardown().await.is_empty());
    }

    #[tokio::test]
    async fn wait_until_finished_is_false_for_a_running_and_an_unknown_task() {
        let reg = BackgroundBashRegistry::new();
        let (task_id, _finish_rx) = reg
            .spawn(
                "sleep 30",
                60,
                std::path::Path::new("/tmp"),
                &[],
                None,
                None,
            )
            .await
            .expect("spawn");

        assert!(
            !reg.wait_until_finished(&task_id, Duration::from_millis(50))
                .await
        );
        assert!(
            !reg.wait_until_finished("never-spawned", Duration::from_millis(50))
                .await
        );

        assert!(reg.kill(&task_id).await);
        assert!(
            reg.wait_until_finished(&task_id, Duration::from_secs(30))
                .await
        );
    }

    #[tokio::test]
    async fn the_stored_command_is_redacted() {
        let reg = BackgroundBashRegistry::new();
        let (task_id, _finish_rx) = reg
            .spawn(
                "psql postgres://u:hunter2@h:5432/d -c 'select 1'",
                5,
                std::path::Path::new("/tmp"),
                &[],
                None,
                None,
            )
            .await
            .expect("spawn");
        assert!(reg.wait_for_finish(&task_id, Duration::from_secs(8)).await);

        let record = reg.completion_record(&task_id).await.expect("record");
        assert!(
            !record.command.contains("hunter2"),
            "the stored command leaked a password: {:?}",
            record.command
        );
    }

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
                None,
            )
            .await
            .expect("spawn");
        assert!(reg.wait_for_finish(&task_id, Duration::from_secs(3)).await);

        assert!(
            !reg.has_running_for_thread(my_thread).await,
            "a finished task must not register as running, retained or not"
        );
        assert!(reg
            .read_output_in_memory_wait(&task_id, Duration::ZERO)
            .await
            .is_some());
    }

    #[tokio::test]
    async fn completion_record_does_not_evict_the_task() {
        let reg = BackgroundBashRegistry::new();
        let (task_id, _finish_rx) = reg
            .spawn(
                "echo done",
                5,
                std::path::Path::new("/tmp"),
                &[],
                None,
                None,
            )
            .await
            .expect("spawn");
        let finished = reg.wait_for_finish(&task_id, Duration::from_secs(3)).await;
        assert!(finished);

        let record = reg
            .completion_record(&task_id)
            .await
            .expect("completion record");
        assert!(record.stdout.contains("done"));

        assert!(reg
            .read_output_in_memory_wait(&task_id, Duration::ZERO)
            .await
            .is_some());

        assert!(
            reg.completion_record(&task_id).await.is_none(),
            "a claimed completion must not be handed out twice"
        );
    }

    #[tokio::test]
    async fn completion_record_does_not_consume_the_drain_cursor() {
        let reg = BackgroundBashRegistry::new();
        let (task_id, _finish_rx) = reg
            .spawn(
                "echo payload",
                5,
                std::path::Path::new("/tmp"),
                &[],
                None,
                None,
            )
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
        let reg = BackgroundBashRegistry::new();
        let (task_id, _finish_rx) = reg
            .spawn(
                "echo hello-before-kill; sleep 30",
                60,
                std::path::Path::new("/tmp"),
                &[],
                None,
                None,
            )
            .await
            .expect("spawn");

        assert!(
            reg.wait_for_stdout(&task_id, "hello-before-kill", Duration::from_secs(5))
                .await,
            "the echo never flushed, so this test cannot say what the kill did",
        );
        assert!(reg.kill(&task_id).await);

        let finished = reg.wait_for_finish(&task_id, Duration::from_secs(20)).await;
        assert!(finished, "task did not finish within 20s after kill");

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
            .spawn("sleep 5", 60, std::path::Path::new("/tmp"), &[], None, None)
            .await
            .expect("spawn");
        assert!(reg.completion_record(&task_id).await.is_none());
        reg.kill(&task_id).await;
    }

    #[tokio::test]
    async fn a_begun_stop_marks_the_task_and_signals_only_when_sent() {
        let reg = BackgroundBashRegistry::new();
        let thread = Uuid::new_v4();
        let (task_id, _finish_rx) = reg
            .spawn(
                "sleep 30",
                60,
                std::path::Path::new("/tmp"),
                &[],
                Some(thread),
                None,
            )
            .await
            .expect("spawn");

        let stop = reg.begin_stop(&task_id).await.expect("running");
        assert_eq!(stop.owner(), Some(thread));
        let handles = reg.running_for_thread(thread).await;
        assert!(handles[0].stop_requested, "{handles:?}");
        assert!(
            reg.begin_stop(&task_id).await.is_none(),
            "a stop already on its way is not taken twice"
        );
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert!(
            reg.is_running(&task_id).await,
            "nothing is signalled before send"
        );

        stop.send();
        assert!(reg.wait_for_finish(&task_id, Duration::from_secs(15)).await);
        assert!(
            reg.completion_record(&task_id)
                .await
                .expect("record")
                .killed
        );
    }

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
                None,
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

        assert!(reg.kill(&task_id).await);
        let finished = reg.wait_for_finish(&task_id, Duration::from_secs(20)).await;
        assert!(finished);

        assert!(
            !reg.has_running_for_thread(my_thread).await,
            "a finished task must NOT keep registering as running"
        );
    }

    #[tokio::test]
    async fn kill_for_thread_kills_only_that_threads_running_tasks() {
        let reg = BackgroundBashRegistry::new();
        let my_thread = Uuid::new_v4();
        let other_thread = Uuid::new_v4();
        let tmp = std::path::Path::new("/tmp");
        let (mine_a, _a) = reg
            .spawn("sleep 30", 60, tmp, &[], Some(my_thread), None)
            .await
            .expect("spawn");
        let (mine_b, _b) = reg
            .spawn("sleep 30", 60, tmp, &[], Some(my_thread), None)
            .await
            .expect("spawn");
        let (theirs, _c) = reg
            .spawn("sleep 30", 60, tmp, &[], Some(other_thread), None)
            .await
            .expect("spawn");

        assert_eq!(reg.kill_for_thread(my_thread).await, 2);
        for task_id in [&mine_a, &mine_b] {
            assert!(reg.wait_for_finish(task_id, Duration::from_secs(20)).await);
            let record = reg.completion_record(task_id).await.expect("finished");
            assert!(record.killed, "a thread kill records the task as killed");
        }
        assert!(
            reg.is_running(&theirs).await,
            "another thread's task must keep running"
        );
        assert_eq!(
            reg.kill_for_thread(my_thread).await,
            0,
            "nothing left to kill on a second call"
        );
        reg.kill(&theirs).await;
    }

    #[tokio::test]
    async fn spawned_by_names_the_owning_thread_and_knows_what_it_does_not_hold() {
        let reg = BackgroundBashRegistry::new();
        let my_thread = Uuid::new_v4();
        let (task_id, _rx) = reg
            .spawn(
                "sleep 30",
                60,
                std::path::Path::new("/tmp"),
                &[],
                Some(my_thread),
                None,
            )
            .await
            .expect("spawn");
        assert_eq!(reg.spawned_by(&task_id, my_thread).await, Some(true));
        assert_eq!(reg.spawned_by(&task_id, Uuid::new_v4()).await, Some(false));
        assert_eq!(reg.spawned_by("no-such-task", my_thread).await, None);
        reg.kill(&task_id).await;
    }

    #[tokio::test]
    async fn running_for_thread_reports_each_task_with_its_watchdog_deadline() {
        let reg = BackgroundBashRegistry::new();
        let my_thread = Uuid::new_v4();
        let other_thread = Uuid::new_v4();

        assert!(
            reg.running_for_thread(my_thread).await.is_empty(),
            "empty registry must report nothing for any thread"
        );

        let timeout_secs = 900;
        let before = Utc::now();
        let (task_id, _finish_rx) = reg
            .spawn(
                "sleep 30",
                timeout_secs,
                std::path::Path::new("/tmp"),
                &[],
                Some(my_thread),
                None,
            )
            .await
            .expect("spawn");

        let running = reg.running_for_thread(my_thread).await;
        assert_eq!(running.len(), 1, "one running task for my_thread");
        assert_eq!(
            running[0].task_id, task_id,
            "the reported id must be the one BackgroundBashCompleted will carry"
        );
        assert!(
            running[0].watchdog_deadline >= before + chrono::Duration::seconds(timeout_secs as i64)
                && running[0].watchdog_deadline
                    <= Utc::now() + chrono::Duration::seconds(timeout_secs as i64),
            "deadline {} is not started_at + {timeout_secs}s",
            running[0].watchdog_deadline
        );

        assert!(
            reg.running_for_thread(other_thread).await.is_empty(),
            "my_thread's task must not leak to another thread"
        );

        assert!(reg.kill(&task_id).await);
        assert!(reg.wait_for_finish(&task_id, Duration::from_secs(20)).await);
        assert!(
            reg.running_for_thread(my_thread).await.is_empty(),
            "a finished task must not still be reported as running"
        );
    }

    #[tokio::test]
    async fn running_for_thread_reports_the_description_and_the_redacted_command() {
        let reg = BackgroundBashRegistry::new();
        let my_thread = Uuid::new_v4();
        let tmp = std::path::Path::new("/tmp");
        let (described, _rx1) = reg
            .spawn(
                "sleep 30",
                60,
                tmp,
                &[],
                Some(my_thread),
                Some("  the nightly sweep "),
            )
            .await
            .expect("spawn");
        let (blank, _rx2) = reg
            .spawn(
                "psql postgres://u:hunter2@h:5432/d -c 'select pg_sleep(30)'",
                60,
                tmp,
                &[],
                Some(my_thread),
                Some("   "),
            )
            .await
            .expect("spawn");

        let running = reg.running_for_thread(my_thread).await;
        let find = |id: &str| running.iter().find(|h| h.task_id == id).expect("reported");
        assert_eq!(
            find(&described).description.as_deref(),
            Some("the nightly sweep")
        );
        assert_eq!(find(&blank).description, None);
        assert!(
            !find(&blank).command.contains("hunter2"),
            "the command shown to the user must be the redacted one: {}",
            find(&blank).command
        );

        reg.kill(&described).await;
        reg.kill(&blank).await;
    }

    #[tokio::test]
    async fn running_for_thread_reports_a_polled_task_as_polled() {
        let reg = BackgroundBashRegistry::new();
        let my_thread = Uuid::new_v4();
        let tmp = std::path::Path::new("/tmp");
        let spec = |delivery| TaskSpec {
            timeout_secs: 60,
            description: None,
            delivery,
            inherited: crate::core::InheritedSecrets::Keep,
        };
        let (polled, _rx1) = reg
            .spawn_task(
                &Sandbox::unsandboxed(),
                "sleep 30",
                tmp,
                &[],
                Some(my_thread),
                spec(CompletionDelivery::Poll),
            )
            .await
            .expect("spawn");
        let (watched, _rx2) = reg
            .spawn_task(
                &Sandbox::unsandboxed(),
                "sleep 30",
                tmp,
                &[],
                Some(my_thread),
                spec(CompletionDelivery::EventWait),
            )
            .await
            .expect("spawn");

        let running = reg.running_for_thread(my_thread).await;
        let find = |id: &str| running.iter().find(|h| h.task_id == id).expect("reported");
        assert!(find(&polled).polled);
        assert!(!find(&watched).polled);

        reg.kill(&polled).await;
        reg.kill(&watched).await;
    }

    async fn unread_case(
        reg: &BackgroundBashRegistry,
        thread: Uuid,
        delivery: CompletionDelivery,
        before_finish: impl AsyncFnOnce(&BackgroundBashRegistry, &str),
    ) -> String {
        let spec = TaskSpec {
            timeout_secs: 60,
            description: Some("the gates"),
            delivery,
            inherited: crate::core::InheritedSecrets::Keep,
        };
        let (task_id, _rx) = reg
            .spawn_task(
                &Sandbox::unsandboxed(),
                "sleep 0.3",
                std::path::Path::new("/tmp"),
                &[],
                Some(thread),
                spec,
            )
            .await
            .expect("spawn");
        before_finish(reg, &task_id).await;
        assert!(reg.wait_for_finish(&task_id, Duration::from_secs(10)).await);
        task_id
    }

    fn unread_ids(reg: &BackgroundBashRegistry, thread: Uuid) -> Vec<String> {
        reg.unread_results_for_thread(thread)
            .into_iter()
            .map(|r| r.task_id)
            .collect()
    }

    #[tokio::test]
    async fn a_polled_finish_is_unread_until_its_owner_reads_it() {
        let reg = BackgroundBashRegistry::new();
        let (owner, other) = (Uuid::new_v4(), Uuid::new_v4());
        let id = unread_case(&reg, owner, CompletionDelivery::Poll, async |_, _| {}).await;
        let results = reg.unread_results_for_thread(owner);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].task_id, id);
        assert_eq!(results[0].description.as_deref(), Some("the gates"));

        reg.mark_result_read(other, &id);
        assert_eq!(unread_ids(&reg, owner), vec![id.clone()]);
        reg.mark_result_read(owner, &id);
        assert!(unread_ids(&reg, owner).is_empty());
    }

    #[tokio::test]
    async fn a_watched_finish_is_never_unread() {
        let reg = BackgroundBashRegistry::new();
        let owner = Uuid::new_v4();
        unread_case(&reg, owner, CompletionDelivery::EventWait, async |_, _| {}).await;
        assert!(unread_ids(&reg, owner).is_empty());
    }

    #[tokio::test]
    async fn a_stopped_polled_task_is_never_unread() {
        let reg = BackgroundBashRegistry::new();
        let owner = Uuid::new_v4();
        unread_case(&reg, owner, CompletionDelivery::Poll, async |reg, id| {
            assert!(reg.kill(id).await);
        })
        .await;
        assert!(unread_ids(&reg, owner).is_empty());
    }

    #[tokio::test]
    async fn a_read_and_a_discard_clear_unread_results() {
        let reg = BackgroundBashRegistry::new();
        let owner = Uuid::new_v4();
        let first = unread_case(&reg, owner, CompletionDelivery::Poll, async |_, _| {}).await;
        let second = unread_case(&reg, owner, CompletionDelivery::Poll, async |_, _| {}).await;
        assert_eq!(unread_ids(&reg, owner), vec![first.clone(), second.clone()]);

        reg.mark_result_read(owner, &first);
        assert_eq!(unread_ids(&reg, owner), vec![second]);
        reg.kill_for_thread(owner).await;
        assert!(unread_ids(&reg, owner).is_empty());
    }

    #[tokio::test]
    async fn a_drain_whose_budget_runs_out_leaves_the_task_running() {
        let reg = BackgroundBashRegistry::new();
        let (task_id, _rx) = reg
            .spawn(
                "sleep 30",
                60,
                std::path::Path::new("/tmp"),
                &[],
                None,
                None,
            )
            .await
            .expect("spawn");

        let snap = reg
            .read_output_in_memory_wait(&task_id, Duration::from_millis(200))
            .await
            .expect("a running task is readable");
        assert!(!snap.finished);
        assert!(
            reg.is_running(&task_id).await,
            "the timed-out read must not stop it"
        );

        reg.kill(&task_id).await;
    }

    #[tokio::test]
    async fn has_running_for_thread_ignores_tasks_with_no_thread_id() {
        let reg = BackgroundBashRegistry::new();
        let some_thread = Uuid::new_v4();

        let (task_id, _finish_rx) = reg
            .spawn(
                "sleep 30",
                60,
                std::path::Path::new("/tmp"),
                &[],
                None,
                None,
            )
            .await
            .expect("spawn");

        assert!(
            !reg.has_running_for_thread(some_thread).await,
            "task with thread_id=None must NOT register under any thread"
        );

        reg.kill(&task_id).await;
    }

    #[tokio::test]
    async fn wait_returns_immediately_when_task_already_finished() {
        let reg = BackgroundBashRegistry::new();
        let (task_id, _finish_rx) = reg
            .spawn(
                "echo eager",
                5,
                std::path::Path::new("/tmp"),
                &[],
                None,
                None,
            )
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
        let reg = BackgroundBashRegistry::new();
        let (task_id, _finish_rx) = reg
            .spawn(
                "echo line1; echo line2; echo line3; \
                 for i in $(seq 4 100); do sleep 0.02; echo line$i; done",
                30,
                std::path::Path::new("/tmp"),
                &[],
                None,
                None,
            )
            .await
            .expect("spawn");

        assert!(
            reg.wait_for_stdout(&task_id, "line1", Duration::from_secs(10))
                .await,
            "the child never emitted its first line"
        );

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
        let mut s = Stream::default();
        s.push(&vec![b'a'; TRIM_TRIGGER_BYTES + 4096]);

        let (text, dropped) = s.drain();
        assert!(dropped > 0, "the trim discarded unread bytes, unreported");
        assert_eq!(
            dropped + text.len(),
            TRIM_TRIGGER_BYTES + 4096,
            "every byte written is either returned or counted as dropped"
        );
        s.push(b"quiet");
        assert_eq!(s.drain(), ("quiet".to_string(), 0));
    }

    #[test]
    fn stream_trim_of_already_read_bytes_is_not_unread_loss() {
        let mut s = Stream::default();
        s.push(&vec![b'a'; TRIM_TRIGGER_BYTES]);
        let (first, dropped) = s.drain();
        assert_eq!(dropped, 0, "no trim yet at exactly the trigger size");
        assert_eq!(first.len(), TRIM_TRIGGER_BYTES);

        s.push(&vec![b'b'; 4096]);
        let (second, dropped) = s.drain();
        assert_eq!(
            second.len(),
            4096,
            "the new bytes survive: {}",
            second.len()
        );
        assert_eq!(dropped, 0, "already-read bytes are not a reportable loss");
        assert!(s.all().1 > 0, "trimmed_total must track every cut byte");
    }

    #[tokio::test]
    async fn wait_reports_elapsed_task_runtime() {
        let reg = BackgroundBashRegistry::new();
        let (task_id, _finish_rx) = reg
            .spawn(
                "sleep 1.2",
                30,
                std::path::Path::new("/tmp"),
                &[],
                None,
                None,
            )
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

        assert!(reg.wait_for_finish(&task_id, Duration::from_secs(20)).await);
        let at_finish = reg
            .read_output_in_memory_wait(&task_id, Duration::ZERO)
            .await
            .expect("snapshot");
        assert!(at_finish.finished);

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
        let reg = BackgroundBashRegistry::new();
        let (task_id, _finish_rx) = reg
            .spawn(
                "sleep 0.3",
                5,
                std::path::Path::new("/tmp"),
                &[],
                None,
                None,
            )
            .await
            .expect("spawn");

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
        let reg = BackgroundBashRegistry::new();
        let (task_id, _finish_rx) = reg
            .spawn(
                "sleep 30",
                60,
                std::path::Path::new("/tmp"),
                &[],
                None,
                None,
            )
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
                None,
            )
            .await
            .expect("spawn");

        const BUDGET: Duration = Duration::from_secs(30);
        let reg_a = reg.clone();
        let task_a = task_id.clone();
        let h_a =
            tokio::spawn(async move { reg_a.read_output_in_memory_wait(&task_a, BUDGET).await });
        let reg_b = reg.clone();
        let task_b = task_id.clone();
        let h_b =
            tokio::spawn(async move { reg_b.read_output_in_memory_wait(&task_b, BUDGET).await });

        let started = std::time::Instant::now();
        let (snap_a, snap_b) = tokio::join!(h_a, h_b);
        let elapsed = started.elapsed();
        let snap_a = snap_a.unwrap().expect("waiter A returned None");
        let snap_b = snap_b.unwrap().expect("waiter B returned None");

        assert!(
            elapsed < BUDGET / 3,
            "both waiters must wake on finish, not sit out the budget (took {:?})",
            elapsed
        );
        assert!(
            snap_a.finished && snap_b.finished,
            "both waiters must observe the finish: a={} b={}",
            snap_a.finished,
            snap_b.finished
        );
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
        let reg = BackgroundBashRegistry::new();
        let (task_id, _finish_rx) = reg
            .spawn(
                "echo a; sleep 30",
                60,
                std::path::Path::new("/tmp"),
                &[],
                None,
                None,
            )
            .await
            .expect("spawn");

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

        let snap2 = reg
            .read_output_in_memory_wait(&task_id, Duration::ZERO)
            .await
            .expect("snapshot 2");
        assert!(snap2.stdout.is_empty(), "stdout 2: {:?}", snap2.stdout);

        reg.kill(&task_id).await;
    }

    async fn run_to_completion(command: &str, timeout_secs: u64) -> OutputSnapshot {
        let reg = BackgroundBashRegistry::new();
        let (task_id, _finish_rx) = reg
            .spawn(
                command,
                timeout_secs,
                std::path::Path::new("/tmp"),
                &[],
                None,
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

    #[tokio::test]
    async fn pipeline_exit_1_is_not_masked_by_tee() {
        let snap = run_to_completion("sh -c 'echo fail; exit 1' 2>&1 | tee /dev/null", 10).await;
        assert_eq!(snap.outcome, Some(TaskOutcome::Exited(1)));
    }

    #[tokio::test]
    async fn pipeline_preserves_the_failing_stages_own_status() {
        let snap = run_to_completion("sh -c 'exit 42' | cat | cat", 10).await;
        assert_eq!(snap.outcome, Some(TaskOutcome::Exited(42)));
    }

    #[tokio::test]
    async fn successful_pipeline_still_reports_zero() {
        let snap = run_to_completion("echo hi | cat | cat", 10).await;
        assert_eq!(snap.outcome, Some(TaskOutcome::Exited(0)));
        assert!(snap.outcome.unwrap().is_success());
    }

    #[tokio::test]
    async fn signal_death_reports_sigkill_not_zero() {
        let snap = run_to_completion("echo bye; kill -9 $$", 10).await;
        assert_eq!(snap.outcome, Some(TaskOutcome::Signaled(9)));
        let outcome = snap.outcome.unwrap();
        assert_eq!(outcome.exit_code(), None, "a signal death has no exit code");
        assert_eq!(outcome.signal(), Some(9));
        assert_eq!(outcome.describe(), "killed by SIGKILL (signal 9)");
        assert!(!outcome.is_success());
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

    mod guard_tests {
        #[test]
        fn both_background_exits_leave_through_the_secret_redaction() {
            let src = crate::test_support::source_scan::read_production_source(
                &crate::test_support::source_scan::engine_source("engine/bash_background.rs"),
            );
            assert!(src.contains("subprocess_secret_values(env,"));
            assert_eq!(
                src.matches("crate::core::redact_subprocess_output(")
                    .count(),
                5,
                "stdout and stderr at the drain and at the completion record, plus the progress tail"
            );
            assert!(!src.contains("redact_secret_values("));
        }
    }
}
