//! PreToolUse hook for Claude Code's `Bash` tool — blocks the kill patterns
//! that have caught the engine's CC subprocesses by accident.
//!
//! Background: every CC subprocess is spawned with `--append-system-prompt
//! <huge string>`, and that string contains the words `cargo`, `rustc`, etc.
//! So a CC session running `ps aux | grep -E "rustc|cargo" | xargs kill` to
//! clean up a stuck cargo build also matches every other CC subprocess's
//! argv — SIGTERM cascades and every concurrent CC dies. This hook refuses
//! the obvious patterns at the source.
//!
//! Wired into `<workspace>/.lucidos/cc-settings.json` via the engine's
//! `cc_settings.rs`. Fails OPEN on parse / I/O errors so a hook bug can't
//! brick every Bash call.

use std::io::Read;

use serde::Deserialize;

use crate::workspace::BoxError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum GuardDecision {
    Allow,
    Block(&'static str),
}

const MSG_PS_XARGS_KILL: &str =
    "Refusing `ps`/`pgrep` piped to `xargs … kill`. Argv-based PID extraction \
     is unsafe inside CC: every claude subprocess has the words `cargo`, \
     `rustc`, etc. embedded in its `--append-system-prompt`, so a `ps | grep \
     cargo` match catches every running claude and SIGTERMs it. Use `pkill -x \
     <name>` (exact process-name match, no `-f`) or capture the PID at launch \
     (`cargo build & PID=$!; kill $PID`).";

const MSG_KILL_CLAUDE: &str =
    "Refusing kill targeting `claude`. CC subprocesses are managed by the \
     Lucidos engine — a kill from inside CC would terminate every concurrent \
     CC session in this workspace. Stop a session via the engine UI, or run \
     `kill $(cat <workspace>/.lucidos/engine.pid)` from outside.";

#[derive(Debug, Deserialize)]
struct HookPayload {
    tool_input: BashToolInput,
}

#[derive(Debug, Deserialize)]
struct BashToolInput {
    #[serde(default)]
    command: String,
}

pub(crate) fn decide(command: &str) -> GuardDecision {
    if matches_ps_xargs_kill(command) {
        return GuardDecision::Block(MSG_PS_XARGS_KILL);
    }
    if matches_kill_claude(command) {
        return GuardDecision::Block(MSG_KILL_CLAUDE);
    }
    GuardDecision::Allow
}

/// `<producer> | … | <xargs … kill …>` where producer is `ps` or `pgrep`.
/// `xargs kill` extracts PIDs from argv-string matches, which is the failure
/// mode that motivated this hook.
fn matches_ps_xargs_kill(cmd: &str) -> bool {
    let mut segments = cmd.split('|');
    let saw_producer = segments.any(|seg| {
        let first = seg.split_whitespace().next().unwrap_or("");
        first == "ps" || first == "pgrep"
    });
    saw_producer && segments.any(segment_runs_xargs_kill)
}

/// True when the segment invokes `xargs` with `kill` as the command it
/// executes. `xargs awk … | kill` is not the same shape — `xargs` runs its
/// argument directly, so `kill` must appear in the same segment, after
/// `xargs`.
fn segment_runs_xargs_kill(seg: &str) -> bool {
    match find_word(seg, "xargs") {
        Some(pos) => contains_word(&seg[pos + "xargs".len()..], "kill"),
        None => false,
    }
}

/// Catches `pkill <…> claude`, `killall <…> claude`, `pgrep <…> claude`,
/// `kill $(pgrep <…> claude)`. Subshell substitution is covered because the
/// inner `pgrep` is in the same statement string.
fn matches_kill_claude(cmd: &str) -> bool {
    for stmt in split_statements(cmd) {
        for tool in &["pkill", "killall", "pgrep"] {
            if let Some(pos) = find_word(stmt, tool) {
                if contains_word(&stmt[pos + tool.len()..], "claude") {
                    return true;
                }
            }
        }
    }
    false
}

/// Split on shell statement boundaries: `;`, newline, `&&`, `||`. Naive — does
/// not respect quotes — which means `echo "pkill claude"` would be blocked.
/// That false positive is acceptable; the literal almost never appears in real
/// commands and erring toward block is the safer side.
fn split_statements(cmd: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0usize;
    let bytes = cmd.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        let two = bytes.get(i + 1).copied();
        if b == b';' || b == b'\n' {
            out.push(&cmd[start..i]);
            start = i + 1;
            i += 1;
        } else if (b == b'&' && two == Some(b'&')) || (b == b'|' && two == Some(b'|')) {
            out.push(&cmd[start..i]);
            start = i + 2;
            i += 2;
        } else {
            i += 1;
        }
    }
    if start < cmd.len() {
        out.push(&cmd[start..]);
    }
    out
}

