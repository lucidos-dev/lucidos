use std::ffi::OsStr;
use std::path::Path;
use std::time::Duration;

mod checkpoint;
mod commits;
mod harden_marker;
mod merge;
mod plan_marker;
mod restart_detection;
mod worktree;

pub(crate) use checkpoint::*;
pub(crate) use commits::*;
pub(crate) use harden_marker::*;
pub(crate) use merge::*;
pub(crate) use plan_marker::*;
pub(crate) use restart_detection::*;
pub(crate) use worktree::*;

/// Run a git command with a 30-second timeout. Always prepends
/// `-c core.quotepath=false` so non-ASCII paths come back as raw UTF-8 instead
/// of git's default `"...\NNN..."` form — every caller treats output as a path.
pub(crate) async fn git_cmd(args: &[&str], dir: &Path) -> Result<std::process::Output, String> {
    git_cmd_env(args, dir, &[]).await
}

/// Like [`git_cmd`] but with extra environment variables. Used by the command
/// checkpoint helpers, which set `GIT_INDEX_FILE` to a throwaway index so a
/// snapshot/restore never disturbs the repo's real index or working tree.
pub(crate) async fn git_cmd_env(
    args: &[&str],
    dir: &Path,
    envs: &[(&str, &OsStr)],
) -> Result<std::process::Output, String> {
    let mut full_args: Vec<&str> = Vec::with_capacity(args.len() + 2);
    full_args.push("-c");
    full_args.push("core.quotepath=false");
    full_args.extend_from_slice(args);
    let mut cmd = tokio::process::Command::new("git");
    cmd.args(&full_args).current_dir(dir);
    for (key, value) in envs {
        cmd.env(key, value);
    }
    match tokio::time::timeout(Duration::from_secs(30), cmd.output()).await {
        Ok(Ok(output)) => Ok(output),
        Ok(Err(e)) => Err(format!("git {} failed: {}", args.join(" "), e)),
        Err(_) => Err(format!("git {} timed out after 30s", args.join(" "))),
    }
}

#[cfg(test)]
#[path = "../git_ops_tests/common.rs"]
mod common;

#[cfg(test)]
#[path = "../git_ops_tests/app_worktree.rs"]
mod app_worktree_tests;

#[cfg(test)]
#[path = "../git_ops_tests/merge.rs"]
mod merge_tests;

#[cfg(test)]
#[path = "../git_ops_tests/harden_marker.rs"]
mod harden_marker_tests;

#[cfg(test)]
#[path = "../git_ops_tests/plan_marker.rs"]
mod plan_marker_tests;

#[cfg(test)]
#[path = "../git_ops_tests/branch_queries.rs"]
mod branch_queries_tests;

#[cfg(test)]
#[path = "../git_ops_tests/recover_exclude.rs"]
mod recover_exclude_tests;

#[cfg(test)]
#[path = "../git_ops_tests/spawn_cleanup.rs"]
mod spawn_cleanup_tests;

#[cfg(test)]
#[path = "../git_ops_tests/worktree_validity.rs"]
mod worktree_validity_tests;
