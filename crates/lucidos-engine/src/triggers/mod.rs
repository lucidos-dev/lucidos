pub mod condition;
pub mod config;
pub mod definition;
pub mod groups;
pub mod replay;
pub mod run_history;
pub mod summary;

pub use config::{
    is_valid_trigger_slug, slugify_trigger_name_with_fallback, validate_script_extension,
    EventSubscription, TriggerConfig, TriggerRun, TriggerRunStatus,
};
pub use groups::{
    find_group_by_name_ci, replay_trigger_group_events, TriggerGroup, TriggerGroupEventRow,
};
pub use replay::{replay_trigger_events, TriggerEventRow};
pub use run_history::{load_trigger_run_history, TriggerRunHistory};
pub use summary::{ensure_non_empty_error, ensure_non_empty_summary, script_fallback_summary};

use std::collections::HashMap;

/// Find all active (non-paused) event-based triggers that match a given event type and payload.
///
/// Returns configs for triggers where:
/// 1. The trigger is not paused
/// 2. At least one of `trigger.on[*].event_type` matches `event_type`, AND
///    that entry's condition (if any) evaluates true against `payload`.
///
/// Conditions are scoped to each subscription, so a single trigger can listen
/// for multiple events with different payload shapes without one filter
/// constraining the other.
pub fn find_matching_event_triggers(
    configs: &HashMap<String, TriggerConfig>,
    event_type: &str,
    payload: &serde_json::Value,
) -> Vec<TriggerConfig> {
    configs
        .values()
        .filter(|t| {
            !t.paused
                && t.on.iter().any(|sub| {
                    sub.event_type == event_type
                        && condition::evaluate(sub.condition.as_ref(), payload)
                })
        })
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn make_event_trigger(id: &str, subs: Vec<EventSubscription>) -> TriggerConfig {
        TriggerConfig {
            id: id.to_string(),
            name: format!("Trigger {}", id),
            slug: format!("trigger-{}", id),
            schedule: vec![],
            timezone: "UTC".to_string(),
            run: TriggerRun::Script {
                path: "scripts/run.py".to_string(),
            },
            on: subs,
            paused: false,
            last_run: None,
            last_run_status: None,
            app_id: None,
            go_to_review: false,
            group_id: None,
            side_effect_grant: vec![],
            plugin_id: None,
        }
    }

    fn sub(event_type: &str, condition: Option<serde_json::Value>) -> EventSubscription {
        EventSubscription {
            event_type: event_type.to_string(),
            condition,
        }
    }

    #[test]
    fn matches_event_trigger_by_type() {
        let mut configs = HashMap::new();
        configs.insert(
            "t1".into(),
            make_event_trigger("t1", vec![sub("SlideTextEdited", None)]),
        );

        let matches = find_matching_event_triggers(&configs, "SlideTextEdited", &json!({}));
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].id, "t1");
    }

    #[test]
    fn no_match_for_different_event_type() {
        let mut configs = HashMap::new();
        configs.insert(
            "t1".into(),
            make_event_trigger("t1", vec![sub("SlideTextEdited", None)]),
        );

        let matches = find_matching_event_triggers(&configs, "OtherEvent", &json!({}));
        assert!(matches.is_empty());
    }

    #[test]
    fn skips_paused_triggers() {
        let mut configs = HashMap::new();
        let mut trigger = make_event_trigger("t1", vec![sub("SlideTextEdited", None)]);
        trigger.paused = true;
        configs.insert("t1".into(), trigger);

        let matches = find_matching_event_triggers(&configs, "SlideTextEdited", &json!({}));
        assert!(matches.is_empty());
    }

    #[test]
    fn condition_filters_payload() {
        let mut configs = HashMap::new();
        configs.insert(
            "t1".into(),
            make_event_trigger(
                "t1",
                vec![sub(
                    "SleepImported",
                    Some(json!({"sleep_score": {"$lt": 70}})),
                )],
            ),
        );

        let matches =
            find_matching_event_triggers(&configs, "SleepImported", &json!({"sleep_score": 55}));
        assert_eq!(matches.len(), 1);

        let matches =
            find_matching_event_triggers(&configs, "SleepImported", &json!({"sleep_score": 85}));
        assert!(matches.is_empty());
    }

    #[test]
    fn multiple_triggers_for_same_event() {
        let mut configs = HashMap::new();
        configs.insert(
            "t1".into(),
            make_event_trigger("t1", vec![sub("DataImported", None)]),
        );
        configs.insert(
            "t2".into(),
            make_event_trigger("t2", vec![sub("DataImported", None)]),
        );
        configs.insert(
            "t3".into(),
            make_event_trigger("t3", vec![sub("OtherEvent", None)]),
        );

        let matches = find_matching_event_triggers(&configs, "DataImported", &json!({}));
        assert_eq!(matches.len(), 2);
    }

    #[test]
    fn cron_only_trigger_does_not_match() {
        let mut configs = HashMap::new();
        let cron_trigger = TriggerConfig {
            id: "cron-only".to_string(),
            name: "Cron Only".to_string(),
            slug: "cron-only".to_string(),
            schedule: vec!["0 0 8 * * *".to_string()],
            timezone: "UTC".to_string(),
            run: TriggerRun::Intent {
                intent: "do something".to_string(),
            },
            on: vec![],
            paused: false,
            last_run: None,
            last_run_status: None,
            app_id: None,
            go_to_review: false,
            group_id: None,
            side_effect_grant: vec![],
            plugin_id: None,
        };
        configs.insert("cron-only".into(), cron_trigger);

        let matches = find_matching_event_triggers(&configs, "SlideTextEdited", &json!({}));
        assert!(matches.is_empty());
    }

    #[test]
    fn matches_when_any_subscription_event_type_matches() {
        let mut configs = HashMap::new();
        configs.insert(
            "multi".into(),
            make_event_trigger(
                "multi",
                vec![sub("OuraSleepImported", None), sub("EmailReceived", None)],
            ),
        );

        assert_eq!(
            find_matching_event_triggers(&configs, "OuraSleepImported", &json!({})).len(),
            1
        );
        assert_eq!(
            find_matching_event_triggers(&configs, "EmailReceived", &json!({})).len(),
            1
        );
        assert!(find_matching_event_triggers(&configs, "OtherEvent", &json!({})).is_empty());
    }

    #[test]
    fn per_subscription_condition_scopes_to_that_event() {
        // Each entry's condition only filters its own event_type, so an unrelated
        // sibling event can fire even when the other entry's condition references
        // fields it doesn't carry.
        let mut configs = HashMap::new();
        configs.insert(
            "per-event".into(),
            make_event_trigger(
                "per-event",
                vec![
                    sub(
                        "OuraSleepImported",
                        Some(json!({"sleep_score": {"$lt": 70}})),
                    ),
                    sub("EmailReceived", None),
                ],
            ),
        );

        assert_eq!(
            find_matching_event_triggers(
                &configs,
                "OuraSleepImported",
                &json!({"sleep_score": 55}),
            )
            .len(),
            1
        );
        assert!(find_matching_event_triggers(
            &configs,
            "OuraSleepImported",
            &json!({"sleep_score": 90}),
        )
        .is_empty());
        assert_eq!(
            find_matching_event_triggers(&configs, "EmailReceived", &json!({"from": "a@b"})).len(),
            1
        );
    }
}
