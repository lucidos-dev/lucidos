//! How long this workspace's keep-alive waits ran, and the give-up point that
//! would have cost them least. `cache_keep_alive = workspace-data` reads it.
//!
//! - **Costs count in beats.** A rewrite costs `rewrite_in_beats`, so one
//!   duration list serves every model and TTL.
//! - **A cap never passes twice the break-even budget**, even when every wait
//!   runs forever (I2).
//! - **The lists live in memory**, rebuilt from the events table every hour.
//!   A restart rebuilds them, so no state lives only in memory.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::keep_alive::{KeepWarmKinds, ParkedCard, WaitType};
use crate::engine::LucidosEngine;
use crate::runtime::CodingAgent;

/// Fewer waits of a type than this, and the type keeps the default rule.
pub(super) const MIN_SAMPLES: usize = 50;

/// How far back the durations reach.
const WINDOW_DAYS: i32 = 30;

/// How often the in-memory lists are rebuilt.
const REFRESH_EVERY: Duration = Duration::from_secs(3600);

/// The cap, in beats, that costs least over `durations`. A wait of `d`
/// seconds needs `floor(d / beat_every_secs)` beats. Past the cap it pays a
/// rewrite instead. A tie takes the shorter cap.
pub(super) fn cheapest_cap(durations: &[f64], beat_every_secs: f64, rewrite_in_beats: f64) -> u32 {
    let needed: Vec<f64> = durations
        .iter()
        .map(|d| (d / beat_every_secs).floor())
        .collect();
    let cost = |cap: u32| {
        let cap = f64::from(cap);
        let (beats, rewrites) = needed.iter().fold((0.0, 0.0), |(beats, rewrites), &n| {
            if n > cap {
                (beats + cap, rewrites + 1.0)
            } else {
                (beats + n, rewrites)
            }
        });
        beats + rewrites * rewrite_in_beats
    };
    let longest = (2.0 * rewrite_in_beats).floor().max(0.0) as u32;
    let mut best = (0, cost(0));
    for cap in 1..=longest {
        let spent = cost(cap);
        if spent < best.1 {
            best = (cap, spent);
        }
    }
    best.0
}

/// The durations of this workspace's finished waits, in seconds, by type.
#[derive(Debug, Default)]
pub(crate) struct WaitDurations {
    by_type: HashMap<WaitType, Vec<f64>>,
}

impl WaitDurations {
    pub(super) fn from_samples(samples: impl IntoIterator<Item = (WaitType, f64)>) -> Self {
        let mut by_type: HashMap<WaitType, Vec<f64>> = HashMap::new();
        for (wait_type, secs) in samples {
            by_type.entry(wait_type).or_default().push(secs);
        }
        Self { by_type }
    }

    /// The tuned cap for `wait_type`, or `None` under [`MIN_SAMPLES`] waits.
    pub(super) fn cap_beats(
        &self,
        wait_type: WaitType,
        beat_every_secs: f64,
        rewrite_in_beats: f64,
    ) -> Option<u32> {
        let durations = self
            .by_type
            .get(&wait_type)
            .filter(|d| d.len() >= MIN_SAMPLES)?;
        Some(cheapest_cap(durations, beat_every_secs, rewrite_in_beats))
    }
}

/// The finished waits of Claude Code threads since `$1` days ago: a card from
/// ask to its first answer, an event wait from its start to its resolution.
/// `kinds` lists what an event wait watched, and is empty for a card.
const DURATIONS_SQL: &str = r#"
WITH cc AS (SELECT thread_id FROM thread_summaries WHERE coding_agent = $2)
SELECT s.event_type AS started,
       ARRAY[]::text[] AS kinds,
       EXTRACT(EPOCH FROM a.created - s.created)::float8 AS secs
  FROM events s
  JOIN LATERAL (
       SELECT e.created FROM events e
        WHERE e.thread_id = s.thread_id
          AND e.sequence > s.sequence
          AND e.event_type = CASE s.event_type
                WHEN 'UserQuestionAsked' THEN 'UserQuestionAnswered'
                ELSE 'CodingAgentPermissionResolved' END
          AND e.payload->>(CASE s.event_type
                WHEN 'UserQuestionAsked' THEN 'tool_use_id' ELSE 'request_id' END)
            = s.payload->>(CASE s.event_type
                WHEN 'UserQuestionAsked' THEN 'tool_use_id' ELSE 'request_id' END)
        ORDER BY e.sequence LIMIT 1) a ON true
 WHERE s.event_type IN ('UserQuestionAsked', 'CodingAgentPermissionRequest')
   AND s.created >= now() - make_interval(days => $1)
   AND s.thread_id IN (SELECT thread_id FROM cc)
