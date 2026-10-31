//! How long this workspace's keep-alive waits ran. Every warming status shows
//! its [`CapOdds`], and an `await_event` result its [`WaitHistory`].
//!
//! - **The lists live in memory**, rebuilt from the events table every hour.
//!   A restart rebuilds them, so no state lives only in memory.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::keep_alive::{KeepWarmKinds, ParkedCard, WaitAgent, WaitType};
use crate::engine::expected_wake::describe_span;
use crate::engine::thread_events::CapOdds;
use crate::engine::LucidosEngine;
use crate::runtime::CodingAgent;

/// How far back the durations reach.
const WINDOW_DAYS: i32 = 30;

/// How often the in-memory lists are rebuilt.
const REFRESH_EVERY: Duration = Duration::from_secs(3600);

impl CapOdds {
    /// How `durations` end against a cap of `cap` beats. A wait of `d` seconds
    /// needs `floor(d / beat_every_secs)` beats.
    pub(super) fn of(durations: &[f64], beat_every_secs: f64, cap: u32) -> Self {
        let mut odds = Self {
            samples: u32::try_from(durations.len()).unwrap_or(u32::MAX),
            slots: vec![0; cap as usize + 1],
            after_cap: 0,
        };
        for d in durations {
            let needed = (d / beat_every_secs).floor().max(0.0);
            if needed <= f64::from(cap) {
                odds.slots[needed as usize] += 1;
            } else {
                odds.after_cap += 1;
            }
        }
        odds
    }
}

/// The durations of this workspace's finished waits, in seconds, by agent and
/// type.
#[derive(Debug, Default)]
pub(crate) struct WaitDurations {
    by_type: HashMap<(WaitAgent, WaitType), Vec<f64>>,
}

impl WaitDurations {
    pub(super) fn from_samples(
        samples: impl IntoIterator<Item = (WaitAgent, WaitType, f64)>,
    ) -> Self {
        let mut by_type: HashMap<(WaitAgent, WaitType), Vec<f64>> = HashMap::new();
        for (agent, wait_type, secs) in samples {
            by_type.entry((agent, wait_type)).or_default().push(secs);
        }
        Self { by_type }
    }

    /// How `agent`'s past waits of `wait_type` end against a cap of `cap`
    /// beats, or `None` with no wait to count.
    pub(super) fn odds(
        &self,
        agent: WaitAgent,
        wait_type: WaitType,
        beat_every_secs: f64,
        cap: u32,
    ) -> Option<CapOdds> {
        let durations = self
            .by_type
            .get(&(agent, wait_type))
            .filter(|d| !d.is_empty())?;
        Some(CapOdds::of(durations, beat_every_secs, cap))
    }

    /// How long `agent`'s past waits of `wait_type` lasted, or `None` with no
    /// wait to count.
    pub(super) fn history(&self, agent: WaitAgent, wait_type: WaitType) -> Option<WaitHistory> {
        WaitHistory::of(self.by_type.get(&(agent, wait_type))?)
    }
}

/// The share of past waits [`WaitHistory::high_secs`] covers.
const HIGH_QUANTILE: f64 = 0.8;

/// How long past waits of one agent and type lasted: the figures the agent
/// reads before it states an expected wake.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct WaitHistory {
    pub samples: usize,
    pub median_secs: f64,
    /// The wait [`HIGH_QUANTILE`] of them ended within.
    pub high_secs: f64,
}

impl WaitHistory {
    fn of(durations: &[f64]) -> Option<Self> {
        let mut sorted: Vec<f64> = durations
            .iter()
            .copied()
            .filter(|d| d.is_finite())
            .collect();
        if sorted.is_empty() {
            return None;
        }
        sorted.sort_by(f64::total_cmp);
        let nearest_rank = |q: f64| sorted[((q * sorted.len() as f64).ceil() as usize).max(1) - 1];
        Some(Self {
            samples: sorted.len(),
            median_secs: nearest_rank(0.5),
            high_secs: nearest_rank(HIGH_QUANTILE),
        })
    }

    /// The figures as one sentence.
    pub(crate) fn describe(&self) -> String {
        format!(
            "Past waits like this here, last {WINDOW_DAYS} days: {}. Half ended within {}, \
             {}% within {}.",
            self.samples,
            describe_span(self.median_secs),
            (HIGH_QUANTILE * 100.0).round(),
            describe_span(self.high_secs),
        )
    }
}

/// The finished waits of Claude Code and Lucidos Agent threads since `$1`
/// days ago: a card from ask to its first answer, an event wait from its
/// start to its resolution. `kinds` lists what an event wait watched, and is
/// empty for a card. `is_coding_agent` tells the two agents apart.
const DURATIONS_SQL: &str = r#"
WITH kept AS (SELECT thread_id, is_coding_agent FROM thread_summaries
               WHERE NOT is_coding_agent OR coding_agent IS DISTINCT FROM $2)
SELECT k.is_coding_agent,
       s.event_type AS started,
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
  JOIN kept k ON k.thread_id = s.thread_id
 WHERE s.event_type IN ('UserQuestionAsked', 'CodingAgentPermissionRequest')
   AND s.created >= now() - make_interval(days => $1)
UNION ALL
SELECT k.is_coding_agent,
       s.event_type,
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
  JOIN kept k ON k.thread_id = s.thread_id
 WHERE s.event_type = 'EventWaitStarted'
   AND s.created >= now() - make_interval(days => $1)
"#;

#[derive(sqlx::FromRow)]
struct DurationRow {
    is_coding_agent: bool,
    started: String,
    kinds: Vec<String>,
    secs: f64,
}

impl DurationRow {
    fn agent(&self) -> WaitAgent {
        if self.is_coding_agent {
            WaitAgent::ClaudeCode
        } else {
            WaitAgent::Lucidos
        }
    }

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
        .bind(CodingAgent::Codex.as_str())
        .fetch_all(pool)
        .await?;
    Ok(WaitDurations::from_samples(rows.iter().filter_map(|row| {
        Some((row.agent(), row.wait_type()?, row.secs))
    })))
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
