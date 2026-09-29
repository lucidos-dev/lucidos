//! Keeps the discovered Claude Code model list fresh
//! (`runtime::cc_model_discovery`). Probes run in the background and never on
//! a request path, so a slow or absent Claude Code only leaves the last list.

use super::LucidosEngine;
use crate::core::PreferenceStore;
use crate::runtime::cc_model_discovery as discovery;

impl LucidosEngine {
    /// Serve the list the last probe found, then probe again if it is missing,
    /// a day old, or from an older Claude Code. Called once at boot.
    pub async fn start_cc_model_discovery(&self) {
        if let Some(cache) = discovery::load_cache(self.workspace_path()) {
            discovery::install(cache);
        }
        self.refresh_cc_models_if_stale(None);
    }

    /// Probe Claude Code in the background when the list needs it.
    /// `reported_version` is what a session's handshake just named. Returns at
    /// once: every read the probe needs happens in the spawned task.
    pub(crate) fn refresh_cc_models_if_stale(&self, reported_version: Option<&str>) {
        let Some(slot) = discovery::claim_refresh(reported_version) else {
            return;
        };
        let pool = self.pool().clone();
        let workspace = self.workspace_path().to_path_buf();
        tokio::spawn(async move {
            let _slot = slot;
            let binary_override = PreferenceStore::get_nonblank(
                &pool,
                crate::core::PREF_CODING_AGENT_CLAUDE_PATH,
                "CcModels",
            )
            .await;
            let permission_mode = PreferenceStore::get_nonblank(
                &pool,
                crate::core::PREF_CODING_AGENT_CLAUDE_PERMISSION_MODE,
                "CcModels",
            )
            .await;
            let user_env =
                crate::core::EnvironmentVariableStore::spawn_pairs(&pool, "CcModels").await;
            // A session in the workspace, minus everything a turn needs: no
            // thread, no prompt, no pinned model, no resume.
            let args = crate::runtime::SpawnArgs {
                worktree_path: &workspace,
                coding_agent_kind: Default::default(),
                workspace_path: &workspace,
                allowed_tools: None,
                system_prompt: None,
                resume_session_id: None,
                model: None,
                reasoning_effort: None,
                thread_id: uuid::Uuid::nil(),
                spawning_event_id: None,
                repo_name: None,
                interactive: false,
                user_env_vars: &user_env,
                account_pin: None,
                binary_override: binary_override.as_deref(),
                permission_mode: permission_mode.as_deref(),
            };
            match crate::runtime::claude_code::probe_cc_models(args).await {
                Ok(found) => {
                    log!(
                        "[CcModels] Claude Code {} lists {} models",
                        found.cc_version.as_deref().unwrap_or("(version unknown)"),
                        found.models.len()
                    );
                    if let Err(e) = discovery::save_cache(&workspace, &found) {
                        log!("[CcModels] Could not save the model list: {}", e);
                    }
                    discovery::install(found);
                }
                Err(e) => {
                    log!("[CcModels] Probe failed, keeping the last list: {}", e);
                    discovery::record_error(e.to_string());
                }
            }
        });
    }
}
