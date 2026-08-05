use super::*;
use serde_json::json;

#[test]
fn from_created_event_prompt() {
    let payload = json!({
        "trigger_id": "sleep-reminder",
            "name": "Sleep Reminder",
            "schedule": ["0 0 22 * * 1-5"],
        "timezone": "Europe/Oslo",
        "run": { "type": "prompt", "text": "Send a push notification reminding me to go to sleep.", "knowhow": [] }
    });

    let config = TriggerConfig::from_created_payload(&payload).unwrap();
    assert_eq!(config.id, "sleep-reminder");
    assert_eq!(config.name, "Sleep Reminder");
    assert_eq!(config.schedule, vec!["0 0 22 * * 1-5"]);
    assert_eq!(config.timezone, "Europe/Oslo");
    assert!(matches!(config.run, TriggerRun::Intent { .. }));
    assert!(!config.paused);
}

#[test]
fn plugin_id_provenance_round_trips_and_defaults_none() {
    // User trigger: no plugin_id → None.
    let user = TriggerConfig::from_created_payload(&json!({
        "trigger_id": "t1", "name": "User", "schedule": [], "timezone": "UTC",
        "run": { "type": "intent", "intent": "hi" },
        "on": [{ "event_type": "X" }],
    }))
    .unwrap();
    assert_eq!(user.plugin_id, None);

    // Plugin trigger: plugin_id carried through.
    let plug = TriggerConfig::from_created_payload(&json!({
        "trigger_id": "t2", "name": "Plug", "schedule": [], "timezone": "UTC",
        "run": { "type": "intent", "intent": "hi" },
        "on": [{ "event_type": "X" }],
        "plugin_id": "browser-learning",
    }))
    .unwrap();
    assert_eq!(plug.plugin_id.as_deref(), Some("browser-learning"));
}

#[test]
fn apply_update_sets_plugin_id_but_a_user_edit_never_strips_it() {
    let mut config = TriggerConfig::from_created_payload(&json!({
        "trigger_id": "t", "name": "T", "schedule": [], "timezone": "UTC",
        "run": { "type": "intent", "intent": "hi" }, "on": [{ "event_type": "X" }],
        "plugin_id": "my-plugin",
    }))
    .unwrap();

    // A user edit (no plugin_id in payload) must preserve provenance, else
    // uninstall could no longer reclaim the trigger.
    config.apply_update(&json!({ "name": "Renamed" }));
    assert_eq!(config.plugin_id.as_deref(), Some("my-plugin"));

    // A re-sync update can (re-)stamp it.
    config.apply_update(&json!({ "plugin_id": "my-plugin" }));
    assert_eq!(config.plugin_id.as_deref(), Some("my-plugin"));
}

#[test]
fn from_created_event_script() {
    let payload = json!({
        "trigger_id": "oura-import",
        "name": "Oura Import",
        "schedule": ["0 0 * * * *"],
        "timezone": "Europe/Oslo",
        "run": { "type": "script", "path": "oura/run.py" }
    });

    let config = TriggerConfig::from_created_payload(&payload).unwrap();
    assert!(matches!(config.run, TriggerRun::Script { .. }));
    if let TriggerRun::Script { path } = &config.run {
        assert_eq!(path, "oura/run.py");
    }
}

#[test]
fn prompt_with_legacy_knowhow_field_is_ignored() {
    // Phase 1 dropped run.knowhow; serde drops unknown fields silently
    // (the default), so on-disk events still carry the field but it must
    // not affect the parsed config.
    let payload = json!({
        "trigger_id": "heatpump-logging",
        "name": "Heatpump Logging",
        "schedule": ["0 */30 * * * *"],
        "timezone": "Europe/Oslo",
        "run": { "type": "prompt", "text": "Log heat pump status", "knowhow": ["heatpump"] }
    });

    let config = TriggerConfig::from_created_payload(&payload).unwrap();
    assert!(matches!(config.run, TriggerRun::Intent { .. }));
    if let TriggerRun::Intent { intent } = &config.run {
        assert_eq!(intent, "Log heat pump status");
    }
}

#[test]
fn apply_update_partial() {
    let payload = json!({
        "trigger_id": "sleep-reminder",
            "name": "Sleep Reminder",
            "schedule": ["0 0 22 * * 1-5"],
        "timezone": "Europe/Oslo",
        "run": { "type": "prompt", "text": "Go to sleep.", "knowhow": [] }
    });
    let mut config = TriggerConfig::from_created_payload(&payload).unwrap();

    let update = json!({
        "trigger_id": "sleep-reminder",
            "schedule": ["0 0 23 * * *"]
    });
    config.apply_update(&update);
    assert_eq!(config.schedule, vec!["0 0 23 * * *"]);
    assert_eq!(config.name, "Sleep Reminder"); // unchanged
}

