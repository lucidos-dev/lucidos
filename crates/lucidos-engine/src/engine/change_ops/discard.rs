use super::*;
use crate::engine::git_ops::{
    auto_commit_safe_files_if_dirty, find_worktree_for_branch, is_merge_of_branch_into_main,
};

impl LucidosEngine {
    pub async fn is_external_repo_thread(&self, thread_id: Uuid) -> Result<bool, sqlx::Error> {
        sqlx::query_scalar::<_, bool>(
            "SELECT coding_agent_is_external_repo FROM thread_summaries WHERE thread_id = $1",
        )
        .bind(thread_id)
        .fetch_optional(&self.pool)
        .await
        .map(|opt| opt.unwrap_or(false))
    }

    /// Discard all pending changes for a thread. `actor` flows into the
    /// resulting `ChangeDiscarded` events so the chip reads "You" rather than
    /// the engine fallback.
    pub async fn discard_pending_for_thread(&self, thread_id: Uuid, actor: Option<MessageOrigin>) {
        let pending = match self.changes().pending_for_thread(thread_id).await {
            Ok(v) => v,
            Err(e) => {
                log!(
                    "[Changes] discard_pending_for_thread({}): pending_for_thread: {} — \
                     skipping discard (pending changes remain in DB)",
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

    /// Enforce the "a coding-agent thread has at most one pending change at a
    /// time" invariant: discard every pending change for `thread_id` that the
    /// `keep` predicate rejects. `ChangeDiscarded` is event-sourced, so a stale
    /// row closes cleanly (its branch/worktree reset to main) instead of
    /// dangling as `pending` — which the frontend reads as "has pending changes"
    /// (`resolveThreadActions`) and which then suppresses Archive forever. See
    /// docs/plans/2026-07-01-orphaned-pending-change-blocks-archive.md.
    ///
    /// The `keep` predicate is called synchronously per row, so callers scope
    /// exactly what survives: `propose_change` keeps the branch being proposed
    /// (dropping stale OTHER-branch changes), and the apply-time net keeps the
    /// change that just applied.
    pub(crate) async fn discard_pending_for_thread_except(
        &self,
        thread_id: Uuid,
        actor: Option<MessageOrigin>,
        keep: impl Fn(&crate::core::changes::Change) -> bool,
    ) {
        let pending = match self.changes().pending_for_thread(thread_id).await {
            Ok(v) => v,
            Err(e) => {
                log!(
                    "[Changes] discard_pending_for_thread_except({}): pending_for_thread: {} — \
                     skipping reconcile (stale pending changes remain in DB)",
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
                "[Changes] Reconcile: discarding stale pending change {} (branch {}) for thread {} — \
                 thread already has a newer change",
                change.id,
                change.branch_name,
                thread_id
            );
            if let Err(e) = self.discard_change(change.id, actor.clone()).await {
                log!(
                    "[Changes] Reconcile: failed to discard stale change {} for thread {}: {}",
                    change.id,
                    thread_id,
                    e
                );
            }
        }
    }

    /// Apply-time net for the "≤1 pending change per thread" invariant: after a
    /// change applies, discard any OTHER pending change the thread still holds.
    /// `propose_change` is the primary guard (it prevents a second pending change
    /// from ever coexisting); this catches a pre-existing orphan that predates
    /// that guard or reached the thread via a path that bypassed it.
    pub(crate) async fn discard_orphaned_pending_siblings(
        &self,
        thread_id: Uuid,
        keep_change_id: Uuid,
        actor: Option<MessageOrigin>,
    ) {
        self.discard_pending_for_thread_except(thread_id, actor, |c| c.id == keep_change_id)
            .await;
    }

    /// Discard a single pending change.
    ///
    /// Phase 6.3 of the CC resume architecture: Discard preserves the thread's
    /// worktree directory and the branch ref so the thread stays alive and the
    /// next user message resumes the same Claude Code session. The branch's commits
    /// (the ones the user is discarding) are wiped by resetting the worktree
    /// to main HEAD via `reset_worktree_to_main_after_discard`. The branch is
    /// NOT deleted — keeping it lets the same `cc_session_id` resume on the
    /// same branch ref instead of having to recreate everything.
    ///
    /// If the discarded change leaves OTHER pending changes referencing the
    /// same branch (multi-change-on-one-branch case), skip the worktree reset
    /// — the other changes' commits would be wiped along with this one. The
    /// branch and worktree stay as-is, preserving the still-pending work.
    pub async fn discard_change(
        &self,
        change_id: Uuid,
        actor: Option<MessageOrigin>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let change = self
            .changes()
            .get_by_id(change_id)
            .await?
            .ok_or("Change not found")?;
        if change.status == "discarded" {
            // Idempotent: already discarded, return success
            return Ok(());
        }
        if change.status != "pending" {
            return Err(format!("Change is already {}", change.status).into());
        }

        // Mark as discarded FIRST, before touching git — event is the source of truth
        self.event_bus
            .emit_or_log(
                crate::engine::event_bus::BusEvent::Thread {
                    thread_id: change.thread_id.unwrap_or(change_id),
                    event: crate::engine::thread_events::ThreadEvent::ChangeDiscarded {
                        change_id: change_id.to_string(),
                        actor,
                        path: String::new(),
                    },
                    meta: crate::engine::thread_events::EventMeta::NONE,
                },
                "[Changes] ChangeDiscarded",
            )
            .await;

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
            crate::engine::apply_all_driver::DISCARDED_MEMBER_REASON.to_string(),
        ));

        // Other pending changes on the same branch? If so, leave the branch
        // and worktree untouched — wiping the branch back to main would also
        // discard the still-pending work. On DB error, treat as if others
        // exist — preserving the branch is safer than wiping work we can't
        // tell is still referenced.
        let others = self
            .changes()
            .other_pending_for_branch(&change.branch_name, change_id)
            .await
            .unwrap_or_else(|e| {
                log!(
                    "[Changes] discard_change: other_pending_for_branch({}, {}): {} — \
                     keeping branch defensively",
                    change.branch_name,
                    change_id,
                    e
                );
                true
            });
        if others {
            log!(
                "[Changes] Discarded change {} but kept branch {} and worktree — other pending changes reference it",
                change_id,
                change.branch_name
            );
            return Ok(());
        }

        // Phase 6.3: reset the worktree to main HEAD; preserve directory +
        // branch so the next user message resumes the same Claude Code session.
        let repo_root = std::path::PathBuf::from(&change.repo_root);
        if let Some(wt_path) = find_worktree_for_branch(&repo_root, &change.branch_name).await {
            reset_worktree_to_main_after_discard(&wt_path).await?;
            log!(
                "[Changes] Discarded change {}: reset worktree {} on branch {} to main; branch preserved",
                change_id,
                wt_path.display(),
                change.branch_name
            );
        } else {
            // No worktree on disk for this branch (legacy threads, or already
            // cleaned up). Reset the branch ref to main directly so the
            // commits don't linger as a phantom pending state.
            let reset = git_cmd(&["branch", "-f", &change.branch_name, "main"], &repo_root)
                .await
                .map_err(|e| {
                    format!(
                        "git branch -f {} main failed in repo {}: {}",
                        change.branch_name,
                        repo_root.display(),
                        e
                    )
                })?;
            if !reset.status.success() {
                return Err(format!(
                    "git branch -f {} main failed in repo {}: {}",
                    change.branch_name,
                    repo_root.display(),
                    String::from_utf8_lossy(&reset.stderr).trim()
                )
                .into());
            }
            log!(
                "[Changes] Discarded change {}: no worktree on branch {} — reset branch ref to main; branch preserved",
                change_id,
                change.branch_name
            );
        }
        Ok(())
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
        if change.status == "reverted" {
            return Ok("Change already reverted.".to_string());
        }
        if change.status != "applied" {
            return Err(format!(
                "Change is '{}', only applied changes can be reverted",
                change.status
            )
            .into());
        }

        let repo_root = std::path::PathBuf::from(&change.repo_root);

        // Auto-commit safe files (docs) if they're the only dirty files
        if auto_commit_safe_files_if_dirty(&repo_root).await {
            return Err("Cannot revert: the repository has uncommitted changes. Commit or stash them first.".into());
        }

        let result = if let (Some(ref pre_sha), Some(ref post_sha)) =
            (&change.pre_merge_sha, &change.post_merge_sha)
        {
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

    /// Legacy revert: search for merge commit in recent git history,
    /// falling back to branch-ref based revert for fast-forwarded merges.
    async fn revert_legacy(
        &self,
        repo_root: &Path,
        branch_name: &str,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        // Try 1: find a merge commit that merged this branch INTO main.
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

        // Try 2: branch ref still exists — find its commits via merge-base and revert the range
        if let Ok(ref_output) = git_cmd(&["rev-parse", "--verify", branch_name], repo_root).await {
            if ref_output.status.success() {
                let branch_sha = String::from_utf8_lossy(&ref_output.stdout)
                    .trim()
                    .to_string();
                let base_output = git_cmd(&["merge-base", "HEAD", branch_name], repo_root)
                    .await
                    .map_err(|e| format!("Failed to find merge-base: {}", e))?;
                if base_output.status.success() {
                    let base_sha = String::from_utf8_lossy(&base_output.stdout)
                        .trim()
                        .to_string();

                    // If the branch tip is an ancestor of HEAD, its commits were fast-forwarded into main
                    let is_ancestor = git_cmd(
                        &["merge-base", "--is-ancestor", &branch_sha, "HEAD"],
                        repo_root,
                    )
                    .await
                    .map(|o| o.status.success())
                    .unwrap_or(false);

                    if is_ancestor && base_sha != branch_sha {
                        return self
                            .revert_with_shas(repo_root, &base_sha, &branch_sha, branch_name)
                            .await;
                    }
                }
            }
        }

        Err(format!(
            "Could not find commits for branch '{}'. \
             The branch may have been deleted and this change was applied before revert tracking was added.",
            branch_name
        ).into())
    }
}
