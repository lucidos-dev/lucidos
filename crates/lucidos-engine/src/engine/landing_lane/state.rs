//! The landing lane's pure decisions: which change waits, which changes form
//! a batch, and when a batch ends. No git and no events here. The driver feeds
//! facts in and acts on what comes out.

use std::collections::{BTreeSet, HashSet};

use super::LaneMember;
use uuid::Uuid;

/// Where a change stands in the lane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Place {
    Waiting,
    /// In the resolving batch, not landed yet.
    Resolving,
    Absent,
}

/// Why the user cannot remove a change from the lane one by one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LaneRemoval {
    /// It lands with the resolving batch, which goes whole or not at all.
    InBatch,
    /// It is not waiting in the lane.
    NotWaiting,
}

/// Which of some changes the lane holds, for an Apply All Cancel.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct LaneHold {
    pub(crate) waiting: Vec<Uuid>,
    /// Those in the open batch, still to land.
    pub(crate) in_batch: Vec<Uuid>,
    /// The open batch, when it holds any of them.
    pub(crate) batch: Option<Uuid>,
}

impl LaneHold {
    /// Every change the lane holds, waiting or in the batch.
    pub(crate) fn members(&self) -> Vec<Uuid> {
        self.waiting.iter().chain(&self.in_batch).copied().collect()
    }
}

/// The changes a clean pass could not merge, resolving together on one
/// integration branch. The batch lands or fails whole, as its lane thread
/// decides.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ResolvingBatch {
    pub(crate) batch_id: Uuid,
    /// `main` when the batch formed, where its integration branch starts.
    pub(crate) base_sha: String,
    pub(crate) lane_thread_id: Uuid,
    members: Vec<LaneMember>,
    /// Members whose `ChangeApplied` is recorded. Only a restart that cut a
    /// landing off between two members leaves some here and some not.
    landed: Vec<Uuid>,
}

impl ResolvingBatch {
    pub(crate) fn new(
        batch_id: Uuid,
        base_sha: String,
        lane_thread_id: Uuid,
        members: Vec<LaneMember>,
    ) -> Self {
        Self {
            batch_id,
            base_sha,
            lane_thread_id,
            members,
            landed: Vec::new(),
        }
    }

    pub(crate) fn members(&self) -> impl Iterator<Item = &LaneMember> {
        self.members.iter()
    }

    /// The members still to land, in batch order.
    pub(crate) fn unlanded(&self) -> impl Iterator<Item = &LaneMember> {
        self.members
            .iter()
            .filter(|m| !self.landed.contains(&m.change_id))
    }

    pub(crate) fn all_landed(&self) -> bool {
        self.unlanded().next().is_none()
    }

    fn mark_landed(&mut self, change_id: Uuid) {
        if self.members.iter().any(|m| m.change_id == change_id)
            && !self.landed.contains(&change_id)
        {
            self.landed.push(change_id);
        }
    }
}

/// The queue, at most one resolving batch, and the open Apply All presses.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Lane {
    waiting: Vec<LaneMember>,
    resolving: Option<ResolvingBatch>,
    /// Presses still handing changes over. While one is open the lane opens
    /// no batch, so a press's conflicts resolve together (ADR 0480).
    presses: BTreeSet<Uuid>,
}

impl Lane {
    pub(crate) fn waiting(&self) -> &[LaneMember] {
        &self.waiting
    }

    pub(crate) fn resolving(&self) -> Option<&ResolvingBatch> {
        self.resolving.as_ref()
    }

    /// The resolving batch, when it is `batch_id`.
    pub(crate) fn resolving_batch(&self, batch_id: Uuid) -> Option<&ResolvingBatch> {
        self.resolving.as_ref().filter(|b| b.batch_id == batch_id)
    }

    /// The resolving batch `thread_id` is the lane thread of.
    pub(crate) fn batch_of_lane_thread(&self, thread_id: Uuid) -> Option<Uuid> {
        self.resolving
            .as_ref()
            .filter(|b| b.lane_thread_id == thread_id)
            .map(|b| b.batch_id)
    }

    pub(crate) fn place(&self, change_id: Uuid) -> Place {
        if self.waiting.iter().any(|m| m.change_id == change_id) {
            return Place::Waiting;
        }
        let resolving = self
            .resolving
            .as_ref()
            .is_some_and(|b| b.unlanded().any(|m| m.change_id == change_id));
        if resolving {
            Place::Resolving
        } else {
            Place::Absent
        }
    }

