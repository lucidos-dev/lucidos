/// What the idle handler should do after CC reports a turn-ending Result. The
/// natural turn terminator and `CodingAgentIdled` have already landed, so
/// nothing here emits an additional terminal event. `EndSession` MUST translate
/// to a plain loop break at the call site, not `stop.notify_one()`.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum IdleAction {
    /// Conflict-resolution session: break the run loop so post-loop cleanup
    /// tears down CC and removes the merge worktree.
    EndSession,
    /// Normal user session: kill CC; the next message resumes a fresh process.
    ExitSubprocess,
    /// Engine shutdown: leave CC for `recover_orphaned_worktrees` to resume
    /// after restart.
    Nothing,
}

/// Conflict resolution wins regardless of shutdown: the merge worktree is
/// throwaway and recovery never resumes a conflict session.
pub(super) fn idle_action(is_conflict: bool, is_shutdown: bool) -> IdleAction {
    if is_conflict {
        IdleAction::EndSession
    } else if is_shutdown {
        IdleAction::Nothing
    } else {
        IdleAction::ExitSubprocess
    }
}

/// Settle `AgentSession::inputs_awaiting_result` for one `Result` and return what
/// the driver still owes.
///
/// The two backends promise different numbers of Results per input, which is why
/// the counter needs a function rather than a constant:
///
/// * **Claude Code** merges back-to-back stdin inputs into a SINGLE Result, so
///   one Result answers every input forwarded so far.
/// * **Codex** runs one child per accepted input and emits one Result EACH, so
///   the rest are still owed.
///
/// `result_may_predate_a_forward` is the load-bearing qualifier on the Claude
/// Code rule, which holds only for inputs the agent had taken when it ended the
/// turn. `events_rx` and `msg_rx` have no causal ordering, so `select!` can
/// forward an input and only then hand the loop a `Result` produced before it.
/// Zeroing there would terminate the subprocess with the user's message still
/// inside it. A set flag means "this Result may not be the answer": keep one
/// input owed and let the next Result settle it.
///
/// Saturating everywhere, because the count can legitimately already be zero.
pub(super) fn settle_inputs_awaiting_result(
    coding_agent: crate::runtime::CodingAgent,
    before: u32,
    result_may_predate_a_forward: bool,
) -> u32 {
    match coding_agent {
        crate::runtime::CodingAgent::Codex => before.saturating_sub(1),
        crate::runtime::CodingAgent::ClaudeCode if result_may_predate_a_forward => {
            before.saturating_sub(1)
        }
        crate::runtime::CodingAgent::ClaudeCode => 0,
    }
}

/// Advance the forward-confirmation state for one agent event, and answer whether
/// THIS event might have been produced before the last forwarded input reached the
/// agent.
///
/// `events_rx` and `msg_rx` have no causal ordering. An agent event is therefore
/// not self-evidently a reaction to the input the run loop just forwarded, and
/// two rules make the answer safe:
///
/// 1. **Events already queued at the forward are skipped.** They were produced
///    before the agent could have seen the input, so they prove nothing. Without
///    this, a `Result` that sat in the channel the whole time reads as
///    confirmation, which is the buffered-event hole.
/// 2. **An event never vouches for itself.** The answer is the state as it stood
///    BEFORE this event, so the first genuinely-later event still counts as
///    possibly-predating. The agent can write a Result to stdout microseconds
///    before reading stdin, and neither protocol acknowledges an input.
///
/// The cost of both is at most one extra kept-alive idle. The cost of the other
/// direction is a subprocess cancelled while it still holds the user's message.
pub(super) fn agent_event_may_predate_forward(
    forwarded_input_unconfirmed: &mut bool,
    agent_events_queued_at_forward: &mut usize,
) -> bool {
    let may_predate = *forwarded_input_unconfirmed;
    if *forwarded_input_unconfirmed {
        if *agent_events_queued_at_forward > 0 {
            *agent_events_queued_at_forward -= 1;
        } else {
            *forwarded_input_unconfirmed = false;
        }
    }
    may_predate
}

/// Per-Result termination decision once `idle_action` returned
/// `ExitSubprocess`. Each `KeepAlive` variant names its own reason, so the log
/// line at the call site is derived from the variant and cannot drift.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum TerminateDecision {
    /// Kill CC via `agent_cancel.cancel()`. The next message resumes a
    /// fresh CC process via `--resume`.
    Terminate,
    /// A follow-up is genuinely still on its way to this session. Keep CC alive so
    /// it lands without a respawn round-trip. The fields carry which of the three
    /// windows said so, so the log line names the real reason.
    KeepAliveForFollowup {
        /// Sent, not yet forwarded: messages sitting unread in `msg_rx`.
        queued: usize,
        /// Forwarded, not yet answered: what the driver still owes after
        /// [`settle_inputs_awaiting_result`].
        awaiting_result: u32,
        /// Armed, not yet sent: `arm_followup_redirect` reserved this subprocess
        /// and its caller has not routed the message yet.
        redirect_pending: bool,
    },
    /// The chat-agent's `run_bash_background` LLM tool still has a task running
    /// for this thread. `spawn_bash_completion_watcher` pushes a resume prompt
    /// into `msg_tx` when the bash completes. Killing the subprocess here would
    /// force that wake through stale-session recovery.
    KeepAliveForBgBash,
}

