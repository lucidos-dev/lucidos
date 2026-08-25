use super::super::LucidosEngine;
use super::ToolOutcome;
use crate::core::shell::{command_shell, TaskOutcome};
use crate::core::{redact_postgres_secrets, sanitize_for_jsonb};
use crate::engine::event_bus::BusEvent;
use crate::engine::thread_events::{EventMeta, ThreadEvent};
use crate::engine::types::AgentUserInput;
use crate::llm::tools::{
    BG_DEFAULT_TIMEOUT_SECS, BG_MAX_TIMEOUT_SECS, DEFAULT_TIMEOUT_SECS, MAX_TIMEOUT_SECS,
};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::AsyncReadExt;

const MAX_OUTPUT_BYTES: usize = 100 * 1024; // 100 KB

/// How long the pipe readers may go with no bytes and no EOF before we call
/// the pipes detached.
///
/// A flat deadline cannot express this. The readers can still be draining a
/// backlog after the shell exits. On a loaded host they can also be starved
/// for longer than any figure small enough to be worth waiting. So the window
/// measures QUIET, and a reader making progress keeps extending it.
const DETACHED_QUIET_WINDOW: Duration = Duration::from_millis(500);

/// Hard cap on the whole post-exit drain, so a detached process that writes
/// continuously cannot hold the tool open. Reached only by a process that is
/// both detached and chatty; a plain detached one goes quiet immediately.
const DETACHED_MAX_WAIT: Duration = Duration::from_secs(5);

/// The outcome of waiting on a `run_bash` shell.
///
/// `wait_with_output()` could not express the middle variant, and that is the
/// bug this type exists to make impossible. It returns on pipe EOF rather than
/// on shell exit. So a shell that detached something holding those pipes looked
/// identical to a shell still running. `run_bash` then reported `command timed
/// out` for a launch that had already succeeded (ADR 0100).
#[derive(Debug)]
enum ShellWait {
    /// The shell exited and both pipes closed.
    Completed {
        outcome: TaskOutcome,
        stdout: Vec<u8>,
        stderr: Vec<u8>,
    },
    /// The shell exited, but a process it detached still holds the pipes.
    /// Carries whatever drained before the grace expired.
    Detached {
        outcome: TaskOutcome,
        stdout: Vec<u8>,
        stderr: Vec<u8>,
    },
    /// The shell itself never exited within the budget.
    TimedOut,
}

/// Where a pipe reader puts what it reads, and whether anyone still wants it.
///
/// A shared buffer rather than a `Vec` the task owns, because the caller stops
/// waiting before the reader stops reading. A task that owns its buffer takes
/// every collected byte with it, and those bytes are what the caller needs.
struct PipeSink {
    buf: Mutex<Vec<u8>>,
    /// Bytes this reader has taken off the pipe, ever. Read by the post-exit
    /// wait to tell a reader draining a backlog from one sitting on an idle
    /// pipe. Monotonic, so a stale read can only under-report progress.
    read_bytes: AtomicU64,
    /// Set once the caller has taken its snapshot. The reader keeps reading
    /// and throws the bytes away, which is what holds the read end open.
    discard: AtomicBool,
}

impl PipeSink {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            buf: Mutex::new(Vec::new()),
            read_bytes: AtomicU64::new(0),
            discard: AtomicBool::new(false),
        })
    }

    fn take(&self) -> Vec<u8> {
        std::mem::take(&mut *self.buf.lock().expect("drain buffer"))
    }

    fn progress(&self) -> u64 {
        self.read_bytes.load(Ordering::Relaxed)
    }
}

/// Read `pipe` to exhaustion, appending into `sink` until it says stop.
///
/// It reads to EOF even after the caller has gone, and that is the point. The
/// reader owns the pipe handle, so dropping the task closes the read end. The
/// next write by a process we deliberately did not kill would then take
/// `EPIPE` or `SIGPIPE`, killing it by the back door (ADR 0100).
async fn drain_pipe<R: tokio::io::AsyncRead + Unpin>(mut pipe: R, sink: Arc<PipeSink>) {
    let mut buf = [0u8; 8192];
    loop {
        match pipe.read(&mut buf).await {
            Ok(0) => return,
            Ok(n) => {
                sink.read_bytes.fetch_add(n as u64, Ordering::Relaxed);
                if !sink.discard.load(Ordering::Relaxed) {
                    sink.buf
                        .lock()
                        .expect("drain buffer")
                        .extend_from_slice(&buf[..n]);
                }
            }
            Err(e) => {
                // The write end is gone in a way that is not a clean EOF.
                // Nothing more can arrive, so stop and keep what we have.
                log!("[Bash] pipe read ended with an error: {}", e);
                return;
            }
        }
    }
}

/// Wait for both readers to reach EOF after the shell has exited. `true` means
/// they got there, so the output is complete.
///
/// `false` means the pipes went quiet while still open, which is a process the
/// shell detached holding the write ends. Quiet has to clear two bars, because
/// discarding a live pipe throws away a successful command's output:
///
/// - No bytes arrived. Progress resets the window, so a reader working through
///   a backlog is never mistaken for an idle one.
/// - A canary task finished, proving the runtime serviced the readers. Without
///   it a starved reader reads as an idle pipe under load.
///
/// Each reader is taken out of its slot the moment it completes. A window can
/// expire with one pipe closed and the other still live, and a `JoinHandle`
/// that has returned `Ready` must never be polled again.
async fn drain_until_quiet(
    readers: &mut [Option<tokio::task::JoinHandle<()>>],
    out_sink: &Arc<PipeSink>,
    err_sink: &Arc<PipeSink>,
) -> bool {
    let deadline = tokio::time::Instant::now() + DETACHED_MAX_WAIT;
    loop {
        let before = out_sink.progress() + err_sink.progress();
        let window = DETACHED_QUIET_WINDOW.min(deadline - tokio::time::Instant::now());
        // Spawned alongside the window, so it competes for the same scheduler.
        // A trivial task that does not finish in half a second means the
        // runtime never got to the readers either.
        let canary = tokio::spawn(async {});
        let finished = tokio::time::timeout(window, async {
            for slot in readers.iter_mut() {
                if let Some(reader) = slot.as_mut() {
                    // `drain_pipe` maps every read error to a clean stop, and
                    // nothing aborts these tasks, so a JoinError is
                    // unreachable.
                    let _ = reader.await;
                    // Only reached when the await resolved. A window expiring
                    // mid-await leaves the slot filled, which is correct: that
                    // handle is merely pending and may be polled again.
                    *slot = None;
                }
            }
        })
        .await
        .is_ok();

        if finished {
            return true;
        }
        if tokio::time::Instant::now() >= deadline {
            return false;
        }
        if out_sink.progress() + err_sink.progress() != before {
            continue;
        }
        // No bytes this window. That is only evidence of an idle pipe if the
        // readers actually ran. A starved reader looks identical, and calling
        // it detached discards output the OS had already buffered.
        if !canary.is_finished() {
            continue;
        }
        return false;
    }
}

