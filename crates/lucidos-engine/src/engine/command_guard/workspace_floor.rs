//! Read-only discovery for the unconditional workspace destruction floor.
use super::Catastrophe;
use std::path::{Component, Path, PathBuf};
use std::sync::LazyLock;

mod live_segments;
mod operations;

use live_segments::{Input, LiveSegment};
pub(super) use operations::names_command;

pub(super) fn formats_or_overwrites_disk(command: &str, is_python: bool) -> bool {
    operations::disk(command, is_python)
}

const REASON: &str = "a command that deletes a Lucidos workspace (only the gateway may do that, \
                      after the user confirms it on a paired device)";

pub(super) fn reason(command: &str, cwd: Option<&Path>) -> Option<Catastrophe> {
    proven_first(operations::destructive(command), || {
        filesystem_reason(command, cwd)
    })
}

pub(super) fn python_reason(code: &str, cwd: Option<&Path>) -> Option<Catastrophe> {
    proven_first(operations::destructive_python(code), || {
        verdict(cwd, |scan, scope| scan.python(code, scope, true, 0))
    })
}

/// A fallback match never hides a proven one that a later check finds.
fn proven_first(
    found: Option<Catastrophe>,
    next: impl FnOnce() -> Option<Catastrophe>,
) -> Option<Catastrophe> {
    match found {
        Some(Catastrophe::Proven(_)) => found,
        _ => next().or(found),
    }
}

fn filesystem_reason(command: &str, cwd: Option<&Path>) -> Option<Catastrophe> {
    verdict(cwd, |scan, scope| scan.shell(command, scope, true, 0))
}

/// Reads every target from the text before probing the filesystem, so a
/// command that names none costs no disk read.
fn verdict(
    cwd: Option<&Path>,
    read: impl FnOnce(&mut TargetScan<'_>, &mut Scope),
) -> Option<Catastrophe> {
    let default_cwd = std::env::current_dir().ok();
    let cwd = cwd.or(default_cwd.as_deref());
    let home = super::home_dir();
    let scan = TargetScan::read(cwd, home.as_deref(), read);
    judge(&scan, cwd, registered_roots)
}

/// A live target proves the destruction. A target in output that may run
/// later, or a scan that reaches its limit, needs consent.
fn judge(
    scan: &TargetScan<'_>,
    cwd: Option<&Path>,
    roots: impl FnOnce() -> Vec<PathBuf>,
) -> Option<Catastrophe> {
    if scan.targets.is_empty() {
        return scan.limit_reached.then_some(Catastrophe::Unproven(REASON));
    }
    let roots = roots();
    let hit = |live: bool| {
        scan.targets
            .iter()
            .filter(|target| target.live == live)
            .any(|target| hits_workspace(&target.path, cwd, &roots))
    };
    if hit(true) {
        Some(Catastrophe::Proven(REASON))
    } else if scan.limit_reached || hit(false) {
        Some(Catastrophe::Unproven(REASON))
    } else {
        None
    }
}

fn sql_drop_database(command: &str) -> bool {
    static DROP: LazyLock<regex::Regex> =
        LazyLock::new(|| regex::Regex::new(r"\bdrop\s+database\b").unwrap());
    DROP.is_match(command)
}

fn registered_roots() -> Vec<PathBuf> {
    crate::paths::registered_workspace_dirs(&crate::paths::gateway_data_dirs())
        .iter()
        .map(|dir| normalize(dir))
        .collect()
}

fn normalize(path: &Path) -> PathBuf {
    if let Ok(path) = std::fs::canonicalize(path) {
        return path;
    }
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            c => out.push(c.as_os_str()),
        }
    }
    out
}

fn own_worktree(path: &Path, cwd: Option<&Path>) -> bool {
    cwd.is_some_and(|cwd| {
        let cwd = normalize(cwd);
        if !path.starts_with(&cwd) {
            return false;
        }
        let Ok(git) = std::fs::read_to_string(cwd.join(".git")) else {
            return false;
        };
        let Some(gitdir) = git.trim().strip_prefix("gitdir: ") else {
            return false;
        };
        cwd.join(gitdir).join("commondir").is_file()
    })
}

fn expand_path(word: &str, cwd: Option<&Path>, home: Option<&Path>) -> Option<PathBuf> {
    let path = if matches!(word, "~" | "$HOME" | "${HOME}") {
        home?.to_path_buf()
    } else if let Some(tail) = word
        .strip_prefix("~/")
        .or_else(|| word.strip_prefix("$HOME/"))
        .or_else(|| word.strip_prefix("${HOME}/"))
    {
        home?.join(tail)
    } else {
        PathBuf::from(word)
    };
    Some(if path.is_absolute() {
        path
    } else {
        cwd?.join(path)
    })
}

/// Only operands that can lose data belong here; destinations of mv and
/// selectors of find are not deletion targets.
fn destructive_operands(command: &str) -> Vec<String> {
    let words: Vec<_> = super::shell_argv(command)
        .into_iter()
        .map(|w| w.text)
        .collect();
    let refs: Vec<_> = words.iter().map(String::as_str).collect();
    let candidates = super::danger_head_candidates(&refs);
    let Some((head, args)) = candidates
        .iter()
        .find(|(head, _)| matches!(head.as_str(), "rm" | "mv" | "trash" | "rsync" | "find"))
    else {
        return Vec::new();
    };
    let operands = || {
        args.iter()
            .filter(|s| !s.starts_with('-'))
            .map(|s| s.trim_end_matches([')', ';']).to_string())
            .collect::<Vec<_>>()
    };
    match head.as_str() {
        "rm" | "trash" => operands(),
        "mv" => {
            let mut paths = operands();
            // -t names a destination before the source operands.
            if let Some(i) = args.iter().position(|s| {
                *s == "--target-directory"
                    || (!s.starts_with("--") && s.starts_with('-') && s.ends_with('t'))
            }) {
                if let Some(dest) = args.get(i + 1) {
                    paths.retain(|p| p != *dest);
                }
            } else if !args.iter().any(|s| {
                s.starts_with("--target-directory=")
                    || (!s.starts_with("--") && s.starts_with('-') && s.contains('t'))
            }) {
                paths.pop();
            }
            paths
        }
        "rsync" if args.iter().any(|s| s.starts_with("--delete")) => {
            let mut paths = Vec::new();
            let mut skip = false;
            for arg in *args {
                if skip {
                    skip = false;
                    continue;
                }
                if matches!(
                    *arg,
                    "--exclude"
                        | "--include"
                        | "--filter"
                        | "-f"
                        | "-e"
                        | "--rsh"
                        | "--exclude-from"
                        | "--include-from"
                        | "--files-from"
                        | "--log-file"
                        | "--out-format"
                        | "--backup-dir"
                        | "--suffix"
                        | "--chmod"
                        | "--chown"
                        | "--compare-dest"
                        | "--copy-dest"
                        | "--link-dest"
                ) {
                    skip = true;
                } else if !arg.starts_with('-') {
                    paths.push(arg.trim_end_matches(')').to_string());
                }
            }
            paths.last().cloned().into_iter().collect()
        }
        "find" => {
            let deletes = args.contains(&"-delete")
                || args.windows(2).enumerate().any(|(i, w)| {
                    matches!(w[0], "-exec" | "-execdir")
                        && (matches!(
                            Path::new(w[1]).file_name().and_then(|s| s.to_str()),
                            Some("rm" | "trash" | "mv")
                        ) || destructive_text(&args[i + 1..].join(" ")))
                });
            if !deletes {
                return Vec::new();
            }
            let roots: Vec<_> = args
                .iter()
                .skip_while(|s| matches!(**s, "-L" | "-H" | "-P"))
                .take_while(|s| !s.starts_with('-') && !matches!(**s, "!" | "(" | ")"))
                .map(|s| s.to_string())
                .collect();
            let roots = if roots.is_empty() {
                vec![".".into()]
            } else {
                roots
            };
            let action = args
                .iter()
                .position(|s| matches!(*s, "-delete" | "-exec" | "-execdir"))
                .unwrap_or(usize::MAX);
            let selector = args
                .iter()
                .position(|s| matches!(*s, "-name" | "-iname"))
                .unwrap_or(usize::MAX);
            let simple_selector = selector < action
                && !args
                    .iter()
                    .any(|s| matches!(*s, "!" | "-not" | "-o" | "-or"))
                && args
                    .iter()
                    .filter(|s| matches!(**s, "-name" | "-iname"))
                    .count()
                    == 1;
            if let Some(i) = args
                .iter()
                .position(|s| simple_selector && (*s == "-name" || *s == "-iname"))
            {
                if let Some(name) = args.get(i + 1) {
                    let mut paths: Vec<_> = roots
                        .iter()
                        .map(|root| format!("{root}/**/{name}"))
                        .collect();
                    if let Ok(pattern) = glob::Pattern::new(name) {
                        paths.extend(
                            roots
                                .iter()
                                .filter(|root| {
                                    Path::new(root).file_name().is_some_and(|base| {
                                        pattern.matches_with(
                                            &base.to_string_lossy(),
                                            glob::MatchOptions {
                                                case_sensitive: false,
                                                ..Default::default()
                                            },
                                        )
                                    })
                                })
                                .cloned(),
                        );
                    }
                    return paths;
                }
            }
            roots
        }
        _ => Vec::new(),
    }
}

