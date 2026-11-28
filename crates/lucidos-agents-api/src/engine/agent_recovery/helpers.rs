use super::super::git_ops::{git_answer, git_cmd, GitAnswer};
pub(crate) use crate::engine::coding_turn_end::{last_turn_end, last_turn_ended_cleanly, TurnEnd};
pub use crate::engine::continuation_reason::*;
use crate::engine::thread_events::CodingAgentKind;
use std::path::{Path, PathBuf};
use uuid::Uuid;

pub const CONTINUE_RESUME_USER_MESSAGE: &str = "Continue from where you left off.";

const ANSWER_OUT_OF_BAND_NOTE: &str =
    "[Note from engine: the user answered your question, but your session was no longer \
     running when they did, so their answer could not come back to you as the tool result. \
     Your transcript may show that question tool call as rejected, denied or interrupted, \
     possibly with a line claiming the user did not want to proceed. That line is an engine \
     teardown artifact and it is false: the user declined nothing. They did not approve \
     anything either. Their actual answer follows.]";

const ANSWER_OUT_OF_BAND_TRAILER: &str =
    "Continue the turn from that answer, and from nothing else. Do not re-ask the same \
     question unless the answer left it open. Do not read the interrupted tool call as \
     approval, as a refusal, or as permission to carry on with what you were doing before \
     you asked.";

pub(crate) async fn continue_input_for_reason(
    pool: &sqlx::PgPool,
    workspace: &std::path::Path,
    thread_id: Uuid,
    reason: Option<&str>,
) -> String {
    match reason {
        Some(HARDEN_REQUESTED_REASON) => {
            return crate::engine::continuation_reason::AUTO_HARDEN_MESSAGE.to_string();
        }
        Some(PLAN_REQUESTED_REASON) => {
            if let Some(hold) = crate::engine::change_ops::held_plan(pool, thread_id).await {
                return hold.agent_nudge().to_string();
            }
        }
        Some(BUILD_REQUESTED_REASON) => {
            return crate::engine::approved_plan::APPROVED_PLAN_REENTRY_TEXT.to_string();
        }
        Some(HANDOFF_REQUESTED_REASON) => {
            if let Ok(Some(crossing)) =
                crate::engine::context_ceiling::past_ceiling(pool, thread_id).await
            {
                return crate::engine::context_ceiling::ceiling_reentry_prompt(crossing);
            }
        }
        Some(REPORTS_REQUESTED_REASON) => return REPORTS_REQUESTED_MESSAGE.to_string(),
        _ => {}
    }
    if reason != Some(ANSWERED_AFTER_IDLE_REASON) {
        return CONTINUE_RESUME_USER_MESSAGE.to_string();
    }
    match crate::engine::agent_question::answered_question_recap(pool, workspace, thread_id).await {
        Some(recap) => {
            format!("{ANSWER_OUT_OF_BAND_NOTE}\n\n{recap}\n{ANSWER_OUT_OF_BAND_TRAILER}")
        }
        None => {
            crate::log_legacy!(
                "[CCQuestion] answered_after_idle continuation for thread {} has no \
                 recoverable answer, resuming with the bare continue message",
                thread_id
            );
            CONTINUE_RESUME_USER_MESSAGE.to_string()
        }
    }
}

pub(crate) async fn continue_retry_input(
    pool: &sqlx::PgPool,
    workspace: &std::path::Path,
    thread_id: Uuid,
    reason: Option<&str>,
) -> String {
    let tail = continue_input_for_reason(pool, workspace, thread_id, reason).await;
    crate::engine::successor::resume_failed_input(pool, thread_id, &tail).await
}

