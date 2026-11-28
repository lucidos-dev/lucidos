use super::*;
use crate::runtime::is_user_question_tool;

#[test]
fn question_tools_are_suppressed_other_tools_are_not() {
    assert!(is_user_question_tool(
        crate::runtime::CC_NATIVE_ASK_USER_QUESTION_TOOL
    ));
    assert!(is_user_question_tool(
        crate::runtime::CC_MCP_ASK_USER_QUESTION_TOOL
    ));
    assert!(is_user_question_tool(
        crate::runtime::CODEX_ASK_USER_QUESTION_TOOL
    ));
    for name in [
        "Bash",
        "Edit",
        "command_execution",
        "file_change",
        "mcp__lucidos__approve",
        crate::runtime::CC_PERMISSION_PROMPT_TOOL,
        "mcp__other__ask_user_question",
    ] {
        assert!(
            !is_user_question_tool(name),
            "{name} must emit CodingAgentToolCalled"
        );
    }
}

#[test]
fn generated_result_always_emits_idle() {
    let (terminal, emit_idle) = classify_result(false, None, false, None, false);
    assert_eq!(terminal, TerminalKind::Generated);
    assert!(
        emit_idle,
        "Generated Result must emit CodingAgentIdled so the thread reaches \
         Current — not Archive — after CC produces a Result"
    );
}

#[test]
fn cc_error_classifies_as_failed() {
    let err = "Stream interrupted: connection reset".to_string();
    let (terminal, emit_idle) = classify_result(false, None, false, Some(err.clone()), false);
    assert_eq!(terminal, TerminalKind::Failed { error: err });
    assert!(
        emit_idle,
        "Failed Result is still a turn boundary — must emit CodingAgentIdled \
         so the next follow-up resumes via --resume"
    );
}

#[test]
fn shutdown_wins_over_cc_error() {
    let (terminal, emit_idle) =
        classify_result(false, None, true, Some("api timeout".to_string()), false);
    assert_eq!(
        terminal,
        TerminalKind::Aborted(crate::engine::thread_events::AbortCause::EngineShutdown)
    );
    assert!(
        !emit_idle,
        "shutdown must skip CodingAgentIdled regardless of cc_error"
    );
}

#[test]
fn user_hit_stop_wins_over_cc_error() {
    use crate::engine::thread_events::CancelCause;
    let err = "[ede_diagnostic] result_type=user stop_reason=tool_use".to_string();
    let (terminal, emit_idle) = classify_result(true, None, false, Some(err), false);
    assert_eq!(
        terminal,
        TerminalKind::Canceled(CancelCause::UserStop),
        "an interrupted turn (cc_error set by the cancel) must be Canceled, not Failed"
    );
    assert!(
        emit_idle,
        "cancel is a turn boundary: CodingAgentIdled must follow so the session stays resumable"
    );
}

#[test]
fn empty_text_classifies_as_failed_with_empty_response_error() {
    let (terminal, emit_idle) = classify_result(false, None, false, None, true);
    assert_eq!(
        terminal,
        TerminalKind::Failed {
            error: EMPTY_RESPONSE_ERROR.to_string(),
        }
    );
    assert!(
        emit_idle,
        "empty-text Failed is still a turn boundary — must emit \
         CodingAgentIdled so the dispatcher closes the turn"
    );
}

#[test]
fn cc_error_wins_over_empty_text() {
    let err = "rate_limit_error".to_string();
    let (terminal, _) = classify_result(false, None, false, Some(err.clone()), true);
    assert_eq!(terminal, TerminalKind::Failed { error: err });
}

#[test]
fn user_hit_stop_wins_over_empty_text() {
    use crate::engine::thread_events::CancelCause;
    let (terminal, _) = classify_result(true, None, false, None, true);
    assert_eq!(terminal, TerminalKind::Canceled(CancelCause::UserStop));
}

