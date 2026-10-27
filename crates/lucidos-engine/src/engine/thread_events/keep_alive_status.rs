//! The *cache keep-alive status*: what the keep-alive does for one thread, as
//! its tick last saw it. The tick is its one writer, and the cache-warmth route
//! reads the same type for the cache-warm line.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// The keep-alive's state for one thread, in the one grammar every surface
/// speaks: Warming, Stopped, Ended, Turned off, or Not kept.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum CacheKeepAliveStatus {
    /// The engine pings the session, within its break-even budget.
    Warming {
        /// When the wait it keeps warm began.
        since: DateTime<Utc>,
        /// The pings sent since then.
        beats: u32,
        /// The pings that cost as much as one cache rewrite.
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
    },
    /// The engine stopped pinging at the give-up time set when the wait began.
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
    },
    /// The wait ended while the engine kept it warm.
    Ended {
        /// When the wait it kept warm began.
        since: DateTime<Utc>,
        /// When the wait ended.
        at: DateTime<Utc>,
        beats: u32,
        cost_usd: f64,
        /// What the warm cache saved the next request: the reread price of the
        /// run's context. Absent unless that request began while the cache was
        /// still warm.
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
    },
    /// The thread waits, and the engine does not keep it warm.
    NotKept {
        reason: CacheKeepAliveNotKeptReason,
        /// When paused pings start again. Set for `pings_paused` only.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        until: Option<DateTime<Utc>>,
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
        }
    }

    /// Not kept: the provider's pings pause until `until`.
    pub fn pings_paused(until: DateTime<Utc>) -> Self {
        Self::NotKept {
            reason: CacheKeepAliveNotKeptReason::PingsPaused,
            until: Some(until),
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
}
