//! The assertion vocabulary: everything a probe can say without a judge.
//!
//! ADR 0087 decision 6 makes scoring programmatic first, so this is the closed
//! set of things the harness can check. A probe declares one assertion or it
//! declares `judge`, never both (I9). Adding a variant here is how a new probe
//! becomes expressible; asking the judge instead is what I9 forbids.
//!
//! Every variant reads three sources and nothing else: the arm's `data/` tree,
//! the thread's rows from the event store, and the thread's final response.

use std::path::{Path, PathBuf};

use regex::Regex;
use serde::{Deserialize, Serialize};

type Fallible<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

/// One `ToolCalled` row, flattened to what an assertion asks about.
#[derive(Debug, Clone)]
pub struct ToolCall {
    pub sequence: i64,
    pub name: String,
    /// The call's arguments, rendered as JSON text so an argument regex can
    /// match across nested keys without the probe naming a path into them.
    pub args: String,
}

/// One event-store row an assertion reads.
#[derive(Debug, Clone)]
pub struct EventRow {
    pub sequence: i64,
    pub event_type: String,
    pub payload: String,
    pub created: chrono::DateTime<chrono::Utc>,
}

/// Everything an assertion is allowed to see.
///
/// Events are workspace-wide and tool calls are this thread's. An event is
/// workspace state that outlives the thread that wrote it, so a later task
/// re-testing an earlier fact reads it. A tool call is behaviour, and only this
/// thread's behaviour is this thread's probe.
pub struct AssertionContext<'a> {
    /// The arm workspace's `data/` directory.
    pub data_dir: &'a Path,
    pub final_response: &'a str,
    pub tool_calls: &'a [ToolCall],
    pub events: &'a [EventRow],
    /// Sequence at which round 2 began, when the thread reached one. Used by
    /// the recovery window, which starts after round 1 by definition.
    pub round_two_sequence: Option<i64>,
    /// Sequence of the message that opened turn two, for a task with a
    /// follow-up. `None` on the one-turn tasks, which is most of them.
    pub followup_sequence: Option<i64>,
}

/// Where a tool-call count starts looking.
///
/// Round two used to be the only window on offer, and it is the wrong one for a
/// two-turn task. Round two begins early in turn ONE. A probe asking what the
/// model did after the turn boundary would read most of turn one as well.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum After {
    /// Every call the thread made.
    #[default]
    Start,
    /// Calls after round 1, which is the recovery window.
    RoundTwo,
    /// Calls after the follow-up prompt landed, which is turn two.
    Followup,
}

/// The stretch of one thread a count covers.
enum Window {
    /// Every call the thread made.
    Whole,
    /// Calls after this sequence.
    From(i64),
    /// A boundary the thread never reached. No call can sit after it.
    Never,
}

impl Window {
    fn holds(&self, sequence: i64) -> bool {
        match self {
            Window::Whole => true,
            Window::From(floor) => sequence > *floor,
            Window::Never => false,
        }
    }
}

