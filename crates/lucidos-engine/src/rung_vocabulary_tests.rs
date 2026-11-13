//! Guards the two rung vocabularies (docs/glossary.md § Cache rung). A cache
//! rung marks the prompt cache in a memory view, and an intelligence rung is a
//! step on an intelligence ladder. A bare `rung` identifier would blur them, so
//! every identifier naming one carries its qualifier. A string stays free: the
//! wire and the UI still say "rungs".

use std::path::{Path, PathBuf};

use crate::test_support::source_scan::src_root;

const CACHE_RUNG: &[&str] = &["cache_rung", "CACHE_RUNG"];
/// The *rung estimates* name the file of every pair's bands, a concept of
/// its own.
const INTELLIGENCE_RUNG: &[&str] = &[
    "intelligence_rung",
    "IntelligenceRung",
    "INTELLIGENCE_RUNG",
    "rung_estimates",
    "RungEstimates",
    "RUNG_ESTIMATES",
];

/// Each guarded area, relative to the engine's `src/`, with the qualifiers
/// every rung identifier there carries. An area is a directory or one file.
const AREAS: &[(&str, &[&str])] = &[
    ("engine/summary_tree", CACHE_RUNG),
    ("runtime/intelligence_ladder.rs", INTELLIGENCE_RUNG),
    ("runtime/intelligence_ladder_tests.rs", INTELLIGENCE_RUNG),
    ("api/intelligence_ladder.rs", INTELLIGENCE_RUNG),
    (
        "../../lucidos-cli/src/intelligence_ladder.rs",
        INTELLIGENCE_RUNG,
    ),
];

/// Every `.rs` file in `area`, tests included.
fn rust_files(area: &Path) -> Vec<PathBuf> {
    if area.is_file() {
        return vec![area.to_path_buf()];
    }
    let mut out = Vec::new();
    let mut stack = vec![area.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).expect("read_dir").flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().and_then(|e| e.to_str()) == Some("rs") {
                out.push(path);
            }
        }
    }
    out
}

/// `text` with its `//` comments and string literals blanked. A string is
/// prose or a wire name, never an identifier, and it may span lines.
fn code_of(text: &str) -> String {
    let mut code = String::new();
    let (mut in_string, mut in_comment, mut escaped) = (false, false, false);
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\n' {
            in_comment = false;
            code.push(c);
            continue;
        }
        if in_comment {
            continue;
        }
        if in_string {
            match c {
                _ if escaped => escaped = false,
                '\\' => escaped = true,
                '"' => in_string = false,
                _ => {}
            }
            code.push(' ');
            continue;
        }
        match c {
            '"' => in_string = true,
            '/' if chars.peek() == Some(&'/') => {
                in_comment = true;
                continue;
            }
            _ => {}
        }
        code.push(c);
    }
    code
}

/// The rung identifiers in `text` that lack every one of `qualifiers`.
fn bare_rungs(text: &str, qualifiers: &[&str]) -> Vec<String> {
    code_of(text)
        .split(|c: char| !(c.is_alphanumeric() || c == '_'))
        .filter(|word| word.to_lowercase().contains("rung"))
        .filter(|word| !qualifiers.iter().any(|q| word.contains(q)))
        .map(str::to_string)
        .collect()
}

#[test]
fn every_rung_identifier_names_its_kind() {
    let mut found = Vec::new();
    for (area, qualifiers) in AREAS {
        for path in rust_files(&src_root().join(area)) {
            let text = std::fs::read_to_string(&path).expect("read source");
            for word in bare_rungs(&text, qualifiers) {
                found.push(format!("{}: {word}", path.display()));
            }
        }
    }
    assert!(
        found.is_empty(),
        "bare rung identifiers:\n{}",
        found.join("\n")
    );
}

#[test]
fn the_scan_flags_a_bare_rung_and_skips_comments_and_strings() {
    let text = "fn rungs() {}\n// the rungs of a view\nlet cache_rungs = THREAD_CACHE_RUNGS;\n\
                #[serde(rename = \"rungs\")] intelligence_rungs: \"a \\\" \\\n rung\", rung";
    assert_eq!(
        bare_rungs(text, CACHE_RUNG),
        vec!["rungs", "intelligence_rungs", "rung"]
    );
    assert_eq!(
        bare_rungs(text, INTELLIGENCE_RUNG),
        vec!["rungs", "cache_rungs", "THREAD_CACHE_RUNGS", "rung"]
    );
}
