use super::live_segments::{can_shadow_command, live_segments};
use crate::engine::command_guard::{self as guard, Catastrophe};
use std::cell::LazyCell;
use std::collections::HashMap;

const MAX_DEPTH: usize = 8;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Operation<'a> {
    Workspace(Fallback),
    Disk,
    NamedCommand(&'a str),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Fallback {
    Substring,
    Off,
}

impl Operation<'_> {
    fn conservative(self, text: &str) -> bool {
        match self {
            Self::Workspace(Fallback::Substring) => conservative(text),
            Self::Workspace(Fallback::Off) | Self::NamedCommand(_) => false,
            Self::Disk => guard::disk_operation_text(text),
        }
    }
}

fn finding(walk: impl Fn(Operation) -> bool) -> Option<Catastrophe> {
    if walk(Operation::Workspace(Fallback::Off)) {
        Some(Catastrophe::Proven(super::REASON))
    } else if walk(Operation::Workspace(Fallback::Substring)) {
        Some(Catastrophe::Unproven(super::REASON))
    } else {
        None
    }
}

pub(in crate::engine::command_guard) fn names_command(command: &str, name: &str) -> bool {
    shell(command, 0, command, Operation::NamedCommand(name))
}

fn conservative(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    let url = lower.replace("%2f", "/");
    (lower.contains("lucidos_")
        && (lower.contains("dropdb")
            || lower.contains("pg_ctl")
            || super::sql_drop_database(&lower)))
        || (url.contains("/~/api/v1/control/workspaces/") && lower.contains("delete"))
        || url.contains("/~/api/v1/control/workspace-delete-batches")
        || url.contains("/~/api/v1/control/cleanup")
        || url.contains("/~/api/v1/control/trash")
}

const PROOF: Operation<'static> = Operation::Workspace(Fallback::Off);

pub(super) fn destructive(command: &str) -> Option<Catastrophe> {
    finding(|operation| {
        inspect(command, 0, command, operation)
            || (operation == PROOF && conservative(command) && bound_database_operation(command))
    })
}

fn bound_database_operation(command: &str) -> bool {
    static ASSIGN: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(
            r#"^(export\s+)?([A-Za-z_]\w*)=('[^'\n]*'|"[^"\\$`\n]*"|[\w./:+@%,-]*)(?:\s+|$)"#,
        )
        .unwrap()
    });
    let Some(segments) = live_segments(command) else {
        return false;
    };
    let mut bindings = HashMap::new();
    let mut piped = None;
    let mut conditional = false;
    let mut follows_pipe = false;
    for segment in segments {
        let text = segment.text.trim();
        if !segment.inputs.is_empty() || can_shadow_command(text) {
            return false;
        }
        let previous_pipe = std::mem::replace(&mut follows_pipe, segment.piped);
        let depends_on_previous = std::mem::replace(&mut conditional, segment.conditional);
        let mut local_pgdata = None;
        let mut executable = text;
        if let Some(assign) = ASSIGN.captures(text) {
            let name = assign.get(2).unwrap().as_str();
            let value = assign.get(3).unwrap().as_str();
            let value = if value.starts_with(['\'', '"']) {
                &value[1..value.len() - 1]
            } else {
                value
            };
            let tail = &text[assign.get(0).unwrap().end()..];
            if tail.is_empty() {
                piped = None;
                bindings.remove(name);
                if !depends_on_previous && !previous_pipe && !segment.piped && !segment.backgrounded
                {
                    bindings.insert(name.to_string(), value.to_string());
                }
                continue;
            }
            if name == "PGDATA" && assign.get(1).is_none() {
                local_pgdata = Some(value);
                executable = tail;
            }
        }
        let head = executable.split_whitespace().next().unwrap_or_default();
        match head {
            "pg_ctl" | "dropdb" | "psql" | "echo" | "printf" => {
                let resolved = resolve_shell_references(executable, &bindings);
                if piped_sql_drop(piped.take(), &resolved, PROOF) {
                    return true;
                }
                let resolved = if head == "pg_ctl" {
                    local_pgdata
                        .or_else(|| bindings.get("PGDATA").map(String::as_str))
                        .filter(|path| !path.is_empty() && !path.starts_with('-'))
                        .map(|path| format!("{resolved} -D {}", guard::shell_quoted(path)))
                        .unwrap_or(resolved)
                } else {
                    resolved
                };
                if matches!(head, "pg_ctl" | "dropdb" | "psql")
                    && self::segment(&resolved, 0, &resolved, PROOF)
                {
                    return true;
                }
                piped = segment.piped.then(|| echoed_text(&resolved)).flatten();
            }
            "" | "cd" | "true" | ":" => piped = None,
            _ => return false,
        }
    }
    false
}

