//! How long this workspace's keep-alive waits ran. Every warming status shows
//! its [`CapOdds`], and an `await_event` result its [`WaitHistory`]. A parked
//! card's cap is learned from the cards asked near its hour
//! ([`WaitDurations::learned_cap`]).
//!
//! - **The lists live in memory**, rebuilt from the events table every hour
//!   and whenever the `timezone` preference changes. A restart rebuilds them,
//!   so no state lives only in memory.
//! - **Each wait keeps the local hour and day it began**, in the preference
//!   zone ([`LocalStart`]).
//! - **A card's wait ends when the cache is next read**: at the first real
//!   request after the card closes. An answer resumes the turn at once. A
//!   dismissed card waits on the user's next message, or forever. An allowed
//!   permission card ends at the allow: its tool runs before the next request.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use chrono::{DateTime, NaiveDate, Offset, TimeZone, Timelike, Utc};
use chrono_tz::Tz;

use super::keep_alive::{KeepWarmKinds, ParkedCard, WaitAgent, WaitType};
use crate::engine::expected_wake::describe_span;
use crate::engine::thread_events::CapOdds;
use crate::engine::LucidosEngine;
use crate::runtime::CodingAgent;

/// How far back the durations reach.
const WINDOW_DAYS: i32 = 30;

/// How often the in-memory lists are rebuilt.
const REFRESH_EVERY: Duration = Duration::from_secs(3600);

/// The hours either side of the ask's local hour that a learned cap counts.
const LEARN_HOURS_EITHER_SIDE: u8 = 2;

/// The fewest cards in the window that a cap is learned from.
const LEARN_FLOOR_CARDS: usize = 30;

/// The fewest distinct days those cards must come from.
const LEARN_FLOOR_DAYS: usize = 7;

/// How long a closed card with no real request after it stays out of the
/// durations. Past it the card counts as abandoned. A day outlasts every
/// learned cap on a 5-minute or 1-hour cache.
const CARD_UNDECIDED: Duration = Duration::from_secs(24 * 3600);

/// The beats a wait of `secs` needs: infinite for an abandoned card.
fn beats_needed(secs: f64, beat_every_secs: f64) -> f64 {
    (secs / beat_every_secs).floor().max(0.0)
}

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
            let needed = beats_needed(*d, beat_every_secs);
            if needed <= f64::from(cap) {
                odds.slots[needed as usize] += 1;
            } else {
                odds.after_cap += 1;
            }
        }
        odds
    }
}

/// The zone the `timezone` preference names. Unset or unparseable reads UTC.
pub(super) fn preference_zone(timezone: &str) -> Tz {
    timezone.parse().unwrap_or(Tz::UTC)
}

/// When a wait began, on the user's clock. History and plan both take it
/// from here, so a wait and its history agree on the hour.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct LocalStart {
    pub hour: u8,
    pub day: NaiveDate,
}

impl LocalStart {
    pub(super) fn of(at: DateTime<Utc>, zone: Tz) -> Self {
        let local = at.with_timezone(&zone);
        Self {
            hour: u8::try_from(local.hour()).expect("an hour fits a u8"),
            day: local.date_naive(),
        }
    }
}

/// One finished wait: how long it ran, and when it began.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct WaitSample {
    /// Infinite for an abandoned card.
    pub secs: f64,
    pub start: LocalStart,
}

#[cfg(test)]
impl WaitSample {
    /// A wait of `secs` that began at noon on one fixed day.
    pub(crate) fn at_noon(secs: f64) -> Self {
        Self {
            secs,
            start: LocalStart {
                hour: 12,
                day: NaiveDate::from_ymd_opt(2026, 1, 1).expect("a date"),
            },
        }
    }
}

/// Local hours from `from` up to but not including `to`, wrapping at
/// midnight.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct HourWindow {
    pub from: u8,
    pub to: u8,
}

impl HourWindow {
    /// The hours within [`LEARN_HOURS_EITHER_SIDE`] of `hour`.
    pub(super) fn around(hour: u8) -> Self {
        Self {
            from: (hour + 24 - LEARN_HOURS_EITHER_SIDE) % 24,
            to: (hour + LEARN_HOURS_EITHER_SIDE + 1) % 24,
        }
    }

    pub(super) fn contains(self, hour: u8) -> bool {
        (hour + 24 - self.from) % 24 < (self.to + 24 - self.from) % 24
    }
}

/// A parked card's cap, learned from this workspace's cards like it asked at
/// the same time of day.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct LearnedCap {
    pub pings: u32,
    /// The cards the window holds.
    pub samples: u32,
    pub hours: HourWindow,
    /// The zone's offset from UTC at the ask, in minutes.
    pub utc_offset_mins: i32,
}

