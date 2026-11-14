//! The bus consumer that runs the compactor for a Tree workspace and keeps its
//! queue current.
//!
//! A thread event that can change a thread's log marks that thread dirty. A
//! turn end or an artifact write marks the workspace dirty. A thread delete
//! tells the compactor that workspace positions moved. It also re-marks each
//! deleted thread, so a node a drain wrote during the delete is swept.
//!
//! A raised backfill cap re-seeds the compactor, and a cap reached tells the
//! user with a notification. A change of the compactor's model wakes the
//! nodes waiting on a failing one.

use std::sync::Arc;

use super::compactor::{CompactionModel, Compactor, CompactorDeps};
use super::compactor_models::compactor_route;
use super::log::{ENTRY_EVENT_TYPES, TURN_END_EVENT_TYPES};
use super::module::{tree_chosen, MemoryModule};
use super::spend::StartEstimate;
use super::store;
use super::SummaryScope;
use crate::engine::aux_purpose::SUMMARY_COMPACTION_PREFS;
use crate::engine::event_bus::{BusEvent, EmittedEvent, LagReplay, ReplayScope, SystemEvent};
use crate::engine::thread_events::MessageOrigin;
use crate::engine::{ContextPurpose, LucidosEngine};
use crate::scheduler::notifications::SettingsPage;

/// Run the compactor while the workspace is on the Tree memory module. It
/// starts at boot when `memory_module` reads `tree`, and starts, pauses or
/// resumes when the preference changes. A Classic workspace, or one that never
/// chose, makes no compactor call beyond the few in flight when it paused.
pub fn spawn(engine: Arc<LucidosEngine>) {
    // Subscribe before reading the preference, so a change landing in between
    // is not missed.
    let (rx, replay) = LagReplay::subscribe(&engine.event_bus, ReplayScope::Workspace, None);
    tokio::spawn(async move {
        let mut running = None;
        if tree_chosen(engine.pool()).await {
            running = Some(start(&engine, None));
        }
        listen(rx, &engine, running, replay).await;
    });
}

fn start(engine: &Arc<LucidosEngine>, starter: Option<MessageOrigin>) -> Arc<Compactor> {
    log!("[SummaryTree] The workspace is on the Tree memory module, starting the compactor");
    let compactor = Compactor::new(
        engine.pool().clone(),
        engine.event_bus.clone(),
        Arc::new(EngineDeps(engine.clone())),
    );
    compactor.set_starter(starter);
    engine.summary_tree().set(compactor.clone());
    compactor.start();
    compactor
}

