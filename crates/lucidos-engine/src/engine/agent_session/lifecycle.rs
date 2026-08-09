/// What the idle handler should do after CC reports a turn-ending Result.
/// The natural turn terminator and `CodingAgentIdled` have already landed —
/// nothing here emits an additional terminal event. `EndSession` MUST
/// translate to a plain loop break at the call site, not `stop.notify_one()`.
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

/// Conflict resolution wins regardless of shutdown — the merge worktree is
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
/// The two backends make different promises about how many Results an input earns,
/// and that asymmetry is the whole reason the counter needs a function rather than
/// a constant:
///
/// * **Claude Code** merges back-to-back stdin inputs into a SINGLE Result. One
///   Result therefore answers every input forwarded so far, and the remainder is
///   zero.
/// * **Codex** runs one app-server / exec child per accepted input and emits one
///   Result EACH. `TurnOutcome::Continue` in `runtime/codex.rs` deliberately keeps
///   queued inputs across an interrupt on exactly that promise, so a Result answers
///   one input and the rest are still owed.
///
/// Applying the Codex rule to Claude Code is the defect this replaced: a turn that
/// merged three instructions settled to two, reported two phantom follow-ups in
/// flight, kept a finished session alive, and swallowed the API-drop auto-resume.
/// See `docs/plans/2026-08-07-api-drop-resume-suppressed-by-phantom-followup-count.md`.
///
/// `result_may_predate_a_forward` is the load-bearing qualifier on the Claude Code
/// rule, and without it the fix trades one dropped-work bug for another. "One
/// Result answers everything forwarded so far" holds only for inputs the agent had
/// actually taken when it ended the turn. `events_rx` and `msg_rx` are separate
/// channels with no causal ordering, so `select!` can forward an input and only
/// then hand the loop a `Result` the agent produced before that input arrived.
/// Zeroing there would terminate the subprocess with the user's message still
/// inside it, which is the exact race the counter this replaced was written
/// against. The run loop sets the flag when it forwards and clears it on any agent
/// output, so a set flag means "nothing from the agent since, this Result may not
/// be the answer": keep one input owed and let the next Result settle it.
///
/// Saturating everywhere the count decrements, because it can legitimately already
/// be zero: a silent resume / warm-up turn seeds 0 and still produces a Result.
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
/// `events_rx` and `msg_rx` are separate channels with no causal ordering, so an
/// agent event is not self-evidently a reaction to the input the run loop just
/// forwarded. Two rules make the answer safe:
///
/// 1. **Events already queued at the forward are skipped.** They were produced
///    before the agent could have seen the input, so they prove nothing. Without
///    this, a `Result` that had been sitting in the channel the whole time reads as
///    confirmation, which is the buffered-event hole.
/// 2. **An event never vouches for itself.** The answer is the state as it stood
///    BEFORE this event, so the first genuinely-later event still counts as
///    possibly-predating. That covers the irreducible remainder: the agent can
///    write a Result to stdout microseconds before reading stdin, and nothing in
///    either protocol acknowledges an input.
///
/// The cost of both is at most one extra kept-alive idle. The cost of getting it
/// wrong in the other direction is a subprocess cancelled while it still holds the
/// user's message. Mutable-state-in, answer-out like `reset_per_turn_flags`, so the
/// run loop keeps the state as plain locals and this stays testable.
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
/// `ExitSubprocess`. Each `KeepAlive` variant names its own reason so the
/// log line at the call site is derived from the variant, with no risk of the
/// log message drifting from the actual cause.
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
    /// The chat-agent's `run_bash_background` LLM tool still has a task
    /// running for this thread. `spawn_bash_completion_watcher` will push a
    /// resume prompt into `msg_tx` when the bash completes; killing CC
    /// here would force the wake path through stale-session recovery,
    /// exactly the regression that gate exists to prevent.
    KeepAliveForBgBash,
}

/// Decide whether to terminate the CC subprocess at idle.
///
/// A follow-up has three disjoint windows between the moment it is promised and the
/// moment the driver answers it, and each gets its own signal rather than one
/// counter standing in for all three:
///
/// | Window | Signal |
/// |---|---|
/// | armed, not yet sent | `redirect_pending`, taken from `AgentSession::redirect_followup_pending` |
/// | sent, not yet forwarded | `queued`, i.e. `msg_rx.len()` |
/// | forwarded, not yet answered | `awaiting_result`, the post-settle remainder |
///
/// Every one of them is read under the `agent_sessions` lock the caller already
/// holds, which is what makes them exact rather than approximate: that lock
/// serializes against the fast-path check-increment-send in `chat::process`, and an
/// unbounded-channel send is synchronous, so a message that was sent is a message
/// that is in the channel.
///
/// Precedence: followup > chat-agent bg bash > terminate. Followup wins because it
/// is user-initiated and the strongest signal. The chat-agent bg-bash signal only
/// matters when nothing is coming: it keeps CC alive so
/// `spawn_bash_completion_watcher` can push a resume prompt when the bash finishes
/// (killing CC would force that wake through stale-session recovery).
///
/// When all three windows are empty this returns `Terminate`, which is also what
/// carries a turn that died on a transient upstream API error to the idle exit where
/// `maybe_auto_resume_after_api_error` lives. A false positive here does not merely
/// leak a subprocess, it silently cancels that recovery.
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

