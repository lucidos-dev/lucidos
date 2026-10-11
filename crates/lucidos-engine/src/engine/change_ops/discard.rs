use super::*;
use crate::engine::git_ops::{
    auto_commit_safe_files_if_dirty, find_worktree_for_branch, is_merge_of_branch_into_main,
    WorktreeLookup,
};

/// Clear a discarded change's branch state, given what git said about it.
///
/// A worktree on disk is reset to main HEAD. The directory and the branch stay,
/// so the next user message resumes the same Claude Code session. With no
/// worktree the branch ref is reset directly, so the commits do not linger as a
/// phantom pending state.
///
/// The lookup is a parameter rather than an inner call so a test can drive the
/// `Unknown` arm against a real repo. Unknown skips: `git branch -f` moves a
/// ref with no old-value guard, over a branch that may hold the only copy of
/// the work. The change is still discarded, and the branch is left ahead of
/// main with nothing pointing at it, which is inert.
pub(crate) async fn settle_discarded_branch(
    repo_root: &Path,
    branch_name: &str,
    change_id: Uuid,
    lookup: WorktreeLookup,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    match lookup {
        // The branch is checked out in the repo's primary checkout, where the
        // user works. It is never a thread's tree, so a reset there would wipe
        // their uncommitted and untracked files.
        WorktreeLookup::Found(wt_path) if !is_linked_worktree(&wt_path) => {
            log!(
                "[Changes] Discarded change {}: branch {} is checked out in {}, which is not a \
                 linked worktree; leaving that checkout and the branch alone",
                change_id,
                branch_name,
                wt_path.display()
            );
        }
        WorktreeLookup::Found(wt_path) => {
            reset_worktree_to_main_after_discard(&wt_path).await?;
            log!(
                "[Changes] Discarded change {}: reset worktree {} on branch {} to main; branch preserved",
                change_id,
                wt_path.display(),
                branch_name
            );
        }
        WorktreeLookup::Unknown => {
            log!(
                "[Changes] Discarded change {}: git worktree list gave no answer for branch {}; \
                 leaving the branch ref alone rather than moving it on a guess",
                change_id,
                branch_name
            );
        }
        WorktreeLookup::NotFound => {
            let reset = git_cmd(&["branch", "-f", branch_name, "main"], repo_root)
                .await
                .map_err(|e| {
                    format!(
                        "git branch -f {} main failed in repo {}: {}",
                        branch_name,
                        repo_root.display(),
                        e
                    )
                })?;
            if !reset.status.success() {
                return Err(format!(
                    "git branch -f {} main failed in repo {}: {}",
                    branch_name,
                    repo_root.display(),
                    String::from_utf8_lossy(&reset.stderr).trim()
                )
                .into());
            }
            log!(
                "[Changes] Discarded change {}: no worktree on branch {}; reset branch ref to main, branch preserved",
                change_id,
                branch_name
            );
        }
    }
    Ok(())
}

/// Whether `path` is a linked worktree, whose `.git` is a file pointing into
/// the main repo. The primary checkout has a `.git` directory instead. An
/// unreadable `.git` answers `false`, the side that resets nothing.
fn is_linked_worktree(path: &Path) -> bool {
    std::fs::symlink_metadata(path.join(".git")).is_ok_and(|meta| meta.is_file())
}

impl LucidosEngine {
    pub async fn is_external_repo_thread(&self, thread_id: Uuid) -> Result<bool, sqlx::Error> {
        super::thread_is_external_repo(&self.pool, thread_id).await
    }

    /// Discard every open change (pending or set aside) for a thread. `actor`
    /// flows into the resulting `ChangeDiscarded` events so the chip reads
    /// "You" rather than the engine fallback.
    pub async fn discard_open_changes_for_thread(
        &self,
        thread_id: Uuid,
        actor: Option<MessageOrigin>,
    ) {
        // Before the loop, so a thread with no change yet still loses its tasks.
        self.abandon_background_tasks(thread_id, "Discard").await;
        let pending = match self.changes().open_for_thread(thread_id).await {
            Ok(v) => v,
            Err(e) => {
                log!(
                    "[Changes] discard_open_changes_for_thread({}): open_for_thread: {}; \
                     skipping discard (open changes remain in DB)",
                    thread_id,
                    e
                );
                return;
            }
        };
        for change in &pending {
            if let Err(e) = self.discard_change(change.id, actor.clone()).await {
                log!(
                    "[Changes] Failed to discard change {} for thread {}: {}",
                    change.id,
                    thread_id,
                    e
                );
            }
        }
    }

