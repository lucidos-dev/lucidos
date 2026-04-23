use super::super::CognosEngine;
use crate::engine::event_bus::{BusEvent, SystemEvent};
use crate::triggers::TriggerRun;
use std::str::FromStr;

impl CognosEngine {
    pub(crate) async fn execute_scheduler_tool(
        &self,
        name: &str,
        args: &serde_json::Value,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        match name {
            "create_trigger" => {
                let name = args["name"].as_str().unwrap_or("");
                let on_event = args
                    .get("on_event")
                    .and_then(|v| v.as_str())
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty());
                let condition = args.get("condition").filter(|v| !v.is_null()).cloned();

                // Parse cron: accepts a single string or an array of strings (optional if on_event is set)
                let cron_expressions = if args.get("cron").is_some() && !args["cron"].is_null() {
                    match parse_cron_arg(&args["cron"]) {
                        Ok(exprs) => exprs,
                        Err(e) => return Ok(e),
                    }
                } else {
                    Vec::new()
                };

                if name.is_empty() {
                    return Ok("Error: name is required".to_string());
                }
                if cron_expressions.is_empty() && on_event.is_none() {
                    return Ok(
                        "Error: At least one of 'cron' or 'on_event' is required".to_string()
                    );
                }

                // Parse run field: { type: "intent", text: "...", knowhow: [...] } or { type: "script", path: "..." }
                let run: TriggerRun = match args.get("run") {
                    Some(run_val) if !run_val.is_null() => {
                        serde_json::from_value(run_val.clone())
                            .map_err(|e| format!("Invalid 'run' field: {}. Expected {{ type: 'intent', text: '...', knowhow: [] }} or {{ type: 'script', path: '...' }}", e))?
                    }
                    _ => {
                        // Backward compat: accept prompt_text as a shorthand
                        let prompt_text = args.get("prompt_text").and_then(|v| v.as_str()).unwrap_or("");
                        if prompt_text.is_empty() {
                            return Ok("Error: 'run' is required. Use { type: 'intent', text: '...', knowhow: [] } or { type: 'script', path: '...' }".to_string());
                        }
                        TriggerRun::Intent { text: prompt_text.to_string(), knowhow: vec![] }
                    }
                };

                // Check if timezone is set - required for triggers
                let tz_val = self.user_timezone.read().await.clone();
                if tz_val.is_empty() {
                    return Ok("Error: User timezone is not set. Use set_timezone tool first to set the user's timezone before creating triggers.".to_string());
                }

                // Emit via EventBus — persists to events table AND notifies scheduler instantly
                let trigger_id_str = uuid::Uuid::new_v4().to_string();
                let cron_display = cron_expressions.join(", ");
                let mut event_payload = serde_json::json!({
                    "trigger_id": trigger_id_str,
                    "name": name,
                    "schedule": cron_expressions,
                    "timezone": tz_val,
                    "run": serde_json::to_value(&run).unwrap(),
                });
                if let Some(ref ev) = on_event {
                    event_payload["on"] = serde_json::json!(ev);
                }
                if let Some(ref cond) = condition {
                    event_payload["condition"] = cond.clone();
                }

                self.event_bus
                    .emit(BusEvent::System(SystemEvent::TriggerCreated {
                        trigger_id: trigger_id_str.clone(),
                        payload: event_payload,
                        actor: None,
                    }))
                    .await?;

                let trigger_desc = trigger_description(&cron_display, on_event.as_deref());
                let run_desc = match &run {
                    TriggerRun::Intent { text, .. } => {
                        let end = text.floor_char_boundary(50);
                        format!("intent '{}'", &text[..end])
                    }
                    TriggerRun::Script { path } => format!("script '{}'", path),
                };

                Ok(format!(
                    "[ACTION COMPLETED] Created trigger '{}' (ID: {}) running {} with {} in timezone {}.",
                    name, trigger_id_str, run_desc, trigger_desc, tz_val
                ))
            }
            "list_triggers" => {
                let configs: Vec<crate::triggers::TriggerConfig> = {
                    let configs = self.trigger_configs.read().unwrap();
                    configs.values().cloned().collect()
                };

                if configs.is_empty() {
                    Ok("No triggers found.".to_string())
                } else {
                    let mut result = format!("Found {} trigger(s):\n\n", configs.len());
                    for config in &configs {
                        let status = if config.enabled {
                            "enabled"
                        } else {
                            "disabled"
                        };
                        let last_run = config
                            .last_run
                            .map(|t| t.format("%Y-%m-%d %H:%M").to_string())
                            .unwrap_or_else(|| "never".to_string());
                        let run_info = match &config.run {
                            TriggerRun::Intent { text, knowhow } => {
                                let kh = if knowhow.is_empty() {
                                    String::new()
                                } else {
                                    format!(" [knowhow: {}]", knowhow.join(", "))
                                };
                                let end = text.floor_char_boundary(60);
                                format!("intent: {}{}", &text[..end], kh)
                            }
                            TriggerRun::Script { path } => format!("script: {}", path),
                        };
                        let trigger_type = config.trigger_type_label();
                        let schedule_display = if !config.schedule.is_empty() {
                            format!(
                                "  Schedule: {} ({})\n",
                                config.schedule.join(", "),
                                config.timezone
                            )
                        } else {
                            String::new()
                        };
                        let event_display = if let Some(ref ev) = config.on {
                            let cond = config
                                .condition
                                .as_ref()
                                .map(|c| format!(" when {}", c))
                                .unwrap_or_default();
                            format!("  Event: on {}{}\n", ev, cond)
                        } else {
                            String::new()
                        };
                        result.push_str(&format!(
                            "- **{}** (ID: {}) [{}]\n{}{}  Status: {}\n  Last run: {}\n  Run: {}\n\n",
                            config.name, config.id, trigger_type, schedule_display, event_display, status, last_run, run_info
                        ));
                    }
                    Ok(result)
                }
            }
            "update_trigger" => {
                let trigger_id = match args["trigger_id"].as_str() {
                    Some(s) if !s.is_empty() => s.to_string(),
                    _ => return Ok("Error: trigger_id is required".to_string()),
                };

                let new_name = args.get("name").and_then(|v| v.as_str());
                let new_run: Option<TriggerRun> = match args.get("run").filter(|v| !v.is_null()) {
                    Some(v) => Some(serde_json::from_value(v.clone())
                        .map_err(|e| format!("Invalid 'run' field: {}. Expected {{ type: 'intent', text: '...', knowhow: [] }} or {{ type: 'script', path: '...' }}", e))?),
                    None => None,
                };
                // Parse on_event once — reused for payload and validation
                let new_on_event: Option<Option<String>> = if args.get("on_event").is_some() {
                    Some(
                        args["on_event"]
                            .as_str()
                            .map(|s| s.trim().to_string())
                            .filter(|s| !s.is_empty()),
                    )
                } else {
                    None
                };
                let new_condition: Option<Option<serde_json::Value>> =
                    if args.get("condition").is_some() {
                        Some(args.get("condition").filter(|v| !v.is_null()).cloned())
                    } else {
                        None
                    };
                // Parse cron: Option<Vec<String>>
                // None = not provided (keep existing), Some(vec![]) = clear, Some(vec![...]) = set
                let new_cron: Option<Vec<String>> = if args.get("cron").is_some() {
                    if args["cron"].is_null() {
                        Some(vec![])
                    } else {
                        Some(match parse_cron_arg(&args["cron"]) {
                            Ok(exprs) => exprs,
                            Err(e) => return Ok(e),
                        })
                    }
                } else {
                    None
                };

                if new_name.is_none()
                    && new_run.is_none()
                    && new_cron.is_none()
                    && new_on_event.is_none()
                    && new_condition.is_none()
                {
                    return Ok(
                        "Error: At least one field besides trigger_id must be provided".to_string(),
                    );
                }

                // Read existing config from in-memory state
                let existing = {
                    let configs = self.trigger_configs.read().unwrap();
                    configs.get(&trigger_id).cloned()
                };
                let existing = match existing {
                    Some(c) => c,
                    None => return Ok(format!("Error: No trigger found with ID {}", trigger_id)),
                };

                // Build update payload with only changed fields
                let mut update_payload = serde_json::json!({
                    "trigger_id": trigger_id,
                });
                let mut updated_fields = Vec::new();

                if let Some(n) = new_name {
                    update_payload["name"] = serde_json::json!(n);
                    updated_fields.push("name");
                }
                if let Some(crons) = &new_cron {
                    update_payload["schedule"] = serde_json::json!(crons);
                    updated_fields.push("schedule");
                }
                if let Some(ref run) = new_run {
                    update_payload["run"] = serde_json::to_value(run).unwrap();
                    updated_fields.push("run");
                }
                if let Some(ref on) = new_on_event {
                    update_payload["on"] = serde_json::json!(on);
                    updated_fields.push("on_event");
                }
                if let Some(ref cond) = new_condition {
                    update_payload["condition"] = serde_json::json!(cond);
                    updated_fields.push("condition");
                }

                // Ensure trigger still has at least one firing mechanism
                let updated_schedule = new_cron.as_ref().unwrap_or(&existing.schedule);
                let updated_on = new_on_event.clone().unwrap_or_else(|| existing.on.clone());
                if updated_schedule.is_empty() && updated_on.is_none() {
                    return Ok(
                        "Error: Trigger must have at least one cron schedule or event type"
                            .to_string(),
                    );
                }

                self.event_bus
                    .emit(BusEvent::System(SystemEvent::TriggerUpdated {
                        trigger_id: trigger_id.clone(),
                        payload: update_payload,
                        actor: None,
                    }))
                    .await?;

                let display_name = new_name.unwrap_or(&existing.name);
                let display_schedule = new_cron
                    .as_ref()
                    .map(|c| c.join(", "))
                    .unwrap_or_else(|| existing.schedule.join(", "));

                Ok(format!(
                    "[ACTION COMPLETED] Updated trigger '{}' (ID: {}). Changed: {}. Schedule: {} ({})",
                    display_name, trigger_id, updated_fields.join(", "), display_schedule, existing.timezone
                ))
            }
            "delete_trigger" => {
                let trigger_id = match args["trigger_id"].as_str() {
                    Some(s) if !s.is_empty() => s.to_string(),
                    _ => return Ok("Error: trigger_id is required".to_string()),
                };

                // Check if trigger exists in in-memory state
                let existing = {
                    let configs = self.trigger_configs.read().unwrap();
                    configs.get(&trigger_id).cloned()
                };
                let existing = match existing {
                    Some(c) => c,
                    None => return Ok(format!("Error: No trigger found with ID {}", trigger_id)),
                };

                let self_deleting =
                    crate::scheduler::user_tasks::is_self_deleting_trigger(&trigger_id);

                // Emit via EventBus
                self.event_bus
                    .emit(BusEvent::System(SystemEvent::TriggerDeleted {
                        trigger_id: trigger_id.clone(),
                        payload: serde_json::json!({
                            "trigger_id": trigger_id,
                            "name": existing.name,
                            "self_deleting": self_deleting,
                        }),
                        actor: None,
                    }))
                    .await?;

                Ok(format!(
                    "[ACTION COMPLETED] Deleted trigger '{}' (ID: {}).",
                    existing.name, trigger_id
                ))
            }
            _ => Ok(format!("Unknown scheduler tool: {}", name)),
        }
    }
}

