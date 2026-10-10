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
//! hardening run can take many minutes). Spawning the apply lets the driver
//! immediately return to listening for the next message.
//!
//! `emit_merge_conflict_detected` sends `Parked`: the member's conflict now
//! belongs to a resolver, so the next member starts beside it (ADR 0314).

use uuid::Uuid;

use crate::core::changes::ChangeStatus;
use crate::engine::apply_all_batches::{Advance, ApplyFailure, BatchProgress};
use crate::engine::event_bus::{BusEvent, SystemEvent};
use crate::engine::thread_events::MessageOrigin;
use crate::engine::LucidosEngine;

/// Failure reason for a batch member that left `pending` before the batch
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

/// Why the driver must not apply a pending `change` when its turn comes, or
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

/// How recovery should treat a batch member, derived from its `changes.status`.
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

/// Pure mapping from a member's `changes.status` to its recovery classification.
/// `None` = the change row is gone (treated as terminal).
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

/// Whether startup recovery may apply a pending batch member itself.
#[derive(Debug, PartialEq, Eq)]
enum RecoveredDrive {
    /// Nobody else will resolve the member, so recovery applies it.
    Drive,
    /// A running session owns the member. Its terminal event advances the batch.
    WaitForSession,
    /// A queued resume inherits the member's open conflict resolution. The
    /// resumed continuation re-attaches it, or closes it with `ChangeApplyFailed`.
    WaitForResume,
}

/// A queued resume has no session yet at boot. Driving its open conflict
/// resolution then merges code the resolver never finished.
///
/// With no resume queued, an open pairing is stranded and is driven, as
/// `decide_merge_ownership` lets it through (ADR 0060). `resolution_open` is
/// `None` when the pairing query failed or was not needed. With a resume
/// queued, an unknown waits, because driving is the destructive direction.
///
/// A continuation that cannot re-attach, for example after a failed worktree
/// lookup, leaves the pairing open. The batch then waits until the user
/// cancels it or applies the change, or until the next boot drives it.
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

/// Whether the owner of a member recovery must not drive is a resolver,
/// which lets the queue move on. `None` when recovery drives the member.
fn owner_is_resolving(decision: &RecoveredDrive, resolution_open: Option<bool>) -> Option<bool> {
    match decision {
        RecoveredDrive::Drive => None,
        // A queued resume waits only on an open or unknown resolution.
        RecoveredDrive::WaitForResume => Some(true),
        // An unknown conflict state holds the queue.
        RecoveredDrive::WaitForSession => Some(resolution_open == Some(true)),
    }
}

/// A member's state for the batch to take in, whatever path it came from.
/// `ChangeApplied` / `ChangeApplyFailed` send the terminal two, and
/// `MergeConflictDetected` sends `Parked`.
#[derive(Debug, Clone)]
pub(crate) enum ApplyAllDriveMsg {
    Applied(Uuid),
    Failed(Uuid, String),
    Parked(Uuid),
}

