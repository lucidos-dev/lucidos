use super::*;
use std::fs::{self, File};
use std::path::Path;
use std::time::{Duration, SystemTime};
use tempfile::TempDir;
use tokio_util::sync::CancellationToken;

/// A data/ tree under a temp workspace. Each file gets an mtime of
/// `base + age_rank` seconds in the past, so a larger rank is older.
struct Workspace {
    dir: TempDir,
}

impl Workspace {
    fn new() -> Self {
        Self {
            dir: TempDir::new().unwrap(),
        }
    }

    fn write(&self, rel: &str, bytes: impl AsRef<[u8]>) -> &Self {
        self.write_aged(rel, bytes, 0)
    }

    fn write_aged(&self, rel: &str, bytes: impl AsRef<[u8]>, age_secs: u64) -> &Self {
        let path = self.dir.path().join("data").join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, bytes).unwrap();
        let modified = SystemTime::now() - Duration::from_secs(1_000 + age_secs);
        File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(modified)
            .unwrap();
        self
    }

    fn root(&self) -> &Path {
        self.dir.path()
    }

    fn search(&self, query: &str, mode: TextSearchMode) -> TextSearchResponse {
        search_workspace_text(self.root(), query, mode, &CancellationToken::new()).unwrap()
    }

    fn hits(&self, query: &str, mode: TextSearchMode) -> Vec<TextSearchHit> {
        match self.search(query, mode) {
            TextSearchResponse::Searched { hits, .. } => hits,
            other => panic!("expected hits, got {other:?}"),
        }
    }
}

fn located(hits: &[TextSearchHit]) -> Vec<(&str, usize)> {
    hits.iter().map(|h| (h.path.as_str(), h.line)).collect()
}

#[test]
fn a_query_below_the_minimum_is_refused_with_the_minimum() {
    let ws = Workspace::new();
    ws.write("artifacts/a.md", "ab ab ab");
    assert_eq!(
        ws.search(" ab ", TextSearchMode::All),
        TextSearchResponse::QueryTooShort {
            min_query_chars: MIN_QUERY_CHARS
        }
    );
}

#[test]
fn metacharacters_match_only_themselves() {
    let ws = Workspace::new();
    ws.write(
        "artifacts/a.md",
        "a.b( literal\naxb( decoy\n[x] literal\nx decoy\n\\d+ literal\n42 decoy\n",
    );
    for query in ["a.b(", "[x]", "\\d+"] {
        let hits = ws.hits(query, TextSearchMode::All);
        assert_eq!(hits.len(), 1, "{query}");
        assert_eq!(hits[0].matched, query);
    }
}

#[test]
fn matching_ignores_case_beyond_ascii() {
    let ws = Workspace::new();
    ws.write("knowhow/no.md", "Svaret er ÆRLIG talt nei\nÉLAN vital\n");
    assert_eq!(ws.hits("ærlig", TextSearchMode::All)[0].matched, "ÆRLIG");
    assert_eq!(ws.hits("élan", TextSearchMode::All)[0].matched, "ÉLAN");
}

/// Simple case folding is the contract: one character never matches two.
#[test]
fn case_folding_never_expands_one_character_into_two() {
    let ws = Workspace::new();
    ws.write("artifacts/de.md", "STRASSE\n");
    assert!(ws.hits("straße", TextSearchMode::All).is_empty());
}

#[test]
fn only_the_four_browseable_roots_are_read_and_never_build_output() {
    let ws = Workspace::new();
    ws.write("artifacts/keep.md", "needle here")
        .write("apps/demo/index.html", "needle here")
        .write("apps/demo/node_modules/pkg/index.js", "needle here")
        .write("apps/demo/dist/bundle.js", "needle here")
        .write("config/apis.json", "needle here")
        .write("knowhow/k.md", "needle here")
        .write("triggers/t/trigger.json", "needle here");
    let mut paths: Vec<_> = ws
        .hits("needle", TextSearchMode::All)
        .into_iter()
        .map(|h| h.path)
        .collect();
    paths.sort();
    assert_eq!(
        paths,
        [
            "apps/demo/index.html",
            "artifacts/keep.md",
            "knowhow/k.md",
            "triggers/t/trigger.json"
        ]
    );
}

#[test]
fn binary_and_invalid_utf8_files_are_skipped() {
    let ws = Workspace::new();
    ws.write("artifacts/bin.dat", b"needle\0needle".as_slice())
        .write("artifacts/latin1.txt", b"needle caf\xe9".as_slice())
        .write("artifacts/ok.txt", "needle");
    assert_eq!(
        located(&ws.hits("needle", TextSearchMode::All)),
        [("artifacts/ok.txt", 1)]
    );
}

