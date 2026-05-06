/// Decide whether a CC session should auto-end when it goes idle.
/// Only conflict resolution auto-ends — the merge is already committed,
/// there's no Add to Changes / Apply Now choice to make.
/// All other sessions (normal, resumed, orphan recovery) stay idle so
/// the user can review and choose what to do with the changes.
pub(super) fn should_auto_end_on_idle(is_conflict: bool) -> bool {
    is_conflict
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

/// Decide whether the CC subprocess should exit when the turn goes idle.
///
/// The runtime contract is uniform: every idle exits the CC subprocess,
/// regardless of whether a change was committed during the turn. The next
/// follow-up arrives via `--resume` against a fresh subprocess. The CC
/// session id is persisted by the engine, the worktree+branch persist on
/// disk, and `--resume` rehydrates the conversation — there is no benefit
/// to keeping an idle subprocess in memory between turns, and doing so
/// previously caused inconsistencies (a kept-alive process for "no change"
/// idles vs. a killed process for "change" idles meant two different
/// recovery paths to keep correct).
///
/// Engine shutdown is the single exception. During shutdown the post-loop
/// branch preserves the worktree+branch so `recover_orphaned_worktrees` can
/// resume the session after restart; killing the subprocess here would race
/// with that preservation path. Shutdown is therefore the only case where
/// the subprocess is allowed to outlive the idle event.
///
/// End-to-end verification of the session-map clearing post-idle is
/// exercised by browser e2e (`cc-resume-after-exit.spec.ts`) and the
/// follow-up verification smoke; the unit tests below pin only this pure
/// decision predicate.
pub(super) fn should_exit_subprocess_on_idle(is_shutdown: bool) -> bool {
    !is_shutdown
}

/// Which terminal event closes the current CC turn.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum TerminalKind {
    Generated,
    Canceled,
    Aborted,
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
/// Precedence: shutdown > cc_error > user_hit_stop > generated. Shutdown wins
/// because the engine is going down regardless of how CC ended the turn — the
/// next process emits `Aborted` and the recovery path re-resumes from there.
pub(super) fn classify_result(
    is_silent_resume: bool,
    user_hit_stop: bool,
    is_shutdown: bool,
    cc_error: Option<String>,
) -> (Option<TerminalKind>, bool) {
    if is_silent_resume {
        return (None, false);
    }
    let terminal = if is_shutdown {
        TerminalKind::Aborted
    } else if let Some(error) = cc_error {
        TerminalKind::Failed { error }
    } else if user_hit_stop {
        TerminalKind::Canceled
    } else {
        TerminalKind::Generated
    };
    let emit_idle = !is_shutdown;
    (Some(terminal), emit_idle)
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

/// Decide what terminal event the cancel arm of the run loop should emit when
/// the cancel signal fires (user clicked Cancel, or engine shutdown propagated
/// the cancel via the same channel).
///
/// User-driven cancel always emits `Canceled` — including when CC has just
/// transitioned to `is_waiting` (the race between `CodingAgentIdled` landing
/// and `cancel.notified()` firing). Without this, the prior `CodingAgentIdled`
/// alone would render the exchange as "Done" even though the user explicitly
/// clicked Cancel.
///
/// Shutdown of an actively-working CC emits `Aborted`. Shutdown of an
/// already-idle CC emits nothing — the exchange completed legitimately and
/// must not be relabeled "Aborted" by the engine going down.
pub(super) fn cancel_terminal_kind(
    is_shutdown: bool,
    is_waiting: bool,
) -> Option<TerminalKind> {
    match (is_shutdown, is_waiting) {
        (true, true) => None,
        (true, false) => Some(TerminalKind::Aborted),
        (false, _) => Some(TerminalKind::Canceled),
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum SessionEndAction {
    Propose,
    KeepExternalBranch,
    CleanupBranches,
}

/// External repos with commits keep their branch even when the diff is empty —
/// the user owns push/PR for that ref and we won't `branch -D` something they
/// might want to keep.
pub(super) fn classify_session_end_action(
    has_commits: bool,
    proposal_files_empty: bool,
    is_external_repo: bool,
) -> SessionEndAction {
    match (has_commits, is_external_repo) {
        (true, true) => SessionEndAction::KeepExternalBranch,
        (true, false) if !proposal_files_empty => SessionEndAction::Propose,
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

    #[test]
    fn conflict_resolution_auto_ends_on_idle() {
        assert!(
            should_auto_end_on_idle(true),
            "conflict resolution must auto-end"
        );
    }

    #[test]
    fn non_conflict_sessions_do_not_auto_end_on_idle() {
        assert!(!should_auto_end_on_idle(false),
            "non-conflict sessions must NOT auto-end — user needs Add to Changes / Apply Now choice");
    }

    #[test]
    fn idle_outside_shutdown_exits_the_cc_subprocess() {
        // Runtime contract: every non-shutdown idle exits. The follow-up
        // arrives via `--resume` against a fresh subprocess.
        assert!(
            should_exit_subprocess_on_idle(false),
            "non-shutdown idle must exit so the next turn re-spawns via --resume"
        );
    }

    #[test]
    fn idle_during_shutdown_keeps_the_cc_subprocess_for_recovery() {
        // The post-loop shutdown branch preserves the worktree+branch so
        // `recover_orphaned_worktrees` can resume after restart. Killing CC
        // here would race with that.
        assert!(
            !should_exit_subprocess_on_idle(true),
            "shutdown idle must NOT trigger subprocess exit — preserves worktree for recovery"
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
        let (terminal, emit_idle) = classify_result(false, false, false, None);
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
            classify_result(false, false, false, Some(err.clone()));
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
        );
        assert_eq!(terminal, Some(TerminalKind::Aborted));
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
            classify_result(false, true, false, Some(err.clone()));
        assert_eq!(terminal, Some(TerminalKind::Failed { error: err }));
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
        let cases = [
            // (is_silent_resume, user_hit_stop, is_shutdown) → (terminal, emit_idle)
            ((false, false, false), (Some(TerminalKind::Generated), true)),
            ((false, true, false), (Some(TerminalKind::Canceled), true)),
            ((false, false, true), (Some(TerminalKind::Aborted), false)),
            // Shutdown overrides user_hit_stop — Aborted, idle skipped.
            ((false, true, true), (Some(TerminalKind::Aborted), false)),
            // Silent resume / warmup (no user content) emits nothing.
            ((true, false, false), (None, false)),
            ((true, true, false), (None, false)),
            ((true, false, true), (None, false)),
            ((true, true, true), (None, false)),
        ];
        for ((silent, stop, shutdown), expected) in cases {
            assert_eq!(
                classify_result(silent, stop, shutdown, None),
                expected,
                "(is_silent_resume={}, user_hit_stop={}, is_shutdown={})",
                silent,
                stop,
                shutdown,
            );
        }
    }

    /// User-driven cancel always emits `Canceled`, even when CC has just
    /// reached `is_waiting=true` between cancel.notify_one() and the cancel
    /// arm firing. Without this, the prior `CodingAgentIdled` alone would
    /// resolve the exchange to "Done" — but the user explicitly clicked
    /// Cancel and expects to see "Canceled".
    #[test]
    fn user_cancel_emits_canceled_even_when_idle_raced_in_first() {
        assert_eq!(
            cancel_terminal_kind(false, true),
            Some(TerminalKind::Canceled),
            "user cancel during the is_waiting race must still emit Canceled"
        );
        assert_eq!(
            cancel_terminal_kind(false, false),
            Some(TerminalKind::Canceled),
            "user cancel of an actively-working CC must emit Canceled"
        );
    }

    /// Shutdown of an idle CC must emit nothing — the prior exchange already
    /// completed cleanly via `CodingAgentIdled`, and emitting `Aborted` here
    /// would relabel a finished exchange as crashed when the engine goes
    /// down.
    #[test]
    fn shutdown_of_idle_session_emits_no_terminal_event() {
        assert_eq!(
            cancel_terminal_kind(true, true),
            None,
            "shutdown when CC is already idle must NOT emit a terminal event"
        );
    }

    /// Shutdown of a working CC emits `Aborted` — the in-flight exchange was
    /// killed by the engine, not finished by CC.
    #[test]
    fn shutdown_of_working_session_emits_aborted() {
        assert_eq!(
            cancel_terminal_kind(true, false),
            Some(TerminalKind::Aborted),
            "shutdown during active work must emit Aborted"
        );
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
            // (has_commits, files_empty, is_external) → action
            ((true, false, false), Propose),
            ((true, true, false), CleanupBranches), // ← phantom-Change regression
            ((true, false, true), KeepExternalBranch),
            ((true, true, true), KeepExternalBranch),
            ((false, false, false), CleanupBranches),
            ((false, true, false), CleanupBranches),
            ((false, false, true), CleanupBranches),
            ((false, true, true), CleanupBranches),
        ];
        for ((has_commits, files_empty, is_external), expected) in cases {
            assert_eq!(
                classify_session_end_action(has_commits, files_empty, is_external),
                expected,
                "(has_commits={has_commits}, files_empty={files_empty}, is_external={is_external})",
            );
        }
    }
}