/// Locate xargs' consumer without treating option values as commands.
fn xargs_deletes(args: &[&str]) -> bool {
    let mut i = 0;
    while let Some(arg) = args.get(i) {
        if *arg == "--" {
            i += 1;
            break;
        }
        if !arg.starts_with('-') {
            break;
        }
        let takes_value = matches!(
            *arg,
            "-I" | "-n"
                | "-P"
                | "-L"
                | "-s"
                | "-E"
                | "-d"
                | "-J"
                | "-R"
                | "-S"
                | "-a"
                | "--arg-file"
                | "--max-args"
                | "--max-procs"
                | "--max-chars"
                | "--delimiter"
        );
        i += if takes_value { 2 } else { 1 };
    }
    args.get(i..)
        .is_some_and(|consumer| !consumer.is_empty() && destructive_text(&consumer.join(" ")))
}

fn destructive_text(command: &str) -> bool {
    super::command_segments(command).any(|segment| {
        let inner = super::unwrap_shell_command(&segment);
        let words = super::shell_argv(&inner);
        let refs: Vec<_> = words.iter().map(|w| w.text.as_str()).collect();
        super::danger_head_candidates(&refs)
            .iter()
            .any(|(head, args)| {
                matches!(head.as_str(), "rm" | "mv" | "trash")
                    || (matches!(head.as_str(), "xargs" | "gxargs") && xargs_deletes(args))
                    || ((head.starts_with("python") || head == "perl")
                        && args.iter().any(|s| s.contains("rmtree(")))
                    || (matches!(head.as_str(), "sh" | "bash" | "zsh")
                        && args
                            .iter()
                            .any(|s| s.contains("rm ") || s.contains("rmtree(")))
            })
    })
}

/// Only rmtree arguments and literal bindings feeding them are targets.
/// When the lexer cannot place every string, any match may be data, so each
/// one is a target that needs consent rather than proof.
fn python_call_targets(code: &str) -> Vec<(String, bool)> {
    let identifiers = operations::python_identifiers(code);
    let placed = identifiers.is_some();
    let targets = python_call_targets_where(code, |at| {
        identifiers
            .as_ref()
            .is_none_or(|words| words.binary_search_by_key(&at, |word| word.start).is_ok())
    });
    targets
        .into_iter()
        .map(|(target, direct)| (target, direct && placed))
        .collect()
}

fn python_call_targets_where(code: &str, is_code: impl Fn(usize) -> bool) -> Vec<(String, bool)> {
    static CALL: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::Regex::new(r#"\brmtree\s*\(\s*(?:(?:os\.path\.expanduser|Path|pathlib\.Path)\s*\(\s*)?(?:[rRuU]?'([^']*)'|[rRuU]?"([^"]*)"|([A-Za-z_]\w*))"#).unwrap()
    });
    static BIND: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::Regex::new(r#"(?:^|[;\n])\s*([A-Za-z_]\w*)\s*=\s*(?:(?:os\.path\.expanduser|Path|pathlib\.Path)\s*\(\s*)?(?:[rRuU]?'([^']*)'|[rRuU]?"([^"]*)")"#).unwrap()
    });
    let mut bindings = std::collections::HashMap::<_, Vec<_>>::new();
    for c in BIND.captures_iter(code) {
        let name = c.get(1).unwrap();
        if is_code(name.start()) {
            let value = c.get(2).or_else(|| c.get(3)).unwrap().as_str();
            bindings
                .entry(name.as_str())
                .or_default()
                .push((name.start(), value));
        }
    }
    static JOIN: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::Regex::new(r"\brmtree\s*\(\s*os\.path\.join\s*\(([^()]*)\)").unwrap()
    });
    let joined = JOIN.captures_iter(code).filter_map(|c| {
        if !is_code(c.get(0)?.start()) {
            return None;
        }
        let mut path = PathBuf::new();
        for part in c[1].split(',') {
            let part = part.trim();
            let literal = part.strip_prefix('r').unwrap_or(part);
            let value = literal
                .strip_prefix(char::from(39))
                .and_then(|s| s.strip_suffix(char::from(39)))
                .or_else(|| literal.strip_prefix('"').and_then(|s| s.strip_suffix('"')))?;
            path.push(value);
        }
        Some((path.to_string_lossy().into_owned(), true))
    });
    let mut targets: Vec<_> = joined.collect();
    let mut first_calls = std::collections::HashMap::new();
    for c in CALL.captures_iter(code) {
        let call = c.get(0).unwrap().start();
        if !is_code(call) {
            continue;
        }
        if let Some(literal) = c.get(1).or_else(|| c.get(2)) {
            targets.push((literal.as_str().to_string(), true));
        } else if let Some((&name, values)) = c
            .get(3)
            .and_then(|name| bindings.get_key_value(name.as_str()))
        {
            let earlier = values.partition_point(|(at, _)| *at < call);
            if let Some((_, value)) = values[..earlier].last() {
                targets.push((value.to_string(), true));
            }
            first_calls.entry(name).or_insert(call);
        }
    }
    // A function can read any binding made after its definition. The
    // scanner cannot prove which of those values the function reads.
    for (name, call) in first_calls {
        let values = &bindings[name];
        let earlier = values.partition_point(|(at, _)| *at < call);
        targets.extend(
            values[earlier..]
                .iter()
                .map(|(_, value)| (value.to_string(), false)),
        );
    }
    targets
}

