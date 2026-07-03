use super::super::LucidosEngine;
use super::ToolOutcome;
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

        let mut cmd = tokio::process::Command::new("/bin/sh");
        cmd.args(["-c", command])
            .current_dir(self.workspace_path())
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
        let exit_code = output.status.code().unwrap_or(-1);

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

        if !output.status.success() {
            if !response.is_empty() {
                response.push_str("\n\n");
            }
            response.push_str(&format!("[exit code: {}]", exit_code));
        }

        if response.is_empty() {
            response = format!("[exit code: {}]", exit_code);
        }

        Ok(response)
    }

    /// `run_bash_background(command, timeout_secs?)` — spawn a long-running
    /// shell command and return a `task_id` immediately. Emits
    /// `BackgroundBashStarted`; the watcher emits `BackgroundBashCompleted`
    /// and evicts the entry when the child exits or the watchdog kills it.
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

    /// Awaits the registry watchdog's notify, evicts the finished task,
    /// emits `BackgroundBashCompleted` with the final state, and — if the
    /// owning CC session is still parked on this thread — pushes a synthetic
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
            let Some(task) = registry.take_finished(&task_id).await else {
                log!("[BashBg] watcher fired but task {} already gone", task_id);
                return;
            };
            let cmd_prefix = {
                let s = safe_command.as_str();
                s[..s.floor_char_boundary(200)].to_string()
            };
            let exit_code = task.exit_code;
            let timed_out = task.timed_out;
            let killed = task.killed;
            let event = ThreadEvent::BackgroundBashCompleted {
                task_id: task_id.clone(),
                command: cmd_prefix.clone(),
                exit_code,
                stdout: finalize_stream(&task.stdout),
                stderr: finalize_stream(&task.stderr),
                started_at: task.started_at,
                finished_at: task.finished_at.unwrap_or_else(chrono::Utc::now),
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
                    format_bash_wake_text(&task_id, &cmd_prefix, exit_code, killed, timed_out);
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
    /// blocks server-side until new output arrives, the task finishes,
    /// or the wait elapses. Replaces the sleep-poll antipattern where
    /// a chat agent spawned `run_python_background` then issued a
    /// fresh `run_python` containing `time.sleep(N)` — that burned two
    /// tool calls per wait, doubled context, and stalled the turn.
    /// Values above the max are clamped silently so a misprompted
    /// agent can't pin a model turn forever.
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

        if let Some(snap) = self
            .bash_background
            .read_output_in_memory_wait(task_id, wait)
            .await
        {
            return Ok(serde_json::json!({
                "stdout": finalize_stream(snap.stdout.as_bytes()),
                "stderr": finalize_stream(snap.stderr.as_bytes()),
                "exit_code": snap.exit_code,
                "finished": snap.finished,
                "timed_out": snap.timed_out,
                "killed": snap.killed,
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
            Some((payload,)) => Ok(serde_json::json!({
                "stdout": payload.get("stdout").cloned().unwrap_or(serde_json::Value::String(String::new())),
                "stderr": payload.get("stderr").cloned().unwrap_or(serde_json::Value::String(String::new())),
                "exit_code": payload.get("exit_code").cloned().unwrap_or(serde_json::Value::Null),
                "finished": true,
                "timed_out": payload.get("timed_out").cloned().unwrap_or(serde_json::Value::Bool(false)),
                "killed": payload.get("killed").cloned().unwrap_or(serde_json::Value::Bool(false)),
            })
            .to_string()),
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

/// Text the engine pushes to CC via `msg_tx` when a background task
/// (spawned via `run_bash_background` OR `run_python_background`) completes
/// and CC is parked waiting on it. Extracted as a free function so the
/// formatting can be regression-pinned without spinning up the full watcher
/// (which depends on `agent_sessions`, the event bus, and a real
/// `BackgroundBashRegistry`).
///
/// The text gives CC enough context to act without re-querying — the
/// task_id, the command prefix it spawned, and an outcome word — but
/// CC still calls `bash_output(task_id)` to read the actual stdout/
/// stderr. Killed and timed-out cases get their own outcome word so
/// CC knows the result isn't "exit code 0 — clean success".
///
/// "Background task" instead of "Background bash task" — the same watcher
/// fires for python-spawned tasks (via `run_python_background`), and the
/// command-prefix line tells the LLM which it was. `bash_output` /
/// `bash_kill` are correct because they ARE the consumer tools for both.
fn format_bash_wake_text(
    task_id: &str,
    cmd_prefix: &str,
    exit_code: Option<i32>,
    killed: bool,
    timed_out: bool,
) -> String {
    let status = match (exit_code, killed, timed_out) {
        (_, true, _) => "killed".to_string(),
        (_, _, true) => "timed out".to_string(),
        (Some(c), _, _) => format!("exit code {}", c),
        (None, _, _) => "no exit code".to_string(),
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
        let text = format_bash_wake_text("task-123", "cargo test --lib", Some(0), false, false);
        assert!(text.contains("task-123"), "task_id must appear so CC can refer to it");
        assert!(text.contains("exit code 0"), "clean exit must say 'exit code 0'");
        assert!(text.contains("cargo test --lib"), "command prefix gives CC context");
        assert!(
            text.contains("bash_output(\"task-123\")"),
            "must instruct CC to read the result via bash_output"
        );
    }

    #[test]
    fn format_bash_wake_text_non_zero_exit() {
        let text = format_bash_wake_text("t1", "npm test", Some(1), false, false);
        assert!(text.contains("exit code 1"));
    }

    /// `killed` outranks an exit_code value: a child reaped by SIGKILL/
    /// SIGTERM may still report an exit code, but the CC-facing semantic
    /// is "the engine killed this" — not "the command finished with X".
    #[test]
    fn format_bash_wake_text_killed_outranks_exit_code() {
        let text = format_bash_wake_text("t2", "sleep 30", Some(143), true, false);
        assert!(text.contains("killed"), "killed must be the outcome word");
        assert!(
            !text.contains("exit code"),
            "killed must NOT also report the exit code (would confuse CC)"
        );
    }

    /// Timeout is the second special outcome. Same precedence reason as
    /// killed — CC needs to know the result wasn't a real completion.
    #[test]
    fn format_bash_wake_text_timed_out() {
        let text = format_bash_wake_text("t3", "sleep 99999", None, false, true);
        assert!(text.contains("timed out"));
        assert!(!text.contains("exit code"));
    }

    #[test]
    fn format_bash_wake_text_no_exit_code() {
        let text = format_bash_wake_text("t4", "true", None, false, false);
        assert!(text.contains("no exit code"));
    }
}