#[test]
fn apply_update_name_only() {
    let payload = json!({
            "trigger_id": "test",
        "name": "Old Name",
        "schedule": ["0 0 8 * * *"],
        "timezone": "UTC",
        "run": { "type": "prompt", "text": "test", "knowhow": [] }
    });
    let mut config = TriggerConfig::from_created_payload(&payload).unwrap();

    config.apply_update(&json!({ "trigger_id": "test", "name": "New Name" }));
    assert_eq!(config.name, "New Name");
    assert_eq!(config.schedule, vec!["0 0 8 * * *"]); // unchanged
}

#[test]
fn apply_update_run_field() {
    let payload = json!({
        "trigger_id": "test",
        "name": "Test",
        "schedule": ["0 0 8 * * *"],
        "timezone": "UTC",
        "run": { "type": "prompt", "text": "old prompt", "knowhow": [] }
    });
    let mut config = TriggerConfig::from_created_payload(&payload).unwrap();

    config.apply_update(&json!({
        "trigger_id": "test",
        "run": { "type": "script", "path": "test/run.py" }
    }));
    assert!(matches!(config.run, TriggerRun::Script { .. }));
}

#[test]
fn intent_variant_deserializes_legacy_text_alias() {
    // On-disk TriggerCreated events from before the rename carry `text:`.
    let val = json!({"type": "intent", "text": "legacy payload"});
    let run: TriggerRun = serde_json::from_value(val).unwrap();
    if let TriggerRun::Intent { intent } = run {
        assert_eq!(intent, "legacy payload");
    } else {
        panic!("Expected Intent variant");
    }
}

#[test]
fn trigger_run_serde_roundtrip() {
    let prompt = TriggerRun::Intent {
        intent: "Do something".into(),
    };
    let json = serde_json::to_value(&prompt).unwrap();
    assert_eq!(json["type"], "intent");
    assert_eq!(json["intent"], "Do something");
    assert!(
        json.get("text").is_none(),
        "serialized output must not contain the legacy `text` key"
    );
    assert!(
        json.get("knowhow").is_none(),
        "serialized output must not contain the deleted `knowhow` field"
    );

    let back: TriggerRun = serde_json::from_value(json).unwrap();
    assert!(matches!(back, TriggerRun::Intent { .. }));

    let script = TriggerRun::Script {
        path: "test/run.py".into(),
    };
    let json = serde_json::to_value(&script).unwrap();
    assert_eq!(json["type"], "script");
    let back: TriggerRun = serde_json::from_value(json).unwrap();
    assert!(matches!(back, TriggerRun::Script { .. }));

    let shell = TriggerRun::Script {
        path: "backup/run.sh".into(),
    };
    let json = serde_json::to_value(&shell).unwrap();
    assert_eq!(json["type"], "script");
    assert_eq!(json["path"], "backup/run.sh");
    let back: TriggerRun = serde_json::from_value(json).unwrap();
    if let TriggerRun::Script { path } = &back {
        assert_eq!(path, "backup/run.sh");
    } else {
        panic!("Expected Script variant");
    }
}

#[test]
fn from_created_event_shell_script() {
    let payload = json!({
        "trigger_id": "backup-job",
        "name": "Daily Backup",
        "schedule": ["0 0 2 * * *"],
        "timezone": "Europe/Oslo",
        "run": { "type": "script", "path": "triggers/backup/scripts/run.sh" }
    });

    let config = TriggerConfig::from_created_payload(&payload).unwrap();
    assert!(matches!(config.run, TriggerRun::Script { .. }));
    if let TriggerRun::Script { path } = &config.run {
        assert_eq!(path, "triggers/backup/scripts/run.sh");
    }
}

#[test]
fn missing_trigger_id_errors() {
    let payload = json!({
        "name": "Test",
        "schedule": ["0 0 8 * * *"],
        "timezone": "UTC",
        "run": { "type": "prompt", "text": "test", "knowhow": [] }
    });
    assert!(TriggerConfig::from_created_payload(&payload).is_err());
}

#[test]
fn missing_name_errors() {
    let payload = json!({
        "trigger_id": "test",
        "schedule": ["0 0 8 * * *"],
        "timezone": "UTC",
        "run": { "type": "prompt", "text": "test", "knowhow": [] }
    });
    assert!(TriggerConfig::from_created_payload(&payload).is_err());
}

#[test]
fn defaults_timezone_to_utc() {
    let payload = json!({
        "trigger_id": "test",
        "name": "Test",
        "schedule": ["0 0 8 * * *"],
        "run": { "type": "prompt", "text": "test", "knowhow": [] }
    });
    let config = TriggerConfig::from_created_payload(&payload).unwrap();
    assert_eq!(config.timezone, "UTC");
}

