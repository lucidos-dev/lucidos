use super::idle_snapshot::{CodingAgentIdleSnapshot, IdleTurn};
use crate::engine::agent_recovery::recovery::AgentRecoveryExt;
use crate::engine::agent_session::lifecycle::{
    auto_resume_after_api_error, classify_session_end_action, conflict_abort_deletes_temp_state,
    conflict_resolution_cleanup_action, discarded_worktree_removal, held_completion_release,
    lane_thread_end, should_auto_commit_on_cleanup, transient_api_failure_error,
    ConflictResolutionCleanupAction, HeldCompletionRelease, SafetyNetAction, SafetyNetInput,
    SessionEndAction, TerminalKind, TurnCancel, WorktreeRemoval, MAX_API_ERROR_AUTO_RESUMES,
};
use crate::engine::agent_session::resume::api_error_auto_resumes_spent;
use crate::engine::agent_session::run_session::idle_snapshot::IdleSnapshotExt;
use crate::engine::agent_session::runtime_helpers::RuntimeHelpersExt;
use crate::engine::change_ops::{branch_is_hardened, ProposeOutcome};
use crate::engine::git_ops::{
    auto_commit_preserving_marker, branch_changed_files, commits_in_range, consume_harden_marker,
    default_local_branch, describe_branch_changes, ff_merge_to_main, files_have_client_update,
    files_require_restart, git_cmd, git_commit_no_edit, git_ran_ok, has_branch_commits,
    push_main_in_background, worktree_current_branch,
};
use crate::engine::session_follow_ups::{drain_lost_followups, lost_followups_to_orphans};
use crate::engine::session_queries::change_description_fallback;
use crate::engine::thread_events::EventChannel;
use crate::engine::{AgentSession, LucidosEngine, ProcessResult, StopReason};
use crate::runtime::AgentExitFacts;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use uuid::Uuid;

fn own_session_entry<'a>(
    sessions: &'a HashMap<Uuid, AgentSession>,
    thread_id: Uuid,
    token: &std::sync::atomic::AtomicBool,
) -> Option<&'a AgentSession> {
    sessions
        .get(&thread_id)
        .filter(|s| std::ptr::eq(Arc::as_ptr(&s.external_terminal_emitted), token))
}

pub(in crate::engine::agent_session) fn own_model_settings(
    sessions: &HashMap<Uuid, AgentSession>,
    thread_id: Uuid,
    token: &std::sync::atomic::AtomicBool,
) -> (Option<String>, Option<String>) {
    own_session_entry(sessions, thread_id, token)
        .map(|s| (s.current_model.clone(), s.reasoning_effort_in_force.clone()))
        .unwrap_or_default()
}

pub(in crate::engine::agent_session) fn note_effort_in_force(
    sessions: &mut HashMap<Uuid, AgentSession>,
    thread_id: Uuid,
    token: &std::sync::atomic::AtomicBool,
    effort: Option<&str>,
) {
    let Some(effort) = effort else { return };
    if own_session_entry(sessions, thread_id, token).is_some() {
        if let Some(session) = sessions.get_mut(&thread_id) {
            session.reasoning_effort_in_force = Some(effort.to_string());
        }
    }
}

pub(super) fn reap_own_session_entry(
    sessions: &mut HashMap<Uuid, AgentSession>,
    thread_id: Uuid,
    token: &std::sync::atomic::AtomicBool,
) -> Option<AgentSession> {
    if own_session_entry(sessions, thread_id, token).is_some() {
        return sessions.remove(&thread_id);
    }
    if sessions.contains_key(&thread_id) {
        log_legacy!(
            "[AgentSession] Teardown for thread {} left a replacement session's entry alone",
            thread_id
        );
    }
    None
}

fn mark_own_entry_exited(
    sessions: &mut HashMap<Uuid, AgentSession>,
    thread_id: Uuid,
    token: &std::sync::atomic::AtomicBool,
) {
    if own_session_entry(sessions, thread_id, token).is_none() {
        return;
    }
    if let Some(s) = sessions.get_mut(&thread_id) {
        s.process_exited = true;
        s.idle_notify.notify_waiters();
    }
}

fn reap_reading_stop(
    sessions: &mut HashMap<Uuid, AgentSession>,
    thread_id: Uuid,
    token: &std::sync::atomic::AtomicBool,
) -> Option<StopReason> {
    let stop = own_session_entry(sessions, thread_id, token).and_then(|s| s.pending_stop);
    reap_own_session_entry(sessions, thread_id, token);
    stop
}

