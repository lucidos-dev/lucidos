use super::*;

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
    assert_eq!(idle_action(true, true), IdleAction::EndSession);
}

#[test]
fn normal_idle_outside_shutdown_exits_subprocess() {
    assert_eq!(
        idle_action(false, false),
        IdleAction::ExitSubprocess,
        "non-shutdown idle must exit so the next turn re-spawns via --resume"
    );
}

#[test]
fn normal_idle_during_shutdown_does_nothing() {
    assert_eq!(
        idle_action(false, true),
        IdleAction::Nothing,
        "shutdown idle must NOT touch CC — preserves worktree for recovery"
    );
}

#[test]
fn terminate_decision_default_terminates() {
    assert_eq!(
        terminate_decision(0, 0, false, false),
        TerminateDecision::Terminate,
        "nothing queued, nothing owed, no reservation: terminate is the default"
    );
}

#[test]
fn terminate_decision_handed_off_leaves_queued_follow_ups_to_the_successor() {
    assert_eq!(
        terminate_decision(2, 0, true, true),
        TerminateDecision::Terminate,
        "a handed-off session must not take another turn for a queued follow-up"
    );
}

#[test]
fn a_handed_off_session_forwards_no_more_input() {
    const RUN_SRC: &str = include_str!("../run_session/run.rs");
    let arm = RUN_SRC
        .find("msg_rx.recv(), if input_gate.receives() &&")
        .expect("the input arm must be off once the session handed off");
    let body = &RUN_SRC[arm..];
    let forward = body
        .find("agent_input_tx.send(agent_input)")
        .expect("the arm forwards");
    let check = body
        .find("pending_handoff::handoff_pending(")
        .expect("the arm checks for a handoff");
    assert!(
        check < forward,
        "the arm must check for a handoff before it forwards"
    );
}

#[test]
fn every_idle_exit_refreshes_the_handoff_flag() {
    const RUN_SRC: &str = include_str!("../run_session/run.rs");
    let exit = RUN_SRC
        .find("async fn exit_idle_subprocess_unless_owed(")
        .expect("the idle exit exists");
    let refresh = RUN_SRC[exit..]
        .find("input_gate.handed_off =")
        .expect("the idle exit must write its handoff read back to the run loop");
    assert!(
        RUN_SRC[exit + refresh..]
            .trim_start_matches("input_gate.handed_off =")
            .trim_start()
            .starts_with("crate::engine::pending_handoff::handoff_pending("),
        "the flag it writes is the handoff read"
    );
    for call in [
        "self.exit_idle_subprocess_unless_owed(",
        "self.idle_exit_ends_session(",
    ] {
        for (at, _) in RUN_SRC.match_indices(call) {
            let (invocation, _) = RUN_SRC[at..]
                .split_once(".await")
                .expect("the idle exit is awaited");
            assert!(
                invocation.contains("&mut input_gate"),
                "an idle exit must refresh the loop's flag: {invocation}"
            );
        }
    }
}

#[test]
fn a_provider_block_closes_the_input_gate() {
    const RUN_SRC: &str = include_str!("../run_session/run.rs");
    assert!(RUN_SRC.contains("!self.handed_off && !self.provider_blocked"));
    let result_arm = RUN_SRC
        .find("AgentEvent::Result { text, error: cc_error, provider_error, turn_end, .. } => {")
        .expect("the run loop handles a Result");
    let set = RUN_SRC[result_arm..]
        .find("input_gate.provider_blocked = true;")
        .expect("a policy block closes the gate");
    assert!(RUN_SRC[result_arm..result_arm + set].contains(".is_policy_block()"));
    let exit = RUN_SRC
        .find("async fn exit_idle_subprocess_unless_owed(")
        .expect("the idle exit exists");
    assert!(RUN_SRC[exit..].contains("let leaves_queue_to_next_session = !input_gate.forwards();"));
}

#[test]
fn terminate_decision_handed_off_still_waits_for_what_the_agent_was_sent() {
    assert_eq!(
        terminate_decision(1, 1, false, true),
        TerminateDecision::KeepAliveForFollowup {
            queued: 1,
            unread: 1,
            redirect_pending: false,
        },
    );
}

#[test]
fn terminate_decision_queued_followup_keeps_alive() {
    assert_eq!(
        terminate_decision(1, 0, false, false),
        TerminateDecision::KeepAliveForFollowup {
            queued: 1,
            unread: 0,
            redirect_pending: false,
        },
        "a message unread in msg_rx must not die with the subprocess"
    );
}

