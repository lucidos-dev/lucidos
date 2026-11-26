use super::git_cmd;
use std::collections::HashMap;
use std::path::Path;
#[cfg(test)]
use std::path::PathBuf;
use std::sync::{LazyLock, Mutex};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MergeProbe {
    Clean,
    Conflicts(Vec<String>),
    /// INVARIANT: git could not answer: it failed to run, timed out, or could not resolve
    /// a revision. Never read this as clean.
    Unknown,
}

impl MergeProbe {
    pub fn into_conflicts(self) -> Vec<String> {
        match self {
            MergeProbe::Conflicts(files) => files,
            MergeProbe::Clean | MergeProbe::Unknown => Vec::new(),
        }
    }
}

pub async fn probe_merge(dir: &Path, ours: &str, theirs: &str) -> MergeProbe {
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
                log_legacy!(
                    "[Git] `git {}` failed in {}: {}",
                    args.join(" "),
                    dir.display(),
                    String::from_utf8_lossy(&o.stderr).trim()
                );
            }
            probe
        }
        Err(e) => {
            log_legacy!("[Git] cannot probe a merge in {}: {}", dir.display(), e);
            MergeProbe::Unknown
        }
    }
}

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

/// INVARIANT: Answers keyed by repo and the two commit shas. A sha names its content, so
/// an entry is true for as long as it exists, and nothing invalidates it.
type ProbeKey = (String, String, String);
static PROBE_CACHE: LazyLock<Mutex<HashMap<ProbeKey, MergeProbe>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

const PROBE_CACHE_CAPACITY: usize = 512;

pub async fn predict_merge_into_main(repo_root: &Path, branch: &str) -> MergeProbe {
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
#[path = "conflict_probe_tests.rs"]
mod tests;