pub(crate) async fn cleanup_stale_worktree(wt_path: &Path) {
    if crate::engine::worktree_cleanup::is_worktree_dirty(wt_path).await {
        log_legacy!(
            "[Recovery] Keeping duplicate worktree {}: it has uncommitted changes",
            wt_path.display()
        );
        return;
    }
    match crate::engine::worktree_cleanup::remove_worktree_and_optionally_delete_branch(
        wt_path,
        None,
        crate::engine::worktree_cleanup::BranchDisposal::WhenMerged,
    )
    .await
    {
        Some(outcome) => log_legacy!(
            "[Recovery] Removed duplicate worktree {} ({} bytes, branch_deleted={})",
            wt_path.display(),
            outcome.freed_bytes,
            outcome.branch_deleted
        ),
        None => log_legacy!(
            "[Recovery] Could not remove duplicate worktree {}: it is skipped until the next restart",
            wt_path.display()
        ),
    }
}

pub(crate) fn orphan_recovery_target(
    branch_to_thread: &std::collections::HashMap<String, Uuid>,
    branch_name: &str,
) -> Option<Uuid> {
    branch_to_thread.get(branch_name).copied()
}

pub(crate) fn recovery_repo_roots(
    lucidos_repo_root: &Path,
    workspace_root: &Path,
    external_repos: &[crate::core::repositories::Repository],
) -> Vec<(PathBuf, Option<String>)> {
    let canon = |p: &Path| p.canonicalize().unwrap_or_else(|_| p.to_path_buf());

    let mut seen: std::collections::HashSet<PathBuf> =
        std::collections::HashSet::from([canon(lucidos_repo_root)]);
    let mut roots: Vec<(PathBuf, Option<String>)> = vec![(lucidos_repo_root.to_path_buf(), None)];

    if seen.insert(canon(workspace_root)) {
        roots.push((workspace_root.to_path_buf(), None));
    }

    for repo in external_repos {
        let path = PathBuf::from(&repo.path);
        if !path.exists() {
            log_legacy!(
                "[Recovery] Skipping external repo '{}': path does not exist: {}",
                repo.name,
                repo.path
            );
            continue;
        }
        if !seen.insert(canon(&path)) {
            log_legacy!(
                "[Recovery] Skipping external repo '{}': same path as an already-scanned repo",
                repo.name
            );
            continue;
        }
        roots.push((path, Some(repo.id.to_string())));
    }

    roots
}

pub(crate) const RECOVERY_GIT_CONCURRENCY: usize = 8;

/// INVARIANT: The first root, in `roots` order, whose refs hold `refs/heads/<branch>`.
/// Every root is asked at once, but the pick is the one a serial scan would
/// make. That keeps the load-bearing order of [`recovery_repo_roots`].
pub(crate) async fn first_root_holding_branch<'a>(
    roots: &'a [(PathBuf, Option<String>)],
    branch: &str,
) -> Option<&'a (PathBuf, Option<String>)> {
    use futures::StreamExt;
    let refname = format!("refs/heads/{branch}");
    let answers: Vec<GitAnswer> = futures::stream::iter(roots.iter().map(|(root, _)| {
        let refname = refname.as_str();
        async move { git_answer(&["rev-parse", "--verify", refname], root).await }
    }))
    .buffered(RECOVERY_GIT_CONCURRENCY)
    .collect()
    .await;
    first_confirmed(roots, &answers)
}

fn first_confirmed<'a, T>(items: &'a [T], answers: &[GitAnswer]) -> Option<&'a T> {
    items
        .iter()
        .zip(answers)
        .find(|(_, answer)| answer.or_unknown(false))
        .map(|(item, _)| item)
}

pub(crate) async fn lookup_app_spawn_id(pool: &sqlx::PgPool, thread_id: Uuid) -> Option<String> {
    let row: Option<(Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT payload->>'coding_agent_kind', payload->>'app_id' FROM events \
         WHERE event_type = 'SessionStarted' AND thread_id = $1 \
         ORDER BY sequence DESC LIMIT 1",
    )
    .bind(thread_id)
    .fetch_optional(pool)
    .await
    .unwrap_or_else(|e| {
        log_legacy!(
            "[Recovery] app-id lookup for thread {} failed: {}. \
             A lost worktree would be rebuilt in full rather than sparse",
            thread_id,
            e
        );
        None
    });
    match row {
        Some((Some(kind), Some(app_id))) if kind == "app" && !app_id.is_empty() => Some(app_id),
        _ => None,
    }
}

