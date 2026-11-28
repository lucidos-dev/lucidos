use lucidos_engine::core::snapshot::{first_boot::VerifiedRestoreBoot, BoxError};
use lucidos_engine::engine::{
    agent_host::AgentHost,
    event_bus::EventBus,
    provider_host::WorkspaceProviders,
    restore_boot::{settle_restored_authority, RestoreSettlement},
    LucidosEngine,
};
use lucidos_engine::llm::ProviderBuildContext;
use std::{path::PathBuf, sync::Arc};

pub async fn construct_engine(
    workspace_path: PathBuf,
    database_url: &str,
    provider_ctx: ProviderBuildContext,
    agent_host: Arc<dyn AgentHost>,
    workspace_providers: Box<dyn WorkspaceProviders>,
    restore_boot: Option<VerifiedRestoreBoot<'_>>,
) -> Result<LucidosEngine, Box<dyn std::error::Error + Send + Sync>> {
    let mut engine = LucidosEngine::new_with_settlement(
        workspace_path,
        database_url,
        provider_ctx,
        agent_host,
        workspace_providers,
        restore_boot,
        &BootRestoreSettlement,
    )
    .await?;
    engine.register_engine_build_runner(crate::engine_build::CheckoutEngineBuildRunner);
    Ok(engine)
}

struct BootRestoreSettlement;

#[async_trait::async_trait]
impl RestoreSettlement for BootRestoreSettlement {
    async fn settle(
        &self,
        bus: &EventBus,
        restore_boot: Option<VerifiedRestoreBoot<'_>>,
    ) -> Result<(), BoxError> {
        settle_before_dispatch(bus, restore_boot.as_ref()).await?;
        if let Some(restore_boot) = restore_boot {
            restore_boot.complete(bus).await?;
        }
        Ok(())
    }
}

/// INVARIANT: End archived authority before announcing the restore.
/// Every step is durable and retryable after a fresh process acquires ownership.
async fn settle_before_dispatch(
    bus: &EventBus,
    restore_boot: Option<&VerifiedRestoreBoot<'_>>,
) -> Result<(), BoxError> {
    if let Some(restore_boot) = restore_boot {
        restore_boot.validate_boundary(bus.pool()).await?;
    }
    settle_restored_authority(bus).await?;
    if let Some(restore_boot) = restore_boot {
        restore_boot.announce_restored(bus).await?;
    }
    Ok(())
}

#[cfg(test)]
#[path = "restore_boot_engine_tests.rs"]
mod restore_boot_engine_tests;
