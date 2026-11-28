use uuid::Uuid;

use crate::core::changes::ChangeStatus;
use crate::engine::apply_all_batches::{Advance, ApplyFailure, BatchProgress};
use crate::engine::event_bus::{BusEvent, SystemEvent};
use crate::engine::thread_events::MessageOrigin;
use crate::engine::LucidosEngine;

/// INVARIANT: Failure reason for a batch member that left `pending` before the batch
/// finished: set aside, discarded, or its row gone. Recovery marks it terminal,
/// so the batch reaches `is_complete()` instead of stalling on a member that can
/// never apply. Also used by `discard_change`'s live `notify_apply_all(Failed …)`
/// so a member discarded mid-batch (e.g. the "≤1 pending change per thread"
/// reconcile dropping a sibling that is itself a batch member) advances the batch
/// instead of stalling it on an `apply_change` that returns `Err` without a
/// terminal event.
pub(crate) const WITHDRAWN_MEMBER_REASON: &str =
    "Change was set aside, discarded or removed before the batch finished";

const MEMBER_THREAD_UNSETTLED: &str = "Its thread was working again when its turn in the batch \
     came. Apply it once the thread settles.";
const MEMBER_SETTLE_UNKNOWN: &str = "Lucidos could not check whether its thread had settled, so \
     it was not applied. Apply it again.";
const MEMBER_EMPTY: &str = "It has no file changes left.";

/// INVARIANT: Why the driver must not apply a pending `change` when its turn comes, or
/// `None` to apply it. This is `run_apply_all`'s opening filter, asked again:
/// a later member's turn can come minutes after the press.
///
/// A change that left `pending` goes ahead, because `apply_change` answers it
/// with the terminal event the batch waits on.
fn batch_member_skip_reason(
    change: &crate::core::changes::Change,
    thread_unsettled: bool,
) -> Option<&'static str> {
    if !change.is_pending() {
        None
    } else if thread_unsettled {
        Some(MEMBER_THREAD_UNSETTLED)
    } else if crate::core::changes::is_empty_pending_change(change) {
        Some(MEMBER_EMPTY)
    } else {
        None
    }
}

type PersistedBatch = (Uuid, Vec<Uuid>, Option<serde_json::Value>);

/// INVARIANT: End archived Apply All ownership before boot subscribers or archive
/// recovery can dispatch. Eligible fresh rows remain for normal reconstruction.
pub(crate) async fn settle_restored_batches(
    pool: &sqlx::PgPool,
    bus: &crate::engine::event_bus::EventBus,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    if crate::core::snapshot::boundary::read(pool).await?.is_some() {
        recoverable_batches(pool, bus).await?;
    }
    Ok(())
}

/// INVARIANT: Read the durable admission set before reconstructing any batch owners.
/// An archived press is a canceled request, not permission to apply restored
/// changes. Announce before removing its row; failed writes refuse admission
/// and retain the row for the next boot. A crash after emit retries only removal.
async fn recoverable_batches(
    pool: &sqlx::PgPool,
    bus: &crate::engine::event_bus::EventBus,
) -> Result<Vec<PersistedBatch>, Box<dyn std::error::Error + Send + Sync>> {
    let boundary = crate::core::snapshot::boundary::read(pool).await?;
    let rows: Vec<PersistedBatch> = sqlx::query_as(
        "SELECT batch_id, change_ids, actor FROM apply_all_batches ORDER BY created_at",
    )
    .fetch_all(pool)
    .await?;
    let Some(boundary) = boundary else {
        return Ok(rows);
    };
    let mut eligible = Vec::new();
    for row in rows {
        // INVARIANT: A later duplicate start cannot renew the original owner's press.
        let first: Option<i64> = sqlx::query_scalar(
            "SELECT MIN(sequence) FROM events WHERE event_type = 'ApplyAllBatchStarted' AND aggregate_id = $1")
            .bind(row.0.to_string()).fetch_one(pool).await?;
        if first.is_some_and(|seq| seq > boundary.event_sequence) {
            eligible.push(row);
            continue;
        }
        let completed: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM events WHERE event_type = 'ApplyAllBatchCompleted' AND aggregate_id = $1)")
            .bind(row.0.to_string()).fetch_one(pool).await?;
        if !completed {
            let applied: Vec<Uuid> = sqlx::query_scalar(
                "SELECT id FROM changes WHERE id = ANY($1) AND status = 'applied'",
            )
            .bind(&row.1)
            .fetch_all(pool)
            .await?;
            let failed = row
                .1
                .iter()
                .filter(|id| !applied.contains(id))
                .map(|id| ApplyFailure {
                    change_id: *id,
                    error: "Workspace restored from snapshot; apply this change again.".into(),
                })
                .collect();
            bus.emit(BusEvent::System(SystemEvent::ApplyAllBatchCompleted {
                batch_id: row.0,
                applied,
                failed,
            }))
            .await?;
        }
        sqlx::query("DELETE FROM apply_all_batches WHERE batch_id = $1")
            .bind(row.0)
            .execute(pool)
            .await?;
    }
    Ok(eligible)
}

