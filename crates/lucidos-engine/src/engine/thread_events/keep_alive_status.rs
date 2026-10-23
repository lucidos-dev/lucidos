//! The *cache keep-alive status*: what the keep-alive does for one waiting
//! thread, as its tick last saw it. The tick is its one writer.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// The keep-alive's state for one thread. Absent when the thread has nothing
/// to keep warm.
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
    /// The thread waits, and the engine does not keep it warm.
    NotKept { reason: CacheKeepAliveNotKeptReason },
}

/// Why a waiting thread gets no keep-alive.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CacheKeepAliveNotKeptReason {
    /// Every working child is `no-wake`, so no finish would read the cache.
    NoWakeChildren,
    /// Every active child that would wake the parent waits on the user, which
    /// has no bound. A `no-wake` child may still run beside them.
    ChildrenAwaitingUser,
    /// The user turned the keep-alive off for this thread until it runs again.
    TurnedOff,
    /// The provider cannot prefill the Lucidos Agent's last request.
    UncacheableProvider,
    /// The engine remembers no request for the Lucidos Agent thread since it
    /// restarted, so the thread pays one rewrite.
    Restarted,
}
