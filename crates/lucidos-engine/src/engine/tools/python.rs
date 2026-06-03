use super::super::LucidosEngine;
use super::ToolOutcome;
use crate::core::redact_postgres_secrets;
use crate::engine::event_bus::{BusEvent, SystemEvent};
use crate::engine::thread_events::{EventMeta, ThreadEvent};
use crate::llm::tools::{BG_DEFAULT_TIMEOUT_SECS, BG_MAX_TIMEOUT_SECS};

impl LucidosEngine {
    pub(crate) async fn execute_python_tool(
        &self,
        args: &serde_json::Value,
        thread_id: uuid::Uuid,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        let code = args["code"].as_str().unwrap_or("");
        let commit_message = args.get("commit_message").and_then(|v| v.as_str());

        if let Some(packages) = args.get("packages").and_then(|v| v.as_array()) {
            let names: Vec<&str> = packages.iter().filter_map(|p| p.as_str()).collect();
            if !names.is_empty() {
                if let Err(e) = self.python_runtime.ensure_packages(&names).await {
                    return Err(format!("installing packages: {}", e).into());
                }
            }
        }

        let env_vars = self.build_script_env_vars(Some(thread_id)).await;
        let run_id = uuid::Uuid::new_v4().to_string();
        let staging_dir = self.workspace_path().join(".lucidos/staging").join(&run_id);

        let output = match self
            .python_runtime
            .execute_staged(code, env_vars, &staging_dir)
            .await
        {
            Ok(output) => output,
            Err(e) => {
                std::fs::remove_dir_all(&staging_dir).ok();
                return Err(e.to_string().into());
            }
        };

        let data_staging = staging_dir.join("data");
        let mut created = Vec::new();
        let mut updated = Vec::new();

        if data_staging.exists() {
            let data_dir = self.workspace_path().join("data");
            Self::collect_staged_files(
                &data_staging,
                &data_staging,
                &data_dir,
                &mut created,
                &mut updated,
            )?;

            if !created.is_empty() || !updated.is_empty() {
                for path in created.iter().chain(updated.iter()) {
                    let src = data_staging.join(path);
                    let dst = data_dir.join(path);
                    if let Some(parent) = dst.parent() {
                        std::fs::create_dir_all(parent)?;
                    }
                    std::fs::copy(&src, &dst)?;
                }

                let all_paths: Vec<String> =
                    created.iter().chain(updated.iter()).cloned().collect();
                let message = commit_message.unwrap_or("Python script output");
                let commit_sha = self
                    .artifact_manager
                    .commit_data_paths(&all_paths, message)
                    .await
                    .map_err(|e| format!("Git commit failed: {}", e))?;

                for path in &created {
                    self.event_bus
                        .emit(BusEvent::System(SystemEvent::ArtifactCreated {
                            artifact_path: path.clone(),
                            commit: commit_sha.clone(),
                            source: Some("run_python".to_string()),
                        }))
                        .await?;
                }
                for path in &updated {
                    self.event_bus
                        .emit(BusEvent::System(SystemEvent::ArtifactUpdated {
                            artifact_path: path.clone(),
                            commit: commit_sha.clone(),
                            source: Some("run_python".to_string()),
                        }))
                        .await?;
                }
            }
        }

        std::fs::remove_dir_all(&staging_dir).ok();

        let mut response = output;
        if !created.is_empty() || !updated.is_empty() {
            response.push_str("\n\n[FILES]");
            for path in &created {
                response.push_str(&format!("\n  created: data/{}", path));
            }
            for path in &updated {
                response.push_str(&format!("\n  updated: data/{}", path));
            }
        }

        Ok(response)
    }

