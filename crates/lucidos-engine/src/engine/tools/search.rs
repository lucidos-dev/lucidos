//! Pure search helpers for `glob_files` and `grep_files` tools.
//!
//! Both tools operate on the workspace's `data/` directory and scope themselves
//! to the same subdirectories that `list_files` walks (artifacts, apps, knowhow,
//! triggers). Patterns are written relative to `data/` so the LLM can use the
//! same paths it sees in `list_files` output (e.g. `apps/**/index.html`).

use serde::Serialize;
use std::path::Path;

pub const GLOB_DEFAULT_LIMIT: usize = 200;
pub const GLOB_MAX_LIMIT: usize = 1000;
pub const GREP_DEFAULT_MAX_MATCHES: usize = 100;
pub const GREP_MAX_MAX_MATCHES: usize = 500;
pub const GREP_MAX_CONTEXT_LINES: usize = 5;
/// Per-line truncation cap. Files like PDF text dumps or single-line JSON have
/// individual lines of many KB; without a cap the matched line + context window
/// can dump megabytes of text into the LLM context.
pub const GREP_MAX_LINE_CHARS: usize = 300;
/// Hard ceiling on the serialized text content (matched line + context lines)
/// across all returned matches. ~50 KB ≈ ~12k tokens — comfortably under any
/// model's context budget for a single tool result.
pub const GREP_MAX_TOTAL_BYTES: usize = 50 * 1024;
const BINARY_SNIFF_BYTES: usize = 8 * 1024;
/// Skip files larger than this when grepping — keeps a single pathological log
/// file from OOM-ing the engine. Glob walk is metadata-only so doesn't need it.
const GREP_MAX_FILE_BYTES: u64 = 10 * 1024 * 1024;

#[derive(Debug, Serialize)]
pub struct GlobResult {
    pub paths: Vec<String>,
    pub truncated: bool,
}