#[test]
fn legacy_on_string_payload_no_longer_yields_a_subscription() {
    // Migration 20260516195912 rewrites this shape before replay, so the
    // reader treating it as malformed is correct, not a regression.
    let payload = json!({
        "trigger_id": "test",
        "name": "Test",
        "schedule": [],
        "timezone": "UTC",
        "run": { "type": "prompt", "text": "react", "knowhow": [] },
        "on": "OuraSleepImported",
        "condition": { "sleep_score": { "$lt": 70 } }
    });
    let config = TriggerConfig::from_created_payload(&payload).unwrap();
    assert!(config.on.is_empty());
}

#[test]
fn new_on_array_with_per_entry_conditions_parsed() {
    let payload = json!({
        "trigger_id": "test",
        "name": "Test",
        "schedule": [],
        "timezone": "UTC",
        "run": { "type": "intent", "intent": "react" },
        "on": [
            {
                "event_type": "OuraSleepImported",
                "condition": { "sleep_score": { "$lt": 70 } }
            },
            { "event_type": "EmailReceived" }
        ]
    });
    let config = TriggerConfig::from_created_payload(&payload).unwrap();
    assert_eq!(config.on.len(), 2);
    assert_eq!(config.on[0].event_type, "OuraSleepImported");
    assert!(config.on[0].condition.is_some());
    assert_eq!(config.on[1].event_type, "EmailReceived");
    assert!(config.on[1].condition.is_none());
}

#[test]
fn array_of_bare_strings_parsed_as_no_condition_entries() {
    // `["X", "Y"]` is a shorthand the LLM tool / SDK callers can use when
    // none of the events need a condition.
    let on = parse_event_subscriptions(Some(&json!(["A", "B"])));
    assert_eq!(on.len(), 2);
    assert_eq!(on[0].event_type, "A");
    assert_eq!(on[1].event_type, "B");
    assert!(on[0].condition.is_none());
    assert!(on[1].condition.is_none());
}

#[test]
fn malformed_subscription_entries_are_dropped() {
    // One bad entry must not wedge the whole list.
    let on = parse_event_subscriptions(Some(&json!([
        { "event_type": "Good" },
        { "not_event_type": "Bad" },
        "",
        { "event_type": "   " },
        "StringOk"
    ])));
    let names: Vec<&str> = on.iter().map(|s| s.event_type.as_str()).collect();
    assert_eq!(names, vec!["Good", "StringOk"]);
}

#[test]
fn trigger_run_deserialize_prompt_legacy_alias() {
    let val = json!({"type": "prompt", "text": "do something"});
    let run: Result<TriggerRun, _> = serde_json::from_value(val);
    assert!(
        run.is_ok(),
        "TriggerRun should deserialize the legacy 'prompt' variant"
    );
    if let TriggerRun::Intent { intent } = run.unwrap() {
        assert_eq!(intent, "do something");
    } else {
        panic!("Expected Intent variant");
    }
}

#[test]
fn trigger_run_deserialize_intent_minimal() {
    let val = json!({"type": "intent", "text": "do something"});
    let run: Result<TriggerRun, _> = serde_json::from_value(val);
    assert!(
        run.is_ok(),
        "TriggerRun should deserialize from minimal {{type, text}} shape"
    );
}

#[test]
fn apply_update_run_carries_intent() {
    let payload = json!({
        "trigger_id": "test",
        "name": "Test",
        "schedule": ["0 0 8 * * *"],
        "timezone": "UTC",
        "run": { "type": "prompt", "text": "old prompt" }
    });
    let mut config = TriggerConfig::from_created_payload(&payload).unwrap();

    config.apply_update(&json!({
        "trigger_id": "test",
        "run": { "type": "prompt", "text": "new prompt" }
    }));

    if let TriggerRun::Intent { intent } = &config.run {
        assert_eq!(intent, "new prompt");
    } else {
        panic!("Expected Intent variant");
    }
}

#[test]
fn null_on_and_condition_yield_empty_subscriptions() {
    let payload = json!({
        "trigger_id": "test",
        "name": "Test",
        "schedule": ["0 0 8 * * *"],
        "timezone": "UTC",
        "run": { "type": "prompt", "text": "test", "knowhow": [] },
        "on": null,
        "condition": null
    });
    let config = TriggerConfig::from_created_payload(&payload).unwrap();
    assert!(config.on.is_empty());
}

#[test]
fn apply_update_clear_schedule() {
    let payload = json!({
        "trigger_id": "test",
        "name": "Hybrid Trigger",
            "schedule": ["0 0 8 * * *"],
        "timezone": "UTC",
        "run": { "type": "prompt", "text": "test", "knowhow": [] },
        "on": [{ "event_type": "SomeEvent" }]
    });
    let mut config = TriggerConfig::from_created_payload(&payload).unwrap();
    assert_eq!(config.schedule, vec!["0 0 8 * * *"]);

    // Clear schedule by setting to empty array (from cron: null in LLM tool)
    config.apply_update(&json!({ "trigger_id": "test", "schedule": [] }));
    assert!(config.schedule.is_empty());
    assert_eq!(config.on.len(), 1); // unchanged
    assert_eq!(config.on[0].event_type, "SomeEvent");
}