    /// `run_python_background(code, packages?, timeout_secs?)` — install
    /// packages into the per-workspace venv, write the script to
    /// `.lucidos/exhaust/<run_id>/script.py`, then hand a venv-rooted
    /// `python <script>` invocation off to `BackgroundBashRegistry::spawn`.
    /// Returns `task_id` immediately; the caller drains incremental output
    /// through `bash_output(task_id)` and cancels via `bash_kill(task_id)` —
    /// the same surface `run_bash_background` uses, so the LLM-facing
    /// drain/cancel contract is identical.
    ///
    /// Design choices vs. the synchronous `run_python`:
    /// - **No staging / no auto-commit.** Long-running tasks need apps to
    ///   see partial output as it lands in `data/`. The watcher would also
    ///   have to do git work asynchronously, which is a foot-gun if a
    ///   multi-hour backtest produces 100 MB of CSVs. Callers commit
    ///   explicitly via `run_bash_background` if they want git history.
    /// - **Reuses `BackgroundBashStarted`/`BackgroundBashCompleted` events.**
    ///   The spawn really is `/bin/sh -c "python <script>"`, so the bash
    ///   audit-trail rows are accurate; the `command` field captures the
    ///   exact python invocation. Adding parallel `BackgroundPython*`
    ///   events would duplicate the registry, the watcher, and the
    ///   `bash_output` fallback path for the same semantics.
    pub(crate) async fn execute_python_background_tool(
        &self,
        args: &serde_json::Value,
        thread_id: uuid::Uuid,
    ) -> ToolOutcome {
        let code = match args.get("code").and_then(|v| v.as_str()) {
            Some(c) if !c.is_empty() => c,
            _ => return Err("Error: code is required".to_string()),
        };

        let timeout_secs = args
            .get("timeout_secs")
            .and_then(|v| v.as_u64())
            .unwrap_or(BG_DEFAULT_TIMEOUT_SECS)
            .min(BG_MAX_TIMEOUT_SECS);

        // Ensure venv exists. `ensure_packages` would do this transitively
        // when `packages` is non-empty, but a script with zero declared
        // packages (e.g. stdlib-only or relying on already-installed deps)
        // still needs the venv python binary to exist before we hand the
        // invocation to bash spawn.
        if let Err(e) = self.python_runtime.ensure_venv().await {
            return Err(format!("Error: failed to prepare python venv: {}", e));
        }

        if let Some(packages) = args.get("packages").and_then(|v| v.as_array()) {
            let names: Vec<&str> = packages.iter().filter_map(|p| p.as_str()).collect();
            if !names.is_empty() {
                if let Err(e) = self.python_runtime.ensure_packages(&names).await {
                    return Err(format!("Error: installing packages: {}", e));
                }
            }
        }

        // Write the script to a per-run directory under `.lucidos/exhaust/`
        // so it stays around for audit and the `command` field of the
        // emitted `BackgroundBashStarted` event references a real path.
        // We use a fresh UUID for the script directory — the user-visible
        // `task_id` is whatever `BackgroundBashRegistry::spawn` returns,
        // which is generated inside spawn and unavailable until after the
        // script file is written.
        let run_id = uuid::Uuid::new_v4().to_string();
        let script_dir = self
            .workspace_path()
            .join(".lucidos/exhaust")
            .join(&run_id);
        if let Err(e) = std::fs::create_dir_all(&script_dir) {
            return Err(format!(
                "Error: failed to create script staging dir {}: {}",
                script_dir.display(),
                e
            ));
        }
        let script_path = script_dir.join("script.py");
        if let Err(e) = std::fs::write(&script_path, code) {
            return Err(format!(
                "Error: failed to write script to {}: {}",
                script_path.display(),
                e
            ));
        }

        let env_vars = self.build_script_env_vars(Some(thread_id)).await;
        let command = build_python_background_command(self.python_runtime.python_bin(), &script_path);
        // The constructed command embeds absolute paths only — no user
        // secrets — but stay consistent with bash.rs and redact through
        // the same helper before logging.
        let safe_command = redact_postgres_secrets(&command);
        log!(
            "[PythonBg] Spawning: {}",
            &safe_command[..safe_command.floor_char_boundary(200)]
        );

        let (task_id, finish_rx) = match self
            .bash_background
            .spawn(
                &command,
                timeout_secs,
                self.workspace_path(),
                &env_vars,
                Some(thread_id),
            )
            .await
        {
            Ok(pair) => pair,
            Err(e) => {
                // No `BackgroundBashStarted` will be emitted, so the
                // script file on disk has no event-store row to correlate
                // back to. Drop it on the way out — mirrors the sync
                // `execute_python_tool` cleanup pattern (python.rs
                // `remove_dir_all(&staging_dir).ok()` on its error path).
                // Without this, chronic spawn failure (sh fork EAGAIN,
                // FD exhaustion, broken venv) would accumulate orphan
                // dirs under `.lucidos/exhaust/` indefinitely — the
                // startup sweep wipes `.lucidos/staging` but preserves
                // exhaust for audit.
                std::fs::remove_dir_all(&script_dir).ok();
                return Err(format!(
                    "Error: failed to spawn background python: {}",
                    e
                ));
            }
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
            log!("[PythonBg] failed to emit BackgroundBashStarted: {}", e);
        }

        // Reuse the bash watcher — same registry, same completion
        // contract, same auto-wake for parked CC sessions. The watcher's
        // wake text refers to "Background task" (language-agnostic, not
        // "Background bash task") so the LLM sees the right framing for
        // both bash- and python-spawned tasks.
        self.spawn_bash_completion_watcher(
            thread_id,
            task_id.clone(),
            safe_command,
            finish_rx,
        );

        Ok(serde_json::json!({
            "task_id": task_id,
            "started_at": started_at,
            "timeout_secs": timeout_secs,
        })
        .to_string())
    }

