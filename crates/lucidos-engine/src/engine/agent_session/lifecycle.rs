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
pub(super) fn classify_result(
    is_silent_resume: bool,
    user_hit_stop: bool,
    is_shutdown: bool,
) -> (Option<TerminalKind>, bool) {
    if is_silent_resume {
        return (None, false);
    }
    let terminal = if is_shutdown {
        TerminalKind::Aborted
    } else if user_hit_stop {
        TerminalKind::Canceled
    } else {
        TerminalKind::Generated
    };
    (Some(terminal), !is_shutdown)
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

    /// Pin every input combination to its expected (terminal, emit_idle) pair.
    /// The two invariants this guards:
    ///   - `Generated` → `emit_idle = true`. Skipping idle here is the bug —
    ///     CC really finished, so the thread row must flip to `waiting`/`idle`.
    ///   - `Aborted` → `emit_idle = false`. Emitting idle on shutdown makes
    ///     `recover_orphaned_worktrees` think the session is "truly idle" and
    ///     skip recovery on restart.
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
                classify_result(silent, stop, shutdown),
                expected,
                "(is_silent_resume={}, user_hit_stop={}, is_shutdown={})",
                silent,
                stop,
                shutdown,
            );
        }
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
}
