//! The single write path for the plugin marketplace registry
//! (`data/config/plugin-marketplaces.json`).
//!
//! Registration used to be two independent copies of the same eight steps: one
//! in `api::plugins::add_marketplace_handler` and one in the `plugins` tool's
//! `register_marketplace` action. Both took the repo lock, loaded, upserted,
//! saved, committed, emitted `DataFileWritten`, and spawned the update check.
//! Neither emitted an entity event, so nothing told an open Plugins panel that
//! the list had changed, and the panel showed a stale list (a missing
//! marketplace, or the old name after a rename) until a manual reload.
//!
//! Adding the emit to both call sites would have fixed today's bug and left the
//! next call site free to forget it, which is exactly the failure
//! `core::announced_surfaces` was written about after the `manage_repositories`
//! tool wrote a `repositories` row and announced nothing. So the emit lives
//! *inside* the write path here, and the entry points call it rather than
//! reassembling the sequence:
//!
//! - [`register_with_bus`] / [`unregister_with_bus`] are the cores: load,
//!   mutate, save, commit, announce. They take their collaborators rather than
//!   `&LucidosEngine` (which cannot be constructed in a unit test) so the
//!   announcement is directly assertable against `MockEventBus`.
//! - [`LucidosEngine::register_plugin_marketplace`] /
//!   [`LucidosEngine::unregister_plugin_marketplace`] add the two things that
//!   need the live engine: the workspace repo lock, and the post-registration
//!   marketplace update-check pass.

use std::path::Path;

use crate::core::plugin_marketplaces::{
    add_marketplace, load_registry, remove_marketplace, save_registry, PluginMarketplace,
    MARKETPLACES_DATA_PATH,
};
use crate::core::ArtifactManager;
use crate::engine::event_bus::{BusEvent, EventBusEmitter, SystemEvent};
use crate::engine::thread_events::MessageOrigin;
use crate::engine::LucidosEngine;

/// A registration that landed. `created` distinguishes a first registration
/// from a re-registration of the same source (the rename path), which is what
/// picks the commit message and what the HTTP/tool responses report.
pub(crate) struct MarketplaceRegistration {
    pub marketplace: PluginMarketplace,
    pub marketplaces: Vec<PluginMarketplace>,
    pub created: bool,
    pub commit: String,
}

/// A removal that landed. A removal of an id that was never registered is
/// `Ok(None)` from [`unregister_with_bus`], not an error, so the 404 the HTTP
/// handler owes does not have to be recovered by sniffing an error string.
pub(crate) struct MarketplaceRemoval {
    pub marketplaces: Vec<PluginMarketplace>,
    pub commit: String,
}

/// Why a registration failed. A typed split rather than the usual boxed error
/// because the HTTP handler *branches* on it (400 for a source the user typed
/// wrong, 500 for a registry the engine could not read/write/commit), which is
/// the structural consumption `.claude/rules/rust.md` asks for before a custom
/// error type is worth it. The alternative in this file's own neighbourhood is
/// `api::plugins::pending_status`, which recovers the same distinction by
/// matching on a message prefix.
#[derive(Debug)]
pub(crate) enum MarketplaceWriteError {
    /// `source` is empty, or is not a git / GitHub tree URL.
    InvalidSource(String),
    /// The registry could not be read, written, or committed.
    Failed(String),
}

impl std::fmt::Display for MarketplaceWriteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidSource(msg) | Self::Failed(msg) => write!(f, "{msg}"),
        }
    }
}

/// Fire-and-forget emit for the `&dyn EventBusEmitter` seam, mirroring
/// `EventBus::emit_or_log` (an inherent method on the concrete bus, which this
/// path deliberately does not take). The registry write has already committed
/// by the time we get here, so a broadcast failure is logged, never turned into
/// a write failure: `core::announced_surfaces` states that the announcement
/// guarantee is reachability, not atomicity.
async fn announce(bus: &dyn EventBusEmitter, event: BusEvent, ctx: &str) {
    if let Err(e) = bus.emit(event).await {
        log!("[EventBus] {} emit failed: {}", ctx, e);
    }
}

/// Register `source`, or re-register it under a new `name`. An upsert: the
/// marketplace id is a hash of the canonical source, so registering a source
/// that is already present rewrites its name and raw source string in place
/// and reports `created: false`. Both outcomes announce
/// `PluginMarketplaceRegistered` (see the variant's doc for why the rename is
/// not its own event).
pub(crate) async fn register_with_bus(
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

/// Unregister the marketplace with `id`. `Ok(None)` when no such marketplace is
/// registered: nothing is written, committed, or announced.
pub(crate) async fn unregister_with_bus(
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

    let commit = artifact_manager
        .commit_data_path(MARKETPLACES_DATA_PATH, "Remove plugin marketplace")
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

/// The file-level audit event for the registry commit, emitted alongside the
/// entity event on every mutation. Kept because it is what carries the commit
/// sha for a `data/` write, the same pairing the `Artifact*` entity events have
/// with their `DataFile*` audit event.
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

impl LucidosEngine {
    /// Register or rename a plugin marketplace, under the workspace repo lock,
    /// then kick off a marketplace scan / update-check pass in the background
    /// (it notifies about newly-available plugin updates, it never installs).
    ///
    /// Both the `POST /api/v1/plugins/marketplaces` handler and the `plugins`
    /// tool's `register_marketplace` action go through here, so the announcement
    /// cannot be skipped by an entry point.
    pub(crate) async fn register_plugin_marketplace(
        &self,
        source: &str,
        name: Option<&str>,
        actor: Option<MessageOrigin>,
    ) -> Result<MarketplaceRegistration, MarketplaceWriteError> {
        let registration = {
            let _repo_guard = self.lock_workspace_repo().await;
            register_with_bus(
                &self.workspace_path,
                &self.artifact_manager,
                &self.event_bus,
                source,
                name,
                actor,
            )
            .await?
        };

        let update_engine = self.clone_arc();
        let update_pool = self.pool.clone();
        tokio::spawn(async move {
            crate::scheduler::plugin_updates::run_plugin_marketplace_update_check(
                update_engine,
                update_pool,
            )
            .await;
        });

        Ok(registration)
    }

    /// Unregister a plugin marketplace, under the workspace repo lock.
    /// `Ok(None)` when the id was not registered. No update-check pass: a
    /// removal can only shrink the catalog, so there is nothing new to find.
    pub(crate) async fn unregister_plugin_marketplace(
        &self,
        id: &str,
        actor: Option<MessageOrigin>,
    ) -> Result<Option<MarketplaceRemoval>, String> {
        let _repo_guard = self.lock_workspace_repo().await;
        unregister_with_bus(
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
