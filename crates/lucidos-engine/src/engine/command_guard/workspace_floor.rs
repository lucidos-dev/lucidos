//! Read-only discovery for the unconditional workspace destruction floor.
use std::path::{Component, Path, PathBuf};
use std::sync::LazyLock;

const REASON: &str = "workspace destruction (requires the gateway's device-confirmed cleanup)";

pub(super) fn reason(command: &str, cwd: Option<&Path>) -> Option<&'static str> {
    let lower = command.to_ascii_lowercase();
    if lower.contains("lucidos_")
        && (sql_drop_database(&lower) || lower.contains("dropdb") || lower.contains("pg_ctl"))
    {
        return Some(REASON);
    }
    let url = lower.replace("%2f", "/");
    if (url.contains("/~/api/v1/control/workspaces/")
        && (lower.contains("delete") || lower.contains("-xdelete")))
        || url.contains("/~/api/v1/control/cleanup")
        || url.contains("/~/api/v1/control/trash")
    {
        return Some(REASON);
    }
    if !super::command_segments(command).any(|segment| {
        (command.contains("xargs") && destructive_text(command))
            || !python_targets(command).is_empty()
            || !destructive_operands(&super::unwrap_shell_command(&segment)).is_empty()
    }) {
        return None;
    }
    let roots = registered_roots();
    let default_cwd = std::env::current_dir().ok();
    targets_workspace(command, cwd.or(default_cwd.as_deref()), &roots).then_some(REASON)
}

fn sql_drop_database(command: &str) -> bool {
    static DROP: LazyLock<regex::Regex> =
        LazyLock::new(|| regex::Regex::new(r"\bdrop\s+database\b").unwrap());
    DROP.is_match(command)
}

fn registered_roots() -> Vec<PathBuf> {
    let mut data_dirs: Vec<_> = lucidos_installs::scan_this_machine(None)
        .installs
        .into_iter()
        .filter_map(|i| i.data_dir)
        .collect();
    if let Some(dir) = lucidos_home::gateway_data_dir() {
        data_dirs.push(dir);
    }
    let mut roots = Vec::new();
    for data in data_dirs {
        let Ok(bytes) = std::fs::read(data.join("config/workspaces.json")) else {
            continue;
        };
        let Ok(registry) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
            continue;
        };
        if let Some(entries) = registry["workspaces"].as_array() {
            for entry in entries {
                if let Some(dir) = entry["dir"].as_str() {
                    roots.push(normalize(&data.join(dir)));
                }
            }
        }
    }
    roots
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
        return python_targets(command);
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
fn python_call_targets(code: &str) -> Vec<String> {
    static CALL: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::Regex::new(r#"\brmtree\s*\(\s*(?:(?:os\.path\.expanduser|Path|pathlib\.Path)\s*\(\s*)?(?:[rRuU]?'([^']*)'|[rRuU]?"([^"]*)"|([A-Za-z_]\w*))"#).unwrap()
    });
    static BIND: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::Regex::new(r#"(?:^|[;\n])\s*([A-Za-z_]\w*)\s*=\s*(?:(?:os\.path\.expanduser|Path|pathlib\.Path)\s*\(\s*)?(?:[rRuU]?'([^']*)'|[rRuU]?"([^"]*)")"#).unwrap()
    });
    let bindings: std::collections::HashMap<_, _> = BIND
        .captures_iter(code)
        .map(|c| {
            (
                c[1].to_string(),
                c.get(2).or_else(|| c.get(3)).unwrap().as_str().to_string(),
            )
        })
        .collect();
    static JOIN: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::Regex::new(r"\brmtree\s*\(\s*os\.path\.join\s*\(([^()]*)\)").unwrap()
    });
    let joined = JOIN.captures_iter(code).filter_map(|c| {
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
        Some(path.to_string_lossy().into_owned())
    });
    CALL.captures_iter(code)
        .filter_map(|c| {
            c.get(1)
                .or_else(|| c.get(2))
                .map(|v| v.as_str().to_string())
                .or_else(|| c.get(3).and_then(|v| bindings.get(v.as_str()).cloned()))
        })
        .chain(joined)
        .collect()
}

fn python_targets(command: &str) -> Vec<String> {
    let words = super::shell_argv(command);
    let refs: Vec<_> = words.iter().map(|w| w.text.as_str()).collect();
    let heads = super::danger_head_candidates(&refs);
    if heads
        .iter()
        .any(|(head, _)| head == "python" || head.starts_with("python3"))
        || super::command_segments(command).any(|segment| {
            let words = super::shell_argv(&segment);
            let refs: Vec<_> = words.iter().map(|w| w.text.as_str()).collect();
            super::danger_head_candidates(&refs)
                .iter()
                .any(|(head, _)| head == "python" || head.starts_with("python3"))
                || (words.first().is_some_and(|w| w.text == "uv")
                    && words.iter().any(|w| w.text.starts_with("python")))
        })
        || (words.first().is_some_and(|w| w.text == "uv")
            && words.iter().any(|w| w.text.starts_with("python")))
    {
        if command.contains("<<") {
            return python_call_targets(command);
        }
        return words
            .windows(2)
            .filter(|w| w[0].text == "-c")
            .flat_map(|w| python_call_targets(&w[1].text))
            .collect();
    }
    let first = command
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty() && !line.starts_with('#'))
        .unwrap_or("");
    if first.starts_with("import ")
        || first.starts_with("from ")
        || first.starts_with("shutil.rmtree(")
        || first.contains("='")
        || first.contains("= '")
        || first.contains("= \"")
        || first.contains("=\"")
        || first.starts_with("\"")
        || first.starts_with(char::from(39))
    {
        python_call_targets(command)
    } else {
        Vec::new()
    }
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

