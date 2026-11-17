//! Database and control operations, inspected in their execution context.
use crate::engine::command_guard as guard;

const MAX_DEPTH: usize = 8;

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
    inspect(command, 0, command)
}

pub(super) fn destructive_python(code: &str) -> bool {
    python(code, 0, code)
}

fn inspect(command: &str, depth: usize, context: &str) -> bool {
    if depth >= MAX_DEPTH {
        return conservative(context);
    }
    let first = command
        .lines()
        .map(str::trim)
        .find(|s| !s.is_empty() && !s.starts_with('#'))
        .unwrap_or("");
    if first.starts_with("import ") || first.starts_with("from ") || first.starts_with("print(") {
        return python(command, depth + 1, context);
    }
    shell(command, depth + 1, context)
}

/// Split only live shell control operators. Heredoc bodies belong to their
/// consumer, rather than becoming new shell commands at each newline.
fn shell(command: &str, depth: usize, context: &str) -> bool {
    let mut quotes = guard::QuoteState::default();
    let mut pending = Vec::new();
    let mut start = 0;
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
                if inspect(&command[i..][span.body.clone()], depth + 1, context) {
                    return true;
                }
                i += span.whole.end;
                word_start = false;
                continue;
            }
        }
        let byte = command.as_bytes()[i];
        if byte == b'#' && word_start {
            if segment(&command[start..i], depth, context) {
                return true;
            }
            i = command[i..].find('\n').map_or(command.len(), |n| i + n);
            start = i;
            continue;
        }
        if byte == b'(' && command[i + 1..].trim_start().starts_with(')') {
            return conservative(context);
        }
        if command[i..].starts_with("<<<") {
            // A here-string may feed a program. Keep unknown input conservative.
            return conservative(context);
        }
        if command[i..].starts_with("<<") {
            let Some((doc, end)) = guard::heredoc_opener(command, i + 2) else {
                return conservative(context);
            };
            pending.push((doc, start));
            i = end;
            word_start = false;
            continue;
        }
        if b";|&\n".contains(&byte) {
            if segment(&command[start..i], depth, context) {
                return true;
            }
            if byte == b'\n' && !pending.is_empty() {
                let mut at = i + 1;
                for (doc, consumer) in pending.drain(..) {
                    let Some((body_end, end)) = guard::heredoc_body_end(command, at, &doc) else {
                        return conservative(context);
                    };
                    let body = &command[at..body_end];
                    if (!doc.literal
                        && guard::substitution_bodies(body)
                            .iter()
                            .any(|s| inspect(s, depth + 1, context)))
                        || stdin(&command[consumer..i], body, depth, context, doc.literal)
                    {
                        return true;
                    }
                    at = end;
                }
                i = at;
                start = at;
                word_start = true;
                continue;
            }
            start = i + 1;
        }
        word_start = b" \t\r\n;|&()".contains(&byte);
        i += guard::char_len_at(command, i);
    }
    segment(&command[start..], depth, context)
}

fn stdin(consumer: &str, body: &str, depth: usize, context: &str, literal: bool) -> bool {
    let words = guard::shell_argv(consumer);
    let refs: Vec<_> = words.iter().map(|w| w.text.as_str()).collect();
    // A pipeline may send the input into an interpreter further downstream.
    if consumer.contains('|') {
        return conservative(context) || inspect(body, depth + 1, context);
    }
    guard::danger_head_candidates(&refs)
        .iter()
        .any(|(head, args)| match head.as_str() {
            h if guard::DATA_SINK_HEADS.contains(&h) => {
                !bare_head(&refs, args, h) && conservative(context)
            }
            h if guard::EXEC_WRAPPER_HEADS.contains(&h)
                || guard::EXEC_WRAPPER_HEADS_WITH_OPERAND.contains(&h) =>
            {
                false
            }
            _ if !literal && body.contains('$') => conservative(context),
            "psql" => sql_drop(body),
            h if guard::GUARD_SHELLS.contains(&h) => inspect(body, depth + 1, context),
            h if h.starts_with("python") => python(body, depth + 1, context),
            _ => conservative(context),
        })
}

fn segment(command: &str, depth: usize, context: &str) -> bool {
    if depth >= MAX_DEPTH {
        return conservative(context);
    }
    if guard::substitution_bodies(command)
        .iter()
        .any(|s| inspect(s, depth + 1, context))
    {
        return true;
    }
    let inner = guard::unwrap_shell_command(command);
    if inner != command {
        return inspect(&inner, depth + 1, context);
    }
    let words = guard::shell_argv(command);
    let refs: Vec<_> = words.iter().map(|w| w.text.as_str()).collect();
    if refs.iter().any(|w| guard::is_code_injecting_assignment(w)) && conservative(context) {
        return true;
    }
    guard::danger_head_candidates(&refs)
        .iter()
        .any(|(head, args)| match head.as_str() {
            "dropdb" if words.iter().any(|w| w.expands) => conservative(context),
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
                            || conservative(context),
                            |p| p.to_ascii_lowercase().contains("lucidos_"),
                        )
            }
            "psql" | "curl" | "wget" if words.iter().any(|w| w.expands) => conservative(context),
            "psql" => psql(args, command, context),
            "curl" | "wget" => {
                (guard::http_args_hide_options(head, args) && conservative(context))
                    || control_request(args)
            }
            h if h.starts_with("python") => {
                if let Some(code) = args.windows(2).find_map(|w| (w[0] == "-c").then_some(w[1])) {
                    python(code, depth + 1, context)
                } else {
                    !heredoc_is_code_input(command, args) && conservative(context)
                }
            }
            h if guard::GUARD_SHELLS.contains(&h) && heredoc_is_code_input(command, args) => false,
            "cd" => false,
            h if guard::DATA_SINK_HEADS.contains(&h) => {
                !bare_head(&refs, args, h) && conservative(context)
            }
            "rg" | "grep" | "which" => {
                guard::args_have_exec_capable_flag(args) && conservative(context)
            }
            h if guard::EXEC_WRAPPER_HEADS.contains(&h)
                || guard::EXEC_WRAPPER_HEADS_WITH_OPERAND.contains(&h) =>
            {
                false
            }
            // An unknown program can run a file written by an earlier sink,
            // or change how a later command resolves (for example, `hash`).
            _ => conservative(context),
        })
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
fn python(code: &str, depth: usize, context: &str) -> bool {
    if depth >= MAX_DEPTH {
        return conservative(context);
    }
    let Some(calls) = guard::python_shell_outs(code) else {
        return conservative(context);
    };
    if calls
        .commands
        .iter()
        .any(|cmd| inspect(cmd, depth + 1, context))
    {
        return true;
    }
    let Some(executable) = python_without_literals(&calls.rest) else {
        return conservative(context);
    };
    let compact: String = executable.chars().filter(|c| !c.is_whitespace()).collect();
    // The existing side-effect proof is deliberately broad. Requiring known
    // data calls as well prevents an unknown callable from receiving an inert
    // string exemption, including an alias of an execution primitive.
    let unknown_execution =
        !guard::python_is_statically_safe(&compact) || !python_data_calls(&executable);
    unknown_execution && conservative(context)
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
