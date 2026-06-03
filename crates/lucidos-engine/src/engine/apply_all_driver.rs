//! Engine-side driver for `apply_all_batches::ApplyAllRegistry`.
//!
//! Architecture: a single background task receives `ApplyAllDriveMsg` events
//! over an mpsc channel and advances the registry. `emit_change_applied` /
//! `emit_apply_failed` push onto the channel via `notify_apply_all`. The
//! driver task owns the registry interaction (lock, advance, spawn next).
//!
//! The channel is what breaks the recursive call cycle that would otherwise
//! exist: `apply_change` → `emit_change_applied` → driver → `apply_change`.
//! Async functions in such a cycle can't be auto-trait-checked for `Send`
//! without `Box::pin`; the channel decouples the cycle into independent
//! tasks.
//!
//! Why "advance then spawn next" instead of "drive in a single loop": the
//! driver task should never block waiting for an apply to complete (a
//! conflict-resolution CC can run for minutes). Spawning the apply lets the
//! driver immediately return to listening for the next ChangeApplied, so
//! parallel batches are possible if a future UI feature wants them.

use uuid::Uuid;

use crate::engine::apply_all_batches::{ApplyFailure, BatchProgress};
use crate::engine::event_bus::{BusEvent, SystemEvent};
use crate::engine::thread_events::MessageOrigin;
use crate::engine::LucidosEngine;

/// Messages from `emit_change_applied` / `emit_apply_failed` to the
/// apply-all driver task.
#[derive(Debug, Clone)]
pub(crate) enum ApplyAllDriveMsg {
    Applied(Uuid),
    Failed(Uuid, String),
}

impl LucidosEngine {
    /// Push a "change resolved" notification to the apply-all driver task.
    /// No-op when the channel send fails (driver task already shut down) —
    /// the batch is then orphaned in memory but the persisted
    /// `ApplyAllBatchStarted` event still lets recovery resume on restart.
    /// Non-blocking; never holds the registry lock.
    pub(crate) fn notify_apply_all(&self, msg: ApplyAllDriveMsg) {
        if let Err(e) = self.apply_all_drive_tx.send(msg) {
            log!(
                "[ApplyAll] notify channel closed — dropping {:?}; recovery on \
                 restart will resume from the persisted ApplyAllBatchStarted",
                e.0,
            );
        }
    }

    /// Start the apply-all driver task. Spawned once at engine startup.
    /// The receiver was stashed in `APPLY_ALL_DRIVE_RX` during
    /// `LucidosEngine::new`; this method takes it out (one-shot) and feeds
    /// it to the driver loop. Mirrors the `start_parent_callback_listener`
    /// pattern for symmetry.
    pub fn start_apply_all_driver(self: &std::sync::Arc<Self>) {
        let rx = crate::engine::APPLY_ALL_DRIVE_RX.with(|cell| cell.borrow_mut().take());
        let Some(mut rx) = rx else {
            log!("[ApplyAll] driver receiver missing — listener not started");
            return;
        };
        let engine = self.clone();
        tokio::spawn(async move {
            log!("[ApplyAll] driver task started");
            while let Some(msg) = rx.recv().await {
                match msg {
                    ApplyAllDriveMsg::Applied(change_id) => {
                        engine.advance_apply_all_batch(change_id, Ok(())).await;
                    }
                    ApplyAllDriveMsg::Failed(change_id, error) => {
                        engine
                            .advance_apply_all_batch(change_id, Err(error))
                            .await;
                    }
                }
            }
            log!("[ApplyAll] driver task exiting — channel closed");
        });
    }

    /// Seed a new Apply All batch. Emits the durable `ApplyAllBatchStarted`
    /// event (recoverable on restart) and adds the live batch to the
    /// in-memory registry. Returns the batch_id so the HTTP handler can
    /// surface it to the caller.
    ///
    /// The first apply is fired by the HTTP handler itself (synchronously,
    /// so the caller gets a useful response). Subsequent applies flow
    /// through the driver task via `notify_apply_all`.
    pub(crate) async fn start_apply_all_batch(
        &self,
        change_ids: Vec<Uuid>,
        actor: Option<MessageOrigin>,
    ) -> Uuid {
        debug_assert!(
            !change_ids.is_empty(),
            "start_apply_all_batch: change_ids must be non-empty",
        );
        let batch_id = Uuid::new_v4();
        self.event_bus
            .emit_or_log(
                BusEvent::System(SystemEvent::ApplyAllBatchStarted {
                    batch_id,
                    change_ids: change_ids.clone(),
                    actor: actor.clone(),
                }),
                "[ApplyAll] ApplyAllBatchStarted",
            )
            .await;
        let progress = BatchProgress::new(batch_id, change_ids, actor);
        self.apply_all_batches.lock().await.insert(progress);
        log!("[ApplyAll] batch {} seeded", batch_id);
        batch_id
    }

