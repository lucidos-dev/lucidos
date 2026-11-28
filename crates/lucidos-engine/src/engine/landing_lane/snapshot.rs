use super::state::Lane;
use uuid::Uuid;

pub use crate::engine::thread_events::landing_lane::{
    ArrivalHoldSnapshot, LaneSnapshot, LaneStep, OpenBatchSnapshot, RemoteLaneEntry,
};

pub(crate) fn snapshot_of(lane: &Lane, landing: Option<Uuid>) -> LaneSnapshot {
    LaneSnapshot {
        setting: Default::default(),
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
        remote_members: lane
            .waiting()
            .iter()
            .chain(
                lane.resolving()
                    .into_iter()
                    .flat_map(|batch| batch.unlanded()),
            )
            .filter_map(|member| {
                let source = member.remote.as_ref()?;
                Some(RemoteLaneEntry {
                    change_id: member.change_id,
                    title: source.title.clone(),
                    gateway: source.gateway.clone(),
                })
            })
            .collect(),
        multiple_landers: false,
        gathering: lane.gathering(),
        waiting_for_origin: None,
        arrival_hold: lane.arrival_hold().map(|hold| ArrivalHoldSnapshot {
            batch_id: hold.batch_id,
            arrivals: hold.arrivals().to_vec(),
        }),
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
            remote: None,
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
        assert_eq!(snapshot_of(&Lane::default(), None), LaneSnapshot::default());
    }

    #[test]
    fn an_empty_lane_defaults_to_landing_here() {
        assert_eq!(crate::core::prefs::LANDING_SETTING.default_text(), "here");
        let json = serde_json::to_value(LaneSnapshot::default()).unwrap();
        assert_eq!(json["setting"], "here");
        let older = serde_json::json!({"open_batch": null, "waiting": [], "gathering": false});
        let snapshot: LaneSnapshot = serde_json::from_value(older).unwrap();
        assert_eq!(snapshot, LaneSnapshot::default());
    }

    #[test]
    fn the_snapshot_holds_the_open_batch_and_what_waits_behind_it() {
        let mut lane = lane_with_batch(vec![member(1), member(2)]);
        lane.wait(member(3));
        lane.wait(member(4));
        assert_eq!(
            snapshot_of(&lane, None),
            LaneSnapshot {
                setting: Default::default(),
                open_batch: Some(OpenBatchSnapshot {
                    batch_id: BATCH,
                    lane_thread_id: LANE_THREAD,
                    members: vec![id(1), id(2)],
                    step: LaneStep::Resolving,
                }),
                waiting: vec![id(3), id(4)],
                remote_members: vec![],
                multiple_landers: false,
                gathering: false,
                waiting_for_origin: None,
                arrival_hold: None,
            }
        );
    }

    #[test]
    fn the_snapshot_names_the_batch_that_holds_arrivals() {
        let mut lane = lane_with_batch(vec![member(1)]);
        lane.wait(member(2));
        lane.hold_arrivals(BATCH);
        lane.hold(member(3));
        let snapshot = snapshot_of(&lane, None);
        assert_eq!(
            snapshot.arrival_hold,
            Some(ArrivalHoldSnapshot {
                batch_id: BATCH,
                arrivals: vec![id(3)],
            })
        );
        assert_eq!(snapshot.waiting, vec![id(2), id(3)]);
        let json = serde_json::to_value(&snapshot).unwrap();
        assert_eq!(json["arrival_hold"]["batch_id"], BATCH.to_string());
        let unheld = serde_json::to_value(snapshot_of(&Lane::default(), None)).unwrap();
        assert!(unheld.get("arrival_hold").is_none());
    }

    #[test]
    fn the_snapshot_says_when_the_lane_gathers_a_press() {
        let mut lane = Lane::default();
        lane.open_press(Uuid::new_v4());
        lane.set_aside(BATCH, "base".into(), LANE_THREAD, vec![member(1)]);
        let snapshot = snapshot_of(&lane, None);
        assert_eq!(snapshot.open_batch, None);
        assert_eq!(snapshot.waiting, vec![id(1)]);
        assert!(snapshot.gathering);
        let json = serde_json::to_value(&snapshot).unwrap();
        assert_eq!(json["gathering"], true);
    }

    #[test]
    fn the_step_is_landing_only_while_the_lane_lands_this_batch() {
        let lane = lane_with_batch(vec![member(1)]);
        let step = |landing| snapshot_of(&lane, landing).open_batch.unwrap().step;
        assert_eq!(step(None), LaneStep::Resolving);
        assert_eq!(step(Some(Uuid::new_v4())), LaneStep::Resolving);
        assert_eq!(step(Some(BATCH)), LaneStep::Landing);
    }

    #[test]
    fn a_landed_member_leaves_the_open_batch() {
        let mut lane = lane_with_batch(vec![member(1), member(2)]);
        lane.landed(id(1));
        let batch = snapshot_of(&lane, Some(BATCH)).open_batch.unwrap();
        assert_eq!(batch.members, vec![id(2)]);
    }

    #[test]
    fn the_wire_shape_is_snake_case() {
        let lane = lane_with_batch(vec![member(1)]);
        let json = serde_json::to_value(snapshot_of(&lane, Some(BATCH))).unwrap();
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

    #[test]
    fn the_snapshot_says_why_origin_holds_the_lane() {
        let mut lane = Lane::default();
        lane.wait(member(1));
        let mut snapshot = snapshot_of(&lane, None);
        snapshot.waiting_for_origin = Some("could not fetch".into());
        let json = serde_json::to_value(&snapshot).unwrap();
        assert_eq!(json["waiting_for_origin"], "could not fetch");
        assert_eq!(json["waiting"][0], id(1).to_string());
    }
}