    /// Enforce the "a coding-agent thread has at most one open change at a
    /// time" invariant: discard every open change (pending or set aside) for
    /// `thread_id` that the `keep` predicate rejects. `ChangeDiscarded` is event-sourced, so a stale
    /// row closes cleanly (its branch/worktree reset to main) instead of
    /// dangling as `pending` — which the frontend reads as "has pending changes"
    /// (`resolveThreadActions`) and which then suppresses Archive forever. See
    /// docs/plans/2026-07-01-orphaned-pending-change-blocks-archive.md.
    ///
    /// The `keep` predicate is called synchronously per row, so callers scope
    /// exactly what survives: `propose_change` keeps the branch being proposed
    /// (dropping stale OTHER-branch changes), and the apply-time net keeps the
    /// change that just applied.
    pub(crate) async fn discard_open_changes_for_thread_except(
        &self,
        thread_id: Uuid,
        actor: Option<MessageOrigin>,
        keep: impl Fn(&crate::core::changes::Change) -> bool,
    ) {
        let pending = match self.changes().open_for_thread(thread_id).await {
            Ok(v) => v,
            Err(e) => {
                log!(
                    "[Changes] discard_open_changes_for_thread_except({}): open_for_thread: {}; \
                     skipping reconcile (stale open changes remain in DB)",
                    thread_id,
                    e
                );
                return;
            }
        };
        for change in &pending {
            if keep(change) {
                continue;
            }
            log!(
                "[Changes] Reconcile: discarding stale open change {} (branch {}) for thread {}: \
                 thread already has a newer change",
                change.id,
                change.branch_name,
                thread_id
            );
            if let Err(e) = self.discard_change_quietly(change.id, actor.clone()).await {
                log!(
                    "[Changes] Reconcile: failed to discard stale change {} for thread {}: {}",
                    change.id,
                    thread_id,
                    e
                );
            }
        }
    }

    /// Apply-time net for the "≤1 open change per thread" invariant: after a
    /// change applies, discard any OTHER open change the thread still holds.
    /// `propose_change` is the primary guard (it prevents a second pending change
    /// from ever coexisting); this catches a pre-existing orphan that predates
    /// that guard or reached the thread via a path that bypassed it.
    pub(crate) async fn discard_orphaned_pending_siblings(
        &self,
        thread_id: Uuid,
        keep_change_id: Uuid,
        actor: Option<MessageOrigin>,
    ) {
        self.discard_open_changes_for_thread_except(thread_id, actor, |c| c.id == keep_change_id)
            .await;
    }

    /// Discard a single open change (pending or set aside) because the user
    /// asked to.
    ///
    /// Also settles the thread's parent when the thread still owed it a card
    /// (ADR 0252). The engine's own reconciles call
    /// [`Self::discard_change_quietly`] instead: no user discarded anything
    /// there, and a parent must not be told so.
    pub async fn discard_change(
        &self,
        change_id: Uuid,
        actor: Option<MessageOrigin>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let thread_id = self
            .changes()
            .get_by_id(change_id)
            .await?
            .and_then(|change| change.thread_id);
        if let Some(thread_id) = thread_id {
            // Still running in the worktree the discard is about to reset.
            self.abandon_background_tasks(thread_id, "Discard").await;
        }
        self.discard_change_quietly(change_id, actor).await?;
        if let Some(thread_id) = thread_id {
            self.event_bus
                .settle_child(thread_id, crate::engine::event_bus::ChildSettle::Discarded)
                .await;
        }
        Ok(())
    }