#[test]
fn redirect_followup_classifies_as_superseded_by_followup() {
    use crate::engine::thread_events::CancelCause;
    let (terminal, emit_idle) = classify_result(
        true,
        Some(CancelCause::SupersededByFollowup),
        false,
        None,
        false,
    );
    assert_eq!(
        terminal,
        TerminalKind::Canceled(CancelCause::SupersededByFollowup),
        "a follow-up redirect interrupt must carry the SupersededByFollowup cause"
    );
    assert!(
        emit_idle,
        "redirect cancel is still a turn boundary: CodingAgentIdled must follow"
    );
    let (with_err, _) = classify_result(
        true,
        Some(CancelCause::SupersededByFollowup),
        false,
        Some("[ede_diagnostic] result_type=user stop_reason=tool_use".to_string()),
        true,
    );
    assert_eq!(
        with_err,
        TerminalKind::Canceled(CancelCause::SupersededByFollowup),
    );
    let (clean, _) = classify_result(
        false,
        Some(CancelCause::SupersededByFollowup),
        false,
        None,
        false,
    );
    assert_eq!(clean, TerminalKind::Generated);
    let (shutdown, _) = classify_result(
        true,
        Some(CancelCause::SupersededByFollowup),
        true,
        None,
        false,
    );
    assert_eq!(
        shutdown,
        TerminalKind::Aborted(crate::engine::thread_events::AbortCause::EngineShutdown),
    );
}

#[test]
fn shutdown_wins_over_empty_text() {
    use crate::engine::thread_events::AbortCause;
    let (terminal, emit_idle) = classify_result(false, None, true, None, true);
    assert_eq!(terminal, TerminalKind::Aborted(AbortCause::EngineShutdown));
    assert!(
        !emit_idle,
        "shutdown must skip CodingAgentIdled even when text is empty"
    );
}

#[test]
fn an_unrequested_interrupt_fails_unless_the_user_stopped() {
    use crate::engine::thread_events::CancelCause;
    let err = || Some(crate::runtime::codex::UNREQUESTED_INTERRUPT_ERROR.to_string());
    let (stopped, _) = classify_result(true, None, false, err(), false);
    assert_eq!(stopped, TerminalKind::Canceled(CancelCause::UserStop));
    let (redirected, _) = classify_result(
        true,
        Some(CancelCause::SupersededByFollowup),
        false,
        err(),
        false,
    );
    assert_eq!(
        redirected,
        TerminalKind::Canceled(CancelCause::SupersededByFollowup)
    );
    let (unrequested, emit_idle) = classify_result(false, None, false, err(), false);
    assert_eq!(
        unrequested,
        TerminalKind::Failed {
            error: err().unwrap()
        }
    );
    assert!(emit_idle);
}

#[test]
fn empty_text_failed_does_not_propose() {
    let (terminal, _) = classify_result(false, None, false, None, true);
    let write = idle_change_write(IdleSession::default(), &Some(terminal));
    assert_eq!(
        write,
        Some(IdleChangeWrite::Unfinished),
        "empty-text Failed (OOM / SIGTERM) is withheld, never proposed"
    );
}

fn stale_baseline() -> StaleResumeInputs {
    StaleResumeInputs {
        has_resume_session: true,
        resume_attach_confirmed: false,
        result_text_empty: true,
        buffered_text_empty: true,
        no_prior_results_this_turn: true,
        no_tool_calls_this_turn: true,
        cc_error: false,
    }
}

#[test]
fn empty_result_on_resume_with_no_error_is_stale_resume() {
    assert!(is_stale_resume_signal(stale_baseline()));
}

#[test]
fn empty_result_on_resume_with_cc_error_is_not_stale_resume() {
    assert!(!is_stale_resume_signal(StaleResumeInputs {
        cc_error: true,
        ..stale_baseline()
    }));
}

#[test]
fn empty_result_on_resume_but_made_tool_calls_is_not_stale_resume() {
    assert!(!is_stale_resume_signal(StaleResumeInputs {
        no_tool_calls_this_turn: false,
        ..stale_baseline()
    }));
}

#[test]
fn confirmed_attach_is_never_stale_resume() {
    assert!(!is_stale_resume_signal(StaleResumeInputs {
        resume_attach_confirmed: true,
        ..stale_baseline()
    }));
}

