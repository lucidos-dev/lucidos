//! Destructive operations, inspected in their execution context.
use crate::engine::command_guard as guard;
use std::cell::LazyCell;

const MAX_DEPTH: usize = 8;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Operation<'a> {
    Workspace,
    Disk,
    NamedCommand(&'a str),
}

impl Operation<'_> {
    fn conservative(self, text: &str) -> bool {
        match self {
            Self::Workspace => conservative(text),
            Self::Disk => guard::disk_operation_text(text),
            Self::NamedCommand(_) => false,
        }
    }
}

pub(in crate::engine::command_guard) fn names_command(command: &str, name: &str) -> bool {
    shell(command, 0, command, Operation::NamedCommand(name))
}

/// Ambiguous executable syntax keeps the previous conservative refusal.
fn conservative(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    let url = lower.replace("%2f", "/");
    (lower.contains("lucidos_")
        && (lower.contains("dropdb")
            || lower.contains("pg_ctl")
            || super::sql_drop_database(&lower)))
        || (url.contains("/~/api/v1/control/workspaces/") && lower.contains("delete"))
        || url.contains("/~/api/v1/control/cleanup")
        || url.contains("/~/api/v1/control/trash")
}

pub(super) fn destructive(command: &str) -> bool {
    inspect(command, 0, command, Operation::Workspace)
}

pub(super) fn destructive_python(code: &str) -> bool {
    python(code, 0, code, Operation::Workspace)
}