impl AssertionContext<'_> {
    /// What `after` selects on this thread.
    ///
    /// A boundary the thread never reached bounds an empty window rather than
    /// the whole thread. A turn that never ran holds no calls, so counting
    /// every call instead would answer a question nobody asked.
    fn window(&self, after: After) -> Window {
        let boundary = match after {
            After::Start => return Window::Whole,
            After::RoundTwo => self.round_two_sequence,
            After::Followup => self.followup_sequence,
        };
        boundary.map_or(Window::Never, Window::From)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Assertion {
    /// Every listed assertion holds. Expresses "contains X and not Y" without
    /// splitting one probe into two.
    All { of: Vec<Assertion> },
    /// At least one listed assertion holds. Expresses a fact with two homes,
    /// such as a constraint stated either in a trigger intent or its knowhow.
    Any { of: Vec<Assertion> },
    /// At least one file matches the pattern, which may contain `*` and `**`.
    FileExists { path: String },
    /// Some matching file's text matches the regex.
    FileMatches { path: String, regex: String },
    /// No matching file's text matches the regex.
    FileNotMatches { path: String, regex: String },
    /// Some matching file's last non-empty line is this literal.
    FileEndsWith { path: String, literal: String },
    /// No markdown table in the file has more than `max` columns.
    MarkdownTableMaxColumns { path: String, max: usize },
    /// The number of matching files is within bounds.
    GlobCount {
        path: String,
        min: Option<usize>,
        max: Option<usize>,
    },
    /// A matching file was written after the newest event of this type.
    FileModifiedAfterEvent { path: String, event_type: String },
    /// Exactly this many events of the type exist in the workspace.
    EventCount { event_type: String, count: usize },
    /// At least one event of the type exists. Distinct from `EventCount` on
    /// purpose: a precondition asks whether something happened, and pinning it
    /// to a count would void a task because the agent did it twice.
    EventExists { event_type: String },
    /// Every regex matches at least one event payload of the type, and each
    /// matches a different one. That is what "eight events with the given
    /// payloads" means: eight distinct rows, not one row matching eight times.
    EventPayloadsCover {
        event_type: String,
        regexes: Vec<String>,
    },
    /// How many times a tool was called, optionally filtered by an argument
    /// regex and by the recovery window.
    ToolCallCount {
        tool: String,
        #[serde(default)]
        args_regex: Option<String>,
        #[serde(default)]
        min: Option<usize>,
        #[serde(default)]
        max: Option<usize>,
        #[serde(default)]
        after: After,
    },
    /// The thread's final response matches.
    ResponseMatches { regex: String },
    /// The thread's final response does not match.
    ResponseNotMatches { regex: String },
    /// The final response repeats a run of words from the file, which is how a
    /// real quotation is told from "the sub-thread finished".
    ResponseQuotesFile { path: String, min_words: usize },
}

impl Assertion {
    /// Compile every regex and glob, so evaluation cannot fail on a typo.
    ///
    /// Called at fixture load. A probe with an unparseable regex is a broken
    /// measurement, and finding that out mid-run would void a whole repeat.
    pub fn validate(&self) -> Fallible<()> {
        match self {
            Assertion::All { of } | Assertion::Any { of } => {
                of.iter().try_for_each(|a| a.validate())
            }
            Assertion::FileMatches { regex, .. }
            | Assertion::FileNotMatches { regex, .. }
            | Assertion::ResponseMatches { regex }
            | Assertion::ResponseNotMatches { regex } => {
                Regex::new(regex)?;
                Ok(())
            }
            Assertion::EventPayloadsCover { regexes, .. } => {
                for regex in regexes {
                    Regex::new(regex)?;
                }
                Ok(())
            }
            Assertion::ToolCallCount { args_regex, .. } => {
                if let Some(regex) = args_regex {
                    Regex::new(regex)?;
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }

    /// Evaluate against one thread's world.
    ///
    /// `Err` means the assertion could not be evaluated at all, which is a
    /// harness failure rather than a probe outcome. A missing file is `false`,
    /// because that is what the probe is asking about.
    pub fn evaluate(&self, ctx: &AssertionContext) -> Fallible<bool> {
        match self {
            Assertion::All { of } => {
                for assertion in of {
                    if !assertion.evaluate(ctx)? {
                        return Ok(false);
                    }
                }
                Ok(true)
            }
            Assertion::Any { of } => {
                for assertion in of {
                    if assertion.evaluate(ctx)? {
                        return Ok(true);
                    }
                }
                Ok(false)
            }
            Assertion::FileExists { path } => Ok(!matching_files(ctx.data_dir, path)?.is_empty()),
            Assertion::FileMatches { path, regex } => {
                let regex = Regex::new(regex)?;
                for file in matching_files(ctx.data_dir, path)? {
                    if regex.is_match(&read_text(&file)?) {
                        return Ok(true);
                    }
                }
                Ok(false)
            }
            Assertion::FileNotMatches { path, regex } => {
                let regex = Regex::new(regex)?;
                for file in matching_files(ctx.data_dir, path)? {
                    if regex.is_match(&read_text(&file)?) {
                        return Ok(false);
                    }
                }
                Ok(true)
            }
            Assertion::FileEndsWith { path, literal } => {
                for file in matching_files(ctx.data_dir, path)? {
                    let text = read_text(&file)?;
                    if text
                        .lines()
                        .rev()
                        .find(|l| !l.trim().is_empty())
                        .map(|l| l.trim() == literal)
                        .unwrap_or(false)
                    {
                        return Ok(true);
                    }
                }
                Ok(false)
            }
            Assertion::MarkdownTableMaxColumns { path, max } => {
                for file in matching_files(ctx.data_dir, path)? {
                    if widest_markdown_table(&read_text(&file)?) > *max {
                        return Ok(false);
                    }
                }
                Ok(true)
            }
            Assertion::GlobCount { path, min, max } => {
                let count = matching_files(ctx.data_dir, path)?.len();
                Ok(within(count, *min, *max))
            }
            Assertion::FileModifiedAfterEvent { path, event_type } => {
                let Some(event) = ctx
                    .events
                    .iter()
                    .filter(|e| &e.event_type == event_type)
                    .max_by_key(|e| e.sequence)
                else {
                    return Ok(false);
                };
                for file in matching_files(ctx.data_dir, path)? {
                    let modified: chrono::DateTime<chrono::Utc> =
                        std::fs::metadata(&file)?.modified()?.into();
                    if modified > event.created {
                        return Ok(true);
                    }
                }
                Ok(false)
            }
            Assertion::EventCount { event_type, count } => Ok(ctx
                .events
                .iter()
                .filter(|e| &e.event_type == event_type)
                .count()
                == *count),
            Assertion::EventExists { event_type } => {
                Ok(ctx.events.iter().any(|e| &e.event_type == event_type))
            }
            Assertion::EventPayloadsCover {
                event_type,
                regexes,
            } => {
                let payloads: Vec<&str> = ctx
                    .events
                    .iter()
                    .filter(|e| &e.event_type == event_type)
                    .map(|e| e.payload.as_str())
                    .collect();
                let mut claimed = vec![false; payloads.len()];
                for regex in regexes {
                    let regex = Regex::new(regex)?;
                    let Some(index) = payloads
                        .iter()
                        .enumerate()
                        .position(|(i, p)| !claimed[i] && regex.is_match(p))
                    else {
                        return Ok(false);
                    };
                    claimed[index] = true;
                }
                Ok(true)
            }
            Assertion::ToolCallCount {
                tool,
                args_regex,
                min,
                max,
                after,
            } => {
                let window = ctx.window(*after);
                let compiled = args_regex.as_deref().map(Regex::new).transpose()?;
                let count = ctx
                    .tool_calls
                    .iter()
                    .filter(|call| &call.name == tool)
                    .filter(|call| window.holds(call.sequence))
                    .filter(|call| compiled.as_ref().is_none_or(|r| r.is_match(&call.args)))
                    .count();
                Ok(within(count, *min, *max))
            }
            Assertion::ResponseMatches { regex } => {
                Ok(Regex::new(regex)?.is_match(ctx.final_response))
            }
            Assertion::ResponseNotMatches { regex } => {
                Ok(!Regex::new(regex)?.is_match(ctx.final_response))
            }
            Assertion::ResponseQuotesFile { path, min_words } => {
                for file in matching_files(ctx.data_dir, path)? {
                    if quotes_a_run(&read_text(&file)?, ctx.final_response, *min_words) {
                        return Ok(true);
                    }
                }
                Ok(false)
            }
        }
    }
}

fn within(count: usize, min: Option<usize>, max: Option<usize>) -> bool {
    min.is_none_or(|m| count >= m) && max.is_none_or(|m| count <= m)
}

fn read_text(path: &Path) -> Fallible<String> {
    Ok(String::from_utf8_lossy(&std::fs::read(path)?).into_owned())
}

/// Files under `root` whose relative path matches `pattern`.
///
/// `*` matches within one path segment and `**` matches any number of them.
/// Nothing else is special, so a literal path is its own pattern.
pub fn matching_files(root: &Path, pattern: &str) -> Fallible<Vec<PathBuf>> {
    let mut found = Vec::new();
    walk(root, root, &mut |relative, absolute| {
        if glob_match(pattern, relative) {
            found.push(absolute.to_path_buf());
        }
    })?;
    found.sort();
    Ok(found)
}

fn walk(root: &Path, dir: &Path, visit: &mut impl FnMut(&str, &Path)) -> Fallible<()> {
    if !dir.exists() {
        return Ok(());
    }
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if entry.file_type()?.is_dir() {
            walk(root, &path, visit)?;
        } else if let Ok(relative) = path.strip_prefix(root) {
            visit(&relative.to_string_lossy(), &path);
        }
    }
    Ok(())
}

/// Segment-wise glob over a relative path.
pub fn glob_match(pattern: &str, path: &str) -> bool {
    let pattern: Vec<&str> = pattern.split('/').collect();
    let path: Vec<&str> = path.split('/').collect();
    segments_match(&pattern, &path)
}

fn segments_match(pattern: &[&str], path: &[&str]) -> bool {
    match pattern.first() {
        None => path.is_empty(),
        Some(&"**") => (0..=path.len()).any(|skip| segments_match(&pattern[1..], &path[skip..])),
        Some(head) => match path.first() {
            Some(segment) if segment_match(head, segment) => {
                segments_match(&pattern[1..], &path[1..])
            }
            _ => false,
        },
    }
}

/// `*` inside one segment, matching any run of characters including none.
fn segment_match(pattern: &str, segment: &str) -> bool {
    let mut parts = pattern.split('*');
    let Some(first) = parts.next() else {
        return pattern == segment;
    };
    if !segment.starts_with(first) {
        return false;
    }
    let mut rest = &segment[first.len()..];
    let parts: Vec<&str> = parts.collect();
    for (index, part) in parts.iter().enumerate() {
        let last = index + 1 == parts.len();
        if last {
            return rest.ends_with(part);
        }
        match rest.find(part) {
            Some(at) => rest = &rest[at + part.len()..],
            None => return false,
        }
    }
    // Only reachable when the pattern held no `*`, in which case the whole
    // segment had to be consumed. Returning `true` made a literal pattern a
    // PREFIX match: `conventions.md` was satisfied by `conventions.md.bak`.
    rest.is_empty()
}

/// Widest markdown table row in the text, counted in columns.
///
/// A row is a line starting and ending with a pipe. Columns are the cells
/// between the outer pipes, so `| a | b |` is two.
pub fn widest_markdown_table(text: &str) -> usize {
    text.lines()
        .map(str::trim)
        .filter(|line| line.starts_with('|') && line.ends_with('|') && line.len() > 1)
        .map(|line| line.trim_matches('|').split('|').count())
        .max()
        .unwrap_or(0)
}

/// Whether `response` repeats a run of at least `min_words` words from `source`.
pub fn quotes_a_run(source: &str, response: &str, min_words: usize) -> bool {
    let normalize = |text: &str| -> Vec<String> {
        text.split_whitespace()
            .map(|word| {
                word.chars()
                    .filter(|c| c.is_alphanumeric())
                    .flat_map(char::to_lowercase)
                    .collect::<String>()
            })
            .filter(|word| !word.is_empty())
            .collect()
    };
    let source = normalize(source);
    let response = normalize(response);
    if min_words == 0 || source.len() < min_words || response.len() < min_words {
        return false;
    }
    source
        .windows(min_words)
        .any(|run| response.windows(min_words).any(|other| other == run))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx<'a>(data_dir: &'a Path, response: &'a str) -> AssertionContext<'a> {
        AssertionContext {
            data_dir,
            final_response: response,
            tool_calls: &[],
            events: &[],
            round_two_sequence: None,
            followup_sequence: None,
        }
    }

    struct TempTree(PathBuf);

    impl TempTree {
        fn new(label: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "lucidos-eval-{label}-{}",
                uuid::Uuid::new_v4().simple()
            ));
            std::fs::create_dir_all(&path).unwrap();
            TempTree(path)
        }

        fn write(&self, relative: &str, contents: &str) {
            let path = self.0.join(relative);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, contents).unwrap();
        }
    }

    impl Drop for TempTree {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn a_literal_pattern_is_not_a_prefix_match() {
        assert!(glob_match("a/conventions.md", "a/conventions.md"));
        assert!(!glob_match("a/conventions.md", "a/conventions.md.bak"));
        assert!(!glob_match("a/manifest.json", "a/manifest.json.old"));
        // A trailing star still means "anything after this".
        assert!(glob_match("a/conventions.md*", "a/conventions.md.bak"));
    }

    #[test]
    fn a_glob_matches_within_and_across_segments() {
        assert!(glob_match(
            "apps/*/manifest.json",
            "apps/build-health/manifest.json"
        ));
        assert!(!glob_match(
            "apps/*/manifest.json",
            "apps/a/b/manifest.json"
        ));
        assert!(glob_match("apps/**/*.css", "apps/a/b/style.css"));
        assert!(glob_match(
            "artifacts/*-collected.json",
            "artifacts/example-repo-collected.json"
        ));
        assert!(!glob_match(
            "artifacts/*-collected.json",
            "artifacts/report.md"
        ));
    }

    #[test]
    fn table_width_counts_cells_between_the_outer_pipes() {
        let text = "| a | b |\n|---|---|\n| 1 | 2 |\n";
        assert_eq!(widest_markdown_table(text), 2);
        assert_eq!(widest_markdown_table("| a | b | c | d | e |"), 5);
        assert_eq!(widest_markdown_table("no table here"), 0);
    }

    #[test]
    fn the_column_cap_fails_on_a_wider_table() {
        let tree = TempTree::new("columns");
        tree.write("r.md", "| a | b | c | d | e |\n|-|-|-|-|-|\n");
        let assertion = Assertion::MarkdownTableMaxColumns {
            path: "r.md".into(),
            max: 4,
        };
        assert!(!assertion.evaluate(&ctx(&tree.0, "")).unwrap());
    }

    #[test]
    fn all_composes_a_contains_and_a_does_not_contain() {
        let tree = TempTree::new("all");
        tree.write("r.md", "Build Health report: 37.5% failed\n");
        let assertion = Assertion::All {
            of: vec![
                Assertion::FileMatches {
                    path: "r.md".into(),
                    regex: "Build Health".into(),
                },
                Assertion::FileNotMatches {
                    path: "r.md".into(),
                    regex: "CI Health".into(),
                },
            ],
        };
        assert!(assertion.evaluate(&ctx(&tree.0, "")).unwrap());
    }

    #[test]
    fn a_missing_file_is_false_and_not_an_error() {
        let tree = TempTree::new("missing");
        let assertion = Assertion::FileExists {
            path: "nope.md".into(),
        };
        assert!(!assertion.evaluate(&ctx(&tree.0, "")).unwrap());
    }

    #[test]
    fn ends_with_ignores_a_trailing_blank_line() {
        let tree = TempTree::new("endswith");
        tree.write("w.md", "body\nprepared-by: sub\n\n");
        let assertion = Assertion::FileEndsWith {
            path: "w.md".into(),
            literal: "prepared-by: sub".into(),
        };
        assert!(assertion.evaluate(&ctx(&tree.0, "")).unwrap());
    }

    fn memory_calls() -> [ToolCall; 3] {
        [1i64, 9, 21].map(|sequence| ToolCall {
            sequence,
            name: "memory".into(),
            args: "{\"query\":\"schedule\"}".into(),
        })
    }

    fn counting(min: usize, max: usize, after: After) -> Assertion {
        Assertion::ToolCallCount {
            tool: "memory".into(),
            args_regex: None,
            min: Some(min),
            max: Some(max),
            after,
        }
    }

    /// Three windows over one thread. Round two starts at 5 and the follow-up
    /// lands at 15, so each window sees a different slice of the same calls.
    #[test]
    fn each_window_counts_only_the_calls_that_fall_inside_it() {
        let calls = memory_calls();
        let context = AssertionContext {
            data_dir: Path::new("/nonexistent"),
            final_response: "",
            tool_calls: &calls,
            events: &[],
            round_two_sequence: Some(5),
            followup_sequence: Some(15),
        };
        assert!(counting(3, 3, After::Start).evaluate(&context).unwrap());
        assert!(counting(2, 2, After::RoundTwo).evaluate(&context).unwrap());
        assert!(counting(1, 1, After::Followup).evaluate(&context).unwrap());
    }

    /// A turn that never ran holds no calls. Counting the whole thread instead
    /// would let a `max` probe pass on the rounds it exists to exclude.
    #[test]
    fn a_window_the_thread_never_reached_is_empty_and_not_the_whole_thread() {
        let calls = memory_calls();
        let context = AssertionContext {
            data_dir: Path::new("/nonexistent"),
            final_response: "",
            tool_calls: &calls,
            events: &[],
            round_two_sequence: None,
            followup_sequence: None,
        };
        assert!(counting(0, 0, After::RoundTwo).evaluate(&context).unwrap());
        assert!(counting(0, 0, After::Followup).evaluate(&context).unwrap());
        assert!(counting(3, 3, After::Start).evaluate(&context).unwrap());
    }

    /// The default is the whole thread, so a probe writing no `after` key
    /// keeps counting what it counted before the windows existed.
    #[test]
    fn a_probe_naming_no_window_counts_the_whole_thread() {
        let assertion: Assertion =
            serde_json::from_str(r#"{"kind":"tool_call_count","tool":"memory","min":3,"max":3}"#)
                .unwrap();
        let calls = memory_calls();
        let context = AssertionContext {
            data_dir: Path::new("/nonexistent"),
            final_response: "",
            tool_calls: &calls,
            events: &[],
            round_two_sequence: Some(5),
            followup_sequence: Some(15),
        };
        assert!(assertion.evaluate(&context).unwrap());
    }

    #[test]
    fn payload_cover_needs_a_distinct_row_per_regex() {
        let events: Vec<EventRow> = ["pass 412", "pass 398"]
            .iter()
            .enumerate()
            .map(|(i, body)| EventRow {
                sequence: i as i64,
                event_type: "BuildObserved".into(),
                payload: (*body).into(),
                created: chrono::Utc::now(),
            })
            .collect();
        let context = AssertionContext {
            data_dir: Path::new("/nonexistent"),
            final_response: "",
            tool_calls: &[],
            events: &events,
            round_two_sequence: None,
            followup_sequence: None,
        };
        let distinct = Assertion::EventPayloadsCover {
            event_type: "BuildObserved".into(),
            regexes: vec!["412".into(), "398".into()],
        };
        assert!(distinct.evaluate(&context).unwrap());
        let overlapping = Assertion::EventPayloadsCover {
            event_type: "BuildObserved".into(),
            regexes: vec!["pass".into(), "pass".into(), "pass".into()],
        };
        assert!(!overlapping.evaluate(&context).unwrap());
    }

    #[test]
    fn a_quotation_needs_a_run_of_words_and_not_a_shared_one() {
        assert!(quotes_a_run(
            "The failure rate rose on Monday and fell again.",
            "It reports that the failure rate rose on Monday.",
            5
        ));
        assert!(!quotes_a_run(
            "The failure rate rose on Monday.",
            "The sub-thread finished its work.",
            5
        ));
    }

    #[test]
    fn any_passes_when_one_of_two_homes_carries_the_fact() {
        let tree = TempTree::new("any");
        tree.write(
            "knowhow/collection.md",
            "never notify between 22:00 and 07:00\n",
        );
        let assertion = Assertion::Any {
            of: vec![
                Assertion::FileMatches {
                    path: "intent.md".into(),
                    regex: "22:00".into(),
                },
                Assertion::FileMatches {
                    path: "knowhow/**/*.md".into(),
                    regex: "22:00".into(),
                },
            ],
        };
        assert!(assertion.evaluate(&ctx(&tree.0, "")).unwrap());
    }

    #[test]
    fn any_fails_when_no_home_carries_it() {
        let tree = TempTree::new("any-empty");
        tree.write("knowhow/collection.md", "collect the data\n");
        let assertion = Assertion::Any {
            of: vec![Assertion::FileMatches {
                path: "knowhow/**/*.md".into(),
                regex: "22:00".into(),
            }],
        };
        assert!(!assertion.evaluate(&ctx(&tree.0, "")).unwrap());
    }

    #[test]
    fn a_bad_regex_is_caught_at_validation() {
        let assertion = Assertion::ResponseMatches {
            regex: "(unclosed".into(),
        };
        assert!(assertion.validate().is_err());
    }
}
