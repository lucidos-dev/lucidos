use super::*;

fn should_trigger_watchdog(
    is_waiting: bool,
    last_event_at_ms: i64,
    now_ms: i64,
    limit_ms: i64,
    ceiling_ms: i64,
    tools_in_flight: i32,
) -> bool {
    matches!(
        watchdog_gate(
            is_waiting,
            last_event_at_ms,
            now_ms,
            limit_ms,
            ceiling_ms,
            tools_in_flight,
        ),
        WatchdogGate::Fire,
    )
}

#[test]
fn watchdog_never_fires_when_idle() {
    let limit = WATCHDOG_INACTIVITY_LIMIT_MS;
    let now = 10_000_000;
    let last = now - limit - 60_000;
    assert!(
        !should_trigger_watchdog(true, last, now, limit, WATCHDOG_HUNG_TOOL_CEILING_MS, 0),
        "idle (is_waiting=true) sessions must never trigger the watchdog"
    );
}

#[test]
fn watchdog_does_not_fire_when_mid_turn_but_recent() {
    let limit = WATCHDOG_INACTIVITY_LIMIT_MS;
    let now = 10_000_000;
    let last = now - 30_000;
    assert!(
        !should_trigger_watchdog(false, last, now, limit, WATCHDOG_HUNG_TOOL_CEILING_MS, 0),
        "recent activity must not trigger the watchdog"
    );
}

#[test]
fn watchdog_fires_when_mid_turn_and_stale_with_no_tool() {
    let limit = WATCHDOG_INACTIVITY_LIMIT_MS;
    let now = 10_000_000;
    let last = now - limit - 1;
    assert!(
        should_trigger_watchdog(false, last, now, limit, WATCHDOG_HUNG_TOOL_CEILING_MS, 0),
        "mid-turn + stale + no tool in flight must trigger the watchdog \
         so the safety net can ResponseAbort"
    );
}

#[test]
fn watchdog_does_not_fire_while_tool_in_flight() {
    let limit = WATCHDOG_INACTIVITY_LIMIT_MS;
    let now = 10_000_000;
    let last = now - limit - 60_000;
    assert!(
        !should_trigger_watchdog(false, last, now, limit, WATCHDOG_HUNG_TOOL_CEILING_MS, 1),
        "tool in flight (e.g. TaskOutput, AskUserQuestion, long Bash) is \
         legitimate silence — watchdog must NOT fire"
    );
    assert!(
        !should_trigger_watchdog(false, last, now, limit, WATCHDOG_HUNG_TOOL_CEILING_MS, 5),
        "multiple tools in flight must also disarm the watchdog"
    );
}

#[test]
fn watchdog_fires_after_tool_completes_if_anthropic_hangs() {
    let limit = WATCHDOG_INACTIVITY_LIMIT_MS;
    let now = 10_000_000;
    let last = now - limit - 1;
    assert!(
        should_trigger_watchdog(false, last, now, limit, WATCHDOG_HUNG_TOOL_CEILING_MS, 0),
        "after the last tool result, tools_in_flight=0 re-arms the \
         watchdog so a dead Anthropic socket still gets killed"
    );
}

#[test]
fn watchdog_does_not_fire_when_heartbeat_uninitialized() {
    let limit = WATCHDOG_INACTIVITY_LIMIT_MS;
    let now = 10_000_000;
    assert!(
        !should_trigger_watchdog(false, 0, now, limit, WATCHDOG_HUNG_TOOL_CEILING_MS, 0),
        "uninitialized heartbeat must not trigger the watchdog"
    );
}

#[test]
fn watchdog_boundary_is_strictly_greater_than_limit() {
    let limit = WATCHDOG_INACTIVITY_LIMIT_MS;
    let now = 10_000_000;
    assert!(
        !should_trigger_watchdog(
            false,
            now - limit,
            now,
            limit,
            WATCHDOG_HUNG_TOOL_CEILING_MS,
            0
        ),
        "exactly at the limit must not fire (strict-greater check)"
    );
    assert!(
        should_trigger_watchdog(
            false,
            now - limit - 1,
            now,
            limit,
            WATCHDOG_HUNG_TOOL_CEILING_MS,
            0
        ),
        "1ms past the limit must fire"
    );
}

#[test]
fn watchdog_gate_is_waiting_short_circuits_first() {
    let limit = WATCHDOG_INACTIVITY_LIMIT_MS;
    let now = 10_000_000;
    let stale = now - limit - 60_000;
    assert_eq!(
        watchdog_gate(true, stale, now, limit, WATCHDOG_HUNG_TOOL_CEILING_MS, 0),
        WatchdogGate::SkipIsWaiting,
    );
    assert_eq!(
        watchdog_gate(true, stale, now, limit, WATCHDOG_HUNG_TOOL_CEILING_MS, 3),
        WatchdogGate::SkipIsWaiting,
    );
}

#[test]
fn watchdog_gate_bad_heartbeat_skips_before_tools_check() {
    let limit = WATCHDOG_INACTIVITY_LIMIT_MS;
    let now = 10_000_000;
    assert_eq!(
        watchdog_gate(false, 0, now, limit, WATCHDOG_HUNG_TOOL_CEILING_MS, 0),
        WatchdogGate::SkipBadHeartbeat,
    );
}

