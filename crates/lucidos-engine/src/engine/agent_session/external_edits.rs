//! External-edit detection (Phase 8.2 + 8.3).
//!
//! Between CC turns, the engine's per-thread worktree lives on disk and the
//! user is free to open the worktree in their own editor. When CC resumes,
//! it has no idea anything changed — `--resume` replays the prior session
//! state, not the worktree's current state. The helpers in this module
//! reconcile the two:
//!
//! - [`compute_external_edit_note`] — diff the worktree's current HEAD +
//!   `git status` against the SHA recorded on the previous
//!   `CodingAgentIdled`. If anything moved, build a short note that the
//!   spawn dispatcher prepends to the user's next prompt so CC can react
//!   to the changes ("the user committed X, modified Y") instead of being
//!   surprised by them later.
//!
//! - [`verify_branch`] — refuse to spawn if the user `git checkout`-ed
//!   another branch in the worktree between turns. Continuing on the wrong
//!   branch would silently commit CC's work to the user's feature branch
//!   (or worse, main). Better to surface the mismatch loudly than to
//!   pretend everything is fine.
//!
//! Both helpers are deliberately I/O-only and stateless — they can be unit
//! tested with a real git in a tempdir without any engine context.

use std::path::Path;

use crate::engine::git_ops::{
    git_answer, git_answer_with, git_cmd, worktree_current_branch, GitAnswer,
};

/// Result of [`verify_branch`] when the worktree's checked-out branch
/// doesn't match the engine's expected branch.
///
/// Custom error type (exception to the `Box<dyn Error>` convention in
/// CLAUDE.md) because the spawn dispatcher needs the structured fields
/// (`expected`, `found`) to surface a precise mismatch to the user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BranchMismatch {
    pub expected: String,
    pub found: Option<String>,
}

impl std::fmt::Display for BranchMismatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.found {
            Some(actual) => write!(
                f,
                "worktree is on branch '{}' but expected '{}'. Resolve manually \
                 (e.g. `git checkout {}` in the worktree) before continuing.",
                actual, self.expected, self.expected
            ),
            None => write!(
                f,
                "worktree has a detached HEAD but expected branch '{}'. Resolve \
                 manually before continuing.",
                self.expected
            ),
        }
    }
}

impl std::error::Error for BranchMismatch {}

/// Run `git rev-parse HEAD` in `worktree_path` and return the SHA on success.
/// Returns `None` for any failure (missing dir, branch with zero commits,
/// detached HEAD points to a missing object). Never panics.
pub(crate) async fn git_head_sha(worktree_path: &Path) -> Option<String> {
    match git_cmd(&["rev-parse", "HEAD"], worktree_path).await {
        Ok(o) if o.status.success() => {
            let s = String::from_utf8_lossy(&o.stdout).trim().to_string();
            if s.is_empty() {
                None
            } else {
                Some(s)
            }
        }
        _ => None,
    }
}

/// Run `git status --porcelain` and return the raw lines.
async fn git_status_lines(worktree_path: &Path) -> Vec<String> {
    match git_cmd(&["status", "--porcelain"], worktree_path).await {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout)
            .lines()
            .map(|l| l.to_string())
            .filter(|l| !l.is_empty())
            .collect(),
        _ => Vec::new(),
    }
}

/// Return a one-line-per-commit summary of `<last_sha>..HEAD`. Empty string
/// if the range is empty or the command fails.
async fn git_log_oneline(worktree_path: &Path, last_sha: &str) -> String {
    let range = format!("{}..HEAD", last_sha);
    match git_cmd(
        &["log", "--oneline", "--no-decorate", &range],
        worktree_path,
    )
    .await
    {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).trim().to_string(),
        _ => String::new(),
    }
}

