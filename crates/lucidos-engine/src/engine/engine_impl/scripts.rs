use super::super::*;
use crate::core::subprocess_origin::subprocess_origin_env_vars;

const SHELL_SCRIPT_TIMEOUT_SECS: u64 = crate::llm::tools::MAX_TIMEOUT_SECS;

impl LucidosEngine {
    pub async fn record_trigger_completed(
        &self,
        trigger_id: &str,
        trigger_name: &str,
        result_summary: &str,
        thread_id: Option<Uuid>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
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

    pub async fn get_conversation_at_event(
        &self,
        event_id: Uuid,
    ) -> Result<ConversationSnapshot, Box<dyn std::error::Error + Send + Sync>> {
        self.event_store
            .get_conversation_at_event(event_id, &self.workspace_path)
            .await
    }

    pub(crate) async fn build_script_env_vars(
        &self,
        thread_id: Option<Uuid>,
        emitting_trigger_id: Option<&str>,
        mentions: &crate::core::credentials::SecretMentions,
    ) -> ScriptEnv {
        let mut env = named_secret_env_vars(&self.pool, mentions).await;
        let mut vars = self
            .build_env_without_secrets(subprocess_origin_env_vars(
                thread_id,
                crate::scheduler::user_tasks::current_event_trigger_depth(),
                emitting_trigger_id,
            ))
            .await;
        vars.append(&mut env.vars);
        env.vars = vars;
        env
    }

    /// INVARIANT: Env for a coding agent's *background task*: what the agent's own shell
    /// gets (`runtime::spawn_env::apply_lucidos_env`), and no more.
    ///
    /// **No `CRED_*` or `OAUTH_*`.** A coding agent's shell never holds the
    /// workspace's secrets. A task it starts through the engine must not
    /// either, or the route would hand them over. The compile env follows the
    /// agent's rule, so a build that works in its shell works here too.
    pub async fn build_agent_task_env_vars(
        &self,
        thread_id: Uuid,
        worktree: &std::path::Path,
        sandbox: crate::runtime::spawn_env::AgentSandbox,
    ) -> Result<Vec<(String, String)>, String> {
        let depth =
            crate::scheduler::user_tasks::chain_depth_for_thread(thread_id).unwrap_or_default();
        let mut env_vars = self
            .build_env_without_secrets(subprocess_origin_env_vars(Some(thread_id), depth, None))
            .await;
        env_vars.extend(
            crate::runtime::spawn_env::agent_compile_env(sandbox, worktree)
                .into_iter()
                .map(|(key, value)| (key.to_string(), value)),
        );
        let class = crate::engine::main_check::repair_build_class(&self.pool, thread_id)
            .await
            .map_err(|error| format!("Could not read repair build class: {error}"))?;
        env_vars.push((
            lucidos_build_slot::ENV_CLASS.to_string(),
            class.as_str().to_string(),
        ));
        Ok(env_vars)
    }

    pub async fn build_event_hook_env_vars(
        &self,
        chain_depth: u32,
        slug: &str,
    ) -> Vec<(String, String)> {
        self.build_env_without_secrets(crate::core::subprocess_origin::event_hook_origin_env_vars(
            chain_depth,
            slug,
        ))
        .await
    }

    pub async fn build_event_hook_choices_env_vars(&self) -> Vec<(String, String)> {
        self.build_env_without_secrets(Vec::new()).await
    }

    async fn build_env_without_secrets(
        &self,
        origin: Vec<(&'static str, String)>,
    ) -> Vec<(String, String)> {
        use crate::core::EnvironmentVariableStore;

        // INVARIANT: User-managed environment variables FIRST. The spawn applies the pairs
        // in order, so an engine-owned var always wins a collision. `env_pairs`
        // also drops reserved names, as a second backstop.
        let mut env_vars = EnvironmentVariableStore::spawn_pairs(&self.pool, "Python").await;
        let ps_shim_dir = crate::runtime::agent_sandbox::ps_shim_dir_for_workspace(
            &self.pool,
            self.workspace_path(),
            crate::runtime::lucidos_cli::lucidos_cli_dir(),
        )
        .await;
        env_vars.extend(engine_owned_script_env(
            self.workspace_path(),
            ps_shim_dir.as_deref(),
            origin,
        ));
        env_vars
    }

    pub async fn build_tool_env_vars(
        &self,
        thread_id: Uuid,
        mentions: &crate::core::credentials::SecretMentions,
    ) -> ScriptEnv {
        let trigger = crate::scheduler::user_tasks::current_trigger_id();
        self.build_script_env_vars(Some(thread_id), trigger.as_deref(), mentions)
            .await
    }

    pub fn trigger_execution_sandbox(
        &self,
        sandbox: &crate::runtime::agent_sandbox::Sandbox,
        trigger_id: &str,
    ) -> crate::runtime::agent_sandbox::Sandbox {
        let workspace = self.workspace_path();
        use crate::scheduler::user_tasks::TRIGGER_DEFINITION;

        let grants = TRIGGER_DEFINITION
            .try_with(|digest| {
                self.trigger_configs
                    .read()
                    .unwrap()
                    .get(trigger_id)
                    .filter(|config| config.definition_digest() == *digest)
                    .map(|config| config.folder_grants.clone())
            })
            .ok()
            .flatten()
            .unwrap_or_default();
        let folders: Vec<_> = grants
            .into_iter()
            .chain(crate::scheduler::user_tasks::run_folder_grants())
            .filter(|grant| grant.still_valid(workspace))
            .map(|grant| grant.folder)
            .collect();
        sandbox.for_trigger(workspace, &folders)
    }

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
        let script_env = self
            .build_script_env_vars(
                None,
                emitting_trigger_id,
                &crate::core::credentials::SecretMentions::of_script_file(&full_path),
            )
            .await;
        if let Some(note) = &script_env.secret_note {
            log_legacy!("[Script] {script_path}: {note}");
        }
        env_vars.extend(script_env.vars.iter().cloned());
        env_vars.extend(extra_env.iter().cloned());

        let secrets =
            crate::core::subprocess_secret_values(&env_vars, crate::core::inherited_secret_env());

        let sandbox =
            crate::runtime::agent_sandbox::Sandbox::resolve(self.pool(), self.workspace_path())
                .await?;
        let sandbox = match emitting_trigger_id {
            Some(id) => self.trigger_execution_sandbox(&sandbox, id),
            None => sandbox,
        };
        let result = match extension {
            // INVARIANT: A script runs IN PLACE, from its real on-disk path — never from
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
                .execute_file_with_env(&sandbox, &full_path, env_vars)
                .await
                .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> { e.into() }),
            "sh" => {
                self.execute_shell_script(&sandbox, &full_path, env_vars)
                    .await
            }
            _ => {
                let msg = match crate::triggers::validate_script_extension(script_path) {
                    Err(e) => e,
                    Ok(()) => format!("No runtime configured for '.{}' scripts", extension),
                };
                return Err(msg.into());
            }
        };

        let output = match result {
            Ok(output) => crate::core::redact_subprocess_output(&output, &secrets),
            Err(e) => {
                let diagnostic = crate::core::redact_subprocess_output(&e.to_string(), &secrets);
                return Err(crate::triggers::folder_grant::script_failure(
                    &sandbox,
                    self.workspace_path(),
                    diagnostic,
                ));
            }
        };

        self.commit_dirty_logged("Script task output", script_path)
            .await;

        Ok(output)
    }

