use super::*;
use serde_json::json;

/// Every cron guard case in this file is timezone-independent (Feb 31 does not
/// exist anywhere), so the tests validate in UTC.
const UTC: chrono_tz::Tz = chrono_tz::UTC;

// -- parse_cron_arg tests --

#[test]
fn parse_cron_arg_single_string() {
    let val = json!("0 0 8 * * *");
    let result = parse_cron_arg(&val, UTC).unwrap();
    assert_eq!(result.expressions, vec!["0 0 8 * * *"]);
}

#[test]
fn parse_cron_arg_array_of_strings() {
    let val = json!(["0 0 8 * * *", "0 0 20 * * *"]);
    let result = parse_cron_arg(&val, UTC).unwrap();
    assert_eq!(result.expressions, vec!["0 0 8 * * *", "0 0 20 * * *"]);
}

#[test]
fn parse_cron_arg_single_element_array() {
    let val = json!(["0 30 9 * * 1-5"]);
    let result = parse_cron_arg(&val, UTC).unwrap();
    assert_eq!(result.expressions, vec!["0 30 9 * * 1-5"]);
}

#[test]
fn parse_cron_arg_rejects_empty_array() {
    let val = json!([]);
    let result = parse_cron_arg(&val, UTC);
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("must not be empty"));
}

#[test]
fn parse_cron_arg_rejects_non_string_in_array() {
    let val = json!(["0 0 8 * * *", 42]);
    let result = parse_cron_arg(&val, UTC);
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("must be a string"));
}

#[test]
fn parse_cron_arg_rejects_number() {
    let val = json!(42);
    let result = parse_cron_arg(&val, UTC);
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("must be a string or array"));
}

#[test]
fn parse_cron_arg_rejects_null() {
    let val = json!(null);
    let result = parse_cron_arg(&val, UTC);
    assert!(result.is_err());
}

#[test]
fn parse_cron_arg_validates_field_count() {
    let val = json!("0 0 8 * *"); // 5 fields instead of 6
    let result = parse_cron_arg(&val, UTC);
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("Must have 6 fields"));
}

#[test]
fn parse_cron_arg_validates_syntax() {
    let val = json!("0 0 25 * * *"); // hour 25 is invalid
    let result = parse_cron_arg(&val, UTC);
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("Check syntax"));
}

#[test]
fn parse_cron_arg_validates_all_expressions_in_array() {
    // First is valid, second has wrong field count
    let val = json!(["0 0 8 * * *", "0 0 8 * *"]);
    let result = parse_cron_arg(&val, UTC);
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("Must have 6 fields"));
}