/// Wait for `child` to exit while draining both pipes concurrently.
///
/// Concurrent draining is load-bearing, not tidiness: `child.wait()` alone
/// deadlocks against a child that fills a pipe buffer, which is the trap
/// `wait_with_output()` used to cover for us.
async fn wait_for_shell(
    mut child: tokio::process::Child,
    timeout: Duration,
) -> Result<ShellWait, Box<dyn std::error::Error + Send + Sync>> {
    let out_sink = PipeSink::new();
    let err_sink = PipeSink::new();
    let mut readers: Vec<Option<tokio::task::JoinHandle<()>>> = Vec::with_capacity(2);
    if let Some(pipe) = child.stdout.take() {
        readers.push(Some(tokio::spawn(drain_pipe(pipe, Arc::clone(&out_sink)))));
    }
    if let Some(pipe) = child.stderr.take() {
        readers.push(Some(tokio::spawn(drain_pipe(pipe, Arc::clone(&err_sink)))));
    }

    // Hand the readers over to whoever still holds the write ends, on every
    // exit path. They stop by themselves at EOF, which for an ordinary command
    // is immediate. Aborting instead would close the read ends.
    let release = |out: &Arc<PipeSink>, err: &Arc<PipeSink>| {
        out.discard.store(true, Ordering::Relaxed);
        err.discard.store(true, Ordering::Relaxed);
    };

    let status = match tokio::time::timeout(timeout, child.wait()).await {
        Ok(status) => status.map_err(|e| format!("executing command: {}", e))?,
        Err(_) => {
            release(&out_sink, &err_sink);
            // Dropping `child` here fires the `kill_on_drop` the caller set,
            // so the shell is SIGKILLed before this function returns.
            return Ok(ShellWait::TimedOut);
        }
    };

    let drained = drain_until_quiet(&mut readers, &out_sink, &err_sink).await;

    // Typed, so a signal death can't be flattened into a bare number. The old
    // `status.code().unwrap_or(-1)` reported a SIGSEGV as `-1`, which reads
    // like an ordinary exit code.
    let outcome = TaskOutcome::from_status(status);
    let stdout = out_sink.take();
    let stderr = err_sink.take();
    release(&out_sink, &err_sink);
    Ok(if drained {
        ShellWait::Completed {
            outcome,
            stdout,
            stderr,
        }
    } else {
        ShellWait::Detached {
            outcome,
            stdout,
            stderr,
        }
    })
}

/// Sanitize raw subprocess bytes for storage in a jsonb event payload and
/// truncate to the LLM-facing cap. Centralized so the sync `run_bash` and
/// the async background path always apply the same transformation.
fn finalize_stream(bytes: &[u8]) -> String {
    let sanitized = sanitize_for_jsonb(&String::from_utf8_lossy(bytes));
    truncate_output(&sanitized, MAX_OUTPUT_BYTES)
}

/// Same, but keeps the END of an oversized stream instead of the start.
/// A `bash_output` drain is a *window* on a still-running task: the newest
/// lines are the ones that say where the build got to and which one failed,
/// and head-truncation would throw away precisely those. Now that
/// `wait_secs` blocks for its full budget a single drain can span two
/// minutes of a chatty build, so a window CAN exceed the cap — before the
/// fix each drain returned a few hundred bytes and this never bit.
///
/// `already_dropped` is output the registry's own buffer cap discarded
/// before we ever saw it. It has to be folded into the marker: reporting
/// only what this function trims would state a byte count *lower* than the
/// true loss, and a truncation marker that understates reads as a bound.
fn finalize_drain(text: &str, already_dropped: usize) -> String {
    let sanitized = sanitize_for_jsonb(text);
    truncate_output_tail(&sanitized, MAX_OUTPUT_BYTES, already_dropped)
}

impl LucidosEngine {
    pub(crate) async fn execute_bash_tool(
        &self,
        args: &serde_json::Value,
        thread_id: uuid::Uuid,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        let command = match args.get("command").and_then(|v| v.as_str()) {
            Some(c) if !c.is_empty() => c,
            _ => return Err("command is required".into()),
        };

        let timeout_secs = args
            .get("timeout_secs")
            .and_then(|v| v.as_u64())
            .unwrap_or(DEFAULT_TIMEOUT_SECS)
            .min(MAX_TIMEOUT_SECS);

        let env_vars = self.build_script_env_vars(Some(thread_id)).await;

        // `pipefail` shell — see `core::shell`. A `cmd | tee log` would
        // otherwise report tee's 0 and hide the real failure.
        let mut cmd = command_shell().command(command);
        cmd.current_dir(self.workspace_path())
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            // Two callers can drop this Command future:
            // 1. The agent loop's run_tool_with_cancel wrapper on user cancel.
            // 2. The tokio::time::timeout below when the per-tool timeout fires.
            // Without kill_on_drop both leak the shell child. With it set,
            // both paths SIGKILL the child correctly.
            .kill_on_drop(true);

        for (key, value) in &env_vars {
            cmd.env(key, value);
        }

        let log_command = redact_postgres_secrets(command);
        log!(
            "[Bash] Running: {}",
            &log_command[..log_command.floor_char_boundary(200)]
        );

        let child = cmd
            .spawn()
            .map_err(|e| format!("Failed to spawn command: {}", e))?;

        // Wait on the SHELL, not on pipe EOF. A command that detaches a
        // process keeps the pipes open after the shell is gone. Waiting on EOF
        // reported that as a timeout (ADR 0100).
        let waited = wait_for_shell(child, Duration::from_secs(timeout_secs)).await?;
        let (raw_stdout, raw_stderr, detached, outcome) = match waited {
            ShellWait::TimedOut => {
                // The shell itself never exited. `wait_for_shell` has already
                // dropped it, so the OS has SIGKILLed it by now.
                return Err(format!("command timed out after {}s", timeout_secs).into());
            }
            ShellWait::Completed {
                outcome,
                stdout,
                stderr,
            } => (stdout, stderr, false, outcome),
            ShellWait::Detached {
                outcome,
                stdout,
                stderr,
            } => (stdout, stderr, true, outcome),
        };

        let stdout = finalize_stream(&raw_stdout);
        let stderr = finalize_stream(&raw_stderr);

        let mut response = String::new();

        if !stdout.is_empty() {
            response.push_str(&stdout);
        }

        if !stderr.is_empty() {
            if !response.is_empty() {
                response.push_str("\n\n");
            }
            response.push_str(&format!("[stderr]\n{}", stderr));
        }

        if !outcome.is_success() {
            if !response.is_empty() {
                response.push_str("\n\n");
            }
            response.push_str(&format!("[{}]", outcome.describe()));
        }

        if response.is_empty() {
            response = format!("[{}]", outcome.describe());
        }

        if detached {
            // Say plainly that the launch worked. The old behaviour reported a
            // timeout here. An agent that believes its command failed retries
            // it, or goes hunting a bug that is not there.
            response.push_str(&format!(
                "\n\n[the shell exited ({}), but a process it detached still holds this \
                 command's output pipes. Output above is only what arrived before they \
                 went quiet. Use run_bash_background for work that must outlive the call.]",
                outcome.describe(),
            ));
        }

        Ok(response)
    }

