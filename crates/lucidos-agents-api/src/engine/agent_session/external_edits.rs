use std::path::Path;

use crate::engine::agent_session::resume::IdleAnchor;
use crate::engine::git_ops::{git_answer, git_answer_with, git_cmd, worktree_current_branch};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum BranchMismatch {
    OnOtherBranch { expected: String, found: String },
    Detached { expected: String },
    Unanswered { expected: String },
}

impl std::fmt::Display for BranchMismatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::OnOtherBranch { expected, found } => write!(
                f,
                "worktree is on branch '{}' but expected '{}'. Resolve manually \
                 (e.g. `git checkout {}` in the worktree) before continuing.",
                found, expected, expected
            ),
            Self::Detached { expected } => write!(
                f,
                "worktree has a detached HEAD but expected branch '{}'. Resolve \
                 manually before continuing.",
                expected
            ),
            Self::Unanswered { expected } => write!(
                f,
                "git could not say which branch the worktree is on, so it cannot be \
                 confirmed as '{}'. Send the message again to retry.",
                expected
            ),
        }
    }
}

impl std::error::Error for BranchMismatch {}

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
        None => false,
    };

    if !head_moved && dirty.is_empty() {
        return None;
    }

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
        const MAX_LINES: usize = 50;
        let total = dirty.len();
        for line in dirty.iter().take(MAX_LINES) {
            sections.push_str(line);
            sections.push('\n');
        }
        if total > MAX_LINES {
            sections.push_str(&format!("… and {} more file(s)\n", total - MAX_LINES));
        }
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

pub(crate) async fn verify_branch(
    worktree_path: &Path,
    expected_branch: &str,
) -> Result<(), BranchMismatch> {
    if !worktree_path.exists() {
        return Ok(());
    }
    match git_cmd(&["rev-parse", "--abbrev-ref", "HEAD"], worktree_path).await {
        Ok(o) if o.status.success() => {
            let found = String::from_utf8_lossy(&o.stdout).trim().to_string();
            if found == expected_branch {
                Ok(())
            } else if found == "HEAD" {
                Err(BranchMismatch::Detached {
                    expected: expected_branch.to_string(),
                })
            } else {
                Err(BranchMismatch::OnOtherBranch {
                    expected: expected_branch.to_string(),
                    found,
                })
            }
        }
        Ok(o) => {
            log_legacy!(
                "[AgentSession] `git rev-parse --abbrev-ref HEAD` failed in {}: {}",
                worktree_path.display(),
                String::from_utf8_lossy(&o.stderr).trim()
            );
            Err(BranchMismatch::Unanswered {
                expected: expected_branch.to_string(),
            })
        }
        Err(e) => {
            log_legacy!(
                "[AgentSession] cannot read the checked-out branch in {}: {}",
                worktree_path.display(),
                e
            );
            Err(BranchMismatch::Unanswered {
                expected: expected_branch.to_string(),
            })
        }
    }
}

pub(crate) async fn is_ancestor(
    worktree_path: &Path,
    ancestor_sha: &str,
    descendant_ref: &str,
) -> bool {
    git_answer(
        &["merge-base", "--is-ancestor", ancestor_sha, descendant_ref],
        worktree_path,
    )
    .await
    .or_unknown(false)
}

pub(crate) async fn try_adopt_renegade_branch(
    repo_root: &Path,
    worktree_path: &Path,
    tracked_branch: &str,
    anchor: &IdleAnchor,
) -> Option<(String, String)> {
    let current =
        branch_continuing_tracked(repo_root, worktree_path, tracked_branch, anchor).await?;
    let note = build_adoption_note(&current);
    Some((current, note))
}

pub(crate) async fn branch_continuing_tracked(
    repo_root: &Path,
    worktree_path: &Path,
    tracked_branch: &str,
    anchor: &IdleAnchor,
) -> Option<String> {
    // INVARIANT: Read the branch ONCE and gate that value. A re-read can approve one
    // branch and hand the caller another, as `try_adopt_branch_at_idle` says.
    let current = worktree_current_branch(worktree_path).await?;
    if current == tracked_branch {
        return None;
    }
    let proved = match anchor {
        IdleAnchor::Unknown => false,
        IdleAnchor::Found(sha) => {
            is_ancestor(worktree_path, sha, "HEAD").await
                && branch_provenance(repo_root, worktree_path, &current, tracked_branch)
                    .await
                    .is_proven()
        }
        IdleAnchor::Absent => {
            let provenance =
                branch_provenance(repo_root, worktree_path, &current, tracked_branch).await;
            tracked_branch_continues_into_head(worktree_path, tracked_branch, provenance).await
        }
    };
    proved.then_some(current)
}

pub(crate) async fn worktree_serves_branch(
    repo_root: &Path,
    worktree_path: &Path,
    tracked_branch: &str,
    anchor: &IdleAnchor,
) -> bool {
    worktree_current_branch(worktree_path).await.as_deref() == Some(tracked_branch)
        || branch_continuing_tracked(repo_root, worktree_path, tracked_branch, anchor)
            .await
            .is_some()
}

