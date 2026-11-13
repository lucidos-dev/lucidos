//! The engine's one unified-diff shape and one parser for it.
//!
//! Three surfaces render a diff and all three return this shape, so the
//! frontend has a single `DiffFile` to draw with (`DiffView`,
//! `InlineDiffList`): the repository diff and the change diff (both in
//! `api::repositories`), and the command-checkpoint diff
//! (`api::command_checkpoint`), which diffs a checkpoint's pre image against
//! its post image. They differ only in which two revisions they hand to
//! `git diff`; everything downstream of that is this module.

use serde::Serialize;

#[derive(Serialize)]
pub struct RepoDiff {
    pub files: Vec<DiffFile>,
}

#[derive(Serialize)]
pub struct DiffFile {
    pub path: String,
    pub status: String,
    pub hunks: Vec<DiffHunk>,
}

#[derive(Serialize)]
pub struct DiffHunk {
    pub old_start: u32,
    pub old_count: u32,
    pub new_start: u32,
    pub new_count: u32,
    pub lines: Vec<DiffLine>,
}

#[derive(Serialize)]
pub struct DiffLine {
    #[serde(rename = "type")]
    pub line_type: String,
    pub content: String,
}

/// Decode git's C-quoted path form (`"foo\360..."`) back to raw UTF-8.
/// Returns `path` unchanged when not surrounded by `"`.
fn unquote_git_path(path: &str) -> String {
    let bytes = path.as_bytes();
    if bytes.len() < 2 || bytes[0] != b'"' || bytes[bytes.len() - 1] != b'"' {
        return path.to_string();
    }
    let inner = &bytes[1..bytes.len() - 1];
    let mut out: Vec<u8> = Vec::with_capacity(inner.len());
    let mut i = 0;
    while i < inner.len() {
        let b = inner[i];
        if b == b'\\' && i + 1 < inner.len() {
            let next = inner[i + 1];
            if (b'0'..=b'7').contains(&next) && i + 3 < inner.len() {
                let d2 = inner[i + 2];
                let d3 = inner[i + 3];
                if (b'0'..=b'7').contains(&d2) && (b'0'..=b'7').contains(&d3) {
                    let val = ((next - b'0') << 6) | ((d2 - b'0') << 3) | (d3 - b'0');
                    out.push(val);
                    i += 4;
                    continue;
                }
            }
            let standard = match next {
                b'\\' => Some(b'\\'),
                b'"' => Some(b'"'),
                b'a' => Some(0x07),
                b'b' => Some(0x08),
                b't' => Some(b'\t'),
                b'n' => Some(b'\n'),
                b'v' => Some(0x0B),
                b'f' => Some(0x0C),
                b'r' => Some(b'\r'),
                _ => None,
            };
            if let Some(v) = standard {
                out.push(v);
                i += 2;
                continue;
            }
        }
        out.push(b);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Extract the b-side path from a `diff --git ...` header. Handles both bare
/// (`a/X b/X`) and quoted (`"a/X" "b/X"`) forms. Empty string when nothing
/// matches: never let the raw header bleed through as a filename.
fn parse_diff_git_path(line: &str) -> String {
    if let Some(idx) = line.rfind(" \"b/") {
        let quoted = &line[idx + 1..];
        if quoted.ends_with('"') {
            let unquoted = unquote_git_path(quoted);
            if let Some(rest) = unquoted.strip_prefix("b/") {
                return rest.to_string();
            }
        }
    }
    if let Some(idx) = line.rfind(" b/") {
        return line[idx + 3..].to_string();
    }
    String::new()
}

fn parse_diff_plus_path(line: &str) -> Option<String> {
    if let Some(rest) = line.strip_prefix("+++ b/") {
        return Some(rest.to_string());
    }
    if let Some(rest) = line.strip_prefix("+++ ") {
        if rest.starts_with('"') && rest.ends_with('"') {
            let unquoted = unquote_git_path(rest);
            if let Some(stripped) = unquoted.strip_prefix("b/") {
                return Some(stripped.to_string());
            }
        }
    }
    None
}

pub(crate) fn parse_diff_output(output: &str) -> Vec<DiffFile> {
    let mut files = Vec::new();
    let mut current_file: Option<DiffFile> = None;
    let mut current_hunk: Option<DiffHunk> = None;

    for line in output.lines() {
        if line.starts_with("diff --git") {
            if let Some(mut f) = current_file.take() {
                if let Some(h) = current_hunk.take() {
                    f.hunks.push(h);
                }
                files.push(f);
            }
            current_file = Some(DiffFile {
                path: parse_diff_git_path(line),
                status: "modified".into(),
                hunks: Vec::new(),
            });
        } else if let Some(path) = parse_diff_plus_path(line) {
            if let Some(ref mut f) = current_file {
                f.path = path;
            }
        } else if line.starts_with("--- /dev/null") {
            if let Some(ref mut f) = current_file {
                f.status = "added".into();
            }
        } else if line.starts_with("+++ /dev/null") {
            if let Some(ref mut f) = current_file {
                f.status = "deleted".into();
            }
        } else if let Some(rest) = line.strip_prefix("--- a/") {
            if let Some(ref mut f) = current_file {
                if f.path.is_empty() {
                    f.path = rest.to_string();
                }
            }
        } else if line.starts_with("@@ ") {
            if let Some(ref mut f) = current_file {
                if let Some(h) = current_hunk.take() {
                    f.hunks.push(h);
                }
            }
            if let Some(hunk) = parse_hunk_header(line) {
                current_hunk = Some(hunk);
            }
        } else if let Some(ref mut hunk) = current_hunk {
            if let Some(rest) = line.strip_prefix('+') {
                hunk.lines.push(DiffLine {
                    line_type: "addition".into(),
                    content: rest.to_string(),
                });
            } else if let Some(rest) = line.strip_prefix('-') {
                hunk.lines.push(DiffLine {
                    line_type: "deletion".into(),
                    content: rest.to_string(),
                });
            } else if let Some(rest) = line.strip_prefix(' ') {
                hunk.lines.push(DiffLine {
                    line_type: "context".into(),
                    content: rest.to_string(),
                });
            }
        }
    }

    if let Some(mut f) = current_file {
        if let Some(h) = current_hunk {
            f.hunks.push(h);
        }
        files.push(f);
    }

    files.retain(|f| !crate::engine::claude_code::is_engine_injected_path(&f.path));
    files
}

fn parse_hunk_header(line: &str) -> Option<DiffHunk> {
    let parts: Vec<&str> = line.split_whitespace().collect();
    if parts.len() < 3 {
        return None;
    }

    let old = parts[1].trim_start_matches('-');
    let new = parts[2].trim_start_matches('+');

    let (old_start, old_count) = parse_range(old);
    let (new_start, new_count) = parse_range(new);

    Some(DiffHunk {
        old_start,
        old_count,
        new_start,
        new_count,
        lines: Vec::new(),
    })
}

fn parse_range(s: &str) -> (u32, u32) {
    if let Some((start, count)) = s.split_once(',') {
        (start.parse().unwrap_or(0), count.parse().unwrap_or(0))
    } else {
        (s.parse().unwrap_or(0), 1)
    }
}

#[cfg(test)]
#[path = "diff_tests.rs"]
mod tests;
