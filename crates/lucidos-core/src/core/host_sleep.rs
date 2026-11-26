//! INVARIANT: Did this computer sleep? Read off two clocks, with no OS API.
//!
//! The wall clock (`SystemTime`) keeps running while the machine sleeps. The
//! monotonic clock (`Instant`) stops on both platforms we ship: macOS reads
//! `CLOCK_UPTIME_RAW` and Linux reads `CLOCK_MONOTONIC`. So when wall time gains
//! on monotonic time over a span, the host slept inside it.
//!
//! On Windows `Instant` counts through sleep, so only the late-tick sign would
//! work there. The engine does not build for Windows today. Reasoning:
//! `docs/adr/0367-a-sleeping-host-is-not-a-dead-ingress.md`.
//!
//! Everything here is pure, apart from [`ClockReading::now`].

use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub const CLOCK_GAP_TOLERANCE: Duration = Duration::from_secs(30);

pub const LATE_TICK_TOLERANCE: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, Copy)]
pub struct ClockReading {
    pub wall: SystemTime,
    pub mono: Instant,
}

impl ClockReading {
    pub fn now() -> Self {
        Self {
            wall: SystemTime::now(),
            mono: Instant::now(),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct RoundClocks {
    pub previous_end: Option<ClockReading>,
    pub start: ClockReading,
    pub end: ClockReading,
    pub period: Duration,
}

/// INVARIANT: How long the host slept during this round or since the one before it.
///
/// `None` means it stayed awake. Any one of three signs is enough:
///
/// - the wall clock gained on the monotonic clock during the round;
/// - it gained between the previous round's end and this one's start;
/// - the tick arrived more than [`LATE_TICK_TOLERANCE`] after its plan.
///
/// The duration is a lower bound. With a previous round, the gap measures the
/// sleep before this one exactly. That round may itself have slept past this
/// tick, so its lateness would count those minutes twice. Without one, the
/// lateness stands in: the host was asleep from the planned instant on.
pub fn slept(round: &RoundClocks) -> Option<Duration> {
    let during = clock_gap(round.start, round.end);
    let late = Some(tick_lateness(round.start.wall, round.period))
        .filter(|late| *late > LATE_TICK_TOLERANCE);
    let before = match round.previous_end {
        Some(previous) => clock_gap(previous, round.start),
        None => late,
    };

    if before.is_none() && during.is_none() && late.is_none() {
        return None;
    }
    Some(before.unwrap_or_default() + during.unwrap_or_default())
}

fn clock_gap(earlier: ClockReading, later: ClockReading) -> Option<Duration> {
    let wall = later.wall.duration_since(earlier.wall).ok()?;
    let mono = later.mono.saturating_duration_since(earlier.mono);
    wall.checked_sub(mono)
        .filter(|gap| *gap > CLOCK_GAP_TOLERANCE)
}

fn tick_lateness(start: SystemTime, period: Duration) -> Duration {
    let since_epoch = start.duration_since(UNIX_EPOCH).unwrap_or_default();
    match period.as_secs() {
        0 => Duration::ZERO,
        period => Duration::from_secs(since_epoch.as_secs() % period),
    }
}

#[cfg(test)]
#[path = "host_sleep_tests.rs"]
mod tests;
