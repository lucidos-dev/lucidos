//! How long an apply's slow phases usually take in this workspace, and when
//! the phase in flight began. Design: `docs/plans/2026-09-27-apply-time-estimate.md`.
//!
//! Two phases run a whole coding-agent session, so they alone are worth an
//! estimate: a hardening and a conflict resolution. A plain merge takes seconds.

use sqlx::PgPool;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// How far back a run can count.
const WINDOW_DAYS: i32 = 90;
/// A run's weight halves for every this many seconds of age, so the estimate
/// follows the process as it gets faster.
const HALF_LIFE_SECS: f64 = 14.0 * 86_400.0;
/// Fewer runs than this give no estimate at all.
const MIN_RUNS: usize = 5;
/// A run longer than this sat parked, and measures the user, not the phase.
const MAX_RUN_SECS: f64 = 4.0 * 3_600.0;
/// How long a computed estimate is served before a background refresh.
const FRESH_FOR: Duration = Duration::from_secs(600);

pub use crate::engine::thread_events::apply::{ApplyEstimates, PhaseEstimate};

/// One completed phase run.
#[derive(Debug, Clone, Copy)]
pub(crate) struct PhaseRun {
    pub age_secs: f64,
    pub duration_secs: f64,
}

/// The recency-weighted median of `runs`, once enough of them count.
pub(crate) fn estimate(runs: &[PhaseRun]) -> Option<PhaseEstimate> {
    let mut weighted: Vec<(f64, f64)> = runs
        .iter()
        .filter(|r| (0.0..=MAX_RUN_SECS).contains(&r.duration_secs))
        .map(|r| {
            (
                r.duration_secs,
                0.5_f64.powf(r.age_secs.max(0.0) / HALF_LIFE_SECS),
            )
        })
        .collect();
    if weighted.len() < MIN_RUNS {
        return None;
    }
    weighted.sort_by(|a, b| a.0.total_cmp(&b.0));
    let half = weighted.iter().map(|(_, w)| w).sum::<f64>() / 2.0;
    let mut seen = 0.0;
    let median = weighted
        .iter()
        .find(|(_, w)| {
            seen += w;
            seen >= half
        })
        .or(weighted.last())
        .map(|(d, _)| *d)?;
    Some(PhaseEstimate {
        typical_secs: median.round() as u64,
        runs: weighted.len(),
    })
}

#[cfg(test)]
use crate::core::changes::open_hardening_starts;
use crate::core::changes::{HARDENING_ENDS, HARDENING_START};

/// The events that end a conflict resolution, the first counting as success.
/// A second `MergeConflictDetected` restarts the resolution.
const CONFLICT_ENDS: &str =
    "'ChangeApplied','ChangeApplyFailed','ChangeDiscarded','MergeConflictDetected'";

/// Completed runs of both phases inside the window, as `(phase start event,
/// run)`. A run counts only when it ended in success.
async fn completed_runs(pool: &PgPool) -> sqlx::Result<Vec<(String, PhaseRun)>> {
    let rows: Vec<(String, f64, f64)> = sqlx::query_as(&format!(
        "SELECT s.event_type, \
                EXTRACT(EPOCH FROM now() - s.created)::float8, \
                EXTRACT(EPOCH FROM e.created - s.created)::float8 \
         FROM events s \
         JOIN LATERAL ( \
             SELECT x.event_type, x.created FROM events x \
             WHERE x.aggregate_id = s.aggregate_id AND x.sequence > s.sequence \
               AND x.event_type = ANY(CASE s.event_type \
                   WHEN 'MergeConflictDetected' THEN ARRAY[{CONFLICT_ENDS}] \
                   ELSE ARRAY[{HARDENING_ENDS}] END) \
             ORDER BY x.sequence LIMIT 1 \
         ) e ON TRUE \
         WHERE s.event_type IN ('MergeConflictDetected', '{HARDENING_START}') \
           AND s.created >= now() - make_interval(days => $1) \
           AND e.event_type = CASE s.event_type \
               WHEN 'MergeConflictDetected' THEN 'ChangeApplied' ELSE 'ChangeHardened' END"
    ))
    .bind(WINDOW_DAYS)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(phase, age_secs, duration_secs)| {
            (
                phase,
                PhaseRun {
                    age_secs,
                    duration_secs,
                },
            )
        })
        .collect())
}

/// Compute both estimates from the events table.
pub(crate) async fn compute(pool: &PgPool) -> sqlx::Result<ApplyEstimates> {
    let runs = completed_runs(pool).await?;
    let of = |phase: &str| -> Vec<PhaseRun> {
        runs.iter()
            .filter(|(p, _)| p == phase)
            .map(|(_, r)| *r)
            .collect()
    };
    Ok(ApplyEstimates {
        hardening: estimate(&of(HARDENING_START)),
        resolving_conflict: estimate(&of("MergeConflictDetected")),
    })
}

/// The last computed estimates, served stale while a refresh runs.
#[derive(Default)]
pub struct ApplyEstimateCache {
    state: Mutex<CacheState>,
}

#[derive(Default)]
struct CacheState {
    computed: Option<(ApplyEstimates, Instant)>,
    refreshing: bool,
}

impl ApplyEstimateCache {
    /// The estimates to serve now. With none computed yet, they are computed
    /// inline and a failure goes to the caller. After that a stale value is
    /// served as it is and refreshed in the background, so a changes broadcast
    /// never waits on the history query.
    pub async fn current(self: &Arc<Self>, pool: &PgPool) -> sqlx::Result<ApplyEstimates> {
        let served = {
            let mut state = self.state.lock().expect("estimate cache");
            let served = state.computed.map(|(value, _)| value);
            let stale = state
                .computed
                .is_some_and(|(_, at)| at.elapsed() >= FRESH_FOR);
            if stale && !state.refreshing {
                state.refreshing = true;
                let cache = Arc::clone(self);
                let pool = pool.clone();
                tokio::spawn(async move { cache.refresh_in_background(&pool).await });
            }
            served
        };
        match served {
            Some(value) => Ok(value),
            None => {
                let value = compute(pool).await?;
                self.state.lock().expect("estimate cache").computed = Some((value, Instant::now()));
                Ok(value)
            }
        }
    }

    /// Replace a stale value. This task has no caller to hand a failure to, so
    /// it logs it and keeps serving the stale value, and the next read retries.
    async fn refresh_in_background(&self, pool: &PgPool) {
        let computed = compute(pool).await;
        let mut state = self.state.lock().expect("estimate cache");
        state.refreshing = false;
        match computed {
            Ok(value) => state.computed = Some((value, Instant::now())),
            Err(e) => log_legacy!("[ApplyEstimate] refreshing phase estimates failed: {}", e),
        }
    }
}

#[cfg(test)]
#[path = "apply_estimate_tests.rs"]
mod tests;