#[derive(Debug, Serialize)]
pub struct GrepMatch {
    pub path: String,
    pub line_number: usize,
    pub line: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub context_before: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub context_after: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct GrepResult {
    pub matches: Vec<GrepMatch>,
    pub truncated: bool,
}

/// Reject patterns that try to escape `data/` — matches the policy that
/// every other file tool applies via `resolve_data_path`. Even though the
/// matcher only sees `data/`-relative strings, accepting `..`-style patterns
/// silently returns empty and trains the LLM to think files are missing.
fn validate_pattern(label: &str, pattern: &str) -> Result<(), String> {
    if crate::core::is_path_traversal(pattern) {
        Err(format!(
            "{} must be relative under data/ — '..', leading '/' and leading '\\' are rejected",
            label
        ))
    } else {
        Ok(())
    }
}

/// Match files under `data/` against a glob pattern (relative to `data/`).
pub fn glob_files(
    workspace_path: &Path,
    pattern: &str,
    limit: usize,
) -> Result<GlobResult, String> {
    let limit = limit.clamp(1, GLOB_MAX_LIMIT);
    let pattern = pattern.trim();
    if pattern.is_empty() {
        return Err("pattern is required".to_string());
    }
    validate_pattern("pattern", pattern)?;
    let compiled = glob::Pattern::new(pattern)
        .map_err(|e| format!("Invalid glob pattern '{}': {}", pattern, e))?;

    let entries = crate::core::list_searchable_data_files(workspace_path)
        .map_err(|e| format!("Failed to list workspace files: {}", e))?;

    let mut matches = Vec::new();
    let mut truncated = false;
    for (rel, _) in entries {
        if compiled.matches(&rel) {
            if matches.len() >= limit {
                truncated = true;
                break;
            }
            matches.push(rel);
        }
    }

    Ok(GlobResult {
        paths: matches,
        truncated,
    })
}

fn looks_binary(bytes: &[u8]) -> bool {
    bytes.iter().take(BINARY_SNIFF_BYTES).any(|b| *b == 0)
}

/// Char-aware so we never split a multi-byte UTF-8 sequence — `&s[..N]` would
/// panic on inputs like the Polish/Norwegian artifacts we routinely grep.
fn cap_line(s: &str) -> String {
    let mut chars = s.chars();
    let head: String = chars.by_ref().take(GREP_MAX_LINE_CHARS).collect();
    if chars.next().is_some() {
        let mut out = head;
        out.push('…');
        out
    } else {
        head
    }
}

/// Search file contents for a regex pattern. Walks the same directories as
/// `list_files` / `glob_files` and stops as soon as `max_matches` is reached.
pub fn grep_files(
    workspace_path: &Path,
    pattern: &str,
    path_glob: Option<&str>,
    case_insensitive: bool,
    max_matches: usize,
    context_lines: usize,
) -> Result<GrepResult, String> {
    let max_matches = max_matches.clamp(1, GREP_MAX_MAX_MATCHES);
    let context_lines = context_lines.min(GREP_MAX_CONTEXT_LINES);
    let pattern = pattern.trim();
    if pattern.is_empty() {
        return Err("pattern is required".to_string());
    }

    let regex = regex::RegexBuilder::new(pattern)
        .case_insensitive(case_insensitive)
        .build()
        .map_err(|e| format!("Invalid regex '{}': {}", pattern, e))?;

    let path_filter = path_glob
        .map(str::trim)
        .filter(|g| !g.is_empty())
        .map(|g| {
            validate_pattern("path_glob", g)?;
            glob::Pattern::new(g).map_err(|e| format!("Invalid path_glob '{}': {}", g, e))
        })
        .transpose()?;

    let entries = crate::core::list_searchable_data_files(workspace_path)
        .map_err(|e| format!("Failed to list workspace files: {}", e))?;

    let mut matches = Vec::new();
    let mut truncated = false;
    let mut total_bytes: usize = 0;

    'outer: for (rel, abs) in entries {
        if let Some(ref pat) = path_filter {
            if !pat.matches(&rel) {
                continue;
            }
        }

        if let Ok(meta) = std::fs::metadata(&abs) {
            if meta.len() > GREP_MAX_FILE_BYTES {
                continue;
            }
        }
        let bytes = match std::fs::read(&abs) {
            Ok(b) => b,
            Err(_) => continue,
        };
        if looks_binary(&bytes) {
            continue;
        }
        let content = match std::str::from_utf8(&bytes) {
            Ok(s) => s,
            Err(_) => continue,
        };

        let lines: Vec<&str> = content.lines().collect();
        for (idx, line) in lines.iter().enumerate() {
            if regex.is_match(line) {
                if matches.len() >= max_matches {
                    truncated = true;
                    break 'outer;
                }
                let before_start = idx.saturating_sub(context_lines);
                let after_end = (idx + 1 + context_lines).min(lines.len());
                let capped_line = cap_line(line);
                let context_before: Vec<String> =
                    lines[before_start..idx].iter().map(|s| cap_line(s)).collect();
                let context_after: Vec<String> = lines[idx + 1..after_end]
                    .iter()
                    .map(|s| cap_line(s))
                    .collect();
                let match_bytes = capped_line.len()
                    + context_before.iter().map(|s| s.len()).sum::<usize>()
                    + context_after.iter().map(|s| s.len()).sum::<usize>();
                matches.push(GrepMatch {
                    path: rel.clone(),
                    line_number: idx + 1,
                    line: capped_line,
                    context_before,
                    context_after,
                });
                total_bytes += match_bytes;
                // Soft cap: check after pushing so we always include the match
                // that trips it — otherwise a single huge match could return zero.
                if total_bytes >= GREP_MAX_TOTAL_BYTES {
                    truncated = true;
                    break 'outer;
                }
            }
        }
    }

