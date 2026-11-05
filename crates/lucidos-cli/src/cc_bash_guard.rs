//! PreToolUse hook for Claude Code's `Bash` tool — blocks the kill patterns
//! that have caught the engine's CC subprocesses by accident.
//!
//! Background: a CC subprocess's argv names tools like `cargo` and `rustc`
//! (its `--allowedTools` list, for one). So a CC session running
//! `ps aux | grep -E "rustc|cargo" | xargs kill` to clean up a stuck cargo
//! build also matches other CC subprocesses. SIGTERM cascades and every
//! concurrent CC dies. `pkill -f cargo` and `kill $(pgrep -f cargo)` match
//! the same argv. This hook refuses the obvious patterns at the source.
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

const MSG_PS_OUTPUT_KILL: &str =
    "Refusing `ps`/`pgrep` output fed to `kill` (through `xargs`, a loop or \
     `$(…)`). Argv-based PID extraction \
     is unsafe inside CC: a claude subprocess's arguments can name `cargo`, \
     `rustc`, etc., so a `ps | grep cargo` match catches every running claude \
     and SIGTERMs it. Use `pkill -x \
     <name>` (exact process-name match, no `-f`) or capture the PID at launch \
     (`cargo build & PID=$!; kill $PID`).";

const MSG_KILL_CLAUDE: &str =
    "Refusing kill targeting `claude`. CC subprocesses are managed by the \
     Lucidos engine — a kill from inside CC would terminate every concurrent \
     CC session in this workspace. Stop a session via the engine UI, or run \
     `kill $(cat <workspace>/.lucidos/engine.pid)` from outside.";

const MSG_FULL_CMDLINE_KILL: &str =
    "Refusing `pkill -f`, and `pgrep -f` feeding `kill`. Full-command-line \
     matching is unsafe inside CC: a claude subprocess's arguments can name \
     `cargo`, `rustc`, etc., so the pattern matches every running claude and \
     kills it. Use `pkill -x <name>` (exact process-name match, no `-f`) or \
     capture the PID at launch (`cargo build & PID=$!; kill $PID`). The guard \
     reads the command as text, so a quoted mention trips it too: rephrase it.";

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
    // Bash joins a backslash-newline into one line before it parses words.
    let command = &command.replace("\\\n", " ");
    if matches_ps_output_kill(command) {
        return GuardDecision::Block(MSG_PS_OUTPUT_KILL);
    }
    if matches_kill_claude(command) {
        return GuardDecision::Block(MSG_KILL_CLAUDE);
    }
    if matches_full_cmdline_kill(command) {
        return GuardDecision::Block(MSG_FULL_CMDLINE_KILL);
    }
    GuardDecision::Allow
}

/// Option letters that take a value in both procps and BSD `pkill`/`pgrep`.
/// The rest of a short-flag cluster after one of them is that value.
const VALUE_OPTIONS: &str = "dFGgPstUu";

/// `pkill -f …` anywhere, or `pgrep -f …` in a command that also mentions the
/// word `kill`. The whole command counts, not one statement, so a PID list
/// carried through a variable or a `for` loop still trips it. Like the other
/// rules it reads text, not parsed commands: a quoted mention trips it too.
fn matches_full_cmdline_kill(cmd: &str) -> bool {
    passes_full_match(cmd, "pkill")
        || (passes_full_match(cmd, "pgrep") && contains_word(cmd, "kill"))
}

/// True when any mention of `tool` in `cmd` passes `-f` or `--full`, alone or
/// inside a combined short flag such as `-9f`. Options may follow the pattern,
/// because procps getopt permutes argv; `--` ends them.
fn passes_full_match(cmd: &str, tool: &str) -> bool {
    let mut rest = cmd;
    while let Some(pos) = find_word(rest, tool) {
        rest = &rest[pos + tool.len()..];
        let full = rest[..unquoted_boundary(rest)]
            .split_whitespace()
            .map(|arg| arg.trim_matches(|c| c == '"' || c == '\''))
            .take_while(|arg| *arg != "--")
            .any(is_full_match_flag);
        if full {
            return true;
        }
    }
    false
}

