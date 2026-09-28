//! The recommended cleanup behind Disk Usage's "Free up space" button.
//!
//! One pass over every worktree, on the user's request. A *finished worktree*
//! is removed. Every other worktree loses its build artifacts and keeps its
//! source and branch. A live or pinned thread is skipped, and so is a
//! stranded tree. Nothing is lost that main or a rebuild does not restore.
//!
//! It is explicit user intent, like the per-row buttons, so it is not a second
//! reclamation owner in the sense of ADR 0035.

use super::*;
use crate::engine::thread_events::MessageOrigin;

/// What one pass reclaimed.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct RecommendedCleanupOutcome {
    /// Finished worktrees removed.
    pub removed_count: u32,
    /// Worktrees whose build artifacts were stripped.
    pub cleaned_count: u32,
    pub freed_bytes: u64,
}

/// What the pass does to one worktree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RecommendedAction {
    Remove,
    StripArtifacts,
}

/// Run the recommended cleanup over every `thread-<short>` worktree.
///
/// Every check runs right before its worktree is touched, and liveness again
/// after the slow ones. A failure on one worktree is logged and skipped.
pub(crate) async fn run_recommended_cleanup(
    pool: &PgPool,
    bus: &EventBus,
    workspace_root: &Path,
    active_threads: &dyn ActiveThreads,
    actor: Option<MessageOrigin>,
) -> RecommendedCleanupOutcome {
    let dir = worktrees_dir(workspace_root);
    let entries = match std::fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(e) => {
            log!(
                "[WorktreeCleanup] recommended: cannot read worktrees dir {}: {}",
                dir.display(),
                e
            );
            return RecommendedCleanupOutcome::default();
        }
    };
    let changes = crate::core::changes_projection::ChangesProjection::new(pool.clone());
    let mut outcome = RecommendedCleanupOutcome::default();
    for entry in entries.flatten() {
        let worktree = entry.path();
        let Some(short) = worktree
            .file_name()
            .and_then(|n| n.to_str())
            .and_then(parse_thread_short)
        else {
            continue;
        };
        if !is_safe_subpath(&dir, &worktree) {
            continue;
        }
        let ShortThreadLookup::Found(thread_id) = lookup_thread_by_short(pool, &short).await else {
            continue;
        };
        let Some(action) =
            recommended_action(pool, &changes, active_threads, thread_id, &worktree).await
        else {
            continue;
        };
        // The checks above can wait on git and the database.
        if active_threads.is_active(thread_id).await {
            continue;
        }
        let (tier, freed_bytes, branch_deleted) = match action {
            RecommendedAction::Remove => {
                let Some(removed) = remove_worktree_and_optionally_delete_branch(
                    &worktree,
                    None,
                    BranchDisposal::WhenMerged,
                )
                .await
                else {
                    continue;
                };
                outcome.removed_count += 1;
                (0, removed.freed_bytes, removed.branch_deleted)
            }
            RecommendedAction::StripArtifacts => {
                let Some(freed) = prune_build_artifacts(&worktree).await else {
                    continue;
                };
                outcome.cleaned_count += 1;
                (1, freed, false)
            }
        };
        outcome.freed_bytes = outcome.freed_bytes.saturating_add(freed_bytes);
        log!(
            "[WorktreeCleanup] recommended: tier-{} freed {} bytes for thread {}",
            tier,
            freed_bytes,
            thread_id
        );
        bus.emit_or_log(
            BusEvent::Thread {
                thread_id,
                event: ThreadEvent::WorktreeCleaned {
                    tier,
                    freed_bytes,
                    branch_deleted,
                },
                meta: EventMeta::with_actor(actor.clone()),
            },
            "[WorktreeCleanup] recommended WorktreeCleaned",
        )
        .await;
    }
    outcome
}

/// `None` leaves the worktree alone: live, pinned, stranded, or a pin lookup
/// that could not answer.
async fn recommended_action(
    pool: &PgPool,
    changes: &crate::core::changes_projection::ChangesProjection,
    active_threads: &dyn ActiveThreads,
    thread_id: Uuid,
    worktree: &Path,
) -> Option<RecommendedAction> {
    if active_threads.is_active(thread_id).await || worktree_git_admin_missing(worktree) {
        return None;
    }
    match thread_is_saved(pool, thread_id).await {
        Ok(false) => {}
        Ok(true) => return None,
        Err(e) => {
            log!(
                "[WorktreeCleanup] recommended: is_saved lookup failed for thread {}: {}; skipping",
                thread_id,
                e
            );
            return None;
        }
    }
    if is_finished_worktree(pool, changes, thread_id, worktree).await {
        Some(RecommendedAction::Remove)
    } else {
        Some(RecommendedAction::StripArtifacts)
    }
}
