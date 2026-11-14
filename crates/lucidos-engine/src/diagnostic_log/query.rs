//! A bounded query over the diagnostic log: parse and check the parameters,
//! scan both segments, and keep the newest matches within the line and byte
//! budgets. Every message is sanitized again as it is read.

use std::collections::VecDeque;
use std::io::Read;
use std::path::Path;

use chrono::{DateTime, Duration, Utc};
use serde::Serialize;

use super::sanitize;
use super::writer::{self, Record};

pub const DEFAULT_SINCE: Duration = Duration::hours(1);
pub const MAX_WINDOW: Duration = Duration::days(7);
pub const MAX_CONTAINS: usize = 4;
pub const MAX_FILTER_BYTES: usize = 256;
pub const REGEX_SIZE_LIMIT: usize = 1024 * 1024;
pub const MAX_LABEL_BYTES: usize = 64;
pub const DEFAULT_LIMIT: usize = 200;
pub const MAX_LIMIT: usize = 2000;
pub const DEFAULT_BYTE_LIMIT: usize = 64 * 1024;
pub const MAX_BYTE_LIMIT: usize = 256 * 1024;

/// A checked query. Only [`parse`] builds one, so every bound holds.
#[derive(Debug)]
pub struct Query {
    since: DateTime<Utc>,
    until: DateTime<Utc>,
    contains: Vec<String>,
    regex: Option<regex::Regex>,
    label: Option<String>,
    pid: Option<u32>,
    limit: usize,
    byte_limit: usize,
}

impl Query {
    /// The filters, for the route's one summary line. Never the lines served.
    pub fn summary(&self) -> String {
        format!(
            "since {} until {}, {} contains, regex {}, label {:?}, pid {:?}",
            self.since.to_rfc3339(),
            self.until.to_rfc3339(),
            self.contains.len(),
            self.regex.is_some(),
            self.label,
            self.pid
        )
    }
}

/// Parse a raw query string. Unknown and repeated keys refuse, as does any
/// value past its bound. `limit` and `byte_limit` clamp to their maximum,
/// which only narrows the answer.
pub fn parse(raw: Option<&str>, now: DateTime<Utc>) -> Result<Query, String> {
    let mut since = None;
    let mut until = None;
    let mut contains = Vec::new();
    let mut regex = None;
    let mut label = None;
    let mut pid = None;
    let mut limit = None;
    let mut byte_limit = None;
    for pair in raw.unwrap_or_default().split('&').filter(|p| !p.is_empty()) {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        let key = decode(key)?;
        let value = decode(value)?;
        match key.as_str() {
            "since" => set_once(&mut since, "since", parse_time(&value, now)?)?,
            "until" => set_once(&mut until, "until", parse_time(&value, now)?)?,
            "contains" => {
                if contains.len() == MAX_CONTAINS {
                    return Err(format!("at most {MAX_CONTAINS} `contains` filters"));
                }
                contains.push(bounded_text("contains", value, MAX_FILTER_BYTES)?);
            }
            "regex" => {
                let pattern = bounded_text("regex", value, MAX_FILTER_BYTES)?;
                let compiled = regex::RegexBuilder::new(&pattern)
                    .size_limit(REGEX_SIZE_LIMIT)
                    .build()
                    .map_err(|e| format!("`regex` does not compile: {e}"))?;
                set_once(&mut regex, "regex", compiled)?;
            }
            "label" => {
                let text = bounded_text("label", value, MAX_LABEL_BYTES)?;
                set_once(&mut label, "label", text)?;
            }
            "pid" => {
                let parsed = value
                    .parse::<u32>()
                    .map_err(|_| format!("`pid` must be a process id, got {value:?}"))?;
                set_once(&mut pid, "pid", parsed)?;
            }
            "limit" => set_once(&mut limit, "limit", count("limit", &value, MAX_LIMIT)?)?,
            "byte_limit" => set_once(
                &mut byte_limit,
                "byte_limit",
                count("byte_limit", &value, MAX_BYTE_LIMIT)?,
            )?,
            other => return Err(format!("unknown parameter `{other}`")),
        }
    }
    let until = until.unwrap_or(now);
    let since = since.unwrap_or(until - DEFAULT_SINCE);
    if since > until {
        return Err("`since` is after `until`".into());
    }
    if until - since > MAX_WINDOW {
        return Err(format!(
            "the window is longer than {} days",
            MAX_WINDOW.num_days()
        ));
    }
    Ok(Query {
        since,
        until,
        contains,
        regex,
        label,
        pid,
        limit: limit.unwrap_or(DEFAULT_LIMIT),
        byte_limit: byte_limit.unwrap_or(DEFAULT_BYTE_LIMIT),
    })
}

