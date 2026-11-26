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

const MSG_CD_WITH_REDIRECT: &str =
    "Refusing a `cd` combined with an output redirection (`>`, `>>`, `&>`, \
     `tee`). Claude Code's own safety check stops every such command for \
     manual approval, whatever the allow rules say, so the user would get a \
     permission card. Drop the `cd` and use absolute paths: \
     `cat > /tmp/dir/file.html` rather than `cd /tmp/dir && cat > file.html`. \
     The guard reads the command as text, so a `>` inside a heredoc body \
     counts too: write that file without the `cd`.";

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
    if matches_cd_with_redirect(command) {
        return GuardDecision::Block(MSG_CD_WITH_REDIRECT);
    }
    GuardDecision::Allow
}

fn matches_cd_with_redirect(cmd: &str) -> bool {
    runs_cd(cmd) && (has_output_redirect(cmd) || runs_tee(cmd))
}

fn runs_cd(cmd: &str) -> bool {
    in_command_position(cmd, "cd")
}

fn runs_tee(cmd: &str) -> bool {
    in_command_position(cmd, "tee")
}

const COMMAND_PREFIXES: [&str; 11] = [
    "if", "elif", "while", "until", "do", "then", "else", "!", "time", "builtin", "command",
];

fn is_assignment(word: &str) -> bool {
    word.split_once('=').is_some_and(|(name, _)| {
        name.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_')
            && name.bytes().all(is_word_char)
    })
}

fn in_command_position(cmd: &str, word: &str) -> bool {
    let mut offset = 0;
    while let Some(pos) = find_word(&cmd[offset..], word) {
        let start = offset + pos;
        offset = start + word.len();
        let lead = cmd[..start].trim_end_matches([' ', '\t']);
        let after_keyword = lead
            .rsplit(|c: char| c.is_whitespace() || ";&|(".contains(c))
            .next()
            .is_some_and(|last| COMMAND_PREFIXES.contains(&last) || is_assignment(last));
        if lead.is_empty() || lead.ends_with([';', '&', '|', '(', '{', '`', '\n']) || after_keyword
        {
            return true;
        }
    }
    false
}

fn has_output_redirect(cmd: &str) -> bool {
    let bytes = cmd.as_bytes();
    let mut quote = None;
    let mut escaped = false;
    for (i, &b) in bytes.iter().enumerate() {
        if std::mem::take(&mut escaped) {
            continue;
        }
        match (quote, b) {
            (_, b'\n') => quote = None,
            (Some(b'\''), b'\'') => quote = None,
            (Some(b'\''), _) => {}
            (_, b'\\') => escaped = true,
            (None, b'"' | b'\'') => quote = Some(b),
            (Some(b'"'), b'"') => quote = None,
            (None, b'>') if bytes.get(i.wrapping_sub(1)) != Some(&b'>') => {
                let mut next = i + 1;
                if matches!(bytes.get(next), Some(b'>' | b'|')) {
                    next += 1;
                }
                let copies_descriptor = bytes.get(next) == Some(&b'&')
                    && bytes
                        .get(next + 1)
                        .is_some_and(|c| c.is_ascii_digit() || *c == b'-');
                if !copies_descriptor && bytes.get(next) != Some(&b'(') {
                    return true;
                }
            }
            _ => {}
        }
    }
    false
}

const VALUE_OPTIONS: &str = "dFGgPstUu";

fn matches_full_cmdline_kill(cmd: &str) -> bool {
    passes_full_match(cmd, "pkill")
        || (passes_full_match(cmd, "pgrep") && contains_word(cmd, "kill"))
}

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

fn matches_ps_output_kill(cmd: &str) -> bool {
    contains_word(cmd, "kill")
        && ["ps", "pgrep"]
            .iter()
            .any(|producer| feeds_kill(cmd, producer))
}

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

fn lists_containers(host: &str) -> bool {
    matches!(host, "docker" | "podman" | "container") || host.ends_with("compose")
}

fn inside_substitution(before: &str) -> bool {
    paren_depth(before) > 0 || before.matches('`').count() % 2 == 1
}