impl ApplyAllDriveMsg {
    fn change_id(&self) -> Uuid {
        match self {
            Self::Applied(id) | Self::Failed(id, _) | Self::Parked(id) => *id,
        }
    }
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
                engine.advance_apply_all_batch(msg).await;
            }
            log!("[ApplyAll] driver task exiting — channel closed");
        });
    }

    /// One Apply All press, engine side.
    ///
    /// Filters the pending list exactly as the button's rule says, then arms
    /// the sweep when the owner asked. It seeds a batch and applies its first
    /// member synchronously, so the caller has a real result. The driver takes
    /// the rest.
    ///
    /// Both surfaces call this: the HTTP handler and the agent's
    /// `apply_as_they_settle`. Neither owns the rule, which is what stops the
    /// tool quietly becoming a second Apply All with no batch behind it.
    pub(crate) async fn run_apply_all(
        self: &std::sync::Arc<Self>,
        actor: Option<MessageOrigin>,
        keep_going: bool,
    ) -> Result<ApplyAllOutcome, sqlx::Error> {
        let all_pending = self.changes().list_pending().await?;
        let total_pending = all_pending.len();
        // Exclude changes whose thread has not settled: mid-turn, or parked and
        // due to wake (ADR 0106). This path calls `apply_change` directly, which
        // bypasses the per-change `guard_change_action` gate. Without it we
        // would merge a branch the coding agent is still committing on, racing
        // the session's next proposal (real thread 76b4ee76).
        //
        // The sweep answers those dropped changes: not applied now, applied
        // when their thread lands.
        let live_filtered =
            crate::core::changes::drop_unsettled_thread_changes(self.pool(), all_pending).await?;
        let unsettled = total_pending - live_filtered.len();
        // Also drop changes with no files left. The per-change endpoint 409s
        // those, and this path would otherwise do what the button refuses:
        // merge no-op commits, possibly spending a harden run on an empty diff.
        let pending = crate::core::changes::drop_empty_changes(live_filtered);
        let change_ids: Vec<Uuid> = pending.iter().map(|c| c.id).collect();

        // Arm BEFORE the first apply. That apply is awaited here and can spend
        // minutes hardening, and an instruction the owner gave must not wait on
        // it.
        let armed = if keep_going {
            self.sweep_standing_applies(Some(Uuid::new_v4()), actor.clone(), &change_ids)
                .await
        } else {
            0
        };

        let Some(first) = pending.first() else {
            return Ok(ApplyAllOutcome::NothingToApply {
                total_pending,
                unsettled,
                armed,
            });
        };
        let batch_id = self
            .start_apply_all_batch(change_ids.clone(), actor.clone())
            .await;
        let first_result = self.apply_batch_member(first.id, actor).await;
        Ok(ApplyAllOutcome::Started {
            batch_id,
            batch_size: change_ids.len(),
            armed,
            first_branch: first.branch_name.clone(),
            first_result,
        })
    }

    /// Apply one member the batch started, and report what the apply cannot
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

    /// Start `change_id` as a background task, for the driver and recovery.
    fn spawn_batch_member(
        self: &std::sync::Arc<Self>,
        change_id: Uuid,
        actor: Option<MessageOrigin>,
    ) {
        let engine = self.clone();
        tokio::spawn(async move {
            if let Some(reason) = engine.batch_member_refusal(change_id).await {
                log!("[ApplyAll] not applying batch member {change_id}: {reason}");
                engine.notify_apply_all(ApplyAllDriveMsg::Failed(change_id, reason.to_string()));
                return;
            }
            if let Err(e) = engine.apply_batch_member(change_id, actor).await {
                log!("[ApplyAll] apply_change({change_id}) returned Err: {e}");
            }
        });
    }

    /// [`batch_member_skip_reason`] for the stored row. A row that is gone or
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
                log!("[ApplyAll] settle check for batch member {change_id} failed: {e}");
                return Some(MEMBER_SETTLE_UNKNOWN);
            }
        };
        batch_member_skip_reason(&change, thread_unsettled)
    }

    /// Seed a new Apply All batch. Emits the durable `ApplyAllBatchStarted`
    /// event (recoverable on restart) and adds the live batch to the
    /// in-memory registry. Returns the batch_id so the HTTP handler can
    /// surface it to the caller.
    ///
    /// The first member is marked applying here, and the caller applies it
    /// synchronously, so it gets a useful response. Subsequent applies flow
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
        let mut progress = BatchProgress::new(batch_id, change_ids, actor);
        progress.start_next();
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
    /// batch toast. For each batch, in order:
    /// 1. Remove it from the registry, so the driver stops advancing.
    /// 2. Interrupt the live sessions of unresolved members: the one hardening
    ///    or merging, and every parked resolver.
    /// 3. Mark every still-pending member canceled, so the batch is complete.
    /// 4. Emit `ApplyAllBatchCompleted`.
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
        // Interrupt the live coding-agent sessions: the member mid-harden or
        // merge, and every parked resolver. The queued members have no live
        // session (interrupt is a lookup miss for them), so this only touches
        // the applies actually running.
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
    /// session's terminal event advances it). Each pending member keeps the
    /// owner a restart found (`recovered_owner`). If no owner holds the queue,
    /// the next unowned member is driven through the idempotent
    /// `apply_change`. An owner emits the terminal event that advances the
    /// batch.
    ///
    /// MUST run after agent recovery queues its switch resumes, and before
    /// `resume_pending_switches` drains that queue.
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
                    Ok(Some(c)) => Some(c.status()),
                    Ok(None) => None, // row gone → treat as terminal
                    Err(e) => {
                        log!(
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
                    RecoveredMember::Pending => { /* leave pending — re-drive below */ }
                    RecoveredMember::Terminal => {
                        progress.record_failed(change_id, WITHDRAWN_MEMBER_REASON.to_string());
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

            // Still in progress. Restore who owns each unresolved member, then
            // start the next one if none of them holds the queue.
            for change_id in progress.pending_members() {
                if let Some(resolving) = self.recovered_owner(batch_id, change_id).await {
                    progress.restore_owned(change_id, resolving);
                }
            }
            let next = progress.start_next();
            self.apply_all_batches.lock().await.insert(progress);
            if let Some(change_id) = next {
                log!(
                    "[ApplyAll] recovery: driving pending member {} of batch {}",
                    change_id,
                    batch_id
                );
                self.spawn_batch_member(change_id, actor);
            }
        }
    }

    /// Who owns a pending member at boot: `None` when nobody does and
    /// recovery may apply it, else `Some(resolving)`. A resolver lets the
    /// queue move on. A session doing anything else, or one whose conflict
    /// state is unknown, holds it (ADR 0314).
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
        log!(
            "[ApplyAll] recovery: batch {} member {}: {:?} (resolving: {}), waiting for its \
             terminal event",
            batch_id,
            change_id,
            decision,
            resolving
        );
        Some(resolving)
    }

    /// Take in one member's new state and decide what to do next. Called
    /// only from the driver task. Holds the registry lock just long enough to
    /// inspect + mutate, then releases before emitting the completion event
    /// or spawning the next apply.
    async fn advance_apply_all_batch(self: &std::sync::Arc<Self>, msg: ApplyAllDriveMsg) {
        let change_id = msg.change_id();
        let next_step = {
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
            };
            match advance {
                // A duplicate or late report, e.g. the conflict-recovery
                // cleanup emits `ChangeApplied` and the post-CC merge re-check
                // then emits `ChangeApplyFailed` for the same change_id.
                // Acting on it could start a second apply.
                Advance::Duplicate => {
                    log!(
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
            }
        };
        match next_step {
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
            }
            NextStep::Start { next_change, actor } => {
                log!(
                    "[ApplyAll] starting batch member {} (after {})",
                    next_change,
                    change_id
                );
                self.spawn_batch_member(next_change, actor);
            }
            NextStep::Wait => {}
        }
        // The batch snapshot rides the changes list, so every state change
        // repaints the menu.
        self.broadcast_changes_updated().await;
    }
}

