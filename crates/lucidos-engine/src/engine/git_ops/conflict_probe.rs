//! Whether merging a branch into `main` would conflict, asked without merging.
//!
//! `git merge-tree --write-tree` computes the merge in memory. It writes the
//! result's objects to the object store and touches no ref, index or worktree.

use super::*;
use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

/// What a merge of one revision into another would do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum MergeProbe {
    Clean,
    /// The paths that would conflict, each named once.
    Conflicts(Vec<String>),
    /// git could not answer: it failed to run, timed out, or could not resolve
    /// a revision. Never read this as clean.
    Unknown,
}

/// Probe a merge of `theirs` into `ours`, run in `dir`.
pub(crate) async fn probe_merge(dir: &Path, ours: &str, theirs: &str) -> MergeProbe {
    let args = [
        "merge-tree",
        "--write-tree",
        "--name-only",
        "--no-messages",
        ours,
        theirs,
    ];
    match git_cmd(&args, dir).await {
        Ok(o) => {
            let probe = classify_merge_tree(o.status.code(), &String::from_utf8_lossy(&o.stdout));
            if probe == MergeProbe::Unknown {
                log!(
                    "[Git] `git {}` failed in {}: {}",
                    args.join(" "),
                    dir.display(),
                    String::from_utf8_lossy(&o.stderr).trim()
                );
            }
            probe
        }
        Err(e) => {
            log!("[Git] cannot probe a merge in {}: {}", dir.display(), e);
            MergeProbe::Unknown
        }
    }
}

/// Read `merge-tree --write-tree --name-only --no-messages`: exit 0 is a clean
/// merge, exit 1 a conflicted one, and anything else a failure. On a conflict
/// the first stdout line is the result tree and each later line one path.
fn classify_merge_tree(exit_code: Option<i32>, stdout: &str) -> MergeProbe {
    match exit_code {
        Some(0) => MergeProbe::Clean,
        Some(1) => MergeProbe::Conflicts(
            stdout
                .lines()
                .skip(1)
                .map(str::trim)
                .filter(|l| !l.is_empty())
                .map(String::from)
                .collect(),
        ),
        _ => MergeProbe::Unknown,
    }
}

/// Answers keyed by repo and the two commit shas. A sha names its content, so
/// an entry is true for as long as it exists, and nothing invalidates it.
type ProbeKey = (String, String, String);
static PROBE_CACHE: LazyLock<Mutex<HashMap<ProbeKey, MergeProbe>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Enough for every pending change against a few recent `main` tips. Past it
/// the cache starts over, since entries for an old `main` are never asked again.
const PROBE_CACHE_CAPACITY: usize = 512;

/// Would merging `branch` into `main` in `repo_root` conflict right now?
///
/// Resolves both tips first, so an answer is reused until either one moves.
/// An `Unknown` is not cached: the next call asks git again.
pub(crate) async fn predict_merge_into_main(repo_root: &Path, branch: &str) -> MergeProbe {
    // The trailing `--` makes both names revisions, never paths, and is
    // echoed back as a third line.
    let tips = git_cmd(&["rev-parse", "main", branch, "--"], repo_root).await;
    let (main_sha, branch_sha) = match tips {
        Ok(o) if o.status.success() => {
            let stdout = String::from_utf8_lossy(&o.stdout).to_string();
            let mut lines = stdout.lines().map(str::trim);
            match (lines.next(), lines.next()) {
                (Some(m), Some(b)) if !m.is_empty() && !b.is_empty() => {
                    (m.to_string(), b.to_string())
                }
                _ => return MergeProbe::Unknown,
            }
        }
        _ => return MergeProbe::Unknown,
    };
    let key = (
        repo_root.to_string_lossy().to_string(),
        main_sha,
        branch_sha,
    );
    if let Some(hit) = PROBE_CACHE.lock().expect("probe cache").get(&key) {
        return hit.clone();
    }
    let probe = probe_merge(repo_root, &key.1, &key.2).await;
    if probe != MergeProbe::Unknown {
        let mut cache = PROBE_CACHE.lock().expect("probe cache");
        if cache.len() >= PROBE_CACHE_CAPACITY {
            cache.clear();
        }
        cache.insert(key, probe.clone());
    }
    probe
}

#[cfg(test)]
#[path = "../git_ops_tests/conflict_probe.rs"]
mod tests;
