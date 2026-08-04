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

const MAX_OUTPUT_BYTES: usize = 100 * 1024; // 100 KB

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

        let output = match tokio::time::timeout(
            std::time::Duration::from_secs(timeout_secs),
            child.wait_with_output(),
        )
        .await
        {
            Ok(Ok(output)) => output,
            Ok(Err(e)) => return Err(format!("executing command: {}", e).into()),
            Err(_) => {
                // Timeout — wait_with_output() is dropped here, taking the
                // owned Child with it; kill_on_drop on the Command above
                // means the OS sends SIGKILL. The shell child exits before
                // this function returns.
                return Err(format!("command timed out after {}s", timeout_secs).into());
            }
        };

        let stdout = finalize_stream(&output.stdout);
        let stderr = finalize_stream(&output.stderr);
        // Typed, so a signal death can't be flattened into a bare number. The
        // old `status.code().unwrap_or(-1)` reported a SIGSEGV as `-1`, which
        // reads like an ordinary exit code.
        let outcome = TaskOutcome::from_status(output.status);

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