#[test]
fn terminate_decision_armed_redirect_keeps_alive() {
    assert_eq!(
        terminate_decision(0, 0, true, false),
        TerminateDecision::KeepAliveForFollowup {
            queued: 0,
            unread: 0,
            redirect_pending: true,
        },
        "an armed redirect's follow-up is coming even though the channel is empty"
    );
}

#[test]
fn terminate_decision_has_no_background_task_keep_alive() {
    const LIFECYCLE_SRC: &str = include_str!("../lifecycle.rs");
    assert!(
        !LIFECYCLE_SRC.contains("KeepAliveForBgBash"),
        "a background task must not keep an idle coding-agent subprocess alive"
    );
}

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
    let mut redirect_cause = Some(crate::engine::thread_events::CancelCause::SupersededByFollowup);
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
        &mut redirect_cause,
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
        redirect_cause.is_none(),
        "must clear in lockstep with user_hit_stop — else a stale redirect cause \
         could mislabel a later real Stop as a redirect"
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

#[test]
fn conflict_session_never_proposes_change_at_idle() {
    assert!(
        idle_change_write(
            IdleSession {
                is_resolution_session: true,
                ..IdleSession::default()
            },
            &Some(TerminalKind::Generated)
        )
        .is_none(),
        "conflict-resolution sessions must NOT propose a phantom change at idle"
    );
}

#[test]
fn normal_generated_session_may_touch_change_state() {
    assert!(
        idle_change_write(IdleSession::default(), &Some(TerminalKind::Generated))
            == Some(IdleChangeWrite::Complete),
        "normal Generated session must reach the propose/reconcile branch"
    );
}

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

#[test]
fn a_pending_handoff_never_proposes_at_idle() {
    use crate::engine::thread_events::CancelCause;
    for holds_live_wait in [false, true] {
        let handed_off = IdleSession {
            handoff_pending: true,
            holds_live_wait,
            ..IdleSession::default()
        };
        for terminal in [
            Some(TerminalKind::Generated),
            Some(TerminalKind::Canceled(CancelCause::UserStop)),
            None,
        ] {
            let write = idle_change_write(handed_off, &terminal);
            assert_eq!(write, Some(IdleChangeWrite::HandedOff), "{terminal:?}");
            assert!(!write.unwrap().may_create());
        }
    }
}

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

#[test]
fn only_a_finished_write_proposes() {
    assert!(IdleChangeWrite::Complete.finished());
    assert!(IdleChangeWrite::Resync.finished());
    assert!(!IdleChangeWrite::Unfinished.finished());
}

#[test]
fn no_terminal_kind_does_not_propose_at_idle() {
    assert!(
        idle_change_write(IdleSession::default(), &None).is_none(),
        "None terminal (safety-net abort) must NOT auto-propose"
    );
}

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
    assert_eq!(
        idle_change_flags(Some(&[]), (true, true)),
        (false, false),
        "an answered-empty diff must clear the state, not carry it forward"
    );
}