#[test]
fn apply_update_clear_on_event() {
    let payload = json!({
        "trigger_id": "test",
        "name": "Hybrid Trigger",
            "schedule": ["0 0 8 * * *"],
        "timezone": "UTC",
        "run": { "type": "prompt", "text": "test", "knowhow": [] },
        "on": [{
            "event_type": "SomeEvent",
            "condition": { "score": { "$lt": 70 } }
        }]
    });
    let mut config = TriggerConfig::from_created_payload(&payload).unwrap();
    assert_eq!(config.on.len(), 1);

    config.apply_update(&json!({ "trigger_id": "test", "on": null }));
    assert!(config.on.is_empty());
}

#[test]
fn apply_update_orphan_condition_key_is_ignored_post_migration() {
    // Migration 20260516195912 folds legacy condition-only updates into
    // the prior subscription at startup, so an orphan key at runtime has
    // nowhere to land.
    let mut config = TriggerConfig::from_created_payload(&json!({
        "trigger_id": "test",
        "name": "Test",
        "schedule": [],
        "timezone": "UTC",
        "run": { "type": "prompt", "text": "test", "knowhow": [] },
        "on": [{
            "event_type": "SomeEvent",
            "condition": { "score": { "$lt": 70 } }
        }]
    }))
    .unwrap();
    assert!(config.on[0].condition.is_some());

    config.apply_update(&json!({ "trigger_id": "test", "condition": null }));
    assert_eq!(config.on.len(), 1);
    assert!(config.on[0].condition.is_some());
}

#[test]
fn apply_update_replaces_subscriptions_with_new_array() {
    let mut config = TriggerConfig::from_created_payload(&json!({
        "trigger_id": "test",
        "name": "Test",
        "schedule": [],
        "timezone": "UTC",
        "run": { "type": "intent", "intent": "x" },
        "on": [{ "event_type": "OldEvent" }]
    }))
    .unwrap();

    config.apply_update(&json!({
        "trigger_id": "test",
        "on": [
            { "event_type": "NewA", "condition": { "x": 1 } },
            { "event_type": "NewB" }
        ]
    }));

    assert_eq!(config.on.len(), 2);
    assert_eq!(config.on[0].event_type, "NewA");
    assert!(config.on[0].condition.is_some());
    assert_eq!(config.on[1].event_type, "NewB");
    assert!(config.on[1].condition.is_none());
}

#[test]
fn apply_update_absent_fields_unchanged() {
    let payload = json!({
        "trigger_id": "test",
        "name": "Original",
        "schedule": ["0 0 8 * * *"],
        "timezone": "Europe/Oslo",
        "run": { "type": "prompt", "text": "original prompt", "knowhow": ["domain"] },
        "on": [{
            "event_type": "SomeEvent",
            "condition": { "key": "val" }
        }]
    });
    let mut config = TriggerConfig::from_created_payload(&payload).unwrap();

    config.apply_update(&json!({ "trigger_id": "test", "name": "Renamed" }));
    assert_eq!(config.name, "Renamed");
    assert_eq!(config.schedule, vec!["0 0 8 * * *"]);
    assert_eq!(config.timezone, "Europe/Oslo");
    assert_eq!(config.on.len(), 1);
    assert_eq!(config.on[0].event_type, "SomeEvent");
    assert!(config.on[0].condition.is_some());
    if let TriggerRun::Intent { intent } = &config.run {
        assert_eq!(intent, "original prompt");
    } else {
        panic!("Expected Intent variant");
    }
}

#[test]
fn apply_update_switch_run_type() {
    let payload = json!({
        "trigger_id": "test",
        "name": "Test",
        "schedule": ["0 0 8 * * *"],
        "timezone": "UTC",
        "run": { "type": "prompt", "text": "old prompt", "knowhow": [] }
    });
    let mut config = TriggerConfig::from_created_payload(&payload).unwrap();

    // Switch from intent to script
    config.apply_update(&json!({
        "trigger_id": "test",
        "run": { "type": "script", "path": "test/run.py" }
    }));
    if let TriggerRun::Script { path } = &config.run {
        assert_eq!(path, "test/run.py");
    } else {
        panic!("Expected Script variant");
    }

    // Switch back to intent
    config.apply_update(&json!({
        "trigger_id": "test",
        "run": { "type": "intent", "text": "new prompt" }
    }));
    if let TriggerRun::Intent { intent } = &config.run {
        assert_eq!(intent, "new prompt");
    } else {
        panic!("Expected Intent variant");
    }
}

