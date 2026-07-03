//! Conversation lookup, script env/exec, dirty-commit helper.
//!
//! Part of the `LucidosEngine` inherent impl, split from engine_impl.rs.

use super::super::*;

/// Wall-clock budget for a scheduled `.sh` task before its child is killed.
const SHELL_SCRIPT_TIMEOUT_SECS: u64 = 300;

impl LucidosEngine {
    /// Record a trigger completion.
    /// LLM triggers have a real thread_id and go through EventBus as a thread event.
    /// Script triggers have no thread — they use a system event.
    pub async fn record_trigger_completed(
        &self,
        trigger_id: &str,
        trigger_name: &str,
        result_summary: &str,
        thread_id: Option<Uuid>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        // Engine-level guarantee: a trigger run's summary is never empty. The
        // script/intent call sites supply a kind-specific fallback, but this is
        // the single choke point both run kinds (and any future caller) pass
        // through, so the guarantee holds regardless — a blank summary never
        // reads as a no-op fire to the learning/audit sweeps. See
        // `crate::triggers::summary`.
        let result_summary =
            crate::triggers::ensure_non_empty_summary(result_summary, trigger_name);
        if let Some(tid) = thread_id {
            self.event_bus
                .emit(event_bus::BusEvent::Thread {
                    thread_id: tid,
                    event: thread_events::ThreadEvent::TriggerCompleted {
                        trigger_id: trigger_id.to_string(),
                        trigger_name: Some(trigger_name.to_string()),
                        result_summary: Some(result_summary),
                    },
                    meta: thread_events::EventMeta {
                        channel: Some(crate::engine::thread_events::EventChannel::Trigger),
                        ..thread_events::EventMeta::NONE
                    },
                })
                .await?;
        } else {
            self.event_bus
                .emit(crate::engine::event_bus::BusEvent::System(
                    crate::engine::event_bus::SystemEvent::TriggerCompleted {
                        trigger_id: trigger_id.to_string(),
                        trigger_name: trigger_name.to_string(),
                        result_summary,
                    },
                ))
                .await?;
        }
        Ok(())
    }

    /// Get a snapshot of the conversation at a specific event (thin wrapper)
    pub async fn get_conversation_at_event(
        &self,
        event_id: Uuid,
    ) -> Result<ConversationSnapshot, Box<dyn std::error::Error + Send + Sync>> {
        self.event_store
            .get_conversation_at_event(event_id, &self.workspace_path)
            .await
    }

