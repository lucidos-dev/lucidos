use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Supported script file extensions and their runtime labels.
pub const SUPPORTED_SCRIPT_EXTENSIONS: &[(&str, &str)] = &[("py", "Python"), ("sh", "Bash")];

/// Validate that a script path has a supported file extension.
/// Returns `Ok(())` if valid, `Err(message)` if unsupported or missing.
pub fn validate_script_extension(path: &str) -> Result<(), String> {
    let ext = std::path::Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("");
    if SUPPORTED_SCRIPT_EXTENSIONS.iter().any(|(e, _)| *e == ext) {
        return Ok(());
    }
    let list = supported_extensions_display();
    if ext.is_empty() {
        Err(format!(
            "Script path must have a file extension (supported: {})",
            list
        ))
    } else {
        Err(format!(
            "Unsupported script extension '.{}' (supported: {})",
            ext, list
        ))
    }
}

/// Format the supported extensions list for error messages.
fn supported_extensions_display() -> String {
    SUPPORTED_SCRIPT_EXTENSIONS
        .iter()
        .map(|(e, _)| *e)
        .collect::<Vec<_>>()
        .join(", ")
}

/// What a trigger executes — either an LLM intent or a deterministic script.
/// The tagged union enforces mutual exclusivity: know-how is only relevant for intents.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum TriggerRun {
    #[serde(rename = "intent", alias = "prompt")]
    Intent {
        #[serde(alias = "text")]
        intent: String,
        #[serde(default)]
        knowhow: Vec<String>,
    },
    #[serde(rename = "script")]
    Script { path: String },
}

/// In-memory representation of a trigger, rebuilt from events on startup.
#[derive(Debug, Clone, Serialize)]
pub struct TriggerConfig {
    pub id: String,
    pub name: String,
    pub schedule: Vec<String>,
    pub timezone: String,
    pub run: TriggerRun,
    pub on: Option<String>,
    pub condition: Option<Value>,
    pub paused: bool,
    pub last_run: Option<DateTime<Utc>>,
    /// Directory name of the app that owns this trigger (e.g. `"trigger-workflow"`),
    /// stamped onto `NotificationCreated.app_id` so the popover can deep-link to
    /// the app. None for standalone triggers. For script triggers under
    /// `apps/<X>/...` without an explicit value, `owning_app_id` derives `<X>`.
    pub app_id: Option<String>,
    /// When true, threads spawned by this trigger surface in REVIEW on
    /// completion instead of going straight to HISTORY. Use for triggers
    /// whose output the user is expected to read — daily summaries, alerts,
    /// scheduled reports. Default false preserves the unattended-execution
    /// behavior expected of most cron triggers.
    pub go_to_review: bool,
}