#[test]
fn validate_script_extension_py() {
    assert!(validate_script_extension("triggers/oura/scripts/run.py").is_ok());
}

#[test]
fn validate_script_extension_sh() {
    assert!(validate_script_extension("triggers/backup/scripts/run.sh").is_ok());
}

#[test]
fn validate_script_extension_unsupported() {
    let err = validate_script_extension("scripts/run.rb").unwrap_err();
    assert!(err.contains(".rb"));
    assert!(err.contains("Unsupported"));
}

#[test]
fn validate_script_extension_missing() {
    let err = validate_script_extension("scripts/run").unwrap_err();
    assert!(err.contains("must have a file extension"));
}

#[test]
fn paused_defaults_to_false_for_new_trigger() {
    let payload = json!({
        "trigger_id": "t1", "name": "T", "schedule": ["0 0 8 * * *"], "timezone": "UTC",
        "run": { "type": "intent", "text": "x", "knowhow": [] }
    });
    let config = TriggerConfig::from_created_payload(&payload).unwrap();
    assert!(!config.paused);
}

#[test]
fn from_created_payload_reads_explicit_paused_true() {
    let payload = json!({
        "trigger_id": "t1", "name": "T", "schedule": ["0 0 8 * * *"], "timezone": "UTC",
        "run": { "type": "intent", "text": "x", "knowhow": [] },
        "paused": true
    });
    let config = TriggerConfig::from_created_payload(&payload).unwrap();
    assert!(config.paused);
}

#[test]
fn from_created_payload_legacy_enabled_false_becomes_paused_true() {
    let payload = json!({
        "trigger_id": "t1", "name": "T", "schedule": ["0 0 8 * * *"], "timezone": "UTC",
        "run": { "type": "intent", "text": "x", "knowhow": [] },
        "enabled": false
    });
    let config = TriggerConfig::from_created_payload(&payload).unwrap();
    assert!(config.paused);
}

#[test]
fn apply_update_paused_field() {
    let payload = json!({
        "trigger_id": "t1", "name": "T", "schedule": ["0 0 8 * * *"], "timezone": "UTC",
        "run": { "type": "intent", "text": "x", "knowhow": [] }
    });
    let mut config = TriggerConfig::from_created_payload(&payload).unwrap();
    assert!(!config.paused);
    config.apply_update(&json!({ "trigger_id": "t1", "paused": true }));
    assert!(config.paused);
    config.apply_update(&json!({ "trigger_id": "t1", "paused": false }));
    assert!(!config.paused);
}

#[test]
fn apply_update_legacy_enabled_field_inverts_to_paused() {
    let payload = json!({
        "trigger_id": "t1", "name": "T", "schedule": ["0 0 8 * * *"], "timezone": "UTC",
        "run": { "type": "intent", "text": "x", "knowhow": [] }
    });
    let mut config = TriggerConfig::from_created_payload(&payload).unwrap();
    config.apply_update(&json!({ "trigger_id": "t1", "enabled": false }));
    assert!(config.paused);
    config.apply_update(&json!({ "trigger_id": "t1", "enabled": true }));
    assert!(!config.paused);
}

#[test]
fn from_created_payload_reads_explicit_app_id() {
    // Regression: notification popover's "open the app" button compares
    // notification.app_id against app directory names. Triggers must be able
    // to declare which app dir they belong to so the comparison can match.
    let payload = json!({
        "trigger_id": "uuid-abc-123", "name": "Smart CI Nightly",
        "schedule": ["0 0 8 * * *"], "timezone": "UTC",
        "run": { "type": "intent", "text": "x", "knowhow": [] },
        "app_id": "trigger-workflow"
    });
    let config = TriggerConfig::from_created_payload(&payload).unwrap();
    assert_eq!(config.app_id, Some("trigger-workflow".to_string()));
    assert_eq!(config.owning_app_id(), Some("trigger-workflow".to_string()));
}

#[test]
fn from_created_payload_app_id_defaults_to_none() {
    // Existing triggers (and standalone ones) have no app_id — must round-trip as None,
    // never silently fall back to the trigger UUID.
    let payload = json!({
        "trigger_id": "uuid-abc-123", "name": "Standalone",
        "schedule": ["0 0 8 * * *"], "timezone": "UTC",
        "run": { "type": "intent", "text": "x", "knowhow": [] }
    });
    let config = TriggerConfig::from_created_payload(&payload).unwrap();
    assert_eq!(config.app_id, None);
    assert_eq!(
        config.owning_app_id(),
        None,
        "intent trigger without explicit app_id must not invent one"
    );
}