    /// Build CRED_*, OAUTH_*, LUCIDOS_WORKSPACE, and PATH environment variables
    /// for script execution. Used by `execute_python_tool` (LLM),
    /// `execute_bash_tool`, and `execute_script` (scheduled tasks).
    ///
    /// `LUCIDOS_WORKSPACE` + the `.lucidos/bin` symlink let scripts call
    /// `lucidos data write` / `lucidos events emit` / `lucidos events query`
    /// instead of hand-rolling HTTP requests back to the engine.
    pub(crate) async fn build_script_env_vars(
        &self,
        thread_id: Option<Uuid>,
    ) -> Vec<(String, String)> {
        use crate::core::oauth;
        use crate::core::{CredentialStore, EnvironmentVariableStore, OAuthStore};
        use crate::runtime::lucidos_cli::{lucidos_cli_dir, workspace_script_env_vars};

        // User-managed environment variables FIRST. Everything engine-owned
        // below (workspace/PATH, PG*, origin token, host-protection, CRED_*,
        // OAUTH_*) is appended after, and the spawn applies the pairs in order
        // via `cmd.env`, so a user var can never override an engine-owned one
        // (`env_pairs` also drops reserved names as a second backstop).
        let mut env_vars: Vec<(String, String)> = Vec::new();
        match EnvironmentVariableStore::env_pairs(&self.pool).await {
            Ok(pairs) => env_vars.extend(pairs),
            Err(e) => log!(
                "[Python] Failed to load user environment variables for env injection: {}",
                e
            ),
        }

        // Prepend the bundled PG client dir (packaged: <resources>/postgres/bin)
        // to the script PATH so the advertised bare `psql -c '…'` resolves; it's
        // not on the launchd minimal PATH. Unset in dev (docker PG on PATH).
        let pg_bin_dir = std::env::var_os("LUCIDOS_PG_BIN_DIR").map(std::path::PathBuf::from);
        env_vars.extend(workspace_script_env_vars(
            self.workspace_path(),
            lucidos_cli_dir(),
            pg_bin_dir.as_deref(),
        ));

        // PG* env so spawned scripts can run `psql -c '…'` bare. Keeps the
        // password out of argv (which we capture into events) — see
        // `core::pg_env_vars` doc for the full rationale.
        env_vars.extend(crate::core::pg_env_vars_cached().iter().cloned());

        // Subprocess-origin env (token + optional source thread id). Single
        // source of truth via `api::actor::subprocess_origin_env_vars` so a
        // future subprocess surface (MCP child, signer host, …) cannot ship
        // without origin attribution — the failure mode that grew the
        // original incident.
        env_vars.extend(
            crate::api::actor::subprocess_origin_env_vars(thread_id)
                .into_iter()
                .map(|(k, v)| (k.to_string(), v)),
        );

        // Host-process kill-guard env (LUCIDOS_HOST_PID + optional
        // FRONTEND_PID + optional API_PORT). Same single-source-of-truth
        // rationale as subprocess_origin_env_vars above — without this, a
        // python/bash tool that shells out to `web-dev.sh` or `ports.sh`
        // could free up the engine's port by killing the engine itself.
        env_vars.extend(
            crate::api::actor::host_protection_env_vars(self.workspace_path())
                .into_iter()
                .map(|(k, v)| (k.to_string(), v)),
        );

        // Credentials → CRED_* vars
        match CredentialStore::list_all_with_secrets(&self.pool).await {
            Ok(creds) => env_vars.extend(crate::core::credentials::credential_env_vars(creds)),
            Err(e) => log!(
                "[Python] Failed to load credentials for env injection: {}",
                e
            ),
        }

        // OAuth accounts → OAUTH_* vars (auto-refreshed)
        match OAuthStore::list_all_with_tokens(&self.pool).await {
            Ok(mut accounts) => {
                for account in &mut accounts {
                    if let Err(e) = oauth::refresh_oauth_if_needed(&self.pool, account).await {
                        log!(
                            "[Python] OAuth refresh failed for {}: {}",
                            account.provider,
                            e
                        );
                    }
                }
                env_vars.extend(oauth::account_env_vars(accounts));
            }
            Err(e) => log!(
                "[Python] Failed to load OAuth accounts for env injection: {}",
                e
            ),
        }

        env_vars
    }

    /// Execute a script file by workspace-relative path. Used by scheduled script tasks.
    ///
    /// Runtime is determined by file extension:
    /// - `.py` → Python (per-workspace venv)
    /// - `.sh` → Bash (`/bin/sh`)
    pub async fn execute_script(
        &self,
        script_path: &str,
        args: &[String],
        extra_env: &[(String, String)],
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        if crate::core::is_path_traversal(script_path) {
            return Err("Invalid script path: must be relative, no '..'".into());
        }

        let full_path = self.workspace_path.join(script_path);
        if !full_path.exists() {
            return Err(format!("Script not found: {}", script_path).into());
        }

        let extension = std::path::Path::new(script_path)
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("");

        // Script args + shared credentials/OAuth env vars (injected into ALL runtimes)
        let mut env_vars = vec![
            ("LUCIDOS_SCRIPT_PATH".to_string(), script_path.to_string()),
            (
                "LUCIDOS_ARGS".to_string(),
                serde_json::to_string(args).unwrap_or_default(),
            ),
        ];
        for (i, arg) in args.iter().enumerate() {
            env_vars.push((format!("LUCIDOS_ARG_{}", i), arg.clone()));
        }
        // Scheduled scripts run without a thread context — no source thread
        // id to attribute their HTTP callbacks back to.
        env_vars.extend(self.build_script_env_vars(None).await);
        env_vars.extend(extra_env.iter().cloned());

        let output = match extension {
            "py" => {
                let code = std::fs::read_to_string(&full_path)
                    .map_err(|e| format!("Failed to read script {}: {}", script_path, e))?;
                self.python_runtime
                    .execute_with_env(&code, env_vars)
                    .await
                    .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> { e.into() })?
            }
            "sh" => self.execute_shell_script(&full_path, env_vars).await?,
            _ => {
                let msg = match crate::triggers::validate_script_extension(script_path) {
                    Err(e) => e,
                    Ok(()) => format!("No runtime configured for '.{}' scripts", extension),
                };
                return Err(msg.into());
            }
        };

