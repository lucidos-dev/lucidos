pub mod actor;
pub mod affected;
pub mod config;
pub mod continue_key;
pub mod definition;
pub use crate::core::trigger_folder_grant as folder_grant;
pub mod groups;
mod groups_projection;
pub mod registry;
pub mod replay;
pub mod run_history;
pub mod summary;

pub use crate::core::event_subscription::EventSubscription;
pub use actor::{ActorField, ACTOR_FIELD_KEY};
pub use config::{
    is_valid_reasoning_effort, is_valid_trigger_slug, mint_unique_trigger_slug,
    normalize_mcp_servers, normalize_route_setting, resolve_trigger_provider_update,
    slugify_trigger_name_with_fallback, validate_script_extension, validate_trigger_provider,
    validate_trigger_reasoning_effort, TriggerConfig, TriggerRun, TriggerRunStatus,
};
pub use continue_key::ContinueKeySpec;
pub use groups::{
    find_group_by_name_ci, replay_trigger_group_events, TriggerGroup, TriggerGroupEventRow,
    TriggerGroupProjection,
};
pub use replay::{replay_trigger_events, TriggerEventRow};
pub use run_history::{load_trigger_run_history, TriggerRunHistory};
pub use summary::{ensure_non_empty_error, ensure_non_empty_summary, script_fallback_summary};

use std::collections::HashMap;

