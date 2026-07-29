//! Plugin trigger auto-registration validation (ADR 0019).

use super::{plugin_trigger_slug, validate_plugin_triggers_event_driven};
use crate::core::plugins::PlannedFile;
use std::path::PathBuf;

fn tmpfile(name: &str, contents: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!(
        "lucidos_plugin_trig_{}_{}",
        name,
        uuid::Uuid::new_v4()
    ));
    std::fs::write(&p, contents).unwrap();
    p
}

#[test]
fn plugin_trigger_slug_matches_only_trigger_toml() {
    assert_eq!(
        plugin_trigger_slug("triggers/daily-reflect/trigger.toml"),
        Some("daily-reflect")
    );
    // Not a trigger.toml / wrong depth / other content dirs → None.
    assert_eq!(plugin_trigger_slug("triggers/daily-reflect/knowhow/a.md"), None);
    assert_eq!(plugin_trigger_slug("triggers/trigger.toml"), None);
    assert_eq!(plugin_trigger_slug("apps/foo/trigger.toml"), None);
    assert_eq!(plugin_trigger_slug("knowhow/x.md"), None);
}

#[test]
fn event_driven_trigger_passes_validation() {
    let src = tmpfile(
        "event",
        r#"
name = "On sleep"
[run]
type = "intent"
intent = "react"
[[on]]
event_type = "SleepImported"
"#,
    );
    let planned = vec![PlannedFile {
        source: src,
        data_relative: "triggers/on-sleep/trigger.toml".to_string(),
    }];
    assert!(validate_plugin_triggers_event_driven(&planned).is_ok());
}

#[test]
fn cron_bearing_trigger_is_rejected() {
    let src = tmpfile(
        "cron",
        r#"
name = "Nightly"
schedule = ["0 0 3 * * *"]
timezone = "UTC"
[run]
type = "intent"
intent = "run"
"#,
    );
    let planned = vec![PlannedFile {
        source: src,
        data_relative: "triggers/nightly/trigger.toml".to_string(),
    }];
    let err = validate_plugin_triggers_event_driven(&planned).unwrap_err();
    assert!(err.contains("cron schedule"), "unexpected error: {err}");
}

#[test]
fn malformed_trigger_toml_is_rejected() {
    let src = tmpfile("bad", "this is not = valid toml [[[");
    let planned = vec![PlannedFile {
        source: src,
        data_relative: "triggers/bad/trigger.toml".to_string(),
    }];
    assert!(validate_plugin_triggers_event_driven(&planned).is_err());
}

#[test]
fn non_trigger_files_are_ignored_by_validation() {
    let planned = vec![PlannedFile {
        source: PathBuf::from("/nonexistent/does-not-matter"),
        data_relative: "knowhow/notes.md".to_string(),
    }];
    // No trigger.toml → no read attempted → Ok even though the path is bogus.
    assert!(validate_plugin_triggers_event_driven(&planned).is_ok());
}