/// Compute a "your worktree changed since last idle" note for prepending to
/// the next CC user prompt. Returns `None` when the worktree is unchanged from
/// the recorded SHA (no new commits, no uncommitted changes).
///
/// The note is intentionally short — CC just needs to know that something
/// moved and what kind of move it was. Long diffs are out of scope; CC can
/// run `git status` / `git log` itself if it needs more.
///
/// The wording deliberately does **not** name a cause. This detector sees a
/// SHA and a `git status`; it cannot tell a hand edit from an engine reset, and
/// asserting "the user edited files" for a reset the *engine* performed on the
/// user's Discard click is a misattribution that sends CC looking for the wrong
/// thing.
///
/// `head_move_explained` is the caller saying another note already states the
/// cause of the HEAD move (`turn_gap::TurnGapNote::explains_worktree_reset`:
/// an Apply, a Discard, or a tier-2 worktree clean). It suppresses exactly one
/// line, the "HEAD moved (no log available)" fallback, which is the signature
/// of a *backwards* reset and carries no information once the cause is known. A
/// non-empty log (someone really did commit) and any uncommitted change are
/// still reported, so nothing real is lost. If that leaves nothing to say, the
/// whole note is `None`.
///
/// If `last_sha` is `None`, returns `None` — the caller should skip
/// injection on the very first turn (no prior idle to compare against).
/// Likewise returns `None` when the worktree path doesn't exist on disk.
pub(crate) async fn compute_external_edit_note(
    worktree_path: &Path,
    last_sha: Option<&str>,
    head_move_explained: bool,
) -> Option<String> {
    let last_sha = last_sha?;
    if !worktree_path.exists() {
        return None;
    }

    let current_sha = git_head_sha(worktree_path).await;
    let dirty = git_status_lines(worktree_path).await;

    let head_moved = match current_sha.as_deref() {
        Some(cur) => cur != last_sha,
        // Couldn't read HEAD — treat as unchanged so we don't spam CC with
        // a confusing note for a transient git failure. A real branch
        // mismatch is caught by `verify_branch` on the same code path.
        None => false,
    };

    if !head_moved && dirty.is_empty() {
        return None;
    }

    // Build the sections first: with the HEAD-move line suppressed and a clean
    // tree there is nothing left to say, and the note must not render as a bare
    // header.
    let mut sections = String::new();

    if head_moved {
        let log = git_log_oneline(worktree_path, last_sha).await;
        if log.is_empty() {
            if !head_move_explained {
                sections.push_str(
                    "\nCommitted changes since your last action: HEAD moved (no log available).",
                );
            }
        } else {
            sections.push_str("\nCommitted changes since your last action:\n");
            sections.push_str(&log);
        }
    }

    if !dirty.is_empty() {
        sections.push_str("\nUncommitted changes:\n");
        // `git status --porcelain` lines are already short and one-per-file.
        // Cap at 50 to keep the prompt bounded for huge edits.
        const MAX_LINES: usize = 50;
        let total = dirty.len();
        for line in dirty.iter().take(MAX_LINES) {
            sections.push_str(line);
            sections.push('\n');
        }
        if total > MAX_LINES {
            sections.push_str(&format!("… and {} more file(s)\n", total - MAX_LINES));
        }
        // Trim trailing newline before the closing bracket for tidiness.
        if sections.ends_with('\n') {
            sections.pop();
        }
    }

    if sections.is_empty() {
        return None;
    }

    let mut note =
        String::from("[Note from engine: your worktree changed since you were last active.");
    note.push_str(&sections);
    note.push(']');
    Some(note)
}

/// Verify that the worktree's currently checked-out branch matches the
/// engine's expected branch. Returns `Ok(())` when they match (or when the
/// worktree path doesn't exist — there's nothing to verify).
///
/// Returns `Err(BranchMismatch)` when the user has externally checked out
/// a different branch (or detached HEAD) — the caller should refuse to
/// spawn CC, since continuing would silently commit to the wrong ref.
pub(crate) async fn verify_branch(
    worktree_path: &Path,
    expected_branch: &str,
) -> Result<(), BranchMismatch> {
    if !worktree_path.exists() {
        return Ok(());
    }
    let found = match git_cmd(&["rev-parse", "--abbrev-ref", "HEAD"], worktree_path).await {
        Ok(o) if o.status.success() => {
            let b = String::from_utf8_lossy(&o.stdout).trim().to_string();
            if b == "HEAD" {
                None
            } else {
                Some(b)
            }
        }
        _ => {
            // Couldn't read the branch (e.g. corrupt git state). Better to
            // proceed than to permanently wedge the thread on a transient
            // failure — `git_head_sha` will also return None and the worst
            // case is a missing edit-note, not data loss.
            return Ok(());
        }
    };

    if found.as_deref() == Some(expected_branch) {
        Ok(())
    } else {
        Err(BranchMismatch {
            expected: expected_branch.to_string(),
            found,
        })
    }
}