/// The programs a `python -c` or `uv run python -c` segment runs.
fn python_command_code(words: &[super::ShellWord]) -> Vec<String> {
    let refs: Vec<_> = words.iter().map(|w| w.text.as_str()).collect();
    let runs_python = super::danger_head_candidates(&refs)
        .iter()
        .any(|(head, _)| head == "python" || head.starts_with("python3"))
        || (words.first().is_some_and(|w| w.text == "uv")
            && words.iter().any(|w| w.text.starts_with("python")));
    if !runs_python {
        return Vec::new();
    }
    words
        .windows(2)
        .filter(|w| w[0].text == "-c")
        .map(|w| w[1].text.clone())
        .collect()
}

/// Bare Python handed to a shell reader, judged by its first line.
fn looks_like_python(command: &str) -> bool {
    let first = command
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty() && !line.starts_with('#'))
        .unwrap_or("");
    first.starts_with("import ")
        || first.starts_with("from ")
        || first.starts_with("shutil.rmtree(")
        || first.contains("='")
        || first.contains("= '")
        || first.contains("= \"")
        || first.contains("=\"")
        || first.starts_with('"')
        || first.starts_with('\'')
}

/// Expand directory globs with the same fail-closed budget as discovery.
/// Descendant symlinks are never traversed.
fn glob_directories(
    base: &Path,
    parts: &[String],
    out: &mut Vec<PathBuf>,
    budget: &mut usize,
) -> Result<(), ()> {
    if parts.is_empty() {
        out.push(base.to_path_buf());
        return Ok(());
    }
    if *budget == 0 {
        return Err(());
    }
    *budget -= 1;
    let part = &parts[0];
    if !part.contains(['*', '?', '[']) {
        return glob_directories(&base.join(part), &parts[1..], out, budget);
    }
    if part == "**" {
        glob_directories(base, &parts[1..], out, budget)?;
    }
    if !base.is_dir() {
        return Ok(());
    }
    let pattern = glob::Pattern::new(part).map_err(|_| ())?;
    for entry in std::fs::read_dir(base).map_err(|_| ())? {
        let entry = entry.map_err(|_| ())?;
        if !entry.file_type().map_err(|_| ())?.is_dir() {
            continue;
        }
        if part == "**" {
            glob_directories(&entry.path(), parts, out, budget)?;
        } else if pattern.matches_with(
            &entry.file_name().to_string_lossy(),
            glob::MatchOptions {
                case_sensitive: false,
                ..Default::default()
            },
        ) {
            glob_directories(&entry.path(), &parts[1..], out, budget)?;
        }
    }
    Ok(())
}

/// A path a command may delete, resolved where the command runs.
struct Target {
    path: PathBuf,
    /// False when the scanner cannot establish that the operation runs.
    live: bool,
}

/// The shell state a later segment inherits.
#[derive(Clone, Default)]
struct Scope {
    cwd: Option<PathBuf>,
    variables: std::collections::HashMap<String, String>,
    // A substitution or persistent redirect can feed output into code.
    stdout_may_execute: bool,
}

/// Stop following nested inputs past this depth and keep the result uncertain.
const MAX_SCAN_DEPTH: usize = 8;

struct TargetScan<'a> {
    home: Option<&'a Path>,
    targets: Vec<Target>,
    limit_reached: bool,
}

impl<'a> TargetScan<'a> {
    fn read(
        cwd: Option<&Path>,
        home: Option<&'a Path>,
        read: impl FnOnce(&mut Self, &mut Scope),
    ) -> Self {
        let mut scan = Self {
            home,
            targets: Vec::new(),
            limit_reached: false,
        };
        let mut scope = Scope {
            cwd: cwd.map(Path::to_path_buf),
            ..Scope::default()
        };
        read(&mut scan, &mut scope);
        scan
    }

    fn push(&mut self, operand: &str, scope: &Scope, live: bool) {
        static VARIABLE: LazyLock<regex::Regex> =
            LazyLock::new(|| regex::Regex::new(r"\$\{([A-Za-z_]\w*)\}|\$([A-Za-z_]\w*)").unwrap());
        let operand = VARIABLE.replace_all(operand, |caps: &regex::Captures<'_>| {
            let name = caps.get(1).or_else(|| caps.get(2)).unwrap().as_str();
            scope
                .variables
                .get(name)
                .cloned()
                .unwrap_or_else(|| caps[0].to_string())
        });
        if let Some(path) = expand_path(&operand, scope.cwd.as_deref(), self.home) {
            self.targets.push(Target { path, live });
        }
    }

    fn shell(&mut self, command: &str, scope: &mut Scope, live: bool, depth: usize) {
        if depth > MAX_SCAN_DEPTH {
            self.limit_reached = true;
            return;
        }
        if looks_like_python(command) {
            for (target, direct) in python_call_targets(command) {
                self.push(&target, scope, live && direct);
            }
        }
        let segments =
            live_segments::live_segments(command).unwrap_or_else(|| plain_segments(command));
        for (index, segment) in segments.iter().enumerate() {
            // A substitution runs in a subshell, so its `cd` moves no later segment.
            for body in super::substitution_bodies(&segment.text) {
                let mut child_scope = scope.clone();
                child_scope.stdout_may_execute = true;
                self.shell(body, &mut child_scope, live, depth + 1);
            }
            let inner = super::unwrap_shell_command(&segment.text);
            if *inner != *segment.text {
                self.shell(&inner, &mut scope.clone(), live, depth + 1);
            } else {
                let feeds_delete = segments[index + 1..]
                    .iter()
                    .scan(segment.piped, |piped, next| {
                        std::mem::replace(piped, next.piped).then_some(next)
                    })
                    .any(|next| {
                        let words = super::shell_argv(&next.text);
                        let refs: Vec<_> = words.iter().map(|w| w.text.as_str()).collect();
                        super::danger_head_candidates(&refs)
                            .iter()
                            .any(|(head, args)| {
                                matches!(head.as_str(), "xargs" | "gxargs") && xargs_deletes(args)
                            })
                    });
                self.simple(&inner, feeds_delete, scope, live, depth);
            }
            for input in &segment.inputs {
                match input {
                    Input::Shell(body) => self.shell(body, &mut scope.clone(), live, depth + 1),
                    Input::Python(body) => self.python(body, scope, live, depth + 1),
                    Input::Uncertain(body) => {
                        self.shell(body, &mut scope.clone(), false, depth + 1)
                    }
                    Input::Data { body, written } if *written || scope.stdout_may_execute => {
                        self.shell(body, &mut scope.clone(), false, depth + 1);
                    }
                    Input::Data { .. } => {}
                }
            }
        }
    }

    /// One simple command. A `cd` moves the scope; every other command only
    /// adds the operands it can lose.
    fn simple(
        &mut self,
        command: &str,
        feeds_delete: bool,
        scope: &mut Scope,
        live: bool,
        depth: usize,
    ) {
        let argv = super::shell_argv(command);
        if argv.first().is_some_and(|w| w.text == "exec") && command.contains('>') {
            scope.stdout_may_execute = true;
        }
        for word in &argv {
            if let Some((name, value)) = word.text.split_once('=') {
                if !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                    scope.variables.insert(name.to_string(), value.to_string());
                }
            }
        }
        let destination = if argv
            .first()
            .is_some_and(|w| w.text.trim_start_matches('(') == "cd")
        {
            Some(
                argv.iter()
                    .skip(1)
                    .find(|w| !w.text.starts_with('-'))
                    .map_or("~".to_string(), |w| w.text.clone()),
            )
        } else {
            super::cd_destination(command)
        };
        if let Some(destination) = destination {
            scope.cwd = expand_path(&destination, scope.cwd.as_deref(), self.home);
            return;
        }
        for code in python_command_code(&argv) {
            self.python(&code, scope, live, depth + 1);
        }
        let mut operands = destructive_operands(command);
        if feeds_delete {
            let refs: Vec<_> = argv.iter().map(|w| w.text.as_str()).collect();
            let heads = super::danger_head_candidates(&refs);
            if heads.iter().any(|(head, _)| head == "find") {
                operands.extend(destructive_operands(&format!("{command} -delete")));
            } else {
                if heads.iter().any(|(head, _)| head == "ls")
                    && argv.iter().skip(1).all(|w| w.text.starts_with('-'))
                {
                    operands.push(".".into());
                }
                operands.extend(
                    argv.iter()
                        .skip(1)
                        .filter(|w| !w.text.starts_with('-'))
                        .map(|w| w.text.clone()),
                );
            }
        }
        for operand in operands {
            self.push(&operand, scope, live);
        }
    }

    /// Read `rmtree` arguments, code passed to Python evaluators and shell-outs.
    fn python(&mut self, code: &str, scope: &Scope, live: bool, depth: usize) {
        if depth > MAX_SCAN_DEPTH {
            self.limit_reached = true;
            return;
        }
        for (target, direct) in python_call_targets(code) {
            self.push(&target, scope, live && direct);
        }
        for (source, executed) in operations::python_source_literals(code) {
            self.python(&source, scope, live && executed, depth + 1);
        }
        let Some(outs) = super::python_shell_outs(code) else {
            // The lexer cannot place every string, so no literal is data.
            return self.shell(code, &mut scope.clone(), live, depth + 1);
        };
        for command in outs
            .commands
            .into_iter()
            .chain(operations::python_evident_shell_outs(code))
        {
            self.shell(&command, &mut scope.clone(), live, depth + 1);
        }
    }
}

