use super::*;

/// Bool wrapper kept for the legacy fire/no-fire tests below. Takes the ceiling
/// explicitly; callers pass `WATCHDOG_HUNG_TOOL_CEILING_MS` (these tests exercise
/// the normal limit — their elapsed gaps are well under the ceiling — so it's
/// irrelevant to them). The ceiling has its own dedicated tests below
/// (`watchdog_gate_fires_past_ceiling_*`). New tests match on `watchdog_gate(...)`
/// directly — see `watchdog_gate_*`.
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

/// Idle CC sits silent waiting for the next user input — that silence is
/// the resting state, not a hang. The watchdog must NOT fire here, even
/// if the gap exceeds the limit.
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

/// Mid-turn but recent — CC just emitted text, tool result, etc. Watchdog
/// stays armed silently.
#[test]
fn watchdog_does_not_fire_when_mid_turn_but_recent() {
    let limit = WATCHDOG_INACTIVITY_LIMIT_MS;
    let now = 10_000_000;
    let last = now - 30_000; // 30s ago, well within 10min limit
    assert!(
        !should_trigger_watchdog(false, last, now, limit, WATCHDOG_HUNG_TOOL_CEILING_MS, 0),
        "recent activity must not trigger the watchdog"
    );
}

/// The actual hang: mid-turn, no tool in flight, last event older than
/// the limit. Network died during an in-flight Anthropic API call; CC
/// is sleeping forever on a TCP socket the kernel hasn't torn down.
/// This is the one scenario the watchdog ships to catch.
#[test]
fn watchdog_fires_when_mid_turn_and_stale_with_no_tool() {
    let limit = WATCHDOG_INACTIVITY_LIMIT_MS;
    let now = 10_000_000;
    let last = now - limit - 1; // just past the limit
    assert!(
        should_trigger_watchdog(false, last, now, limit, WATCHDOG_HUNG_TOOL_CEILING_MS, 0),
        "mid-turn + stale + no tool in flight must trigger the watchdog \
         so the safety net can ResponseAbort"
    );
}

/// The bug we're fixing: a long-running tool (TaskOutput polling a
/// background bash, AskUserQuestion waiting for the user, a multi-minute
/// Bash, an agent sub-task, etc.) holds CC silent past the 10-min
/// window, and the wall-clock-only watchdog killed it on suspicion of a
/// hang. The opt-in design refuses to fire while a tool is in flight —
/// tool execution is legitimate silence.
#[test]
fn watchdog_does_not_fire_while_tool_in_flight() {
    let limit = WATCHDOG_INACTIVITY_LIMIT_MS;
    let now = 10_000_000;
    let last = now - limit - 60_000; // way past the limit
    assert!(
        !should_trigger_watchdog(false, last, now, limit, WATCHDOG_HUNG_TOOL_CEILING_MS, 1),
        "tool in flight (e.g. TaskOutput, AskUserQuestion, long Bash) is \
         legitimate silence — watchdog must NOT fire"
    );
    // Higher counts (CC batches multiple tool calls in one turn) likewise
    // disarm the watchdog.
    assert!(
        !should_trigger_watchdog(false, last, now, limit, WATCHDOG_HUNG_TOOL_CEILING_MS, 5),
        "multiple tools in flight must also disarm the watchdog"
    );
}

/// After the last tool result lands, `tools_in_flight` returns to 0 and
/// the watchdog re-arms. If the next Anthropic round-trip dies on a dead
/// TCP socket and CC sleeps past the limit, this is exactly the path
/// the watchdog must catch. Pin that re-arming works.
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

/// Defensive: if `last_event_at_ms == 0`, skip rather than fire — a 0
/// timestamp means the heartbeat hasn't been initialized, not that the
/// session has been silent since the Unix epoch.
#[test]
fn watchdog_does_not_fire_when_heartbeat_uninitialized() {
    let limit = WATCHDOG_INACTIVITY_LIMIT_MS;
    let now = 10_000_000;
    assert!(
        !should_trigger_watchdog(false, 0, now, limit, WATCHDOG_HUNG_TOOL_CEILING_MS, 0),
        "uninitialized heartbeat must not trigger the watchdog"
    );
}

/// Boundary: exactly at the limit (now - last == limit) does NOT fire —
/// the strict-greater check gives one tick of grace. One millisecond past
/// the limit fires.
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

// ─── watchdog_gate diagnostic outcome ──────────────────────────────
//
// The `WatchdogGate` enum names every reason the watchdog might not
// fire so the diagnostic log can pin which gate held during a stuck
// period. These tests guard the gate → tag mapping (log scrapers grep
// these tags) and the routing from input state to gate variant.