/// True iff `ancestor_sha` is reachable from `descendant_ref`.
pub(crate) async fn is_ancestor(
    worktree_path: &Path,
    ancestor_sha: &str,
    descendant_ref: &str,
) -> bool {
    // `or_unknown(false)`: a probe that could not run must never claim
    // ancestry. The only caller (`try_adopt_renegade_branch`) reads a `false`
    // as "unsafe to adopt" and lets the spawn refuse loudly on the branch
    // mismatch, whereas an unverified `true` would silently retarget the
    // thread onto a branch that may not contain its work. The rule is in
    // `.claude/rules/rust.md`: an unanswered probe is UNKNOWN, never a "no",
    // and never the answer that authorizes losing work.
    git_answer(
        &["merge-base", "--is-ancestor", ancestor_sha, descendant_ref],
        worktree_path,
    )
    .await
    .or_unknown(false)
}

/// Decide whether the worktree's current branch can be safely adopted as
/// the new tracked branch. Safe means HEAD's history contains
/// `last_sha` — proving the new branch was created on top of our prior
/// state, so adoption preserves all our commits.
///
/// Returns `Some((new_branch_name, note_for_cc))` when adoption is safe.
/// Returns `None` when there's no last SHA, the worktree HEAD can't be
/// read, or HEAD has diverged from our state.
pub(crate) async fn try_adopt_renegade_branch(
    worktree_path: &Path,
    last_sha: Option<&str>,
) -> Option<(String, String)> {
    let last = last_sha?;
    let new_branch = worktree_current_branch(worktree_path).await?;
    if !is_ancestor(worktree_path, last, "HEAD").await {
        return None;
    }
    Some((new_branch.clone(), build_adoption_note(&new_branch)))
}

/// [`try_adopt_renegade_branch`] for the **idle** boundary rather than the
/// spawn one.
///
/// The tracked branch name is a spawn-time snapshot of a fact that lives in
/// git, inside a worktree the coding agent owns: `git branch -m` from a repo's
/// own skill is ordinary, and we already decided (at spawn) to follow the agent
/// onto its new branch. Until this existed, adoption ran ONLY on respawn, so a
/// single-turn session that renamed its own branch computed its end-of-turn
/// diff against a ref that no longer existed, git exited 128, and the Diff
/// button stayed dark forever. Idle is the boundary whose answer is durable
/// (`CodingAgentIdled.has_changes` feeds `thread_summaries.coding_agent_has_diff`),
/// which is why it, of all the branch-reading sites, must not trust the cache.
///
/// Adoption needs BOTH gates to pass:
///
/// 1. The same ancestry check [`try_adopt_renegade_branch`] makes: the worktree
///    HEAD must descend from `anchor_sha`, where this session last knew itself
///    to be. No anchor means no check to make, so no adoption.
/// 2. [`tracked_branch_continues_into_head`]: the current branch must be
///    provably a continuation of the tracked one, either because git's own
///    reflog records the rename, or because the tracked ref is still an
///    ancestor of HEAD.
///
/// Gate 2 is what the spawn path doesn't need and this one does. At spawn,
/// `anchor_sha` is the previous idle's HEAD, which already contains the
/// thread's commits. On a session's FIRST idle there is no previous idle, so
/// the anchor is the worktree's HEAD at spawn, which for a fresh branch is just
/// the base tip: on its own, gate 1 would then accept ANY branch forked from
/// the same base. Gate 2 is what makes that anchor safe, and it is why "the
/// tracked ref is gone" is not accepted as evidence of a rename: an agent that
/// checks out a sibling branch and then deletes the tracked one produces the
/// same absence, and adopting there would point the thread's Diff (and a later
/// Discard, which deletes the branch) at work that was never ours.
///
/// Every probe involved refuses adoption on `GitAnswer::Unknown`: the question
/// was not answered, and an unanswered probe must never authorize retargeting a
/// thread (`.claude/rules/rust.md`).
///
/// Returns `Some((new_branch, note_for_the_agent))` when the worktree's branch
/// should be adopted, `None` to keep the tracked one.
pub(crate) async fn try_adopt_branch_at_idle(
    repo_root: &Path,
    worktree_path: &Path,
    tracked_branch: &str,
    anchor_sha: Option<&str>,
) -> Option<(String, String)> {
    // Read the branch ONCE and gate that value. Delegating the adoption back to
    // `try_adopt_renegade_branch` would re-read it, so the name checked and the
    // name adopted could differ; the gates below are worth nothing if they
    // approve one branch and the caller is handed another.
    let anchor = anchor_sha?;
    let current = worktree_current_branch(worktree_path).await?;
    if current == tracked_branch {
        return None;
    }
    if !tracked_branch_continues_into_head(repo_root, worktree_path, &current, tracked_branch).await
    {
        return None;
    }
    if !is_ancestor(worktree_path, anchor, "HEAD").await {
        return None;
    }
    let note = build_adoption_note(&current);
    Some((current, note))
}