/// Whether `key` picks the compactor's model or its reasoning effort.
fn is_compactor_model_pref(key: &str) -> bool {
    key == SUMMARY_COMPACTION_PREFS.model.key() || key == SUMMARY_COMPACTION_PREFS.reasoning.key()
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

/// Who wrote the preference change in `emitted`.
fn switch_actor(emitted: &EmittedEvent) -> Option<MessageOrigin> {
    match &emitted.typed {
        BusEvent::System(SystemEvent::PreferencesChanged { actor, .. }) => actor.clone(),
        _ => None,
    }
}

async fn listen(
    mut rx: tokio::sync::broadcast::Receiver<EmittedEvent>,
    engine: &Arc<LucidosEngine>,
    mut running: Option<Arc<Compactor>>,
    mut replay: LagReplay,
) {
    // Hold announcements are durable too: replay a lag rather than dropping
    // the one event that owns the provider's notification.
    while let Some(frames) = replay.read(rx.recv().await, engine.pool()).await {
        for emitted in frames {
            if let Some(module) = switched_to(&emitted) {
                let tree = module == MemoryModule::Tree;
                running = switch(engine, running, tree, switch_actor(&emitted)).await;
                continue;
            }
            if let BusEvent::System(SystemEvent::ModelSpendCapReached {
                provider,
                resets_at,
                ..
            }) = &emitted.typed
            {
                notify_spend_cap(engine, provider, *resets_at).await;
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
                BusEvent::System(SystemEvent::PreferencesChanged { key, .. })
                    if is_compactor_model_pref(key) =>
                {
                    compactor.model_changed().await
                }
                BusEvent::System(SystemEvent::ModelSpendCapHoldEnded { .. }) => {
                    compactor.spend_cap_hold_ended().await;
                }
                BusEvent::System(SystemEvent::TreeBackfillCapRaised { .. }) => {
                    compactor.cap_raised().await
                }
                BusEvent::System(SystemEvent::TreeBackfillCapReached { spent_usd, cap_usd }) => {
                    notify_cap_reached(engine, *spent_usd, *cap_usd).await
                }
                _ => {}
            }
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
    starter: Option<MessageOrigin>,
) -> Option<Arc<Compactor>> {
    match (running, tree) {
        (None, true) => Some(start(engine, starter)),
        (Some(compactor), true) => {
            if compactor.is_paused() {
                log!("[SummaryTree] The workspace is back on the Tree memory module, resuming the compactor");
                compactor.set_starter(starter);
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

/// Tell the user the backfill paused at its cap, and where to continue it.
async fn notify_cap_reached(engine: &LucidosEngine, spent_usd: f64, cap_usd: f64) {
    let page = SettingsPage::MEMORY;
    let message = format!(
        "Building Tree memory has spent ${spent_usd:.2} of its ${cap_usd:.2} cap, so it paused. \
         Threads it already summarized keep up with new messages. Continue it, or switch back \
         to Classic, under {}.",
        page.link()
    );
    if let Err(e) = engine
        .create_notification(
            "Tree memory paused at its spending cap",
            &message,
            None,
            None,
            None,
            page.tap(),
            None,
        )
        .await
    {
        log!(
            "[SummaryTree] Could not notify that the backfill cap was reached: {}",
            e
        );
    }
}

/// Tell the user the compactor's model has not answered for a long while,
/// and where to pick another.
async fn notify_model_down(engine: &LucidosEngine, model: &str) {
    let page = SettingsPage::MEMORY;
    let message = format!(
        "{model} has not answered for several minutes, so new summaries wait. Pick another \
         model for Tree memory under {}.",
        page.link()
    );
    if let Err(e) = engine
        .create_notification(
            "Tree memory can't reach its model",
            &message,
            None,
            None,
            None,
            page.tap(),
            None,
        )
        .await
    {
        log!(
            "[SummaryTree] Could not notify that {} is not answering: {}",
            model,
            e
        );
    }
}

struct EngineDeps(Arc<LucidosEngine>);

#[async_trait::async_trait]
impl CompactorDeps for EngineDeps {
    /// The router, with the compactor's model and attempt cap pinned, so any
    /// configured backend serves it.
    async fn model(&self) -> Result<CompactionModel, String> {
        let call = self.0.aux_call(ContextPurpose::SummaryCompaction).await;
        let configured = self.0.configured_provider_kinds();
        let route = compactor_route(
            call.running_model(),
            call.reasoning(),
            self.0.model_registry(),
            &configured,
        )?;
        Ok(CompactionModel {
            provider: call.provider(),
            model: call.running_model().to_string(),
            effort: route.effort,
            deadline: call.deadline(),
            lane: route.lane,
            flex: route.flex,
        })
    }

    async fn backfill_estimate(&self) -> Result<StartEstimate, String> {
        let model = self.model().await?;
        let estimate = super::tree_backfill_estimate(&self.0)
            .await
            .map_err(|e| format!("could not estimate the backfill: {e}"))?;
        let cost = model
            .effort
            .as_deref()
            .and_then(|effort| estimate.cost_for(&model.model, effort))
            .cloned();
        Ok(StartEstimate {
            model: model.model,
            effort: model.effort,
            calls: estimate.calls,
            cost,
        })
    }

    async fn notify_model_down(&self, model: &str) {
        notify_model_down(&self.0, model).await
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

async fn notify_spend_cap(
    engine: &LucidosEngine,
    provider: &str,
    resets_at: Option<chrono::DateTime<chrono::Utc>>,
) {
    let page = SettingsPage::PROVIDERS;
    let reset = match resets_at {
        Some(reset) => {
            let tz = engine
                .user_timezone()
                .await
                .parse::<chrono_tz::Tz>()
                .unwrap_or(chrono_tz::UTC);
            format!(
                "Resets at {}.",
                reset.with_timezone(&tz).format("%Y-%m-%d %H:%M")
            )
        }
        None => "The provider did not supply a reset time.".into(),
    };
    let message = format!("{provider} reached its spend cap. {reset} Background calls wait. Use Try now after raising the cap under {}.", page.link());
    if let Err(error) = engine
        .create_notification(
            "Provider spend cap reached",
            &message,
            None,
            None,
            None,
            page.tap(),
            None,
        )
        .await
    {
        log!(
            "[SummaryTree] Could not announce provider spend cap: {}",
            error
        );
    }
}
