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

    /// Build the env for script execution: the user's variables, the engine's
    /// own (`LUCIDOS_WORKSPACE`, `PATH`, the database handle, origin), and the
    /// `CRED_*` / `OAUTH_*` variables `mentions` names. Used by `run_bash`,
    /// `run_bash_background`, `run_python`, `run_python_background` and
    /// `execute_script` (scheduled tasks).
    ///
    /// `LUCIDOS_WORKSPACE` + the `.lucidos/bin` symlink let scripts call
    /// `lucidos data write` / `lucidos events emit` / `lucidos events query`
    /// instead of hand-rolling HTTP requests back to the engine.
    ///
    /// `emitting_trigger_id` is the trigger whose fire this subprocess is, so
    /// its emits back through the CLI carry the marker ADR 0137 needs. Every
    /// caller states it, and `None` is the honest answer outside a fire.
    pub(crate) async fn build_script_env_vars(
        &self,
        thread_id: Option<Uuid>,
        emitting_trigger_id: Option<&str>,
        mentions: &crate::core::credentials::SecretMentions,
    ) -> ScriptEnv {
        let mut env = named_secret_env_vars(&self.pool, mentions).await;
        let mut vars = self
            .build_env_without_secrets(
                thread_id,
                crate::scheduler::user_tasks::current_event_trigger_depth(),
                emitting_trigger_id,
            )
            .await;
        vars.append(&mut env.vars);
        env.vars = vars;
        env
    }

    /// Env for a coding agent's *background task*: what the agent's own shell
    /// gets (`runtime::spawn_env::apply_lucidos_env`), and no more.
    ///
    /// **No `CRED_*` or `OAUTH_*`.** A coding agent's shell never holds the
    /// workspace's secrets. A task it starts through the engine must not
    /// either, or the route would hand them over. The compile env follows the
    /// agent's rule, so a build that works in its shell works here too.
    pub(crate) async fn build_agent_task_env_vars(
        &self,
        thread_id: Uuid,
        worktree: &std::path::Path,
    ) -> Vec<(String, String)> {
        let depth =
            crate::scheduler::user_tasks::chain_depth_for_thread(thread_id).unwrap_or_default();
        let mut env_vars = self
            .build_env_without_secrets(Some(thread_id), depth, None)
            .await;
        env_vars.extend(
            crate::runtime::spawn_env::agent_compile_env(
                crate::runtime::spawn_env::AgentSandbox::Unsandboxed,
                worktree,
            )
            .into_iter()
            .map(|(key, value)| (key.to_string(), value)),
        );
        env_vars
    }

    /// Everything a Lucidos-spawned subprocess gets except the secrets.
    async fn build_env_without_secrets(
        &self,
        thread_id: Option<Uuid>,
        chain_depth: u32,
        emitting_trigger_id: Option<&str>,
    ) -> Vec<(String, String)> {
        use crate::core::EnvironmentVariableStore;

        // User-managed environment variables FIRST. The spawn applies the pairs
        // in order, so an engine-owned var always wins a collision. `env_pairs`
        // also drops reserved names, as a second backstop.
        let mut env_vars = EnvironmentVariableStore::spawn_pairs(&self.pool, "Python").await;
        env_vars.extend(engine_owned_script_env(
            self.workspace_path(),
            thread_id,
            chain_depth,
            emitting_trigger_id,
        ));
        env_vars
    }

    /// Env for a bash or python tool, which is [`Self::build_script_env_vars`]
    /// plus the ambient trigger. The tool runs inside the fire rather than as
    /// work the fire hands off, so the marker travels with it (ADR 0137).
    ///
    /// Reading `ACTIVE_TRIGGER_ID` is right for exactly that reason. A spawn
    /// that starts a new thread passes `None` instead, and says so at its own
    /// call site.
    pub(crate) async fn build_tool_env_vars(
        &self,
        thread_id: Uuid,
        mentions: &crate::core::credentials::SecretMentions,
    ) -> ScriptEnv {
        let trigger = crate::scheduler::user_tasks::current_trigger_id();
        self.build_script_env_vars(Some(thread_id), trigger.as_deref(), mentions)
            .await
    }

    /// Execute a script file by workspace-relative path. Used by scheduled script tasks.
    ///
    /// Runtime is determined by file extension:
    /// - `.py` → Python (per-workspace venv)
    /// - `.sh` → Bash (`/bin/sh`)
    ///
    /// `emitting_trigger_id` is set when this script IS a trigger's fire, which
    /// is every scheduled-script run. It reaches the origin token, so the
    /// script's own emits cannot wake the trigger that ran it (ADR 0137).
    pub async fn execute_script(
        &self,
        script_path: &str,
        args: &[String],
        extra_env: &[(String, String)],
        emitting_trigger_id: Option<&str>,
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

        // Script args + the credentials/OAuth vars the script file names
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
        // A scheduled script runs without a thread context, so it has no source
        // thread id to attribute its HTTP callbacks back to. It does carry its
        // trigger: the script IS the fire, not work the fire handed off.
        let script_env = self
            .build_script_env_vars(
                None,
                emitting_trigger_id,
                &crate::core::credentials::SecretMentions::of_script_file(&full_path),
            )
            .await;
        if let Some(note) = &script_env.secret_note {
            log!("[Script] {script_path}: {note}");
        }
        env_vars.extend(script_env.vars.iter().cloned());
        env_vars.extend(extra_env.iter().cloned());

        // Read before the env is moved into a runtime below. A script's stdout
        // becomes the trigger summary and its stderr becomes the failure
        // notification, so both carry any credential the script echoed. Same
        // treatment the synchronous `run_bash` and `run_python` tools give
        // their own output. See `core::subprocess_secret_values`.
        let secrets =
            crate::core::subprocess_secret_values(&env_vars, crate::core::inherited_secret_env());

        let result = match extension {
            // A script runs IN PLACE, from its real on-disk path — never from
            // a copy. `__file__`-relative resolution is the whole
            // point: a trigger script reaching its sibling `../state/` dir the
            // ordinary way must land in the real `data/triggers/<slug>/state/`.
            // Executing a copy under `.lucidos/exhaust/<uuid>/` silently
            // redirects every such path into a phantom dir that no human writes
            // to, and nothing errors (the 2026-07-29 notary-verdict-watch
            // incident). Mirrors the `.sh` branch, which has always run the real
            // path. The copy mechanism stays for `run_python` — there the LLM
            // supplies code as a string with no home on disk.
            "py" => self
                .python_runtime
                .execute_file_with_env(&full_path, env_vars)
                .await
                .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> { e.into() }),
            "sh" => self.execute_shell_script(&full_path, env_vars).await,
            _ => {
                let msg = match crate::triggers::validate_script_extension(script_path) {
                    Err(e) => e,
                    Ok(()) => format!("No runtime configured for '.{}' scripts", extension),
                };
                return Err(msg.into());
            }
        };

        // Both arms, because a failure is the likelier leak: a script that dies
        // mid-request prints the request it was making.
        let output = match result {
            Ok(output) => {
                crate::core::redact_subprocess_output(&script_env.with_note(output), &secrets)
            }
            Err(e) => {
                return Err(crate::core::redact_subprocess_output(
                    &script_env.with_note(e.to_string()),
                    &secrets,
                )
                .into())
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
            crate::core::inherited_secret_env(),
            std::time::Duration::from_secs(SHELL_SCRIPT_TIMEOUT_SECS),
        )
        .await?;

        let stdout = sanitize_for_jsonb(&String::from_utf8_lossy(&output.stdout));
        let stderr = sanitize_for_jsonb(&String::from_utf8_lossy(&output.stderr));

        // Typed for the same reason as the bash tools: `code().unwrap_or(-1)`
        // reported a script killed by SIGSEGV as "exit -1", a number that reads
        // like an ordinary status. `describe()` names the signal instead.
        let outcome = crate::core::shell::TaskOutcome::from_status(output.status);
        if outcome.is_success() {
            Ok(stdout)
        } else {
            Err(format!("Shell script error ({}):\n{}", outcome.describe(), stderr).into())
        }
    }

    /// Commit all dirty data/ files with a 30s timeout, logging success/failure.
    /// Shared by all code paths that may produce dirty files (scripts, Claude Code, run_python).
    pub(crate) async fn commit_dirty_logged(&self, message: &str, context: &str) {
        // `commit_all_dirty` takes the workspace repo lock itself and moves it
        // into its blocking closure, so a caller must not hold it here. The
        // timeout therefore bounds waiting for the lock as well as the snapshot.
        // Giving up while still queued starts no git work at all. The next
        // auto-commit picks the same files up, because it stages all of data/.
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

/// A chat or script subprocess's env, and the note for any secret name its
/// text mentioned that the workspace does not have.
#[derive(Debug, Default)]
pub(crate) struct ScriptEnv {
    pub(crate) vars: Vec<(String, String)>,
    pub(crate) secret_note: Option<String>,
}

impl ScriptEnv {
    /// `result` with the secret-names note appended, when there is one. The
    /// one place a tool result or a script output gains it.
    pub(crate) fn with_note(&self, mut result: String) -> String {
        if let Some(note) = &self.secret_note {
            if !result.is_empty() {
                result.push_str("\n\n");
            }
            result.push_str(note);
        }
        result
    }
}

/// The `CRED_*` and `OAUTH_*` vars `mentions` names, the OAuth tokens
/// refreshed first, and the note for any name nothing set.
///
/// A text naming no env-shaped word reads no credential, and one naming no
/// `OAUTH_*` variable loads and refreshes no OAuth account. A failed lookup
/// is logged and drops the note, which would otherwise call every name
/// unknown.
pub(crate) async fn named_secret_env_vars(
    pool: &sqlx::PgPool,
    mentions: &crate::core::credentials::SecretMentions,
) -> ScriptEnv {
    use crate::core::credentials::NamedSecretVars;
    use crate::core::credentials::{named_credential_env_vars, secret_names_note};
    use crate::core::{oauth, CredentialStore, OAuthStore};
    let mut named = NamedSecretVars::default();
    let mut lookup_failed = false;

    if mentions.wants_credentials() {
        match CredentialStore::list_all_with_secrets(pool).await {
            Ok(creds) => {
                let resolver = crate::core::secret_reference::engine_resolver(pool);
                named.extend(named_credential_env_vars(creds, mentions, &resolver).await)
            }
            Err(e) => {
                lookup_failed = true;
                log!("[Script] Failed to load credentials for env injection: {e}");
            }
        }
    }

    if mentions.wants_oauth() {
        match OAuthStore::list_all_with_tokens(pool).await {
            Ok(accounts) => {
                let mut chosen = Vec::new();
                for mut account in accounts {
                    let names = oauth::account_env_names(&account.provider);
                    named.known_names.extend(names.iter().cloned());
                    if !names.iter().any(|name| mentions.names(name)) {
                        continue;
                    }
                    if let Err(e) = oauth::refresh_oauth_if_needed(pool, &mut account).await {
                        log!(
                            "[Script] OAuth refresh failed for {}: {e}",
                            account.provider
                        );
                    }
                    chosen.push(account);
                }
                named.env_vars.extend(oauth::account_env_vars(chosen));
            }
            Err(e) => {
                lookup_failed = true;
                log!("[Script] Failed to load OAuth accounts for env injection: {e}");
            }
        }
    }

    let names_note = if lookup_failed {
        None
    } else {
        secret_names_note(mentions, &named.known_names)
    };
    let notes: Vec<String> = [names_note, named.unavailable_note()]
        .into_iter()
        .flatten()
        .collect();
    let secret_note = (!notes.is_empty()).then(|| notes.join("\n"));
    ScriptEnv {
        vars: named.env_vars,
        secret_note,
    }
}

/// The engine-owned, secret-free part of a script's env: workspace and PATH,
/// the database handle, origin attribution and host protection.
fn engine_owned_script_env(
    workspace: &std::path::Path,
    thread_id: Option<Uuid>,
    chain_depth: u32,
    emitting_trigger_id: Option<&str>,
) -> Vec<(String, String)> {
    use crate::runtime::lucidos_cli::{lucidos_cli_dir, workspace_script_env_vars};

    // Prepend the bundled PG client dir (packaged: <resources>/postgres/bin)
    // to the script PATH so `psql "$LUCIDOS_DATABASE_URL"` resolves; it's not
    // on the launchd minimal PATH. Unset in dev (docker PG on PATH).
    let pg_bin_dir = std::env::var_os("LUCIDOS_PG_BIN_DIR").map(std::path::PathBuf::from);
    let mut env_vars =
        workspace_script_env_vars(workspace, lucidos_cli_dir(), pg_bin_dir.as_deref());

    // The read-only database handle, and no `PG*` vars: an empty connection
    // string must reach nothing (ADR 0481).
    env_vars.extend(crate::core::lucidos_database_url_env());

    // Subprocess-origin env (token + optional source thread id), from the one
    // definition in `api::actor::subprocess_origin_env_vars`. So a future
    // subprocess surface (MCP child, signer host, …) cannot ship without
    // origin attribution, the failure mode that grew the original incident.
    //
    // The depth is read by the caller, inline, because a script trigger
    // builds its env inside the fire's own scope. It is what lets the
    // script's `lucidos events emit` stay on the fire's chain instead of
    // restarting it at 0 on the axum request task.
    env_vars.extend(
        crate::api::actor::subprocess_origin_env_vars(thread_id, chain_depth, emitting_trigger_id)
            .into_iter()
            .map(|(k, v)| (k.to_string(), v)),
    );

    // Host-process kill-guard env (LUCIDOS_HOST_PID + optional
    // FRONTEND_PID + optional API_PORT), from one definition too. Without it,
    // a python/bash tool that shells out to `web-dev.sh` or `ports.sh` could
    // free up the engine's port by killing the engine itself.
    env_vars.extend(
        crate::api::actor::host_protection_env_vars()
            .into_iter()
            .map(|(k, v)| (k.to_string(), v)),
    );
    env_vars
}

/// Spawn the script under the engine's `pipefail` shell (see `core::shell`) in
/// `workspace_dir` with `env_vars` and without the `inherited` secrets,
/// waiting up to `timeout` for it to finish.
/// `kill_on_drop(true)` is the load-bearing part: on timeout the
/// `wait_with_output` future is dropped, taking the owned child with it, and
/// the OS sends SIGKILL — so a hung scheduled script can't leak an orphaned
/// process. Mirrors `execute_bash_tool` (tools/bash.rs), including the
/// pipefail guarantee: a `… | tee log` inside a trigger script would otherwise
/// report the last stage's status and hide the real failure.
async fn run_shell_script_with_timeout(
    script_path: &std::path::Path,
    workspace_dir: &std::path::Path,
    env_vars: &[(String, String)],
    inherited: &crate::core::InheritedSecretEnv,
    timeout: std::time::Duration,
) -> Result<std::process::Output, Box<dyn std::error::Error + Send + Sync>> {
    let mut cmd = crate::core::shell::command_shell().script(script_path);
    cmd.current_dir(workspace_dir)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    crate::core::apply_to_subprocess_env(
        &mut cmd,
        env_vars,
        crate::core::InheritedSecrets::Strip(inherited),
    );

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

    /// The engine half of every script, tool and agent-task env. The user's
    /// own variables come first and may name `PG*`; the engine adds none
    /// (ADR 0481).
    #[test]
    fn a_script_env_gets_the_read_only_handle_and_no_libpq_defaults() {
        crate::test_support::install_agent_database_url();
        let workspace = tempfile::TempDir::new().expect("tempdir");
        let env = engine_owned_script_env(workspace.path(), Some(Uuid::new_v4()), 0, None);
        for name in [
            "PGUSER",
            "PGPASSWORD",
            "PGHOST",
            "PGPORT",
            "PGDATABASE",
            "DATABASE_URL",
        ] {
            assert!(
                !env.iter().any(|(key, _)| key == name),
                "{name} reached a script"
            );
        }
        let handle = env
            .iter()
            .find(|(key, _)| key == crate::core::LUCIDOS_DATABASE_URL)
            .map(|(_, value)| value.as_str())
            .expect("a script gets the database handle");
        assert!(
            handle.ends_with(lucidos_pg_url::READ_ONLY_OPTION),
            "{handle}"
        );
    }

    /// A scheduled script's output leaves through the redaction, not around it.
    ///
    /// Sibling of `the_bash_tool_result_leaves_through_the_secret_redaction` in
    /// `engine/tools/bash.rs`, and the same invariant. `execute_script` injects
    /// the secrets the script names, then hands stdout to the trigger
    /// summary and stderr to the failure notification. The scheduler persists
    /// both.
    ///
    /// A source scan because driving the composition needs an engine and a
    /// database, where the wiring is what actually regresses.
    /// `core::subprocess_secrets` covers the redaction itself.
    #[test]
    fn a_scheduled_script_result_leaves_through_the_secret_redaction() {
        let src = crate::test_support::source_scan::read_production_source(
            &crate::test_support::source_scan::src_root().join("engine/engine_impl/scripts.rs"),
        );
        let at = src
            .find("crate::core::subprocess_secret_values(&env_vars,")
            .expect(
                "execute_script must read the injected secrets before the env is moved \
                 into a runtime",
            );
        // Bound to the end of `execute_script`, so a later redaction elsewhere
        // in the file cannot satisfy this count and a moved one cannot hide.
        let end = src[at..]
            .find("// Auto-commit any files the script touched")
            .expect("execute_script still commits dirty artifacts after the script returns");
        let body = &src[at..at + end];
        assert_eq!(
            body.matches("crate::core::redact_subprocess_output(")
                .count(),
            2,
            "both arms must redact. A script that dies mid-request prints the request it \
             was making, so the error path is the likelier leak of the two."
        );
        assert!(
            src.contains("crate::core::InheritedSecrets::Strip(inherited)"),
            "a scheduled shell script must not inherit the engine's secrets"
        );
    }

    use crate::core::credentials::SecretMentions;
    use crate::core::{AuthType, CredentialStore, OAuthStore};
    use crate::engine::event_bus::EventBus;

    async fn store_credentials(pool: &sqlx::PgPool, bus: &EventBus) {
        for (name, value) in [
            ("github", "gh-secret-value"),
            ("slack", "slack-secret-value"),
        ] {
            CredentialStore::upsert(
                pool,
                bus,
                name,
                &[],
                AuthType::Bearer,
                value,
                None,
                None,
                None,
            )
            .await
            .unwrap();
        }
    }

    /// Run `script` the way a scheduled script runs, with the secrets its own
    /// file names, and return its stdout.
    async fn run_script_with_its_secrets(pool: &sqlx::PgPool, script: &str) -> String {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("task.sh");
        std::fs::write(&path, script).unwrap();
        let env = named_secret_env_vars(pool, &SecretMentions::of_script_file(&path)).await;
        let output = run_shell_script_with_timeout(
            &path,
            dir.path(),
            &env.vars,
            &crate::core::InheritedSecretEnv::default(),
            std::time::Duration::from_secs(10),
        )
        .await
        .unwrap();
        env.with_note(String::from_utf8_lossy(&output.stdout).into_owned())
    }

    /// The core of path 1: with two credentials stored, a script naming one
    /// sees that one in its environment and not the other.
    #[tokio::test]
    async fn a_subprocess_gets_only_the_credential_it_names() {
        let (pool, db) = crate::test_support::setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        store_credentials(&pool, &bus).await;

        let out = run_script_with_its_secrets(
            &pool,
            "echo \"github=${CRED_GITHUB:-unset}\"\nenv | grep -c '^CRED_' || true\n",
        )
        .await;
        assert!(out.contains("github=gh-secret-value"), "{out}");
        assert!(!out.contains("slack-secret-value"), "{out}");
        assert!(
            out.lines().any(|line| line == "1"),
            "exactly one CRED_ variable: {out}"
        );

        crate::test_support::teardown_test_db(&db).await;
    }

    /// An unknown name never fails the run. The output carries a note naming
    /// the real credentials, and no value.
    #[tokio::test]
    async fn an_unknown_name_runs_and_notes_the_real_names() {
        let (pool, db) = crate::test_support::setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        store_credentials(&pool, &bus).await;

        let out = run_script_with_its_secrets(&pool, "echo \"nope=${CRED_NOPE:-unset}\"\n").await;
        assert!(out.contains("nope=unset"), "{out}");
        assert!(
            out.contains("[lucidos] CRED_NOPE is not a credential in this workspace"),
            "{out}"
        );
        assert!(
            out.contains("CRED_GITHUB") && out.contains("CRED_SLACK"),
            "{out}"
        );
        assert!(
            !out.contains("gh-secret-value") && !out.contains("slack-secret-value"),
            "{out}"
        );

        crate::test_support::teardown_test_db(&db).await;
    }

    /// An OAuth account follows the same rule: only a script naming one of
    /// its variables gets the token.
    #[tokio::test]
    async fn an_oauth_token_reaches_only_a_script_naming_it() {
        let (pool, db) = crate::test_support::setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        OAuthStore::connect(
            &pool,
            &bus,
            "google",
            Some("user@example.com"),
            None,
            "oauth-access-value",
            None,
            None,
            "",
            "",
            None,
        )
        .await
        .unwrap();

        let named = named_secret_env_vars(
            &pool,
            &SecretMentions::scan("curl -H \"Bearer $OAUTH_GOOGLE_ACCESS_TOKEN\""),
        )
        .await;
        assert!(named
            .vars
            .iter()
            .any(|(k, v)| k == "OAUTH_GOOGLE_ACCESS_TOKEN" && v == "oauth-access-value"));
        assert!(named.secret_note.is_none());

        for text in ["ls -la", "echo $CRED_GITHUB"] {
            let unnamed = named_secret_env_vars(&pool, &SecretMentions::scan(text)).await;
            assert!(
                !unnamed.vars.iter().any(|(k, _)| k.starts_with("OAUTH_")),
                "{text}"
            );
        }

        crate::test_support::teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn shell_script_timeout_kills_child() {
        // A script that sleeps then writes a sentinel. With kill_on_drop the
        // timeout SIGKILLs the shell mid-sleep, so the sentinel is never
        // written. Without it, the orphaned child would finish the sleep and
        // touch the sentinel after the future was dropped.
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().to_path_buf();
        let sentinel = dir.join("sentinel");
        let script = dir.join("slow.sh");
        std::fs::write(
            &script,
            format!("sleep 3\ntouch '{}'\n", sentinel.display()),
        )
        .unwrap();

        let err = run_shell_script_with_timeout(
            &script,
            &dir,
            &[],
            &crate::core::InheritedSecretEnv::default(),
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
    }
}
