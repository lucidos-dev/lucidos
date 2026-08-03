use super::*;
use std::path::Path;

/// `git rev-parse HEAD` in the worktree. Used by `auto_commit_preserving_marker`
/// to re-stamp the hardening record after an auto-commit moves HEAD.
pub(crate) async fn current_head_sha(worktree_path: &Path) -> Option<String> {
    let output = git_cmd(&["rev-parse", "HEAD"], worktree_path).await.ok()?;
    if !output.status.success() {
        return None;
    }
    let sha = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!sha.is_empty()).then_some(sha)
}

/// `git rev-parse <branch>` in the main repo — works after the worktree has
/// been removed, which is the case at apply time and during stale-session
/// recovery. Returns `None` if the branch ref doesn't exist.
pub(crate) async fn branch_head_sha(repo_root: &Path, branch_name: &str) -> Option<String> {
    let output = git_cmd(&["rev-parse", branch_name], repo_root).await.ok()?;
    if !output.status.success() {
        return None;
    }
    let sha = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!sha.is_empty()).then_some(sha)
}

/// The repo's **root-commit SHA** — the hash of its initial (parent-less) commit
/// (`git rev-list --max-parents=0 HEAD`). This is intrinsic to the git history:
/// it survives moving, renaming, and re-cloning the checkout, so it is the basis
/// of a repository's deterministic identity (see `core::repositories::deterministic_id`).
/// A history with multiple root commits (merged-in unrelated histories) yields
/// several — we pick the lexically-smallest for a stable, order-independent
/// answer and log the multi-root case. Returns `None` when the repo has no
/// commits yet or git fails; callers fall back to a path-derived id.
pub(crate) async fn root_commit_sha(repo_root: &Path) -> Option<String> {
    let output = git_cmd(&["rev-list", "--max-parents=0", "HEAD"], repo_root)
        .await
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let mut roots: Vec<String> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect();
    if roots.len() > 1 {
        log!(
            "[GitOps] {} root commits at {} — using lexically-smallest for stable repo id",
            roots.len(),
            repo_root.display()
        );
    }
    roots.sort();
    roots.into_iter().next()
}

/// Auto-commit harmless dirty files (docs/plans), then return whether the repo
/// is still dirty. This prevents apply/revert from blocking on safe changes.
/// Also re-attaches HEAD to main if detached (a precondition for accurate
/// dirty-file detection -- detached HEAD reports false diffs).
///
/// Returns `true` if the repo has uncommitted changes (after any auto-commit).
pub(crate) async fn auto_commit_safe_files_if_dirty(repo_root: &Path) -> bool {
    ensure_head_on_main(repo_root).await;
    let output = match git_cmd(&["status", "--porcelain"], repo_root).await {
        Ok(o) => o,
        Err(e) => {
            log!(
                "[GitOps] auto_commit_safe_files_if_dirty: git status failed at {}: {} — treating as clean",
                repo_root.display(),
                e
            );
            return false;
        }
    };
    let status = String::from_utf8_lossy(&output.stdout);
    // Porcelain format: "XY filename" -- status is first 2 chars, then a space, then path
    // Skip untracked files (??) -- they don't block git merge
    let dirty_files: Vec<&str> = status
        .lines()
        .filter(|l| l.len() >= 4 && !l.starts_with("??"))
        .map(|l| l[3..].trim())
        .collect();
    if dirty_files.is_empty() {
        return false;
    }
    // Auto-commit harmless dirty files that shouldn't block merging Lucidos changes
    let auto_committable = dirty_files.iter().all(|f| f.starts_with("docs/plans/"));
    if auto_committable {
        let msg = "chore: commit docs changes";
        let mut add_args: Vec<&str> = vec!["add", "--"];
        add_args.extend(dirty_files.iter());
        let _ = git_cmd(&add_args, repo_root).await;
        let _ = git_cmd(&["commit", "-m", msg], repo_root).await;
        log!("[Git] Auto-committed dirty files: {:?}", dirty_files);
        return false;
    }
    true
}