fn paren_depth(text: &str) -> usize {
    text.chars().fold(0, |depth, c| match c {
        '(' => depth + 1,
        ')' => depth.saturating_sub(1),
        _ => depth,
    })
}

fn has_single_pipe(s: &str) -> bool {
    let bytes = s.as_bytes();
    (0..bytes.len()).any(|i| {
        bytes[i] == b'|' && bytes.get(i + 1) != Some(&b'|') && (i == 0 || bytes[i - 1] != b'|')
    })
}

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

fn check_standing_deny(command: &str) -> Result<Option<String>, String> {
    use crate::http::ConnectRetry as _;
    use std::time::{Duration, Instant};

    const RESTART_HINT: &str =
        "The engine may be restarting. Wait a few seconds and run the command again.";
    let workspace = crate::workspace::resolve_from_env()
        .map_err(|_| "could not resolve the engine address".to_string())?;
    let url = format!(
        "{}/api/v1/internal/command-deny-check",
        workspace.base_url()
    );
    let response_timeout = Duration::from_secs(5);
    let client = crate::http::client_with_timeout(response_timeout)
        .map_err(|_| "could not initialize the HTTP client".to_string())?;
    let start = Instant::now();
    let response = client
        .post(url)
        .timeout(response_timeout)
        .json(&serde_json::json!({ "command": command }))
        .send_with_connect_retry_within(Duration::from_secs(3))
        .map_err(|error| {
            if error.is_timeout() {
                format!("the engine request timed out. {RESTART_HINT}")
            } else if error.is_connect() {
                let cause = crate::http::root_cause(&error.without_url());
                let cause = if crate::http::contains_engine_token(&cause) {
                    "connection failed"
                } else {
                    &cause
                };
                format!(
                    "could not connect after {:.1}s ({cause}). {RESTART_HINT}",
                    start.elapsed().as_secs_f64()
                )
            } else {
                "the engine request failed".to_string()
            }
        })?;

    let status = response.status();
    if !status.is_success() {
        let body = response.json::<serde_json::Value>().ok();
        let reason = body
            .as_ref()
            .and_then(|body| body.get("reason"))
            .and_then(serde_json::Value::as_str)
            .filter(|reason| {
                !reason.is_empty()
                    && reason.len() <= 64
                    && reason.bytes().all(|byte| {
                        byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"_-".contains(&byte)
                    })
                    && !crate::http::contains_engine_token(reason)
            });
        return Err(match reason {
            Some(reason) => format!("engine returned HTTP {status} ({reason})"),
            None => format!("engine returned HTTP {status}"),
        });
    }

    let body = response.bytes().map_err(|error| {
        if error.is_timeout() {
            format!("the engine response timed out. {RESTART_HINT}")
        } else {
            "could not read the engine response".to_string()
        }
    })?;
    serde_json::from_slice(&body).map_err(|_| "could not parse the engine response".to_string())
}

pub(crate) fn run() -> Result<u8, BoxError> {
    run_hook(true)
}

pub(crate) fn run_command_deny() -> Result<u8, BoxError> {
    run_hook(false)
}

