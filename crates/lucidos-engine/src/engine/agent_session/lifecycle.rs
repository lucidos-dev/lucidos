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

/// Decide whether to snapshot the worktree as a pending change when CC
/// goes idle.
///
/// External repos never produce Lucidos changes (the dev pushes/PRs from
/// the session itself). Shutdown is mid-work, not genuinely idle —
/// snapshotting partial work creates a spurious "Change proposed" panel
/// that the resumed session would have to clean up.
///
/// Conflict-resolution sessions are the subtle case: they run in a
/// `merge-tmp/<change-id>` worktree where the merge IS the original
/// change being applied. Proposing here creates a phantom second change
/// row keyed on the temp branch, which the post-commit hook never tags
/// (different worktree path), so it shows up in the UI without an
/// inline timeline card. The original change's `ChangeApplied` lands
/// shortly after — leaving the phantom orphaned at "pending". Refuse
/// here so the merge work flows back into the change being applied.
pub(super) fn should_propose_change_at_idle(
    wt_has_changes: bool,
    is_external_repo: bool,
    is_shutdown: bool,
    is_conflict_session: bool,
) -> bool {
    wt_has_changes && !is_external_repo && !is_shutdown && !is_conflict_session
}

/// Which terminal event closes the current CC turn. Cancel/Abort variants
/// carry the typed cause so the emit site cannot mis-classify why the turn
/// ended.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum TerminalKind {
    Generated,
    Canceled(crate::engine::thread_events::CancelCause),
    Aborted(crate::engine::thread_events::AbortCause),
    /// CC reported the turn ended in failure (mid-stream API error,
    /// `error_max_turns`, etc.). Carries the user-facing reason so
    /// `make_terminal_event` can populate `ResponseFailed.error` —
    /// without this the partial response renders as a complete answer.
    Failed { error: String },
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
/// any other path where CC produces a Result with no real assistant text. Without
/// this branch the empty Result classifies as `Generated` and the turn looks
/// successful in the UI even though the user got nothing back.
pub(super) const EMPTY_RESPONSE_ERROR: &str =
    "Claude Code produced no visible response — the session ended without a real \
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
/// keep the subprocess alive) is the run-loop's responsibility — see
/// `AgentSession.pending_followups`.
///
/// Precedence: silent_resume > shutdown > cc_error > user_hit_stop >
/// text_is_empty > generated. Shutdown wins because the engine is going down
/// regardless of how CC ended the turn — the next process emits `Aborted` and
/// the recovery path re-resumes from there. `text_is_empty` sits below
/// user_hit_stop because a deliberate user cancel that happens to land on an
/// empty Result is still a cancel, not a silent failure.
pub(super) fn classify_result(
    is_silent_resume: bool,
    user_hit_stop: bool,
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
    } else if let Some(error) = cc_error {
        TerminalKind::Failed { error }
    } else if user_hit_stop {
        TerminalKind::Canceled(CancelCause::UserStop)
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

/// True when the change being proposed at idle was produced by a `Failed`
/// turn (CC streamed an API error and exited). Only `Failed` qualifies —
/// `Canceled` is a deliberate user stop (apply may still make sense),
/// `Aborted` is engine-side and the worktree is preserved for recovery.
pub(super) fn change_is_incomplete_from_terminal(
    terminal_kind: &Option<TerminalKind>,
) -> bool {
    matches!(terminal_kind, Some(TerminalKind::Failed { .. }))
}

/// True when an arriving `Result` is the "stale --resume" signal — CC echoed
/// our forwarded user message back as an empty answer because the persisted
/// session id no longer exists. The run-loop responds by killing the worktree
/// and retrying with a fresh spawn.
///
/// `cc_error.is_none()` is load-bearing: an empty Result with `is_error: true`
/// is a real upstream failure (network drop, 5xx), not an expired session.
/// Without that gate a transient API error would delete the worktree and
/// branch, destroying user work.
pub(super) fn is_stale_resume_signal(
    has_resume_session: bool,
    result_text_empty: bool,
    buffered_text_empty: bool,
    no_prior_results_this_turn: bool,
    user_message_present: bool,
    cc_error: bool,
) -> bool {
    has_resume_session
        && result_text_empty
        && buffered_text_empty
        && no_prior_results_this_turn
        && user_message_present
        && !cc_error
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
    CleanupBranches,
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
pub(super) fn classify_session_end_action(
    has_commits: bool,
    proposal_files_empty: bool,
    is_external_repo: bool,
    safety_net_fired: bool,
) -> SessionEndAction {
    match (has_commits, is_external_repo, safety_net_fired) {
        (true, true, _) => SessionEndAction::KeepExternalBranch,
        (true, false, true) => SessionEndAction::CrashedKeepBranch,
        (true, false, false) if !proposal_files_empty => SessionEndAction::Propose,
        _ => SessionEndAction::CleanupBranches,
    }
}

/// Clear all per-turn flags at a turn boundary so prior-turn state can't
/// leak into the next. `emitted_terminal_event` in particular gates the
/// post-loop safety net; without resetting it, a follow-up that ends with
/// CC exiting before producing a Result silently skips the safety net.
pub(super) fn reset_per_turn_flags(
    is_waiting: &mut bool,
    last_emitted_idle: &mut bool,
    emitted_terminal_event: &mut bool,
    user_hit_stop: &mut bool,
) {
    *is_waiting = false;
    *last_emitted_idle = false;
    *emitted_terminal_event = false;
    *user_hit_stop = false;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `EndSession` must translate to a plain `break` at the call site, not
    /// `stop.notify_one()` — the stop arm would emit a phantom
    /// `ResponseCanceled` on top of the natural `ResponseGenerated`.
    #[test]
    fn conflict_resolution_ends_session_on_idle() {
        assert_eq!(
            idle_action(true, false),
            IdleAction::EndSession,
            "conflict resolution must end the loop, not route through the stop signal"
        );
    }

    #[test]
    fn conflict_resolution_during_shutdown_still_ends_session() {
        // The merge worktree is throwaway and recovery never resumes a
        // conflict session; shutdown does not change the action.
        assert_eq!(idle_action(true, true), IdleAction::EndSession);
    }

    #[test]
    fn normal_idle_outside_shutdown_exits_subprocess() {
        // Runtime contract: every non-shutdown idle kills CC. The next turn
        // arrives via `--resume` against a fresh subprocess.
        assert_eq!(
            idle_action(false, false),
            IdleAction::ExitSubprocess,
            "non-shutdown idle must exit so the next turn re-spawns via --resume"
        );
    }

    #[test]
    fn normal_idle_during_shutdown_does_nothing() {
        // The post-loop shutdown branch preserves the worktree+branch so
        // `recover_orphaned_worktrees` can resume after restart. Killing CC
        // here would race with that.
        assert_eq!(
            idle_action(false, true),
            IdleAction::Nothing,
            "shutdown idle must NOT touch CC — preserves worktree for recovery"
        );
    }

    #[test]
    fn reset_per_turn_flags_clears_all_flags() {
        let mut is_waiting = true;
        let mut last_emitted_idle = true;
        let mut emitted_terminal_event = true;
        let mut user_hit_stop = true;

        reset_per_turn_flags(
            &mut is_waiting,
            &mut last_emitted_idle,
            &mut emitted_terminal_event,
            &mut user_hit_stop,
        );

        assert!(!is_waiting, "CC is no longer waiting — a new turn began");
        assert!(!last_emitted_idle, "next idle emission is for the new turn");
        assert!(
            !emitted_terminal_event,
            "must clear so the post-loop safety net can fire if the new turn \
             ends without producing a Result event"
        );
        assert!(
            !user_hit_stop,
            "stop applies to the prior turn's response, not the next one"
        );
    }

    /// Conflict-resolution sessions must never propose at idle — the merge
    /// IS the original change being applied; a phantom second pending row
    /// (keyed on `merge-tmp/<change-id>`) shows up in the UI and the
    /// original change's `ChangeApplied` leaves it orphaned.
    #[test]
    fn conflict_session_never_proposes_change_at_idle() {
        // wt_has_changes=true (the merge result IS files), not external,
        // not shutdown, conflict session: must refuse.
        assert!(
            !should_propose_change_at_idle(true, false, false, true),
            "conflict-resolution sessions must NOT propose a phantom change at idle"
        );
    }

    /// Normal sessions with worktree changes do propose — that's what makes
    /// the Apply button appear. Anchor-test for the happy path.
    #[test]
    fn normal_session_with_changes_proposes_at_idle() {
        assert!(
            should_propose_change_at_idle(true, false, false, false),
            "normal session with changes must propose so Apply button surfaces"
        );
    }

    #[test]
    fn empty_worktree_does_not_propose() {
        assert!(
            !should_propose_change_at_idle(false, false, false, false),
            "no point proposing when the worktree has no changes"
        );
    }

    #[test]
    fn external_repo_does_not_propose() {
        assert!(
            !should_propose_change_at_idle(true, true, false, false),
            "external repos manage their own push/PR — no Lucidos change row"
        );
    }

    #[test]
    fn shutdown_does_not_propose() {
        assert!(
            !should_propose_change_at_idle(true, false, true, false),
            "shutdown is mid-work, not a genuine idle — would create a spurious panel on resume"
        );
    }

    /// CC merges back-to-back stdin inputs into a single Result, so the
    /// engine's `pending_followups` counter cannot be relied on to predict
    /// whether more Results are coming. A normal `Generated` Result must
    /// always emit `CodingAgentIdled` regardless of inflight inputs —
    /// otherwise the thread sits in stored=Archived → DisplaySection::Archive
    /// instead of Review. Inflight-followup race protection lives in the
    /// run-loop's subprocess-termination decision, not here.
    #[test]
    fn generated_result_always_emits_idle() {
        let (terminal, emit_idle) = classify_result(false, false, false, None, false);
        assert_eq!(terminal, Some(TerminalKind::Generated));
        assert!(
            emit_idle,
            "Generated Result must emit CodingAgentIdled so the thread reaches \
             Review — not Archive — after CC produces a Result"
        );
    }

    /// CC's stream-json result with `is_error: true` — produced when the
    /// upstream API call dies mid-stream — must classify as `Failed` so the
    /// frontend renders the partial response with a red failure indicator
    /// instead of the green check ResponseGenerated would produce.
    #[test]
    fn cc_error_classifies_as_failed() {
        let err = "Stream interrupted: connection reset".to_string();
        let (terminal, emit_idle) =
            classify_result(false, false, false, Some(err.clone()), false);
        assert_eq!(terminal, Some(TerminalKind::Failed { error: err }));
        assert!(
            emit_idle,
            "Failed Result is still a turn boundary — must emit CodingAgentIdled \
             so the next follow-up resumes via --resume"
        );
    }

    /// Shutdown beats CC error — the engine is going down regardless of how
    /// CC ended its turn. Without this, an `Aborted` recovery path would
    /// double-emit on restart (Failed already landed, then Aborted overwrites).
    #[test]
    fn shutdown_wins_over_cc_error() {
        let (terminal, emit_idle) = classify_result(
            false,
            false,
            true,
            Some("api timeout".to_string()),
            false,
        );
        assert_eq!(
            terminal,
            Some(TerminalKind::Aborted(
                crate::engine::thread_events::AbortCause::EngineShutdown
            ))
        );
        assert!(
            !emit_idle,
            "shutdown must skip CodingAgentIdled regardless of cc_error"
        );
    }

    /// CC error beats user_hit_stop — the user clicking Stop and CC failing
    /// in the same window is rare, but the underlying failure is more
    /// actionable than a benign cancel label.
    #[test]
    fn cc_error_wins_over_user_hit_stop() {
        let err = "upstream 503".to_string();
        let (terminal, _) =
            classify_result(false, true, false, Some(err.clone()), false);
        assert_eq!(terminal, Some(TerminalKind::Failed { error: err }));
    }

    /// Empty assistant text on an otherwise-clean turn classifies as `Failed`,
    /// not `Generated`. Without this branch, a CC subprocess that bailed after
    /// an OOM-killed Bash (exit 137) emits `ResponseGenerated { text: "" }`
    /// and `CodingAgentIdled { has_changes: true }` — the UI then shows a
    /// silent "completed" turn even though the user got nothing back, and
    /// the partial worktree changes look reviewable. Routing through `Failed`
    /// surfaces the red dot AND tags the change as incomplete via
    /// `change_is_incomplete_from_terminal`.
    #[test]
    fn empty_text_classifies_as_failed_with_empty_response_error() {
        let (terminal, emit_idle) =
            classify_result(false, false, false, None, true);
        assert_eq!(
            terminal,
            Some(TerminalKind::Failed {
                error: EMPTY_RESPONSE_ERROR.to_string(),
            })
        );
        assert!(
            emit_idle,
            "empty-text Failed is still a turn boundary — must emit \
             CodingAgentIdled so the dispatcher closes the turn"
        );
    }

    /// CC's own error message wins over the generic empty-text fallback —
    /// without this, an `is_error: true` Result with empty text would surface
    /// "no visible response" instead of the actual upstream cause (e.g. a
    /// rate-limit or 5xx). The engine's failure message must be the more
    /// specific one when CC has told us why.
    #[test]
    fn cc_error_wins_over_empty_text() {
        let err = "rate_limit_error".to_string();
        let (terminal, _) =
            classify_result(false, false, false, Some(err.clone()), true);
        assert_eq!(terminal, Some(TerminalKind::Failed { error: err }));
    }

    /// User-driven cancel that happens to land on an empty Result is still a
    /// cancel — the user clicked Stop, the turn ended deliberately. Routing
    /// to Failed here would mislabel a deliberate stop as an unexpected
    /// failure and break the cancel UX.
    #[test]
    fn user_hit_stop_wins_over_empty_text() {
        use crate::engine::thread_events::CancelCause;
        let (terminal, _) = classify_result(false, true, false, None, true);
        assert_eq!(
            terminal,
            Some(TerminalKind::Canceled(CancelCause::UserStop))
        );
    }

    /// Shutdown wins over empty text — engine going down classifies as
    /// `Aborted` (not `Failed`) regardless of what CC sent. Without this,
    /// a shutdown that lands on an empty Result would emit ResponseFailed
    /// and the recovery path would skip re-resuming the session.
    #[test]
    fn shutdown_wins_over_empty_text() {
        use crate::engine::thread_events::AbortCause;
        let (terminal, emit_idle) =
            classify_result(false, false, true, None, true);
        assert_eq!(
            terminal,
            Some(TerminalKind::Aborted(AbortCause::EngineShutdown))
        );
        assert!(
            !emit_idle,
            "shutdown must skip CodingAgentIdled even when text is empty"
        );
    }

    /// Silent resume drops the empty-text Failed too — a warmup with no user
    /// content always emits nothing, regardless of what CC produced. Without
    /// this, an engine-internal warmup resume would surface a spurious
    /// "no visible response" failure on a thread the user never engaged.
    #[test]
    fn silent_resume_drops_empty_text_too() {
        let (terminal, emit_idle) =
            classify_result(true, false, false, None, true);
        assert!(terminal.is_none());
        assert!(!emit_idle);
    }

    /// `change_is_incomplete_from_terminal` already returns true for
    /// `TerminalKind::Failed { .. }` (covered by
    /// `change_is_incomplete_from_terminal_table` above). This pin asserts the
    /// empty-text Failed flows through that same path — without it, an OOM
    /// "completion" would propose changes as if the work were complete
    /// instead of warning the user that the change is incomplete.
    #[test]
    fn empty_text_failed_marks_change_incomplete() {
        let (terminal, _) =
            classify_result(false, false, false, None, true);
        assert!(
            change_is_incomplete_from_terminal(&terminal),
            "empty-text Failed must mark the proposed change as incomplete \
             so the apply UI confirms before landing partial work"
        );
    }

    /// Empty Result on a resumed turn with no error → real stale-resume
    /// signal. The run-loop kills the worktree+branch and retries with a
    /// fresh spawn.
    #[test]
    fn empty_result_on_resume_with_no_error_is_stale_resume() {
        assert!(is_stale_resume_signal(
            true,  // has_resume_session
            true,  // result_text_empty
            true,  // buffered_text_empty
            true,  // no_prior_results_this_turn
            true,  // user_message_present
            false, // cc_error
        ));
    }

    /// Empty Result on a resumed turn WITH a CC-reported error → real
    /// upstream failure, NOT stale resume. Without this guard a transient
    /// network drop would `worktree remove --force` + `branch -D`, destroying
    /// user work that the live session was about to commit.
    #[test]
    fn empty_result_on_resume_with_cc_error_is_not_stale_resume() {
        assert!(!is_stale_resume_signal(
            true,  // has_resume_session
            true,  // result_text_empty
            true,  // buffered_text_empty
            true,  // no_prior_results_this_turn
            true,  // user_message_present
            true,  // cc_error
        ));
    }

    /// Non-resumed turn never qualifies (the retry path only makes sense
    /// when the dead session id actually came from a prior CodingAgentIdled).
    #[test]
    fn fresh_session_is_never_stale_resume() {
        assert!(!is_stale_resume_signal(false, true, true, true, true, false));
    }

    /// Silent resume (warmup with no user content) still drops every Result,
    /// even when CC reports an error. Without this, an error-during-warmup
    /// would emit ResponseFailed against a thread the user never engaged.
    #[test]
    fn silent_resume_drops_cc_error_too() {
        let (terminal, emit_idle) = classify_result(
            true,
            false,
            false,
            Some("error_during_execution".to_string()),
            false,
        );
        assert!(terminal.is_none());
        assert!(!emit_idle);
    }

    /// Pin every input combination to its expected (terminal, emit_idle) pair.
    /// The invariants this guards:
    ///   - `Generated` → `emit_idle = true`. Skipping idle here is the bug —
    ///     CC really finished, so the thread row must flip to `waiting`/`idle`
    ///     and the section must move to Review.
    ///   - `Aborted` → `emit_idle = false`. Emitting idle on shutdown makes
    ///     `recover_orphaned_worktrees` think the session is "truly idle" and
    ///     skip recovery on restart.
    ///   - `Canceled` (user-driven stop) → `emit_idle = true`. Cancel is a
    ///     turn boundary; the dispatcher needs `CodingAgentIdled` to pick up
    ///     the next message via `--resume`.
    #[test]
    fn classify_result_table() {
        use crate::engine::thread_events::{AbortCause, CancelCause};
        let cases = [
            // (is_silent_resume, user_hit_stop, is_shutdown) → (terminal, emit_idle)
            ((false, false, false), (Some(TerminalKind::Generated), true)),
            (
                (false, true, false),
                (Some(TerminalKind::Canceled(CancelCause::UserStop)), true),
            ),
            (
                (false, false, true),
                (Some(TerminalKind::Aborted(AbortCause::EngineShutdown)), false),
            ),
            // Shutdown overrides user_hit_stop — Aborted, idle skipped.
            (
                (false, true, true),
                (Some(TerminalKind::Aborted(AbortCause::EngineShutdown)), false),
            ),
            // Silent resume / warmup (no user content) emits nothing.
            ((true, false, false), (None, false)),
            ((true, true, false), (None, false)),
            ((true, false, true), (None, false)),
            ((true, true, true), (None, false)),
        ];
        for ((silent, stop, shutdown), expected) in cases {
            assert_eq!(
                classify_result(silent, stop, shutdown, None, false),
                expected,
                "(is_silent_resume={}, user_hit_stop={}, is_shutdown={})",
                silent,
                stop,
                shutdown,
            );
        }
    }

    /// Real Cancel click on an actively-working CC emits `ResponseCanceled` —
    /// this is the only path that should ever produce that event.
    #[test]
    fn real_cancel_on_working_cc_emits_canceled() {
        use crate::engine::thread_events::CancelCause;
        assert_eq!(
            stop_terminal_kind(false, false, false),
            Some(TerminalKind::Canceled(CancelCause::UserStop)),
            "real Cancel click on actively-working CC must emit Canceled"
        );
    }

    /// Cancel click that races in after CC went idle emits nothing — the
    /// previous turn's `ResponseGenerated` already terminated it. Without
    /// this, the late Cancel would land a phantom "Canceled the response"
    /// on a turn that finished cleanly.
    #[test]
    fn cancel_racing_idle_emits_no_terminal_event() {
        assert_eq!(
            stop_terminal_kind(false, true, false),
            None,
            "Cancel that raced after CC went idle must NOT emit Canceled — \
             previous turn already finished cleanly"
        );
    }

    /// Apply / Discard / Archive trigger the stop signal but their own
    /// lifecycle event (`ChangeApplied` / `ChangeDiscarded` / `ThreadArchived`)
    /// is the terminator. The stop arm must NOT emit `ResponseCanceled` on
    /// top — the user didn't cancel.
    #[test]
    fn user_action_suppresses_terminal_regardless_of_idle() {
        assert_eq!(
            stop_terminal_kind(false, false, true),
            None,
            "Apply/Discard/Archive on actively-working CC must NOT emit Canceled — \
             the lifecycle event is the terminator"
        );
        assert_eq!(
            stop_terminal_kind(false, true, true),
            None,
            "Apply/Discard/Archive on idle CC must NOT emit Canceled either — \
             nothing in flight to cancel and the lifecycle event is the terminator"
        );
    }

    /// Shutdown of an idle CC must emit nothing — the prior exchange already
    /// completed cleanly via `CodingAgentIdled`, and emitting `Aborted` here
    /// would relabel a finished exchange as crashed when the engine goes
    /// down.
    #[test]
    fn shutdown_of_idle_session_emits_no_terminal_event() {
        assert_eq!(
            stop_terminal_kind(true, true, false),
            None,
            "shutdown when CC is already idle must NOT emit a terminal event"
        );
        assert_eq!(
            stop_terminal_kind(true, true, true),
            None,
            "shutdown wins over user-action — idle still emits nothing"
        );
    }

    /// Shutdown of a working CC emits `Aborted` — the in-flight exchange was
    /// killed by the engine, not finished by CC. Shutdown wins over the
    /// user-action suppress flag — the system kill is the dominant cause.
    #[test]
    fn shutdown_of_working_session_emits_aborted() {
        use crate::engine::thread_events::AbortCause;
        assert_eq!(
            stop_terminal_kind(true, false, false),
            Some(TerminalKind::Aborted(AbortCause::EngineShutdown)),
            "shutdown during active work must emit Aborted"
        );
        assert_eq!(
            stop_terminal_kind(true, false, true),
            Some(TerminalKind::Aborted(AbortCause::EngineShutdown)),
            "shutdown wins over user-action suppression — Aborted still fires"
        );
    }

    /// `Failed` terminal kind tags the resulting `ChangeProposed` so the apply
    /// UI confirms before landing partial work. `Generated` / `Canceled` /
    /// `Aborted` / `None` must NOT tag — `Canceled` is a deliberate user stop
    /// where applying makes sense, `Aborted` is engine-side and the worktree
    /// is preserved for recovery, `None` is silent-resume warmup with no
    /// change to propose.
    #[test]
    fn change_is_incomplete_from_terminal_table() {
        use crate::engine::thread_events::{AbortCause, CancelCause};
        assert!(change_is_incomplete_from_terminal(&Some(TerminalKind::Failed {
            error: "stream interrupted".into(),
        })));
        assert!(!change_is_incomplete_from_terminal(&Some(
            TerminalKind::Generated
        )));
        assert!(!change_is_incomplete_from_terminal(&Some(
            TerminalKind::Canceled(CancelCause::UserStop)
        )));
        assert!(!change_is_incomplete_from_terminal(&Some(
            TerminalKind::Aborted(AbortCause::EngineShutdown)
        )));
        assert!(!change_is_incomplete_from_terminal(&None));
    }

    /// Only "no text AND no images" counts as silent — image-only turns are
    /// real user content. Skipping the image check here freezes the thread row
    /// at `running` after CC delivers an answer, because `classify_result`
    /// then suppresses both the terminal event and `CodingAgentIdled`.
    #[test]
    fn image_only_message_is_not_silent_resume() {
        assert!(!is_silent_resume(true, true));
        assert!(is_silent_resume(true, false));
        assert!(!is_silent_resume(false, false));
        assert!(!is_silent_resume(false, true));
    }

    /// Pin every input combination of `classify_session_end_action`. The
    /// invariants this guards:
    ///   - `(has_commits=true, files_empty=true, external=false)` → `CleanupBranches`,
    ///     not `Propose`. This is the phantom-Change regression — observed
    ///     when CC's auto-commit on cleanup advances the branch ref while
    ///     the user concurrently clicked Apply Now, leaving the post-Apply
    ///     branch with commits whose contents already live on main. Without
    ///     this filter the engine emits a `ChangeProposed` with no files
    ///     and the thread title as the fallback description, which the
    ///     frontend renders as a pending Change the user can only Discard.
    ///   - External repos with commits keep their branch regardless of
    ///     `files_empty` — the user owns push/PR there; deleting the ref
    ///     because the net diff happens to be zero would lose work.
    ///   - No commits on the branch always cleans up, regardless of the
    ///     other inputs (the diff signal is moot).
    #[test]
    fn classify_session_end_action_table() {
        use SessionEndAction::*;
        let cases = [
            // (has_commits, files_empty, is_external, safety_net_fired) → action
            //
            // Healthy turn (safety_net_fired=false) — same as before this column existed:
            ((true, false, false, false), Propose),
            ((true, true, false, false), CleanupBranches), // phantom-Change regression
            ((true, false, true, false), KeepExternalBranch),
            ((true, true, true, false), KeepExternalBranch),
            ((false, false, false, false), CleanupBranches),
            ((false, true, false, false), CleanupBranches),
            ((false, false, true, false), CleanupBranches),
            ((false, true, true, false), CleanupBranches),
            //
            // Safety-net fired — CC died mid-stream:
            //   - In our own repo with commits: CrashedKeepBranch (keep work,
            //     no ChangeProposed). files_empty doesn't matter; even an
            //     empty-diff commit is partial work.
            //   - External repo with commits: still KeepExternalBranch — user
            //     owns the ref regardless of how the session ended.
            //   - No commits: CleanupBranches — nothing to keep.
            ((true, false, false, true), CrashedKeepBranch),
            ((true, true, false, true), CrashedKeepBranch),
            ((true, false, true, true), KeepExternalBranch),
            ((true, true, true, true), KeepExternalBranch),
            ((false, false, false, true), CleanupBranches),
            ((false, true, false, true), CleanupBranches),
            ((false, false, true, true), CleanupBranches),
            ((false, true, true, true), CleanupBranches),
        ];
        for ((has_commits, files_empty, is_external, safety_net_fired), expected) in cases {
            assert_eq!(
                classify_session_end_action(
                    has_commits,
                    files_empty,
                    is_external,
                    safety_net_fired,
                ),
                expected,
                "(has_commits={has_commits}, files_empty={files_empty}, is_external={is_external}, safety_net_fired={safety_net_fired})",
            );
        }
    }
}
