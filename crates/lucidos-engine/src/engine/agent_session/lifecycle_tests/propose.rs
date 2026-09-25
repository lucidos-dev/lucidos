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
        label: "My iPhone".into(),
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
        !may_touch_change_state_at_idle(false, false, true, &Some(TerminalKind::Generated)),
        "conflict-resolution sessions must NOT propose a phantom change at idle"
    );
}

/// Normal sessions that ended Generated may write change state — that's what
/// makes the Apply button appear. Anchor-test for the happy path.
#[test]
fn normal_generated_session_may_touch_change_state() {
    assert!(
        may_touch_change_state_at_idle(false, false, false, &Some(TerminalKind::Generated)),
        "normal Generated session must reach the propose/reconcile branch"
    );
}

/// A `Generated` idle proposes immediately even though a background bash task
/// may still be running. This is the deliberate reversal of the old bg-bash
/// propose-gate: the 5-minute wait that gate imposed was worse than the rare
/// wasted re-harden it prevented. Correctness is covered by harden-at-apply
/// (un-hardened changes re-run `/harden` before they can merge); app/external
/// changes skip `/harden` and accept the risk. The gate no longer takes a
/// bg-bash parameter at all — this anchors that the decision is blind to
/// background bash.
#[test]
fn generated_proposes_regardless_of_background_bash() {
    assert!(
        may_touch_change_state_at_idle(false, false, false, &Some(TerminalKind::Generated)),
        "Generated idle with changes must propose immediately — background \
         bash no longer gates the proposal (harden-at-apply is the net)"
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
    let clean_idle =
        may_touch_change_state_at_idle(false, false, false, &Some(TerminalKind::Generated));
    assert!(
        clean_idle,
        "an empty-diff Generated idle must still reach the reconcile arm"
    );
}

#[test]
fn external_repo_does_not_propose() {
    assert!(
        !may_touch_change_state_at_idle(true, false, false, &Some(TerminalKind::Generated)),
        "external repos manage their own push/PR — no Lucidos change row"
    );
}

#[test]
fn shutdown_does_not_propose() {
    assert!(
        !may_touch_change_state_at_idle(false, true, false, &Some(TerminalKind::Generated)),
        "shutdown is mid-work, not a genuine idle — would create a spurious panel on resume"
    );
}

/// Failed terminal (CC error / empty Result / mid-stream API drop) MUST
/// NOT auto-propose. Previously this proposed with `incomplete: true`
/// flag — the user's directive is to never auto-surface half-assed work
/// for Apply. The work stays in the worktree on the branch; the user
/// can resume the thread to continue or discard manually.
#[test]
fn failed_terminal_does_not_propose_at_idle() {
    assert!(
        !may_touch_change_state_at_idle(
            false,
            false,
            false,
            &Some(TerminalKind::Failed {
                error: "stream interrupted".into(),
            })
        ),
        "Failed terminal must NOT auto-propose — half-assed work, no Apply card"
    );
}

/// User clicked Stop mid-turn. The work is partial — never auto-surface
/// for Apply. The user can resume the thread to continue.
#[test]
fn canceled_terminal_does_not_propose_at_idle() {
    use crate::engine::thread_events::CancelCause;
    assert!(
        !may_touch_change_state_at_idle(
            false,
            false,
            false,
            &Some(TerminalKind::Canceled(CancelCause::UserStop))
        ),
        "Canceled terminal must NOT auto-propose — user stopped mid-work"
    );
}

/// Aborted terminal (engine shutdown) MUST NOT propose. The is_shutdown
/// gate would already refuse, but pin the terminal-kind gate independently
/// so the rule reads from one place instead of relying on two parallel
/// checks staying in sync.
#[test]
fn aborted_terminal_does_not_propose_at_idle() {
    use crate::engine::thread_events::AbortCause;
    assert!(
        !may_touch_change_state_at_idle(
            false,
            false,
            false,
            &Some(TerminalKind::Aborted(AbortCause::EngineShutdown))
        ),
        "Aborted terminal must NOT auto-propose — engine-side, work preserved for recovery"
    );
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
        !may_touch_change_state_at_idle(false, false, false, &None),
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
    // regression `may_touch_change_state_at_idle` documents.
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