    async fn execute_shell_script(
        &self,
        sandbox: &crate::runtime::agent_sandbox::Sandbox,
        script_path: &std::path::Path,
        env_vars: Vec<(String, String)>,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        use crate::core::sanitize_for_jsonb;

        let (output, outcome, denial) = run_shell_script_with_timeout(
            sandbox,
            script_path,
            self.workspace_path(),
            &env_vars,
            crate::core::inherited_secret_env(),
            std::time::Duration::from_secs(SHELL_SCRIPT_TIMEOUT_SECS),
        )
        .await?;

        let stdout = sanitize_for_jsonb(&String::from_utf8_lossy(&output.stdout));
        let stderr = sanitize_for_jsonb(&String::from_utf8_lossy(&output.stderr));

        if outcome.is_success() {
            Ok(stdout)
        } else {
            let error = crate::core::script_output::script_error_output(
                &stderr,
                &stdout,
                stdout.len() as u64,
            );
            let denial = denial.map(|d| format!("\n{d}")).unwrap_or_default();
            Err(format!(
                "Shell script error ({}):\n{error}{denial}",
                outcome.describe()
            )
            .into())
        }
    }

    pub async fn commit_dirty_logged(&self, message: &str, context: &str) {
        // INVARIANT: The timeout covers preparation and waiting for the repo lock.
        // Once the final rescan and commit start, the blocking task keeps the lock
        // until it finishes. A later auto-commit can pick up any remaining files.
        match tokio::time::timeout(
            std::time::Duration::from_secs(30),
            self.artifact_manager.commit_all_dirty(message),
        )
        .await
        {
            Ok(Ok(Some(commit))) => {
                log_legacy!(
                    "[Engine] Auto-committed dirty data files after {} ({})",
                    context,
                    &commit[..commit.floor_char_boundary(7)]
                );
            }
            Ok(Err(e)) => {
                log_legacy!(
                    "[Engine] Failed to commit dirty data files after {}: {}",
                    context,
                    e
                );
            }
            Err(_) => {
                log_legacy!(
                    "[Engine] commit_all_dirty timed out (30s) after {}",
                    context
                );
            }
            Ok(Ok(None)) => {}
        }
    }
}