/// Build human-readable trigger description for create responses.
fn trigger_description(cron_display: &str, on_event: Option<&str>) -> String {
    match (cron_display.is_empty(), on_event) {
        (false, Some(ev)) => format!("schedule '{}' AND event '{}'", cron_display, ev),
        (true, Some(ev)) => format!("event '{}'", ev),
        _ => format!("schedule '{}'", cron_display),
    }
}

/// Translate the day-of-week field from standard cron convention (0=Sun, 1=Mon, ..., 6=Sat, 7=Sun)
/// to the `cron` crate's 1-based convention (1=Sun, 2=Mon, ..., 7=Sat).
fn translate_dow_for_cron_crate(expr: &str) -> String {
    fn shift(s: &str) -> Option<String> {
        s.parse::<u8>()
            .ok()
            .filter(|&n| n <= 7)
            .map(|n| format!("{}", (n % 7) + 1))
    }

    let parts: Vec<&str> = expr.split_whitespace().collect();
    if parts.len() != 6 {
        return expr.to_string();
    }

    let dow = parts[5];

    if dow == "*" || dow.starts_with("*/") || dow.chars().any(|c| c.is_ascii_alphabetic()) {
        return expr.to_string();
    }

    let translated_dow = dow
        .split(',')
        .map(|segment| {
            if let Some((a, b)) = segment.split_once('-') {
                match (shift(a), shift(b)) {
                    (Some(a), Some(b)) => format!("{}-{}", a, b),
                    _ => segment.to_string(),
                }
            } else if let Some((base, step)) = segment.split_once('/') {
                match shift(base) {
                    Some(b) => format!("{}/{}", b, step),
                    None => segment.to_string(),
                }
            } else {
                shift(segment).unwrap_or_else(|| segment.to_string())
            }
        })
        .collect::<Vec<_>>()
        .join(",");

    format!(
        "{} {} {} {} {} {}",
        parts[0], parts[1], parts[2], parts[3], parts[4], translated_dow
    )
}

