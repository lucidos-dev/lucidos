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

/// Failure reason recorded for a batch member that was discarded or whose change
/// row vanished before the batch finished — so recovery can mark it terminal and
/// let the batch reach `is_complete()` instead of stalling on a member that can
/// never apply. Also used by `discard_change`'s live `notify_apply_all(Failed …)`
/// so a member discarded mid-batch (e.g. the "≤1 pending change per thread"
/// reconcile dropping a sibling that is itself a batch member) advances the batch
/// instead of stalling it on an `apply_change` that returns `Err` without a
/// terminal event.
pub(crate) const DISCARDED_MEMBER_REASON: &str =
    "Change was discarded or removed before the batch finished";

/// How recovery should treat a batch member, derived from its `changes.status`.
/// `Terminal` covers `discarded`, a missing row, and any unknown status — all
/// must become a terminal `Failed` so the batch can complete; leaving such a
/// member `Pending` would re-drive it into an `apply_change` `Noop` that never
/// emits a terminal event, stalling the batch forever.
#[derive(Debug, PartialEq, Eq)]
enum RecoveredMember {
    Applied,
    Pending,
    Terminal,
}

/// Pure mapping from a member's `changes.status` to its recovery classification.
/// `None` = the change row is gone (treated as terminal).
fn classify_recovered_member(status: Option<&str>) -> RecoveredMember {
    match status {
        Some("applied") => RecoveredMember::Applied,
        Some("pending") => RecoveredMember::Pending,
        _ => RecoveredMember::Terminal,
    }
}

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
                        engine.advance_apply_all_batch(change_id, Err(error)).await;
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
        // Durable mirror of the in-memory registry (see the
        // `apply_all_batches` table migration). Membership only — per-member
        // resolution is reconstructed from `changes.status` on recovery. A
        // failed INSERT degrades to the pre-table behavior (in-memory only, lost
        // on restart) rather than blocking the apply, so log and continue.
        let actor_json = serde_json::to_value(&actor).ok();
        if let Err(e) = sqlx::query(
            "INSERT INTO apply_all_batches (batch_id, change_ids, actor) \
             VALUES ($1, $2, $3) ON CONFLICT (batch_id) DO NOTHING",
        )
        .bind(batch_id)
        .bind(&change_ids)
        .bind(actor_json)
        .execute(self.pool())
        .await
        {
            log!(
                "[ApplyAll] failed to persist batch {} membership: {}",
                batch_id,
                e
            );
        }
        let progress = BatchProgress::new(batch_id, change_ids, actor);
        self.apply_all_batches.lock().await.insert(progress);
        log!("[ApplyAll] batch {} seeded", batch_id);
        batch_id
    }

    /// Delete a batch's durable membership row. Called wherever the batch is
    /// removed from the in-memory registry (driver completion, cancel, startup
    /// recovery) so the table stays a faithful mirror — a leftover row would be
    /// re-recovered on the next boot. Best-effort: a failed DELETE only risks a
    /// harmless re-recovery (which re-emits an idempotent `ApplyAllBatchCompleted`
    /// the frontend already tolerates), so log and move on.
    async fn persist_batch_removed(&self, batch_id: Uuid) {
        if let Err(e) = sqlx::query("DELETE FROM apply_all_batches WHERE batch_id = $1")
            .bind(batch_id)
            .execute(self.pool())
            .await
        {
            log!("[ApplyAll] failed to delete batch {} row: {}", batch_id, e);
        }
    }

    /// Cancel every in-flight Apply All batch — the user clicked Cancel on the
    /// batch toast. For each batch: remove it from the registry first (so the
    /// driver stops advancing to the next member), interrupt the in-flight
    /// member's live coding-agent session (the one currently hardening or
    /// merging), mark every still-pending member canceled so the batch reads as
    /// complete, then emit `ApplyAllBatchCompleted`.
    ///
    /// Semantics: already-applied members stay applied; the in-flight apply
    /// aborts back to pending (best-effort — a merge that already landed before
    /// the interrupt processes still lands and emits `ChangeApplied`, which the
    /// now-removed batch ignores); queued members are left untouched as pending.
    /// Returns the number of batches canceled (0 = nothing was running).
    pub(crate) async fn cancel_apply_all_batches(&self, actor: Option<MessageOrigin>) -> usize {
        // Snapshot pending members and remove the batches under ONE lock so the
        // driver can't spawn a new member's apply between the snapshot and the
        // removal. `get_by_id` / `interrupt_agent` run after the lock is dropped.
        let (pending_by_batch, finals): (Vec<Vec<Uuid>>, Vec<BatchProgress>) = {
            let mut reg = self.apply_all_batches.lock().await;
            let mut pendings = Vec::new();
            let mut finals = Vec::new();
            for batch_id in reg.batch_ids() {
                if let Some(batch) = reg.get_mut(batch_id) {
                    let pending = batch.pending_members();
                    for change_id in &pending {
                        batch.record_failed(*change_id, "Apply All canceled".into());
                    }
                    pendings.push(pending);
                }
                if let Some(final_state) = reg.remove(batch_id) {
                    finals.push(final_state);
                }
            }
            (pendings, finals)
        };
        if finals.is_empty() {
            return 0;
        }
        // Interrupt the in-flight coding-agent session — the one pending member
        // whose thread is mid-harden/merge. The queued members have no live
        // session (interrupt is a lookup miss for them), so this only touches
        // the apply that's actually running.
        for pending in &pending_by_batch {
            for &change_id in pending {
                let thread_id = match self.changes().get_by_id(change_id).await {
                    Ok(Some(c)) => c.thread_id,
                    _ => None,
                };
                if let Some(thread_id) = thread_id {
                    if self.is_agent_running_for(thread_id).await {
                        // Gated on a live session, so the settle fallback is
                        // unreachable from here and its variant is moot.
                        if let Err(e) = self
                            .interrupt_agent(
                                Some(thread_id),
                                actor.clone(),
                                crate::engine::claude_code::SettleTerminal::StuckProjection,
                            )
                            .await
                        {
                            log!(
                                "[ApplyAll] cancel: interrupt_agent({}) failed: {}",
                                thread_id,
                                e
                            );
                        }
                    }
                }
            }
        }
        let count = finals.len();
        for final_state in finals {
            log!(
                "[ApplyAll] batch {} canceled — applied={}, canceled/failed={}",
                final_state.batch_id(),
                final_state.applied_ids().len(),
                final_state.failures().len()
            );
            self.event_bus
                .emit_or_log(
                    BusEvent::System(SystemEvent::ApplyAllBatchCompleted {
                        batch_id: final_state.batch_id(),
                        applied: final_state.applied_ids(),
                        failed: final_state.failures(),
                    }),
                    "[ApplyAll] ApplyAllBatchCompleted (canceled)",
                )
                .await;
            self.persist_batch_removed(final_state.batch_id()).await;
        }
        self.broadcast_changes_updated().await;
        count
    }

    /// Rebuild the Apply-All registry from the durable `apply_all_batches`
    /// table after a restart and resolve any batch the previous process
    /// abandoned. Without this, a batch interrupted by an engine restart
    /// (an earlier member required a restart, or a conflict-resolution apply
    /// landed a restart-requiring change) was lost — the in-memory registry came
    /// back empty, so the eventual `ChangeApplied` / `ChangeApplyFailed` found no
    /// batch in `advance_apply_all_batch`, `ApplyAllBatchCompleted` was never
    /// emitted, and the frontend's "Applying changes…" toast stuck forever.
    ///
    /// Per-member resolution is NOT persisted; it's reconstructed from the
    /// authoritative `changes.status`: `applied` → applied; `discarded` / row
    /// gone → terminal (so the batch can complete); `pending` → re-drive. A
    /// fully-resolved batch emits the missing `ApplyAllBatchCompleted` now; an
    /// in-progress one is re-seeded into the live registry (so any auto-resuming
    /// session's terminal event advances it) and its next pending member is
    /// driven through the idempotent `apply_change` — unless that member's thread
    /// already has a running agent session (its terminal event will advance the
    /// re-seeded batch, so re-driving would risk a double apply).
    ///
    /// MUST run after agent/CC recovery so a member with an auto-resuming session
    /// is observed as running rather than re-driven.
    pub async fn recover_apply_all_batches(self: &std::sync::Arc<Self>) {
        let rows: Vec<(Uuid, Vec<Uuid>, Option<serde_json::Value>)> = match sqlx::query_as(
            "SELECT batch_id, change_ids, actor FROM apply_all_batches ORDER BY created_at",
        )
        .fetch_all(self.pool())
        .await
        {
            Ok(r) => r,
            Err(e) => {
                log!("[ApplyAll] recovery query failed: {}", e);
                return;
            }
        };
        if rows.is_empty() {
            return;
        }
        log!(
            "[ApplyAll] recovering {} unfinished batch(es) from previous process",
            rows.len()
        );

        for (batch_id, change_ids, actor_json) in rows {
            let actor: Option<MessageOrigin> =
                actor_json.and_then(|v| serde_json::from_value(v).ok());
            let mut progress = BatchProgress::new(batch_id, change_ids.clone(), actor.clone());

            // Reconstruct per-member state from the authoritative changes.status.
            for &change_id in &change_ids {
                let status = match self.changes().get_by_id(change_id).await {
                    Ok(Some(c)) => Some(c.status),
                    Ok(None) => None, // row gone → treat as terminal
                    Err(e) => {
                        log!(
                            "[ApplyAll] recovery: changes lookup for {} failed: {} — \
                             treating as pending (will re-drive)",
                            change_id,
                            e
                        );
                        Some("pending".to_string())
                    }
                };
                match classify_recovered_member(status.as_deref()) {
                    RecoveredMember::Applied => {
                        progress.record_applied(change_id);
                    }
                    RecoveredMember::Pending => { /* leave pending — re-drive below */ }
                    RecoveredMember::Terminal => {
                        progress.record_failed(change_id, DISCARDED_MEMBER_REASON.to_string());
                    }
                }
            }

            if progress.is_complete() {
                log!(
                    "[ApplyAll] recovered batch {} already resolved — emitting ApplyAllBatchCompleted",
                    batch_id
                );
                self.event_bus
                    .emit_or_log(
                        BusEvent::System(SystemEvent::ApplyAllBatchCompleted {
                            batch_id,
                            applied: progress.applied_ids(),
                            failed: progress.failures(),
                        }),
                        "[ApplyAll] ApplyAllBatchCompleted (recovery)",
                    )
                    .await;
                self.persist_batch_removed(batch_id).await;
                self.broadcast_changes_updated().await;
                continue;
            }

            // Still in progress — re-seed the live registry, then kick the next
            // pending member (serial, exactly like the driver's ApplyNext arm).
            let next = progress.next_pending();
            self.apply_all_batches.lock().await.insert(progress);
            let Some(change_id) = next else {
                continue;
            };
            let thread_id = match self.changes().get_by_id(change_id).await {
                Ok(Some(c)) => c.thread_id,
                _ => None,
            };
            if let Some(tid) = thread_id {
                if self.is_agent_running_for(tid).await {
                    log!(
                        "[ApplyAll] recovery: batch {} member {} has a running session — \
                         waiting for its terminal event to advance",
                        batch_id,
                        change_id
                    );
                    continue;
                }
            }
            log!(
                "[ApplyAll] recovery: driving pending member {} of batch {}",
                change_id,
                batch_id
            );
            let engine = self.clone_arc();
            tokio::spawn(async move {
                if let Err(e) = engine.apply_change(change_id, actor).await {
                    log!(
                        "[ApplyAll] recovery: apply_change({change_id}) returned Err: {e} — \
                         waiting for ChangeApplyFailed to advance the batch",
                    );
                }
            });
        }
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
            let batch = reg
                .get_mut(batch_id)
                .expect("batch_for_change just found it");
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
                let final_state = reg.remove(batch_id).expect("just confirmed via get_mut");
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
                self.persist_batch_removed(batch_id).await;
                self.broadcast_changes_updated().await;
            }
            NextStep::ApplyNext { next_change, actor } => {
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The load-bearing classification: a discarded / removed member MUST be
    /// terminal so the batch can complete; only an explicitly `pending` member
    /// stays pending (and gets re-driven). Misclassifying `discarded` as
    /// `pending` re-drives it into an `apply_change` `Noop` that never emits a
    /// terminal event — the batch stalls and the toast sticks forever.
    #[test]
    fn classify_member_status_mapping() {
        assert_eq!(
            classify_recovered_member(Some("applied")),
            RecoveredMember::Applied
        );
        assert_eq!(
            classify_recovered_member(Some("pending")),
            RecoveredMember::Pending
        );
        assert_eq!(
            classify_recovered_member(Some("discarded")),
            RecoveredMember::Terminal
        );
        // Missing row (None) and any unexpected status are terminal.
        assert_eq!(classify_recovered_member(None), RecoveredMember::Terminal);
        assert_eq!(
            classify_recovered_member(Some("weird-future-status")),
            RecoveredMember::Terminal
        );
    }

    /// Reconstructing a batch where every member resolved (applied or
    /// discarded) yields a complete batch — recovery emits the missing
    /// `ApplyAllBatchCompleted` for it.
    #[test]
    fn fully_resolved_batch_reconstructs_as_complete() {
        let ids: Vec<Uuid> = (0..3).map(|_| Uuid::new_v4()).collect();
        let mut progress = BatchProgress::new(Uuid::new_v4(), ids.clone(), None);
        // applied, applied, discarded → all terminal.
        for (i, &id) in ids.iter().enumerate() {
            match classify_recovered_member(if i < 2 { Some("applied") } else { None }) {
                RecoveredMember::Applied => {
                    progress.record_applied(id);
                }
                RecoveredMember::Terminal => {
                    progress.record_failed(id, DISCARDED_MEMBER_REASON.to_string());
                }
                RecoveredMember::Pending => unreachable!(),
            }
        }
        assert!(progress.is_complete());
        assert_eq!(progress.applied_ids(), vec![ids[0], ids[1]]);
        assert_eq!(progress.failures().len(), 1);
        assert_eq!(progress.failures()[0].error, DISCARDED_MEMBER_REASON);
    }

    /// A batch with a still-`pending` member reconstructs as incomplete, and
    /// `next_pending` points at the first pending member — the one recovery
    /// drives through `apply_change`.
    #[test]
    fn batch_with_pending_member_reconstructs_incomplete_and_drives_next() {
        let ids: Vec<Uuid> = (0..3).map(|_| Uuid::new_v4()).collect();
        let mut progress = BatchProgress::new(Uuid::new_v4(), ids.clone(), None);
        // applied, pending, pending.
        progress.record_applied(ids[0]);
        // ids[1], ids[2] left pending.
        assert!(!progress.is_complete());
        assert_eq!(progress.next_pending(), Some(ids[1]));
    }
}