fn resolve_shell_references(command: &str, bindings: &HashMap<String, String>) -> String {
    static REFERENCE: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r"^\$(?:\{([A-Za-z_]\w*)\}|([A-Za-z_]\w*))").unwrap()
    });
    let mut quotes = guard::QuoteState::default();
    let mut out = String::with_capacity(command.len());
    let mut i = 0;
    while i < command.len() {
        if let Some(end) = quotes.step(command, i) {
            let end = end.min(command.len());
            out.push_str(&command[i..end]);
            i = end;
            continue;
        }
        if command[i..].starts_with('$') {
            if let Some(reference) = REFERENCE.captures(&command[i..]) {
                let name = reference
                    .get(1)
                    .or_else(|| reference.get(2))
                    .unwrap()
                    .as_str();
                if let Some(value) = bindings.get(name) {
                    if quotes.in_double {
                        for c in value.chars() {
                            if matches!(c, '\\' | '"' | '$' | '`') {
                                out.push('\\');
                            }
                            out.push(c);
                        }
                        i += reference.get(0).unwrap().end();
                        continue;
                    }
                    let literal = guard::shell_quoted(value);
                    if literal == *value {
                        out.push_str(&literal);
                        i += reference.get(0).unwrap().end();
                        continue;
                    }
                }
            }
        }
        let c = command[i..].chars().next().unwrap();
        out.push(c);
        i += c.len_utf8();
    }
    out
}

pub(super) fn destructive_python(code: &str) -> Option<Catastrophe> {
    finding(|operation| python(code, 0, code, operation))
}

pub(super) fn disk(command: &str, is_python: bool) -> bool {
    if is_python {
        python(command, 0, command, Operation::Disk)
    } else {
        // INVARIANT: Leading shell comments cannot supply operands or executable input.
        // Exclude them from the fallback context as well as the operation walk.
        // Do not strip inline text or interpreter input with shell rules.
        let mut command = command.trim_start_matches([' ', '\t', '\n']);
        while command.starts_with('#') {
            command = command
                .split_once('\n')
                .map_or("", |(_, rest)| rest.trim_start_matches([' ', '\t', '\n']));
        }
        inspect(command, 0, command, Operation::Disk)
    }
}

