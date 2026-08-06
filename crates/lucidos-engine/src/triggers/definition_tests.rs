use super::{
    ensure_trigger_toml_gitignored, rebuild_trigger_definitions, remove_trigger_definition,
    trigger_toml_data_relpath, write_trigger_definition, TriggerDefinition,
};
use crate::engine::command_guard::SideEffectCategory;
use crate::triggers::config::{TriggerConfig, TriggerRun};
use crate::triggers::EventSubscription;

fn tmpdir(name: &str) -> std::path::PathBuf {
    let p = std::env::temp_dir().join(format!(
        "lucidos_trigger_defn_{}_{}",
        name,
        uuid::Uuid::new_v4()
    ));
    std::fs::create_dir_all(&p).unwrap();
    p
}

fn config(slug: &str, run: TriggerRun) -> TriggerConfig {
    TriggerConfig {
        id: uuid::Uuid::new_v4().to_string(),
        name: format!("Trigger {slug}"),
        slug: slug.to_string(),
        schedule: vec![],
        timezone: "UTC".to_string(),
        run,
        on: vec![],
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

#[test]
fn roundtrips_an_intent_trigger() {
    let c = config(
        "daily-reflect",
        TriggerRun::Intent {
            intent: "Reflect on the day".to_string(),
        },
    );
    let def = TriggerDefinition::from_config(&c);
    let toml = def.to_toml().expect("serialize");
    let parsed = TriggerDefinition::from_toml(&toml).expect("parse");
    assert_eq!(parsed, def);
}

#[test]
fn roundtrips_an_event_trigger_with_condition_and_grant() {
    let mut c = config(
        "low-sleep-nudge",
        TriggerRun::Script {
            path: "triggers/low-sleep-nudge/scripts/run.py".to_string(),
        },
    );
    c.on = vec![EventSubscription {
        event_type: "SleepImported".to_string(),
        condition: Some(serde_json::json!({ "sleep_score": { "$lt": 70 } })),
    }];
    c.side_effect_grant = vec![SideEffectCategory::Email, SideEffectCategory::ExternalApi];
    c.app_id = Some("sleep".to_string());
    c.go_to_review = true;

    let def = TriggerDefinition::from_config(&c);
    let toml = def.to_toml().expect("serialize event trigger");
    let parsed = TriggerDefinition::from_toml(&toml).expect("parse event trigger");
    assert_eq!(parsed, def);
    assert_eq!(parsed.on.len(), 1);
    assert_eq!(parsed.on[0].event_type, "SleepImported");
    assert!(parsed.on[0].condition.is_some());
}

#[test]
fn to_trigger_payload_stamps_provenance_and_forces_event_driven() {
    let mut c = config(
        "on-x",
        TriggerRun::Intent {
            intent: "react".to_string(),
        },
    );
    c.on = vec![EventSubscription {
        event_type: "X".to_string(),
        condition: None,
    }];
    // Even a (hypothetical) cron schedule on the def is dropped — plugin
    // triggers register event-driven only.
    c.schedule = vec!["0 0 3 * * *".to_string()];
    let def = TriggerDefinition::from_config(&c);
    let payload = def.to_trigger_payload("new-id", "my-plugin");

    // Round-trip through the same parser the scheduler uses.
    let parsed = TriggerConfig::from_created_payload(&payload).expect("payload parses");
    assert_eq!(parsed.id, "new-id");
    assert_eq!(parsed.plugin_id.as_deref(), Some("my-plugin"));
    assert!(parsed.schedule.is_empty(), "schedule forced empty");
    assert_eq!(parsed.on.len(), 1);
}

#[test]
fn from_config_drops_runtime_state() {
    let mut c = config(
        "x",
        TriggerRun::Intent {
            intent: "hi".to_string(),
        },
    );
    c.paused = true;
    c.last_run = Some(chrono::Utc::now());
    let toml = TriggerDefinition::from_config(&c).to_toml().unwrap();
    // Runtime/identity state must NOT appear in the on-disk definition.
    assert!(!toml.contains("paused"));
    assert!(!toml.contains("last_run"));
    assert!(!toml.contains(&c.id));
}

#[test]
fn writes_then_removes_definition_and_prunes_empty_dir() {
    let ws = tmpdir("write_remove");
    let c = config(
        "watcher",
        TriggerRun::Intent {
            intent: "watch".to_string(),
        },
    );
    write_trigger_definition(&ws, &c);
    let file = ws.join("data").join(trigger_toml_data_relpath("watcher"));
    assert!(file.exists(), "trigger.toml should be written");

    remove_trigger_definition(&ws, "watcher");
    assert!(!file.exists(), "trigger.toml should be removed");
    assert!(
        !ws.join("data/triggers/watcher").exists(),
        "empty slug dir should be pruned"
    );
}

#[test]
fn remove_keeps_dir_with_sibling_knowhow() {
    let ws = tmpdir("keep_sibling");
    let c = config(
        "keeper",
        TriggerRun::Intent {
            intent: "k".to_string(),
        },
    );
    write_trigger_definition(&ws, &c);
    let knowhow = ws.join("data/triggers/keeper/knowhow");
    std::fs::create_dir_all(&knowhow).unwrap();
    std::fs::write(knowhow.join("notes.md"), "hi").unwrap();

    remove_trigger_definition(&ws, "keeper");
    assert!(!ws.join("data/triggers/keeper/trigger.toml").exists());
    // Sibling knowhow → the dir must survive (only trigger.toml is pruned).
    assert!(
        knowhow.join("notes.md").exists(),
        "sibling knowhow must remain"
    );
}

#[test]
fn rebuild_writes_live_and_prunes_orphans() {
    let ws = tmpdir("rebuild");
    // Seed an orphan trigger.toml that is NOT in the live set.
    let orphan = ws.join("data/triggers/gone/trigger.toml");
    std::fs::create_dir_all(orphan.parent().unwrap()).unwrap();
    std::fs::write(&orphan, "name = \"stale\"\n").unwrap();

    let live = vec![config(
        "alive",
        TriggerRun::Intent {
            intent: "live".to_string(),
        },
    )];
    rebuild_trigger_definitions(&ws, &live);

    assert!(
        ws.join("data/triggers/alive/trigger.toml").exists(),
        "live trigger.toml written"
    );
    assert!(!orphan.exists(), "orphan trigger.toml pruned");
}

#[test]
fn ensure_gitignored_is_idempotent() {
    let ws = tmpdir("gitignore");
    std::fs::create_dir_all(ws.join(".git/info")).unwrap();
    ensure_trigger_toml_gitignored(&ws);
    ensure_trigger_toml_gitignored(&ws);
    let exclude = std::fs::read_to_string(ws.join(".git/info/exclude")).unwrap();
    let occurrences = exclude
        .lines()
        .filter(|l| l.trim() == "data/triggers/*/trigger.toml")
        .count();
    assert_eq!(occurrences, 1, "pattern added exactly once");
}