    /// Walk staging directory and classify files as created or updated.
    fn collect_staged_files(
        base: &std::path::Path,
        dir: &std::path::Path,
        workspace_data: &std::path::Path,
        created: &mut Vec<String>,
        updated: &mut Vec<String>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        if !dir.exists() {
            return Ok(());
        }
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_dir() {
                Self::collect_staged_files(base, &path, workspace_data, created, updated)?;
            } else {
                let relative = path
                    .strip_prefix(base)
                    .map_err(|e| format!("Path strip failed: {}", e))?
                    .to_string_lossy()
                    .to_string();
                if workspace_data.join(&relative).exists() {
                    updated.push(relative);
                } else {
                    created.push(relative);
                }
            }
        }
        Ok(())
    }
}

/// Shell-quote a path for safe inclusion in a `/bin/sh -c "<cmd>"` string.
/// The background bash registry hands commands to `sh -c`, so the python
/// invocation `run_python_background` builds must be parseable by the shell
/// even when the workspace path contains spaces or other metacharacters.
/// Standard POSIX trick: wrap in single quotes; if the value itself contains
/// a single quote, close-escape-reopen with `'\''`.
fn sh_quote(path: &std::path::Path) -> String {
    let s = path.to_string_lossy();
    if !s.contains('\'') {
        format!("'{}'", s)
    } else {
        format!("'{}'", s.replace('\'', "'\\''"))
    }
}

/// Build the `/bin/sh -c` payload `BackgroundBashRegistry::spawn` will
/// execute for a `run_python_background` call. The venv-rooted python
/// interpreter is invoked with the staged script path; both are sh-quoted
/// so workspace paths containing spaces don't shell-split.
fn build_python_background_command(
    python_bin: &std::path::Path,
    script_path: &std::path::Path,
) -> String {
    format!("{} {}", sh_quote(python_bin), sh_quote(script_path))
}

#[cfg(test)]
mod tests {
    use super::{build_python_background_command, sh_quote};
    use crate::engine::tools::bash_background::BackgroundBashRegistry;
    use crate::runtime::python::PythonRuntime;
    use std::path::PathBuf;
    use std::time::Duration;
    use tempfile::tempdir;

    #[test]
    fn sh_quote_wraps_plain_path_in_single_quotes() {
        let q = sh_quote(&PathBuf::from("/tmp/foo/bar"));
        assert_eq!(q, "'/tmp/foo/bar'");
    }

    #[test]
    fn sh_quote_handles_path_with_spaces() {
        // A workspace under "/Users/Anne Doe/workspaces/dev" must not
        // shell-split into `/Users/Anne` + `Doe/...` when we hand the
        // command to `sh -c`.
        let q = sh_quote(&PathBuf::from("/Users/Anne Doe/ws"));
        assert_eq!(q, "'/Users/Anne Doe/ws'");
    }

    #[test]
    fn sh_quote_escapes_embedded_single_quote() {
        // POSIX trick: close-escape-reopen — `'…'\''…'` so a workspace
        // path with a literal `'` doesn't break out of the wrapping
        // single-quote context.
        let q = sh_quote(&PathBuf::from("/tmp/it's/here"));
        assert_eq!(q, "'/tmp/it'\\''s/here'");
    }

    #[test]
    fn build_python_background_command_joins_interpreter_and_script() {
        let cmd = build_python_background_command(
            &PathBuf::from("/ws/.lucidos/runtime/python/venv/bin/python"),
            &PathBuf::from("/ws/.lucidos/exhaust/abc/script.py"),
        );
        assert_eq!(
            cmd,
            "'/ws/.lucidos/runtime/python/venv/bin/python' '/ws/.lucidos/exhaust/abc/script.py'"
        );
    }