pub(crate) async fn try_adopt_branch_at_idle(
    repo_root: &Path,
    worktree_path: &Path,
    tracked_branch: &str,
    anchor_sha: Option<&str>,
) -> Option<(String, String)> {
    // INVARIANT: Read the branch ONCE and gate that value. Delegating the adoption back to
    // `try_adopt_renegade_branch` would re-read it, so the name checked and the
    // name adopted could differ; the gates below are worth nothing if they
    // approve one branch and the caller is handed another.
    let anchor = anchor_sha?;
    let current = worktree_current_branch(worktree_path).await?;
    if current == tracked_branch {
        return None;
    }
    let provenance = branch_provenance(repo_root, worktree_path, &current, tracked_branch).await;
    if !tracked_branch_continues_into_head(worktree_path, tracked_branch, provenance).await {
        return None;
    }
    if !is_ancestor(worktree_path, anchor, "HEAD").await {
        return None;
    }
    let note = build_adoption_note(&current);
    Some((current, note))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BranchProvenance {
    CreatedHere,
    RenamedFromTracked,
    Unproven,
}

impl BranchProvenance {
    fn is_proven(self) -> bool {
        !matches!(self, BranchProvenance::Unproven)
    }
}

async fn branch_provenance(
    repo_root: &Path,
    worktree_path: &Path,
    current_branch: &str,
    tracked_branch: &str,
) -> BranchProvenance {
    if worktree_created_the_branch(worktree_path, current_branch).await {
        BranchProvenance::CreatedHere
    } else if branch_reflog_records_rename_from(repo_root, current_branch, tracked_branch).await {
        BranchProvenance::RenamedFromTracked
    } else {
        log_legacy!(
            "[AgentSession] No reflog shows how branch '{}' came from '{}', refusing adoption",
            current_branch,
            tracked_branch
        );
        BranchProvenance::Unproven
    }
}

async fn tracked_branch_continues_into_head(
    worktree_path: &Path,
    tracked_branch: &str,
    provenance: BranchProvenance,
) -> bool {
    match provenance {
        BranchProvenance::RenamedFromTracked => true,
        BranchProvenance::CreatedHere => {
            let tracked_ref = format!("refs/heads/{}", tracked_branch);
            is_ancestor(worktree_path, &tracked_ref, "HEAD").await
        }
        BranchProvenance::Unproven => false,
    }
}

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

/// INVARIANT: Did THIS worktree create `branch`, in the git operation that moved its HEAD
/// onto it? `git checkout -b` and `git switch -c` do exactly that. It is the
/// only thing separating an agent's own ticket branch from a pre-existing
/// sibling while the tracked ref sits at the base. The two leave the same
/// static git state, so the evidence has to be git's reflogs.
///
/// Two conditions, both required:
///
/// - the branch's OLDEST reflog entry is its creation (`branch: Created from`);
/// - this worktree's own HEAD reflog carries a `checkout: moving from <x> to
///   <branch>` entry whose timestamp AND resulting sha both equal that one's.
///
/// One git process stamps every reflog entry it writes with one cached
/// timestamp, so the pair matches exactly however long the checkout runs. A
/// pre-existing branch was created earlier, at a sha that is where it started
/// rather than the tip we landed on. Reflog timestamps have one-second
/// resolution, and the residual that leaves is measured in
/// `docs/plans/2026-08-27-branch-adoption-proves-the-branch-is-ours.md`.
///
/// The HEAD reflog is per-worktree, so this reads it in the worktree.
async fn worktree_created_the_branch(worktree_path: &Path, branch: &str) -> bool {
    const CREATED: &str = "branch: Created from ";
    const MOVED: &str = "checkout: moving from ";

    let Some(branch_log) = reflog_entries(worktree_path, branch).await else {
        return false;
    };
    let Some(created) = branch_log.last().filter(|e| e.subject.starts_with(CREATED)) else {
        return false;
    };
    let Some(head_log) = reflog_entries(worktree_path, "HEAD").await else {
        return false;
    };
    let landed_here = format!(" to {}", branch);
    head_log.iter().any(|e| {
        e.subject.starts_with(MOVED)
            && e.subject.ends_with(&landed_here)
            && e.at == created.at
            && e.new_sha == created.new_sha
    })
}

struct ReflogEntry {
    new_sha: String,
    at: String,
    subject: String,
}

async fn reflog_entries(dir: &Path, reference: &str) -> Option<Vec<ReflogEntry>> {
    let args = [
        "reflog",
        "show",
        "--date=unix",
        "--format=%H %gd %gs",
        reference,
    ];
    match git_cmd(&args, dir).await {
        Ok(o) if o.status.success() => Some(
            String::from_utf8_lossy(&o.stdout)
                .lines()
                .filter_map(parse_reflog_line)
                .collect(),
        ),
        _ => None,
    }
}

fn parse_reflog_line(line: &str) -> Option<ReflogEntry> {
    let mut fields = line.splitn(3, ' ');
    let new_sha = fields.next()?.to_string();
    let selector = fields.next()?;
    let subject = fields.next().unwrap_or_default().to_string();
    let at = selector.rsplit_once("@{")?.1.strip_suffix('}')?.to_string();
    Some(ReflogEntry {
        new_sha,
        at,
        subject,
    })
}

pub(crate) fn build_adoption_note(new_branch: &str) -> String {
    format!(
        "[Note from engine: while you were idle, your worktree was switched to branch '{0}' \
         (likely by a skill or external tool). It contains your prior work, so I've adopted \
         it as the new tracked branch — continuing on '{0}' from now on.]",
        new_branch
    )
}

pub(crate) fn side_branch_note(side_branch: &str, tracked: &str) -> String {
    format!(
        "[Note from engine: your worktree is on branch '{side_branch}', which you created from \
         your thread branch '{tracked}'. Lucidos still tracks '{tracked}': Apply lands only \
         '{tracked}'. Switch back to it, with any work you want applied, before you finish.]"
    )
}

#[cfg(test)]
#[path = "external_edits_tests.rs"]
mod tests;
