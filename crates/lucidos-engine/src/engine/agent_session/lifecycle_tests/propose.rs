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
        terminate_decision(0, 0, false, false),
        TerminateDecision::Terminate,
        "nothing queued, nothing owed, no reservation, no bg bash: terminate is the default"
    );
}

/// A warm-up / silent-resume turn seeds `inputs_awaiting_result` at 0 and its
/// Result settles to 0. It used to need its own reasoning because a count of 1 was
/// ambiguous between "the initial turn owes a Result" and "a follow-up is queued";
/// now it reaches the same state as the default case by a different route.
#[test]
fn terminate_decision_warm_up_turn_terminates() {
    let awaiting = settle_inputs_awaiting_result(crate::runtime::CodingAgent::ClaudeCode, 0, false);
    assert_eq!(
        terminate_decision(0, awaiting, false, false),
        TerminateDecision::Terminate,
        "a warm-up turn owes nothing after its Result: terminate"
    );
}

/// The regression this signature exists for. Claude Code merged three forwarded
/// instructions into one Result, so nothing is owed and nothing is queued, and the
/// old `> 1` rule read the raw count of 3 as two follow-ups in flight. It kept a
/// finished session alive and swallowed the API-drop auto-resume on 2026-08-07.
#[test]
fn terminate_decision_merged_claude_code_turn_terminates() {
    let awaiting = settle_inputs_awaiting_result(crate::runtime::CodingAgent::ClaudeCode, 3, false);
    assert_eq!(
        terminate_decision(0, awaiting, false, false),
        TerminateDecision::Terminate,
        "a merged Claude Code turn owes nothing: terminate so the API-drop resume can run"
    );
}

/// The other half of the same rule, and the one that keeps this fix from trading
/// the phantom for a dropped message. `select!` can forward an input and only then
/// hand the loop a `Result` the agent produced before that input arrived, so the
/// Result cannot have answered it. Zeroing there would cancel the subprocess with
/// the user's message still inside.
#[test]
fn terminate_decision_claude_code_result_predating_a_forward_keeps_alive() {
    let awaiting = settle_inputs_awaiting_result(crate::runtime::CodingAgent::ClaudeCode, 2, true);
    assert_eq!(
        terminate_decision(0, awaiting, false, false),
        TerminateDecision::KeepAliveForFollowup {
            queued: 0,
            awaiting_result: 1,
            redirect_pending: false,
        },
        "a Result that predates the forward leaves that input owed: keep the subprocess"
    );
}

/// A follow-up was sent but the run loop has not forwarded it yet, so it is sitting
/// in `msg_rx`. Keep CC alive so the next turn consumes it without a respawn
/// round-trip. This is the window the idle-termination race lives in
/// (`docs/plans/2026-06-27-cc-idle-termination-followup-race.md`).
#[test]
fn terminate_decision_queued_followup_keeps_alive() {
    assert_eq!(
        terminate_decision(1, 0, false, false),
        TerminateDecision::KeepAliveForFollowup {
            queued: 1,
            awaiting_result: 0,
            redirect_pending: false,
        },
        "a message unread in msg_rx must not die with the subprocess"
    );
}

/// Codex emits one Result per accepted input, so after the interrupted turn's
/// Result the driver still owes one and `TurnOutcome::Continue` is holding it.
/// Killing the subprocess here would drop work the user already sent.
#[test]
fn terminate_decision_codex_still_owes_a_result_keeps_alive() {
    let awaiting = settle_inputs_awaiting_result(crate::runtime::CodingAgent::Codex, 2, false);
    assert_eq!(
        terminate_decision(0, awaiting, false, false),
        TerminateDecision::KeepAliveForFollowup {
            queued: 0,
            awaiting_result: 1,
            redirect_pending: false,
        },
        "Codex owes one more Result: keep the driver alive to run its queued input"
    );
}

/// The same Codex session one turn later: the queued input has been answered, so
/// nothing is owed and the subprocess goes.
#[test]
fn terminate_decision_codex_fully_settled_terminates() {
    let awaiting = settle_inputs_awaiting_result(crate::runtime::CodingAgent::Codex, 1, false);
    assert_eq!(
        terminate_decision(0, awaiting, false, false),
        TerminateDecision::Terminate,
        "Codex has answered every forwarded input: terminate"
    );
}

/// `arm_followup_redirect` reserved the subprocess and its caller has not routed
/// the message yet, so it provably is not in `msg_rx`. This is the only window the
/// channel cannot answer for.
#[test]
fn terminate_decision_armed_redirect_keeps_alive() {
    assert_eq!(
        terminate_decision(0, 0, true, false),
        TerminateDecision::KeepAliveForFollowup {
            queued: 0,
            awaiting_result: 0,
            redirect_pending: true,
        },
        "an armed redirect's follow-up is coming even though the channel is empty"
    );
}

/// Chat-agent's `run_bash_background` is the long-standing skip.
/// `spawn_bash_completion_watcher` re-wakes CC via `msg_tx` on
/// completion; killing here would force the wake path through stale-
/// session recovery.
#[test]
fn terminate_decision_chat_bg_bash_keeps_alive() {
    assert_eq!(
        terminate_decision(0, 0, false, true),
        TerminateDecision::KeepAliveForBgBash,
        "chat-agent bg bash pending: keep CC alive for the auto-wake"
    );
}