/// Parse a standard cron expression into a `cron::Schedule`, translating day-of-week
/// from standard convention (0=Sun, 6=Sat) to the `cron` crate's 1-based convention (1=Sun, 7=Sat).
pub(crate) fn parse_standard_cron(expr: &str) -> Result<cron::Schedule, String> {
    let translated = translate_dow_for_cron_crate(expr);
    cron::Schedule::from_str(&translated).map_err(|e| e.to_string())
}

/// Parse the `cron` tool argument, which can be either a single string or a JSON array of strings.
/// Returns a Vec of validated cron expression strings, or an error message.
pub(crate) fn parse_cron_arg(value: &serde_json::Value) -> Result<Vec<String>, String> {
    let expressions: Vec<String> = match value {
        serde_json::Value::String(s) => vec![s.clone()],
        serde_json::Value::Array(arr) => {
            if arr.is_empty() {
                return Err("Error: cron array must not be empty".to_string());
            }
            arr.iter()
                .map(|v| {
                    v.as_str()
                        .map(|s| s.to_string())
                        .ok_or_else(|| "Error: each cron expression must be a string".to_string())
                })
                .collect::<Result<Vec<_>, _>>()?
        }
        _ => return Err("Error: cron must be a string or array of strings".to_string()),
    };

    // Validate each expression
    for expr in &expressions {
        let parts: Vec<&str> = expr.split_whitespace().collect();
        if parts.len() != 6 {
            return Err(format!(
                "Error: Invalid cron expression '{}'. Must have 6 fields: second minute hour day-of-month month day-of-week. Example: '0 0 8 * * *' for 8am daily.",
                expr
            ));
        }
        if parse_standard_cron(expr).is_err() {
            return Err(format!(
                "Error: Invalid cron expression '{}'. Check syntax.",
                expr
            ));
        }
    }

    Ok(expressions)
}

