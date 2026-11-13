//! Git bundles of unresolved change branches, carried inside a backup.
//!
//! A change branch lives in the repository its coding agent worked in. For a
//! workspace repository that is the workspace's own `.git`, which the tar
//! already holds. Every other repository (the Lucidos source checkout, an
//! external repo) sits outside the workspace, so the backup bundles its
//! branches here. Each bundle takes `main` as its prerequisite, so it holds
//! only the commits `main` lacks.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use super::BoxError;
use crate::engine::git_ops::{git_cmd, git_cmd_env_timeout};

/// The archive directory holding the bundles and their manifest.
pub const ARCHIVE_DIR: &str = "lucidos_pending_changes";
const MANIFEST: &str = "manifest.json";
const MAIN: &str = "refs/heads/main";

/// A restore may fetch `origin` to supply missing prerequisites, and a large
/// repository takes longer than the usual git ceiling.
const ORIGIN_FETCH_TIMEOUT: Duration = Duration::from_secs(600);

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct Manifest {
    pub repos: Vec<RepoBundle>,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct RepoBundle {
    pub repo_root: String,
    /// The bundle's file name inside [`ARCHIVE_DIR`].
    pub bundle: String,
    pub branches: Vec<BundledBranch>,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct BundledBranch {
    pub name: String,
    pub tip: String,
}

/// Bundle every branch in `branches` that has commits `main` lacks, into
/// `out_dir`, with a manifest beside them. Repositories inside `workspace` are
/// skipped, since the tar carries their `.git`. Returns `false` when nothing
/// needed bundling, and then writes nothing.
///
/// A branch that no longer exists is skipped: its change row outlived it. So
/// is a path that is no longer a git checkout. A repository that git cannot
/// read fails the backup, because a backup must never silently drop pending
/// work.
pub(crate) async fn write_bundles(
    workspace: &Path,
    branches: &[(String, String)],
    out_dir: &Path,
) -> Result<bool, BoxError> {
    let mut manifest = Manifest { repos: Vec::new() };
    for (repo_root, names) in group_by_repo(branches) {
        let repo = Path::new(repo_root);
        if is_inside(repo, workspace) {
            continue;
        }
        // A checkout's `.git` is a directory, or a file in a linked worktree.
        if !repo.join(".git").exists() {
            crate::log!(
                "[Backup] Skipping pending branches of {}: no git checkout is there",
                repo.display()
            );
            continue;
        }
        let has_main = rev_parse(repo, MAIN).await?.is_some();
        let mut bundled = Vec::new();
        for name in names {
            let Some(tip) = rev_parse(repo, &format!("refs/heads/{name}")).await? else {
                continue;
            };
            if has_main && !has_commits_beyond_main(repo, name).await? {
                continue;
            }
            bundled.push(BundledBranch {
                name: name.to_string(),
                tip,
            });
        }
        if bundled.is_empty() {
            continue;
        }

        std::fs::create_dir_all(out_dir)?;
        let bundle = format!("{}.bundle", manifest.repos.len());
        let bundle_path = out_dir.join(&bundle);
        let bundle_arg = bundle_path.to_string_lossy();
        let mut args = vec!["bundle", "create", bundle_arg.as_ref()];
        let refs: Vec<String> = bundled
            .iter()
            .map(|b| format!("refs/heads/{}", b.name))
            .collect();
        args.extend(refs.iter().map(String::as_str));
        let not_main = format!("^{MAIN}");
        if has_main {
            args.push(&not_main);
        }
        run(repo, &args).await?;
        manifest.repos.push(RepoBundle {
            repo_root: repo_root.to_string(),
            bundle,
            branches: bundled,
        });
    }
    if manifest.repos.is_empty() {
        return Ok(false);
    }
    std::fs::write(
        out_dir.join(MANIFEST),
        serde_json::to_vec_pretty(&manifest)?,
    )?;
    Ok(true)
}

/// Bring back every branch the bundles in `dir` hold, each into the repository
/// it came from. Returns one line per branch that could not come back. A
/// missing repository or a refused branch never stops the others.
///
/// An existing branch is only fast-forwarded, never overwritten, so a restore
/// cannot discard newer work in the target repository.
pub(crate) async fn restore_bundles(dir: &Path) -> Result<Vec<String>, BoxError> {
    let manifest: Manifest = serde_json::from_slice(&std::fs::read(dir.join(MANIFEST))?)?;
    let mut problems = Vec::new();
    for repo in &manifest.repos {
        let root = Path::new(&repo.repo_root);
        if rev_parse(root, "HEAD").await.is_err() {
            problems.push(format!(
                "{}: no git repository there, so its {} pending branch(es) were not restored",
                repo.repo_root,
                repo.branches.len()
            ));
            continue;
        }
        let bundle = dir.join(&repo.bundle);
        let mut fetched_origin = false;
        for branch in &repo.branches {
            let result = match fetch_branch(root, &bundle, &branch.name).await {
                Err(e) if lacks_prerequisites(&e) && !fetched_origin => {
                    fetched_origin = true;
                    match fetch_origin(root).await {
                        Ok(()) => fetch_branch(root, &bundle, &branch.name).await,
                        Err(origin) => Err(format!("{e} (fetching origin failed too: {origin})")),
                    }
                }
                other => other,
            };
            let problem = match result {
                Err(e) => Some(e),
                Ok(()) => match rev_parse(root, &format!("refs/heads/{}", branch.name)).await {
                    Ok(Some(tip)) if tip == branch.tip => None,
                    Ok(tip) => Some(format!(
                        "the branch is at {}, not the backed-up {}",
                        tip.as_deref().unwrap_or("nothing"),
                        branch.tip
                    )),
                    Err(e) => Some(e.to_string()),
                },
            };
            if let Some(problem) = problem {
                problems.push(format!("{} {}: {problem}", repo.repo_root, branch.name));
            }
        }
    }
    Ok(problems)
}

fn group_by_repo(branches: &[(String, String)]) -> Vec<(&str, Vec<&str>)> {
    let mut groups: Vec<(&str, Vec<&str>)> = Vec::new();
    for (repo, branch) in branches {
        match groups.iter_mut().find(|(r, _)| *r == repo.as_str()) {
            Some((_, names)) if !names.contains(&branch.as_str()) => names.push(branch),
            Some(_) => {}
            None => groups.push((repo, vec![branch])),
        }
    }
    groups
}

fn is_inside(repo: &Path, workspace: &Path) -> bool {
    let canonical = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| PathBuf::from(p));
    canonical(repo).starts_with(canonical(workspace))
}

/// The commit `rev` names, `None` when it names nothing. `Err` when git could
/// not answer, which a caller must not read as "no such branch".
async fn rev_parse(repo: &Path, rev: &str) -> Result<Option<String>, BoxError> {
    let commit = format!("{rev}^{{commit}}");
    let out = git_cmd(&["rev-parse", "--verify", "--quiet", &commit], repo).await?;
    match out.status.code() {
        Some(0) => Ok(Some(
            String::from_utf8_lossy(&out.stdout).trim().to_string(),
        )),
        Some(1) => Ok(None),
        _ => Err(format!(
            "git rev-parse {rev} in {} failed: {}",
            repo.display(),
            String::from_utf8_lossy(&out.stderr).trim()
        )
        .into()),
    }
}

async fn has_commits_beyond_main(repo: &Path, branch: &str) -> Result<bool, BoxError> {
    let range = format!("{MAIN}..refs/heads/{branch}");
    let out = run(repo, &["rev-list", "--count", &range]).await?;
    Ok(out.trim() != "0")
}

async fn run(repo: &Path, args: &[&str]) -> Result<String, BoxError> {
    let out = git_cmd(args, repo).await?;
    if !out.status.success() {
        return Err(format!(
            "git {} in {} failed: {}",
            args.first().copied().unwrap_or_default(),
            repo.display(),
            String::from_utf8_lossy(&out.stderr).trim()
        )
        .into());
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Fetch one branch from the bundle without `--force`, so git refuses
/// anything but creating it or fast-forwarding it.
async fn fetch_branch(repo: &Path, bundle: &Path, branch: &str) -> Result<(), String> {
    let refspec = format!("refs/heads/{branch}:refs/heads/{branch}");
    let bundle = bundle.to_string_lossy();
    let out = git_cmd(&["fetch", "--no-tags", bundle.as_ref(), &refspec], repo).await?;
    if out.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

fn lacks_prerequisites(stderr: &str) -> bool {
    stderr.contains("prerequisite")
}

async fn fetch_origin(repo: &Path) -> Result<(), String> {
    let out = git_cmd_env_timeout(
        &["fetch", "--no-tags", "origin"],
        repo,
        &[],
        ORIGIN_FETCH_TIMEOUT,
    )
    .await?;
    if out.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

#[cfg(test)]
#[path = "pending_bundles_tests.rs"]
pub(crate) mod tests;