    /// `run_bash_background(command, timeout_secs?)` — spawn a long-running
    /// shell command and return a `task_id` immediately. Emits
    /// `BackgroundBashStarted`; the watcher emits `BackgroundBashCompleted`
    /// when the child exits or the watchdog kills it. The registry entry
    /// outlives that event by the retention window, so a `bash_output` drain
    /// landing at the completion instant still gets the final tail.
    pub(crate) async fn execute_bash_background_tool(
        &self,
        args: &serde_json::Value,
        thread_id: uuid::Uuid,
    ) -> ToolOutcome {
        let command = match args.get("command").and_then(|v| v.as_str()) {
            Some(c) if !c.is_empty() => c,
            _ => return Err("Error: command is required".to_string()),
        };

        let timeout_secs = args
            .get("timeout_secs")
            .and_then(|v| v.as_u64())
            .unwrap_or(BG_DEFAULT_TIMEOUT_SECS)
            .min(BG_MAX_TIMEOUT_SECS);

        let env_vars = self.build_script_env_vars(Some(thread_id)).await;
        let safe_command = redact_postgres_secrets(command);
        log!(
            "[BashBg] Spawning: {}",
            &safe_command[..safe_command.floor_char_boundary(200)]
        );

        let (task_id, finish_rx) = match self
            .bash_background
            .spawn(
                command,
                timeout_secs,
                self.workspace_path(),
                &env_vars,
                Some(thread_id),
            )
            .await
        {
            Ok(pair) => pair,
            Err(e) => return Err(format!("Error: failed to spawn background command: {}", e)),
        };

        let started_at = chrono::Utc::now();

        if let Err(e) = self
            .event_bus
            .emit(BusEvent::Thread {
                thread_id,
                event: ThreadEvent::BackgroundBashStarted {
                    task_id: task_id.clone(),
                    command: safe_command.clone(),
                    timeout_secs,
                    started_at,
                },
                meta: EventMeta::NONE,
            })
            .await
        {
            log!("[BashBg] failed to emit BackgroundBashStarted: {}", e);
        }

        self.spawn_bash_completion_watcher(thread_id, task_id.clone(), safe_command, finish_rx);

        Ok(serde_json::json!({
            "task_id": task_id,
            "started_at": started_at,
            "timeout_secs": timeout_secs,
        })
        .to_string())
    }

