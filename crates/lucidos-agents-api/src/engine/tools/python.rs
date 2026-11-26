use super::super::LucidosEngine;
use super::app_lint::app_lint_note_on_disk;
use super::files::{data_file_event, knowhow_size_note, DataFileChange};
use super::ToolOutcome;
use crate::engine::bash_background::{CompletionDelivery, TaskSpec};
use crate::engine::event_bus::{BusEvent, SystemEvent};
use crate::engine::tool_arguments::requested_packages;
use crate::engine::tools::bash::BashExt;
use crate::engine::tools::widgets::WidgetFitExt;
use crate::llm::tools::{BG_DEFAULT_TIMEOUT_SECS, BG_MAX_TIMEOUT_SECS};
use crate::runtime::agent_sandbox::Sandbox;

fn staged_write_events(
    created: &[String],
    updated: &[String],
    commit: &str,
    thread_id: uuid::Uuid,
    writer: crate::engine::event_bus::ArtifactWriter,
) -> Vec<SystemEvent> {
    let created = created.iter().map(|path| (path, false));
    let updated = updated.iter().map(|path| (path, true));
    created
        .chain(updated)
        .filter_map(|(path, existed)| match path.strip_prefix("artifacts/") {
            Some(artifact_path) => Some(SystemEvent::artifact_change(
                existed,
                artifact_path.to_string(),
                commit.to_string(),
                Some("run_python".to_string()),
                Some(thread_id),
                writer.clone(),
            )),
            None => data_file_event(path, DataFileChange::Written { commit }),
        })
        .collect()
}

