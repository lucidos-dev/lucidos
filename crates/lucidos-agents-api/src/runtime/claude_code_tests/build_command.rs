use super::build_command_test_support::byte_idle_deadline;

#[test]
fn build_command_raises_cc_byte_idle_deadline_past_the_engine_watchdog() {
    let ms = byte_idle_deadline();
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
    assert!(
        (10_000..=1_800_000).contains(&ms),
        "CC clamps this env var to [10000, 1800000]ms; {ms}ms would not survive the clamp"
    );
}
