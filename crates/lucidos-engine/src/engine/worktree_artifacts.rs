//! Paths Lucidos injects into coding-agent worktrees.

/// Marker file written to each CC worktree identifying the owning workspace.
pub const WORKTREE_WORKSPACE_MARKER: &str = ".lucidos-workspace";

/// Engine-injected runtime directory under every workspace.
/// `ensure_workspace_bin_symlink` writes `.lucidos/bin/lucidos` (the CLI
/// symlink) here. External repos rarely gitignore `.lucidos/`, so without
/// the exclude every auto-commit drags the symlink along as a fake "diff".
/// `branch_changed_files` filters the same prefix so already-committed
/// instances also stop showing up.
pub(crate) const RUNTIME_PATH_PREFIX: &str = ".lucidos/";

/// Paths the engine writes into every CC worktree as runtime artifacts.
/// Each is appended to the worktree's `.git/info/exclude` at session start so
/// external repos never accumulate Lucidos-internal files in their git. Files
/// stay visible on disk (CC reads them); git just doesn't see them. No-op for
/// the Lucidos repo itself, where the skill file is intentionally tracked —
/// gitignore rules are silent for already-tracked paths.
///
/// A `**/` prefix matches at any depth. The injected CLI skill lives at the
/// root for repository spawns and under the app folder for app spawns.
pub const WORKTREE_EXCLUDE_PATHS: &[&str] = &[
    WORKTREE_WORKSPACE_MARKER,
    "**/.claude/skills/lucidos-cli/",
    RUNTIME_PATH_PREFIX,
];

/// True for engine-injected paths in `WORKTREE_EXCLUDE_PATHS`. A trailing slash
/// matches a directory prefix. Other entries match exactly. A leading `**/`
/// also matches paths nested under an app directory.
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

    // Root match — same semantics as before for non-`**/` entries.
    let root_match = if dir_form.is_some() {
        path == trimmed || path.starts_with(body)
    } else {
        path == trimmed
    };
    if root_match {
        return true;
    }

    // Anywhere-deeper match — only for `**/`-prefixed entries.
    if anywhere {
        let nested_dir = format!("/{}", trimmed);
        let nested_prefix = format!("/{}/", trimmed);
        return path.ends_with(&nested_dir) || path.contains(&nested_prefix);
    }

    false
}
