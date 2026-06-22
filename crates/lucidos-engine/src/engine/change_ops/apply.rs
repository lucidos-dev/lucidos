use super::*;
use crate::engine::git_ops::{
    auto_commit_safe_files_if_dirty, auto_commit_worktree, catchup_and_ff_to_main, commits_in_range,
    ff_main_to, files_have_client_update, find_branch_merge_in_main, find_worktree_for_branch,
    has_branch_commits, is_harden_marker_present, push_main_in_background,
    recover_no_commits_branch, worktree_add, worktrees_dir, NoCommitsRecovery, MERGE_MUTEX,
};
use crate::engine::agent_session::InPlaceMergeStart;
use crate::engine::{ApplyResult, ApplyStatus};

impl LucidosEngine {
    /// Mark a change applied without merging anything: delete the branch ref,
    /// emit `ChangeApplied`, broadcast the projection, return `ApplyResult::noop`.
    /// Shared by the two `apply_change` recovery paths that resolve to a no-op
    /// (`LegitimateNoOp`, `AlreadyApplied`).
    async fn finalize_change_as_noop(
        &self,
        change: &crate::core::changes::Change,
        change_id: Uuid,
        repo_root: &Path,
        actor: Option<MessageOrigin>,
        result_message: &'static str,
    ) -> ApplyResult {
        let _ = git_cmd(&["branch", "-D", &change.branch_name], repo_root).await;
        self.emit_change_applied(
            change.thread_id.unwrap_or(change_id),
            change_id,
            false,
            false,
            Vec::new(),
            change.thread_title.clone(),
            actor,
            None,
            None,
        )
        .await;
        self.broadcast_changes_updated().await;
        ApplyResult::noop(
            change_id,
            change.thread_id,
            change.files.len(),
            result_message,
        )
    }