    Ok(GrepResult { matches, truncated })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    fn make_workspace() -> tempfile::TempDir {
        let dir = tempdir().unwrap();
        let data = dir.path().join("data");
        for sub in ["artifacts", "apps", "knowhow", "triggers", "postgres"] {
            fs::create_dir_all(data.join(sub)).unwrap();
        }
        // .lucidos must NOT be searched
        fs::create_dir_all(dir.path().join(".lucidos/exhaust")).unwrap();
        fs::write(dir.path().join(".lucidos/exhaust/cache.txt"), "secret").unwrap();
        // postgres must NOT be searched even though it's under data/
        fs::write(data.join("postgres/pg_stuff.txt"), "secret pg").unwrap();
        dir
    }

    fn write(workspace: &Path, rel: &str, content: &str) {
        let full = workspace.join("data").join(rel);
        fs::create_dir_all(full.parent().unwrap()).unwrap();
        fs::write(full, content).unwrap();
    }

    // -------- glob_files --------

    #[test]
    fn glob_matches_simple_pattern() {
        let ws = make_workspace();
        write(ws.path(), "artifacts/notes.md", "x");
        write(ws.path(), "artifacts/todo.md", "y");
        write(ws.path(), "artifacts/data.csv", "z");

        let r = glob_files(ws.path(), "artifacts/*.md", 200).unwrap();
        assert_eq!(
            r.paths,
            vec!["artifacts/notes.md".to_string(), "artifacts/todo.md".to_string()]
        );
        assert!(!r.truncated);
    }

    #[test]
    fn glob_matches_recursive_double_star() {
        let ws = make_workspace();
        write(ws.path(), "apps/foo/index.html", "x");
        write(ws.path(), "apps/foo/sub/index.html", "y");
        write(ws.path(), "apps/bar/main.html", "z");

        let r = glob_files(ws.path(), "apps/**/index.html", 200).unwrap();
        assert_eq!(
            r.paths,
            vec![
                "apps/foo/index.html".to_string(),
                "apps/foo/sub/index.html".to_string(),
            ]
        );
    }

    #[test]
    fn glob_matches_all_md_anywhere() {
        let ws = make_workspace();
        write(ws.path(), "artifacts/a.md", "x");
        write(ws.path(), "knowhow/b.md", "y");
        write(ws.path(), "triggers/sub/c.md", "z");
        write(ws.path(), "apps/x/notes.md", "w");

        let r = glob_files(ws.path(), "**/*.md", 200).unwrap();
        assert_eq!(r.paths.len(), 4);
        assert!(r.paths.contains(&"artifacts/a.md".to_string()));
        assert!(r.paths.contains(&"knowhow/b.md".to_string()));
        assert!(r.paths.contains(&"triggers/sub/c.md".to_string()));
        assert!(r.paths.contains(&"apps/x/notes.md".to_string()));
    }

    #[test]
    fn glob_skips_postgres_and_lucidos() {
        let ws = make_workspace();
        // No matches under artifacts/apps/knowhow/triggers — only postgres and .lucidos
        let r = glob_files(ws.path(), "**/*.txt", 200).unwrap();
        assert!(
            r.paths.is_empty(),
            "should not find files in postgres/ or .lucidos/, got {:?}",
            r.paths
        );
    }

    #[test]
    fn glob_truncates_at_limit() {
        let ws = make_workspace();
        for i in 0..10 {
            write(ws.path(), &format!("artifacts/f{:02}.md", i), "x");
        }
        let r = glob_files(ws.path(), "artifacts/*.md", 3).unwrap();
        assert_eq!(r.paths.len(), 3);
        assert!(r.truncated);
        assert_eq!(r.paths[0], "artifacts/f00.md");
        assert_eq!(r.paths[2], "artifacts/f02.md");
    }

    #[test]
    fn glob_clamps_limit_above_max() {
        let ws = make_workspace();
        write(ws.path(), "artifacts/a.md", "x");
        // limit > GLOB_MAX_LIMIT should be clamped, not error
        let r = glob_files(ws.path(), "**/*.md", GLOB_MAX_LIMIT * 5).unwrap();
        assert_eq!(r.paths, vec!["artifacts/a.md".to_string()]);
    }

