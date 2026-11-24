use crate::engine::thread_events::{CodingAgentExit, CodingAgentExitStage};
use crate::runtime::{AgentExitFacts, CodingAgent};

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum IdleAction {
    /// Resolution session (a merge fix or a lane thread): break the run loop
    /// so its completion applies, lands or reports the resolution. A
    /// follow-up still on its way keeps the session for one more turn.
    EndSession,
    /// Normal user session: kill CC; the next message resumes a fresh process.
    ExitSubprocess,
    /// Engine shutdown: leave CC for `recover_orphaned_worktrees` to resume
    /// after restart.
    Nothing,
}

pub(super) fn idle_action(is_resolution_session: bool, is_shutdown: bool) -> IdleAction {
    if is_resolution_session {
        IdleAction::EndSession
    } else if is_shutdown {
        IdleAction::Nothing
    } else {
        IdleAction::ExitSubprocess
    }
}

/// Whether a new turn opened on a session whose last turn already emitted its
/// terminal event. The run loop forwarded nothing to start such a turn.
/// So it must re-arm emission itself, or drop the turn's events as stragglers.
/// Two shapes do it:
///
/// - `question_resumed`: an answered question woke the agent's blocked hook.
/// - `read_owed_input`: the agent read an input forwarded during the previous
///   turn only after that turn's `Result` (ADR 0268). The caller passes false
///   once the session is exiting, so a late replay cannot revive it.
pub(super) fn starts_turn_after_terminal(
    question_resumed: bool,
    read_owed_input: bool,
    emitted_terminal_event: bool,
) -> bool {
    emitted_terminal_event && (question_resumed || read_owed_input)
}

pub(super) fn turn_has_terminal(
    emitted_terminal_event: bool,
    external_terminal: &std::sync::atomic::AtomicBool,
) -> bool {
    emitted_terminal_event || external_terminal.load(std::sync::atomic::Ordering::Acquire)
}

pub(crate) fn mark_turn_boundary(s: &mut crate::engine::AgentSession) {
    s.is_waiting = true;
    s.redirect_cause = None;
    s.cancel_actor = None;
    s.in_ceiling_reentry = false;
    if futures::FutureExt::now_or_never(s.interrupt.notified()).is_some() {
        crate::log_legacy!(
            "[AgentSession] Retired an interrupt that arrived after its turn's Result"
        );
    }
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
        /// Forwarded, not yet read: what the agent has not reported read
        /// (`InputLedger::owed`).
        unread: u32,
        /// Armed, not yet sent: `arm_followup_redirect` reserved this subprocess
        /// and its caller has not routed the message yet.
        redirect_pending: bool,
    },
}

