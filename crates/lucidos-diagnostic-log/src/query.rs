//! A bounded query over the diagnostic log: parse and check the parameters,
//! scan both segments, and keep the newest matches within the line and byte
//! budgets. Every message is sanitized again as it is read.

use std::collections::BTreeMap;
use std::io::Read;
use std::path::Path;

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};

use super::writer::{self, Record};
use super::{sanitize, Destination};

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
const MAX_LINE_BYTES: u64 = 128 * 1024;

/// A process whose diagnostic records can appear in the live timeline.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Source {
    Engine,
    Gateway,
    Client,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SourceStatus {
    Ok,
    Unsupported,
    NotPresent,
    Unreadable,
    GatewayUnreachable,
    TeamGateway,
    GatewayTooOld,
    Busy,
}

/// A checked query. Only [`parse`] builds one, so every bound holds.
#[derive(Clone, Debug)]
pub struct Query {
    source: Option<Source>,
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
    pub fn includes(&self, source: Source) -> bool {
        self.source.is_none_or(|selected| selected == source)
    }

    pub fn machine_sources(&self) -> impl Iterator<Item = Source> + '_ {
        [Source::Gateway, Source::Client]
            .into_iter()
            .filter(|source| self.includes(*source))
    }

    pub fn engine_only(&self) -> bool {
        self.source == Some(Source::Engine)
    }

    /// Forward relative times as absolute times so both processes scan the same window.
    pub fn forward_params(&self) -> Vec<(&'static str, String)> {
        let mut params = vec![
            ("since", self.since.to_rfc3339()),
            ("until", self.until.to_rfc3339()),
            ("limit", self.limit.to_string()),
            ("byte_limit", self.byte_limit.to_string()),
            (
                "source",
                match self.source {
                    Some(Source::Engine) => "engine",
                    Some(Source::Gateway) => "gateway",
                    Some(Source::Client) => "client",
                    None => "all",
                }
                .into(),
            ),
        ];
        params.extend(
            self.contains
                .iter()
                .cloned()
                .map(|value| ("contains", value)),
        );
        params.extend(self.label.iter().cloned().map(|value| ("label", value)));
        params.extend(
            self.regex
                .iter()
                .map(|regex| ("regex", regex.as_str().into())),
        );
        params.extend(self.pid.map(|pid| ("pid", pid.to_string())));
        params
    }

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
    parse_query(raw, now, true)
}

/// Parse filters for an inbox bundle, which has no live source selector.
pub fn parse_inbox(raw: Option<&str>, now: DateTime<Utc>) -> Result<Query, String> {
    parse_query(raw, now, false)
}

