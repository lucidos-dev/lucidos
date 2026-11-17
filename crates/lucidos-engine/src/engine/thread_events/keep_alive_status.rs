//! The *cache keep-alive status*: what the keep-alive does for one thread, as
//! its tick last saw it. The tick is its one writer, and the cache-warmth route
//! reads the same type for the cache-warm line.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::{PlannedWake, WakeSource};

/// The keep-alive's state for one thread, in the one grammar every surface
/// speaks: Warming, Stopped, Ended, Turned off, or Not kept.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum CacheKeepAliveStatus {
    /// The engine pings the session, within its reread cap.
    Warming {
        /// When the run began: the wait start, or the agent's last real idle
        /// when that came later. A real turn mid-wait starts a new run.
        since: DateTime<Utc>,
        /// The pings sent since then.
        beats: u32,
        /// The pings the plan allows this run, from `since` to `give_up_at`:
        /// at most the reread cap.
        budget_beats: u32,
        /// How often a ping goes out, in seconds.
        beat_every_secs: u32,
        /// What those pings cost, in USD.
        cost_usd: f64,
        /// The context a wake-up would reread if the cache expired.
        context_tokens: u64,
        /// When the next ping is due. Absent before the session's first request.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        next_beat_at: Option<DateTime<Utc>>,
        /// When the engine stops pinging.
        give_up_at: DateTime<Utc>,
        /// What one ping costs: a cache read of the context. Absent for an
        /// unpriced model, and on statuses stored before prices.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        ping_usd: Option<f64>,
        /// What a reply after the cache goes cold pays to reread the context.
        /// Absent like `ping_usd`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reread_usd: Option<f64>,
        /// How past waits like this one ended, against the give-up cap.
        /// Absent with no past wait to count.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        odds: Option<CapOdds>,
        /// The expected wake the plan follows. Absent with none, when the
        /// bare reread cap applies.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_wake: Option<PlannedWake>,
        /// What set the run's give-up time. Absent on statuses stored before
        /// it.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cap_source: Option<CapSource>,
    },
    /// The engine stopped pinging at the end of its plan.
    Stopped {
        at: DateTime<Utc>,
        beats: u32,
        cost_usd: f64,
        /// The context the wake-up rereads instead.
        context_tokens: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        ping_usd: Option<f64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reread_usd: Option<f64>,
        /// How often a ping went out, in seconds. Absent on statuses stored
        /// before it.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        beat_every_secs: Option<u32>,
        /// The odds the run's cap was chosen on. Absent like `beat_every_secs`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        odds: Option<CapOdds>,
        /// What set the run's give-up time. Absent on statuses stored before
        /// it.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cap_source: Option<CapSource>,
    },
    /// The wait ended while the engine kept it warm.
    Ended {
        /// When the run began, as `Warming` named it.
        since: DateTime<Utc>,
        /// When the wait ended.
        at: DateTime<Utc>,
        beats: u32,
        cost_usd: f64,
        /// What the warm cache saved the next request: the reread price of the
        /// run's context. Absent unless that request began while the cache was
        /// still warm. Zero when the last real request alone kept it warm.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        saved_usd: Option<f64>,
        /// The context the run kept warm. Absent on statuses stored before it.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        context_tokens: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        ping_usd: Option<f64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reread_usd: Option<f64>,
        /// How often a ping went out, in seconds. Absent on statuses stored
        /// before it.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        beat_every_secs: Option<u32>,
        /// The odds the run's cap was chosen on. Absent like `beat_every_secs`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        odds: Option<CapOdds>,
        /// What set the run's give-up time. Absent on statuses stored before
        /// it.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cap_source: Option<CapSource>,
    },
    /// The user turned the keep-alive off for this thread at `at`.
    TurnedOff {
        at: DateTime<Utc>,
        beats: u32,
        cost_usd: f64,
        /// The context a reply after the cache goes cold rereads.
        context_tokens: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        ping_usd: Option<f64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reread_usd: Option<f64>,
        /// How often a ping went out, in seconds. Absent on statuses stored
        /// before it.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        beat_every_secs: Option<u32>,
        /// The odds the run's cap was chosen on. Absent like `beat_every_secs`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        odds: Option<CapOdds>,
        /// What set the run's give-up time. Absent on statuses stored before
        /// it.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cap_source: Option<CapSource>,
    },
    /// The thread waits, and the engine does not keep it warm.
    NotKept {
        reason: CacheKeepAliveNotKeptReason,
        /// When paused pings start again. Set for `pings_paused` only.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        until: Option<DateTime<Utc>>,
        /// What set the give-up time the wait passed, or the cap the expected
        /// wake lies past. Set for `past_give_up_time` and
        /// `wake_past_reread_cap` only.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cap_source: Option<CapSource>,
    },
}

