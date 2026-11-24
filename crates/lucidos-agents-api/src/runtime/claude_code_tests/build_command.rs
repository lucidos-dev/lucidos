use super::build_command_test_support::byte_idle_deadline;

#[test]
fn build_command_raises_cc_byte_idle_deadline_past_the_engine_watchdog() {
    // The ordering IS the fix: whichever deadline is shorter decides what a
    // provider stall costs. Engine watchdog first => kill plus auto-resume via
    // ContinuationRequested. CC watchdog first => dead thread. Asserted against
    // the real constant so the two cannot drift into the wrong order.
    let ms = byte_idle_deadline();
    // BOTH engine silence detectors, not just the in-loop one: the external
    // watchdog is the out-of-loop backstop for a session the run loop lost
    // track of, and its decision is `Resume` / `ResumeIfRunning`, equally
    // non-destructive. CC's deadline has to be outside whichever of the two
    // fires last, or the destructive handler wins again for that case.
    for (name, engine_limit) in [
        (
            "in-loop inactivity watchdog",
            crate::engine::agent_session::lifecycle::WATCHDOG_INACTIVITY_LIMIT_MS,
        ),
        (
            "external watchdog",
            crate::engine::agent_session::external_watchdog::EXTERNAL_WATCHDOG_LIMIT_MS,
        ),
    ] {
        assert!(
            ms > engine_limit,
            "CC's byte-idle deadline ({ms}ms) must exceed the engine's {name} \
             ({engine_limit}ms) so a provider stall auto-resumes instead of failing the thread"
        );
    }
    // CC clamps the value to [10_000, 1_800_000]; anything above the ceiling is
    // silently reduced, so sending more would misrepresent the effective deadline.
    assert!(
        (10_000..=1_800_000).contains(&ms),
        "CC clamps this env var to [10000, 1800000]ms; {ms}ms would not survive the clamp"
    );
}