fn parse_query(raw: Option<&str>, now: DateTime<Utc>, live: bool) -> Result<Query, String> {
    let mut source = None;
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
            "source" if live => {
                let selected = match value.as_str() {
                    "engine" => Some(Source::Engine),
                    "gateway" => Some(Source::Gateway),
                    "client" => Some(Source::Client),
                    "all" => None,
                    _ => return Err("`source` must be engine, gateway, client or all".into()),
                };
                set_once(&mut source, "source", selected)?;
            }
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
        source: source.flatten(),
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

#[derive(Debug, Serialize, Deserialize)]
pub struct QueryLine {
    pub ts: String,
    pub pid: u32,
    pub label: String,
    pub msg: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<Source>,
}

type TimelineKey = (DateTime<Utc>, Option<Source>);

impl QueryLine {
    fn key(&self) -> TimelineKey {
        (
            self.ts.parse().expect("checked record timestamp"),
            self.source,
        )
    }
}

/// What a query returns: the newest matches, oldest first.
#[derive(Debug, Serialize, Deserialize)]
pub struct QueryResult {
    pub lines: Vec<QueryLine>,
    pub returned: usize,
    /// Matches older than the returned lines that a budget left out.
    pub earlier_matches: usize,
    /// True when `byte_limit` cut the answer short.
    pub truncated: bool,
    pub byte_size: usize,
    /// The oldest record retained, or `None` for an empty log.
    pub coverage_starts_at: Option<String>,
    pub hint: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub sources: BTreeMap<Source, SourceStatus>,
    /// Newest line excluded by the byte budget. A merge must stop here too.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    byte_cut: Option<TimelineKey>,
}

impl QueryResult {
    pub fn into_public(mut self) -> Self {
        self.byte_cut = None;
        self
    }

    pub fn unavailable(source: Source, status: SourceStatus) -> Self {
        Self {
            lines: Vec::new(),
            returned: 0,
            earlier_matches: 0,
            truncated: false,
            byte_size: 0,
            coverage_starts_at: None,
            hint: None,
            sources: BTreeMap::from([(source, status)]),
            byte_cut: None,
        }
    }
}

pub fn machine_unavailable(query: &Query, status: SourceStatus) -> QueryResult {
    merge(
        query,
        query
            .machine_sources()
            .map(|source| QueryResult::unavailable(source, status)),
    )
}

/// Query a live source, including its availability when the directory is missing.
pub fn run_source(dir: &Path, query: &Query, source: Source) -> QueryResult {
    match run(dir, query, source) {
        Ok(answer) => answer,
        Err(error) => QueryResult::unavailable(
            source,
            match error.kind() {
                std::io::ErrorKind::NotFound => SourceStatus::NotPresent,
                _ => SourceStatus::Unreadable,
            },
        ),
    }
}

/// Run over the segments in `dir`. Refuses a symlinked dir or segment.
pub fn run(dir: &Path, query: &Query, source: Source) -> std::io::Result<QueryResult> {
    let mut scan = Scan::default();
    scan.read(dir, query, source)?;
    Ok(scan.finish(query, BTreeMap::from([(source, SourceStatus::Ok)])))
}

/// Apply the same checked filters and budgets to an already-rendered inbox timeline.
pub fn run_records(
    query: &Query,
    records: impl IntoIterator<Item = Record>,
) -> std::io::Result<QueryResult> {
    let mut scan = Scan::default();
    scan.add(query, records, None);
    Ok(scan.finish(query, BTreeMap::new()).into_public())
}

#[derive(Default)]
struct Scan {
    kept: BTreeMap<(DateTime<Utc>, usize), QueryLine>,
    matches: usize,
    coverage: Option<DateTime<Utc>>,
}

impl Scan {
    fn read(&mut self, dir: &Path, query: &Query, source: Source) -> std::io::Result<()> {
        let segments = read_segments(dir)?;
        let records = segments
            .iter()
            .flat_map(|text| text.lines())
            .filter_map(|line| {
                if line.len() as u64 > MAX_LINE_BYTES {
                    return None;
                }
                serde_json::from_str::<Record>(line).ok()
            });
        self.add(query, records, Some(source));
        Ok(())
    }

    fn add(
        &mut self,
        query: &Query,
        records: impl IntoIterator<Item = Record>,
        source: Option<Source>,
    ) {
        for record in records {
            let Ok(ts) = record.ts.parse::<DateTime<Utc>>() else {
                continue;
            };
            self.coverage = Some(self.coverage.map_or(ts, |oldest| oldest.min(ts)));
            if let Some(line) = matching(query, ts, record, source) {
                self.kept.insert((ts, self.matches), line);
                self.matches += 1;
                if self.kept.len() > query.limit {
                    self.kept.pop_first();
                }
            }
        }
    }

    fn finish(self, query: &Query, sources: BTreeMap<Source, SourceStatus>) -> QueryResult {
        let earlier_matches = self.matches - self.kept.len();
        finish(
            query,
            self.kept.into_values().collect(),
            earlier_matches,
            self.coverage,
            None,
            sources,
        )
    }
}

/// Merge bounded timelines and apply the same budgets to the combined answer.
pub fn merge(query: &Query, results: impl IntoIterator<Item = QueryResult>) -> QueryResult {
    let mut lines = Vec::new();
    let mut earlier_matches = 0;
    let mut coverage = None;
    let mut byte_cut = None;
    let mut sources = BTreeMap::new();
    for result in results {
        lines.extend(result.lines);
        earlier_matches += result.earlier_matches;
        coverage = coverage
            .into_iter()
            .chain(
                result
                    .coverage_starts_at
                    .as_deref()
                    .and_then(|ts| ts.parse().ok()),
            )
            .min();
        byte_cut = byte_cut.max(result.byte_cut);
        sources.extend(result.sources);
    }
    lines.sort_by_cached_key(QueryLine::key);
    finish(query, lines, earlier_matches, coverage, byte_cut, sources)
}

fn finish(
    query: &Query,
    mut candidates: Vec<QueryLine>,
    mut earlier_matches: usize,
    coverage: Option<DateTime<Utc>>,
    mut byte_cut: Option<TimelineKey>,
    sources: BTreeMap<Source, SourceStatus>,
) -> QueryResult {
    let mut lines = Vec::new();
    let mut byte_size = 0;
    while let Some(line) = candidates.pop() {
        if lines.len() == query.limit {
            earlier_matches += candidates.len() + 1;
            break;
        }
        let key = line.key();
        // Equal keys from a single source were retained ahead of its cut.
        let size = serde_json::to_vec(&line)
            .expect("query line serializes")
            .len()
            + 1;
        if byte_cut.is_some_and(|cut| key < cut) || byte_size + size > query.byte_limit {
            byte_cut = byte_cut.max(Some(key));
            earlier_matches += candidates.len() + 1;
            break;
        }
        byte_size += size;
        lines.push(line);
    }
    lines.reverse();
    let truncated = byte_cut.is_some();
    let coverage_starts_at = coverage.map(|ts| ts.to_rfc3339());
    let mut hint = hint(
        query,
        &lines,
        earlier_matches,
        truncated,
        &coverage_starts_at,
    );
    if sources
        .values()
        .any(|status| *status == SourceStatus::TeamGateway)
    {
        let message = "Client logs on a team or paired gateway require Send diagnostics";
        hint = Some(match hint {
            Some(previous) => format!("{previous}; {message}"),
            None => message.into(),
        });
    }
    QueryResult {
        returned: lines.len(),
        lines,
        earlier_matches,
        truncated,
        byte_size,
        coverage_starts_at,
        hint,
        sources,
        byte_cut,
    }
}

/// The text of each existing segment, oldest first.
///
/// The current segment opens first. A rotation before the rotated one opens
/// makes both handles one file, which is then read once, so no segment is lost.
fn read_segments(dir: &Path) -> std::io::Result<Vec<String>> {
    let directory = open_directory(dir)?;
    let current = open_segment(&directory, dir, writer::FILE_NAME)?;
    let rotated = open_segment(&directory, dir, writer::ROTATED_FILE_NAME)?;
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

fn open_directory(dir: &Path) -> std::io::Result<std::fs::File> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW);
    }
    options.open(dir)
}

