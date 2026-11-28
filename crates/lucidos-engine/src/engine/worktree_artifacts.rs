pub const WORKTREE_WORKSPACE_MARKER: &str = ".lucidos-workspace";

/// INVARIANT: Engine-injected runtime directory under every workspace.
/// `ensure_workspace_bin_symlink` writes `.lucidos/bin/lucidos` (the CLI
/// symlink) here. External repos rarely gitignore `.lucidos/`, so without
/// the exclude every auto-commit drags the symlink along as a fake "diff".
/// `branch_changed_files` filters the same prefix so already-committed
/// instances also stop showing up.
pub(crate) const RUNTIME_PATH_PREFIX: &str = ".lucidos/";

pub const WORKTREE_EXCLUDE_PATHS: &[&str] = &[
    WORKTREE_WORKSPACE_MARKER,
    "**/.claude/skills/lucidos-cli/",
    RUNTIME_PATH_PREFIX,
];

pub fn is_engine_injected_path(path: &str) -> bool {
    WORKTREE_EXCLUDE_PATHS
        .iter()
        .any(|entry| entry_matches_path(entry, path))
}

fn entry_matches_path(entry: &str, path: &str) -> bool {
    let (anywhere, body) = match entry.strip_prefix("**/") {
        Some(rest) => (true, rest),
        None => (false, entry),
    };
    let dir_form = body.strip_suffix('/');
    let trimmed = dir_form.unwrap_or(body);

    let root_match = if dir_form.is_some() {
        path == trimmed || path.starts_with(body)
    } else {
        path == trimmed
    };
    if root_match {
        return true;
    }

    if anywhere {
        let nested_dir = format!("/{}", trimmed);
        let nested_prefix = format!("/{}/", trimmed);
        return path.ends_with(&nested_dir) || path.contains(&nested_prefix);
    }

    false
}
