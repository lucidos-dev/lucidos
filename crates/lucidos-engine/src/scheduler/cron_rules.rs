use cron::TimeUnitSpec;
use std::str::FromStr;

/// Translate the day-of-week field from standard cron convention (0=Sun, 1=Mon, ..., 6=Sat, 7=Sun)
/// to the `cron` crate's 1-based convention (1=Sun, 2=Mon, ..., 7=Sat).
pub fn translate_dow_for_cron_crate(expr: &str) -> String {
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

    if dow == "*" || dow.starts_with("*/") {
        return expr.to_string();
    }

    // Decide named vs numeric per comma segment, not for the whole field. A
    // mixed field (`Mon,1`) carries one segment of each kind: the named
    // segment has no digits to shift, and the numeric one still needs the
    // shift even though its sibling segment is a name. A new segment shape
    // needs the same check added to `numeric_dow_range_with_step_token`
    // below.
    //
    // A combined range-and-step token (`1-5/2`) is not shifted here, and
    // never needs to be: `validate_cron_expressions` rejects it first, via
    // `numeric_dow_range_with_step_token` below (see its doc comment for why).
    let translated_dow = dow
        .split(',')
        .map(|segment| {
            if segment.chars().any(|c| c.is_ascii_alphabetic()) {
                return segment.to_string();
            }
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
pub fn parse_standard_cron(expr: &str) -> Result<cron::Schedule, String> {
    let translated = translate_dow_for_cron_crate(expr);
    cron::Schedule::from_str(&translated).map_err(|e| e.to_string())
}

/// Resolve an IANA timezone name for cron validation, falling back to UTC.
///
/// The fallback is safe for the never-fires guard, which is timezone-independent:
/// Feb 31 does not exist anywhere. It only shifts the wall-clock times reported
/// in the preview, so a bad name is logged rather than swallowed.
pub fn cron_tz_or_utc(name: &str, context: &str) -> chrono_tz::Tz {
    match name.parse() {
        Ok(tz) => tz,
        Err(_) => {
            log_legacy!(
                "[Scheduler] Invalid timezone '{}' for {}, validating cron in UTC",
                name,
                context
            );
            chrono_tz::UTC
        }
    }
}

/// Report three upcoming fire times. This exposes scheduling mistakes such as a
/// monthly job that fires once a year.
pub const CRON_PREVIEW_COUNT: usize = 3;

/// A validated set of cron expressions, plus the advice a caller should surface
/// alongside it.
///
/// Restricting both day fields is legal and enables nth-weekday recipes. Return
/// that warning with the successful result. Merge upcoming runs across all
/// expressions under OR semantics.
#[derive(Debug, Clone, Default)]
pub struct ValidatedCron {
    pub expressions: Vec<String>,
    pub warnings: Vec<String>,
    pub next_runs: Vec<chrono::DateTime<chrono_tz::Tz>>,
}

impl ValidatedCron {
    /// The preview rendered for a text surface (the LLM tool result). Empty when
    /// the set itself is empty (an update that clears the schedule).
    pub fn preview_line(&self) -> Option<String> {
        if self.next_runs.is_empty() {
            return None;
        }
        let times: Vec<String> = self
            .next_runs
            .iter()
            .map(|t| t.format("%Y-%m-%d %H:%M %Z").to_string())
            .collect();
        Some(format!("Next {} runs: {}.", times.len(), times.join(", ")))
    }

    /// The preview rendered for a JSON surface (the HTTP API, and through it the
    /// CLI and the SDK).
    pub fn next_runs_rfc3339(&self) -> Vec<String> {
        self.next_runs.iter().map(|t| t.to_rfc3339()).collect()
    }

    /// Text appended to a create / update tool result: the next-run preview, then
    /// any AND-footgun warnings. Empty when there is nothing to add, so the
    /// caller can concatenate it unconditionally.
    pub fn advice_suffix(&self) -> String {
        let mut parts: Vec<String> = Vec::new();
        if let Some(line) = self.preview_line() {
            parts.push(line);
        }
        parts.extend(self.warnings.iter().map(|w| format!("WARNING: {}", w)));
        if parts.is_empty() {
            String::new()
        } else {
            format!(" {}", parts.join(" "))
        }
    }
}

/// Find a numeric day-of-week range-with-step token (`1-5/2`) in a day-of-week
/// field, and return it verbatim.
///
/// `translate_dow_for_cron_crate` cannot shift this shape. It reads the
/// range end as `5/2`, fails to parse it, and leaves the token unshifted.
/// The token then fires on the `cron` crate's own numbering, not the user's.
/// A named token (`Mon-Fri/2`) has no such gap: names bypass translation and
/// the crate numbers them the same way the user wrote them.
///
/// Checked per comma-separated segment, not on the field as a whole. A
/// mixed field (`Mon,1-5/2`) must still catch the numeric segment: a letter
/// in one segment skips only that segment. A new segment shape must be
/// checked here and in `translate_dow_for_cron_crate`'s classifier: past
/// fixes here missed one of the two.
fn numeric_dow_range_with_step_token(dow: &str) -> Option<&str> {
    let is_numeric = |s: &str| s.parse::<u8>().is_ok();
    dow.split(',').find(|segment| {
        let Some((range, step)) = segment.split_once('/') else {
            return false;
        };
        let Some((start, end)) = range.split_once('-') else {
            return false;
        };
        is_numeric(start) && is_numeric(end) && is_numeric(step)
    })
}

/// Validate the cron expressions a trigger will run on: field count, syntax, and
/// whether each one can ever fire at all.
///
/// A syntactically valid expression can have no matching date, such as February
/// 31. Check `Schedule::upcoming(tz).next()` to catch it. The same check
/// catches schedules with no result within the crate's supported years.
///
/// `tz` only affects *when* the reported runs land: a never-fires expression is
/// never-fires in every timezone, so a UTC fallback does not weaken the check.
///
/// Error messages carry no prefix, so an HTTP caller can surface them verbatim.
/// [`parse_cron_arg`] adds the `Error:` prefix the LLM tool surface expects.
pub fn validate_cron_expressions(
    expressions: Vec<String>,
    tz: chrono_tz::Tz,
) -> Result<ValidatedCron, String> {
    let mut schedules: Vec<cron::Schedule> = Vec::with_capacity(expressions.len());
    let mut warnings: Vec<String> = Vec::new();

    for expr in &expressions {
        let parts: Vec<&str> = expr.split_whitespace().collect();
        if parts.len() != 6 {
            return Err(format!(
                "Invalid cron expression '{}'. Must have 6 fields: second minute hour day-of-month month day-of-week. Example: '0 0 8 * * *' for 8am daily.",
                expr
            ));
        }
        if let Some(token) = numeric_dow_range_with_step_token(parts[5]) {
            return Err(format!(
                "Cron expression '{}' uses day-of-week token '{}', which combines a \
                 numeric range and a step. This shape is not supported: list the days \
                 instead ('1,3,5'), or use the named weekday range instead ('Mon-Fri/2').",
                expr, token
            ));
        }
        let schedule = match parse_standard_cron(expr) {
            Ok(s) => s,
            Err(_) => return Err(format!("Invalid cron expression '{}'. Check syntax.", expr)),
        };
        if schedule.upcoming(tz).next().is_none() {
            return Err(format!(
                "Cron expression '{}' can never fire: {}.",
                expr,
                diagnose_never_fires(&schedule)
            ));
        }
        if let Some(warning) = and_footgun_warning(&schedule, expr) {
            warnings.push(warning);
        }
        schedules.push(schedule);
    }

    let next_runs = next_occurrences_multi(&schedules, tz, CRON_PREVIEW_COUNT);
    Ok(ValidatedCron {
        expressions,
        warnings,
        next_runs,
    })
}

/// The `cron` crate searches from 1970 through 2100. Include the upper bound in
/// the diagnosis because a matching date may lie beyond it.
///
/// Rejecting that case is still correct rather than over-strict. The runner
/// consults the same `upcoming()` oracle and exits with "no more occurrences"
/// when it answers `None`, so such a trigger genuinely would not fire: accepting
/// it would recreate exactly the silent non-firing this guard exists to stop.
const CRON_SEARCH_HORIZON_YEAR: u32 = 2100;

/// Longest a month can ever be. February is 29, not 28: `0 0 9 29 2 *` is rare
/// but perfectly real (2028, 2032, 2036), and must not be rejected.
fn longest_month_length(month: u32) -> u32 {
    match month {
        2 => 29,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

const MONTH_NAMES: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

/// Best-effort attribution for an expression the crate says can never match.
///
/// One cause dominates in practice: a day-of-month no selected month is long
/// enough to contain (Feb 30, Feb 31, the 31st of a 30-day month). That case gets
/// named precisely, because "invalid cron expression" tells the user nothing they
/// can act on. Anything else falls back to a plain statement, which is still far
/// more useful than silence.
pub(crate) fn diagnose_never_fires(schedule: &cron::Schedule) -> String {
    let days: Vec<u32> = schedule.days_of_month().iter().collect();
    let months: Vec<u32> = schedule.months().iter().collect();

    let shortest_day = days.iter().copied().min();
    let longest_month = months.iter().copied().map(longest_month_length).max();
    if let (Some(shortest_day), Some(longest_month)) = (shortest_day, longest_month) {
        // Every selected day exceeds every selected month's ceiling, so the
        // day-of-month / month pair alone is impossible.
        if shortest_day > longest_month {
            let names: Vec<&str> = months
                .iter()
                .filter_map(|m| MONTH_NAMES.get((*m as usize).saturating_sub(1)).copied())
                .collect();
            return format!(
                "day-of-month {} never occurs in month {} ({})",
                join_ordinals(&days),
                join_ordinals(&months),
                names.join(", ")
            );
        }
    }

    format!(
        "no date matches this combination of fields before {}, the last year the cron library searches",
        CRON_SEARCH_HORIZON_YEAR
    )
}

fn join_ordinals(ordinals: &[u32]) -> String {
    ordinals
        .iter()
        .map(|o| o.to_string())
        .collect::<Vec<_>>()
        .join(",")
}

/// True when `sorted` contains `len` consecutive integers.
fn has_consecutive_run(sorted: &[u32], len: usize) -> bool {
    if len <= 1 {
        return !sorted.is_empty();
    }
    let mut run = 1usize;
    for pair in sorted.windows(2) {
        run = if pair[1] == pair[0] + 1 { run + 1 } else { 1 };
        if run >= len {
            return true;
        }
    }
    false
}

/// The day-of-month / day-of-week AND footgun.
///
/// The two fields use AND within an expression. Vixie cron uses OR. Here `0 0 9
/// 1 * Mon` fires only when the first of the month falls on Monday.
///
/// Not every restricted pair is a mistake, so this warns rather than rejects, and
/// stays quiet for the shape that is deliberate. A day-of-month window of 7
/// consecutive days contains every weekday, so the AND is guaranteed exactly one
/// match per selected month: that is precisely how "first Monday" (`1-7 * Mon`)
/// and "last Monday" (`25-31 1,3,5,7,8,10,12 Mon`) are expressed. A narrower
/// window is the footgun, because some months then match nothing.
fn and_footgun_warning(schedule: &cron::Schedule, expr: &str) -> Option<String> {
    if schedule.days_of_month().is_all() || schedule.days_of_week().is_all() {
        return None;
    }
    let days: Vec<u32> = schedule.days_of_month().iter().collect();
    if has_consecutive_run(&days, 7) {
        return None;
    }
    Some(format!(
        "Cron expression '{}' restricts both day-of-month and day-of-week. These are ANDed, \
         not ORed: it fires only when that day-of-month IS that weekday, which can be rare. \
         For \"either one\", pass two expressions instead. For \"the first <weekday> of the \
         month\", widen day-of-month to a 7-day window (e.g. '1-7').",
        expr
    ))
}

/// Parse the `cron` tool argument, which can be either a single string or a JSON
/// array of strings, then validate the result via [`validate_cron_expressions`].
pub fn parse_cron_arg(
    value: &serde_json::Value,
    tz: chrono_tz::Tz,
) -> Result<ValidatedCron, String> {
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

    validate_cron_expressions(expressions, tz).map_err(|e| format!("Error: {}", e))
}

/// Return the first `n` fire times across the set. A trigger fires at the
/// earliest match from any expression.
///
/// Taking `n` from each stream before merging is sufficient: the first `n` of the
/// union are always contained in the union of each stream's own first `n`.
pub fn next_occurrences_multi(
    schedules: &[cron::Schedule],
    tz: chrono_tz::Tz,
    n: usize,
) -> Vec<chrono::DateTime<chrono_tz::Tz>> {
    next_occurrences_after(schedules, tz, chrono::Utc::now(), n)
}

/// [`next_occurrences_multi`] with the clock as an argument.
///
/// Only instants strictly after `now` in UTC count. In the hour a fall-back
/// repeats, `cron` resolves an ambiguous wall time to its EARLIER instant,
/// which has already passed. A loop taking that as its next fire reads it as
/// due, and fires again after every run until the hour is over.
pub fn next_occurrences_after(
    schedules: &[cron::Schedule],
    tz: chrono_tz::Tz,
    now: chrono::DateTime<chrono::Utc>,
    n: usize,
) -> Vec<chrono::DateTime<chrono_tz::Tz>> {
    let local_now = now.with_timezone(&tz);
    let mut merged: Vec<chrono::DateTime<chrono_tz::Tz>> = schedules
        .iter()
        .flat_map(|s| {
            s.after(&local_now)
                .filter(|t| t.with_timezone(&chrono::Utc) > now)
                .take(n)
        })
        .collect();
    merged.sort_unstable();
    merged.dedup();
    merged.truncate(n);
    merged
}

/// Find the nearest next occurrence across multiple cron schedules.
/// Returns the earliest upcoming time from any of the schedules.
pub fn next_occurrence_multi(
    schedules: &[cron::Schedule],
    tz: chrono_tz::Tz,
) -> Option<chrono::DateTime<chrono_tz::Tz>> {
    next_occurrences_multi(schedules, tz, 1).into_iter().next()
}
