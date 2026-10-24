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
    },
    /// The engine stopped pinging: more pings would cost more than a rewrite.
    Stopped {
        at: DateTime<Utc>,
        beats: u32,
        cost_usd: f64,
        /// The context the wake-up rereads instead.
        context_tokens: u64,
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
    },
    /// The user turned the keep-alive off for this thread at `at`.
    TurnedOff {
        at: DateTime<Utc>,
        beats: u32,
        cost_usd: f64,
        /// The context a reply after the cache goes cold rereads.
        context_tokens: u64,
    },
    /// The thread waits, and the engine does not keep it warm.
    NotKept { reason: CacheKeepAliveNotKeptReason },
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
    /// The provider cannot prefill the Lucidos Agent's last request.
    UncacheableProvider,
    /// Lucidos restarted since the thread's last request, or its Claude Code
    /// session ended, so a beat would miss the cached prefix.
    Restarted,
    /// Nothing waits that a keep-alive could serve.
    NothingPending,
}

impl CacheKeepAliveStatus {
    /// The status the thread's projection holds. A reason that names no wait
    /// (`codex`, `off`, `nothing_pending`) records nothing: the cache-warmth
    /// route says it, and no card or panel section follows.
    pub fn recorded(self) -> Option<Self> {
        match self {
            Self::NotKept {
                reason:
                    CacheKeepAliveNotKeptReason::Codex
                    | CacheKeepAliveNotKeptReason::Off
                    | CacheKeepAliveNotKeptReason::NothingPending,
            } => None,
            status => Some(status),
        }
    }

    /// The context a reply after the cache goes cold rereads, where a surface
    /// names it. Absent when the status carries none or it is unknown.
    pub fn reread_tokens(&self) -> Option<u64> {
        match self {
            Self::Warming { context_tokens, .. }
            | Self::Stopped { context_tokens, .. }
            | Self::TurnedOff { context_tokens, .. } => {
                Some(*context_tokens).filter(|tokens| *tokens > 0)
            }
            Self::Ended { .. } | Self::NotKept { .. } => None,
        }
    }
}
