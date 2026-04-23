/// Decide whether a CC session should auto-end when it goes idle.
/// Only conflict resolution auto-ends — the merge is already committed,
/// there's no Add to Changes / Apply Now choice to make.
/// All other sessions (normal, resumed, orphan recovery) stay idle so
/// the user can review and choose what to do with the changes.
pub(super) fn should_auto_end_on_idle(is_conflict: bool) -> bool {
    is_conflict
}

/// Decide whether a CC event while idle means CC resumed work.
/// `still_waiting` = CC was waiting before this event AND no handler
/// (Message/ToolUse) cleared it during processing. System/hook noise is
/// already filtered out by `parse_line` (returns no event), so any event
/// that reaches us is meaningful.
pub(super) fn should_clear_waiting(still_waiting: bool, saw_result: bool) -> bool {
    still_waiting && !saw_result
}

/// Which terminal event closes the current CC turn.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum TerminalKind {
    Generated,
    Canceled,
    Aborted,
}

/// Decide what to emit when a CC `Result` event arrives — both the terminal
/// event kind and whether `CodingAgentIdled` should follow it. Returning both
/// decisions together prevents the TOCTOU race that would otherwise occur if
/// `is_shutdown` were read twice and observed different values across the two
/// branches: `ResponseGenerated` would fire (CC really finished) but the idle
/// event would be skipped, leaving the thread row stuck at `running`.
pub(super) fn classify_result(
    user_message_empty: bool,
    user_hit_stop: bool,
    is_shutdown: bool,
) -> (Option<TerminalKind>, bool) {
    if user_message_empty {
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
    fn clearing_waiting_fires_on_event_while_idle() {
        assert!(
            should_clear_waiting(true, false),
            "any event while waiting (and not a Result) should trigger clearing waiting"
        );
    }

    #[test]
    fn clearing_waiting_skipped_when_result_seen() {
        assert!(
            !should_clear_waiting(true, true),
            "must not clear when saw_result is true"
        );
    }

    #[test]
    fn clearing_waiting_skipped_when_not_waiting() {
        assert!(
            !should_clear_waiting(false, false),
            "must not clear when CC wasn't waiting"
        );
    }

    #[test]
    fn clearing_waiting_skipped_when_handler_already_cleared() {
        let was_waiting = true;
        let is_waiting = false;
        assert!(
            !should_clear_waiting(was_waiting && is_waiting, false),
            "must not clear when a handler already cleared is_waiting"
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
            // (user_msg_empty, user_hit_stop, is_shutdown) → (terminal, emit_idle)
            ((false, false, false), (Some(TerminalKind::Generated), true)),
            ((false, true, false), (Some(TerminalKind::Canceled), true)),
            ((false, false, true), (Some(TerminalKind::Aborted), false)),
            // Shutdown overrides user_hit_stop — Aborted, idle skipped.
            ((false, true, true), (Some(TerminalKind::Aborted), false)),
            // Empty user_message (silent resume / warmup) emits nothing.
            ((true, false, false), (None, false)),
            ((true, true, false), (None, false)),
            ((true, false, true), (None, false)),
            ((true, true, true), (None, false)),
        ];
        for ((empty, stop, shutdown), expected) in cases {
            assert_eq!(
                classify_result(empty, stop, shutdown),
                expected,
                "(user_msg_empty={}, user_hit_stop={}, is_shutdown={})",
                empty,
                stop,
                shutdown,
            );
        }
    }
}