pub(super) fn disk(command: &str, is_python: bool) -> bool {
    if is_python {
        python(command, 0, command, Operation::Disk)
    } else {
        // Leading shell comments cannot supply operands or executable input.
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

/// A heredoc's consumer can feed a later pipe only until its chain ends.
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

/// Split only live shell control operators. Heredoc bodies belong to their
/// consumer, rather than becoming new shell commands at each newline.
fn shell(command: &str, depth: usize, context: &str, operation: Operation) -> bool {
    let mut quotes = guard::QuoteState::default();
    let mut pending: Vec<PendingHeredoc> = Vec::new();
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
        // Redirect modifiers are plumbing, not control operators. Keep the
        // consumer intact across `2>&1`, `&>>file`, `>|file` and `>>!file`.
        // The quote-aware traversal places the operator; the ordinary argv
        // reader places its target, including a quoted or concatenated path.
        if command[i..].starts_with("&>") {
            // Bash/zsh attach this redirect to the preceding command. A
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
            if segment_readings(
                &command[start..i],
                posix_start.map(|at| &command[at..i]),
                depth,
                context,
                operation,
            ) {
                return true;
            }
            i = command[i..].find('\n').map_or(command.len(), |n| i + n);
            start = i;
            posix_start = None;
            continue;
        }
        // A compound can pipe a heredoc after its terminator or after a
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
            if matches!(operation, Operation::NamedCommand(_)) {
                i += 3;
                word_start = true;
                continue;
            }
            // A here-string may feed a program. Keep unknown input conservative.
            return operation.conservative(context);
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
            // Each pending input is closed or piped once, so later sibling
            // separators do not repeatedly walk already resolved inputs.
            for input in &mut pending[open_inputs..] {
                input.pipeline = if byte == b'|' && !conditional {
                    InputPipeline::Piped
                } else {
                    InputPipeline::Closed
                };
            }
            open_inputs = pending.len();
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
    segment_readings(
        &command[start..],
        posix_start.map(|at| &command[at..]),
        depth,
        context,
        operation,
    )
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

/// Read just one redirect target, so sibling redirects never parse the whole
/// remaining suffix. Quote placement stays with the shared shell quote state.
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
    // A pipeline may send the input into an interpreter further downstream.
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
            "psql" if operation == Operation::Workspace => sql_drop(body),
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
        // These control heads have the same unproven compound stdin flow as
        // the live brace/parenthesis cases in the shared traversal.
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
            // These leaf parsers own workspace destruction, not disk writes.
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
            "psql" => psql(args, command, context),
            "curl" | "wget" => {
                (guard::http_args_hide_options(head, args) && operation.conservative(context))
                    || control_request(args)
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
            // The invoked command's operands may come from stdin, so its
            // argv alone cannot prove that it avoids a protected target.
            "xargs" | "gxargs" | "parallel" => operation.conservative(context),
            h if guard::EXEC_WRAPPER_HEADS.contains(&h)
                || guard::EXEC_WRAPPER_HEADS_WITH_OPERAND.contains(&h) =>
            {
                false
            }
            h if INERT_READERS.contains(&h) && *proven_inert => false,
            // An unknown program can run a file written by an earlier sink,
            // or change how a later command resolves (for example, `hash`).
            _ => operation.conservative(context),
        })
}

/// Heads the floor exempts in a segment proven to only read. None evaluates
/// an operand as code, as `test -v` evaluates an array subscript. None runs a
/// command that a config file names, as git runs hooks and `ack` a pager.
const INERT_READERS: [&str; 16] = [
    "sed", "sort", "find", "nl", "cut", "tr", "diff", "cmp", "stat", "file", "du", "jq", "column",
    "basename", "dirname", "realpath",
];

/// True when `command` only reads ([`guard::script_proven_read_only`]) and
/// expands nothing that can run code. zsh's `${(e)x}` and an arithmetic
/// `$((x))` both run a command substitution that the value of `x` holds.
fn segment_proven_inert(command: &str) -> bool {
    !command.contains('`')
        && !command.contains("$(")
        && !command.contains("${")
        && guard::script_proven_read_only(command)
}

fn bare_head(words: &[&str], args: &[&str], head: &str) -> bool {
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

fn psql(args: &[&str], command: &str, context: &str) -> bool {
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
    (unknown_input && conservative(context)) || sql.iter().any(|s| sql_drop(s))
}

fn control_request(args: &[&str]) -> bool {
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
            return conservative(&url);
        };
        let path = parsed.path();
        (delete && path.starts_with("/~/api/v1/control/workspaces/"))
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

/// The existing Python lexer identifies literal subprocess calls. Only code
/// whose remaining executable text has no execution signal loses its literals.
fn python(code: &str, depth: usize, context: &str, operation: Operation) -> bool {
    if depth >= MAX_DEPTH {
        return operation.conservative(context);
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
    // The existing side-effect proof is deliberately broad. Requiring known
    // data calls as well prevents an unknown callable from receiving an inert
    // string exemption, including an alias of an execution primitive.
    let unknown_execution =
        guard::python_side_effect_signal(&compact) || !python_data_calls(&executable);
    unknown_execution && operation.conservative(context)
}

fn python_data_calls(code: &str) -> bool {
    static CALL: std::sync::LazyLock<regex::Regex> =
        std::sync::LazyLock::new(|| regex::Regex::new(r"([\w.]+)\s*\(").unwrap());
    // Calls that only examine values or write ordinary file contents. Unknown
    // names keep the fallback, even when a human could prove them harmless.
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
        })
        && ![
            ".system", ".popen", "__", "eval", "exec", "getattr", "setattr", "lambda", "def ",
        ]
        .iter()
        .any(|s| code.contains(s))
}

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
                // Formatted strings can run arbitrary expressions.
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

/// SQL literals and comments cannot introduce a DROP statement. Double quotes
/// delimit identifiers; dollar quotes delimit data, including function bodies.
fn sql_drop(sql: &str) -> bool {
    if sql.contains('\\') && conservative(sql) {
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
                    return conservative(sql);
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
            // Unicode identifiers can encode the protected database prefix.
            return conservative(sql);
        } else if rest.starts_with('\'') {
            // Retain a separator token so identifiers cannot join across data.
            tokens.push(String::new());
            i += 1;
            loop {
                let Some(end) = sql[i..].find('\'') else {
                    return conservative(sql);
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
                return conservative(sql);
            };
            let tag = &rest[..=end];
            let Some(close) = rest[tag.len()..].find(tag) else {
                return conservative(sql);
            };
            i += tag.len() * 2 + close;
            tokens.push(String::new());
        } else if let Some(quoted) = rest.strip_prefix('"') {
            let Some(end) = quoted.find('"') else {
                return conservative(sql);
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
        return conservative(sql);
    }
    tokens
        .windows(3)
        .any(|w| w[0] == "drop" && w[1] == "database" && w[2].starts_with("lucidos_"))
        || tokens
            .windows(5)
            .any(|w| w[..4] == ["drop", "database", "if", "exists"] && w[4].starts_with("lucidos_"))
}