/// This workspace's finished waits, by agent and type, each dated in `zone`.
#[derive(Debug)]
pub(crate) struct WaitDurations {
    by_type: HashMap<(WaitAgent, WaitType), Vec<WaitSample>>,
    zone: Tz,
}

impl Default for WaitDurations {
    fn default() -> Self {
        Self {
            by_type: HashMap::new(),
            zone: Tz::UTC,
        }
    }
}

impl WaitDurations {
    /// Samples dated in UTC.
    pub(super) fn from_samples(
        samples: impl IntoIterator<Item = (WaitAgent, WaitType, WaitSample)>,
    ) -> Self {
        let mut by_type: HashMap<(WaitAgent, WaitType), Vec<WaitSample>> = HashMap::new();
        for (agent, wait_type, sample) in samples {
            by_type.entry((agent, wait_type)).or_default().push(sample);
        }
        Self {
            by_type,
            ..Self::default()
        }
    }

    fn samples(&self, agent: WaitAgent, wait_type: WaitType) -> &[WaitSample] {
        self.by_type
            .get(&(agent, wait_type))
            .map_or(&[], Vec::as_slice)
    }

    fn secs(&self, agent: WaitAgent, wait_type: WaitType) -> Vec<f64> {
        self.samples(agent, wait_type)
            .iter()
            .map(|s| s.secs)
            .collect()
    }

    /// How `agent`'s past waits of `wait_type` end against a cap of `cap`
    /// beats, or `None` with no wait to count. `hours` keeps only the waits
    /// that began inside it.
    pub(super) fn odds(
        &self,
        agent: WaitAgent,
        wait_type: WaitType,
        beat_every_secs: f64,
        cap: u32,
        hours: Option<HourWindow>,
    ) -> Option<CapOdds> {
        let durations: Vec<f64> = self
            .samples(agent, wait_type)
            .iter()
            .filter(|s| hours.is_none_or(|hours| hours.contains(s.start.hour)))
            .map(|s| s.secs)
            .collect();
        (!durations.is_empty()).then(|| CapOdds::of(&durations, beat_every_secs, cap))
    }

    /// How long `agent`'s past waits of `wait_type` lasted, or `None` with no
    /// finished wait to count. Abandoned cards are left out.
    pub(super) fn history(&self, agent: WaitAgent, wait_type: WaitType) -> Option<WaitHistory> {
        WaitHistory::of(&self.secs(agent, wait_type))
    }