#[test]
fn unconfirmed_attach_still_falls_back_to_the_empty_echo_heuristic() {
    assert!(is_stale_resume_signal(StaleResumeInputs {
        resume_attach_confirmed: false,
        ..stale_baseline()
    }));
}

#[test]
fn fresh_session_is_never_stale_resume() {
    assert!(!is_stale_resume_signal(StaleResumeInputs {
        has_resume_session: false,
        ..stale_baseline()
    }));
}

fn settle_baseline() -> StaleResumeInputs {
    StaleResumeInputs {
        resume_attach_confirmed: true,
        ..stale_baseline()
    }
}

#[test]
fn confirmed_attach_with_no_activity_is_a_resume_settle_result() {
    assert!(is_resume_settle_result(settle_baseline(), true));
}

#[test]
fn an_empty_answer_that_cost_an_api_call_is_not_a_settle_turn() {
    assert!(!is_resume_settle_result(settle_baseline(), false));
}

#[test]
fn stale_resume_and_resume_settle_partition_the_empty_resumed_turn() {
    for confirmed in [true, false] {
        let i = StaleResumeInputs {
            resume_attach_confirmed: confirmed,
            ..stale_baseline()
        };
        assert_ne!(
            is_stale_resume_signal(i),
            is_resume_settle_result(i, true),
            "attach_confirmed={confirmed} must land in exactly one of the two"
        );
    }
}

#[test]
fn resume_settle_needs_zero_tool_calls() {
    assert!(!is_resume_settle_result(
        StaleResumeInputs {
            no_tool_calls_this_turn: false,
            ..settle_baseline()
        },
        true
    ));
}

#[test]
fn resume_settle_never_swallows_a_reported_error() {
    assert!(!is_resume_settle_result(
        StaleResumeInputs {
            cc_error: true,
            ..settle_baseline()
        },
        true
    ));
}

#[test]
fn only_the_first_result_of_a_session_can_be_a_settle_turn() {
    assert!(!is_resume_settle_result(
        StaleResumeInputs {
            no_prior_results_this_turn: false,
            ..settle_baseline()
        },
        true
    ));
}

#[test]
fn resume_settle_needs_both_text_channels_empty() {
    assert!(!is_resume_settle_result(
        StaleResumeInputs {
            result_text_empty: false,
            ..settle_baseline()
        },
        true
    ));
    assert!(!is_resume_settle_result(
        StaleResumeInputs {
            buffered_text_empty: false,
            ..settle_baseline()
        },
        true
    ));
}

#[test]
fn fresh_session_has_no_settle_turn_to_skip() {
    assert!(!is_resume_settle_result(
        StaleResumeInputs {
            has_resume_session: false,
            ..settle_baseline()
        },
        true
    ));
}

#[test]
fn oom_killed_turn_still_fails_rather_than_being_skipped() {
    let oom_shaped = StaleResumeInputs {
        no_tool_calls_this_turn: false,
        ..settle_baseline()
    };
    assert!(!is_resume_settle_result(oom_shaped, true));
    assert_eq!(
        classify_result(false, None, false, None, true),
        (
            TerminalKind::Failed {
                error: EMPTY_RESPONSE_ERROR.to_string()
            },
            true
        )
    );
}

#[test]
fn explicit_no_conversation_found_is_definitive() {
    assert!(is_definitive_session_not_found(Some(
        "No conversation found with session ID: 7c61f11b-414e-4e5e-9da7-bdbd3b3649d6"
    )));
}

#[test]
fn generic_cc_error_is_not_session_not_found() {
    assert!(!is_definitive_session_not_found(Some(
        "API Error: 529 overloaded"
    )));
    assert!(!is_definitive_session_not_found(Some("error_max_turns")));
    assert!(!is_definitive_session_not_found(Some("")));
}

#[test]
fn no_error_is_not_session_not_found() {
    assert!(!is_definitive_session_not_found(None));
}