/// Decide whether a CC idle may write to change state at all — proposing the
/// worktree as a pending change, or reconciling one that already exists.
///
/// Deliberately blind to whether the branch actually has a diff: the caller
/// reads the file list once and routes on it, proposing when there are files
/// and reconciling the existing pending row to zero when there are none. Making
/// "has a diff" part of THIS gate is the bug that shipped change `2cc8391f` — a
/// branch whose commits cancelled out skipped the whole block, so its pending
/// row kept advertising a file (and a restart) that the live Diff didn't show.
///
/// Auto-propose surfaces the worktree as a pending change with an Apply
/// button. We only do that when the turn ended cleanly (Generated) — any
/// other terminal (Failed, Canceled, Aborted, silent resume) is "half-
/// assed work" the user shouldn't be invited to apply blind. The work
/// stays in the worktree on the branch either way; the user just doesn't
/// get a spurious Apply card built from a crash, an upstream API drop, a
/// mid-turn cancel, or an engine shutdown.
///
/// External repos never produce Lucidos changes (the dev pushes/PRs from
/// the session itself). Shutdown is mid-work, not genuinely idle.
///
/// Conflict-resolution sessions are the subtle case: they run in a
/// `merge-tmp/<change-id>` worktree where the merge IS the original
/// change being applied. Proposing here creates a phantom second change
/// row keyed on the temp branch, which the post-commit hook never tags
/// (different worktree path), so it shows up in the UI without an
/// inline timeline card. The original change's `ChangeApplied` lands
/// shortly after — leaving the phantom orphaned at "pending". Refuse
/// here so the merge work flows back into the change being applied.
///
/// Background bash (the chat-agent `run_bash_background` tool or CC's own
/// `Bash{run_in_background:true}`) deliberately does NOT gate this. A CC
/// that idles `Generated` while a background task runs proposes the change
/// immediately so Apply surfaces without delay. Correctness is covered by
/// harden-at-apply: an un-hardened change re-runs `/harden` (tests included)
/// before it can merge, so a background test that hadn't finished can't land
/// broken work. App/external changes skip `/harden` and accept that risk.
/// This replaced an earlier gate whose only automatic recovery was a 5-minute
/// nudge — the wait that gate imposed was worse than the rare wasted re-harden
/// it prevented.
///
/// Every guard above applies identically to proposing and to reconciling —
/// hence the shared gate rather than two parallel checks.
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
/// `changed_files` is `None` when git could not answer: a spawn failure, the
/// 30s `git_cmd` timeout, or a non-zero exit (a deleted or renamed branch ref
/// is the common one). That is UNKNOWN, never a "no", so it preserves `prior`
/// rather than writing `false`. Writing `false` there is what left three live
/// external-repo threads with a dark Diff button: their branch was renamed
/// mid-session, `git diff <base>...<gone-ref>` exited 128, the swallowing
/// `branch_changed_files` wrapper turned that into an empty list, and the idle
/// wrote `has_changes: false` over the correct `true` the post-commit hook had
/// already put in `thread_summaries.coding_agent_has_diff`. The idle's answer
/// is the durable one and it runs last, so the hook cannot win that race.
///
/// An ANSWERED-but-empty probe still means `(false, false)`: a branch whose
/// commits cancelled out (commit then revert) genuinely has no diff, and
/// carrying a stale `true` forward there is the phantom-Apply regression named
/// in `may_touch_change_state_at_idle`'s doc comment.
///
/// `prior` is the thread's last known state: `has_changes` from
/// `thread_summaries.coding_agent_has_diff` (the column this event writes, and
/// the one the post-commit hook corrects mid-turn, so it is right even on a
/// first turn that has no previous idle to read), `requires_restart` from the
/// most recent CC turn-closer payload.
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
    /// `error_max_turns`, etc.). Carries the user-facing reason so
    /// `make_terminal_event` can populate `ResponseFailed.error` —
    /// without this the partial response renders as a complete answer.
    Failed {
        error: String,
    },
}

/// True when CC was woken up with no user-initiated content — engine-internal
/// warm-up resumes only. The follow-up `Result` event from such a turn is a
/// no-op and must not produce a terminal/idle event (the previous turn's
/// `CodingAgentIdled` already records the active `cc_session_id`). Image-only
/// turns count as content, so the call site must include images in the check —
/// otherwise CC delivers an answer but the thread row stays stuck at `running`.
pub(super) fn is_silent_resume(user_text_empty: bool, has_images: bool) -> bool {
    user_text_empty && !has_images
}

/// User-facing error message for the empty-response branch of `classify_result`.
/// Surfaces an OOM-killed bash (exit 137), a SIGTERM'd subprocess (exit 143), or
/// any other path where the backend produces a Result with no real assistant
/// text. Without this branch the empty Result classifies as `Generated` and the
/// turn looks successful in the UI even though the user got nothing back.
///
/// Worded for the coding agent generically rather than for Claude Code:
/// `classify_result` runs for both backends (`run_session` is backend-agnostic),
/// so naming one of them would mislabel every Codex turn that lands here.
pub(super) const EMPTY_RESPONSE_ERROR: &str =
    "The coding agent produced no visible response: the session ended without a real \
     assistant message. This usually means a tool call was killed (OOM with exit 137, \
     SIGTERM with exit 143, or similar). The work is incomplete; re-prompt with a \
     narrower scope.";