fn contains_word(s: &str, word: &str) -> bool {
    find_word(s, word).is_some()
}

fn find_word(haystack: &str, word: &str) -> Option<usize> {
    let bytes = haystack.as_bytes();
    let wb = word.as_bytes();
    if wb.is_empty() || wb.len() > bytes.len() {
        return None;
    }
    let mut i = 0;
    while i + wb.len() <= bytes.len() {
        if &bytes[i..i + wb.len()] == wb {
            let prev_ok = i == 0 || !is_word_char(bytes[i - 1]);
            let next_ok = i + wb.len() == bytes.len() || !is_word_char(bytes[i + wb.len()]);
            if prev_ok && next_ok {
                return Some(i);
            }
        }
        i += 1;
    }
    None
}

fn is_word_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

pub(crate) fn run() -> Result<u8, BoxError> {
    let mut buf = String::new();
    if let Err(e) = std::io::stdin().read_to_string(&mut buf) {
        // Fail open but make the failure visible — silent breakage would mean
        // the guard quietly stops working if the hook contract ever changes.
        eprintln!("cc-bash-guard: stdin read failed, allowing: {}", e);
        return Ok(0);
    }
    let payload: HookPayload = match serde_json::from_str(&buf) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("cc-bash-guard: payload parse failed, allowing: {}", e);
            return Ok(0);
        }
    };

    match decide(&payload.tool_input.command) {
        GuardDecision::Allow => Ok(0),
        GuardDecision::Block(reason) => {
            eprintln!("{}", reason);
            Ok(2)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_blocked(cmd: &str) {
        match decide(cmd) {
            GuardDecision::Block(_) => {}
            GuardDecision::Allow => panic!("expected Block, got Allow for: {}", cmd),
        }
    }

    fn assert_allowed(cmd: &str) {
        match decide(cmd) {
            GuardDecision::Allow => {}
            GuardDecision::Block(reason) => {
                panic!("expected Allow, got Block({}) for: {}", reason, cmd)
            }
        }
    }

    #[test]
    fn blocks_the_2026_05_10_incident_command() {
        // The exact command from the post-mortem (thread 8548dbd2, 09:35:44 local).
        // It killed every concurrent CC because `ps … | grep cargo` matches every
        // claude subprocess's argv (the system prompt mentions `cargo` 3 times).
        let cmd = r#"ps aux | grep -E "rustc|cargo" | grep -v grep | awk '{print $2}' | xargs -r kill 2>&1 | head -5; sleep 2; ps aux | grep -E "rustc.*chromiumoxide" | grep -v grep | wc -l"#;
        assert_blocked(cmd);
    }

    #[test]
    fn blocks_pgrep_piped_to_xargs_kill() {
        assert_blocked("pgrep -f cargo | xargs kill");
    }

    #[test]
    fn blocks_xargs_kill_with_signal_flag() {
        assert_blocked("ps aux | awk '/cargo/ {print $2}' | xargs kill -9");
    }

    #[test]
    fn blocks_pkill_claude() {
        assert_blocked("pkill claude");
    }

    #[test]
    fn blocks_pkill_dash_f_claude() {
        assert_blocked("pkill -f claude");
    }

    #[test]
    fn blocks_killall_claude() {
        assert_blocked("killall claude");
    }

    #[test]
    fn blocks_kill_subshell_pgrep_claude() {
        assert_blocked("kill $(pgrep -f claude)");
    }

    #[test]
    fn blocks_kill_minus_nine_subshell() {
        assert_blocked("kill -9 $(pgrep claude)");
    }

    #[test]
    fn blocks_inside_compound_statement() {
        // Statement separator `;` must not hide the bad sub-command.
        assert_blocked("echo before; pkill claude; echo after");
    }

    #[test]
    fn blocks_after_logical_and() {
        assert_blocked("true && killall claude");
    }

    #[test]
    fn allows_pkill_exact_match() {
        // The recommended replacement — exact name match cannot hit `claude`.
        assert_allowed("pkill -x cargo");
    }

    #[test]
    fn allows_pkill_default_name_match() {
        // Without `-f`, pkill defaults to process-name match, which is safe.
        assert_allowed("pkill cargo");
    }

    #[test]
    fn allows_specific_pid_kill() {
        assert_allowed("kill 12345");
    }

    #[test]
    fn allows_captured_pid_pattern() {
        assert_allowed("cargo build & PID=$!; kill $PID");
    }

    #[test]
    fn allows_pid_from_file() {
        assert_allowed("kill -9 $(cat /tmp/some.pid)");
    }

    #[test]
    fn allows_xargs_without_kill() {
        assert_allowed("ls *.log | xargs grep ERROR");
    }

    #[test]
    fn allows_ps_without_kill_pipeline() {
        assert_allowed("ps aux | grep cargo");
    }

    #[test]
    fn allows_unrelated_commands() {
        assert_allowed("git status");
        assert_allowed("cargo test -p lucidos-engine");
        assert_allowed("./scripts/web-dev.sh -w e2e-test -b");
    }

    #[test]
    fn word_boundary_does_not_match_substring() {
        // `claude` must be a whole word, not part of `proclaude` or similar.
        assert_allowed("pkill -f myproclaudeapp");
    }

    #[test]
    fn word_boundary_ps_does_not_match_helper() {
        // `helps` contains `ps` but is not the `ps` command.
        assert_allowed("./helps_kill_zombies.sh | xargs kill");
    }

    #[test]
    fn ignores_pkill_followed_by_other_target_in_same_statement() {
        // `pkill foo` then a separate `pkill claude` in next statement is still blocked.
        // But pkill foo on its own should be allowed.
        assert_allowed("pkill foo");
    }

    #[test]
    fn handles_empty_command() {
        assert_allowed("");
    }

    #[test]
    fn handles_whitespace_only() {
        assert_allowed("   \n  \t  ");
    }

    #[test]
    fn split_statements_respects_double_amp() {
        let parts = split_statements("a && b || c ; d\ne");
        assert_eq!(parts, vec!["a ", " b ", " c ", " d", "e"]);
    }

    #[test]
    fn block_message_for_xargs_pattern_mentions_pkill_x() {
        let GuardDecision::Block(msg) = decide("ps -ef | awk '{print $2}' | xargs kill") else {
            panic!("expected block");
        };
        assert!(
            msg.contains("pkill -x"),
            "guidance must point at the safe alternative: {}",
            msg
        );
    }

    #[test]
    fn block_message_for_claude_pattern_explains_blast_radius() {
        let GuardDecision::Block(msg) = decide("pkill claude") else {
            panic!("expected block");
        };
        assert!(
            msg.contains("Lucidos engine") || msg.contains("engine"),
            "must reference the engine so reader knows where the lifecycle lives: {}",
            msg
        );
    }
}
