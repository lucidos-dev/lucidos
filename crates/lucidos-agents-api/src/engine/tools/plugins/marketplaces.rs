use std::path::Path;

use crate::core::plugin_marketplaces::{
    add_marketplace, load_registry, remove_marketplace, save_registry, PluginMarketplace,
    MARKETPLACES_DATA_PATH,
};
use crate::core::ArtifactManager;
use crate::engine::event_bus::{BusEvent, EventBusEmitter, SystemEvent};
use crate::engine::git_ops::WorkspaceRepoLock;
use crate::engine::thread_events::MessageOrigin;
use crate::engine::LucidosEngine;

pub(crate) struct MarketplaceRegistration {
    pub marketplace: PluginMarketplace,
    pub marketplaces: Vec<PluginMarketplace>,
    pub created: bool,
    pub commit: String,
}

pub(crate) struct MarketplaceRemoval {
    pub marketplaces: Vec<PluginMarketplace>,
    pub commit: String,
}

#[derive(Debug)]
pub(crate) enum MarketplaceWriteError {
    InvalidSource(String),
    Failed(String),
}

impl std::fmt::Display for MarketplaceWriteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidSource(msg) | Self::Failed(msg) => write!(f, "{msg}"),
        }
    }
}

async fn announce(bus: &dyn EventBusEmitter, event: BusEvent, ctx: &str) {
    if let Err(e) = bus.emit(event).await {
        log_legacy!("[EventBus] {} emit failed: {}", ctx, e);
    }
}

pub(crate) async fn register_with_bus(
    held: &WorkspaceRepoLock,
    workspace_path: &Path,
    artifact_manager: &ArtifactManager,
    bus: &dyn EventBusEmitter,
    source: &str,
    name: Option<&str>,
    actor: Option<MessageOrigin>,
) -> Result<MarketplaceRegistration, MarketplaceWriteError> {
    let mut registry = load_registry(workspace_path)
        .map_err(|e| MarketplaceWriteError::Failed(format!("read marketplace registry: {e}")))?;
    let (marketplace, created) = add_marketplace(&mut registry, source, name)
        .map_err(MarketplaceWriteError::InvalidSource)?;
    save_registry(workspace_path, &registry)
        .map_err(|e| MarketplaceWriteError::Failed(format!("write marketplace registry: {e}")))?;

    let commit = artifact_manager
        .commit_data_path(
            held,
            MARKETPLACES_DATA_PATH,
            if created {
                "Register plugin marketplace"
            } else {
                "Update plugin marketplace"
            },
        )
        .await
        .map_err(|e| MarketplaceWriteError::Failed(format!("commit marketplace registry: {e}")))?;

    announce_write(bus, &commit, actor.clone()).await;
    announce(
        bus,
        BusEvent::System(SystemEvent::PluginMarketplaceRegistered {
            marketplace_id: marketplace.id.clone(),
            name: marketplace.name.clone(),
            source: marketplace.source.clone(),
            actor,
        }),
        "[Plugins] PluginMarketplaceRegistered",
    )
    .await;

    Ok(MarketplaceRegistration {
        marketplace,
        marketplaces: registry.marketplaces,
        created,
        commit,
    })
}

pub(crate) async fn unregister_with_bus(
    held: &WorkspaceRepoLock,
    workspace_path: &Path,
    artifact_manager: &ArtifactManager,
    bus: &dyn EventBusEmitter,
    id: &str,
    actor: Option<MessageOrigin>,
) -> Result<Option<MarketplaceRemoval>, String> {
    let mut registry =
        load_registry(workspace_path).map_err(|e| format!("read marketplace registry: {e}"))?;
    if !remove_marketplace(&mut registry, id) {
        return Ok(None);
    }
    save_registry(workspace_path, &registry)
        .map_err(|e| format!("write marketplace registry: {e}"))?;
    if let Err(e) = crate::core::plugin_media::remove_marketplace_cache(workspace_path, id) {
        log_legacy!("[Plugins] could not delete the media cache of marketplace '{id}': {e}");
    }

    let commit = artifact_manager
        .commit_data_path(held, MARKETPLACES_DATA_PATH, "Remove plugin marketplace")
        .await
        .map_err(|e| format!("commit marketplace registry: {e}"))?;

    announce_write(bus, &commit, actor.clone()).await;
    announce(
        bus,
        BusEvent::System(SystemEvent::PluginMarketplaceRemoved {
            marketplace_id: id.to_string(),
            actor,
        }),
        "[Plugins] PluginMarketplaceRemoved",
    )
    .await;

    Ok(Some(MarketplaceRemoval {
        marketplaces: registry.marketplaces,
        commit,
    }))
}

async fn announce_write(bus: &dyn EventBusEmitter, commit: &str, actor: Option<MessageOrigin>) {
    announce(
        bus,
        BusEvent::System(SystemEvent::DataFileWritten {
            path: MARKETPLACES_DATA_PATH.to_string(),
            commit: Some(commit.to_string()),
            actor,
        }),
        "[Plugins] DataFileWritten",
    )
    .await;
}

#[easy_ext::ext(MarketplacesExt)]
pub(crate) impl LucidosEngine {
    async fn register_plugin_marketplace(
        &self,
        source: &str,
        name: Option<&str>,
        actor: Option<MessageOrigin>,
    ) -> Result<MarketplaceRegistration, MarketplaceWriteError> {
        let registration = {
            register_with_bus(
                &self.lock_workspace_repo().await,
                &self.workspace_path,
                &self.artifact_manager,
                &self.event_bus,
                source,
                name,
                actor,
            )
            .await?
        };

        // INVARIANT: Stamped before the spawn, because the caller's own catalog refetch
        // races the spawned task. A new marketplace with no plugins yet must
        // read as "Scanning marketplaces…", never "No plugins found".
        crate::scheduler::plugin_updates::note_scan_queued(&self.workspace_path);
        let update_engine = self.clone_arc();
        let update_pool = self.pool.clone();
        tokio::spawn(async move {
            crate::scheduler::plugin_updates::run_plugin_marketplace_update_check(
                update_engine,
                update_pool,
                crate::scheduler::plugin_updates::ScanCause::RegistryChanged,
            )
            .await;
        });

        Ok(registration)
    }

    async fn unregister_plugin_marketplace(
        &self,
        id: &str,
        actor: Option<MessageOrigin>,
    ) -> Result<Option<MarketplaceRemoval>, String> {
        unregister_with_bus(
            &self.lock_workspace_repo().await,
            &self.workspace_path,
            &self.artifact_manager,
            &self.event_bus,
            id,
            actor,
        )
        .await
    }
}

#[cfg(test)]
#[path = "../plugins_tests/marketplaces.rs"]
mod tests;