/// Decide what to emit when a CC `Result` event arrives — both the terminal
/// event kind and whether `CodingAgentIdled` should follow it. Returning both
/// decisions together prevents the TOCTOU race that would otherwise occur if
/// `is_shutdown` were read twice and observed different values across the two
/// branches: `ResponseGenerated` would fire (CC really finished) but the idle
/// event would be skipped, leaving the thread row stuck at `running`.
///
/// Every non-silent, non-shutdown Result is a turn boundary and emits
/// `CodingAgentIdled`. Inflight-followup race protection (deciding whether to
/// keep the subprocess alive) is the run-loop's responsibility, via
/// [`terminate_decision`].
///
/// Precedence: silent_resume > shutdown > user_hit_stop > cc_error >
/// text_is_empty > generated. Shutdown wins because the engine is going down
/// regardless of how CC ended the turn — the next process emits `Aborted` and
/// the recovery path re-resumes from there.
///
/// `user_hit_stop` sits ABOVE `cc_error`: the `Stop` button (Cancel = Esc) now
/// routes through CC's native interrupt, and an *interrupted* turn comes back as
/// a `Result` with `is_error: true` (CC reports the aborted turn — e.g.
/// `stop_reason=tool_use`, an `[ede_diagnostic]` line). That error is *caused by*
/// the user's cancel, so it must classify as `Canceled`, not `Failed` — otherwise
/// the user sees a red "Failed" dot for a turn they deliberately stopped, and the
/// branch-preservation gate (which keys on `Canceled`) never fires, so the
/// session loses its branch and can't resume. A real upstream failure on a turn
/// the user *didn't* stop still classifies as `Failed` (user_hit_stop is false
/// there). `text_is_empty` stays below user_hit_stop for the same reason — a
/// cancel landing on an empty Result is still a cancel.
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
        // A Codex mid-turn follow-up redirect is a cancel mechanically (no
        // ResponseGenerated, no change proposal) but NOT a user Stop — render it
        // neutrally via SupersededByFollowup.
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
/// boundary, so the latch — which records "the user interrupted the turn that
/// just ended" — must clear once that turn's cancel/abort terminal has fired.
///
/// The load-bearing case is an interrupt superseded by inflight follow-ups:
/// when the Stop button fires but follow-ups are already queued, the run loop
/// keeps the subprocess alive to drain them (`TerminateDecision::
/// KeepAliveForFollowup`). Those follow-ups complete with real, successful
/// output and emit a SECOND `Result`. Without clearing the latch here, that
/// Result re-classifies as `Canceled(UserStop)` — mislabeling finished,
/// committed work as "Canceled" and emitting a phantom second `ResponseCanceled`
/// carrying the completed text. Clearing makes the next Result classify on its
/// own merits (`Generated`).
///
/// `Generated` / `Failed` can't co-occur with a set latch (`user_hit_stop`
/// ranks above both in `classify_result`), so only the cancel/abort terminals
/// need to clear it.
pub(super) fn terminal_clears_user_hit_stop(terminal: &TerminalKind) -> bool {
    matches!(
        terminal,
        TerminalKind::Canceled(_) | TerminalKind::Aborted(_)
    )
}

/// Auto-commit the worktree on session cleanup ONLY when the last turn
/// finished cleanly (Generated) AND the user didn't ask to discard.
///
/// Anything else — safety-net abort (CC died mid-stream), Failed (upstream
/// API drop / OOM / empty Result), Canceled (user stop), Aborted (engine
/// shutdown), silent-resume warmup — leaves the worktree's contents
/// uncommitted on the branch. The post-commit hook fires per commit and
/// emits a per-commit `ChangeProposed`; without this gate, every cleanup
/// auto-commit on a half-finished session published a spurious Apply card
/// for partial work even though the aggregate `may_touch_change_state_at_idle`
/// gate had already refused.
///
/// Recovery is unaffected: `recover_orphaned_worktrees` re-spawns CC inside
/// the same worktree, and CC sees uncommitted dirt the same as committed
/// state — the user's prior edits are preserved either way.
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
/// owner of reclamation (ADR 0035). So this function does not ask whether the
/// work looks spent, whether the turn ended cleanly, or whether git can confirm
/// the tree is live: the user already answered all of that by clicking Discard,
/// and the branch is deleted alongside the tree.
///
/// One question is still worth asking, and it is the reason this is a function
/// rather than a bare `git worktree remove`: **is this tree actually ours?**
/// Claude Code can `git checkout` inside its own worktree, so a Discard aimed at
/// the session's branch must not delete a tree now sitting on someone else's.
/// `worktree_branch` is `None` for a detached HEAD or an unreadable one, and
/// neither is a positive match. That is the "unknown never authorizes
/// destruction" rule from `.claude/rules/rust.md` applied at the last remaining
/// destructive site on this path.
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
    /// The safety net just emitted `ContinuationRequested` for this session —
    /// the auto-recovery continuation resumes the SAME merge turn via
    /// `--resume`, so the merge duty transfers instead of failing: leave the
    /// in-progress merge, the worktree, the branch, the parked apply actor,
    /// and the event pairing (no `MergeResolutionCleared` /
    /// `ChangeApplyFailed`) untouched. The `Continue` spawn consumer re-derives
    /// the duty from the still-unpaired `MergeConflictDetected` and passes
    /// `conflict_change_id` to the resumed session, whose own completion then
    /// applies or aborts for real.
    HandOff,
}

