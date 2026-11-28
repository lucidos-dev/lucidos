use super::super::LucidosEngine;
use super::ToolOutcome;
use crate::core::redact_postgres_secrets;
use crate::core::shell::{command_shell, finalize_stream, TaskOutcome, KEPT_BYTES_CAP};
use crate::engine::bash_background::{finalize_drain, CompletionDelivery, TaskSpec};
use crate::engine::bash_background_recovery;
use crate::engine::event_bus::BusEvent;
use crate::engine::thread_events::{EventMeta, ThreadEvent};
use crate::llm::tools::{
    BG_DEFAULT_TIMEOUT_SECS, BG_MAX_TIMEOUT_SECS, DEFAULT_TIMEOUT_SECS, MAX_TIMEOUT_SECS,
};
use crate::runtime::agent_sandbox::{Sandbox, SandboxedCommand};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::AsyncReadExt;

const DETACHED_QUIET_WINDOW: Duration = Duration::from_millis(500);

const DETACHED_MAX_WAIT: Duration = Duration::from_secs(5);

#[derive(Debug)]
enum ShellWait {
    Completed {
        outcome: TaskOutcome,
        stdout: Vec<u8>,
        stderr: Vec<u8>,
        stdout_total: u64,
        stderr_total: u64,
    },
    Detached {
        outcome: TaskOutcome,
        stdout: Vec<u8>,
        stderr: Vec<u8>,
        stdout_total: u64,
        stderr_total: u64,
    },
    TimedOut,
}