#[easy_ext::ext(PythonExt)]
pub(crate) impl LucidosEngine {
    async fn execute_python_tool(
        &self,
        args: &serde_json::Value,
        thread_id: uuid::Uuid,
        device_id: Option<&str>,
        sandbox: &Sandbox,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        let code = args["code"].as_str().unwrap_or("");
        let commit_message = args.get("commit_message").and_then(|v| v.as_str());

        if let Err(e) = self
            .python_runtime
            .ensure_packages(sandbox, &requested_packages(args))
            .await
        {
            return Err(format!("installing packages: {}", e).into());
        }

        let env = self
            .build_tool_env_vars(
                thread_id,
                &crate::core::credentials::SecretMentions::of_command(code, self.workspace_path()),
            )
            .await;
        let secrets =
            crate::core::subprocess_secret_values(&env.vars, crate::core::inherited_secret_env());
        let run_id = uuid::Uuid::new_v4().to_string();
        let staging_dir = self.workspace_path().join(".lucidos/staging").join(&run_id);

        let output = match execute_staged_and_clean_up_on_failure(
            &self.python_runtime,
            sandbox,
            code,
            env.vars.clone(),
            &staging_dir,
        )
        .await
        {
            Ok(output) => output,
            Err(e) => {
                return Err(crate::core::redact_subprocess_output(
                    &env.with_note(e.to_string()),
                    &secrets,
                )
                .into())
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
                let writer = crate::core::artifact_writer::ArtifactWriterOrigin::Thread(thread_id)
                    .resolve(self.event_bus.pool())
                    .await?;

                let held = self.lock_workspace_repo().await;
                for path in created.iter().chain(updated.iter()) {
                    let src = data_staging.join(path);
                    let dst = data_dir.join(path);
                    if let Some(parent) = dst.parent() {
                        std::fs::create_dir_all(parent)?;
                    }
                    commit_staged_file(&src, &dst).map_err(|e| {
                        format!(
                            "Failed to commit staged file {} -> {}: {}",
                            src.display(),
                            dst.display(),
                            e
                        )
                    })?;
                }

                let all_paths: Vec<String> =
                    created.iter().chain(updated.iter()).cloned().collect();
                let message = commit_message.unwrap_or("Python script output");
                let commit_sha = self
                    .artifact_manager
                    .commit_data_paths(&held, &all_paths, message)
                    .await
                    .map_err(|e| format!("Git commit failed: {}", e))?;
                drop(held);

                for path in created.iter().chain(updated.iter()) {
                    if let Some(artifact_path) = path.strip_prefix("artifacts/") {
                        self.user_profile
                            .artifact_written_on_disk(self.workspace_path(), artifact_path)
                            .await;
                    }
                }

                for event in staged_write_events(&created, &updated, &commit_sha, thread_id, writer)
                {
                    self.event_bus.emit(BusEvent::System(event)).await?;
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
            let data_dir = self.workspace_path().join(crate::core::DATA_DIR);
            for path in created.iter().chain(updated.iter()) {
                response.push_str(&app_lint_note_on_disk(&data_dir, path).unwrap_or_default());
                response.push_str(&knowhow_size_note(&data_dir, path));
            }
            let written = created.iter().chain(updated.iter()).map(String::as_str);
            response.push_str(&self.widget_fit_notes(written, thread_id, device_id).await);
        }

        Ok(crate::core::redact_subprocess_output(
            &env.with_note(response),
            &secrets,
        ))
    }

    async fn execute_python_background_tool(
        &self,
        args: &serde_json::Value,
        thread_id: uuid::Uuid,
        sandbox: &Sandbox,
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

        if let Err(e) = self.python_runtime.ensure_venv().await {
            return Err(format!("Error: failed to prepare python venv: {}", e));
        }

        if let Err(e) = self
            .python_runtime
            .ensure_packages(sandbox, &requested_packages(args))
            .await
        {
            return Err(format!("Error: installing packages: {}", e));
        }

        let run_id = uuid::Uuid::new_v4().to_string();
        let script_dir = self.workspace_path().join(".lucidos/exhaust").join(&run_id);
        if let Err(e) = std::fs::create_dir_all(&script_dir) {
            return Err(format!(
                "Error: failed to create script staging dir {}: {}",
                crate::core::home_path::abbreviate(&script_dir),
                e
            ));
        }
        let script_path = script_dir.join("script.py");
        if let Err(e) = std::fs::write(&script_path, code) {
            return Err(format!(
                "Error: failed to write script to {}: {}",
                crate::core::home_path::abbreviate(&script_path),
                e
            ));
        }

        let env = self
            .build_tool_env_vars(
                thread_id,
                &crate::core::credentials::SecretMentions::of_command(code, self.workspace_path()),
            )
            .await;
        let command =
            build_python_background_command(self.python_runtime.python_bin(), &script_path);

        let (task_id, started_at) = match self
            .start_background_task(
                sandbox,
                thread_id,
                &command,
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
            .await
        {
            Ok(pair) => pair,
            Err(e) => {
                std::fs::remove_dir_all(&script_dir).ok();
                return Err(e);
            }
        };

        Ok(super::bash::background_start_result(
            &task_id,
            started_at,
            timeout_secs,
            &env,
        ))
    }

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
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if file_type.is_symlink() {
                continue;
            }
            if file_type.is_dir() {
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

async fn execute_staged_and_clean_up_on_failure(
    python_runtime: &crate::runtime::PythonRuntime,
    sandbox: &Sandbox,
    code: &str,
    env_vars: Vec<(String, String)>,
    staging_dir: &std::path::Path,
) -> Result<String, String> {
    match python_runtime
        .execute_staged(sandbox, code, env_vars, staging_dir)
        .await
    {
        Ok(output) => Ok(output),
        Err(e) => {
            std::fs::remove_dir_all(staging_dir).ok();
            Err(e)
        }
    }
}

#[cfg(unix)]
fn commit_staged_file(src: &std::path::Path, dst: &std::path::Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    const PERM_BITS: u32 = 0o7777;
    let intended_mode = match std::fs::metadata(dst) {
        Ok(meta) => meta.permissions().mode() & PERM_BITS,
        Err(_) => {
            std::fs::File::create(dst)?;
            std::fs::metadata(dst)?.permissions().mode() & PERM_BITS
        }
    };
    std::fs::copy(src, dst)?;
    std::fs::set_permissions(dst, std::fs::Permissions::from_mode(intended_mode))
}

#[cfg(not(unix))]
fn commit_staged_file(src: &std::path::Path, dst: &std::path::Path) -> std::io::Result<()> {
    std::fs::copy(src, dst).map(|_| ())
}

fn sh_quote(path: &std::path::Path) -> String {
    let s = path.to_string_lossy();
    if !s.contains('\'') {
        format!("'{}'", s)
    } else {
        format!("'{}'", s.replace('\'', "'\\''"))
    }
}

fn build_python_background_command(
    python_bin: &std::path::Path,
    script_path: &std::path::Path,
) -> String {
    format!("{} {}", sh_quote(python_bin), sh_quote(script_path))
}

#[cfg(test)]
mod tests {
    use super::PythonExt;
    use super::{
        build_python_background_command, commit_staged_file,
        execute_staged_and_clean_up_on_failure, sh_quote, staged_write_events,
    };
    use crate::core::shell::TaskOutcome;
    use crate::engine::bash_background::BackgroundBashRegistry;
    use crate::engine::event_bus::SystemEvent;

    #[test]
    fn staged_writes_announce_in_the_shape_every_other_writer_uses() {
        let thread = uuid::Uuid::new_v4();
        let created = vec![
            "artifacts/fare-watch/fares.json".to_string(),
            "apps/habit-tracker/state.json".to_string(),
        ];
        let updated = vec![
            "artifacts/notes.md".to_string(),
            "knowhow/fares.md".to_string(),
        ];
        let events = staged_write_events(
            &created,
            &updated,
            "abc123",
            thread,
            crate::engine::event_bus::ArtifactWriter::Thread,
        );

        let shapes: Vec<(String, String)> = events
            .iter()
            .map(|event| match event {
                SystemEvent::ArtifactCreated { artifact_path, .. } => {
                    ("ArtifactCreated".into(), artifact_path.clone())
                }
                SystemEvent::ArtifactUpdated { artifact_path, .. } => {
                    ("ArtifactUpdated".into(), artifact_path.clone())
                }
                SystemEvent::DataFileWritten { path, .. } => {
                    ("DataFileWritten".into(), path.clone())
                }
                other => panic!("unexpected event {other:?}"),
            })
            .collect();
        assert_eq!(
            shapes,
            vec![
                ("ArtifactCreated".into(), "fare-watch/fares.json".into()),
                (
                    "DataFileWritten".into(),
                    "apps/habit-tracker/state.json".into()
                ),
                ("ArtifactUpdated".into(), "notes.md".into()),
                ("DataFileWritten".into(), "knowhow/fares.md".into()),
            ]
        );

        for event in &events {
            match event {
                SystemEvent::ArtifactCreated { writer, .. }
                | SystemEvent::ArtifactUpdated { writer, .. } => assert_eq!(
                    writer.as_ref(),
                    Some(&crate::engine::event_bus::ArtifactWriter::Thread)
                ),
                _ => {}
            }
        }
        let SystemEvent::ArtifactCreated {
            source,
            writer_thread_id,
            commit,
            ..
        } = &events[0]
        else {
            unreachable!()
        };
        assert_eq!(source.as_deref(), Some("run_python"));
        assert_eq!(*writer_thread_id, Some(thread));
        assert_eq!(commit, "abc123");
        let SystemEvent::DataFileWritten { commit, .. } = &events[1] else {
            unreachable!()
        };
        assert_eq!(commit.as_deref(), Some("abc123"));
    }

    use crate::runtime::python::PythonRuntime;
    use std::path::PathBuf;
    use std::time::Duration;
    use tempfile::tempdir;

    #[cfg(unix)]
    fn mode_of(path: &std::path::Path) -> u32 {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    #[cfg(unix)]
    fn write_with_mode(path: &std::path::Path, contents: &str, mode: u32) {
        use std::os::unix::fs::PermissionsExt;
        std::fs::write(path, contents).unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn commit_staged_file_keeps_the_existing_destination_mode() {
        let tmp = tempdir().unwrap();
        let src = tmp.path().join("staged.json");
        let dst = tmp.path().join("data.json");
        write_with_mode(&dst, "{\"old\":true}", 0o644);
        write_with_mode(&src, "{\"new\":true}", 0o600);

        commit_staged_file(&src, &dst).unwrap();

        assert_eq!(
            mode_of(&dst),
            0o644,
            "destination mode must survive a commit"
        );
        assert_eq!(std::fs::read_to_string(&dst).unwrap(), "{\"new\":true}");
        assert_eq!(mode_of(&src), 0o600, "the staged copy is left alone");
    }

    #[cfg(unix)]
    #[test]
    fn commit_staged_file_gives_a_new_file_a_umask_respecting_mode() {
        let tmp = tempdir().unwrap();
        let src = tmp.path().join("staged.txt");
        let dst = tmp.path().join("brand-new.txt");
        write_with_mode(&src, "hello", 0o600);

        let reference = tmp.path().join("reference.txt");
        std::fs::write(&reference, "x").unwrap();
        let expected = mode_of(&reference);

        commit_staged_file(&src, &dst).unwrap();

        assert_eq!(mode_of(&dst), expected, "a new file follows the umask");
        if expected != 0o600 {
            assert_ne!(
                mode_of(&dst),
                0o600,
                "a new file must not inherit the staged copy's private mode"
            );
        }
        assert_eq!(std::fs::read_to_string(&dst).unwrap(), "hello");
    }

    async fn staging_fixture(ws: &std::path::Path, ceiling: Duration) -> PythonRuntime {
        std::fs::create_dir_all(ws.join("data/artifacts")).unwrap();
        std::fs::write(ws.join("data/artifacts/report.csv"), "original").unwrap();
        let runtime = PythonRuntime::new(ws.to_path_buf()).unwrap();
        runtime
            .execute(
                &crate::runtime::agent_sandbox::Sandbox::unsandboxed(),
                "print('warmup')",
            )
            .await
            .expect("warmup");
        runtime.with_execution_timeout(ceiling)
    }

    #[tokio::test]
    async fn a_timed_out_staged_run_commits_nothing_and_cleans_up() {
        let dir = tempdir().unwrap();
        let ws = dir.path();
        let runtime = staging_fixture(ws, Duration::from_secs(1)).await;

        let staging = ws.join(".lucidos/staging").join("timed-out-run");
        let err = execute_staged_and_clean_up_on_failure(
            &runtime,
            &crate::runtime::agent_sandbox::Sandbox::unsandboxed(),
            "open('data/artifacts/report.csv', 'w').write('half written')\n\
             import time\n\
             time.sleep(60)",
            vec![],
            &staging,
        )
        .await
        .expect_err("the script sleeps past the ceiling");

        assert!(err.contains("timed out"), "got: {err}");
        assert!(
            !staging.exists(),
            "a timed-out run must remove its staging tree: {}",
            staging.display()
        );
        assert_eq!(
            std::fs::read_to_string(ws.join("data/artifacts/report.csv")).unwrap(),
            "original",
            "a timed-out run must not publish its half-written output"
        );
    }

    #[tokio::test]
    async fn a_clean_staged_run_keeps_its_tree_for_the_committer() {
        let dir = tempdir().unwrap();
        let ws = dir.path();
        let runtime = staging_fixture(ws, Duration::from_secs(30)).await;

        let staging = ws.join(".lucidos/staging").join("clean-run");
        let out = execute_staged_and_clean_up_on_failure(
            &runtime,
            &crate::runtime::agent_sandbox::Sandbox::unsandboxed(),
            "open('data/artifacts/report.csv', 'w').write('fresh'); print('done')",
            vec![],
            &staging,
        )
        .await
        .expect("a fast script must still succeed");

        assert_eq!(out.trim(), "done");
        assert_eq!(
            std::fs::read_to_string(staging.join("data/artifacts/report.csv")).unwrap(),
            "fresh",
            "the staged write must survive for the committer to publish"
        );
    }

    #[tokio::test]
    async fn a_crashed_staged_run_also_cleans_up() {
        let dir = tempdir().unwrap();
        let ws = dir.path();
        let runtime = staging_fixture(ws, Duration::from_secs(30)).await;

        let staging = ws.join(".lucidos/staging").join("crashed-run");
        let err = execute_staged_and_clean_up_on_failure(
            &runtime,
            &crate::runtime::agent_sandbox::Sandbox::unsandboxed(),
            "open('data/artifacts/report.csv', 'w').write('half written')\n\
             raise ValueError('kaboom')",
            vec![],
            &staging,
        )
        .await
        .expect_err("the script raises");

        assert!(err.contains("ValueError"), "got: {err}");
        assert!(
            !staging.exists(),
            "a crashed run must remove its staging tree"
        );
        assert_eq!(
            std::fs::read_to_string(ws.join("data/artifacts/report.csv")).unwrap(),
            "original"
        );
    }

    #[test]
    fn sh_quote_wraps_plain_path_in_single_quotes() {
        let q = sh_quote(&PathBuf::from("/tmp/foo/bar"));
        assert_eq!(q, "'/tmp/foo/bar'");
    }

    #[test]
    fn sh_quote_handles_path_with_spaces() {
        let q = sh_quote(&PathBuf::from("/Users/me/my ws"));
        assert_eq!(q, "'/Users/me/my ws'");
    }

    #[test]
    fn sh_quote_escapes_embedded_single_quote() {
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
            .spawn(&command, 10, dir.path(), &[], None, None)
            .await
            .expect("spawn");

        let finished = reg.wait_for_finish(&task_id, Duration::from_secs(8)).await;
        assert!(finished, "python task did not finish within 8s");

        let snap = reg
            .read_output_in_memory_wait(&task_id, std::time::Duration::ZERO)
            .await
            .expect("snapshot");
        assert_eq!(
            snap.outcome,
            Some(TaskOutcome::Exited(0)),
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
            .spawn(&command, 10, dir.path(), &env, None, None)
            .await
            .expect("spawn");

        let finished = reg.wait_for_finish(&task_id, Duration::from_secs(8)).await;
        assert!(finished);

        let snap = reg
            .read_output_in_memory_wait(&task_id, std::time::Duration::ZERO)
            .await
            .expect("snapshot");
        assert_eq!(
            snap.outcome,
            Some(TaskOutcome::Exited(0)),
            "stderr: {}",
            snap.stderr
        );
        assert!(
            snap.stdout.contains("CRED_TEST=[REDACTED]"),
            "the child must receive CRED_TEST and the drain must mask it: {}",
            snap.stdout
        );
        assert!(
            !snap.stdout.contains("secret-value"),
            "the drain handed the model a live credential: {}",
            snap.stdout
        );
        assert!(
            snap.stdout.contains("LUCIDOS_WORKSPACE="),
            "stdout did not see LUCIDOS_WORKSPACE: {}",
            snap.stdout
        );
    }

    #[cfg(unix)]
    #[test]
    fn collecting_staged_files_never_follows_a_symlink() {
        let tmp = tempdir().unwrap();
        let staging = tmp.path().join("staging");
        let host = tmp.path().join("host");
        std::fs::create_dir_all(staging.join("sub")).unwrap();
        std::fs::create_dir_all(&host).unwrap();
        std::fs::write(host.join("passwd"), "host file").unwrap();
        std::fs::write(staging.join("sub/real.txt"), "staged").unwrap();
        std::os::unix::fs::symlink(&host, staging.join("dir_link")).unwrap();
        std::os::unix::fs::symlink(host.join("passwd"), staging.join("file_link")).unwrap();
        std::os::unix::fs::symlink(&staging, staging.join("sub/loop")).unwrap();

        let data = tmp.path().join("data");
        let (mut created, mut updated) = (Vec::new(), Vec::new());
        <crate::engine::LucidosEngine as PythonExt>::collect_staged_files(
            &staging,
            &staging,
            &data,
            &mut created,
            &mut updated,
        )
        .unwrap();

        assert_eq!(created, vec!["sub/real.txt".to_string()]);
        assert!(updated.is_empty());
    }

    mod guard_tests {
        fn production_src() -> String {
            crate::test_support::source_scan::read_production_source(
                &crate::test_support::source_scan::engine_source("engine/tools/python.rs"),
            )
        }

        #[test]
        fn both_python_tool_exits_leave_through_the_secret_redaction() {
            let src = production_src();
            let at = src
                .find("async fn execute_python_tool")
                .expect("execute_python_tool is still here");
            let end = src[at..]
                .find("async fn execute_python_background_tool(")
                .expect("execute_python_tool is still followed by the background tool");
            let body = &src[at..at + end];
            assert_eq!(
                body.matches("redact_subprocess_output(").count(),
                2,
                "both the Ok and the Err exit of execute_python_tool must go through \
             `core::redact_subprocess_output`. A traceback carries the script's stderr, \
             which is where an echoed credential lands."
            );
            assert!(body.contains("subprocess_secret_values(&env.vars,"));
        }

        #[test]
        fn the_workspace_lock_spans_the_copy_through_the_commit() {
            let src = production_src();
            let at = src
                .find("async fn execute_python_tool")
                .expect("execute_python_tool is still here");
            let end = src[at..]
                .find("async fn execute_python_background_tool(")
                .expect("execute_python_tool is still followed by the background tool");
            let body = &src[at..at + end];
            assert_eq!(body.matches("lock_workspace_repo()").count(), 1);
            let lock = body.find("lock_workspace_repo()").unwrap();
            let first_copy = body.find("commit_staged_file(").unwrap();
            let commit = body.find("commit_data_paths(").unwrap();
            let release = body.find("drop(held)").expect("the lock is released");
            assert!(lock < first_copy, "the lock must precede the first copy");
            assert!(commit < release, "the lock must outlive the commit");
        }

        #[test]
        fn the_profile_refresh_runs_before_the_staged_write_events() {
            let src = production_src();
            let at = src
                .find("async fn execute_python_tool")
                .expect("execute_python_tool is still here");
            let end = src[at..]
                .find("async fn execute_python_background_tool(")
                .expect("execute_python_tool is still followed by the background tool");
            let body = &src[at..at + end];
            let refresh = body.find("artifact_written_on_disk(").unwrap();
            let emit = body.find("staged_write_events(").unwrap();
            assert!(refresh < emit, "the refresh must come before the emits");
        }

        #[test]
        fn a_failed_background_spawn_removes_the_script_directory() {
            let src = production_src();
            let at = src
                .find("async fn execute_python_background_tool(")
                .expect("the background tool is still here");
            let body = &src[at..];
            let spawn = body.find("start_background_task(").unwrap();
            let cleanup = body
                .find("remove_dir_all(&script_dir)")
                .expect("the spawn failure arm removes the script directory");
            let leave = body[cleanup..].find("return Err(e)").unwrap();
            assert!(spawn < cleanup);
            assert!(leave < 80, "the cleanup must sit in the failure arm");
        }

        #[test]
        fn run_python_executes_through_the_cleaning_wrapper() {
            let src = production_src();
            let at = src
                .find("async fn execute_python_tool")
                .expect("execute_python_tool is still here");
            let end = src[at..]
                .find("async fn execute_python_background_tool(")
                .expect("execute_python_tool is still followed by the background tool");
            let body = &src[at..at + end];
            assert_eq!(
                body.matches("execute_staged_and_clean_up_on_failure(")
                    .count(),
                1
            );
            assert!(
                !body.contains(".execute_staged("),
                "the runtime was called directly"
            );
        }
    }
}