/// The quote-blind split, for text whose quotes or heredocs cannot be placed.
fn plain_segments(command: &str) -> Vec<LiveSegment> {
    let separators = super::segment_separators(command);
    super::command_segments(command)
        .enumerate()
        .map(|(index, text)| LiveSegment {
            text,
            piped: separators.get(index).is_some_and(|s| s == "|"),
            backgrounded: separators.get(index).is_some_and(|s| s == "&"),
            conditional: separators
                .get(index)
                .is_some_and(|s| matches!(s.as_str(), "&&" | "||")),
            inputs: Vec::new(),
        })
        .collect()
}

/// Whether deleting `target` loses a workspace or a directory holding one.
fn hits_workspace(target: &Path, cwd: Option<&Path>, roots: &[PathBuf]) -> bool {
    let text = target.to_string_lossy();
    let mut candidates = Vec::new();
    if text.contains(['*', '?', '[']) {
        // A whole-component star at the root deletes its contents.
        if target.file_name().is_some_and(|s| s == "*") {
            if let Some(parent) = target.parent() {
                candidates.push(parent.to_path_buf());
            }
        }
        let mut base = PathBuf::new();
        let mut parts = Vec::new();
        for component in target.components() {
            let text = component.as_os_str().to_string_lossy().to_string();
            if parts.is_empty() && !text.contains(['*', '?', '[']) {
                base.push(component);
            } else {
                parts.push(text);
            }
        }
        if own_worktree(&normalize(&base), cwd) {
            return false;
        }
        if glob_directories(&base, &parts, &mut candidates, &mut 2048).is_err() {
            return true;
        }
        for root in roots {
            if glob::Pattern::new(&text).is_ok_and(|p| p.matches_path(root)) {
                candidates.push(root.clone());
            }
        }
    } else {
        candidates.push(target.to_path_buf());
    }
    candidates.into_iter().any(|path| {
        let path = normalize(&path);
        !own_worktree(&path, cwd)
            && (roots.iter().any(|root| root.starts_with(&path))
                || (path.is_dir() && contains_workspace(&path, cwd, &mut 2048)))
    })
}