fn decode(text: &str) -> Result<String, String> {
    urlencoding::decode(&text.replace('+', " "))
        .map(|decoded| decoded.into_owned())
        .map_err(|_| format!("{text:?} is not valid UTF-8 once decoded"))
}

fn set_once<T>(slot: &mut Option<T>, key: &str, value: T) -> Result<(), String> {
    if slot.replace(value).is_some() {
        return Err(format!("`{key}` given twice"));
    }
    Ok(())
}

fn bounded_text(key: &str, value: String, max: usize) -> Result<String, String> {
    if value.is_empty() {
        return Err(format!("`{key}` is empty"));
    }
    if value.len() > max {
        return Err(format!("`{key}` is longer than {max} bytes"));
    }
    Ok(value)
}

fn count(key: &str, value: &str, max: usize) -> Result<usize, String> {
    match value.parse::<usize>() {
        Ok(0) | Err(_) => Err(format!("`{key}` must be a positive number, got {value:?}")),
        Ok(n) => Ok(n.min(max)),
    }
}

/// RFC 3339, or an age before `now` such as `90s`, `30m`, `2h` or `3d`.
fn parse_time(value: &str, now: DateTime<Utc>) -> Result<DateTime<Utc>, String> {
    if let Ok(time) = DateTime::parse_from_rfc3339(value) {
        return Ok(time.with_timezone(&Utc));
    }
    let refuse = || format!("{value:?} is neither RFC 3339 nor an age like 90m, 2h or 3d");
    let (unit_at, _) = value.char_indices().next_back().ok_or_else(refuse)?;
    let (amount, unit) = value.split_at(unit_at);
    let amount: i64 = amount.parse().map_err(|_| refuse())?;
    let age = match unit {
        "s" => Duration::try_seconds(amount),
        "m" => Duration::try_minutes(amount),
        "h" => Duration::try_hours(amount),
        "d" => Duration::try_days(amount),
        _ => None,
    }
    .filter(|age| *age >= Duration::zero())
    .ok_or_else(refuse)?;
    now.checked_sub_signed(age).ok_or_else(refuse)
}

/// What a query returns: the newest matches, oldest first.
#[derive(Debug, Serialize)]
pub struct QueryResult {
    pub lines: Vec<Record>,
    pub returned: usize,
    /// Matches older than the returned lines that a budget left out.
    pub earlier_matches: usize,
    /// True when `byte_limit` cut the answer short.
    pub truncated: bool,
    pub byte_size: usize,
    /// The oldest record retained, so a caller learns its window predates
    /// retention. `None` when the log is empty.
    pub coverage_starts_at: Option<String>,
    pub hint: Option<String>,
}

/// Run `query` over the segments in `dir`. Refuses a symlinked dir or
/// segment, as the writer does.
pub fn run(dir: &Path, query: &Query) -> std::io::Result<QueryResult> {
    if !std::fs::symlink_metadata(dir)?.is_dir() {
        return Err(std::io::Error::other(format!(
            "{} is a symlink or not a directory",
            dir.display()
        )));
    }
    let mut kept: VecDeque<Record> = VecDeque::new();
    let mut earlier_matches = 0;
    let mut coverage_starts_at: Option<DateTime<Utc>> = None;
    for text in read_segments(dir)? {
        for line in text.lines() {
            let Ok(record) = serde_json::from_str::<Record>(line) else {
                continue;
            };
            let Ok(ts) = DateTime::parse_from_rfc3339(&record.ts) else {
                continue;
            };
            let ts = ts.with_timezone(&Utc);
            coverage_starts_at = Some(coverage_starts_at.map_or(ts, |oldest| oldest.min(ts)));
            if let Some(record) = matching(query, ts, record) {
                kept.push_back(record);
                if kept.len() > query.limit {
                    kept.pop_front();
                    earlier_matches += 1;
                }
            }
        }
    }
    let mut lines = Vec::with_capacity(kept.len());
    let mut byte_size = 0;
    let mut truncated = false;
    while let Some(record) = kept.pop_back() {
        let size = serde_json::to_vec(&record).map_or(0, |bytes| bytes.len()) + 1;
        if byte_size + size > query.byte_limit {
            truncated = true;
            earlier_matches += kept.len() + 1;
            break;
        }
        byte_size += size;
        lines.push(record);
    }
    lines.reverse();
    let coverage_starts_at = coverage_starts_at.map(|ts| ts.to_rfc3339());
    let hint = hint(
        query,
        &lines,
        earlier_matches,
        truncated,
        &coverage_starts_at,
    );
    Ok(QueryResult {
        returned: lines.len(),
        lines,
        earlier_matches,
        truncated,
        byte_size,
        coverage_starts_at,
        hint,
    })
}