#[test]
fn watchdog_gate_tools_in_flight_carries_count() {
    let limit = WATCHDOG_INACTIVITY_LIMIT_MS;
    let now = 10_000_000;
    let stale = now - limit - 60_000;
    assert_eq!(
        watchdog_gate(false, stale, now, limit, WATCHDOG_HUNG_TOOL_CEILING_MS, 1),
        WatchdogGate::SkipToolsInFlight(1),
    );
    assert_eq!(
        watchdog_gate(false, stale, now, limit, WATCHDOG_HUNG_TOOL_CEILING_MS, 7),
        WatchdogGate::SkipToolsInFlight(7),
    );
}

#[test]
fn watchdog_gate_not_stale_when_within_limit() {
    let limit = WATCHDOG_INACTIVITY_LIMIT_MS;
    let now = 10_000_000;
    let recent = now - 30_000;
    assert_eq!(
        watchdog_gate(false, recent, now, limit, WATCHDOG_HUNG_TOOL_CEILING_MS, 0),
        WatchdogGate::NotStale,
    );
}

#[test]
fn watchdog_gate_fires_when_stale_and_no_blocker() {
    let limit = WATCHDOG_INACTIVITY_LIMIT_MS;
    let now = 10_000_000;
    let stale = now - limit - 1;
    assert_eq!(
        watchdog_gate(false, stale, now, limit, WATCHDOG_HUNG_TOOL_CEILING_MS, 0),
        WatchdogGate::Fire,
    );
}

#[test]
fn watchdog_gate_fires_past_ceiling_with_tools_in_flight() {
    let limit = WATCHDOG_INACTIVITY_LIMIT_MS;
    let ceiling = WATCHDOG_HUNG_TOOL_CEILING_MS;
    let now = 100_000_000;
    let within = now - limit - 60_000;
    assert_eq!(
        watchdog_gate(false, within, now, limit, ceiling, 3),
        WatchdogGate::SkipToolsInFlight(3),
        "tools in flight within the ceiling stays a legitimate skip",
    );
    let past = now - ceiling - 1;
    assert_eq!(
        watchdog_gate(false, past, now, limit, ceiling, 3),
        WatchdogGate::FirePastCeiling(3),
        "tools in flight past the ceiling must hand back FirePastCeiling",
    );
}

#[test]
fn watchdog_gate_ceiling_boundary_is_strictly_greater() {
    let limit = WATCHDOG_INACTIVITY_LIMIT_MS;
    let ceiling = WATCHDOG_HUNG_TOOL_CEILING_MS;
    let now = 100_000_000;
    assert_eq!(
        watchdog_gate(false, now - ceiling, now, limit, ceiling, 1),
        WatchdogGate::SkipToolsInFlight(1),
        "exactly at the ceiling must not fire",
    );
    assert_eq!(
        watchdog_gate(false, now - ceiling - 1, now, limit, ceiling, 1),
        WatchdogGate::FirePastCeiling(1),
        "1ms past the ceiling must fire",
    );
}

#[test]
fn watchdog_gate_is_waiting_beats_ceiling() {
    let limit = WATCHDOG_INACTIVITY_LIMIT_MS;
    let ceiling = WATCHDOG_HUNG_TOOL_CEILING_MS;
    let now = 100_000_000;
    let past = now - ceiling - 60_000;
    assert_eq!(
        watchdog_gate(true, past, now, limit, ceiling, 3),
        WatchdogGate::SkipIsWaiting,
    );
}

#[test]
fn watchdog_gate_diag_tags_are_stable() {
    assert_eq!(WatchdogGate::Fire.diag_tag(), "fire");
    assert_eq!(
        WatchdogGate::FirePastCeiling(2).diag_tag(),
        "fire_past_ceiling"
    );
    assert_eq!(WatchdogGate::NotStale.diag_tag(), "not_stale");
    assert_eq!(WatchdogGate::SkipIsWaiting.diag_tag(), "skip_is_waiting");
    assert_eq!(
        WatchdogGate::SkipBadHeartbeat.diag_tag(),
        "skip_bad_heartbeat"
    );
    assert_eq!(
        WatchdogGate::SkipToolsInFlight(4).diag_tag(),
        "skip_tools_in_flight"
    );
}

#[test]
fn watchdog_diag_log_threshold_is_half_the_fire_limit() {
    assert_eq!(
        WATCHDOG_DIAG_LOG_THRESHOLD_MS * 2,
        WATCHDOG_INACTIVITY_LIMIT_MS,
    );
}

const GATEWAY_MESSAGE: &str = "Couldn't load settings from Cloud gateway \
https://claude-gateway.example.com. Check your network connection, or sign in again with /login.";

fn safety_net_action_for(
    safety_net_fired: bool,
    watchdog_fired: bool,
    external_already: bool,
    killed_by_signal: bool,
    engine_cancelled: bool,
) -> SafetyNetAction {
    safety_net_action(&SafetyNetInput {
        safety_net_fired,
        watchdog_fired,
        external_terminal_already_emitted: external_already,
        engine_cancelled,
        init_seen: false,
        exit: &AgentExitFacts {
            killed_by_signal,
            ..AgentExitFacts::default()
        },
        coding_agent: CodingAgent::ClaudeCode,
    })
}