fn inspect(command: &str, depth: usize, context: &str, operation: Operation) -> bool {
    if depth >= MAX_DEPTH {
        return operation.conservative(context);
    }
    if matches!(operation, Operation::NamedCommand(_)) {
        return shell(command, depth + 1, context, operation);
    }
    let first = command
        .lines()
        .map(str::trim)
        .find(|s| !s.is_empty() && !s.starts_with('#'))
        .unwrap_or("");
    if first.starts_with("import ") || first.starts_with("from ") || first.starts_with("print(") {
        return python(command, depth + 1, context, operation);
    }
    shell(command, depth + 1, context, operation)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum InputPipeline {
    Open,
    Closed,
    Piped,
}

struct PendingHeredoc {
    doc: guard::Heredoc,
    consumer: usize,
    posix_consumer: Option<usize>,
    pipeline: InputPipeline,
}

fn shell(command: &str, depth: usize, context: &str, operation: Operation) -> bool {
    let mut quotes = guard::QuoteState::default();
    let mut pending: Vec<PendingHeredoc> = Vec::new();
    let mut piped: Option<String> = None;
    let mut open_inputs = 0;
    let mut start = 0;
    let mut posix_start: Option<usize> = None;
    let mut i = 0;
    let mut word_start = true;
    while i < command.len() {
        if let Some(next) = quotes.step_all_quotes(command, i) {
            if !command[i..].starts_with("\\\n") {
                word_start = false;
            }
            i = next;
            continue;
        }
        if let Some(end) = guard::expansion_end(command, i) {
            word_start = false;
            i = end;
            continue;
        }
        if ["$(", "<(", ">("]
            .iter()
            .any(|s| command[i..].starts_with(s))
            || command.as_bytes()[i] == b'`'
        {
            if let Some(span) = guard::substitution_spans(&command[i..]).first() {
                if inspect(
                    &command[i..][span.body.clone()],
                    depth + 1,
                    context,
                    operation,
                ) {
                    return true;
                }
                i += span.whole.end;
                word_start = false;
                continue;
            }
        }
        let byte = command.as_bytes()[i];
        // INVARIANT: Redirect modifiers are plumbing, not control operators. Keep the
        // consumer intact across `2>&1`, `&>>file`, `>|file` and `>>!file`.
        // The quote-aware traversal places the operator; the ordinary argv
        // reader places its target, including a quoted or concatenated path.
        if command[i..].starts_with("&>") {
            // INVARIANT: Bash/zsh attach this redirect to the preceding command. A
            // POSIX shell instead backgrounds that command and starts a new
            // one at `>`. Unwrapping loses the interpreter, so inspect both
            // readings rather than choosing this machine's shell semantics.
            // Finish only the current POSIX prefix, then retain its next
            // start alongside the unchanged Bash start. Sibling redirects
            // spend no recursion depth and no suffix is scanned repeatedly.
            if segment(
                &command[posix_start.unwrap_or(start)..i],
                depth,
                context,
                operation,
            ) {
                return true;
            }
            posix_start = Some(i + 1);
            i += 1;
            continue;
        }
        if command[i..].starts_with("<&") {
            i += 2;
            word_start = false;
            continue;
        }
        if byte == b'>' {
            let mut target = i + 1;
            if command[target..].starts_with('>') {
                target += 1;
            }
            if command[target..].starts_with(['|', '!', '&']) {
                target += 1;
            }
            if operation == Operation::Disk && disk_redirect_target(command, target) {
                return true;
            }
            i = target;
            word_start = false;
            continue;
        }
        if byte == b'#' && word_start {
            if piped_sql_drop(piped.take(), &command[start..i], operation)
                || segment_readings(
                    &command[start..i],
                    posix_start.map(|at| &command[at..i]),
                    depth,
                    context,
                    operation,
                )
            {
                return true;
            }
            i = command[i..].find('\n').map_or(command.len(), |n| i + n);
            start = i;
            posix_start = None;
            continue;
        }
        // INVARIANT: A compound can pipe a heredoc after its terminator or after a
        // separator inside the group. This simple-command walk cannot prove
        // that stdin stays data there, so retain the original conservative
        // floor. Protected expansions, arithmetic and quoted data were
        // already skipped above. No signal means keep scanning, not Safe.
        let brace_group = byte == b'{'
            && word_start
            && command
                .as_bytes()
                .get(i + 1)
                .is_none_or(|next| guard::ends_shell_word(*next));
        if (byte == b'(' || brace_group) && operation.conservative(context) {
            return true;
        }
        if command[i..].starts_with("<<<") {
            if operation.conservative(context) {
                return true;
            }
            i += 3;
            word_start = true;
            continue;
        }
        if command[i..].starts_with("<<") {
            let Some((doc, end)) = guard::heredoc_opener(command, i + 2) else {
                return operation.conservative(context);
            };
            pending.push(PendingHeredoc {
                doc,
                consumer: start,
                posix_consumer: posix_start,
                pipeline: InputPipeline::Open,
            });
            i = end;
            word_start = false;
            continue;
        }
        if b";|&\n".contains(&byte) {
            let conditional = command[i..].starts_with("||") || command[i..].starts_with("&&");
            let separator_len = if conditional || command[i..].starts_with("|&") {
                2
            } else {
                1
            };
            for input in &mut pending[open_inputs..] {
                input.pipeline = if byte == b'|' && !conditional {
                    InputPipeline::Piped
                } else {
                    InputPipeline::Closed
                };
            }
            open_inputs = pending.len();
            let current = &command[start..i];
            if piped_sql_drop(piped.take(), current, operation) {
                return true;
            }
            if byte == b'|' && !conditional {
                piped = echoed_text(current);
            }
            if segment_readings(
                &command[start..i],
                posix_start.map(|at| &command[at..i]),
                depth,
                context,
                operation,
            ) {
                return true;
            }
            posix_start = None;
            if byte == b'\n' && !pending.is_empty() {
                let mut at = i + 1;
                for PendingHeredoc {
                    doc,
                    consumer,
                    posix_consumer,
                    pipeline,
                } in pending.drain(..)
                {
                    let Some((body_end, end)) = guard::heredoc_body_end(command, at, &doc) else {
                        return operation.conservative(context);
                    };
                    let body = &command[at..body_end];
                    if (!doc.literal
                        && guard::substitution_bodies(body)
                            .iter()
                            .any(|s| inspect(s, depth + 1, context, operation)))
                        || stdin(
                            &command[consumer..i],
                            body,
                            depth,
                            context,
                            doc.literal,
                            pipeline == InputPipeline::Piped,
                            operation,
                        )
                        || posix_consumer.is_some_and(|consumer| {
                            stdin(
                                &command[consumer..i],
                                body,
                                depth,
                                context,
                                doc.literal,
                                pipeline == InputPipeline::Piped,
                                operation,
                            )
                        })
                    {
                        return true;
                    }
                    at = end;
                }
                i = at;
                start = at;
                open_inputs = 0;
                word_start = true;
                continue;
            }
            start = i + separator_len;
            i += separator_len - 1;
        }
        word_start = b" \t\r\n;|&()".contains(&byte);
        i += guard::char_len_at(command, i);
    }
    piped_sql_drop(piped, &command[start..], operation)
        || segment_readings(
            &command[start..],
            posix_start.map(|at| &command[at..]),
            depth,
            context,
            operation,
        )
}

fn piped_sql_drop(input: Option<String>, consumer: &str, operation: Operation) -> bool {
    operation == PROOF
        && input.is_some_and(|sql| {
            let words = guard::shell_argv(consumer);
            let refs: Vec<_> = words.iter().map(|w| w.text.as_str()).collect();
            guard::danger_head_candidates(&refs)
                .iter()
                .any(|(head, args)| {
                    head == "psql"
                        && !args.iter().any(|a| {
                            ["-c", "--command", "-f", "--file"]
                                .iter()
                                .any(|flag| a.starts_with(flag))
                        })
                })
                && sql_drop(&sql, operation)
        })
}

fn echoed_text(segment: &str) -> Option<String> {
    let words = guard::shell_argv(segment);
    if words.iter().any(|w| w.expands) {
        return None;
    }
    let refs: Vec<_> = words.iter().map(|w| w.text.as_str()).collect();
    let head = guard::command_head_index(&refs);
    match refs.get(head..)? {
        ["echo", args @ ..]
            if args
                .iter()
                .all(|arg| !arg.starts_with('-') && !arg.contains('\\')) =>
        {
            Some(args.join(" "))
        }
        ["printf", "%s" | "%s\\n", text] => Some((*text).to_string()),
        ["printf", format] => {
            let text = format.strip_suffix("\\n").unwrap_or(format);
            (!text.contains(['%', '\\']) && !text.starts_with('-')).then(|| text.to_owned())
        }
        _ => None,
    }
}

fn segment_readings(
    main: &str,
    posix: Option<&str>,
    depth: usize,
    context: &str,
    operation: Operation,
) -> bool {
    segment(main, depth, context, operation)
        || posix.is_some_and(|command| segment(command, depth, context, operation))
}

fn disk_redirect_target(command: &str, mut at: usize) -> bool {
    while command
        .as_bytes()
        .get(at)
        .is_some_and(u8::is_ascii_whitespace)
    {
        at += 1;
    }
    let start = at;
    let mut quotes = guard::QuoteState::default();
    while at < command.len() {
        if let Some(next) = quotes.step_all_quotes(command, at) {
            at = next;
        } else if guard::ends_shell_word(command.as_bytes()[at]) {
            break;
        } else {
            at += guard::char_len_at(command, at);
        }
    }
    guard::shell_argv(&command[start..at])
        .first()
        .is_some_and(|word| guard::is_disk_device(&word.text))
}

fn stdin(
    consumer: &str,
    body: &str,
    depth: usize,
    context: &str,
    literal: bool,
    piped: bool,
    operation: Operation,
) -> bool {
    let words = guard::shell_argv(consumer);
    let refs: Vec<_> = words.iter().map(|w| w.text.as_str()).collect();
    if piped {
        return operation.conservative(context) || inspect(body, depth + 1, context, operation);
    }
    guard::danger_head_candidates(&refs)
        .iter()
        .any(|(head, args)| match head.as_str() {
            h if guard::DATA_SINK_HEADS.contains(&h) => {
                !bare_head(&refs, args, h) && operation.conservative(context)
            }
            "xargs" | "gxargs" | "parallel" => operation.conservative(context),
            h if guard::EXEC_WRAPPER_HEADS.contains(&h)
                || guard::EXEC_WRAPPER_HEADS_WITH_OPERAND.contains(&h) =>
            {
                false
            }
            h if matches!(operation, Operation::NamedCommand(_)) => {
                guard::GUARD_SHELLS.contains(&h) && inspect(body, depth + 1, context, operation)
            }
            _ if !literal && body.contains('$') => operation.conservative(context),
            "psql" if matches!(operation, Operation::Workspace(_)) => sql_drop(body, operation),
            h if guard::GUARD_SHELLS.contains(&h) => inspect(body, depth + 1, context, operation),
            h if h.starts_with("python") => python(body, depth + 1, context, operation),
            _ => operation.conservative(context),
        })
}

fn segment(command: &str, depth: usize, context: &str, operation: Operation) -> bool {
    if depth >= MAX_DEPTH {
        return operation.conservative(context);
    }
    if guard::substitution_bodies(command)
        .iter()
        .any(|s| inspect(s, depth + 1, context, operation))
    {
        return true;
    }
    let inner = guard::unwrap_shell_command(command);
    if inner != command && !matches!(operation, Operation::NamedCommand(_)) {
        return inspect(&inner, depth + 1, context, operation);
    }
    let words = guard::shell_argv(command);
    let refs: Vec<_> = words.iter().map(|w| w.text.as_str()).collect();
    if let Operation::NamedCommand(name) = operation {
        return guard::danger_head_candidates(&refs)
            .iter()
            .any(|(head, _)| head == name)
            || (inner != command && inspect(&inner, depth + 1, context, operation));
    }
    if refs
        .get(guard::command_head_index(&refs))
        .is_some_and(|head| {
            matches!(
                guard::normalized_head(head),
                "if" | "then"
                    | "else"
                    | "elif"
                    | "fi"
                    | "while"
                    | "until"
                    | "for"
                    | "select"
                    | "do"
                    | "done"
                    | "case"
                    | "esac"
                    | "function"
                    | "coproc"
                    | "time"
                    | "[["
            )
        })
        && operation.conservative(context)
    {
        return true;
    }
    if refs.iter().any(|w| guard::is_code_injecting_assignment(w))
        && operation.conservative(context)
    {
        return true;
    }
    let proven_inert = LazyCell::new(|| segment_proven_inert(command));
    guard::executed_head_candidates(&refs)
        .iter()
        .any(|(head, args)| match head.as_str() {
            h if operation == Operation::Disk && (h == "mkfs" || h.starts_with("mkfs.")) => {
                args.iter().any(|arg| guard::is_disk_device(arg))
                    || (words.iter().any(|word| word.expands) && operation.conservative(context))
            }
            "dd" if operation == Operation::Disk => {
                args.iter()
                    .any(|arg| arg.strip_prefix("of=").is_some_and(guard::is_disk_device))
                    || (words.iter().any(|word| word.expands) && operation.conservative(context))
            }
            "dropdb" | "pg_ctl" | "psql" | "curl" | "wget" if operation == Operation::Disk => {
                operation.conservative(context)
            }
            "dropdb" if words.iter().any(|w| w.expands) => operation.conservative(context),
            "dropdb" => database_operand(args),
            "pg_ctl" => {
                let harmless = args
                    .iter()
                    .any(|s| matches!(*s, "status" | "--version" | "-V" | "--help" | "-?"))
                    && !args.iter().any(|s| {
                        matches!(
                            *s,
                            "stop" | "restart" | "kill" | "promote" | "start" | "reload"
                        )
                    });
                let target = args
                    .windows(2)
                    .find_map(|w| matches!(w[0], "-D" | "--pgdata").then_some(w[1]))
                    .or_else(|| {
                        args.iter().find_map(|a| {
                            a.strip_prefix("--pgdata=")
                                .or_else(|| a.strip_prefix("-D").filter(|v| !v.is_empty()))
                        })
                    });
                !harmless
                    && target
                        .filter(|_| !words.iter().any(|w| w.expands))
                        .map_or_else(
                            || operation.conservative(context),
                            |p| p.to_ascii_lowercase().contains("lucidos_"),
                        )
            }
            "psql" | "curl" | "wget" if words.iter().any(|w| w.expands) => {
                operation.conservative(context)
            }
            "psql" => psql(args, command, context, operation),
            "curl" | "wget" => {
                (guard::http_args_hide_options(head, args) && operation.conservative(context))
                    || control_request(args, operation)
            }
            h if h.starts_with("python") => {
                if let Some(code) = args.windows(2).find_map(|w| (w[0] == "-c").then_some(w[1])) {
                    python(code, depth + 1, context, operation)
                } else {
                    !heredoc_is_code_input(command, args) && operation.conservative(context)
                }
            }
            h if guard::GUARD_SHELLS.contains(&h) && heredoc_is_code_input(command, args) => false,
            "cd" => false,
            h if guard::DATA_SINK_HEADS.contains(&h) => {
                !bare_head(&refs, args, h) && operation.conservative(context)
            }
            "rg" | "grep" | "which" => {
                guard::args_have_exec_capable_flag(args) && operation.conservative(context)
            }
            // INVARIANT: The invoked command's operands may come from stdin, so its
            // argv alone cannot prove that it avoids a protected target.
            "xargs" | "gxargs" | "parallel" => operation.conservative(context),
            h if guard::EXEC_WRAPPER_HEADS.contains(&h)
                || guard::EXEC_WRAPPER_HEADS_WITH_OPERAND.contains(&h) =>
            {
                false
            }
            h if INERT_READERS.contains(&h) && *proven_inert => false,
            // INVARIANT: An unknown program can run a file written by an earlier sink,
            // or change how a later command resolves (for example, `hash`).
            _ => operation.conservative(context),
        })
}

/// INVARIANT: Heads the floor exempts in a segment proven to only read. None evaluates
/// an operand as code, as `test -v` evaluates an array subscript. None runs a
/// command that a config file names, as git runs hooks and `ack` a pager.
const INERT_READERS: [&str; 16] = [
    "sed", "sort", "find", "nl", "cut", "tr", "diff", "cmp", "stat", "file", "du", "jq", "column",
    "basename", "dirname", "realpath",
];

/// INVARIANT: True when `command` only reads ([`guard::script_proven_read_only`]) and
/// expands nothing that can run code. zsh's `${(e)x}` and an arithmetic
/// `$((x))` both run a command substitution that the value of `x` holds.
fn segment_proven_inert(command: &str) -> bool {
    !command.contains('`')
        && !command.contains("$(")
        && !command.contains("${")
        && guard::script_proven_read_only(command)
}

pub(super) fn bare_head(words: &[&str], args: &[&str], head: &str) -> bool {
    words.get(words.len() - args.len() - 1) == Some(&head)
}

fn heredoc_is_code_input(command: &str, args: &[&str]) -> bool {
    command.contains("<<")
        && args
            .iter()
            .take_while(|a| !a.starts_with('<'))
            .all(|a| a.starts_with('-'))
}

fn database_operand(args: &[&str]) -> bool {
    let mut skip = false;
    args.iter().any(|arg| {
        if std::mem::take(&mut skip) {
            return false;
        }
        if matches!(
            *arg,
            "-h" | "-p" | "-U" | "--host" | "--port" | "--username" | "--maintenance-db"
        ) {
            skip = true;
            return false;
        }
        if arg.starts_with('-') {
            return false;
        }
        arg.parse::<sqlx::postgres::PgConnectOptions>().map_or_else(
            |_| arg.to_ascii_lowercase().contains("lucidos_"),
            |opts| {
                opts.get_database()
                    .is_some_and(|db| db.to_ascii_lowercase().starts_with("lucidos_"))
            },
        )
    })
}

fn psql(args: &[&str], command: &str, context: &str, operation: Operation) -> bool {
    let sql: Vec<_> = args
        .windows(2)
        .filter_map(|w| matches!(w[0], "-c" | "--command").then_some(w[1]))
        .chain(args.iter().filter_map(|a| {
            a.strip_prefix("--command=")
                .or_else(|| a.strip_prefix("-c").filter(|s| !s.is_empty()))
        }))
        .collect();
    let unknown_input = args.iter().any(|a| {
        a.starts_with("-v")
            || a.starts_with("--set")
            || a.starts_with("-f")
            || a.starts_with("--file")
    }) || (sql.is_empty() && !command.contains("<<"));
    (unknown_input && operation.conservative(context)) || sql.iter().any(|s| sql_drop(s, operation))
}

fn control_request(args: &[&str], operation: Operation) -> bool {
    if !guard::is_mutating_http(args) {
        return false;
    }
    let delete = args.iter().enumerate().any(|(i, arg)| {
        request_method(arg)
            .and_then(|m| {
                if m.is_empty() {
                    args.get(i + 1).copied()
                } else {
                    Some(m)
                }
            })
            .is_some_and(|m| m.eq_ignore_ascii_case("DELETE"))
    });
    let mut skip = false;
    args.iter().any(|arg| {
        if std::mem::take(&mut skip) {
            return false;
        }
        if matches!(
            *arg,
            "-H" | "--header" | "-d" | "--data" | "--data-raw" | "--json" | "-o" | "--output"
        ) {
            skip = true;
            return false;
        }
        let url = arg
            .strip_prefix("--url=")
            .unwrap_or(arg)
            .to_ascii_lowercase()
            .replace("%2f", "/");
        if url.starts_with('-') || url.contains(char::is_whitespace) {
            return false;
        }
        let absolute = if url.contains("://") {
            url.clone()
        } else {
            format!("http://{url}")
        };
        let Ok(parsed) = reqwest::Url::parse(&absolute) else {
            return operation.conservative(&url);
        };
        let path = parsed.path();
        (delete && path.starts_with("/~/api/v1/control/workspaces/"))
            || path == "/~/api/v1/control/workspace-delete-batches"
            || path.starts_with("/~/api/v1/control/cleanup")
            || path.starts_with("/~/api/v1/control/trash")
    })
}

fn request_method(arg: &str) -> Option<&str> {
    if let Some(guard::ShortClusterSignal::Method(method)) = guard::short_cluster_signal(arg) {
        return Some(method);
    }
    let long = arg.strip_prefix("--")?;
    let long = long.strip_prefix("expand-").unwrap_or(long);
    let (name, value) = long.split_once('=').unwrap_or((long, ""));
    (name.len() >= 2 && ["request", "method"].iter().any(|f| f.starts_with(name))).then_some(value)
}

fn python(code: &str, depth: usize, context: &str, operation: Operation) -> bool {
    if depth >= MAX_DEPTH {
        return operation.conservative(context);
    }
    if operation == PROOF
        && python_evident_shell_outs(code)
            .iter()
            .any(|cmd| inspect(cmd, depth + 1, context, operation))
    {
        return true;
    }
    let Some(calls) = guard::python_shell_outs(code) else {
        return operation.conservative(context);
    };
    if calls
        .commands
        .iter()
        .any(|cmd| inspect(cmd, depth + 1, context, operation))
    {
        return true;
    }
    let Some(executable) = python_without_literals(&calls.rest) else {
        return operation.conservative(context);
    };
    let compact: String = executable.chars().filter(|c| !c.is_whitespace()).collect();
    // INVARIANT: The existing side-effect proof is deliberately broad. Requiring known
    // data calls as well prevents an unknown callable from receiving an inert
    // string exemption, including an alias of an execution primitive.
    let unknown_execution =
        guard::python_side_effect_signal(&compact) || !python_data_calls(&executable);
    unknown_execution && operation.conservative(context)
}

pub(super) fn python_evident_shell_outs(code: &str) -> Vec<String> {
    let Some(words) = python_identifiers(code) else {
        return Vec::new();
    };
    let mut commands = os_shell_literals(code, &words);
    if let Some(resolved) = python_with_bindings(code, &words, false) {
        if let Some(outs) = guard::python_shell_outs(&resolved) {
            commands.extend(outs.commands);
        }
        if let Some(words) = python_identifiers(&resolved) {
            commands.extend(os_shell_literals(&resolved, &words));
        }
    }
    commands
}

pub(super) fn python_source_literals(code: &str) -> Vec<(String, bool)> {
    let Some(words) = python_identifiers(code) else {
        return Vec::new();
    };
    if !words
        .iter()
        .any(|w| matches!(&code[w.clone()], "exec" | "eval" | "compile"))
    {
        return Vec::new();
    }
    let resolved = python_with_bindings(code, &words, true);
    let (code, words) = if let Some(code) = resolved.as_deref() {
        let Some(words) = python_identifiers(code) else {
            return Vec::new();
        };
        (code, words)
    } else {
        (code, words)
    };
    words
        .iter()
        .filter_map(|word| {
            let name = &code[word.clone()];
            if !matches!(name, "exec" | "eval" | "compile")
                || code[..word.start].trim_end().ends_with('.')
            {
                return None;
            }
            let argument = code[word.end..]
                .trim_start()
                .strip_prefix('(')?
                .trim_start();
            let offset = code.len() - argument.len();
            let (source, end) = python_literal_raw(code, offset, true)?;
            let (source, decoded) = if code[offset..].starts_with(['r', 'R']) {
                (source.to_string(), true)
            } else {
                decode_simple_escapes(source)
            };
            code[end..]
                .trim_start()
                .starts_with([',', ')'])
                .then(|| (source, decoded && name != "compile"))
        })
        .collect()
}

pub(super) fn python_identifiers(code: &str) -> Option<Vec<std::ops::Range<usize>>> {
    let mut words = Vec::new();
    let mut i = 0;
    while let Some(c) = code[i..].chars().next() {
        if c == '#' {
            i = code[i..].find('\n').map_or(code.len(), |o| i + o);
        } else if c == '\'' || c == '"' {
            i = guard::python_string_end(code, i, "")?;
        } else if c == '_' || c.is_alphanumeric() {
            let end = guard::python_word_end(code, i);
            let word = &code[i..end];
            if code[end..].starts_with(['\'', '"']) && guard::is_python_string_prefix(word) {
                i = guard::python_string_end(code, end, word)?;
            } else {
                words.push(i..end);
                i = end;
            }
        } else {
            i += c.len_utf8();
        }
    }
    Some(words)
}

fn plain_python_string(code: &str, at: usize) -> Option<(&str, usize)> {
    python_literal_text(code, at, false)
}

fn python_literal_text(code: &str, at: usize, allow_triple: bool) -> Option<(&str, usize)> {
    python_literal_raw(code, at, allow_triple).filter(|(text, _)| !text.contains('\\'))
}

fn decode_simple_escapes(raw: &str) -> (String, bool) {
    let mut out = String::with_capacity(raw.len());
    let mut complete = true;
    let mut chars = raw.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some(q @ ('\\' | '\'' | '"')) => out.push(q),
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('r') => out.push('\r'),
            other => {
                complete = false;
                out.push('\\');
                out.extend(other);
            }
        }
    }
    (out, complete)
}

