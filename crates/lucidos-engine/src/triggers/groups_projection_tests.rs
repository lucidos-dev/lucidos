use super::*;
use crate::triggers::replay_trigger_group_events;
use chrono::DateTime;
use serde_json::json;
use serde_json::Value;

fn row(seq: i64, event_type: &str, payload: Value) -> TriggerGroupEventRow {
    TriggerGroupEventRow {
        seq,
        event_type: event_type.into(),
        payload,
        created: DateTime::from_timestamp(seq, 0).unwrap(),
    }
}

fn permutations<T: Clone>(items: &[T], visit: &mut dyn FnMut(Vec<T>)) {
    if items.is_empty() {
        visit(Vec::new());
        return;
    }
    for i in 0..items.len() {
        let mut rest = items.to_vec();
        let first = rest.remove(i);
        permutations(&rest, &mut |mut tail| {
            tail.insert(0, first.clone());
            visit(tail);
        });
    }
}

#[test]
fn every_delivery_order_preserves_independent_fields_and_groups() {
    let rows = vec![
        row(
            1,
            "TriggerGroupCreated",
            json!({"group_id":"a","name":"Old","order":100}),
        ),
        row(
            2,
            "TriggerGroupRenamed",
            json!({"group_id":"a","name":"Intermediate"}),
        ),
        row(
            3,
            "TriggerGroupReordered",
            json!({"group_id":"a","order":150}),
        ),
        row(
            4,
            "TriggerGroupRenamed",
            json!({"group_id":"a","name":"New"}),
        ),
        row(
            5,
            "TriggerGroupReordered",
            json!({"group_id":"a","order":200}),
        ),
        row(
            6,
            "TriggerGroupCreated",
            json!({"group_id":"b","name":"Other","order":50}),
        ),
    ];
    permutations(&rows, &mut |delivered| {
        let mut projection = TriggerGroupProjection::default();
        for event in delivered.iter().chain(&delivered) {
            projection.apply(event);
        }
        assert_eq!(projection["a"].name, "New");
        assert_eq!(projection["a"].order, 200);
        assert_eq!(projection["a"].created, rows[0].created);
        assert_eq!(projection["b"].name, "Other");
        assert_eq!(projection.len(), 2);
    });
}

#[test]
fn every_delivery_order_preserves_deletion_and_later_creation() {
    let rows = vec![
        row(
            1,
            "TriggerGroupCreated",
            json!({"group_id":"a","name":"Old","order":100}),
        ),
        row(
            2,
            "TriggerGroupRenamed",
            json!({"group_id":"a","name":"Renamed"}),
        ),
        row(3, "TriggerGroupDeleted", json!({"group_id":"a"})),
    ];
    permutations(&rows, &mut |delivered| {
        let mut projection = replay_trigger_group_events(delivered);
        assert!(!projection.contains_key("a"));
        projection.apply(&row(
            4,
            "TriggerGroupCreated",
            json!({"group_id":"a","name":"Recreated","order":10}),
        ));
        for old in rows.iter().rev() {
            projection.apply(old);
        }
        assert_eq!(projection["a"].name, "Recreated");
        assert_eq!(projection["a"].order, 10);
    });
}

#[test]
fn stale_snapshot_merge_keeps_newer_fields_and_tombstones() {
    let created = row(
        1,
        "TriggerGroupCreated",
        json!({"group_id":"a","name":"Old","order":100}),
    );
    let mut snapshot = replay_trigger_group_events(vec![created.clone()]);
    let mut live = snapshot.clone();
    live.apply(&row(
        2,
        "TriggerGroupRenamed",
        json!({"group_id":"a","name":"New"}),
    ));
    live.apply(&row(
        3,
        "TriggerGroupReordered",
        json!({"group_id":"a","order":200}),
    ));
    live.apply(&row(
        4,
        "TriggerGroupCreated",
        json!({"group_id":"b","name":"Other"}),
    ));
    live.merge(snapshot.clone());
    assert_eq!(live["a"].name, "New");
    assert_eq!(live["a"].order, 200);
    assert!(live.contains_key("b"));

    live.apply(&row(5, "TriggerGroupDeleted", json!({"group_id":"a"})));
    live.merge(snapshot.clone());
    assert!(!live.contains_key("a"));
    snapshot.merge(live);
    snapshot.apply(&created);
    assert!(!snapshot.contains_key("a"));
    assert!(snapshot.contains_key("b"));
}

#[test]
fn replayed_deletion_survives_an_older_live_frame() {
    let created = row(
        1,
        "TriggerGroupCreated",
        json!({"group_id":"a","name":"Old"}),
    );
    let snapshot = replay_trigger_group_events(vec![
        created.clone(),
        row(2, "TriggerGroupDeleted", json!({"group_id":"a"})),
    ]);
    let mut live = TriggerGroupProjection::default();
    live.merge(snapshot);
    live.apply(&created);
    assert!(live.is_empty());
}

#[test]
fn snapshot_and_live_updates_merge_independent_fields_in_both_directions() {
    let created = row(
        1,
        "TriggerGroupCreated",
        json!({"group_id":"a","name":"Old","order":100}),
    );
    let snapshot = replay_trigger_group_events(vec![
        created.clone(),
        row(
            2,
            "TriggerGroupRenamed",
            json!({"group_id":"a","name":"New"}),
        ),
    ]);
    let mut live = replay_trigger_group_events(vec![
        created,
        row(
            3,
            "TriggerGroupReordered",
            json!({"group_id":"a","order":200}),
        ),
    ]);
    let mut reverse = snapshot.clone();
    reverse.merge(live.clone());
    live.merge(snapshot);
    for projection in [live, reverse] {
        assert_eq!(projection["a"].name, "New");
        assert_eq!(projection["a"].order, 200);
    }
}

#[test]
fn duplicate_creation_is_stable_and_replay_restores_persisted_timestamp() {
    let created = row(
        1,
        "TriggerGroupCreated",
        json!({"group_id":"a","name":"Original"}),
    );
    let mut live = replay_trigger_group_events(vec![created.clone()]);
    let mut delayed = created.clone();
    delayed.created += chrono::Duration::seconds(10);
    live.apply(&delayed);
    assert_eq!(live["a"].created, created.created);
    let mut live = replay_trigger_group_events(vec![delayed]);
    live.merge(replay_trigger_group_events(vec![created.clone()]));
    assert_eq!(live["a"].created, created.created);
}