/// What one Apply All press did, for the surface that made it.
pub(crate) enum ApplyAllOutcome {
    /// A batch started, and its first member has already been applied.
    Started {
        batch_id: Uuid,
        batch_size: usize,
        armed: usize,
        first_branch: String,
        first_result: Result<crate::engine::ApplyResult, String>,
    },
    /// Nothing could be applied right now. The two counts are what lets the
    /// caller name the real reason rather than one blanket refusal.
    NothingToApply {
        total_pending: usize,
        unsettled: usize,
        armed: usize,
    },
}

/// What `advance_apply_all_batch` decided to do once it released the
/// registry lock. Splitting the decision from the action keeps the lock
/// scope tight and makes the control flow readable.
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

    /// The load-bearing classification: a discarded / removed member MUST be
    /// terminal so the batch can complete; only an explicitly `pending` member
    /// stays pending (and gets re-driven). Misclassifying `discarded` as
    /// `pending` re-drives it into an `apply_change` `Noop` that never emits a
    /// terminal event — the batch stalls and the toast sticks forever.
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

    /// A switch restart cut a conflict resolution off mid-harden. At boot its
    /// resume is queued but not spawned, so no session runs yet.
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

    /// ADR 0060: after a crash nothing resumes, so an open pairing is stranded
    /// and must not block the batch.
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

    /// Reconstructing a batch where every member resolved (applied or
    /// discarded) yields a complete batch — recovery emits the missing
    /// `ApplyAllBatchCompleted` for it.
    #[test]
    fn fully_resolved_batch_reconstructs_as_complete() {
        let ids: Vec<Uuid> = (0..3).map(|_| Uuid::new_v4()).collect();
        let mut progress = BatchProgress::new(Uuid::new_v4(), ids.clone(), None);
        // applied, applied, discarded → all terminal.
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

    /// A batch with a still-`pending` member reconstructs as incomplete, and
    /// the first pending member is the one recovery drives.
    #[test]
    fn batch_with_pending_member_reconstructs_incomplete_and_drives_next() {
        let ids: Vec<Uuid> = (0..3).map(|_| Uuid::new_v4()).collect();
        let mut progress = BatchProgress::new(Uuid::new_v4(), ids.clone(), None);
        progress.record_applied(ids[0]);
        assert!(!progress.is_complete());
        assert_eq!(progress.start_next(), Some(ids[1]));
    }

    /// A restart during a parked resolution must not hold the queue behind it,
    /// and must not re-drive it either.
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

    /// A later member's turn comes minutes after the press. What the opening
    /// filter would drop then must not be applied, or a live agent's
    /// half-written branch lands on main.
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

    /// A member that already left `pending` goes to `apply_change`, whose
    /// terminal event is what advances the batch.
    #[test]
    fn a_resolved_member_is_left_to_apply_change() {
        let applied = crate::core::changes::Change {
            state: crate::core::changes::ChangeStatusData::Applied(Default::default()),
            ..pending_member()
        };
        assert_eq!(batch_member_skip_reason(&applied, true), None);
    }
}