/// Byte index of the first statement or pipe boundary in `args` outside
/// quotes and escapes, so `'cargo|rustc'` or `cargo\|rustc` does not end it.
/// The scan starts unquoted and ignores `)` and backticks, so
/// `pkill -u $(id -u) -f` and `$(which pkill) -f` read through. Inside a
/// `bash -c "…"` script a `\"` is a real quote, so both readings run and the
/// longer one wins. Every guess makes the scan read further.
fn unquoted_boundary(args: &str) -> usize {
    boundary_scan(args, true).max(boundary_scan(args, false))
}

fn boundary_scan(args: &str, escapes_quotes: bool) -> usize {
    let mut quote = None;
    let mut escaped = false;
    for (i, c) in args.char_indices() {
        let is_quote = c == '"' || c == '\'';
        if std::mem::take(&mut escaped) && (escapes_quotes || !is_quote) {
            continue;
        }
        match (quote, c) {
            (Some('\''), '\'') => quote = None,
            (Some('\''), _) => {}
            (_, '\\') => escaped = true,
            (None, '"' | '\'') => quote = Some(c),
            (Some(open), _) if c == open => quote = None,
            (None, ';' | '\n' | '|' | '&') => return i,
            _ => {}
        }
    }
    args.len()
}

fn is_full_match_flag(arg: &str) -> bool {
    if let Some(long) = arg.strip_prefix("--") {
        // getopt_long accepts any unambiguous prefix, such as `--fu`.
        return !long.is_empty() && "full".starts_with(long);
    }
    let Some(short) = arg.strip_prefix('-') else {
        return false;
    };
    for letter in short.chars() {
        if letter == 'f' {
            return true;
        }
        if VALUE_OPTIONS.contains(letter) {
            return false;
        }
    }
    false
}

/// `ps` or `pgrep` output that can reach `kill`, as in
/// `ps aux | grep cargo | xargs kill` or `kill $(ps aux | grep cargo)`.
/// Extracting PIDs from argv-string matches is the failure mode that motivated
/// this hook. A `ps -p $PID` check on a captured PID pipes nothing, so it passes.
fn matches_ps_output_kill(cmd: &str) -> bool {
    contains_word(cmd, "kill")
        && ["ps", "pgrep"]
            .iter()
            .any(|producer| feeds_kill(cmd, producer))
}

/// True when a `producer` run pipes its output and `kill` follows, or runs
/// inside a substitution that pipes it, or is a `<(…)` feed. A check after the
/// kill, as in `kill $PID; ps aux | grep cargo`, does not count.
fn feeds_kill(cmd: &str, producer: &str) -> bool {
    let mut offset = 0;
    while let Some(pos) = find_word(&cmd[offset..], producer) {
        let start = offset + pos;
        offset = start + producer.len();
        let (before, after) = (&cmd[..start], &cmd[offset..]);
        if lists_containers(before.split_whitespace().next_back().unwrap_or("")) {
            continue;
        }
        let fed = before
            .rfind("<(")
            .is_some_and(|open| paren_depth(&before[open..]) > 0);
        let piped = has_single_pipe(after);
        if fed || (piped && (contains_word(after, "kill") || inside_substitution(before))) {
            return true;
        }
    }
    false
}

/// True when `host` is a command whose `ps` subcommand lists containers.
fn lists_containers(host: &str) -> bool {
    matches!(host, "docker" | "podman" | "container") || host.ends_with("compose")
}

/// True when `before` leaves a `(` or backtick open.
fn inside_substitution(before: &str) -> bool {
    paren_depth(before) > 0 || before.matches('`').count() % 2 == 1
}

/// How many `(` stay open at the end of `text`. A `)` with nothing open, such
/// as a `case` arm label, closes nothing.
fn paren_depth(text: &str) -> usize {
    text.chars().fold(0, |depth, c| match c {
        '(' => depth + 1,
        ')' => depth.saturating_sub(1),
        _ => depth,
    })
}