#[derive(Debug, Default)]
pub struct ScriptEnv {
    pub vars: Vec<(String, String)>,
    pub secret_note: Option<String>,
}

impl ScriptEnv {
    pub fn with_note(&self, mut result: String) -> String {
        if let Some(note) = &self.secret_note {
            if !result.is_empty() {
                result.push_str("\n\n");
            }
            result.push_str(note);
        }
        result
    }
}

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
                log_legacy!("[Script] Failed to load credentials for env injection: {e}");
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
                        log_legacy!(
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
                log_legacy!("[Script] Failed to load OAuth accounts for env injection: {e}");
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

pub async fn stored_secret_values(pool: &sqlx::PgPool) -> Result<Vec<String>, String> {
    use crate::core::credentials::credential_env_vars_for;
    use crate::core::{oauth, CredentialStore, EnvironmentVariableStore, OAuthStore};

    let mut pairs = EnvironmentVariableStore::env_pairs(pool)
        .await
        .map_err(|e| format!("failed to load environment variables: {e}"))?;
    let creds = CredentialStore::list_all_with_secrets(pool)
        .await
        .map_err(|e| format!("failed to load credentials: {e}"))?;
    let resolver = crate::core::secret_reference::engine_resolver(pool);
    for cred in creds
        .into_iter()
        .filter(|cred| cred.auth_type.is_injectable() && !cred.holds_reference())
    {
        pairs.extend(credential_env_vars_for(cred, &resolver).await?);
    }
    let accounts = OAuthStore::list_all_with_tokens(pool)
        .await
        .map_err(|e| format!("failed to load OAuth accounts: {e}"))?;
    pairs.extend(oauth::account_env_vars(accounts));
    Ok(crate::core::subprocess_secret_values(
        &pairs,
        crate::core::inherited_secret_env(),
    ))
}

/// INVARIANT: Masks every stored event payload against [`stored_secret_values`].
///
/// Each reader that hands stored rows out, to the model or over HTTP, calls
/// this. An `Err` means the rows are not masked, so the caller must withhold
/// them.
pub async fn mask_stored_events(
    pool: &sqlx::PgPool,
    events: &mut [crate::core::EventRow],
) -> Result<(), String> {
    if events.is_empty() {
        return Ok(());
    }
    let secrets = stored_secret_values(pool).await?;
    for event in events {
        crate::core::redact_subprocess_output_in_json(&mut event.payload, &secrets);
    }
    Ok(())
}

fn engine_owned_script_env(
    workspace: &std::path::Path,
    ps_shim_dir: Option<&std::path::Path>,
    origin: Vec<(&'static str, String)>,
) -> Vec<(String, String)> {
    use crate::runtime::lucidos_cli::{lucidos_cli_dir, workspace_script_env_vars};

    let pg_bin_dir = std::env::var_os("LUCIDOS_PG_BIN_DIR").map(std::path::PathBuf::from);
    let mut env_vars = workspace_script_env_vars(
        workspace,
        lucidos_cli_dir(),
        ps_shim_dir,
        pg_bin_dir.as_deref(),
    );

    env_vars.extend(crate::core::lucidos_database_url_env());

    env_vars.extend(origin.into_iter().map(|(k, v)| (k.to_string(), v)));

    env_vars.extend(
        crate::core::subprocess_origin::host_protection_env_vars()
            .into_iter()
            .map(|(k, v)| (k.to_string(), v)),
    );
    env_vars
}

/// INVARIANT: Spawn the script under the engine's `pipefail` shell (see `core::shell`) in
/// `workspace_dir` with `env_vars` and without the `inherited` secrets,
/// waiting up to `timeout` for it to finish. A failed script also returns any
/// Seatbelt write denial the log holds for it.
/// `kill_on_drop(true)` is the load-bearing part: on timeout the
/// `wait_with_output` future is dropped, taking the owned child with it, and
/// the OS sends SIGKILL — so a hung scheduled script can't leak an orphaned
/// process. Mirrors `execute_bash_tool` (tools/bash.rs), including the
/// pipefail guarantee: a `… | tee log` inside a trigger script would otherwise
/// report the last stage's status and hide the real failure.
async fn run_shell_script_with_timeout(
    sandbox: &crate::runtime::agent_sandbox::Sandbox,
    script_path: &std::path::Path,
    workspace_dir: &std::path::Path,
    env_vars: &[(String, String)],
    inherited: &crate::core::InheritedSecretEnv,
    timeout: std::time::Duration,
) -> Result<
    (
        std::process::Output,
        crate::core::shell::TaskOutcome,
        Option<String>,
    ),
    Box<dyn std::error::Error + Send + Sync>,
> {
    let mut cmd = crate::core::shell::command_shell().script(sandbox, script_path)?;
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

    let started = chrono::Utc::now();
    let child = cmd
        .spawn()
        .map_err(|e| format!("Failed to spawn shell script: {}", e))?;

    let pid = child.id();
    match tokio::time::timeout(timeout, child.wait_with_output()).await {
        Ok(Ok(output)) => {
            let outcome = crate::core::shell::TaskOutcome::from_status(output.status);
            let denial = sandbox.seatbelt_write_refusal(pid, started, &outcome).await;
            Ok((output, outcome, denial))
        }
        Ok(Err(e)) => Err(format!("Error executing shell script: {}", e).into()),
        Err(_) => Err(format!("Shell script timed out after {}s", timeout.as_secs()).into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_script_env_gets_the_read_only_handle_and_no_libpq_defaults() {
        crate::test_support::install_agent_database_url();
        let workspace = tempfile::TempDir::new().expect("tempdir");
        let env = engine_owned_script_env(
            workspace.path(),
            None,
            subprocess_origin_env_vars(Some(Uuid::new_v4()), 0, None),
        );
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

    async fn run_script_with_its_secrets(pool: &sqlx::PgPool, script: &str) -> String {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("task.sh");
        std::fs::write(&path, script).unwrap();
        let env = named_secret_env_vars(pool, &SecretMentions::of_script_file(&path)).await;
        let (output, _, _) = run_shell_script_with_timeout(
            &crate::runtime::agent_sandbox::Sandbox::unsandboxed(),
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
            &crate::runtime::agent_sandbox::Sandbox::unsandboxed(),
            &script,
            &dir,
            &[],
            &crate::core::InheritedSecretEnv::default(),
            std::time::Duration::from_millis(300),
        )
        .await
        .expect_err("script should time out");
        assert!(err.to_string().contains("timed out"), "got: {}", err);

        tokio::time::sleep(std::time::Duration::from_secs(4)).await;
        assert!(
            !sentinel.exists(),
            "sentinel was created — the timed-out shell child leaked (kill_on_drop missing)"
        );
    }

    mod guard_tests {
        use super::*;

        #[test]
        fn script_environment_handles_stay_reachable_in_the_sandbox() {
            crate::test_support::install_agent_database_url();
            let workspace = tempfile::tempdir().unwrap();
            let env = engine_owned_script_env(
                workspace.path(),
                None,
                subprocess_origin_env_vars(Some(Uuid::new_v4()), 0, None),
            );
            crate::runtime::agent_sandbox::env_guard_tests::assert_spawn_env_reachable(
                workspace.path(),
                env,
            );
        }

        #[test]
        fn a_scheduled_script_result_leaves_through_the_secret_redaction() {
            let src = crate::test_support::source_scan::read_production_source(
                &crate::test_support::source_scan::engine_source("engine/engine_impl/scripts.rs"),
            );
            let at = src
                .find("crate::core::subprocess_secret_values(&env_vars,")
                .expect(
                    "execute_script must read the injected secrets before the env is moved \
                 into a runtime",
                );
            let end = src[at..]
                .find("self.commit_dirty_logged(\"Script task output\", script_path)")
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
    }
}