/// Subjects we never surface to the user — internal auto-commits.
fn is_internal_auto_commit(subject: &str) -> bool {
    matches!(
        subject,
        "Coding agent changes (auto-committed)"
            | "Coding agent changes (recovered after restart)"
            | "Coding agent changes (pre-merge auto-commit)"
            | "Coding agent changes (post-merge auto-commit)"
            // Legacy subjects produced before the coding-agent rename. Keep
            // matching them so internal auto-commits on threads created before
            // this change stay filtered out of user-facing commit lists
            // instead of suddenly surfacing as "meaningful" commits.
            | "Claude Code changes (auto-committed)"
            | "Claude Code changes (recovered after restart)"
            | "Claude Code changes (pre-merge auto-commit)"
            | "Claude Code changes (post-merge auto-commit)"
    )
}

/// Run `git log --format=%s <args>` and return user-meaningful commit subjects.
/// Internal auto-commits and blank lines are filtered out.
async fn commit_subjects(repo_root: &Path, log_args: &[&str]) -> Vec<String> {
    let mut args = vec!["log", "--format=%s"];
    args.extend_from_slice(log_args);
    match git_cmd(&args, repo_root).await {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout)
            .lines()
            .filter(|l| !l.is_empty() && !is_internal_auto_commit(l))
            .map(|l| l.to_string())
            .collect(),
        _ => Vec::new(),
    }
}

/// Read meaningful commit subjects in the range `pre_sha..post_sha`, oldest first.
pub(crate) async fn commits_in_range(
    repo_root: &Path,
    pre_sha: &str,
    post_sha: &str,
) -> Vec<String> {
    if pre_sha == post_sha {
        return Vec::new();
    }
    let range = format!("{}..{}", pre_sha, post_sha);
    commit_subjects(repo_root, &["--reverse", &range]).await
}

/// Build a description for a pending change from the commit subjects on a branch.
/// Reads `git log --format=%s <base>..branch` and summarizes the subjects.
/// If no meaningful commits found, uses `fallback` as the description.
/// If `suffix` is provided, it's appended in parentheses (e.g. "recovered").
pub(crate) async fn describe_branch_changes(
    repo_root: &Path,
    range_arg: &str,
    fallback: &str,
    suffix: Option<&str>,
) -> String {
    let subjects = commit_subjects(repo_root, &[range_arg]).await;

    let base = if subjects.is_empty() {
        fallback.to_string()
    } else {
        subjects.join("\n")
    };

    match suffix {
        Some(s) => format!("{} ({})", base, s),
        None => base,
    }
}

/// Check if a branch has commits vs main (i.e., the branch has diverged from the default branch).
/// Returns `true` if there are commits on the branch not on main, or on error (safe default).
///
/// Uses `main` as the base ref rather than `HEAD` because the repo's checked-out
/// branch may differ from `main` (especially for external repos), which would give
/// wrong results.
pub(crate) async fn has_branch_commits(repo_root: &Path, branch_name: &str) -> bool {
    let base = default_local_branch(repo_root).await;
    let range = format!("{}..{}", base, branch_name);
    match git_cmd(&["log", "--oneline", &range], repo_root).await {
        Ok(o) if o.status.success() => !o.stdout.is_empty(),
        Ok(o) => {
            log!(
                "[Git] git log failed for branch {}: {}",
                branch_name,
                String::from_utf8_lossy(&o.stderr).trim()
            );
            true
        }
        Err(e) => {
            log!("[Git] git log failed for branch {}: {}", branch_name, e);
            true
        }
    }
}

/// Get the list of changed files between the branch's diff base and the branch
/// (three-dot merge-base diff). The base is `default_diff_base` — the SAME ref
/// the Diff button diffs against (`origin/<default>` when the local default
/// branch has diverged, otherwise the local default) — so the
/// `coding_agent_has_diff` gate this feeds and the diff the button renders can
/// never disagree. Using the local default directly here was the
/// `example-repo` migration bug: a branch whose commits already live on
/// `origin/main` but not on a force-rewritten local `main` showed 53 changed
/// files against local `main` yet 0 against `origin/main`, lighting the Diff
/// button on an empty diff.
///
/// Strips engine-injected paths — see `is_engine_injected_path` for rationale.
///
/// Errors are swallowed into an empty list, which is safe for every caller that
/// only asks "is there something to propose here?" — a git hiccup then simply
/// skips a proposal that the next idle re-tries. It is NOT safe for a caller
/// that *writes* the empty answer somewhere durable: use
/// [`branch_changed_files_checked`] there.
pub(crate) async fn branch_changed_files(repo_root: &Path, branch_name: &str) -> Vec<String> {
    branch_changed_files_checked(repo_root, branch_name)
        .await
        .unwrap_or_default()
}