/// True when `s` holds a pipe `|` that is not half of a `||`.
fn has_single_pipe(s: &str) -> bool {
    let bytes = s.as_bytes();
    (0..bytes.len()).any(|i| {
        bytes[i] == b'|' && bytes.get(i + 1) != Some(&b'|') && (i == 0 || bytes[i - 1] != b'|')
    })
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
        // claude subprocess's argv (its system prompt, then on argv, named `cargo`).
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
    fn blocks_pkill_full_match() {
        assert_blocked("pkill -f cargo");
        assert_blocked("pkill --full cargo");
        assert_blocked("/usr/bin/pkill -f cargo");
        assert_blocked("pkill -f 'cargo build'");
    }

    #[test]
    fn blocks_pkill_full_inside_combined_short_flags() {
        assert_blocked("pkill -9f rustc");
        assert_blocked("pkill -fl cargo");
    }

    #[test]
    fn blocks_pkill_full_after_the_pattern() {
        // procps getopt permutes, so a trailing `-f` still applies.
        assert_blocked("pkill cargo -f");
        // A separator inside the quoted pattern does not end the arguments.
        assert_blocked("pkill 'cargo|rustc' -f");
        assert_blocked(r"pkill cargo\|rustc -f");
        // Inside `bash -c "…"` the escaped quotes are real ones.
        assert_blocked(r#"bash -c "pkill \"cargo|rustc\" -f""#);
        assert_blocked(r#"pkill "a\"b|c" -f"#);
        assert_blocked(r#"kill $(pgrep "a;b" -f)"#);
    }

    #[test]
    fn blocks_pkill_full_in_a_later_statement() {
        assert_blocked("cargo build; pkill -f rustc");
        assert_blocked("echo $(pkill -f rustc)");
    }

    #[test]
    fn blocks_kill_of_pgrep_full_substitution() {
        assert_blocked("kill $(pgrep -f rustc)");
        assert_blocked("kill -9 `pgrep -f rustc`");
        assert_blocked("kill $(pgrep --full rustc)");
    }

    #[test]
    fn blocks_kill_of_pgrep_full_through_a_variable() {
        assert_blocked("PIDS=$(pgrep -f rustc); kill $PIDS");
        assert_blocked("for p in $(pgrep -fd' ' rustc); do kill $p; done");
    }

    #[test]
    fn blocks_pkill_full_behind_a_wrapper() {
        assert_blocked("sudo pkill -f cargo");
        assert_blocked("sudo -u root pkill -f cargo");
        assert_blocked("timeout 10 pkill -f cargo");
        assert_blocked("setsid pkill -f cargo");
        assert_blocked(r#"FOO="1" pkill -f cargo"#);
        assert_blocked("bash -lc 'pkill -f cargo'");
        assert_blocked("if true; then pkill -f cargo; fi");
        assert_blocked("case $x in a) pkill -f cargo;; esac");
        assert_blocked("ls | xargs -n 1 pkill -f");
    }

    #[test]
    fn blocks_a_quoted_or_escaped_command_name() {
        assert_blocked(r#""pkill" -f cargo"#);
        assert_blocked(r"\pkill -f cargo");
    }

    #[test]
    fn blocks_a_command_name_from_a_substitution() {
        assert_blocked("$(which pkill) -f cargo");
        assert_blocked(r#"$(which "pkill") -f cargo"#);
        assert_blocked("`command -v pkill` -f rustc");
    }

    #[test]
    fn blocks_full_match_after_a_substituted_option_value() {
        assert_blocked("pkill -u $(id -u) -f cargo");
        assert_blocked("pkill -U `whoami` -f node");
        assert_blocked("kill $(pgrep -u $(id -u) -f rustc)");
    }

    #[test]
    fn blocks_full_match_after_a_stray_apostrophe() {
        assert_blocked("# don't touch other sessions\npkill 'cargo|rustc' -f");
    }

    #[test]
    fn allows_a_later_statement_flag_after_a_plain_pkill() {
        assert_allowed("pkill -x cargo; tail -f /var/log/app.log");
        assert_allowed("pkill -x node && docker logs -f web");
        assert_allowed("sh -c 'pkill -x node; tail -f app.log'");
    }

    #[test]
    fn blocks_ps_output_fed_to_kill_without_xargs() {
        assert_blocked("kill $(ps aux | grep cargo | awk '{print $2}')");
        assert_blocked("kill $(/bin/ps aux | grep cargo | awk '{print $2}')");
        assert_blocked("kill -9 `ps -ef | grep rustc | awk '{print $2}'`");
        assert_blocked("ps aux | grep cargo | awk '{print $2}' | while read p; do kill $p; done");
        assert_blocked("ps aux | awk '/cargo/ {print $2}' | { while read p; do kill $p; done; }");
        assert_blocked("for p in $(ps aux | grep cargo | awk '{print $2}'); do kill $p; done");
        assert_blocked("cd /tmp && ps aux | grep cargo | awk '{print $2}' | xargs kill");
        assert_blocked(
            "while read p; do kill $p; done < <(ps aux | grep cargo | awk '{print $2}')",
        );
        assert_blocked("kill $(ps -u $(id -u) -o pid=,args= | grep cargo | awk '{print $1}')");
        assert_blocked(r#"PS=$(ps aux); echo "$PS" | grep cargo | awk '{print $2}' | xargs kill"#);
        assert_blocked("kill $(awk '/cargo/ {print $2}' <(ps aux))");
        assert_blocked("kill $(awk '/cargo/ {print $2}' <(sudo /bin/ps aux))");
        assert_blocked("case $OS in Darwin) kill $(ps aux | grep cargo | awk '{print $2}');; esac");
    }

    #[test]
    fn allows_a_ps_check_after_killing_a_captured_pid() {
        assert_allowed("cargo run & PID=$!; sleep 5; kill $PID; sleep 1; ps aux | grep cargo");
    }

    #[test]
    fn allows_container_ps_feeding_docker_kill() {
        assert_allowed("docker-compose ps -q | xargs docker kill");
        assert_allowed("docker container ps -q | xargs docker kill");
    }

    #[test]
    fn allows_ps_that_does_not_feed_kill() {
        assert_allowed("ps -p $PID && kill $PID");
        assert_allowed("ps -p 1 || kill 1");
        assert_allowed("docker ps -q | xargs docker kill");
        assert_allowed(r#"if [ -n "$(ps -p $PID -o pid=)" ]; then kill $PID; fi"#);
    }

    #[test]
    fn blocks_full_match_on_a_later_line() {
        assert_blocked("cargo build\npkill -f cargo");
        assert_blocked("cd /tmp\nkill $(pgrep -f rustc)");
        assert_blocked("pgrep -f rustc | while read p; do sudo -u me kill $p; done");
    }

    #[test]
    fn blocks_a_quoted_mention_by_design() {
        // The guard reads text and errs toward refusing; the message says so.
        assert_blocked(r#"git commit -m "fix: refuse pkill -f""#);
        // Inside quotes the scan cannot tell where the script ends, so the
        // later `tail -f` counts as a pkill flag.
        assert_blocked("ssh box 'pkill -x cargo'; tail -f /var/log/app.log");
        let GuardDecision::Block(msg) = decide(r#"rg -n "pkill -f" scripts/"#) else {
            panic!("expected block");
        };
        assert!(msg.contains("quoted mention"), "{}", msg);
    }

    #[test]
    fn blocks_full_match_split_by_a_line_continuation() {
        assert_blocked("pkill \\\n  -f cargo");
        assert_blocked("kill $(pgrep \\\n -f rustc)");
    }

    #[test]
    fn blocks_an_abbreviated_full_option() {
        assert_blocked("pkill --fu cargo");
    }

    #[test]
    fn allows_an_f_inside_an_option_value() {
        assert_allowed("pkill -ufred cargo");
        assert_allowed("pkill -Gstaff cargo");
    }

    #[test]
    fn allows_narrow_pkill_and_pgrep() {
        assert_allowed("pkill -9 cargo");
        assert_allowed("pkill -x -u me cargo");
        assert_allowed("kill $(pgrep cargo)");
        assert_allowed("kill $(pgrep -x cargo)");
    }

    #[test]
    fn allows_pgrep_full_without_kill() {
        // Inspecting full command lines is harmless; feeding them to kill is not.
        assert_allowed("pgrep -f cargo");
        assert_allowed("pgrep -fl cargo | head");
    }

    #[test]
    fn allows_pkill_option_after_end_of_options() {
        // After `--`, `-f` is the pattern, not the full-match flag.
        assert_allowed("pkill -x -- -f");
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
        assert_allowed("pkill myproclaudeapp");
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
    fn block_message_for_full_match_mentions_pkill_x() {
        let GuardDecision::Block(msg) = decide("pkill -f cargo") else {
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
