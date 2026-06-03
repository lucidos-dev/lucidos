//! Apply-All batch progress tracker.
//!
//! Pure-logic state machine for an in-flight Apply All batch — the user
//! clicked Apply All, the engine emitted `ApplyAllBatchStarted` with every
//! pending change ID, and now each member is being applied one at a time.
//! The driver task (`engine::apply_all_driver`) advances the batch as
//! `ChangeApplied` / `ChangeApplyFailed` events land, and emits
//! `ApplyAllBatchCompleted` when every member has resolved.
//!
//! Member status is one-shot: the first terminal event wins. A late race
//! event for the same member is a no-op, and replayed events during
//! restart recovery produce the same final state as the live flow.

use crate::engine::thread_events::MessageOrigin;
use serde::Serialize;
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

/// Lifecycle state of one batch member. `Pending` is the initial state;
/// `Applied` / `Failed` are terminal — once set, a second `record_*` for
/// the same member is a no-op (first-write-wins).
#[derive(Debug, Clone, PartialEq, Eq)]
enum MemberStatus {
    Pending,
    Applied,
    Failed(String),
}

impl MemberStatus {
    fn is_terminal(&self) -> bool {
        !matches!(self, MemberStatus::Pending)
    }
}

/// In-memory state for a single in-flight Apply All batch. Order-preserving:
/// `next_pending()` returns members in submission order so the user-visible
/// progression matches the pending-list order they saw in the UI.
#[derive(Debug, Clone)]
pub(crate) struct BatchProgress {
    batch_id: Uuid,
    actor: Option<MessageOrigin>,
    /// (change_id, status) pairs in submission order. One Vec covers
    /// "members in order", "applied set", "failed list" — every query
    /// becomes a single pass, no per-element cross-lookup between two
    /// structures.
    members: Vec<(Uuid, MemberStatus)>,
}