#[test]
fn watchdog_gate_is_waiting_short_circuits_first() {
    let limit = WATCHDOG_INACTIVITY_LIMIT_MS;
    let now = 10_000_000;
    let stale = now - limit - 60_000;
    // is_waiting wins even when every other condition would fire.
    assert_eq!(
        watchdog_gate(true, stale, now, limit, WATCHDOG_HUNG_TOOL_CEILING_MS, 0),
        WatchdogGate::SkipIsWaiting,
    );
    // …and even when tools are in flight (is_waiting is a stronger
    // skip than tools — both indicate legitimate silence).
    assert_eq!(
        watchdog_gate(true, stale, now, limit, WATCHDOG_HUNG_TOOL_CEILING_MS, 3),
        WatchdogGate::SkipIsWaiting,
    );
}

#[test]
fn watchdog_gate_bad_heartbeat_skips_before_tools_check() {
    let limit = WATCHDOG_INACTIVITY_LIMIT_MS;
    let now = 10_000_000;
    // Uninitialized heartbeat skips even when tools=0 + mid-turn would
    // otherwise fire. We don't want to kill a session on a defensive
    // 0 read just because the elapsed math looks alarming.
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
    let recent = now - 30_000; // 30s ago — within the 10-min limit
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

/// The root-cause fix: a tool "in flight" (here a hung `/harden` sub-agent)
/// past the hung-tool ceiling no longer earns an unbounded skip — the gate
/// hands back `FirePastCeiling` so the caller can recover after a projection
/// re-check. Below the ceiling it stays `SkipToolsInFlight`.
#[test]
fn watchdog_gate_fires_past_ceiling_with_tools_in_flight() {
    let limit = WATCHDOG_INACTIVITY_LIMIT_MS;
    let ceiling = WATCHDOG_HUNG_TOOL_CEILING_MS;
    let now = 100_000_000;
    // Past the normal limit but within the ceiling → still a tools skip.
    let within = now - limit - 60_000;
    assert_eq!(
        watchdog_gate(false, within, now, limit, ceiling, 3),
        WatchdogGate::SkipToolsInFlight(3),
        "tools in flight within the ceiling stays a legitimate skip",
    );
    // Past the ceiling → FirePastCeiling, carrying the count.
    let past = now - ceiling - 1;
    assert_eq!(
        watchdog_gate(false, past, now, limit, ceiling, 3),
        WatchdogGate::FirePastCeiling(3),
        "tools in flight past the ceiling must hand back FirePastCeiling",
    );
}

/// Ceiling boundary: exactly at the ceiling does NOT fire (strict-greater,
/// mirroring the normal-limit boundary); one ms past does.
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

/// `is_waiting` still short-circuits even past the ceiling — a turn-boundary
/// wait is never a hang regardless of elapsed time.
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

/// Tag stability: the diagnostic log groups by tag, so a tag rename
/// breaks every existing scraper. Add new variants here when the enum
/// grows; never silently rename an existing tag.
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

/// Pin the diag threshold = half the fire limit so a future bump of one
/// without the other fails this test.
#[test]
fn watchdog_diag_log_threshold_is_half_the_fire_limit() {
    assert_eq!(
        WATCHDOG_DIAG_LOG_THRESHOLD_MS * 2,
        WATCHDOG_INACTIVITY_LIMIT_MS,
    );
}

// ─── safety_net_action decision tree ──────────────────────────────────
//
// Pin the input combinations so a refactor of `run_session.rs` cleanup
// can't silently drop a branch (e.g. forget the `ContinuationRequested`
// emit on watchdog fire, leaving threads stuck at "running" when the
// network dies mid-call, or forget the stray-signal auto-resume).
//
// Args: (safety_net_fired, watchdog_fired, external_already,
//        killed_by_signal, engine_cancelled).

#[test]
fn safety_net_action_nothing_when_loop_ended_naturally() {
    // Loop emitted a natural terminator (Generated, Failed, Canceled,
    // Aborted) — no safety net needed. The other inputs are irrelevant.
    assert_eq!(
        safety_net_action(false, false, false, false, false),
        SafetyNetAction::Nothing,
    );
    assert_eq!(
        safety_net_action(false, true, false, false, false),
        SafetyNetAction::Nothing,
        "watchdog flag is irrelevant when the natural terminator already landed",
    );
    assert_eq!(
        safety_net_action(false, false, true, false, false),
        SafetyNetAction::Nothing,
    );
    assert_eq!(
        safety_net_action(false, false, false, true, false),
        SafetyNetAction::Nothing,
        "a signal kill is irrelevant when the natural terminator already landed",
    );
}

#[test]
fn safety_net_action_skip_when_external_terminal_already_emitted() {
    // The engine-restart fast-path landed a terminal before reaching
    // cleanup — a second terminal would relabel the closed turn.
    assert_eq!(
        safety_net_action(true, false, true, false, false),
        SafetyNetAction::Skip,
    );
    assert_eq!(
        safety_net_action(true, true, true, false, false),
        SafetyNetAction::Skip,
        "external terminal wins over the watchdog too — no recovery, no abort \
         once a real terminal already closed the turn",
    );
    assert_eq!(
        safety_net_action(true, false, true, true, false),
        SafetyNetAction::Skip,
        "external terminal wins over a stray signal-kill too",
    );
}

#[test]
fn safety_net_action_continuation_requested_on_watchdog_fire() {
    // Watchdog killed CC mid-call (dead TCP socket) — route to
    // auto-recovery, not abort, so the user never sees a spurious
    // failure state.
    assert_eq!(
        safety_net_action(true, true, false, false, false),
        SafetyNetAction::EmitContinuationRequested,
        "watchdog fire MUST route to auto-recovery, not abort",
    );
}

#[test]
fn safety_net_action_continuation_requested_on_stray_signal_kill() {
    // The exit=143 bug: CC died from a stray external SIGTERM that the
    // engine did NOT initiate — auto-resume instead of a red-dot abort.
    assert_eq!(
        safety_net_action(true, false, false, true, false),
        SafetyNetAction::EmitContinuationRequested,
        "a stray signal-kill with no engine cancel MUST auto-resume",
    );
}

#[test]
fn safety_net_action_aborted_when_engine_cancelled_the_signal_kill() {
    // A deliberate engine teardown (user Stop, shutdown, restart, eviction)
    // SIGKILLs the child — that is killed_by_signal, but engine_cancelled
    // gates it OUT of auto-resume. (In practice a deliberate cancel also
    // emits its own terminal so the safety net wouldn't fire; this pins the
    // gate regardless.)
    assert_eq!(
        safety_net_action(true, false, false, true, true),
        SafetyNetAction::EmitAbortedSafetyNet,
        "engine-initiated signal kill MUST NOT auto-resume",
    );
}

#[test]
fn safety_net_action_aborted_when_no_watchdog_no_external_terminal() {
    // Driver crash / EOF / parser glitch (no signal) — emit the red-dot
    // abort so the UI flips out of "running" and the user knows the work
    // didn't complete.
    assert_eq!(
        safety_net_action(true, false, false, false, false),
        SafetyNetAction::EmitAbortedSafetyNet,
        "non-watchdog non-signal safety net MUST emit ResponseAborted",
    );
}

/// A turn the backend closed itself on a transient upstream failure never
/// reaches the safety net (it emitted a real terminal), so the auto-resume is a
/// SEPARATE decision made from that terminal. These are the exact strings
/// Claude Code produced on 2026-08-04: both threads died on the first one.
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

/// Everything else stays a dead end, because resuming it would reproduce it.
/// The prose case is the reason this matches a PREFIX: a turn that merely
/// mentions an api error while explaining one is a successful turn.
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

/// No terminal other than a `Failed` is a candidate: `Generated` finished,
/// `Canceled` was the user's Stop, and `Aborted` already belongs to shutdown or
/// safety-net recovery.
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

/// The bound. Attempts below the cap resume; the one that would exceed it stops
/// and leaves the red dot standing, so a broken upstream surfaces instead of
/// looping. A query that could not answer reports the budget as spent, which
/// this same gate then refuses.
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

/// Two paths own the thread instead, and both must win over the auto-resume.
/// Shutdown: post-restart recovery re-adopts in-flight threads, and a
/// continuation emitted into a dying engine races it. Conflict resolution: the
/// session carries an apply's merge duty whose cleanup decides hand-off vs abort
/// from the watchdog's own flags, so an API drop mid-merge keeps today's
/// behavior (abort the merge, leave the change pending).
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

/// The new reason opens its own resume boundary, like every other automatic
/// resume. `answered_after_idle` stays out: that continuation continues an
/// existing exchange rather than starting a recovery.
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

/// **The regression test for the incident.** Every test above passes on a
/// version of this feature that can never run, and one shipped: the emit lived
/// only in the post-loop `finalize_direct_agent`, which a turn that ends with a
/// Result never reaches. It idles, and the idle-exit arm of the event loop
/// returns straight out of `run_direct_agent`. A reported `API Error` drop is
/// always that shape, so the recovery was dead through four consecutive
/// incidents while its whole unit table stayed green. See
/// `docs/plans/2026-08-05-api-drop-auto-resume-emit-site-unreachable.md`.
///
/// Asserted against the source text, like
/// `the_completion_path_removes_only_the_two_worktrees_it_is_allowed_to`,
/// because that is where the property lives. This is not a placeholder for a
/// behavioural test, and building the `run_direct_agent` harness the tree
/// currently lacks would not retire it: a behavioural test can only show that
/// the exits reachable *today* emit the continuation, and the defect here is a
/// path that reaches neither call site. The property is "every way out of the
/// run loop passes through the helper", which is a statement about the shape of
/// the function, so it is checked against the function's text.
#[test]
fn both_session_exits_reach_the_api_drop_auto_resume() {
    const RUN_SRC: &str = include_str!("../run_session/run.rs");
    const COMPLETION_SRC: &str = include_str!("../run_session/completion.rs");
    const CALL: &str = "self.maybe_auto_resume_after_api_error(";

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

    // The idle exit: `AgentEvent::Exited` while `is_waiting`, which is how every
    // turn that produced a Result ends. Bounded by the arm's own release log and
    // its `return`, so the call has to sit inside the arm and not merely
    // somewhere in the file.
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
        .find("self.clear_cc_debounce(thread_id);")
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

/// **The second regression test for the same incident, one layer up.** Reaching the
/// idle exit is necessary but not sufficient: the idle handler can decline to end
/// the run at all. When `terminate_decision` returns a `KeepAlive`, the loop keeps
/// the subprocess and continues, so `run_direct_agent` reaches neither call site the
/// test above pins. That is how a Claude Code turn which merged three instructions
/// into one Result reported two phantom follow-ups in flight and swallowed the
/// recovery on 2026-08-07, with the sibling test green throughout.
///
/// The fix was to stop guessing. Each of the three windows a follow-up can be in
/// gets a signal that is exact under the lock the decision already holds, so this
/// guards the two that a future edit could quietly turn back into a guess: the
/// channel read, and the per-backend settle. Same source-text rationale as its
/// sibling, and the same honest limit.
/// See `docs/plans/2026-08-07-api-drop-resume-suppressed-by-phantom-followup-count.md`.
#[test]
fn the_idle_keep_alive_cannot_be_fed_by_a_phantom_count() {
    const RUN_SRC: &str = include_str!("../run_session/run.rs");

    assert!(
        !RUN_SRC.contains("pending_followups"),
        "the phantom counter must stay deleted. It counted messages SENT and settled once \
         per Result, so a Claude Code turn that merged N inputs into one Result reported \
         N-1 follow-ups that had already been consumed.",
    );

    let exit_arm = RUN_SRC
        .find("IdleAction::ExitSubprocess => {")
        .expect("run.rs must still carry the idle-exit subprocess decision");
    let arm = &RUN_SRC[exit_arm..];
    let decision = arm
        .find("match terminate_decision(")
        .expect("the ExitSubprocess arm must still route through terminate_decision");
    let settle = arm.find("settle_inputs_awaiting_result(").expect(
        "the ExitSubprocess arm must settle the forwarded-input count through the shared \
         per-backend helper. Inlining the rule is how one backend's promise (Codex answers \
         one input per Result) got applied to the other (Claude Code answers all of them).",
    );
    assert!(
        settle < decision,
        "the settle must run before the decision reads its remainder",
    );
    // Scoped to the call's own argument list rather than the whole arm, so a
    // passing mention of `msg_rx` in a comment cannot satisfy the guard. (It did:
    // the comment above the lock names the read, and an arm-wide search found that
    // instead of the argument.)
    let args_end = arm[decision..]
        .find(") {")
        .expect("terminate_decision's argument list must be closed");
    assert!(
        arm[decision..decision + args_end].contains("msg_rx.len()"),
        "the channel depth must be an ARGUMENT of terminate_decision. `msg_rx` is the only \
         authoritative answer to `is a follow-up still unread`, and it is exact here because \
         this arm holds the same agent_sessions lock the fast-path send takes.",
    );

    // The ordering qualifier on the Claude Code merge rule. Without it the settle
    // trades the phantom for a dropped message, because `select!` can forward an
    // input and only then hand the loop a Result that predates it.
    assert!(
        RUN_SRC.contains("forwarded_input_unconfirmed = true;")
            && RUN_SRC.contains("agent_events_queued_at_forward = events_rx.len();")
            && RUN_SRC.contains("agent_event_may_predate_forward("),
        "the run loop must arm the forward-ordering state when it forwards an input, record how \
         many events were ALREADY queued at that moment, and advance it through \
         `agent_event_may_predate_forward` on every agent event. Dropping the queued-event count \
         is the buffered-event hole: a Result that sat in the channel the whole time then reads \
         as proof the agent accepted an input it has never seen.",
    );

    // The `Terminate` branch is what carries an API-drop turn to the idle exit the
    // sibling test pins. If it ever stops cancelling, the recovery goes with it.
    assert!(
        arm[decision..].contains("TerminateDecision::Terminate =>"),
        "terminate_decision's Terminate branch must still exist in this arm: it is the only \
         path from a transient API drop to the auto-resume emit site.",
    );
}