pub(super) fn terminate_decision(
    queued: usize,
    unread: u32,
    redirect_pending: bool,
    leaves_queue_to_next_session: bool,
) -> TerminateDecision {
    let follow_up_coming =
        unread > 0 || (!leaves_queue_to_next_session && (queued > 0 || redirect_pending));
    if follow_up_coming {
        TerminateDecision::KeepAliveForFollowup {
            queued,
            unread,
            redirect_pending,
        }
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
/// A clean `Generated` terminal proposes its work, and `propose_change` can
/// still hold it on the plan floor (ADR 0397). A user Stop, a failure or an
/// abort did not finish, so its work is withheld, never proposed (ADR 0400).
/// Every other terminal writes nothing: a redirect the next turn continues,
/// and an Apply or Discard stop, which owns the change itself.
///
/// External repos never produce Lucidos changes, and a shutdown is mid-work
/// rather than idle. A resolution session works where the merge IS what lands:
/// a merge fix's worktree, or a lane thread's integration branch. Proposing
/// there creates a phantom second change row nothing resolves.
///
/// A live event wait holds a clean idle's proposal (ADR 0395): the agent parked
/// mid-work, and the wait re-opens the thread. That idle only keeps an existing
/// change in step with the branch. A wait that ends without re-opening the
/// thread hands the proposal to `propose_work_a_canceled_wait_held`.
///
/// A session whose handoff is pending proposes nothing, whatever its terminal:
/// its successor session carries the branch work and decides it at its own
/// turn end. Proposing now would flash Apply in the gap before that session.
pub(super) fn idle_change_write(
    session: IdleSession,
    terminal_kind: &Option<TerminalKind>,
) -> Option<IdleChangeWrite> {
    if session.is_external_repo || session.is_shutdown || session.is_resolution_session {
        return None;
    }
    if session.handoff_pending {
        return Some(IdleChangeWrite::HandedOff);
    }
    match terminal_kind {
        Some(TerminalKind::Generated) if session.holds_live_wait => Some(IdleChangeWrite::Resync),
        Some(TerminalKind::Generated) => Some(IdleChangeWrite::Complete),
        Some(
            TerminalKind::Canceled(crate::engine::thread_events::CancelCause::UserStop)
            | TerminalKind::Failed { .. }
            | TerminalKind::Aborted(_),
        ) => Some(IdleChangeWrite::Unfinished),
        _ => None,
    }
}

/// What an idle knows about its session, for [`idle_change_write`].
#[derive(Debug, Clone, Copy, Default)]
pub(super) struct IdleSession {
    pub(super) is_external_repo: bool,
    pub(super) is_shutdown: bool,
    pub(super) is_resolution_session: bool,
    /// The thread holds an event wait that has not been delivered, expired or
    /// canceled.
    pub(super) holds_live_wait: bool,
    /// The session handed off, and its successor session waits to start.
    pub(super) handoff_pending: bool,
}

/// How an idle may write change state, from [`idle_change_write`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum IdleChangeWrite {
    /// The turn finished: its branch work is a change ready to review.
    Complete,
    /// The turn did not finish: a Stop, a failure or an abort. Its branch work
    /// stays unproposed, announced as withheld (ADR 0400).
    Unfinished,
    /// The turn ended holding a live event wait: an open change follows the
    /// branch, and no new one is proposed.
    Resync,
    /// The session handed off: its successor session carries the branch work.
    /// A pending change goes back to unproposed work, unannounced.
    HandedOff,
}

impl IdleChangeWrite {
    pub(super) fn finished(self) -> bool {
        self != Self::Unfinished
    }

    pub(super) fn may_create(self) -> bool {
        self == Self::Complete || self == Self::Unfinished
    }
}

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
pub(crate) enum TerminalKind {
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

/// Why a coding-agent spawn with nothing to send is refused.
pub(super) const EMPTY_INPUT_ERROR: &str =
    "Nothing to send to the coding agent: the message has no text and no image.";

pub(super) fn require_agent_input(
    text: &str,
    images: Option<&[crate::engine::types::ChatImage]>,
) -> Result<(), &'static str> {
    if text.trim().is_empty() && images.is_none_or(<[_]>::is_empty) {
        return Err(EMPTY_INPUT_ERROR);
    }
    Ok(())
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

pub(super) fn classify_result(
    user_hit_stop: bool,
    redirect_cause: Option<crate::engine::thread_events::CancelCause>,
    is_shutdown: bool,
    cc_error: Option<String>,
    text_is_empty: bool,
) -> (TerminalKind, bool) {
    use crate::engine::thread_events::{AbortCause, CancelCause};
    let terminal = if is_shutdown {
        TerminalKind::Aborted(AbortCause::EngineShutdown)
    } else if user_hit_stop {
        // A redirect (a follow-up) is mechanically a
        // cancel, but it is NOT a user Stop, so it carries its own cause.
        TerminalKind::Canceled(redirect_cause.unwrap_or(CancelCause::UserStop))
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
    (terminal, emit_idle)
}

pub(super) fn terminal_clears_user_hit_stop(terminal: &TerminalKind) -> bool {
    matches!(
        terminal,
        TerminalKind::Canceled(_) | TerminalKind::Aborted(_)
    )
}

/// Auto-commit the worktree on session cleanup ONLY when the last turn
/// finished cleanly (Generated) AND the user didn't ask to discard.
///
/// Anything else leaves the worktree's contents uncommitted on the branch. So
/// a half-finished session never commits partial work that a later proposal
/// would carry.
///
/// Recovery is unaffected: `recover_orphaned_worktrees` re-spawns the agent in
/// the same worktree, and it sees uncommitted dirt the same as committed state.
pub(super) fn should_auto_commit_on_cleanup(
    should_discard: bool,
    last_terminal: &Option<TerminalKind>,
) -> bool {
    !should_discard && matches!(last_terminal, Some(TerminalKind::Generated))
}

pub(super) fn result_auto_commits(terminal: &TerminalKind) -> bool {
    !matches!(terminal, TerminalKind::Aborted(_))
}

/// Whether the turn that just ended is the user's own, finished turn: a
/// `Generated` terminal, outside an engine merge resolution and an engine
/// turn. An engine turn is a cache keep-alive or a *ceiling re-entry*. A
/// shutdown aborts the turn, so it never reads as `Generated`.
///
/// Only such a turn runs forced decisions and waits for background tasks.
/// The context ceiling settles every terminal separately.
pub(super) fn turn_ended_cleanly(
    terminal: &Option<TerminalKind>,
    is_resolution_session: bool,
    engine_turn: bool,
) -> bool {
    matches!(terminal, Some(TerminalKind::Generated)) && !is_resolution_session && !engine_turn
}

/// What an idle knows when it decides whether to send the model-swap
/// re-entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct ModelSwapIdle {
    /// This process saw a swap it has not told the agent about yet.
    pub pending: bool,
    /// Another re-entry went out at this idle.
    pub other_reentry: bool,
    pub ended_cleanly: bool,
    /// The idle wrote `Complete`, which rules out a pending handoff and a
    /// live event wait.
    pub idle_complete: bool,
    pub emit_idle: bool,
    pub is_shutdown: bool,
    /// Input waits for the agent, and its turn comes first.
    pub input_owed: bool,
}

/// Whether this idle sends the model-swap re-entry. Only a clean, finished
/// turn with nothing else on its way does; otherwise the swap waits.
pub(super) fn model_swap_reentry_due(idle: ModelSwapIdle) -> bool {
    idle.pending
        && idle.ended_cleanly
        && idle.idle_complete
        && idle.emit_idle
        && !idle.other_reentry
        && !idle.is_shutdown
        && !idle.input_owed
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CeilingTurnEnd {
    Wait,
    Reenter,
    Brief,
}

pub(super) fn ceiling_turn_end(
    terminal: Option<&TerminalKind>,
    is_shutdown: bool,
) -> CeilingTurnEnd {
    use crate::engine::thread_events::CancelCause;
    if is_shutdown {
        return CeilingTurnEnd::Wait;
    }
    match terminal {
        Some(
            TerminalKind::Generated | TerminalKind::Canceled(CancelCause::SupersededByFollowup),
        ) => CeilingTurnEnd::Reenter,
        Some(TerminalKind::Canceled(CancelCause::UserStop) | TerminalKind::Failed { .. }) => {
            CeilingTurnEnd::Brief
        }
        _ => CeilingTurnEnd::Wait,
    }
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
/// than a bare `git worktree remove`: **is this tree actually ours?** That is
/// [`foreign_worktree_reason`].
pub(super) fn discarded_worktree_removal(
    worktree_branch: Option<&str>,
    session_branch: &str,
) -> WorktreeRemoval {
    match foreign_worktree_reason(worktree_branch, session_branch) {
        None => WorktreeRemoval::Remove,
        Some(reason) => WorktreeRemoval::Keep(reason),
    }
}

pub(super) fn foreign_worktree_reason(
    worktree_branch: Option<&str>,
    session_branch: &str,
) -> Option<&'static str> {
    match worktree_branch {
        Some(b) if b == session_branch => None,
        Some(_) => Some("worktree is checked out on a different branch"),
        None => Some(
            "could not read which branch the worktree is on (detached HEAD, or git gave no answer)",
        ),
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum ConflictResolutionCleanupAction {
    Apply,
    Abort { message: &'static str },
    HandOff,
}

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

/// How a lane thread's run ended, for the landing lane.
///
/// A user Stop ends the turn and keeps the batch open, as Stop keeps any
/// session resumable. That covers a Stop the run loop took (`stop`) and a
/// turn the agent ended as canceled. An Apply, Discard or Archive on the
/// thread fails the batch: the thread is not coming back to it.
///
/// Otherwise it is the decision [`conflict_resolution_cleanup_action`] makes:
/// only a clean `Generated` turn with nothing unmerged completes, and a
/// pending continuation hands the batch on (`None`). Anything else fails the
/// batch whole.
pub(super) fn lane_thread_end(
    has_unmerged: bool,
    last_terminal: &Option<TerminalKind>,
    continuation_pending: bool,
    stop: Option<crate::engine::StopReason>,
) -> Option<crate::engine::landing_lane::LaneThreadEnd> {
    use crate::engine::landing_lane::{LaneThreadEnd, LANE_THREAD_CLOSED};
    use crate::engine::thread_events::CancelCause;
    use crate::engine::StopReason;
    match stop {
        Some(StopReason::UserStop) => return Some(LaneThreadEnd::Stopped),
        Some(StopReason::Apply | StopReason::Discard | StopReason::Archive) => {
            return Some(LaneThreadEnd::Failed(LANE_THREAD_CLOSED.to_string()))
        }
        None => {}
    }
    match conflict_resolution_cleanup_action(has_unmerged, last_terminal, continuation_pending) {
        ConflictResolutionCleanupAction::Apply => Some(LaneThreadEnd::Completed),
        ConflictResolutionCleanupAction::HandOff => None,
        ConflictResolutionCleanupAction::Abort { .. } => Some(match last_terminal {
            Some(TerminalKind::Canceled(CancelCause::UserAction)) => {
                LaneThreadEnd::Failed(LANE_THREAD_CLOSED.to_string())
            }
            Some(TerminalKind::Canceled(_)) => LaneThreadEnd::Stopped,
            _ => LaneThreadEnd::Failed(
                "The lane thread ended without finishing its merges.".to_string(),
            ),
        }),
    }
}

pub(super) fn conflict_abort_deletes_temp_state(
    merge_temp_branch: Option<&str>,
    session_branch: &str,
) -> bool {
    merge_temp_branch.is_some_and(|tb| tb == session_branch)
}

/// Inputs to [`is_stale_resume_signal`]. A named struct rather than a positional
/// argument list because every field is a `bool`, the same reason
/// [`super::external_watchdog::ExternalWatchdogInput`] exists next door. Seven
/// positional bools is a silent-transposition hazard at exactly the call site
/// where a transposition kills a live session.
///
/// `Copy` because one value feeds BOTH predicates at the call site.
/// [`is_stale_resume_signal`] and [`is_resume_settle_result`] read the same
/// seven fields and differ only in which way `resume_attach_confirmed` points,
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
        && !i.cc_error
}

pub(super) fn is_resume_settle_result(i: StaleResumeInputs, no_api_call_this_turn: bool) -> bool {
    no_api_call_this_turn
        && i.has_resume_session
        && i.resume_attach_confirmed
        && i.result_text_empty
        && i.buffered_text_empty
        && i.no_prior_results_this_turn
        && i.no_tool_calls_this_turn
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
    /// Propose the branch's net work.
    Propose,
    /// The turn did not finish (a Stop or a failure), so its net work stays
    /// unproposed and is announced as withheld (ADR 0400).
    WithholdUnfinished,
    KeepExternalBranch,
    /// Safety net fired (CC EOF without a Result event) and commits exist on the
    /// branch. Keep the commits on disk so the user can resume, but do NOT emit
    /// `ChangeProposed`: surfacing a half-finished crash as a ready-to-apply
    /// change misleads the user. The thread's terminal is `ResponseAborted`, so
    /// the UI shows the crash state.
    CrashedKeepBranch,
    KeepCanceledBranch,
    KeepEmptyBranch,
    /// The session handed off with net work on the branch. Its successor
    /// session carries the work and decides it at its own turn end, so
    /// nothing is proposed or announced now.
    CarriedBySuccessor,
}

pub(super) fn classify_session_end_action(
    has_commits: bool,
    proposal_files_empty: bool,
    is_external_repo: bool,
    safety_net_fired: bool,
    cancel: TurnCancel,
    handoff_pending: bool,
) -> SessionEndAction {
    let proposable = has_commits && !proposal_files_empty;
    match (has_commits, is_external_repo, safety_net_fired, cancel) {
        (true, true, _, _) => SessionEndAction::KeepExternalBranch,
        (true, false, true, _) => SessionEndAction::CrashedKeepBranch,
        _ if handoff_pending && proposable => SessionEndAction::CarriedBySuccessor,
        (_, _, _, TurnCancel::Stopped | TurnCancel::Failed) if proposable => {
            SessionEndAction::WithholdUnfinished
        }
        (_, _, _, TurnCancel::Stopped | TurnCancel::Redirected) => {
            SessionEndAction::KeepCanceledBranch
        }
        _ if proposable => SessionEndAction::Propose,
        _ => SessionEndAction::KeepEmptyBranch,
    }
}

/// How the turn's last terminal cancelled it, for [`classify_session_end_action`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum TurnCancel {
    /// Not cancelled, or cancelled by an Apply, Discard or Archive that owns
    /// the change itself.
    None,
    /// The user pressed Stop.
    Stopped,
    /// A follow-up interrupted the turn, and the work continues.
    Redirected,
    /// The turn ended in failure, so it did not finish.
    Failed,
}

impl TurnCancel {
    pub(super) fn of(terminal: &Option<TerminalKind>) -> Self {
        use crate::engine::thread_events::CancelCause;
        match terminal {
            Some(TerminalKind::Canceled(CancelCause::UserStop)) => Self::Stopped,
            Some(TerminalKind::Canceled(cause)) if cause.is_redirect() => Self::Redirected,
            Some(TerminalKind::Failed { .. }) => Self::Failed,
            _ => Self::None,
        }
    }
}

pub(super) fn reset_per_turn_flags(
    is_waiting: &mut bool,
    last_emitted_idle: &mut bool,
    emitted_terminal_event: &mut bool,
    user_hit_stop: &mut bool,
    redirect_cause: &mut Option<crate::engine::thread_events::CancelCause>,
    terminal: TurnTerminal<'_>,
    cancel_actor: &mut Option<crate::engine::thread_events::MessageOrigin>,
) {
    *is_waiting = false;
    *last_emitted_idle = false;
    *emitted_terminal_event = false;
    *user_hit_stop = false;
    *redirect_cause = None;
    *terminal.kind = None;
    *terminal.withheld_api_error = None;
    *cancel_actor = None;
}

/// The run loop's two views of ONE ended turn, borrowed together.
///
/// They must move in lockstep. `withheld_api_error` is what the released
/// *auto-resume hold* handed back for `kind`. Carried into the next turn it
/// would resume off a decision taken two exits ago, and announce that
/// terminal's error for this one. So they travel as ONE argument, which a
/// caller cannot clear half of. ([`StaleResumeInputs`] next door is a named
/// struct too, for a different hazard: transposing positional bools.)
pub(super) struct TurnTerminal<'a> {
    pub kind: &'a mut Option<TerminalKind>,
    pub withheld_api_error: &'a mut Option<String>,
}

pub(crate) const WATCHDOG_INACTIVITY_LIMIT_MS: i64 = 10 * 60 * 1000;

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
    FirePastCeiling(i32),
    NotStale,
    SkipIsWaiting,
    SkipBadHeartbeat,
    /// Inner count surfaces in the diagnostic log so a stuck AskUserQuestion
    /// is identifiable.
    SkipToolsInFlight(i32),
}

impl WatchdogGate {
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum SafetyNetAction {
    /// Loop ended with a natural terminator, so the in-loop emit already closed
    /// the turn.
    Nothing,
    /// Watchdog killed CC mid-call (network died, dead TCP socket). Emit
    /// `ContinuationRequested` so the spawn dispatcher boots a fresh `--resume`
    /// without surfacing a user-visible abort.
    EmitContinuationRequested,
    /// The agent's process ended on its own and said why. Emit
    /// `ResponseFailed` with the CLI's words, and no retry.
    EmitFailed {
        error: String,
        agent_exit: CodingAgentExit,
    },
    /// CC's event loop died without a natural terminator and nothing said why
    /// (driver crash, a silent exit 0, parser glitch). Emit
    /// `ResponseAborted(SafetyNet)` so the UI flips to the red-dot abort
    /// state: the user needs to know the work did not complete.
    EmitAbortedSafetyNet,
    Skip,
}

/// Everything [`safety_net_action`] reads.
pub(super) struct SafetyNetInput<'a> {
    pub safety_net_fired: bool,
    pub watchdog_fired: bool,
    pub external_terminal_already_emitted: bool,
    pub engine_cancelled: bool,
    /// The agent's startup line arrived (Claude Code's `system/init`, Codex's
    /// thread start). An exit before it is a start failure. Judged by timing,
    /// never by the CLI's wording.
    pub init_seen: bool,
    pub exit: &'a AgentExitFacts,
    pub coding_agent: CodingAgent,
}

/// Decide what the post-loop safety net should emit. Pure function;
/// `run_session.rs` glues this to the actual emit calls.
pub(super) fn safety_net_action(input: &SafetyNetInput) -> SafetyNetAction {
    if !input.safety_net_fired {
        return SafetyNetAction::Nothing;
    }
    if input.external_terminal_already_emitted {
        return SafetyNetAction::Skip;
    }
    if input.watchdog_fired {
        return SafetyNetAction::EmitContinuationRequested;
    }
    if input.engine_cancelled {
        return SafetyNetAction::EmitAbortedSafetyNet;
    }
    // Stray external signal-kill with no deliberate engine cancel: recover like
    // the watchdog rather than surfacing an abort.
    if input.exit.killed_by_signal {
        return SafetyNetAction::EmitContinuationRequested;
    }
    let failed_exit = input.exit.code.is_some_and(|code| code != 0);
    if failed_exit || !input.exit.stderr_tail.is_empty() {
        let stage = if input.init_seen {
            CodingAgentExitStage::MidTurn
        } else {
            CodingAgentExitStage::Startup
        };
        return SafetyNetAction::EmitFailed {
            error: exit_error_text(input.coding_agent, stage, input.exit),
            agent_exit: CodingAgentExit {
                stage,
                code: input.exit.code,
            },
        };
    }
    SafetyNetAction::EmitAbortedSafetyNet
}

/// What a failed exit tells the user: the CLI's own stderr, or a plain
/// sentence naming the agent and the exit code when it wrote nothing.
pub(super) fn exit_error_text(
    coding_agent: CodingAgent,
    stage: CodingAgentExitStage,
    exit: &AgentExitFacts,
) -> String {
    if !exit.stderr_tail.is_empty() {
        return exit.stderr_tail.clone();
    }
    let agent = coding_agent.display_name();
    let when = match stage {
        CodingAgentExitStage::Startup => "before its session began",
        CodingAgentExitStage::MidTurn => "in the middle of a reply",
    };
    match exit.code {
        Some(code) => {
            format!("{agent} exited with code {code} {when} and wrote nothing to stderr.")
        }
        None => format!("{agent} exited {when} and wrote nothing to stderr."),
    }
}

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

/// The error text of a terminal [`is_transient_api_failure`] accepts, and `None`
/// for every other terminal.
///
/// The *auto-resume hold* carries this string, so the engine can still announce
/// the failure if the resume it withheld the card for never happens. Asked
/// through the same predicate rather than re-matching the prefix, because a
/// second copy of that rule is how the two sides drift apart.
pub(super) fn transient_api_failure_error(terminal: &Option<TerminalKind>) -> Option<&str> {
    match terminal {
        Some(t @ TerminalKind::Failed { error }) if is_transient_api_failure(t) => Some(error),
        _ => None,
    }
}

pub(super) fn auto_resume_after_api_error(
    terminal: &Option<TerminalKind>,
    resumes_spent: i64,
    is_shutdown: bool,
    is_resolution_session: bool,
) -> bool {
    terminal.as_ref().is_some_and(is_transient_api_failure)
        && resumes_spent < MAX_API_ERROR_AUTO_RESUMES
        && !is_shutdown
        && !is_resolution_session
}

/// What the engine owes the parent once it has settled whether the terminal it
/// withheld will actually be resumed. See [`held_completion_release`].
#[derive(Debug, PartialEq, Eq)]
pub(super) enum HeldCompletionRelease {
    /// Somebody is coming. Stay quiet: the card belongs at the real terminal.
    KeepWithholding,
    /// Nobody is coming. Announce the terminal the fan-in held back, or the
    /// parent waits on a child that will never report.
    Announce,
}

pub(super) fn held_completion_release(
    continuation_persisted: bool,
    followup_queued: bool,
) -> HeldCompletionRelease {
    if followup_queued || continuation_persisted {
        HeldCompletionRelease::KeepWithholding
    } else {
        HeldCompletionRelease::Announce
    }
}

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
