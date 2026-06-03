use std::path::Path;
use std::time::Duration;

mod commits;
mod harden_marker;
mod merge;
mod restart_detection;
mod worktree;

pub(crate) use commits::*;
pub(crate) use harden_marker::*;
pub(crate) use merge::*;
pub(crate) use restart_detection::*;
pub(crate) use worktree::*;

/// Run a git command with a 30-second timeout. Always prepends
/// `-c core.quotepath=false` so non-ASCII paths come back as raw UTF-8 instead
/// of git's default `"...\NNN..."` form — every caller treats output as a path.
pub(crate) async fn git_cmd(args: &[&str], dir: &Path) -> Result<std::process::Output, String> {
    let mut full_args: Vec<&str> = Vec::with_capacity(args.len() + 2);
    full_args.push("-c");
    full_args.push("core.quotepath=false");
    full_args.extend_from_slice(args);
    match tokio::time::timeout(
        Duration::from_secs(30),
        tokio::process::Command::new("git")
            .args(&full_args)
            .current_dir(dir)
            .output(),
    )
    .await
    {
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
#[path = "../git_ops_tests/branch_queries.rs"]
mod branch_queries_tests;

#[cfg(test)]
#[path = "../git_ops_tests/recover_exclude.rs"]
mod recover_exclude_tests;
