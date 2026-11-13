//! Rebuild the lane from events after a restart.
//!
//! The lane keeps no table. Its queue and its resolving batch are a fold over
//! the lane's own events and the `ChangeApplied` of its members, in sequence
//! order. A change whose row is no longer pending leaves the queue. What the
//! open batch does next follows from its lane thread ([`batch_recovery`]).

use super::state::{Lane, Place, ResolvingBatch};
use super::LaneMember;
use std::collections::{HashMap, HashSet};
use uuid::Uuid;

/// One event the lane's state depends on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum LaneRecord {
    Queued(LaneMember),
    Unqueued(Uuid),
    Applied(Uuid),
    BatchStarted {
        batch_id: Uuid,
        base_sha: String,
        lane_thread_id: Uuid,
        members: Vec<LaneMember>,
    },
    BatchEnded(Uuid),
}

/// Fold `records`, oldest first, into the lane they describe.
pub(crate) fn fold(records: &[LaneRecord], is_pending: impl Fn(Uuid) -> bool) -> Lane {
    let mut lane = Lane::default();
    for record in records {
        match record {
            LaneRecord::Queued(member) => lane.wait(member.clone()),
            LaneRecord::Unqueued(id) => {
                lane.unqueue(*id);
            }
            LaneRecord::Applied(id) => lane.landed(*id),
            LaneRecord::BatchStarted {
                batch_id,
                base_sha,
                lane_thread_id,
                members,
            } => lane.open_batch(ResolvingBatch::new(
                *batch_id,
                base_sha.clone(),
                *lane_thread_id,
                members.clone(),
            )),
            LaneRecord::BatchEnded(batch_id) => {
                lane.end_batch(*batch_id);
            }
        }
    }
    lane.retain_waiting(is_pending);
    lane
}

/// What recovery hands the driver: the lane, every batch this workspace
/// opened, open or closed, and the members a failed batch left queued.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct LaneRestore {
    pub(crate) lane: Lane,
    /// Only these batches' lane worktrees are this workspace's to retire.
    pub(crate) opened_here: HashSet<Uuid>,
    /// Pending changes whose latest lane event queued them, yet the lane
    /// holds them nowhere. A crash between `LaneBatchFailed` and a member's
    /// `ChangeUnqueued` leaves one.
    pub(crate) stranded: Vec<Uuid>,
}

impl LaneRestore {
    pub(crate) fn of(records: &[LaneRecord], is_pending: impl Fn(Uuid) -> bool) -> Self {
        let lane = fold(records, &is_pending);
        let stranded = stranded(records, &lane, &is_pending);
        Self {
            lane,
            opened_here: records
                .iter()
                .filter_map(|record| match record {
                    LaneRecord::BatchStarted { batch_id, .. } => Some(*batch_id),
                    _ => None,
                })
                .collect(),
            stranded,
        }
    }
}

/// The pending changes in `records` still queued by their latest lane event
/// that `lane` holds nowhere, in the order they were first queued.
fn stranded(records: &[LaneRecord], lane: &Lane, is_pending: impl Fn(Uuid) -> bool) -> Vec<Uuid> {
    let mut order = Vec::new();
    let mut queued = HashMap::new();
    for record in records {
        let (change_id, is_queued) = match record {
            LaneRecord::Queued(member) => (member.change_id, true),
            LaneRecord::Unqueued(id) | LaneRecord::Applied(id) => (*id, false),
            LaneRecord::BatchStarted { .. } | LaneRecord::BatchEnded(_) => continue,
        };
        if queued.insert(change_id, is_queued).is_none() {
            order.push(change_id);
        }
    }
    order
        .into_iter()
        .filter(|id| queued[id] && is_pending(*id) && lane.place(*id) == Place::Absent)
        .collect()
}

/// Where the open batch's lane thread stood when the engine stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LaneThreadState {
    /// The batch formed, but its lane thread never got its first prompt.
    NotOpened,
    /// Its last turn ended cleanly and nothing ran after it.
    Finished,
    /// A turn was cut off, stopped or failed, or waits on a question. Agent
    /// recovery and the thread's own Continue own it from here.
    Unfinished,
    /// It was archived or deleted, and the engine stopped before the lane
    /// heard of it.
    Closed,
}

/// What a restart does with the open batch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BatchRecovery {
    /// Open the lane thread, as the batch's start would have.
    OpenLaneThread,
    /// Land it, as the lane thread's clean completion would have.
    Land,
    /// Wait for the lane thread: a user switch resumes it, and a crash leaves
    /// it a Continue. The lane opens no second batch meanwhile.
    AwaitLaneThread,
    /// Fail it whole, as archiving or deleting the lane thread does.
    Fail,
}

