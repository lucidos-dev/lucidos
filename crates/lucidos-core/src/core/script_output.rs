const PY_ERROR_LINE_BUDGET: usize = 30;
pub const PY_ERROR_BYTE_BUDGET: usize = 4096;

pub fn truncate_python_error(stderr: &str) -> String {
    clamp_to_byte_budget(trim_python_error_lines(stderr))
}

pub fn script_error_output(stderr: &str, stdout_end: &str, stdout_total: u64) -> String {
    if !stderr.trim().is_empty() {
        return stderr.to_string();
    }
    let end = stdout_end.trim_end();
    if end.trim().is_empty() {
        return String::new();
    }
    let cut = end.ceil_char_boundary(end.len().saturating_sub(PY_ERROR_BYTE_BUDGET));
    let omitted = stdout_total.saturating_sub(stdout_end.len() as u64) + cut as u64;
    let omitted = if omitted > 0 {
        format!("[... {omitted} bytes of stdout omitted ...]\n")
    } else {
        String::new()
    };
    format!(
        "(stderr was empty, so this is the end of stdout)\n{omitted}{}",
        &end[cut..]
    )
}

fn clamp_to_byte_budget(text: String) -> String {
    let excess = text.len().saturating_sub(PY_ERROR_BYTE_BUDGET);
    if excess == 0 {
        return text;
    }
    let cut = text.ceil_char_boundary(excess);
    format!("[... {cut} bytes of stderr omitted ...]\n{}", &text[cut..])
}