/// Decide whether to terminate the CC subprocess at idle.
///
/// A follow-up has three disjoint windows between the moment it is promised and
/// the moment the driver answers it, and each gets its own signal:
///
/// | Window | Signal |
/// |---|---|
/// | armed, not yet sent | `redirect_pending` |
/// | sent, not yet forwarded | `queued`, i.e. `msg_rx.len()` |
/// | forwarded, not yet answered | `awaiting_result`, the post-settle remainder |
///
/// All three are read under the `agent_sessions` lock the caller already holds,
/// which makes them exact. That lock serializes against the fast-path
/// check-increment-send in `chat::process`, and an unbounded-channel send is
/// synchronous, so a message that was sent is in the channel.
///
/// Precedence: followup > chat-agent bg bash > terminate. With all three windows
/// empty this returns `Terminate`, which also carries a turn that died on a
/// transient upstream API error to the idle exit. A false positive there
/// silently cancels that recovery.
pub(super) fn terminate_decision(
    queued: usize,
    awaiting_result: u32,
    redirect_pending: bool,
    bg_bash_running: bool,
) -> TerminateDecision {
    if queued > 0 || awaiting_result > 0 || redirect_pending {
        TerminateDecision::KeepAliveForFollowup {
            queued,
            awaiting_result,
            redirect_pending,
        }
    } else if bg_bash_running {
        TerminateDecision::KeepAliveForBgBash
    } else {
        TerminateDecision::Terminate
    }
}

/// Decide whether a CC idle may write to change state at all: proposing the
/// worktree as a pending change, or reconciling one that already exists.
///
/// Deliberately blind to whether the branch has a diff. The caller reads the
/// file list once and routes on it. It proposes when there are files, and
/// reconciles the existing row to zero when there are none. Folding "has a
/// diff" into THIS gate makes a branch whose commits cancelled out skip the
/// whole block. Its pending row then keeps advertising a file the live Diff
/// does not show.
///
/// Only a clean `Generated` terminal qualifies. Any other one is half-finished
/// work the user should not be invited to apply blind, and it stays in the
/// worktree either way.
///
/// External repos never produce Lucidos changes, and a shutdown is mid-work
/// rather than idle. A conflict-resolution session runs in a `merge-tmp`
/// worktree where the merge IS the change being applied. Proposing there
/// creates a phantom second change row nothing resolves. Background bash
/// deliberately does NOT gate this: harden-at-apply re-runs the tests before
/// an un-hardened change can merge.
pub(super) fn may_touch_change_state_at_idle(
    is_external_repo: bool,
    is_shutdown: bool,
    is_conflict_session: bool,
    terminal_kind: &Option<TerminalKind>,
) -> bool {
    !is_external_repo
        && !is_shutdown
        && !is_conflict_session
        && matches!(terminal_kind, Some(TerminalKind::Generated))
}

/// Collapse the idle's ONE diff probe into the `(has_changes,
/// requires_restart)` pair stamped on `CodingAgentIdled`.
///
/// `changed_files` is `None` when git could not answer: a spawn failure, the 30s
/// `git_cmd` timeout, or a non-zero exit. That is UNKNOWN, never a "no", so it
/// preserves `prior` rather than writing `false`. Writing `false` there leaves a
/// renamed-branch thread with a dark Diff button. The idle's answer is the
/// durable one and runs last, so the post-commit hook cannot win that race.
///
/// An ANSWERED-but-empty probe still means `(false, false)`: a branch whose
/// commits cancelled out genuinely has no diff, and carrying a stale `true`
/// forward there is the phantom-Apply regression named in
/// [`may_touch_change_state_at_idle`].
///
/// `prior` is the thread's last known state: `has_changes` from
/// `thread_summaries.coding_agent_has_diff`, which the post-commit hook corrects
/// mid-turn, and `requires_restart` from the most recent turn-closer payload.
pub(super) fn idle_change_flags(
    changed_files: Option<&[String]>,
    prior: (bool, bool),
) -> (bool, bool) {
    match changed_files {
        Some([]) => (false, false),
        Some(files) => (true, crate::engine::git_ops::files_require_restart(files)),
        None => prior,
    }
}

/// Which terminal event closes the current CC turn. Cancel/Abort variants
/// carry the typed cause so the emit site cannot mis-classify why the turn
/// ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum TerminalKind {
    Generated,
    Canceled(crate::engine::thread_events::CancelCause),
    Aborted(crate::engine::thread_events::AbortCause),
    /// CC reported the turn ended in failure (mid-stream API error,
    /// `error_max_turns`). Carries the user-facing reason so
    /// `make_terminal_event` can populate `ResponseFailed.error`. Without it the
    /// partial response renders as a complete answer.
    Failed {
        error: String,
    },
}

/// True when CC was woken up with no user-initiated content: engine-internal
/// warm-up resumes only. The follow-up `Result` from such a turn is a no-op and
/// must not produce a terminal or idle event. The previous turn's
/// `CodingAgentIdled` already records the active `cc_session_id`. Image-only
/// turns count as content, so the call site must include images. Otherwise CC
/// delivers an answer and the thread row stays stuck at `running`.
pub(super) fn is_silent_resume(user_text_empty: bool, has_images: bool) -> bool {
    user_text_empty && !has_images
}