    /// Discard a single open change, and tell no parent.
    ///
    /// Phase 6.3 of the CC resume architecture: Discard preserves the thread's
    /// worktree directory and the branch ref so the thread stays alive and the
    /// next user message resumes the same Claude Code session. The branch's commits
    /// (the ones the user is discarding) are wiped by resetting the worktree
    /// to main HEAD via `reset_worktree_to_main_after_discard`. The branch is
    /// NOT deleted — keeping it lets the same `cc_session_id` resume on the
    /// same branch ref instead of having to recreate everything.
    ///
    /// If the discarded change leaves OTHER open changes referencing the same
    /// branch (multi-change-on-one-branch case), skip the worktree reset: the
    /// other changes' commits would be wiped along with this one. The branch
    /// and worktree stay as-is, preserving the still-open work.
    pub(crate) async fn discard_change_quietly(
        &self,
        change_id: Uuid,
        actor: Option<MessageOrigin>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let change = self
            .changes()
            .get_by_id(change_id)
            .await?
            .ok_or("Change not found")?;
        match change.status() {
            ChangeStatus::Pending | ChangeStatus::SetAside => {}
            // Idempotent: already discarded, return success
            ChangeStatus::Discarded => return Ok(()),
            status @ (ChangeStatus::Applied | ChangeStatus::Reverted | ChangeStatus::Withdrawn) => {
                return Err(format!("Change is already {status}").into());
            }
        }

        // Mark as discarded FIRST, before touching git: the event is the source
        // of truth. The emit is AWAITED for its result rather than
        // fire-and-forget, because everything below this line is destructive
        // (`git reset --hard main`, `git clean -fd`, `git branch -f <branch>
        // main`). A dropped emit used to destroy the branch's commits and every
        // untracked file while the projection still read `pending` and the
        // caller was told the discard succeeded. Failing here instead leaves the
        // change pending and the work intact, which is the recoverable
        // direction.
        self.event_bus
            .emit(crate::engine::event_bus::BusEvent::Thread {
                thread_id: change.thread_id.unwrap_or(change_id),
                event: crate::engine::thread_events::ThreadEvent::ChangeDiscarded {
                    change_id: change_id.to_string(),
                    actor,
                    path: String::new(),
                },
                meta: crate::engine::thread_events::EventMeta::NONE,
            })
            .await
            .map_err(|e| {
                log!(
                    "[Changes] ChangeDiscarded emit failed for {}: {}. Leaving the branch intact.",
                    change_id,
                    e
                );
                format!("could not record the discard of change {change_id}: {e}")
            })?;

        // Feed the Apply-All driver: if this discarded change is a live batch
        // member, mark it terminal so the batch advances instead of stalling.
        // Symmetric to `emit_change_applied`'s `Applied` notify — a no-op for
        // non-members. Without it, a member discarded mid-batch (e.g. the
        // "≤1 pending change per thread" reconcile dropping a sibling that is
        // also a batch member) would leave the driver spawning `apply_change`
        // on a now-`discarded` row, which returns `Err` with no terminal event —
        // the batch never completes and the "Applying changes…" toast sticks.
        self.notify_apply_all(crate::engine::apply_all_driver::ApplyAllDriveMsg::Failed(
            change_id,
            crate::engine::apply_all_driver::WITHDRAWN_MEMBER_REASON.to_string(),
        ));

        // Other open changes on the same branch? If so, leave the branch and
        // worktree untouched: wiping the branch back to main would also discard
        // the still-open work. On DB error, treat as if others exist, since
        // preserving the branch is safer than wiping work that may be referenced.
        let others = self
            .changes()
            .other_open_for_branch(&change.branch_name, change_id)
            .await
            .unwrap_or_else(|e| {
                log!(
                    "[Changes] discard_change: other_open_for_branch({}, {}): {}; \
                     keeping branch defensively",
                    change.branch_name,
                    change_id,
                    e
                );
                true
            });
        if others {
            log!(
                "[Changes] Discarded change {} but kept branch {} and worktree: other open changes reference it",
                change_id,
                change.branch_name
            );
            return Ok(());
        }

        let repo_root = std::path::PathBuf::from(&change.repo_root);
        let lookup = find_worktree_for_branch(&repo_root, &change.branch_name).await;
        settle_discarded_branch(&repo_root, &change.branch_name, change_id, lookup).await
    }

