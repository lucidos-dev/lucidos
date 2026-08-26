pub mod config;
pub mod definition;
pub mod groups;
pub mod registry;
pub mod replay;
pub mod run_history;
pub mod summary;

/// The subscription primitive a trigger's `on:` list is made of. It lives in
/// [`crate::core::event_subscription`] rather than here because a thread's
/// *event wait* subscribes with the same shape and the same matcher; the
/// re-export keeps trigger code reading in trigger terms.
pub use crate::core::event_subscription::EventSubscription;
pub use config::{
    is_valid_reasoning_effort, is_valid_trigger_slug, normalize_route_setting,
    slugify_trigger_name_with_fallback, validate_script_extension,
    validate_trigger_reasoning_effort, TriggerConfig, TriggerRun, TriggerRunStatus,
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
/// 2. Its id is not `emitting_trigger_id`
/// 3. At least one of `trigger.on[*].event_type` matches `event_type`, AND
///    that entry's condition (if any) evaluates true against `payload`.
///
/// `emitting_trigger_id` is the trigger whose fire emitted this event, if any.
/// It never matches its own event, at any depth. `None` suppresses nobody,
/// which is the direction to fail in: an extra wake, never a missing one. See
/// `docs/adr/0137-a-trigger-never-wakes-itself.md`.
///
/// Conditions are scoped to each subscription, so a single trigger can listen
/// for multiple events with different payload shapes without one filter
/// constraining the other.
///
/// The predicate itself is [`EventSubscription::any_matches`], shared verbatim
/// with the event-wait dispatcher. Do not inline the name comparison or reach
/// into `condition::evaluate` here: the two dispatch paths agreeing on every
/// (subscription, event) pair is the whole reason the primitive is shared, and
/// `matcher_parity_with_the_shared_predicate` below pins it.
///
/// The caller is responsible for the subscribability gate
/// ([`crate::core::event_subscription::is_subscribable`]); see
/// `start_trigger_event_subscriber` in `crates/lucidos-engine/src/scheduler/mod.rs`.
pub fn find_matching_event_triggers(
    configs: &HashMap<String, TriggerConfig>,
    event_type: &str,
    payload: &serde_json::Value,
    emitting_trigger_id: Option<&str>,
) -> Vec<TriggerConfig> {
    configs
        .values()
        .filter(|t| !t.paused && EventSubscription::any_matches(&t.on, event_type, payload))
        // Second, so the log names only triggers that really would have woken.
        // The depth cap and the shutdown gate both log their suppressions, and
        // a silent one here reads as a trigger that never matched.
        .filter(|t| {
            let is_own_fire = Some(t.id.as_str()) == emitting_trigger_id;
            if is_own_fire {
                crate::log!(
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
            model: None,
            reasoning_effort: None,
        }
    }

    fn sub(event_type: &str, condition: Option<serde_json::Value>) -> EventSubscription {
        EventSubscription {
            event_type: event_type.to_string(),
            condition,
        }
    }

    /// The matcher for an event no trigger emitted, which is what an ordinary
    /// user turn produces. The self-exclusion tests below name an emitter and
    /// so call the real function.
    fn matching(
        configs: &HashMap<String, TriggerConfig>,
        event_type: &str,
        payload: &serde_json::Value,
    ) -> Vec<TriggerConfig> {
        find_matching_event_triggers(configs, event_type, payload, None)
    }

    /// I8: the trigger dispatch path must return the same verdict as the shared
    /// predicate for every (subscription, event) pair, so the event-wait
    /// dispatcher and this one cannot disagree. The table is owned by
    /// `core::event_subscription`, which asserts the same cases against
    /// `EventSubscription::matches` directly; running it through
    /// `find_matching_event_triggers` is what proves the trigger side did not
    /// quietly re-implement the comparison.
    #[test]
    fn matcher_parity_with_the_shared_predicate() {
        use crate::core::event_subscription::tests::PARITY_CASES;
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

    /// The trigger half of thread scoping. A `condition` naming `thread_id`
    /// works here for the same reason it works for an *event wait*: the
    /// scheduler offers the matcher a *matchable payload*, built by the same
    /// function the wait dispatcher calls, so one `on_event:` filter cannot
    /// mean two different things depending on which subscriber reads it.
    ///
    /// Built through `matchable_thread_payload` rather than a hand-written
    /// `json!` carrying a `thread_id`, because a literal would pass even if the
    /// scheduler stopped injecting: no `ThreadEvent` declares the field.
    #[test]
    fn a_trigger_can_scope_on_event_to_one_thread() {
        use crate::core::event_subscription::matchable_thread_payload;

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
            &matchable_thread_payload(&idle, watched),
        );
        assert_eq!(hit.len(), 1, "the watched thread's idle must fire it");

        let miss = matching(
            &configs,
            "CodingAgentIdled",
            &matchable_thread_payload(&idle, other),
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
            model: None,
            reasoning_effort: None,
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
            matching(&configs, "OuraSleepImported", &json!({"sleep_score": 55}),).len(),
            1
        );
        assert!(matching(&configs, "OuraSleepImported", &json!({"sleep_score": 90}),).is_empty());
        assert_eq!(
            matching(&configs, "EmailReceived", &json!({"from": "a@b"})).len(),
            1
        );
    }

    /// Two triggers, both subscribed to `TriggerCompleted`, which is what an
    /// idle detector's broad subscription looks like.
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

    /// The invariant: a trigger is never woken by an event its own fire
    /// emitted. Broad-subscribe plus a cheap internal gate is the right shape
    /// for an idle detector. `TriggerCompleted` is one of the terminator events
    /// it has to watch, and being a trigger is what makes it emit that event.
    /// So the engine holds the rule, not the author.
    #[test]
    fn a_trigger_is_not_woken_by_its_own_completion() {
        let configs = two_idle_detectors();
        let matches =
            find_matching_event_triggers(&configs, "TriggerCompleted", &json!({}), Some("idle"));
        let ids: Vec<&str> = matches.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(ids, vec!["sibling"], "only the other trigger wakes");
    }

    /// The other half, and the failure that would be worse than the noise: an
    /// idle detector that stops seeing other triggers finish never registers
    /// the workspace as idle.
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

    /// Fail-open. An unstamped frame suppresses nobody, which covers an
    /// ordinary user turn, an HTTP handler, and work a fire handed off to a
    /// fresh task. The last one is deliberate: a trigger waiting on a
    /// coding-agent session it started must still fire.
    #[test]
    fn an_event_no_trigger_emitted_wakes_every_subscriber() {
        let configs = two_idle_detectors();
        let matches = find_matching_event_triggers(&configs, "TriggerCompleted", &json!({}), None);
        assert_eq!(matches.len(), 2, "no marker, no suppression");
    }

    /// An id that matches no trigger drops nobody either. A deleted trigger's
    /// last event is the concrete case.
    #[test]
    fn an_unknown_emitter_suppresses_nobody() {
        let configs = two_idle_detectors();
        let matches =
            find_matching_event_triggers(&configs, "TriggerCompleted", &json!({}), Some("gone"));
        assert_eq!(matches.len(), 2);
    }
}