fn effective_session_branch<'a>(tracked: &'a str, actual: Option<&'a str>, base: &str) -> &'a str {
    match actual {
        Some(actual) if actual != tracked && actual != base => {
            log_legacy!(
                "[AgentSession] Worktree is on branch '{}', tracked branch was '{}', using actual branch",
                actual,
                tracked
            );
            actual
        }
        _ => tracked,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ResumeStandDown {
    Shutdown,
    FollowUpQueued,
    StopPending,
}

pub(super) fn auto_resume_stand_down(
    shutting_down: bool,
    followups_queued: bool,
    stop_pending: bool,
) -> Option<ResumeStandDown> {
    if shutting_down {
        Some(ResumeStandDown::Shutdown)
    } else if followups_queued {
        Some(ResumeStandDown::FollowUpQueued)
    } else if stop_pending {
        Some(ResumeStandDown::StopPending)
    } else {
        None
    }
}

pub(super) enum IdleExit {
    Release(Option<Box<AgentSession>>),
    HonorStop,
}

pub(super) fn idle_exit(
    sessions: &mut HashMap<Uuid, AgentSession>,
    thread_id: Uuid,
    token: &std::sync::atomic::AtomicBool,
) -> IdleExit {
    if own_session_entry(sessions, thread_id, token).is_some_and(|s| s.pending_stop.is_some()) {
        return IdleExit::HonorStop;
    }
    IdleExit::Release(reap_own_session_entry(sessions, thread_id, token).map(Box::new))
}

pub(super) fn carries_out_late_discard(
    at_snapshot: Option<StopReason>,
    at_reap: Option<StopReason>,
    is_resolution_session: bool,
    another_session_coming: bool,
) -> bool {
    at_reap == Some(StopReason::Discard)
        && at_snapshot != Some(StopReason::Discard)
        && !is_resolution_session
        && !another_session_coming
}

async fn remove_discarded_worktree(
    worktree: &Path,
    repo_root: &Path,
    session_branch: &str,
    worktree_branch: Option<&str>,
) {
    match discarded_worktree_removal(worktree_branch, session_branch) {
        WorktreeRemoval::Keep(reason) => {
            log_legacy!(
                "[AgentSession] Keeping worktree {}: {}",
                worktree.display(),
                reason
            );
        }
        WorktreeRemoval::Remove => {
            let wt_path_str = worktree.to_string_lossy();
            if let Err(e) =
                git_ran_ok(&["worktree", "remove", "--force", &wt_path_str], repo_root).await
            {
                log_legacy!(
                    "[AgentSession] Failed to remove discarded worktree {}: {}",
                    worktree.display(),
                    e
                );
            }
        }
    }
}

#[easy_ext::ext(CompletionExt)]
pub(in crate::engine::agent_session) impl LucidosEngine {
    async fn terminal_model_settings(
        &self,
        thread_id: Uuid,
        token: &std::sync::atomic::AtomicBool,
    ) -> (Option<String>, Option<String>) {
        own_model_settings(&*self.agent_sessions.lock().await, thread_id, token)
    }

    async fn hold_completion_if_api_error_resume(
        &self,
        thread_id: Uuid,
        terminal: &Option<TerminalKind>,
        is_shutdown: bool,
        is_resolution_session: bool,
    ) {
        let Some(error) = transient_api_failure_error(terminal) else {
            return;
        };
        let resumes_spent = api_error_auto_resumes_spent(
            self.pool(),
            thread_id,
            crate::engine::agent_recovery::AUTO_RESUME_AFTER_API_ERROR_REASON,
        )
        .await;
        if !auto_resume_after_api_error(terminal, resumes_spent, is_shutdown, is_resolution_session)
        {
            log_legacy!(
                "[AgentSession] thread {} ended on a transient upstream API failure with {} of {} auto-resumes spent: not resuming, so its parent hears about the failure now",
                thread_id,
                resumes_spent,
                MAX_API_ERROR_AUTO_RESUMES,
            );
            return;
        }
        log_legacy!(
            "[AgentSession] thread {} ended on a transient upstream API failure ({} of {} auto-resumes already spent): resuming, and withholding its parent's completion card until the real terminal",
            thread_id,
            resumes_spent,
            MAX_API_ERROR_AUTO_RESUMES,
        );
        self.event_bus
            .auto_resume_holds()
            .hold(thread_id, error.to_string());
    }

    /// INVARIANT: Emit `ContinuationRequested{auto_resume_after_api_error}` when the turn
    /// ended on a TRANSIENT upstream failure the backend reported itself (its
    /// own `API Error: …`, e.g. a connection closed mid-response). Resume the
    /// session rather than leaving the thread dead behind a red dot: when the
    /// identical network failure shows up as SILENCE the watchdog already does
    /// exactly this, and the only reason the reported flavour had no recovery
    /// is that it arrives as a natural turn terminator instead of a hang. Two
    /// unattended runs sat dead for four and eight hours on 2026-08-04 for want
    /// of it.
    ///
    /// Returns whether a continuation was persisted, so finalize knows a
    /// resumed turn now owns the worktree.
    ///
    /// Bounded, unlike the watchdog's: see `auto_resume_after_api_error`.
    ///
    /// **It decides nothing.** `hold_completion_if_api_error_resume` already
    /// made the call, at terminal-classification time, and what its released
    /// *auto-resume hold* handed back is what this reads. That is deliberate:
    /// the fan-in stood down on that hold, so a second copy of the predicate
    /// here could disagree with a card already out the door.
    ///
    /// **Called from BOTH ways a session run can end**, and it has to be, which
    /// is the whole reason this is a helper rather than an inline block. The
    /// first version lived only in `finalize_direct_agent` and therefore never
    /// ran once: a turn that ends with a Result goes idle, and the idle-exit arm
    /// of the event loop returns straight out of `run_direct_agent` without
    /// reaching the post-loop finalize (see the call in `run.rs`). A reported
    /// API drop is always that shape, so the recovery was dead on arrival for
    /// four consecutive incidents. See
    /// `docs/plans/2026-08-05-api-drop-auto-resume-emit-site-unreachable.md`.
    ///
    /// The two call sites are mutually exclusive by construction (the idle-exit
    /// arm returns, every other exit breaks into finalize), so one
    /// `run_direct_agent` invocation emits at most one continuation and no
    /// idempotence guard is needed.
    ///
    /// "Both ways a session run can end" is true and was, on its own, incomplete:
    /// the idle handler can decline to end the run at all. When
    /// `terminate_decision` returns a `KeepAlive` the loop keeps the subprocess and
    /// continues, reaching neither site. That is a legitimate stand-down when a
    /// follow-up really is coming, and it was a silent cancellation of this recovery
    /// for four weeks while the decision guessed from a count of messages sent
    /// rather than reading the three windows a follow-up can actually be in
    /// (2026-08-07; see
    /// `docs/plans/2026-08-07-api-drop-resume-suppressed-by-phantom-followup-count.md`).
    ///
    /// Both sites also call it after their follow-up drain and pass
    /// `followups_queued`, because this recovery is for the UNATTENDED case. A
    /// drained follow-up is re-submitted by the caller, so the thread is already
    /// going to be driven, and resuming on top of it would inject "continue from
    /// where you left off" beside the user's real message.
    ///
    /// Both sites call this AFTER the turn's own `CodingAgentIdled` and after
    /// the subprocess is torn down. The ordering is load-bearing twice over: a
    /// `ContinuationRequested` is only re-dispatched at startup while no CC
    /// lifecycle event follows it (`CC_LIFECYCLE_EVENTS_SQL`), so emitting
    /// before the idle would make the durable orphan floor skip a request that
    /// was never dispatched; and emitting while the subprocess is still alive
    /// would race the spawn dispatcher into a session that is about to be
    /// cancelled.
    async fn maybe_auto_resume_after_api_error(
        &self,
        thread_id: Uuid,
        meta: &crate::engine::thread_events::EventMeta,
        withheld_error: Option<String>,
        followups_queued: bool,
        stop_pending: bool,
    ) -> bool {
        let Some(withheld_error) = withheld_error else {
            return false;
        };
        let shutting_down = self.is_shutting_down();
        let continuation_persisted = match auto_resume_stand_down(
            shutting_down,
            followups_queued,
            stop_pending,
        ) {
            Some(ResumeStandDown::Shutdown) => {
                log_legacy!(
                        "[AgentSession] thread {} would auto-resume after a transient upstream API failure, but the engine is shutting down: telling its parent instead of resuming",
                        thread_id,
                    );
                false
            }
            Some(ResumeStandDown::FollowUpQueued) => {
                log_legacy!(
                        "[AgentSession] thread {} ended on a transient upstream API failure, but a follow-up is already queued for it: leaving the thread to that message rather than resuming",
                        thread_id,
                    );
                false
            }
            Some(ResumeStandDown::StopPending) => {
                log_legacy!(
                        "[AgentSession] thread {} ended on a transient upstream API failure, but a Stop, Apply, Discard or Archive is pending for it: telling its parent instead of resuming",
                        thread_id,
                    );
                false
            }
            None => crate::engine::thread_events::emit_continuation_requested_or_log(
                &self.event_bus,
                thread_id,
                crate::engine::agent_recovery::AUTO_RESUME_AFTER_API_ERROR_REASON,
                meta.actor.clone(),
                "[AgentSession] ContinuationRequested (auto-resume after transient API failure)",
            )
            .await,
        };
        match held_completion_release(continuation_persisted, followups_queued) {
            HeldCompletionRelease::KeepWithholding => {}
            HeldCompletionRelease::Announce => {
                self.event_bus
                    .announce_withheld_completion(thread_id, withheld_error)
                    .await;
            }
        }
        continuation_persisted
    }

    #[allow(clippy::too_many_arguments)]
    async fn finalize_direct_agent(
        &self,
        thread_id: Uuid,
        request_id: Uuid,
        meta: &crate::engine::thread_events::EventMeta,
        conflict: Option<(
            crate::core::changes::Change,
            &crate::engine::change_ops::apply_claim::ApplyClaim,
        )>,
        lane_duty: Option<crate::engine::landing_lane::LaneDuty>,
        lane_answered: &mut bool,
        cwd: PathBuf,
        repo_root: PathBuf,
        branch_name: String,
        worktree_path: Option<PathBuf>,
        images: Vec<String>,
        msg_rx: &mut tokio::sync::mpsc::UnboundedReceiver<crate::engine::AgentUserInput>,
        claude_text_buf: String,
        last_terminal_kind: Option<TerminalKind>,
        withheld_api_error: Option<String>,
        external_terminal_emitted: Arc<std::sync::atomic::AtomicBool>,
        external_continuation_requested: Arc<std::sync::atomic::AtomicBool>,
        agent_cancel: &tokio_util::sync::CancellationToken,
        emitted_terminal_event: bool,
        watchdog_fired: bool,
        exit: AgentExitFacts,
        init_seen: bool,
        last_emitted_idle: bool,
        is_external_repo: bool,
        mut proposed_change: bool,
        coding_agent: crate::runtime::CodingAgent,
    ) -> Result<ProcessResult, Box<dyn std::error::Error + Send + Sync>> {
        {
            let mut guard = self.agent_sessions.lock().await;
            mark_own_entry_exited(&mut guard, thread_id, &external_terminal_emitted);
        }

        let cc_orphans = lost_followups_to_orphans(drain_lost_followups(msg_rx));

        let safety_net_fired = !emitted_terminal_event;
        let engine_cancelled = agent_cancel.is_cancelled();
        if safety_net_fired {
            log_legacy!(
                "[AgentSession] safety net firing for thread {} — buffered_text_len={}, watchdog_fired={}, killed_by_signal={}, exit_code={:?}, init_seen={}, engine_cancelled={}",
                thread_id,
                claude_text_buf.len(),
                watchdog_fired,
                exit.killed_by_signal,
                exit.code,
                init_seen,
                engine_cancelled,
            );
        }
        let is_shutdown = self.thread_session_is_shutting_down(thread_id).await;
        let external_already =
            crate::engine::agent_session::runtime_helpers::external_terminal_already_emitted(
                &self.pool,
                &external_terminal_emitted,
                thread_id,
                meta.request_event_id,
                is_shutdown,
                "safety net",
            )
            .await;
        let net_action =
            crate::engine::agent_session::lifecycle::safety_net_action(&SafetyNetInput {
                safety_net_fired,
                watchdog_fired,
                external_terminal_already_emitted: external_already,
                engine_cancelled,
                init_seen,
                exit: &exit,
                coding_agent,
            });
        match &net_action {
            SafetyNetAction::Nothing | SafetyNetAction::Skip => {}
            SafetyNetAction::EmitFailed { error, agent_exit } => {
                log_legacy!(
                    "[AgentSession] thread {}: {} exited on its own ({:?}, code {:?}); failing the turn with its words and not retrying",
                    thread_id,
                    coding_agent.display_name(),
                    agent_exit.stage,
                    agent_exit.code,
                );
                self.event_bus
                    .emit_or_log(
                        crate::engine::event_bus::BusEvent::Thread {
                            thread_id,
                            event: crate::engine::thread_events::ThreadEvent::ResponseFailed {
                                error: error.clone(),
                                spend_cap: crate::llm::spend_cap::SpendCapNotice::from_agent_text(
                                    "Coding agent",
                                    error,
                                ),
                                agent_exit: Some(*agent_exit),
                                provider_error: None,
                            },
                            meta: meta.clone(),
                        },
                        "[AgentSession] safety-net ResponseFailed (the agent exited on its own)",
                    )
                    .await;
            }
            SafetyNetAction::EmitContinuationRequested => {
                external_continuation_requested.store(true, std::sync::atomic::Ordering::Release);
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
                Self::stamp_host_actor_if_aborted(
                    &mut emit_meta,
                    true,
                    crate::engine::thread_events::MessageOrigin::system(),
                );
                let (model, reasoning_effort) = self
                    .terminal_model_settings(thread_id, &external_terminal_emitted)
                    .await;
                crate::engine::thread_events::emit_response_aborted(
                    &self.event_bus,
                    thread_id,
                    crate::engine::thread_events::AbortCause::SafetyNet,
                    claude_text_buf.clone(),
                    vec![],
                    model,
                    reasoning_effort,
                    emit_meta,
                    "[AgentSession] safety-net ResponseAborted",
                )
                .await;
            }
        }

        let pending_stop = {
            let guard = self.agent_sessions.lock().await;
            own_session_entry(&guard, thread_id, &external_terminal_emitted)
                .and_then(|s| s.pending_stop)
        };
        let should_discard = matches!(pending_stop, Some(StopReason::Discard));
        let is_resolution_session = conflict.is_some() || lane_duty.is_some();
        if !is_shutdown
            && !matches!(
                pending_stop,
                Some(StopReason::Apply | StopReason::Discard | StopReason::Archive)
            )
            && (crate::engine::thread_stop::thread_is_stopped(self.pool(), thread_id).await
                || matches!(last_terminal_kind, Some(TerminalKind::Failed { .. }))
                || matches!(
                    net_action,
                    SafetyNetAction::EmitFailed { .. } | SafetyNetAction::EmitAbortedSafetyNet
                ))
        {
            self.settle_owed_ceiling_handoff(thread_id).await;
        }

        let continuation_requested = self
            .maybe_auto_resume_after_api_error(
                thread_id,
                meta,
                withheld_api_error,
                !cc_orphans.is_empty(),
                pending_stop.is_some(),
            )
            .await;

        agent_cancel.cancel();

        let is_shutdown = self.thread_session_is_shutting_down(thread_id).await;

        if is_shutdown {
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
            reap_own_session_entry(&mut guard, thread_id, &external_terminal_emitted);
            log_legacy!(
                "[Shutdown] Skipping cleanup for thread {}: session will resume after restart",
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
                orphaned_injections: cc_orphans,
            });
        }

        if let Some(duty) = lane_duty {
            let has_unmerged = crate::engine::git_ops::has_unmerged_paths(&cwd)
                .await
                .or_unknown(true);
            let continuation_pending = net_action == SafetyNetAction::EmitContinuationRequested
                || external_continuation_requested.load(std::sync::atomic::Ordering::Acquire);
            *lane_answered = true;
            match lane_thread_end(
                has_unmerged,
                &last_terminal_kind,
                continuation_pending,
                pending_stop,
            ) {
                Some(end) => self.report_lane_thread_end(duty.batch_id, thread_id, end),
                None => log_legacy!(
                    "[AgentSession] Lane batch {} handed off to the continuation of thread {}",
                    duty.batch_id,
                    thread_id
                ),
            }
        } else if let Some((change, apply_claim)) = conflict {
            let has_unmerged = crate::engine::git_ops::has_unmerged_paths(&cwd)
                .await
                .or_unknown(true);

            let wt_str = cwd.to_string_lossy();

            let cleanup_action = if apply_claim.is_current() {
                conflict_resolution_cleanup_action(
                    has_unmerged,
                    &last_terminal_kind,
                    net_action == SafetyNetAction::EmitContinuationRequested
                        || external_continuation_requested
                            .load(std::sync::atomic::Ordering::Acquire),
                )
            } else {
                ConflictResolutionCleanupAction::HandOff
            };
            match cleanup_action {
                ConflictResolutionCleanupAction::HandOff => {
                    log_legacy!(
                        "[AgentSession] Conflict resolution for {} handed off to auto-recovery continuation — change {} stays pending, merge state preserved",
                        change.branch_name,
                        change.id
                    );
                }
                ConflictResolutionCleanupAction::Abort { message } => {
                    let apply_actor = self.pending_apply_actors.take(change.id);
                    let _ = git_cmd(&["merge", "--abort"], &cwd).await;
                    log_legacy!(
                        "[AgentSession] Conflict resolution not applied for {}: {}",
                        change.branch_name,
                        message
                    );
                    if conflict_abort_deletes_temp_state(
                        change.merge_worktree().map(|m| m.temp_branch.as_str()),
                        &branch_name,
                    ) {
                        if let Err(e) =
                            git_ran_ok(&["worktree", "remove", "--force", &wt_str], &repo_root)
                                .await
                        {
                            log_legacy!(
                                "[ConflictResolution] Failed to remove temp merge worktree {}: {}",
                                wt_str,
                                e
                            );
                        }
                        if let Err(e) =
                            git_ran_ok(&["branch", "-D", &branch_name], &repo_root).await
                        {
                            log_legacy!(
                                "[ConflictResolution] Failed to delete temp merge branch {}: {}",
                                branch_name,
                                e
                            );
                        }
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
                    let merge_in_progress =
                        crate::engine::git_ops::git_answer(&["rev-parse", "MERGE_HEAD"], &cwd)
                            .await
                            .or_unknown(true);
                    if merge_in_progress {
                        if let Err(e) = git_commit_no_edit(&cwd).await {
                            log_legacy!(
                                "[ConflictResolution] {} (change {}, branch {})",
                                e,
                                change.id,
                                change.branch_name
                            );
                        }
                    }

                    let bound_refusal = self
                        .bounded_fix_refusal_for_resolution(
                            &repo_root,
                            Path::new(wt_str.as_ref()),
                            &change.branch_name,
                            &branch_name,
                        )
                        .await;
                    let merge_result = match bound_refusal {
                        Some(msg) => Err(msg.into()),
                        None => {
                            ff_merge_to_main(&repo_root, &wt_str, &branch_name, &change.branch_name)
                                .await
                        }
                    };
                    match merge_result {
                        Ok((pre_sha, post_sha)) => {
                            let commits = commits_in_range(&repo_root, &pre_sha, &post_sha).await;
                            let kind_ctx = crate::engine::change_ops::load_apply_kind_context(
                                &self.pool,
                                change.thread_id,
                            )
                            .await;
                            let requires_restart_effective =
                                change.requires_restart && !kind_ctx.is_app();
                            if !kind_ctx.is_app() {
                                push_main_in_background(&repo_root, &self.event_bus);
                            }
                            self.emit_change_applied(
                                apply_claim,
                                change.thread_id.unwrap_or(thread_id),
                                requires_restart_effective,
                                files_have_client_update(&change.files),
                                commits,
                                change.thread_title.clone(),
                                apply_actor.clone(),
                                Some(crate::engine::change_ops::AppliedMerge::new(
                                    &pre_sha, &post_sha,
                                )),
                            )
                            .await?;
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
                                Some(change.thread_id.unwrap_or(thread_id)),
                            )
                            .await;
                            self.emit_merge_resolution_cleared(
                                change.thread_id.unwrap_or(thread_id),
                                change.id,
                                "[ConflictResolution] MergeResolutionCleared",
                            )
                            .await;
                            log_legacy!(
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
                            log_legacy!(
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
            // INVARIANT: Actor ordering tests use this boundary: // Normal worktree cleanup
            let wt = worktree_path.as_ref().unwrap();

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
                let pending_lookup = self
                    .changes()
                    .get_open_by_branch(&branch_name)
                    .await
                    .unwrap_or_else(|e| {
                        log_legacy!(
                            "[AgentSession] get_open_by_branch({}): {}; skipping discard emit",
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
                self.event_bus
                    .settle_child(thread_id, crate::engine::event_bus::ChildSettle::Discarded)
                    .await;
                let discard_branch = worktree_current_branch(wt).await;
                remove_discarded_worktree(wt, &repo_root, &branch_name, discard_branch.as_deref())
                    .await;
                log_legacy!("[AgentSession] Discarding changes (branch {})", branch_name);
                if let Err(e) = git_ran_ok(&["branch", "-D", &branch_name], &repo_root).await {
                    log_legacy!(
                        "[AgentSession] Failed to delete branch {}: {}",
                        branch_name,
                        e
                    );
                }
            } else {
                let actual_branch = worktree_current_branch(wt).await;
                let base = default_local_branch(&repo_root).await;
                let effective_branch =
                    effective_session_branch(&branch_name, actual_branch.as_deref(), &base);

                let was_hardened =
                    branch_is_hardened(&self.pool, self.changes(), &repo_root, effective_branch)
                        .await;
                let has_commits = has_branch_commits(&repo_root, effective_branch).await;
                let changed_files = if has_commits {
                    branch_changed_files(&repo_root, effective_branch).await
                } else {
                    Vec::new()
                };

                match classify_session_end_action(
                    has_commits,
                    changed_files.is_empty(),
                    is_external_repo,
                    safety_net_fired,
                    TurnCancel::of(&last_terminal_kind),
                    crate::engine::pending_handoff::handoff_pending(self.pool(), thread_id).await,
                ) {
                    SessionEndAction::CarriedBySuccessor => {
                        match crate::engine::change_ops::withhold_handed_off_work(
                            self.pool(),
                            &self.event_bus,
                            thread_id,
                            &repo_root,
                            effective_branch,
                        )
                        .await
                        {
                            Ok(true) => self.broadcast_changes_updated().await,
                            Ok(false) => {}
                            Err(e) => log_legacy!(
                                "[AgentSession] Could not withdraw the handed-off change of thread {}: {}",
                                thread_id,
                                e
                            ),
                        }
                    }
                    SessionEndAction::KeepExternalBranch => {
                        log_legacy!(
                            "[AgentSession] External repo branch {} — keeping branch, no change proposed",
                            effective_branch
                        );
                    }
                    SessionEndAction::KeepCanceledBranch => {
                        log_legacy!(
                            "[AgentSession] Turn cancelled on branch {} with nothing to propose; keeping branch resumable",
                            effective_branch
                        );
                    }
                    SessionEndAction::CrashedKeepBranch => {
                        log_legacy!(
                            "[AgentSession] Safety net fired with commits on {} — keeping branch on disk, NOT proposing change (thread ended in ResponseAborted)",
                            effective_branch
                        );
                    }
                    action @ (SessionEndAction::Propose | SessionEndAction::WithholdUnfinished) => {
                        let requires_restart = files_require_restart(&changed_files);

                        log_legacy!(
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
                            .propose_turn_work(
                                crate::engine::change_ops::ProposeChangeInput {
                                    thread_id,
                                    branch_name: effective_branch,
                                    repo_root: &repo_root_str,
                                    description: &description,
                                    files: &changed_files,
                                    requires_restart,
                                    channel: EventChannel::ClaudeCode,
                                    hardened: was_hardened,
                                    origin: None,
                                },
                                action == SessionEndAction::Propose,
                                crate::engine::change_ops::HoldReentry::default(),
                            )
                            .await
                        {
                            Ok(ProposeOutcome::Proposed(_)) => {
                                proposed_change = true;
                                if was_hardened {
                                    consume_harden_marker(&self.pool, &repo_root, effective_branch)
                                        .await;
                                }
                                if !last_emitted_idle {
                                    self.emit_coding_agent_idled(
                                        thread_id,
                                        CodingAgentIdleSnapshot {
                                            has_changes: !changed_files.is_empty(),
                                            is_external_repo,
                                            requires_restart,
                                            bg_bash_pending: false,
                                            worktree_path: worktree_path.as_deref(),
                                            turn: IdleTurn::of(
                                                self.event_bus.in_keep_alive_turn(thread_id),
                                                self.agent_sessions
                                                    .lock()
                                                    .await
                                                    .get(&thread_id)
                                                    .is_some_and(|s| s.in_ceiling_reentry),
                                            ),
                                            turn_end: None,
                                        },
                                        meta,
                                        coding_agent,
                                    )
                                    .await;
                                }
                            }
                            Ok(
                                ProposeOutcome::Held(_)
                                | ProposeOutcome::Unfinished
                                | ProposeOutcome::AwaitingLanding(_)
                                | ProposeOutcome::Refused(_),
                            ) => {}
                            Err(e) => {
                                log_legacy!("[AgentSession] Failed to propose change: {}", e);
                            }
                        }
                    }
                    SessionEndAction::KeepEmptyBranch => {
                        if has_commits {
                            log_legacy!(
                                "[AgentSession] Branch {} has commits but empty diff vs main — keeping branch (session resumable), not proposing",
                                effective_branch
                            );
                        } else {
                            log_legacy!(
                                "[AgentSession] Branch {} has no proposable diff — keeping branch (session resumable), not proposing",
                                effective_branch
                            );
                        }
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

        let pending_stop_at_reap = {
            let mut guard = self.agent_sessions.lock().await;
            reap_reading_stop(&mut guard, thread_id, &external_terminal_emitted)
        };
        let auto_apply = matches!(pending_stop_at_reap, Some(StopReason::Apply));

        let another_session_coming =
            continuation_requested || net_action == SafetyNetAction::EmitContinuationRequested;
        if carries_out_late_discard(
            pending_stop,
            pending_stop_at_reap,
            is_resolution_session,
            another_session_coming,
        ) {
            match self.hold_thread_for_worktree_reset(thread_id).await {
                Ok(Some(_hold)) => {
                    log_legacy!(
                        "[AgentSession] A Discard for thread {} arrived after its cleanup; carrying it out now",
                        thread_id
                    );
                    if let Err(e) = self
                        .clone_arc()
                        .end_stale_waiting_session(thread_id, true, None)
                        .await
                    {
                        log_legacy!(
                            "[AgentSession] Late Discard for thread {} failed: {}",
                            thread_id,
                            e
                        );
                    }
                    self.event_bus
                        .settle_child(thread_id, crate::engine::event_bus::ChildSettle::Discarded)
                        .await;
                    proposed_change = false;
                }
                Ok(None) => log_legacy!(
                    "[AgentSession] A late Discard for thread {} found a new run on the thread; leaving its worktree alone",
                    thread_id
                ),
                Err(e) => log_legacy!(
                    "[AgentSession] Could not hold thread {} for its late Discard: {}",
                    thread_id,
                    e
                ),
            }
        }

        // INVARIANT: CC text was already streamed via ClaudeCodeText progress events.
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::AgentUserInput;
    use tokio::sync::mpsc::UnboundedReceiver;

    fn session_with(
        process_exited: bool,
    ) -> (
        AgentSession,
        Arc<std::sync::atomic::AtomicBool>,
        UnboundedReceiver<AgentUserInput>,
    ) {
        let (mut session, rx) = AgentSession::for_test();
        session.process_exited = process_exited;
        let token = session.external_terminal_emitted.clone();
        (session, token, rx)
    }

    #[test]
    fn the_teardown_reaps_its_own_entry() {
        let thread_id = Uuid::new_v4();
        let (session, token, _rx) = session_with(true);
        let mut sessions = HashMap::from([(thread_id, session)]);

        assert!(own_session_entry(&sessions, thread_id, &token).is_some());
        let reaped = reap_own_session_entry(&mut sessions, thread_id, &token);
        assert!(
            reaped.is_some_and(|s| Arc::ptr_eq(&s.external_terminal_emitted, &token)),
            "the reap hands back the entry it removed"
        );
        assert!(
            sessions.is_empty(),
            "the teardown must still give its own entry back"
        );
    }

    #[test]
    fn a_replacement_session_survives_even_when_it_reads_dead() {
        let thread_id = Uuid::new_v4();
        let (_outgoing, our_token, _our_rx) = session_with(true);
        let (replacement, _their_token, _their_rx) = session_with(true);
        let mut sessions = HashMap::from([(thread_id, replacement)]);

        assert!(
            own_session_entry(&sessions, thread_id, &our_token).is_none(),
            "identity, not liveness: an exited replacement is still not ours"
        );
        assert!(
            reap_own_session_entry(&mut sessions, thread_id, &our_token).is_none(),
            "the outgoing teardown reaps nothing"
        );
        assert!(
            sessions.contains_key(&thread_id),
            "the replacement's entry must survive the outgoing teardown"
        );
    }

    #[test]
    fn a_terminal_reports_the_entry_model_and_the_effort_in_force() {
        let thread_id = Uuid::new_v4();
        let (mut session, token, _rx) = session_with(false);
        session.current_model = Some("spawn-model".into());
        session.current_reasoning_effort = Some("high".into());
        session.reasoning_effort_in_force = Some("low".into());
        let mut sessions = HashMap::from([(thread_id, session)]);

        assert_eq!(
            own_model_settings(&sessions, thread_id, &token),
            (Some("spawn-model".into()), Some("low".into()))
        );

        let (replacement, _their_token, _their_rx) = session_with(false);
        sessions.insert(thread_id, replacement);
        assert_eq!(
            own_model_settings(&sessions, thread_id, &token),
            (None, None),
            "a replacement's model belongs to its own turn"
        );
    }

    #[test]
    fn the_effort_in_force_follows_the_calls_of_this_run_only() {
        let thread_id = Uuid::new_v4();
        let (mut session, token, _rx) = session_with(false);
        session.reasoning_effort_in_force = Some("medium".into());
        let mut sessions = HashMap::from([(thread_id, session)]);

        note_effort_in_force(&mut sessions, thread_id, &token, Some("high"));
        note_effort_in_force(&mut sessions, thread_id, &token, None);
        assert_eq!(
            own_model_settings(&sessions, thread_id, &token)
                .1
                .as_deref(),
            Some("high")
        );

        let (replacement, their_token, _their_rx) = session_with(false);
        sessions.insert(thread_id, replacement);
        note_effort_in_force(&mut sessions, thread_id, &token, Some("low"));
        assert_eq!(
            own_model_settings(&sessions, thread_id, &their_token).1,
            None,
            "an outgoing run never writes into its replacement"
        );
    }

    #[test]
    fn a_stop_accepted_during_idle_terminate_reaches_finalize() {
        let thread_id = Uuid::new_v4();
        for reason in [StopReason::Apply, StopReason::Discard, StopReason::Archive] {
            let (mut session, token, _rx) = session_with(true);
            session.pending_stop = Some(reason);
            let mut sessions = HashMap::from([(thread_id, session)]);

            assert!(
                matches!(
                    idle_exit(&mut sessions, thread_id, &token),
                    IdleExit::HonorStop
                ),
                "{reason:?} was accepted and must not be dropped"
            );
            assert!(
                sessions.contains_key(&thread_id),
                "finalize reads {reason:?} off the entry, so it must stay"
            );
        }
    }

    #[test]
    fn finalize_never_auto_resumes_over_a_pending_stop() {
        const SRC: &str = include_str!("completion.rs");
        let body = &SRC[SRC
            .find("async fn finalize_direct_agent(")
            .expect("finalize is still here")..];
        let snapshot = body
            .find("let pending_stop = {")
            .expect("finalize snapshots its own pending stop");
        let call = body
            .find("maybe_auto_resume_after_api_error(")
            .expect("finalize still reaches the auto-resume");
        let args = &body[call..call + body[call..].find(".await").unwrap()];
        assert!(
            snapshot < call,
            "the snapshot must come before the decision"
        );
        assert!(
            args.contains("pending_stop.is_some()"),
            "the auto-resume must hear about the pending stop: {args}"
        );
    }

    #[test]
    fn a_discard_that_missed_the_cleanup_is_still_carried_out() {
        use StopReason::{Apply, Discard};
        assert!(carries_out_late_discard(None, Some(Discard), false, false));

        assert!(
            !carries_out_late_discard(Some(Discard), Some(Discard), false, false),
            "the cleanup already discarded"
        );
        assert!(!carries_out_late_discard(None, Some(Apply), false, false));
        assert!(!carries_out_late_discard(None, None, false, false));
        assert!(!carries_out_late_discard(None, Some(Discard), true, false));
        assert!(!carries_out_late_discard(None, Some(Discard), false, true));
    }

    #[test]
    fn an_idle_exit_with_no_stop_releases_its_own_entry() {
        let thread_id = Uuid::new_v4();
        let (session, token, _rx) = session_with(true);
        let mut sessions = HashMap::from([(thread_id, session)]);

        assert!(matches!(
            idle_exit(&mut sessions, thread_id, &token),
            IdleExit::Release(Some(_))
        ));
        assert!(sessions.is_empty());
    }

    #[test]
    fn an_idle_exit_ignores_a_replacements_pending_stop() {
        let thread_id = Uuid::new_v4();
        let (_outgoing, our_token, _our_rx) = session_with(true);
        let (mut replacement, _their_token, _their_rx) = session_with(false);
        replacement.pending_stop = Some(StopReason::Apply);
        let mut sessions = HashMap::from([(thread_id, replacement)]);

        assert!(matches!(
            idle_exit(&mut sessions, thread_id, &our_token),
            IdleExit::Release(None)
        ));
        assert!(sessions.contains_key(&thread_id));
    }

    #[test]
    fn run_rs_never_removes_a_session_entry_by_thread_id_alone() {
        const RUN_SRC: &str = include_str!("run.rs");
        let src = &RUN_SRC[..RUN_SRC.find("#[cfg(test)]").unwrap_or(RUN_SRC.len())];
        for (at, _) in src.match_indices(".remove(&thread_id)") {
            let lock = src[..at]
                .rfind(".lock()")
                .expect("every removal in run.rs sits under a lock");
            let start = src[..lock].rfind([';', '{']).map_or(0, |n| n + 1);
            let statement = src[start..lock].trim();
            assert!(
                !statement.contains("agent_sessions"),
                "run.rs removes an agent_sessions entry by thread id alone, which deletes a \
                 replacement session's entry. Use `reap_own_session_entry`. Lock: {statement}"
            );
        }
        assert!(
            src.contains("idle_exit("),
            "the idle exit reaps through `idle_exit`, which wraps the shared helper"
        );
    }

    #[test]
    fn a_replacements_pending_stop_is_not_read_by_this_teardown() {
        let thread_id = Uuid::new_v4();
        let (_outgoing, our_token, _our_rx) = session_with(true);
        let (mut replacement, _their_token, _their_rx) = session_with(true);
        replacement.pending_stop = Some(StopReason::Discard);
        let sessions = HashMap::from([(thread_id, replacement)]);

        assert!(
            own_session_entry(&sessions, thread_id, &our_token)
                .and_then(|s| s.pending_stop)
                .is_none(),
            "a Discard aimed at the replacement must not reach this teardown"
        );
    }

    #[test]
    fn reaping_an_absent_entry_does_nothing() {
        let (_session, token, _rx) = session_with(true);
        let mut sessions: HashMap<Uuid, AgentSession> = HashMap::new();
        reap_own_session_entry(&mut sessions, Uuid::new_v4(), &token);
        assert!(sessions.is_empty());
    }

    const COMPLETION_SRC: &str = include_str!("completion.rs");

    fn production_src() -> &'static str {
        let cut = COMPLETION_SRC
            .find("#[cfg(test)]")
            .expect("completion.rs still carries its test module marker");
        &COMPLETION_SRC[..cut]
    }

    fn line_at(src: &str, at: usize) -> &str {
        let start = src[..at].rfind('\n').map(|n| n + 1).unwrap_or(0);
        src[start..].lines().next().unwrap_or_default().trim()
    }

    #[test]
    fn every_branch_delete_here_reports_its_refusal() {
        let src = production_src();
        let sites: Vec<usize> = src
            .match_indices(r#"&["branch", "-D","#)
            .map(|m| m.0)
            .collect();
        for at in &sites {
            let line = line_at(src, *at);
            assert!(
                line.contains("git_ran_ok("),
                "a `git branch -D` in completion.rs must run through `git_ran_ok`, \
                 which folds a non-zero exit into `Err`. `git_cmd` calls a refused \
                 delete `Ok` and the log never names it. Offending line: {line}"
            );
        }
        assert_eq!(
            sites.len(),
            2,
            "completion.rs deletes two branches: the one an explicit Discard threw \
             away, and the Tier-3 temp branch the merge attempt made"
        );
    }

    #[tokio::test]
    async fn git_refuses_to_delete_a_branch_a_worktree_holds() {
        const BRANCH: &str = "claude-code/20260803-045152-discard";
        let (_tmp, repo, wt) = crate::test_support::make_repo_and_worktree(BRANCH).await;
        assert!(wt.exists(), "the worktree is what makes git refuse");

        assert!(
            git_cmd(&["branch", "-D", BRANCH], &repo).await.is_ok(),
            "git_cmd reads a refused delete as Ok, which is why the old site was silent"
        );
        assert!(
            git_ran_ok(&["branch", "-D", BRANCH], &repo).await.is_err(),
            "git_ran_ok must surface the refusal so the log can name it"
        );
        assert!(
            crate::engine::git_ops::branch_head_sha(&repo, BRANCH)
                .await
                .is_some(),
            "and the branch survived both attempts, discarded commits and all"
        );
    }

    #[test]
    fn every_exit_here_returns_the_drained_followups() {
        let src = production_src();
        let exits: Vec<usize> = src
            .match_indices("orphaned_injections:")
            .map(|m| m.0)
            .collect();
        for at in &exits {
            let line = line_at(src, *at);
            assert!(
                line.contains("cc_orphans"),
                "every `ProcessResult` in completion.rs hands the drained follow-ups \
                 back. An exit returning an empty list drops a persisted user message \
                 with nothing in the log naming it. Offending line: {line}"
            );
        }
        assert_eq!(
            exits.len(),
            2,
            "completion.rs has two exits: the shutdown one and the normal one"
        );
    }

    fn squash(s: &str) -> String {
        s.split_whitespace().collect::<Vec<_>>().join(" ")
    }

    fn finalize_body() -> String {
        let src = production_src();
        let at = src
            .find("async fn finalize_direct_agent(")
            .expect("finalize is still here");
        squash(&src[at..])
    }

    #[test]
    fn marking_exited_leaves_a_replacement_entry_alone() {
        let thread_id = Uuid::new_v4();
        let (own, token, _rx) = session_with(false);
        let mut sessions = HashMap::from([(thread_id, own)]);

        mark_own_entry_exited(&mut sessions, thread_id, &token);
        assert!(
            sessions[&thread_id].process_exited,
            "our own entry is stamped"
        );

        let (replacement, _their_token, _their_rx) = session_with(false);
        sessions.insert(thread_id, replacement);
        mark_own_entry_exited(&mut sessions, thread_id, &token);
        assert!(
            !sessions[&thread_id].process_exited,
            "stamping a replacement dead lets the reap mistake it for ours"
        );
    }

    #[test]
    fn finalize_stamps_its_entry_exited_before_it_emits_anything() {
        let body = finalize_body();
        let stamp = body
            .find("mark_own_entry_exited(")
            .expect("finalize stamps its entry exited");
        let net = body
            .find("match &net_action {")
            .expect("finalize acts on the safety net");
        let first_emit = body.find("emit_or_log(").expect("finalize emits events");
        assert!(
            stamp < net && stamp < first_emit,
            "a follow-up arriving after the loop must see the entry exited before any terminal event"
        );
    }

    #[test]
    fn reaping_reads_the_stop_and_drops_the_entry_in_one_call() {
        let thread_id = Uuid::new_v4();
        let (mut own, token, _rx) = session_with(true);
        own.pending_stop = Some(StopReason::Apply);
        let mut sessions = HashMap::from([(thread_id, own)]);

        assert_eq!(
            reap_reading_stop(&mut sessions, thread_id, &token),
            Some(StopReason::Apply)
        );
        assert!(sessions.is_empty(), "the same call drops the entry");

        let (mut replacement, _their_token, _their_rx) = session_with(false);
        replacement.pending_stop = Some(StopReason::Discard);
        sessions.insert(thread_id, replacement);
        assert_eq!(reap_reading_stop(&mut sessions, thread_id, &token), None);
        assert!(sessions.contains_key(&thread_id));
    }

    #[test]
    fn a_queued_follow_up_stands_the_auto_resume_down() {
        assert_eq!(
            auto_resume_stand_down(false, true, false),
            Some(ResumeStandDown::FollowUpQueued)
        );
        assert_eq!(auto_resume_stand_down(false, false, false), None);
    }

    #[test]
    fn a_pending_stop_stands_the_auto_resume_down() {
        assert_eq!(
            auto_resume_stand_down(false, false, true),
            Some(ResumeStandDown::StopPending)
        );
    }

    #[test]
    fn a_shutdown_stands_the_auto_resume_down_first() {
        assert_eq!(
            auto_resume_stand_down(true, true, true),
            Some(ResumeStandDown::Shutdown)
        );
        assert_eq!(
            auto_resume_stand_down(true, false, false),
            Some(ResumeStandDown::Shutdown)
        );
    }

    #[test]
    fn the_resume_helper_and_finalize_pass_follow_ups_and_stops_through() {
        let flat: String = production_src()
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect::<String>()
            .replace(",)", ")");
        assert!(
            flat.contains("auto_resume_stand_down(shutting_down,followups_queued,stop_pending)"),
            "the helper must decide its stand-down through auto_resume_stand_down"
        );
        let body = finalize_body();
        let call = body
            .find("maybe_auto_resume_after_api_error(")
            .expect("finalize reaches the auto-resume");
        let args = &body[call..call + body[call..].find(".await").unwrap()];
        assert!(
            args.contains("!cc_orphans.is_empty()"),
            "finalize must tell the auto-resume about the drained follow-ups: {args}"
        );
    }

    #[test]
    fn the_withheld_card_is_announced_when_no_resume_was_persisted() {
        let src = squash(production_src());
        let helper = src
            .find("async fn maybe_auto_resume_after_api_error(")
            .expect("the resume helper is still here");
        let tail = &src[helper..];
        let decision = tail
            .find("held_completion_release(continuation_persisted, followups_queued)")
            .expect("the release decision takes the persisted flag and the follow-up flag");
        let arms = &tail[decision..];
        let announce = arms
            .find("HeldCompletionRelease::Announce =>")
            .expect("the Announce arm is still here");
        let end = arms
            .find(" continuation_persisted }")
            .expect("the helper returns the flag");
        assert!(
            arms[announce..end].contains("announce_withheld_completion(thread_id, withheld_error)"),
            "the Announce arm must tell the parent, or nothing else moves the thread"
        );
    }

    #[test]
    fn the_continuation_flag_is_set_before_the_continuation_is_emitted() {
        let body = finalize_body();
        let arm = body
            .find("SafetyNetAction::EmitContinuationRequested => {")
            .expect("the continuation arm is still here");
        let arm = &body[arm..];
        let store = arm
            .find("external_continuation_requested.store(true")
            .expect("the arm sets the continuation flag");
        let emit = arm
            .find("emit_continuation_requested_or_log(")
            .expect("the arm emits the continuation");
        assert!(
            store < emit,
            "a continuation that takes the apply claim must already see the flag"
        );
    }

    #[test]
    fn the_safety_net_abort_is_stamped_with_the_system_actor() {
        let body = finalize_body();
        let arm = body
            .find("SafetyNetAction::EmitAbortedSafetyNet => {")
            .expect("the abort arm is still here");
        let arm = &body[arm..];
        let arm = &arm[..arm
            .find("emit_response_aborted(")
            .expect("the arm emits the abort")];
        assert!(arm.contains("stamp_host_actor_if_aborted("));
        assert!(
            arm.contains("MessageOrigin::system()"),
            "a device actor would read as Paused by restart and promise a resume"
        );
        assert!(!arm.contains("teardown_actor"));
    }

    #[test]
    fn finalize_feeds_the_safety_net_flag_to_the_session_end_decision() {
        let body = finalize_body();
        assert!(
            body.contains("safety_net_fired, TurnCancel::of("),
            "a safety-net run must not propose its partial commits as a change"
        );
        assert!(body.contains("SafetyNetInput { safety_net_fired, watchdog_fired,"));
    }

    #[test]
    fn the_engine_cancel_flag_is_read_before_finalize_cancels_the_token() {
        let body = finalize_body();
        let read = body
            .find("engine_cancelled = agent_cancel.is_cancelled()")
            .expect("finalize reads whether the engine cancelled the session");
        let cancel = body
            .find("agent_cancel.cancel();")
            .expect("finalize cancels the token");
        assert!(
            read < cancel,
            "reading after the cancel makes our own teardown look like an engine cancel"
        );
        assert!(body.contains("engine_cancelled, init_seen,"));
    }

    #[test]
    fn finalize_asks_about_shutdown_again_before_its_cleanup() {
        let body = finalize_body();
        const ASK: &str = "self.thread_session_is_shutting_down(thread_id).await";
        let asks: Vec<usize> = body.match_indices(ASK).map(|m| m.0).collect();
        assert_eq!(
            asks.len(),
            2,
            "one ask for the safety net and a fresh one for the destructive cleanup"
        );
        let cancel = body.find("agent_cancel.cancel();").unwrap();
        let guard = body.find("if is_shutdown {").expect("the shutdown exit");
        assert!(asks[0] < cancel && cancel < asks[1] && asks[1] < guard);
    }

    #[test]
    fn a_shutdown_returns_before_any_cleanup_runs() {
        let body = finalize_body();
        let start = body.find("if is_shutdown {").expect("the shutdown exit");
        let end = body
            .find("if let Some(duty) = lane_duty {")
            .expect("the cleanup branches follow the shutdown exit");
        assert!(start < end);
        let exit = &body[start..end];
        assert!(exit.contains("return Ok(ProcessResult {"));
        for forbidden in [
            "propose_turn_work(",
            "remove_discarded_worktree(",
            "end_stale_waiting_session(",
            "git_ran_ok(",
            "git_cmd(",
        ] {
            assert!(
                !exit.contains(forbidden),
                "a shutdown must preserve the worktree and branch, found {forbidden}"
            );
        }
    }

    #[test]
    fn every_pending_stop_read_goes_through_the_own_entry_lookup() {
        let src = production_src();
        let reads: Vec<usize> = src.match_indices("s.pending_stop").map(|m| m.0).collect();
        assert!(reads.len() >= 3, "the scan still finds the stop reads");
        for at in reads {
            let lookup = src[..at].rfind("own_session_entry(");
            assert!(
                lookup.is_some_and(|p| at - p < 150),
                "a pending_stop read must come from this teardown's own entry: {}",
                line_at(src, at)
            );
        }
    }

    #[test]
    fn a_session_that_ended_on_the_default_branch_proposes_from_the_tracked_one() {
        let tracked = "claude-code/20260101-000000-work";
        assert_eq!(
            effective_session_branch(tracked, Some("main"), "main"),
            tracked,
            "proposing from the default branch could delete main"
        );
        assert_eq!(
            effective_session_branch(tracked, Some("feature/other"), "main"),
            "feature/other"
        );
        assert_eq!(
            effective_session_branch(tracked, Some(tracked), "main"),
            tracked
        );
        assert_eq!(effective_session_branch(tracked, None, "main"), tracked);
    }

    #[test]
    fn every_unmerged_probe_here_assumes_conflicts_remain_when_unanswered() {
        let src = squash(production_src());
        let sites: Vec<usize> = src
            .match_indices("has_unmerged_paths(&cwd)")
            .map(|m| m.0)
            .collect();
        assert_eq!(sites.len(), 2, "the lane report and the conflict cleanup");
        for at in sites {
            let statement = &src[at..];
            let statement = &statement[..statement.find(';').unwrap()];
            assert!(
                statement.ends_with(".or_unknown(true)"),
                "an unanswered probe must count as unmerged: {statement}"
            );
        }
    }

    #[test]
    fn an_unanswered_merge_head_probe_re_runs_the_commit() {
        let src = squash(production_src());
        let at = src
            .find(r#""rev-parse", "MERGE_HEAD""#)
            .expect("the merge-open probe is still here");
        let statement = &src[at..];
        let statement = &statement[..statement.find(';').unwrap()];
        assert!(
            statement.ends_with(".or_unknown(true)"),
            "skipping the commit would publish a half-finished merge: {statement}"
        );
    }

    #[test]
    fn a_continuation_that_took_the_apply_claim_or_the_watchdog_hands_the_merge_off() {
        let src = squash(production_src());
        let at = src
            .find("cleanup_action = if apply_claim.is_current()")
            .expect("the cleanup action is gated on the apply claim");
        let gate = &src[at..];
        let gate = &gate[..gate.find("match cleanup_action {").unwrap()];
        assert!(gate.contains("SafetyNetAction::EmitContinuationRequested"));
        assert!(
            gate.contains("external_continuation_requested")
                && gate.contains(".load(std::sync::atomic::Ordering::Acquire)"),
            "the external watchdog's recovery is told apart only by its own flag"
        );
        assert!(gate.contains("} else { ConflictResolutionCleanupAction::HandOff }"));
    }

    #[test]
    fn both_non_hand_off_arms_take_the_parked_apply_actor() {
        let src = squash(production_src());
        let abort = src
            .find("ConflictResolutionCleanupAction::Abort { message } => {")
            .expect("the Abort arm is still here");
        let apply = src
            .find("ConflictResolutionCleanupAction::Apply => {")
            .expect("the Apply arm is still here");
        let normal = src
            .find("// Normal worktree cleanup")
            .expect("the normal cleanup follows the conflict arms");
        const TAKE: &str = "self.pending_apply_actors.take(change.id)";
        assert!(
            src[abort..apply].contains(TAKE),
            "Abort must stamp ChangeApplyFailed with the device that clicked Apply"
        );
        assert!(
            src[apply..normal].contains(TAKE),
            "Apply must stamp ChangeApplied with the device that clicked Apply"
        );
    }

    #[test]
    fn the_empty_branch_arm_deletes_nothing() {
        let body = finalize_body();
        let start = body
            .find("SessionEndAction::KeepEmptyBranch => {")
            .expect("the empty-branch arm is still here");
        let end = body
            .find("let pending_stop_at_reap")
            .expect("the final reap follows the cleanup");
        let arm = &body[start..end];
        assert!(arm.contains("reconcile_emptied_pending_change("));
        for forbidden in ["git_cmd(", "git_ran_ok(", "remove_", "delete"] {
            assert!(
                !arm.contains(forbidden),
                "an empty branch is a resumable thread's ref, found {forbidden}"
            );
        }
    }

    #[test]
    fn a_late_discard_holds_the_thread_before_it_acts() {
        let body = finalize_body();
        let reap = body
            .find("reap_reading_stop(")
            .expect("finalize reaps and reads the stop once");
        let hold = body
            .find("hold_thread_for_worktree_reset(thread_id)")
            .expect("a late discard holds the thread");
        let end_session = body
            .find("end_stale_waiting_session(")
            .expect("a late discard ends the session");
        assert!(reap < hold && hold < end_session);
        let held = body[hold..]
            .find("Ok(Some(_hold)) =>")
            .expect("the hold stays bound for the discard's length")
            + hold;
        assert!(held < end_session);
        let none = body[hold..]
            .find("Ok(None) =>")
            .expect("a run that claimed the thread first is handled")
            + hold;
        let none_arm = &body[none..];
        let none_arm = &none_arm[..none_arm.find("Err(e) =>").unwrap()];
        assert!(
            !none_arm.contains("end_stale_waiting_session("),
            "a run that claimed the thread first is resuming into this worktree"
        );
    }
}