#[test]
fn parse_cron_arg_prefixes_validation_errors_for_the_llm_surface() {
    // The HTTP layer surfaces `validate_cron_expressions`' message verbatim in a
    // toast, so the bare form has no prefix; the LLM tool surface adds one.
    let bare = validate_cron_expressions(vec!["0 0 9 31 2 *".to_string()], UTC).unwrap_err();
    assert!(!bare.starts_with("Error:"), "got: {bare}");
    let tool = parse_cron_arg(&json!("0 0 9 31 2 *"), UTC).unwrap_err();
    assert!(tool.starts_with("Error:"), "got: {tool}");
    assert!(
        tool.contains(&bare),
        "prefix must wrap the same message: {tool}"
    );
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

// -- day-of-week crossing-range tests: what the `cron` crate does with 5-0 --
//
// A numeric range like `5-0` (Friday through Sunday) or `6-1` (Saturday
// through Monday) crosses Sunday. `translate_dow_for_cron_crate` shifts each
// endpoint alone, so the crossing survives translation with start > end
// (`5-0` becomes `6-1`, `6-1` becomes `7-2`). These tests establish, rather
// than assume, what `cron` 0.15 actually does. It rejects the range with a
// parse error, on both the numeric and the named form.

#[test]
fn cron_crate_numeric_dow_range_5_0_crossing_sunday_is_rejected() {
    let translated = translate_dow_for_cron_crate("0 0 12 * * 5-0");
    assert_eq!(translated, "0 0 12 * * 6-1");
    let err = parse_standard_cron("0 0 12 * * 5-0")
        .expect_err("cron 0.15 rejects a day-of-week range whose start exceeds its end");
    assert!(err.contains("Invalid range"), "got: {err}");
    assert!(err.contains("6-1"), "got: {err}");
}

#[test]
fn cron_crate_numeric_dow_range_6_1_crossing_sunday_is_rejected() {
    let translated = translate_dow_for_cron_crate("0 0 12 * * 6-1");
    assert_eq!(translated, "0 0 12 * * 7-2");
    let err = parse_standard_cron("0 0 12 * * 6-1")
        .expect_err("cron 0.15 rejects a day-of-week range whose start exceeds its end");
    assert!(err.contains("Invalid range"), "got: {err}");
    assert!(err.contains("7-2"), "got: {err}");
}

#[test]
fn cron_crate_named_dow_range_fri_sun_crossing_sunday_is_also_rejected() {
    // Named days bypass translation entirely. The crate numbers them
    // Sunday-first too (Sun=1 ... Sat=7), so Fri-Sun hits the same
    // start <= end check as the numeric form above.
    assert_eq!(
        translate_dow_for_cron_crate("0 0 12 * * Fri-Sun"),
        "0 0 12 * * Fri-Sun",
        "named ranges must not be touched by translation"
    );
    let err = parse_standard_cron("0 0 12 * * Fri-Sun")
        .expect_err("the named form hits the same start <= end check as the numeric form");
    assert!(err.contains("Invalid named range"), "got: {err}");
}

// -- day-of-week range-with-step: a crate quirk, blocked before it can fire --
//
// Found by /harden's Codex review pass on this same branch, not by the
// crossing-range investigation above. `translate_dow_for_cron_crate` only
// shifts a plain number or a plain `a-b` range. A combined `a-b/step` token
// is misread: `split_once('-')` treats `5/2` as the range end, `shift` can't
// parse it as a number, and the whole segment passes through untranslated.
//
// This shape never reaches a real trigger: `validate_cron_expressions`
// rejects it first (see the test below). The test here calls
// `translate_dow_for_cron_crate` and `parse_standard_cron` directly,
// bypassing that validation layer, to document what the `cron` crate itself
// does with the token. It is a characterisation test of the crate, not a
// path a Lucidos user can reach.

#[test]
fn cron_crate_dow_range_with_step_is_silently_untranslated() {
    use chrono::{Datelike, TimeZone};

    // Standard cron: 1-5/2 should mean "Mon, Wed, Fri" (every other weekday).
    // It passes through unshifted, so the crate parses it under its OWN
    // Sunday-first numbering instead: 1=Sun..5=Thu, stepped by 2 = Sun,Tue,Thu.
    let translated = translate_dow_for_cron_crate("0 0 12 * * 1-5/2");
    assert_eq!(
        translated, "0 0 12 * * 1-5/2",
        "the gap: this token is left exactly as written, not shifted"
    );

    let schedule = parse_standard_cron("0 0 12 * * 1-5/2")
        .expect("this token parses without error: the bug is silent, not a rejection");
    let monday = chrono_tz::UTC
        .with_ymd_and_hms(2026, 4, 13, 0, 0, 0)
        .unwrap();
    // A set, not a sequence: the crate returns fires in calendar order, not
    // shift order. A plain Vec comparison would depend on which weekday
    // `monday` falls on.
    let fires: std::collections::HashSet<chrono::Weekday> = schedule
        .after(&monday)
        .take(3)
        .map(|t| t.weekday())
        .collect();
    assert_eq!(
        fires,
        [
            chrono::Weekday::Sun,
            chrono::Weekday::Tue,
            chrono::Weekday::Thu
        ]
        .into(),
        "got the crate's own Sunday-first reading, not the Mon/Wed/Fri the user wrote: {fires:?}"
    );
}

#[test]
fn validate_cron_expressions_rejects_numeric_dow_range_with_step() {
    let err = validate_cron_expressions(vec!["0 0 12 * * 1-5/2".to_string()], UTC)
        .expect_err("a numeric range-with-step day-of-week token must be rejected");
    assert!(
        err.contains("1-5/2"),
        "must name the offending token: {err}"
    );
    assert!(
        err.contains("Mon-Fri/2"),
        "must name the named form as a safe alternative: {err}"
    );
}

#[test]
fn validate_cron_expressions_rejects_the_numeric_token_even_mixed_with_a_named_day() {
    // The range-with-step token can't be safely shifted (see
    // numeric_dow_range_with_step_token below), even now that
    // translate_dow_for_cron_crate shifts other segments in a mixed field.
    // A mixed field carrying it must still be caught: the check inspects
    // each comma segment on its own, matching the granularity translate uses.
    let err = validate_cron_expressions(vec!["0 0 12 * * Mon,1-5/2".to_string()], UTC)
        .expect_err("the numeric segment must still be caught inside a mixed list");
    assert!(
        err.contains("1-5/2"),
        "must name the offending token: {err}"
    );
}

#[test]
fn named_dow_range_with_step_still_fires_mon_wed_fri() {
    use chrono::{Datelike, TimeZone};

    // Mon-Fri/2 bypasses translation entirely (names are untouched). The
    // crate numbers names Sunday-first, same as digits, so this lands on
    // Mon,Wed,Fri exactly as written. A later tightening of the numeric
    // check must not catch this working form too.
    validate_cron_expressions(vec!["0 0 12 * * Mon-Fri/2".to_string()], UTC)
        .expect("the named form is a working expression and must be accepted");

    let schedule = parse_standard_cron("0 0 12 * * Mon-Fri/2").unwrap();
    let monday = chrono_tz::UTC
        .with_ymd_and_hms(2026, 4, 13, 0, 0, 0)
        .unwrap();
    let fires: std::collections::HashSet<chrono::Weekday> = schedule
        .after(&monday)
        .take(3)
        .map(|t| t.weekday())
        .collect();
    assert_eq!(
        fires,
        [
            chrono::Weekday::Mon,
            chrono::Weekday::Wed,
            chrono::Weekday::Fri
        ]
        .into(),
        "named range-with-step must fire on the days written, not a Sunday-first misread: {fires:?}"
    );
}

// -- mixed named/numeric day-of-week: a sibling of the range-with-step bug --
//
// `translate_dow_for_cron_crate` used to bail on the WHOLE field when any
// character was alphabetic. That is correct for a purely named field, but
// wrong for a mixed one: the numeric segment shares the field with a name
// and needs the shift just as much as it would alone. Checked per
// comma-segment now, so a letter in one segment skips only that segment.

#[test]
fn mixed_dow_named_and_numeric_shifts_only_the_numeric_segment() {
    use chrono::{Datelike, TimeZone};

    // Standard numbering: 1 = Monday, same as "Mon". The field is redundant
    // and means Monday only.
    validate_cron_expressions(vec!["0 0 12 * * Mon,1".to_string()], UTC)
        .expect("a mixed named/numeric day-of-week field is a working expression");

    let schedule = parse_standard_cron("0 0 12 * * Mon,1").unwrap();
    let monday = chrono_tz::UTC
        .with_ymd_and_hms(2026, 4, 13, 0, 0, 0)
        .unwrap();
    let fires: std::collections::HashSet<chrono::Weekday> = schedule
        .after(&monday)
        .take(2)
        .map(|t| t.weekday())
        .collect();
    assert_eq!(
        fires,
        [chrono::Weekday::Mon].into(),
        "Mon,1 both name Monday under standard numbering: {fires:?}"
    );
}

#[test]
fn mixed_dow_named_and_numeric_range_shifts_only_the_numeric_segment() {
    use chrono::{Datelike, TimeZone};

    // Standard numbering: 1-2 = Mon-Tue, so the field means Sat, Mon, Tue.
    validate_cron_expressions(vec!["0 0 12 * * Sat,1-2".to_string()], UTC)
        .expect("a mixed named/numeric range day-of-week field is a working expression");

    let schedule = parse_standard_cron("0 0 12 * * Sat,1-2").unwrap();
    let monday = chrono_tz::UTC
        .with_ymd_and_hms(2026, 4, 13, 0, 0, 0)
        .unwrap();
    let fires: std::collections::HashSet<chrono::Weekday> = schedule
        .after(&monday)
        .take(3)
        .map(|t| t.weekday())
        .collect();
    assert_eq!(
        fires,
        [
            chrono::Weekday::Sat,
            chrono::Weekday::Mon,
            chrono::Weekday::Tue
        ]
        .into(),
        "Sat,1-2 must fire Sat, Mon, Tue under standard numbering: {fires:?}"
    );
}

#[test]
fn mixed_dow_named_and_numeric_step_shifts_only_the_numeric_segment() {
    use chrono::{Datelike, TimeZone};

    // `1/2` has a step and no range, so the range-with-step rejection guard
    // does not catch it: it requires a `-`. This is a plain wrong-day case,
    // not a rejectable shape. Standard numbering: 1/2 starts at Monday and
    // steps by 2 through the week, landing on Mon, Wed, Fri.
    validate_cron_expressions(vec!["0 0 12 * * Sat,1/2".to_string()], UTC)
        .expect("a mixed named/numeric step day-of-week field is a working expression");

    let schedule = parse_standard_cron("0 0 12 * * Sat,1/2").unwrap();
    let monday = chrono_tz::UTC
        .with_ymd_and_hms(2026, 4, 13, 0, 0, 0)
        .unwrap();
    let fires: std::collections::HashSet<chrono::Weekday> = schedule
        .after(&monday)
        .take(4)
        .map(|t| t.weekday())
        .collect();
    assert_eq!(
        fires,
        [
            chrono::Weekday::Sat,
            chrono::Weekday::Mon,
            chrono::Weekday::Wed,
            chrono::Weekday::Fri
        ]
        .into(),
        "Sat,1/2 must fire Sat, Mon, Wed, Fri under standard numbering: {fires:?}"
    );
}

#[test]
fn mixed_dow_named_and_out_of_range_numeric_still_rejected() {
    // The numeric segment `8` is out of range and deliberately left
    // untranslated so the crate rejects it. That must survive per-segment
    // translation: a mixed field carrying it still errors rather than
    // getting silently mangled.
    let err = validate_cron_expressions(vec!["0 0 12 * * Mon,8".to_string()], UTC)
        .expect_err("an out-of-range numeric day-of-week segment must still be rejected");
    assert!(err.contains("Invalid cron expression"), "got: {err}");
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
    let desc = trigger_description("", &[sub("OuraSleepImported"), sub("EmailReceived")]);
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
    let result =
        check_scheduling_tool_in_trigger(tn::PAUSE_TRIGGER, Some("self-id"), Some("self-id"), None);
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
    let result = check_scheduling_tool_in_trigger(tn::LIST_TRIGGERS, None, Some("self-id"), None);
    assert!(result.is_none());
}

// -- never-fires guard --
//
// Each of these parses cleanly and passes every syntax check, then does nothing
// forever. That is the failure this guard exists for: there is no error to
// notice, so the trigger sits in the panel looking healthy.

/// `parse_cron_arg`'s error for a single expression, for the reject cases below.
fn reject(expr: &str) -> String {
    parse_cron_arg(&json!(expr), UTC)
        .expect_err(&format!("'{expr}' can never fire and must be rejected"))
}

#[test]
fn rejects_february_31() {
    let err = reject("0 0 9 31 2 *");
    assert!(err.contains("can never fire"), "got: {err}");
    assert!(
        err.contains("day-of-month 31 never occurs in month 2 (February)"),
        "the error must name the offending fields, got: {err}"
    );
}

#[test]
fn rejects_february_30() {
    let err = reject("0 0 9 30 2 *");
    assert!(
        err.contains("day-of-month 30 never occurs in month 2 (February)"),
        "got: {err}"
    );
}

#[test]
fn rejects_the_31st_of_thirty_day_months() {
    let err = reject("0 0 9 31 4,6,9,11 *");
    assert!(
        err.contains(
            "day-of-month 31 never occurs in month 4,6,9,11 (April, June, September, November)"
        ),
        "got: {err}"
    );
}

#[test]
fn rejects_impossible_date_with_a_weekday() {
    // Feb 30 AND a Sunday. The date alone is impossible, so that is what we name.
    let err = reject("0 0 9 30 2 Sun");
    assert!(
        err.contains("day-of-month 30 never occurs in month 2 (February)"),
        "got: {err}"
    );
}

#[test]
fn rejects_a_dead_expression_anywhere_in_the_array() {
    // A dead entry beside live ones is still a silent bug: the user believes
    // they scheduled two things and only got one.
    let err = parse_cron_arg(&json!(["0 0 8 * * *", "0 0 9 31 2 *"]), UTC).unwrap_err();
    assert!(err.contains("0 0 9 31 2 *"), "got: {err}");
    assert!(err.contains("can never fire"), "got: {err}");
}

#[test]
fn accepts_february_29_and_previews_the_next_three_leap_years() {
    // The regression this guard must not cause: Feb 29 is rare, not impossible.
    // February's ceiling is 29, not 28.
    let result = parse_cron_arg(&json!("0 0 9 29 2 *"), UTC)
        .expect("Feb 29 is a real date and must be accepted");
    let years: Vec<i32> = result
        .next_runs
        .iter()
        .map(chrono::Datelike::year)
        .collect();
    assert_eq!(years, vec![2028, 2032, 2036], "got: {:?}", result.next_runs);
    assert!(result.warnings.is_empty(), "got: {:?}", result.warnings);
}

#[test]
fn accepts_an_ordinary_daily_schedule_with_no_advice() {
    let result = parse_cron_arg(&json!("0 0 8 * * *"), UTC).unwrap();
    assert!(result.warnings.is_empty());
    assert_eq!(result.next_runs.len(), CRON_PREVIEW_COUNT);
}

// -- the day-of-month / day-of-week AND footgun --

/// The warnings a single expression produces, for the cases below.
fn warnings_for(expr: &str) -> Vec<String> {
    parse_cron_arg(&json!(expr), UTC)
        .unwrap_or_else(|e| panic!("'{expr}' must be accepted, got: {e}"))
        .warnings
}

#[test]
fn warns_but_accepts_a_single_day_anded_with_a_weekday() {
    // Reads as "the 1st, plus every Monday"; actually fires only when the 1st IS
    // a Monday, about 1.7 times a year.
    let warnings = warnings_for("0 0 9 1 * Mon");
    assert_eq!(warnings.len(), 1, "got: {warnings:?}");
    assert!(warnings[0].contains("day-of-month and day-of-week"));
    assert!(
        warnings[0].contains("ANDed"),
        "the warning must explain WHY it is surprising, got: {}",
        warnings[0]
    );
}

#[test]
fn warns_on_scattered_days_anded_with_a_weekday() {
    // Two isolated days: most months match neither.
    assert_eq!(warnings_for("0 0 9 15,25 * Fri").len(), 1);
}

#[test]
fn does_not_warn_on_the_nth_weekday_idiom() {
    // A 7-day window contains every weekday, so the AND matches exactly once per
    // month. This is how "first Monday" and "second Tuesday" are expressed, and
    // warning on them would train the user to ignore the warning.
    assert!(warnings_for("0 0 9 1-7 * Mon").is_empty());
    assert!(warnings_for("0 0 9 8-14 * Tue").is_empty());
}

#[test]
fn does_not_warn_when_only_one_of_the_two_fields_restricts() {
    assert!(warnings_for("0 0 9 1 * *").is_empty());
    assert!(warnings_for("0 0 9 * * Mon").is_empty());
    // A spelled-out full range restricts nothing either.
    assert!(warnings_for("0 0 9 1-31 * Mon").is_empty());
}

// -- the last-weekday-of-month recipe (system-knowhow/triggers.md) --

/// "Last Monday of the month", as documented. Day-of-month windows are the last
/// 7 candidate days of each month-length class, so the AND lands on exactly one
/// Monday per month.
const LAST_MONDAY: [&str; 3] = [
    "0 0 9 25-31 1,3,5,7,8,10,12 Mon",
    "0 0 9 24-30 4,6,9,11 Mon",
    "0 0 9 22-28 2 Mon",
];

#[test]
fn last_monday_recipe_is_accepted_without_warnings() {
    let val = json!(LAST_MONDAY.to_vec());
    let result = parse_cron_arg(&val, UTC).expect("the documented recipe must be accepted");
    assert_eq!(result.expressions.len(), 3);
    assert!(
        result.warnings.is_empty(),
        "each expression uses a 7-day window, so none is the footgun; got: {:?}",
        result.warnings
    );
}

#[test]
fn last_monday_recipe_fires_exactly_once_per_month() {
    use chrono::Datelike;
    use std::str::FromStr;

    let schedules: Vec<cron::Schedule> = LAST_MONDAY
        .iter()
        .map(|e| parse_standard_cron(e).unwrap())
        .collect();

    // Walk a fixed window rather than "from now", so the assertion does not
    // drift with the wall clock.
    let from = chrono::DateTime::<chrono::Utc>::from_str("2030-01-01T00:00:00Z")
        .unwrap()
        .with_timezone(&UTC);
    let mut fires_per_month: std::collections::BTreeMap<(i32, u32), usize> = Default::default();
    for schedule in &schedules {
        for fire in schedule.after(&from).take_while(|t| t.year() < 2033) {
            *fires_per_month
                .entry((fire.year(), fire.month()))
                .or_default() += 1;
        }
    }

    assert_eq!(
        fires_per_month.len(),
        36,
        "every month in 2030-2032 must be covered, got {}",
        fires_per_month.len()
    );
    for ((year, month), count) in &fires_per_month {
        assert_eq!(
            *count, 1,
            "{year}-{month:02} fired {count} times, expected exactly 1"
        );
    }
}

// -- the merged next-runs preview --

#[test]
fn preview_merges_across_the_array_under_or_semantics() {
    let val = json!(["0 0 8 * * *", "0 0 20 * * *"]);
    let result = parse_cron_arg(&val, UTC).unwrap();
    assert_eq!(result.next_runs.len(), CRON_PREVIEW_COUNT);
    assert!(
        result.next_runs.windows(2).all(|w| w[0] < w[1]),
        "the preview must be ascending and deduped, got: {:?}",
        result.next_runs
    );
    // Both expressions must be represented: the 8am and 8pm runs interleave, so
    // taking three from only the first would report three consecutive 8ams.
    let hours: std::collections::BTreeSet<u32> = result
        .next_runs
        .iter()
        .map(chrono::Timelike::hour)
        .collect();
    assert_eq!(
        hours,
        [8, 20].into_iter().collect(),
        "got: {:?}",
        result.next_runs
    );
}

#[test]
fn preview_dedupes_identical_expressions() {
    let val = json!(["0 0 8 * * *", "0 0 8 * * *"]);
    let result = parse_cron_arg(&val, UTC).unwrap();
    assert_eq!(result.next_runs.len(), CRON_PREVIEW_COUNT);
    assert!(result.next_runs.windows(2).all(|w| w[0] < w[1]));
}

#[test]
fn advice_suffix_carries_the_preview_and_the_warning() {
    let result = parse_cron_arg(&json!("0 0 9 1 * Mon"), UTC).unwrap();
    let suffix = result.advice_suffix();
    assert!(suffix.contains("Next 3 runs:"), "got: {suffix}");
    assert!(suffix.contains("WARNING:"), "got: {suffix}");
}

#[test]
fn advice_suffix_is_empty_for_a_cleared_schedule() {
    // An update that clears the cron has no runs to preview.
    assert_eq!(ValidatedCron::default().advice_suffix(), "");
}

#[test]
fn next_occurrences_multi_is_the_source_of_truth_for_the_single_form() {
    use std::str::FromStr;
    let s1 = cron::Schedule::from_str("0 0 8 * * *").unwrap();
    let s2 = cron::Schedule::from_str("0 0 6 * * *").unwrap();
    let schedules = [s1, s2];
    assert_eq!(
        next_occurrence_multi(&schedules, UTC),
        next_occurrences_multi(&schedules, UTC, 1)
            .into_iter()
            .next()
    );
}