    /// Awaits the registry watchdog's notify, reads the finished task's
    /// [`CompletionRecord`], emits `BackgroundBashCompleted` with the final
    /// state, and, if the owning CC session is still parked on this thread,
    /// pushes a synthetic
    /// `AgentUserInput { kind: User }` onto its `msg_tx` so CC resumes the
    /// turn and reads the bash result via `bash_output`. The synthetic input
    /// flows through the standard `User`-kind path so `run_session` emits a
    /// `CodingAgentPromptSent` (an exchange-starter in the frontend's
    /// `EXCHANGE_START_TYPES` set) — without this, CC's resumed tool calls
    /// would be orphaned into the prior exchange, because
    /// `BackgroundBashCompleted` itself is classified as `metadata`.
    ///
    /// The wake is the counterpart to the idle-handler gate in
    /// `run_session.rs` that suppresses propose+terminate while
    /// `BackgroundBashRegistry::has_running_for_thread` is true. Without
    /// the wake, a CC that idled mid-/harden waiting on background tests
    /// stays alive forever — the engine never tells it the bash is done,
    /// and the user has to type a manual follow-up to break the deadlock.
    ///
    /// Reading the record does NOT evict the task. That coupling is what made
    /// a drain landing anywhere between the read and the emit below miss both
    /// the registry and the not-yet-written event row, and surface to the
    /// agent as `unknown task_id` on work that had succeeded. The entry now
    /// stays drainable until the registry's retention sweep takes it.
    pub(super) fn spawn_bash_completion_watcher(
        &self,
        thread_id: uuid::Uuid,
        task_id: String,
        safe_command: String,
        finish_rx: tokio::sync::oneshot::Receiver<()>,
    ) {
        let bus = self.event_bus.clone();
        let registry = self.bash_background.clone();
        let agent_sessions = self.agent_sessions.clone();
        tokio::spawn(async move {
            // finish_rx only errors when the runtime is shutting down.
            if finish_rx.await.is_err() {
                return;
            }
            let Some(record) = registry.completion_record(&task_id).await else {
                log!("[BashBg] watcher fired but task {} already gone", task_id);
                return;
            };
            let cmd_prefix = {
                let s = safe_command.as_str();
                s[..s.floor_char_boundary(200)].to_string()
            };
            let outcome = record.outcome;
            let timed_out = record.timed_out;
            let killed = record.killed;
            let event = ThreadEvent::BackgroundBashCompleted {
                task_id: task_id.clone(),
                command: cmd_prefix.clone(),
                exit_code: outcome.exit_code(),
                signal: outcome.signal(),
                // Tail, not head — `bash_output` falls back to this payload
                // once the task is evicted, and the drain path it must agree
                // with keeps the tail. For a 40-minute build the last lines
                // are the failure; the first are `Compiling serde`.
                //
                // The record carries the whole retained buffer plus every byte
                // the registry's buffer cap cut over the task's life, so the
                // marker counts real loss and not just what the 100 KB tail
                // cap trimmed here.
                stdout: finalize_drain(&record.stdout, record.stdout_dropped),
                stderr: finalize_drain(&record.stderr, record.stderr_dropped),
                started_at: record.started_at,
                finished_at: record.finished_at,
                timed_out,
                killed,
            };
            if let Err(e) = bus
                .emit(BusEvent::Thread {
                    thread_id,
                    event,
                    meta: EventMeta::NONE,
                })
                .await
            {
                log!("[BashBg] failed to emit BackgroundBashCompleted: {}", e);
            }

            // Auto-wake the parked CC session. The wake message is informative
            // enough that CC can act without re-querying — but CC will still
            // call `bash_output` to read the actual stdout/stderr. Skip the
            // wake when the session is gone (CC truly exited, or this is a
            // chat-mode background bash with no CC session at all).
            let msg_tx = {
                let guard = agent_sessions.lock().await;
                guard.get(&thread_id).map(|s| s.msg_tx.clone())
            };
            if let Some(tx) = msg_tx {
                let wake_text =
                    format_bash_wake_text(&task_id, &cmd_prefix, outcome, killed, timed_out);
                // `AgentInputKind::User` so `run_session` emits the standard
                // `CodingAgentPromptSent` audit row — that's the frontend's
                // exchange-starter for CC's resumed work. `BackgroundBashCompleted`
                // itself is classified as `metadata` (not `start`) in
                // `EXCHANGE_START_TYPES`, so suppressing the emit would orphan
                // CC's tool calls into the prior exchange. The text is obviously
                // engine-driven ("Background task X finished...") so the
                // `User`-attributed chip won't confuse readers — we mirror the
                // `AUTO_HARDEN_MESSAGE` pattern in `change_ops::request_hardening_in_session`.
                if tx
                    .send(AgentUserInput {
                        text: wake_text,
                        images: None,
                        origin_event_id: None,
                        kind: crate::engine::types::AgentInputKind::User,
                    })
                    .is_err()
                {
                    // Channel closed = the run_session loop has already
                    // torn down. Nothing to wake; the next user follow-up
                    // (or apply click) will re-spawn CC fresh.
                    log!(
                        "[BashBg] auto-wake skipped: msg_tx closed for thread {} (session torn down)",
                        thread_id
                    );
                }
            }
        });
    }

    /// `bash_output(task_id, wait_secs?)` — drain in-memory output if
    /// the task is still running, else fall back to the persisted
    /// `BackgroundBashCompleted` event. Returns a JSON string.
    ///
    /// When `wait_secs` is provided (1..=`BASH_OUTPUT_MAX_WAIT_SECS`),
    /// blocks server-side for the full budget unless the task finishes
    /// first. Replaces the sleep-poll antipattern where a chat agent
    /// spawned `run_python_background` then issued a fresh `run_python`
    /// containing `time.sleep(N)` — that burned two tool calls per wait,
    /// doubled context, and stalled the turn. Values above the max are
    /// clamped silently so a misprompted agent can't pin a model turn
    /// forever.
    ///
    /// The result carries `elapsed_secs` (task runtime) and `waited_secs`
    /// (how long THIS call actually blocked) because the model has no
    /// clock: without them it infers elapsed time from what it *asked*
    /// for and narrates "roughly 20 minutes in" 90 seconds into a build.
    pub(crate) async fn execute_bash_output_tool(
        &self,
        args: &serde_json::Value,
        thread_id: uuid::Uuid,
    ) -> ToolOutcome {
        let task_id = match args.get("task_id").and_then(|v| v.as_str()) {
            Some(t) if !t.is_empty() => t,
            _ => return Err("Error: task_id is required".to_string()),
        };

        // Reject non-integer wait_secs explicitly. Silently coercing
        // a stringified or float value to 0 would let a misprompted
        // LLM think it waited, immediately re-poll, and trip the
        // 3-strike guard on a verbatim retry — exactly the antipattern
        // wait_secs exists to prevent. Missing arg is fine (defaults
        // to 0 = non-blocking drain).
        let wait_secs = match args.get("wait_secs") {
            None => 0u64,
            Some(serde_json::Value::Null) => 0u64,
            Some(v) => match v.as_u64() {
                Some(n) => {
                    n.min(crate::engine::tools::bash_background::BASH_OUTPUT_MAX_WAIT_SECS as u64)
                }
                None => {
                    return Err(format!(
                        "Error: wait_secs must be a non-negative integer (0..={}), got {}",
                        crate::engine::tools::bash_background::BASH_OUTPUT_MAX_WAIT_SECS,
                        v
                    ));
                }
            },
        };
        let wait = std::time::Duration::from_secs(wait_secs);

        // A message from the user outranks the rest of the wait: the loop
        // only picks injections up BETWEEN iterations, so without this the
        // follow-up sits unread for up to two minutes while we block.
        // Cancel is already handled a level up, in `run_tool_with_cancel`.
        let wakeup = (!wait.is_zero())
            .then(|| self.injection_wakeup(thread_id))
            .flatten();

        let call_started = std::time::Instant::now();
        let snapshot = match wakeup {
            Some((notify, pending)) => {
                // Register the waiter BEFORE reading the counter. `enable()`
                // is what makes this race-free: any inject() from here on
                // wakes a registered waiter, and any that already happened is
                // visible in `pending`. Checking first and registering second
                // would drop an injection landing in between — and
                // notify_waiters leaves no permit to recover it.
                let notified = notify.notified();
                tokio::pin!(notified);
                notified.as_mut().enable();

                if pending.load(std::sync::atomic::Ordering::Acquire) > 0 {
                    // The user already spoke — typically while the LLM call
                    // that produced THIS tool call was in flight. Don't block.
                    self.bash_background
                        .read_output_in_memory_wait(task_id, std::time::Duration::ZERO)
                        .await
                } else {
                    tokio::select! {
                        biased;
                        snap = self.bash_background.read_output_in_memory_wait(task_id, wait) => snap,
                        // The user spoke mid-wait. Drain what's there and hand
                        // the turn back so the loop reads their message now.
                        _ = notified => {
                            self.bash_background
                                .read_output_in_memory_wait(task_id, std::time::Duration::ZERO)
                                .await
                        }
                    }
                }
            }
            None => {
                self.bash_background
                    .read_output_in_memory_wait(task_id, wait)
                    .await
            }
        };

        if let Some(snap) = snapshot {
            return Ok(serde_json::json!({
                "stdout": finalize_drain(&snap.stdout, snap.stdout_dropped),
                "stderr": finalize_drain(&snap.stderr, snap.stderr_dropped),
                "exit_code": snap.outcome.and_then(|o| o.exit_code()),
                "signal": snap.outcome.and_then(|o| o.signal()),
                "status": snap.outcome.map(|o| o.describe()),
                "finished": snap.finished,
                "timed_out": snap.timed_out,
                "killed": snap.killed,
                "elapsed_secs": snap.elapsed_secs,
                "waited_secs": call_started.elapsed().as_secs(),
            })
            .to_string());
        }

        let row: Option<(serde_json::Value,)> = match sqlx::query_as(
            r#"SELECT payload FROM events
               WHERE event_type = 'BackgroundBashCompleted'
                 AND aggregate_id = $1
                 AND payload->>'task_id' = $2
               ORDER BY sequence DESC
               LIMIT 1"#,
        )
        .bind(thread_id.to_string())
        .bind(task_id)
        .fetch_optional(&self.pool)
        .await
        {
            Ok(r) => r,
            Err(e) => return Err(format!("Error: failed to query completion event: {}", e)),
        };

        match row {
            Some((payload,)) => {
                // Rebuild the outcome from the persisted pair so this branch
                // renders exactly what the in-memory drain rendered — the two
                // branches are the same tool call to the LLM and must never
                // disagree. Legacy rows have no `signal` key.
                let outcome = TaskOutcome::from_persisted(
                    payload
                        .get("exit_code")
                        .and_then(|v| v.as_i64())
                        .map(|c| c as i32),
                    payload
                        .get("signal")
                        .and_then(|v| v.as_i64())
                        .map(|s| s as i32),
                );
                Ok(serde_json::json!({
                    "stdout": payload.get("stdout").cloned().unwrap_or(serde_json::Value::String(String::new())),
                    "stderr": payload.get("stderr").cloned().unwrap_or(serde_json::Value::String(String::new())),
                    "exit_code": outcome.exit_code(),
                    "signal": outcome.signal(),
                    "status": outcome.describe(),
                    "finished": true,
                    "timed_out": payload.get("timed_out").cloned().unwrap_or(serde_json::Value::Bool(false)),
                    "killed": payload.get("killed").cloned().unwrap_or(serde_json::Value::Bool(false)),
                    // Same two time fields as the in-memory branch, so the LLM
                    // never has to guess how long the task ran just because the
                    // registry entry was gone by the time it drained.
                    // `waited_secs` is measured, not assumed zero: an unknown
                    // task_id returns without waiting, but a drain that DID
                    // block and then found the retention window closed spent
                    // real time doing it.
                    "elapsed_secs": persisted_elapsed_secs(&payload),
                    "waited_secs": call_started.elapsed().as_secs(),
                })
                .to_string())
            }
            None => Err(format!("Error: unknown task_id '{}'", task_id)),
        }
    }