pub(crate) async fn recreate_lost_worktree(
    repo_root: &Path,
    branch: &str,
    wt_path: &Path,
    app_spawn_id: Option<&str>,
) -> Result<(), String> {
    match app_spawn_id {
        Some(app_id) => {
            crate::engine::git_ops::create_sparse_app_worktree(repo_root, app_id, branch, wt_path)
                .await
                .map(|_reused_or_created| ())
        }
        None => match crate::engine::git_ops::worktree_add(repo_root, wt_path, &[branch]).await {
            Ok(o) if o.status.success() => Ok(()),
            Ok(o) => Err(String::from_utf8_lossy(&o.stderr).trim().to_string()),
            Err(e) => Err(e),
        },
    }
}

pub(crate) fn recovery_branch_is_external_repo(
    repo_root: &Path,
    lucidos_repo_root: &Path,
    workspace_root: &Path,
) -> bool {
    crate::engine::git_ops::is_external_repo_path(repo_root, lucidos_repo_root)
        && crate::engine::git_ops::is_external_repo_path(repo_root, workspace_root)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum StaleSessionRepo {
    Owned(PathBuf),
    External,
}

pub(crate) fn stale_session_repo(
    kind: Option<&str>,
    projection_says_external: bool,
    lucidos_repo_root: &Path,
    workspace_root: &Path,
) -> StaleSessionRepo {
    let kind = kind.map(CodingAgentKind::parse).unwrap_or_default();
    if projection_says_external || matches!(kind, CodingAgentKind::External) {
        return StaleSessionRepo::External;
    }
    match kind {
        CodingAgentKind::App => StaleSessionRepo::Owned(workspace_root.to_path_buf()),
        _ => StaleSessionRepo::Owned(lucidos_repo_root.to_path_buf()),
    }
}

pub(crate) fn stale_discard_branch_delete_root<'a>(
    discard: bool,
    repo: &'a StaleSessionRepo,
    branch: &str,
) -> Option<&'a Path> {
    if !discard || !crate::engine::git_ops::is_coding_agent_branch(branch) {
        return None;
    }
    match repo {
        StaleSessionRepo::Owned(root) => Some(root.as_path()),
        StaleSessionRepo::External => None,
    }
}