/// Open relative to the checked directory handle so a concurrent rename cannot
/// redirect the scan. Nonblocking open lets us reject special files safely.
#[cfg(unix)]
fn open_segment(
    dir: &std::fs::File,
    _path: &Path,
    name: &str,
) -> std::io::Result<Option<std::fs::File>> {
    use std::os::fd::{AsRawFd, FromRawFd};
    let name = std::ffi::CString::new(name).expect("segment names contain no NUL");
    // SAFETY: the directory handle and NUL-terminated name live through openat.
    let fd = unsafe {
        libc::openat(
            dir.as_raw_fd(),
            name.as_ptr(),
            libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC,
        )
    };
    let file = if fd < 0 {
        Err(std::io::Error::last_os_error())
    } else {
        // SAFETY: openat returned a new descriptor owned by this function.
        Ok(unsafe { std::fs::File::from_raw_fd(fd) })
    };
    checked_segment(file)
}

#[cfg(not(unix))]
fn open_segment(
    _dir: &std::fs::File,
    path: &Path,
    name: &str,
) -> std::io::Result<Option<std::fs::File>> {
    checked_segment(std::fs::File::open(path.join(name)))
}

fn checked_segment(file: std::io::Result<std::fs::File>) -> std::io::Result<Option<std::fs::File>> {
    match file {
        Ok(file) if file.metadata()?.is_file() => Ok(Some(file)),
        Ok(_) => Err(std::io::Error::other(
            "diagnostic segment is not a regular file",
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
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
fn matching(
    query: &Query,
    ts: DateTime<Utc>,
    record: Record,
    source: Option<Source>,
) -> Option<QueryLine> {
    if ts < query.since || ts > query.until {
        return None;
    }
    if query.pid.is_some_and(|pid| pid != record.pid) {
        return None;
    }
    let label = sanitize::for_read(&record.label);
    let msg = record.render(Destination::Owner);
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
    Some(QueryLine {
        ts: record.ts,
        pid: record.pid,
        label,
        msg,
        source,
    })
}

fn hint(
    query: &Query,
    lines: &[QueryLine],
    earlier_matches: usize,
    truncated: bool,
    coverage_starts_at: &Option<String>,
) -> Option<String> {
    let predates = coverage_starts_at
        .as_deref()
        .and_then(|ts| DateTime::parse_from_rfc3339(ts).ok())
        .is_some_and(|oldest| query.since < oldest);
    let mut warnings = Vec::new();
    if predates {
        warnings.push(if lines.is_empty() && earlier_matches == 0 {
            "no match; the window starts before the oldest retained line".to_string()
        } else {
            "partial history; the window starts before the oldest retained line".to_string()
        });
    }
    if earlier_matches > 0 {
        let cause = if truncated { "byte_limit" } else { "limit" };
        warnings.push(format!(
            "{earlier_matches} earlier matches left out by {cause}; narrow the filters or move `until` back"
        ));
    }
    (!warnings.is_empty()).then(|| warnings.join("; "))
}

#[cfg(test)]
#[path = "query_tests.rs"]
mod tests;