fn safety_net_action_after_exit(
    exit: AgentExitFacts,
    init_seen: bool,
    watchdog_fired: bool,
    engine_cancelled: bool,
    coding_agent: CodingAgent,
) -> SafetyNetAction {
    safety_net_action(&SafetyNetInput {
        safety_net_fired: true,
        watchdog_fired,
        external_terminal_already_emitted: false,
        engine_cancelled,
        init_seen,
        exit: &exit,
        coding_agent,
    })
}

fn exit_with(code: Option<i32>, stderr: &str) -> AgentExitFacts {
    AgentExitFacts {
        killed_by_signal: false,
        code,
        stderr_tail: stderr.to_string(),
    }
}

#[test]
fn safety_net_action_nothing_when_loop_ended_naturally() {
    assert_eq!(
        safety_net_action_for(false, false, false, false, false),
        SafetyNetAction::Nothing,
    );
    assert_eq!(
        safety_net_action_for(false, true, false, false, false),
        SafetyNetAction::Nothing,
        "watchdog flag is irrelevant when the natural terminator already landed",
    );
    assert_eq!(
        safety_net_action_for(false, false, true, false, false),
        SafetyNetAction::Nothing,
    );
    assert_eq!(
        safety_net_action_for(false, false, false, true, false),
        SafetyNetAction::Nothing,
        "a signal kill is irrelevant when the natural terminator already landed",
    );
}

#[test]
fn safety_net_action_skip_when_external_terminal_already_emitted() {
    assert_eq!(
        safety_net_action_for(true, false, true, false, false),
        SafetyNetAction::Skip,
    );
    assert_eq!(
        safety_net_action_for(true, true, true, false, false),
        SafetyNetAction::Skip,
        "external terminal wins over the watchdog too — no recovery, no abort \
         once a real terminal already closed the turn",
    );
    assert_eq!(
        safety_net_action_for(true, false, true, true, false),
        SafetyNetAction::Skip,
        "external terminal wins over a stray signal-kill too",
    );
}

#[test]
fn safety_net_action_continuation_requested_on_watchdog_fire() {
    assert_eq!(
        safety_net_action_for(true, true, false, false, false),
        SafetyNetAction::EmitContinuationRequested,
        "watchdog fire MUST route to auto-recovery, not abort",
    );
}

#[test]
fn safety_net_action_continuation_requested_on_stray_signal_kill() {
    assert_eq!(
        safety_net_action_for(true, false, false, true, false),
        SafetyNetAction::EmitContinuationRequested,
        "a stray signal-kill with no engine cancel MUST auto-resume",
    );
}

#[test]
fn safety_net_action_aborted_when_engine_cancelled_the_signal_kill() {
    assert_eq!(
        safety_net_action_for(true, false, false, true, true),
        SafetyNetAction::EmitAbortedSafetyNet,
        "engine-initiated signal kill MUST NOT auto-resume",
    );
}

#[test]
fn safety_net_action_aborted_when_no_watchdog_no_external_terminal() {
    assert_eq!(
        safety_net_action_for(true, false, false, false, false),
        SafetyNetAction::EmitAbortedSafetyNet,
        "non-watchdog non-signal safety net MUST emit ResponseAborted",
    );
}

fn startup_failed(code: i32) -> CodingAgentExit {
    CodingAgentExit {
        stage: CodingAgentExitStage::Startup,
        code: Some(code),
    }
}

#[test]
fn an_exit_before_the_startup_line_fails_loudly_with_the_cli_words() {
    assert_eq!(
        safety_net_action_after_exit(
            exit_with(Some(1), GATEWAY_MESSAGE),
            false,
            false,
            false,
            CodingAgent::ClaudeCode
        ),
        SafetyNetAction::EmitFailed {
            error: GATEWAY_MESSAGE.to_string(),
            agent_exit: startup_failed(1),
        },
    );
}

#[test]
fn a_silent_start_failure_names_the_agent_and_the_exit_code() {
    for (agent, name) in [
        (CodingAgent::ClaudeCode, "Claude Code"),
        (CodingAgent::Codex, "Codex"),
    ] {
        let SafetyNetAction::EmitFailed { error, agent_exit } =
            safety_net_action_after_exit(exit_with(Some(2), ""), false, false, false, agent)
        else {
            panic!("a non-zero exit with no stderr is still a failure");
        };
        assert_eq!(
            error,
            format!(
                "{name} exited with code 2 before its session began and wrote nothing to stderr."
            )
        );
        assert_eq!(agent_exit, startup_failed(2));
    }
}

#[test]
fn an_exit_after_the_startup_line_is_a_mid_turn_crash() {
    let SafetyNetAction::EmitFailed { error, agent_exit } = safety_net_action_after_exit(
        exit_with(Some(1), "Error: out of memory"),
        true,
        false,
        false,
        CodingAgent::ClaudeCode,
    ) else {
        panic!("a crash after init must fail the turn");
    };
    assert_eq!(error, "Error: out of memory");
    assert_eq!(
        agent_exit,
        CodingAgentExit {
            stage: CodingAgentExitStage::MidTurn,
            code: Some(1)
        }
    );
}

#[test]
fn stderr_alone_is_enough_to_fail_a_clean_exit() {
    assert!(matches!(
        safety_net_action_after_exit(
            exit_with(Some(0), "something went wrong"),
            false,
            false,
            false,
            CodingAgent::Codex
        ),
        SafetyNetAction::EmitFailed { .. }
    ));
}