    /// Which of `change_ids` the lane holds, and where.
    pub(crate) fn held_of(&self, change_ids: &[Uuid]) -> LaneHold {
        let mut hold = LaneHold::default();
        for &change_id in change_ids {
            match self.place(change_id) {
                Place::Waiting => hold.waiting.push(change_id),
                Place::Resolving => hold.in_batch.push(change_id),
                Place::Absent => {}
            }
        }
        if !hold.in_batch.is_empty() {
            hold.batch = self.resolving.as_ref().map(|b| b.batch_id);
        }
        hold
    }

    /// Whether the user may remove `change_id` on its own. Only a waiting
    /// change may go: a resolving member leaves only with its whole batch.
    pub(crate) fn removal(&self, change_id: Uuid) -> Result<(), LaneRemoval> {
        match self.place(change_id) {
            Place::Waiting => Ok(()),
            Place::Resolving => Err(LaneRemoval::InBatch),
            Place::Absent => Err(LaneRemoval::NotWaiting),
        }
    }

    /// Whether the waiting queue waits: behind the resolving batch, or for an
    /// open press to hand over the rest of its changes.
    pub(crate) fn queue_held(&self) -> bool {
        self.resolving.is_some() || !self.presses.is_empty()
    }

    /// Whether the queue waits for an open press, with no batch resolving.
    pub(crate) fn gathering(&self) -> bool {
        self.resolving.is_none() && !self.presses.is_empty()
    }

    pub(crate) fn open_press(&mut self, press_id: Uuid) {
        self.presses.insert(press_id);
    }

    /// The press `press_id` has nothing left to hand over. True when that
    /// releases the queue for its pass.
    pub(crate) fn submit_press(&mut self, press_id: Uuid) -> bool {
        self.presses.remove(&press_id) && !self.queue_held()
    }

    /// Queue a member. One already waiting keeps its place and its snapshot:
    /// a queued change is frozen.
    pub(crate) fn wait(&mut self, member: LaneMember) {
        if self.place(member.change_id) == Place::Absent {
            self.waiting.push(member);
        }
    }

    /// Every waiting member, for the next clean pass. Empty while the queue
    /// is held: it starts the pass once the batch ends or the press is in.
    pub(crate) fn take_pass(&mut self) -> Vec<LaneMember> {
        if self.queue_held() {
            return Vec::new();
        }
        std::mem::take(&mut self.waiting)
    }

    /// The waiting members named in `ids`, in queue order, even while the
    /// queue is held. The rest keep waiting.
    pub(crate) fn take_members(&mut self, ids: &HashSet<Uuid>) -> Vec<LaneMember> {
        let (taken, rest) = std::mem::take(&mut self.waiting)
            .into_iter()
            .partition(|m| ids.contains(&m.change_id));
        self.waiting = rest;
        taken
    }

    /// Take in what a clean pass could not merge. With the queue free they
    /// form a batch, returned for the driver to announce. Otherwise they wait
    /// behind what already waits, so a batch keeps the order changes arrived.
    pub(crate) fn set_aside(
        &mut self,
        batch_id: Uuid,
        base_sha: String,
        lane_thread_id: Uuid,
        members: Vec<LaneMember>,
    ) -> Option<&ResolvingBatch> {
        if members.is_empty() {
            return None;
        }
        if self.queue_held() {
            self.waiting.extend(members);
            return None;
        }
        self.resolving = Some(ResolvingBatch::new(
            batch_id,
            base_sha,
            lane_thread_id,
            members,
        ));
        self.resolving.as_ref()
    }

    /// Drop a waiting change, as Discard and Set aside do. True when it was
    /// waiting. A resolving member is never dropped here: the lane holds its
    /// claim, which refuses the Discard.
    pub(crate) fn unqueue(&mut self, change_id: Uuid) -> bool {
        let before = self.waiting.len();
        self.waiting.retain(|m| m.change_id != change_id);
        self.waiting.len() != before
    }

    /// A change landed: it leaves the queue, or counts as landed in the batch.
    pub(crate) fn landed(&mut self, change_id: Uuid) {
        self.unqueue(change_id);
        if let Some(batch) = self.resolving.as_mut() {
            batch.mark_landed(change_id);
        }
    }