/// Decide whether a conflict-resolution worktree is safe to fast-forward into
/// main. A clean `Generated` result is the only apply path — and it wins even
/// over a pending continuation: an external-watchdog false positive can race
/// a natural `Generated` end, and deferring an already-committed merge to a
/// fragile `--resume` would risk losing finished work (the continuation then
/// finds the pairing closed by `ChangeApplied` and degrades to a plain
/// resume). A user cancel also beats the hand-off — Stop means "don't land
/// this merge", continuation or not. After those two, a pending auto-recovery
/// continuation hands the duty off (see `HandOff`) — checked before
/// `has_unmerged`, because a stray-killed merge turn is EXPECTED to leave
/// unmerged files behind for the continuation to finish. Anything else means
/// the merge-fix turn was interrupted, failed, aborted, or never produced a
/// trustworthy terminal event, so the original change must stay pending.
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

/// A conflict-resolution Abort may delete git state ONLY when THIS session
/// actually ran in the dedicated temp worktree on the temp branch
/// (`merge_temp_branch` recorded by `MergeResolutionStarted` — the Tier-3
/// shape — AND matching the session's own branch). Tier-2 merges run in the
/// thread's OWN worktree on the REAL change branch; there the failed attempt
/// created nothing but the in-progress merge, so `git merge --abort` is the
/// entire cleanup — deleting the worktree/branch would destroy the user's
/// committed work (the failure-path-cleanup rule in `.claude/rules/rust.md`).
///
/// Matching against the SESSION's branch (not just `merge_temp_branch`
/// presence) is load-bearing: after a hand-off whose temp worktree was
/// pruned, the Continue consumer re-attaches the duty on the thread's own
/// worktree while the change row still carries the stale temp columns — a
/// presence-only gate would then `worktree remove --force` the THREAD
/// worktree the session actually ran in.
pub(super) fn conflict_abort_deletes_temp_state(
    merge_temp_branch: Option<&str>,
    session_branch: &str,
) -> bool {
    merge_temp_branch.is_some_and(|tb| tb == session_branch)
}

/// Inputs to [`is_stale_resume_signal`]. A named struct rather than a
/// positional argument list because every field is a `bool` — the same reason
/// [`super::external_watchdog::ExternalWatchdogInput`] exists next door. Eight
/// positional bools is a silent-transposition hazard at exactly the call site
/// where a transposition kills a live session.
///
/// `Copy` because one value feeds BOTH predicates at the call site:
/// [`is_stale_resume_signal`] and [`is_resume_settle_result`] read these
/// identical eight fields and differ only in which way `resume_attach_confirmed`
/// points, so restating them per call is exactly the transposition hazard above.
/// (The settle predicate takes one FURTHER argument, deliberately not a field
/// here; its doc says why.)
#[derive(Clone, Copy)]
pub(super) struct StaleResumeInputs {
    /// This turn asked the backend to `--resume` a specific session id.
    pub has_resume_session: bool,
    /// The backend's `Init` reported back the SAME session id we asked it to
    /// resume — proof the resume attached to the live conversation.
    pub resume_attach_confirmed: bool,
    pub result_text_empty: bool,
    pub buffered_text_empty: bool,
    pub no_prior_results_this_turn: bool,
    pub no_tool_calls_this_turn: bool,
    pub user_message_present: bool,
    pub cc_error: bool,
}

