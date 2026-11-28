use std::collections::HashMap;
use std::sync::Mutex;

use chrono::{DateTime, Utc};
use uuid::Uuid;

use super::wait_durations::LearnedCap;
use crate::engine::thread_events::{CapSource, PlannedWake, WakeBucket, WakeSource};

pub(crate) const PLAN_SPEND_SHARE: f64 = 0.8;

pub(crate) const OVERRUN_FACTOR: f64 = 1.25;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) enum ReEstimate {
    #[default]
    NotAsked,
    Asking,
    Asked,
}

#[derive(Debug, Default)]
pub(crate) struct ReEstimates(Mutex<HashMap<Uuid, (DateTime<Utc>, ReEstimate)>>);

impl ReEstimates {
    pub(super) fn of(&self, thread_id: Uuid, stated_at: DateTime<Utc>) -> ReEstimate {
        match self.0.lock().unwrap().get(&thread_id) {
            Some((asked_for, state)) if *asked_for == stated_at => *state,
            _ => ReEstimate::NotAsked,
        }
    }

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

    pub(super) fn finish(&self, thread_id: Uuid, stated_at: DateTime<Utc>) {
        if let Some((at, state)) = self.0.lock().unwrap().get_mut(&thread_id) {
            if *at == stated_at {
                *state = ReEstimate::Asked;
            }
        }
    }

    pub(super) fn retain(&self, threads: &[Uuid]) {
        self.0
            .lock()
            .unwrap()
            .retain(|thread_id, _| threads.contains(thread_id));
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct WakeEstimate {
    pub stated_at: DateTime<Utc>,
    pub stated_secs: f64,
    pub expected_secs: f64,
    pub buckets: Vec<(f64, f64, f64)>,
    pub shown: PlannedWake,
    pub asked_again: ReEstimate,
}

impl WakeEstimate {
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PlanEnd {
    HalfReread,
    Learned(LearnedCap),
    RereadCap,
    Timeout,
    Estimate(WakeSource),
    WakePastCap(WakeSource),
}

impl PlanEnd {
    pub(super) fn cap_source(self) -> CapSource {
        match self {
            Self::HalfReread => CapSource::HalfReread,
            Self::Learned(cap) => CapSource::Learned {
                pings: cap.pings,
                samples: cap.samples,
                from_hour: cap.hours.from,
                to_hour: cap.hours.to,
                utc_offset_mins: cap.utc_offset_mins,
            },
            Self::RereadCap => CapSource::RereadCap,
            Self::Timeout => CapSource::Timeout,
            Self::Estimate(by) | Self::WakePastCap(by) => CapSource::Estimate { by },
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct Plan {
    pub give_up_after_secs: f64,
    pub end: PlanEnd,
}

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
            end: PlanEnd::Estimate(wake.shown.source),
        },
        Some(_) => ceiling,
        None => Plan {
            give_up_after_secs: wake.stated_secs.max(0.0),
            end: PlanEnd::WakePastCap(wake.shown.source),
        },
    }
}

struct Limits {
    cap_beats: f64,
    every: f64,
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

    fn covering(&self, expected: f64) -> Option<f64> {
        let expected = self.timeout.map_or(expected, |t| expected.min(t));
        if expected > self.room {
            return None;
        }
        let horizon = (expected * OVERRUN_FACTOR).max(expected + self.every);
        let horizon = self.timeout.map_or(horizon, |t| horizon.min(t));
        (horizon <= PLAN_SPEND_SHARE * self.cap_beats * self.every).then_some(horizon)
    }

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

fn uniform_cdf(start: f64, end: f64, x: f64) -> f64 {
    if end <= start {
        return f64::from(u8::from(x >= end));
    }
    ((x - start) / (end - start)).clamp(0.0, 1.0)
}

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
