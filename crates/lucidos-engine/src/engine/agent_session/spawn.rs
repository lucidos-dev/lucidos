use crate::engine::git_ops::{git_answer, GitAnswer};
use crate::engine::LucidosEngine;
use crate::runtime::{CodingAgent, RunningAgent, SpawnArgs};
use std::path::Path;
use tokio_util::sync::CancellationToken;

/// Try to resume a coding-agent session, falling back to fresh spawn on failure.
/// Routes through the engine's `agent_runtimes` registry — caller picks the agent kind.
/// `args.resume_session_id` controls whether resume is attempted at all.
pub(super) async fn spawn_or_resume(
    engine: &LucidosEngine,
    agent: CodingAgent,
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
                    "[AgentSession] Resume failed ({}), falling back to fresh spawn",
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
    let answer = git_answer(&["rev-parse", "--verify", rb], repo_root).await;
    decide_branch_resolution(answer, rb, resume_session_id)
}

/// Pure half of [`resolve_branch_for_resume`]: given git's answer about the
/// recorded branch, decide whether to reuse it or start over.
///
/// `Unknown` (git timed out or could not run) reuses the branch and keeps the
/// session id, because the two directions fail very differently. Reusing a
/// branch that turns out to be gone fails LOUDLY and recoverably: the following
/// `git worktree add` errors, the spawn surfaces "resend the message to retry",
/// and nothing is destroyed. Starting fresh when the branch was there all along
/// silently severs the thread from its work: the session id is dropped (so
/// conversation continuity is lost), and the spawn then treats the thread's live
/// worktree as a stale artifact of somebody else's branch, which is exactly the
/// route that emptied a worktree on 2026-08-03 while the host was saturated and
/// every `rev-parse` was timing out.
pub(super) fn decide_branch_resolution(
    branch_exists: GitAnswer,
    resume_branch: &str,
    resume_session_id: Option<String>,
) -> BranchResolution {
    if branch_exists.is_unknown() {
        log!(
            "[AgentSession] Could not verify resume branch {} (git gave no answer). Reusing it rather than starting fresh, since dropping a branch that exists loses the session",
            resume_branch
        );
    }
    if branch_exists.or_unknown(true) {
        if !branch_exists.is_unknown() {
            log!(
                "[AgentSession] Reusing existing branch {} for resumed session",
                resume_branch
            );
        }
        BranchResolution {
            branch_name: resume_branch.to_string(),
            reusing_branch: true,
            resume_session_id,
        }
    } else {
        log!(
            "[AgentSession] Resume branch {} no longer exists, creating fresh branch (dropping stale session id)",
            resume_branch
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
    use crate::engine::git_ops::git_cmd;

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

    /// The 2026-08-03 regression. Under e2e load every `rev-parse` was blowing
    /// the 30s ceiling, the old `.unwrap_or(false)` read that as "branch gone",
    /// and the fresh-branch path that follows is what let the spawn treat a live
    /// worktree as somebody else's leftover and remove it. An unanswered probe
    /// must leave both the branch and the session id alone.
    #[test]
    fn unverifiable_resume_branch_is_reused_not_replaced() {
        let resolution = decide_branch_resolution(
            GitAnswer::Unknown,
            "claude-code/live-session",
            Some("good-sid".into()),
        );
        assert!(
            resolution.reusing_branch,
            "an unanswered rev-parse must not be read as a deleted branch"
        );
        assert_eq!(resolution.branch_name, "claude-code/live-session");
        assert_eq!(
            resolution.resume_session_id,
            Some("good-sid".into()),
            "keeping the branch is pointless if the session id is dropped with it"
        );
    }

    /// A real answer still decides, in both directions.
    #[test]
    fn answered_probes_still_decide_branch_reuse() {
        let reused =
            decide_branch_resolution(GitAnswer::Yes, "claude-code/there", Some("sid".into()));
        assert!(reused.reusing_branch);
        assert_eq!(reused.resume_session_id, Some("sid".into()));

        let fresh =
            decide_branch_resolution(GitAnswer::No, "claude-code/pruned", Some("sid".into()));
        assert!(!fresh.reusing_branch);
        assert_ne!(fresh.branch_name, "claude-code/pruned");
        assert_eq!(fresh.resume_session_id, None);
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