#[test]
fn owning_app_id_derives_from_apps_script_path() {
    // Legacy app-scoped script triggers have no explicit app_id field but their
    // path lives under `apps/<X>/...` — derive the app dir from there so the
    // notification popover's link still resolves.
    let payload = json!({
        "trigger_id": "uuid-1", "name": "Some script",
        "schedule": ["0 0 8 * * *"], "timezone": "UTC",
        "run": { "type": "script", "path": "apps/trigger-workflow/triggers/scripts/nightly.py" }
    });
    let config = TriggerConfig::from_created_payload(&payload).unwrap();
    assert_eq!(config.owning_app_id(), Some("trigger-workflow".to_string()));
}

#[test]
fn owning_app_id_none_for_standalone_script_path() {
    // Scripts under `data/triggers/<dir>/...` are standalone — must not be
    // misattributed to any app.
    let payload = json!({
        "trigger_id": "uuid-1", "name": "Oura import",
        "schedule": ["0 0 8 * * *"], "timezone": "UTC",
        "run": { "type": "script", "path": "triggers/oura-import/scripts/run.py" }
    });
    let config = TriggerConfig::from_created_payload(&payload).unwrap();
    assert_eq!(config.owning_app_id(), None);
}

#[test]
fn explicit_app_id_overrides_derivation() {
    // If a script trigger lives under apps/<X>/ but explicitly declares a
    // different owning app (e.g. moved/legacy), the explicit field wins.
    let payload = json!({
        "trigger_id": "uuid-1", "name": "Cross-app",
        "schedule": ["0 0 8 * * *"], "timezone": "UTC",
        "run": { "type": "script", "path": "apps/old-app/triggers/scripts/run.py" },
        "app_id": "new-app"
    });
    let config = TriggerConfig::from_created_payload(&payload).unwrap();
    assert_eq!(config.owning_app_id(), Some("new-app".to_string()));
}

#[test]
fn apply_update_sets_app_id() {
    let payload = json!({
        "trigger_id": "t1", "name": "T",
        "schedule": ["0 0 8 * * *"], "timezone": "UTC",
        "run": { "type": "intent", "text": "x", "knowhow": [] }
    });
    let mut config = TriggerConfig::from_created_payload(&payload).unwrap();
    assert_eq!(config.app_id, None);
    config.apply_update(&json!({ "trigger_id": "t1", "app_id": "trigger-workflow" }));
    assert_eq!(config.app_id, Some("trigger-workflow".to_string()));
}

#[test]
fn apply_update_clears_app_id_with_null() {
    let payload = json!({
        "trigger_id": "t1", "name": "T",
        "schedule": ["0 0 8 * * *"], "timezone": "UTC",
        "run": { "type": "intent", "text": "x", "knowhow": [] },
        "app_id": "trigger-workflow"
    });
    let mut config = TriggerConfig::from_created_payload(&payload).unwrap();
    assert_eq!(config.app_id, Some("trigger-workflow".to_string()));
    config.apply_update(&json!({ "trigger_id": "t1", "app_id": null }));
    assert_eq!(config.app_id, None);
}

#[test]
fn apply_update_absent_app_id_leaves_unchanged() {
    let payload = json!({
        "trigger_id": "t1", "name": "T",
        "schedule": ["0 0 8 * * *"], "timezone": "UTC",
        "run": { "type": "intent", "text": "x", "knowhow": [] },
        "app_id": "trigger-workflow"
    });
    let mut config = TriggerConfig::from_created_payload(&payload).unwrap();
    config.apply_update(&json!({ "trigger_id": "t1", "name": "Renamed" }));
    assert_eq!(config.name, "Renamed");
    assert_eq!(
        config.app_id,
        Some("trigger-workflow".to_string()),
        "absent app_id field must not clobber existing"
    );
}

#[test]
fn derive_app_id_from_apps_path() {
    assert_eq!(
        derive_app_id_from_script_path("apps/trigger-workflow/triggers/scripts/x.py"),
        Some("trigger-workflow".to_string())
    );
    assert_eq!(
        derive_app_id_from_script_path("apps/foo/scripts/y.sh"),
        Some("foo".to_string())
    );
}

#[test]
fn derive_app_id_returns_none_for_non_apps_paths() {
    assert_eq!(
        derive_app_id_from_script_path("triggers/oura/scripts/run.py"),
        None
    );
    assert_eq!(derive_app_id_from_script_path("scripts/legacy.py"), None);
    assert_eq!(derive_app_id_from_script_path("apps/"), None);
    assert_eq!(derive_app_id_from_script_path(""), None);
}

#[test]
fn derive_app_id_rejects_traversal_and_dotfile_dirs() {
    // A malformed `apps/..` or `apps/.git` path must not become a fake app
    // id on the frontend popover.
    assert_eq!(derive_app_id_from_script_path("apps/../foo/bar"), None);
    assert_eq!(derive_app_id_from_script_path("apps/./foo"), None);
    assert_eq!(derive_app_id_from_script_path("apps/.git/x/y"), None);
    assert_eq!(derive_app_id_from_script_path("apps//foo"), None);
}

