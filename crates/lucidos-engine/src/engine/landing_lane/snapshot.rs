//! The landing lane as clients see it: the open lane batch with its step, and
//! the changes waiting for the next batch. `GET /api/v1/changes` and every
//! `ChangesUpdated` frame carry it as `landing_lane`.
//!
//! The driver publishes it after every message it handles, so it is always
//! the driver's own lane. A restart starts it empty until recovery restores
//! the lane.

use super::state::Lane;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// What the landing lane holds right now.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LaneSnapshot {
    /// The lane batch resolving now, if any. At most one is open.
    pub open_batch: Option<OpenBatchSnapshot>,
    /// Changes waiting for the next batch, in landing order.
    pub waiting: Vec<Uuid>,
    /// The queue waits for an Apply All press to hand over the rest of its
    /// changes, with no batch open (ADR 0480).
    pub gathering: bool,
    /// Why the repository's `origin` could not take the last
    /// landing. The lane holds what it could not push and retries (ADR 0589).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub waiting_for_origin: Option<String>,
}

/// The open lane batch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpenBatchSnapshot {
    pub batch_id: Uuid,
    /// The coding-agent thread that resolves the batch.
    pub lane_thread_id: Uuid,
    /// The members still to land, in batch order.
    pub members: Vec<Uuid>,
    pub step: LaneStep,
}

/// What the open batch is doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LaneStep {
    /// Its lane thread works on the merge conflicts, or has yet to finish.
    Resolving,
    /// Its lane thread finished cleanly, and the lane is moving `main`.
    Landing,
}

impl LaneSnapshot {
    /// The snapshot of `lane`. `landing` is the batch the driver is landing,
    /// from its lane thread's clean finish until it lands or goes back to the
    /// lane thread.
    pub(crate) fn of(lane: &Lane, landing: Option<Uuid>) -> Self {
        Self {
            open_batch: lane.resolving().map(|batch| OpenBatchSnapshot {
                batch_id: batch.batch_id,
                lane_thread_id: batch.lane_thread_id,
                members: batch.unlanded().map(|m| m.change_id).collect(),
                step: if landing == Some(batch.batch_id) {
                    LaneStep::Landing
                } else {
                    LaneStep::Resolving
                },
            }),
            waiting: lane.waiting().iter().map(|m| m.change_id).collect(),
            gathering: lane.gathering(),
            waiting_for_origin: None,
        }
    }

    /// The same snapshot, saying why `origin` holds the lane, if it does.
    pub(crate) fn waiting_for_origin(mut self, why: Option<String>) -> Self {
        self.waiting_for_origin = why;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::super::LaneMember;
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

    fn lane_with_batch(members: Vec<LaneMember>) -> Lane {
        let mut lane = Lane::default();
        lane.set_aside(BATCH, "base".into(), LANE_THREAD, members);
        lane
    }

    #[test]
    fn an_empty_lane_has_no_batch_and_nothing_waiting() {
        assert_eq!(
            LaneSnapshot::of(&Lane::default(), None),
            LaneSnapshot::default()
        );
    }

    /// The open batch names its lane thread and the members still to land.
    /// What waits behind it is listed apart, in order.
    #[test]
    fn the_snapshot_holds_the_open_batch_and_what_waits_behind_it() {
        let mut lane = lane_with_batch(vec![member(1), member(2)]);
        lane.wait(member(3));
        lane.wait(member(4));
        assert_eq!(
            LaneSnapshot::of(&lane, None),
            LaneSnapshot {
                open_batch: Some(OpenBatchSnapshot {
                    batch_id: BATCH,
                    lane_thread_id: LANE_THREAD,
                    members: vec![id(1), id(2)],
                    step: LaneStep::Resolving,
                }),
                waiting: vec![id(3), id(4)],
                gathering: false,
                waiting_for_origin: None,
            }
        );
    }

    /// While a press hands changes over, what conflicts waits with no batch
    /// open, and the snapshot says the lane gathers.
    #[test]
    fn the_snapshot_says_when_the_lane_gathers_a_press() {
        let mut lane = Lane::default();
        lane.open_press(Uuid::new_v4());
        lane.set_aside(BATCH, "base".into(), LANE_THREAD, vec![member(1)]);
        let snapshot = LaneSnapshot::of(&lane, None);
        assert_eq!(snapshot.open_batch, None);
        assert_eq!(snapshot.waiting, vec![id(1)]);
        assert!(snapshot.gathering);
        let json = serde_json::to_value(&snapshot).unwrap();
        assert_eq!(json["gathering"], true);
    }

    /// The step reads `landing` only while the driver lands this batch. A
    /// marker for another batch, or none, reads `resolving`.
    #[test]
    fn the_step_is_landing_only_while_the_lane_lands_this_batch() {
        let lane = lane_with_batch(vec![member(1)]);
        let step = |landing| LaneSnapshot::of(&lane, landing).open_batch.unwrap().step;
        assert_eq!(step(None), LaneStep::Resolving);
        assert_eq!(step(Some(Uuid::new_v4())), LaneStep::Resolving);
        assert_eq!(step(Some(BATCH)), LaneStep::Landing);
    }

    /// A member announced as landed leaves the batch's list while the batch
    /// is still open.
    #[test]
    fn a_landed_member_leaves_the_open_batch() {
        let mut lane = lane_with_batch(vec![member(1), member(2)]);
        lane.landed(id(1));
        let batch = LaneSnapshot::of(&lane, Some(BATCH)).open_batch.unwrap();
        assert_eq!(batch.members, vec![id(2)]);
    }

    /// The wire shape clients read: snake_case keys and step values.
    #[test]
    fn the_wire_shape_is_snake_case() {
        let lane = lane_with_batch(vec![member(1)]);
        let json = serde_json::to_value(LaneSnapshot::of(&lane, Some(BATCH))).unwrap();
        assert_eq!(json["open_batch"]["batch_id"], BATCH.to_string());
        assert_eq!(
            json["open_batch"]["lane_thread_id"],
            LANE_THREAD.to_string()
        );
        assert_eq!(json["open_batch"]["members"][0], id(1).to_string());
        assert_eq!(json["open_batch"]["step"], "landing");
        assert_eq!(json["waiting"], serde_json::json!([]));
        let empty = serde_json::to_value(LaneSnapshot::default()).unwrap();
        assert_eq!(empty["open_batch"], serde_json::Value::Null);
        assert!(empty.get("waiting_for_origin").is_none());
    }

    /// While `origin` holds the lane, the snapshot carries why, for the
    /// client to show beside what waits.
    #[test]
    fn the_snapshot_says_why_origin_holds_the_lane() {
        let mut lane = Lane::default();
        lane.wait(member(1));
        let snapshot =
            LaneSnapshot::of(&lane, None).waiting_for_origin(Some("could not fetch".into()));
        let json = serde_json::to_value(&snapshot).unwrap();
        assert_eq!(json["waiting_for_origin"], "could not fetch");
        assert_eq!(json["waiting"][0], id(1).to_string());
    }
}
