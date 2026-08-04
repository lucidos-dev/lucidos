use super::idle_snapshot::CodingAgentIdleSnapshot;
use crate::engine::agent_session::io_helpers::{drain_lost_followups, lost_followups_to_orphans};
use crate::engine::agent_session::lifecycle::{
    classify_session_end_action, conflict_abort_deletes_temp_state,
    conflict_resolution_cleanup_action, discarded_worktree_removal, should_auto_commit_on_cleanup,
    ConflictResolutionCleanupAction, SafetyNetAction, SessionEndAction, TerminalKind,
    WorktreeRemoval,
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
use std::path::{Path, PathBuf};
use std::sync::Arc;
use uuid::Uuid;

/// Remove the worktree of a session the user chose to **Discard**, and only
/// then. Discard is the one session end that asks for the work to go away: the
/// branch is deleted in the same breath, so the tree carries nothing the user
/// still wants.
///
/// Every other session end leaves the worktree alone. Reclamation has exactly
/// one owner, the background `WorktreeCleanup` worker, which decides on
/// evidence it gathers when nothing is racing it (ADR 0035, and
/// `docs/plans/2026-08-03-single-owner-worktree-reclamation.md`). A teardown is
/// the worst possible place to make that call: it frequently runs *because*
/// something just went wrong, so it is exactly when the engine's picture of the
/// world is least reliable, and it can be unwinding concurrently with the
/// safety net relaunching a session into the very tree it is deleting.
///
/// [`discarded_worktree_removal`] still holds the one guard worth keeping here:
/// a worktree checked out on some other branch is not this session's to delete.
async fn remove_discarded_worktree(
    worktree: &Path,
    repo_root: &Path,
    session_branch: &str,
    worktree_branch: Option<&str>,
) {
    match discarded_worktree_removal(worktree_branch, session_branch) {
        WorktreeRemoval::Keep(reason) => {
            log!(
                "[AgentSession] Keeping worktree {}: {}",
                worktree.display(),
                reason
            );
        }
        WorktreeRemoval::Remove => {
            let wt_path_str = worktree.to_string_lossy();
            if let Err(e) =
                git_cmd(&["worktree", "remove", "--force", &wt_path_str], repo_root).await
            {
                log!("[AgentSession] {}", e);
            }
        }
    }
}

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
        external_continuation_requested: Arc<std::sync::atomic::AtomicBool>,
        agent_cancel: &tokio_util::sync::CancellationToken,
        emitted_terminal_event: bool,
        watchdog_fired: bool,
        killed_by_signal: bool,
        last_emitted_idle: bool,
        is_external_repo: bool,
        mut proposed_change: bool,
        coding_agent: crate::runtime::CodingAgent,
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
        //   - stray signal-kill (no engine cancel) → ContinuationRequested (auto-resume)
        //   - else           → ResponseAborted(SafetyNet) (red dot, user notified)
        //   - external terminal already emitted → Skip (don't relabel)
        // The cleanup path below reads `safety_net_fired` and skips
        // propose_change either way so partial commits don't surface as an
        // Apply card.
        let safety_net_fired = !emitted_terminal_event;
        // The engine deliberately cancelled this session (user Stop, shutdown,
        // restart, eviction, stale-resume) when its token is already tripped at
        // the safety-net decision point — finalize cancels it only later (below).
        // Gates the stray-signal auto-resume so our own teardown SIGKILL isn't
        // mistaken for an external kill.
        let engine_cancelled = agent_cancel.is_cancelled();
        if safety_net_fired {
            log!(
                "[AgentSession] safety net firing for thread {} — buffered_text_len={}, watchdog_fired={}, killed_by_signal={}, engine_cancelled={}",
                thread_id,
                claude_text_buf.len(),
                watchdog_fired,
                killed_by_signal,
                engine_cancelled,
            );
        }
        let external_already = Self::external_terminal_already_emitted(
            &external_terminal_emitted,
            thread_id,
            "safety net",
        );
        let net_action = crate::engine::agent_session::lifecycle::safety_net_action(
            safety_net_fired,
            watchdog_fired,
            external_already,
            killed_by_signal,
            engine_cancelled,
        );
        match net_action {
            SafetyNetAction::Nothing | SafetyNetAction::Skip => {}
            SafetyNetAction::EmitContinuationRequested => {
                crate::engine::thread_events::emit_continuation_requested_or_log(
                    &self.event_bus,
                    thread_id,
                    crate::engine::agent_recovery::AUTO_RECOVERY_AFTER_HANG_REASON,
                    meta.actor.clone(),
                    "[AgentSession] safety-net ContinuationRequested (auto-recovery)",
                )
                .await;
            }
            SafetyNetAction::EmitAbortedSafetyNet => {
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
                    "[AgentSession] safety-net ResponseAborted",
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
            // Engine-global shutdown OR the per-session flag. `shutdown_agent_sessions`
            // sets the per-session flag only on sessions present in `agent_sessions`
            // when shutdown began — a session whose `spawn_or_resume` finished and
            // inserted itself AFTER that pass (a slow ~30s `--resume` racing an Apply
            // restart) keeps the flag `false`. Reading only the per-session flag then
            // let finalize run full normal cleanup and `git branch -D` a still-resumable
            // branch (the thread-9e37697e data-loss incident: a child's follow-up
            // resumed during shutdown, its session was inserted after the flag pass, and
            // its no-commit branch was reclaimed). `is_shutting_down()` is set first
            // thing in every shutdown path, so OR-ing it closes the insert-after-the-pass
            // race and preserves the worktree + branch for post-restart recovery.
            let is_shutdown = self.is_shutting_down() || {
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
                            "Coding agent changes (auto-committed on shutdown)",
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
            // `or_unknown(true)` (assume conflicts REMAIN when git could not be
            // asked): a `false` here is what lets `conflict_resolution_cleanup_action`
            // return `Apply` and ff-merge this tree into main, so an unanswered
            // probe must not produce it (`.claude/rules/rust.md`). Via
            // `git_answer_when_ok`, a non-zero exit is Unknown too, not a "no":
            // `git diff` exits non-zero when the path is not a work tree or a
            // filter blew up, which says nothing about unresolved conflicts.
            // Assuming conflicts remain costs an Abort, and the change stays
            // pending for the user to retry.
            let has_unmerged = crate::engine::git_ops::git_answer_when_ok(
                &["diff", "--name-only", "--diff-filter=U"],
                &cwd,
                |o| !o.stdout.is_empty(),
            )
            .await
            .or_unknown(true);

            let wt_str = cwd.to_string_lossy();

            // Two continuation shapes hand off: the in-loop safety net about
            // to emit `ContinuationRequested` (stray kill / in-loop watchdog),
            // and the EXTERNAL watchdog, which already emitted one and set the
            // suppression flag before cancelling us — so `net_action` reads
            // `Skip` there, and only its dedicated flag distinguishes that
            // recovery from a restart abort / concurrent cancel (which must
            // still Abort).
            let cleanup_action = conflict_resolution_cleanup_action(
                has_unmerged,
                &last_terminal_kind,
                net_action == SafetyNetAction::EmitContinuationRequested
                    || external_continuation_requested.load(std::sync::atomic::Ordering::Acquire),
            );
            // Both non-HandOff arms take the parked apply actor: the HTTP
            // Apply call that triggered this CC merge returned long ago; the
            // user's actor was parked in `pending_apply_actors` at
            // apply_change Tier-2/3 entry, and taking it here stamps the
            // resulting ChangeApplied / ChangeApplyFailed with the device
            // that clicked Apply instead of falling through to None (which
            // renders as "Lucidos Engine" in the chat chip). HandOff leaves
            // it parked for the continuation's own completion.
            match cleanup_action {
                ConflictResolutionCleanupAction::HandOff => {
                    // An auto-recovery continuation resumes this very merge
                    // turn via `--resume`. Aborting here would fail the apply
                    // the user is watching and race destructive git cleanup
                    // against the continuation spawn that's concurrently
                    // re-adopting the same worktree (the 2026-07-10
                    // stray-SIGTERM incident). Leave everything — merge state,
                    // worktree, branch, the parked apply actor, and the open
                    // MergeConflictDetected pairing — for the continuation,
                    // whose spawn re-attaches the duty via
                    // `pending_conflict_change_for_thread` and whose own
                    // completion then applies or aborts for real.
                    log!(
                        "[AgentSession] Conflict resolution for {} handed off to auto-recovery continuation — change {} stays pending, merge state preserved",
                        change.branch_name,
                        change.id
                    );
                }
                ConflictResolutionCleanupAction::Abort { message } => {
                    let apply_actor = self.pending_apply_actors.take(change.id);
                    let _ = git_cmd(&["merge", "--abort"], &cwd).await;
                    log!(
                        "[AgentSession] Conflict resolution not applied for {}: {}",
                        change.branch_name,
                        message
                    );
                    // Delete only what the merge attempt created: the Tier-3
                    // temp worktree + temp branch, and only when THIS session
                    // actually ran on them (a stale row after a pruned-temp
                    // re-attach must not condemn the thread worktree). A
                    // Tier-2 merge ran in the thread's OWN worktree on the
                    // REAL change branch — `merge --abort` is the whole
                    // cleanup there; deleting would destroy the user's
                    // committed work (rust.md failure-path-cleanup rule).
                    if conflict_abort_deletes_temp_state(
                        change.merge_temp_branch.as_deref(),
                        &branch_name,
                    ) {
                        let _ =
                            git_cmd(&["worktree", "remove", "--force", &wt_str], &repo_root).await;
                        let _ = git_cmd(&["branch", "-D", &branch_name], &repo_root).await;
                    }
                    self.emit_merge_resolution_cleared(
                        change.thread_id.unwrap_or(thread_id),
                        change.id,
                        "[ConflictResolution] MergeResolutionCleared",
                    )
                    .await;
                    self.emit_apply_failed(
                        change.thread_id.unwrap_or(thread_id),
                        change.id,
                        message,
                        apply_actor,
                    )
                    .await;
                }
                ConflictResolutionCleanupAction::Apply => {
                    let apply_actor = self.pending_apply_actors.take(change.id);
                    // Ensure merge is committed
                    let merge_committed = git_cmd(&["rev-parse", "MERGE_HEAD"], &cwd)
                        .await
                        .map(|o| !o.status.success())
                        .unwrap_or(false);
                    if !merge_committed {
                        // ff_merge_to_main below will surface the user-visible
                        // failure if the merge stays uncommitted, but the log
                        // here preserves the original git stderr so a stuck
                        // merge can be triaged without re-running.
                        if let Err(e) = git_commit_no_edit(&cwd).await {
                            log!(
                                "[ConflictResolution] {} (change {}, branch {})",
                                e,
                                change.id,
                                change.branch_name
                            );
                        }
                    }

                    // Remove worktree and ff-merge to main. The merge source
                    // is the branch THIS session ran on (`branch_name`): the
                    // temp branch for an original Tier-3 session, the change
                    // branch for Tier-2 and for a pruned-temp re-attach —
                    // where the row's recorded `merge_temp_branch` is stale
                    // and would ff the dead attempt's sha instead of the
                    // resolution the session just committed.
                    match ff_merge_to_main(&repo_root, &wt_str, &branch_name, &change.branch_name)
                        .await
                    {
                        Ok((pre_sha, post_sha)) => {
                            let commits = commits_in_range(&repo_root, &pre_sha, &post_sha).await;
                            // Mirror the kind-aware overrides apply_change
                            // applies for app threads: never claim
                            // requires_restart for an app change, and emit
                            // AppUiRefreshRequested so open iframes reload
                            // after a conflict-resolution apply (the
                            // Tier-1/2/3 happy-path branches in change_ops.rs
                            // already do this; the cleanup path used to skip
                            // it).
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
                            log!(
                                "[AgentSession] ff-merge failed after conflict resolution: {}",
                                e
                            );
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
            }
        } else {
            // Normal worktree cleanup
            let wt = worktree_path.as_ref().unwrap();

            // Auto-commit only when the last turn ended cleanly (Generated)
            // AND the user didn't ask to discard. Anything else — safety-net
            // abort, Failed, Canceled, Aborted, mid-turn user stop — leaves
            // worktree dirt uncommitted. See `should_auto_commit_on_cleanup`.
            if should_auto_commit_on_cleanup(should_discard, &last_terminal_kind) {
                self.commit_dirty_logged("Coding agent changes", "coding agent cleanup")
                    .await;
                auto_commit_preserving_marker(
                    &self.pool,
                    wt,
                    &repo_root,
                    &branch_name,
                    "Coding agent changes (auto-committed)",
                )
                .await;
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
                            "[AgentSession] get_pending_by_branch({}): {} — skipping discard emit",
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
                            "[AgentSession] ChangeDiscarded (worktree cleanup)",
                        )
                        .await;
                }
                let discard_branch = worktree_current_branch(wt).await;
                remove_discarded_worktree(wt, &repo_root, &branch_name, discard_branch.as_deref())
                    .await;
                log!("[AgentSession] Discarding changes (branch {})", branch_name);
                if let Err(e) = git_cmd(&["branch", "-D", &branch_name], &repo_root).await {
                    log!(
                        "[AgentSession] Failed to delete branch {}: {}",
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
                        log!("[AgentSession] Worktree is on branch '{}', tracked branch was '{}' — using actual branch",
                            actual, branch_name);
                        actual.as_str()
                    } else {
                        branch_name.as_str()
                    }
                } else {
                    branch_name.as_str()
                };

                let was_hardened =
                    branch_is_hardened(&self.pool, self.changes(), &repo_root, effective_branch)
                        .await;
                let has_commits = has_branch_commits(&repo_root, effective_branch).await;
                let changed_files = if has_commits {
                    branch_changed_files(&repo_root, effective_branch).await
                } else {
                    Vec::new()
                };

                // The worktree deliberately STAYS. A session end that is not an
                // explicit Discard reclaims nothing: `WorktreeCleanup` is the
                // single owner of that decision (ADR 0035). This call site used
                // to run `git worktree remove --force` here, unconditionally,
                // and it destroyed two live worktrees on 2026-08-03: once by
                // acting on a timed-out git probe, once by racing the safety
                // net's auto-resume into the very tree it deleted. Note also
                // that this path runs ONLY when a session ended abnormally, a
                // clean idle exit having returned earlier in `run.rs`. So the
                // disk it ever reclaimed was near zero, while its whole risk
                // surface was the failure paths.

                // A user cancel is a resumable turn boundary, not a terminator:
                // keep the branch even with no commits so the next message can
                // `--resume` this session, and never propose half-finished work.
                // Both real-cancel causes qualify: `UserStop` (Stop = Esc) and
                // `SupersededByFollowup` (a follow-up interrupted a mid-turn Codex
                // turn — the redirected-away partial work must NOT be proposed,
                // and the branch is kept so the follow-up turn resumes on it).
                // Normally the follow-up turn's own terminal overwrites this, but
                // if the follow-up never ran (routing failure / subprocess death
                // after the interrupt) the redirect cancel is the final terminal,
                // and it must still get the keep-branch/no-propose semantics.
                // Apply/Discard/Archive carry their own terminators and never
                // surface here as the turn's terminal kind.
                let user_canceled = matches!(
                    last_terminal_kind,
                    Some(TerminalKind::Canceled(
                        crate::engine::thread_events::CancelCause::UserStop
                            | crate::engine::thread_events::CancelCause::SupersededByFollowup
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
                            "[AgentSession] External repo branch {} — keeping branch, no change proposed",
                            effective_branch
                        );
                    }
                    SessionEndAction::KeepCanceledBranch => {
                        log!(
                            "[AgentSession] User cancelled (Esc) on branch {} — keeping branch resumable, no change proposed",
                            effective_branch
                        );
                    }
                    SessionEndAction::CrashedKeepBranch => {
                        log!(
                            "[AgentSession] Safety net fired with commits on {} — keeping branch on disk, NOT proposing change (thread ended in ResponseAborted)",
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
                            change_description_fallback(self.pool(), thread_id, effective_branch)
                                .await;
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
                                    consume_harden_marker(&self.pool, &repo_root, effective_branch)
                                        .await;
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
                                        coding_agent,
                                    )
                                    .await;
                                }
                            }
                            Err(e) => {
                                log!("[AgentSession] Failed to propose change: {}", e);
                            }
                        }
                    }
                    SessionEndAction::KeepEmptyBranch => {
                        // The session ended with no proposable diff (no commits, or
                        // commits whose changes cancel out). The thread is still ALIVE
                        // and resumable — the next message `--resume`s this branch via
                        // its `cc_session_id`, and `recover_orphaned_worktrees` keys on
                        // the branch ref to re-attach the session after a restart.
                        //
                        // Deleting the branch here — even with zero unique commits — is
                        // the thread-9e37697e data-loss class: a dropped branch forces a
                        // fresh branch from main and a dropped session id, discarding the
                        // thread's conversation continuity. So KEEP the branch. An empty
                        // branch is a cheap ref; the worktree_cleanup sweep reclaims
                        // fully-merged branches once the thread is archived (or under
                        // disk pressure). This mirrors `end_stale_waiting_session`'s
                        // "keep empty branches" recovery stance. The explicit
                        // Discard / conflict-resolution paths above are the only places
                        // that legitimately `git branch -D` (the user asked, or it's a
                        // throwaway merge temp branch).
                        if has_commits {
                            log!(
                                "[AgentSession] Branch {} has commits but empty diff vs main — keeping branch (session resumable), not proposing",
                                effective_branch
                            );
                        } else {
                            log!(
                                "[AgentSession] Branch {} has no proposable diff — keeping branch (session resumable), not proposing",
                                effective_branch
                            );
                        }
                        // Not proposing is not the same as leaving a stale row
                        // behind: if this branch already carries a pending
                        // change, re-sync it to zero files so its card stops
                        // advertising work the branch no longer has. Never
                        // discards, and never creates a row for a branch that
                        // has none — see `reconcile_emptied_pending_change`.
                        self.reconcile_emptied_pending_change(
                            thread_id,
                            &repo_root,
                            effective_branch,
                        )
                        .await;
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