/// True when an arriving `Result` is the "stale --resume" signal — CC echoed
/// our forwarded user message back as an empty answer because the persisted
/// session id no longer exists. The run-loop responds by retrying with a fresh
/// spawn (reusing the existing worktree — it does NOT delete it).
///
/// `resume_attach_confirmed` is the STRUCTURAL gate and vetoes everything below
/// it. The rest of this predicate infers "the session is dead" from output
/// SHAPE, which cannot distinguish a dead session from a live one that happened
/// to say nothing. The session id can: both backends report the id they
/// actually attached to at `Init` (CC's `system.init`; Codex's
/// `thread/start`|`thread/resume` response), and a resume that FAILED yields a
/// different id — CC starts a fresh conversation, Codex falls back to
/// `thread/start` (`codex_app_server_tests::stale_resume_falls_back_to_fresh_thread`).
/// So `init_sid == requested_sid` proves the conversation is live, and no
/// amount of empty output may override it.
///
/// Without this gate the 2026-07-29 wedge: a *Switch to new version* auto-resume
/// re-attached correctly (Init echoed the requested sid), but the prior turn had
/// been killed mid-tool-call, so `claude --print --resume` auto-injected its own
/// synthetic `Continue from where you left off.` / `No response requested.` pair
/// to close the orphaned tool_use and emitted a `result` for THAT — before
/// reading our stdin. Empty text, zero tool calls, 10 ms after Init. The healthy
/// Opus-5 session was cancelled and the thread wedged at `running`.
///
/// `cc_error.is_none()` is load-bearing: an empty Result with `is_error: true`
/// is a real upstream failure (network drop, 5xx), not an expired session.
/// Without that gate a transient API error would trigger a spurious fresh-spawn
/// retry.
///
/// `no_tool_calls_this_turn` is load-bearing for terse models. A genuinely dead
/// resume produces an immediate EMPTY Result with ZERO activity (CC started a
/// fresh conversation with no context and had nothing to say). A live but terse
/// model — Fable-5 routinely emits ~0 assistant text and jumps straight to a
/// tool call — ALSO produces `result_text_empty && buffered_text_empty`, but it
/// MADE tool calls, which proves the session is alive and working. Requiring
/// zero tool calls this turn distinguishes "dead session" from "alive but
/// terse." Without it, every terse Fable resume was misclassified as stale,
/// cancelled, and re-spawned — the 2026-07-02 false stale-resume that spawned a
/// duplicate CC process on the shared worktree (2x quota burn). Opus/Sonnet
/// stream substantive prose first (`buffered_text_empty` is false), so they
/// never reached this predicate at all.
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
/// A `--resume` frequently has leftovers to settle before it will look at our
/// stdin. `claude --print --resume` injects a synthetic `Continue from where you
/// left off.` / `No response requested.` pair to close a tool_use orphaned by
/// the previous session's death, and it drains any `<task-notification>` that
/// session queued for a background Bash. Each of those is a full turn to CC, so
/// each ends with its own `result` on the wire, and the first one can land
/// BEFORE our prompt is even dequeued.
///
/// Such a Result must not terminate our turn, because our turn has not run yet.
/// Treating it as the terminal is what swallowed a user's message on 2026-08-05:
/// the settle Result classified as `Failed` via [`EMPTY_RESPONSE_ERROR`] (an OOM
/// diagnosis for a turn that never made an API call), and the idle path then
/// killed the subprocess 137 ms after CC had dequeued the user's follow-up and
/// started answering it. The answer was never produced and the message was never
/// re-queued. The 2026-07-29 veto had already stopped the same shape from
/// *cancelling* the healthy session; the call site just logged it and fell
/// through to the classification.
///
/// The run loop's response is to skip the Result entirely: no terminal, no idle,
/// no teardown, keep reading events. What arrives next is the real turn.
///
/// `no_api_call_this_turn` is what makes that skip safe, and it is a SEPARATE
/// argument rather than a ninth `StaleResumeInputs` field on purpose. The eight
/// fields describe output SHAPE, and the shape of a settle turn is identical to
/// the shape of a model that was asked our prompt and answered with nothing:
/// skipping the latter would discard a real terminal and strand the turn until
/// the inactivity watchdog fired ten minutes later. A `Usage` event separates
/// them structurally, because it is emitted per real API call and only for one
/// (the parser drops all-zero usage frames, which is exactly what a
/// `<synthetic>` message carries), so zero of them proves the backend never
/// asked the model anything and therefore cannot be answering us. It stays out
/// of the struct because [`is_stale_resume_signal`] must keep reading exactly
/// the eight fields it reads today: a dead `--resume` starts a FRESH
/// conversation and may well call the model before echoing nothing back, so
/// requiring zero API calls there could break the `dev/bf997e21` recovery.
///
/// Every shared field carries the same weight it does in the stale heuristic,
/// and for the same reasons: a tool call or streamed text proves the backend is
/// working on OUR prompt, a `cc_error` is a real failure to report, and a Result
/// later in the session is a genuine turn boundary.
/// `no_prior_results_this_turn` doubles as the bound: the call site records the
/// skipped Result, so a session can skip at most one, which is all a resume
/// settles.
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
/// This is the COMPLEMENT of the `!cc_error` gate in `is_stale_resume_signal`:
/// that heuristic must refuse to treat a `cc_error` as stale (a transient 5xx /
/// network drop would otherwise `worktree remove` + strand user work), but CC's
/// EXPLICIT `No conversation found with session ID: <id>` is not a transient
/// failure — it means the persisted session id is gone and re-resuming it can
/// never succeed. So it's the one whitelisted error string that still triggers
/// the fresh-spawn recovery even though it arrives AS a `cc_error`.
///
/// Root cause it recovers (dev/bf997e21): a mid-flight `CLAUDE_CONFIG_DIR` switch
/// relocates CC's per-session transcript store
/// (`$CLAUDE_CONFIG_DIR/projects/<cwd>/<sid>.jsonl`), so a resume spawned under
/// the new dir can't find a session created under the old one. Also covers a
/// pruned/deleted transcript, a CC version upgrade, or a different machine.
///
/// Matched on the human-readable error text because the `result` event we parse
/// exposes no structured "session not found" code (see `claude_code_parse.rs`).
/// This is a tolerance for CC's error-string contract — tracked in
/// `docs/temporary-measures.md`; switch to a structured signal if CC adds one.
pub(super) fn is_definitive_session_not_found(cc_error: Option<&str>) -> bool {
    cc_error.is_some_and(|e| e.contains("No conversation found with session ID"))
}