struct PipeSink {
    buf: Mutex<Vec<u8>>,
    read_bytes: AtomicU64,
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

async fn drain_pipe<R: tokio::io::AsyncRead + Unpin>(mut pipe: R, sink: Arc<PipeSink>) {
    let mut buf = [0u8; 8192];
    loop {
        match pipe.read(&mut buf).await {
            Ok(0) => return,
            Ok(n) => {
                sink.read_bytes.fetch_add(n as u64, Ordering::Relaxed);
                if !sink.discard.load(Ordering::Relaxed) {
                    let mut kept = sink.buf.lock().expect("drain buffer");
                    let room = KEPT_BYTES_CAP.saturating_sub(kept.len());
                    kept.extend_from_slice(&buf[..n.min(room)]);
                }
            }
            Err(e) => {
                log_legacy!("[Bash] pipe read ended with an error: {}", e);
                return;
            }
        }
    }
}

/// INVARIANT: Wait for both readers to reach EOF after the shell has exited. `true` means
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
        // INVARIANT: Spawned alongside the window, so it competes for the same scheduler.
        // A trivial task that does not finish in half a second means the
        // runtime never got to the readers either.
        let canary = tokio::spawn(async {});
        let finished = tokio::time::timeout(window, async {
            for slot in readers.iter_mut() {
                if let Some(reader) = slot.as_mut() {
                    let _ = reader.await;
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
        // INVARIANT: No bytes this window. That is only evidence of an idle pipe if the
        // readers actually ran. A starved reader looks identical, and calling
        // it detached discards output the OS had already buffered.
        if !canary.is_finished() {
            continue;
        }
        return false;
    }
}

struct ShellGroupGuard(Option<u32>);

impl Drop for ShellGroupGuard {
    fn drop(&mut self) {
        if let Some(pid) = self.0 {
            crate::runtime::spawn_env::kill_child_process_group_now(pid);
        }
    }
}

fn spawn_shell(cmd: &mut SandboxedCommand) -> std::io::Result<tokio::process::Child> {
    crate::runtime::spawn_env::spawn_below_engine(cmd)
}

async fn wait_for_shell(
    mut child: tokio::process::Child,
    timeout: Duration,
) -> Result<ShellWait, Box<dyn std::error::Error + Send + Sync>> {
    let mut group = ShellGroupGuard(child.id());
    let out_sink = PipeSink::new();
    let err_sink = PipeSink::new();
    let mut readers: Vec<Option<tokio::task::JoinHandle<()>>> = Vec::with_capacity(2);
    if let Some(pipe) = child.stdout.take() {
        readers.push(Some(tokio::spawn(drain_pipe(pipe, Arc::clone(&out_sink)))));
    }
    if let Some(pipe) = child.stderr.take() {
        readers.push(Some(tokio::spawn(drain_pipe(pipe, Arc::clone(&err_sink)))));
    }

    let release = |out: &Arc<PipeSink>, err: &Arc<PipeSink>| {
        out.discard.store(true, Ordering::Relaxed);
        err.discard.store(true, Ordering::Relaxed);
    };

    let status = match tokio::time::timeout(timeout, child.wait()).await {
        Ok(status) => {
            group.0 = None;
            status.map_err(|e| format!("executing command: {}", e))?
        }
        Err(_) => {
            release(&out_sink, &err_sink);
            return Ok(ShellWait::TimedOut);
        }
    };

    let drained = drain_until_quiet(&mut readers, &out_sink, &err_sink).await;

    let outcome = TaskOutcome::from_status(status);
    let stdout = out_sink.take();
    let stderr = err_sink.take();
    let (stdout_total, stderr_total) = (out_sink.progress(), err_sink.progress());
    release(&out_sink, &err_sink);
    Ok(if drained {
        ShellWait::Completed {
            outcome,
            stdout,
            stderr,
            stdout_total,
            stderr_total,
        }
    } else {
        ShellWait::Detached {
            outcome,
            stdout,
            stderr,
            stdout_total,
            stderr_total,
        }
    })
}

pub(super) fn background_start_result(
    task_id: &str,
    started_at: chrono::DateTime<chrono::Utc>,
    timeout_secs: u64,
    env: &crate::engine::engine_impl::ScriptEnv,
) -> String {
    let mut result = serde_json::json!({
        "task_id": task_id,
        "started_at": started_at,
        "timeout_secs": timeout_secs,
    });
    if let Some(note) = &env.secret_note {
        result["note"] = serde_json::Value::String(note.clone());
    }
    result.to_string()
}

#[easy_ext::ext(BashExt)]
pub(crate) impl LucidosEngine {
    async fn execute_bash_tool(
        &self,
        args: &serde_json::Value,
        thread_id: uuid::Uuid,
        sandbox: &Sandbox,
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

        let env = self
            .build_tool_env_vars(
                thread_id,
                &crate::core::credentials::SecretMentions::of_command(
                    command,
                    self.workspace_path(),
                ),
            )
            .await;
        let env_vars = &env.vars;

        let mut cmd = command_shell().command(sandbox, command)?;
        cmd.current_dir(self.workspace_path())
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            // INVARIANT: Two callers can drop the wait on this child: a user cancel
            // through run_tool_with_cancel, and the per-tool timeout.
            // `wait_for_shell` kills the shell's group on both; kill_on_drop
            // also covers a drop before the wait starts.
            .kill_on_drop(true);
        crate::core::apply_to_subprocess_env(
            &mut cmd,
            env_vars,
            crate::core::InheritedSecrets::Strip(crate::core::inherited_secret_env()),
        );

        let log_command = redact_postgres_secrets(command);
        log_legacy!(
            "[Bash] Running: {}",
            &log_command[..log_command.floor_char_boundary(200)]
        );

        let started = chrono::Utc::now();
        let child = spawn_shell(&mut cmd).map_err(|e| format!("Failed to spawn command: {}", e))?;

        let pid = child.id();
        let waited = wait_for_shell(child, Duration::from_secs(timeout_secs)).await?;
        let (detached, outcome, raw_stdout, raw_stderr, stdout_total, stderr_total) = match waited {
            ShellWait::TimedOut => {
                return Err(format!("command timed out after {}s", timeout_secs).into());
            }
            ShellWait::Completed {
                outcome,
                stdout,
                stderr,
                stdout_total,
                stderr_total,
            } => (false, outcome, stdout, stderr, stdout_total, stderr_total),
            ShellWait::Detached {
                outcome,
                stdout,
                stderr,
                stdout_total,
                stderr_total,
            } => (true, outcome, stdout, stderr, stdout_total, stderr_total),
        };

        let stdout = finalize_stream(&raw_stdout, stdout_total);
        let mut stderr = finalize_stream(&raw_stderr, stderr_total);
        if let Some(denial) = sandbox.seatbelt_write_refusal(pid, started, &outcome).await {
            if !stderr.is_empty() {
                stderr.push('\n');
            }
            stderr.push_str(&denial);
        }

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
            response.push_str(&format!(
                "\n\n[the shell exited ({}), but a process it detached still holds this \
                 command's output pipes. Output above is only what arrived before they \
                 went quiet. Use run_bash_background for work that must outlive the call.]",
                outcome.describe(),
            ));
        }

        // INVARIANT: Last thing before the result leaves: the caller persists it into the
        // `ToolResult` event verbatim. See `core::subprocess_secret_values`.
        let response = crate::core::redact_subprocess_output(
            &env.with_note(response),
            &crate::core::subprocess_secret_values(env_vars, crate::core::inherited_secret_env()),
        );
        let refused = !outcome.is_success()
            && crate::triggers::folder_grant::folder_refusal(
                sandbox,
                self.workspace_path(),
                &stderr,
            )
            .is_some();
        if refused {
            Err(response.into())
        } else {
            Ok(response)
        }
    }

    async fn execute_bash_background_tool(
        &self,
        args: &serde_json::Value,
        thread_id: uuid::Uuid,
        sandbox: &Sandbox,
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

        let env = self
            .build_tool_env_vars(
                thread_id,
                &crate::core::credentials::SecretMentions::of_command(
                    command,
                    self.workspace_path(),
                ),
            )
            .await;
        let (task_id, started_at) = self
            .start_background_task(
                sandbox,
                thread_id,
                command,
                self.workspace_path(),
                &env.vars,
                TaskSpec {
                    timeout_secs,
                    description: args.get("description").and_then(|v| v.as_str()),
                    delivery: CompletionDelivery::EventWait,
                    inherited: crate::core::InheritedSecrets::Strip(
                        crate::core::inherited_secret_env(),
                    ),
                },
            )
            .await?;

        Ok(background_start_result(
            &task_id,
            started_at,
            timeout_secs,
            &env,
        ))
    }

    async fn start_background_task(
        &self,
        sandbox: &Sandbox,
        thread_id: uuid::Uuid,
        command: &str,
        cwd: &std::path::Path,
        env_vars: &[(String, String)],
        spec: TaskSpec<'_>,
    ) -> Result<(String, chrono::DateTime<chrono::Utc>), String> {
        let safe_command = redact_postgres_secrets(command);
        log_legacy!(
            "[BashBg] Spawning: {}",
            &safe_command[..safe_command.floor_char_boundary(200)]
        );

        let (task_id, finish_rx) = self
            .bash_background
            .spawn_task(sandbox, command, cwd, env_vars, Some(thread_id), spec)
            .await
            .map_err(|e| format!("Error: failed to spawn background command: {}", e))?;

        let started_at = chrono::Utc::now();

        if let Err(e) = self
            .event_bus
            .emit(BusEvent::Thread {
                thread_id,
                event: ThreadEvent::BackgroundBashStarted {
                    task_id: task_id.clone(),
                    command: safe_command.clone(),
                    timeout_secs: spec.timeout_secs,
                    started_at,
                },
                meta: EventMeta::NONE,
            })
            .await
        {
            log_legacy!("[BashBg] failed to emit BackgroundBashStarted: {}", e);
        }

        self.spawn_bash_completion_watcher(thread_id, task_id.clone(), finish_rx);

        Ok((task_id, started_at))
    }

    fn spawn_bash_completion_watcher(
        &self,
        thread_id: uuid::Uuid,
        task_id: String,
        finish_rx: tokio::sync::oneshot::Receiver<()>,
    ) {
        let bus = self.event_bus.clone();
        let registry = self.bash_background.clone();
        tokio::spawn(async move {
            if finish_rx.await.is_err() {
                return;
            }
            let Some(record) = registry.completion_record(&task_id).await else {
                log_legacy!(
                    "[BashBg] watcher fired but task {} is gone or already recorded",
                    task_id
                );
                return;
            };
            // INVARIANT: The teardown may have killed this task on its way out and then
            // lost the race to record it. Built through the shared builder for
            // exactly that reason: the row must not depend on who won.
            let event = bash_background_recovery::completion_event(task_id.clone(), record);
            if let Err(e) = bus
                .emit(BusEvent::Thread {
                    thread_id,
                    event,
                    meta: EventMeta::NONE,
                })
                .await
            {
                log_legacy!("[BashBg] failed to emit BackgroundBashCompleted: {}", e);
                // INVARIANT: Hand the claim back. It is a promise to write the row, and
                // this one did not. Holding it tells the teardown the task is
                // settled, and the boot sweep then reports a success as a loss.
                registry.release_completion_claim(&task_id).await;
            }
        });
    }

    async fn execute_bash_output_tool(
        &self,
        args: &serde_json::Value,
        thread_id: uuid::Uuid,
    ) -> ToolOutcome {
        let task_id = match args.get("task_id").and_then(|v| v.as_str()) {
            Some(t) if !t.is_empty() => t,
            _ => return Err("Error: task_id is required".to_string()),
        };

        let wait_secs = match args.get("wait_secs") {
            None => 0u64,
            Some(serde_json::Value::Null) => 0u64,
            Some(v) => match v.as_u64() {
                Some(n) => n.min(crate::engine::bash_background::BASH_OUTPUT_MAX_WAIT_SECS as u64),
                None => {
                    return Err(format!(
                        "Error: wait_secs must be a non-negative integer (0..={}), got {}",
                        crate::engine::bash_background::BASH_OUTPUT_MAX_WAIT_SECS,
                        v
                    ));
                }
            },
        };
        self.drain_background_output(
            task_id,
            std::time::Duration::from_secs(wait_secs),
            thread_id,
        )
        .await
    }

    async fn drain_background_output(
        &self,
        task_id: &str,
        wait: std::time::Duration,
        thread_id: uuid::Uuid,
    ) -> ToolOutcome {
        // INVARIANT: A message from the user outranks the rest of the wait: the loop
        // only picks injections up BETWEEN iterations, so without this the
        // follow-up sits unread for up to two minutes while we block.
        // Cancel is already handled a level up, in `run_tool_with_cancel`.
        let wakeup = (!wait.is_zero())
            .then(|| self.injection_wakeup(thread_id))
            .flatten();

        let call_started = std::time::Instant::now();
        let snapshot = match wakeup {
            Some((notify, pending)) => {
                // INVARIANT: Register the waiter BEFORE reading the counter. `enable()`
                // is what makes this race-free: any inject() from here on
                // wakes a registered waiter, and any that already happened is
                // visible in `pending`. Checking first and registering second
                // would drop an injection landing in between — and
                // notify_waiters leaves no permit to recover it.
                let notified = notify.notified();
                tokio::pin!(notified);
                notified.as_mut().enable();

                if pending.load(std::sync::atomic::Ordering::Acquire) > 0 {
                    self.bash_background
                        .read_output_in_memory_wait(task_id, std::time::Duration::ZERO)
                        .await
                } else {
                    tokio::select! {
                        biased;
                        snap = self.bash_background.read_output_in_memory_wait(task_id, wait) => snap,
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
            if snap.finished {
                self.bash_background.mark_result_read(thread_id, task_id);
            }
            // INVARIANT: Same `as_reported` rule as the persisted row, so a task the
            // teardown killed does not read as `signal: 9` here and as no
            // status there. The two are one tool call to the LLM.
            let outcome = snap.outcome.map(|o| o.as_reported(snap.abandoned));
            return Ok(serde_json::json!({
                "stdout": finalize_drain(&snap.stdout, snap.stdout_dropped),
                "stderr": finalize_drain(&snap.stderr, snap.stderr_dropped),
                "exit_code": outcome.and_then(|o| o.exit_code()),
                "signal": outcome.and_then(|o| o.signal()),
                "status": outcome.map(|o| o.describe()),
                "finished": snap.finished,
                "timed_out": snap.timed_out,
                "killed": snap.killed,
                "abandoned": snap.abandoned,
                "elapsed_secs": snap.elapsed_secs,
                "waited_secs": call_started.elapsed().as_secs(),
            })
            .to_string());
        }

        match persisted_drain(
            &self.pool,
            &self.bash_background,
            thread_id,
            task_id,
            call_started,
        )
        .await?
        {
            Some(result) => Ok(result),
            None => match bash_background_recovery::task_start_time(&self.pool, thread_id, task_id)
                .await
            {
                Ok(Some(started_at)) => Err(format!(
                    "Error: task '{}' was spawned on this thread at {} and no outcome was ever \
                     recorded for it. The usual cause is an engine stop: a background task is a \
                     child of the engine process. Treat the work as unfinished, and check \
                     whether it is still running before starting it again.",
                    task_id,
                    started_at.to_rfc3339(),
                )),
                Ok(None) => Err(format!("Error: unknown task_id '{}'", task_id)),
                Err(e) => Err(format!(
                    "Error: no output for task '{}', and the event store could not be checked \
                     for whether this thread ever spawned it: {}",
                    task_id, e,
                )),
            },
        }
    }

    async fn execute_bash_kill_tool(
        &self,
        args: &serde_json::Value,
        thread_id: uuid::Uuid,
    ) -> ToolOutcome {
        let task_id = match args.get("task_id").and_then(|v| v.as_str()) {
            Some(t) if !t.is_empty() => t,
            _ => return Err("Error: task_id is required".to_string()),
        };
        match self.stop_background_task_as(thread_id, task_id).await {
            Some(stopped) => Ok(format!(
                "killed {task_id}. Its completion will not re-open this thread.{}",
                crate::engine::agent_session::background_task::ended_waits_note(
                    &stopped.ended_with_others
                )
            )),
            None => Err(format!(
                "Error: unknown or already finished task_id '{task_id}', or a stop is \
                 already on its way"
            )),
        }
    }
}

async fn persisted_drain(
    pool: &sqlx::PgPool,
    registry: &crate::engine::bash_background::BackgroundBashRegistry,
    thread_id: uuid::Uuid,
    task_id: &str,
    call_started: std::time::Instant,
) -> Result<Option<String>, String> {
    let row: Option<(serde_json::Value,)> = sqlx::query_as(
        r#"SELECT payload FROM events
           WHERE event_type = 'BackgroundBashCompleted'
             AND aggregate_id = $1
             AND payload->>'task_id' = $2
           ORDER BY sequence DESC
           LIMIT 1"#,
    )
    .bind(thread_id.to_string())
    .bind(task_id)
    .fetch_optional(pool)
    .await
    .map_err(|e| format!("Error: failed to query completion event: {}", e))?;
    let Some((payload,)) = row else {
        return Ok(None);
    };
    let secrets = crate::engine::engine_impl::stored_secret_values(pool)
        .await
        .map_err(|e| {
            format!("Error: task '{task_id}' output withheld, since it could not be masked: {e}")
        })?;
    let stream = |key: &str| {
        let mut value = payload
            .get(key)
            .cloned()
            .unwrap_or(serde_json::Value::String(String::new()));
        crate::core::redact_subprocess_output_in_json(&mut value, &secrets);
        value
    };

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
    let result = serde_json::json!({
        "stdout": stream("stdout"),
        "stderr": stream("stderr"),
        "exit_code": outcome.exit_code(),
        "signal": outcome.signal(),
        "status": outcome.describe(),
        "finished": true,
        "timed_out": payload.get("timed_out").cloned().unwrap_or(serde_json::Value::Bool(false)),
        "killed": payload.get("killed").cloned().unwrap_or(serde_json::Value::Bool(false)),
        "abandoned": payload.get("abandoned").cloned().unwrap_or(serde_json::Value::Bool(false)),
        "elapsed_secs": persisted_elapsed_secs(&payload),
        "waited_secs": call_started.elapsed().as_secs(),
    })
    .to_string();
    registry.mark_result_read(thread_id, task_id);
    Ok(Some(result))
}

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::bash_background::truncate_output_tail;

    #[tokio::test]
    async fn a_stored_row_holding_a_credential_drains_with_it_masked() {
        let (pool, db) = crate::test_support::setup_test_db().await;
        let secret = "opaque-stored-credential-value";
        crate::test_support::seed_credential(
            &pool,
            "example-api",
            "https://api.example.com",
            crate::core::AuthType::ApiKey,
            secret,
        )
        .await;
        let thread_id = uuid::Uuid::new_v4();
        sqlx::query(
            "INSERT INTO events (id, aggregate, aggregate_id, event_type, payload, created, thread_id) \
             VALUES ($1, 'thread', $2, 'BackgroundBashCompleted', $3, NOW(), $2::uuid)",
        )
        .bind(uuid::Uuid::new_v4())
        .bind(thread_id.to_string())
        .bind(serde_json::json!({
            "task_id": "task-old",
            "command": "env",
            "exit_code": 0,
            "stdout": format!("CRED_EXAMPLE_API={secret}\nbuild ok"),
            "stderr": format!("warning: echoed {secret}"),
            "timed_out": false,
            "killed": false,
        }))
        .execute(&pool)
        .await
        .expect("insert a pre-fix completion row");

        let out = persisted_drain(
            &pool,
            &crate::engine::bash_background::BackgroundBashRegistry::new(),
            thread_id,
            "task-old",
            std::time::Instant::now(),
        )
        .await
        .expect("the drain reads the store")
        .expect("the row is found");

        assert!(!out.contains(secret), "credential replayed: {out}");
        let parsed: serde_json::Value = serde_json::from_str(&out).expect("json result");
        assert!(parsed["stdout"].as_str().unwrap().contains("build ok"));
        assert!(parsed["stderr"]
            .as_str()
            .unwrap()
            .starts_with("warning: echoed"));
        assert_eq!(parsed["exit_code"], 0);

        pool.close().await;
        crate::test_support::teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn a_persisted_read_marks_the_result_read() {
        let (pool, db) = crate::test_support::setup_test_db().await;
        let registry = crate::engine::bash_background::BackgroundBashRegistry::new();
        let thread_id = uuid::Uuid::new_v4();
        let spec = TaskSpec {
            timeout_secs: 60,
            description: None,
            delivery: CompletionDelivery::Poll,
            inherited: crate::core::InheritedSecrets::Keep,
        };
        let (task_id, _rx) = registry
            .spawn_task(
                &Sandbox::unsandboxed(),
                "true",
                std::path::Path::new("/tmp"),
                &[],
                Some(thread_id),
                spec,
            )
            .await
            .expect("spawn");
        assert!(
            registry
                .wait_for_finish(&task_id, Duration::from_secs(10))
                .await
        );
        assert_eq!(registry.unread_results_for_thread(thread_id).len(), 1);
        sqlx::query(
            "INSERT INTO events (id, aggregate, aggregate_id, event_type, payload, created, thread_id) \
             VALUES ($1, 'thread', $2, 'BackgroundBashCompleted', $3, NOW(), $2::uuid)",
        )
        .bind(uuid::Uuid::new_v4())
        .bind(thread_id.to_string())
        .bind(serde_json::json!({ "task_id": task_id, "command": "true", "exit_code": 0 }))
        .execute(&pool)
        .await
        .expect("insert the completion row");

        persisted_drain(
            &pool,
            &registry,
            thread_id,
            &task_id,
            std::time::Instant::now(),
        )
        .await
        .expect("the drain reads the store")
        .expect("the row is found");
        assert!(registry.unread_results_for_thread(thread_id).is_empty());

        pool.close().await;
        crate::test_support::teardown_test_db(&db).await;
    }

    fn spawn_like_the_tool(command: &str) -> tokio::process::Child {
        let mut cmd = command_shell()
            .command(&Sandbox::unsandboxed(), command)
            .expect("unsandboxed command");
        cmd.stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);
        spawn_shell(&mut cmd).expect("spawn")
    }

    async fn engine_for_bash(
        db: &str,
        workspace: &std::path::Path,
    ) -> crate::engine::LucidosEngine {
        crate::engine::LucidosEngine::new(
            workspace.to_path_buf(),
            &crate::test_support::test_db_url(db),
            crate::llm::ProviderBuildContext {
                default_model: "mock".into(),
                model_is_mock: true,
                vertex_project_id: String::new(),
                vertex_location: crate::llm::vertex::location_handle(String::new()),
                vertex_token_cache: None,
                model_registry: Default::default(),
                not_served: Default::default(),
                claude_gateway_access: None,
                chatgpt_plan_access: None,
            },
            Arc::new(crate::engine::agent_recovery::EngineAgentHost),
            Box::new(crate::engine::provider_adapters::ProviderAdapters::default()),
        )
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn run_bash_reports_the_failing_stage_of_a_pipeline() {
        let (pool, db) = crate::test_support::setup_test_db().await;
        let workspace = tempfile::tempdir().unwrap();
        let engine = engine_for_bash(&db, workspace.path()).await;
        let out = engine
            .execute_bash_tool(
                &serde_json::json!({ "command": "false | cat" }),
                uuid::Uuid::new_v4(),
                &Sandbox::unsandboxed(),
            )
            .await
            .expect("a failing command still returns its result");
        assert!(out.contains("exit code 1"), "{out}");

        pool.close().await;
        crate::test_support::teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn run_bash_says_a_detached_launch_worked_and_never_calls_it_a_timeout() {
        let (pool, db) = crate::test_support::setup_test_db().await;
        let workspace = tempfile::tempdir().unwrap();
        let engine = engine_for_bash(&db, workspace.path()).await;
        let out = engine
            .execute_bash_tool(
                &serde_json::json!({
                    "command": "echo launched && sleep 5 >/dev/null 2>&1 & echo queued"
                }),
                uuid::Uuid::new_v4(),
                &Sandbox::unsandboxed(),
            )
            .await
            .expect("a detached launch is not a failure");
        assert!(out.contains("queued"), "{out}");
        assert!(out.contains("the shell exited (exit code 0)"), "{out}");
        assert!(!out.contains("timed out"), "{out}");

        pool.close().await;
        crate::test_support::teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn bash_output_rejects_a_wait_that_is_not_an_integer() {
        let (pool, db) = crate::test_support::setup_test_db().await;
        let workspace = tempfile::tempdir().unwrap();
        let engine = engine_for_bash(&db, workspace.path()).await;
        let thread_id = uuid::Uuid::new_v4();
        for bad in [
            serde_json::json!("5"),
            serde_json::json!(1.5),
            serde_json::json!(-1),
        ] {
            let err = engine
                .execute_bash_output_tool(
                    &serde_json::json!({ "task_id": "task-x", "wait_secs": bad }),
                    thread_id,
                )
                .await
                .expect_err("a coerced wait would read as an immediate re-poll");
            assert!(
                err.contains("wait_secs must be a non-negative integer"),
                "{err}"
            );
        }
        let err = engine
            .execute_bash_output_tool(&serde_json::json!({ "task_id": "task-x" }), thread_id)
            .await
            .expect_err("no such task");
        assert!(err.contains("unknown task_id"), "{err}");

        pool.close().await;
        crate::test_support::teardown_test_db(&db).await;
    }

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
                ..
            }
            | ShellWait::Detached {
                outcome,
                stdout,
                stderr,
                ..
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

    #[tokio::test]
    async fn a_detached_subshell_holding_the_pipes_does_not_look_like_a_timeout() {
        let started = std::time::Instant::now();
        let child = spawn_like_the_tool("echo launched && sleep 30 >/dev/null 2>&1 & echo queued");
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

    #[tokio::test]
    async fn a_detached_process_is_not_killed_by_the_pipes_closing() {
        let tmp = tempfile::tempdir().unwrap();
        let go = tmp.path().join("go");
        let marker = tmp.path().join("marker");

        let child = spawn_like_the_tool(&format!(
            "echo queued && until [ -e '{go}' ] || [ ! -d '{dir}' ]; do sleep 0.05; done \
             && [ -e '{go}' ] && echo late && sleep 0.3 && echo later && touch '{marker}' & echo done",
            dir = tmp.path().display(),
            go = go.display(),
            marker = marker.display(),
        ));
        let waited = wait_for_shell(child, Duration::from_secs(30))
            .await
            .unwrap();
        assert!(
            matches!(waited, ShellWait::Detached { .. }),
            "expected Detached, got {waited:?}"
        );

        std::fs::write(&go, "").unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(30);
        while !marker.exists() && std::time::Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert!(
            marker.exists(),
            "the detached chain died, most likely on SIGPIPE from a closed read end"
        );
    }

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
                ..
            }
            | ShellWait::Detached {
                outcome,
                stdout,
                stderr,
                ..
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

    #[tokio::test]
    async fn a_timeout_reports_a_timeout_and_kills_the_shell() {
        let tmp = tempfile::tempdir().unwrap();
        let marker = tmp.path().join("marker");

        let child = spawn_like_the_tool(&format!("sleep 1; touch '{}'", marker.display()));
        let waited = wait_for_shell(child, Duration::from_millis(200))
            .await
            .unwrap();
        assert!(
            matches!(waited, ShellWait::TimedOut),
            "expected TimedOut, got {waited:?}"
        );

        tokio::time::sleep(Duration::from_millis(1_500)).await;
        assert!(
            !marker.exists(),
            "the shell survived the timeout and kept working"
        );
    }

    #[tokio::test]
    async fn a_timeout_kills_the_shells_background_children() {
        let tmp = tempfile::tempdir().unwrap();
        let marker = tmp.path().join("marker");

        let child = spawn_like_the_tool(&format!(
            "(sleep 1; touch '{}') >/dev/null 2>&1 & sleep 30",
            marker.display()
        ));
        let waited = wait_for_shell(child, Duration::from_millis(200))
            .await
            .unwrap();
        assert!(
            matches!(waited, ShellWait::TimedOut),
            "expected TimedOut, got {waited:?}"
        );

        tokio::time::sleep(Duration::from_millis(1_500)).await;
        assert!(
            !marker.exists(),
            "the background child survived the timeout"
        );
    }

    #[tokio::test]
    async fn a_cancel_kills_the_shells_background_children() {
        let tmp = tempfile::tempdir().unwrap();
        let marker = tmp.path().join("marker");

        let child = spawn_like_the_tool(&format!(
            "(sleep 1; touch '{}') >/dev/null 2>&1 & sleep 30",
            marker.display()
        ));
        let cancelled = tokio::time::timeout(
            Duration::from_millis(200),
            wait_for_shell(child, Duration::from_secs(30)),
        )
        .await;
        assert!(
            cancelled.is_err(),
            "the wait must still be running at the cancel"
        );

        tokio::time::sleep(Duration::from_millis(1_500)).await;
        assert!(!marker.exists(), "the background child survived the cancel");
    }

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

    #[tokio::test]
    async fn a_chatty_command_keeps_a_bounded_head_and_reports_the_full_size() {
        let child = spawn_like_the_tool("head -c 2000000 /dev/zero | tr '\\0' a");
        let (stdout, stdout_total) = match wait_for_shell(child, Duration::from_secs(30))
            .await
            .unwrap()
        {
            ShellWait::Completed {
                stdout,
                stdout_total,
                ..
            }
            | ShellWait::Detached {
                stdout,
                stdout_total,
                ..
            } => (stdout, stdout_total),
            other => panic!("expected the shell to exit, got {other:?}"),
        };
        assert_eq!(stdout.len(), KEPT_BYTES_CAP);
        assert_eq!(stdout_total, 2_000_000);
        let shown = finalize_stream(&stdout, stdout_total);
        assert!(
            shown.ends_with("[truncated: 2000000 bytes total]"),
            "{}",
            &shown[shown.len().saturating_sub(80)..]
        );
    }

    #[test]
    fn truncate_output_tail_keeps_the_end_not_the_start() {
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
        let s = "ééééé";
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
        assert_eq!(persisted_elapsed_secs(&serde_json::json!({})), None);
        assert_eq!(
            persisted_elapsed_secs(&serde_json::json!({"started_at": "not a date"})),
            None
        );
    }

    mod guard_tests {
        fn production_src() -> String {
            crate::test_support::source_scan::read_production_source(
                &crate::test_support::source_scan::engine_source("engine/tools/bash.rs"),
            )
        }

        #[test]
        fn the_bash_tool_result_leaves_through_the_secret_redaction() {
            let src = production_src();
            let at = src
                .find("async fn execute_bash_tool")
                .expect("execute_bash_tool is still here");
            let end = src[at..]
                .find("async fn execute_bash_background_tool(")
                .expect("execute_bash_tool is still followed by the background tool");
            let body = &src[at..at + end];
            assert!(
                body.contains("redact_subprocess_output(")
                    && body.contains("subprocess_secret_values(env_vars,"),
                "execute_bash_tool must return through `core::redact_subprocess_output` \
             over `core::subprocess_secret_values`. A verbose curl or an `env` dump \
             otherwise writes a credential into the events table."
            );
            assert!(
                body.contains("InheritedSecrets::Strip("),
                "a chat shell must not inherit the engine's secrets"
            );
        }

        #[test]
        fn a_failed_completion_emit_hands_the_claim_back() {
            let src = production_src();
            let start = src
                .find("fn spawn_bash_completion_watcher(")
                .expect("the watcher is still here");
            let body = &src[start..];
            let body = &body[..body.find("\n    }\n").expect("end of the watcher")];
            let failed = body
                .find("failed to emit BackgroundBashCompleted")
                .expect("the watcher logs a failed emit");
            assert!(
                body[failed..].contains("release_completion_claim(&task_id)"),
                "a failed emit must release the claim:\n{body}"
            );
        }

        #[test]
        fn the_completion_watcher_only_emits_and_never_pushes_into_a_session() {
            let src = production_src();
            let start = src
                .find("fn spawn_bash_completion_watcher(")
                .expect("the watcher is still here");
            let body = &src[start..];
            let body = &body[..body.find("\n    }\n").expect("end of the watcher")];
            assert!(
                !body.contains("msg_tx") && !body.contains("agent_sessions"),
                "the watcher must leave delivery to the event wait:\n{body}"
            );
        }
    }
}