/// The open batch's next step after a restart. Never a second resolution:
/// a crashed lane thread is never resumed by the lane, only by its own
/// cause-gated recovery or a Continue.
pub(crate) fn batch_recovery(thread: LaneThreadState) -> BatchRecovery {
    match thread {
        LaneThreadState::NotOpened => BatchRecovery::OpenLaneThread,
        LaneThreadState::Finished => BatchRecovery::Land,
        LaneThreadState::Unfinished => BatchRecovery::AwaitLaneThread,
        LaneThreadState::Closed => BatchRecovery::Fail,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn member(n: u8) -> LaneMember {
        LaneMember {
            change_id: Uuid::from_bytes([n; 16]),
            snapshot_sha: format!("snap{n}"),
        }
    }

    fn id(n: u8) -> Uuid {
        member(n).change_id
    }

    const BATCH: Uuid = Uuid::from_bytes([9; 16]);
    const LANE_THREAD: Uuid = Uuid::from_bytes([8; 16]);

    fn started(members: Vec<LaneMember>) -> LaneRecord {
        LaneRecord::BatchStarted {
            batch_id: BATCH,
            base_sha: "base".into(),
            lane_thread_id: LANE_THREAD,
            members,
        }
    }

    fn all_pending(_: Uuid) -> bool {
        true
    }

    /// The full life of a lane, cut at every step: what a restart at that
    /// moment rebuilds. One clean member lands, two conflict and resolve as a
    /// batch, and a fourth waits behind it.
    #[test]
    fn a_restart_at_every_lane_step_rebuilds_that_step() {
        let history = [
            LaneRecord::Queued(member(1)),
            LaneRecord::Applied(id(1)),
            LaneRecord::Queued(member(2)),
            LaneRecord::Queued(member(3)),
            started(vec![member(2), member(3)]),
            LaneRecord::Queued(member(4)),
            LaneRecord::Applied(id(2)),
            LaneRecord::Applied(id(3)),
            LaneRecord::BatchEnded(BATCH),
            LaneRecord::Applied(id(4)),
        ];
        let expected: Vec<(Vec<LaneMember>, Option<Vec<LaneMember>>)> = vec![
            (vec![member(1)], None),
            (vec![], None),
            (vec![member(2)], None),
            (vec![member(2), member(3)], None),
            (vec![], Some(vec![member(2), member(3)])),
            (vec![member(4)], Some(vec![member(2), member(3)])),
            (vec![member(4)], Some(vec![member(3)])),
            (vec![member(4)], Some(vec![])),
            (vec![member(4)], None),
            (vec![], None),
        ];
        for (cut, (waiting, unlanded)) in expected.into_iter().enumerate() {
            let lane = fold(&history[..=cut], all_pending);
            assert_eq!(
                lane.waiting(),
                waiting.as_slice(),
                "waiting after step {cut}"
            );
            assert_eq!(
                lane.resolving()
                    .map(|b| b.unlanded().cloned().collect::<Vec<_>>()),
                unlanded,
                "resolving after step {cut}"
            );
        }
    }

    /// Killed at batch open: the batch is back, with its lane thread, and
    /// the lane thread still has to be opened.
    #[test]
    fn a_restart_at_batch_open_opens_the_lane_thread() {
        let history = [LaneRecord::Queued(member(2)), started(vec![member(2)])];
        let lane = fold(&history, all_pending);
        let batch = lane.resolving().expect("the batch survives");
        assert_eq!(batch.lane_thread_id, LANE_THREAD);
        assert_eq!(
            batch_recovery(LaneThreadState::NotOpened),
            BatchRecovery::OpenLaneThread
        );
    }

    /// Killed mid-resolution: the batch stays open and blocks a second one,
    /// and the lane leaves the thread to its own recovery.
    #[test]
    fn a_restart_mid_resolution_waits_for_the_lane_thread() {
        let history = [
            LaneRecord::Queued(member(2)),
            started(vec![member(2)]),
            LaneRecord::Queued(member(5)),
        ];
        let mut lane = fold(&history, all_pending);
        assert_eq!(
            batch_recovery(LaneThreadState::Unfinished),
            BatchRecovery::AwaitLaneThread
        );
        assert!(lane.take_pass().is_empty(), "no second batch meanwhile");
        assert_eq!(lane.place(id(5)), Place::Waiting);
    }

    /// Killed after the lane thread finished but before the landing: the
    /// restart lands it, exactly as the completion would have.
    #[test]
    fn a_restart_after_a_clean_finish_lands_the_batch() {
        assert_eq!(
            batch_recovery(LaneThreadState::Finished),
            BatchRecovery::Land
        );
    }

    /// Archived or deleted while the engine was going down: the restart fails
    /// the batch, as the archive would have, and never reopens the thread.
    #[test]
    fn a_restart_after_the_lane_thread_closed_fails_the_batch() {
        assert_eq!(batch_recovery(LaneThreadState::Closed), BatchRecovery::Fail);
    }

    /// Killed between two members' `ChangeApplied`: only the rest are left
    /// to announce, and the batch is still open for its verdict.
    #[test]
    fn a_restart_between_announcements_leaves_only_the_rest() {
        let history = [
            started(vec![member(2), member(3)]),
            LaneRecord::Applied(id(2)),
        ];
        let lane = fold(&history, all_pending);
        let batch = lane.resolving().expect("still open");
        assert_eq!(
            batch.unlanded().cloned().collect::<Vec<_>>(),
            vec![member(3)]
        );
    }

    /// A change discarded or applied elsewhere while queued does not come back.
    #[test]
    fn a_change_no_longer_pending_leaves_the_queue() {
        let history = [LaneRecord::Queued(member(1)), LaneRecord::Queued(member(2))];
        let lane = fold(&history, |change| change != id(1));
        assert_eq!(lane.waiting(), &[member(2)]);
    }

    #[test]
    fn an_unqueued_change_is_not_rebuilt() {
        let history = [LaneRecord::Queued(member(1)), LaneRecord::Unqueued(id(1))];
        assert!(fold(&history, all_pending).waiting().is_empty());
    }

    /// Apply pressed again after a failed batch queues a fresh snapshot.
    #[test]
    fn a_requeue_after_a_failed_batch_takes_the_new_snapshot() {
        let again = LaneMember {
            snapshot_sha: "snap2-again".into(),
            ..member(2)
        };
        let history = [
            LaneRecord::Queued(member(2)),
            started(vec![member(2)]),
            LaneRecord::BatchEnded(BATCH),
            LaneRecord::Unqueued(id(2)),
            LaneRecord::Queued(again.clone()),
        ];
        let lane = fold(&history, all_pending);
        assert_eq!(lane.waiting(), &[again]);
        assert!(lane.resolving().is_none());
    }

    /// A restart retires lane worktrees of batches this workspace opened,
    /// open ones included in the set, and no other batch's.
    #[test]
    fn a_restore_knows_every_batch_this_workspace_opened() {
        let closed = Uuid::from_bytes([7; 16]);
        let history = [
            LaneRecord::BatchStarted {
                batch_id: closed,
                base_sha: "b".into(),
                lane_thread_id: Uuid::new_v4(),
                members: vec![member(1)],
            },
            LaneRecord::BatchEnded(closed),
            started(vec![member(2)]),
        ];
        let restore = LaneRestore::of(&history, all_pending);
        assert_eq!(restore.opened_here, HashSet::from([closed, BATCH]));
        assert_eq!(restore.lane.resolving().map(|b| b.batch_id), Some(BATCH));
        assert_eq!(LaneRestore::of(&[], all_pending), LaneRestore::default());
    }

    /// Killed between `LaneBatchFailed` and the members' `ChangeUnqueued`:
    /// each member still reads as queued, yet no lane holds it. The restart
    /// names those still pending, and no member that left, landed or waits.
    #[test]
    fn a_restart_names_the_members_a_failed_batch_left_queued() {
        let history = [
            LaneRecord::Queued(member(1)),
            LaneRecord::Queued(member(2)),
            LaneRecord::Queued(member(3)),
            started(vec![member(1), member(2), member(3)]),
            LaneRecord::Applied(id(3)),
            LaneRecord::Queued(member(4)),
            LaneRecord::BatchEnded(BATCH),
            LaneRecord::Unqueued(id(2)),
            LaneRecord::Queued(member(5)),
            LaneRecord::Queued(member(6)),
            started(vec![member(6)]),
        ];
        assert_eq!(LaneRestore::of(&history, all_pending).stranded, vec![id(1)]);
        let gone = LaneRestore::of(&history, |change| change != id(1));
        assert!(gone.stranded.is_empty(), "no longer pending, so not queued");
    }

    /// A failed batch's members come back neither queued nor resolving.
    #[test]
    fn a_failed_batch_is_not_rebuilt() {
        let history = [
            LaneRecord::Queued(member(2)),
            started(vec![member(2)]),
            LaneRecord::BatchEnded(BATCH),
            LaneRecord::Unqueued(id(2)),
        ];
        let lane = fold(&history, all_pending);
        assert!(lane.resolving().is_none());
        assert_eq!(lane.place(id(2)), Place::Absent);
    }
}