#[test]
fn classify_result_table() {
    use crate::engine::thread_events::{AbortCause, CancelCause};
    let cases = [
        ((false, false), (TerminalKind::Generated, true)),
        (
            (true, false),
            (TerminalKind::Canceled(CancelCause::UserStop), true),
        ),
        (
            (false, true),
            (TerminalKind::Aborted(AbortCause::EngineShutdown), false),
        ),
        (
            (true, true),
            (TerminalKind::Aborted(AbortCause::EngineShutdown), false),
        ),
    ];
    for ((stop, shutdown), expected) in cases {
        assert_eq!(
            classify_result(stop, None, shutdown, None, false),
            expected,
            "(user_hit_stop={}, is_shutdown={})",
            stop,
            shutdown,
        );
    }
}

#[test]
fn real_cancel_on_working_cc_emits_canceled() {
    use crate::engine::thread_events::CancelCause;
    assert_eq!(
        stop_terminal_kind(false, false, false),
        Some(TerminalKind::Canceled(CancelCause::UserStop)),
        "real Cancel click on actively-working CC must emit Canceled"
    );
}

#[test]
fn cancel_racing_idle_emits_no_terminal_event() {
    assert_eq!(
        stop_terminal_kind(false, true, false),
        None,
        "Cancel that raced after CC went idle must NOT emit Canceled — \
         previous turn already finished cleanly"
    );
}

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
        "Apply/Discard/Archive on idle CC must NOT emit Canceled either, \
         nothing in flight to cancel and the lifecycle event is the terminator"
    );
}

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

#[test]
fn should_auto_commit_on_cleanup_table() {
    use crate::engine::thread_events::{AbortCause, CancelCause};
    let cases: &[(bool, &Option<TerminalKind>, bool, &str)] = &[
        (
            true,
            &Some(TerminalKind::Generated),
            false,
            "discard wins over Generated",
        ),
        (true, &None, false, "discard wins over safety-net abort"),
        (
            false,
            &Some(TerminalKind::Generated),
            true,
            "clean Generated commits",
        ),
        (
            false,
            &Some(TerminalKind::Failed {
                error: "stream interrupted".into(),
            }),
            false,
            "Failed must NOT auto-commit — half-assed work",
        ),
        (
            false,
            &Some(TerminalKind::Canceled(CancelCause::UserStop)),
            false,
            "Canceled must NOT auto-commit — user stopped mid-work",
        ),
        (
            false,
            &Some(TerminalKind::Aborted(AbortCause::EngineShutdown)),
            false,
            "Aborted (shutdown) must NOT auto-commit — mid-work",
        ),
        (
            false,
            &None,
            false,
            "safety-net abort (None terminal) must NOT auto-commit",
        ),
    ];
    for (should_discard, terminal, expected, label) in cases {
        assert_eq!(
            should_auto_commit_on_cleanup(*should_discard, terminal),
            *expected,
            "{label}",
        );
    }
}

#[test]
fn only_a_shutdown_result_leaves_the_worktree_uncommitted() {
    use crate::engine::thread_events::CancelCause;
    let shutdown = |is_shutdown| classify_result(false, None, is_shutdown, None, false).0;
    assert!(!result_auto_commits(&shutdown(true)));
    assert!(result_auto_commits(&shutdown(false)));
    assert!(result_auto_commits(&TerminalKind::Canceled(
        CancelCause::UserStop
    )));
    assert!(result_auto_commits(&TerminalKind::Failed {
        error: "stream interrupted".into(),
    }));
}

