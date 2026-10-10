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

/// Anchor: the default idle path with no inflight work kills CC. The next
/// turn arrives via `--resume` against a fresh subprocess.
///
/// This is also the arm that carries a turn which died on a transient upstream API
/// error to the idle exit where `maybe_auto_resume_after_api_error` lives, so a
/// false keep-alive here does not merely leak a subprocess, it silently cancels
/// that recovery.
#[test]
fn terminate_decision_default_terminates() {
    assert_eq!(
        terminate_decision(0, 0, false),
        TerminateDecision::Terminate,
        "nothing queued, nothing owed, no reservation: terminate is the default"
    );
}

/// A follow-up was sent but the run loop has not forwarded it yet, so it is sitting
/// in `msg_rx`. Keep CC alive so the next turn consumes it without a respawn
/// round-trip. This is the window the idle-termination race lives in
/// (`docs/plans/2026-06-27-cc-idle-termination-followup-race.md`).
#[test]
fn terminate_decision_queued_followup_keeps_alive() {
    assert_eq!(
        terminate_decision(1, 0, false),
        TerminateDecision::KeepAliveForFollowup {
            queued: 1,
            unread: 0,
            redirect_pending: false,
        },
        "a message unread in msg_rx must not die with the subprocess"
    );
}

/// `arm_followup_redirect` reserved the subprocess and its caller has not routed
/// the message yet, so it provably is not in `msg_rx`. This is the only window the
/// channel cannot answer for.
#[test]
fn terminate_decision_armed_redirect_keeps_alive() {
    assert_eq!(
        terminate_decision(0, 0, true),
        TerminateDecision::KeepAliveForFollowup {
            queued: 0,
            unread: 0,
            redirect_pending: true,
        },
        "an armed redirect's follow-up is coming even though the channel is empty"
    );
}

/// A running background task is not a follow-up. Its completion re-opens the
/// thread through an event wait, which resumes a terminated session. Holding
/// an idle subprocess for up to an hour would buy nothing.
#[test]
fn terminate_decision_has_no_background_task_keep_alive() {
    const LIFECYCLE_SRC: &str = include_str!("../lifecycle.rs");
    assert!(
        !LIFECYCLE_SRC.contains("KeepAliveForBgBash"),
        "a background task must not keep an idle coding-agent subprocess alive"
    );
}

/// Claude Code reads an input forwarded after a turn's last tool call only as a
/// second turn, after the first turn's `Result`. That read must re-arm the run
/// loop. Otherwise it drops every event of the second turn as a straggler, and
/// the agent's answer never reaches the transcript (ADR 0268).
#[test]
fn a_read_after_the_terminal_starts_a_turn() {
    assert!(starts_turn_after_terminal(false, true, true));
    assert!(
        starts_turn_after_terminal(true, false, true),
        "an answered question resumes a turn"
    );
    assert!(
        !starts_turn_after_terminal(false, true, false),
        "a read folded into the running turn starts nothing new"
    );
    assert!(
        !starts_turn_after_terminal(false, false, true),
        "a straggler, a stray replay, or a read on an exiting session stays dropped"
    );
}

/// A restart teardown emits the turn's terminal from outside the run loop. The
/// agent then answers the interrupt as if the user had rejected its work, and
/// the run loop drops that answer like any other straggler.
#[test]
fn an_external_terminal_ends_the_turn() {
    use std::sync::atomic::AtomicBool;
    assert!(
        turn_has_terminal(false, &AtomicBool::new(true)),
        "the teardown's terminal ends the turn the loop never ended itself"
    );
    assert!(turn_has_terminal(true, &AtomicBool::new(false)));
    assert!(
        !turn_has_terminal(false, &AtomicBool::new(false)),
        "a live turn keeps its output"
    );
}