#[test]
fn unanswerable_probe_preserves_a_true_state() {
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

fn squashed(src: &str) -> String {
    src.split_whitespace().collect::<Vec<_>>().join(" ")
}

mod guard_tests {
    use super::*;

    #[test]
    fn the_external_terminal_flag_is_only_ever_raised() {
        let mut setters = 0;
        for (path, src) in crate::test_support::source_scan::production_sources() {
            let flat = squashed(&src);
            for (at, _) in flat.match_indices("external_terminal") {
                let rest = flat[at..]
                    .trim_start_matches(|c: char| c.is_alphanumeric() || c == '_')
                    .trim_start();
                let shown: String = flat[at..].chars().take(80).collect();
                if let Some(call) = rest.strip_prefix(".store(") {
                    assert!(
                        call.starts_with("true"),
                        "{path} clears the external terminal flag: {shown}"
                    );
                    setters += 1;
                }
                assert!(
                    !rest.starts_with(".swap(false") && !rest.starts_with(".compare_exchange"),
                    "{path} clears the external terminal flag: {shown}"
                );
            }
        }
        assert!(setters > 0, "the scan must see the watchdog's setter");
    }
}

#[test]
fn the_idle_decision_reads_its_windows_under_the_sessions_lock() {
    const RUN_SRC: &str = include_str!("../run_session/run.rs");
    const LOCK: &str = "let mut sessions = self.agent_sessions.lock().await;";
    let body = &RUN_SRC[RUN_SRC
        .find("async fn exit_idle_subprocess_unless_owed(")
        .expect("the idle exit exists")..];
    let lock = body
        .find(LOCK)
        .expect("the idle exit takes the sessions lock");
    let decision = body
        .find("match terminate_decision(")
        .expect("the idle exit decides through terminate_decision");
    assert!(lock < decision, "the lock comes before the decision");
    let between = &body[lock + LOCK.len()..decision];
    assert!(
        between.contains("std::mem::take(&mut s.redirect_followup_pending)"),
        "the armed-redirect window is read under the lock"
    );
    assert!(
        !between.contains("drop(") && !between.contains(".await"),
        "nothing may release the lock or yield between taking it and deciding"
    );
}

#[test]
fn the_result_arm_clears_the_stop_latch_from_the_classified_terminal() {
    const RUN_SRC: &str = include_str!("../run_session/run.rs");
    assert!(
        squashed(RUN_SRC).contains(
            "let clears = terminal_clears_user_hit_stop(&terminal_kind); \
             if clears { user_hit_stop = false; redirect_cause = None; }"
        ),
        "the Result arm must clear the latch when the terminal clears it"
    );
}

#[test]
fn the_result_arm_commits_only_as_result_auto_commits_allows() {
    const RUN_SRC: &str = include_str!("../run_session/run.rs");
    assert!(
        squashed(RUN_SRC).contains(
            "let commits = !keep_alive_turn && \
             last_terminal_kind.as_ref().is_some_and(result_auto_commits);"
        ),
        "the auto-commit before the idle probe must be gated by result_auto_commits"
    );
}

#[test]
fn a_conflict_hand_off_touches_no_merge_state() {
    const COMPLETION_SRC: &str = include_str!("../run_session/completion.rs");
    let start = COMPLETION_SRC
        .find("ConflictResolutionCleanupAction::HandOff => {")
        .expect("completion.rs must still handle the hand-off");
    let arm = &COMPLETION_SRC[start..];
    let arm = &arm[..arm
        .find("ConflictResolutionCleanupAction::Abort { message } =>")
        .expect("the Abort arm follows the hand-off arm")];
    assert!(arm.contains("log_legacy!"), "the slice is the hand-off arm");
    for forbidden in [
        "git_cmd(",
        "git_ran_ok(",
        "pending_apply_actors",
        "emit_",
        "remove_dir",
        "cancel(",
    ] {
        assert!(
            !arm.contains(forbidden),
            "the hand-off arm must leave everything to the continuation, found {forbidden}"
        );
    }
}

#[test]
fn the_settle_skip_is_decided_by_the_api_call_count() {
    const RUN_SRC: &str = include_str!("../run_session/run.rs");
    let flat = squashed(RUN_SRC);
    let at = flat
        .find("if is_resume_settle_result(stale_inputs, api_calls_seen == 0) {")
        .expect("the settle skip must read the API call count");
    let arm = &flat[at..];
    let arm = &arm[..arm
        .find("if api_calls_seen > 0")
        .expect("the logged fall-through follows the skip")];
    assert!(arm.contains("result_texts.push(text.clone());"));
    assert!(arm.contains("continue 'event_loop;"));
}

#[test]
fn the_ceiling_fire_re_checks_that_the_thread_is_running() {
    const RUN_SRC: &str = include_str!("../run_session/run.rs");
    let flat = squashed(RUN_SRC);
    let at = flat
        .find("WatchdogGate::FirePastCeiling(tif) => {")
        .expect("the in-loop watchdog must handle the ceiling fire");
    let arm = &flat[at..];
    let arm = arm[..arm.find("_ => false,").expect("the arm list ends")].trim_end();
    assert!(
        arm.contains("let still_running = crate::engine::thread_liveness::thread_is_running(")
            && arm.contains(".unwrap_or(false);"),
        "the ceiling fire must ask the projection, and an unreadable one must not fire"
    );
    assert!(
        arm.ends_with("still_running }"),
        "the ceiling fire must be decided by the projection answer alone"
    );
}

#[test]
fn the_safety_net_skip_emits_nothing() {
    const COMPLETION_SRC: &str = include_str!("../run_session/completion.rs");
    assert!(
        squashed(COMPLETION_SRC).contains("SafetyNetAction::Nothing | SafetyNetAction::Skip => {}"),
        "Skip must share the arm that emits nothing"
    );
}