#[test]
fn trigger_config_carries_slug_field() {
    let payload = json!({
        "trigger_id": "uuid-1", "name": "Send Daily Summary",
        "slug": "send-daily-summary",
        "schedule": ["0 0 8 * * *"], "timezone": "UTC",
        "run": { "type": "intent", "text": "x" }
    });
    let config = TriggerConfig::from_created_payload(&payload).unwrap();
    assert_eq!(config.slug, "send-daily-summary");
}

#[test]
fn trigger_config_derives_slug_from_name_when_missing() {
    // Legacy events lack the `slug` field — must derive from `name` so
    // existing workspaces resolve trigger knowhow without a backfill.
    let payload = json!({
        "trigger_id": "uuid-1", "name": "Nightly CI Build",
        "schedule": ["0 0 8 * * *"], "timezone": "UTC",
        "run": { "type": "intent", "text": "x" }
    });
    let config = TriggerConfig::from_created_payload(&payload).unwrap();
    assert_eq!(config.slug, "nightly-ci-build");
}

// Kebab-case slugification itself is covered by `core::slug`, which owns the
// shared function; only the trigger-specific fallback is tested here.
#[test]
fn slug_kebab_falls_back_to_uuid_short_when_empty() {
    let s = slugify_trigger_name_with_fallback("!!!", "abcdef-1234-5678");
    assert_eq!(s, "trigger-abcdef12");
}

#[test]
fn apply_update_accepts_slug_edit() {
    let payload = json!({
        "trigger_id": "t1", "name": "Old Name",
        "schedule": ["0 0 8 * * *"], "timezone": "UTC",
        "run": { "type": "intent", "text": "x" }
    });
    let mut config = TriggerConfig::from_created_payload(&payload).unwrap();
    assert_eq!(config.slug, "old-name");

    config.apply_update(&json!({ "trigger_id": "t1", "slug": "renamed" }));
    assert_eq!(config.slug, "renamed");
}

#[test]
fn apply_update_ignores_invalid_slug() {
    let payload = json!({
        "trigger_id": "t1", "name": "Original",
        "schedule": ["0 0 8 * * *"], "timezone": "UTC",
        "run": { "type": "intent", "text": "x" }
    });
    let mut config = TriggerConfig::from_created_payload(&payload).unwrap();
    let before = config.slug.clone();

    config.apply_update(&json!({ "trigger_id": "t1", "slug": "Has Spaces" }));
    assert_eq!(
        config.slug, before,
        "invalid slug must not clobber existing"
    );
}

#[test]
fn is_valid_trigger_slug_accepts_well_formed() {
    assert!(is_valid_trigger_slug("a"));
    assert!(is_valid_trigger_slug("9"));
    assert!(is_valid_trigger_slug("send-daily-summary"));
    assert!(is_valid_trigger_slug("trigger-abc12345"));
}

#[test]
fn is_valid_trigger_slug_rejects_malformed() {
    assert!(!is_valid_trigger_slug(""));
    assert!(!is_valid_trigger_slug("-leading-dash"));
    assert!(!is_valid_trigger_slug("trailing-dash-"));
    assert!(!is_valid_trigger_slug("Has Capitals"));
    assert!(!is_valid_trigger_slug("under_score"));
    assert!(!is_valid_trigger_slug(&"a".repeat(65)));
}

#[test]
fn from_created_payload_reads_explicit_group_id() {
    let payload = json!({
        "trigger_id": "t1", "name": "T",
        "schedule": ["0 0 8 * * *"], "timezone": "UTC",
        "run": { "type": "intent", "text": "x" },
        "group_id": "group-uuid-1"
    });
    let config = TriggerConfig::from_created_payload(&payload).unwrap();
    assert_eq!(config.group_id, Some("group-uuid-1".to_string()));
}

#[test]
fn from_created_payload_group_id_defaults_to_none() {
    // Legacy events lack group_id — must round-trip as None so existing
    // triggers render under the "Ungrouped" section.
    let payload = json!({
        "trigger_id": "t1", "name": "T",
        "schedule": ["0 0 8 * * *"], "timezone": "UTC",
        "run": { "type": "intent", "text": "x" }
    });
    let config = TriggerConfig::from_created_payload(&payload).unwrap();
    assert_eq!(config.group_id, None);
}

#[test]
fn apply_update_sets_group_id() {
    let payload = json!({
        "trigger_id": "t1", "name": "T",
        "schedule": ["0 0 8 * * *"], "timezone": "UTC",
        "run": { "type": "intent", "text": "x" }
    });
    let mut config = TriggerConfig::from_created_payload(&payload).unwrap();
    assert_eq!(config.group_id, None);
    config.apply_update(&json!({ "trigger_id": "t1", "group_id": "group-uuid-1" }));
    assert_eq!(config.group_id, Some("group-uuid-1".to_string()));
}