/// User-facing error message for the empty-response branch of `classify_result`.
/// Surfaces an OOM-killed bash (exit 137), a SIGTERM'd subprocess (exit 143), or
/// any other path where the backend produces no real assistant text. Without
/// this branch the empty Result classifies as `Generated` and the turn looks
/// successful even though the user got nothing back.
///
/// Worded for the coding agent generically rather than for Claude Code, because
/// `classify_result` runs for both backends. Naming one would mislabel every
/// Codex turn that lands here.
pub(super) const EMPTY_RESPONSE_ERROR: &str =
    "The coding agent produced no visible response: the session ended without a real \
     assistant message. This usually means a tool call was killed (OOM with exit 137, \
     SIGTERM with exit 143, or similar). The work is incomplete; re-prompt with a \
     narrower scope.";

/// Decide what to emit when a CC `Result` event arrives: both the terminal event
/// kind and whether `CodingAgentIdled` should follow it. Returning both together
/// prevents the TOCTOU race that would occur if `is_shutdown` were read twice
/// and observed different values: `ResponseGenerated` would fire while the idle
/// was skipped, leaving the thread row stuck at `running`.
///
/// Every non-silent, non-shutdown Result is a turn boundary and emits
/// `CodingAgentIdled`. Deciding whether to keep the subprocess alive for an
/// inflight follow-up is the run loop's job, via [`terminate_decision`].
///
/// Precedence: silent_resume > shutdown > user_hit_stop > cc_error >
/// text_is_empty > generated. Shutdown wins because the engine is going down
/// whatever CC did.
///
/// `user_hit_stop` sits ABOVE `cc_error` because the Stop button routes through
/// CC's native interrupt, and an interrupted turn comes back as a `Result` with
/// `is_error: true`. That error is caused by the cancel, so it must classify as
/// `Canceled`. Classifying it as `Failed` would show a red dot for a turn the
/// user stopped, and the branch-preservation gate keys on `Canceled`.
/// `text_is_empty` stays below for the same reason: a cancel is still a cancel.
pub(super) fn classify_result(
    is_silent_resume: bool,
    user_hit_stop: bool,
    interrupt_is_redirect: bool,
    is_shutdown: bool,
    cc_error: Option<String>,
    text_is_empty: bool,
) -> (Option<TerminalKind>, bool) {
    if is_silent_resume {
        return (None, false);
    }
    use crate::engine::thread_events::{AbortCause, CancelCause};
    let terminal = if is_shutdown {
        TerminalKind::Aborted(AbortCause::EngineShutdown)
    } else if user_hit_stop {
        // A Codex mid-turn follow-up redirect is mechanically a cancel, but it
        // is NOT a user Stop, so render it neutrally.
        TerminalKind::Canceled(if interrupt_is_redirect {
            CancelCause::SupersededByFollowup
        } else {
            CancelCause::UserStop
        })
    } else if let Some(error) = cc_error {
        TerminalKind::Failed { error }
    } else if text_is_empty {
        TerminalKind::Failed {
            error: EMPTY_RESPONSE_ERROR.to_string(),
        }
    } else {
        TerminalKind::Generated
    };
    let emit_idle = !is_shutdown;
    (Some(terminal), emit_idle)
}

/// After a `Result` is classified and its terminal emitted, decide whether the
/// run loop should clear the `user_hit_stop` latch. A `Result` is always a turn
/// boundary. The latch records "the user interrupted the turn that just ended",
/// so it must clear once that turn's terminal has fired.
///
/// The load-bearing case is an interrupt superseded by inflight follow-ups. When
/// the Stop button fires with follow-ups already queued, the run loop keeps the
/// subprocess alive to drain them. Those follow-ups emit a SECOND `Result` with
/// real, successful output. Without clearing the latch, that Result
/// re-classifies as `Canceled(UserStop)`, mislabelling finished work and
/// emitting a phantom second `ResponseCanceled`.
///
/// `Generated` and `Failed` cannot co-occur with a set latch, because
/// `user_hit_stop` ranks above both in [`classify_result`]. So only the cancel
/// and abort terminals need to clear it.
pub(super) fn terminal_clears_user_hit_stop(terminal: &TerminalKind) -> bool {
    matches!(
        terminal,
        TerminalKind::Canceled(_) | TerminalKind::Aborted(_)
    )
}

/// Auto-commit the worktree on session cleanup ONLY when the last turn
/// finished cleanly (Generated) AND the user didn't ask to discard.
///
/// Anything else leaves the worktree's contents uncommitted on the branch. The
/// post-commit hook fires per commit and emits a per-commit `ChangeProposed`.
/// Without this gate a half-finished session publishes a spurious Apply card
/// even though [`may_touch_change_state_at_idle`] already refused.
///
/// Recovery is unaffected: `recover_orphaned_worktrees` re-spawns the agent in
/// the same worktree, and it sees uncommitted dirt the same as committed state.
pub(super) fn should_auto_commit_on_cleanup(
    should_discard: bool,
    last_terminal: &Option<TerminalKind>,
) -> bool {
    !should_discard && matches!(last_terminal, Some(TerminalKind::Generated))
}

/// Whether the session-end path may delete this thread's worktree directory.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum WorktreeRemoval {
    /// Every condition was POSITIVELY established. Safe to remove.
    Remove,
    /// Keep the worktree; the string is the reason, for the log.
    Keep(&'static str),
}

