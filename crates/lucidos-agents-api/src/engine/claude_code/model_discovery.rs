use super::LucidosEngine;
use crate::core::prefs;
use crate::runtime::cc_model_discovery as discovery;
use crate::runtime::AccountPin;

pub async fn start_cc_model_discovery(engine: &LucidosEngine) {
    engine.start_cc_model_discovery().await;
}

#[easy_ext::ext(CcModelDiscoveryExt)]
pub(crate) impl LucidosEngine {
    async fn start_cc_model_discovery(&self) {
        discovery::load(self.workspace_path());
        self.refresh_cc_models_if_stale(None, None);
    }

    fn refresh_cc_models_if_stale(
        &self,
        reported_version: Option<&str>,
        reported_account: Option<&AccountPin>,
    ) {
        let Some(slot) = discovery::claim_refresh(reported_version, reported_account) else {
            return;
        };
        let pool = self.pool().clone();
        let workspace = self.workspace_path().to_path_buf();
        tokio::spawn(async move {
            let _slot = slot;
            let binary_override = prefs::CODING_AGENT_CLAUDE_PATH.read(&pool).await;
            let permission_mode = prefs::CODING_AGENT_CLAUDE_PERMISSION_MODE
                .stored(&pool)
                .await;
            let user_env =
                crate::core::EnvironmentVariableStore::spawn_pairs(&pool, "CcModels").await;
            let sandbox =
                match crate::runtime::agent_sandbox::Sandbox::resolve(&pool, &workspace).await {
                    Ok(sandbox) => sandbox,
                    Err(e) => {
                        log_legacy!("[CcModels] Probe not started: {}", e);
                        discovery::record_error(e.to_string());
                        return;
                    }
                };
            let args = crate::runtime::SpawnArgs {
                worktree_path: &workspace,
                coding_agent_kind: Default::default(),
                workspace_path: &workspace,
                allowed_tools: None,
                system_prompt: None,
                session_context: None,
                resume_session_id: None,
                model: None,
                reasoning_effort: None,
                thread_id: uuid::Uuid::nil(),
                spawning_event_id: None,
                repo_name: None,
                interactive: false,
                build_slot_class: lucidos_build_slot::SlotClass::Ordinary,
                user_env_vars: &user_env,
                account_pin: None,
                binary_override: binary_override.as_deref(),
                permission_mode: permission_mode.as_deref(),
                additional_directories: &[],
                auto_compact: None,
                keep_alive: false,
                agent_run: crate::runtime::agent_run_marker::AgentRun::unrecorded(),
                sandbox,
            };
            match crate::runtime::claude_code::probe_cc_models(args).await {
                Ok(found) => {
                    log_legacy!(
                        "[CcModels] Claude Code {} lists {} models",
                        found.cc_version.as_deref().unwrap_or("(version unknown)"),
                        found.models.len()
                    );
                    if let Err(e) = discovery::save_cache(&workspace, &found) {
                        log_legacy!("[CcModels] Could not save the model list: {}", e);
                    }
                    discovery::install(&workspace, found);
                }
                Err(e) => {
                    log_legacy!("[CcModels] Probe failed, keeping the last list: {}", e);
                    discovery::record_error(e.to_string());
                }
            }
        });
    }

    fn record_cc_model_refusal(
        &self,
        model: &str,
        cc_version: Option<&str>,
        session: Option<&AccountPin>,
        live: Option<&AccountPin>,
    ) -> bool {
        if discovery::record_refusal(self.workspace_path(), model, cc_version, session, live) {
            log_legacy!("[CcModels] Claude Code refused {model}; automatic picks skip it");
        }
        discovery::refusal_untold(model)
    }

    fn cc_model_refusal_untold(&self, model: &str) -> bool {
        discovery::refusal_untold(model)
    }

    fn record_cc_model_refusal_told(&self, model: &str) {
        discovery::record_refusal_told(self.workspace_path(), model);
    }

    fn record_cc_model_served(
        &self,
        model: &str,
        session: Option<&AccountPin>,
        live: Option<&AccountPin>,
    ) {
        if discovery::record_served(self.workspace_path(), model, session, live) {
            log_legacy!("[CcModels] Claude Code served {model} again; its refusal ends");
        }
    }

    fn observe_cc_live_account(&self, live: Option<&AccountPin>) {
        discovery::observe_live_account(self.workspace_path(), live);
    }
}
