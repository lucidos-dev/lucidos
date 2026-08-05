use crate::engine::git_ops::{allocate_coding_agent_branch, git_answer, BranchScope, GitAnswer};
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

/// Everything needed to mint a branch name for a thread that doesn't have one
/// yet: `lucidos-<agent>-<app|repo>-<name>-<slug>`. See
/// `git_ops::branch_name` for the shape and the duplicate numbering.
pub(super) struct FreshBranch<'a> {
    pub(super) repo_root: &'a Path,
    pub(super) agent: CodingAgent,
    pub(super) scope: BranchScope,
    /// The thread's display name (title, else first message), slugified into
    /// the branch. Empty is fine: the allocator falls back to the thread id.
    pub(super) thread_name: &'a str,
    pub(super) thread_id: uuid::Uuid,
}

impl FreshBranch<'_> {
    async fn resolution(&self) -> BranchResolution {
        BranchResolution {
            branch_name: allocate_coding_agent_branch(
                self.repo_root,
                self.agent,
                &self.scope,
                self.thread_name,
                self.thread_id,
            )
            .await,
            reusing_branch: false,
            resume_session_id: None,
        }
    }
}

/// Result of resolving which branch a CC turn should run on.
pub(super) struct BranchResolution {
    pub branch_name: String,
    pub reusing_branch: bool,
    /// May be `None` even if the caller passed `Some` — see `resolve_branch_for_resume`.
    pub resume_session_id: Option<String>,
}