/// Why a waiting thread gets no keep-alive.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CacheKeepAliveNotKeptReason {
    /// A Codex thread, which is never beaten.
    Codex,
    /// The `cache_keep_alive_mode` preference is `off`.
    Off,
    /// Every working child is `no-wake`, so no finish would read the cache.
    NoWakeChildren,
    /// Every active child that would wake the parent waits on the user, which
    /// has no bound. A `no-wake` child may still run beside them.
    ChildrenAwaitingUser,
    /// The last request has no prefill pair or backend.
    UncacheableProvider,
    /// A refused or talkative ping paused this provider until the engine
    /// restarts.
    ProviderPaused,
    /// Historical: recorded while prefill beats were paused (ADR 0581), never
    /// emitted now. Stored events still carry it, so it keeps reading.
    PrefillPaused,
    /// The provider's recent pings kept missing, so pings pause until the
    /// status's `until` (ADR 0580).
    PingsPaused,
    /// The thread's cache went cold before a ping reached it, so a ping
    /// would write, not read.
    CacheExpired,
    /// Lucidos holds no record of the thread's last request: it restarted
    /// since, or the thread's Claude Code session ended. A beat would miss
    /// the cached prefix.
    Restarted,
    /// The wait passed its give-up time before the first ping was due, so no
    /// ping went out. A run that pinged and then gave up is `Stopped`.
    PastGiveUpTime,
    /// The expected wake lies past what pings may cost: past the saving
    /// margin of one reread, or past the reread cap from the wait start. So
    /// no ping goes out, and the wake rereads.
    WakePastRereadCap,
    /// Nothing waits that a keep-alive could serve.
    NothingPending,
    /// A trigger thread no person has written in, so nobody reads a warm cache.
    UnattendedTrigger,
    /// No response said how long the thread's cache lives, so no ping can be
    /// timed. A provider's documented default never counts (ADR 0575).
    UnknownTtl,
    /// The beat would use another model or reasoning effort.
    ModelChanged,
    /// A ping failed to hold the context; only a real request re-arms the run.
    BeatMissed,
}

impl CacheKeepAliveStatus {
    /// Not kept for `reason`, which is never `pings_paused`.
    pub fn not_kept(reason: CacheKeepAliveNotKeptReason) -> Self {
        Self::NotKept {
            reason,
            until: None,
            cap_source: None,
        }
    }

    /// Not kept: the provider's pings pause until `until`.
    pub fn pings_paused(until: DateTime<Utc>) -> Self {
        Self::NotKept {
            reason: CacheKeepAliveNotKeptReason::PingsPaused,
            until: Some(until),
            cap_source: None,
        }
    }

    /// Not kept for `reason`, a give-up the plan set by `cap_source`:
    /// `past_give_up_time` or `wake_past_reread_cap`.
    pub fn past_cap(reason: CacheKeepAliveNotKeptReason, cap_source: CapSource) -> Self {
        Self::NotKept {
            reason,
            until: None,
            cap_source: Some(cap_source),
        }
    }

    /// The status the thread's projection holds. A reason that names no wait
    /// (`codex`, `off`, `nothing_pending`, `unattended_trigger`) records nothing:
    /// the cache-warmth route says it, and no card or panel section follows.
    pub fn recorded(self) -> Option<Self> {
        match self {
            Self::NotKept {
                reason:
                    CacheKeepAliveNotKeptReason::Codex
                    | CacheKeepAliveNotKeptReason::Off
                    | CacheKeepAliveNotKeptReason::NothingPending
                    | CacheKeepAliveNotKeptReason::UnattendedTrigger,
                ..
            } => None,
            status => Some(status),
        }
    }
}

