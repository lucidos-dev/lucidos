pub mod condition;
pub mod config;
pub mod replay;

pub use config::{validate_script_extension, TriggerConfig, TriggerRun};
pub use replay::{replay_trigger_events, TriggerEventRow};

use std::collections::HashMap;

/// Find all active (non-paused) event-based triggers that match a given event type and payload.
///
/// Returns configs for triggers where:
/// 1. The trigger is not paused
/// 2. `trigger.on` matches `event_type`
/// 3. The condition (if any) evaluates to true against `payload`
pub fn find_matching_event_triggers(
    configs: &HashMap<String, TriggerConfig>,
    event_type: &str,
    payload: &serde_json::Value,
) -> Vec<TriggerConfig> {
    configs
        .values()
        .filter(|t| {
            !t.paused
                && t.on.as_deref() == Some(event_type)
                && condition::evaluate(t.condition.as_ref(), payload)
        })
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn make_event_trigger(
        id: &str,
        on: &str,
        condition: Option<serde_json::Value>,
    ) -> TriggerConfig {
        TriggerConfig {
            id: id.to_string(),
            name: format!("Trigger {}", id),
            schedule: vec![],
            timezone: "UTC".to_string(),
            run: TriggerRun::Script {
                path: "scripts/run.py".to_string(),
            },
            on: Some(on.to_string()),
            condition,
            paused: false,
            last_run: None,
        }
    }

    #[test]
    fn matches_event_trigger_by_type() {
        let mut configs = HashMap::new();
        configs.insert(
            "t1".into(),
            make_event_trigger("t1", "SlideTextEdited", None),
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
            make_event_trigger("t1", "SlideTextEdited", None),
        );

        let matches = find_matching_event_triggers(&configs, "OtherEvent", &json!({}));
        assert!(matches.is_empty());
    }

    #[test]
    fn skips_paused_triggers() {
        let mut configs = HashMap::new();
        let mut trigger = make_event_trigger("t1", "SlideTextEdited", None);
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
                "SleepImported",
                Some(json!({"sleep_score": {"$lt": 70}})),
            ),
        );

        // Matching payload
        let matches =
            find_matching_event_triggers(&configs, "SleepImported", &json!({"sleep_score": 55}));
        assert_eq!(matches.len(), 1);

        // Non-matching payload
        let matches =
            find_matching_event_triggers(&configs, "SleepImported", &json!({"sleep_score": 85}));
        assert!(matches.is_empty());
    }

    #[test]
    fn multiple_triggers_for_same_event() {
        let mut configs = HashMap::new();
        configs.insert("t1".into(), make_event_trigger("t1", "DataImported", None));
        configs.insert("t2".into(), make_event_trigger("t2", "DataImported", None));
        configs.insert("t3".into(), make_event_trigger("t3", "OtherEvent", None));

        let matches = find_matching_event_triggers(&configs, "DataImported", &json!({}));
        assert_eq!(matches.len(), 2);
    }

    #[test]
    fn cron_only_trigger_does_not_match() {
        let mut configs = HashMap::new();
        let cron_trigger = TriggerConfig {
            id: "cron-only".to_string(),
            name: "Cron Only".to_string(),
            schedule: vec!["0 0 8 * * *".to_string()],
            timezone: "UTC".to_string(),
            run: TriggerRun::Intent {
                text: "do something".to_string(),
                knowhow: vec![],
            },
            on: None,
            condition: None,
            paused: false,
            last_run: None,
        };
        configs.insert("cron-only".into(), cron_trigger);

        let matches = find_matching_event_triggers(&configs, "SlideTextEdited", &json!({}));
        assert!(matches.is_empty());
    }
}
