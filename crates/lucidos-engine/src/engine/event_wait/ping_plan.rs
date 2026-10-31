//! The *ping plan*: how long a keep-alive run pings, from the thread's newest
//! expected wake (ADR 0603).
//!
//! - **The reread cap prices every plan.** It is the pings that together cost
//!   one reread. A plan runs only when its pings cost at most
//!   [`PLAN_SPEND_SHARE`] of it: the *saving margin*.
//! - **A point estimate pings to [`OVERRUN_FACTOR`] times the expected wait**,
//!   and at least one ping past it: the *overrun margin*.
//! - **A distribution pings to the horizon with the best expected saving**,
//!   under the same two margins.
//! - **Two caps hold whatever the estimate says.** No ping lands past an event
//!   wait's timeout, and no run pings past the reread cap from the wait start.
//! - **With no estimate the bare reread cap applies.**
//!
//! Every time here counts seconds from the wait start, so each tick reads the
//! same plan.

use std::collections::HashMap;
use std::sync::Mutex;

use chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::engine::thread_events::{PlannedWake, WakeBucket, WakeSource};

/// The *saving margin*: a plan's pings may cost at most this share of one
/// reread.
pub(crate) const PLAN_SPEND_SHARE: f64 = 0.8;

/// The *overrun margin*: a plan pings to this multiple of the expected wait.
pub(crate) const OVERRUN_FACTOR: f64 = 1.25;

/// Whether the engine asked again after a plan ran out with no wake. It asks
/// once per estimate, so a failed answer never repeats the call.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) enum ReEstimate {
    #[default]
    NotAsked,
    Asking,
    Asked,
}

/// The re-estimates asked for, by thread: the estimate each one replaces, and
/// how far the ask got. In memory only: a restart costs one more call.
#[derive(Debug, Default)]
pub(crate) struct ReEstimates(Mutex<HashMap<Uuid, (DateTime<Utc>, ReEstimate)>>);

impl ReEstimates {
    /// How far the re-estimate of the estimate stated at `stated_at` got.
    pub(super) fn of(&self, thread_id: Uuid, stated_at: DateTime<Utc>) -> ReEstimate {
        match self.0.lock().unwrap().get(&thread_id) {
            Some((asked_for, state)) if *asked_for == stated_at => *state,
            _ => ReEstimate::NotAsked,
        }
    }

    /// Claim the one re-estimate of the estimate stated at `stated_at`.
    /// `true` only for the first claim.
    pub(super) fn claim(&self, thread_id: Uuid, stated_at: DateTime<Utc>) -> bool {
        let mut asked = self.0.lock().unwrap();
        if asked
            .get(&thread_id)
            .is_some_and(|(at, _)| *at == stated_at)
        {
            return false;
        }
        asked.insert(thread_id, (stated_at, ReEstimate::Asking));
        true
    }

    /// The re-estimate came back, with a new estimate or none. A plan from the
    /// old estimate then stops.
    pub(super) fn finish(&self, thread_id: Uuid, stated_at: DateTime<Utc>) {
        if let Some((at, state)) = self.0.lock().unwrap().get_mut(&thread_id) {
            if *at == stated_at {
                *state = ReEstimate::Asked;
            }
        }
    }

    /// Forget every thread the keep-alive no longer weighs.
    pub(super) fn retain(&self, threads: &[Uuid]) {
        self.0
            .lock()
            .unwrap()
            .retain(|thread_id, _| threads.contains(thread_id));
    }
}

/// The thread's newest expected wake, as a plan reads it.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct WakeEstimate {
    /// When it was stated. It names the estimate a re-estimate replaces.
    pub stated_at: DateTime<Utc>,
    /// Seconds from the wait start to the statement. Negative when the turn
    /// stated it before it armed the wait.
    pub stated_secs: f64,
    /// Seconds from the statement to the expected wake.
    pub expected_secs: f64,
    /// Each bucket's start and end in seconds from the statement, and its
    /// share of the odds. Empty for a point estimate.
    pub buckets: Vec<(f64, f64, f64)>,
    pub shown: PlannedWake,
    pub asked_again: ReEstimate,
}

impl WakeEstimate {
    /// The estimate stated at `stated_at`, for a wait that began at
    /// `wait_began`. A distribution with no weight reads as a point.
    pub(super) fn new(
        stated_at: DateTime<Utc>,
        at: DateTime<Utc>,
        distribution: &[WakeBucket],
        source: WakeSource,
        wait_began: DateTime<Utc>,
        asked_again: ReEstimate,
    ) -> Self {
        let secs =
            |from: DateTime<Utc>, to: DateTime<Utc>| (to - from).num_milliseconds() as f64 / 1000.0;
        Self {
            stated_at,
            stated_secs: secs(wait_began, stated_at),
            expected_secs: secs(stated_at, at),
            buckets: shares(
                distribution
                    .iter()
                    .map(|b| (secs(stated_at, b.by), b.probability)),
            ),
            shown: PlannedWake { at, source },
            asked_again,
        }
    }
}

/// Buckets as `(start, end, share)`: each starts where the one before ended,
/// and the shares sum to one. Empty when nothing carries weight.
fn shares(ends: impl Iterator<Item = (f64, f64)>) -> Vec<(f64, f64, f64)> {
    let ends: Vec<(f64, f64)> = ends.filter(|(_, p)| p.is_finite() && *p >= 0.0).collect();
    let total: f64 = ends.iter().map(|(_, p)| p).sum();
    if total <= 0.0 {
        return Vec::new();
    }
    let starts = std::iter::once(0.0).chain(ends.iter().map(|(end, _)| *end));
    starts
        .zip(&ends)
        .map(|(start, (end, p))| (start, *end, p / total))
        .collect()
}

