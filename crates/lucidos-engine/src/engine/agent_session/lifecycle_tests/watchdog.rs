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
        !should_trigger_watchdog(false, now - limit, now, limit, WATCHDOG_HUNG_TOOL_CEILING_MS, 0),
        "exactly at the limit must not fire (strict-greater check)"
    );
    assert!(
        should_trigger_watchdog(false, now - limit - 1, now, limit, WATCHDOG_HUNG_TOOL_CEILING_MS, 0),
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
