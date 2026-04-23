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
        text: String,
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
    pub enabled: bool,
    pub last_run: Option<DateTime<Utc>>,
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

        Ok(TriggerConfig {
            id,
            name,
            schedule,
            timezone,
            run,
            on,
            condition,
            enabled: true,
            last_run: None,
        })
    }

    /// Compute the next scheduled run time (UTC) from cron expressions and timezone.
    /// Returns None if the trigger is disabled, has no cron expressions, or no future match exists.
    pub fn next_run(&self) -> Option<DateTime<Utc>> {
        if !self.enabled || self.schedule.is_empty() {
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
        if let Some(enabled) = payload["enabled"].as_bool() {
            self.enabled = enabled;
        }
    }
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
        assert!(config.enabled);
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
    fn trigger_run_serde_roundtrip() {
        let prompt = TriggerRun::Intent {
            text: "Do something".into(),
            knowhow: vec!["domain".into()],
        };
        let json = serde_json::to_value(&prompt).unwrap();
        assert_eq!(json["type"], "intent");
        assert_eq!(json["text"], "Do something");

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
        if let TriggerRun::Intent { text, knowhow } = run.unwrap() {
            assert_eq!(text, "do something");
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

        if let TriggerRun::Intent { text, knowhow } = &config.run {
            assert_eq!(text, "new prompt");
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
        if let TriggerRun::Intent { text, knowhow } = &config.run {
            assert_eq!(text, "original prompt");
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
        if let TriggerRun::Intent { text, knowhow } = &config.run {
            assert_eq!(text, "new prompt");
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
}