#[test]
fn conflict_resolution_cleanup_only_applies_clean_generated_turns() {
    use crate::engine::thread_events::{AbortCause, CancelCause};

    assert_eq!(
        conflict_resolution_cleanup_action(false, &Some(TerminalKind::Generated), false),
        ConflictResolutionCleanupAction::Apply,
        "a clean generated merge-fix turn is the only path that may land the apply"
    );
    assert_eq!(
        conflict_resolution_cleanup_action(true, &Some(TerminalKind::Generated), false),
        ConflictResolutionCleanupAction::Abort {
            message: "Conflict resolution incomplete — merge aborted. The change is still pending; try applying again.",
        },
        "unmerged paths must keep the original change pending even after a generated turn"
    );
    assert_eq!(
        conflict_resolution_cleanup_action(
            false,
            &Some(TerminalKind::Canceled(CancelCause::UserStop)),
            false
        ),
        ConflictResolutionCleanupAction::Abort {
            message: "Conflict resolution canceled — merge aborted. The change is still pending; try applying again.",
        },
        "canceling the merge-fix session must not allow cleanup to fast-forward main"
    );
    for terminal in [
        Some(TerminalKind::Failed {
            error: "api dropped".to_string(),
        }),
        Some(TerminalKind::Aborted(AbortCause::EngineShutdown)),
        None,
    ] {
        assert_eq!(
            conflict_resolution_cleanup_action(false, &terminal, false),
            ConflictResolutionCleanupAction::Abort {
                message: "Conflict resolution did not finish cleanly — merge aborted. The change is still pending; try applying again.",
            },
            "non-generated terminal {terminal:?} must keep the original change pending"
        );
    }
}

#[test]
fn a_lane_thread_run_completes_fails_or_hands_off_like_a_merge_fix() {
    use crate::engine::landing_lane::LaneThreadEnd;

    assert_eq!(
        lane_thread_end(false, &Some(TerminalKind::Generated), false, None),
        Some(LaneThreadEnd::Completed)
    );
    assert_eq!(
        lane_thread_end(true, &Some(TerminalKind::Generated), false, None),
        Some(LaneThreadEnd::Failed(
            "The lane thread ended without finishing its merges.".into()
        ))
    );
    assert_eq!(
        lane_thread_end(
            false,
            &Some(TerminalKind::Failed {
                error: "boom".into()
            }),
            false,
            None
        ),
        Some(LaneThreadEnd::Failed(
            "The lane thread ended without finishing its merges.".into()
        ))
    );
    assert_eq!(
        lane_thread_end(true, &None, true, None),
        None,
        "a continuation carries the batch on"
    );
    assert_eq!(
        lane_thread_end(false, &Some(TerminalKind::Generated), true, None),
        Some(LaneThreadEnd::Completed),
        "a clean finish beats a racing continuation"
    );
}

#[test]
fn a_stopped_lane_thread_keeps_its_batch_open() {
    use crate::engine::landing_lane::LaneThreadEnd;
    use crate::engine::thread_events::CancelCause;
    use crate::engine::StopReason;

    for cause in [CancelCause::UserStop, CancelCause::SupersededByFollowup] {
        assert_eq!(
            lane_thread_end(true, &Some(TerminalKind::Canceled(cause)), false, None),
            Some(LaneThreadEnd::Stopped),
            "a canceled turn leaves the batch open: {cause:?}"
        );
    }
    assert_eq!(
        lane_thread_end(
            false,
            &Some(TerminalKind::Generated),
            false,
            Some(StopReason::UserStop)
        ),
        Some(LaneThreadEnd::Stopped),
        "a Stop the run loop took beats the previous turn's clean end"
    );
    for stop in [StopReason::Archive, StopReason::Discard] {
        assert!(
            matches!(
                lane_thread_end(false, &None, false, Some(stop)),
                Some(LaneThreadEnd::Failed(_))
            ),
            "{stop:?} fails the batch"
        );
    }
    assert!(matches!(
        lane_thread_end(
            false,
            &Some(TerminalKind::Canceled(CancelCause::UserAction)),
            false,
            None
        ),
        Some(LaneThreadEnd::Failed(_))
    ));
}