/// Decide whether a **Discard** may delete the worktree the session ran in.
///
/// Discard is the only session end that reclaims a worktree at all. Every other
/// one leaves the tree for the background `WorktreeCleanup` worker, the single
/// owner of reclamation (ADR 0035). So this does not ask whether the work looks
/// spent or whether the turn ended cleanly: the user answered all of that by
/// clicking Discard, and the branch is deleted alongside the tree.
///
/// One question is still worth asking, and it is why this is a function rather
/// than a bare `git worktree remove`: **is this tree actually ours?** Claude
/// Code can `git checkout` inside its own worktree. A Discard aimed at the
/// session's branch must not delete a tree now sitting on someone else's.
/// `worktree_branch` is `None` for a detached HEAD or an unreadable one, and
/// neither is a positive match. That is the "unknown never authorizes
/// destruction" rule from `.claude/rules/rust.md`.
pub(super) fn discarded_worktree_removal(
    worktree_branch: Option<&str>,
    session_branch: &str,
) -> WorktreeRemoval {
    match worktree_branch {
        Some(b) if b == session_branch => WorktreeRemoval::Remove,
        Some(_) => WorktreeRemoval::Keep("worktree is checked out on a different branch"),
        None => WorktreeRemoval::Keep(
            "could not read which branch the worktree is on (detached HEAD, or git gave no answer)",
        ),
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum ConflictResolutionCleanupAction {
    Apply,
    Abort {
        message: &'static str,
    },
    /// The safety net just emitted `ContinuationRequested` for this session. The
    /// auto-recovery continuation resumes the SAME merge turn through
    /// `--resume`, so the merge duty transfers instead of failing: leave the
    /// in-progress merge, the worktree, the branch, the parked apply actor and
    /// the event pairing untouched. The `Continue` spawn consumer re-derives the
    /// duty from the still-unpaired `MergeConflictDetected`, and the resumed
    /// session's own completion applies or aborts for real.
    HandOff,
}

/// Decide whether a conflict-resolution worktree is safe to fast-forward into
/// main.
///
/// A clean `Generated` result is the only apply path, and it wins even over a
/// pending continuation. An external-watchdog false positive can race a natural
/// `Generated` end, and deferring an already-committed merge to a fragile
/// `--resume` would risk losing finished work.
///
/// A user cancel also beats the hand-off: Stop means "do not land this merge",
/// continuation or not. After those two, a pending auto-recovery continuation
/// hands the duty off. That is checked before `has_unmerged`, because a
/// stray-killed merge turn is EXPECTED to leave unmerged files for the
/// continuation to finish. Anything else means the merge-fix turn never produced
/// a trustworthy terminal, so the original change must stay pending.
pub(super) fn conflict_resolution_cleanup_action(
    has_unmerged: bool,
    last_terminal: &Option<TerminalKind>,
    continuation_pending: bool,
) -> ConflictResolutionCleanupAction {
    if !has_unmerged && matches!(last_terminal, Some(TerminalKind::Generated)) {
        return ConflictResolutionCleanupAction::Apply;
    }
    if matches!(last_terminal, Some(TerminalKind::Canceled(_))) {
        return ConflictResolutionCleanupAction::Abort {
            message: "Conflict resolution canceled — merge aborted. The change is still pending; try applying again.",
        };
    }
    if continuation_pending {
        return ConflictResolutionCleanupAction::HandOff;
    }
    if has_unmerged {
        return ConflictResolutionCleanupAction::Abort {
            message: "Conflict resolution incomplete — merge aborted. The change is still pending; try applying again.",
        };
    }
    ConflictResolutionCleanupAction::Abort {
        message: "Conflict resolution did not finish cleanly — merge aborted. The change is still pending; try applying again.",
    }
}

/// A conflict-resolution Abort may delete git state ONLY when THIS session ran
/// in the dedicated temp worktree on the temp branch: `merge_temp_branch` was
/// recorded by `MergeResolutionStarted` AND matches the session's own branch.
///
/// A Tier-2 merge runs in the thread's OWN worktree on the REAL change branch.
/// There the failed attempt created nothing but the in-progress merge, so
/// `git merge --abort` is the entire cleanup. Deleting the worktree or branch
/// would destroy the user's committed work (the failure-path-cleanup rule in
/// `.claude/rules/rust.md`).
///
/// Matching against the SESSION's branch, rather than mere `merge_temp_branch`
/// presence, is load-bearing. After a hand-off whose temp worktree was pruned,
/// the Continue consumer re-attaches the duty on the thread's own worktree. The
/// change row still carries the stale temp columns, so a presence-only gate
/// would force-remove the THREAD worktree the session actually ran in.
pub(super) fn conflict_abort_deletes_temp_state(
    merge_temp_branch: Option<&str>,
    session_branch: &str,
) -> bool {
    merge_temp_branch.is_some_and(|tb| tb == session_branch)
}

/// Inputs to [`is_stale_resume_signal`]. A named struct rather than a positional
/// argument list because every field is a `bool`, the same reason
/// [`super::external_watchdog::ExternalWatchdogInput`] exists next door. Eight
/// positional bools is a silent-transposition hazard at exactly the call site
/// where a transposition kills a live session.
///
/// `Copy` because one value feeds BOTH predicates at the call site.
/// [`is_stale_resume_signal`] and [`is_resume_settle_result`] read the same
/// eight fields and differ only in which way `resume_attach_confirmed` points,
/// so restating them per call is that same hazard. The settle predicate takes
/// one FURTHER argument, deliberately not a field here.
#[derive(Clone, Copy)]
pub(super) struct StaleResumeInputs {
    /// This turn asked the backend to `--resume` a specific session id.
    pub has_resume_session: bool,
    /// The backend's `Init` reported back the SAME session id we asked it to
    /// resume, proving the resume attached to the live conversation.
    pub resume_attach_confirmed: bool,
    pub result_text_empty: bool,
    pub buffered_text_empty: bool,
    pub no_prior_results_this_turn: bool,
    pub no_tool_calls_this_turn: bool,
    pub user_message_present: bool,
    pub cc_error: bool,
}

/// True when an arriving `Result` is the "stale --resume" signal: CC echoed our
/// forwarded user message back as an empty answer because the persisted session
/// id no longer exists. The run loop retries with a fresh spawn, reusing the
/// existing worktree rather than deleting it.
///
/// `resume_attach_confirmed` is the STRUCTURAL gate and vetoes everything below
/// it. The rest infers "the session is dead" from output SHAPE, which cannot
/// tell a dead session from a live one that said nothing. The session id can:
/// both backends report the id they attached to at `Init`, and a FAILED resume
/// yields a different one. So a matching id proves the conversation is live,
/// and no empty output may override it.
///
/// `cc_error.is_none()` is load-bearing: an empty Result with `is_error: true`
/// is a real upstream failure, not an expired session.
///
/// `no_tool_calls_this_turn` is load-bearing for terse models: a dead resume
/// makes no tool calls, while a live terse one does even with no text.
///
/// The output-shape half is a temporary measure, registered in
/// `docs/temporary-measures.md` under "Empty-echo stale-resume".
pub(super) fn is_stale_resume_signal(i: StaleResumeInputs) -> bool {
    i.has_resume_session
        && !i.resume_attach_confirmed
        && i.result_text_empty
        && i.buffered_text_empty
        && i.no_prior_results_this_turn
        && i.no_tool_calls_this_turn
        && i.user_message_present
        && !i.cc_error
}

/// True when an arriving `Result` closes the backend's own **resume-settle
/// turn** rather than the turn we asked for. It is [`is_stale_resume_signal`]
/// with the veto satisfied instead of absent: the SAME empty output shape, on a
/// resume the backend structurally confirmed it attached to.
///
/// A `--resume` often has leftovers to settle first, and each is a full turn
/// that ends with its own `result` on the wire.
/// The first can land BEFORE our prompt is even dequeued. Such a Result must not
/// terminate our turn, because our turn has not run yet. The run loop skips it
/// entirely and keeps reading events.
///
/// `no_api_call_this_turn` makes that skip safe, and it is a SEPARATE argument
/// rather than a ninth field on purpose. The eight fields describe output SHAPE,
/// and a settle turn looks exactly like a model that answered our prompt with
/// nothing. A `Usage` event separates them: it is emitted per real API call, so
/// zero of them proves the backend never asked the model anything. It stays out
/// of the struct because a dead `--resume` may call the model before echoing
/// nothing back, which [`is_stale_resume_signal`] must still recognise.
/// `no_prior_results_this_turn` doubles as the bound: a session can skip at most
/// one Result, which is all a resume settles.
pub(super) fn is_resume_settle_result(i: StaleResumeInputs, no_api_call_this_turn: bool) -> bool {
    no_api_call_this_turn
        && i.has_resume_session
        && i.resume_attach_confirmed
        && i.result_text_empty
        && i.buffered_text_empty
        && i.no_prior_results_this_turn
        && i.no_tool_calls_this_turn
        && i.user_message_present
        && !i.cc_error
}

/// CC's deterministic "the session I asked to `--resume` doesn't exist" error.
///
/// This is the COMPLEMENT of the `!cc_error` gate in [`is_stale_resume_signal`].
/// That heuristic must refuse to treat a `cc_error` as stale, since a transient
/// 5xx would otherwise strand user work. CC's EXPLICIT `No conversation found
/// with session ID: <id>` is not transient: the persisted id is gone and
/// re-resuming can never succeed. So it is the one whitelisted error string that
/// still triggers the fresh-spawn recovery.
///
/// The root cause it recovers is a mid-flight `CLAUDE_CONFIG_DIR` switch. That
/// relocates CC's per-session transcript store, so a resume spawned under the
/// new dir cannot find a session created under the old one. It also covers a
/// pruned transcript, a version upgrade, or a different machine.
///
/// Matched on the human-readable text because the `result` event exposes no
/// structured "session not found" code. That tolerance is registered in
/// `docs/temporary-measures.md`; switch to a structured signal if CC adds one.
pub(super) fn is_definitive_session_not_found(cc_error: Option<&str>) -> bool {
    cc_error.is_some_and(|e| e.contains("No conversation found with session ID"))
}

/// Decide what terminal event the stop arm of the run loop should emit.
///
/// Precedence: shutdown > user-action suppress > idle race > real Cancel.
/// Shutdown wins because the engine is going down whichever UI button triggered
/// the stop. Apply, Discard and Archive set `suppress_user_terminal` because
/// their own lifecycle event is the terminator. An idle CC means the previous
/// turn's `ResponseGenerated` already terminated it, so even a real Cancel click
/// that races in emits nothing: labelling a finished turn "Canceled" would lie.
///
/// This decides *whether* a terminal fires and its base cause. The redirect
/// refinement to `SupersededByFollowup` is applied by `emit_stop_terminal`, so
/// this pure function stays a three-input truth table.
pub(super) fn stop_terminal_kind(
    is_shutdown: bool,
    is_waiting: bool,
    suppress_user_terminal: bool,
) -> Option<TerminalKind> {
    use crate::engine::thread_events::{AbortCause, CancelCause};
    if is_shutdown {
        if is_waiting {
            None
        } else {
            Some(TerminalKind::Aborted(AbortCause::EngineShutdown))
        }
    } else if suppress_user_terminal || is_waiting {
        None
    } else {
        Some(TerminalKind::Canceled(CancelCause::UserStop))
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum SessionEndAction {
    Propose,
    KeepExternalBranch,
    /// Safety net fired (CC EOF without a Result event) and commits exist on the
    /// branch. Keep the commits on disk so the user can resume, but do NOT emit
    /// `ChangeProposed`: surfacing a half-finished crash as a ready-to-apply
    /// change misleads the user. The thread's terminal is `ResponseAborted`, so
    /// the UI shows the crash state.
    CrashedKeepBranch,
    /// The user cancelled the turn. Cancel is a *resumable turn boundary*, not a
    /// terminator: the next message resumes the same `cc_session_id` on this
    /// branch. Keep the branch even with zero commits, so
    /// `resolve_branch_for_resume` finds it. Do NOT propose, because a cancelled
    /// turn is half-finished work (mirrors [`may_touch_change_state_at_idle`]).
    KeepCanceledBranch,
    /// Ordinary session end with no proposable diff. The thread is still ALIVE
    /// and resumable, so the branch is KEPT: the next message resumes it, and
    /// `recover_orphaned_worktrees` keys on the branch ref to re-attach after a
    /// restart. Deleting a zero-commit branch here destroys resumability. The
    /// branch is a cheap ref, and the cleanup sweep reclaims fully-merged
    /// branches once the thread is archived.
    KeepEmptyBranch,
}

/// External repos with commits keep their branch even when the diff is empty.
/// The user owns push and PR for that ref, so we never `branch -D` something
/// they might want.
///
/// `safety_net_fired` is set when CC's event loop ended without a Result event.
/// Any commits on the branch then reflect partial work: they stay on disk via
/// `CrashedKeepBranch` and are never proposed as a change.
///
/// `user_canceled` is set when the last terminal was a user-driven
/// `Canceled(UserStop)`. A cancel keeps the branch so the session stays
/// resumable, and never proposes. It ranks above `Propose` and `KeepEmptyBranch`
/// but below the external and crash arms, which carry more specific semantics.
/// It cannot co-occur with `safety_net_fired`, because a cancel emits a terminal.
pub(super) fn classify_session_end_action(
    has_commits: bool,
    proposal_files_empty: bool,
    is_external_repo: bool,
    safety_net_fired: bool,
    user_canceled: bool,
) -> SessionEndAction {
    match (has_commits, is_external_repo, safety_net_fired) {
        (true, true, _) => SessionEndAction::KeepExternalBranch,
        (true, false, true) => SessionEndAction::CrashedKeepBranch,
        _ if user_canceled => SessionEndAction::KeepCanceledBranch,
        (true, false, false) if !proposal_files_empty => SessionEndAction::Propose,
        _ => SessionEndAction::KeepEmptyBranch,
    }
}

/// Clear all per-turn flags at a turn boundary, so prior-turn state cannot leak
/// into the next. `emitted_terminal_event` gates the post-loop safety net.
/// Without resetting it, a follow-up that ends with CC exiting before a Result
/// silently skips the safety net. `last_terminal_kind` is per-turn for the same
/// reason: [`should_auto_commit_on_cleanup`] must reflect THIS turn's outcome.
///
/// `cancel_actor` and `interrupt_is_redirect` reset in lockstep with
/// `user_hit_stop`, because the interrupt arm sets all three together. A
/// follow-up arriving during the cancel race clears the latch on its own.
/// Without this it would leave the prior turn's device on `meta`, or leave a
/// stale redirect flag that mislabels a later real Stop.
pub(super) fn reset_per_turn_flags(
    is_waiting: &mut bool,
    last_emitted_idle: &mut bool,
    emitted_terminal_event: &mut bool,
    user_hit_stop: &mut bool,
    interrupt_is_redirect: &mut bool,
    last_terminal_kind: &mut Option<TerminalKind>,
    cancel_actor: &mut Option<crate::engine::thread_events::MessageOrigin>,
) {
    *is_waiting = false;
    *last_emitted_idle = false;
    *emitted_terminal_event = false;
    *user_hit_stop = false;
    *interrupt_is_redirect = false;
    *last_terminal_kind = None;
    *cancel_actor = None;
}

/// 10 minutes of CC silence in the narrow "awaiting Anthropic response" window
/// means the subprocess hung. The window this protects is a dead TCP socket: the
/// network died in an in-flight API call, the kernel never noticed, and CC sat
/// forever. Without the watchdog the engine waits forever too, and the user sees
/// a permanently "Working" thread even after the network comes back.
///
/// The watchdog is opt-in: it arms ONLY when CC is mid-turn AND no tool is
/// executing. Tool execution is legitimate silence that we trust CC to time out
/// itself. A wall-clock-only watchdog kills CC during a long `TaskOutput` poll
/// or a multi-minute `AskUserQuestion` wait, both of which destroy user work.
///
/// `pub(crate)` because this limit only produces its non-destructive outcome,
/// a kill plus auto-resume, while it is SHORTER than Claude Code's own byte-idle
/// streaming deadline. That deadline instead ends the turn with a terminal
/// `ResponseFailed`. The runtime sets it and asserts the ordering against this
/// constant, so the two cannot drift silently.
pub(crate) const WATCHDOG_INACTIVITY_LIMIT_MS: i64 = 10 * 60 * 1000;

/// Hard ceiling for the `tools_in_flight > 0` carve-out. The normal limit skips
/// while a tool is mid-execution, because tool runtime is legitimate silence.
/// That skip is otherwise *unbounded*: a tool call that never returns pins
/// `tools_in_flight > 0` forever and neuters both watchdogs.
///
/// Past this ceiling the watchdog fires *regardless* of `tools_in_flight`, but
/// ONLY when the caller confirms the thread is still `running`. It must never
/// fire on `waiting_for_user_answer`: a pending question or permission card is
/// deliberately counted in `tools_in_flight`, and the user may take arbitrarily
/// long to answer (see `WatchdogGate::FirePastCeiling`).
///
/// 45 min is deliberately longer than the longest legitimate single tool, a full
/// release build plus test suite. The fire is non-destructive, so a falsely
/// flagged slow tool costs at most a resume.
///
/// `pub(crate)` so engine construction can pass it to the external watchdog.
pub(crate) const WATCHDOG_HUNG_TOOL_CEILING_MS: i64 = 45 * 60 * 1000;

/// How often the agentic loop's `select!` re-evaluates the watchdog. Coarse on
/// purpose: the limit is 10 min, so detection latency on a hung subprocess is at
/// most `WATCHDOG_INACTIVITY_LIMIT_MS` plus one tick. Smaller values would burn
/// the `agent_sessions` mutex on every tick for no benefit.
pub(super) const WATCHDOG_TICK_INTERVAL_SECS: u64 = 30;

/// Half the fire limit so the per-tick diagnostic surfaces before the fire
/// (rules out "elapsed wasn't far enough" when post-morteming a stuck thread).
pub(super) const WATCHDOG_DIAG_LOG_THRESHOLD_MS: i64 = WATCHDOG_INACTIVITY_LIMIT_MS / 2;

/// Outcome of one watchdog tick. The non-`Fire` variants name the gate that
/// held, so the diagnostic log can pin the cause.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum WatchdogGate {
    Fire,
    /// Past the hung-tool ceiling with tools still in flight. The caller MUST
    /// re-confirm the thread is still `running` before actually firing, because
    /// the pure gate cannot read the projection. A `waiting_for_user_answer`
    /// thread legitimately counts a pending card in `tools_in_flight`. Inner
    /// count surfaces in the diagnostic log.
    FirePastCeiling(i32),
    NotStale,
    SkipIsWaiting,
    SkipBadHeartbeat,
    /// Inner count surfaces in the diagnostic log so a stuck AskUserQuestion
    /// is identifiable.
    SkipToolsInFlight(i32),
}

impl WatchdogGate {
    /// Stable snake_case tag for the diagnostic log so scrapers can grep
    /// without parsing the Debug repr.
    pub(super) fn diag_tag(&self) -> &'static str {
        match self {
            Self::Fire => "fire",
            Self::FirePastCeiling(_) => "fire_past_ceiling",
            Self::NotStale => "not_stale",
            Self::SkipIsWaiting => "skip_is_waiting",
            Self::SkipBadHeartbeat => "skip_bad_heartbeat",
            Self::SkipToolsInFlight(_) => "skip_tools_in_flight",
        }
    }
}

/// What the post-loop safety net should emit when CC's event loop ended without
/// a natural terminator. Always lands SOMETHING terminal: either a
/// `ContinuationRequested` so the spawn dispatcher boots a fresh `--resume`, or
/// `ResponseAborted(SafetyNet)` so the UI flips to the red-dot abort state.
/// Without this guard a subprocess that died between events leaves the thread
/// row stuck at `running`.
///
/// Precedence: external terminal already emitted > watchdog fired > stray
/// signal-kill > plain safety net. External terminal wins because the caller
/// already landed one, and a second would relabel a finished turn. Watchdog wins
/// over plain abort because its whole job is to bypass the user-visible abort
/// when the network died mid-call.
///
/// A stray signal-kill the engine did NOT initiate is the `exit=143` truncation
/// bug: a SIGTERM reached the CC child with no deliberate cancel. It recovers
/// the same way, by auto-resume rather than a red dot the user must clear.
/// Gated on `!engine_cancelled`, so a deliberate teardown's own SIGKILL never
/// masquerades as a stray kill.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SafetyNetAction {
    /// Loop ended with a natural terminator, so the in-loop emit already closed
    /// the turn.
    Nothing,
    /// Watchdog killed CC mid-call (network died, dead TCP socket). Emit
    /// `ContinuationRequested` so the spawn dispatcher boots a fresh `--resume`
    /// without surfacing a user-visible abort.
    EmitContinuationRequested,
    /// CC's event loop died without a natural terminator and the watchdog did
    /// not fire (driver crash, EOF on stdout without a Result, parser glitch).
    /// Emit `ResponseAborted(SafetyNet)` so the UI flips to the red-dot abort
    /// state: the user needs to know the work did not complete.
    EmitAbortedSafetyNet,
    /// An external terminal already landed. Do not emit again, because a second
    /// terminal relabels a turn that is already closed.
    Skip,
}

/// Decide what the post-loop safety net should emit. Pure function;
/// `run_session.rs` glues this to the actual emit calls.
pub(super) fn safety_net_action(
    safety_net_fired: bool,
    watchdog_fired: bool,
    external_terminal_already_emitted: bool,
    killed_by_signal: bool,
    engine_cancelled: bool,
) -> SafetyNetAction {
    if !safety_net_fired {
        return SafetyNetAction::Nothing;
    }
    if external_terminal_already_emitted {
        return SafetyNetAction::Skip;
    }
    if watchdog_fired {
        return SafetyNetAction::EmitContinuationRequested;
    }
    // Stray external signal-kill with no deliberate engine cancel: recover like
    // the watchdog rather than surfacing an abort.
    if killed_by_signal && !engine_cancelled {
        return SafetyNetAction::EmitContinuationRequested;
    }
    SafetyNetAction::EmitAbortedSafetyNet
}

/// How many times in a row the engine auto-resumes a turn the backend ended on
/// a transient upstream API failure. Past that, the red dot stands.
///
/// The budget is CONSECUTIVE: a `ResponseGenerated` or a new user message resets
/// it, so a long unattended session that hits a drop every few hours never
/// exhausts it. Three attempts with no successful turn in between is no longer a
/// transient drop. It is a broken upstream, and resuming again just burns quota.
pub(super) const MAX_API_ERROR_AUTO_RESUMES: i64 = 3;

/// True when a terminal is a TRANSIENT upstream failure the session can be
/// resumed past, rather than a deterministic one that would fail identically.
///
/// The signature is Claude Code's own `API Error` prefix, already the contract
/// `claude_code_parse.rs` uses to decide that string is a real failure reason.
/// Matching the PREFIX rather than a loose substring keeps a turn that merely
/// mentions an api error in prose out of the retry path. Both sides use the same
/// rule, so they cannot drift apart on what the string means.
///
/// Everything else is deliberately excluded. `error_max_turns` reproduces
/// exactly, `No conversation found with session ID` is handled by the
/// stale-resume path, and the empty-response failure is a killed tool a resume
/// would just re-run. This tolerance for an unstructured error string is
/// registered in `docs/temporary-measures.md`.
pub(super) fn is_transient_api_failure(terminal: &TerminalKind) -> bool {
    matches!(terminal, TerminalKind::Failed { error } if error.trim_start().starts_with("API Error"))
}

/// Decide whether the post-loop finalize should emit
/// `ContinuationRequested{auto_resume_after_api_error}` so the spawn dispatcher
/// re-enters this session via `--resume`.
///
/// Every gate here is a refusal, and each one names a path that owns the thread
/// instead:
///
/// * Not a transient upstream failure (see [`is_transient_api_failure`]). A
///   `Generated` turn needs no resume, a `Canceled` one was stopped by the user,
///   and an `Aborted` one is already someone else's recovery.
/// * Budget spent (`MAX_API_ERROR_AUTO_RESUMES`): the upstream is not blipping,
///   it is down.
/// * Engine shutdown: recovery re-adopts in-flight threads after restart, and a
///   continuation emitted into a dying engine races that.
/// * Conflict-resolution session: it carries an apply's merge duty and a parked
///   actor, and its cleanup decides hand-off from the watchdog's own flags. A
///   third continuation source in that interlock is out of scope.
pub(super) fn auto_resume_after_api_error(
    terminal: &Option<TerminalKind>,
    resumes_spent: i64,
    is_shutdown: bool,
    is_conflict_session: bool,
) -> bool {
    terminal.as_ref().is_some_and(is_transient_api_failure)
        && resumes_spent < MAX_API_ERROR_AUTO_RESUMES
        && !is_shutdown
        && !is_conflict_session
}

/// Pure tick outcome. Fires when CC is mid-turn, no tool is in flight, and the
/// last event is past `limit_ms`. Tool execution is legitimate silence, because
/// CC owns timing the tool out. A zero `last_event_at_ms` defensively skips.
///
/// The `tools_in_flight > 0` skip is bounded by `ceiling_ms`: past the ceiling
/// it returns `FirePastCeiling` instead of `SkipToolsInFlight`, so a tool call
/// that never returns cannot neuter the watchdog forever. `FirePastCeiling` is
/// advisory. The impure caller must confirm the thread is still `running` and
/// not `waiting_for_user_answer` before actually firing.
pub(super) fn watchdog_gate(
    is_waiting: bool,
    last_event_at_ms: i64,
    now_ms: i64,
    limit_ms: i64,
    ceiling_ms: i64,
    tools_in_flight: i32,
) -> WatchdogGate {
    if is_waiting {
        return WatchdogGate::SkipIsWaiting;
    }
    if last_event_at_ms <= 0 {
        return WatchdogGate::SkipBadHeartbeat;
    }
    let elapsed = now_ms.saturating_sub(last_event_at_ms);
    if tools_in_flight > 0 {
        // Past the ceiling, a stuck tool no longer earns an unbounded skip.
        // Hand the caller a `FirePastCeiling` to act on after a projection
        // re-check. Below the ceiling, tool runtime is legitimate silence.
        if elapsed > ceiling_ms {
            return WatchdogGate::FirePastCeiling(tools_in_flight);
        }
        return WatchdogGate::SkipToolsInFlight(tools_in_flight);
    }
    if elapsed > limit_ms {
        WatchdogGate::Fire
    } else {
        WatchdogGate::NotStale
    }
}

#[cfg(test)]
#[path = "lifecycle_tests/propose.rs"]
mod propose_tests;

#[cfg(test)]
#[path = "lifecycle_tests/classify.rs"]
mod classify_tests;

#[cfg(test)]
#[path = "lifecycle_tests/watchdog.rs"]
mod watchdog_tests;

#[cfg(test)]
#[path = "lifecycle_tests/worktree_removal.rs"]
mod worktree_removal_tests;