#[test]
fn reset_per_turn_flags_clears_all_flags() {
    let mut is_waiting = true;
    let mut last_emitted_idle = true;
    let mut emitted_terminal_event = true;
    let mut user_hit_stop = true;
    let mut interrupt_is_redirect = true;
    let mut last_terminal_kind = Some(TerminalKind::Generated);
    let mut withheld_api_error = Some("API Error: dropped".to_string());
    let mut cancel_actor = Some(crate::engine::thread_events::MessageOrigin::Device {
        device_id: "ios-1".into(),
    });

    reset_per_turn_flags(
        &mut is_waiting,
        &mut last_emitted_idle,
        &mut emitted_terminal_event,
        &mut user_hit_stop,
        &mut interrupt_is_redirect,
        TurnTerminal {
            kind: &mut last_terminal_kind,
            withheld_api_error: &mut withheld_api_error,
        },
        &mut cancel_actor,
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
    assert!(
        !interrupt_is_redirect,
        "must clear in lockstep with user_hit_stop — else a stale redirect flag \
         could mislabel a later real Stop as SupersededByFollowup"
    );
    assert!(
        last_terminal_kind.is_none(),
        "must clear so the new turn's cleanup decision reflects THIS turn, \
         not the previous one — otherwise a Generated turn followed by a \
         safety-net abort would still auto-commit on cleanup"
    );
    assert!(
        withheld_api_error.is_none(),
        "must clear in lockstep with last_terminal_kind: carried into the next \
         turn it would resume off a decision taken two exits ago, and announce \
         that terminal's error for this one"
    );
    assert!(
        cancel_actor.is_none(),
        "must clear in lockstep with user_hit_stop — else a follow-up arriving \
         during the cancel race leaks the prior turn's cancelling device onto \
         the follow-up's events"
    );
}

/// Conflict-resolution sessions must never propose at idle — the merge
/// IS the original change being applied; a phantom second pending row
/// (keyed on `merge-tmp/<change-id>`) shows up in the UI and the
/// original change's `ChangeApplied` leaves it orphaned.
#[test]
fn conflict_session_never_proposes_change_at_idle() {
    // Not external, not shutdown, conflict session: must refuse — even on a
    // Generated terminal, because the merge result is the original change.
    assert!(
        idle_change_write(
            IdleSession {
                is_conflict_session: true,
                ..IdleSession::default()
            },
            &Some(TerminalKind::Generated)
        )
        .is_none(),
        "conflict-resolution sessions must NOT propose a phantom change at idle"
    );
}

/// Normal sessions that ended Generated may write change state — that's what
/// makes the Apply button appear. Anchor-test for the happy path.
#[test]
fn normal_generated_session_may_touch_change_state() {
    assert!(
        idle_change_write(IdleSession::default(), &Some(TerminalKind::Generated))
            == Some(IdleChangeWrite::Complete),
        "normal Generated session must reach the propose/reconcile branch"
    );
}

/// A clean idle on a thread still holding an event wait proposes no new change.
/// The agent parked mid-work, and the wait re-opens the thread for the idle
/// that proposes. An open change still follows the branch, so a standing apply
/// never fires on a stale row (ADR 0395).
#[test]
fn a_live_event_wait_holds_the_clean_idle_to_a_resync() {
    let waiting = IdleSession {
        holds_live_wait: true,
        ..IdleSession::default()
    };
    let write = idle_change_write(waiting, &Some(TerminalKind::Generated));
    assert_eq!(write, Some(IdleChangeWrite::Resync));
    assert!(
        !write.unwrap().may_create(),
        "a held idle creates no change"
    );
    assert!(
        idle_change_write(IdleSession::default(), &Some(TerminalKind::Generated))
            .unwrap()
            .may_create()
    );
}

/// The hold covers finished-looking work only. A user Stop with a wait live is
/// still unfinished, so its work is withheld and announced (ADR 0400).
#[test]
fn a_live_event_wait_does_not_hold_a_user_stop() {
    use crate::engine::thread_events::CancelCause;
    let waiting = IdleSession {
        holds_live_wait: true,
        ..IdleSession::default()
    };
    assert_eq!(
        idle_change_write(
            waiting,
            &Some(TerminalKind::Canceled(CancelCause::UserStop))
        ),
        Some(IdleChangeWrite::Unfinished)
    );
}

/// The gate takes NO "does the branch have a diff" input, and that omission is
/// load-bearing rather than an oversight. Folding it in (the old
/// `should_propose_change_at_idle`) made the empty-diff arm unreachable, so a
/// branch whose commits cancelled out never reconciled its pending change and
/// the card kept advertising files the live Diff didn't show (change
/// `2cc8391f`). A clean Generated idle must pass this gate either way; the
/// caller then routes on the file list — propose when non-empty, reconcile the
/// existing pending row to zero when empty.
#[test]
fn gate_is_blind_to_whether_the_branch_has_a_diff() {
    let clean_idle = idle_change_write(IdleSession::default(), &Some(TerminalKind::Generated))
        == Some(IdleChangeWrite::Complete);
    assert!(
        clean_idle,
        "an empty-diff Generated idle must still reach the reconcile arm"
    );
}

#[test]
fn external_repo_does_not_propose() {
    assert!(
        idle_change_write(
            IdleSession {
                is_external_repo: true,
                ..IdleSession::default()
            },
            &Some(TerminalKind::Generated)
        )
        .is_none(),
        "external repos manage their own push/PR — no Lucidos change row"
    );
}

#[test]
fn shutdown_does_not_propose() {
    assert!(
        idle_change_write(
            IdleSession {
                is_shutdown: true,
                ..IdleSession::default()
            },
            &Some(TerminalKind::Generated)
        )
        .is_none(),
        "shutdown is mid-work, not a genuine idle — would create a spurious panel on resume"
    );
}

/// A failed terminal (CC error, empty Result, mid-stream API drop) is an
/// unfinished turn. Its work stays on the branch, withheld and announced, and
/// the user resumes or discards it (ADR 0400).
#[test]
fn failed_terminal_withholds_at_idle() {
    let write = idle_change_write(
        IdleSession::default(),
        &Some(TerminalKind::Failed {
            error: "stream interrupted".into(),
        }),
    );
    assert_eq!(write, Some(IdleChangeWrite::Unfinished));
    assert!(!write.unwrap().finished(), "a failed turn never proposes");
}

/// A user Stop withholds what the turn left: an unfinished turn never proposes
/// (ADR 0400). The other cancels write nothing: a redirect's follow-up
/// continues the branch, and an Apply or Discard stop owns the change itself.
#[test]
fn only_a_user_stop_withholds_a_cancelled_turn() {
    use crate::engine::thread_events::CancelCause;
    let write =
        |cause| idle_change_write(IdleSession::default(), &Some(TerminalKind::Canceled(cause)));
    assert_eq!(
        write(CancelCause::UserStop),
        Some(IdleChangeWrite::Unfinished)
    );
    for cause in [
        CancelCause::SupersededByFollowup,
        CancelCause::UserAction,
        CancelCause::Unknown,
    ] {
        assert_eq!(write(cause), None, "{cause:?} must write nothing");
    }
}

/// An aborted terminal is unfinished, so its work is withheld. At engine
/// shutdown the `is_shutdown` gate refuses first, and boot settles the turn.
#[test]
fn aborted_terminal_withholds_unless_the_engine_is_shutting_down() {
    use crate::engine::thread_events::AbortCause;
    let aborted = Some(TerminalKind::Aborted(AbortCause::EngineShutdown));
    assert_eq!(
        idle_change_write(IdleSession::default(), &aborted),
        Some(IdleChangeWrite::Unfinished)
    );
    assert_eq!(
        idle_change_write(
            IdleSession {
                is_shutdown: true,
                ..IdleSession::default()
            },
            &aborted
        ),
        None,
        "shutdown leaves the turn to boot recovery"
    );
}

/// Only a finished turn proposes. A resync keeps an open change in step, and
/// an unfinished turn proposes nothing at all.
#[test]
fn only_a_finished_write_proposes() {
    assert!(IdleChangeWrite::Complete.finished());
    assert!(IdleChangeWrite::Resync.finished());
    assert!(!IdleChangeWrite::Unfinished.finished());
}

/// Safety-net abort sets terminal_kind = None (no terminal was emitted
/// inside the loop). This is the regression we're guarding: previously
/// the cleanup auto-commit fired the post-commit hook, emitting a
/// spurious per-commit ChangeProposed even though the aggregate gate
/// here had no terminal kind to act on. The cleanup auto-commit is now
/// gated by `should_auto_commit_on_cleanup` which makes the same
/// decision for the per-commit path; this test pins that the aggregate
/// path also refuses None.
#[test]
fn no_terminal_kind_does_not_propose_at_idle() {
    assert!(
        idle_change_write(IdleSession::default(), &None).is_none(),
        "None terminal (safety-net abort) must NOT auto-propose"
    );
}

// -------------------- idle_change_flags --------------------

#[test]
fn answered_non_empty_probe_reports_changes() {
    let files = vec!["crates/lucidos-engine/src/engine/mod.rs".to_string()];
    assert_eq!(
        idle_change_flags(Some(&files), (false, false)),
        (true, true),
        "a real diff sets has_changes, and a .rs file requires a restart"
    );
}

#[test]
fn answered_non_empty_probe_without_restart_files() {
    let files = vec!["docs/notes.md".to_string()];
    assert_eq!(
        idle_change_flags(Some(&files), (false, false)),
        (true, false)
    );
}

#[test]
fn answered_empty_probe_clears_a_previously_true_state() {
    // Commit then revert: git ANSWERED, and the answer is that the branch
    // carries no diff. Carrying `true` forward here is the phantom-Apply
    // regression `idle_change_write` documents.
    assert_eq!(
        idle_change_flags(Some(&[]), (true, true)),
        (false, false),
        "an answered-empty diff must clear the state, not carry it forward"
    );
}

#[test]
fn unanswerable_probe_preserves_a_true_state() {
    // The renamed-branch bug: `git diff <base>...<gone-ref>` exits 128. That
    // is UNKNOWN, and must never downgrade the Diff button to dark.
    assert_eq!(
        idle_change_flags(None, (true, true)),
        (true, true),
        "git could not answer, so the thread keeps the state it already had"
    );
}

#[test]
fn unanswerable_probe_preserves_a_false_state() {
    assert_eq!(
        idle_change_flags(None, (false, false)),
        (false, false),
        "unknown preserves, it does not invent changes either"
    );
}

#[test]
fn a_held_proposal_nudges_once_per_branch_head() {
    use IdleChangeWrite::Complete;
    assert!(plan_nudge_due(Complete, Some("a1"), None));
    assert!(
        !plan_nudge_due(Complete, Some("a1"), Some("a1")),
        "an agent that ignored the nudge must end its turn, not loop"
    );
    assert!(
        plan_nudge_due(Complete, Some("b2"), Some("a1")),
        "new commits earn a new nudge"
    );
}

#[test]
fn a_held_proposal_never_nudges_after_a_user_stop() {
    assert!(!plan_nudge_due(
        IdleChangeWrite::Unfinished,
        Some("a1"),
        None
    ));
}

#[test]
fn an_unreadable_head_nudges_once() {
    use IdleChangeWrite::Complete;
    assert!(plan_nudge_due(Complete, None, None));
    assert!(!plan_nudge_due(Complete, None, Some("")));
}