#[test]
fn apply_update_clears_group_id_with_null() {
    let payload = json!({
        "trigger_id": "t1", "name": "T",
        "schedule": ["0 0 8 * * *"], "timezone": "UTC",
        "run": { "type": "intent", "text": "x" },
        "group_id": "group-uuid-1"
    });
    let mut config = TriggerConfig::from_created_payload(&payload).unwrap();
    assert_eq!(config.group_id, Some("group-uuid-1".to_string()));
    config.apply_update(&json!({ "trigger_id": "t1", "group_id": null }));
    assert_eq!(config.group_id, None);
}

#[test]
fn apply_update_absent_group_id_leaves_unchanged() {
    let payload = json!({
        "trigger_id": "t1", "name": "T",
        "schedule": ["0 0 8 * * *"], "timezone": "UTC",
        "run": { "type": "intent", "text": "x" },
        "group_id": "group-uuid-1"
    });
    let mut config = TriggerConfig::from_created_payload(&payload).unwrap();
    config.apply_update(&json!({ "trigger_id": "t1", "name": "Renamed" }));
    assert_eq!(config.name, "Renamed");
    assert_eq!(
        config.group_id,
        Some("group-uuid-1".to_string()),
        "absent group_id field must not clobber existing"
    );
}

#[test]
fn next_run_returns_none_when_paused() {
    let payload = json!({
        "trigger_id": "t1", "name": "T", "schedule": ["0 0 8 * * *"], "timezone": "UTC",
        "run": { "type": "intent", "text": "x", "knowhow": [] }
    });
    let mut config = TriggerConfig::from_created_payload(&payload).unwrap();
    assert!(config.next_run().is_some());
    config.paused = true;
    assert!(config.next_run().is_none());
}

// --- Side-effect grant (ADR 0002, Phase 5) ---

#[test]
fn from_created_payload_parses_side_effect_grant() {
    use crate::engine::command_guard::SideEffectCategory;
    let payload = json!({
        "trigger_id": "t1", "name": "T", "schedule": ["0 0 8 * * *"], "timezone": "UTC",
        "run": { "type": "intent", "intent": "x" },
        "side_effect_grant": ["email", "external_api"],
    });
    let config = TriggerConfig::from_created_payload(&payload).unwrap();
    assert_eq!(
        config.side_effect_grant,
        vec![SideEffectCategory::Email, SideEffectCategory::ExternalApi]
    );
}

#[test]
fn from_created_payload_side_effect_grant_defaults_empty_and_skips_unknown() {
    use crate::engine::command_guard::SideEffectCategory;
    // Absent → empty (no grant).
    let payload = json!({
        "trigger_id": "t1", "name": "T", "schedule": ["0 0 8 * * *"], "timezone": "UTC",
        "run": { "type": "intent", "intent": "x" },
    });
    assert!(TriggerConfig::from_created_payload(&payload)
        .unwrap()
        .side_effect_grant
        .is_empty());

    // Unknown / forward-compat entries are skipped, duplicates deduped.
    let payload = json!({
        "trigger_id": "t1", "name": "T", "schedule": ["0 0 8 * * *"], "timezone": "UTC",
        "run": { "type": "intent", "intent": "x" },
        "side_effect_grant": ["email", "email", "future_category", "cloud_cli"],
    });
    assert_eq!(
        TriggerConfig::from_created_payload(&payload)
            .unwrap()
            .side_effect_grant,
        vec![SideEffectCategory::Email, SideEffectCategory::CloudCli]
    );
}

#[test]
fn apply_update_replaces_and_clears_side_effect_grant() {
    use crate::engine::command_guard::SideEffectCategory;
    let payload = json!({
        "trigger_id": "t1", "name": "T", "schedule": ["0 0 8 * * *"], "timezone": "UTC",
        "run": { "type": "intent", "intent": "x" },
        "side_effect_grant": ["email"],
    });
    let mut config = TriggerConfig::from_created_payload(&payload).unwrap();

    // Replacement.
    config.apply_update(&json!({ "trigger_id": "t1", "side_effect_grant": ["cloud_cli"] }));
    assert_eq!(config.side_effect_grant, vec![SideEffectCategory::CloudCli]);

    // Absent field leaves it as-is.
    config.apply_update(&json!({ "trigger_id": "t1", "name": "T2" }));
    assert_eq!(config.side_effect_grant, vec![SideEffectCategory::CloudCli]);

    // Empty array clears it.
    config.apply_update(&json!({ "trigger_id": "t1", "side_effect_grant": [] }));
    assert!(config.side_effect_grant.is_empty());
}