/// INVARIANT: How recovery should treat a batch member, derived from its `changes.status`.
/// `Terminal` covers a set-aside, discarded or reverted member and a missing row. All
/// must become a terminal `Failed` so the batch can complete; leaving such a
/// member `Pending` would re-drive it into an `apply_change` `Noop` that never
/// emits a terminal event, stalling the batch forever.
#[derive(Debug, PartialEq, Eq)]
enum RecoveredMember {
    Applied,
    Pending,
    Terminal,
}

fn classify_recovered_member(status: Option<ChangeStatus>) -> RecoveredMember {
    match status {
        Some(ChangeStatus::Applied) => RecoveredMember::Applied,
        Some(ChangeStatus::Pending) => RecoveredMember::Pending,
        Some(
            ChangeStatus::SetAside
            | ChangeStatus::Discarded
            | ChangeStatus::Reverted
            | ChangeStatus::Withdrawn,
        )
        | None => RecoveredMember::Terminal,
    }
}

#[derive(Debug, PartialEq, Eq)]
enum RecoveredDrive {
    Drive,
    WaitForSession,
    WaitForResume,
}

fn decide_recovered_drive(
    session_running: bool,
    resume_queued: bool,
    resolution_open: Option<bool>,
) -> RecoveredDrive {
    if session_running {
        RecoveredDrive::WaitForSession
    } else if resume_queued && resolution_open != Some(false) {
        RecoveredDrive::WaitForResume
    } else {
        RecoveredDrive::Drive
    }
}

fn owner_is_resolving(decision: &RecoveredDrive, resolution_open: Option<bool>) -> Option<bool> {
    match decision {
        RecoveredDrive::Drive => None,
        RecoveredDrive::WaitForResume => Some(true),
        RecoveredDrive::WaitForSession => Some(resolution_open == Some(true)),
    }
}

#[derive(Debug, Clone)]
pub(crate) enum ApplyAllDriveMsg {
    Applied(Uuid),
    Failed(Uuid, String),
    Parked(Uuid),
    Laned(Uuid),
}

impl ApplyAllDriveMsg {
    fn change_id(&self) -> Uuid {
        match self {
            Self::Applied(id) | Self::Failed(id, _) | Self::Parked(id) | Self::Laned(id) => *id,
        }
    }
}

impl LucidosEngine {
    /// INVARIANT: Push a "change resolved" notification to the apply-all driver task.
    /// No-op when the channel send fails (driver task already shut down) —
    /// the batch is then orphaned in memory but the persisted
    /// `ApplyAllBatchStarted` event still lets recovery resume on restart.
    /// Non-blocking; never holds the registry lock.
    pub(crate) fn notify_apply_all(&self, msg: ApplyAllDriveMsg) {
        if let Err(e) = self.apply_all_drive_tx.send(msg) {
            log_legacy!(
                "[ApplyAll] notify channel closed — dropping {:?}; recovery on \
                 restart will resume from the persisted ApplyAllBatchStarted",
                e.0,
            );
        }
    }

    pub fn start_apply_all_driver(self: &std::sync::Arc<Self>) {
        let rx = crate::engine::APPLY_ALL_DRIVE_RX.with(|cell| cell.borrow_mut().take());
        let Some(mut rx) = rx else {
            log_legacy!("[ApplyAll] driver receiver missing — listener not started");
            return;
        };
        let engine = self.clone();
        tokio::spawn(async move {
            log_legacy!("[ApplyAll] driver task started");
            while let Some(msg) = rx.recv().await {
                engine.advance_apply_all_batch(msg).await;
            }
            log_legacy!("[ApplyAll] driver task exiting — channel closed");
        });
    }