fn trim_python_error_lines(stderr: &str) -> String {
    let line_count = stderr.lines().count();
    if line_count <= PY_ERROR_LINE_BUDGET && stderr.len() <= PY_ERROR_BYTE_BUDGET {
        return stderr.to_string();
    }
    let lines: Vec<&str> = stderr.lines().collect();
    let traceback_start = lines
        .iter()
        .position(|l| l.trim_start().starts_with("Traceback "));

    if let Some(tb) = traceback_start {
        let file_idxs: Vec<usize> = lines
            .iter()
            .enumerate()
            .skip(tb)
            .filter(|(_, l)| l.trim_start().starts_with("File \""))
            .map(|(i, _)| i)
            .collect();

        let exception_idx = lines
            .iter()
            .enumerate()
            .rev()
            .find(|(i, l)| {
                *i > tb
                    && !l.is_empty()
                    && !l.starts_with(' ')
                    && !l.starts_with('\t')
                    && !l.trim_start().starts_with("File \"")
            })
            .map(|(i, _)| i);

        if file_idxs.len() > 4 {
            let mut out = String::new();
            if tb > 0 {
                let pre_keep = tb.saturating_sub(3);
                if pre_keep > 0 {
                    out.push_str(&format!(
                        "[... {} lines of pre-traceback stderr omitted ...]\n",
                        pre_keep
                    ));
                }
                for line in &lines[pre_keep..tb] {
                    out.push_str(line);
                    out.push('\n');
                }
            }
            out.push_str(lines[tb]);
            out.push('\n');
            let first_file = file_idxs[0];
            out.push_str(lines[first_file]);
            out.push('\n');
            if let Some(code_line) = lines.get(first_file + 1) {
                if (code_line.starts_with("    ") || code_line.starts_with("\t"))
                    && !code_line.trim_start().starts_with("File \"")
                {
                    out.push_str(code_line);
                    out.push('\n');
                }
            }
            let omitted = file_idxs.len() - 2;
            out.push_str(&format!("  [... {} frames omitted ...]\n", omitted));
            let last_file = *file_idxs.last().unwrap();
            out.push_str(lines[last_file]);
            out.push('\n');
            if let Some(code_line) = lines.get(last_file + 1) {
                if (code_line.starts_with("    ") || code_line.starts_with("\t"))
                    && !code_line.trim_start().starts_with("File \"")
                {
                    out.push_str(code_line);
                    out.push('\n');
                }
            }
            // INVARIANT: Exception line. Compare with `> last_file`, not
            // `> last_file + 1`: Python can omit the indented source line
            // below the last frame. Frozen importlib._bootstrap frames,
            // C-extension frames and some re-raise paths do that. The
            // exception then sits right at last_file + 1, and the code-line
            // branch above skips it because it is flush-left. A line at one
            // index is either indented code or a flush-left exception.
            if let Some(ex) = exception_idx {
                if ex > last_file {
                    out.push_str(lines[ex]);
                    out.push('\n');
                }
            }
            return out.trim_end_matches('\n').to_string();
        }
    }

    let drop = line_count.saturating_sub(PY_ERROR_LINE_BUDGET);
    let mut out = String::new();
    if drop > 0 {
        out.push_str(&format!("[... {} lines of stderr omitted ...]\n", drop));
    }
    out.push_str(&lines[drop..].join("\n"));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_error_returns_verbatim() {
        let err = "Traceback (most recent call last):\n  File \"x.py\", line 1, in <module>\n    a = 1/0\nZeroDivisionError: division by zero";
        assert_eq!(truncate_python_error(err), err);
    }

    #[test]
    fn long_traceback_keeps_first_and_last_frame_plus_exception() {
        let mut tb = String::from("Traceback (most recent call last):\n");
        for i in 0..20 {
            tb.push_str(&format!(
                "  File \"/Users/x/.../mod_{i}.py\", line {line}, in func_{i}\n    helper_{i}()\n",
                i = i,
                line = 100 + i
            ));
        }
        tb.push_str("ModuleNotFoundError: No module named 'strategy_params'");

        let trimmed = truncate_python_error(&tb);
        assert!(
            trimmed.starts_with("Traceback "),
            "header must be preserved: {trimmed:?}"
        );
        assert!(
            trimmed.contains("mod_0.py"),
            "first frame must be preserved: {trimmed:?}"
        );
        assert!(
            trimmed.contains("helper_0()"),
            "first frame's code line must be preserved: {trimmed:?}"
        );
        assert!(
            trimmed.contains("mod_19.py"),
            "last frame must be preserved: {trimmed:?}"
        );
        assert!(
            trimmed.contains("helper_19()"),
            "last frame's code line must be preserved: {trimmed:?}"
        );
        assert!(
            !trimmed.contains("mod_10.py"),
            "middle frame should be dropped: {trimmed:?}"
        );
        assert!(
            trimmed.contains("frames omitted"),
            "trim marker required: {trimmed:?}"
        );
        assert!(
            trimmed.contains("ModuleNotFoundError: No module named 'strategy_params'"),
            "exception line must be preserved: {trimmed:?}"
        );
        assert!(
            trimmed.len() < tb.len() / 2,
            "trimmed should be < half the original ({} vs {})",
            trimmed.len(),
            tb.len()
        );
    }

    #[test]
    fn short_traceback_under_budget_returns_verbatim() {
        let tb = "Traceback (most recent call last):\n\
                  \x20 File \"a.py\", line 1, in <module>\n    f()\n\
                  \x20 File \"b.py\", line 2, in f\n    g()\n\
                  \x20 File \"c.py\", line 3, in g\n    raise RuntimeError('boom')\n\
                  RuntimeError: boom";
        assert_eq!(truncate_python_error(tb), tb);
    }

    #[test]
    fn pre_traceback_noise_is_tail_trimmed() {
        let mut s = String::new();
        for i in 0..50 {
            s.push_str(&format!("progress: step {i}\n"));
        }
        s.push_str("Traceback (most recent call last):\n");
        for i in 0..6 {
            s.push_str(&format!(
                "  File \"/x/mod_{i}.py\", line {i}, in func\n    work()\n"
            ));
        }
        s.push_str("ValueError: bad value");

        let trimmed = truncate_python_error(&s);
        assert!(
            trimmed.contains("lines of pre-traceback stderr omitted"),
            "pre-traceback trim marker required: {trimmed:?}"
        );
        assert!(
            trimmed.contains("progress: step 49") || trimmed.contains("progress: step 48"),
            "must keep some recent progress lines: {trimmed:?}"
        );
        assert!(
            !trimmed.contains("progress: step 0\n"),
            "early progress should be dropped: {trimmed:?}"
        );
        assert!(trimmed.contains("Traceback "));
        assert!(trimmed.contains("ValueError: bad value"));
    }

    #[test]
    fn pure_stderr_noise_without_traceback_keeps_tail() {
        let mut s = String::new();
        for i in 0..50 {
            s.push_str(&format!("warning {i}\n"));
        }
        let trimmed = truncate_python_error(&s);
        assert!(
            trimmed.contains("lines of stderr omitted"),
            "trim marker required: {trimmed:?}"
        );
        assert!(
            trimmed.contains("warning 49"),
            "tail must be preserved: {trimmed:?}"
        );
        assert!(
            !trimmed.contains("warning 0\n"),
            "head should be dropped: {trimmed:?}"
        );
    }

    #[test]
    fn empty_stderr_returns_empty() {
        assert_eq!(truncate_python_error(""), "");
    }

    #[test]
    fn exception_preserved_when_last_frame_has_no_source_line() {
        let mut tb = String::from("Traceback (most recent call last):\n");
        for i in 0..40 {
            tb.push_str(&format!(
                "  File \"<frozen importlib._bootstrap>\", line {line}, in func_{i}\n",
                i = i,
                line = 1000 + i
            ));
        }
        tb.push_str("ModuleNotFoundError: No module named 'strategy_params'");

        let trimmed = truncate_python_error(&tb);
        assert!(
            trimmed.contains("ModuleNotFoundError: No module named 'strategy_params'"),
            "exception MUST be preserved even when no source line follows the last frame: {trimmed:?}"
        );
        assert!(
            trimmed.contains("frozen importlib"),
            "frame line preserved: {trimmed:?}"
        );
        assert!(
            trimmed.contains("frames omitted"),
            "trim marker present: {trimmed:?}"
        );
    }

    fn assert_within_byte_budget(input: &str, trimmed: &str) {
        let longest_marker = format!("[... {} bytes of stderr omitted ...]\n", input.len());
        assert!(
            trimmed.len() <= PY_ERROR_BYTE_BUDGET + longest_marker.len(),
            "result must fit the byte budget plus the marker, got {} bytes",
            trimmed.len()
        );
        assert!(
            trimmed.starts_with("[... ") && trimmed.contains("bytes of stderr omitted ...]\n"),
            "byte-trim marker required: {:?}",
            &trimmed[..trimmed.floor_char_boundary(120)]
        );
        let tail = &input[input.floor_char_boundary(input.len() - 64)..];
        assert!(
            trimmed.ends_with(tail),
            "the input's tail must survive: {:?}",
            &trimmed[trimmed.floor_char_boundary(trimmed.len() - 120)..]
        );
    }

    #[test]
    fn one_huge_stderr_line_is_clamped_to_the_byte_budget() {
        let line: String = (0..100 * 1024)
            .map(|i| char::from(b'a' + (i % 26) as u8))
            .collect();
        assert_within_byte_budget(&line, &truncate_python_error(&line));
    }

    #[test]
    fn a_huge_exception_line_is_clamped_to_the_byte_budget() {
        let mut tb = String::from("Traceback (most recent call last):\n");
        for i in 0..6 {
            tb.push_str(&format!(
                "  File \"/x/mod_{i}.py\", line {i}, in f\n    g()\n"
            ));
        }
        tb.push_str("ValueError: ");
        tb.extend((0..50 * 1024).map(|i| if i % 2 == 0 { 'é' } else { 'x' }));
        assert_within_byte_budget(&tb, &truncate_python_error(&tb));
    }

    #[test]
    fn module_not_found_realistic_shape() {
        let tb = "Traceback (most recent call last):\n\
                  \x20 File \"/x/data/apps/habit-tracker/scripts/_smoke_abs.py\", line 5, in <module>\n\
                  \x20   import big_candle_backtest as bcb\n\
                  ModuleNotFoundError: No module named 'big_candle_backtest'";
        assert_eq!(truncate_python_error(tb), tb);
    }

    #[test]
    fn script_error_output_prefers_stderr_and_keeps_the_end_of_stdout() {
        assert_eq!(script_error_output("boom\n", "log\n", 4), "boom\n");
        assert_eq!(script_error_output(" \n", "\n", 1), "");
        let long = format!("{}\nthe real error", "x".repeat(10_000));
        let tail = script_error_output("", &long, long.len() as u64);
        assert!(tail.ends_with("the real error"), "{tail}");
        assert!(tail.contains("bytes of stdout omitted"), "{tail}");
        assert!(tail.len() < 5_000, "{}", tail.len());
    }

    #[test]
    fn the_omitted_count_includes_what_the_reader_dropped() {
        let tail = script_error_output("", "last line", 1_009);
        assert_eq!(
            tail,
            "(stderr was empty, so this is the end of stdout)\n\
         [... 1000 bytes of stdout omitted ...]\nlast line"
        );
        assert!(!script_error_output("", "last line", 9).contains("omitted"));
    }
}