/// Bound read work. An unreadable subtree or exhausted budget fails closed.
/// Never follow descendant symlinks, which can cycle or escape the target.
fn contains_workspace(path: &Path, cwd: Option<&Path>, budget: &mut usize) -> bool {
    if own_worktree(path, cwd) {
        return false;
    }
    if path.join(".lucidos").is_dir() && path.join("data").is_dir() {
        return true;
    }
    if *budget == 0 {
        return true;
    }
    *budget -= 1;
    let Ok(entries) = std::fs::read_dir(path) else {
        return true;
    };
    for entry in entries {
        let Ok(entry) = entry else { return true };
        let Ok(kind) = entry.file_type() else {
            return true;
        };
        if kind.is_dir() && contains_workspace(&entry.path(), cwd, budget) {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shell_verdict(
        command: &str,
        cwd: Option<&Path>,
        roots: &[PathBuf],
        home: Option<&Path>,
    ) -> Option<Catastrophe> {
        let scan = TargetScan::read(cwd, home, |scan, scope| scan.shell(command, scope, true, 0));
        judge(&scan, cwd, || roots.to_vec())
    }

    fn targets_workspace_at(
        command: &str,
        cwd: Option<&Path>,
        roots: &[PathBuf],
        home: Option<&Path>,
    ) -> bool {
        shell_verdict(command, cwd, roots, home) == Some(Catastrophe::Proven(REASON))
    }

    fn targets_workspace(command: &str, cwd: Option<&Path>, roots: &[PathBuf]) -> bool {
        targets_workspace_at(
            command,
            cwd,
            roots,
            crate::engine::command_guard::home_dir().as_deref(),
        )
    }

    #[test]
    fn operational_floor_distinguishes_data_from_execution() {
        for command in [
            "python3 - <<'PY'\nimport os,shutil\nfor k in ['LUCIDOS_PG_BIN_DIR']:\n print(os.environ.get(k))\nprint(shutil.which('pg_ctl'))\nPY",
            "cat > fixture.rs <<'EOF'\nlet name = \"LUCIDOS_PG_BIN_DIR\";\npg_ctl -D /tmp/lucidos_example stop\nEOF",
            "cargo fmt --all",
            "cat <<'EOF'\n$(dropdb lucidos_example)\nEOF",
            "cat <<EOF\nUse $LUCIDOS_PG_BIN_DIR and pg_ctl.\nEOF",
            "env cat <<'EOF'\ndropdb lucidos_example\nEOF",
            "bash <<'EOF'\necho 'dropdb lucidos_example'\nEOF",
            "python3 - <<'PY'\nfrom pathlib import Path\nPath('handoff.md').write_text('''Use LUCIDOS_PG_BIN_DIR and pg_ctl.\nNever run dropdb lucidos_example.\n''')\nPY",
            "echo 'dropdb lucidos_example; pg_ctl stop'",
            "rg 'DROP DATABASE lucidos_example' .",
            "# dropdb lucidos_example\ntrue",
            "LUCIDOS_PG_BIN_DIR=/tmp pg_ctl --version",
            "pg_ctl -D /tmp/lucidos_example status",
            "psql -c \"SELECT 'DROP DATABASE lucidos_example'\"",
            "psql -c 'SELECT $$DROP DATABASE lucidos_example$$'",
            "psql <<'SQL'\n/* outer /* nested */ DROP DATABASE lucidos_example; */ SELECT 1;\nSQL",
            "echo lucidos_example; dropdb ordinary_fixture",
            // A source edit that only reads, rewrites and writes text.
            "python3 - <<'PY'\nfrom pathlib import Path\np=Path('guard.rs')\ns=p.read_text().replace('a', 'url.contains(\"/~/api/v1/control/cleanup\")')\ni=s.index('fn main')\ns=s[:i]+'x'+s[i:]\np.write_text(s)\nPY",
            "echo 'DELETE /~/api/v1/control/workspaces/example'",
            "curl http://localhost/~/api/v1/control/workspaces/example",
            "curl http://localhost/~/api/v1/control/cleanup/batches",
            "curl -X DELETE 'http://localhost/notes?next=/~/api/v1/control/workspaces/example'",
            "curl -X POST --json '{\"next\":\"/~/api/v1/control/cleanup/batches\"}' http://localhost/notes",
            // `command -v` and `-V` describe a name and run nothing.
            "/bin/zsh -c \"sed -n '435,470p;544,562p' .claude/commands/harden.md\nrg -n '\"'^#|''^##'\"' .claude/skills/code-review/SKILL.md\nrg -n 'LUCIDOS_|pub fn resolve|fn path' crates/lucidos-secret-store/src\ncommand -v initdb pg_ctl psql docker\"",
            "rg LUCIDOS_ .; command -p -v pg_ctl",
            "rg LUCIDOS_ .; sudo command -V pg_ctl",
            "rg LUCIDOS_ .; sed -n '1,5p' pg_ctl.md",
            "rg LUCIDOS_ .; cut -d: -f1 pg_ctl.md | sort",
            // The evident-shape provers read ordinary names as ordinary.
            "DB=scratch; dropdb \"$DB\"",
            "PGDATA=/tmp/scratch pg_ctl stop",
            "echo 'SELECT 1' | psql",
            "SQL='SELECT 1; dropdb lucidos_example'; psql -c '$SQL'",
            "python3 -c \"print(\\\"os.system('dropdb lucidos_example')\\\")\"",
        ] {
            assert_eq!(reason(command, None), None, "{command}");
        }
        for command in [
            "dropdb lucidos_example",
            "dropdb postgresql://localhost/lucidos_example",
            "echo $(true)#; dropdb lucidos_example",
            "echo x\\ #; dropdb lucidos_example",
            "psql <<'SQL'\nSELECT 1 AS a$x$;\nDROP DATABASE lucidos_example;\n-- $x$\nSQL",
            "env PGHOST=localhost dropdb lucidos_example",
            "psql -c 'DROP DATABASE lucidos_example'",
            "psql -c 'DROP DATABASE IF EXISTS \"lucidos_example\"'",
            "psql <<'SQL'\n/* outer /* nested */ ' */ DROP DATABASE lucidos_example; -- '\nSQL",
            "pg_ctl -D /tmp/lucidos_example stop",
            "pg_ctl restart -D /tmp/lucidos_example",
            "true; bash -c 'dropdb lucidos_example'",
            "echo $(dropdb lucidos_example)",
            "sh <<'EOF'\ndropdb lucidos_example\nEOF",
            "psql <<'SQL'\nDROP DATABASE lucidos_example;\nSQL",
            "python3 -c \"import subprocess; subprocess.run(['dropdb', 'lucidos_example'])\"",
            "curl -X DELETE http://localhost/~/api/v1/control/workspaces/example",
            "curl -sXDELETE http://localhost/~/api/v1/control/workspaces/example",
            "curl --req=DELETE http://localhost/~/api/v1/control/workspaces/example",
            "curl -X POST http://localhost/~/api/v1/control/cleanup/batches",
            "curl -X POST http://localhost/~/api/v1/control/workspace-delete-batches",
            "curl -X POST http://localhost/~/api/v1/control/trash/empty",
            "command -p pg_ctl -D /tmp/lucidos_example stop",
            "command pg_ctl -D /tmp/lucidos_example stop",
            // A lookup runs its operands when `command` is no prefix.
            "rg LUCIDOS_ .; env -u command -v pg_ctl -D /tmp/lucidos_example stop",
            // Evident shapes: a literal variable, piped SQL, `os.system (`.
            "echo 'DROP DATABASE lucidos_example' | psql",
            "printf 'DROP DATABASE lucidos_example;\\n' | psql -U postgres",
            "printf '%s\\n' 'DROP DATABASE lucidos_example;' | psql",
            "DB=lucidos_example; dropdb \"$DB\"",
            "export DB=lucidos_example && dropdb -h localhost ${DB}",
            "DB=lucidos_example; psql -c \"DROP DATABASE $DB\"",
            "DB=lucidos_example; echo \"DROP DATABASE $DB\" | psql",
            "PGDATA=/tmp/lucidos_example; pg_ctl stop",
            "PGDATA=/tmp/lucidos_example pg_ctl stop -m fast",
            "SQL='DROP DATABASE lucidos_example'; psql -c \"$SQL\"",
            "python3 -c \"import os; os.system ('dropdb lucidos_example')\"",
            "python3 -c \"import os; os.system('dropdb lucidos_example')\"",
            "python3 - <<'PY'\nimport os\nos.popen(\"dropdb lucidos_example\").read()\nPY",
            "python3 - <<'PY'\nimport subprocess\ndb = 'lucidos_example'\nsubprocess.run(['dropdb', db])\nPY",
        ] {
            assert_eq!(
                reason(command, None),
                Some(Catastrophe::Proven(REASON)),
                "{command}"
            );
        }
        // Only the substring fallback catches these. The owner may allow
        // each one once; an unattended lane refuses it.
        for command in [
            "cat() { bash; }; cat <<'EOF'\ndropdb lucidos_example\nEOF",
            "function cat { bash; }; cat <<'EOF'\ndropdb lucidos_example\nEOF",
            "cat > script.sh <<'EOF'\ndropdb lucidos_example\nEOF\nchmod +x script.sh; ./script.sh",
            "cat > script.sql <<'SQL'\nDROP DATABASE lucidos_example;\nSQL\npsql -f script.sql",
            "cat > script.sh <<'SH'\ndropdb lucidos_example\nSH\nsh script.sh",
            "cat > script.py <<'PY'\nimport os\nos.system('dropdb lucidos_example')\nPY\npython3 script.py",
            // A value that is not a literal stays unread.
            "DB=$(cat name.txt)lucidos_example; dropdb \"$DB\"",
            "echo 'DB=lucidos_example'; dropdb \"$DB\"",
            "DB=lucidos_example; unset DB; dropdb \"$DB\"",
            "psql -v db=lucidos_example -c 'DROP DATABASE :db'",
            "psql -c 'DROP DATABASE U&\"lucidos_example\"'",
            // A string method name does not exempt an unknown call beside it.
            "python3 - <<'PY'\nfrom pathlib import Path\ns=Path('a').read_text().replace('x', 'y')\nos.posix_spawn('curl', ['-X', 'POST', 'http://localhost/~/api/v1/control/cleanup/batches'], {})\nPY",
            // sed's `e` command runs its text, so the script is not a read.
            "rg LUCIDOS_ .; sed -n '1e pg_ctl stop' notes.txt",
            "rg LUCIDOS_ .; sed -n 1,5p pg_ctl.md > run.sh; sh run.sh",
            // A lookup's operands run when `command` is redefined, or its
            // output runs later.
            "command() { shift; \"$@\"; }; command -v pg_ctl -D /tmp/lucidos_example stop",
            "rg LUCIDOS_ .; command -v pg_ctl > run.sh; sh run.sh",
            "\"$(command -v pg_ctl)\" -D /tmp/lucidos_example stop",
        ] {
            assert_eq!(
                reason(command, None),
                Some(Catastrophe::Unproven(REASON)),
                "{command}"
            );
        }
    }

    #[test]
    fn unproven_consumers_cannot_exempt_generated_code() {
        for command in [
            "cp /bin/sh ./cat; ./cat <<'EOF'\ndropdb lucidos_example\nEOF",
            "env ./cat <<'EOF'\ndropdb lucidos_example\nEOF",
            "hash -p /bin/bash cat; cat <<'EOF'\ndropdb lucidos_example\nEOF",
            "hash cat=/bin/bash; cat <<'EOF'\ndropdb lucidos_example\nEOF",
            "enable -f ./sink.so cat; cat <<'EOF'\ndropdb lucidos_example\nEOF",
            "cat > Makefile <<'EOF'\nall:\n\tdropdb lucidos_example\nEOF\nmake",
            "cat > script.awk <<'EOF'\nBEGIN {system(\"dropdb lucidos_example\")}\nEOF\nawk -f script.awk",
            // Unlisted follow-ups ask the owner, including cargo plugins.
            "cat > fixture.rs <<'EOF'\npg_ctl -D /tmp/lucidos_example stop\nEOF\ncargo fmt --all",
            // These read, but run code named in a config file that an
            // earlier sink wrote.
            "printf '* filter=x\\n' > .gitattributes; cat >> .git/config <<'EOF'\n[filter \"x\"]\n\tclean = dropdb lucidos_example\nEOF\ngit add notes.md",
            "cat >> .git/config <<'EOF'\n[core]\n\tfsmonitor = dropdb lucidos_example\nEOF\ngit status",
            "printf '#!/bin/sh\\ndropdb lucidos_example\\n' > .git/hooks/pre-commit; git commit --allow-empty -m test",
            "printf '#env\\nLESSOPEN=|dropdb lucidos_example %s\\n' > ~/.lesskey; more notes.md",
            "printf -- '--pager=dropdb lucidos_example\\n' > ~/.ackrc; ack notes",
            // These read, but evaluate an operand as code.
            "rg LUCIDOS_ .; [ -v 'path[$(dropdb lucidos_example; echo 1)]' ]",
            "x='a[$''(dropdb lucidos_example)]'; test -v 'b[x]'",
            "x='$(dropdb lucidos_example)'; sed -n p ${(e)x}",
            "x='a[$''(dropdb lucidos_example)]'; sed -n \"$((x))p\" notes.md",
        ] {
            assert_eq!(
                reason(command, None),
                Some(Catastrophe::Unproven(REASON)),
                "{command}"
            );
        }
    }

    #[test]
    fn operands_globs_and_directory_changes_use_only_destructive_targets() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("my-workspace");
        std::fs::create_dir_all(root.join(".lucidos")).unwrap();
        std::fs::create_dir_all(root.join("data")).unwrap();
        std::fs::create_dir_all(temp.path().join("empty")).unwrap();
        for command in [
            "find . -name '*.ts' -exec grep -l foo {} +".to_string(),
            "mv notes.txt .".into(),
            "grep -rn rmtree .".into(),
            "find . -name '*.md' -exec grep -l 'rm -rf' {} +".into(),
            "find . -name '*.pyc' | xargs rm -f".into(),
            "rm -f *.log".into(),
            "find data -name '*.pyc' -delete".into(),
            "rm a.txt; ls ..".into(),
            format!("cd '{}' && rm tmp.txt", root.display()),
            "rm -rf ../empty*".into(),
            "find . -name '*.orig' -print0 | xargs -0 rm -f; git -C . status".into(),
            "import os, shutil\nfor d in os.listdir('.'):\n if d.endswith('.tmp'): shutil.rmtree(d)".into(),
        ] {
            assert!(!targets_workspace(&command, Some(&root), &[]), "{command}");
        }
        for command in [
            "rm -rf my*".to_string(),
            "find . -name my-workspace | xargs -I {} rm -rf {}".into(),
            "find . -name my-workspace | xargs -n 1 rm -rf".into(),
            "find . -name my-workspace | xargs -P4 -L 1 rm -rf".into(),
            "find my-workspace -exec python3 -c \"__import__('shutil').rmtree('{}')\" \\;".into(),
            "ROOT=~; NAME=my-workspace; rm -rf $ROOT/$NAME".into(),
            "WS=my-workspace; rm -rf ./$WS".into(),
            "# tidy up\np='my-workspace'\nshutil.rmtree(p)".into(),
            "p = 'my-workspace'\nshutil.rmtree(p)".into(),
            "python3 -c \"import shutil; shutil.rmtree(os.path.join('.', 'my-workspace'))\"".into(),
            "ls | xargs rm -rf".into(),
            "find . -name my-workspace -print0 | sort -z | xargs -0 rm -rf".into(),
            "find . -name my-workspace | gxargs rm -rf".into(),
            "find . -name my-workspace | xargs --replace rm -rf {}".into(),
            "find . -name my-workspace | xargs -J % rm -rf %".into(),
            "find . -name my-workspace | xargs -I {} -R 1 rm -rf {}".into(),
            "python3 - <<'EOF'\nfrom shutil import rmtree\nrmtree('my-workspace')\nEOF".into(),
            "python3 -c \"import shutil; shutil.rmtree(r'my-workspace')\"".into(),
            "python3 -c \"import shutil; shutil.rmtree(Path('my-workspace'))\"".into(),
            "cd empty && uv run python -c \"__import__('shutil').rmtree('../my-workspace')\"".into(),
            "mv --target-directory=empty my-workspace".into(),
            "mv -tempty my-workspace".into(),
            "WS=my-workspace; rm -rf $WS".into(),
            "WS=my-workspace; rm -rf $WS/".into(),
            "WS=my-workspace; rm -rf ${WS}/*".into(),
            "mv -vt empty my-workspace".into(),
            "find . -delete -name keep".into(),
            "echo my-workspace | xargs rm -rf".into(),
            "ls -d my* | xargs rm -rf".into(),
            "import shutil; shutil.rmtree('my-workspace')".into(),
            "source .venv/bin/activate && python -c \"import shutil; shutil.rmtree('my-workspace')\"".into(),
            "export WS=my-workspace; rm -rf ${WS}".into(),
            "find . ! -name keep -delete".into(),
            "find . -name '*.log' -o -name my-workspace -exec rm -rf {} +".into(),
            "find -L my-workspace -delete".into(),
            "find . -name my-workspace | xargs rm -rf".into(),
            "find my-workspace -exec sh -c 'rm -rf \"$0\"' {} +".into(),
            "(rm -rf my-workspace)".into(),
            "cd && rm -rf my-workspace".into(),
            "rsync -a --delete empty/ my-workspace/ --exclude .git".into(),
            "python3 -c \"from shutil import rmtree; rmtree('my-workspace/')\"".into(),
            "python3 -c \"import shutil; p='my-workspace/'; shutil.rmtree(p)\"".into(),
            "cd empty && rm -rf ../my*".into(),
            format!("cd '{}' && rm -rf my-workspace", temp.path().display()),
            "cd ~ && rm -rf my-workspace".into(),
            "cd $HOME && rm -rf my-workspace".into(),
            "cd ${HOME} && rm -rf my-workspace".into(),
            "find . -name 'my-*' -exec rm -rf {} +".into(),
            "find . -iname 'MY-*' -delete".into(),
        ] {
            assert!(
                targets_workspace_at(&command, Some(temp.path()), &[], Some(temp.path())),
                "{command}"
            );
        }
        assert!(root.exists());
    }

    #[test]
    fn workspace_targets_and_ancestors_are_blocked_without_executing_commands() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("arbitrary-name");
        std::fs::create_dir_all(root.join(".lucidos")).unwrap();
        std::fs::create_dir_all(root.join("data")).unwrap();
        for cmd in [
            format!("rm -rf '{}'", root.display()),
            format!("mv '{}' /tmp/elsewhere", root.display()),
            format!("find '{}' -delete", temp.path().display()),
            format!("rsync --delete empty/ '{}/'", root.display()),
            format!("trash '{}'", root.display()),
            format!(
                "python3 -c \"import shutil; shutil.rmtree('{}')\"",
                root.display()
            ),
            format!("rm -rf '{}/*'", root.display()),
        ] {
            assert!(targets_workspace(&cmd, Some(temp.path()), &[]), "{cmd}");
        }
        let uncreated = temp.path().join("registered-but-missing");
        assert!(targets_workspace(
            &format!("rm -rf {}", uncreated.display()),
            Some(temp.path()),
            &[uncreated]
        ));
        assert!(!targets_workspace(
            &format!("rm -f {}/data/one-file", root.display()),
            Some(temp.path()),
            &[]
        ));
        assert!(root.exists());
    }

    #[test]
    fn ordinary_temp_dir_and_own_worktree_remain_available() {
        let temp = tempfile::tempdir().unwrap();
        let worktree = temp.path().join("worktree");
        let gitdir = temp.path().join("gitdir");
        std::fs::create_dir_all(&gitdir).unwrap();
        std::fs::create_dir_all(worktree.join(".lucidos")).unwrap();
        std::fs::create_dir_all(worktree.join("data")).unwrap();
        std::fs::write(gitdir.join("commondir"), "..").unwrap();
        std::fs::write(
            worktree.join(".git"),
            format!("gitdir: {}", gitdir.display()),
        )
        .unwrap();
        assert!(!targets_workspace("rm -rf .", Some(&worktree), &[]));
        assert!(!targets_workspace("rm -rf **/*", Some(&worktree), &[]));
        let empty = temp.path().join("empty");
        std::fs::create_dir(&empty).unwrap();
        assert!(!targets_workspace(
            &format!("rm -rf {}", empty.display()),
            Some(temp.path()),
            &[]
        ));
    }

    /// Synthetic rewrites of two recorded refusals: a Python source edit whose
    /// string holds `rm -rf`, and a commit message that quotes an `mv`.
    #[test]
    fn data_written_by_a_sink_never_becomes_an_operation() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("my-workspace");
        std::fs::create_dir_all(root.join(".lucidos")).unwrap();
        std::fs::create_dir_all(root.join("data")).unwrap();
        let home = Some(temp.path());
        let verdict = |command: &str| shell_verdict(command, Some(temp.path()), &[], home);
        let python = |code: &str| {
            let scan = TargetScan::read(Some(temp.path()), home, |scan, scope| {
                scan.python(code, scope, true, 0)
            });
            judge(&scan, Some(temp.path()), Vec::new)
        };
        for command in [
            "python3 - <<'EOF'\np='src/permission.rs'\ns=open(p).read()\nnew='''    let input = zsh(\"psql {secret} -c 'select 1' && rm -rf ~\");\n'''\ns=s.replace('// marker', new)\nopen(p,'w').write(s)\nEOF".to_string(),
            "python3 - <<'EOF'\np='docs/notes.md'\ns=open(p).read()\ns=s.replace('a', \"\"\"Renaming a folder\nabove a root moves it.\n\"\"\", 1)\nopen(p,'w').write(s)\nEOF\ncargo fmt --all --check >/dev/null 2>&1 && echo FMT-OK; git add docs/notes.md && git commit -q -F - <<'EOF'\nfix: a folder above a root cannot move\n\nAn agent could rename a folder (`mv ~/my-workspace ~/elsewhere`)\nand then read it.\nEOF\ngit log --oneline -1".into(),
            "echo 'a; rm -rf ~' && ls".into(),
            "cat <<'EOF'\nrm -rf ~\nEOF".into(),
            "git commit -F - <<'EOF'\nrm -rf ~/my-workspace\nEOF".into(),
            "# rm -rf ~\nls".into(),
            "git commit -F - <<'EOF'\nfix: `mv ~/my-workspace ~/elsewhere` now fails\nEOF".into(),
            "cat <<'EOF'\n$(rm -rf ~/my-workspace)\nEOF".into(),
            "echo '$(rm -rf ~/my-workspace)'".into(),
        ] {
            assert_eq!(verdict(&command), None, "{command}");
        }
        assert_eq!(
            python("x = \"rm -rf ~; mv ~/my-workspace /tmp\"\nprint(x)"),
            None
        );
        assert_eq!(python("print(\"shutil.rmtree('my-workspace')\")"), None);
        assert_eq!(python("# shutil.rmtree('my-workspace')\nprint(1)"), None);

        for command in [
            "sh <<'EOF'\nrm -rf ~/my-workspace\nEOF".to_string(),
            "cat <<'EOF' | sh\nrm -rf ~/my-workspace\nEOF".into(),
            "bash -c 'cd ~ && rm -rf my-workspace'".into(),
            "true; bash -c \"echo hi; rm -rf ~/my-workspace\"".into(),
            "node <<'EOF'\nrm -rf ~/my-workspace\nEOF".into(),
            "echo \"$(rm -rf ~/my-workspace)\"".into(),
            "cat <<EOF\n$(rm -rf ~/my-workspace)\nEOF".into(),
            "sh <<'EOF'\necho `rm -rf ~/my-workspace`\nEOF".into(),
            "python3 - <<'EOF'\nimport shutil\nshutil.rmtree('my-workspace')\nEOF".into(),
            format!(
                "python3 - <<'EOF'\nimport subprocess\nsubprocess.run(['rm', '-rf', '{}'])\nEOF",
                root.display()
            ),
        ] {
            assert_eq!(
                verdict(&command),
                Some(Catastrophe::Proven(REASON)),
                "{command}"
            );
        }
        assert_eq!(
            python("import os\nos.system('rm -rf ~/my-workspace')"),
            Some(Catastrophe::Proven(REASON))
        );

        // A file a sink writes may run later, so its text asks the owner.
        for command in [
            "cat > script.sh <<'EOF'\nrm -rf ~/my-workspace\nEOF\nsh script.sh",
            "tee run.sh <<'EOF' >/dev/null\nrm -rf ~/my-workspace\nEOF\nbash run.sh",
        ] {
            assert_eq!(
                verdict(command),
                Some(Catastrophe::Unproven(REASON)),
                "{command}"
            );
        }
        assert!(root.exists());
    }

    #[test]
    fn child_inputs_do_not_change_the_parent_working_directory() {
        let temp = tempfile::tempdir().unwrap();
        let workspace = temp.path().join("demo");
        let verdict = |command| {
            shell_verdict(
                command,
                Some(temp.path()),
                std::slice::from_ref(&workspace),
                None,
            )
        };
        for command in [
            "cat > notes <<'E'\ncd scratch\nE\nrm -rf demo",
            "sh <<'E'\ncd scratch\nE\nrm -rf demo",
            "true; sh -c 'cd scratch'; rm -rf demo",
            "python3 -c \"import os; os.system('cd scratch')\"; rm -rf demo",
        ] {
            assert_eq!(
                verdict(command),
                Some(Catastrophe::Proven(REASON)),
                "{command}"
            );
        }
        assert_eq!(
            verdict("cat() { sh; }; cat <<'E'\nrm -rf demo\nE"),
            Some(Catastrophe::Unproven(REASON))
        );
    }

    #[test]
    fn captured_sink_output_can_become_code() {
        let temp = tempfile::tempdir().unwrap();
        let workspace = temp.path().join("demo");
        let cwd = Some(temp.path());
        let roots = std::slice::from_ref(&workspace);
        let text = "cat <<'DATA'\nrm demo\nDATA\n";
        assert_eq!(shell_verdict(text, cwd, roots, None), None);
        for command in [
            format!("value=\"$({text})\""),
            format!("exec > buffer\n{text}"),
        ] {
            assert_eq!(
                shell_verdict(&command, cwd, roots, None),
                Some(Catastrophe::Unproven(REASON))
            );
        }
    }

    #[test]
    fn an_unplaceable_python_string_keeps_rmtree_targets_for_consent() {
        let code = "import shutil\ncfg = {'n': 'x'}\nprint(f\"{cfg['n']}\")\nshutil.rmtree('demo')";
        assert!(operations::python_identifiers(code).is_none());
        let temp = tempfile::tempdir().unwrap();
        let cwd = Some(temp.path());
        let roots = vec![temp.path().join("demo")];
        let scan = TargetScan::read(cwd, None, |scan, scope| scan.python(code, scope, true, 0));
        assert_eq!(
            judge(&scan, cwd, || roots.clone()),
            Some(Catastrophe::Unproven(REASON))
        );
        let heredoc = format!("python3 - <<'PY'\n{code}\nPY");
        assert_eq!(
            shell_verdict(&heredoc, cwd, &roots, None),
            Some(Catastrophe::Unproven(REASON))
        );
    }

    #[test]
    fn escaped_python_exec_source_keeps_its_target() {
        let temp = tempfile::tempdir().unwrap();
        let cwd = Some(temp.path());
        let verdict = |code: &str| {
            let scan = TargetScan::read(cwd, None, |scan, scope| scan.python(code, scope, true, 0));
            judge(&scan, cwd, || vec![temp.path().join("demo")])
        };
        for code in [
            "import shutil\nexec(\"shutil.rmtree('demo')\\n\")",
            "import shutil\nexec(\"shutil.rmtree(\\\"demo\\\")\")",
            "import shutil\nexec(r\"shutil.rmtree('demo') # \\d\")",
        ] {
            assert_eq!(verdict(code), Some(Catastrophe::Proven(REASON)), "{code}");
        }
        assert_eq!(
            verdict("import shutil\nexec(\"shutil.rmtree('demo') # \\d\")"),
            Some(Catastrophe::Unproven(REASON))
        );
    }

    #[test]
    fn python_source_consumers_and_deferred_bindings_keep_targets() {
        let source = "shutil.rmtree('demo')";
        assert!(python_call_targets(&format!("print(\"{source}\")")).is_empty());
        let temp = tempfile::tempdir().unwrap();
        let cwd = Some(temp.path());
        let verdict = |code: &str| {
            let scan = TargetScan::read(cwd, None, |scan, scope| scan.python(code, scope, true, 0));
            judge(&scan, cwd, || vec![temp.path().join("demo")])
        };
        assert_eq!(
            verdict(&format!("exec(\"{source}\")")),
            Some(Catastrophe::Proven(REASON))
        );
        assert_eq!(
            verdict(&format!("source = \"\"\"\n{source}\n\"\"\"\nexec(source)")),
            Some(Catastrophe::Proven(REASON))
        );
        assert_eq!(
            verdict(&format!("compile(\"{source}\", '<source>', 'exec')")),
            Some(Catastrophe::Unproven(REASON))
        );
        assert_eq!(
            verdict("def cleanup():\n    shutil.rmtree(target)\ntarget = 'demo'\ncleanup()"),
            Some(Catastrophe::Unproven(REASON))
        );
        assert_eq!(
            verdict("def cleanup():\n    shutil.rmtree(target)\ntarget = 'demo'\ncleanup()\ntarget = 'notes'"),
            Some(Catastrophe::Unproven(REASON))
        );
    }

    #[test]
    fn python_fallback_does_not_consume_the_depth_budget() {
        let code = "\"\"\"Run `printf ok`.\"\"\"\nprint(f\"{row['name']}\")";
        assert!(super::super::python_shell_outs(code).is_none());
        assert_eq!(python_reason(code, None), None);
    }

    #[test]
    fn exhausted_nesting_requires_consent() {
        let depth = MAX_SCAN_DEPTH + 2;
        let command = format!("{}printf ok{}", "$(".repeat(depth), ")".repeat(depth));
        assert_eq!(
            shell_verdict(&command, None, &[], None),
            Some(Catastrophe::Unproven(REASON))
        );
    }

    #[test]
    fn the_last_shell_level_preserves_input_boundaries() {
        let temp = tempfile::tempdir().unwrap();
        let cwd = Some(temp.path());
        let verdict = |command: &str| {
            let scan = TargetScan::read(cwd, None, |scan, scope| {
                scan.shell(command, scope, true, MAX_SCAN_DEPTH);
            });
            judge(&scan, cwd, || vec![temp.path().join("demo")])
        };
        assert_eq!(verdict("cat <<'DATA'\nrm demo\nDATA"), None);
        assert_eq!(
            verdict("sh <<'DATA'\nrm demo\nDATA"),
            Some(Catastrophe::Unproven(REASON))
        );
    }
}