/// What ends a run's pings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PlanEnd {
    /// The reread cap from the wait start: the bare fallback, and the safety
    /// cap under every plan.
    RereadCap,
    /// The event wait's own timeout, which wakes the thread.
    Timeout,
    /// The horizon the expected wake planned.
    Estimate,
    /// The expected wake lies past what pings may cost, so none go out.
    WakePastCap,
}

/// How long a run pings, in seconds from the wait start, and why it stops
/// there. A [`PlanEnd::WakePastCap`] plan ends where its estimate came.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct Plan {
    pub give_up_after_secs: f64,
    pub end: PlanEnd,
}

/// The plan of a wait on events or working children. `cap_beats` is the reread
/// cap, `every` the seconds between pings, and `timeout_secs` when the
/// earliest event wait times out.
pub(super) fn plan_wait(
    cap_beats: f64,
    every: f64,
    timeout_secs: Option<f64>,
    wake: Option<&WakeEstimate>,
) -> Plan {
    let safety = cap_beats * every;
    let ceiling = match timeout_secs {
        Some(timeout) if timeout < safety => Plan {
            give_up_after_secs: timeout,
            end: PlanEnd::Timeout,
        },
        _ => Plan {
            give_up_after_secs: safety,
            end: PlanEnd::RereadCap,
        },
    };
    let Some(wake) = wake else {
        return ceiling;
    };
    let limits = Limits {
        cap_beats,
        every,
        room: safety - wake.stated_secs,
        timeout: timeout_secs.map(|timeout| timeout - wake.stated_secs),
    };
    match limits.horizon(wake) {
        Some(horizon) if wake.stated_secs + horizon < ceiling.give_up_after_secs => Plan {
            give_up_after_secs: wake.stated_secs + horizon,
            end: PlanEnd::Estimate,
        },
        Some(_) => ceiling,
        None => Plan {
            give_up_after_secs: wake.stated_secs.max(0.0),
            end: PlanEnd::WakePastCap,
        },
    }
}

/// What bounds a plan, in seconds from its statement.
struct Limits {
    cap_beats: f64,
    every: f64,
    /// The time left before the safety cap.
    room: f64,
    timeout: Option<f64>,
}

impl Limits {
    fn horizon(&self, wake: &WakeEstimate) -> Option<f64> {
        if wake.buckets.is_empty() {
            self.covering(wake.expected_secs)
        } else {
            self.best(&wake.buckets)
        }
    }

    /// The horizon that covers a wake `expected` seconds away under both
    /// margins. `None` when that wake lies past the safety cap, or its pings
    /// cost more than the saving margin allows.
    fn covering(&self, expected: f64) -> Option<f64> {
        let expected = self.timeout.map_or(expected, |t| expected.min(t));
        if expected > self.room {
            return None;
        }
        let horizon = (expected * OVERRUN_FACTOR).max(expected + self.every);
        let horizon = self.timeout.map_or(horizon, |t| horizon.min(t));
        (horizon <= PLAN_SPEND_SHARE * self.cap_beats * self.every).then_some(horizon)
    }

    /// The covering horizon of the bucket end with the best expected saving.
    /// `None` when no bucket end saves anything.
    fn best(&self, buckets: &[(f64, f64, f64)]) -> Option<f64> {
        let mut best: Option<(f64, f64)> = None;
        for horizon in buckets.iter().filter_map(|(_, end, _)| self.covering(*end)) {
            let saving = self.saving(buckets, horizon);
            if saving > 0.0 && best.is_none_or(|(_, top)| saving > top) {
                best = Some((horizon, saving));
            }
        }
        best.map(|(horizon, _)| horizon)
    }

    /// The expected saving of pinging for `horizon`, in pings: the reread
    /// times the odds the wake finds the cache warm, less the pings sent.
    fn saving(&self, buckets: &[(f64, f64, f64)], horizon: f64) -> f64 {
        let warm = if self.timeout.is_some_and(|t| horizon >= t) {
            1.0
        } else {
            buckets
                .iter()
                .map(|(start, end, share)| share * uniform_cdf(*start, *end, horizon))
                .sum()
        };
        let pinged: f64 = buckets
            .iter()
            .map(|(start, end, share)| share * uniform_capped_mean(*start, *end, horizon))
            .sum();
        self.cap_beats * warm - pinged / self.every
    }
}

/// The odds a wake spread evenly from `start` to `end` comes by `x`.
fn uniform_cdf(start: f64, end: f64, x: f64) -> f64 {
    if end <= start {
        return f64::from(u8::from(x >= end));
    }
    ((x - start) / (end - start)).clamp(0.0, 1.0)
}

/// The mean of `min(wake, cap)` for a wake spread evenly from `start` to `end`.
fn uniform_capped_mean(start: f64, end: f64, cap: f64) -> f64 {
    if cap <= start || end <= start {
        return cap.min(start);
    }
    if cap >= end {
        return (start + end) / 2.0;
    }
    let width = end - start;
    (cap * cap - start * start) / 2.0 / width + cap * (end - cap) / width
}

#[cfg(test)]
#[path = "ping_plan_tests.rs"]
mod tests;