#[test]
fn a_silent_clean_exit_keeps_the_safety_net_abort() {
    assert_eq!(
        safety_net_action_after_exit(
            exit_with(Some(0), ""),
            true,
            false,
            false,
            CodingAgent::ClaudeCode
        ),
        SafetyNetAction::EmitAbortedSafetyNet,
    );
}

#[test]
fn a_watchdog_kill_keeps_its_retry_whatever_the_exit_says() {
    assert_eq!(
        safety_net_action_after_exit(
            exit_with(Some(1), GATEWAY_MESSAGE),
            true,
            true,
            false,
            CodingAgent::ClaudeCode
        ),
        SafetyNetAction::EmitContinuationRequested,
        "a hang is not a start failure",
    );
}

#[test]
fn a_stray_signal_kill_before_the_startup_line_keeps_its_retry() {
    let exit = AgentExitFacts {
        killed_by_signal: true,
        code: Some(143),
        stderr_tail: "terminated".to_string(),
    };
    assert_eq!(
        safety_net_action_after_exit(exit, false, false, false, CodingAgent::ClaudeCode),
        SafetyNetAction::EmitContinuationRequested,
    );
}

#[test]
fn an_engine_teardown_with_words_on_stderr_is_not_the_agents_failure() {
    assert_eq!(
        safety_net_action_after_exit(
            exit_with(None, "terminated by the engine"),
            true,
            false,
            true,
            CodingAgent::ClaudeCode
        ),
        SafetyNetAction::EmitAbortedSafetyNet,
    );
}

#[test]
fn an_external_terminal_wins_over_a_failed_exit() {
    assert_eq!(
        safety_net_action(&SafetyNetInput {
            safety_net_fired: true,
            watchdog_fired: false,
            external_terminal_already_emitted: true,
            engine_cancelled: false,
            init_seen: false,
            exit: &exit_with(Some(1), GATEWAY_MESSAGE),
            coding_agent: CodingAgent::ClaudeCode,
        }),
        SafetyNetAction::Skip,
    );
}

#[test]
fn transient_api_failures_are_recognized_by_the_api_error_prefix() {
    for error in [
        "API Error: Connection closed mid-response. The response above may be incomplete.",
        "API Error: Stream idle timeout - no chunks received",
        "API Error: 529 overloaded",
    ] {
        assert!(
            is_transient_api_failure(&TerminalKind::Failed {
                error: error.to_string()
            }),
            "{error} is a transient upstream drop and MUST be resumable",
        );
    }
}

#[test]
fn deterministic_failures_are_not_transient() {
    for error in [
        "error_max_turns",
        "No conversation found with session ID: 7c61f11b",
        EMPTY_RESPONSE_ERROR,
        "the log shows an API Error further down, which I fixed",
    ] {
        assert!(
            !is_transient_api_failure(&TerminalKind::Failed {
                error: error.to_string()
            }),
            "{error} reproduces on resume and MUST NOT auto-resume",
        );
    }
}

#[test]
fn only_a_failed_terminal_can_auto_resume() {
    use crate::engine::thread_events::{AbortCause, CancelCause};
    for terminal in [
        TerminalKind::Generated,
        TerminalKind::Canceled(CancelCause::UserStop),
        TerminalKind::Aborted(AbortCause::EngineShutdown),
    ] {
        assert!(!is_transient_api_failure(&terminal));
        assert!(!auto_resume_after_api_error(
            &Some(terminal.clone()),
            0,
            false,
            false
        ));
    }
    assert!(
        !auto_resume_after_api_error(&None, 0, false, false),
        "no terminal at all is the safety net's business, not ours",
    );
}

#[test]
fn api_error_auto_resume_is_bounded() {
    let dropped = Some(TerminalKind::Failed {
        error: "API Error: Connection closed mid-response.".to_string(),
    });
    for spent in 0..MAX_API_ERROR_AUTO_RESUMES {
        assert!(
            auto_resume_after_api_error(&dropped, spent, false, false),
            "{spent} of {MAX_API_ERROR_AUTO_RESUMES} spent must still resume",
        );
    }
    assert!(
        !auto_resume_after_api_error(&dropped, MAX_API_ERROR_AUTO_RESUMES, false, false),
        "the budget is a hard cap",
    );
}

#[test]
fn shutdown_and_conflict_resolution_refuse_the_auto_resume() {
    let dropped = Some(TerminalKind::Failed {
        error: "API Error: Connection closed mid-response.".to_string(),
    });
    assert!(!auto_resume_after_api_error(&dropped, 0, true, false));
    assert!(!auto_resume_after_api_error(&dropped, 0, false, true));
    assert!(
        auto_resume_after_api_error(&dropped, 0, false, false),
        "with neither gate set the same input MUST resume (else this test proves nothing)",
    );
}

#[test]
fn api_error_resume_opens_a_resume_exchange() {
    use crate::engine::agent_recovery::{
        continue_should_open_resume_exchange, ANSWERED_AFTER_IDLE_REASON,
        AUTO_RESUME_AFTER_API_ERROR_REASON,
    };
    assert!(continue_should_open_resume_exchange(Some(
        AUTO_RESUME_AFTER_API_ERROR_REASON
    )));
    assert!(!continue_should_open_resume_exchange(Some(
        ANSWERED_AFTER_IDLE_REASON
    )));
}