    /// `bash_kill(task_id)` — cancel a running background task. No-op if
    /// the task is unknown or already finished.
    pub(crate) async fn execute_bash_kill_tool(&self, args: &serde_json::Value) -> ToolOutcome {
        let task_id = match args.get("task_id").and_then(|v| v.as_str()) {
            Some(t) if !t.is_empty() => t,
            _ => return Err("Error: task_id is required".to_string()),
        };
        if self.bash_background.kill(task_id).await {
            Ok(format!("killed {}", task_id))
        } else {
            Err(format!(
                "Error: unknown or already finished task_id '{}'",
                task_id
            ))
        }
    }
}

fn truncate_output(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        let end = s.floor_char_boundary(max);
        format!("{}...\n[truncated — {} bytes total]", &s[..end], s.len())
    }
}

/// Task runtime in seconds from a persisted `BackgroundBashCompleted`
/// payload. `null` for a legacy row that predates the timestamp pair —
/// an honest "unknown" the LLM can read, rather than a fabricated `0`
/// it would narrate as "the build took no time at all".
fn persisted_elapsed_secs(payload: &serde_json::Value) -> Option<i64> {
    let ts = |key: &str| {
        payload
            .get(key)
            .and_then(|v| v.as_str())
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
    };
    let (started, finished) = (ts("started_at")?, ts("finished_at")?);
    Some((finished - started).num_seconds().max(0))
}

