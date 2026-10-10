//! Apply-All batch progress tracker.
//!
//! Pure-logic state machine for an in-flight Apply All batch. The user
//! clicked Apply All, the engine emitted `ApplyAllBatchStarted` with every
//! pending change ID, and the driver task (`engine::apply_all_driver`) now
//! applies the members. It advances the batch as `ChangeApplied` /
//! `ChangeApplyFailed` events land, and emits `ApplyAllBatchCompleted` when
//! every member has resolved.
//!
//! Members apply one at a time, except that a member whose apply handed a
//! merge conflict to a resolver parks as `Resolving`. The queue then moves on
//! while the resolver works (ADR 0314).
//!
//! A terminal status is one-shot: the first terminal event wins. A late race
//! event for the same member is a no-op. Replayed events during restart
//! recovery produce the same final state as the live flow.

use crate::engine::git_ops::is_concurrent_main_conflict;
use crate::engine::thread_events::MessageOrigin;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use uuid::Uuid;

/// Per-change failure reason in an Apply All batch. Named-struct wire
/// format keeps the `failed` field of `ApplyAllBatchCompleted` self-
/// describing (tuple serialization would emit a positional JSON array
/// downstream consumers couldn't introspect).
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ApplyFailure {
    pub change_id: Uuid,
    pub error: String,
}

/// Lifecycle state of one batch member. `Applied` / `Failed` are terminal:
/// once set, a second terminal event for the same member is a no-op.
#[derive(Debug, Clone, PartialEq, Eq)]
enum MemberStatus {
    /// Waiting for its turn.
    Queued,
    /// The driver started its apply: hardening or merging. The queue waits.
    Applying,
    /// A resolver owns its merge conflict. The queue does not wait for it.
    Resolving,
    Applied,
    Failed(String),
}

impl MemberStatus {
    fn is_terminal(&self) -> bool {
        matches!(self, MemberStatus::Applied | MemberStatus::Failed(_))
    }
}

#[derive(Debug, Clone)]
struct Member {
    change_id: Uuid,
    status: MemberStatus,
    /// Went back in the queue once after a later member collided with its
    /// resolution. A second collision is terminal.
    requeued: bool,
}

/// What the driver does after a member's state changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Advance {
    /// Nothing changed: a duplicate or late event, or not a member.
    Duplicate,
    /// Every member has resolved.
    Complete,
    /// Nothing is applying now. Start this member's apply.
    Start(Uuid),
    /// A member is still applying, or only resolvers are left.
    Wait,
}

/// In-memory state for a single in-flight Apply All batch. Order-preserving:
/// members start in submission order so the user-visible progression matches
/// the pending-list order they saw in the UI.
#[derive(Debug, Clone)]
pub(crate) struct BatchProgress {
    batch_id: Uuid,
    actor: Option<MessageOrigin>,
    members: Vec<Member>,
}

impl BatchProgress {
    pub(crate) fn new(batch_id: Uuid, change_ids: Vec<Uuid>, actor: Option<MessageOrigin>) -> Self {
        Self {
            batch_id,
            actor,
            members: change_ids
                .into_iter()
                .map(|change_id| Member {
                    change_id,
                    status: MemberStatus::Queued,
                    requeued: false,
                })
                .collect(),
        }
    }

    pub(crate) fn actor(&self) -> Option<MessageOrigin> {
        self.actor.clone()
    }

    pub(crate) fn batch_id(&self) -> Uuid {
        self.batch_id
    }

    /// True when `change_id` is a member of this batch (regardless of
    /// status). The driver uses this to filter `ChangeApplied` events
    /// that aren't part of any batch — single-Apply clicks must not
    /// advance any batch.
    pub(crate) fn contains(&self, change_id: Uuid) -> bool {
        self.members.iter().any(|m| m.change_id == change_id)
    }