fn targets_workspace(command: &str, cwd: Option<&Path>, roots: &[PathBuf]) -> bool {
    targets_workspace_at(command, cwd, roots, super::home_dir().as_deref())
}

fn targets_workspace_at(
    command: &str,
    cwd: Option<&Path>,
    roots: &[PathBuf],
    home: Option<&Path>,
) -> bool {
    let mut effective = cwd.map(Path::to_path_buf);
    let mut python = python_targets(command);
    let segments: Vec<_> = super::command_segments(command).collect();
    let separators = super::segment_separators(command);
    let mut variables = std::collections::HashMap::<String, String>::new();
    for (index, segment) in segments.iter().enumerate() {
        let inner = super::unwrap_shell_command(segment);
        let argv = super::shell_argv(&inner);
        for word in &argv {
            if let Some((name, value)) = word.text.split_once('=') {
                if !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                    variables.insert(name.to_string(), value.to_string());
                }
            }
        }
        if argv
            .first()
            .is_some_and(|w| w.text.trim_start_matches('(') == "cd")
        {
            let destination = argv
                .iter()
                .skip(1)
                .find(|w| !w.text.starts_with('-'))
                .map_or("~", |w| w.text.as_str());
            effective = expand_path(destination, effective.as_deref(), home);
            continue;
        }
        if let Some(destination) = super::cd_destination(&inner) {
            effective = expand_path(&destination, effective.as_deref(), home);
            continue;
        }
        let mut operands = destructive_operands(&inner);
        let feeds_delete = ((index + 1)..segments.len())
            .take_while(|next| separators.get(next - 1).is_some_and(|s| s == "|"))
            .any(|next| {
                let next = &segments[next];
                let words = super::shell_argv(next);
                let refs: Vec<_> = words.iter().map(|w| w.text.as_str()).collect();
                super::danger_head_candidates(&refs)
                    .iter()
                    .any(|(head, args)| {
                        matches!(head.as_str(), "xargs" | "gxargs") && xargs_deletes(args)
                    })
            });
        if feeds_delete {
            let refs: Vec<_> = argv.iter().map(|w| w.text.as_str()).collect();
            let heads = super::danger_head_candidates(&refs);
            if heads.iter().any(|(head, _)| head == "find") {
                operands.extend(destructive_operands(&format!("{inner} -delete")));
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
        operands.append(&mut python);
        for operand in operands {
            static VARIABLE: LazyLock<regex::Regex> = LazyLock::new(|| {
                regex::Regex::new(r"\$\{([A-Za-z_]\w*)\}|\$([A-Za-z_]\w*)").unwrap()
            });
            let operand = VARIABLE.replace_all(&operand, |caps: &regex::Captures<'_>| {
                let name = caps.get(1).or_else(|| caps.get(2)).unwrap().as_str();
                variables
                    .get(name)
                    .cloned()
                    .unwrap_or_else(|| caps[0].to_string())
            });
            let Some(expanded) = expand_path(&operand, effective.as_deref(), home) else {
                continue;
            };
            let text = expanded.to_string_lossy();
            let mut candidates = Vec::new();
            if text.contains(['*', '?', '[']) {
                // A whole-component star at the root deletes its contents.
                if expanded.file_name().is_some_and(|s| s == "*") {
                    if let Some(parent) = expanded.parent() {
                        candidates.push(parent.to_path_buf());
                    }
                }
                let mut base = PathBuf::new();
                let mut parts = Vec::new();
                for component in expanded.components() {
                    let text = component.as_os_str().to_string_lossy().to_string();
                    if parts.is_empty() && !text.contains(['*', '?', '[']) {
                        base.push(component);
                    } else {
                        parts.push(text);
                    }
                }
                if own_worktree(&normalize(&base), cwd) {
                    continue;
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
                candidates.push(expanded);
            }
            for path in candidates {
                let path = normalize(&path);
                if own_worktree(&path, cwd) {
                    continue;
                }
                if roots.iter().any(|root| root.starts_with(&path)) {
                    return true;
                }
                let mut budget = 2048;
                if path.is_dir() && contains_workspace(&path, cwd, &mut budget) {
                    return true;
                }
            }
        }
    }
    false
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
}