    #[test]
    fn glob_rejects_path_traversal() {
        let ws = make_workspace();
        for bad in ["../../etc/passwd", "/etc/passwd", "\\windows\\system32"] {
            let err = glob_files(ws.path(), bad, 200).unwrap_err();
            assert!(
                err.contains("rejected"),
                "expected traversal rejection for '{}', got: {}",
                bad,
                err
            );
        }
    }

    #[test]
    fn grep_rejects_path_traversal_in_pattern_glob() {
        let ws = make_workspace();
        let err = grep_files(ws.path(), "x", Some("../../*"), false, 100, 0).unwrap_err();
        assert!(err.contains("rejected"), "got: {}", err);
    }

    #[test]
    fn glob_skips_symlinks_to_outside() {
        // Symlinks to anywhere — even valid dirs — must be skipped so we
        // can't be tricked into walking outside data/ or into a loop.
        #[cfg(unix)]
        {
            let ws = make_workspace();
            let outside = tempdir().unwrap();
            fs::write(outside.path().join("secret.md"), "leaked").unwrap();
            std::os::unix::fs::symlink(
                outside.path(),
                ws.path().join("data/artifacts/escape"),
            )
            .unwrap();

            let r = glob_files(ws.path(), "**/*.md", 200).unwrap();
            assert!(
                r.paths.is_empty(),
                "should not follow symlink out of data/, got {:?}",
                r.paths
            );
        }
    }

    #[test]
    fn glob_invalid_pattern_returns_error() {
        let ws = make_workspace();
        let err = glob_files(ws.path(), "artifacts/[unclosed", 200).unwrap_err();
        assert!(err.contains("Invalid glob"));
    }

    #[test]
    fn glob_empty_pattern_returns_error() {
        let ws = make_workspace();
        let err = glob_files(ws.path(), "  ", 200).unwrap_err();
        assert!(err.contains("required"));
    }

    #[test]
    fn glob_results_are_sorted() {
        let ws = make_workspace();
        write(ws.path(), "artifacts/zebra.md", "x");
        write(ws.path(), "artifacts/apple.md", "y");
        write(ws.path(), "artifacts/middle.md", "z");
        let r = glob_files(ws.path(), "artifacts/*.md", 200).unwrap();
        assert_eq!(
            r.paths,
            vec![
                "artifacts/apple.md".to_string(),
                "artifacts/middle.md".to_string(),
                "artifacts/zebra.md".to_string(),
            ]
        );
    }

    // -------- grep_files --------

    #[test]
    fn grep_finds_simple_match() {
        let ws = make_workspace();
        write(ws.path(), "artifacts/a.md", "hello world\nsecond line");
        write(ws.path(), "artifacts/b.md", "no match here");

        let r = grep_files(ws.path(), "world", None, false, 100, 0).unwrap();
        assert_eq!(r.matches.len(), 1);
        assert_eq!(r.matches[0].path, "artifacts/a.md");
        assert_eq!(r.matches[0].line_number, 1);
        assert_eq!(r.matches[0].line, "hello world");
        assert!(r.matches[0].context_before.is_empty());
        assert!(r.matches[0].context_after.is_empty());
        assert!(!r.truncated);
    }

    #[test]
    fn grep_case_sensitive_by_default() {
        let ws = make_workspace();
        write(ws.path(), "artifacts/a.md", "HELLO\nhello");
        let r = grep_files(ws.path(), "hello", None, false, 100, 0).unwrap();
        assert_eq!(r.matches.len(), 1);
        assert_eq!(r.matches[0].line, "hello");
    }

    #[test]
    fn grep_case_insensitive_flag_works() {
        let ws = make_workspace();
        write(ws.path(), "artifacts/a.md", "HELLO\nhello\nWORLD");
        let r = grep_files(ws.path(), "hello", None, true, 100, 0).unwrap();
        assert_eq!(r.matches.len(), 2);
    }