/// [`branch_changed_files`] that distinguishes "git says the diff is empty"
/// from "git could not answer". Fails on spawn failure, the 30s timeout, and a
/// non-zero exit (a deleted/renamed branch ref is the common one).
///
/// The distinction is load-bearing for `reconcile_emptied_pending_change`,
/// which zeroes a pending change's file list from this answer: taking a
/// transient git failure as "no changes" would wipe the recorded file list of
/// work still sitting on the branch.
pub(crate) async fn branch_changed_files_checked(
    repo_root: &Path,
    branch_name: &str,
) -> Result<Vec<String>, String> {
    let base = default_diff_base(repo_root).await;
    let range = format!("{}...{}", base, branch_name);
    let out = git_cmd(&["diff", "--name-only", &range], repo_root).await?;
    if !out.status.success() {
        return Err(format!(
            "git diff --name-only {} failed: {}",
            range,
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter(|l| !crate::engine::claude_code::is_engine_injected_path(l))
        .map(|l| l.to_string())
        .collect())
}

/// Check whether a branch carries authored (non-merge) commits that aren't yet
/// on main. Stricter than `has_branch_commits`, which counts *all* commits ahead
/// of main — including merge commits.
///
/// The distinction matters after an apply: when a branch's work is merged into
/// main and the engine then back-merges main *into* the branch (conflict
/// recovery), the branch is left with a merge commit ahead of main and a
/// criss-crossed history with two merge bases. `has_branch_commits` is fooled by
/// that merge commit (TRUE), and the three-dot `branch_changed_files` base
/// regresses to the original fork point, re-surfacing the already-applied files
/// as a phantom diff — so the startup recovery sweep re-proposed the
/// already-applied change as a new pending change (real thread `bb9e68d6`).
///
/// A coding-agent worktree only ever produces *authored* work as non-merge
/// commits (the agent's edits) — merge commits on the branch are exclusively
/// engine-generated back-merges. So "has a non-merge commit not in main" is the
/// honest test for "is there real un-applied work here".
///
/// Uses the same `default_local_branch` base as `has_branch_commits` (the gate
/// it replaces); on git error it defaults to TRUE so a transient failure never
/// silently swallows a real proposal — the empty-`branch_changed_files` check
/// downstream is the second guard.
pub(crate) async fn has_unmerged_authored_commits(repo_root: &Path, branch_name: &str) -> bool {
    let base = default_local_branch(repo_root).await;
    let range = format!("{}..{}", base, branch_name);
    match git_cmd(&["rev-list", "--no-merges", "--count", &range], repo_root).await {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout)
            .trim()
            .parse::<u64>()
            .map(|n| n > 0)
            .unwrap_or(true),
        Ok(o) => {
            log!(
                "[Git] git rev-list --no-merges failed for branch {}: {}",
                branch_name,
                String::from_utf8_lossy(&o.stderr).trim()
            );
            true
        }
        Err(e) => {
            log!(
                "[Git] git rev-list --no-merges failed for branch {}: {}",
                branch_name,
                e
            );
            true
        }
    }
}

/// Files a Change proposal should reference for this branch, or `None` when the
/// branch isn't proposal-worthy. `None` covers three cases: no authored
/// (non-merge) commits ahead of main (a bare branch, or one whose only commits
/// ahead are engine back-merges of already-applied work — see
/// `has_unmerged_authored_commits`), or authored commits whose changes cancel
/// out (commit + revert, e.g. CC's `npm install` lockfile rename + restore —
/// zero net diff). Returning the file list here lets callers skip a second
/// `branch_changed_files` call inside the proposal flow.
pub(crate) async fn proposal_files_for_branch(
    repo_root: &Path,
    branch_name: &str,
) -> Option<Vec<String>> {
    if !has_unmerged_authored_commits(repo_root, branch_name).await {
        return None;
    }
    let files = branch_changed_files(repo_root, branch_name).await;
    if files.is_empty() {
        None
    } else {
        Some(files)
    }
}

/// Auto-commit uncommitted changes in a worktree with a generic message.
///
/// `or_unknown(true)`: when git cannot say whether there is anything to commit,
/// try anyway. A commit attempt against a clean tree is a no-op that git
/// refuses harmlessly, whereas skipping it leaves the user's edits uncommitted
/// on the strength of a question that was never answered.
pub(crate) async fn auto_commit_worktree(worktree_path: &Path, message: &str) {
    let has_changes = worktree_dirtiness(worktree_path).await.or_unknown(true);
    if has_changes {
        let _ = git_cmd(&["add", "-A"], worktree_path).await;
        let _ = git_cmd(&["commit", "-m", message], worktree_path).await;
    }
}

/// Stage all changes and commit them with `message`, propagating failures.
///
/// Mirrors `auto_commit_worktree` but returns `Err` when `git status`,
/// `git add`, or `git commit` exits non-zero (or fails to spawn). Use this
/// when the caller relies on the commit having actually landed — e.g. the
/// apply-now CC iteration loop, where a silently-dropped commit would lose a
/// real CC change.
///
/// Returns `Ok(true)` when a commit was made, `Ok(false)` when the worktree
/// was clean (nothing to commit).
pub(crate) async fn commit_worktree_or_err(
    worktree_path: &Path,
    message: &str,
) -> Result<bool, String> {
    let status = git_cmd(&["status", "--porcelain"], worktree_path).await?;
    if !status.status.success() {
        return Err(format!(
            "git status --porcelain failed: {}",
            String::from_utf8_lossy(&status.stderr).trim()
        ));
    }
    if status.stdout.is_empty() {
        return Ok(false);
    }
    let add = git_cmd(&["add", "-A"], worktree_path).await?;
    if !add.status.success() {
        return Err(format!(
            "git add -A failed: {}",
            String::from_utf8_lossy(&add.stderr).trim()
        ));
    }
    let commit = git_cmd(&["commit", "-m", message], worktree_path).await?;
    if !commit.status.success() {
        return Err(format!(
            "git commit -m \"{}\" failed: {}",
            message,
            String::from_utf8_lossy(&commit.stderr).trim()
        ));
    }
    Ok(true)
}

/// Run `git commit --no-edit` in `worktree_path`, returning `Err` on failure.
///
/// Used by the conflict-resolution merge path, which has an in-progress merge
/// (MERGE_HEAD present) and just needs to finalize the merge commit. Returns
/// `Err` when the process fails to spawn, times out, or exits non-zero.
/// The caller decides whether to log or propagate — the conflict-resolution
/// cleanup path logs and continues so the downstream `ff_merge_to_main` can
/// surface the real "merge not finalised" error to the user.
pub(crate) async fn git_commit_no_edit(worktree_path: &Path) -> Result<(), String> {
    let out = git_cmd(&["commit", "--no-edit"], worktree_path).await?;
    if !out.status.success() {
        return Err(format!(
            "git commit --no-edit failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(())
}

/// Auto-commit changes in a worktree, preserving the harden marker if fresh.
///
/// The auto-commit can create a new commit (e.g. .claude/ artifacts CC didn't
/// commit), advancing HEAD and invalidating the harden marker even though
/// /harden already reviewed the working tree. This function checks the marker
/// BEFORE committing and re-stamps it afterward with the new HEAD SHA.
///
/// Short-circuits: if the worktree has no uncommitted files, no marker check
/// is needed (HEAD won't move). `or_unknown(true)`, so an unanswerable
/// `git status` takes the commit path rather than the short-circuit: against a
/// clean tree the commit is a harmless no-op and the marker is re-stamped with
/// the unchanged HEAD, while skipping would leave real edits uncommitted.
pub(crate) async fn auto_commit_preserving_marker(
    pool: &sqlx::PgPool,
    worktree_path: &Path,
    repo_root: &Path,
    branch_name: &str,
    message: &str,
) {
    let has_changes = worktree_dirtiness(worktree_path).await.or_unknown(true);
    if !has_changes {
        return;
    }
    let marker_fresh = is_harden_marker_fresh(pool, repo_root, branch_name).await;
    let _ = git_cmd(&["add", "-A"], worktree_path).await;
    let _ = git_cmd(&["commit", "-m", message], worktree_path).await;
    if marker_fresh {
        if let Some(sha) = current_head_sha(worktree_path).await {
            if let Err(e) = record_hardened(pool, repo_root, branch_name, &sha).await {
                log!(
                    "[Git] Failed to re-stamp hardened_branches for {}: {}",
                    branch_name,
                    e
                );
            } else {
                log!("[Git] Re-stamped harden marker after auto-commit");
            }
        }
    }
}

/// Outcome of trying to recover from a "branch has no commits" state in `apply_change`.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum NoCommitsRecovery {
    /// The branch's worktree had uncommitted work that was just auto-committed,
    /// so the branch now has commits. The caller should fall through to the merge path.
    AutoCommitted,
    /// The branch is genuinely empty AND the change has no declared files.
    /// Safe to mark the change applied as a no-op.
    LegitimateNoOp,
    /// The branch had work, but main already contains it (sibling apply, fast-forward,
    /// out-of-band merge, etc.). `git log main..branch` is empty AND main's history
    /// contains commits touching the change's files. Safe to mark applied as a no-op.
    AlreadyApplied,
}

/// Does main's history contain any commit touching at least one of `change_files`?
///
/// Used to distinguish "branch's work was already merged into main" (no-op) from
/// "branch never produced any commits for the referenced files" (corruption).
/// Files referenced by an applied change should always have a corresponding commit
/// somewhere on main, even if the file was later deleted.
///
/// Read-only — unlike [`recover_no_commits_branch`] it never auto-commits the
/// worktree, so `apply_now`'s already-merged check can call it without risking a
/// commit that a subsequent failure-path `reset --hard` would destroy.
///
/// `or_unknown(false)`: a `true` here concludes the work is already on main and
/// resolves the change as a no-op, which is the direction that silently drops
/// it. An unanswered probe therefore reads as "not applied", and the caller
/// surfaces a loud recovery error the user can retry.
pub(crate) async fn main_history_touches_files(repo_root: &Path, change_files: &[String]) -> bool {
    if change_files.is_empty() {
        return false;
    }
    let base = default_local_branch(repo_root).await;
    let mut args: Vec<String> = vec![
        "log".to_string(),
        "--oneline".to_string(),
        "-1".to_string(),
        base,
        "--".to_string(),
    ];
    args.extend(change_files.iter().cloned());
    let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
    git_answer_with(&arg_refs, repo_root, |o| !o.stdout.is_empty())
        .await
        .or_unknown(false)
}

/// When `has_branch_commits` returned false, decide what to do about it.
///
/// Behaviour:
///   - If a worktree exists for the branch with uncommitted work, auto-commit it.
///     This rescues the silent-data-loss case where CC left staged-but-uncommitted
///     work on a branch ref that still points at the merge base.
///   - Re-check whether the branch now has commits.
///   - If yes → `AutoCommitted` (caller proceeds with the normal merge).
///   - If still no commits AND `change_files` is empty → `LegitimateNoOp` (safe no-op).
///   - If still no commits AND main's history touches any of `change_files` →
///     `AlreadyApplied` (the work landed on main via a sibling apply, fast-forward,
///     or out-of-band merge — nothing to do).
///   - Otherwise → `Err(...)` — branch is empty AND main has no commits touching the
///     declared files. This is the genuinely-empty case (likely a never-committed
///     draft); discarding the change is safe.
///
/// The branch ref is NOT deleted by this function; the caller decides.
pub(crate) async fn recover_no_commits_branch(
    repo_root: &Path,
    branch_name: &str,
    change_files: &[String],
) -> Result<NoCommitsRecovery, Box<dyn std::error::Error + Send + Sync>> {
    if let Some(ref wt) = find_worktree_for_branch(repo_root, branch_name).await {
        auto_commit_worktree(wt, "Coding agent changes (pre-apply auto-commit)").await;
    }

    if has_branch_commits(repo_root, branch_name).await {
        return Ok(NoCommitsRecovery::AutoCommitted);
    }

    if change_files.is_empty() {
        return Ok(NoCommitsRecovery::LegitimateNoOp);
    }

    if main_history_touches_files(repo_root, change_files).await {
        return Ok(NoCommitsRecovery::AlreadyApplied);
    }

    let preview = if change_files.len() <= 3 {
        change_files.join(", ")
    } else {
        format!("{}, ...", change_files[..3].join(", "))
    };
    Err(format!(
        "Branch '{}' has no commits and main has no history for the {} file(s) referenced \
         by this change ({}). The work was likely never committed — discard the change to clear it.",
        branch_name,
        change_files.len(),
        preview,
    )
    .into())
}
