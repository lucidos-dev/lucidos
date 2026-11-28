use std::collections::HashMap;

pub use super::groups_projection::TriggerGroupProjection;

use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::Value;

#[derive(Debug, Clone, Serialize)]
pub struct TriggerGroup {
    pub id: String,
    pub name: String,
    pub order: i32,
    pub created: DateTime<Utc>,
}

impl TriggerGroup {
    pub fn from_created_payload(payload: &Value, created: DateTime<Utc>) -> Result<Self, String> {
        let id = payload["group_id"]
            .as_str()
            .ok_or("Missing group_id")?
            .to_string();
        let name = payload["name"].as_str().ok_or("Missing name")?.to_string();
        let order = payload
            .get("order")
            .and_then(|v| v.as_i64())
            .map(|n| n as i32)
            .unwrap_or(0);
        Ok(TriggerGroup {
            id,
            name,
            order,
            created,
        })
    }
}

#[derive(Clone)]
pub struct TriggerGroupEventRow {
    pub seq: i64,
    pub event_type: String,
    pub payload: Value,
    pub created: DateTime<Utc>,
}

pub fn replay_trigger_group_events(events: Vec<TriggerGroupEventRow>) -> TriggerGroupProjection {
    let mut groups = TriggerGroupProjection::default();
    for row in events {
        groups.apply(&row);
    }
    groups
}

pub fn find_group_by_name_ci(groups: &HashMap<String, TriggerGroup>, name: &str) -> Option<String> {
    let needle = name.to_lowercase();
    groups
        .values()
        .find(|g| g.name.to_lowercase() == needle)
        .map(|g| g.id.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn replay(mut events: Vec<TriggerGroupEventRow>) -> TriggerGroupProjection {
        for (i, row) in events.iter_mut().enumerate() {
            row.seq = i as i64 + 1;
        }
        replay_trigger_group_events(events)
    }

    fn make_event(event_type: &str, payload: Value) -> TriggerGroupEventRow {
        TriggerGroupEventRow {
            seq: 0,
            event_type: event_type.to_string(),
            payload,
            created: Utc::now(),
        }
    }

    fn created_payload(id: &str, name: &str, order: i32) -> Value {
        json!({ "group_id": id, "name": name, "order": order })
    }

    #[test]
    fn replay_created_builds_group() {
        let events = vec![make_event(
            "TriggerGroupCreated",
            created_payload("g1", "Morning Routine", 10),
        )];
        let groups = replay(events);
        let g = groups.get("g1").expect("group present");
        assert_eq!(g.name, "Morning Routine");
        assert_eq!(g.order, 10);
    }

    #[test]
    fn replay_created_then_renamed() {
        let events = vec![
            make_event("TriggerGroupCreated", created_payload("g1", "Old Name", 10)),
            make_event(
                "TriggerGroupRenamed",
                json!({ "group_id": "g1", "name": "New Name" }),
            ),
        ];
        let groups = replay(events);
        assert_eq!(groups.get("g1").unwrap().name, "New Name");
        assert_eq!(
            groups.get("g1").unwrap().order,
            10,
            "rename preserves order"
        );
    }

    #[test]
    fn replay_reordered_updates_order_only() {
        let events = vec![
            make_event("TriggerGroupCreated", created_payload("g1", "Group", 10)),
            make_event(
                "TriggerGroupReordered",
                json!({ "group_id": "g1", "order": 50 }),
            ),
        ];
        let groups = replay(events);
        let g = groups.get("g1").unwrap();
        assert_eq!(g.order, 50);
        assert_eq!(g.name, "Group", "reorder does not touch name");
    }

    #[test]
    fn replay_deleted_removes_group() {
        let events = vec![
            make_event("TriggerGroupCreated", created_payload("g1", "Group", 0)),
            make_event("TriggerGroupDeleted", json!({ "group_id": "g1" })),
        ];
        let groups = replay(events);
        assert!(groups.is_empty());
    }

    #[test]
    fn replay_update_without_create_is_ignored() {
        let events = vec![make_event(
            "TriggerGroupRenamed",
            json!({ "group_id": "g1", "name": "Ghost" }),
        )];
        let groups = replay(events);
        assert!(groups.is_empty());
    }

    #[test]
    fn replay_create_with_missing_group_id_is_skipped() {
        let events = vec![make_event(
            "TriggerGroupCreated",
            json!({ "name": "Anonymous", "order": 0 }),
        )];
        let groups = replay(events);
        assert!(groups.is_empty());
    }

    #[test]
    fn replay_full_lifecycle_create_rename_reorder_delete_create_again() {
        let events = vec![
            make_event("TriggerGroupCreated", created_payload("g1", "First", 0)),
            make_event(
                "TriggerGroupRenamed",
                json!({ "group_id": "g1", "name": "Renamed" }),
            ),
            make_event(
                "TriggerGroupReordered",
                json!({ "group_id": "g1", "order": 99 }),
            ),
            make_event("TriggerGroupCreated", created_payload("g2", "Second", 5)),
            make_event("TriggerGroupDeleted", json!({ "group_id": "g1" })),
        ];
        let groups = replay(events);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups.get("g2").unwrap().name, "Second");
        assert!(!groups.contains_key("g1"));
    }

    #[test]
    fn from_created_payload_defaults_order_to_zero() {
        let g = TriggerGroup::from_created_payload(
            &json!({ "group_id": "g1", "name": "G" }),
            Utc::now(),
        )
        .unwrap();
        assert_eq!(g.order, 0);
    }

    #[test]
    fn from_created_payload_missing_name_errors() {
        let err = TriggerGroup::from_created_payload(&json!({ "group_id": "g1" }), Utc::now());
        assert!(err.is_err());
    }

    #[test]
    fn find_group_by_name_ci_matches_case_insensitively() {
        let mut groups = HashMap::new();
        groups.insert(
            "g1".to_string(),
            TriggerGroup {
                id: "g1".into(),
                name: "Morning Routine".into(),
                order: 0,
                created: Utc::now(),
            },
        );

        assert_eq!(
            find_group_by_name_ci(&groups, "morning routine"),
            Some("g1".to_string())
        );
        assert_eq!(
            find_group_by_name_ci(&groups, "MORNING ROUTINE"),
            Some("g1".to_string())
        );
        assert_eq!(find_group_by_name_ci(&groups, "morning"), None);
    }
}