/// The text of each existing segment, oldest first.
///
/// The current segment opens first. A rotation before the rotated one opens
/// makes both handles one file, which is then read once, so no segment is lost.
fn read_segments(dir: &Path) -> std::io::Result<Vec<String>> {
    let current = open_segment(&dir.join(writer::FILE_NAME))?;
    let rotated = open_segment(&dir.join(writer::ROTATED_FILE_NAME))?;
    let rotated = match (&current, rotated) {
        (Some(current), Some(rotated)) if same_file(current, &rotated)? => None,
        (_, rotated) => rotated,
    };
    [rotated, current]
        .into_iter()
        .flatten()
        .map(read_segment)
        .collect()
}

/// A segment open for reading, or `None` when it does not exist yet.
fn open_segment(path: &Path) -> std::io::Result<Option<std::fs::File>> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    match writer::open_no_follow(path, &mut options) {
        Ok(file) => Ok(Some(file)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

#[cfg(unix)]
fn same_file(a: &std::fs::File, b: &std::fs::File) -> std::io::Result<bool> {
    use std::os::unix::fs::MetadataExt;
    let (a, b) = (a.metadata()?, b.metadata()?);
    Ok(a.dev() == b.dev() && a.ino() == b.ino())
}

/// Lucidos ships no Windows build. This arm only keeps the crate compiling.
#[cfg(not(unix))]
fn same_file(_: &std::fs::File, _: &std::fs::File) -> std::io::Result<bool> {
    Ok(false)
}

fn read_segment(file: std::fs::File) -> std::io::Result<String> {
    let mut bytes = Vec::new();
    file.take(writer::SEGMENT_MAX_BYTES * 2)
        .read_to_end(&mut bytes)?;
    Ok(String::from_utf8(bytes)
        .unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned()))
}

/// The record as a reader gets it, when it matches. The text filters run on
/// the sanitized message, so no filter can probe a masked value.
fn matching(query: &Query, ts: DateTime<Utc>, record: Record) -> Option<Record> {
    if ts < query.since || ts > query.until {
        return None;
    }
    if query.pid.is_some_and(|pid| pid != record.pid) {
        return None;
    }
    let label = sanitize::for_read(&record.label);
    let msg = sanitize::for_read(&record.msg);
    if let Some(wanted) = &query.label {
        let tagged = msg
            .strip_prefix('[')
            .and_then(|rest| rest.split_once(']'))
            .is_some_and(|(tag, _)| tag.eq_ignore_ascii_case(wanted));
        if !tagged && !label.eq_ignore_ascii_case(wanted) {
            return None;
        }
    }
    if !query
        .contains
        .iter()
        .all(|text| msg.contains(text.as_str()))
    {
        return None;
    }
    if query.regex.as_ref().is_some_and(|re| !re.is_match(&msg)) {
        return None;
    }
    Some(Record {
        ts: record.ts,
        pid: record.pid,
        label,
        msg,
    })
}

fn hint(
    query: &Query,
    lines: &[Record],
    earlier_matches: usize,
    truncated: bool,
    coverage_starts_at: &Option<String>,
) -> Option<String> {
    let predates = coverage_starts_at
        .as_deref()
        .and_then(|ts| DateTime::parse_from_rfc3339(ts).ok())
        .is_some_and(|oldest| query.since < oldest);
    if earlier_matches > 0 {
        let cause = if truncated { "byte_limit" } else { "limit" };
        Some(format!(
            "{earlier_matches} earlier matches left out by {cause}; narrow the filters or move `until` back"
        ))
    } else if lines.is_empty() && predates {
        Some("no match; the window starts before the oldest retained line".into())
    } else {
        None
    }
}

#[cfg(test)]
#[path = "query_tests.rs"]
mod tests;