#[test]
fn large_text_files_are_skipped_and_counted_but_large_binaries_are_not_counted() {
    let ws = Workspace::new();
    let big = MAX_FILE_BYTES as usize + 1;
    let mut text = "needle\n".repeat(big / 7 + 1);
    text.truncate(big);
    let mut binary = vec![0u8; MAX_FILE_BYTES as usize + 1];
    binary[..6].copy_from_slice(b"needle");
    ws.write("artifacts/big.txt", text)
        .write("artifacts/big.bin", binary)
        .write("artifacts/small.txt", "needle");
    assert_eq!(
        ws.search("needle", TextSearchMode::All),
        TextSearchResponse::Searched {
            hits: vec![TextSearchHit {
                path: "artifacts/small.txt".into(),
                line: 1,
                before: String::new(),
                matched: "needle".into(),
                after: String::new(),
            }],
            truncated: false,
            skipped_large_files: 1,
        }
    );
}

#[test]
fn line_numbers_are_one_based_for_lf_crlf_and_a_last_line_with_no_newline() {
    let ws = Workspace::new();
    ws.write("artifacts/lf.md", "needle first\nmiddle\nlast needle")
        .write_aged("artifacts/crlf.md", "one\r\ntwo needle\r\nthree\r\n", 1);
    let hits = ws.hits("needle", TextSearchMode::All);
    assert_eq!(
        located(&hits),
        [
            ("artifacts/lf.md", 1),
            ("artifacts/lf.md", 3),
            ("artifacts/crlf.md", 2)
        ]
    );
    let crlf = &hits[2];
    assert_eq!((crlf.before.as_str(), crlf.after.as_str()), ("two ", ""));
}

#[test]
fn one_line_with_several_matches_is_one_hit() {
    let ws = Workspace::new();
    ws.write("artifacts/a.md", "needle and needle and NEEDLE\n");
    assert_eq!(
        located(&ws.hits("needle", TextSearchMode::All)),
        [("artifacts/a.md", 1)]
    );
}

#[test]
fn a_match_deep_in_a_long_line_keeps_the_match_and_cuts_the_context() {
    let ws = Workspace::new();
    let line = format!("{}needle{}", "x".repeat(50_000), "y".repeat(50_000));
    ws.write("apps/demo/bundle.min.js", line);
    let hit = &ws.hits("needle", TextSearchMode::All)[0];
    assert_eq!(hit.matched, "needle");
    assert_eq!(
        hit.before,
        format!("{ELLIPSIS}{}", "x".repeat(SNIPPET_BEFORE_CHARS))
    );
    assert_eq!(
        hit.after,
        format!("{}{ELLIPSIS}", "y".repeat(SNIPPET_AFTER_CHARS))
    );
}

#[test]
fn snippets_are_cut_on_character_boundaries() {
    let ws = Workspace::new();
    let line = format!("{}needle{}", "ø".repeat(100), "å".repeat(300));
    ws.write("artifacts/no.md", line);
    let hit = &ws.hits("needle", TextSearchMode::All)[0];
    assert_eq!(
        hit.before,
        format!("{ELLIPSIS}{}", "ø".repeat(SNIPPET_BEFORE_CHARS))
    );
    assert_eq!(
        hit.after,
        format!("{}{ELLIPSIS}", "å".repeat(SNIPPET_AFTER_CHARS))
    );
}

#[test]
fn leading_indentation_is_not_context() {
    let ws = Workspace::new();
    ws.write("apps/demo/app.js", "    \tconst needle = 1;\n");
    let hit = &ws.hits("needle", TextSearchMode::All)[0];
    assert_eq!(
        (hit.before.as_str(), hit.after.as_str()),
        ("const ", " = 1;")
    );
}

#[test]
fn ranking_is_match_level_then_newest_file_then_path_then_line() {
    let ws = Workspace::new();
    ws.write_aged("artifacts/old-word.md", "the needle\n", 30)
        .write_aged("artifacts/new-inside.md", "haystackneedle\n", 0)
        .write_aged("artifacts/mid-word.md", "a needle\nanother needle\n", 10)
        .write_aged("artifacts/exact.md", "needle\n", 40)
        .write_aged("artifacts/b-tie.md", "needle words\n", 20)
        .write_aged("artifacts/a-tie.md", "needle words\n", 20);
    assert_eq!(
        located(&ws.hits("needle", TextSearchMode::All)),
        [
            ("artifacts/exact.md", 1),
            ("artifacts/mid-word.md", 1),
            ("artifacts/mid-word.md", 2),
            ("artifacts/a-tie.md", 1),
            ("artifacts/b-tie.md", 1),
            ("artifacts/old-word.md", 1),
            ("artifacts/new-inside.md", 1),
        ]
    );
}

