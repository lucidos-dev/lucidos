use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde_json::Value;

use super::config::{TriggerConfig, TriggerRunStatus};

/// A raw event row used for replaying trigger lifecycle events.
pub struct TriggerEventRow {
    pub event_type: String,
    pub payload: Value,
    pub created: DateTime<Utc>,
}

/// Replay a sequence of trigger lifecycle events to rebuild in-memory state.
/// Events must be in chronological order (oldest first).
pub fn replay_trigger_events(events: Vec<TriggerEventRow>) -> HashMap<String, TriggerConfig> {
    let mut triggers = HashMap::new();

    for row in events {
        let trigger_id = row.payload["trigger_id"]
            .as_str()
            .or_else(|| row.payload["task_id"].as_str()) // backward compat with old events
            .unwrap_or("")
            .to_string();

        if trigger_id.is_empty() {
            continue;
        }

        match row.event_type.as_str() {
            "TriggerCreated" | "ScheduledTriggerCreated" => {
                if let Ok(config) = TriggerConfig::from_created_payload(&row.payload) {
                    triggers.insert(trigger_id, config);
                }
            }
            "TriggerUpdated" | "ScheduledTriggerUpdated" => {
                if let Some(config) = triggers.get_mut(&trigger_id) {
                    config.apply_update(&row.payload);
                }
            }
            "TriggerDeleted" | "ScheduledTriggerDeleted" => {
                triggers.remove(&trigger_id);
            }
            "TriggerEnabled" | "ScheduledTriggerEnabled" => {
                if let Some(config) = triggers.get_mut(&trigger_id) {
                    config.paused = false;
                }
            }
            "TriggerDisabled" | "ScheduledTriggerDisabled" => {
                if let Some(config) = triggers.get_mut(&trigger_id) {
                    config.paused = true;
                }
            }
            "TriggerExecuted" => {
                if let Some(config) = triggers.get_mut(&trigger_id) {
                    config.last_run = Some(row.created);
                    // The `status` field rides the payload as of the last-run-status
                    // change. A legacy `TriggerExecuted` without it leaves
                    // `last_run_status` untouched (→ None, "timestamp only"); an
                    // unknown/forward-compat value is ignored rather than wedging.
                    match row.payload.get("status").and_then(|v| v.as_str()) {
                        Some("ok") => config.last_run_status = Some(TriggerRunStatus::Ok),
                        Some("failed") => config.last_run_status = Some(TriggerRunStatus::Failed),
                        _ => {}
                    }
                }
            }
            "TriggerStarted" | "ScheduledTriggerStarted" => {
                // Pre-run marker (LLM trigger thread start): stamps the run time
                // but carries no outcome — the trailing `TriggerExecuted` sets the
                // status. Leave `last_run_status` as-is here.
                if let Some(config) = triggers.get_mut(&trigger_id) {
                    config.last_run = Some(row.created);
                }
            }
            _ => {}
        }
    }

    triggers
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn make_event(event_type: &str, payload: Value) -> TriggerEventRow {
        TriggerEventRow {
            event_type: event_type.to_string(),
            payload,
            created: Utc::now(),
        }
    }

    fn created_payload(id: &str, name: &str) -> Value {
        json!({
            "trigger_id": id,
            "name": name,
            "schedule": ["0 0 8 * * *"],
            "timezone": "Europe/Oslo",
            "run": { "type": "prompt", "text": "test prompt", "knowhow": [] }
        })
    }

    #[test]
    fn replay_created_builds_config() {
        let events = vec![make_event(
            "TriggerCreated",
            created_payload("t1", "Trigger 1"),
        )];
        let triggers = replay_trigger_events(events);
        assert_eq!(triggers.len(), 1);
        let t = triggers.get("t1").unwrap();
        assert_eq!(t.name, "Trigger 1");
        assert!(!t.paused);
    }

    #[test]
    fn replay_created_then_updated_merges() {
        let events = vec![
            make_event("TriggerCreated", created_payload("t1", "Trigger 1")),
            make_event(
                "TriggerUpdated",
                json!({ "trigger_id": "t1", "name": "Updated Name", "schedule": ["0 0 22 * * *"] }),
            ),
        ];
        let triggers = replay_trigger_events(events);
        let t = triggers.get("t1").unwrap();
        assert_eq!(t.name, "Updated Name");
        assert_eq!(t.schedule, vec!["0 0 22 * * *"]);
    }

    #[test]
    fn replay_created_then_deleted_removes() {
        let events = vec![
            make_event("TriggerCreated", created_payload("t1", "Trigger 1")),
            make_event("TriggerDeleted", json!({ "trigger_id": "t1" })),
        ];
        let triggers = replay_trigger_events(events);
        assert!(triggers.is_empty());
    }

    #[test]
    fn replay_created_then_disabled() {
        let events = vec![
            make_event("TriggerCreated", created_payload("t1", "Trigger 1")),
            make_event("TriggerDisabled", json!({ "trigger_id": "t1" })),
        ];
        let triggers = replay_trigger_events(events);
        assert!(triggers.get("t1").unwrap().paused);
    }

    #[test]
    fn replay_disabled_then_enabled() {
        let events = vec![
            make_event("TriggerCreated", created_payload("t1", "Trigger 1")),
            make_event("TriggerDisabled", json!({ "trigger_id": "t1" })),
            make_event("TriggerEnabled", json!({ "trigger_id": "t1" })),
        ];
        let triggers = replay_trigger_events(events);
        assert!(!triggers.get("t1").unwrap().paused);
    }

    #[test]
    fn replay_trigger_updated_with_paused_true() {
        let events = vec![
            make_event("TriggerCreated", created_payload("t1", "T")),
            make_event(
                "TriggerUpdated",
                json!({ "trigger_id": "t1", "paused": true }),
            ),
        ];
        let triggers = replay_trigger_events(events);
        assert!(triggers.get("t1").unwrap().paused);
    }

    #[test]
    fn replay_legacy_trigger_updated_with_enabled_false() {
        let events = vec![
            make_event("TriggerCreated", created_payload("t1", "T")),
            make_event(
                "TriggerUpdated",
                json!({ "trigger_id": "t1", "enabled": false }),
            ),
        ];
        let triggers = replay_trigger_events(events);
        assert!(triggers.get("t1").unwrap().paused);
    }

    #[test]
    fn replay_started_sets_last_run() {
        let events = vec![
            make_event("TriggerCreated", created_payload("t1", "Trigger 1")),
            TriggerEventRow {
                event_type: "ScheduledTriggerStarted".into(),
                payload: json!({ "trigger_id": "t1" }),
                created: chrono::DateTime::parse_from_rfc3339("2026-04-05T08:00:00Z")
                    .unwrap()
                    .with_timezone(&Utc),
            },
        ];
        let triggers = replay_trigger_events(events);
        let t = triggers.get("t1").unwrap();
        assert!(t.last_run.is_some());
    }

    #[test]
    fn replay_executed_with_status_sets_last_run_status() {
        // A `TriggerExecuted` carrying the outcome rebuilds both the timestamp
        // and the OK/failed status — so a threadless trigger's health survives a
        // restart the same way `last_run` does.
        let events = vec![
            make_event("TriggerCreated", created_payload("t1", "Probe")),
            make_event(
                "TriggerExecuted",
                json!({ "trigger_id": "t1", "last_run": "2026-07-01T08:00:00Z", "status": "failed" }),
            ),
        ];
        let triggers = replay_trigger_events(events);
        let t = triggers.get("t1").unwrap();
        assert!(t.last_run.is_some());
        assert_eq!(t.last_run_status, Some(TriggerRunStatus::Failed));

        // And the success case.
        let ok = replay_trigger_events(vec![
            make_event("TriggerCreated", created_payload("t2", "Probe 2")),
            make_event(
                "TriggerExecuted",
                json!({ "trigger_id": "t2", "status": "ok" }),
            ),
        ]);
        assert_eq!(
            ok.get("t2").unwrap().last_run_status,
            Some(TriggerRunStatus::Ok)
        );
    }

    #[test]
    fn replay_legacy_executed_without_status_leaves_status_none() {
        // A pre-status-field `TriggerExecuted` still sets `last_run` but carries
        // no outcome — the row degrades to "timestamp only", never a wrong status.
        let events = vec![
            make_event("TriggerCreated", created_payload("t1", "Probe")),
            make_event("TriggerExecuted", json!({ "trigger_id": "t1" })),
        ];
        let triggers = replay_trigger_events(events);
        let t = triggers.get("t1").unwrap();
        assert!(t.last_run.is_some());
        assert_eq!(t.last_run_status, None);
    }

    #[test]
    fn replay_created_defaults_status_none() {
        // A trigger that has never fired has no last-run status.
        let events = vec![make_event(
            "TriggerCreated",
            created_payload("t1", "Never run"),
        )];
        let t = replay_trigger_events(events);
        assert_eq!(t.get("t1").unwrap().last_run_status, None);
    }

    #[test]
    fn replay_update_without_create_is_ignored() {
        let events = vec![make_event(
            "TriggerUpdated",
            json!({ "trigger_id": "t1", "name": "Ghost" }),
        )];
        let triggers = replay_trigger_events(events);
        assert!(triggers.is_empty());
    }

    #[test]
    fn replay_empty_events() {
        let triggers = replay_trigger_events(vec![]);
        assert!(triggers.is_empty());
    }

    #[test]
    fn replay_multiple_triggers() {
        let events = vec![
            make_event("TriggerCreated", created_payload("t1", "Trigger 1")),
            make_event("TriggerCreated", created_payload("t2", "Trigger 2")),
            make_event("TriggerDeleted", json!({ "trigger_id": "t1" })),
        ];
        let triggers = replay_trigger_events(events);
        assert_eq!(triggers.len(), 1);
        assert!(triggers.contains_key("t2"));
    }

    #[test]
    fn replay_handles_old_scheduled_prefix() {
        let events = vec![make_event(
            "ScheduledTriggerCreated",
            created_payload("t1", "Old Style"),
        )];
        let triggers = replay_trigger_events(events);
        assert_eq!(triggers.len(), 1);
        assert_eq!(triggers.get("t1").unwrap().name, "Old Style");
    }

    #[test]
    fn replay_create_with_null_trigger_id_is_ignored() {
        // Legacy `task_id` fallback in `replay_trigger_events` only resolves the
        // HashMap key for non-Create events (Updated/Deleted/...). Create events
        // still go through `from_created_payload`, which requires a non-null
        // `trigger_id` field. A payload with only `task_id` set is silently
        // dropped at the parse step, leaving no trigger registered.
        let events = vec![make_event(
            "ScheduledTriggerCreated",
            json!({
                "task_id": "t1",
                "trigger_id": null,
                "name": "Legacy",
                "schedule": ["0 0 8 * * *"],
                "timezone": "UTC",
                "run": { "type": "prompt", "text": "test", "knowhow": [] }
            }),
        )];
        let triggers = replay_trigger_events(events);
        assert_eq!(triggers.len(), 0);
    }
}
