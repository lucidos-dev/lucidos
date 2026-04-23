use crate::engine::git_ops::git_cmd;
use crate::engine::CognosEngine;
use crate::runtime::{AgentKind, RunningAgent, SpawnArgs};
use std::path::Path;
use tokio_util::sync::CancellationToken;

/// Try to resume a coding-agent session, falling back to fresh spawn on failure.
/// Routes through the engine's `agent_runtimes` registry — caller picks the agent kind.
/// `args.resume_session_id` controls whether resume is attempted at all.
pub(super) async fn spawn_or_resume(
    engine: &CognosEngine,
    agent: AgentKind,
    args: SpawnArgs<'_>,
    cancel: CancellationToken,
) -> Result<RunningAgent, Box<dyn std::error::Error + Send + Sync>> {
    let runtime = engine
        .agent_runtimes
        .get(&agent)
        .ok_or_else(|| format!("No registered runtime for agent {:?}", agent))?;
    if args.resume_session_id.is_some() {
        let mut fresh = args.clone();
        match runtime.spawn(args, cancel.clone()).await {
            Ok(rt) => return Ok(rt),
            Err(e) => {
                log!(
                    "[ClaudeCode] Resume failed ({}), falling back to fresh spawn",
                    e
                );
            }
        }
        fresh.resume_session_id = None;
        return runtime.spawn(fresh, cancel).await;
    }
    runtime.spawn(args, cancel).await
}

pub(crate) fn generate_cc_branch_name() -> String {
    let ts = chrono::Utc::now().format("%Y%m%d-%H%M%S");
    let suffix = &uuid::Uuid::new_v4().as_simple().to_string()[..6];
    format!("claude-code/{}-{}", ts, suffix)
}

/// Result of resolving which branch a CC turn should run on.
pub(super) struct BranchResolution {
    pub branch_name: String,
    pub reusing_branch: bool,
    /// May be `None` even if the caller passed `Some` — see `resolve_branch_for_resume`.
    pub resume_session_id: Option<String>,
}

/// Decide the git branch for this CC turn and validate the resume context against it.
///
/// When `resume_branch` is set but the branch no longer exists in the repo (e.g. the
/// previous change was applied and a later cleanup pruned the branch), generate a
/// fresh branch AND drop `resume_session_id`: spawning CC with `--resume <stale-uuid>`
/// against a fresh worktree makes CC exit immediately with
/// `No conversation found with session ID: ...`, which the safety net then turns into
/// a user-visible "Aborted" badge.
pub(super) async fn resolve_branch_for_resume(
    repo_root: &Path,
    resume_session_id: Option<String>,
    resume_branch: Option<&str>,
) -> BranchResolution {
    let Some(rb) = resume_branch else {
        return BranchResolution {
            branch_name: generate_cc_branch_name(),
            reusing_branch: false,
            resume_session_id: None,
        };
    };
    let exists = git_cmd(&["rev-parse", "--verify", rb], repo_root)
        .await
        .map(|o| o.status.success())
        .unwrap_or(false);
    if exists {
        log!(
            "[ClaudeCode] Reusing existing branch {} for resumed session",
            rb
        );
        BranchResolution {
            branch_name: rb.to_string(),
            reusing_branch: true,
            resume_session_id,
        }
    } else {
        log!(
            "[ClaudeCode] Resume branch {} no longer exists — creating fresh branch (dropping stale session id)",
            rb
        );
        BranchResolution {
            branch_name: generate_cc_branch_name(),
            reusing_branch: false,
            resume_session_id: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn make_test_repo() -> (tempfile::TempDir, std::path::PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().to_path_buf();
        let _ = git_cmd(&["init"], &repo).await;
        let _ = git_cmd(&["checkout", "-b", "main"], &repo).await;
        tokio::fs::write(repo.join("init.txt"), "initial")
            .await
            .unwrap();
        let _ = git_cmd(&["add", "."], &repo).await;
        let _ = git_cmd(&["commit", "-m", "initial commit"], &repo).await;
        (tmp, repo)
    }

    #[tokio::test]
    async fn missing_resume_branch_drops_stale_session_id() {
        let (_tmp, repo) = make_test_repo().await;
        let resolution = resolve_branch_for_resume(
            &repo,
            Some("e4a3d60a-ea4d-4592-b252-0558f8798cf3".into()),
            Some("claude-code/already-merged-and-pruned"),
        )
        .await;
        assert!(
            !resolution.reusing_branch,
            "must not reuse a branch that doesn't exist"
        );
        assert!(
            resolution.branch_name.starts_with("claude-code/"),
            "must generate a fresh CC branch name, got {}",
            resolution.branch_name
        );
        assert_ne!(
            resolution.branch_name, "claude-code/already-merged-and-pruned",
            "fresh branch must differ from the missing one"
        );
        assert_eq!(
            resolution.resume_session_id, None,
            "stale session_id must be dropped — otherwise CC dies with 'No conversation found'"
        );
    }

    #[tokio::test]
    async fn existing_resume_branch_keeps_session_id() {
        let (_tmp, repo) = make_test_repo().await;
        let _ = git_cmd(&["branch", "claude-code/active-session"], &repo).await;
        let resolution = resolve_branch_for_resume(
            &repo,
            Some("good-sid".into()),
            Some("claude-code/active-session"),
        )
        .await;
        assert!(resolution.reusing_branch);
        assert_eq!(resolution.branch_name, "claude-code/active-session");
        assert_eq!(resolution.resume_session_id, Some("good-sid".into()));
    }

    #[tokio::test]
    async fn no_resume_branch_generates_fresh_with_no_session_id() {
        let (_tmp, repo) = make_test_repo().await;
        let resolution = resolve_branch_for_resume(&repo, Some("ignored-sid".into()), None).await;
        assert!(!resolution.reusing_branch);
        assert!(resolution.branch_name.starts_with("claude-code/"));
        assert_eq!(
            resolution.resume_session_id, None,
            "without a resume branch, the session_id has no anchor and must be dropped"
        );
    }

    #[test]
    fn cc_branch_names_are_unique_when_generated_simultaneously() {
        let names: Vec<String> = (0..10).map(|_| generate_cc_branch_name()).collect();
        let unique: std::collections::HashSet<&String> = names.iter().collect();
        assert_eq!(
            names.len(),
            unique.len(),
            "branch names must be unique: {:?}",
            names
        );
    }

    #[test]
    fn cc_branch_name_has_expected_format() {
        let name = generate_cc_branch_name();
        assert!(
            name.starts_with("claude-code/"),
            "must start with claude-code/: {}",
            name
        );
        let parts: Vec<&str> = name
            .strip_prefix("claude-code/")
            .unwrap()
            .splitn(3, '-')
            .collect();
        assert_eq!(parts.len(), 3, "expected 3 parts after prefix: {:?}", parts);
        assert_eq!(
            parts[0].len(),
            8,
            "date part should be 8 chars: {}",
            parts[0]
        );
        assert_eq!(
            parts[1].len(),
            6,
            "time part should be 6 chars: {}",
            parts[1]
        );
        assert_eq!(parts[2].len(), 6, "suffix should be 6 chars: {}", parts[2]);
    }
}