#[test]
fn conflict_resolution_hands_off_when_continuation_pending() {
    use crate::engine::thread_events::{AbortCause, CancelCause};

    for (has_unmerged, terminal) in [
        (true, None),
        (false, None),
        (
            true,
            Some(TerminalKind::Failed {
                error: "killed".to_string(),
            }),
        ),
        (false, Some(TerminalKind::Aborted(AbortCause::SafetyNet))),
        (true, Some(TerminalKind::Generated)),
    ] {
        assert_eq!(
            conflict_resolution_cleanup_action(has_unmerged, &terminal, true),
            ConflictResolutionCleanupAction::HandOff,
            "continuation_pending must hand off (has_unmerged={has_unmerged}, terminal {terminal:?})"
        );
    }

    assert_eq!(
        conflict_resolution_cleanup_action(false, &Some(TerminalKind::Generated), true),
        ConflictResolutionCleanupAction::Apply,
        "a committed merge with a clean Generated end applies immediately — \
         never deferred to the continuation (watchdog false-positive race)"
    );
    assert_eq!(
        conflict_resolution_cleanup_action(
            false,
            &Some(TerminalKind::Canceled(CancelCause::UserStop)),
            true
        ),
        ConflictResolutionCleanupAction::Abort {
            message: "Conflict resolution canceled — merge aborted. The change is still pending; try applying again.",
        },
        "a user Stop aborts even with a continuation pending"
    );
}

#[test]
fn conflict_abort_deletes_only_temp_merge_state() {
    assert!(conflict_abort_deletes_temp_state(
        Some("merge-tmp/x"),
        "merge-tmp/x"
    ));
    assert!(!conflict_abort_deletes_temp_state(
        Some("merge-tmp/x"),
        "claude-code/20260707-abc"
    ));
    assert!(!conflict_abort_deletes_temp_state(
        None,
        "claude-code/20260707-abc"
    ));
}

#[test]
fn a_spawn_needs_text_or_an_image() {
    let image = crate::engine::types::ChatImage {
        base64: "aGk=".into(),
        mime_type: "image/png".into(),
    };
    assert!(require_agent_input("fix the test", None).is_ok());
    assert!(require_agent_input("", Some(std::slice::from_ref(&image))).is_ok());
    for (text, images) in [("", None), (" \n", None), ("", Some(&[][..]))] {
        assert_eq!(
            require_agent_input(text, images),
            Err(EMPTY_INPUT_ERROR),
            "{text:?} with {images:?} images must be refused"
        );
    }
}

#[test]
fn run_direct_agent_refuses_empty_input_before_anything_else() {
    let source = include_str!("../run_session/run.rs");
    let signature = source
        .find("async fn run_direct_agent(")
        .expect("run_direct_agent is defined in run_session/run.rs");
    let body_start = signature
        + source[signature..]
            .find("> {\n")
            .expect("run_direct_agent has a body")
        + "> {\n".len();
    let first_statement = source[body_start..].trim_start();
    assert!(
        first_statement.starts_with("require_agent_input(user_message, user_images)?;"),
        "run_direct_agent must refuse empty input first, found: {}",
        first_statement.lines().next().unwrap_or_default()
    );
}