    /// Open a batch recorded in the events. Its members leave the queue.
    pub(super) fn open_batch(&mut self, batch: ResolvingBatch) {
        self.waiting
            .retain(|w| !batch.members().any(|m| m.change_id == w.change_id));
        self.resolving = Some(batch);
    }

    /// Close the batch `batch_id`, landed or failed, and hand it back. The
    /// queue behind it starts the next pass. A failed batch's members are
    /// not queued again: only an Apply press does that.
    pub(crate) fn end_batch(&mut self, batch_id: Uuid) -> Option<ResolvingBatch> {
        if self.resolving_batch(batch_id).is_some() {
            self.resolving.take()
        } else {
            None
        }
    }

    pub(super) fn retain_waiting(&mut self, keep: impl Fn(Uuid) -> bool) {
        self.waiting.retain(|m| keep(m.change_id));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn member(n: u8) -> LaneMember {
        LaneMember {
            change_id: Uuid::from_bytes([n; 16]),
            snapshot_sha: format!("{n:040}"),
        }
    }

    const BATCH: Uuid = Uuid::from_bytes([9; 16]);
    const LANE_THREAD: Uuid = Uuid::from_bytes([8; 16]);

    fn lane_with_batch(members: Vec<LaneMember>) -> Lane {
        let mut lane = Lane::default();
        lane.set_aside(BATCH, "base".into(), LANE_THREAD, members);
        lane
    }

    const PRESS: Uuid = Uuid::from_bytes([7; 16]);

    /// What conflicts while a press is open waits for the press, and opens no
    /// batch. The queue starts no pass meanwhile.
    #[test]
    fn an_open_press_holds_set_aside_members_back() {
        let mut lane = Lane::default();
        lane.open_press(PRESS);
        let opened = lane.set_aside(BATCH, "base".into(), LANE_THREAD, vec![member(1)]);
        assert!(
            opened.is_none(),
            "no batch while the press hands changes over"
        );
        lane.set_aside(BATCH, "base".into(), LANE_THREAD, vec![member(2)]);
        assert!(lane.resolving().is_none());
        assert!(lane.gathering());
        assert_eq!(lane.waiting(), &[member(1), member(2)], "in press order");
        assert!(lane.take_pass().is_empty(), "the queue waits for the press");
    }

    /// Once the last open press has handed over every change, the queue goes
    /// into one pass, and what still conflicts opens one batch.
    #[test]
    fn the_last_submitted_press_releases_the_queue_into_one_batch() {
        let other = Uuid::from_bytes([6; 16]);
        let mut lane = Lane::default();
        lane.open_press(PRESS);
        lane.open_press(other);
        lane.set_aside(
            BATCH,
            "base".into(),
            LANE_THREAD,
            vec![member(1), member(2)],
        );
        assert!(!lane.submit_press(PRESS), "another press is still open");
        assert!(lane.take_pass().is_empty());
        assert!(lane.submit_press(other));
        assert!(!lane.gathering());
        let pass = lane.take_pass();
        assert_eq!(pass, vec![member(1), member(2)]);
        let batch = lane
            .set_aside(BATCH, "base".into(), LANE_THREAD, pass)
            .expect("one batch");
        assert_eq!(batch.members().count(), 2);
    }

    #[test]
    fn an_unknown_or_repeated_press_changes_nothing() {
        let mut lane = Lane::default();
        lane.wait(member(1));
        let before = lane.clone();
        assert!(!lane.submit_press(PRESS), "never opened");
        assert_eq!(lane, before);
        lane.open_press(PRESS);
        assert!(lane.submit_press(PRESS));
        assert!(!lane.submit_press(PRESS), "already submitted");
        assert_eq!(lane, before);
    }

    /// A press submitted while a batch resolves leaves its queue behind the
    /// batch, which starts the next pass when it ends.
    #[test]
    fn a_press_submitted_behind_a_resolving_batch_releases_nothing() {
        let mut lane = lane_with_batch(vec![member(1)]);
        lane.open_press(PRESS);
        assert!(
            !lane.gathering(),
            "the batch holds the queue, not the press"
        );
        lane.set_aside(Uuid::new_v4(), "b".into(), Uuid::new_v4(), vec![member(2)]);
        assert!(!lane.submit_press(PRESS));
        assert!(lane.take_pass().is_empty());
        assert_eq!(lane.waiting(), &[member(2)]);
    }

    #[test]
    fn with_no_press_open_a_set_aside_member_opens_a_batch() {
        let mut lane = Lane::default();
        let batch = lane.set_aside(BATCH, "base".into(), LANE_THREAD, vec![member(1)]);
        assert!(batch.is_some());
        assert!(!lane.gathering());
    }

    #[test]
    fn a_gathered_change_may_be_removed() {
        let mut lane = Lane::default();
        lane.open_press(PRESS);
        lane.set_aside(BATCH, "base".into(), LANE_THREAD, vec![member(1)]);
        assert_eq!(lane.removal(member(1).change_id), Ok(()));
        assert!(lane.unqueue(member(1).change_id));
    }

    #[test]
    fn at_most_one_batch_resolves() {
        let mut lane = lane_with_batch(vec![member(1)]);
        let second = lane.set_aside(
            Uuid::new_v4(),
            "base2".into(),
            Uuid::new_v4(),
            vec![member(2)],
        );
        assert!(
            second.is_none(),
            "a second batch never opens beside the first"
        );
        assert_eq!(lane.resolving().unwrap().members().count(), 1);
        assert_eq!(lane.waiting(), &[member(2)]);
    }

    #[test]
    fn a_batch_knows_its_lane_thread() {
        let lane = lane_with_batch(vec![member(1)]);
        let batch = lane.resolving_batch(BATCH).expect("open");
        assert_eq!(batch.lane_thread_id, LANE_THREAD);
        assert!(lane.resolving_batch(Uuid::new_v4()).is_none());
        assert_eq!(lane.batch_of_lane_thread(LANE_THREAD), Some(BATCH));
        assert_eq!(lane.batch_of_lane_thread(Uuid::new_v4()), None);
        assert_eq!(Lane::default().batch_of_lane_thread(LANE_THREAD), None);
    }

    #[test]
    fn set_aside_members_wait_in_arrival_order() {
        let mut lane = lane_with_batch(vec![member(1)]);
        lane.wait(member(2));
        lane.set_aside(
            Uuid::new_v4(),
            "base".into(),
            Uuid::new_v4(),
            vec![member(3)],
        );
        assert_eq!(lane.waiting(), &[member(2), member(3)]);
    }

    #[test]
    fn nothing_set_aside_opens_no_batch() {
        let mut lane = Lane::default();
        assert!(lane
            .set_aside(Uuid::new_v4(), "b".into(), Uuid::new_v4(), vec![])
            .is_none());
        assert!(lane.resolving().is_none());
    }

    #[test]
    fn a_queued_change_is_frozen_at_its_first_snapshot() {
        let mut lane = lane_with_batch(vec![member(1)]);
        lane.wait(member(2));
        let moved = LaneMember {
            snapshot_sha: "moved".into(),
            ..member(2)
        };
        lane.wait(moved);
        assert_eq!(lane.waiting(), &[member(2)]);
    }

    #[test]
    fn a_resolving_member_is_not_queued_again() {
        let mut lane = lane_with_batch(vec![member(1)]);
        lane.wait(member(1));
        assert!(lane.waiting().is_empty());
        assert_eq!(lane.place(member(1).change_id), Place::Resolving);
    }

    #[test]
    fn the_waiting_queue_starts_no_pass_while_a_batch_resolves() {
        let mut lane = lane_with_batch(vec![member(1)]);
        lane.wait(member(2));
        assert!(lane.take_pass().is_empty());
        assert_eq!(lane.waiting(), &[member(2)]);
    }

    /// The lane thread decides the batch, so one member's landing alone never
    /// closes it.
    #[test]
    fn a_batch_ends_only_when_its_lane_thread_decides() {
        let mut lane = lane_with_batch(vec![member(1), member(2)]);
        lane.landed(member(1).change_id);
        assert_eq!(lane.place(member(1).change_id), Place::Absent);
        assert_eq!(lane.place(member(2).change_id), Place::Resolving);
        assert!(lane.resolving().is_some(), "still open");
        lane.landed(member(2).change_id);
        assert!(lane.resolving().unwrap().all_landed());
        assert!(
            lane.resolving().is_some(),
            "landing all members is not a verdict"
        );
    }

    #[test]
    fn an_ended_batch_releases_the_queue_in_order() {
        let mut lane = lane_with_batch(vec![member(1)]);
        lane.wait(member(2));
        lane.wait(member(3));
        let ended = lane.end_batch(BATCH).expect("the open batch");
        assert_eq!(
            ended.members().cloned().collect::<Vec<_>>(),
            vec![member(1)]
        );
        assert_eq!(lane.take_pass(), vec![member(2), member(3)]);
        assert!(lane.waiting().is_empty());
    }

    /// Members `origin` refused pass while a batch holds the queue. The rest
    /// keep their places.
    #[test]
    fn taking_named_members_passes_the_held_queue_in_order() {
        let mut lane = lane_with_batch(vec![member(1)]);
        for n in [2, 3, 4] {
            lane.wait(member(n));
        }
        assert!(lane.take_pass().is_empty(), "the batch holds the queue");
        let ids = HashSet::from([member(4).change_id, member(2).change_id]);
        assert_eq!(lane.take_members(&ids), vec![member(2), member(4)]);
        assert_eq!(lane.waiting(), &[member(3)]);
    }

    /// A failed batch queues nothing by itself: its members leave the lane,
    /// every one of them, whatever they had reached.
    #[test]
    fn a_failed_batch_requeues_nothing() {
        let mut lane = lane_with_batch(vec![member(1), member(2)]);
        let ended = lane.end_batch(BATCH).expect("the open batch");
        assert_eq!(ended.unlanded().count(), 2, "it fails whole");
        for n in [1, 2] {
            assert_eq!(lane.place(member(n).change_id), Place::Absent);
        }
        assert!(lane.take_pass().is_empty());
    }

    #[test]
    fn ending_another_batch_changes_nothing() {
        let mut lane = lane_with_batch(vec![member(1)]);
        let before = lane.clone();
        assert!(lane.end_batch(Uuid::new_v4()).is_none());
        assert_eq!(lane, before);
    }

    #[test]
    fn a_landing_outside_the_batch_only_leaves_the_queue() {
        let mut lane = lane_with_batch(vec![member(1)]);
        lane.wait(member(2));
        lane.landed(member(2).change_id);
        assert!(lane.waiting().is_empty(), "a change that landed leaves");
        assert_eq!(lane.place(member(1).change_id), Place::Resolving);
    }

    /// Remove takes a waiting change out on its own. A resolving member goes
    /// only with its batch, and one already landed is no longer in the lane.
    #[test]
    fn only_a_waiting_change_may_be_removed() {
        let mut lane = lane_with_batch(vec![member(1), member(2)]);
        lane.wait(member(3));
        lane.landed(member(2).change_id);
        assert_eq!(lane.removal(member(3).change_id), Ok(()));
        assert_eq!(lane.removal(member(1).change_id), Err(LaneRemoval::InBatch));
        assert_eq!(
            lane.removal(member(2).change_id),
            Err(LaneRemoval::NotWaiting),
            "a landed member has left the lane"
        );
        assert_eq!(
            lane.removal(member(4).change_id),
            Err(LaneRemoval::NotWaiting)
        );
    }

    /// An Apply All Cancel asks which of its members the lane holds. A
    /// waiting one leaves alone, and one in the open batch takes the batch
    /// with it. A landed member or a stranger is not the lane's.
    #[test]
    fn the_lane_names_which_canceled_members_it_holds() {
        let mut lane = lane_with_batch(vec![member(1), member(2)]);
        lane.wait(member(3));
        lane.landed(member(2).change_id);
        let ids: Vec<Uuid> = (1..=4).map(|n| member(n).change_id).collect();
        let hold = lane.held_of(&ids);
        assert_eq!(hold.waiting, vec![member(3).change_id]);
        assert_eq!(hold.in_batch, vec![member(1).change_id]);
        assert_eq!(hold.batch, Some(BATCH));
        assert_eq!(
            hold.members(),
            vec![member(3).change_id, member(1).change_id]
        );

        let outside = lane.held_of(&[member(3).change_id, member(4).change_id]);
        assert_eq!(outside.batch, None, "no batch member, no batch to cancel");
        assert_eq!(outside.members(), vec![member(3).change_id]);
    }

    #[test]
    fn unqueue_drops_only_a_waiting_change() {
        let mut lane = lane_with_batch(vec![member(1)]);
        lane.wait(member(2));
        assert!(lane.unqueue(member(2).change_id));
        assert!(
            !lane.unqueue(member(1).change_id),
            "a resolving member stays"
        );
        assert!(!lane.unqueue(member(3).change_id));
        assert_eq!(lane.place(member(1).change_id), Place::Resolving);
    }
}