    #[test]
    fn grep_supports_regex_syntax() {
        let ws = make_workspace();
        write(ws.path(), "artifacts/a.md", "TODO: x\nDONE: y\nTODO: z");
        let r = grep_files(ws.path(), r"^TODO:", None, false, 100, 0).unwrap();
        assert_eq!(r.matches.len(), 2);
        assert_eq!(r.matches[0].line_number, 1);
        assert_eq!(r.matches[1].line_number, 3);
    }

    #[test]
    fn grep_path_glob_filters_files() {
        let ws = make_workspace();
        write(ws.path(), "apps/foo/index.html", "<h1>Hello</h1>");
        write(ws.path(), "apps/bar/page.md", "Hello there");
        write(ws.path(), "artifacts/notes.md", "Hello world");

        let r = grep_files(ws.path(), "Hello", Some("apps/**/*.html"), false, 100, 0).unwrap();
        assert_eq!(r.matches.len(), 1);
        assert_eq!(r.matches[0].path, "apps/foo/index.html");
    }

    #[test]
    fn grep_skips_files_over_max_bytes() {
        let ws = make_workspace();
        let big = "X".repeat((GREP_MAX_FILE_BYTES + 1) as usize);
        write(ws.path(), "artifacts/huge.log", &big);
        write(ws.path(), "artifacts/small.md", "X");
        let r = grep_files(ws.path(), "X", None, false, 100, 0).unwrap();
        assert_eq!(r.matches.len(), 1);
        assert_eq!(r.matches[0].path, "artifacts/small.md");
    }

    #[test]
    fn grep_skips_binary_files() {
        let ws = make_workspace();
        // Binary file with null byte and the search pattern in it
        let mut binary: Vec<u8> = b"hello before".to_vec();
        binary.push(0);
        binary.extend_from_slice(b"hello after");
        let bin_path = ws.path().join("data/artifacts/blob.bin");
        std::fs::write(&bin_path, &binary).unwrap();
        write(ws.path(), "artifacts/text.md", "hello text");

        let r = grep_files(ws.path(), "hello", None, false, 100, 0).unwrap();
        assert_eq!(r.matches.len(), 1);
        assert_eq!(r.matches[0].path, "artifacts/text.md");
    }

    #[test]
    fn grep_truncates_at_max_matches() {
        let ws = make_workspace();
        let mut content = String::new();
        for _ in 0..10 {
            content.push_str("hit\n");
        }
        write(ws.path(), "artifacts/a.md", &content);
        let r = grep_files(ws.path(), "hit", None, false, 3, 0).unwrap();
        assert_eq!(r.matches.len(), 3);
        assert!(r.truncated);
    }

    #[test]
    fn grep_clamps_max_matches_above_limit() {
        let ws = make_workspace();
        write(ws.path(), "artifacts/a.md", "x");
        // Should clamp without erroring
        let r = grep_files(ws.path(), "x", None, false, GREP_MAX_MAX_MATCHES * 10, 0).unwrap();
        assert_eq!(r.matches.len(), 1);
    }

    #[test]
    fn grep_context_lines_before_and_after() {
        let ws = make_workspace();
        write(
            ws.path(),
            "artifacts/a.md",
            "line1\nline2\nline3 MATCH\nline4\nline5",
        );
        let r = grep_files(ws.path(), "MATCH", None, false, 100, 2).unwrap();
        assert_eq!(r.matches.len(), 1);
        let m = &r.matches[0];
        assert_eq!(m.line_number, 3);
        assert_eq!(m.line, "line3 MATCH");
        assert_eq!(m.context_before, vec!["line1".to_string(), "line2".to_string()]);
        assert_eq!(m.context_after, vec!["line4".to_string(), "line5".to_string()]);
    }

