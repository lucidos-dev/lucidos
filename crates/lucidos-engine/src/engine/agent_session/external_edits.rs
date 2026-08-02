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

use crate::engine::git_ops::{git_cmd, worktree_current_branch};

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
    git_cmd(
        &["merge-base", "--is-ancestor", ancestor_sha, descendant_ref],
        worktree_path,
    )
    .await
    .map(|o| o.status.success())
    .unwrap_or(false)
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