/// *Cap odds*: how this workspace's finished waits of one agent and wait type
/// ended, counted against a give-up cap. `slots` holds one entry per ping
/// count, from none up to the cap.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapOdds {
    /// The finished waits counted.
    pub samples: u32,
    /// `slots[k]`: the waits that needed `k` pings, the cache still warm.
    /// Slot 0 ended before the first ping.
    pub slots: Vec<u32>,
    /// The waits that outlasted the cap: they paid every ping and the reread.
    pub after_cap: u32,
}

/// *Cap source*: what set a keep-alive run's give-up time, so every surface
/// can say where the estimate came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CapSource {
    /// Half the reread cap: the parked card's fallback (ADR 0547).
    HalfReread,
    /// The reread cap: an event wait with no expected wake inside it.
    RereadCap,
    /// The event wait's own timeout, which wakes the thread.
    Timeout,
    /// The expected wake, as `by` stated it.
    Estimate { by: WakeSource },
    /// A parked card's cap of `pings`, learned from the `samples` cards like
    /// it asked from `from_hour` up to `to_hour`, in the `timezone`
    /// preference's zone. The window may wrap past midnight.
    /// `utc_offset_mins` is that zone's offset at the ask, so a reader can
    /// show the hours on its own clock.
    Learned {
        pings: u32,
        samples: u32,
        from_hour: u8,
        to_hour: u8,
        utc_offset_mins: i32,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_historical_prefill_paused_status_still_reads() {
        let stored = r#"{"state":"not_kept","reason":"prefill_paused"}"#;
        let status: CacheKeepAliveStatus = serde_json::from_str(stored).unwrap();
        assert_eq!(
            status,
            CacheKeepAliveStatus::not_kept(CacheKeepAliveNotKeptReason::PrefillPaused)
        );
        assert_eq!(serde_json::to_string(&status).unwrap(), stored);
    }

    #[test]
    fn a_status_stored_before_cap_sources_still_reads() {
        let stored = [
            r#"{"state":"warming","since":"2026-10-01T10:00:00Z","beats":1,"budget_beats":12,"beat_every_secs":230,"cost_usd":0.1,"context_tokens":1000,"give_up_at":"2026-10-01T10:46:00Z"}"#,
            r#"{"state":"stopped","at":"2026-10-01T10:46:00Z","beats":12,"cost_usd":1.2,"context_tokens":1000}"#,
            r#"{"state":"ended","since":"2026-10-01T10:00:00Z","at":"2026-10-01T10:20:00Z","beats":5,"cost_usd":0.5}"#,
            r#"{"state":"turned_off","at":"2026-10-01T10:20:00Z","beats":5,"cost_usd":0.5,"context_tokens":1000}"#,
            r#"{"state":"not_kept","reason":"past_give_up_time"}"#,
        ];
        for json in stored {
            let status: CacheKeepAliveStatus = serde_json::from_str(json).unwrap();
            assert_eq!(serde_json::to_string(&status).unwrap(), json, "{json}");
        }
    }

    #[test]
    fn a_cap_source_is_tagged_by_kind() {
        let source = CapSource::Estimate {
            by: WakeSource::Agent,
        };
        assert_eq!(
            serde_json::to_string(&source).unwrap(),
            r#"{"kind":"estimate","by":"agent"}"#
        );
        assert_eq!(
            serde_json::to_string(&CapSource::HalfReread).unwrap(),
            r#"{"kind":"half_reread"}"#
        );
        assert_eq!(
            serde_json::to_string(&CapSource::Learned {
                pings: 3,
                samples: 41,
                from_hour: 22,
                to_hour: 3,
                utc_offset_mins: 60,
            })
            .unwrap(),
            r#"{"kind":"learned","pings":3,"samples":41,"from_hour":22,"to_hour":3,"utc_offset_mins":60}"#
        );
    }
}