    pub async fn run_apply_all(
        self: &std::sync::Arc<Self>,
        actor: Option<MessageOrigin>,
        keep_going: bool,
    ) -> Result<ApplyAllOutcome, sqlx::Error> {
        let all_pending = self.changes().list_pending().await?;
        let total_pending = all_pending.len();
        let ready =
            crate::core::changes::keep_apply_ready_changes(self.pool(), all_pending).await?;
        let not_ready = total_pending - ready.len();
        let (ready, already_applying_ids) = self.drop_changes_already_applying(ready).await?;
        let already_applying = already_applying_ids.len();
        let pending = crate::core::changes::drop_empty_changes(ready);
        let change_ids: Vec<Uuid> = pending.iter().map(|c| c.id).collect();

        // INVARIANT: Arm BEFORE the first apply. That apply is awaited here and can spend
        // minutes hardening, and an instruction the owner gave must not wait on
        // it.
        let armed = if keep_going {
            // INVARIANT: The sweep skips what this press applies, and must skip what the
            // lane or another apply already owns: a paused thread's change
            // would otherwise be applied a second time.
            let applying_now: Vec<Uuid> = change_ids
                .iter()
                .chain(&already_applying_ids)
                .copied()
                .collect();
            self.sweep_standing_applies(Some(Uuid::new_v4()), actor.clone(), &applying_now)
                .await
        } else {
            0
        };

        let started = self.start_apply_all_batch(change_ids, actor.clone()).await;
        let Some((batch_id, members)) = started else {
            return Ok(ApplyAllOutcome::NothingToApply {
                total_pending,
                not_ready,
                already_applying,
                armed,
            });
        };
        let first = pending
            .iter()
            .find(|c| c.id == members[0])
            .expect("a batch member is one of the pending changes");
        let first_result = self.apply_batch_member(first.id, actor).await;
        Ok(ApplyAllOutcome::Started {
            batch_id,
            batch_size: members.len(),
            armed,
            first_branch: first.branch_name.clone(),
            first_result,
        })
    }

    /// INVARIANT: `changes` without those the landing lane holds or an apply has
    /// claimed, plus the ids it dropped. The lane is read from events, which
    /// answer before the lane restores after a restart.
    async fn drop_changes_already_applying(
        &self,
        changes: Vec<crate::core::changes::Change>,
    ) -> Result<(Vec<crate::core::changes::Change>, Vec<Uuid>), sqlx::Error> {
        let ids: Vec<Uuid> = changes.iter().map(|c| c.id).collect();
        let mut applying =
            crate::engine::landing_lane::lane_held_changes(self.pool(), &ids).await?;
        applying.extend(ids.into_iter().filter(|id| self.apply_claims.is_held(*id)));
        let kept = crate::core::changes::drop_applying_changes(changes, &applying);
        applying.retain(|id| !kept.iter().any(|c| c.id == *id));
        Ok((kept, applying.into_iter().collect()))
    }

    /// INVARIANT: Apply one member the batch started, and report what the apply cannot
    /// report itself: `Noop` (already applied), `Conflict` owned by a resolver
    /// that was already running, and early `Err` paths (change not found,
    /// status mismatch). A report the apply also made is harmless: terminal
    /// status is first-write-wins, and a second park changes nothing.
    async fn apply_batch_member(
        self: &std::sync::Arc<Self>,
        change_id: Uuid,
        actor: Option<MessageOrigin>,
    ) -> Result<crate::engine::ApplyResult, String> {
        let result = self
            .apply_change(change_id, actor)
            .await
            .map_err(|e| e.to_string());
        match &result {
            Ok(r) if matches!(r.status, crate::engine::ApplyStatus::Noop) => {
                self.notify_apply_all(ApplyAllDriveMsg::Applied(change_id));
            }
            Ok(r) if matches!(r.status, crate::engine::ApplyStatus::Conflict) => {
                self.notify_apply_all(ApplyAllDriveMsg::Parked(change_id));
            }
            Err(e) => {
                self.notify_apply_all(ApplyAllDriveMsg::Failed(change_id, e.clone()));
            }
            Ok(_) => {}
        }
        result
    }

    fn spawn_batch_member(
        self: &std::sync::Arc<Self>,
        change_id: Uuid,
        actor: Option<MessageOrigin>,
    ) {
        let engine = self.clone();
        tokio::spawn(async move {
            if let Some(reason) = engine.batch_member_refusal(change_id).await {
                log_legacy!("[ApplyAll] not applying batch member {change_id}: {reason}");
                engine.notify_apply_all(ApplyAllDriveMsg::Failed(change_id, reason.to_string()));
                return;
            }
            if let Err(e) = engine.apply_batch_member(change_id, actor).await {
                log_legacy!("[ApplyAll] apply_change({change_id}) returned Err: {e}");
            }
        });
    }

