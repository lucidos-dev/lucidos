use super::super::LucidosEngine;
use super::ToolOutcome;
use crate::core::{redact_postgres_secrets, sanitize_for_jsonb};
use crate::engine::event_bus::BusEvent;
use crate::engine::thread_events::{EventMeta, ThreadEvent};

const MAX_OUTPUT_BYTES: usize = 100 * 1024; // 100 KB
pub(crate) const DEFAULT_TIMEOUT_SECS: u64 = 60;
pub(crate) const MAX_TIMEOUT_SECS: u64 = 300;

/// Background bash defaults. Higher than the synchronous tool because the
/// caller can poll across many turns; the LLM is still bounded by
/// `BG_MAX_TIMEOUT_SECS` to prevent runaway processes.
pub(crate) const BG_DEFAULT_TIMEOUT_SECS: u64 = 600;
pub(crate) const BG_MAX_TIMEOUT_SECS: u64 = 3600;

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

        let env_vars = self.build_script_env_vars().await;

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

        let env_vars = self.build_script_env_vars().await;
        let safe_command = redact_postgres_secrets(command);
        log!(
            "[BashBg] Spawning: {}",
            &safe_command[..safe_command.floor_char_boundary(200)]
        );

        let (task_id, finish_rx) = match self
            .bash_background
            .spawn(command, timeout_secs, self.workspace_path(), &env_vars)
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
    /// and emits `BackgroundBashCompleted` with the final state.
    fn spawn_bash_completion_watcher(
        &self,
        thread_id: uuid::Uuid,
        task_id: String,
        safe_command: String,
        finish_rx: tokio::sync::oneshot::Receiver<()>,
    ) {
        let bus = self.event_bus.clone();
        let registry = self.bash_background.clone();
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
            let event = ThreadEvent::BackgroundBashCompleted {
                task_id: task_id.clone(),
                command: cmd_prefix,
                exit_code: task.exit_code,
                stdout: finalize_stream(&task.stdout),
                stderr: finalize_stream(&task.stderr),
                started_at: task.started_at,
                finished_at: task.finished_at.unwrap_or_else(chrono::Utc::now),
                timed_out: task.timed_out,
                killed: task.killed,
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
        });
    }

    /// `bash_output(task_id)` — drain in-memory output if the task is
    /// still running, else fall back to the persisted
    /// `BackgroundBashCompleted` event. Returns a JSON string.
    pub(crate) async fn execute_bash_output_tool(
        &self,
        args: &serde_json::Value,
        thread_id: uuid::Uuid,
    ) -> ToolOutcome {
        let task_id = match args.get("task_id").and_then(|v| v.as_str()) {
            Some(t) if !t.is_empty() => t,
            _ => return Err("Error: task_id is required".to_string()),
        };

        if let Some(snap) = self.bash_background.read_output_in_memory(task_id).await {
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
}
