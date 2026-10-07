//! The bus consumer that runs the compactor for a Tree workspace and keeps its
//! queue current.
//!
//! A thread event that can change a thread's log marks that thread dirty. A
//! turn end or an artifact write marks the workspace dirty. A thread delete
//! tells the compactor that workspace positions moved. It also re-marks each
//! deleted thread, so a node a drain wrote during the delete is swept.

use std::sync::Arc;

use tokio_stream::wrappers::errors::BroadcastStreamRecvError;
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::StreamExt;

use super::compactor::{CompactionModel, Compactor, CompactorDeps};
use super::compactor_models::compactor_lane;
use super::log::{ENTRY_EVENT_TYPES, TURN_END_EVENT_TYPES};
use super::module::MemoryModule;
use super::store;
use super::SummaryScope;
use crate::engine::event_bus::{BusEvent, EmittedEvent, SystemEvent};
use crate::engine::{ContextPurpose, LucidosEngine};
use crate::llm::model_registry::ProviderKind;

/// Run the compactor while the workspace is on the Tree memory module. It
/// starts at boot when `memory_module` reads `tree`, and starts, pauses or
/// resumes when the preference changes. A Classic workspace makes no compactor
/// call beyond the few already in flight when it paused.
pub fn spawn(engine: Arc<LucidosEngine>) {
    // Subscribe before reading the preference, so a change landing in between
    // is not missed.
    let rx = engine.event_bus.subscribe();
    tokio::spawn(async move {
        let mut running = None;
        if MemoryModule::chosen(engine.pool()).await == MemoryModule::Tree {
            running = Some(start(&engine));
        }
        listen(rx, &engine, running).await;
    });
}

fn start(engine: &Arc<LucidosEngine>) -> Arc<Compactor> {
    log!("[SummaryTree] The workspace is on the Tree memory module, starting the compactor");
    let compactor = Compactor::new(
        engine.pool().clone(),
        engine.event_bus.clone(),
        Arc::new(EngineDeps(engine.clone())),
    );
    engine.summary_tree().set(compactor.clone());
    compactor.start();
    compactor
}

/// Whether an event switches the module, and to which.
pub(super) fn switched_to(emitted: &EmittedEvent) -> Option<MemoryModule> {
    match &emitted.typed {
        BusEvent::System(SystemEvent::PreferencesChanged { key, value, .. })
            if key == crate::core::prefs::MEMORY_MODULE.key() =>
        {
            Some(MemoryModule::from_pref(value.as_deref()))
        }
        _ => None,
    }
}

async fn listen(
    rx: tokio::sync::broadcast::Receiver<EmittedEvent>,
    engine: &Arc<LucidosEngine>,
    mut running: Option<Arc<Compactor>>,
) {
    let stream = BroadcastStream::new(rx);
    tokio::pin!(stream);
    // A lag must not end the loop: the missed events stay in the table.
    while let Some(result) = stream.next().await {
        let emitted = match result {
            Ok(e) => e,
            // The missed events may have dirtied any scope, or switched the
            // module, so look again.
            Err(BroadcastStreamRecvError::Lagged(n)) => {
                log!("[SummaryTree] Broadcast lagged by {} events; re-seeding", n);
                let tree = MemoryModule::chosen(engine.pool()).await == MemoryModule::Tree;
                running = switch(engine, running, tree).await;
                if let Some(compactor) = &running {
                    compactor.forget_workspace();
                    if let Err(e) = compactor.seed().await {
                        log!("[SummaryTree] Could not re-seed after the lag: {}", e);
                    }
                }
                continue;
            }
        };
        if let Some(module) = switched_to(&emitted) {
            running = switch(engine, running, module == MemoryModule::Tree).await;
            continue;
        }
        let Some(compactor) = &running else {
            continue;
        };
        if emitted.seq.is_none() {
            continue;
        }
        match &emitted.typed {
            BusEvent::Thread {
                thread_id, event, ..
            } => {
                let event_type = event.event_type();
                let ends_turn = TURN_END_EVENT_TYPES.contains(&event_type);
                if ends_turn || ENTRY_EVENT_TYPES.contains(&event_type) {
                    compactor.mark_dirty(SummaryScope::Thread(*thread_id), true);
                }
                if ends_turn {
                    compactor.mark_dirty(SummaryScope::Workspace, true);
                }
            }
            BusEvent::System(
                SystemEvent::ArtifactCreated { .. }
                | SystemEvent::ArtifactUpdated { .. }
                | SystemEvent::ArtifactImported { .. },
            ) => compactor.mark_dirty(SummaryScope::Workspace, true),
            BusEvent::System(SystemEvent::ThreadsDeleted { thread_ids, .. }) => {
                compactor.forget_threads(thread_ids)
            }
            _ => {}
        }
    }
    log!("[SummaryTree] Compactor event stream ended");
}