    /// End-to-end pipeline test: prove the `run_python_background` plumbing
    /// works — venv python from `PythonRuntime` invoked via
    /// `BackgroundBashRegistry::spawn`, with output drainable through the
    /// standard `read_output_in_memory_wait` path (the same path `bash_output`
    /// reads from). Without this test the wiring between the two
    /// independent subsystems is unverified at unit level — only at full-
    /// engine integration time, where a regression is much louder to
    /// diagnose.
    ///
    /// The wall-clock-3600s "longer than the 300s sync ceiling" property
    /// the user originally lost a backtest sweep over is structurally
    /// guaranteed by the bash registry path (its `BG_MAX_TIMEOUT_SECS` is
    /// 3600s and `BackgroundBashRegistry::spawn` doesn't impose any
    /// shorter ceiling — see `timeout_kills_long_running_task` in
    /// `bash_background.rs` for the watchdog regression). What this test
    /// adds on top is the python-specific link: the venv python is
    /// actually invokable through that path with the same env-var /
    /// cwd / drain contract.
    #[tokio::test]
    async fn python_via_bash_background_runs_stdlib_script_and_drains_output() {
        let dir = tempdir().unwrap();
        let runtime = PythonRuntime::new(dir.path().to_path_buf()).unwrap();
        runtime.ensure_venv().await.expect("ensure_venv");

        let script_dir = dir.path().join(".lucidos/exhaust/test-run");
        std::fs::create_dir_all(&script_dir).unwrap();
        let script_path = script_dir.join("script.py");
        std::fs::write(
            &script_path,
            "import sys\n\
             print('python_ok')\n\
             print('on stderr', file=sys.stderr)\n",
        )
        .unwrap();

        let reg = BackgroundBashRegistry::new();
        let command = build_python_background_command(runtime.python_bin(), &script_path);
        let (task_id, _finish_rx) = reg
            .spawn(&command, 10, dir.path(), &[], None)
            .await
            .expect("spawn");

        let finished = reg.wait_for_finish(&task_id, Duration::from_secs(8)).await;
        assert!(finished, "python task did not finish within 8s");

        let snap = reg
            .read_output_in_memory_wait(&task_id, std::time::Duration::ZERO)
            .await
            .expect("snapshot");
        assert_eq!(
            snap.exit_code,
            Some(0),
            "python invocation failed: stderr={}",
            snap.stderr
        );
        assert!(
            snap.stdout.contains("python_ok"),
            "stdout was: {}",
            snap.stdout
        );
        assert!(
            snap.stderr.contains("on stderr"),
            "stderr was: {}",
            snap.stderr
        );
        assert!(snap.finished);
    }

    /// Env vars handed to `BackgroundBashRegistry::spawn` reach the python
    /// child — the user's spec is explicit that CRED_* / OAUTH_* /
    /// LUCIDOS_WORKSPACE must be visible to scripts the same way they are
    /// in the sync `run_python` tool. Pin the env-pass-through here so a
    /// refactor that drops the `&env` arg from the spawn call breaks this
    /// test instead of silently shipping a tool whose scripts can't reach
    /// their secrets.
    #[tokio::test]
    async fn python_via_bash_background_inherits_provided_env_vars() {
        let dir = tempdir().unwrap();
        let runtime = PythonRuntime::new(dir.path().to_path_buf()).unwrap();
        runtime.ensure_venv().await.expect("ensure_venv");

        let script_dir = dir.path().join(".lucidos/exhaust/env-test");
        std::fs::create_dir_all(&script_dir).unwrap();
        let script_path = script_dir.join("script.py");
        std::fs::write(
            &script_path,
            "import os\n\
             print('CRED_TEST=' + os.environ.get('CRED_TEST', '<missing>'))\n\
             print('LUCIDOS_WORKSPACE=' + os.environ.get('LUCIDOS_WORKSPACE', '<missing>'))\n",
        )
        .unwrap();

        let env = vec![
            ("CRED_TEST".to_string(), "secret-value".to_string()),
            (
                "LUCIDOS_WORKSPACE".to_string(),
                dir.path().to_string_lossy().to_string(),
            ),
        ];
        let reg = BackgroundBashRegistry::new();
        let command = build_python_background_command(runtime.python_bin(), &script_path);
        let (task_id, _finish_rx) = reg
            .spawn(&command, 10, dir.path(), &env, None)
            .await
            .expect("spawn");

        let finished = reg.wait_for_finish(&task_id, Duration::from_secs(8)).await;
        assert!(finished);

        let snap = reg
            .read_output_in_memory_wait(&task_id, std::time::Duration::ZERO)
            .await
            .expect("snapshot");
        assert_eq!(snap.exit_code, Some(0), "stderr: {}", snap.stderr);
        assert!(
            snap.stdout.contains("CRED_TEST=secret-value"),
            "stdout did not see CRED_TEST: {}",
            snap.stdout
        );
        assert!(
            snap.stdout.contains("LUCIDOS_WORKSPACE="),
            "stdout did not see LUCIDOS_WORKSPACE: {}",
            snap.stdout
        );
    }
}