/// Decide what terminal event the stop arm of the run loop should emit.
///
/// Precedence: shutdown > user-action suppress > idle race > real Cancel.
/// Shutdown wins because the engine is going down regardless of which UI
/// button triggered the stop. Apply / Discard / Archive set
/// `suppress_user_terminal=true` because their own lifecycle event
/// (`ChangeApplied` / `ChangeDiscarded` / `ThreadArchived`) is the
/// terminator. An idle CC means the previous turn's `ResponseGenerated`
/// already terminated it, so even a real Cancel click that races in
/// emits nothing — labeling a finished turn "Canceled" would lie.
///
/// This decides *whether* a terminal fires and its base cause (`UserStop` for a
/// real Cancel). The redirect refinement to `SupersededByFollowup` is applied by
/// `emit_stop_terminal` (its only redirect-capable caller, the escalation arm),
/// so this pure function stays a 3-input truth table.
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
    /// Safety net fired (CC EOF without Result event) and commits exist on the
    /// branch. Keep the commits on disk so the user can resume and continue,
    /// but do NOT emit `ChangeProposed` — surfacing a half-finished crash as
    /// a ready-to-apply pending change misleads the user. The thread's
    /// terminal event is `ResponseAborted`, so the UI shows the crash state.
    CrashedKeepBranch,
    /// The user cancelled the turn (Stop button = Esc). Cancel is a *resumable
    /// turn boundary*, not a terminator — the next message `--resume`s the same
    /// `cc_session_id` on this branch. Keep the branch (even with zero commits)
    /// so `resolve_branch_for_resume` finds it; do NOT propose, because a
    /// cancelled turn is half-finished work the user shouldn't be invited to
    /// apply (mirrors `may_touch_change_state_at_idle`). Without this the
    /// no-commits cancel path falls into `KeepEmptyBranch` (still resumable, but
    /// without the cancel-specific "half-finished, don't propose" semantics).
    KeepCanceledBranch,
    /// Ordinary session end with no proposable diff (no commits, or commits
    /// whose changes cancel out). The thread is still ALIVE and resumable, so
    /// the branch is KEPT — the next message `--resume`s it via its
    /// `cc_session_id`, and `recover_orphaned_worktrees` keys on the branch ref
    /// to re-attach the session after a restart. Deleting a zero-commit branch
    /// here (the pre-2026-06 behavior) destroyed resumability — the
    /// thread-9e37697e data-loss class. The branch is a cheap ref; the
    /// worktree_cleanup sweep reclaims fully-merged branches once the thread is
    /// archived (or under disk pressure). Renamed from `CleanupBranches` when
    /// the deletion was removed — the action no longer cleans up anything.
    KeepEmptyBranch,
}

/// External repos with commits keep their branch even when the diff is empty —
/// the user owns push/PR for that ref and we won't `branch -D` something they
/// might want to keep.
///
/// `safety_net_fired` is set when CC's event loop ended without a Result event
/// (process crash, stream EOF, parser glitch). In that case any commits on the
/// branch reflect partial work — they stay on disk via `CrashedKeepBranch` but
/// are never proposed as a change. External repos keep `KeepExternalBranch`
/// regardless because the user owns push/PR for their own refs.
///
/// `user_canceled` is set when the last terminal was a user-driven
/// `Canceled(UserStop)` (the Stop button, which now routes through CC's native
/// interrupt/Esc). A cancel keeps the branch so the session stays resumable,
/// and never proposes — it ranks above `Propose`/`KeepEmptyBranch` but below the
/// external/crash keep-branch arms (those already keep the branch and carry
/// more specific semantics). It can't co-occur with `safety_net_fired`: a
/// cancel emits a terminal event, so `safety_net_fired` (= no terminal) is
/// false.
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

/// Clear all per-turn flags at a turn boundary so prior-turn state can't
/// leak into the next. `emitted_terminal_event` in particular gates the
/// post-loop safety net; without resetting it, a follow-up that ends with
/// CC exiting before producing a Result silently skips the safety net.
/// `last_terminal_kind` is per-turn for the same reason: the cleanup gate
/// (`should_auto_commit_on_cleanup`) must reflect THIS turn's outcome,
/// not whatever the previous turn happened to leave behind.
///
/// `cancel_actor` and `interrupt_is_redirect` are reset in lockstep with
/// `user_hit_stop`: the interrupt arm sets all three together (cancelling device
/// → `meta.actor`; redirect provenance → the cancel cause), so a new turn must
/// clear them together. Without this, a follow-up arriving on `msg_rx` during the
/// cancel+follow-up race clears the latch but leaves the prior turn's device on
/// `meta` (leaking it onto the follow-up's `CodingAgentPromptSent` /
/// `ResponseGenerated`) or a stale redirect flag that could mislabel a later
/// real Stop as `SupersededByFollowup`.
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

