//! *Text search*: a literal phrase inside workspace files, for Search
//! Everywhere's Text category. A live parallel scan of the four browseable data
//! roots, with no index (ADR 0383).

use std::cmp::Ordering;
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};
use std::time::SystemTime;

use rayon::prelude::*;
use regex::{Regex, RegexBuilder};
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use crate::core::list_user_data_files;
use crate::engine::title_match::{title_match, TitleMatch};
use crate::engine::tools::search::{cap_chars, looks_binary, BINARY_SNIFF_BYTES, ELLIPSIS};

const MIN_QUERY_CHARS: usize = 3;
const MAX_FILE_BYTES: u64 = 2 * 1024 * 1024;
/// The Text section's share of the All tab.
const PREVIEW_LINES: usize = 5;
const PREVIEW_LINES_PER_FILE: usize = 3;
/// Preview mode ranks the lines it found once it holds this many.
const PREVIEW_CANDIDATE_BUDGET: usize = 200;
const ALL_LINES_CAP: usize = 5_000;
const SNIPPET_BEFORE_CHARS: usize = 32;
const SNIPPET_AFTER_CHARS: usize = 160;
/// Files scanned in parallel between two stop checks.
const SCAN_CHUNK_FILES: usize = 256;

/// `preview` answers the All tab: a few of the best lines, found fast.
/// `all` answers the Text tab: every line, up to [`ALL_LINES_CAP`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum TextSearchMode {
    Preview,
    All,
}

impl TextSearchMode {
    /// Lines the scan collects, across every file, before it stops.
    fn line_budget(self) -> usize {
        match self {
            Self::Preview => PREVIEW_CANDIDATE_BUDGET,
            Self::All => ALL_LINES_CAP,
        }
    }

    /// Lines the response returns, best first.
    fn returned_lines(self) -> usize {
        match self {
            Self::Preview => PREVIEW_LINES,
            Self::All => ALL_LINES_CAP,
        }
    }
}

/// One matching line. `before` and `after` are its context, cut to a few
/// dozen characters with an ellipsis where cut. The three strings concatenate
/// to the visible snippet, so a client needs no offsets to highlight `matched`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct TextSearchHit {
    pub(crate) path: String,
    /// 1-based.
    pub(crate) line: usize,
    pub(crate) before: String,
    pub(crate) matched: String,
    pub(crate) after: String,
}

#[derive(Debug, PartialEq, Serialize)]
#[serde(tag = "status", rename_all = "kebab-case")]
pub(crate) enum TextSearchResponse {
    QueryTooShort {
        min_query_chars: usize,
    },
    #[serde(rename = "ok")]
    Searched {
        hits: Vec<TextSearchHit>,
        /// The scan found or skipped lines it did not return: a cap cut them,
        /// preview mode stopped early, or the request was cancelled.
        truncated: bool,
        /// Text files over [`MAX_FILE_BYTES`], not searched.
        skipped_large_files: usize,
    },
}

/// Searches the workspace's browseable data files. `cancelled` is polled per
/// file, so a dropped request stops reading.
pub(crate) fn search_workspace_text(
    workspace_path: &Path,
    query: &str,
    mode: TextSearchMode,
    cancelled: &CancellationToken,
) -> Result<TextSearchResponse, Box<dyn std::error::Error + Send + Sync>> {
    let query = query.trim();
    if query.chars().count() < MIN_QUERY_CHARS {
        return Ok(TextSearchResponse::QueryTooShort {
            min_query_chars: MIN_QUERY_CHARS,
        });
    }
    // A pasted wall of text can exceed the regex size limit.
    let matcher = RegexBuilder::new(&regex::escape(query))
        .case_insensitive(true)
        .build()?;
    let entries = list_user_data_files(workspace_path)?;
    Ok(search_entries(entries, &matcher, query, mode, cancelled))
}

struct Candidate {
    path: String,
    abs: PathBuf,
    modified: SystemTime,
}

struct RankedHit {
    level: TitleMatch,
    modified: SystemTime,
    hit: TextSearchHit,
}

fn best_first(a: &RankedHit, b: &RankedHit) -> Ordering {
    b.level
        .cmp(&a.level)
        .then_with(|| b.modified.cmp(&a.modified))
        .then_with(|| a.hit.path.cmp(&b.hit.path))
        .then_with(|| a.hit.line.cmp(&b.hit.line))
}

/// One file's lines, and whether it had matching lines the scan left out.
struct FileScan {
    hits: Vec<RankedHit>,
    cut: bool,
}

fn search_entries(
    entries: Vec<(String, PathBuf)>,
    matcher: &Regex,
    query: &str,
    mode: TextSearchMode,
    cancelled: &CancellationToken,
) -> TextSearchResponse {
    let (mut candidates, skipped_large_files) = triage(entries, cancelled);
    // Newest first, so an early stop keeps the files the user touched last.
    candidates.sort_by(|a, b| b.modified.cmp(&a.modified).then(a.path.cmp(&b.path)));

    // Shared by the parallel scans, so memory stays bounded by the budget
    // rather than by the budget times the files in a chunk.
    let collected = AtomicUsize::new(0);
    let mut found: Vec<RankedHit> = Vec::new();
    let mut truncated = false;
    for (i, chunk) in candidates.chunks(SCAN_CHUNK_FILES).enumerate() {
        if cancelled.is_cancelled() {
            break;
        }
        let scans: Vec<FileScan> = chunk
            .par_iter()
            .map(|file| scan_file(file, matcher, query, mode, &collected, cancelled))
            .collect();
        for scan in scans {
            truncated |= scan.cut;
            found.extend(scan.hits);
        }
        let scanned_all = (i + 1) * SCAN_CHUNK_FILES >= candidates.len();
        if collected.load(AtomicOrdering::Relaxed) >= mode.line_budget() && !scanned_all {
            truncated = true;
            break;
        }
    }

    found.sort_by(best_first);
    let keep = mode.returned_lines();
    truncated |= found.len() > keep || cancelled.is_cancelled();
    found.truncate(keep);
    TextSearchResponse::Searched {
        hits: found.into_iter().map(|r| r.hit).collect(),
        truncated,
        skipped_large_files,
    }
}