    fn member_mut(&mut self, change_id: Uuid) -> Option<&mut Member> {
        self.members.iter_mut().find(|m| m.change_id == change_id)
    }

    /// The one rule that starts work: when no member is applying, the
    /// earliest queued member becomes the one applying.
    pub(crate) fn start_next(&mut self) -> Option<Uuid> {
        if self
            .members
            .iter()
            .any(|m| m.status == MemberStatus::Applying)
        {
            return None;
        }
        let next = self
            .members
            .iter_mut()
            .find(|m| m.status == MemberStatus::Queued)?;
        next.status = MemberStatus::Applying;
        Some(next.change_id)
    }

    fn advance(&mut self) -> Advance {
        if self.is_complete() {
            return Advance::Complete;
        }
        self.start_next().map_or(Advance::Wait, Advance::Start)
    }

    /// The applying member handed a merge conflict to a resolver. It waits
    /// off to the side, and the next queued member starts.
    pub(crate) fn park(&mut self, change_id: Uuid) -> Advance {
        match self.member_mut(change_id) {
            Some(m) if m.status == MemberStatus::Applying => {
                m.status = MemberStatus::Resolving;
                self.advance()
            }
            _ => Advance::Duplicate,
        }
    }

    /// Restore a queued member to the state a restart found it in: a live
    /// or resuming session owns it, resolving a conflict or not.
    ///
    /// A resolver can own any member. A session doing anything else holds the
    /// queue only for the earliest member not parked, the one the batch had
    /// started. A later member's live session is not this batch's work, so
    /// that member stays queued and is driven in its turn.
    pub(crate) fn restore_owned(&mut self, change_id: Uuid, resolving: bool) {
        let Some(index) = self.members.iter().position(|m| m.change_id == change_id) else {
            return;
        };
        if self.members[index].status != MemberStatus::Queued {
            return;
        }
        let earliest_unparked = !self.members[..index]
            .iter()
            .any(|m| matches!(m.status, MemberStatus::Queued | MemberStatus::Applying));
        if resolving {
            self.members[index].status = MemberStatus::Resolving;
        } else if earliest_unparked {
            self.members[index].status = MemberStatus::Applying;
        }
    }

    /// Mark `change_id` applied. Returns `true` if state changed; `false`
    /// when the change isn't a member or was already terminal. First-write-
    /// wins is what keeps a `ChangeApplied` racing a stale
    /// `ChangeApplyFailed` (or vice versa) from starting a second apply.
    pub(crate) fn record_applied(&mut self, change_id: Uuid) -> bool {
        self.record(change_id, MemberStatus::Applied)
    }

    /// Mark `change_id` failed with `error`. Same first-write-wins +
    /// state-change return as `record_applied`.
    pub(crate) fn record_failed(&mut self, change_id: Uuid, error: String) -> bool {
        self.record(change_id, MemberStatus::Failed(error))
    }

    fn record(&mut self, change_id: Uuid, terminal: MemberStatus) -> bool {
        match self.member_mut(change_id) {
            Some(m) if !m.status.is_terminal() => {
                m.status = terminal;
                true
            }
            _ => false,
        }
    }

    /// Record one member's terminal event and say what the driver does next.
    ///
    /// A resolving member that lost to a member landing meanwhile goes back
    /// in the queue once, rather than failing a change one more pass lands.
    pub(crate) fn resolve(&mut self, change_id: Uuid, result: Result<(), String>) -> Advance {
        let state_changed = match result {
            Ok(()) => self.record_applied(change_id),
            Err(error) => match self.member_mut(change_id) {
                Some(m)
                    if m.status == MemberStatus::Resolving
                        && !m.requeued
                        && is_concurrent_main_conflict(&error) =>
                {
                    m.status = MemberStatus::Queued;
                    m.requeued = true;
                    true
                }
                _ => self.record_failed(change_id, error),
            },
        };
        if !state_changed {
            return Advance::Duplicate;
        }
        self.advance()
    }