    /// INVARIANT: [`batch_member_skip_reason`] for the stored row. A row that is gone or
    /// unreadable goes ahead, and `apply_change` reports that failure itself.
    /// A settle check that could not run refuses, since applying is the
    /// direction that can merge a branch still being written.
    async fn batch_member_refusal(&self, change_id: Uuid) -> Option<&'static str> {
        let change = match self.changes().get_by_id(change_id).await {
            Ok(Some(change)) => change,
            Ok(None) | Err(_) => return None,
        };
        let thread_ids = change.thread_id.into_iter();
        let unsettled = crate::core::changes::unsettled_thread_ids(self.pool(), thread_ids).await;
        let thread_unsettled = match unsettled {
            Ok(ids) => change.thread_id.is_some_and(|tid| ids.contains(&tid)),
            Err(e) => {
                log_legacy!("[ApplyAll] settle check for batch member {change_id} failed: {e}");
                return Some(MEMBER_SETTLE_UNKNOWN);
            }
        };
        batch_member_skip_reason(&change, thread_unsettled)
    }

    /// INVARIANT: Seed a new Apply All batch from the `change_ids` no running batch
    /// holds. Emits the durable `ApplyAllBatchStarted` event (recoverable on
    /// restart), opens its press in the landing lane, and adds the live batch
    /// to the in-memory registry. Returns the batch id and its members, or
    /// `None` when every change already belongs to a running batch.
    ///
    /// The first member is marked applying here, and the caller applies it
    /// synchronously, so it gets a useful response. Subsequent applies flow
    /// through the driver task via `notify_apply_all`.
    ///
    /// Everything runs under the registry lock, so two presses never share a
    /// change. Its reports would reach only one of them, and the other's press
    /// would stay open in the lane for good. A Cancel also waits for the press
    /// to open before it submits it.
    pub(crate) async fn start_apply_all_batch(
        &self,
        change_ids: Vec<Uuid>,
        actor: Option<MessageOrigin>,
    ) -> Option<(Uuid, Vec<Uuid>)> {
        let mut reg = self.apply_all_batches.lock().await;
        let change_ids = reg.not_in_any_batch(change_ids);
        if change_ids.is_empty() {
            return None;
        }
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
            log_legacy!(
                "[ApplyAll] failed to persist batch {} membership: {}",
                batch_id,
                e
            );
        }
        let mut progress = BatchProgress::new(batch_id, change_ids.clone(), actor);
        progress.start_next();
        self.open_lane_press(batch_id);
        reg.insert(progress);
        log_legacy!("[ApplyAll] batch {} seeded", batch_id);
        Some((batch_id, change_ids))
    }

    async fn persist_batch_removed(&self, batch_id: Uuid) {
        if let Err(e) = sqlx::query("DELETE FROM apply_all_batches WHERE batch_id = $1")
            .bind(batch_id)
            .execute(self.pool())
            .await
        {
            log_legacy!("[ApplyAll] failed to delete batch {} row: {}", batch_id, e);
        }
    }

    /// INVARIANT: Cancel every in-flight Apply All batch — the user clicked Cancel on the
    /// batch toast. For each batch, in order:
    /// 1. Remove it from the registry, so the driver stops advancing.
    /// 2. Take unresolved members out of the landing lane: a waiting one
    ///    leaves it, and a lane batch holding one fails whole. Then submit the
    ///    press, so the lane stops gathering for it.
    /// 3. Interrupt the live sessions of the other unresolved members: the one
    ///    hardening or merging, and every parked resolver. A lane member's
    ///    source thread is never interrupted: the lane, not its agent, holds it.
    /// 4. Mark every still-pending member canceled, so the batch is complete.
    /// 5. Emit `ApplyAllBatchCompleted`.
    ///
    /// Semantics: already-applied members stay applied; the in-flight apply
    /// aborts back to pending (best-effort — a merge that already landed before
    /// the interrupt processes still lands and emits `ChangeApplied`, which the
    /// now-removed batch ignores); queued members are left untouched as pending.
    /// Returns the number of batches canceled (0 = nothing was running).
    pub async fn cancel_apply_all_batches(&self, actor: Option<MessageOrigin>) -> usize {
        // INVARIANT: Snapshot pending members and remove the batches under ONE lock so the
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
        let in_lane: std::collections::HashSet<Uuid> = self
            .cancel_lane_members(pending_by_batch.concat(), actor.clone())
            .await
            .into_iter()
            .collect();
        for final_state in &finals {
            self.submit_lane_press(final_state.batch_id());
        }
        for pending in &pending_by_batch {
            for &change_id in pending.iter().filter(|id| !in_lane.contains(id)) {
                let thread_id = match self.changes().get_by_id(change_id).await {
                    Ok(Some(c)) => c.thread_id,
                    _ => None,
                };
                if let Some(thread_id) = thread_id {
                    if self.is_agent_running_for(thread_id).await {
                        if let Err(e) = self
                            .agent_host()
                            .interrupt_agent(
                                self,
                                Some(thread_id),
                                actor.clone(),
                                crate::engine::agent_host::SettleTerminal::StuckProjection,
                            )
                            .await
                        {
                            log_legacy!(
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
            log_legacy!(
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

    /// INVARIANT: Rebuild the Apply-All registry from the durable `apply_all_batches`
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
    /// session's terminal event advances it). Each pending member keeps the
    /// owner a restart found (`recovered_owner`). If no owner holds the queue,
    /// the next unowned member is driven through the idempotent
    /// `apply_change`. An owner emits the terminal event that advances the
    /// batch.
    ///
    /// MUST run after agent recovery queues its switch resumes, and before
    /// `resume_pending_switches` drains that queue.
    pub async fn recover_apply_all_batches(self: &std::sync::Arc<Self>) {
        let rows = match recoverable_batches(self.pool(), &self.event_bus).await {
            Ok(r) => r,
            Err(e) => {
                log_legacy!("[ApplyAll] recovery query failed: {}", e);
                return;
            }
        };
        if rows.is_empty() {
            return;
        }
        log_legacy!(
            "[ApplyAll] recovering {} unfinished batch(es) from previous process",
            rows.len()
        );

        for (batch_id, change_ids, actor_json) in rows {
            let actor: Option<MessageOrigin> =
                actor_json.and_then(|v| serde_json::from_value(v).ok());
            let mut progress = BatchProgress::new(batch_id, change_ids.clone(), actor.clone());

            for &change_id in &change_ids {
                let status = match self.changes().get_by_id(change_id).await {
                    Ok(Some(c)) => Some(c.status()),
                    Ok(None) => None,
                    Err(e) => {
                        log_legacy!(
                            "[ApplyAll] recovery: changes lookup for {} failed: {} — \
                             treating as pending (will re-drive)",
                            change_id,
                            e
                        );
                        Some(ChangeStatus::Pending)
                    }
                };
                match classify_recovered_member(status) {
                    RecoveredMember::Applied => {
                        progress.record_applied(change_id);
                    }
                    RecoveredMember::Pending => {}
                    RecoveredMember::Terminal => {
                        progress.record_failed(change_id, WITHDRAWN_MEMBER_REASON.to_string());
                    }
                }
            }

            if progress.is_complete() {
                log_legacy!(
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

            let pending = progress.pending_members();
            let laned =
                match crate::engine::landing_lane::lane_held_changes(self.pool(), &pending).await {
                    Ok(held) => held,
                    Err(e) => {
                        // INVARIANT: An unknown lane never authorizes an apply, and parking
                        // every member would hang the batch. The batch stays
                        // persisted, so the next boot recovers it.
                        log_legacy!(
                            "[ApplyAll] recovery: lane lookup for batch {} failed: {} — \
                             leaving the batch for the next boot",
                            batch_id,
                            e
                        );
                        continue;
                    }
                };
            for change_id in pending {
                if laned.contains(&change_id) {
                    progress.restore_laned(change_id);
                } else if let Some(resolving) = self.recovered_owner(batch_id, change_id).await {
                    progress.restore_owned(change_id, resolving);
                }
            }
            let next = progress.start_next();
            self.apply_all_batches.lock().await.insert(progress);
            if let Some(change_id) = next {
                log_legacy!(
                    "[ApplyAll] recovery: driving pending member {} of batch {}",
                    change_id,
                    batch_id
                );
                self.spawn_batch_member(change_id, actor);
            }
        }
    }

    async fn recovered_owner(&self, batch_id: Uuid, change_id: Uuid) -> Option<bool> {
        let thread_id = match self.changes().get_by_id(change_id).await {
            Ok(Some(c)) => c.thread_id,
            _ => None,
        }?;
        let session_running = self.is_agent_running_for(thread_id).await;
        let resume_queued = self.switch_resume_queued(thread_id);
        let resolution_open = if resume_queued || session_running {
            self.conflict_pairing_open_or_unknown(thread_id, change_id)
                .await
        } else {
            None
        };
        let decision = decide_recovered_drive(session_running, resume_queued, resolution_open);
        let resolving = owner_is_resolving(&decision, resolution_open)?;
        log_legacy!(
            "[ApplyAll] recovery: batch {} member {}: {:?} (resolving: {}), waiting for its \
             terminal event",
            batch_id,
            change_id,
            decision,
            resolving
        );
        Some(resolving)
    }

    async fn advance_apply_all_batch(self: &std::sync::Arc<Self>, msg: ApplyAllDriveMsg) {
        let change_id = msg.change_id();
        let (batch_id, submitted, next_step) = {
            let mut reg = self.apply_all_batches.lock().await;
            let Some(batch_id) = reg.batch_for_change(change_id) else {
                return;
            };
            let batch = reg
                .get_mut(batch_id)
                .expect("batch_for_change just found it");
            let advance = match msg {
                ApplyAllDriveMsg::Applied(id) => batch.resolve(id, Ok(())),
                ApplyAllDriveMsg::Failed(id, error) => batch.resolve(id, Err(error)),
                ApplyAllDriveMsg::Parked(id) => batch.park(id),
                ApplyAllDriveMsg::Laned(id) => batch.park_in_lane(id),
            };
            let submitted = batch.is_submitted();
            let step = match advance {
                Advance::Duplicate => {
                    log_legacy!(
                        "[ApplyAll] nothing changed for {} in batch {}, skipping advance",
                        change_id,
                        batch_id,
                    );
                    return;
                }
                Advance::Wait => NextStep::Wait,
                Advance::Complete => {
                    let final_state = reg.remove(batch_id).expect("just confirmed via get_mut");
                    NextStep::Complete {
                        batch_id,
                        applied: final_state.applied_ids(),
                        failed: final_state.failures(),
                    }
                }
                Advance::Start(next) => NextStep::Start {
                    next_change: next,
                    actor: batch.actor(),
                },
            };
            (batch_id, submitted, step)
        };
        if submitted {
            self.submit_lane_press(batch_id);
        }
        match next_step {
            NextStep::Complete {
                batch_id,
                applied,
                failed,
            } => {
                log_legacy!(
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
            }
            NextStep::Start { next_change, actor } => {
                log_legacy!(
                    "[ApplyAll] starting batch member {} (after {})",
                    next_change,
                    change_id
                );
                self.spawn_batch_member(next_change, actor);
            }
            NextStep::Wait => {}
        }
        self.broadcast_changes_updated().await;
    }
}

pub enum ApplyAllOutcome {
    Started {
        batch_id: Uuid,
        batch_size: usize,
        armed: usize,
        first_branch: String,
        first_result: Result<crate::engine::ApplyResult, String>,
    },
    NothingToApply {
        total_pending: usize,
        not_ready: usize,
        already_applying: usize,
        armed: usize,
    },
}

enum NextStep {
    Complete {
        batch_id: Uuid,
        applied: Vec<Uuid>,
        failed: Vec<ApplyFailure>,
    },
    Start {
        next_change: Uuid,
        actor: Option<MessageOrigin>,
    },
    Wait,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_member_status_mapping() {
        assert_eq!(
            classify_recovered_member(Some(ChangeStatus::Applied)),
            RecoveredMember::Applied
        );
        assert_eq!(
            classify_recovered_member(Some(ChangeStatus::Pending)),
            RecoveredMember::Pending
        );
        assert_eq!(
            classify_recovered_member(Some(ChangeStatus::Discarded)),
            RecoveredMember::Terminal
        );
        assert_eq!(
            classify_recovered_member(Some(ChangeStatus::Reverted)),
            RecoveredMember::Terminal
        );
        assert_eq!(classify_recovered_member(None), RecoveredMember::Terminal);
    }

    #[test]
    fn a_resuming_conflict_resolution_waits_before_its_session_spawns() {
        assert_eq!(
            decide_recovered_drive(false, true, Some(true)),
            RecoveredDrive::WaitForResume
        );
    }

    #[test]
    fn a_resuming_thread_with_an_unknown_resolution_waits() {
        assert_eq!(
            decide_recovered_drive(false, true, None),
            RecoveredDrive::WaitForResume
        );
    }

    #[test]
    fn a_stranded_conflict_resolution_is_driven() {
        assert_eq!(
            decide_recovered_drive(false, false, Some(true)),
            RecoveredDrive::Drive
        );
        assert_eq!(
            decide_recovered_drive(false, false, None),
            RecoveredDrive::Drive
        );
    }

    #[test]
    fn a_resume_with_no_open_resolution_is_driven() {
        assert_eq!(
            decide_recovered_drive(false, true, Some(false)),
            RecoveredDrive::Drive
        );
    }

    #[test]
    fn a_running_session_waits_whatever_else_holds() {
        for resume_queued in [false, true] {
            for resolution_open in [Some(false), Some(true), None] {
                assert_eq!(
                    decide_recovered_drive(true, resume_queued, resolution_open),
                    RecoveredDrive::WaitForSession
                );
            }
        }
    }

    #[test]
    fn fully_resolved_batch_reconstructs_as_complete() {
        let ids: Vec<Uuid> = (0..3).map(|_| Uuid::new_v4()).collect();
        let mut progress = BatchProgress::new(Uuid::new_v4(), ids.clone(), None);
        for (i, &id) in ids.iter().enumerate() {
            match classify_recovered_member(if i < 2 {
                Some(ChangeStatus::Applied)
            } else {
                None
            }) {
                RecoveredMember::Applied => {
                    progress.record_applied(id);
                }
                RecoveredMember::Terminal => {
                    progress.record_failed(id, WITHDRAWN_MEMBER_REASON.to_string());
                }
                RecoveredMember::Pending => unreachable!(),
            }
        }
        assert!(progress.is_complete());
        assert_eq!(progress.applied_ids(), vec![ids[0], ids[1]]);
        assert_eq!(progress.failures().len(), 1);
        assert_eq!(progress.failures()[0].error, WITHDRAWN_MEMBER_REASON);
    }

    #[test]
    fn batch_with_pending_member_reconstructs_incomplete_and_drives_next() {
        let ids: Vec<Uuid> = (0..3).map(|_| Uuid::new_v4()).collect();
        let mut progress = BatchProgress::new(Uuid::new_v4(), ids.clone(), None);
        progress.record_applied(ids[0]);
        assert!(!progress.is_complete());
        assert_eq!(progress.start_next(), Some(ids[1]));
    }

    #[test]
    fn a_running_resolver_is_restored_beside_the_queue() {
        let owner = owner_is_resolving(&RecoveredDrive::WaitForSession, Some(true));
        assert_eq!(owner, Some(true));
        let ids: Vec<Uuid> = (0..2).map(|_| Uuid::new_v4()).collect();
        let mut progress = BatchProgress::new(Uuid::new_v4(), ids.clone(), None);
        progress.restore_owned(ids[0], true);
        assert_eq!(progress.start_next(), Some(ids[1]));
    }

    #[test]
    fn a_session_not_resolving_or_unknown_holds_the_queue() {
        for open in [Some(false), None] {
            assert_eq!(
                owner_is_resolving(&RecoveredDrive::WaitForSession, open),
                Some(false)
            );
        }
        assert_eq!(
            owner_is_resolving(&RecoveredDrive::WaitForResume, None),
            Some(true),
            "a queued resume inherits the open resolution"
        );
        assert_eq!(owner_is_resolving(&RecoveredDrive::Drive, Some(true)), None);
    }

    fn pending_member() -> crate::core::changes::Change {
        use crate::core::changes::{Change, ChangeStatusData};
        Change {
            id: Uuid::new_v4(),
            request_id: Uuid::new_v4(),
            thread_id: Some(Uuid::new_v4()),
            branch_name: "claude-code/member".into(),
            repo_root: "/repo".into(),
            description: "desc".into(),
            file_count: 1,
            files: vec!["a.rs".into()],
            requires_restart: false,
            state: ChangeStatusData::Pending {
                merge: None,
                thread: Default::default(),
                apply: Default::default(),
            },
            created_at: chrono::Utc::now(),
            resolved_at: None,
            hardened: false,
            thread_title: None,
            commits: vec![],
            summary: None,
            incomplete: false,
        }
    }

    #[test]
    fn a_member_the_opening_filter_would_drop_is_skipped_when_its_turn_comes() {
        let clean = pending_member();
        assert_eq!(batch_member_skip_reason(&clean, false), None);
        assert_eq!(
            batch_member_skip_reason(&clean, true),
            Some(MEMBER_THREAD_UNSETTLED)
        );
        let empty = crate::core::changes::Change {
            file_count: 0,
            ..pending_member()
        };
        assert_eq!(batch_member_skip_reason(&empty, false), Some(MEMBER_EMPTY));
    }

    #[test]
    fn a_resolved_member_is_left_to_apply_change() {
        let applied = crate::core::changes::Change {
            state: crate::core::changes::ChangeStatusData::Applied(Default::default()),
            ..pending_member()
        };
        assert_eq!(batch_member_skip_reason(&applied, true), None);
    }
    #[tokio::test]
    async fn restored_apply_all_settles_old_batches_and_keeps_fresh_batches() {
        use crate::engine::event_bus::EventBus;
        use crate::test_support::{setup_test_db, teardown_test_db};
        let (pool, db) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let old = Uuid::new_v4();
        let fresh = Uuid::new_v4();
        let member = Uuid::new_v4();
        let applied = Uuid::new_v4();
        sqlx::query("INSERT INTO changes (id, request_id, branch_name, repo_root, description, file_count, files, requires_restart, status) VALUES ($1, $2, 'applied-branch', '/repo', 'change', 1, ARRAY['a.rs'], false, 'applied')")
            .bind(applied).bind(Uuid::new_v4()).execute(&pool).await.unwrap();
        for id in [old, fresh] {
            let members = if id == old {
                vec![member, applied]
            } else {
                vec![member]
            };
            bus.emit(BusEvent::System(SystemEvent::ApplyAllBatchStarted {
                batch_id: id,
                change_ids: members.clone(),
                actor: None,
            }))
            .await
            .unwrap();
            sqlx::query("INSERT INTO apply_all_batches (batch_id, change_ids) VALUES ($1, $2)")
                .bind(id)
                .bind(members)
                .execute(&pool)
                .await
                .unwrap();
            if id == old {
                sqlx::query("INSERT INTO snapshot_restore_boundary (restore_id, snapshot, restored_at, event_sequence) SELECT $1, $2, now(), MAX(sequence) FROM events")
                    .bind(Uuid::new_v4()).bind("20261031T120000Z-0123456789abcdef")
                    .execute(&pool).await.unwrap();
            }
        }
        bus.emit(BusEvent::System(SystemEvent::ApplyAllBatchStarted {
            batch_id: old,
            change_ids: vec![member],
            actor: None,
        }))
        .await
        .unwrap();
        for _ in 0..2 {
            settle_restored_batches(&pool, &bus).await.unwrap();
            let rows = recoverable_batches(&pool, &bus).await.unwrap();
            assert_eq!(rows.iter().map(|r| r.0).collect::<Vec<_>>(), vec![fresh]);
        }
        let completed: Vec<serde_json::Value> = sqlx::query_scalar(
            "SELECT payload FROM events WHERE event_type = 'ApplyAllBatchCompleted' AND aggregate_id = $1")
            .bind(old.to_string()).fetch_all(&pool).await.unwrap();
        assert_eq!(completed.len(), 1);
        assert_eq!(
            completed[0]["data"]["applied"],
            serde_json::json!([applied])
        );
        assert_eq!(
            completed[0]["data"]["failed"][0]["change_id"],
            member.to_string()
        );
        assert_eq!(
            completed[0]["data"]["failed"][0]["error"],
            "Workspace restored from snapshot; apply this change again."
        );
        pool.close().await;
        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn restored_apply_all_refuses_missing_provenance_and_failed_settlement() {
        use crate::engine::event_bus::EventBus;
        use crate::test_support::{refuse_event_type, setup_test_db, teardown_test_db};
        let (pool, db) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let id = Uuid::new_v4();
        sqlx::query("INSERT INTO apply_all_batches (batch_id, change_ids) VALUES ($1, $2)")
            .bind(id)
            .bind(vec![Uuid::new_v4()])
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO snapshot_restore_boundary (restore_id, snapshot, restored_at, event_sequence) VALUES ($1, $2, now(), 0)")
            .bind(Uuid::new_v4()).bind("20261031T120000Z-0123456789abcdef")
            .execute(&pool).await.unwrap();
        refuse_event_type(&pool, "ApplyAllBatchCompleted").await;
        for _ in 0..2 {
            assert!(settle_restored_batches(&pool, &bus).await.is_err());
            let count: i64 = sqlx::query_scalar("SELECT count(*) FROM apply_all_batches")
                .fetch_one(&pool)
                .await
                .unwrap();
            assert_eq!(count, 1);
        }
        sqlx::query("DROP TRIGGER refuse_event ON events")
            .execute(&pool)
            .await
            .unwrap();
        assert!(recoverable_batches(&pool, &bus).await.unwrap().is_empty());
        sqlx::query("INSERT INTO apply_all_batches (batch_id, change_ids) VALUES ($1, $2)")
            .bind(id)
            .bind(vec![Uuid::new_v4()])
            .execute(&pool)
            .await
            .unwrap();
        assert!(recoverable_batches(&pool, &bus).await.unwrap().is_empty());
        let count: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM events WHERE event_type = 'ApplyAllBatchCompleted'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(count, 1);
        sqlx::query("ALTER TABLE snapshot_restore_boundary RENAME TO unavailable_restore_boundary")
            .execute(&pool)
            .await
            .unwrap();
        assert!(settle_restored_batches(&pool, &bus).await.is_err());
        pool.close().await;
        teardown_test_db(&db).await;
    }
}