pub(crate) async fn recover_branch_ref_from_worktree(
    workspace_path: &Path,
    thread_id: Uuid,
    branch_name: &str,
) -> Option<(PathBuf, PathBuf)> {
    let wt = crate::engine::session_queries::deterministic_worktree_path(workspace_path, thread_id);
    if !matches!(tokio::fs::try_exists(&wt).await, Ok(true)) {
        return None;
    }
    // INVARIANT: Resolve the worktree's HEAD to a concrete commit. `--verify` makes git
    // fail (non-zero) rather than echo the literal "HEAD" when the symref
    // dangles, so a deleted-branch-but-symref-still-points-at-it worktree
    // (unrecoverable without fsck) falls through to the last-resort path
    // instead of recreating the branch at a bogus value.
    let head_sha = match git_cmd(&["rev-parse", "--verify", "HEAD"], &wt).await {
        Ok(o) if o.status.success() => {
            let sha = String::from_utf8_lossy(&o.stdout).trim().to_string();
            if sha.is_empty() {
                return None;
            }
            sha
        }
        _ => return None,
    };
    let repo_root = crate::engine::worktree_cleanup::resolve_repo_root_from_worktree(&wt).await?;
    match git_cmd(&["branch", branch_name, &head_sha], &repo_root).await {
        Ok(o) if o.status.success() => {
            log_legacy!(
                "[Recovery] Recreated missing branch {} at {} from surviving worktree {} — session resumable",
                branch_name,
                &head_sha[..head_sha.floor_char_boundary(8)],
                wt.display()
            );
            Some((repo_root, wt))
        }
        Ok(o) => {
            log_legacy!(
                "[Recovery] Could not recreate branch {} from worktree {}: {}",
                branch_name,
                wt.display(),
                String::from_utf8_lossy(&o.stderr).trim()
            );
            None
        }
        Err(e) => {
            log_legacy!(
                "[Recovery] git branch create for {} errored: {}",
                branch_name,
                e
            );
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::git_ops::{git_cmd, worktrees_dir};
    use std::collections::HashMap;

    async fn init_repo(workspace: &Path) {
        git_cmd(&["init", "--initial-branch=main"], workspace)
            .await
            .unwrap();
        git_cmd(&["config", "user.email", "recover@test"], workspace)
            .await
            .unwrap();
        git_cmd(&["config", "user.name", "Recover Test"], workspace)
            .await
            .unwrap();
        tokio::fs::write(workspace.join("seed.txt"), "seed")
            .await
            .unwrap();
        git_cmd(&["add", "."], workspace).await.unwrap();
        git_cmd(&["commit", "-m", "seed"], workspace).await.unwrap();
        let _ = worktrees_dir(workspace);
    }

    async fn rev_parse(repo: &Path, rev: &str) -> Option<String> {
        let o = git_cmd(&["rev-parse", "--verify", rev], repo).await.ok()?;
        o.status
            .success()
            .then(|| String::from_utf8_lossy(&o.stdout).trim().to_string())
    }

    #[tokio::test]
    async fn recover_branch_ref_recreates_missing_branch_from_surviving_worktree() {
        let tmp = tempfile::tempdir().unwrap();
        let workspace = tmp.path().to_path_buf();
        init_repo(&workspace).await;

        let thread_id = Uuid::new_v4();
        let branch = "claude-code/recover-test";
        let wt = crate::engine::session_queries::deterministic_worktree_path(&workspace, thread_id);
        git_cmd(
            &[
                "worktree",
                "add",
                "-b",
                branch,
                &wt.to_string_lossy(),
                "main",
            ],
            &workspace,
        )
        .await
        .unwrap();
        let head_before = rev_parse(&wt, "HEAD")
            .await
            .expect("worktree HEAD resolves");

        git_cmd(&["checkout", "--detach"], &wt).await.unwrap();
        git_cmd(&["branch", "-D", branch], &workspace)
            .await
            .unwrap();
        assert!(
            rev_parse(&workspace, &format!("refs/heads/{}", branch))
                .await
                .is_none(),
            "precondition: branch ref must be gone"
        );

        let recovered = recover_branch_ref_from_worktree(&workspace, thread_id, branch).await;
        let (repo_root, wt_path) =
            recovered.expect("must recover the branch from the surviving worktree");
        assert_eq!(wt_path, wt, "returns the recovered worktree path");
        let canon = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
        assert_eq!(
            canon(&repo_root),
            canon(&workspace),
            "resolves the worktree's repo root"
        );

        let head_after = rev_parse(&workspace, &format!("refs/heads/{}", branch))
            .await
            .expect("branch ref must be recreated");
        assert_eq!(
            head_after, head_before,
            "recreated branch must point at the worktree's recorded HEAD, not a guessed commit"
        );
    }

    #[tokio::test]
    async fn recover_branch_ref_returns_none_when_worktree_absent() {
        let tmp = tempfile::tempdir().unwrap();
        let workspace = tmp.path().to_path_buf();
        init_repo(&workspace).await;
        let _ = worktrees_dir(&workspace);

        let recovered =
            recover_branch_ref_from_worktree(&workspace, Uuid::new_v4(), "claude-code/missing")
                .await;
        assert!(
            recovered.is_none(),
            "absent worktree must not recover — caller ends the stuck session"
        );
    }

    #[tokio::test]
    async fn recover_branch_ref_returns_none_on_dangling_head() {
        let tmp = tempfile::tempdir().unwrap();
        let workspace = tmp.path().to_path_buf();
        init_repo(&workspace).await;

        let thread_id = Uuid::new_v4();
        let branch = "claude-code/dangling-test";
        let wt = crate::engine::session_queries::deterministic_worktree_path(&workspace, thread_id);
        git_cmd(
            &[
                "worktree",
                "add",
                "-b",
                branch,
                &wt.to_string_lossy(),
                "main",
            ],
            &workspace,
        )
        .await
        .unwrap();
        let ref_path = workspace
            .join(".git/refs/heads")
            .join(branch.strip_prefix("claude-code/").unwrap());
        let _ = tokio::fs::remove_file(workspace.join(format!(".git/refs/heads/{}", branch))).await;
        let _ = tokio::fs::remove_file(&ref_path).await;
        git_cmd(&["pack-refs", "--all"], &workspace).await.ok();
        let _ = tokio::fs::remove_file(workspace.join(format!(".git/refs/heads/{}", branch))).await;

        let recovered = recover_branch_ref_from_worktree(&workspace, thread_id, branch).await;
        assert!(
            recovered.is_none(),
            "dangling HEAD symref is unrecoverable — must NOT guess a SHA"
        );
    }

    fn registered_repo(name: &str, path: &Path) -> crate::core::repositories::Repository {
        crate::core::repositories::Repository {
            id: Uuid::new_v4(),
            name: name.to_string(),
            path: path.to_string_lossy().into_owned(),
            description: None,
            root_commit_sha: None,
            created_at: chrono::Utc::now(),
        }
    }

    fn root_paths(roots: &[(PathBuf, Option<String>)]) -> Vec<PathBuf> {
        roots.iter().map(|(p, _)| p.clone()).collect()
    }

    #[test]
    fn recovery_repo_roots_scans_the_workspace_repo() {
        let lucidos = PathBuf::from("/src/lucidos");
        let workspace = PathBuf::from("/home/u/workspaces/dev");

        let roots = recovery_repo_roots(&lucidos, &workspace, &[]);

        assert_eq!(
            root_paths(&roots),
            vec![lucidos, workspace],
            "the workspace repo must be scanned, after the Lucidos source repo"
        );
        assert!(
            roots.iter().all(|(_, id)| id.is_none()),
            "neither engine-owned repo is in the registry, so neither carries an id"
        );
    }

    #[test]
    fn recovery_repo_roots_puts_registered_repos_last() {
        let tmp = tempfile::tempdir().unwrap();
        let external = tmp.path().join("external-repo");
        std::fs::create_dir_all(&external).unwrap();
        let lucidos = PathBuf::from("/src/lucidos");
        let workspace = PathBuf::from("/home/u/workspaces/dev");
        let repos = vec![registered_repo("example-repo", &external)];

        let roots = recovery_repo_roots(&lucidos, &workspace, &repos);

        assert_eq!(root_paths(&roots), vec![lucidos, workspace, external]);
        assert!(
            roots[2].1.is_some(),
            "a registered repo carries the id its worktree marker records"
        );
    }

    #[test]
    fn first_confirmed_takes_the_earliest_yes_in_scan_order() {
        let roots = ["lucidos", "workspace", "external"];
        let answers = [GitAnswer::No, GitAnswer::Yes, GitAnswer::Yes];
        assert_eq!(first_confirmed(&roots, &answers), Some(&"workspace"));
    }

    #[test]
    fn first_confirmed_never_picks_an_unanswered_root() {
        let roots = ["lucidos", "workspace"];
        let answers = [GitAnswer::Unknown, GitAnswer::Yes];
        assert_eq!(first_confirmed(&roots, &answers), Some(&"workspace"));
        let answers = [GitAnswer::Unknown, GitAnswer::No];
        assert_eq!(first_confirmed(&roots, &answers), None);
    }

    #[tokio::test]
    async fn first_root_holding_branch_matches_a_serial_scan() {
        let tmp = tempfile::tempdir().unwrap();
        let without = tmp.path().join("without");
        let first = tmp.path().join("first");
        let second = tmp.path().join("second");
        for repo in [&without, &first, &second] {
            std::fs::create_dir_all(repo).unwrap();
            init_repo(repo).await;
        }
        let branch = "lucidos-claude-code-repo-example-12345678";
        for repo in [&first, &second] {
            git_cmd(&["branch", branch], repo).await.unwrap();
        }
        let roots = vec![
            (tmp.path().join("missing"), None),
            (without, None),
            (first.clone(), Some("first-id".to_string())),
            (second, Some("second-id".to_string())),
        ];

        let found = first_root_holding_branch(&roots, branch).await;
        assert_eq!(
            found,
            Some(&(first, Some("first-id".to_string()))),
            "the earliest root holding the branch wins"
        );
        assert_eq!(
            first_root_holding_branch(&roots, "no-such-branch").await,
            None
        );
    }

    #[test]
    fn recovery_repo_roots_dedupes_a_workspace_inside_the_lucidos_repo() {
        let tmp = tempfile::tempdir().unwrap();
        let both = tmp.path().to_path_buf();

        let roots = recovery_repo_roots(&both, &both, &[]);

        assert_eq!(root_paths(&roots), vec![both]);
    }

    #[test]
    fn recovery_repo_roots_dedupes_a_workspace_registered_as_an_external_repo() {
        let tmp = tempfile::tempdir().unwrap();
        let workspace = tmp.path().to_path_buf();
        let lucidos = PathBuf::from("/src/lucidos");
        let repos = vec![registered_repo("my-workspace", &workspace)];

        let roots = recovery_repo_roots(&lucidos, &workspace, &repos);

        assert_eq!(root_paths(&roots), vec![lucidos, workspace]);
        assert!(
            roots[1].1.is_none(),
            "the workspace entry wins, so the duplicate registry id is dropped"
        );
    }

    #[test]
    fn recovery_repo_roots_dedupes_a_registered_lucidos_repo() {
        let tmp = tempfile::tempdir().unwrap();
        let lucidos = tmp.path().to_path_buf();
        let workspace = PathBuf::from("/home/u/workspaces/dev");
        let repos = vec![registered_repo("Lucidos", &lucidos)];

        let roots = recovery_repo_roots(&lucidos, &workspace, &repos);

        assert_eq!(root_paths(&roots), vec![lucidos, workspace]);
    }

    #[test]
    fn recovery_repo_roots_skips_a_registered_repo_that_is_gone() {
        let lucidos = PathBuf::from("/src/lucidos");
        let workspace = PathBuf::from("/home/u/workspaces/dev");
        let repos = vec![registered_repo(
            "example-repo",
            Path::new("/nonexistent/example-repo"),
        )];

        let roots = recovery_repo_roots(&lucidos, &workspace, &repos);

        assert_eq!(root_paths(&roots), vec![lucidos, workspace]);
    }

    #[test]
    fn recovery_branch_is_external_repo_excludes_both_engine_owned_roots() {
        let tmp = tempfile::tempdir().unwrap();
        let lucidos = tmp.path().join("lucidos");
        let workspace = tmp.path().join("workspace");
        let external = tmp.path().join("example-repo");
        for p in [&lucidos, &workspace, &external] {
            std::fs::create_dir_all(p).unwrap();
        }

        assert!(
            !recovery_branch_is_external_repo(&lucidos, &lucidos, &workspace),
            "a Lucidos-source branch is not external"
        );
        assert!(
            !recovery_branch_is_external_repo(&workspace, &lucidos, &workspace),
            "an app branch lives in the workspace repo and is not external"
        );
        assert!(
            recovery_branch_is_external_repo(&external, &lucidos, &workspace),
            "a registered repo's branch is external"
        );
    }

    fn lucidos_root() -> PathBuf {
        PathBuf::from("/src/lucidos")
    }

    fn workspace_root() -> PathBuf {
        PathBuf::from("/home/u/workspaces/dev")
    }

    fn repo_for(kind: Option<&str>, projection_says_external: bool) -> StaleSessionRepo {
        stale_session_repo(
            kind,
            projection_says_external,
            &lucidos_root(),
            &workspace_root(),
        )
    }

    #[test]
    fn stale_session_repo_routes_an_app_thread_to_the_workspace_git() {
        assert_eq!(
            repo_for(Some("app"), false),
            StaleSessionRepo::Owned(workspace_root())
        );
    }

    #[test]
    fn stale_session_repo_routes_every_other_kind_to_the_lucidos_repo() {
        for kind in [Some("lucidos"), Some("something-new"), None] {
            assert_eq!(
                repo_for(kind, false),
                StaleSessionRepo::Owned(lucidos_root()),
                "kind {:?} must take the Lucidos repo",
                kind
            );
        }
    }

    #[test]
    fn stale_session_repo_reads_external_from_either_signal() {
        assert_eq!(
            repo_for(Some("external"), false),
            StaleSessionRepo::External
        );
        assert_eq!(repo_for(None, true), StaleSessionRepo::External);
    }

    #[test]
    fn stale_session_repo_resolves_a_contradiction_to_external() {
        assert_eq!(repo_for(Some("app"), true), StaleSessionRepo::External);
    }

    #[test]
    fn stale_discard_deletes_a_branch_only_on_all_three_facts() {
        let owned = StaleSessionRepo::Owned(workspace_root());
        let branch = "lucidos-claude-code-app-habit-tracker-add-streaks-ae6846f4";

        assert_eq!(
            stale_discard_branch_delete_root(true, &owned, branch),
            Some(workspace_root().as_path()),
            "a Discard on an app thread deletes its branch in the workspace git"
        );
        assert_eq!(
            stale_discard_branch_delete_root(false, &owned, branch),
            None,
            "an Apply or Stop deletes nothing: only Discard asks for the work to go"
        );
        assert_eq!(
            stale_discard_branch_delete_root(true, &StaleSessionRepo::External, branch),
            None,
            "an external root was never resolved, so it authorizes no delete"
        );
        assert_eq!(
            stale_discard_branch_delete_root(true, &owned, "main"),
            None,
            "a branch the engine did not create is not this settle's to delete"
        );
    }

    #[tokio::test]
    async fn cleanup_stale_worktree_keeps_a_tree_with_uncommitted_edits() {
        let tmp = tempfile::tempdir().unwrap();
        let workspace = tmp.path().to_path_buf();
        init_repo(&workspace).await;
        let dirty = worktrees_dir(&workspace).join("dup-dirty");
        let clean = worktrees_dir(&workspace).join("dup-clean");
        let unreadable = worktrees_dir(&workspace).join("dup-unreadable");
        for (wt, branch) in [
            (&dirty, "dup-dirty-branch"),
            (&clean, "dup-clean-branch"),
            (&unreadable, "dup-unreadable-branch"),
        ] {
            git_cmd(
                &[
                    "worktree",
                    "add",
                    "-b",
                    branch,
                    &wt.to_string_lossy(),
                    "main",
                ],
                &workspace,
            )
            .await
            .unwrap();
        }
        tokio::fs::write(dirty.join("edit.txt"), "uncommitted")
            .await
            .unwrap();

        tokio::fs::write(
            workspace.join(".git/worktrees/dup-unreadable/index"),
            "not an index",
        )
        .await
        .unwrap();

        cleanup_stale_worktree(&dirty).await;
        cleanup_stale_worktree(&clean).await;
        cleanup_stale_worktree(&unreadable).await;

        assert!(
            dirty.join("edit.txt").exists(),
            "an uncommitted edit must survive the duplicate cleanup"
        );
        assert!(
            unreadable.exists(),
            "a tree git cannot describe counts as dirty and is kept"
        );
        assert!(!clean.exists(), "a clean duplicate worktree is removed");
    }

    async fn workspace_with_lost_app_branch(workspace: &Path, branch: &str) {
        init_repo(workspace).await;
        for dir in [
            "data/apps/habit-tracker",
            "data/apps/other",
            "data/artifacts",
        ] {
            tokio::fs::create_dir_all(workspace.join(dir))
                .await
                .unwrap();
        }
        tokio::fs::write(
            workspace.join("data/apps/habit-tracker/index.html"),
            "<h1>h</h1>",
        )
        .await
        .unwrap();
        tokio::fs::write(workspace.join("data/apps/other/index.html"), "<h1>o</h1>")
            .await
            .unwrap();
        tokio::fs::write(workspace.join("data/artifacts/report.md"), "private")
            .await
            .unwrap();
        git_cmd(&["add", "."], workspace).await.unwrap();
        git_cmd(&["commit", "-m", "scaffold"], workspace)
            .await
            .unwrap();
        git_cmd(&["branch", branch], workspace).await.unwrap();
    }

    #[tokio::test]
    async fn recreate_lost_worktree_rebuilds_an_app_worktree_sparsely() {
        let tmp = tempfile::tempdir().unwrap();
        let workspace = tmp.path().to_path_buf();
        let branch = "lucidos-claude-code-app-habit-tracker-add-streaks-ae6846f4";
        workspace_with_lost_app_branch(&workspace, branch).await;
        let wt = worktrees_dir(&workspace).join("thread-ae6846f4");

        recreate_lost_worktree(&workspace, branch, &wt, Some("habit-tracker"))
            .await
            .expect("an app worktree must rebuild on its surviving branch");

        assert!(
            wt.join("data/apps/habit-tracker/index.html").exists(),
            "the thread's own app folder must be materialised"
        );
        assert!(
            !wt.join("data/apps/other/index.html").exists(),
            "a sibling app must stay outside the sparse cone"
        );
        assert!(
            !wt.join("data/artifacts/report.md").exists(),
            "artifacts must stay outside the sparse cone"
        );
    }

    #[tokio::test]
    async fn recreate_lost_worktree_rebuilds_a_non_app_worktree_in_full() {
        let tmp = tempfile::tempdir().unwrap();
        let workspace = tmp.path().to_path_buf();
        let branch = "lucidos-claude-code-repo-fix-oauth-urls-ae6846f4";
        workspace_with_lost_app_branch(&workspace, branch).await;
        let wt = worktrees_dir(&workspace).join("thread-ae6846f4");

        recreate_lost_worktree(&workspace, branch, &wt, None)
            .await
            .expect("a non-app worktree must rebuild on its surviving branch");

        let head = git_cmd(&["rev-parse", "--abbrev-ref", "HEAD"], &wt)
            .await
            .unwrap();
        assert_eq!(String::from_utf8_lossy(&head.stdout).trim(), branch);
        let narrowed = git_cmd(&["config", "--get", "core.sparseCheckout"], &wt)
            .await
            .unwrap();
        assert!(
            !narrowed.status.success(),
            "a non-app rebuild must not narrow the checkout"
        );
    }

    #[test]
    fn orphan_recovery_target_surfaces_known_branch_and_skips_unknown() {
        let tid = Uuid::new_v4();
        let mut branch_to_thread = HashMap::new();
        branch_to_thread.insert("claude-code/known".to_string(), tid);

        assert_eq!(
            orphan_recovery_target(&branch_to_thread, "claude-code/known"),
            Some(tid)
        );

        assert_eq!(
            orphan_recovery_target(&branch_to_thread, "claude-code/orphaned"),
            None
        );
    }
}
