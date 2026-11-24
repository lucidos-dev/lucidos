use lucidos_engine::core::snapshot::first_boot::VerifiedRestoreBoot;
use lucidos_engine::engine::{agent_host::AgentHost, LucidosEngine};
use lucidos_engine::llm::ProviderBuildContext;
use std::{path::PathBuf, sync::Arc};

/// Constructs the engine after the executable acquires workspace ownership
/// and verifies any restore capability.
pub async fn construct_engine(
    workspace_path: PathBuf,
    database_url: &str,
    provider_ctx: ProviderBuildContext,
    agent_host: Arc<dyn AgentHost>,
    restore_boot: Option<VerifiedRestoreBoot<'_>>,
) -> Result<LucidosEngine, Box<dyn std::error::Error + Send + Sync>> {
    match restore_boot {
        Some(guard) => {
            LucidosEngine::new_with_restore(
                workspace_path,
                database_url,
                provider_ctx,
                agent_host,
                guard,
            )
            .await
        }
        None => LucidosEngine::new(workspace_path, database_url, provider_ctx, agent_host).await,
    }
}
