use crate::core::{EmailStore, Intent, IntentStore};
use crate::engine::tools::image::ImageExt;
use crate::engine::LucidosEngine;
use crate::llm::ToolCapabilities;

pub(crate) struct TurnCapabilities {
    pub gates: ToolCapabilities,
    pub intents: Vec<Intent>,
    pub schedule: crate::engine::SweepSchedule,
}

#[easy_ext::ext(CapabilitiesExt)]
pub(crate) impl LucidosEngine {
    async fn read_turn_capabilities(&self) -> TurnCapabilities {
        let email_account = match EmailStore::list(&self.pool).await {
            Ok(accounts) => !accounts.is_empty(),
            Err(e) => {
                crate::log_legacy!(
                    "[Tools] email account read failed ({}); offering the email tools anyway",
                    e
                );
                true
            }
        };

        let data_dir = self.workspace_path.join(crate::core::DATA_DIR);
        let intents = IntentStore::load_all(&data_dir);

        // INVARIANT: The self-curated context mode. Read here rather than beside its own
        // consumers so the tools array, the system prompt and the turn's
        // payload all answer from ONE read: a mid-turn Settings change cannot
        // then leave the prompt describing a mode the payload is not in.
        // Total, and an unreadable row resolves OFF.
        let context_mode = crate::core::prefs::SELF_CURATED_CONTEXT_MODE
            .read(&self.pool)
            .await;
        let schedule = if context_mode {
            crate::core::PreferenceStore::self_curated_context_schedule(&self.pool).await
        } else {
            crate::engine::SweepSchedule::default()
        };

        TurnCapabilities {
            gates: ToolCapabilities {
                email_account,
                image_provider: self.current_image_provider().await.is_some(),
                judgment_provider: crate::llm::judgment::judgment_available(&self.pool).await,
                context_mode,
                memory_tree: crate::engine::summary_tree::module::tree_ready(&self.pool).await,
            },
            intents,
            schedule,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::CapabilitiesExt;
    use crate::engine::LucidosEngine;
    use crate::test_support::{setup_test_db, teardown_test_db, test_db_url};

    #[tokio::test]
    async fn an_unreadable_database_opens_the_email_gate_and_closes_the_memory_tree() {
        let (pool, db) = setup_test_db().await;
        let workspace = tempfile::tempdir().unwrap();
        let engine = LucidosEngine::new(
            workspace.path().to_path_buf(),
            &test_db_url(&db),
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
            std::sync::Arc::new(crate::engine::agent_recovery::EngineAgentHost),
        )
        .await
        .unwrap();
        engine.pool.close().await;

        let capabilities = engine.read_turn_capabilities().await;

        assert!(
            capabilities.gates.email_account,
            "an unreadable email read withdrew the email tools"
        );
        assert!(
            !capabilities.gates.memory_tree,
            "an unreadable tree read opened the memory tree"
        );

        pool.close().await;
        teardown_test_db(&db).await;
    }
}
