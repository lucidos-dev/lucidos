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

fn sub(name: &str) -> EventSubscription {
    EventSubscription {
        event_type: name.to_string(),
        condition: None,
    }
}

#[test]
fn trigger_description_schedule_only() {
    let desc = trigger_description("0 0 8 * * *", &[]);
    assert_eq!(desc, "schedule '0 0 8 * * *'");
}

#[test]
fn trigger_description_event_only() {
    let desc = trigger_description("", &[sub("OuraSleepImported")]);
    assert_eq!(desc, "event 'OuraSleepImported'");
}

#[test]
fn trigger_description_hybrid() {
    let desc = trigger_description("0 0 8 * * *", &[sub("OuraSleepImported")]);
    assert_eq!(desc, "schedule '0 0 8 * * *' AND event 'OuraSleepImported'");
}

#[test]
fn trigger_description_multi_event_lists_all() {
    // The create-confirmation message must surface every subscribed event
    // so the LLM can read it back to the user.
    let desc = trigger_description(
        "",
        &[sub("OuraSleepImported"), sub("EmailReceived")],
    );
    assert_eq!(desc, "event 'OuraSleepImported, EmailReceived'");
}

// -- parse_on_arg tests --

#[test]
fn parse_on_arg_absent_or_null() {
    assert!(parse_on_arg(None).unwrap().is_empty());
    assert!(parse_on_arg(Some(&json!(null))).unwrap().is_empty());
}

#[test]
fn parse_on_arg_single_string() {
    let subs = parse_on_arg(Some(&json!("OuraSleepImported"))).unwrap();
    assert_eq!(subs.len(), 1);
    assert_eq!(subs[0].event_type, "OuraSleepImported");
    assert!(subs[0].condition.is_none());
}

#[test]
fn parse_on_arg_array_of_strings() {
    let subs = parse_on_arg(Some(&json!(["A", "B", "C"]))).unwrap();
    assert_eq!(subs.len(), 3);
    assert_eq!(subs[1].event_type, "B");
}

#[test]
fn parse_on_arg_array_of_objects_with_conditions() {
    let subs = parse_on_arg(Some(&json!([
        { "event_type": "X", "condition": { "k": 1 } },
        { "event_type": "Y" }
    ])))
    .unwrap();
    assert_eq!(subs.len(), 2);
    assert!(subs[0].condition.is_some());
    assert!(subs[1].condition.is_none());
}

#[test]
fn parse_on_arg_single_object() {
    let subs = parse_on_arg(Some(&json!({
        "event_type": "Z",
        "condition": { "k": 2 }
    })))
    .unwrap();
    assert_eq!(subs.len(), 1);
    assert_eq!(subs[0].event_type, "Z");
    assert!(subs[0].condition.is_some());
}

#[test]
fn parse_on_arg_rejects_object_without_event_type() {
    let err = parse_on_arg(Some(&json!({ "condition": {} }))).unwrap_err();
    assert!(err.contains("event_type"));
}

#[test]
fn parse_on_arg_rejects_array_entry_with_wrong_shape() {
    let err = parse_on_arg(Some(&json!([42]))).unwrap_err();
    assert!(err.contains("entry"));
}

#[test]
fn parse_on_arg_drops_blank_strings() {
    let subs = parse_on_arg(Some(&json!(["", "  ", "Good"]))).unwrap();
    assert_eq!(subs.len(), 1);
    assert_eq!(subs[0].event_type, "Good");
}

// -- Layer 2: hard tool-layer guards during trigger fires --
//
// The guard is a pure function: tests pass `active_trigger_id` directly.
// The dispatcher reads the `ACTIVE_TRIGGER_ID` task-local once and feeds
// it in — that wiring is exercised by the call site in
// `execute_scheduler_tool`.