fn run_hook(claude_code: bool) -> Result<u8, BoxError> {
    let lucidos_session = !claude_code || std::env::var_os("LUCIDOS_THREAD_ID").is_some();
    let mut buf = String::new();
    if let Err(e) = std::io::stdin().read_to_string(&mut buf) {
        eprintln!("Could not read the command hook input: {e}");
        return Ok(if lucidos_session { 2 } else { 0 });
    }
    let payload: HookPayload = match serde_json::from_str(&buf) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("Could not parse the command hook input: {e}");
            return Ok(if lucidos_session { 2 } else { 0 });
        }
    };

    if lucidos_session {
        match check_standing_deny(&payload.tool_input.command) {
            Ok(None) => {}
            Ok(Some(reason)) => {
                eprintln!("{reason}");
                return Ok(2);
            }
            Err(reason) => {
                eprintln!(
                    "Refused by the command guard: could not verify standing denies: {reason}"
                );
                return Ok(2);
            }
        }
    }

    if !claude_code {
        return Ok(0);
    }
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
        assert_blocked("pkill cargo -f");
        assert_blocked("pkill 'cargo|rustc' -f");
        assert_blocked(r"pkill cargo\|rustc -f");
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
        assert_blocked(r#"git commit -m "fix: refuse pkill -f""#);
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
        assert_allowed("pgrep -f cargo");
        assert_allowed("pgrep -fl cargo | head");
    }

    #[test]
    fn allows_pkill_option_after_end_of_options() {
        assert_allowed("pkill -x -- -f");
    }

    #[test]
    fn allows_pkill_exact_match() {
        assert_allowed("pkill -x cargo");
    }

    #[test]
    fn allows_pkill_default_name_match() {
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
        assert_allowed("pkill myproclaudeapp");
    }

    #[test]
    fn word_boundary_ps_does_not_match_helper() {
        assert_allowed("./helps_kill_zombies.sh | xargs kill");
    }

    #[test]
    fn ignores_pkill_followed_by_other_target_in_same_statement() {
        assert_allowed("pkill foo");
    }

    #[test]
    fn blocks_the_cd_heredoc_command_that_prompted_the_user() {
        let cmd = "mkdir -p /tmp/toast-open && cd /tmp/toast-open && for v in a b; do\n\
                   label=$([ $v = a ] && echo \"Open\" || echo \"Open thread\")\n\
                   cat > $v.html <<EOF\n<div>$label</div>\nEOF\n\
                   echo \"\" > $v.css\ndone; cd - >/dev/null";
        let GuardDecision::Block(msg) = decide(cmd) else {
            panic!("expected block");
        };
        assert!(msg.contains("absolute paths"), "{}", msg);
    }

    #[test]
    fn blocks_cd_with_any_output_redirection() {
        assert_blocked("cd /tmp/x && echo hi > out.txt");
        assert_blocked("cd /tmp/x && echo hi >> out.txt");
        assert_blocked("cd /tmp/x; make 2> err.log");
        assert_blocked("cd /tmp/x && make &> build.log");
        assert_blocked("cd /tmp/x && echo hi >| out.txt");
        assert_blocked("echo hi > /tmp/x/out.txt && cd /tmp/x");
        assert_blocked("(cd crates && ls) > /tmp/list.txt");
        assert_blocked("for d in a b; do cd $d; ls > list.txt; done");
        assert_blocked("if true; then cd /tmp; fi; echo x > y");
        assert_blocked("cd /tmp/x && echo hi | tee out.txt");
        assert_blocked("cd crates && cargo build >&build.log");
        assert_blocked("builtin cd /tmp/x && echo hi > out.txt");
        assert_blocked("command cd /tmp/x && echo hi > out.txt");
        assert_blocked("if cd /tmp; then echo x > out; fi");
        assert_blocked("while cd ..; do ls > list.txt; done");
        assert_blocked("TMPDIR=/tmp cd /tmp && echo x > out");
    }

    #[test]
    fn blocks_cd_redirect_after_a_stray_apostrophe() {
        assert_blocked("cd /tmp/x\n# don't clobber\nls > out.txt");
        assert_blocked("cd /tmp/x && cat <<EOF\nit's\nEOF\nls > out.txt");
    }

    #[test]
    fn allows_cd_with_descriptor_copies_only() {
        assert_allowed("cd crates && cargo build 2>&1 | tail -5");
        assert_allowed("cd crates && echo oops >&2");
        assert_allowed("cd crates && diff <(ls) >(cat)");
        assert_allowed("cd crates && exec 3>&-");
    }

    #[test]
    fn allows_redirection_without_cd() {
        assert_allowed("echo hi > /tmp/out.txt");
        assert_allowed("cat > /tmp/x/a.html <<EOF\n<div>x</div>\nEOF");
        assert_allowed("git -C crates log > /tmp/log.txt");
        assert_allowed("echo hi | tee /tmp/out.txt");
    }

    #[test]
    fn allows_cd_without_redirection() {
        assert_allowed("cd crates && cargo test");
        assert_allowed(r#"cd crates && echo "a > b""#);
        assert_allowed("cd crates && echo 'a > b'");
    }

    #[test]
    fn ignores_cd_and_tee_outside_command_position() {
        assert_allowed("echo cd > /tmp/note.txt");
        assert_allowed("grep -r tee src > /tmp/hits.txt");
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