fn python_literal_raw(code: &str, at: usize, allow_triple: bool) -> Option<(&str, usize)> {
    let quote = at + usize::from(code[at..].starts_with(['r', 'R', 'u', 'U']));
    let q = code[quote..]
        .chars()
        .next()
        .filter(|c| matches!(c, '\'' | '"'))?;
    let delimiter = if code[quote..].starts_with(&q.to_string().repeat(3)) {
        if !allow_triple {
            return None;
        }
        3
    } else {
        1
    };
    let end = guard::python_string_end(code, quote, &code[at..quote])?;
    Some((&code[quote + delimiter..end - delimiter], end))
}

fn os_shell_literals(code: &str, words: &[std::ops::Range<usize>]) -> Vec<String> {
    static CALL: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r"^\s*\.\s*(?:system|popen)\s*\(\s*").unwrap()
    });
    words
        .iter()
        .filter(|w| &code[(*w).clone()] == "os" && !code[..w.start].trim_end().ends_with('.'))
        .filter_map(|w| {
            let call = CALL.find(&code[w.end..])?;
            let (text, end) = plain_python_string(code, w.end + call.end())?;
            code[end..]
                .trim_start()
                .starts_with([',', ')'])
                .then(|| text.to_string())
        })
        .collect()
}

fn python_with_bindings(
    code: &str,
    words: &[std::ops::Range<usize>],
    allow_triple: bool,
) -> Option<String> {
    static ASSIGN: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r"^[ \t]*(//|\*\*|<<|>>|[-+*/%&|^@])?=[ \t]*").unwrap()
    });
    let statement_start = |at: usize| {
        code[..at]
            .trim_end_matches([' ', '\t'])
            .ends_with(['\n', ';'])
            || code[..at].trim().is_empty()
    };
    let mut bindings = std::collections::HashMap::new();
    let mut out = String::new();
    let mut copied = 0;
    for word in words {
        let name = &code[word.clone()];
        if let Some(assign) = ASSIGN
            .captures(&code[word.end..])
            .filter(|c| !code[word.end + c.get(0).unwrap().end()..].starts_with('='))
        {
            if statement_start(word.start) {
                let value = word.end + assign.get(0).unwrap().end();
                let literal = python_literal_text(code, value, allow_triple)
                    .filter(|(_, end)| {
                        assign.get(1).is_none()
                            && code[*end..]
                                .trim_start_matches([' ', '\t'])
                                .chars()
                                .next()
                                .is_none_or(|c| matches!(c, '\n' | ';' | '#'))
                    })
                    .map(|(_, end)| &code[value..end]);
                match literal {
                    Some(literal) => bindings.insert(name, literal),
                    None => bindings.remove(name),
                };
            }
            continue;
        }
        if code[..word.start].trim_end().ends_with('.') {
            continue;
        }
        if let Some(literal) = bindings.get(name) {
            out.push_str(&code[copied..word.start]);
            out.push_str(literal);
            copied = word.end;
        }
    }
    (copied > 0).then(|| out + &code[copied..])
}