/// Precedence: a queued follow-up is the strongest signal, outranking
/// the bg-bash flag. User-initiated input takes priority.
#[test]
fn terminate_decision_followup_wins_over_bg_bash() {
    assert_eq!(
        terminate_decision(1, 0, false, true),
        TerminateDecision::KeepAliveForFollowup {
            queued: 1,
            awaiting_result: 0,
            redirect_pending: false,
        },
        "user follow-up beats chat-agent bg bash: consume the message first"
    );
}

/// Claude Code merges back-to-back stdin inputs into a single Result, so one Result
/// answers everything forwarded so far, whatever N was, as long as the agent had
/// spoken since the last forward.
#[test]
fn settle_claude_code_zeroes_any_backlog() {
    for before in [0_u32, 1, 2, 3, 17] {
        assert_eq!(
            settle_inputs_awaiting_result(crate::runtime::CodingAgent::ClaudeCode, before, false),
            0,
            "a Claude Code Result answers all {before} forwarded input(s) at once"
        );
    }
}

/// The qualifier. A Result the loop received with no agent output since the last
/// forward may have been produced before that input arrived, so it cannot have
/// answered it: one input stays owed and the next Result settles it. Without this,
/// the merge rule silently cancels a subprocess that is still holding the user's
/// message, which is the race the counter this replaced was written against.
#[test]
fn settle_claude_code_keeps_one_when_the_result_may_predate_the_forward() {
    assert_eq!(
        settle_inputs_awaiting_result(crate::runtime::CodingAgent::ClaudeCode, 2, true),
        1,
        "the forwarded-but-unanswered input stays owed"
    );
    assert_eq!(
        settle_inputs_awaiting_result(crate::runtime::CodingAgent::ClaudeCode, 0, true),
        0,
        "nothing forwarded means nothing to keep owed, and no underflow"
    );
}

/// With nothing forwarded, no event can predate anything and the state stays put.
#[test]
fn no_forward_means_no_event_predates_it() {
    let (mut unconfirmed, mut queued) = (false, 0usize);
    for _ in 0..3 {
        assert!(!agent_event_may_predate_forward(
            &mut unconfirmed,
            &mut queued
        ));
    }
}

/// The common shape: a forward lands with the channel drained, the agent then works
/// for a while. The first event still counts as possibly-predating (an event never
/// vouches for itself), every one after it is proof the agent took the input.
#[test]
fn a_forward_into_a_drained_channel_is_confirmed_by_the_second_event() {
    let (mut unconfirmed, mut queued) = (true, 0usize);
    assert!(
        agent_event_may_predate_forward(&mut unconfirmed, &mut queued),
        "the first event may itself be the one that predates the forward"
    );
    assert!(!agent_event_may_predate_forward(
        &mut unconfirmed,
        &mut queued
    ));
    assert!(!agent_event_may_predate_forward(
        &mut unconfirmed,
        &mut queued
    ));
}

/// **The buffered-event hole**, flagged by the Codex reviewer on this very change.
/// The run loop routinely leaves several events queued while it awaits an emit, so
/// a forward can be followed by an older non-Result event and then the older
/// `Result`. Letting that first buffered event confirm the forward would settle the
/// new input to zero and cancel the subprocess before the agent ever ran it.
#[test]
fn events_buffered_before_the_forward_cannot_confirm_it() {
    let (mut unconfirmed, mut queued) = (true, 2usize);
    for stale in 0..2 {
        assert!(
            agent_event_may_predate_forward(&mut unconfirmed, &mut queued),
            "event {stale} was already in the channel at the forward and proves nothing"
        );
    }
    assert!(
        agent_event_may_predate_forward(&mut unconfirmed, &mut queued),
        "the first genuinely-later event still does not vouch for itself"
    );
    assert!(
        !agent_event_may_predate_forward(&mut unconfirmed, &mut queued),
        "only now is the agent proven to have taken the input"
    );
}

/// Codex runs one child per accepted input and emits one Result each, so a Result
/// answers exactly one, whatever the ordering flag says. Saturating because a
/// warm-up turn seeds 0 and still produces a Result.
#[test]
fn settle_codex_decrements_one_and_saturates() {
    for may_predate in [false, true] {
        assert_eq!(
            settle_inputs_awaiting_result(crate::runtime::CodingAgent::Codex, 3, may_predate),
            2
        );
        assert_eq!(
            settle_inputs_awaiting_result(crate::runtime::CodingAgent::Codex, 1, may_predate),
            0
        );
        assert_eq!(
            settle_inputs_awaiting_result(crate::runtime::CodingAgent::Codex, 0, may_predate),
            0,
            "a warm-up Codex turn owes nothing and must not underflow"
        );
    }
}

#[test]
fn reset_per_turn_flags_clears_all_flags() {
    let mut is_waiting = true;
    let mut last_emitted_idle = true;
    let mut emitted_terminal_event = true;
    let mut user_hit_stop = true;
    let mut interrupt_is_redirect = true;
    let mut last_terminal_kind = Some(TerminalKind::Generated);
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
        &mut last_terminal_kind,
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
        "None terminal (silent resume / safety-net abort) must NOT auto-propose"
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