#[test]
fn both_session_exits_reach_the_api_drop_auto_resume() {
    const RUN_SRC: &str = include_str!("../run_session/run.rs");
    const COMPLETION_SRC: &str = include_str!("../run_session/completion.rs");
    const CALL: &str = ".maybe_auto_resume_after_api_error(";

    assert!(
        COMPLETION_SRC.contains("async fn maybe_auto_resume_after_api_error("),
        "the shared auto-resume helper must stay in completion.rs under that name",
    );
    assert_eq!(
        COMPLETION_SRC.matches(CALL).count(),
        1,
        "finalize_direct_agent must call the helper exactly once: it is the exit for every \
         session end that does NOT idle first (safety net, user Stop, shutdown, conflict \
         resolution).",
    );

    let release_log = RUN_SRC
        .find("CC process exited while idle")
        .expect("run.rs must still carry the idle-exit arm");
    let arm = &RUN_SRC[release_log..];
    let call = arm.find(CALL).expect(
        "the idle-exit arm must call the auto-resume helper before returning. Without it a \
         transient upstream API drop leaves the thread dead behind a red dot until a human \
         notices, which is the bug this whole feature exists to fix.",
    );
    let teardown = arm
        .find("self.upsert_cc_commands_cache(repo_key, info)")
        .expect("the idle-exit arm must still tear the session down before returning");
    let drain = arm
        .find("let orphans = lost_followups_to_orphans(")
        .expect("the idle-exit arm must still drain follow-ups before returning");
    let ret = arm
        .find("return Ok(ProcessResult {")
        .expect("the idle-exit arm must still return a ProcessResult");
    assert!(
        teardown < call && call < ret,
        "the auto-resume call must sit after the session teardown and before the return: \
         emitting while the subprocess is still in agent_sessions races the spawn dispatcher \
         into a session the loop is about to cancel.",
    );
    assert!(
        drain < call,
        "the auto-resume call must sit after the follow-up drain: it stands down when a \
         follow-up is already queued, and it can only know that once `orphans` exists.",
    );
}

#[test]
fn only_a_transient_api_failure_yields_an_error_for_the_hold() {
    use crate::engine::agent_session::lifecycle::transient_api_failure_error;

    assert_eq!(
        transient_api_failure_error(&Some(TerminalKind::Failed {
            error: "API Error: Connection lost mid-response.".to_string(),
        })),
        Some("API Error: Connection lost mid-response."),
        "the hold must carry what the withheld card would have said",
    );
    assert_eq!(
        transient_api_failure_error(&Some(TerminalKind::Generated)),
        None,
        "a terminal the engine would never resume withholds nothing",
    );
}

#[test]
fn a_resume_that_never_happens_still_reaches_the_parent() {
    assert_eq!(
        held_completion_release(false, false),
        HeldCompletionRelease::Announce,
        "no continuation and nobody else coming: the withheld failure is the last word",
    );
    assert_eq!(
        held_completion_release(true, false),
        HeldCompletionRelease::KeepWithholding,
        "the resume is on its way, so the card belongs at the real terminal",
    );
    assert_eq!(
        held_completion_release(false, true),
        HeldCompletionRelease::KeepWithholding,
        "a drained follow-up is re-submitted and its turn reports for itself; \
         `parent_callback_pending` was never cleared, so that card still fires",
    );
    assert_eq!(
        held_completion_release(true, true),
        HeldCompletionRelease::KeepWithholding,
        "unreachable today (the follow-up stand-down precedes any emit), and it must \
         stay on the quiet side if it ever becomes reachable",
    );
}

#[test]
fn the_auto_resume_hold_is_taken_before_the_terminal_is_emitted() {
    const RUN_SRC: &str = include_str!("../run_session/run.rs");
    const COMPLETION_SRC: &str = include_str!("../run_session/completion.rs");

    assert!(
        COMPLETION_SRC.contains("async fn hold_completion_if_api_error_resume("),
        "the hold decision must stay in completion.rs, beside the resume it pairs with",
    );

    let classify = RUN_SRC
        .find("let (terminal_kind, emit_idle) = classify_result(")
        .expect("run.rs must still classify the Result before acting on it");
    let arm = &RUN_SRC[classify..];
    let hold = arm.find(".hold_completion_if_api_error_resume(").expect(
        "the Result arm must decide the auto-resume before it emits the terminal. Without \
         it the child-to-parent fan-in announces a failure for a child the engine is about \
         to resume, and a parent that believes the card spawns a duplicate session onto the \
         same files.",
    );
    let emit = arm
        .find("\"[AgentSession] terminal event (Result classify)\"")
        .expect("the Result arm must still emit its terminal event");
    assert!(
        hold < emit,
        "the hold must be taken BEFORE the terminal emit: the emit is what runs the fan-in, \
         so a hold taken afterwards cannot withhold anything.",
    );
}