/// Start, pause or resume the compactor to match the module. One compactor
/// serves the engine's lifetime, so a scope never has two drains.
async fn switch(
    engine: &Arc<LucidosEngine>,
    running: Option<Arc<Compactor>>,
    tree: bool,
) -> Option<Arc<Compactor>> {
    match (running, tree) {
        (None, true) => Some(start(engine)),
        (Some(compactor), true) => {
            if compactor.is_paused() {
                log!("[SummaryTree] The workspace is back on the Tree memory module, resuming the compactor");
                compactor.resume().await;
            }
            Some(compactor)
        }
        (running, false) => {
            if let Some(compactor) = running.as_ref().filter(|c| !c.is_paused()) {
                log!("[SummaryTree] The workspace left the Tree memory module, pausing the compactor");
                compactor.pause();
            }
            clear_ready_flag(engine.pool(), &engine.event_bus).await;
            running
        }
    }
}

/// Clear the ready flag, announcing it only when it was set.
pub(super) async fn clear_ready_flag(
    pool: &sqlx::PgPool,
    bus: &crate::engine::event_bus::EventBus,
) {
    match store::clear_ready(pool).await {
        Ok(true) => {
            bus.emit_or_log(
                BusEvent::System(SystemEvent::TreeBackfillReset {}),
                "[SummaryTree] TreeBackfillReset",
            )
            .await
        }
        Ok(false) => {}
        Err(e) => log!("[SummaryTree] Could not clear the ready flag: {}", e),
    }
}

/// The providers this engine's router holds. A router that reports no list
/// filters nothing, so every provider counts.
pub(super) fn configured_providers(engine: &LucidosEngine) -> Vec<ProviderKind> {
    engine
        .current_provider()
        .configured_providers()
        .unwrap_or_else(|| ProviderKind::ALL.to_vec())
}

struct EngineDeps(Arc<LucidosEngine>);

#[async_trait::async_trait]
impl CompactorDeps for EngineDeps {
    /// The router, with the compactor's model and attempt cap pinned, so any
    /// configured backend serves it.
    async fn model(&self) -> Result<CompactionModel, String> {
        let call = self.0.aux_call(ContextPurpose::SummaryCompaction).await;
        let configured = configured_providers(&self.0);
        let lane = compactor_lane(call.model(), self.0.model_registry(), &configured)?;
        Ok(CompactionModel {
            provider: call.provider(),
            model: call.model().to_string(),
            effort: call.reasoning().map(str::to_string),
            deadline: call.deadline(),
            lane,
        })
    }

    fn read_artifact(&self, path: &str, commit: &str) -> Option<String> {
        match self
            .0
            .artifact_manager
            .read_artifact_at_commit_string(path, commit)
        {
            Ok(content) => Some(content),
            Err(e) => {
                log!(
                    "[SummaryTree] Could not read {} at {}, so its leaf names the write only: {}",
                    path,
                    commit,
                    e
                );
                None
            }
        }
    }
}