    /// Apply a single pending change: merge its branch into main.
    /// `actor` identifies who initiated the apply. HTTP callers construct via
    /// `api::actor::build_message_origin`; engine-internal callers pass `None`.
    pub async fn apply_change(
        self: &Arc<Self>,
        change_id: Uuid,
        actor: Option<MessageOrigin>,
    ) -> Result<ApplyResult, Box<dyn std::error::Error + Send + Sync>> {
        let change = self
            .changes()
            .get_by_id(change_id)
            .await?
            .ok_or("Change not found")?;

        // Resolve coding-agent-kind context up front so every branch below
        // (hardening gate, restart gating, post-success refresh) reads the
        // same value. App threads:
        //   - skip the /harden gate (apps own their hardening),
        //   - force requires_restart=false (data-tree changes never
        //     restart the engine),
        //   - emit AppUiRefreshRequested when iframe-bundled files change
        //     so open iframes reload.
        let kind_ctx = load_apply_kind_context(&self.pool, change.thread_id).await;
        let effective_requires_restart = change.requires_restart && !kind_ctx.is_app();

        if change.status == "applied" {
            // Echo the stored merge SHAs so a re-apply still gives callers
            // a verifiable reference to the original merge. Use the
            // effective restart flag here too — an app re-apply must never
            // claim it needs a restart even if a stale row says otherwise.
            return Ok(ApplyResult {
                status: ApplyStatus::Noop,
                change_id,
                thread_id: change.thread_id,
                restart_required: effective_requires_restart,
                message: "Change already applied.".to_string(),
                applied_commit: change.post_merge_sha.clone(),
                previous_commit: change.pre_merge_sha.clone(),
                commits_applied: change.commits.len(),
                files_changed: change.files.len(),
                ..ApplyResult::default()
            });
        }
        if change.status != "pending" {
            return Err(format!("Change is already {}", change.status).into());
        }

        // Idempotency fast-path: when the branch ref is gone, check if it was
        // already merged into main out-of-band (e.g. by an agentic loop calling
        // `git merge` directly). Only kicks in when the live branch is missing,
        // so live-branch flows (Tier 1/2/3) keep ownership of the merge.
        {
            let repo_root = std::path::PathBuf::from(&change.repo_root);
            let branch_exists =
                git_cmd(&["rev-parse", "--verify", &change.branch_name], &repo_root)
                    .await
                    .map(|o| o.status.success())
                    .unwrap_or(false);
            if !branch_exists {
                if let Some((pre_sha, post_sha)) =
                    find_branch_merge_in_main(&repo_root, &change.branch_name).await
                {
                    log!("[Changes] Branch {} already merged into main as {} — marking change {} applied",
                        change.branch_name,
                        &post_sha[..post_sha.floor_char_boundary(8)],
                        change_id);
                    let commits = commits_in_range(&repo_root, &pre_sha, &post_sha).await;
                    // Phase 6.2: branch was already deleted out-of-band, but
                    // the worktree may still be on disk. Keep it — the thread
                    // is still alive and the next user message will reuse it.
                    // We can't reset to main here because the branch ref is
                    // gone, so the worktree is effectively detached at the
                    // pre-merge SHA; the spawn dispatcher handles that case.
                    let thread_id = change.thread_id.unwrap_or(change_id);
                    self.emit_change_applied(
                        thread_id,
                        change_id,
                        effective_requires_restart,
                        files_have_client_update(&change.files),
                        commits.clone(),
                        change.thread_title.clone(),
                        actor.clone(),
                        Some(pre_sha.clone()),
                        Some(post_sha.clone()),
                    )
                    .await;
                    self.maybe_emit_app_ui_refresh(&kind_ctx, &change.files, actor.as_ref()).await;
                    self.emit_entity_events_for_change_apply(
                        &change.files,
                        Some(&pre_sha),
                        Some(&post_sha),
                        actor.clone(),
                    )
                    .await;
                    self.broadcast_changes_updated().await;
                    return Ok(ApplyResult::applied_with_merge(
                        change_id,
                        change.thread_id,
                        effective_requires_restart,
                        pre_sha,
                        post_sha,
                        &commits,
                        change.files.len(),
                    ));
                }
            }
        }

        // Clear stale merge worktree metadata if the directory no longer exists
        if let Some(ref wt) = change.merge_worktree_path {
            if !std::path::Path::new(wt).exists() {
                log!(
                    "[Changes] Stale merge worktree {} no longer exists, clearing metadata",
                    wt
                );
                if let Some(tid) = change.thread_id {
                    self.emit_merge_resolution_cleared(
                        tid,
                        change_id,
                        "[Changes] MergeResolutionCleared",
                    )
                    .await;
                }
            }
        }

        // Implementation-plan floor (Lucidos-source only). A gate-satisfying
        // marker — an APPROVED plan (`planned`), or an explicit
        // `lucidos planned mark --simple` acknowledgment — MUST exist before a
        // Lucidos-source change can apply. A `proposed` (awaiting-approval)
        // marker does NOT satisfy the floor: the human hasn't approved the plan
        // yet. The Claude-Code `cc-plan-gate` PreToolUse hook and the prompt
        // rule are meant to make a non-satisfying branch unreachable; this is
        // the hard backstop (and the ONLY enforcement for Codex, which has no
        // PreToolUse hook). App and external-repo changes are exempt — neither
        // uses the `docs/plans/` convention or the marker. Per the resolved
        // design decision: if the marker is Missing/Proposed here, refuse the
        // apply (no auto-recovery).
        // App and external-repo changes are exempt — neither uses the
        // `docs/plans/` convention or the marker, so don't even query.
        if kind_ctx.is_lucidos_source() {
            let plan_state = self
                .plan_marker_state(
                    &std::path::PathBuf::from(&change.repo_root),
                    &change.branch_name,
                )
                .await;
            if !plan_state.satisfies_gate() {
                let msg = if plan_state.is_present() {
                    // Present but not satisfying => Proposed (awaiting approval).
                    "The implementation plan on this branch is awaiting approval. The user must \
                     approve the plan, after which the coding agent runs `lucidos planned approve` \
                     to unblock implementation. Approve the plan, then apply."
                } else {
                    "No implementation-plan marker on this branch. Before applying, the coding \
                     agent must run the `implementation-plan` skill (records a plan for the user to \
                     approve) or `lucidos planned mark --simple \"<reason>\"` (acknowledges a local \
                     fix). Re-run the session to set the marker, then apply."
                };
                log!(
                    "[Changes] Apply blocked — plan marker not satisfying ({:?}) for branch {} (change {})",
                    plan_state,
                    change.branch_name,
                    change_id
                );
                self.emit_apply_failed(
                    change.thread_id.unwrap_or(change_id),
                    change_id,
                    msg,
                    actor.clone(),
                )
                .await;
                return Err(msg.into());
            }
        }

        // App threads skip the /harden gate entirely — apps own their own
        // hardening (per the app's `.claude/commands/harden.md` if it ships
        // one); the engine never invokes `/harden` for `data/apps/<id>/`
        // changes and the changes.hardened flag is ignored for them.
        let marker_hardened = if kind_ctx.is_app() {
            true
        } else {
            is_harden_marker_present(
                &self.pool,
                &std::path::PathBuf::from(&change.repo_root),
                &change.branch_name,
            )
            .await
        };
        if marker_hardened && !change.hardened && !kind_ctx.is_app() {
            log!(
                "[Changes] Marker found but DB not set — marking change {} as hardened",
                change_id
            );
            if let Some(tid) = change.thread_id {
                self.emit_change_hardened(tid, change_id, "[Changes] ChangeHardened")
                    .await;
            }
        }
        if !change.hardened && !marker_hardened {
            let thread_id = match change.thread_id {
                Some(tid) => tid,
                None => {
                    let msg = "Change has no associated thread — cannot run hardening automatically. Discard and re-create with a Claude Code session.";
                    self.emit_apply_failed(change_id, change_id, msg, actor.clone())
                        .await;
                    return Err(msg.into());
                }
            };

            // Live session: route harden through it (emit + send). If send fails
            // the boundary is already up — fall through to the worktree spawn
            // without re-emitting. No live session: emit boundary now.
            if let Some(session) = CodingAgentChangeOps::live_session_info(self.as_ref(), thread_id).await {
                match self
                    .request_hardening_in_session(thread_id, &session.msg_tx)
                    .await
                {
                    Ok(()) => {
                        log!(
                            "[Changes] Reusing existing session on thread {} for hardening",
                            thread_id
                        );
                        return Ok(ApplyResult::hardening(
                            change_id,
                            thread_id,
                            change.files.len(),
                            "Hardening sent to existing session — please wait.",
                        ));
                    }
                    Err(e) => {
                        log!(
                            "[Changes] Live session on thread {} did not accept harden message ({}); falling back to fresh worktree",
                            thread_id,
                            e
                        );
                    }
                }
            } else {
                self.emit_missing_hardening_detected(thread_id).await;
            }

            let repo_root = std::path::PathBuf::from(&change.repo_root);

            // Reuse an existing CC worktree on this branch if one's still on
            // disk (the producing Claude Code session has exited, but its worktree and
            // branch lock survive). Without this, `git worktree add` below
            // would fail with "branch already used by worktree" — which is
            // what broke nightly trigger applies (orchestrator POSTs apply
            // immediately after CC exits, before any cleanup pass).
            let wt_path = if let Some(existing) =
                find_worktree_for_branch(&repo_root, &change.branch_name).await
            {
                log!(
                    "[Changes] Reusing existing worktree {} for hardening of branch {}",
                    existing.display(),
                    change.branch_name
                );
                existing
            } else {
                let wt_path = worktrees_dir(self.workspace_path())
                    .join(format!("harden-{}", change_id.as_simple()));
                let wt_str = wt_path.to_string_lossy().into_owned();
                let _ = git_cmd(&["worktree", "remove", "--force", &wt_str], &repo_root).await;
                match worktree_add(&repo_root, &wt_path, &[&change.branch_name]).await {
                    Ok(o) if o.status.success() => {}
                    Ok(o) => {
                        let msg = format!(
                            "Failed to create worktree for hardening: {}",
                            String::from_utf8_lossy(&o.stderr).trim()
                        );
                        self.emit_apply_failed(thread_id, change_id, &msg, actor.clone())
                            .await;
                        return Err(msg.into());
                    }
                    Err(e) => {
                        let msg = format!("Failed to create worktree for hardening: {}", e);
                        self.emit_apply_failed(thread_id, change_id, &msg, actor.clone())
                            .await;
                        return Err(msg.into());
                    }
                }
                wt_path
            };

            log!(
                "[Changes] Not hardened — spawning hardening recovery for change {} (thread {})",
                change_id,
                thread_id
            );
            CodingAgentChangeOps::spawn_hardening(
                self.as_ref(),
                thread_id,
                wt_path,
                change.branch_name.clone(),
                Some(change_id),
                actor.clone(),
            );

            return Ok(ApplyResult::hardening(
                change_id,
                thread_id,
                change.files.len(),
                "Hardening started — change will be applied after hardening completes.",
            ));
        }

        let repo_root = std::path::PathBuf::from(&change.repo_root);

        // Validate branch exists — don't auto-discard, let the user decide
        let branch_exists = git_cmd(&["rev-parse", "--verify", &change.branch_name], &repo_root)
            .await
            .map(|o| o.status.success())
            .unwrap_or(false);
        if !branch_exists {
            let msg = format!(
                "Branch '{}' no longer exists. The change may need to be discarded manually.",
                change.branch_name
            );
            self.emit_apply_failed(
                change.thread_id.unwrap_or(change_id),
                change_id,
                &msg,
                actor.clone(),
            )
            .await;
            return Err(msg.into());
        }

        // When the branch ref looks empty, the worktree may still hold uncommitted
        // CC work (multi-draft compose, never auto-committed). Rescue it before
        // taking any destructive action; refuse to silently apply when files are
        // declared but the branch has nothing to merge.
        if !has_branch_commits(&repo_root, &change.branch_name).await {
            match recover_no_commits_branch(&repo_root, &change.branch_name, &change.files).await {
                Ok(NoCommitsRecovery::AutoCommitted) => {
                    log!(
                        "[Changes] Auto-committed worktree for branch {} — proceeding with merge",
                        change.branch_name
                    );
                }
                Ok(NoCommitsRecovery::LegitimateNoOp) => {
                    return Ok(self
                        .finalize_change_as_noop(
                            &change,
                            change_id,
                            &repo_root,
                            actor.clone(),
                            "Change applied (no commits to merge).",
                        )
                        .await);
                }
                Ok(NoCommitsRecovery::AlreadyApplied) => {
                    log!(
                        "[Changes] Branch {} already merged into main — marking change {} as applied (no-op)",
                        change.branch_name,
                        change_id
                    );
                    return Ok(self
                        .finalize_change_as_noop(
                            &change,
                            change_id,
                            &repo_root,
                            actor.clone(),
                            "Change already present on main — marked applied.",
                        )
                        .await);
                }
                Err(e) => {
                    let msg = e.to_string();
                    log!(
                        "[Changes] Refusing silent-apply for change {}: {}",
                        change_id,
                        msg
                    );
                    self.emit_apply_failed(
                        change.thread_id.unwrap_or(change_id),
                        change_id,
                        &msg,
                        actor.clone(),
                    )
                    .await;
                    return Err(e);
                }
            }
        }

        // Auto-commit safe files (docs) if they're the only dirty files, then
        // reject if the workspace tree still has genuine uncommitted changes.
        //
        // MERGE_MUTEX is held alongside workspace_repo_lock here: a concurrent
        // apply's merge advances `refs/heads/main` via `ff_main_to`'s
        // `update-ref` and only afterwards resets the working tree via
        // `checkout -f main`. Between those two git calls the tree transiently
        // looks dirty (`main` moved, the working tree hasn't caught up). Without
        // MERGE_MUTEX this gate would observe that mid-merge window and reject a
        // perfectly valid concurrent apply with a spurious "uncommitted changes"
        // error (regression caught by app_coding_agent_concurrent_apply). Taking
        // MERGE_MUTEX guarantees the gate only ever sees a settled tree — the
        // other apply's merge has either not started or finished its checkout.
        // Lock order is always MERGE_MUTEX → workspace_repo_lock; no other site
        // acquires both (merges take only MERGE_MUTEX, data writes take only
        // workspace_repo_lock), so this can't deadlock.
        //
        // Both guards drop at the end of this block so neither is held across a
        // tier's Claude Code subprocess await — those merges happen in separate
        // worktrees and would otherwise block every data API write for minutes.
        {
            let _merge_guard = MERGE_MUTEX.lock().await;
            let _repo_guard = self.lock_workspace_repo().await;
            if auto_commit_safe_files_if_dirty(&repo_root).await {
                let msg = "Cannot merge: the repository has uncommitted changes. Commit or stash them first, then try again.";
                self.emit_apply_failed(
                    change.thread_id.unwrap_or(change_id),
                    change_id,
                    msg,
                    actor.clone(),
                )
                .await;
                return Err(msg.into());
            }
        }

        // Tier 1: If a live agent session exists for this thread, merge main
        // into its worktree instead of creating a temp worktree. The original
        // agent has full context and can resolve conflicts intelligently.
        //
        // A clean fast-forward merge runs inline (sub-second) and returns
        // `applied`. When `main` has diverged and CC-assisted conflict
        // resolution is needed, that runs in a background task and we return
        // `Conflict` immediately — the caller's turn (the parent chat thread's
        // `apply_change` tool call) is not held open for the potentially
        // many-minute resolution. The eventual `ChangeApplied` plus the EventBus
        // parent-callback fan-out's `ChildThreadCompleted` give the parent a
        // fresh follow-up turn instead of one turn frozen the whole time.
        if let Some(thread_id) = change.thread_id {
            match self.begin_in_place_merge(thread_id).await {
                InPlaceMergeStart::Ready(session) => {
                    log!(
                        "[Changes] Live agent session found for thread {} — merging in-place",
                        thread_id
                    );

                    // Auto-commit any uncommitted work before merging.
                    auto_commit_worktree(
                        &session.worktree_path,
                        "Coding agent changes (pre-merge auto-commit)",
                    )
                    .await;

                    // Fast path: clean fast-forward — finalize synchronously.
                    if let Ok((pre_sha, post_sha)) = catchup_and_ff_to_main(
                        &repo_root,
                        &session.worktree_path,
                        &change.branch_name,
                    )
                    .await
                    {
                        log!(
                            "[Changes] Fast-forward merge succeeded for {} (Tier 1)",
                            change.branch_name
                        );
                        // We claimed the in-progress flag in `begin_in_place_merge`;
                        // clear it since we're finalizing inline (no spawned task).
                        self.clear_apply_now_in_progress(thread_id).await;
                        // apply_now_success emits ChangeApplied, the entity-cache
                        // events (App*/Artifact*), AND AppUiRefreshRequested (for
                        // app threads) internally — no sibling emit needed here.
                        let commits = self
                            .apply_now_success(
                                thread_id,
                                change_id,
                                effective_requires_restart,
                                files_have_client_update(&change.files),
                                &pre_sha,
                                &post_sha,
                                &session.worktree_path,
                                &repo_root,
                                &change.branch_name,
                                actor.clone(),
                            )
                            .await;
                        return Ok(ApplyResult::applied_with_merge(
                            change_id,
                            Some(thread_id),
                            effective_requires_restart,
                            pre_sha,
                            post_sha,
                            &commits,
                            change.files.len(),
                        ));
                    }

                    // `main` diverged — hand the CC-assisted resolution to a
                    // background task and return immediately. The task clears
                    // the in-progress flag and emits the terminal event.
                    log!(
                        "[Changes] main diverged for {} — spawning async in-place conflict recovery",
                        change.branch_name
                    );
                    self.spawn_in_place_conflict_recovery(
                        thread_id,
                        change_id,
                        session,
                        change.branch_name.clone(),
                        repo_root.clone(),
                        effective_requires_restart,
                        files_have_client_update(&change.files),
                        actor.clone(),
                    );
                    return Ok(ApplyResult::conflict(
                        change_id,
                        thread_id,
                        change.files.len(),
                        "Merge conflict — the coding-agent session is resolving it. \
                         The change will apply automatically when resolution completes.",
                    ));
                }
                InPlaceMergeStart::AlreadyInProgress => {
                    log!(
                        "[Changes] Apply already in progress for thread {} — returning conflict",
                        thread_id
                    );
                    return Ok(ApplyResult::conflict(
                        change_id,
                        thread_id,
                        change.files.len(),
                        "An apply is already in progress for this thread — it will finish on its own.",
                    ));
                }
                // No live session — fall through to the dead-session tiers below.
                InPlaceMergeStart::NoLiveSession => {}
            }
        }

        // Tier 2: dead session with worktree on disk — try ff, else resume CC for merge
        if let Some(thread_id) = change.thread_id {
            if let Some(wt_path) = find_worktree_for_branch(&repo_root, &change.branch_name).await {
                // Auto-commit any uncommitted CC work before merging
                auto_commit_worktree(&wt_path, "Coding agent changes (pre-merge auto-commit)").await;

                // Fast path: try ff directly
                match catchup_and_ff_to_main(&repo_root, &wt_path, &change.branch_name).await {
                    Ok((pre_sha, post_sha)) => {
                        log!(
                            "[Changes] Fast path succeeded for {} (Tier 2)",
                            change.branch_name
                        );
                        let commits = commits_in_range(&repo_root, &pre_sha, &post_sha).await;
                        // Phase 6.2: keep worktree on disk; reset to main HEAD
                        // so the next user message resumes CC at a clean state.
                        // The branch ref is still alive (catchup_and_ff_to_main
                        // tries `branch -D` against repo_root, which fails
                        // silently when a worktree owns the branch).
                        if let Err(e) = reset_worktree_to_main_after_apply(&wt_path).await {
                            self.emit_apply_failed(
                                thread_id,
                                change_id,
                                &e.to_string(),
                                actor.clone(),
                            )
                            .await;
                            return Err(e);
                        }
                        // Phase 4: Apply no longer terminates the thread —
                        // SessionEnded is terminal-only. Phase 6.2: the
                        // worktree and branch are preserved above so the
                        // next user message resumes the same Claude Code session.
                        // ChangeApplied below is the user-visible signal.
                        self.emit_change_applied(
                            thread_id,
                            change_id,
                            effective_requires_restart,
                            files_have_client_update(&change.files),
                            commits.clone(),
                            change.thread_title.clone(),
                            actor.clone(),
                            Some(pre_sha.clone()),
                            Some(post_sha.clone()),
                        )
                        .await;
                        self.maybe_emit_app_ui_refresh(&kind_ctx, &change.files, actor.as_ref()).await;
                        self.emit_entity_events_for_change_apply(
                            &change.files,
                            Some(&pre_sha),
                            Some(&post_sha),
                            actor.clone(),
                        )
                        .await;
                        self.broadcast_changes_updated().await;
                        return Ok(ApplyResult::applied_with_merge(
                            change_id,
                            Some(thread_id),
                            effective_requires_restart,
                            pre_sha,
                            post_sha,
                            &commits,
                            change.files.len(),
                        ));
                    }
                    Err(_) => {
                        // ff failed — CC needs to merge main into the branch
                        log!(
                            "[Changes] Fast path failed for {} — resuming CC for merge",
                            change.branch_name
                        );
                    }
                }

                // Look up resume token for potential resume
                let resume_token =
                    CodingAgentChangeOps::lookup_session_id_for_resume(self.as_ref(), thread_id).await;

                // Park the actor by change_id so the conflict-recovery cleanup
                // in `run_session.rs` (the `if let Some(change) = conflict_change`
                // branch — grep for `pending_apply_actors.take`) stamps the
                // resulting ChangeApplied / ChangeApplyFailed with the device
                // that clicked Apply. Without this the cleanup falls through to
                // None, which renders as "Lucidos Engine" in the chat chip.
                // Mirrors the Tier-3 stash below — Tier 2 needs it too because
                // the cleanup is the sole emitter for the merge outcome (this
                // code path no longer re-emits after `run_merge_session_tier2`
                // returns).
                if let Some(a) = actor.as_ref() {
                    self.pending_apply_actors.stash(change_id, a.clone());
                }

                match CodingAgentChangeOps::run_merge_session_tier2(
                    self.as_ref(),
                    thread_id,
                    change_id,
                    &wt_path,
                    &change.branch_name,
                    &change.description,
                    resume_token,
                )
                .await
                {
                    Ok(_) => {
                        // The conflict-recovery cleanup in `run_session.rs`
                        // (the `if let Some(change) = conflict_change` branch)
                        // has already run `ff_merge_to_main` and emitted
                        // ChangeApplied (or ChangeApplyFailed on a leftover
                        // unmerged state). Do NOT re-check merge state and
                        // re-emit here. The cleanup's ff_merge_to_main deletes
                        // the branch, so a `merge-base --is-ancestor main
                        // branch` check would falsely report "not merged" and
                        // a re-emitted ChangeApplyFailed would lie about a
                        // merge that actually succeeded ("Claude Code session
                        // ended without completing the merge — try applying
                        // again"). Re-read the change row to construct an
                        // accurate ApplyResult for the HTTP caller; the
                        // EventBus emit in the cleanup runs synchronously so
                        // the projection reflects the new terminal state by
                        // the time we're here.
                        match self.changes().get_by_id(change_id).await {
                            Ok(Some(c)) if c.status == "applied" => {
                                // ChangeApplied + AppUiRefreshRequested +
                                // entity events were already emitted by the
                                // run_session.rs conflict-recovery cleanup
                                // (see the long comment above). Don't
                                // re-emit them here — duplicate iframe
                                // reloads otherwise.
                                self.broadcast_changes_updated().await;
                                return Ok(ApplyResult::applied_with_merge(
                                    change_id,
                                    Some(thread_id),
                                    c.requires_restart && !kind_ctx.is_app(),
                                    c.pre_merge_sha.unwrap_or_default(),
                                    c.post_merge_sha.unwrap_or_default(),
                                    &c.commits,
                                    c.files.len(),
                                ));
                            }
                            Ok(_) => {
                                // Cleanup didn't mark it applied — the failure
                                // event it emitted (e.g. "Conflict resolution
                                // incomplete") is already on the wire so the
                                // UI surfaces the message from there.
                                return Ok(ApplyResult::conflict(
                                    change_id,
                                    thread_id,
                                    change.files.len(),
                                    "Merge did not complete — try applying again.",
                                ));
                            }
                            Err(e) => {
                                log!(
                                    "[Changes] DB error re-reading change {} after Tier 2 merge: {} — treating as conflict",
                                    change_id,
                                    e
                                );
                                return Ok(ApplyResult::conflict(
                                    change_id,
                                    thread_id,
                                    change.files.len(),
                                    "Database error after merge — try applying again.",
                                ));
                            }
                        }
                    }
                    Err(e) => {
                        log!(
                            "[Changes] CC merge failed for {}: {} — falling through to Tier 3",
                            change_id,
                            e
                        );
                        // Tier 3 will spawn its own merge session and re-stash
                        // the actor with its own scope; clear this Tier-2
                        // stash so the fallthrough doesn't double-park.
                        self.pending_apply_actors.take(change_id);
                        let _ = git_cmd(&["merge", "--abort"], &wt_path).await;
                    }
                }
            }
        }

        // Tier 3: No worktree — try ff directly, spawn CC in temp worktree if needed

        // Fast path: branch may already be a descendant of main
        {
            let _merge_guard = MERGE_MUTEX.lock().await;
            let main_sha = git_cmd(&["rev-parse", "main"], &repo_root)
                .await
                .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
                .unwrap_or_default();
            let branch_sha = git_cmd(&["rev-parse", &change.branch_name], &repo_root)
                .await
                .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
                .unwrap_or_default();

            if let Ok(shas) = ff_main_to(&repo_root, &branch_sha, &main_sha).await {
                log!(
                    "[Changes] Fast path succeeded for {} (Tier 3)",
                    change.branch_name
                );
                let commits = commits_in_range(&repo_root, &shas.0, &shas.1).await;
                // Phase 6.2: keep the branch alive — it's at the same SHA as
                // main now, and the thread may resume CC against it later. No
                // worktree exists for this branch in Tier 3, so there's
                // nothing to reset.
                // Lucidos-source threads push main to remote so origin tracks
                // the merged work. App threads merge to the workspace git's
                // main, which is local-only by design — no remote push.
                if !kind_ctx.is_app() {
                    push_main_in_background(&repo_root);
                }
                self.emit_change_applied(
                    change.thread_id.unwrap_or(change_id),
                    change_id,
                    effective_requires_restart,
                    files_have_client_update(&change.files),
                    commits.clone(),
                    change.thread_title.clone(),
                    actor.clone(),
                    Some(shas.0.clone()),
                    Some(shas.1.clone()),
                )
                .await;
                self.maybe_emit_app_ui_refresh(&kind_ctx, &change.files, actor.as_ref()).await;
                self.emit_entity_events_for_change_apply(
                    &change.files,
                    Some(&shas.0),
                    Some(&shas.1),
                    actor.clone(),
                )
                .await;
                self.broadcast_changes_updated().await;
                return Ok(ApplyResult::applied_with_merge(
                    change_id,
                    change.thread_id,
                    effective_requires_restart,
                    shas.0,
                    shas.1,
                    &commits,
                    change.files.len(),
                ));
            }
        }

        // Auto-merge path: main moved forward — try a worktree-based catchup-and-ff
        // before falling back to spawning CC. Handles the common race where a parallel
        // commit on main left this branch behind, but the merge is conflict-free.
        {
            let temp_wt = worktrees_dir(self.workspace_path())
                .join(format!("apply-{}", change_id.as_simple()));
            let temp_wt_str = temp_wt.to_string_lossy().into_owned();
            let _ = git_cmd(&["worktree", "remove", "--force", &temp_wt_str], &repo_root).await;

            let add_ok = matches!(
                worktree_add(&repo_root, &temp_wt, &[&change.branch_name]).await,
                Ok(o) if o.status.success()
            );

            if add_ok {
                let result =
                    catchup_and_ff_to_main(&repo_root, &temp_wt, &change.branch_name).await;
                let _ = git_cmd(&["worktree", "remove", "--force", &temp_wt_str], &repo_root).await;

                if let Ok((pre_sha, post_sha)) = result {
                    // `catchup_and_ff_to_main` deletes change.branch_name on success.
                    // On failure, leave it intact so the CC slow path below can still merge from it.
                    log!(
                        "[Changes] Auto-merge path succeeded for {} (Tier 3)",
                        change.branch_name
                    );
                    let commits = commits_in_range(&repo_root, &pre_sha, &post_sha).await;
                    self.emit_change_applied(
                        change.thread_id.unwrap_or(change_id),
                        change_id,
                        effective_requires_restart,
                        files_have_client_update(&change.files),
                        commits.clone(),
                        change.thread_title.clone(),
                        actor.clone(),
                        Some(pre_sha.clone()),
                        Some(post_sha.clone()),
                    )
                    .await;
                    self.maybe_emit_app_ui_refresh(&kind_ctx, &change.files, actor.as_ref()).await;
                    self.emit_entity_events_for_change_apply(
                        &change.files,
                        Some(&pre_sha),
                        Some(&post_sha),
                        actor.clone(),
                    )
                    .await;
                    self.broadcast_changes_updated().await;
                    return Ok(ApplyResult::applied_with_merge(
                        change_id,
                        change.thread_id,
                        effective_requires_restart,
                        pre_sha,
                        post_sha,
                        &commits,
                        change.files.len(),
                    ));
                }
            }
        }

        // Slow path: create temp worktree and spawn CC to handle the merge
        let thread_id = change.thread_id.unwrap_or_else(Uuid::new_v4);
        let temp_branch = format!("merge-tmp/{}", change_id.as_simple());
        let wt_path =
            worktrees_dir(self.workspace_path()).join(format!("merge-{}", change_id.as_simple()));
        let wt_path_str = wt_path.to_string_lossy().into_owned();

        let _ = git_cmd(&["worktree", "remove", "--force", &wt_path_str], &repo_root).await;
        let _ = git_cmd(&["branch", "-D", &temp_branch], &repo_root).await;

        match worktree_add(&repo_root, &wt_path, &["-b", &temp_branch]).await {
            Ok(o) if o.status.success() => {}
            Ok(o) => {
                let msg = format!(
                    "Failed to create merge worktree: {}",
                    String::from_utf8_lossy(&o.stderr).trim()
                );
                self.emit_apply_failed(
                    change.thread_id.unwrap_or(change_id),
                    change_id,
                    &msg,
                    actor.clone(),
                )
                .await;
                return Err(msg.into());
            }
            Err(e) => {
                let msg = format!("Failed to create merge worktree: {}", e);
                self.emit_apply_failed(
                    change.thread_id.unwrap_or(change_id),
                    change_id,
                    &msg,
                    actor.clone(),
                )
                .await;
                return Err(msg.into());
            }
        }

        self.event_bus
            .emit_or_log(
                crate::engine::event_bus::BusEvent::Thread {
                    thread_id,
                    event: crate::engine::thread_events::ThreadEvent::MergeResolutionStarted {
                        change_id: change_id.to_string(),
                        worktree_path: wt_path_str.clone(),
                        temp_branch: temp_branch.clone(),
                    },
                    meta: crate::engine::thread_events::EventMeta::NONE,
                },
                "[Changes] MergeResolutionStarted",
            )
            .await;

        // Park the actor by change_id so the cleanup in `run_session` can
        // stamp ChangeApplied / ChangeApplyFailed with the device that clicked
        // Apply, instead of falling through to the "Lucidos Engine" chip.
        // The HTTP call returns immediately below — by the time the spawned
        // CC merges and idles, this scope is long gone, so the stash is the
        // only way to keep the actor available for the eventual emit.
        if let Some(a) = actor.as_ref() {
            self.pending_apply_actors.stash(change_id, a.clone());
        }
        CodingAgentChangeOps::spawn_merge_session(self.as_ref(), thread_id, change_id, &change.description);
        Ok(ApplyResult::conflict(
            change_id,
            thread_id,
            change.files.len(),
            "Branch needs merging — agent is handling it.",
        ))
    }
}