impl BatchProgress {
    pub(crate) fn new(batch_id: Uuid, change_ids: Vec<Uuid>, actor: Option<MessageOrigin>) -> Self {
        Self {
            batch_id,
            actor,
            members: change_ids
                .into_iter()
                .map(|id| (id, MemberStatus::Pending))
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
        self.members.iter().any(|(id, _)| *id == change_id)
    }

    fn status_mut(&mut self, change_id: Uuid) -> Option<&mut MemberStatus> {
        self.members
            .iter_mut()
            .find(|(id, _)| *id == change_id)
            .map(|(_, status)| status)
    }

    /// Mark `change_id` applied. Returns `true` if state changed; `false`
    /// when the change isn't a member or was already terminal. The driver
    /// uses the return value to skip re-advancing the batch on a duplicate
    /// terminal event — without that gate a `ChangeApplied` racing a stale
    /// `ChangeApplyFailed` (or vice versa) for the same id would re-spawn
    /// `apply_change` on the next pending member.
    pub(crate) fn record_applied(&mut self, change_id: Uuid) -> bool {
        if let Some(status @ MemberStatus::Pending) = self.status_mut(change_id) {
            *status = MemberStatus::Applied;
            true
        } else {
            false
        }
    }

    /// Mark `change_id` failed with `error`. Same first-write-wins +
    /// state-change return as `record_applied`.
    pub(crate) fn record_failed(&mut self, change_id: Uuid, error: String) -> bool {
        if let Some(status @ MemberStatus::Pending) = self.status_mut(change_id) {
            *status = MemberStatus::Failed(error);
            true
        } else {
            false
        }
    }

    /// The next change to apply, or `None` if every member has resolved.
    /// Order-preserving — earliest unresolved member from the original list.
    pub(crate) fn next_pending(&self) -> Option<Uuid> {
        self.members.iter().find_map(|(id, status)| match status {
            MemberStatus::Pending => Some(*id),
            _ => None,
        })
    }

    /// True when every member has resolved. The driver emits
    /// `ApplyAllBatchCompleted` and removes the batch from the registry.
    pub(crate) fn is_complete(&self) -> bool {
        self.members.iter().all(|(_, status)| status.is_terminal())
    }

    pub(crate) fn applied_ids(&self) -> Vec<Uuid> {
        self.members
            .iter()
            .filter_map(|(id, status)| matches!(status, MemberStatus::Applied).then_some(*id))
            .collect()
    }

    pub(crate) fn failures(&self) -> Vec<ApplyFailure> {
        self.members
            .iter()
            .filter_map(|(id, status)| match status {
                MemberStatus::Failed(error) => Some(ApplyFailure {
                    change_id: *id,
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
}

#[cfg(test)]
mod tests {
    use super::*;

    fn uuids(n: usize) -> Vec<Uuid> {
        (0..n).map(|_| Uuid::new_v4()).collect()
    }

    fn failure(change_id: Uuid, error: &str) -> ApplyFailure {
        ApplyFailure {
            change_id,
            error: error.to_string(),
        }
    }

    #[test]
    fn batch_progresses_through_all_members_in_order() {
        let ids = uuids(3);
        let mut batch = BatchProgress::new(Uuid::new_v4(), ids.clone(), None);

        assert_eq!(batch.next_pending(), Some(ids[0]));
        batch.record_applied(ids[0]);
        assert_eq!(batch.next_pending(), Some(ids[1]));
        batch.record_applied(ids[1]);
        assert_eq!(batch.next_pending(), Some(ids[2]));
        batch.record_applied(ids[2]);
        assert_eq!(batch.next_pending(), None);
        assert!(batch.is_complete());
        assert_eq!(batch.applied_ids(), ids);
        assert!(batch.failures().is_empty());
    }

    /// After a conflict suspends the batch, recovery completes and
    /// `ChangeApplied` lands for the conflict member. The batch must
    /// advance to the next member — without this, Apply All abandons the
    /// remainder even after the conflict resolves.
    #[test]
    fn batch_resumes_after_conflict_resolves() {
        let ids = uuids(3);
        let mut batch = BatchProgress::new(Uuid::new_v4(), ids.clone(), None);

        batch.record_applied(ids[0]);
        assert_eq!(batch.next_pending(), Some(ids[1]));

        batch.record_applied(ids[1]);
        assert_eq!(
            batch.next_pending(),
            Some(ids[2]),
            "batch must continue after the conflict member resolves"
        );
        batch.record_applied(ids[2]);
        assert!(batch.is_complete());
    }

    /// A permanent failure on a middle member must not halt the batch — one
    /// bad change should not block the rest. The completion event lists
    /// the failure.
    #[test]
    fn failed_member_does_not_block_remainder() {
        let ids = uuids(3);
        let mut batch = BatchProgress::new(Uuid::new_v4(), ids.clone(), None);

        batch.record_applied(ids[0]);
        batch.record_failed(ids[1], "merge failed unrecoverably".into());

        assert_eq!(batch.next_pending(), Some(ids[2]));
        batch.record_applied(ids[2]);
        assert!(batch.is_complete());
        assert_eq!(batch.applied_ids(), vec![ids[0], ids[2]]);
        assert_eq!(
            batch.failures(),
            vec![failure(ids[1], "merge failed unrecoverably")],
        );
    }

    /// Replayed events during restart recovery must not double-count. The
    /// recovery sweep replays `ApplyAllBatchStarted` plus every
    /// `ChangeApplied` / `ChangeApplyFailed` from the event store; the
    /// same event landing twice (replay) must produce the same final state.
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
    /// a duplicate terminal event. Without this signal a `ChangeApplied`
    /// followed by a stale `ChangeApplyFailed` for the same id would push
    /// `next_pending` twice and the driver would re-spawn `apply_change`
    /// on the next member, racing the in-flight call.
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
        assert!(!batch.record_applied(ids[1]), "applied-after-failed is no-op");
    }

    /// Cross-state guard: a change can't be both applied AND failed. The
    /// first terminal event wins; the second is a no-op. Guards against
    /// races between the conflict cleanup ff-merge emitting `ChangeApplied`
    /// and a stale `ChangeApplyFailed` for the same change_id.
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

    /// Empty batch is trivially complete — guards against a no-op
    /// `apply_all_changes` call (zero pending) emitting an
    /// `ApplyAllBatchStarted` that never resolves.
    #[test]
    fn empty_batch_is_complete_immediately() {
        let batch = BatchProgress::new(Uuid::new_v4(), vec![], None);
        assert!(batch.is_complete());
        assert_eq!(batch.next_pending(), None);
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