/// The pure half's verdict, before any name is minted. Separate from
/// [`BranchResolution`] so allocating a fresh name (which costs a git call)
/// happens only on the path that actually needs one.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum BranchDecision {
    Reuse {
        branch_name: String,
        resume_session_id: Option<String>,
    },
    Fresh,
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
    fresh: &FreshBranch<'_>,
    resume_session_id: Option<String>,
    resume_branch: Option<&str>,
) -> BranchResolution {
    let Some(rb) = resume_branch else {
        return fresh.resolution().await;
    };
    let answer = git_answer(&["rev-parse", "--verify", rb], fresh.repo_root).await;
    match decide_branch_resolution(answer, rb, resume_session_id) {
        BranchDecision::Reuse {
            branch_name,
            resume_session_id,
        } => BranchResolution {
            branch_name,
            reusing_branch: true,
            resume_session_id,
        },
        BranchDecision::Fresh => fresh.resolution().await,
    }
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
) -> BranchDecision {
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
        BranchDecision::Reuse {
            branch_name: resume_branch.to_string(),
            resume_session_id,
        }
    } else {
        log!(
            "[AgentSession] Resume branch {} no longer exists, creating fresh branch (dropping stale session id)",
            resume_branch
        );
        BranchDecision::Fresh
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::git_ops::{git_cmd, LUCIDOS_BRANCH_PREFIX};

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

    fn fresh_branch(repo: &std::path::Path) -> FreshBranch<'_> {
        FreshBranch {
            repo_root: repo,
            agent: CodingAgent::ClaudeCode,
            scope: BranchScope::Repo(BranchScope::LUCIDOS_REPO.to_string()),
            thread_name: "fix the auth timeout",
            thread_id: uuid::Uuid::new_v4(),
        }
    }

    #[tokio::test]
    async fn missing_resume_branch_drops_stale_session_id() {
        let (_tmp, repo) = make_test_repo().await;
        let resolution = resolve_branch_for_resume(
            &fresh_branch(&repo),
            Some("e4a3d60a-ea4d-4592-b252-0558f8798cf3".into()),
            Some("claude-code/already-merged-and-pruned"),
        )
        .await;
        assert!(
            !resolution.reusing_branch,
            "must not reuse a branch that doesn't exist"
        );
        assert_eq!(
            resolution.branch_name, "lucidos-claude-code-repo-lucidos-fix-the-auth-timeout",
            "must mint a fresh thread-named branch"
        );
        assert_eq!(
            resolution.resume_session_id, None,
            "stale session_id must be dropped, otherwise CC dies with 'No conversation found'"
        );
    }

    /// A legacy `claude-code/*` branch is never renamed: a resumed thread that
    /// predates the thread-named scheme keeps the branch holding its commits.
    #[tokio::test]
    async fn existing_legacy_resume_branch_keeps_its_name_and_session_id() {
        let (_tmp, repo) = make_test_repo().await;
        let _ = git_cmd(&["branch", "claude-code/active-session"], &repo).await;
        let resolution = resolve_branch_for_resume(
            &fresh_branch(&repo),
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
        let decision = decide_branch_resolution(
            GitAnswer::Unknown,
            "lucidos-claude-code-repo-lucidos-live-session",
            Some("good-sid".into()),
        );
        assert_eq!(
            decision,
            BranchDecision::Reuse {
                branch_name: "lucidos-claude-code-repo-lucidos-live-session".into(),
                resume_session_id: Some("good-sid".into()),
            },
            "an unanswered rev-parse must not be read as a deleted branch, and keeping \
             the branch is pointless if the session id is dropped with it"
        );
    }

    /// A real answer still decides, in both directions.
    #[test]
    fn answered_probes_still_decide_branch_reuse() {
        assert_eq!(
            decide_branch_resolution(
                GitAnswer::Yes,
                "lucidos-codex-repo-lucidos-there",
                Some("sid".into())
            ),
            BranchDecision::Reuse {
                branch_name: "lucidos-codex-repo-lucidos-there".into(),
                resume_session_id: Some("sid".into()),
            }
        );
        assert_eq!(
            decide_branch_resolution(GitAnswer::No, "claude-code/pruned", Some("sid".into())),
            BranchDecision::Fresh,
            "a pruned branch starts over, and Fresh carries no session id by construction"
        );
    }

    #[tokio::test]
    async fn no_resume_branch_generates_fresh_with_no_session_id() {
        let (_tmp, repo) = make_test_repo().await;
        let resolution =
            resolve_branch_for_resume(&fresh_branch(&repo), Some("ignored-sid".into()), None).await;
        assert!(!resolution.reusing_branch);
        assert!(resolution.branch_name.starts_with(LUCIDOS_BRANCH_PREFIX));
        assert_eq!(
            resolution.resume_session_id, None,
            "without a resume branch, the session_id has no anchor and must be dropped"
        );
    }

    /// Two threads with the same title in the same repo must not collide: the
    /// second takes `-2`. Exercised end to end (through the ref listing) rather
    /// than only against the pure allocator.
    #[tokio::test]
    async fn a_second_thread_with_the_same_name_is_numbered() {
        let (_tmp, repo) = make_test_repo().await;
        let first = resolve_branch_for_resume(&fresh_branch(&repo), None, None).await;
        assert_eq!(
            first.branch_name,
            "lucidos-claude-code-repo-lucidos-fix-the-auth-timeout"
        );
        let _ = git_cmd(&["branch", &first.branch_name], &repo).await;

        let second = resolve_branch_for_resume(&fresh_branch(&repo), None, None).await;
        assert_eq!(
            second.branch_name,
            "lucidos-claude-code-repo-lucidos-fix-the-auth-timeout-2"
        );
    }

    /// Every minted name has to survive git's own ref validation, whatever the
    /// thread was called.
    #[tokio::test]
    async fn minted_names_are_valid_git_refs() {
        let (_tmp, repo) = make_test_repo().await;
        let names = [
            "fix the auth timeout",
            "🎉🎉🎉",
            "!!!",
            "日本語のスレッド",
            "Refs/../../etc/passwd",
            "a name ending in .lock",
            &"very long thread name that keeps going ".repeat(12),
        ];
        for name in names {
            let mut fresh = fresh_branch(&repo);
            fresh.thread_name = name;
            let resolution = resolve_branch_for_resume(&fresh, None, None).await;
            let out = git_cmd(
                &["check-ref-format", "--branch", &resolution.branch_name],
                &repo,
            )
            .await
            .unwrap();
            assert!(
                out.status.success(),
                "git rejected {:?} minted from {:?}",
                resolution.branch_name,
                name
            );
        }
    }
}