#[test]
fn the_auto_resume_hold_is_released_inside_the_arm_that_took_it() {
    const RUN_SRC: &str = include_str!("../run_session/run.rs");

    let classify = RUN_SRC
        .find("let (terminal_kind, emit_idle) = classify_result(")
        .expect("run.rs must still classify the Result before acting on it");
    let arm = &RUN_SRC[classify..];
    let hold = arm
        .find(".hold_completion_if_api_error_resume(")
        .expect("the Result arm must still take the hold");
    let idle_emit = arm
        .find("self.emit_coding_agent_idled(")
        .expect("the Result arm must still emit this turn's CodingAgentIdled");
    let release = arm.find("auto_resume_holds().release(thread_id)").expect(
        "the Result arm must release the hold it took. Left to the resume site, the hold \
             spans every exit in between, so a Stop or a safety net in a kept-alive turn is \
             swallowed and the parent hears nothing about it. Whether such a Stop also \
             cancels a pending resume is a separate, PRE-EXISTING question the plan records.",
    );
    assert!(
        hold < idle_emit && idle_emit < release,
        "the release must follow the idle emit: both the terminal and its CodingAgentIdled \
         are fan-in-visible, so releasing between them lets the idle announce a completion \
         for a child that is about to resume.",
    );

    assert!(
        RUN_SRC.contains("let mut withheld_api_error: Option<String> = None;")
            && RUN_SRC.contains("&mut withheld_api_error,"),
        "the released hold must travel as a per-turn local that `reset_per_turn_flags` \
         clears, not as a register entry that outlives its turn.",
    );
}

#[test]
fn the_idle_arm_proposes_before_it_idles() {
    const RUN_SRC: &str = include_str!("../run_session/run.rs");

    let classify = RUN_SRC
        .find("let (terminal_kind, emit_idle) = classify_result(")
        .expect("run.rs must still classify the Result before acting on it");
    let arm = &RUN_SRC[classify..];
    let idle_emit = arm
        .find("self.emit_coding_agent_idled(")
        .expect("the Result arm must still emit this turn's CodingAgentIdled");
    for write in [
        "withhold_handed_off_work(",
        "self.propose_turn_work(",
        "self.reconcile_emptied_pending_change(",
    ] {
        let at = arm
            .find(write)
            .unwrap_or_else(|| panic!("the Result arm must still call {write}"));
        assert!(
            at < idle_emit,
            "{write} must run BEFORE the idle emit: the idle builds the parent's card, \
             so a change written afterwards never reaches it",
        );
    }
    let handed_off = arm
        .find("if idle_write == Some(IdleChangeWrite::HandedOff)")
        .expect("the Result arm must still route a handed-off idle on its own");
    let propose = arm.find("self.propose_turn_work(").unwrap();
    assert!(
        handed_off < propose,
        "a handed-off idle must branch off before the propose, or Apply flashes"
    );
}

#[test]
fn the_idle_arm_covers_polled_tasks_before_it_idles() {
    const RUN_SRC: &str = include_str!("../run_session/run.rs");

    let classify = RUN_SRC
        .find("let (terminal_kind, emit_idle) = classify_result(")
        .expect("run.rs must still classify the Result before acting on it");
    let arm = &RUN_SRC[classify..];
    let gate = arm
        .find("let ended_cleanly = turn_ended_cleanly(&last_terminal_kind, is_resolution_session, keep_alive_turn || ceiling_turn);")
        .expect("the Result arm must still decide whether the turn ended cleanly");
    let arming = arm
        .find("self.arm_wait_for_running_background_tasks(thread_id, OwnerTurn::Ended)")
        .expect("the Result arm must arm with the turn ended");
    const CLEAN_END: &str = "if ended_cleanly {";
    let opens = arm[..arming]
        .rfind(CLEAN_END)
        .expect("the arming must sit inside the clean-end gate");
    assert!(
        gate < opens,
        "the gate must open after the clean-end decision"
    );
    assert!(
        arm[opens + CLEAN_END.len()..arming].trim().is_empty(),
        "the arming must be the clean-end gate's first statement"
    );
    for later in [
        "let holds_live_wait =",
        "self.gate_coding_agent_turn_end(",
        "self.emit_coding_agent_idled(",
    ] {
        let at = arm
            .find(later)
            .unwrap_or_else(|| panic!("the Result arm must still reach {later}"));
        assert!(
            arming < at,
            "the turn-end arming must run BEFORE {later}, which reads the thread's waits"
        );
    }
}

#[test]
fn the_idle_arm_decides_before_it_idles_and_re_enters_after() {
    const RUN_SRC: &str = include_str!("../run_session/run.rs");

    let classify = RUN_SRC
        .find("let (terminal_kind, emit_idle) = classify_result(")
        .expect("run.rs must still classify the Result before acting on it");
    let arm = &RUN_SRC[classify..];
    let idle_emit = arm
        .find("self.emit_coding_agent_idled(")
        .expect("the Result arm must still emit this turn's CodingAgentIdled");
    let decide = arm
        .find("self.gate_coding_agent_turn_end(thread_id, wt_has_changes, session_start)")
        .expect("the Result arm must force the missing turn-end decisions");
    let reenter = arm
        .find("reentry_ledger.take_due(&gate)")
        .expect("the Result arm must send the turn-end re-entries");
    assert!(
        decide < idle_emit,
        "the turn-end decisions are recorded before the idle"
    );
    assert!(
        idle_emit < reenter,
        "a re-entry opens its own exchange after the idle"
    );
}