#[test]
fn guard_allows_create_trigger_outside_fire() {
    // No active trigger id — normal user chat. All scheduling calls must
    // pass through unchanged. This is the path the user takes when they
    // ask the LLM "create a trigger that ..." in a regular message.
    let result = check_scheduling_tool_in_trigger(tn::CREATE_TRIGGER, None, None, None);
    assert!(result.is_none());
}

#[test]
fn guard_blocks_create_trigger_during_fire() {
    // The infinite-loop bug: a fired trigger asks the LLM to do X, the LLM
    // mistakenly responds by creating a near-identical trigger to do X
    // again. The guard must reject create_trigger unconditionally.
    let result = check_scheduling_tool_in_trigger(
        tn::CREATE_TRIGGER,
        None,
        Some("firing-id"),
        Some("Daily news"),
    );
    let err = result.expect("create_trigger inside fire must be rejected");
    assert!(err.to_lowercase().contains("disabled"));
    assert!(err.contains("Daily news"));
    assert!(err.contains("firing-id"));
}

#[test]
fn guard_blocks_update_trigger_during_fire() {
    // Update is the same shape of risk as create — letting the LLM rewrite
    // its own schedule mid-fire could shift the next-fire time, expand
    // condition matchers, or rewrite the intent. Reject all updates.
    let result = check_scheduling_tool_in_trigger(
        tn::UPDATE_TRIGGER,
        Some("firing-id"),
        Some("firing-id"),
        Some("Daily news"),
    );
    assert!(
        result.is_some(),
        "update_trigger inside fire must be rejected even on self id"
    );
}

#[test]
fn guard_allows_self_delete_during_fire() {
    // The trigger's intent text may legitimately say "stop firing after
    // this" — the LLM's only correct call is delete_trigger on its own id.
    // Already covered by the existing `is_self_deleting_trigger` plumbing
    // for cancellation; this guard must let that call through.
    let result = check_scheduling_tool_in_trigger(
        tn::DELETE_TRIGGER,
        Some("self-id"),
        Some("self-id"),
        Some("Once"),
    );
    assert!(result.is_none(), "self-delete must be allowed");
}

#[test]
fn guard_blocks_cross_delete_during_fire() {
    // A fired trigger trying to delete some OTHER trigger is not a pattern
    // we want to support — the user did not consent to that scheduling
    // change at fire time. Block it.
    let result = check_scheduling_tool_in_trigger(
        tn::DELETE_TRIGGER,
        Some("other-id"),
        Some("self-id"),
        Some("Daily news"),
    );
    let err = result.expect("cross-trigger delete must be rejected");
    assert!(err.contains("self-id"));
}

#[test]
fn guard_allows_self_pause_during_fire() {
    // Symmetric to self-delete: pausing oneself is the polite version of
    // self-delete and must be allowed.
    let result = check_scheduling_tool_in_trigger(
        tn::PAUSE_TRIGGER,
        Some("self-id"),
        Some("self-id"),
        None,
    );
    assert!(result.is_none());
}

#[test]
fn guard_blocks_cross_pause_during_fire() {
    // Pausing a different trigger from inside this one's fire is not user
    // intent — block it.
    let result = check_scheduling_tool_in_trigger(
        tn::PAUSE_TRIGGER,
        Some("other-id"),
        Some("self-id"),
        None,
    );
    assert!(result.is_some());
}

#[test]
fn guard_blocks_resume_other_during_fire() {
    // Resume of a different trigger is the same risk as cross-pause.
    let result = check_scheduling_tool_in_trigger(
        tn::RESUME_TRIGGER,
        Some("other-id"),
        Some("self-id"),
        None,
    );
    assert!(result.is_some());
}

#[test]
fn guard_passes_unrelated_tool_names_through() {
    // The guard only cares about the five trigger-mutating tools. Any other
    // tool name (including list_triggers, which is read-only) must always
    // be allowed. Defensive: the dispatcher should never call us for those,
    // but the guard returning None for them keeps it honest.
    let result =
        check_scheduling_tool_in_trigger(tn::LIST_TRIGGERS, None, Some("self-id"), None);
    assert!(result.is_none());
}