/// Find the nearest next occurrence across multiple cron schedules.
/// Returns the earliest upcoming time from any of the schedules.
pub(crate) fn next_occurrence_multi(
    schedules: &[cron::Schedule],
    tz: chrono_tz::Tz,
) -> Option<chrono::DateTime<chrono_tz::Tz>> {
    schedules.iter().filter_map(|s| s.upcoming(tz).next()).min()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // -- parse_cron_arg tests --

    #[test]
    fn parse_cron_arg_single_string() {
        let val = json!("0 0 8 * * *");
        let result = parse_cron_arg(&val).unwrap();
        assert_eq!(result, vec!["0 0 8 * * *"]);
    }

    #[test]
    fn parse_cron_arg_array_of_strings() {
        let val = json!(["0 0 8 * * *", "0 0 20 * * *"]);
        let result = parse_cron_arg(&val).unwrap();
        assert_eq!(result, vec!["0 0 8 * * *", "0 0 20 * * *"]);
    }

    #[test]
    fn parse_cron_arg_single_element_array() {
        let val = json!(["0 30 9 * * 1-5"]);
        let result = parse_cron_arg(&val).unwrap();
        assert_eq!(result, vec!["0 30 9 * * 1-5"]);
    }

    #[test]
    fn parse_cron_arg_rejects_empty_array() {
        let val = json!([]);
        let result = parse_cron_arg(&val);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("must not be empty"));
    }

    #[test]
    fn parse_cron_arg_rejects_non_string_in_array() {
        let val = json!(["0 0 8 * * *", 42]);
        let result = parse_cron_arg(&val);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("must be a string"));
    }

    #[test]
    fn parse_cron_arg_rejects_number() {
        let val = json!(42);
        let result = parse_cron_arg(&val);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("must be a string or array"));
    }

    #[test]
    fn parse_cron_arg_rejects_null() {
        let val = json!(null);
        let result = parse_cron_arg(&val);
        assert!(result.is_err());
    }

    #[test]
    fn parse_cron_arg_validates_field_count() {
        let val = json!("0 0 8 * *"); // 5 fields instead of 6
        let result = parse_cron_arg(&val);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Must have 6 fields"));
    }

    #[test]
    fn parse_cron_arg_validates_syntax() {
        let val = json!("0 0 25 * * *"); // hour 25 is invalid
        let result = parse_cron_arg(&val);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Check syntax"));
    }

    #[test]
    fn parse_cron_arg_validates_all_expressions_in_array() {
        // First is valid, second has wrong field count
        let val = json!(["0 0 8 * * *", "0 0 8 * *"]);
        let result = parse_cron_arg(&val);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Must have 6 fields"));
    }

    // -- next_occurrence_multi tests --

    #[test]
    fn next_occurrence_multi_picks_earliest() {
        use chrono::Timelike;
        use std::str::FromStr;

        // 8am daily and 6am daily — 6am should be next (or same day if before both)
        let s1 = cron::Schedule::from_str("0 0 8 * * *").unwrap();
        let s2 = cron::Schedule::from_str("0 0 6 * * *").unwrap();

        let tz: chrono_tz::Tz = "UTC".parse().unwrap();
        let next = next_occurrence_multi(&[s1, s2], tz);
        assert!(next.is_some());

        // The earliest should be the 6am one (or same day if before both)
        let next_time = next.unwrap();
        assert!(next_time.hour() == 6 || next_time.hour() == 8);
        // Verify it's truly the minimum
        let s1_next = cron::Schedule::from_str("0 0 8 * * *")
            .unwrap()
            .upcoming(tz)
            .next()
            .unwrap();
        let s2_next = cron::Schedule::from_str("0 0 6 * * *")
            .unwrap()
            .upcoming(tz)
            .next()
            .unwrap();
        assert_eq!(next_time, s1_next.min(s2_next));
    }

    #[test]
    fn next_occurrence_multi_single_schedule() {
        use std::str::FromStr;

        let s = cron::Schedule::from_str("0 0 12 * * *").unwrap();
        let tz: chrono_tz::Tz = "UTC".parse().unwrap();
        let next = next_occurrence_multi(std::slice::from_ref(&s), tz);
        assert_eq!(next, s.upcoming(tz).next());
    }

    #[test]
    fn next_occurrence_multi_empty_returns_none() {
        let tz: chrono_tz::Tz = "UTC".parse().unwrap();
        let next = next_occurrence_multi(&[], tz);
        assert!(next.is_none());
    }

    // -- day-of-week translation tests --

    #[test]
    fn cron_crate_dow_5_is_friday_after_translation() {
        use chrono::{Datelike, TimeZone};

        // Standard cron: 5 = Friday. After translation, dow=5 should schedule on Friday.
        let translated = translate_dow_for_cron_crate("0 0 12 * * 5");
        let schedule = cron::Schedule::from_str(&translated).unwrap();

        // Start from a known Monday (April 13, 2026) to avoid ambiguity
        let monday = chrono_tz::UTC
            .with_ymd_and_hms(2026, 4, 13, 0, 0, 0)
            .unwrap();
        let next = schedule.after(&monday).next().unwrap();
        assert_eq!(
            next.weekday(),
            chrono::Weekday::Fri,
            "dow=5 should map to Friday (standard cron convention)"
        );
    }

    #[test]
    fn cron_crate_dow_0_is_sunday_after_translation() {
        use chrono::{Datelike, TimeZone};

        let translated = translate_dow_for_cron_crate("0 0 12 * * 0");
        let schedule = cron::Schedule::from_str(&translated).unwrap();

        let monday = chrono_tz::UTC
            .with_ymd_and_hms(2026, 4, 13, 0, 0, 0)
            .unwrap();
        let next = schedule.after(&monday).next().unwrap();
        assert_eq!(next.weekday(), chrono::Weekday::Sun);
    }

    #[test]
    fn cron_crate_dow_range_1_5_is_weekdays_after_translation() {
        use chrono::{Datelike, TimeZone};

        let translated = translate_dow_for_cron_crate("0 0 12 * * 1-5");
        let schedule = cron::Schedule::from_str(&translated).unwrap();

        // Start from Saturday April 11, 2026
        let saturday = chrono_tz::UTC
            .with_ymd_and_hms(2026, 4, 11, 13, 0, 0)
            .unwrap();
        let next = schedule.after(&saturday).next().unwrap();
        assert_eq!(
            next.weekday(),
            chrono::Weekday::Mon,
            "1-5 range should map to Mon-Fri"
        );
    }

    #[test]
    fn cron_crate_dow_comma_list_after_translation() {
        use chrono::{Datelike, TimeZone};

        // Standard cron: 0,6 = Sunday,Saturday
        let translated = translate_dow_for_cron_crate("0 0 12 * * 0,6");
        let schedule = cron::Schedule::from_str(&translated).unwrap();

        // Start from Monday April 13, 2026
        let monday = chrono_tz::UTC
            .with_ymd_and_hms(2026, 4, 13, 0, 0, 0)
            .unwrap();
        let next = schedule.after(&monday).next().unwrap();
        assert!(
            next.weekday() == chrono::Weekday::Sat || next.weekday() == chrono::Weekday::Sun,
            "0,6 should map to weekend days, got {:?}",
            next.weekday()
        );
    }

    #[test]
    fn translate_dow_wildcard_unchanged() {
        assert_eq!(translate_dow_for_cron_crate("0 0 12 * * *"), "0 0 12 * * *");
    }

    #[test]
    fn translate_dow_named_days_unchanged() {
        assert_eq!(
            translate_dow_for_cron_crate("0 0 12 * * MON-FRI"),
            "0 0 12 * * MON-FRI"
        );
        assert_eq!(
            translate_dow_for_cron_crate("0 0 12 * * SAT,SUN"),
            "0 0 12 * * SAT,SUN"
        );
    }

    #[test]
    fn translate_dow_7_wraps_to_sunday() {
        // Standard cron: 7 is alias for Sunday (same as 0)
        let translated = translate_dow_for_cron_crate("0 0 12 * * 7");
        // Should become 1 (Sunday in cron crate)
        assert_eq!(translated, "0 0 12 * * 1");
    }

    #[test]
    fn translate_dow_out_of_range_passes_through() {
        // Out-of-range values should pass through untranslated for the cron parser to reject
        let translated = translate_dow_for_cron_crate("0 0 12 * * 8");
        assert_eq!(translated, "0 0 12 * * 8");
        assert!(parse_standard_cron("0 0 12 * * 8").is_err());

        let translated = translate_dow_for_cron_crate("0 0 12 * * 999");
        assert_eq!(translated, "0 0 12 * * 999");
        assert!(parse_standard_cron("0 0 12 * * 999").is_err());
    }

    // -- trigger helpers tests --

    #[test]
    fn trigger_description_schedule_only() {
        let desc = trigger_description("0 0 8 * * *", None);
        assert_eq!(desc, "schedule '0 0 8 * * *'");
    }

    #[test]
    fn trigger_description_event_only() {
        let desc = trigger_description("", Some("OuraSleepImported"));
        assert_eq!(desc, "event 'OuraSleepImported'");
    }

    #[test]
    fn trigger_description_hybrid() {
        let desc = trigger_description("0 0 8 * * *", Some("OuraSleepImported"));
        assert_eq!(desc, "schedule '0 0 8 * * *' AND event 'OuraSleepImported'");
    }
}