fn python_data_calls(code: &str) -> bool {
    static CALL: std::sync::LazyLock<regex::Regex> =
        std::sync::LazyLock::new(|| regex::Regex::new(r"([\w.]+)\s*\(").unwrap());
    static IMPORT: std::sync::LazyLock<regex::Regex> =
        std::sync::LazyLock::new(|| regex::Regex::new(r"\b(from|import)\s+([^\n;]+)").unwrap());
    let imports_known = IMPORT.captures_iter(code).all(|c| {
        if &c[1] == "from" {
            return c[2].trim() == "pathlib import Path";
        }
        c[2].split(',').all(|item| {
            let module = item.split_whitespace().next().unwrap_or("");
            matches!(
                module,
                "os" | "shutil" | "pathlib" | "sys" | "json" | "math" | "re"
            )
        })
    });
    imports_known
        && CALL.captures_iter(code).all(|c| {
            matches!(
                &c[1],
                "print"
                    | "str"
                    | "repr"
                    | "len"
                    | "range"
                    | "Path"
                    | "pathlib.Path"
                    | "open"
                    | "shutil.which"
                    | "os.environ.get"
                    | "os.environ.getitem"
            ) || c[1].ends_with(".write_text")
                || c[1].ends_with(".write_bytes")
                || c[1].rsplit_once('.').is_some_and(|(receiver, method)| {
                    !matches!(receiver, "os" | "shutil") && PURE_METHODS.contains(&method)
                })
        })
        && ![
            ".system", ".popen", "__", "eval", "exec", "getattr", "setattr", "lambda", "def ",
        ]
        .iter()
        .any(|s| code.contains(s))
}

