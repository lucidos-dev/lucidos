use super::{
    ensure_trigger_toml_gitignored, rebuild_trigger_definitions, remove_trigger_definition,
    write_trigger_definition, TriggerDefinition,
};
use crate::engine::command_guard::SideEffectCategory;
use crate::triggers::config::{TriggerConfig, TriggerRun};
use crate::triggers::EventSubscription;

/// A private fixture workspace, removed when the guard drops. Hold the guard
/// for as long as the test reads the tree: dropping it deletes the directory.
fn tmpdir(name: &str) -> tempfile::TempDir {
    let prefix = format!("lucidos_trigger_defn_{name}_");
    tempfile::Builder::new()
        .prefix(&prefix)
        .tempdir()
        .expect("tempdir")
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
        model: None,
        reasoning_effort: None,
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

/// `model` / `reasoning_effort` are scalars, so they must serialise ABOVE the
/// `on` array-of-tables and the `run` table. Emitted after either, the `toml`
/// crate rejects the whole document and the projection silently stops being
/// written (see the ordering note at the top of `definition.rs`).
#[test]
fn roundtrips_a_trigger_pinned_to_a_model_and_effort() {
    let mut c = config(
        "cheap-digest",
        TriggerRun::Intent {
            intent: "Summarize today".to_string(),
        },
    );
    c.model = Some("gemini-3.5-flash".to_string());
    c.reasoning_effort = Some("low".to_string());
    c.on = vec![EventSubscription {
        event_type: "DayEnded".to_string(),
        condition: None,
    }];

    let def = TriggerDefinition::from_config(&c);
    let toml = def.to_toml().expect("serialize a model-pinned trigger");
    let parsed = TriggerDefinition::from_toml(&toml).expect("parse a model-pinned trigger");
    assert_eq!(parsed, def);
    assert_eq!(parsed.model.as_deref(), Some("gemini-3.5-flash"));
    assert_eq!(parsed.reasoning_effort.as_deref(), Some("low"));
}

/// A trigger on the account default writes no model keys at all, so an existing
/// workspace's projected files are byte-identical after this change.
#[test]
fn default_model_is_omitted_from_the_projection() {
    let c = config(
        "plain",
        TriggerRun::Intent {
            intent: "hi".to_string(),
        },
    );
    let toml = TriggerDefinition::from_config(&c).to_toml().unwrap();
    assert!(!toml.contains("model"));
    assert!(!toml.contains("reasoning_effort"));
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
    let tmp = tmpdir("write_remove");
    let ws = tmp.path();
    let c = config(
        "watcher",
        TriggerRun::Intent {
            intent: "watch".to_string(),
        },
    );
    write_trigger_definition(ws, &c);
    let file = ws.join("data/triggers/watcher/trigger.toml");
    assert!(file.exists(), "trigger.toml should be written");

    remove_trigger_definition(ws, "watcher");
    assert!(!file.exists(), "trigger.toml should be removed");
    assert!(
        !ws.join("data/triggers/watcher").exists(),
        "empty slug dir should be pruned"
    );
}

#[test]
fn remove_keeps_dir_with_sibling_knowhow() {
    let tmp = tmpdir("keep_sibling");
    let ws = tmp.path();
    let c = config(
        "keeper",
        TriggerRun::Intent {
            intent: "k".to_string(),
        },
    );
    write_trigger_definition(ws, &c);
    let knowhow = ws.join("data/triggers/keeper/knowhow");
    std::fs::create_dir_all(&knowhow).unwrap();
    std::fs::write(knowhow.join("notes.md"), "hi").unwrap();

    remove_trigger_definition(ws, "keeper");
    assert!(!ws.join("data/triggers/keeper/trigger.toml").exists());
    // Sibling knowhow → the dir must survive (only trigger.toml is pruned).
    assert!(
        knowhow.join("notes.md").exists(),
        "sibling knowhow must remain"
    );
}

#[test]
fn rebuild_writes_live_and_prunes_orphans() {
    let tmp = tmpdir("rebuild");
    let ws = tmp.path();
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
    rebuild_trigger_definitions(ws, &live);

    assert!(
        ws.join("data/triggers/alive/trigger.toml").exists(),
        "live trigger.toml written"
    );
    assert!(!orphan.exists(), "orphan trigger.toml pruned");
}

#[test]
fn ensure_gitignored_is_idempotent() {
    let tmp = tmpdir("gitignore");
    let ws = tmp.path();
    std::fs::create_dir_all(ws.join(".git/info")).unwrap();
    ensure_trigger_toml_gitignored(ws);
    ensure_trigger_toml_gitignored(ws);
    let exclude = std::fs::read_to_string(ws.join(".git/info/exclude")).unwrap();
    let occurrences = exclude
        .lines()
        .filter(|l| l.trim() == "data/triggers/*/trigger.toml")
        .count();
    assert_eq!(occurrences, 1, "pattern added exactly once");
}

#[test]
fn ensure_gitignored_appends_to_the_users_existing_patterns() {
    let tmp = tmpdir("gitignore_append");
    let ws = tmp.path();
    let exclude = ws.join(".git/info/exclude");
    std::fs::create_dir_all(exclude.parent().unwrap()).unwrap();
    std::fs::write(&exclude, "*.swp\nscratch/\n").unwrap();

    ensure_trigger_toml_gitignored(ws);

    let text = std::fs::read_to_string(&exclude).unwrap();
    assert!(text.contains("*.swp"), "user pattern kept: {text}");
    assert!(text.contains("scratch/"), "user pattern kept: {text}");
    assert!(text.contains("data/triggers/*/trigger.toml"));
}

/// An unreadable exclude file is left exactly as it was.
///
/// The read used to `unwrap_or_default`. A non-UTF-8 byte then read as an
/// empty file, and the write replaced every pattern the user had.
#[test]
fn ensure_gitignored_leaves_an_unreadable_exclude_untouched() {
    let tmp = tmpdir("gitignore_unreadable");
    let ws = tmp.path();
    let exclude = ws.join(".git/info/exclude");
    std::fs::create_dir_all(exclude.parent().unwrap()).unwrap();
    // A lone 0xFF byte is not valid UTF-8, so `read_to_string` fails with
    // `InvalidData` rather than reporting a missing file.
    let original: Vec<u8> = b"# keep me\nsecrets.env\n\xFF\n".to_vec();
    std::fs::write(&exclude, &original).unwrap();

    ensure_trigger_toml_gitignored(ws);

    assert_eq!(
        std::fs::read(&exclude).unwrap(),
        original,
        "an unreadable exclude file must not be rewritten"
    );
}