#[test]
fn classify_session_end_action_table() {
    use SessionEndAction::*;
    use TurnCancel::{Failed, None as NotCanceled, Redirected, Stopped};
    let cases = [
        ((true, false, false, false, NotCanceled), Propose),
        ((true, true, false, false, NotCanceled), KeepEmptyBranch),
        ((true, false, true, false, NotCanceled), KeepExternalBranch),
        ((true, true, true, false, NotCanceled), KeepExternalBranch),
        ((false, false, false, false, NotCanceled), KeepEmptyBranch),
        ((false, true, false, false, NotCanceled), KeepEmptyBranch),
        ((false, false, true, false, NotCanceled), KeepEmptyBranch),
        ((false, true, true, false, NotCanceled), KeepEmptyBranch),
        ((true, false, false, true, NotCanceled), CrashedKeepBranch),
        ((true, true, false, true, NotCanceled), CrashedKeepBranch),
        ((true, false, true, true, NotCanceled), KeepExternalBranch),
        ((true, true, true, true, NotCanceled), KeepExternalBranch),
        ((false, false, false, true, NotCanceled), KeepEmptyBranch),
        ((false, true, false, true, NotCanceled), KeepEmptyBranch),
        ((false, false, true, true, NotCanceled), KeepEmptyBranch),
        ((false, true, true, true, NotCanceled), KeepEmptyBranch),
        ((false, true, false, false, Stopped), KeepCanceledBranch),
        ((false, false, false, false, Stopped), KeepCanceledBranch),
        ((true, false, false, false, Stopped), WithholdUnfinished),
        ((true, true, false, false, Stopped), KeepCanceledBranch),
        ((true, false, false, false, Failed), WithholdUnfinished),
        ((true, true, false, false, Failed), KeepEmptyBranch),
        ((false, true, false, false, Failed), KeepEmptyBranch),
        ((true, false, true, false, Failed), KeepExternalBranch),
        ((true, false, false, true, Failed), CrashedKeepBranch),
        ((true, false, false, false, Redirected), KeepCanceledBranch),
        ((false, true, false, false, Redirected), KeepCanceledBranch),
        ((true, false, true, false, Stopped), KeepExternalBranch),
        ((true, false, false, true, Stopped), CrashedKeepBranch),
        ((true, false, true, false, Redirected), KeepExternalBranch),
    ];
    for ((has_commits, files_empty, is_external, safety_net_fired, cancel), expected) in cases {
        assert_eq!(
            classify_session_end_action(
                has_commits,
                files_empty,
                is_external,
                safety_net_fired,
                cancel,
                false,
            ),
            expected,
            "(has_commits={has_commits}, files_empty={files_empty}, is_external={is_external}, safety_net_fired={safety_net_fired}, cancel={cancel:?})",
        );
        let handed_off = match expected {
            KeepExternalBranch | CrashedKeepBranch => expected,
            _ if has_commits && !files_empty => CarriedBySuccessor,
            other => other,
        };
        assert_eq!(
            classify_session_end_action(
                has_commits,
                files_empty,
                is_external,
                safety_net_fired,
                cancel,
                true,
            ),
            handed_off,
            "handed off: (has_commits={has_commits}, files_empty={files_empty}, is_external={is_external}, safety_net_fired={safety_net_fired}, cancel={cancel:?})",
        );
    }
}

#[test]
fn terminal_clears_user_hit_stop_for_cancel_and_abort_only() {
    use crate::engine::thread_events::{AbortCause, CancelCause};
    assert!(terminal_clears_user_hit_stop(&TerminalKind::Canceled(
        CancelCause::UserStop
    )));
    assert!(terminal_clears_user_hit_stop(&TerminalKind::Canceled(
        CancelCause::UserAction
    )));
    assert!(terminal_clears_user_hit_stop(&TerminalKind::Aborted(
        AbortCause::EngineShutdown
    )));
    assert!(!terminal_clears_user_hit_stop(&TerminalKind::Generated));
    assert!(!terminal_clears_user_hit_stop(&TerminalKind::Failed {
        error: "x".to_string()
    }));
}

#[test]
fn inflight_followup_completion_after_cancel_is_generated_not_double_cancel() {
    use crate::engine::thread_events::CancelCause;

    let mut user_hit_stop = true;
    let (first, _) = classify_result(
        user_hit_stop,
        None,
        false,
        Some("[ede_diagnostic] result_type=user stop_reason=tool_use".to_string()),
        true,
    );
    assert_eq!(
        first,
        TerminalKind::Canceled(CancelCause::UserStop),
        "the interrupt must classify as the first Canceled"
    );

    if terminal_clears_user_hit_stop(&first) {
        user_hit_stop = false;
    }
    assert!(
        !user_hit_stop,
        "the user-stop latch must clear after the Canceled terminal so the \
         next Result classifies fresh"
    );

    let (second, _) = classify_result(user_hit_stop, None, false, None, false);
    assert_eq!(
        second,
        TerminalKind::Generated,
        "a successful completion after an interrupt superseded by inflight \
         follow-ups must be ResponseGenerated, not a second ResponseCanceled"
    );
}