/// Gate 2 of [`try_adopt_branch_at_idle`]: is `current_branch` provably a
/// continuation of `tracked_branch`?
///
/// Two ways to prove it, and both are positive evidence:
///
/// - The tracked ref is still there and is reachable from HEAD, so whatever the
///   agent created was built on top of our work (`git checkout -b`).
/// - git's own reflog for the current branch records the rename. `git branch -m`
///   moves the old ref's reflog onto the new name and appends a
///   `Branch: renamed refs/heads/<old> to refs/heads/<new>` entry, so the new
///   branch carries proof of where it came from.
///
/// The absence of the tracked ref is deliberately NOT evidence. An agent that
/// checks out a sibling branch and then deletes the tracked one leaves exactly
/// the same absence, and a first idle's anchor can be no stronger than the
/// shared base, so accepting absence would let an unrelated branch pass both
/// gates. That branch would then own the thread's Diff and, on an explicit
/// Discard, be the branch deleted.
///
/// A repo with reflogs disabled (`core.logAllRefUpdates=false`) yields no
/// evidence and therefore no adoption. That is the safe direction: the thread
/// keeps its tracked branch and the next spawn re-derives.
async fn tracked_branch_continues_into_head(
    repo_root: &Path,
    worktree_path: &Path,
    current_branch: &str,
    tracked_branch: &str,
) -> bool {
    let tracked_ref = format!("refs/heads/{}", tracked_branch);
    match git_answer(
        &["rev-parse", "--verify", "--quiet", &tracked_ref],
        repo_root,
    )
    .await
    {
        GitAnswer::Yes => is_ancestor(worktree_path, &tracked_ref, "HEAD").await,
        GitAnswer::No => {
            branch_reflog_records_rename_from(repo_root, current_branch, tracked_branch).await
        }
        // Could not ask. Never retarget on an unanswered probe.
        GitAnswer::Unknown => {
            log!(
                "[AgentSession] Could not verify whether ref {} still exists, refusing idle branch adoption",
                tracked_ref
            );
            false
        }
    }
}

/// Does `current_branch`'s reflog record that it was renamed from
/// `tracked_branch`? The message is written by git itself (`builtin/branch.c`),
/// so this is git's own account of the rename rather than an inference from
/// what is missing.
async fn branch_reflog_records_rename_from(
    repo_root: &Path,
    current_branch: &str,
    tracked_branch: &str,
) -> bool {
    let renamed_from = format!("Branch: renamed refs/heads/{} to ", tracked_branch);
    git_answer_with(
        &["reflog", "show", "--format=%gs", current_branch],
        repo_root,
        |out| {
            String::from_utf8_lossy(&out.stdout)
                .lines()
                .any(|line| line.starts_with(&renamed_from))
        },
    )
    .await
    .or_unknown(false)
}

fn build_adoption_note(new_branch: &str) -> String {
    format!(
        "[Note from engine: while you were idle, your worktree was switched to branch '{0}' \
         (likely by a skill or external tool). It contains your prior work, so I've adopted \
         it as the new tracked branch — continuing on '{0}' from now on.]",
        new_branch
    )
}

#[cfg(test)]
#[path = "external_edits_tests.rs"]
mod tests;