#[test]
fn the_ceiling_reentry_is_an_engine_turn_read_after_the_idle() {
    const RUN_SRC: &str = include_str!("../run_session/run.rs");
    assert!(RUN_SRC.contains("ceiling_delivery.observe(&ev, new_turn_after_terminal);"));
    assert!(!RUN_SRC.contains("std::mem::take(&mut ceiling_reentry_forwarded)"));
    let classify = RUN_SRC
        .find("let (terminal_kind, emit_idle) = classify_result(")
        .expect("run.rs must still classify the Result before acting on it");
    let arm = &RUN_SRC[classify..];
    let find = |needle: &str| {
        arm.find(needle)
            .unwrap_or_else(|| panic!("the Result arm must still contain {needle}"))
    };
    let taken = find("let ceiling_read = ceiling_delivery.turn_ended();");
    let clean = find("turn_ended_cleanly(&last_terminal_kind, is_resolution_session, keep_alive_turn || ceiling_turn)");
    let idle_emit = find("self.emit_coding_agent_idled(");
    const ORDINARY_END: &str = "let ceiling = if ceiling_end == CeilingTurnEnd::Reenter && emit_idle && !keep_alive_turn && !is_resolution_session {";
    let gated = find(ORDINARY_END);
    let read = find("crate::engine::context_ceiling::ceiling_crossing(self.pool(), thread_id, inputs.owed() > 0)");
    let send = find("context_ceiling::record_ceiling_reentry(");
    let head = find("ceiling_reentry_forwarded = forward_ceiling_input(");
    let idle_exit = find("match idle_action(");
    assert!(
        taken < clean,
        "the turn consumes only its own read classification"
    );
    assert!(idle_emit < gated, "the re-entry follows the idle");
    assert!(
        arm[gated + ORDINARY_END.len()..read].trim().is_empty(),
        "the reading is the clean-end gate's first statement"
    );
    assert!(read < send && send < head && head < idle_exit);
}

#[test]
fn the_reentry_turn_holds_input_and_ends_in_a_handoff() {
    const RUN_SRC: &str = include_str!("../run_session/run.rs");

    assert!(
        RUN_SRC.contains(
            "Some(mut user_input) = msg_rx.recv(), if input_gate.receives() && !ceiling_reentry_forwarded => {"
        ),
        "the input arm closes while the re-entry turn runs"
    );
    assert!(RUN_SRC.contains("s.in_ceiling_reentry = true;"));

    let classify = RUN_SRC
        .find("let (terminal_kind, emit_idle) = classify_result(")
        .expect("run.rs must still classify the Result before acting on it");
    let arm = &RUN_SRC[classify..];
    let find = |needle: &str| {
        arm.find(needle)
            .unwrap_or_else(|| panic!("the Result arm must still contain {needle}"))
    };
    let terminal = find("event: terminal_event,");
    let taken = find("let ceiling_read = ceiling_delivery.turn_ended();");
    let settle = find(
        "if !is_shutdown && (ceiling_read.is_some() || ceiling_end == CeilingTurnEnd::Brief) {\n",
    );
    let pending = find("handoff_pending: crate::engine::pending_handoff::handoff_pending(");
    let idle_emit = find("self.emit_coding_agent_idled(");
    assert!(
        terminal < settle,
        "a Stop's ResponseCanceled must come first, or it would withdraw the brief"
    );
    assert!(taken < settle && settle < pending && pending < idle_emit);
    assert!(arm[settle..pending].contains("self.settle_owed_ceiling_handoff(thread_id).await;"));
}

#[test]
fn the_idle_arm_gates_its_proposal_on_live_event_waits() {
    const RUN_SRC: &str = include_str!("../run_session/run.rs");

    let read_at = RUN_SRC
        .find("let holds_live_wait = !self.live_waits.for_thread(thread_id).await.is_empty();")
        .expect("the Result arm must read the thread's live event waits");
    let from_read = &RUN_SRC[read_at..];
    let gate_at = from_read
        .find("idle_change_write(")
        .expect("the live-wait read must precede the change gate");
    let gate_end = gate_at
        + from_read[gate_at..]
            .find("&last_terminal_kind")
            .expect("the change gate must still take the terminal kind");
    assert!(
        from_read[gate_at..gate_end].contains("holds_live_wait,"),
        "the change gate must receive the live-wait answer"
    );
    assert!(
        from_read.contains(
            "Some(_) if !write.may_create() && !self.branch_has_open_change(&branch_name).await =>"
        ),
        "a held idle must create no change, and only re-sync an open one"
    );
}

#[test]
fn the_resume_actuates_the_decision_rather_than_re_deciding() {
    const COMPLETION_SRC: &str = include_str!("../run_session/completion.rs");

    let helper = COMPLETION_SRC
        .find("async fn maybe_auto_resume_after_api_error(")
        .expect("the resume helper must keep its name");
    let body = &COMPLETION_SRC[helper..];
    let next_fn = body
        .find("async fn finalize_direct_agent(")
        .expect("finalize_direct_agent must still follow the resume helper");
    let scope = &body[..next_fn];

    assert!(
        scope.contains("let Some(withheld_error) = withheld_error else {"),
        "the resume helper must act on the decision the run loop handed it",
    );
    assert_eq!(
        scope.matches("auto_resume_after_api_error(").count(),
        scope.matches("maybe_auto_resume_after_api_error(").count(),
        "the resume helper must not call the decision function: it was already asked \
         before the terminal was emitted, and a second copy could disagree with a card \
         already out the door.",
    );

    let shutdown = scope
        .find("self.is_shutting_down()")
        .expect("the resume helper must re-ask about shutdown");
    let decision = scope
        .find("held_completion_release(")
        .expect("the resume helper must route through the release decision");
    assert!(
        shutdown < decision,
        "the shutdown check must precede the release decision",
    );
    assert!(
        !scope[shutdown..decision].contains("return"),
        "the shutdown arm must not return early: it withheld a card and scheduled no \
         resume, so the parent has to be told or it waits on a child forever.",
    );
}

