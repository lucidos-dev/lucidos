const FALLBACK_SUMMARY_MAX_CHARS: usize = 200;

fn clamp_line(line: &str) -> String {
    let mut chars = line.trim().chars();
    let head: String = chars.by_ref().take(FALLBACK_SUMMARY_MAX_CHARS).collect();
    if chars.next().is_some() {
        format!("{head}…")
    } else {
        head
    }
}

fn last_non_empty_line(text: &str) -> Option<String> {
    text.lines()
        .map(str::trim)
        .rfind(|l| !l.is_empty())
        .map(clamp_line)
}

pub fn script_fallback_summary(stdout: &str, trigger_name: &str, exit_code: i32) -> String {
    last_non_empty_line(stdout).unwrap_or_else(|| {
        clamp_line(&format!(
            "{} completed (exit {}, no output)",
            trigger_name.trim(),
            exit_code
        ))
    })
}

pub fn ensure_non_empty_summary(summary: &str, trigger_name: &str) -> String {
    if summary.trim().is_empty() {
        clamp_line(&format!("{} completed (no output)", trigger_name.trim()))
    } else {
        summary.to_string()
    }
}

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
        let s = script_fallback_summary("", "Idle detector", 0);
        assert_eq!(s, "Idle detector completed (exit 0, no output)");
        assert!(!s.trim().is_empty());
    }

    #[test]
    fn script_fallback_whitespace_only_stdout_uses_status_line() {
        let s = script_fallback_summary("  \n\t\n   \n", "Idle detector", 0);
        assert_eq!(s, "Idle detector completed (exit 0, no output)");
    }

    #[test]
    fn script_fallback_multiline_stdout_last_non_empty_line_wins() {
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
        let huge = "x".repeat(10_000);
        let s = script_fallback_summary(&format!("noise\n{huge}"), "T", 0);
        assert_eq!(
            s.chars().count(),
            FALLBACK_SUMMARY_MAX_CHARS + 1,
            "the cap plus the ellipsis: {s}"
        );
        assert!(s.ends_with('…'));
        assert!(!s.contains('\n'));
    }

    #[test]
    fn fallback_line_handles_multibyte_without_panicking() {
        let s = script_fallback_summary(&"é".repeat(500), "T", 0);
        assert!(s.ends_with('…'));
        assert_eq!(s.chars().count(), FALLBACK_SUMMARY_MAX_CHARS + 1);
    }

    #[test]
    fn the_cap_counts_characters_not_bytes() {
        let line = "確認".repeat(50);
        assert_eq!(line.chars().count(), 100);
        assert!(line.len() > FALLBACK_SUMMARY_MAX_CHARS, "300 bytes");

        let s = script_fallback_summary(&line, "T", 0);
        assert_eq!(s, line, "a 100-character line is well inside the cap");
        assert!(!s.ends_with('…'));
    }
}
