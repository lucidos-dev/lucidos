//! Engine-level guarantee that a trigger run's `result_summary` is never empty.
//!
//! A blank summary makes idle-detector-style triggers (broad subscription + a
//! cheap internal gate that mostly no-ops) read as a "no-op fire flood" to the
//! workspace-learning and workspace-audit sweeps — a false positive we'd
//! otherwise have to carve out per-trigger. Guaranteeing a meaningful one-liner
//! here, once, lets every trigger benefit instead of patching each script to
//! print a status line.
//!
//! The fallbacks are deterministic and single-line:
//! - script trigger with no usable stdout: the script's last non-empty stdout
//!   line if any, else `"<name> completed (exit <code>, no output)"`.
//! - intent trigger with no final text (and any other empty summary):
//!   `"<name> completed (no output)"`.
//! - failed run with no error detail: `"<name> failed"`.

/// Cap for an engine-synthesized fallback line. A fallback is a status
/// one-liner, not a transcript — keep it short.
const FALLBACK_SUMMARY_MAX_CHARS: usize = 200;

/// Trim `line` and cap it at [`FALLBACK_SUMMARY_MAX_CHARS`] chars (char-boundary
/// safe), appending an ellipsis when truncated.
fn clamp_line(line: &str) -> String {
    let line = line.trim();
    if line.len() > FALLBACK_SUMMARY_MAX_CHARS {
        let end = line.floor_char_boundary(FALLBACK_SUMMARY_MAX_CHARS);
        format!("{}…", &line[..end])
    } else {
        line.to_string()
    }
}

/// The last line of `text` that has non-whitespace content, trimmed and capped.
/// `None` when `text` is empty or whitespace-only.
fn last_non_empty_line(text: &str) -> Option<String> {
    text.lines()
        .map(str::trim)
        .rfind(|l| !l.is_empty())
        .map(clamp_line)
}

/// Fallback summary for a script trigger that finished (exit 0 — `execute_script`
/// returns `Err` on any non-zero exit) with no usable summary. Prefers the
/// script's last non-empty stdout line; otherwise a deterministic status line
/// naming the trigger and exit code. Always a single trimmed, capped line.
pub fn script_fallback_summary(stdout: &str, trigger_name: &str, exit_code: i32) -> String {
    last_non_empty_line(stdout).unwrap_or_else(|| {
        clamp_line(&format!(
            "{} completed (exit {}, no output)",
            trigger_name.trim(),
            exit_code
        ))
    })
}

/// Final engine-level guarantee: return `summary` when it has non-whitespace
/// content, else the generic `"<name> completed (no output)"` fallback. Applied
/// at the single `record_trigger_completed` choke point so no caller — script,
/// intent, or future — can emit a blank `result_summary`. The non-empty case is
/// returned verbatim so the existing summary handling (whole output, already
/// truncated by the caller) is unchanged.
pub fn ensure_non_empty_summary(summary: &str, trigger_name: &str) -> String {
    if summary.trim().is_empty() {
        clamp_line(&format!("{} completed (no output)", trigger_name.trim()))
    } else {
        summary.to_string()
    }
}

/// Normalize a failed run's error reason so the surfaced failure is never blank.
/// Returns `reason` verbatim when it has content, else a deterministic
/// `"<name> failed"` line.
pub fn ensure_non_empty_error(reason: &str, trigger_name: &str) -> String {
    if reason.trim().is_empty() {
        format!("{} failed", trigger_name.trim())
    } else {
        reason.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn script_fallback_empty_stdout_uses_status_line() {
        // The reported case: a script trigger exits 0 with no stdout. The
        // engine must substitute a meaningful one-liner, not leave it blank.
        let s = script_fallback_summary("", "Idle detector", 0);
        assert_eq!(s, "Idle detector completed (exit 0, no output)");
        assert!(!s.trim().is_empty());
    }

    #[test]
    fn script_fallback_whitespace_only_stdout_uses_status_line() {
        // Whitespace-only stdout has no non-empty line either — same status line.
        let s = script_fallback_summary("  \n\t\n   \n", "Idle detector", 0);
        assert_eq!(s, "Idle detector completed (exit 0, no output)");
    }

    #[test]
    fn script_fallback_multiline_stdout_last_non_empty_line_wins() {
        // Multi-line stdout → the last non-empty (trimmed) line is the summary,
        // so a script's trailing status line is what surfaces.
        let s = script_fallback_summary("starting\nworking\nnothing to do\n\n  \n", "T", 0);
        assert_eq!(s, "nothing to do");
    }

    #[test]
    fn script_fallback_single_line_is_that_line() {
        let s = script_fallback_summary("all good\n", "T", 0);
        assert_eq!(s, "all good");
    }

    #[test]
    fn ensure_non_empty_summary_empty_uses_generic_fallback() {
        // Intent trigger with no final text routes through this at the choke
        // point and must get the generic "(no output)" line.
        assert_eq!(
            ensure_non_empty_summary("", "Morning brief"),
            "Morning brief completed (no output)"
        );
        assert_eq!(
            ensure_non_empty_summary("   \n  ", "Morning brief"),
            "Morning brief completed (no output)"
        );
    }

    #[test]
    fn ensure_non_empty_summary_keeps_non_empty_verbatim() {
        // The preferred explicit summary (script status line / LLM final text)
        // is preserved exactly — the engine fallback only kicks in when blank.
        let real = "Imported 3 sleep records";
        assert_eq!(ensure_non_empty_summary(real, "T"), real);
    }

    #[test]
    fn ensure_non_empty_error_empty_uses_failed_line() {
        assert_eq!(
            ensure_non_empty_error("", "Oura import"),
            "Oura import failed"
        );
        assert_eq!(
            ensure_non_empty_error("   ", "Oura import"),
            "Oura import failed"
        );
    }

    #[test]
    fn ensure_non_empty_error_keeps_reason_verbatim() {
        let reason = "Shell script error (exit code 1):\nboom";
        assert_eq!(ensure_non_empty_error(reason, "T"), reason);
    }

    #[test]
    fn fallback_line_is_capped_and_single_line() {
        // A pathological last line must be trimmed to a single capped line so
        // the summary stays a one-liner regardless of script output.
        let huge = "x".repeat(10_000);
        let s = script_fallback_summary(&format!("noise\n{huge}"), "T", 0);
        assert!(
            s.chars().count() <= FALLBACK_SUMMARY_MAX_CHARS + 1,
            "len {}",
            s.chars().count()
        );
        assert!(s.ends_with('…'));
        assert!(!s.contains('\n'));
    }

    #[test]
    fn fallback_line_handles_multibyte_without_panicking() {
        // Capping must respect char boundaries (no byte-index slice panic).
        let s = script_fallback_summary(&"é".repeat(500), "T", 0);
        assert!(s.ends_with('…'));
    }
}