#[test]
fn preview_caps_lines_per_file_and_in_total_and_says_there_is_more() {
    let ws = Workspace::new();
    ws.write("artifacts/many.md", "needle\n".repeat(10))
        .write_aged("artifacts/more.md", "needle\n".repeat(10), 1);
    let TextSearchResponse::Searched {
        hits, truncated, ..
    } = ws.search("needle", TextSearchMode::Preview)
    else {
        panic!("expected hits");
    };
    assert_eq!(
        located(&hits),
        [
            ("artifacts/many.md", 1),
            ("artifacts/many.md", 2),
            ("artifacts/many.md", 3),
            ("artifacts/more.md", 1),
            ("artifacts/more.md", 2),
        ]
    );
    assert_eq!(hits.len(), PREVIEW_LINES);
    assert!(truncated);
}

#[test]
fn preview_says_there_is_more_when_one_file_holds_more_than_its_share() {
    let ws = Workspace::new();
    ws.write("artifacts/many.md", "needle\n".repeat(10));
    let TextSearchResponse::Searched {
        hits, truncated, ..
    } = ws.search("needle", TextSearchMode::Preview)
    else {
        panic!("expected hits");
    };
    assert_eq!(hits.len(), PREVIEW_LINES_PER_FILE);
    assert!(truncated);
}

#[test]
fn all_mode_returns_every_line_with_no_cap_per_file() {
    let ws = Workspace::new();
    ws.write("artifacts/many.md", "needle\n".repeat(40));
    let TextSearchResponse::Searched {
        hits, truncated, ..
    } = ws.search("needle", TextSearchMode::All)
    else {
        panic!("expected hits");
    };
    assert_eq!(hits.len(), 40);
    assert!(!truncated);
}

#[test]
fn all_mode_stops_at_the_cap_and_says_so() {
    let ws = Workspace::new();
    ws.write("artifacts/flood.md", "needle\n".repeat(ALL_LINES_CAP + 7));
    let TextSearchResponse::Searched {
        hits, truncated, ..
    } = ws.search("needle", TextSearchMode::All)
    else {
        panic!("expected hits");
    };
    assert_eq!(hits.len(), ALL_LINES_CAP);
    assert!(truncated);
}

#[test]
fn all_mode_holds_the_cap_across_many_files() {
    let ws = Workspace::new();
    let per_file = ALL_LINES_CAP / 4 + 1;
    for i in 0..8 {
        ws.write(&format!("artifacts/f{i}.md"), "needle\n".repeat(per_file));
    }
    let TextSearchResponse::Searched {
        hits, truncated, ..
    } = ws.search("needle", TextSearchMode::All)
    else {
        panic!("expected hits");
    };
    assert_eq!(hits.len(), ALL_LINES_CAP);
    assert!(truncated);
}

#[test]
fn a_query_too_large_to_compile_is_an_error_not_a_panic() {
    let ws = Workspace::new();
    ws.write("artifacts/a.md", "needle");
    let huge = "k".repeat(1_000_000);
    assert!(search_workspace_text(
        ws.root(),
        &huge,
        TextSearchMode::All,
        &CancellationToken::new()
    )
    .is_err());
}

#[test]
fn a_cancelled_search_reads_no_file() {
    let ws = Workspace::new();
    ws.write("artifacts/a.md", "needle");
    let cancelled = CancellationToken::new();
    cancelled.cancel();
    let response =
        search_workspace_text(ws.root(), "needle", TextSearchMode::All, &cancelled).unwrap();
    assert_eq!(
        response,
        TextSearchResponse::Searched {
            hits: vec![],
            truncated: true,
            skipped_large_files: 0
        }
    );
}

#[test]
fn a_workspace_with_no_data_dir_finds_nothing() {
    let ws = Workspace::new();
    assert!(ws.hits("needle", TextSearchMode::All).is_empty());
}

/// Timing against a real workspace, read-only. Point it at one with
/// `TEXT_SEARCH_TIMING_WORKSPACE=<workspace dir>` and run with `--ignored`.
#[test]
#[ignore]
fn timing_against_a_real_workspace() {
    let Ok(root) = std::env::var("TEXT_SEARCH_TIMING_WORKSPACE") else {
        panic!("set TEXT_SEARCH_TIMING_WORKSPACE to a workspace directory");
    };
    let root = Path::new(&root);
    for (query, mode) in [
        ("workspace", TextSearchMode::Preview),
        ("workspace", TextSearchMode::All),
        ("zqxjv-absent", TextSearchMode::Preview),
        ("zqxjv-absent", TextSearchMode::All),
    ] {
        for run in 0..3 {
            let started = std::time::Instant::now();
            let response =
                search_workspace_text(root, query, mode, &CancellationToken::new()).unwrap();
            let hits = match response {
                TextSearchResponse::Searched { hits, .. } => hits.len(),
                TextSearchResponse::QueryTooShort { .. } => 0,
            };
            println!(
                "{query:>14} {mode:?} run {run}: {hits} hits in {:?}",
                started.elapsed()
            );
        }
    }
}
