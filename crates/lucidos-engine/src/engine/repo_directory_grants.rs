use std::path::{Component, Path, PathBuf};

const REPO_CC_SETTINGS: &str = ".claude/settings.json";

pub async fn resolve(worktree: &Path, workspace_root: &Path) -> Vec<PathBuf> {
    let worktree = match std::fs::canonicalize(worktree) {
        Ok(worktree) => worktree,
        Err(e) => {
            crate::log_legacy!(
                "[RepoDirectoryGrants] Could not resolve {}: {}",
                worktree.display(),
                e
            );
            return Vec::new();
        }
    };
    let entries = entries_leaving_the_repo(&worktree);
    if entries.is_empty() {
        return Vec::new();
    }
    let protected = match Protected::find(&worktree, workspace_root).await {
        Ok(protected) => protected,
        Err(e) => {
            crate::log_legacy!(
                "[RepoDirectoryGrants] No main checkout for {} ({}). Not granting {:?}.",
                worktree.display(),
                e,
                entries
            );
            return Vec::new();
        }
    };
    if protected.main_checkout == worktree {
        return Vec::new();
    }
    let mut granted = Vec::new();
    for entry in entries {
        match reanchor(&entry, &protected) {
            Ok(dir) => {
                crate::log_legacy!(
                    "[RepoDirectoryGrants] Granting {} for {:?}",
                    dir.display(),
                    entry
                );
                if !granted.contains(&dir) {
                    granted.push(dir);
                }
            }
            Err(why) => crate::log_legacy!(
                "[RepoDirectoryGrants] Not granting {:?} from {}: {}",
                entry,
                worktree.join(REPO_CC_SETTINGS).display(),
                why
            ),
        }
    }
    granted
}

fn entries_leaving_the_repo(worktree: &Path) -> Vec<String> {
    let path = worktree.join(REPO_CC_SETTINGS);
    let body = match std::fs::read_to_string(&path) {
        Ok(body) => body,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
        Err(e) => {
            crate::log_legacy!(
                "[RepoDirectoryGrants] Could not read {}: {}",
                path.display(),
                e
            );
            return Vec::new();
        }
    };
    let settings: serde_json::Value = match serde_json::from_str(&body) {
        Ok(settings) => settings,
        Err(e) => {
            crate::log_legacy!(
                "[RepoDirectoryGrants] Could not parse {}: {}",
                path.display(),
                e
            );
            return Vec::new();
        }
    };
    let Some(entries) = settings.pointer("/permissions/additionalDirectories") else {
        return Vec::new();
    };
    let Some(entries) = entries.as_array() else {
        crate::log_legacy!(
            "[RepoDirectoryGrants] {}: additionalDirectories is not a list",
            path.display()
        );
        return Vec::new();
    };
    let Some(root) = fold_dots(worktree) else {
        return Vec::new();
    };
    entries
        .iter()
        .filter_map(|entry| {
            let entry = entry.as_str();
            if entry.is_none() {
                crate::log_legacy!(
                    "[RepoDirectoryGrants] {}: skipping a non-string additionalDirectories entry",
                    path.display()
                );
            }
            entry
        })
        .filter(|entry| !Path::new(entry).is_absolute() && !entry.starts_with('~'))
        .filter(|entry| fold_dots(&root.join(entry)).is_none_or(|p| !p.starts_with(&root)))
        .map(str::to_string)
        .collect()
}

struct Protected {
    main_checkout: PathBuf,
    git_dir: PathBuf,
    worktree: PathBuf,
    workspace: PathBuf,
}

impl Protected {
    async fn find(worktree: &Path, workspace_root: &Path) -> Result<Self, String> {
        use crate::engine::git_ops::{git_common_dir, git_main_worktree};
        let canonical = |p: PathBuf| std::fs::canonicalize(p).map_err(|e| e.to_string());
        Ok(Self {
            main_checkout: canonical(git_main_worktree(worktree).await?)?,
            git_dir: canonical(git_common_dir(worktree).await?)?,
            worktree: worktree.to_path_buf(),
            workspace: canonical(workspace_root.to_path_buf())?,
        })
    }
}

fn reanchor(entry: &str, protected: &Protected) -> Result<PathBuf, String> {
    let target = std::fs::canonicalize(protected.main_checkout.join(entry))
        .map_err(|e| format!("it does not resolve to a directory ({e})"))?;
    if !target.is_dir() {
        return Err(format!("{} is not a directory", target.display()));
    }
    let overlapped = [
        ("the main checkout", &protected.main_checkout),
        ("the repo's .git", &protected.git_dir),
        ("the worktree", &protected.worktree),
        ("the workspace", &protected.workspace),
    ]
    .into_iter()
    .find(|(_, dir)| target.starts_with(dir) || dir.starts_with(&target));
    match overlapped {
        Some((name, _)) => Err(format!("{} overlaps {name}", target.display())),
        None => Ok(target),
    }
}

fn fold_dots(path: &Path) -> Option<PathBuf> {
    let mut folded = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !folded.pop() {
                    return None;
                }
            }
            other => folded.push(other),
        }
    }
    Some(folded)
}

#[cfg(test)]
#[path = "repo_directory_grants_tests.rs"]
mod tests;