/// 10 minutes of CC silence in the narrow "awaiting Anthropic response"
/// window = subprocess hung. The window the watchdog protects is the
/// originally-observed bug: the network died during an in-flight Anthropic
/// API call, the kernel didn't notice (no RST, no FIN), and CC sat forever
/// on a dead TCP socket. Without this watchdog the engine waits forever —
/// the user sees a permanently "Working" thread even after restoring
/// network.
///
/// The watchdog is opt-in: it arms ONLY when CC is mid-turn AND no tool is
/// currently executing. Tool execution (Bash, Read, Grep, AskUserQuestion,
/// TaskOutput, agent sub-tasks, anything) is legitimate silence that we
/// trust CC to time out itself. Earlier wall-clock-only versions of this
/// watchdog killed CC during a 5-min `TaskOutput` poll on a background task
/// and during multi-minute `AskUserQuestion` waits — both legitimate, both
/// destroyed user work. The opt-in design fires for the dead-socket case
/// it was designed for and stays out of every other legitimate-silence
/// case.
///
/// `pub(crate)` (not `pub(super)`) because this limit only produces the
/// non-destructive outcome (kill plus auto-resume via `ContinuationRequested`)
/// when it is SHORTER than Claude Code's own byte-idle streaming deadline,
/// which ends the turn with a terminal `ResponseFailed` instead. The runtime
/// sets that deadline (`runtime::claude_code::CC_BYTE_STREAM_IDLE_TIMEOUT_MS`)
/// and asserts the ordering against this constant, so the two cannot drift into
/// the wrong order silently.
pub(crate) const WATCHDOG_INACTIVITY_LIMIT_MS: i64 = 10 * 60 * 1000;

/// Hard ceiling for the `tools_in_flight > 0` carve-out. The normal limit
/// (`WATCHDOG_INACTIVITY_LIMIT_MS` / `EXTERNAL_WATCHDOG_LIMIT_MS`) skips while a
/// tool is mid-execution because tool runtime is legitimate silence. But that
/// skip is otherwise *unbounded*: a tool call that never returns — e.g. a hung
/// `/harden` sub-agent (`Agent`/Task) or a wedged subprocess that stops emitting
/// without exiting — pins `tools_in_flight > 0` forever and neuters both
/// watchdogs (the 2026-06-22 thread-72120ca6 incident: 3 parallel harden
/// sub-agents never returned, thread stuck `running` for 3.5h). Past this
/// ceiling the watchdog fires *regardless* of `tools_in_flight` — but ONLY when
/// the caller confirms the thread is still `running` (genuine tool execution),
/// never when it is `waiting_for_user_answer` (a pending question/permission
/// card is deliberately counted in `tools_in_flight`, and the user may take
/// arbitrarily long to answer — see `WatchdogGate::FirePastCeiling`). 45 min is
/// deliberately longer than the longest legit single tool (a full `cargo`
/// release build + test suite), and the fire is non-destructive (auto-resume via
/// `--resume`), so a falsely-flagged slow tool costs at most a resume.
///
/// `pub(crate)` (not `pub(super)`) so engine construction (`engine_impl`) can
/// pass it to the external watchdog.
pub(crate) const WATCHDOG_HUNG_TOOL_CEILING_MS: i64 = 45 * 60 * 1000;

/// How often the agentic loop's `select!` re-evaluates the watchdog. Coarse
/// granularity (30s) — the limit is 10 min, so detection latency on a hung
/// subprocess is at most `WATCHDOG_INACTIVITY_LIMIT_MS + WATCHDOG_TICK`.
/// Smaller values would burn the `agent_sessions` mutex on every tick for
/// no benefit.
pub(super) const WATCHDOG_TICK_INTERVAL_SECS: u64 = 30;

/// Half the fire limit so the per-tick diagnostic surfaces before the fire
/// (rules out "elapsed wasn't far enough" when post-morteming a stuck thread).
pub(super) const WATCHDOG_DIAG_LOG_THRESHOLD_MS: i64 = WATCHDOG_INACTIVITY_LIMIT_MS / 2;

/// Outcome of one watchdog tick. The non-`Fire` variants name the gate that
/// held so the diagnostic log can pin the cause — the May-2026 stuck-thread
/// incident lacked this and post-hoc analysis could only guess.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum WatchdogGate {
    Fire,
    /// Past the hung-tool ceiling with tools still in flight. The caller MUST
    /// re-confirm the thread is still `running` (not `waiting_for_user_answer`,
    /// where a pending question/permission card is legitimately counted in
    /// `tools_in_flight`) before actually firing — the pure gate can't read the
    /// projection. Inner count surfaces in the diagnostic log.
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