pub fn find_matching_event_triggers(
    configs: &HashMap<String, TriggerConfig>,
    event_type: &str,
    payload: &serde_json::Value,
    emitting_trigger_id: Option<&str>,
) -> Vec<TriggerConfig> {
    configs
        .values()
        .filter(|t| !t.paused && EventSubscription::any_matches(&t.on, event_type, payload))
        .filter(|t| {
            let is_own_fire = Some(t.id.as_str()) == emitting_trigger_id;
            if is_own_fire {
                crate::log_legacy!(
                    "[Triggers] Not waking '{}' on {}: its own fire emitted it",
                    t.name,
                    event_type
                );
            }
            !is_own_fire
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
            provider: None,
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
            program_grants: vec![],
            folder_grants: vec![],
            program_denies: vec![],
            folder_denies: vec![],
            coding_agent_grant: false,
            context_ceiling: None,
            mcp_servers: vec![],
            plugin_id: None,
            model: None,
            reasoning_effort: None,
            continue_key: None,
            actor: None,
        }
    }

    fn sub(event_type: &str, condition: Option<serde_json::Value>) -> EventSubscription {
        EventSubscription {
            event_type: event_type.to_string(),
            condition,
        }
    }

    fn matching(
        configs: &HashMap<String, TriggerConfig>,
        event_type: &str,
        payload: &serde_json::Value,
    ) -> Vec<TriggerConfig> {
        find_matching_event_triggers(configs, event_type, payload, None)
    }

    #[test]
    fn matcher_parity_with_the_shared_predicate() {
        use crate::core::event_subscription::PARITY_CASES;
        for (sub_type, cond, event_type, payload, expected) in PARITY_CASES {
            let mut configs = HashMap::new();
            configs.insert(
                "t1".into(),
                make_event_trigger(
                    "t1",
                    vec![sub(
                        sub_type,
                        cond.map(|c| serde_json::from_str(c).unwrap()),
                    )],
                ),
            );
            let payload: serde_json::Value = serde_json::from_str(payload).unwrap();
            let hit = !matching(&configs, event_type, &payload).is_empty();
            assert_eq!(
                hit, *expected,
                "trigger dispatch disagrees with EventSubscription::matches for \
                 subscription {sub_type} cond={cond:?} vs event {event_type} {payload}",
            );
        }
    }

    #[test]
    fn a_trigger_can_scope_on_event_to_one_thread() {
        use crate::core::event_subscription::matchable_thread_payload;
        use crate::engine::thread_events::EventMeta;

        let watched = uuid::Uuid::new_v4();
        let other = uuid::Uuid::new_v4();
        let idle = crate::engine::thread_events::ThreadEvent::CodingAgentIdled {
            has_changes: true,
            is_external_repo: false,
            requires_restart: false,
            cc_session_id: None,
            coding_agent: crate::runtime::CodingAgent::ClaudeCode,
            reason: None,
            worktree_path: None,
            worktree_head_sha: None,
            bg_bash_pending: false,
            turn_end: None,
        };

        let mut configs = HashMap::new();
        configs.insert(
            "scoped".into(),
            make_event_trigger(
                "scoped",
                vec![sub(
                    "CodingAgentIdled",
                    Some(serde_json::json!({"thread_id": watched.to_string()})),
                )],
            ),
        );

        let hit = matching(
            &configs,
            "CodingAgentIdled",
            &matchable_thread_payload(&idle, &EventMeta::NONE, watched),
        );
        assert_eq!(hit.len(), 1, "the watched thread's idle must fire it");

        let miss = matching(
            &configs,
            "CodingAgentIdled",
            &matchable_thread_payload(&idle, &EventMeta::NONE, other),
        );
        assert!(
            miss.is_empty(),
            "another session's idle must not, or the filter is decorative"
        );
    }

    #[test]
    fn matches_event_trigger_by_type() {
        let mut configs = HashMap::new();
        configs.insert(
            "t1".into(),
            make_event_trigger("t1", vec![sub("SlideTextEdited", None)]),
        );

        let matches = matching(&configs, "SlideTextEdited", &json!({}));
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

        let matches = matching(&configs, "OtherEvent", &json!({}));
        assert!(matches.is_empty());
    }

    #[test]
    fn skips_paused_triggers() {
        let mut configs = HashMap::new();
        let mut trigger = make_event_trigger("t1", vec![sub("SlideTextEdited", None)]);
        trigger.paused = true;
        configs.insert("t1".into(), trigger);

        let matches = matching(&configs, "SlideTextEdited", &json!({}));
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

        let matches = matching(&configs, "SleepImported", &json!({"sleep_score": 55}));
        assert_eq!(matches.len(), 1);

        let matches = matching(&configs, "SleepImported", &json!({"sleep_score": 85}));
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

        let matches = matching(&configs, "DataImported", &json!({}));
        assert_eq!(matches.len(), 2);
    }

    #[test]
    fn cron_only_trigger_does_not_match() {
        let mut configs = HashMap::new();
        let cron_trigger = TriggerConfig {
            provider: None,
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
            program_grants: vec![],
            folder_grants: vec![],
            program_denies: vec![],
            folder_denies: vec![],
            coding_agent_grant: false,
            context_ceiling: None,
            mcp_servers: vec![],
            plugin_id: None,
            model: None,
            reasoning_effort: None,
            continue_key: None,
            actor: None,
        };
        configs.insert("cron-only".into(), cron_trigger);

        let matches = matching(&configs, "SlideTextEdited", &json!({}));
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

        assert_eq!(matching(&configs, "OuraSleepImported", &json!({})).len(), 1);
        assert_eq!(matching(&configs, "EmailReceived", &json!({})).len(), 1);
        assert!(matching(&configs, "OtherEvent", &json!({})).is_empty());
    }

    #[test]
    fn per_subscription_condition_scopes_to_that_event() {
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
            matching(&configs, "OuraSleepImported", &json!({"sleep_score": 55}),).len(),
            1
        );
        assert!(matching(&configs, "OuraSleepImported", &json!({"sleep_score": 90}),).is_empty());
        assert_eq!(
            matching(&configs, "EmailReceived", &json!({"from": "a@b"})).len(),
            1
        );
    }

    fn two_idle_detectors() -> HashMap<String, TriggerConfig> {
        let mut configs = HashMap::new();
        for id in ["idle", "sibling"] {
            configs.insert(
                id.into(),
                make_event_trigger(id, vec![sub("TriggerCompleted", None)]),
            );
        }
        configs
    }

    #[test]
    fn a_trigger_is_not_woken_by_its_own_completion() {
        let configs = two_idle_detectors();
        let matches =
            find_matching_event_triggers(&configs, "TriggerCompleted", &json!({}), Some("idle"));
        let ids: Vec<&str> = matches.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(ids, vec!["sibling"], "only the other trigger wakes");
    }

    #[test]
    fn a_trigger_is_still_woken_by_another_triggers_completion() {
        let configs = two_idle_detectors();
        let matches =
            find_matching_event_triggers(&configs, "TriggerCompleted", &json!({}), Some("sibling"));
        let ids: Vec<&str> = matches.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(
            ids,
            vec!["idle"],
            "the sibling's fire still wakes the idle detector"
        );
    }

    #[test]
    fn an_event_no_trigger_emitted_wakes_every_subscriber() {
        let configs = two_idle_detectors();
        let matches = find_matching_event_triggers(&configs, "TriggerCompleted", &json!({}), None);
        assert_eq!(matches.len(), 2, "no marker, no suppression");
    }

    #[test]
    fn an_unknown_emitter_suppresses_nobody() {
        let configs = two_idle_detectors();
        let matches =
            find_matching_event_triggers(&configs, "TriggerCompleted", &json!({}), Some("gone"));
        assert_eq!(matches.len(), 2);
    }
}