    #[test]
    fn grep_context_clamps_at_file_boundaries() {
        let ws = make_workspace();
        write(ws.path(), "artifacts/a.md", "MATCH\nafter");
        let r = grep_files(ws.path(), "MATCH", None, false, 100, 5).unwrap();
        let m = &r.matches[0];
        assert!(m.context_before.is_empty(), "no lines before line 1");
        assert_eq!(m.context_after, vec!["after".to_string()]);
    }

    #[test]
    fn grep_context_lines_clamped_at_max() {
        let ws = make_workspace();
        let mut content = String::new();
        for i in 0..20 {
            content.push_str(&format!("filler{}\n", i));
        }
        content.push_str("MATCH\n");
        for i in 0..20 {
            content.push_str(&format!("after{}\n", i));
        }
        write(ws.path(), "artifacts/a.md", &content);
        // request 50 — should clamp to GREP_MAX_CONTEXT_LINES (5)
        let r = grep_files(ws.path(), "MATCH", None, false, 100, 50).unwrap();
        let m = &r.matches[0];
        assert_eq!(m.context_before.len(), GREP_MAX_CONTEXT_LINES);
        assert_eq!(m.context_after.len(), GREP_MAX_CONTEXT_LINES);
    }

    #[test]
    fn grep_skips_postgres_and_lucidos_dirs() {
        let ws = make_workspace();
        // Contents are set up by make_workspace; nothing under artifacts/apps/knowhow/triggers
        let r = grep_files(ws.path(), "secret", None, false, 100, 0).unwrap();
        assert!(r.matches.is_empty(), "must not search postgres/ or .lucidos/");
    }

    #[test]
    fn grep_invalid_regex_returns_error() {
        let ws = make_workspace();
        let err = grep_files(ws.path(), "[unclosed", None, false, 100, 0).unwrap_err();
        assert!(err.contains("Invalid regex"));
    }

    #[test]
    fn grep_empty_pattern_returns_error() {
        let ws = make_workspace();
        let err = grep_files(ws.path(), "  ", None, false, 100, 0).unwrap_err();
        assert!(err.contains("required"));
    }

    #[test]
    fn grep_invalid_path_glob_returns_error() {
        let ws = make_workspace();
        let err =
            grep_files(ws.path(), "x", Some("apps/[unclosed"), false, 100, 0).unwrap_err();
        assert!(err.contains("Invalid path_glob"));
    }

    #[test]
    fn grep_empty_path_glob_treated_as_none() {
        let ws = make_workspace();
        write(ws.path(), "artifacts/a.md", "hello");
        let r = grep_files(ws.path(), "hello", Some(""), false, 100, 0).unwrap();
        assert_eq!(r.matches.len(), 1);
    }

    #[test]
    fn grep_truncates_long_match_line() {
        let ws = make_workspace();
        // PDF-style dump: one giant line containing the match.
        let prefix = "x".repeat(GREP_MAX_LINE_CHARS + 500);
        let line = format!("{}MATCH{}", prefix, "y".repeat(500));
        write(ws.path(), "artifacts/a.md", &line);
        let r = grep_files(ws.path(), "MATCH", None, false, 100, 0).unwrap();
        assert_eq!(r.matches.len(), 1);
        let returned = &r.matches[0].line;
        assert!(
            returned.chars().count() <= GREP_MAX_LINE_CHARS + 1,
            "expected <= {} chars, got {}",
            GREP_MAX_LINE_CHARS + 1,
            returned.chars().count()
        );
        assert!(
            returned.ends_with('…'),
            "expected ellipsis marker, got: {}",
            returned
        );
    }