/// Keep the last `max` bytes rather than the first. The marker leads so the
/// LLM sees "this is a tail" before the content, and states how much was
/// dropped so it knows to widen the window (or read a log file) if it needs
/// the earlier lines. `already_dropped` counts output lost upstream (the
/// registry's buffer cap) and is added to whatever this call trims, so the
/// figure is the total gap and never just the visible part of it.
fn truncate_output_tail(s: &str, max: usize, already_dropped: usize) -> String {
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

/// Text the engine pushes to CC via `msg_tx` when a background task
/// (spawned via `run_bash_background` OR `run_python_background`) completes
/// and CC is parked waiting on it. Extracted as a free function so the
/// formatting can be regression-pinned without spinning up the full watcher
/// (which depends on `agent_sessions`, the event bus, and a real
/// `BackgroundBashRegistry`).
///
/// The text gives CC enough context to act without re-querying — the
/// task_id, the command prefix it spawned, and an outcome phrase — but
/// CC still calls `bash_output(task_id)` to read the actual stdout/
/// stderr. Killed and timed-out cases keep their own leading word so
/// CC knows the result isn't "exit code 0 — clean success".
///
/// The phrase comes from [`TaskOutcome::describe`], the same source the
/// `bash_output` JSON and the sync `run_bash` result use, so the three
/// LLM-facing surfaces cannot drift apart. Crucially it never invents a
/// number: a signal death reads `killed by SIGKILL (signal 9)` and a status
/// the engine failed to obtain reads `exit code unknown`.
///
/// The engine-caused endings (`bash_kill`, watchdog timeout) are reported
/// *alongside* the real status rather than instead of it — "timed out —
/// killed by SIGKILL (signal 9)" tells CC both that the deadline fired and
/// how the child actually died, which the old bare "timed out" did not.
///
/// "Background task" instead of "Background bash task" — the same watcher
/// fires for python-spawned tasks (via `run_python_background`), and the
/// command-prefix line tells the LLM which it was. `bash_output` /
/// `bash_kill` are correct because they ARE the consumer tools for both.
fn format_bash_wake_text(
    task_id: &str,
    cmd_prefix: &str,
    outcome: TaskOutcome,
    killed: bool,
    timed_out: bool,
) -> String {
    let status = match (killed, timed_out) {
        (true, _) => format!("stopped by bash_kill — {}", outcome.describe()),
        (_, true) => format!("timed out — {}", outcome.describe()),
        _ => outcome.describe(),
    };
    format!(
        "Background task {} finished ({}): {}\n\nUse `bash_output(\"{}\")` to read the result and continue your work.",
        task_id, status, cmd_prefix, task_id
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Spawn through the same shell and pipe setup `execute_bash_tool` uses,
    /// so these exercise the real fd topology rather than a simplified one.
    fn spawn_like_the_tool(command: &str) -> tokio::process::Child {
        command_shell()
            .command(command)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .expect("spawn")
    }

    /// Output and status for the ordinary case, and promptly.
    ///
    /// It deliberately does NOT assert `Completed` over `Detached`. Two
    /// concurrent spawns can cross-inherit a pipe write end through the
    /// parent's `dup2` window. A sibling test's shell then holds this one's
    /// pipe open and delays EOF. The bytes and the status are exact either
    /// way, and those are what the caller reads.
    #[tokio::test]
    async fn an_ordinary_command_reports_its_output_and_exit_status() {
        let started = std::time::Instant::now();
        let child = spawn_like_the_tool("echo out; echo err >&2; exit 3");
        let (outcome, stdout, stderr) = match wait_for_shell(child, Duration::from_secs(10))
            .await
            .unwrap()
        {
            ShellWait::Completed {
                outcome,
                stdout,
                stderr,
            }
            | ShellWait::Detached {
                outcome,
                stdout,
                stderr,
            } => (outcome, stdout, stderr),
            other => panic!("expected the shell to exit, got {other:?}"),
        };

        assert_eq!(outcome, TaskOutcome::Exited(3));
        assert_eq!(String::from_utf8_lossy(&stdout), "out\n");
        assert_eq!(String::from_utf8_lossy(&stderr), "err\n");
        assert!(
            started.elapsed() < Duration::from_secs(9),
            "a command that exits must not wait out its budget: {:?}",
            started.elapsed()
        );
    }

    /// The incident shape. `&` binds looser than `&&`, so the whole chain runs
    /// in one backgrounded subshell that keeps both pipe write ends open.
    /// `wait_with_output()` waits for pipe EOF, so it blocked the full timeout
    /// and reported a failure for a launch that had already succeeded.
    #[tokio::test]
    async fn a_detached_subshell_holding_the_pipes_does_not_look_like_a_timeout() {
        let started = std::time::Instant::now();
        let child = spawn_like_the_tool("echo launched && sleep 30 >/dev/null 2>&1 & echo queued");
        // A 30s budget the detached `sleep` would blow through if we waited on
        // pipe EOF. Reaching Detached quickly is the whole point.
        let waited = wait_for_shell(child, Duration::from_secs(30))
            .await
            .unwrap();

        assert!(
            started.elapsed() < Duration::from_secs(5),
            "must not wait for the detached child: took {:?}",
            started.elapsed()
        );
        match waited {
            ShellWait::Detached {
                outcome, stdout, ..
            } => {
                assert_eq!(
                    outcome,
                    TaskOutcome::Exited(0),
                    "the shell itself succeeded"
                );
                assert!(
                    String::from_utf8_lossy(&stdout).contains("queued"),
                    "output drained before the grace expired: {:?}",
                    String::from_utf8_lossy(&stdout)
                );
            }
            other => panic!("expected Detached, got {other:?}"),
        }
    }

    /// Returning must not kill what we chose not to kill. The reader task owns
    /// the pipe handle, so aborting it closes the read end, and the detached
    /// process takes `SIGPIPE` on its next write. `run_bash` would then report
    /// a survivor it had just killed. Reading on, and discarding, is the fix.
    ///
    /// The chain writes to the inherited pipe a second after the grace has
    /// expired, then touches the marker. A closed read end kills it before it
    /// gets there.
    #[tokio::test]
    async fn a_detached_process_is_not_killed_by_the_pipes_closing() {
        let marker =
            std::env::temp_dir().join(format!("lucidos-detached-live-{}", std::process::id()));
        // Unrecoverable cleanup: a leftover from an earlier aborted run.
        let _ = std::fs::remove_file(&marker);

        let child = spawn_like_the_tool(&format!(
            "echo queued && sleep 1 && echo late && touch '{}' & echo done",
            marker.display()
        ));
        let waited = wait_for_shell(child, Duration::from_secs(30))
            .await
            .unwrap();
        assert!(
            matches!(waited, ShellWait::Detached { .. }),
            "expected Detached, got {waited:?}"
        );

        tokio::time::sleep(Duration::from_millis(2_000)).await;
        assert!(
            marker.exists(),
            "the detached chain died, most likely on SIGPIPE from a closed read end"
        );
        // Unrecoverable cleanup: the marker has served its purpose.
        let _ = std::fs::remove_file(&marker);
    }

    /// One pipe closes while the other stays live and keeps producing, so the
    /// quiet window expires and the wait loops.
    ///
    /// That is the only path where a reader completes but its partner does
    /// not. A `JoinHandle` that has returned `Ready` must never be polled
    /// again. The detached group redirects its stdout away, so the stdout
    /// reader reaches EOF while the stderr reader is still going.
    #[tokio::test]
    async fn a_pipe_closing_while_the_other_keeps_producing_does_not_repoll_it() {
        let child = spawn_like_the_tool(
            "echo out; { exec >/dev/null; i=0; while [ $i -lt 6 ]; do echo tick >&2; \
             sleep 0.3; i=$((i+1)); done; } & echo done",
        );
        let (outcome, stdout, stderr) = match wait_for_shell(child, Duration::from_secs(30))
            .await
            .unwrap()
        {
            ShellWait::Completed {
                outcome,
                stdout,
                stderr,
            }
            | ShellWait::Detached {
                outcome,
                stdout,
                stderr,
            } => (outcome, stdout, stderr),
            other => panic!("expected the shell to exit, got {other:?}"),
        };

        assert_eq!(outcome, TaskOutcome::Exited(0));
        assert!(
            String::from_utf8_lossy(&stdout).contains("done"),
            "stdout: {:?}",
            String::from_utf8_lossy(&stdout)
        );
        assert!(
            String::from_utf8_lossy(&stderr).contains("tick"),
            "the live pipe kept draining across windows: {:?}",
            String::from_utf8_lossy(&stderr)
        );
    }

    /// The distinction the fix rests on: a shell that never exits is still a
    /// real timeout, and must not be softened into a success.
    ///
    /// It must also STOP the work, not just stop waiting for it.
    /// `wait_for_shell` drops the child, and the caller's `kill_on_drop` turns
    /// that into a SIGKILL. The marker file is the proof: a surviving shell
    /// would create it a second later.
    #[tokio::test]
    async fn a_timeout_reports_a_timeout_and_kills_the_shell() {
        let marker =
            std::env::temp_dir().join(format!("lucidos-timeout-kill-{}", std::process::id()));
        // Unrecoverable cleanup: a leftover from an earlier aborted run.
        let _ = std::fs::remove_file(&marker);

        let child = spawn_like_the_tool(&format!("sleep 1; touch '{}'", marker.display()));
        let waited = wait_for_shell(child, Duration::from_millis(200))
            .await
            .unwrap();
        assert!(
            matches!(waited, ShellWait::TimedOut),
            "expected TimedOut, got {waited:?}"
        );

        // Outlive the `sleep` the shell was running when we gave up on it.
        tokio::time::sleep(Duration::from_millis(1_500)).await;
        assert!(
            !marker.exists(),
            "the shell survived the timeout and kept working"
        );
    }

    /// Dropping `wait_with_output()` for `child.wait()` reopens the classic
    /// deadlock: a child that fills the pipe buffer blocks forever unless
    /// something drains it concurrently. 328 KB is several times any pipe
    /// buffer, so this hangs to the timeout if the readers are not concurrent.
    ///
    /// Like the test above it accepts either exited variant, for the same
    /// spawn-race reason. Every byte surviving is the assertion that matters.
    #[tokio::test]
    async fn a_chatty_command_does_not_deadlock_on_a_full_pipe() {
        let child = spawn_like_the_tool(
            r#"awk 'BEGIN{ for (i = 0; i < 8000; i++) print "0123456789012345678901234567890123456789" }'"#,
        );
        let (outcome, stdout) = match wait_for_shell(child, Duration::from_secs(20))
            .await
            .unwrap()
        {
            ShellWait::Completed {
                outcome, stdout, ..
            }
            | ShellWait::Detached {
                outcome, stdout, ..
            } => (outcome, stdout),
            other => panic!("expected the shell to exit, got {other:?}"),
        };

        assert_eq!(outcome, TaskOutcome::Exited(0));
        assert_eq!(stdout.len(), 8000 * 41, "every byte must survive");
    }

    #[test]
    fn truncate_output_short_string() {
        let s = "hello world";
        assert_eq!(truncate_output(s, 100), "hello world");
    }

    #[test]
    fn truncate_output_long_string() {
        let s = "a".repeat(200);
        let result = truncate_output(&s, 50);
        assert!(result.starts_with(&"a".repeat(50)));
        assert!(result.contains("[truncated"));
        assert!(result.contains("200 bytes total"));
    }

    #[test]
    fn truncate_output_tail_keeps_the_end_not_the_start() {
        // A drain window is a progress view: the newest lines say where the
        // build got to and which step failed. Head-truncating an oversized
        // window would hand the LLM `Compiling serde` and drop the error.
        let s = format!("{}FAILED: linker error", "noise\n".repeat(500));
        let result = truncate_output_tail(&s, 100, 0);
        assert!(
            result.ends_with("FAILED: linker error"),
            "tail truncation must keep the end: {:?}",
            result
        );
        assert!(
            result.starts_with("[truncated"),
            "the marker leads so the LLM knows it's a tail: {:?}",
            result
        );
        assert!(result.contains("earlier bytes dropped"));
    }

    #[test]
    fn truncate_output_tail_counts_upstream_loss_too() {
        // A full-budget wait can outrun the registry's 2 MB buffer cap, which
        // discards unread bytes before this function ever sees them. Counting
        // only what we trim here would report a smaller gap than really
        // occurred — and a truncation marker that understates reads as a
        // bound, so the LLM would trust a number that is a lie.
        let s = "x".repeat(300);
        let result = truncate_output_tail(&s, 100, 5_000);
        assert!(
            result.contains("5200 earlier bytes dropped"),
            "marker must add upstream loss (5000) to its own trim (200): {:?}",
            &result[..result.find('\n').unwrap_or(result.len())]
        );
        assert!(
            result.contains("of 5300 total"),
            "total must include the bytes lost upstream: {:?}",
            &result[..result.find('\n').unwrap_or(result.len())]
        );
    }

    #[test]
    fn truncate_output_tail_marks_upstream_loss_even_when_it_fits() {
        // Output that fits the cap is normally returned verbatim — but if the
        // registry already dropped bytes, saying nothing would present a
        // partial window as complete.
        let result = truncate_output_tail("the tail", 100, 900);
        assert!(
            result.contains("900 earlier bytes dropped"),
            "a short window after upstream loss must still be marked: {:?}",
            result
        );
        assert!(result.ends_with("the tail"));
    }

    #[test]
    fn truncate_output_tail_short_string_is_untouched() {
        assert_eq!(truncate_output_tail("hello world", 100, 0), "hello world");
    }

    #[test]
    fn truncate_output_tail_multibyte_boundary() {
        // ceil_char_boundary must not split a multi-byte char — slicing by
        // raw byte index would panic. 'é' is 2 bytes, so a 5-byte budget
        // lands mid-char and has to round outward.
        let s = "ééééé"; // 10 bytes in UTF-8
        let result = truncate_output_tail(s, 5, 0);
        assert!(result.contains("[truncated"));
        assert!(result.ends_with("éé"), "got {:?}", result);
    }

    #[test]
    fn persisted_elapsed_secs_reads_the_timestamp_pair() {
        let payload = serde_json::json!({
            "started_at": "2026-07-27T10:00:00Z",
            "finished_at": "2026-07-27T10:41:30Z",
        });
        assert_eq!(persisted_elapsed_secs(&payload), Some(2490));
    }

    #[test]
    fn persisted_elapsed_secs_is_none_for_a_legacy_row() {
        // An honest "unknown" beats a fabricated 0 the LLM would narrate as
        // "the build took no time at all".
        assert_eq!(persisted_elapsed_secs(&serde_json::json!({})), None);
        assert_eq!(
            persisted_elapsed_secs(&serde_json::json!({"started_at": "not a date"})),
            None
        );
    }

    #[test]
    fn truncate_output_multibyte_boundary() {
        let s = "ééééé"; // 10 bytes in UTF-8
        let result = truncate_output(s, 5);
        assert!(result.contains("[truncated"));
    }

    /// Regression for the May-2026 premature-Apply incident: when CC
    /// idled mid-/harden waiting on its own `run_bash_background` Rust
    /// tests, the engine had no way to tell CC the bash had finished —
    /// the user had to type a manual follow-up to break the deadlock.
    /// The watcher now pushes `format_bash_wake_text` onto `msg_tx`
    /// with `AgentInputKind::User` (so `CodingAgentPromptSent` fires
    /// as the exchange-starter). Pin the text so a CC-side regression
    /// that stops calling `bash_output` after the wake shows up in
    /// this test, not in the next user incident.
    #[test]
    fn format_bash_wake_text_clean_exit_zero() {
        let text = format_bash_wake_text(
            "task-123",
            "cargo test --lib",
            TaskOutcome::Exited(0),
            false,
            false,
        );
        assert!(
            text.contains("task-123"),
            "task_id must appear so CC can refer to it"
        );
        assert!(
            text.contains("exit code 0"),
            "clean exit must say 'exit code 0'"
        );
        assert!(
            text.contains("cargo test --lib"),
            "command prefix gives CC context"
        );
        assert!(
            text.contains("bash_output(\"task-123\")"),
            "must instruct CC to read the result via bash_output"
        );
    }

    #[test]
    fn format_bash_wake_text_non_zero_exit() {
        let text = format_bash_wake_text("t1", "npm test", TaskOutcome::Exited(1), false, false);
        assert!(text.contains("exit code 1"));
    }

    /// The two statuses the 2026-07-26 nightly actually observed, both of
    /// which reached the agent as "exit code 0". The summary must carry the
    /// real number.
    #[test]
    fn format_bash_wake_text_reports_the_nightly_statuses_verbatim() {
        let clippy = format_bash_wake_text(
            "t-clippy",
            "cargo clippy --all-targets | tee build.log",
            TaskOutcome::Exited(101),
            false,
            false,
        );
        assert!(clippy.contains("exit code 101"), "got: {clippy}");
        assert!(
            !clippy.contains("exit code 0"),
            "the masking trap is back: {clippy}"
        );

        let e2e = format_bash_wake_text(
            "t-e2e",
            "./scripts/e2e.sh | tee e2e.log",
            TaskOutcome::Exited(1),
            false,
            false,
        );
        assert!(e2e.contains("exit code 1"), "got: {e2e}");
        assert!(
            !e2e.contains("exit code 0"),
            "the masking trap is back: {e2e}"
        );
    }

    /// A signal death is named, never rendered as an exit code and never as
    /// a bare number the reader can mistake for one.
    #[test]
    fn format_bash_wake_text_names_the_signal() {
        let text = format_bash_wake_text("t5", "./flaky", TaskOutcome::Signaled(9), false, false);
        assert!(
            text.contains("killed by SIGKILL (signal 9)"),
            "signal death must be named: {text}"
        );
        assert!(
            !text.contains("exit code"),
            "a signal death has no exit code: {text}"
        );

        let segv = format_bash_wake_text("t6", "./crash", TaskOutcome::Signaled(11), false, false);
        assert!(
            segv.contains("killed by SIGSEGV (signal 11)"),
            "got: {segv}"
        );
    }

    /// `bash_kill` leads with the cause so CC can't read the line as a
    /// completion, and still reports how the child actually died.
    #[test]
    fn format_bash_wake_text_killed_leads_with_the_cause() {
        let text = format_bash_wake_text("t2", "sleep 30", TaskOutcome::Signaled(9), true, false);
        assert!(
            text.contains("stopped by bash_kill"),
            "the kill must be the leading fact: {text}"
        );
        assert!(
            text.contains("SIGKILL"),
            "and it must still say how the child died: {text}"
        );
        assert!(
            !text.contains("exit code"),
            "a SIGKILLed child has no exit code: {text}"
        );
    }

    /// Timeout is the other engine-caused ending. Same shape: the deadline
    /// leads, the real status follows.
    #[test]
    fn format_bash_wake_text_timed_out_reports_deadline_and_signal() {
        let text =
            format_bash_wake_text("t3", "sleep 99999", TaskOutcome::Signaled(9), false, true);
        assert!(text.contains("timed out"), "got: {text}");
        assert!(
            text.contains("killed by SIGKILL (signal 9)"),
            "the timeout summary must name the signal the watchdog used: {text}"
        );
        assert!(!text.contains("exit code"), "got: {text}");
    }

    /// The invariant the whole change exists for: a status the engine could
    /// not obtain says so in words. It is never `0`, never `-1`, never any
    /// digit at all.
    #[test]
    fn format_bash_wake_text_unknown_status_is_words_not_a_number() {
        let text = format_bash_wake_text("t4", "true", TaskOutcome::Unknown, false, false);
        assert!(
            text.contains("exit code unknown"),
            "an unavailable status must say so: {text}"
        );
        let status_phrase = text
            .split_once('(')
            .and_then(|(_, rest)| rest.split_once(')'))
            .map(|(inside, _)| inside)
            .expect("summary carries a parenthesised status");
        assert!(
            !status_phrase.chars().any(|c| c.is_ascii_digit()),
            "unknown must not render any number, got: {status_phrase}"
        );
    }
}