/// What the post-loop safety net should emit when CC's event loop ended
/// without a natural terminator (`!emitted_terminal_event`). Always lands
/// SOMETHING terminal — either a `ContinuationRequested` so the spawn dispatcher
/// boots a fresh `--resume`, or `ResponseAborted(SafetyNet)` so the UI
/// flips to the red-dot abort state. Without this guard, a Claude Code subprocess
/// that died between events leaves the thread row stuck at `running`
/// indefinitely.
///
/// Precedence: external terminal already emitted > watchdog fired (auto-
/// recovery) > stray signal-kill (auto-recovery) > plain safety net (abort).
/// External terminal wins because the caller already landed a terminal (engine
/// restart, race), and a second terminal would relabel a finished turn.
/// Watchdog wins over plain abort because the watchdog's whole job is to bypass
/// the user-visible abort when the network died mid-call — auto-resume picks up
/// cleanly via `--resume` once the kernel notices the dead socket.
///
/// A stray signal-kill (`killed_by_signal`) the engine did NOT initiate
/// (`!engine_cancelled`) is the `exit=143` truncation bug: a SIGTERM reached
/// the CC child without a deliberate cancel. It is recoverable the same way —
/// auto-resume via `--resume` rather than a red-dot abort the user must clear.
/// Gated on `!engine_cancelled` so a deliberate teardown's own SIGKILL (user
/// Stop, shutdown, restart, eviction, stale-resume) never masquerades as a
/// stray kill. The process-group isolation fix should prevent the stray
/// SIGTERM in the first place; this is the defense-in-depth recovery for any
/// residual mid-stream signal death.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SafetyNetAction {
    /// Loop ended with a natural terminator (Generated, Failed, Canceled,
    /// Aborted) — the in-loop emit already closed the turn.
    Nothing,
    /// Watchdog killed CC mid-call (network died, dead TCP socket). Emit
    /// `ContinuationRequested` so the spawn dispatcher boots a fresh `--resume`
    /// without surfacing a user-visible abort.
    EmitContinuationRequested,
    /// CC's event loop died without a natural terminator and the watchdog
    /// didn't fire (driver crash, EOF on stdout without a Result, parser
    /// glitch). Emit `ResponseAborted(SafetyNet)` so the UI flips to the
    /// red-dot abort state — the user needs to know the work didn't
    /// complete.
    EmitAbortedSafetyNet,
    /// An external terminal already landed (engine restart fast-path, or a
    /// race with a concurrent cancel). Don't emit again — a second
    /// terminal relabels a turn that's already closed.
    Skip,
}

/// Decide what the post-loop safety net should emit. Pure function —
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
    // Stray external signal-kill (exit=143) with no deliberate engine cancel —
    // recover like the watchdog rather than surfacing an abort.
    if killed_by_signal && !engine_cancelled {
        return SafetyNetAction::EmitContinuationRequested;
    }
    SafetyNetAction::EmitAbortedSafetyNet
}

/// How many times in a row the engine will auto-resume a turn the backend ended
/// on a transient upstream API failure, before it stops and lets the red dot
/// stand.
///
/// The budget is CONSECUTIVE: a `ResponseGenerated` or a new user message resets
/// it (see `api_error_auto_resumes_spent`), so a long unattended session that
/// hits a drop every few hours never exhausts it. Three attempts with no
/// successful turn in between is no longer a transient drop, it is a broken
/// upstream, and resuming again just burns the user's quota against a wall.
pub(super) const MAX_API_ERROR_AUTO_RESUMES: i64 = 3;

/// True when a terminal is a TRANSIENT upstream failure the session can be
/// resumed past, as opposed to a deterministic one that would fail identically
/// on every retry.
///
/// The signature is Claude Code's own `API Error` prefix, which is already the
/// contract `claude_code_parse.rs` uses to decide that string is a real failure
/// reason at all (`API Error: 500 {…}`, `API Error: Stream idle timeout`, `API
/// Error: Connection closed mid-response.`). Matching the PREFIX rather than a
/// loose substring is what keeps a turn that merely mentions an api error in
/// prose out of the retry path, and it is the same rule on both sides so the two
/// cannot drift into disagreeing about what the string means.
///
/// Everything else is deliberately excluded. `error_max_turns` reproduces
/// exactly. `No conversation found with session ID` is handled far earlier by
/// the stale-resume path and can never succeed on a re-resume. The empty-response
/// failure is a killed tool, which a resume would just re-run. This is a
/// tolerance for an unstructured backend error string, registered in
/// `docs/temporary-measures.md`.
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
///   an `Aborted` one is already someone else's recovery, and a deterministic
///   `Failed` would reproduce.
/// * Budget spent (`MAX_API_ERROR_AUTO_RESUMES`): the upstream is not blipping,
///   it is down.
/// * Engine shutdown: recovery re-adopts in-flight threads after restart, and a
///   continuation emitted into a dying engine races that.
/// * Conflict-resolution session: it carries an apply's merge duty and a parked
///   actor, and its cleanup decides hand-off-vs-abort from the watchdog's own
///   flags. Adding a third continuation source to that interlock is deliberately
///   out of scope, so an API drop mid-merge still aborts the merge and leaves the
///   change pending for the user to Apply again.
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

/// Pure tick outcome. Fires (`Fire`) when CC is mid-turn (`!is_waiting`), no
/// tool is in flight (tool execution is legitimate silence — CC owns timing out
/// the tool), and the last event is past `limit_ms`. A zero `last_event_at_ms`
/// defensively skips (heartbeat uninitialized / race).
///
/// The `tools_in_flight > 0` skip is bounded by `ceiling_ms`: past the ceiling
/// it returns `FirePastCeiling` instead of `SkipToolsInFlight`, so a tool call
/// that never returns can't neuter the watchdog forever. `FirePastCeiling` is
/// advisory — the impure caller must confirm the thread is still `running`
/// (genuine tool execution) and not `waiting_for_user_answer` (a pending
/// question/permission card, legitimately counted in `tools_in_flight`, that
/// the user may take arbitrarily long to answer) before actually firing.
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
        // Past the ceiling, a stuck tool no longer earns an unbounded skip —
        // hand the caller a `FirePastCeiling` to act on after a projection
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