impl TriggerConfig {
    /// Build a TriggerConfig from a TriggerCreated event payload.
    pub fn from_created_payload(payload: &Value) -> Result<Self, String> {
        let id = payload["trigger_id"]
            .as_str()
            .ok_or("Missing trigger_id")?
            .to_string();
        let name = payload["name"].as_str().ok_or("Missing name")?.to_string();
        let schedule = payload["schedule"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();
        let timezone = payload["timezone"].as_str().unwrap_or("UTC").to_string();
        let run: TriggerRun = serde_json::from_value(payload["run"].clone())
            .map_err(|e| format!("Invalid run field: {}", e))?;
        let on = payload["on"].as_str().map(String::from);
        let condition = payload.get("condition").filter(|v| !v.is_null()).cloned();

        let paused = read_paused_field(payload).unwrap_or(false);
        let app_id = payload
            .get("app_id")
            .and_then(|v| v.as_str())
            .map(String::from);
        let go_to_review = payload
            .get("go_to_review")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        Ok(TriggerConfig {
            id,
            name,
            schedule,
            timezone,
            run,
            on,
            condition,
            paused,
            last_run: None,
            app_id,
            go_to_review,
        })
    }

    /// Directory name of the app that owns this trigger, used to stamp notifications.
    /// Prefers the explicit `app_id` field; for script triggers without one, falls back
    /// to the leading `apps/<dir>/` path segment so legacy app-scoped scripts still link
    /// back to their app. Returns None when the trigger is genuinely standalone.
    pub fn owning_app_id(&self) -> Option<String> {
        if let Some(ref aid) = self.app_id {
            return Some(aid.clone());
        }
        if let TriggerRun::Script { ref path } = self.run {
            return derive_app_id_from_script_path(path);
        }
        None
    }

    /// Compute the next scheduled run time (UTC) from cron expressions and timezone.
    /// Returns None if the trigger is paused, has no cron expressions, or no future match exists.
    pub fn next_run(&self) -> Option<DateTime<Utc>> {
        if self.paused || self.schedule.is_empty() {
            return None;
        }
        let schedules: Vec<cron::Schedule> = self
            .schedule
            .iter()
            .filter_map(|expr| {
                crate::engine::tools::scheduler::parse_standard_cron(expr)
                    .map_err(|e| {
                        log!(
                            "[Triggers] Corrupt cron expression '{}' in trigger {}: {}",
                            expr,
                            self.id,
                            e
                        );
                    })
                    .ok()
            })
            .collect();
        let tz: chrono_tz::Tz = self.timezone.parse().unwrap_or_else(|_| {
            log!(
                "[Triggers] Invalid timezone '{}' for trigger {}, using UTC",
                self.timezone,
                self.id
            );
            chrono_tz::UTC
        });
        crate::engine::tools::scheduler::next_occurrence_multi(&schedules, tz)
            .map(|dt| dt.with_timezone(&Utc))
    }

    /// Human-readable trigger type label.
    pub fn trigger_type_label(&self) -> &'static str {
        let has_cron = !self.schedule.is_empty();
        let has_event = self.on.is_some();
        match (has_cron, has_event) {
            (true, true) => "Hybrid",
            (false, true) => "Event",
            _ => "Schedule",
        }
    }

    /// Apply a partial update from a TriggerUpdated event payload.
    /// Only fields present in the payload are updated.
    pub fn apply_update(&mut self, payload: &Value) {
        if let Some(name) = payload["name"].as_str() {
            self.name = name.to_string();
        }
        if let Some(schedule) = payload["schedule"].as_array() {
            self.schedule = schedule
                .iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect();
        }
        if let Some(tz) = payload["timezone"].as_str() {
            self.timezone = tz.to_string();
        }
        if payload.get("run").is_some() && !payload["run"].is_null() {
            if let Ok(run) = serde_json::from_value(payload["run"].clone()) {
                self.run = run;
            }
        }
        if payload.get("on").is_some() {
            self.on = payload["on"].as_str().map(String::from);
        }
        if payload.get("condition").is_some() {
            self.condition = payload.get("condition").filter(|v| !v.is_null()).cloned();
        }
        if let Some(paused) = read_paused_field(payload) {
            self.paused = paused;
        }
        // app_id update: explicit null clears, string sets, absent leaves as-is
        if let Some(v) = payload.get("app_id") {
            if v.is_null() {
                self.app_id = None;
            } else if let Some(s) = v.as_str() {
                self.app_id = Some(s.to_string());
            }
        }
        if let Some(v) = payload.get("go_to_review").and_then(|v| v.as_bool()) {
            self.go_to_review = v;
        }
    }
}

/// Extract the owning app directory from a script path, if it lives under `apps/<X>/`.
/// Rejects path-traversal segments (`.`, `..`) and leading-dot dirs so a malformed
/// path can't become a fake app id on the frontend popover.
/// Examples:
/// - `"apps/trigger-workflow/triggers/scripts/run.py"` → `Some("trigger-workflow")`
/// - `"triggers/oura-import/scripts/run.py"` → `None`
/// - `"apps/../foo/bar"` / `"apps/.git/x"` / `"apps//x"` → `None`
pub(crate) fn derive_app_id_from_script_path(path: &str) -> Option<String> {
    let mut parts = path.split('/');
    if parts.next()? != "apps" {
        return None;
    }
    let dir = parts.next()?;
    if dir.is_empty() || dir.starts_with('.') {
        return None;
    }
    Some(dir.to_string())
}