    /// The cap with the least cost over `agent`'s past `card` cards asked
    /// within [`LEARN_HOURS_EITHER_SIDE`] hours of `asked_at`, local time.
    /// `None` below the floor: [`LEARN_FLOOR_CARDS`] cards across
    /// [`LEARN_FLOOR_DAYS`] days.
    ///
    /// A card needing `k` beats costs `k` pings under a cap of at least `k`.
    /// Past the cap it costs every ping plus the reread, `reread_cap` pings.
    /// No cap exceeds `reread_cap`, and a tie goes to the smaller cap.
    pub(super) fn learned_cap(
        &self,
        agent: WaitAgent,
        card: ParkedCard,
        asked_at: DateTime<Utc>,
        beat_every_secs: f64,
        reread_cap: f64,
    ) -> Option<LearnedCap> {
        let hours = HourWindow::around(LocalStart::of(asked_at, self.zone).hour);
        let window: Vec<&WaitSample> = self
            .samples(agent, WaitType::Parked(card))
            .iter()
            .filter(|s| hours.contains(s.start.hour))
            .collect();
        let days: HashSet<NaiveDate> = window.iter().map(|s| s.start.day).collect();
        if window.len() < LEARN_FLOOR_CARDS
            || days.len() < LEARN_FLOOR_DAYS
            || !reread_cap.is_finite()
        {
            return None;
        }
        let needed: Vec<f64> = window
            .iter()
            .map(|s| beats_needed(s.secs, beat_every_secs))
            .collect();
        let cost = |cap: f64| -> f64 {
            needed
                .iter()
                .map(|&k| if k <= cap { k } else { cap + reread_cap })
                .sum()
        };
        // Past the longest finite wait, a longer cap only costs more.
        let longest = needed
            .iter()
            .copied()
            .filter(|k| k.is_finite())
            .fold(0.0, f64::max);
        let top = reread_cap.floor().min(longest).max(0.0) as u32;
        let mut best = (0, cost(0.0));
        for cap in 1..=top {
            let spent = cost(f64::from(cap));
            if spent < best.1 {
                best = (cap, spent);
            }
        }
        Some(LearnedCap {
            pings: best.0,
            samples: u32::try_from(window.len()).unwrap_or(u32::MAX),
            hours,
            utc_offset_mins: self
                .zone
                .offset_from_utc_datetime(&asked_at.naive_utc())
                .fix()
                .local_minus_utc()
                / 60,
        })
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
/// days ago. A card runs from its ask to the first real request after it
/// closed. With none within `$3` seconds it runs forever, and before then it
/// is left out. An allowed permission card ends at the allow instead.
///
/// An event wait runs from its start to its resolution. `kinds`
/// lists what an event wait watched, and is empty for a card.
/// `is_coding_agent` tells the two agents apart.
fn durations_sql() -> String {
    let read_at =
        crate::engine::successor::first_real_request_started_after_sql("s.thread_id", "a.sequence");
    format!(
        r#"
WITH kept AS (SELECT thread_id, is_coding_agent FROM thread_summaries
               WHERE NOT is_coding_agent OR coding_agent IS DISTINCT FROM $2)
SELECT k.is_coding_agent,
       s.event_type AS started,
       ARRAY[]::text[] AS kinds,
       s.created AS began_at,
       CASE WHEN a.allowed THEN EXTRACT(EPOCH FROM a.created - s.created)::float8
            WHEN n.read_at IS NULL THEN 'Infinity'::float8
            ELSE EXTRACT(EPOCH FROM GREATEST(n.read_at, a.created) - s.created)::float8 END AS secs
  FROM events s
  JOIN LATERAL (
       SELECT e.created, e.sequence,
              COALESCE((e.payload->>'allowed')::boolean, false) AS allowed
         FROM events e
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
 CROSS JOIN LATERAL (SELECT {read_at} AS read_at) n
  JOIN kept k ON k.thread_id = s.thread_id
 WHERE s.event_type IN ('UserQuestionAsked', 'CodingAgentPermissionRequest')
   AND s.created >= now() - make_interval(days => $1)
   AND (a.allowed OR n.read_at IS NOT NULL OR a.created < now() - make_interval(secs => $3))
UNION ALL
SELECT k.is_coding_agent,
       s.event_type,
       ARRAY(SELECT jsonb_array_elements(s.payload->'on')->>'event_type'),
       s.created,
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
"#
    )
}

#[derive(sqlx::FromRow)]
struct DurationRow {
    is_coding_agent: bool,
    started: String,
    kinds: Vec<String>,
    began_at: DateTime<Utc>,
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

/// The finished waits, each dated on the clock of `zone`.
pub(super) async fn read_wait_durations(
    pool: &sqlx::PgPool,
    zone: Tz,
) -> Result<WaitDurations, sqlx::Error> {
    let rows: Vec<DurationRow> = sqlx::query_as(&durations_sql())
        .bind(WINDOW_DAYS)
        .bind(CodingAgent::Codex.as_str())
        .bind(CARD_UNDECIDED.as_secs_f64())
        .fetch_all(pool)
        .await?;
    let durations = WaitDurations::from_samples(rows.iter().filter_map(|row| {
        let sample = WaitSample {
            secs: row.secs,
            start: LocalStart::of(row.began_at, zone),
        };
        Some((row.agent(), row.wait_type()?, sample))
    }));
    Ok(WaitDurations { zone, ..durations })
}

/// The durations the tick reads, and when and in which zone they were last
/// rebuilt.
#[derive(Default)]
pub(crate) struct WaitDurationsCache(Mutex<CachedDurations>);

#[derive(Default)]
struct CachedDurations {
    durations: Arc<WaitDurations>,
    refreshed: Option<(Instant, Tz)>,
}

impl WaitDurationsCache {
    pub(super) fn current(&self) -> Arc<WaitDurations> {
        self.0.lock().unwrap().durations.clone()
    }

    /// Whether a rebuild in `zone` is due: an hour has passed, or the zone
    /// changed. A `true` claims it, so one tick starts it.
    fn claim_refresh(&self, now: Instant, zone: Tz) -> bool {
        let mut cached = self.0.lock().unwrap();
        let due = cached.refreshed.is_none_or(|(at, built_in)| {
            built_in != zone || now.duration_since(at) >= REFRESH_EVERY
        });
        if due {
            cached.refreshed = Some((now, zone));
        }
        due
    }

    fn store(&self, durations: WaitDurations) {
        self.0.lock().unwrap().durations = Arc::new(durations);
    }
}

impl LucidosEngine {
    /// Rebuild the wait durations in the background once an hour, and at once
    /// when the `timezone` preference changes. A failed read keeps the old
    /// lists until the next hour.
    pub(super) async fn refresh_wait_durations_when_due(&self) {
        let zone = preference_zone(&self.user_timezone().await);
        if !self
            .keep_alive_durations
            .claim_refresh(Instant::now(), zone)
        {
            return;
        }
        let pool = self.pool.clone();
        let cache = self.keep_alive_durations.clone();
        tokio::spawn(async move {
            match read_wait_durations(&pool, zone).await {
                Ok(durations) => cache.store(durations),
                Err(e) => crate::log!("[KeepAlive] Could not read the wait durations: {e}"),
            }
        });
    }
}

#[cfg(test)]
#[path = "wait_durations_tests.rs"]
mod tests;