    /// Every still-unresolved member, in order: applying, resolving and
    /// queued alike. The batch-cancel path interrupts and fails them all.
    pub(crate) fn pending_members(&self) -> Vec<Uuid> {
        self.members
            .iter()
            .filter(|m| !m.status.is_terminal())
            .map(|m| m.change_id)
            .collect()
    }

    /// True when every member has resolved. The driver emits
    /// `ApplyAllBatchCompleted` and removes the batch from the registry.
    pub(crate) fn is_complete(&self) -> bool {
        self.members.iter().all(|m| m.status.is_terminal())
    }

    pub(crate) fn applied_ids(&self) -> Vec<Uuid> {
        self.ids_with(|s| *s == MemberStatus::Applied)
    }

    fn ids_with(&self, pred: impl Fn(&MemberStatus) -> bool) -> Vec<Uuid> {
        self.members
            .iter()
            .filter(|m| pred(&m.status))
            .map(|m| m.change_id)
            .collect()
    }

    fn snapshot_into(&self, snapshot: &mut ApplyAllBatchSnapshot) {
        for m in &self.members {
            snapshot.change_ids.push(m.change_id);
            match m.status {
                MemberStatus::Applied | MemberStatus::Failed(_) => {
                    snapshot.resolved_change_ids.push(m.change_id)
                }
                MemberStatus::Applying => snapshot.applying_change_ids.push(m.change_id),
                MemberStatus::Resolving => snapshot.resolving_change_ids.push(m.change_id),
                MemberStatus::Queued => {}
            }
        }
    }

    pub(crate) fn failures(&self) -> Vec<ApplyFailure> {
        self.members
            .iter()
            .filter_map(|m| match &m.status {
                MemberStatus::Failed(error) => Some(ApplyFailure {
                    change_id: m.change_id,
                    error: error.clone(),
                }),
                _ => None,
            })
            .collect()
    }
}

/// In-memory registry of all in-flight Apply All batches. The driver owns
/// one instance on the engine. Concurrent batches are allowed (one per
/// device, in principle) but in practice the UI starts only one at a time.
#[derive(Debug, Default)]
pub(crate) struct ApplyAllRegistry {
    inner: HashMap<Uuid, BatchProgress>,
}

impl ApplyAllRegistry {
    pub(crate) fn insert(&mut self, progress: BatchProgress) {
        self.inner.insert(progress.batch_id(), progress);
    }

    pub(crate) fn remove(&mut self, batch_id: Uuid) -> Option<BatchProgress> {
        self.inner.remove(&batch_id)
    }

    /// IDs of every live batch. The cancel-all path snapshots these so it can
    /// drain + remove each batch under one lock.
    pub(crate) fn batch_ids(&self) -> Vec<Uuid> {
        self.inner.keys().copied().collect()
    }

    /// Find the batch that contains `change_id`, if any. Returns the batch
    /// ID so the caller can take a mutable reference via `get_mut`.
    pub(crate) fn batch_for_change(&self, change_id: Uuid) -> Option<Uuid> {
        self.inner
            .iter()
            .find_map(|(batch_id, progress)| progress.contains(change_id).then_some(*batch_id))
    }

    pub(crate) fn get_mut(&mut self, batch_id: Uuid) -> Option<&mut BatchProgress> {
        self.inner.get_mut(&batch_id)
    }

    /// The running Apply All as the apply toast draws it, or `None` when no
    /// batch runs. Concurrent batches read as one longer run.
    pub(crate) fn snapshot(&self) -> Option<ApplyAllBatchSnapshot> {
        if self.inner.is_empty() {
            return None;
        }
        let mut snapshot = ApplyAllBatchSnapshot::default();
        for progress in self.inner.values() {
            progress.snapshot_into(&mut snapshot);
        }
        Some(snapshot)
    }
}