        // Auto-commit any files the script touched under artifacts/
        self.commit_dirty_logged("Script task output", script_path)
            .await;

        Ok(output)
    }

    async fn execute_shell_script(
        &self,
        script_path: &std::path::Path,
        env_vars: Vec<(String, String)>,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        use crate::core::sanitize_for_jsonb;

        let output = run_shell_script_with_timeout(
            script_path,
            self.workspace_path(),
            &env_vars,
            std::time::Duration::from_secs(SHELL_SCRIPT_TIMEOUT_SECS),
        )
        .await?;

        let stdout = sanitize_for_jsonb(&String::from_utf8_lossy(&output.stdout));
        let stderr = sanitize_for_jsonb(&String::from_utf8_lossy(&output.stderr));

        if output.status.success() {
            Ok(stdout)
        } else {
            Err(format!(
                "Shell script error (exit {}):\n{}",
                output.status.code().unwrap_or(-1),
                stderr
            )
            .into())
        }
    }

    /// Commit all dirty data/ files with a 30s timeout, logging success/failure.
    /// Shared by all code paths that may produce dirty files (scripts, Claude Code, run_python).
    pub(crate) async fn commit_dirty_logged(&self, message: &str, context: &str) {
        match tokio::time::timeout(
            std::time::Duration::from_secs(30),
            self.artifact_manager.commit_all_dirty(message),
        )
        .await
        {
            Ok(Ok(Some(commit))) => {
                log!(
                    "[Engine] Auto-committed dirty data files after {} ({})",
                    context,
                    &commit[..commit.floor_char_boundary(7)]
                );
            }
            Ok(Err(e)) => {
                log!(
                    "[Engine] Failed to commit dirty data files after {}: {}",
                    context,
                    e
                );
            }
            Err(_) => {
                log!(
                    "[Engine] commit_all_dirty timed out (30s) after {}",
                    context
                );
            }
            Ok(Ok(None)) => {}
        }
    }
}

/// Spawn `/bin/sh <script>` in `workspace_dir` with `env_vars`, waiting up to
/// `timeout` for it to finish. `kill_on_drop(true)` is the load-bearing part:
/// on timeout the `wait_with_output` future is dropped, taking the owned child
/// with it, and the OS sends SIGKILL — so a hung scheduled script can't leak an
/// orphaned process. Mirrors `execute_bash_tool` (tools/bash.rs).
async fn run_shell_script_with_timeout(
    script_path: &std::path::Path,
    workspace_dir: &std::path::Path,
    env_vars: &[(String, String)],
    timeout: std::time::Duration,
) -> Result<std::process::Output, Box<dyn std::error::Error + Send + Sync>> {
    let mut cmd = tokio::process::Command::new("/bin/sh");
    cmd.arg(script_path)
        .current_dir(workspace_dir)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);

    for (key, value) in env_vars {
        cmd.env(key, value);
    }

    let child = cmd
        .spawn()
        .map_err(|e| format!("Failed to spawn shell script: {}", e))?;

    match tokio::time::timeout(timeout, child.wait_with_output()).await {
        Ok(Ok(output)) => Ok(output),
        Ok(Err(e)) => Err(format!("Error executing shell script: {}", e).into()),
        Err(_) => Err(format!("Shell script timed out after {}s", timeout.as_secs()).into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn shell_script_timeout_kills_child() {
        // A script that sleeps then writes a sentinel. With kill_on_drop the
        // timeout SIGKILLs the shell mid-sleep, so the sentinel is never
        // written. Without it, the orphaned child would finish the sleep and
        // touch the sentinel after the future was dropped.
        let dir = std::env::temp_dir().join("lucidos_test_shell_kill_on_drop");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let sentinel = dir.join("sentinel");
        let script = dir.join("slow.sh");
        std::fs::write(&script, format!("sleep 3\ntouch '{}'\n", sentinel.display())).unwrap();

        let err = run_shell_script_with_timeout(
            &script,
            &dir,
            &[],
            std::time::Duration::from_millis(300),
        )
        .await
        .expect_err("script should time out");
        assert!(err.to_string().contains("timed out"), "got: {}", err);

        // Wait past the script's sleep; a leaked child would have touched the
        // sentinel by now.
        tokio::time::sleep(std::time::Duration::from_secs(4)).await;
        assert!(
            !sentinel.exists(),
            "sentinel was created — the timed-out shell child leaked (kill_on_drop missing)"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
}