    #[test]
    fn grep_truncates_long_context_lines() {
        let ws = make_workspace();
        let big = "z".repeat(GREP_MAX_LINE_CHARS + 200);
        let content = format!("{}\nMATCH\n{}", big, big);
        write(ws.path(), "artifacts/a.md", &content);
        let r = grep_files(ws.path(), "MATCH", None, false, 100, 1).unwrap();
        assert_eq!(r.matches.len(), 1);
        let m = &r.matches[0];
        assert_eq!(m.context_before.len(), 1);
        assert_eq!(m.context_after.len(), 1);
        for ctx in [&m.context_before[0], &m.context_after[0]] {
            assert!(
                ctx.chars().count() <= GREP_MAX_LINE_CHARS + 1,
                "context line too long: {} chars",
                ctx.chars().count()
            );
            assert!(ctx.ends_with('…'), "context missing ellipsis: {}", ctx);
        }
    }

    #[test]
    fn grep_short_lines_not_truncated() {
        let ws = make_workspace();
        write(ws.path(), "artifacts/a.md", "short MATCH line\nbefore\nMATCH again");
        let r = grep_files(ws.path(), "MATCH", None, false, 100, 1).unwrap();
        for m in &r.matches {
            assert!(
                !m.line.ends_with('…'),
                "short line should not be truncated: {}",
                m.line
            );
            for c in m.context_before.iter().chain(m.context_after.iter()) {
                assert!(!c.ends_with('…'), "short context should not be truncated: {}", c);
            }
        }
    }

    #[test]
    fn grep_stops_at_total_bytes_cap() {
        let ws = make_workspace();
        // Each line ~250 chars + "MATCH" — under per-line cap, but stacking
        // many of them blows past the total cap quickly.
        let mut content = String::new();
        for _ in 0..1000 {
            content.push_str("MATCH ");
            content.push_str(&"q".repeat(250));
            content.push('\n');
        }
        write(ws.path(), "artifacts/a.md", &content);
        let r = grep_files(ws.path(), "MATCH", None, false, 500, 0).unwrap();
        assert!(r.truncated, "expected total-bytes cap to trip");
        let total_bytes: usize = r
            .matches
            .iter()
            .map(|m| {
                m.line.len()
                    + m.context_before.iter().map(|s| s.len()).sum::<usize>()
                    + m.context_after.iter().map(|s| s.len()).sum::<usize>()
            })
            .sum();
        // Some overshoot expected (we always include the match that trips the cap),
        // but the result must stay within an order of magnitude of the limit.
        assert!(
            total_bytes <= GREP_MAX_TOTAL_BYTES * 2,
            "total bytes {} should be near the cap of {}",
            total_bytes,
            GREP_MAX_TOTAL_BYTES
        );
        assert!(
            r.matches.len() < 500,
            "should have stopped well before max_matches=500, got {}",
            r.matches.len()
        );
    }

    #[test]
    fn grep_total_bytes_cap_always_returns_first_match() {
        let ws = make_workspace();
        // Single line just under the per-line cap and over the total cap on its own
        // (after per-line truncation it's still well within total cap; this just
        // verifies we don't drop the first match even if it would push us over).
        let line = format!("{} MATCH", "p".repeat(GREP_MAX_LINE_CHARS - 10));
        write(ws.path(), "artifacts/a.md", &line);
        let r = grep_files(ws.path(), "MATCH", None, false, 100, 0).unwrap();
        assert_eq!(r.matches.len(), 1, "first match must always be returned");
    }

    #[test]
    fn grep_truncation_handles_multibyte_chars() {
        let ws = make_workspace();
        // Mix of multi-byte chars to ensure we don't slice mid-codepoint.
        let prefix = "ñ".repeat(GREP_MAX_LINE_CHARS + 100);
        let line = format!("{}MATCH", prefix);
        write(ws.path(), "artifacts/a.md", &line);
        let r = grep_files(ws.path(), "MATCH", None, false, 100, 0).unwrap();
        assert_eq!(r.matches.len(), 1);
        let returned = &r.matches[0].line;
        assert!(returned.ends_with('…'));
        assert!(
            returned.chars().count() <= GREP_MAX_LINE_CHARS + 1,
            "char-aware truncation should respect codepoint boundaries"
        );
    }
}