/// How far a running Apply All has got. A member in none of the three lists
/// is queued.
#[derive(Debug, Default, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ApplyAllBatchSnapshot {
    pub change_ids: Vec<Uuid>,
    /// Members that applied or failed.
    pub resolved_change_ids: Vec<Uuid>,
    /// Members hardening or merging. The queue waits for them.
    pub applying_change_ids: Vec<Uuid>,
    /// Members whose merge conflict a resolver owns. The queue moved on.
    pub resolving_change_ids: Vec<Uuid>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn collision() -> String {
        format!(
            "Merge failed after conflict resolution: {}: CONFLICT",
            crate::engine::git_ops::CONCURRENT_MAIN_CONFLICT
        )
    }

    fn uuids(n: usize) -> Vec<Uuid> {
        (0..n).map(|_| Uuid::new_v4()).collect()
    }

    fn failure(change_id: Uuid, error: &str) -> ApplyFailure {
        ApplyFailure {
            change_id,
            error: error.to_string(),
        }
    }

    /// A batch whose first member the driver has started, as
    /// `start_apply_all_batch` leaves it.
    fn started(ids: &[Uuid]) -> BatchProgress {
        let mut batch = BatchProgress::new(Uuid::new_v4(), ids.to_vec(), None);
        assert_eq!(batch.start_next(), Some(ids[0]));
        batch
    }

    fn snapshot_of(batch: &BatchProgress) -> ApplyAllBatchSnapshot {
        let mut snapshot = ApplyAllBatchSnapshot::default();
        batch.snapshot_into(&mut snapshot);
        snapshot
    }

    #[test]
    fn batch_progresses_through_all_members_in_order() {
        let ids = uuids(3);
        let mut batch = started(&ids);

        assert_eq!(batch.resolve(ids[0], Ok(())), Advance::Start(ids[1]));
        assert_eq!(batch.resolve(ids[1], Ok(())), Advance::Start(ids[2]));
        assert_eq!(batch.resolve(ids[2], Ok(())), Advance::Complete);
        assert_eq!(batch.applied_ids(), ids);
        assert!(batch.failures().is_empty());
    }

    /// Only one member hardens or merges at a time.
    #[test]
    fn nothing_starts_while_a_member_is_applying() {
        let ids = uuids(2);
        let mut batch = started(&ids);
        assert_eq!(batch.start_next(), None);
        assert_eq!(snapshot_of(&batch).applying_change_ids, vec![ids[0]]);
    }

    /// The reported stall: member 7 of 9 resolved a conflict for twenty
    /// minutes while 8 and 9 waited. A parked member lets the queue move on.
    #[test]
    fn a_conflicting_member_parks_and_the_next_one_starts() {
        let ids = uuids(3);
        let mut batch = started(&ids);

        assert_eq!(batch.park(ids[0]), Advance::Start(ids[1]));
        let snapshot = snapshot_of(&batch);
        assert_eq!(snapshot.resolving_change_ids, vec![ids[0]]);
        assert_eq!(snapshot.applying_change_ids, vec![ids[1]]);

        assert_eq!(batch.resolve(ids[1], Ok(())), Advance::Start(ids[2]));
        assert_eq!(
            batch.resolve(ids[0], Ok(())),
            Advance::Wait,
            "a resolver landing must not start the member already applying"
        );
        assert_eq!(batch.resolve(ids[2], Ok(())), Advance::Complete);
        assert_eq!(batch.applied_ids(), ids);
    }

    /// The batch completes only when the last resolver lands.
    #[test]
    fn a_batch_waits_for_its_parked_members_before_completing() {
        let ids = uuids(2);
        let mut batch = started(&ids);
        assert_eq!(batch.park(ids[0]), Advance::Start(ids[1]));
        assert_eq!(batch.park(ids[1]), Advance::Wait);
        assert_eq!(batch.resolve(ids[1], Ok(())), Advance::Wait);
        assert!(!batch.is_complete());
        assert_eq!(
            batch.resolve(ids[0], Err("harden failed".into())),
            Advance::Complete
        );
        assert_eq!(batch.failures(), vec![failure(ids[0], "harden failed")]);
    }

    /// A park for a member that is not applying is a late or stray report,
    /// and must not start a second member.
    #[test]
    fn a_park_for_a_member_not_applying_changes_nothing() {
        let ids = uuids(3);
        let mut batch = started(&ids);
        assert_eq!(batch.park(ids[1]), Advance::Duplicate, "ids[1] is queued");
        assert_eq!(batch.park(ids[0]), Advance::Start(ids[1]));
        assert_eq!(batch.park(ids[0]), Advance::Duplicate, "already parked");
        assert_eq!(batch.park(Uuid::new_v4()), Advance::Duplicate);
        assert_eq!(snapshot_of(&batch).applying_change_ids, vec![ids[1]]);
    }

    /// A parked member whose resolver lost to a member landing meanwhile goes
    /// back in the queue once, and starts when nothing else is applying.
    #[test]
    fn a_collided_resolution_is_requeued_once() {
        let ids = uuids(2);
        let mut batch = started(&ids);
        assert_eq!(batch.park(ids[0]), Advance::Start(ids[1]));

        assert_eq!(
            batch.resolve(ids[0], Err(collision())),
            Advance::Wait,
            "ids[1] is applying, so the retry waits its turn"
        );
        assert_eq!(batch.resolve(ids[1], Ok(())), Advance::Start(ids[0]));

        assert_eq!(batch.park(ids[0]), Advance::Wait);
        assert_eq!(
            batch.resolve(ids[0], Err(collision())),
            Advance::Complete,
            "a second collision is terminal, so a batch cannot loop"
        );
        assert_eq!(batch.failures(), vec![failure(ids[0], &collision())]);
    }

    #[test]
    fn a_requeued_member_starts_at_once_when_nothing_is_applying() {
        let ids = uuids(1);
        let mut batch = started(&ids);
        assert_eq!(batch.park(ids[0]), Advance::Wait);
        assert_eq!(
            batch.resolve(ids[0], Err(collision())),
            Advance::Start(ids[0])
        );
    }

    /// Only a collision while resolving earns the retry. The same text from a
    /// member that was applying, or any other failure, is final.
    #[test]
    fn only_a_resolving_member_that_collided_is_requeued() {
        let ids = uuids(2);
        let mut batch = started(&ids);
        assert_eq!(
            batch.resolve(ids[0], Err(collision())),
            Advance::Start(ids[1])
        );
        assert_eq!(batch.park(ids[1]), Advance::Wait);
        assert_eq!(
            batch.resolve(ids[1], Err("timed out".into())),
            Advance::Complete
        );
        assert_eq!(batch.failures().len(), 2);
    }

    /// A permanent failure on a middle member must not halt the batch — one
    /// bad change should not block the rest. The completion event lists
    /// the failure.
    #[test]
    fn failed_member_does_not_block_remainder() {
        let ids = uuids(3);
        let mut batch = started(&ids);

        assert_eq!(batch.resolve(ids[0], Ok(())), Advance::Start(ids[1]));
        assert_eq!(
            batch.resolve(ids[1], Err("merge failed unrecoverably".into())),
            Advance::Start(ids[2])
        );
        assert_eq!(batch.resolve(ids[2], Ok(())), Advance::Complete);
        assert_eq!(batch.applied_ids(), vec![ids[0], ids[2]]);
        assert_eq!(
            batch.failures(),
            vec![failure(ids[1], "merge failed unrecoverably")],
        );
    }

    /// Replayed events during restart recovery must not double-count. The
    /// same event landing twice must produce the same final state.
    #[test]
    fn record_methods_are_idempotent_under_replay() {
        let ids = uuids(2);
        let mut batch = BatchProgress::new(Uuid::new_v4(), ids.clone(), None);

        batch.record_applied(ids[0]);
        batch.record_applied(ids[0]);
        batch.record_applied(ids[0]);
        assert_eq!(batch.applied_ids(), vec![ids[0]]);

        batch.record_failed(ids[1], "first error".into());
        batch.record_failed(ids[1], "duplicate".into());
        batch.record_failed(ids[1], "another".into());
        assert_eq!(
            batch.failures(),
            vec![failure(ids[1], "first error")],
            "first failure wins — a replay must not overwrite the original cause"
        );

        assert!(batch.is_complete());
    }

    /// State-change return is what the driver uses to skip re-advancing on
    /// a duplicate terminal event.
    #[test]
    fn record_methods_return_true_only_on_state_change() {
        let ids = uuids(2);
        let unrelated = Uuid::new_v4();
        let mut batch = BatchProgress::new(Uuid::new_v4(), ids.clone(), None);

        assert!(batch.record_applied(ids[0]), "first apply changes state");
        assert!(
            !batch.record_applied(ids[0]),
            "second apply on same id is a no-op"
        );
        assert!(
            !batch.record_failed(ids[0], "stale".into()),
            "failed-after-applied must not flip state, must not signal change"
        );
        assert!(
            !batch.record_applied(unrelated),
            "non-member must not signal change"
        );
        assert!(batch.record_failed(ids[1], "real failure".into()));
        assert!(
            !batch.record_applied(ids[1]),
            "applied-after-failed is no-op"
        );
    }

    /// The reported sequence: X applies, the driver starts B, then X's apply
    /// discards its same-thread sibling Y. Y's failure must not start B again,
    /// since B is already being applied.
    #[test]
    fn a_queued_member_resolving_out_of_turn_does_not_restart_the_in_flight_one() {
        let ids = uuids(3);
        let (x, b, y) = (ids[0], ids[1], ids[2]);
        let mut batch = started(&ids);

        assert_eq!(batch.resolve(x, Ok(())), Advance::Start(b));
        assert_eq!(
            batch.resolve(y, Err("discarded".into())),
            Advance::Wait,
            "b is in flight; naming it again would start a second apply"
        );
        assert_eq!(batch.resolve(b, Ok(())), Advance::Complete);
    }

    #[test]
    fn resolve_moves_on_only_when_the_in_flight_member_resolves() {
        let ids = uuids(4);
        let mut batch = started(&ids);

        assert_eq!(batch.resolve(ids[2], Err("gone".into())), Advance::Wait);
        assert_eq!(batch.resolve(ids[0], Ok(())), Advance::Start(ids[1]));
        assert_eq!(
            batch.resolve(ids[1], Ok(())),
            Advance::Start(ids[3]),
            "the member resolved out of turn is skipped"
        );
        assert_eq!(batch.resolve(ids[1], Ok(())), Advance::Duplicate);
        assert_eq!(batch.resolve(ids[3], Ok(())), Advance::Complete);
    }

    /// Cross-state guard: a change can't be both applied AND failed. The
    /// first terminal event wins; the second is a no-op.
    #[test]
    fn applied_then_failed_keeps_applied() {
        let ids = uuids(1);
        let mut batch = BatchProgress::new(Uuid::new_v4(), ids.clone(), None);

        batch.record_applied(ids[0]);
        batch.record_failed(ids[0], "race condition".into());

        assert_eq!(batch.applied_ids(), vec![ids[0]]);
        assert!(
            batch.failures().is_empty(),
            "applied wins over later failed"
        );
        assert!(batch.is_complete());
    }

    #[test]
    fn failed_then_applied_keeps_failed() {
        let ids = uuids(1);
        let mut batch = BatchProgress::new(Uuid::new_v4(), ids.clone(), None);

        batch.record_failed(ids[0], "primary error".into());
        batch.record_applied(ids[0]);

        assert!(batch.applied_ids().is_empty());
        assert_eq!(batch.failures(), vec![failure(ids[0], "primary error")]);
        assert!(batch.is_complete());
    }

    /// Recovery restores what a restart found: the owned members, then the
    /// start rule picks the next member only if none of them is applying.
    #[test]
    fn restored_members_decide_what_recovery_starts() {
        let ids = uuids(3);
        let mut batch = BatchProgress::new(Uuid::new_v4(), ids.clone(), None);
        batch.restore_owned(ids[0], true);
        assert_eq!(
            batch.start_next(),
            Some(ids[1]),
            "a resolver does not hold the queue"
        );

        let mut batch = BatchProgress::new(Uuid::new_v4(), ids.clone(), None);
        batch.restore_owned(ids[0], true);
        batch.restore_owned(ids[1], false);
        assert_eq!(
            batch.start_next(),
            None,
            "the member hardening holds the queue"
        );
        assert_eq!(batch.resolve(ids[1], Ok(())), Advance::Start(ids[2]));
    }

    /// The old recovery only ever waited on the first pending member. A live
    /// session on a later, queued member's thread is not this batch's work.
    /// Holding the queue for it would stall the batch: nothing ever drives it.
    #[test]
    fn only_the_earliest_unparked_member_can_hold_the_queue_at_recovery() {
        let ids = uuids(3);
        let mut batch = BatchProgress::new(Uuid::new_v4(), ids.clone(), None);
        batch.restore_owned(ids[2], false);
        assert!(snapshot_of(&batch).applying_change_ids.is_empty());
        assert_eq!(batch.start_next(), Some(ids[0]));
    }

    /// `contains` must return false for non-members so the driver doesn't
    /// route a single-Apply `ChangeApplied` through batch advancement.
    #[test]
    fn contains_returns_false_for_non_members() {
        let members = uuids(2);
        let unrelated = Uuid::new_v4();
        let batch = BatchProgress::new(Uuid::new_v4(), members.clone(), None);

        assert!(batch.contains(members[0]));
        assert!(batch.contains(members[1]));
        assert!(!batch.contains(unrelated));
    }

    /// Registry routing: when a member's `ChangeApplied` lands, the driver
    /// asks the registry which batch (if any) contains it.
    #[test]
    fn registry_routes_change_to_owning_batch() {
        let ids_a = uuids(2);
        let ids_b = uuids(2);
        let batch_a = BatchProgress::new(Uuid::new_v4(), ids_a.clone(), None);
        let batch_b = BatchProgress::new(Uuid::new_v4(), ids_b.clone(), None);
        let a_id = batch_a.batch_id();
        let b_id = batch_b.batch_id();

        let mut reg = ApplyAllRegistry::default();
        reg.insert(batch_a);
        reg.insert(batch_b);

        assert_eq!(reg.batch_for_change(ids_a[0]), Some(a_id));
        assert_eq!(reg.batch_for_change(ids_a[1]), Some(a_id));
        assert_eq!(reg.batch_for_change(ids_b[0]), Some(b_id));
        assert_eq!(reg.batch_for_change(ids_b[1]), Some(b_id));
        assert_eq!(reg.batch_for_change(Uuid::new_v4()), None);
    }

    /// `remove` returns the final state so the driver can read the complete
    /// applied/failed lists when emitting `ApplyAllBatchCompleted`.
    #[test]
    fn registry_remove_returns_final_progress() {
        let ids = uuids(2);
        let batch_id = Uuid::new_v4();
        let mut batch = BatchProgress::new(batch_id, ids.clone(), None);
        batch.record_applied(ids[0]);
        batch.record_failed(ids[1], "boom".into());

        let mut reg = ApplyAllRegistry::default();
        reg.insert(batch);

        let final_state = reg
            .remove(batch_id)
            .expect("batch must exist before remove");
        assert_eq!(final_state.applied_ids(), vec![ids[0]]);
        assert_eq!(final_state.failures(), vec![failure(ids[1], "boom")]);
        assert!(reg.remove(batch_id).is_none(), "second remove returns None");
    }

    /// `pending_members` returns every unresolved member in order, parked
    /// ones included: the cancel path interrupts and fails them all.
    #[test]
    fn pending_members_returns_unresolved_in_order() {
        let ids = uuids(4);
        let mut batch = started(&ids);
        assert_eq!(batch.pending_members(), ids);

        assert_eq!(batch.park(ids[0]), Advance::Start(ids[1]));
        batch.record_failed(ids[2], "boom".into());
        assert_eq!(batch.pending_members(), vec![ids[0], ids[1], ids[3]]);

        for id in [ids[0], ids[1], ids[3]] {
            batch.record_failed(id, "Apply All canceled".into());
        }
        assert!(batch.pending_members().is_empty());
        assert!(
            batch.is_complete(),
            "canceling all pending completes the batch"
        );
    }

    /// `batch_ids` lists every live batch so cancel-all can drain them.
    #[test]
    fn registry_batch_ids_lists_all_live_batches() {
        let batch_a = BatchProgress::new(Uuid::new_v4(), uuids(1), None);
        let batch_b = BatchProgress::new(Uuid::new_v4(), uuids(1), None);
        let a_id = batch_a.batch_id();
        let b_id = batch_b.batch_id();
        let mut reg = ApplyAllRegistry::default();
        reg.insert(batch_a);
        reg.insert(batch_b);

        let mut ids = reg.batch_ids();
        ids.sort();
        let mut want = vec![a_id, b_id];
        want.sort();
        assert_eq!(ids, want);

        reg.remove(a_id);
        assert_eq!(reg.batch_ids(), vec![b_id]);
    }

    /// The Lucidos menu reads every member's state off the snapshot, so it
    /// keeps submission order and sorts members into their lists.
    #[test]
    fn snapshot_lists_members_in_order_by_state() {
        let mut reg = ApplyAllRegistry::default();
        assert_eq!(reg.snapshot(), None, "no batch, no snapshot");

        let ids = uuids(5);
        let mut batch = started(&ids);
        batch.resolve(ids[0], Ok(()));
        batch.resolve(ids[1], Err("boom".into()));
        batch.park(ids[2]);
        reg.insert(batch);

        let snapshot = reg.snapshot().expect("a batch is running");
        assert_eq!(snapshot.change_ids, ids);
        assert_eq!(snapshot.resolved_change_ids, vec![ids[0], ids[1]]);
        assert_eq!(snapshot.resolving_change_ids, vec![ids[2]]);
        assert_eq!(snapshot.applying_change_ids, vec![ids[3]]);
        let json = serde_json::to_value(&snapshot).unwrap();
        assert_eq!(
            json["resolving_change_ids"].as_array().map(Vec::len),
            Some(1)
        );
        assert_eq!(
            json["applying_change_ids"].as_array().map(Vec::len),
            Some(1)
        );
    }

    /// Empty batch is trivially complete — guards against a no-op
    /// `apply_all_changes` call (zero pending) emitting an
    /// `ApplyAllBatchStarted` that never resolves.
    #[test]
    fn empty_batch_is_complete_immediately() {
        let mut batch = BatchProgress::new(Uuid::new_v4(), vec![], None);
        assert!(batch.is_complete());
        assert_eq!(batch.start_next(), None);
    }

    /// Failure serializes with named fields, not positional tuple
    /// elements — keeps the wire format self-describing.
    #[test]
    fn failure_serializes_with_named_fields() {
        let f = failure(Uuid::nil(), "some error");
        let json = serde_json::to_value(&f).unwrap();
        assert_eq!(json["change_id"], Uuid::nil().to_string());
        assert_eq!(json["error"], "some error");
        assert!(json.get(0).is_none(), "must NOT serialize as a tuple array");
    }
}