enum Triaged {
    Scan(Candidate),
    TooLargeText,
    Skip,
}

/// Splits the entries into files to scan and a count of text files too large
/// to scan. A file that vanished or cannot be read since the walk is skipped:
/// the walk and the scan race every write.
fn triage(
    entries: Vec<(String, PathBuf)>,
    cancelled: &CancellationToken,
) -> (Vec<Candidate>, usize) {
    let triaged: Vec<Triaged> = entries
        .into_par_iter()
        .map(|(path, abs)| {
            if cancelled.is_cancelled() {
                return Triaged::Skip;
            }
            let Ok(meta) = fs::metadata(&abs) else {
                return Triaged::Skip;
            };
            if meta.len() <= MAX_FILE_BYTES {
                let modified = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
                return Triaged::Scan(Candidate {
                    path,
                    abs,
                    modified,
                });
            }
            match read_head(&abs) {
                Ok((head, _)) if !looks_binary(&head) => Triaged::TooLargeText,
                _ => Triaged::Skip,
            }
        })
        .collect();
    let mut candidates = Vec::new();
    let mut skipped_large = 0;
    for t in triaged {
        match t {
            Triaged::Scan(c) => candidates.push(c),
            Triaged::TooLargeText => skipped_large += 1,
            Triaged::Skip => {}
        }
    }
    (candidates, skipped_large)
}

/// The first [`BINARY_SNIFF_BYTES`] of a file, and the open file positioned
/// after them, so a binary is rejected without reading the rest.
fn read_head(abs: &Path) -> std::io::Result<(Vec<u8>, File)> {
    let mut file = File::open(abs)?;
    let mut head = Vec::with_capacity(BINARY_SNIFF_BYTES);
    (&mut file)
        .take(BINARY_SNIFF_BYTES as u64)
        .read_to_end(&mut head)?;
    Ok((head, file))
}

/// Every matching line of one file, one hit per line, while the search's
/// shared line budget lasts. Preview mode keeps the file's best
/// [`PREVIEW_LINES_PER_FILE`].
fn scan_file(
    file: &Candidate,
    matcher: &Regex,
    query: &str,
    mode: TextSearchMode,
    collected: &AtomicUsize,
    cancelled: &CancellationToken,
) -> FileScan {
    let nothing = FileScan {
        hits: Vec::new(),
        cut: false,
    };
    if cancelled.is_cancelled() {
        return nothing;
    }
    if collected.load(AtomicOrdering::Relaxed) >= mode.line_budget() {
        return FileScan {
            hits: Vec::new(),
            cut: true,
        };
    }
    let Ok((mut bytes, mut rest)) = read_head(&file.abs) else {
        return nothing;
    };
    if looks_binary(&bytes) || rest.read_to_end(&mut bytes).is_err() {
        return nothing;
    }
    let Ok(text) = std::str::from_utf8(&bytes) else {
        return nothing;
    };

    let mut hits = Vec::new();
    let mut cut = false;
    let mut line = 1;
    let mut counted_to = 0;
    let mut line_end = 0;
    for m in matcher.find_iter(text) {
        if m.start() < line_end {
            continue;
        }
        if collected.fetch_add(1, AtomicOrdering::Relaxed) >= mode.line_budget() {
            cut = true;
            break;
        }
        line += text[counted_to..m.start()].matches('\n').count();
        counted_to = m.start();
        let line_start = text[..m.start()].rfind('\n').map_or(0, |i| i + 1);
        line_end = text[m.start()..]
            .find('\n')
            .map_or(text.len(), |i| m.start() + i);
        let line_text = text[line_start..line_end].trim_end_matches('\r');
        let matched_end = m.end().min(line_start + line_text.len());
        hits.push(RankedHit {
            level: title_match(line_text, query),
            modified: file.modified,
            hit: TextSearchHit {
                path: file.path.clone(),
                line,
                before: cut_before(text[line_start..m.start()].trim_start()),
                matched: text[m.start()..matched_end].to_string(),
                after: cap_chars(&line_text[matched_end - line_start..], SNIPPET_AFTER_CHARS),
            },
        });
    }
    if mode == TextSearchMode::Preview && hits.len() > PREVIEW_LINES_PER_FILE {
        hits.sort_by(best_first);
        hits.truncate(PREVIEW_LINES_PER_FILE);
        cut = true;
    }
    FileScan { hits, cut }
}

/// The last [`SNIPPET_BEFORE_CHARS`] characters, led by [`ELLIPSIS`] if cut.
fn cut_before(s: &str) -> String {
    let count = s.chars().count();
    if count <= SNIPPET_BEFORE_CHARS {
        return s.to_string();
    }
    std::iter::once(ELLIPSIS)
        .chain(s.chars().skip(count - SNIPPET_BEFORE_CHARS))
        .collect()
}

#[cfg(test)]
#[path = "text_search_tests.rs"]
mod tests;