const PURE_METHODS: [&str; 12] = [
    "read_text",
    "read_bytes",
    "replace",
    "index",
    "find",
    "count",
    "split",
    "splitlines",
    "join",
    "strip",
    "startswith",
    "endswith",
];

fn python_without_literals(code: &str) -> Option<String> {
    let mut out = String::new();
    let mut i = 0;
    while i < code.len() {
        let c = code[i..].chars().next()?;
        if c == '#' {
            i = code[i..].find('\n').map_or(code.len(), |n| i + n);
        } else if matches!(c, '\'' | '"') {
            i = guard::python_string_end(code, i, "")?;
            out.push(' ');
        } else if c == '_' || c.is_alphabetic() {
            let end = guard::python_word_end(code, i);
            let word = &code[i..end];
            if code[end..].starts_with(['\'', '"']) && guard::is_python_string_prefix(word) {
                // INVARIANT: Formatted strings can run arbitrary expressions.
                if word.to_ascii_lowercase().contains(['f', 't']) {
                    return None;
                }
                i = guard::python_string_end(code, end, word)?;
                out.push(' ');
            } else {
                out.push_str(word);
                i = end;
            }
        } else {
            out.push(c);
            i += c.len_utf8();
        }
    }
    Some(out)
}

fn sql_drop(sql: &str, operation: Operation) -> bool {
    if sql.contains('\\') && operation.conservative(sql) {
        return true;
    }
    let mut tokens = Vec::new();
    let mut i = 0;
    while i < sql.len() {
        let rest = &sql[i..];
        if rest.starts_with("--") {
            i += rest.find('\n').unwrap_or(rest.len());
        } else if rest.starts_with("/*") {
            let mut nesting = 1usize;
            i += 2;
            while nesting > 0 {
                let rest = &sql[i..];
                if rest.is_empty() {
                    return operation.conservative(sql);
                }
                if rest.starts_with("/*") {
                    nesting += 1;
                    i += 2;
                } else if rest.starts_with("*/") {
                    nesting -= 1;
                    i += 2;
                } else {
                    i += rest.chars().next().unwrap().len_utf8();
                }
            }
        } else if rest.starts_with("U&\"") || rest.starts_with("u&\"") {
            // INVARIANT: Unicode identifiers can encode the protected database prefix.
            return operation.conservative(sql);
        } else if rest.starts_with('\'') {
            // INVARIANT: Retain a separator token so identifiers cannot join across data.
            tokens.push(String::new());
            i += 1;
            loop {
                let Some(end) = sql[i..].find('\'') else {
                    return operation.conservative(sql);
                };
                i += end + 1;
                if sql[i..].starts_with('\'') {
                    i += 1;
                } else {
                    break;
                }
            }
        } else if let Some(quoted) = rest.strip_prefix('$') {
            let Some(end) = quoted.find('$').map(|n| n + 1) else {
                return operation.conservative(sql);
            };
            let tag = &rest[..=end];
            let Some(close) = rest[tag.len()..].find(tag) else {
                return operation.conservative(sql);
            };
            i += tag.len() * 2 + close;
            tokens.push(String::new());
        } else if let Some(quoted) = rest.strip_prefix('"') {
            let Some(end) = quoted.find('"') else {
                return operation.conservative(sql);
            };
            tokens.push(quoted[..end].to_ascii_lowercase());
            i += end + 2;
        } else {
            let c = rest.chars().next().unwrap();
            if c.is_alphanumeric() || c == '_' {
                let end = rest
                    .find(|c: char| !c.is_alphanumeric() && c != '_' && c != '$')
                    .unwrap_or(rest.len());
                tokens.push(rest[..end].to_ascii_lowercase());
                i += end;
            } else {
                if c == ';' {
                    tokens.push(String::new());
                }
                i += c.len_utf8();
            }
        }
    }
    if tokens.iter().any(|t| {
        matches!(
            t.as_str(),
            "do" | "execute" | "dblink_exec" | "gexec" | "program"
        )
    }) {
        return operation.conservative(sql);
    }
    tokens
        .windows(3)
        .any(|w| w[0] == "drop" && w[1] == "database" && w[2].starts_with("lucidos_"))
        || tokens
            .windows(5)
            .any(|w| w[..4] == ["drop", "database", "if", "exists"] && w[4].starts_with("lucidos_"))
}