/// Read the paused state from an event payload.
/// Prefers the new `paused` field; falls back to the inverted legacy `enabled` field
/// so events persisted before the rename still apply correctly. Returns None when neither
/// field is present, letting callers decide their default.
fn read_paused_field(payload: &Value) -> Option<bool> {
    payload
        .get("paused")
        .and_then(|v| v.as_bool())
        .or_else(|| {
            payload
                .get("enabled")
                .and_then(|v| v.as_bool())
                .map(|e| !e)
        })
}

#[cfg(test)]
mod tests {
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
    fn prompt_knowhow() {
        let payload = json!({
            "trigger_id": "heatpump-logging",
            "name": "Heatpump Logging",
            "schedule": ["0 */30 * * * *"],
            "timezone": "Europe/Oslo",
            "run": { "type": "prompt", "text": "Log heat pump status", "knowhow": ["heatpump"] }
        });

        let config = TriggerConfig::from_created_payload(&payload).unwrap();
        if let TriggerRun::Intent { knowhow, .. } = &config.run {
            assert_eq!(knowhow, &vec!["heatpump".to_string()]);
        } else {
            panic!("Expected Intent variant");
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
        if let TriggerRun::Intent { intent, knowhow } = run {
            assert_eq!(intent, "legacy payload");
            assert!(knowhow.is_empty());
        } else {
            panic!("Expected Intent variant");
        }
    }

    #[test]
    fn trigger_run_serde_roundtrip() {
        let prompt = TriggerRun::Intent {
            intent: "Do something".into(),
            knowhow: vec!["domain".into()],
        };
        let json = serde_json::to_value(&prompt).unwrap();
        assert_eq!(json["type"], "intent");
        assert_eq!(json["intent"], "Do something");
        assert!(
            json.get("text").is_none(),
            "serialized output must not contain the legacy `text` key"
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
    fn on_and_condition_parsed() {
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
        assert_eq!(config.on, Some("OuraSleepImported".to_string()));
        assert!(config.condition.is_some());
    }

    #[test]
    fn trigger_run_deserialize_prompt_without_knowhow() {
        // LLM often omits knowhow when calling update_trigger — must default to []
        let val = json!({"type": "prompt", "text": "do something"});
        let run: Result<TriggerRun, _> = serde_json::from_value(val);
        assert!(
            run.is_ok(),
            "TriggerRun should deserialize without knowhow field"
        );
        if let TriggerRun::Intent { intent, knowhow } = run.unwrap() {
            assert_eq!(intent, "do something");
            assert!(knowhow.is_empty());
        } else {
            panic!("Expected Intent variant");
        }
    }

    #[test]
    fn trigger_run_deserialize_intent_without_knowhow() {
        let val = json!({"type": "intent", "text": "do something"});
        let run: Result<TriggerRun, _> = serde_json::from_value(val);
        assert!(
            run.is_ok(),
            "TriggerRun should deserialize without knowhow field"
        );
    }

    #[test]
    fn apply_update_run_without_knowhow() {
        let payload = json!({
            "trigger_id": "test",
            "name": "Test",
            "schedule": ["0 0 8 * * *"],
            "timezone": "UTC",
            "run": { "type": "prompt", "text": "old prompt", "knowhow": [] }
        });
        let mut config = TriggerConfig::from_created_payload(&payload).unwrap();

        // LLM sends run without knowhow field — update must still apply
        config.apply_update(&json!({
            "trigger_id": "test",
            "run": { "type": "prompt", "text": "new prompt" }
        }));

        if let TriggerRun::Intent { intent, knowhow } = &config.run {
            assert_eq!(intent, "new prompt");
            assert!(knowhow.is_empty());
        } else {
            panic!("Expected Intent variant");
        }
    }

    #[test]
    fn null_on_and_condition_are_none() {
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
        assert!(config.on.is_none());
        assert!(config.condition.is_none());
    }

    #[test]
    fn apply_update_clear_schedule() {
        let payload = json!({
            "trigger_id": "test",
            "name": "Hybrid Trigger",
            "schedule": ["0 0 8 * * *"],
            "timezone": "UTC",
            "run": { "type": "prompt", "text": "test", "knowhow": [] },
            "on": "SomeEvent"
        });
        let mut config = TriggerConfig::from_created_payload(&payload).unwrap();
        assert_eq!(config.schedule, vec!["0 0 8 * * *"]);

        // Clear schedule by setting to empty array (from cron: null in LLM tool)
        config.apply_update(&json!({ "trigger_id": "test", "schedule": [] }));
        assert!(config.schedule.is_empty());
        assert_eq!(config.on, Some("SomeEvent".to_string())); // unchanged
    }

    #[test]
    fn apply_update_clear_on_event() {
        let payload = json!({
            "trigger_id": "test",
            "name": "Hybrid Trigger",
            "schedule": ["0 0 8 * * *"],
            "timezone": "UTC",
            "run": { "type": "prompt", "text": "test", "knowhow": [] },
            "on": "SomeEvent",
            "condition": { "score": { "$lt": 70 } }
        });
        let mut config = TriggerConfig::from_created_payload(&payload).unwrap();
        assert_eq!(config.on, Some("SomeEvent".to_string()));
        assert!(config.condition.is_some());

        // Clear on_event by setting to null
        config.apply_update(&json!({ "trigger_id": "test", "on": null }));
        assert!(config.on.is_none());
        assert!(config.condition.is_some()); // unchanged
    }

    #[test]
    fn apply_update_clear_condition() {
        let payload = json!({
            "trigger_id": "test",
            "name": "Test",
            "schedule": [],
            "timezone": "UTC",
            "run": { "type": "prompt", "text": "test", "knowhow": [] },
            "on": "SomeEvent",
            "condition": { "score": { "$lt": 70 } }
        });
        let mut config = TriggerConfig::from_created_payload(&payload).unwrap();
        assert!(config.condition.is_some());

        config.apply_update(&json!({ "trigger_id": "test", "condition": null }));
        assert!(config.condition.is_none());
        assert_eq!(config.on, Some("SomeEvent".to_string())); // unchanged
    }

    #[test]
    fn apply_update_absent_fields_unchanged() {
        let payload = json!({
            "trigger_id": "test",
            "name": "Original",
            "schedule": ["0 0 8 * * *"],
            "timezone": "Europe/Oslo",
            "run": { "type": "prompt", "text": "original prompt", "knowhow": ["domain"] },
            "on": "SomeEvent",
            "condition": { "key": "val" }
        });
        let mut config = TriggerConfig::from_created_payload(&payload).unwrap();

        // Update only name — all other fields must remain unchanged
        config.apply_update(&json!({ "trigger_id": "test", "name": "Renamed" }));
        assert_eq!(config.name, "Renamed");
        assert_eq!(config.schedule, vec!["0 0 8 * * *"]);
        assert_eq!(config.timezone, "Europe/Oslo");
        assert_eq!(config.on, Some("SomeEvent".to_string()));
        assert!(config.condition.is_some());
        if let TriggerRun::Intent { intent, knowhow } = &config.run {
            assert_eq!(intent, "original prompt");
            assert_eq!(knowhow, &vec!["domain".to_string()]);
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
            "run": { "type": "intent", "text": "new prompt", "knowhow": ["kh1"] }
        }));
        if let TriggerRun::Intent { intent, knowhow } = &config.run {
            assert_eq!(intent, "new prompt");
            assert_eq!(knowhow, &vec!["kh1".to_string()]);
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
}