#[test]
fn only_a_clean_turn_end_arms_the_background_wait() {
    use crate::engine::thread_events::{AbortCause, CancelCause};

    let generated = Some(TerminalKind::Generated);
    assert!(turn_ended_cleanly(&generated, false, false));
    assert!(
        !turn_ended_cleanly(&generated, true, false),
        "merge resolution"
    );
    assert!(
        !turn_ended_cleanly(&generated, false, true),
        "a keep-alive or a ceiling re-entry"
    );

    let stopped = classify_result(true, None, false, None, false).0;
    let redirected = classify_result(
        true,
        Some(CancelCause::SupersededByFollowup),
        false,
        None,
        false,
    )
    .0;
    let shut_down = classify_result(false, None, true, None, false).0;
    let failed = classify_result(false, None, false, Some("stream interrupted".into()), false).0;
    let unfinished = [stopped, redirected, shut_down, failed]
        .into_iter()
        .chain(CancelCause::ALL.map(TerminalKind::Canceled))
        .chain(
            [
                AbortCause::EngineShutdown,
                AbortCause::SafetyNet,
                AbortCause::RecoveryAfterRestart,
                AbortCause::ProcessKilled,
                AbortCause::StaleSettle,
                AbortCause::SessionDropped,
                AbortCause::SnapshotRestored,
                AbortCause::Unknown,
            ]
            .map(TerminalKind::Aborted),
        );
    for terminal in unfinished {
        assert!(
            !turn_ended_cleanly(&Some(terminal.clone()), false, false),
            "{terminal:?} must arm nothing"
        );
    }
    assert!(!turn_ended_cleanly(&None, false, false));
}

#[test]
fn a_model_swap_reentry_waits_for_a_clean_idle_with_nothing_else_due() {
    let ready = ModelSwapIdle {
        pending: true,
        other_reentry: false,
        ended_cleanly: true,
        idle_complete: true,
        emit_idle: true,
        is_shutdown: false,
        input_owed: false,
    };
    assert!(model_swap_reentry_due(ready));
    let held = [
        (
            "nothing pending",
            ModelSwapIdle {
                pending: false,
                ..ready
            },
        ),
        (
            "another re-entry",
            ModelSwapIdle {
                other_reentry: true,
                ..ready
            },
        ),
        (
            "a Stop or failure",
            ModelSwapIdle {
                ended_cleanly: false,
                ..ready
            },
        ),
        (
            "a pending handoff or live event wait",
            ModelSwapIdle {
                idle_complete: false,
                ..ready
            },
        ),
        (
            "no idle written",
            ModelSwapIdle {
                emit_idle: false,
                ..ready
            },
        ),
        (
            "a shutdown",
            ModelSwapIdle {
                is_shutdown: true,
                ..ready
            },
        ),
        (
            "input owed",
            ModelSwapIdle {
                input_owed: true,
                ..ready
            },
        ),
    ];
    for (why, idle) in held {
        assert!(!model_swap_reentry_due(idle), "{why} must hold the swap");
    }
}

#[test]
fn context_ceiling_turn_ends_settle_debt_by_kind() {
    use crate::engine::thread_events::{AbortCause, CancelCause};
    assert_eq!(
        ceiling_turn_end(Some(&TerminalKind::Generated), false),
        CeilingTurnEnd::Reenter
    );
    assert_eq!(
        ceiling_turn_end(
            Some(&TerminalKind::Canceled(CancelCause::SupersededByFollowup)),
            false
        ),
        CeilingTurnEnd::Reenter
    );
    assert_eq!(
        ceiling_turn_end(Some(&TerminalKind::Canceled(CancelCause::UserStop)), false),
        CeilingTurnEnd::Brief
    );
    assert_eq!(
        ceiling_turn_end(
            Some(&TerminalKind::Failed {
                error: "failure".into()
            }),
            false
        ),
        CeilingTurnEnd::Brief
    );
    assert_eq!(
        ceiling_turn_end(
            Some(&TerminalKind::Aborted(AbortCause::EngineShutdown)),
            false
        ),
        CeilingTurnEnd::Wait
    );
    assert_eq!(
        ceiling_turn_end(Some(&TerminalKind::Generated), true),
        CeilingTurnEnd::Wait
    );
    assert_eq!(
        ceiling_turn_end(
            Some(&TerminalKind::Canceled(CancelCause::UserAction)),
            false
        ),
        CeilingTurnEnd::Wait
    );
    assert_eq!(ceiling_turn_end(None, false), CeilingTurnEnd::Wait);
}