    /// Revert a previously applied change by reverting its commits.
    /// Uses stored pre/post merge SHAs when available, falls back to searching merge history.
    pub async fn revert_change(
        &self,
        change_id: Uuid,
        actor: Option<MessageOrigin>,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        let change = self
            .changes()
            .get_by_id(change_id)
            .await?
            .ok_or("Change not found")?;
        let shas = match &change.state {
            ChangeStatusData::Applied(shas) => shas,
            ChangeStatusData::Reverted(_) => return Ok("Change already reverted.".to_string()),
            ChangeStatusData::Pending { .. }
            | ChangeStatusData::SetAside
            | ChangeStatusData::Discarded
            | ChangeStatusData::Withdrawn => {
                return Err(format!(
                    "Change is '{}', only applied changes can be reverted",
                    change.status()
                )
                .into());
            }
        };

        let repo_root = std::path::PathBuf::from(&change.repo_root);

        // Auto-commit safe files (docs) if they're the only dirty files
        if auto_commit_safe_files_if_dirty(&repo_root).await {
            return Err("Cannot revert: the repository has uncommitted changes. Commit or stash them first.".into());
        }

        let result = if let (Some(pre_sha), Some(post_sha)) = (&shas.pre, &shas.post) {
            self.revert_with_shas(&repo_root, pre_sha, post_sha, &change.branch_name)
                .await
        } else {
            self.revert_legacy(&repo_root, &change.branch_name).await
        };

        match result {
            Ok(()) => {
                log!(
                    "[Changes] Reverted change {} (branch {})",
                    change_id,
                    change.branch_name
                );
                self.event_bus
                    .emit_or_log(
                        crate::engine::event_bus::BusEvent::Thread {
                            thread_id: change.thread_id.unwrap_or(change_id),
                            event: crate::engine::thread_events::ThreadEvent::ChangeReverted {
                                change_id: change_id.to_string(),
                                actor,
                                path: String::new(),
                            },
                            meta: crate::engine::thread_events::EventMeta::NONE,
                        },
                        "[Changes] ChangeReverted",
                    )
                    .await;
                Ok("Change reverted.".to_string())
            }
            Err(e) => Err(e),
        }
    }

    /// Revert using stored pre/post merge SHAs.
    async fn revert_with_shas(
        &self,
        repo_root: &Path,
        pre_sha: &str,
        post_sha: &str,
        branch_name: &str,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        revert_with_shas(repo_root, pre_sha, post_sha, branch_name).await
    }

    /// Legacy revert: find the merge commit in recent git history. A change
    /// that was fast-forwarded before SHA tracking left no record of main's
    /// pre-merge commit, so it cannot be reverted and errors below.
    async fn revert_legacy(
        &self,
        repo_root: &Path,
        branch_name: &str,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        // Find a merge commit that merged this branch INTO main.
        // Must match "Merge branch 'feature'" or "Merge feature:" patterns,
        // NOT "Merge branch 'main' into feature" (which is the reverse direction).
        let log_output = git_cmd(&["log", "--merges", "--oneline", "-50"], repo_root)
            .await
            .map_err(|e| format!("Failed to read git log: {}", e))?;
        if log_output.status.success() {
            let log_text = String::from_utf8_lossy(&log_output.stdout);
            if let Some(merge_hash) = log_text
                .lines()
                .find(|line| is_merge_of_branch_into_main(line, branch_name))
                .and_then(|line| line.split_whitespace().next())
            {
                return match git_cmd(&["revert", merge_hash, "-m", "1", "--no-edit"], repo_root)
                    .await
                {
                    Ok(o) if o.status.success() => Ok(()),
                    Ok(o) => {
                        let stderr = String::from_utf8_lossy(&o.stderr).trim().to_string();
                        let _ = git_cmd(&["revert", "--abort"], repo_root).await;
                        Err(format!("Revert failed (conflicts): {}", stderr).into())
                    }
                    Err(e) => Err(format!("Revert error: {}", e).into()),
                };
            }
        }

        Err(format!(
            "Could not find commits for branch '{}'. \
             The branch may have been deleted and this change was applied before revert tracking was added.",
            branch_name
        ).into())
    }
}