    /// Update the registry for one resolved change and decide what to do
    /// next. Called only from the driver task. Holds the registry lock just
    /// long enough to inspect + mutate, then releases before emitting the
    /// completion event or spawning the next apply.
    async fn advance_apply_all_batch(&self, change_id: Uuid, result: Result<(), String>) {
        let next_step = {
            let mut reg = self.apply_all_batches.lock().await;
            let Some(batch_id) = reg.batch_for_change(change_id) else {
                return;
            };
            let batch = reg.get_mut(batch_id).expect("batch_for_change just found it");
            let state_changed = match result {
                Ok(()) => batch.record_applied(change_id),
                Err(error) => batch.record_failed(change_id, error),
            };
            // Duplicate terminal event for an already-resolved member (e.g.
            // the conflict-recovery cleanup emits `ChangeApplied` and the
            // post-CC merge re-check then emits `ChangeApplyFailed` for the
            // same change_id). Re-running next_pending here would re-spawn
            // apply_change on the next member, racing the in-flight call.
            if !state_changed {
                log!(
                    "[ApplyAll] duplicate terminal event for {} in batch {} — skipping advance",
                    change_id,
                    batch_id,
                );
                return;
            }
            if batch.is_complete() {
                let final_state = reg
                    .remove(batch_id)
                    .expect("just confirmed via get_mut");
                NextStep::Complete {
                    batch_id,
                    applied: final_state.applied_ids(),
                    failed: final_state.failures(),
                }
            } else if let Some(next) = batch.next_pending() {
                NextStep::ApplyNext {
                    next_change: next,
                    actor: batch.actor(),
                }
            } else {
                // Should be unreachable — `!is_complete()` means
                // `next_pending().is_some()`. Defensive log keeps the door
                // shut on a future refactor where the two methods drift.
                log!(
                    "[ApplyAll] inconsistency: batch {batch_id} is not complete \
                     but next_pending is None — leaving batch in registry",
                );
                NextStep::Nothing
            }
        };
        match next_step {
            NextStep::Nothing => {}
            NextStep::Complete {
                batch_id,
                applied,
                failed,
            } => {
                log!(
                    "[ApplyAll] batch {} complete — applied={}, failed={}",
                    batch_id,
                    applied.len(),
                    failed.len()
                );
                self.event_bus
                    .emit_or_log(
                        BusEvent::System(SystemEvent::ApplyAllBatchCompleted {
                            batch_id,
                            applied,
                            failed,
                        }),
                        "[ApplyAll] ApplyAllBatchCompleted",
                    )
                    .await;
                self.broadcast_changes_updated().await;
            }
            NextStep::ApplyNext {
                next_change,
                actor,
            } => {
                log!(
                    "[ApplyAll] advancing batch to next change {} (after {})",
                    next_change,
                    change_id
                );
                let engine = self.clone_arc();
                tokio::spawn(async move {
                    if let Err(e) = engine.apply_change(next_change, actor).await {
                        log!(
                            "[ApplyAll] apply_change({next_change}) returned Err: {e} — \
                             the inner apply path should have emitted ChangeApplyFailed; \
                             waiting for that to advance the batch",
                        );
                    }
                });
            }
        }
    }
}

/// What `advance_apply_all_batch` decided to do once it released the
/// registry lock. Splitting the decision from the action keeps the lock
/// scope tight and makes the control flow readable.
enum NextStep {
    Nothing,
    Complete {
        batch_id: Uuid,
        applied: Vec<Uuid>,
        failed: Vec<ApplyFailure>,
    },
    ApplyNext {
        next_change: Uuid,
        actor: Option<MessageOrigin>,
    },
}