UNION ALL
SELECT s.event_type,
       ARRAY(SELECT jsonb_array_elements(s.payload->'on')->>'event_type'),
       EXTRACT(EPOCH FROM r.created - s.created)::float8
  FROM events s
  JOIN LATERAL (
       SELECT e.created FROM events e
        WHERE e.thread_id = s.thread_id
          AND e.sequence > s.sequence
          AND e.event_type IN ('EventWaitDelivered', 'EventWaitExpired', 'EventWaitCanceled')
          AND e.payload->>'wait_id' = s.payload->>'wait_id'
        ORDER BY e.sequence LIMIT 1) r ON true
 WHERE s.event_type = 'EventWaitStarted'
   AND s.created >= now() - make_interval(days => $1)
   AND s.thread_id IN (SELECT thread_id FROM cc)
"#;

#[derive(sqlx::FromRow)]
struct DurationRow {
    started: String,
    kinds: Vec<String>,
    secs: f64,
}

impl DurationRow {
    /// The wait's type. `None` for an event wait that watched a kind the
    /// keep-alive never keeps warm.
    fn wait_type(&self) -> Option<WaitType> {
        match ParkedCard::of_event_type(&self.started) {
            Some(card) => Some(WaitType::Parked(card)),
            None => KeepWarmKinds::of(self.kinds.iter().map(String::as_str)).map(WaitType::Events),
        }
    }
}

pub(super) async fn read_wait_durations(pool: &sqlx::PgPool) -> Result<WaitDurations, sqlx::Error> {
    let rows: Vec<DurationRow> = sqlx::query_as(DURATIONS_SQL)
        .bind(WINDOW_DAYS)
        .bind(CodingAgent::ClaudeCode.as_str())
        .fetch_all(pool)
        .await?;
    Ok(WaitDurations::from_samples(
        rows.iter()
            .filter_map(|row| Some((row.wait_type()?, row.secs))),
    ))
}

/// The durations the tick reads, and when they were last rebuilt.
#[derive(Default)]
pub(crate) struct WaitDurationsCache(Mutex<CachedDurations>);

#[derive(Default)]
struct CachedDurations {
    durations: Arc<WaitDurations>,
    refreshed_at: Option<Instant>,
}

impl WaitDurationsCache {
    pub(super) fn current(&self) -> Arc<WaitDurations> {
        self.0.lock().unwrap().durations.clone()
    }

    /// Whether a rebuild is due. A `true` claims it, so one tick starts it.
    fn claim_refresh(&self, now: Instant) -> bool {
        let mut cached = self.0.lock().unwrap();
        let due = cached
            .refreshed_at
            .is_none_or(|at| now.duration_since(at) >= REFRESH_EVERY);
        if due {
            cached.refreshed_at = Some(now);
        }
        due
    }

    fn store(&self, durations: WaitDurations) {
        self.0.lock().unwrap().durations = Arc::new(durations);
    }
}

impl LucidosEngine {
    /// Rebuild the wait durations in the background once an hour. A failed
    /// read keeps the old lists until the next hour.
    pub(super) fn refresh_wait_durations_when_due(&self) {
        if !self.keep_alive_durations.claim_refresh(Instant::now()) {
            return;
        }
        let pool = self.pool.clone();
        let cache = self.keep_alive_durations.clone();
        tokio::spawn(async move {
            match read_wait_durations(&pool).await {
                Ok(durations) => cache.store(durations),
                Err(e) => crate::log!("[KeepAlive] Could not read the wait durations: {e}"),
            }
        });
    }
}

#[cfg(test)]
#[path = "wait_durations_tests.rs"]
mod tests;
