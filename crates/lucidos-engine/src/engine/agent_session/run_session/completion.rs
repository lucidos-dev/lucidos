use super::idle_snapshot::CodingAgentIdleSnapshot;
use crate::engine::agent_session::io_helpers::{drain_lost_followups, lost_followups_to_orphans};
use crate::engine::agent_session::lifecycle::{
    classify_session_end_action, should_auto_commit_on_cleanup, SessionEndAction, TerminalKind,
};
use crate::engine::agent_session::resume::change_description_fallback;
use crate::engine::change_ops::branch_is_hardened;
use crate::engine::git_ops::{
    auto_commit_preserving_marker, branch_changed_files, commits_in_range, consume_harden_marker,
    default_local_branch, describe_branch_changes, ff_merge_to_main, files_have_client_update,
    files_require_restart, git_cmd, git_commit_no_edit, has_branch_commits,
    worktree_current_branch,
};
use crate::engine::thread_events::EventChannel;
use crate::engine::{LucidosEngine, ProcessResult, StopReason};
use std::path::PathBuf;
use std::sync::Arc;
use uuid::Uuid;

impl LucidosEngine {
    /// Completion / teardown lifecycle stage of `run_direct_agent`, extracted
    /// from the driver: runs after the event loop exits. Marks the session
    /// exited, drains lost follow-ups, fires the safety net, then performs
    /// either conflict-resolution cleanup (ff-merge to main) or normal
    /// worktree cleanup (auto-commit, discard, or change proposal) before
    /// returning the final `ProcessResult`.
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn finalize_direct_agent(
        &self,
        thread_id: Uuid,
        request_id: Uuid,
        meta: &crate::engine::thread_events::EventMeta,
        conflict_change: Option<crate::core::changes::Change>,
        cwd: PathBuf,
        repo_root: PathBuf,
        branch_name: String,
        worktree_path: Option<PathBuf>,
        images: Vec<String>,
        mut msg_rx: tokio::sync::mpsc::UnboundedReceiver<crate::engine::AgentUserInput>,
        claude_text_buf: String,
        normalized_model: Option<String>,
        cc_reasoning_effort: Option<String>,
        last_terminal_kind: Option<TerminalKind>,
        external_terminal_emitted: Arc<std::sync::atomic::AtomicBool>,
        agent_cancel: &tokio_util::sync::CancellationToken,
        emitted_terminal_event: bool,
        watchdog_fired: bool,
        last_emitted_idle: bool,
        is_external_repo: bool,
        mut proposed_change: bool,
    ) -> Result<ProcessResult, Box<dyn std::error::Error + Send + Sync>> {
        // Mark exited before emitting terminal events — forces follow-ups arriving
        // after the event loop exits to spawn a new session instead of routing to
        // a dead channel (closes the process_exited race window in chat.rs fast-path).
        {
            let mut guard = self.agent_sessions.lock().await;
            if let Some(s) = guard.get_mut(&thread_id) {
                s.process_exited = true;
                s.idle_notify.notify_waiters();
            }
        }
        self.clear_cc_debounce(thread_id);

        // Drain follow-ups queued while CC was busy. Convert to orphaned injections
        // so the caller re-processes them instead of showing "interrupted".
        let cc_orphans = lost_followups_to_orphans(drain_lost_followups(&mut msg_rx));

        // Safety net: CC's event loop ended without a natural terminator.
        // The decision tree is pinned by `safety_net_action` (lifecycle.rs):
        //   - watchdog_fired → ContinuationRequested (auto-recovery, user never knew)
        //   - else           → ResponseAborted(SafetyNet) (red dot, user notified)
        //   - external terminal already emitted → Skip (don't relabel)
        // The cleanup path below reads `safety_net_fired` and skips
        // propose_change either way so partial commits don't surface as an
        // Apply card.
        let safety_net_fired = !emitted_terminal_event;
        if safety_net_fired {
            log!(
                "[ClaudeCode] safety net firing for thread {} — buffered_text_len={}, watchdog_fired={}",
                thread_id,
                claude_text_buf.len(),
                watchdog_fired,
            );
        }
        let external_already = Self::external_terminal_already_emitted(
            &external_terminal_emitted,
            thread_id,
            "safety net",
        );
        match crate::engine::agent_session::lifecycle::safety_net_action(
            safety_net_fired,
            watchdog_fired,
            external_already,
        ) {
            crate::engine::agent_session::lifecycle::SafetyNetAction::Nothing
            | crate::engine::agent_session::lifecycle::SafetyNetAction::Skip => {}
            crate::engine::agent_session::lifecycle::SafetyNetAction::EmitContinuationRequested => {
                crate::engine::thread_events::emit_continuation_requested_or_log(
                    &self.event_bus,
                    thread_id,
                    crate::engine::agent_recovery::AUTO_RECOVERY_AFTER_HANG_REASON,
                    meta.actor.clone(),
                    "[ClaudeCode] safety-net ContinuationRequested (auto-recovery)",
                )
                .await;
            }
            crate::engine::agent_session::lifecycle::SafetyNetAction::EmitAbortedSafetyNet => {
                let mut emit_meta = meta.clone();
                Self::stamp_system_actor_if_aborted(&mut emit_meta, true);
                crate::engine::thread_events::emit_response_aborted(
                    &self.event_bus,
                    thread_id,
                    crate::engine::thread_events::AbortCause::SafetyNet,
                    claude_text_buf.clone(),
                    vec![],
                    normalized_model.clone(),
                    cc_reasoning_effort.clone(),
                    emit_meta,
                    "[ClaudeCode] safety-net ResponseAborted",
                )
                .await;
            }
        }

        // Make sure the runtime task tears down its child process — driver
        // already drained and logged stderr inside its own task.
        agent_cancel.cancel();

        // During engine shutdown, skip all cleanup — preserve the worktree and branch
        // so recover_orphaned_worktrees can resume the session after restart.
        // Read discard early so we can skip unnecessary work (auto-commit,
        // hardening) when the user chose Discard.
        let should_discard = matches!(
            self.pending_stop_reason(thread_id).await,
            Some(StopReason::Discard),
        );

        // Auto-commit any uncommitted changes so they survive on disk.
        {
            let is_shutdown = {
                let guard = self.agent_sessions.lock().await;
                guard
                    .get(&thread_id)
                    .map(|s| s.shutting_down.load(std::sync::atomic::Ordering::Relaxed))
                    .unwrap_or(false)
            };
            if is_shutdown {
                // Same gate as the other cleanup paths — only commit (and so
                // fire the per-commit hook → ChangeProposed) when the last
                // turn ended Generated. Mid-turn shutdown is half-assed; the
                // worktree's uncommitted state survives for recovery either
                // way, so skipping the auto-commit costs us nothing.
                if let Some(ref wt) = worktree_path {
                    if should_auto_commit_on_cleanup(false, &last_terminal_kind) {
                        auto_commit_preserving_marker(
                            &self.pool,
                            wt,
                            &repo_root,
                            &branch_name,
                            "Claude Code changes (auto-committed on shutdown)",
                        )
                        .await;
                    }
                }
                let mut guard = self.agent_sessions.lock().await;
                guard.remove(&thread_id);
                log!(
                    "[Shutdown] Skipping cleanup for thread {} — session will resume after restart",
                    thread_id
                );
                return Ok(ProcessResult {
                    response: String::new(),
                    steps: vec![],
                    images,
                    request_id,
                    thread_id,
                    proposed_change: false,
                    auto_apply: false,
                    orphaned_injections: vec![],
                });
            }
        }

        if let Some(change) = conflict_change {
            // Conflict resolution cleanup — merge happened in a worktree, ff-merge to main.
            // The HTTP Apply call that triggered this CC merge returned long ago; the
            // user's actor was parked in `pending_apply_actors` at apply_change Tier 3
            // entry. Take it back here so the resulting ChangeApplied / ChangeApplyFailed
            // carries the device that clicked Apply instead of falling through to None
            // (which renders as "Lucidos Engine" in the chat chip).
            let apply_actor = self.pending_apply_actors.take(change.id);

            let has_unmerged = git_cmd(&["diff", "--name-only", "--diff-filter=U"], &cwd)
                .await
                .map(|o| !o.stdout.is_empty())
                .unwrap_or(true);

            let wt_str = cwd.to_string_lossy();
            let temp_branch = change
                .merge_temp_branch
                .as_deref()
                .unwrap_or(&change.branch_name);

            if has_unmerged {
                let _ = git_cmd(&["merge", "--abort"], &cwd).await;
                log!(
                    "[AgentSession] Conflict resolution incomplete for {} — merge aborted in worktree",
                    change.branch_name
                );
                let _ = git_cmd(&["worktree", "remove", "--force", &wt_str], &repo_root).await;
                let _ = git_cmd(&["branch", "-D", temp_branch], &repo_root).await;
                self.emit_merge_resolution_cleared(
                    change.thread_id.unwrap_or(thread_id),
                    change.id,
                    "[ConflictResolution] MergeResolutionCleared",
                )
                .await;
                self.emit_apply_failed(
                    change.thread_id.unwrap_or(thread_id),
                    change.id,
                    "Conflict resolution incomplete — merge aborted. The change is still pending; try applying again.",
                    apply_actor,
                )
                .await;
            } else {
                // Ensure merge is committed
                let merge_committed = git_cmd(&["rev-parse", "MERGE_HEAD"], &cwd)
                    .await
                    .map(|o| !o.status.success())
                    .unwrap_or(false);
                if !merge_committed {
                    // ff_merge_to_main below will surface the user-visible failure
                    // if the merge stays uncommitted, but the log here preserves
                    // the original git stderr so a stuck merge can be triaged
                    // without re-running.
                    if let Err(e) = git_commit_no_edit(&cwd).await {
                        log!(
                            "[ConflictResolution] {} (change {}, branch {})",
                            e,
                            change.id,
                            change.branch_name
                        );
                    }
                }

                // Remove worktree and ff-merge to main
                match ff_merge_to_main(&repo_root, &wt_str, temp_branch, &change.branch_name).await {
                    Ok((pre_sha, post_sha)) => {
                        let commits = commits_in_range(&repo_root, &pre_sha, &post_sha).await;
                        // Mirror the kind-aware overrides apply_change applies
                        // for app threads: never claim requires_restart for an
                        // app change, and emit AppUiRefreshRequested so open
                        // iframes reload after a conflict-resolution apply
                        // (the Tier-1/2/3 happy-path branches in change_ops.rs
                        // already do this; the cleanup path used to skip it).
                        let kind_ctx = crate::engine::change_ops::load_apply_kind_context(
                            &self.pool,
                            change.thread_id,
                        )
                        .await;
                        let requires_restart_effective =
                            change.requires_restart && !kind_ctx.is_app();
                        self.emit_change_applied(
                            change.thread_id.unwrap_or(thread_id),
                            change.id,
                            requires_restart_effective,
                            files_have_client_update(&change.files),
                            commits,
                            change.thread_title.clone(),
                            apply_actor.clone(),
                            Some(pre_sha.clone()),
                            Some(post_sha.clone()),
                        )
                        .await;
                        self.maybe_emit_app_ui_refresh(
                            &kind_ctx,
                            &change.files,
                            apply_actor.as_ref(),
                        )
                        .await;
                        self.emit_entity_events_for_change_apply(
                            &change.files,
                            Some(&pre_sha),
                            Some(&post_sha),
                            apply_actor,
                        )
                        .await;
                        self.emit_merge_resolution_cleared(
                            change.thread_id.unwrap_or(thread_id),
                            change.id,
                            "[ConflictResolution] MergeResolutionCleared",
                        )
                        .await;
                        log!(
                            "[AgentSession] Conflict resolution complete — change {} applied via ff-merge",
                            change.id
                        );
                    }
                    Err(e) => {
                        self.emit_merge_resolution_cleared(
                            change.thread_id.unwrap_or(thread_id),
                            change.id,
                            "[ConflictResolution] MergeResolutionCleared",
                        )
                        .await;
                        log!("[AgentSession] ff-merge failed after conflict resolution: {}", e);
                        self.emit_apply_failed(
                            change.thread_id.unwrap_or(thread_id),
                            change.id,
                            &format!("Merge failed after conflict resolution: {}", e),
                            apply_actor,
                        )
                        .await;
                    }
                }
            }
        } else {
            // Normal worktree cleanup
            let wt = worktree_path.as_ref().unwrap();

            // Auto-commit only when the last turn ended cleanly (Generated)
            // AND the user didn't ask to discard. Anything else — safety-net
            // abort, Failed, Canceled, Aborted, mid-turn user stop — leaves
            // worktree dirt uncommitted. See `should_auto_commit_on_cleanup`.
            if should_auto_commit_on_cleanup(should_discard, &last_terminal_kind) {
                self.commit_dirty_logged("Claude Code changes", "Claude Code cleanup")
                    .await;
                auto_commit_preserving_marker(&self.pool, wt, &repo_root, &branch_name, "Claude Code changes (auto-committed)").await;
            }

            if should_discard {
                // User chose "Discard & End Session" — remove worktree, delete branch
                // Discard any pending change for this branch so the frontend doesn't show it as waiting
                let pending_lookup = self
                    .changes()
                    .get_pending_by_branch(&branch_name)
                    .await
                    .unwrap_or_else(|e| {
                        log!(
                            "[ClaudeCode] get_pending_by_branch({}): {} — skipping discard emit",
                            branch_name,
                            e
                        );
                        None
                    });
                if let Some(change) = pending_lookup {
                    self.event_bus
                        .emit_or_log(
                            crate::engine::event_bus::BusEvent::Thread {
                                thread_id,
                                event: crate::engine::thread_events::ThreadEvent::ChangeDiscarded {
                                    change_id: change.id.to_string(),
                                    actor: None,
                                    path: String::new(),
                                },
                                meta: meta.clone(),
                            },
                            "[ClaudeCode] ChangeDiscarded (worktree cleanup)",
                        )
                        .await;
                }
                let wt_path_str = wt.to_string_lossy();
                if let Err(e) =
                    git_cmd(&["worktree", "remove", "--force", &wt_path_str], &repo_root).await
                {
                    log!("[AgentSession] {}", e);
                }
                log!("[ClaudeCode] Discarding changes (branch {})", branch_name);
                if let Err(e) = git_cmd(&["branch", "-D", &branch_name], &repo_root).await {
                    log!(
                        "[ClaudeCode] Failed to delete branch {}: {}",
                        branch_name,
                        e
                    );
                }
            } else {
                // Detect if the Claude Code session switched to a different branch inside the
                // worktree (e.g. created a feature branch in an external repo). If so,
                // use the actual branch for the change proposal instead of the tracked
                // claude-code/* branch — that branch has no commits.
                let actual_branch = worktree_current_branch(wt).await;
                let base = default_local_branch(&repo_root).await;
                let effective_branch = if let Some(ref actual) = actual_branch {
                    // Only use the actual branch if CC switched to a real feature branch,
                    // not if it ended up on the default branch (main/master) — otherwise
                    // the cleanup path could delete main.
                    if actual != &branch_name && actual != &base {
                        log!("[ClaudeCode] Worktree is on branch '{}', tracked branch was '{}' — using actual branch",
                            actual, branch_name);
                        actual.as_str()
                    } else {
                        branch_name.as_str()
                    }
                } else {
                    branch_name.as_str()
                };

                let was_hardened = branch_is_hardened(&self.pool, self.changes(), &repo_root, effective_branch).await;
                let has_commits = has_branch_commits(&repo_root, effective_branch).await;
                let changed_files = if has_commits {
                    branch_changed_files(&repo_root, effective_branch).await
                } else {
                    Vec::new()
                };

                // Remove the worktree directory (the branch stays)
                let wt_path_str = wt.to_string_lossy();
                if let Err(e) =
                    git_cmd(&["worktree", "remove", "--force", &wt_path_str], &repo_root).await
                {
                    log!("[AgentSession] {}", e);
                }

                // A user cancel (Stop = Esc) is a resumable turn boundary, not a
                // terminator: keep the branch even with no commits so the next
                // message can `--resume` this session, and never propose
                // half-finished work. Specifically `Canceled(UserStop)` —
                // Apply/Discard/Archive carry their own terminators and never
                // surface here as the turn's terminal kind.
                let user_canceled = matches!(
                    last_terminal_kind,
                    Some(TerminalKind::Canceled(
                        crate::engine::thread_events::CancelCause::UserStop
                    ))
                );

                match classify_session_end_action(
                    has_commits,
                    changed_files.is_empty(),
                    is_external_repo,
                    safety_net_fired,
                    user_canceled,
                ) {
                    SessionEndAction::KeepExternalBranch => {
                        log!(
                            "[ClaudeCode] External repo branch {} — keeping branch, no change proposed",
                            effective_branch
                        );
                    }
                    SessionEndAction::KeepCanceledBranch => {
                        log!(
                            "[ClaudeCode] User cancelled (Esc) on branch {} — keeping branch resumable, no change proposed",
                            effective_branch
                        );
                    }
                    SessionEndAction::CrashedKeepBranch => {
                        log!(
                            "[ClaudeCode] Safety net fired with commits on {} — keeping branch on disk, NOT proposing change (thread ended in ResponseAborted)",
                            effective_branch
                        );
                    }
                    SessionEndAction::Propose => {
                        let requires_restart = files_require_restart(&changed_files);

                        log!(
                            "[AgentSession] Storing change on branch {}{}",
                            effective_branch,
                            if requires_restart {
                                " (requires restart)"
                            } else {
                                ""
                            }
                        );
                        let repo_root_str = repo_root.to_string_lossy().to_string();

                        let fallback =
                            change_description_fallback(self.pool(), thread_id, effective_branch).await;
                        let base = default_local_branch(&repo_root).await;
                        let log_range = format!("{}..{}", base, effective_branch);
                        let description =
                            describe_branch_changes(&repo_root, &log_range, &fallback, None).await;

                        match self
                            .propose_change(crate::engine::change_ops::ProposeChangeInput {
                                thread_id,
                                branch_name: effective_branch,
                                repo_root: &repo_root_str,
                                description: &description,
                                files: &changed_files,
                                requires_restart,
                                channel: EventChannel::ClaudeCode,
                                hardened: was_hardened,
                                // Live agent proposal at session end — origin is
                                // carried by the surrounding MessageReceived.
                                origin: None,
                                // Session-end cleanup runs after the terminal
                                // event already landed; the per-turn idle path
                                // owns the failure tag, so this fallback path
                                // never originates `incomplete`.
                                incomplete: false,
                            })
                            .await
                        {
                            Ok(_change_id) => {
                                proposed_change = true;
                                if was_hardened {
                                    consume_harden_marker(&self.pool, &repo_root, effective_branch).await;
                                }
                                if !last_emitted_idle {
                                    // Session-end cleanup path: by the time we reach this
                                    // branch the session is wrapping up and a change has
                                    // already been proposed, so bg-bash gating is no longer
                                    // a meaningful signal — `bg_bash_pending = false`.
                                    self.emit_coding_agent_idled(
                                        thread_id,
                                        CodingAgentIdleSnapshot {
                                            has_changes: !changed_files.is_empty(),
                                            is_external_repo,
                                            requires_restart,
                                            bg_bash_pending: false,
                                            worktree_path: worktree_path.as_deref(),
                                        },
                                        meta,
                                    )
                                    .await;
                                }
                            }
                            Err(e) => {
                                log!("[AgentSession] Failed to propose change: {}", e);
                            }
                        }
                    }
                    SessionEndAction::CleanupBranches => {
                        if has_commits {
                            log!(
                                "[AgentSession] Branch {} has commits but empty diff vs main — cleaning up without proposing",
                                effective_branch
                            );
                        }
                        // No real changes on the effective branch — clean up the tracked branch
                        // if it's different (CC switched branches, original has no changes).
                        // On DB error we keep the branch (safer than deleting work we
                        // can't tell is still referenced).
                        let pending_orig = self
                            .changes()
                            .has_pending_for_branch(&branch_name)
                            .await
                            .unwrap_or_else(|e| {
                                log!(
                                    "[AgentSession] has_pending_for_branch({}): {} — \
                                     keeping branch defensively",
                                    branch_name,
                                    e
                                );
                                true
                            });
                        if effective_branch != branch_name && !pending_orig {
                            let _ = git_cmd(&["branch", "-D", &branch_name], &repo_root).await;
                        }
                        let pending_eff = self
                            .changes()
                            .has_pending_for_branch(effective_branch)
                            .await
                            .unwrap_or_else(|e| {
                                log!(
                                    "[AgentSession] has_pending_for_branch({}): {} — \
                                     keeping branch defensively",
                                    effective_branch,
                                    e
                                );
                                true
                            });
                        if !pending_eff {
                            let _ = git_cmd(&["branch", "-D", effective_branch], &repo_root).await;
                        }
                    }
                }
            }
        }

        // Read pending_stop and remove the session in one lock acquisition.
        // Keeping the session in the map during git/change-proposal work lets
        // the stop endpoint set `pending_stop = Some(Apply)` even if the user
        // clicks "Apply Now" while cleanup is already in progress (avoids a
        // 404 race).
        let auto_apply = {
            let mut guard = self.agent_sessions.lock().await;
            let val = matches!(
                guard.get(&thread_id).and_then(|s| s.pending_stop),
                Some(StopReason::Apply),
            );
            guard.remove(&thread_id);
            val
        };

        // SessionEnded is now terminal-only (Phase 4 of CC resume architecture).
        // Per-turn idle is signaled by `CodingAgentIdled`, which was already
        // emitted earlier in the loop. Discard, ChangesProposed, and Completed
        // turns all keep the thread alive — no SessionEnded fires here.

        // CC text was already streamed via ClaudeCodeText progress events.
        // Return empty response — frontend uses streamingResponse (ccText) as final content.
        Ok(ProcessResult {
            response: String::new(),
            steps: vec![],
            images,
            request_id,
            thread_id,
            proposed_change,
            auto_apply,
            orphaned_injections: cc_orphans,
        })
    }
}
