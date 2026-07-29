//! Trigger groups — user-visible folders that organize triggers in the panel.
//!
//! A group is a pure label: it has no schedule, runs no code, and does not
//! coordinate trigger firing. Each `TriggerConfig` may belong to at most one
//! group via `group_id`; ungrouped triggers render under an implicit
//! "Ungrouped" section in the UI.
//!
//! State is event-sourced — the engine rebuilds the registry from
//! `TriggerGroup*` events at startup, exactly like the trigger registry.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::Value;

/// In-memory representation of a trigger group, rebuilt from events on startup.
#[derive(Debug, Clone, Serialize)]
pub struct TriggerGroup {
    /// Stable UUID (as string, for parity with TriggerConfig.id).
    pub id: String,
    /// User-facing label; unique within the workspace (case-insensitive).
    pub name: String,
    /// Sort position in the panel, ascending. Ties broken by `created` asc.
    pub order: i32,
    /// First-emit time, taken from the originating event row.
    pub created: DateTime<Utc>,
}

impl TriggerGroup {
    /// Build a TriggerGroup from a `TriggerGroupCreated` payload + event row time.
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

    /// Apply a `TriggerGroupRenamed` payload. No-op if `name` is missing or non-string.
    pub fn apply_renamed_payload(&mut self, payload: &Value) {
        if let Some(name) = payload.get("name").and_then(|v| v.as_str()) {
            self.name = name.to_string();
        }
    }

    /// Apply a `TriggerGroupReordered` payload. No-op if `order` is missing.
    pub fn apply_reordered_payload(&mut self, payload: &Value) {
        if let Some(order) = payload.get("order").and_then(|v| v.as_i64()) {
            self.order = order as i32;
        }
    }
}

/// A raw event row used for replaying trigger-group lifecycle events.
pub struct TriggerGroupEventRow {
    pub event_type: String,
    pub payload: Value,
    pub created: DateTime<Utc>,
}

/// Replay a sequence of trigger-group lifecycle events to rebuild the in-memory
/// registry. Events must be in chronological order (oldest first).
pub fn replay_trigger_group_events(
    events: Vec<TriggerGroupEventRow>,
) -> HashMap<String, TriggerGroup> {
    let mut groups: HashMap<String, TriggerGroup> = HashMap::new();

    for row in events {
        let group_id = row
            .payload
            .get("group_id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        if group_id.is_empty() {
            continue;
        }

        match row.event_type.as_str() {
            "TriggerGroupCreated" => {
                if let Ok(g) = TriggerGroup::from_created_payload(&row.payload, row.created) {
                    groups.insert(group_id, g);
                }
            }
            "TriggerGroupRenamed" => {
                if let Some(g) = groups.get_mut(&group_id) {
                    g.apply_renamed_payload(&row.payload);
                }
            }
            "TriggerGroupReordered" => {
                if let Some(g) = groups.get_mut(&group_id) {
                    g.apply_reordered_payload(&row.payload);
                }
            }
            "TriggerGroupDeleted" => {
                groups.remove(&group_id);
            }
            _ => {}
        }
    }

    groups
}

/// Case-insensitive lookup for unique-name enforcement at the API boundary.
/// Returns the matching group's id when one exists, otherwise None.
pub fn find_group_by_name_ci(
    groups: &HashMap<String, TriggerGroup>,
    name: &str,
) -> Option<String> {
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

    fn make_event(event_type: &str, payload: Value) -> TriggerGroupEventRow {
        TriggerGroupEventRow {
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
        let events = vec![make_event("TriggerGroupCreated", created_payload("g1", "Morning Routine", 10))];
        let groups = replay_trigger_group_events(events);
        let g = groups.get("g1").expect("group present");
        assert_eq!(g.name, "Morning Routine");
        assert_eq!(g.order, 10);
    }

    #[test]
    fn replay_created_then_renamed() {
        let events = vec![
            make_event("TriggerGroupCreated", created_payload("g1", "Old Name", 10)),
            make_event("TriggerGroupRenamed", json!({ "group_id": "g1", "name": "New Name" })),
        ];
        let groups = replay_trigger_group_events(events);
        assert_eq!(groups.get("g1").unwrap().name, "New Name");
        assert_eq!(groups.get("g1").unwrap().order, 10, "rename preserves order");
    }

    #[test]
    fn replay_reordered_updates_order_only() {
        let events = vec![
            make_event("TriggerGroupCreated", created_payload("g1", "Group", 10)),
            make_event("TriggerGroupReordered", json!({ "group_id": "g1", "order": 50 })),
        ];
        let groups = replay_trigger_group_events(events);
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
        let groups = replay_trigger_group_events(events);
        assert!(groups.is_empty());
    }

    #[test]
    fn replay_update_without_create_is_ignored() {
        let events = vec![make_event(
            "TriggerGroupRenamed",
            json!({ "group_id": "g1", "name": "Ghost" }),
        )];
        let groups = replay_trigger_group_events(events);
        assert!(groups.is_empty());
    }

    #[test]
    fn replay_create_with_missing_group_id_is_skipped() {
        let events = vec![make_event(
            "TriggerGroupCreated",
            json!({ "name": "Anonymous", "order": 0 }),
        )];
        let groups = replay_trigger_group_events(events);
        assert!(groups.is_empty());
    }

    #[test]
    fn replay_full_lifecycle_create_rename_reorder_delete_create_again() {
        // Round-trip: a workspace can churn through groups; the registry must
        // converge on the final state regardless of intermediate operations.
        let events = vec![
            make_event("TriggerGroupCreated", created_payload("g1", "First", 0)),
            make_event("TriggerGroupRenamed", json!({ "group_id": "g1", "name": "Renamed" })),
            make_event("TriggerGroupReordered", json!({ "group_id": "g1", "order": 99 })),
            make_event("TriggerGroupCreated", created_payload("g2", "Second", 5)),
            make_event("TriggerGroupDeleted", json!({ "group_id": "g1" })),
        ];
        let groups = replay_trigger_group_events(events);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups.get("g2").unwrap().name, "Second");
        assert!(!groups.contains_key("g1"));
    }

    #[test]
    fn from_created_payload_defaults_order_to_zero() {
        let g = TriggerGroup::from_created_payload(&json!({ "group_id": "g1", "name": "G" }), Utc::now())
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

        assert_eq!(find_group_by_name_ci(&groups, "morning routine"), Some("g1".to_string()));
        assert_eq!(find_group_by_name_ci(&groups, "MORNING ROUTINE"), Some("g1".to_string()));
        assert_eq!(find_group_by_name_ci(&groups, "morning"), None);
    }
}