#[test]
fn the_idle_keep_alive_cannot_be_fed_by_a_phantom_count() {
    const RUN_SRC: &str = include_str!("../run_session/run.rs");

    assert!(
        !RUN_SRC.contains("pending_followups"),
        "the phantom counter must stay deleted. It counted messages SENT and settled once \
         per Result, so a Claude Code turn that merged N inputs into one Result reported \
         N-1 follow-ups that had already been consumed.",
    );

    let call_sites = |needle: &str| -> Vec<&str> {
        RUN_SRC
            .match_indices(needle)
            .map(|(at, _)| {
                let call = &RUN_SRC[at..];
                &call[..call.find(";").expect("the call ends")]
            })
            .collect()
    };
    let direct = call_sites("self.exit_idle_subprocess_unless_owed(");
    let settled = call_sites("self.idle_exit_ends_session(");
    assert!(!direct.is_empty(), "the idle arm decides the idle exit");
    assert!(
        settled.len() >= 2,
        "the withdraw arm and the take-settle arm both decide the idle exit again"
    );
    let calls: Vec<&str> = direct.into_iter().chain(settled).collect();
    for call in &calls {
        assert!(
            call.contains("inputs.owed()"),
            "the idle exit must read what is still unread from the input ledger. A count \
             settled at each Result is how a Claude Code input that arrived after the \
             turn's last tool call was killed with the subprocess (ADR 0268). Got: {call}",
        );
    }
    let exit_arm = RUN_SRC
        .find("IdleAction::ExitSubprocess => {")
        .expect("run.rs must still carry the idle-exit subprocess decision");
    let arm_body = &RUN_SRC[exit_arm..];
    assert!(
        arm_body[..arm_body.find('}').expect("the arm closes")]
            .contains("self.exit_idle_subprocess_unless_owed("),
        "the ExitSubprocess arm must route through exit_idle_subprocess_unless_owed",
    );

    let body = &RUN_SRC[RUN_SRC
        .find("async fn exit_idle_subprocess_unless_owed(")
        .expect("run.rs must still carry the idle-exit decision")..];
    let decision = body
        .find("match terminate_decision(")
        .expect("the idle exit must still route through terminate_decision");
    let args_end = body[decision..]
        .find(") {")
        .expect("terminate_decision's argument list must be closed");
    assert!(
        body[decision..decision + args_end].contains("msg_rx.len()"),
        "the channel depth must be an ARGUMENT of terminate_decision. `msg_rx` is the only \
         authoritative answer to `is a follow-up still unread`, and it is exact here because \
         the decision holds the same agent_sessions lock the fast-path send takes.",
    );
    let arm = &body[decision..];

    assert!(
        RUN_SRC.contains("inputs.forwarded(") && RUN_SRC.contains("inputs.observe(&ev)"),
        "the run loop must record every forwarded input in the ledger and pass every agent \
         event through `observe`, which is the only place a read report settles one.",
    );

    assert!(
        arm.contains("TerminateDecision::Terminate =>"),
        "terminate_decision's Terminate branch must still exist in the idle exit: it is the only \
         path from a transient API drop to the auto-resume emit site.",
    );
}

#[test]
fn a_resolution_session_ends_only_when_no_follow_up_is_owed() {
    const RUN_SRC: &str = include_str!("../run_session/run.rs");
    let end_arm = RUN_SRC
        .find("IdleAction::EndSession => {")
        .expect("run.rs must still decide where a resolution session ends");
    let arm = &RUN_SRC[end_arm..];
    let arm = &arm[..arm
        .find("IdleAction::ExitSubprocess => {")
        .expect("the next arm")];
    let decision = arm
        .find("self.exit_idle_subprocess_unless_owed(")
        .expect("the EndSession arm must ask whether a follow-up is owed");
    let end = arm
        .find("break 'event_loop;")
        .expect("the EndSession arm still ends the loop");
    assert!(
        decision < end,
        "the session may end only after the owed-input check said nothing is coming"
    );
}

#[test]
fn context_ceiling_steer_preserves_turn_state_and_stop_withdraws_first() {
    const RUN_SRC: &str = include_str!("../run_session/run.rs");
    let forward = RUN_SRC
        .split("fn forward_ceiling_input(")
        .nth(1)
        .unwrap()
        .split("fn drain_startup_death(")
        .next()
        .unwrap();
    assert!(forward.contains("defer_on_steer_failure: true"));
    assert!(!forward.contains("reset_per_turn_flags"));
    let stop = RUN_SRC
        .split("_ = interrupt.notified()")
        .nth(1)
        .unwrap()
        .split("_ = stop.notified()")
        .next()
        .unwrap();
    assert!(
        stop.find("withdraw_ceiling_before_stop(").unwrap()
            < stop.find("ControlRequest::Interrupt").unwrap()
    );
    assert!(stop.contains("interruption_cause.is_none() && !shutdown_interrupt"));
    assert!(RUN_SRC.contains("held_ceiling_inputs.push(user_input);"));
    assert!(RUN_SRC.contains("for held in held_ceiling_inputs.into_iter().rev()"));
}
