//! How long a prompt cache lives, as the engine knows it from evidence.
//!
//! The provider's response is the evidence: its usage block splits a write by
//! lifetime. Where it splits nothing, a TTL the request asked for counts only
//! on a route probes showed honours it. Anything else is Unknown, and a
//! provider's documented default never counts (ADR 0575). The keep-alive times
//! its beats from it, and a new session's context states it (ADR 0470).

use serde::Deserialize;

use crate::engine::types::ApiUsage;
pub(crate) use crate::llm::cache_lifetime::CacheLifetime;

/// How long a prompt cache entry lives, and what says so.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CacheTtl {
    /// The response split the write by lifetime.
    Reported(CacheLifetime),
    /// The response split nothing, and the request asked for this lifetime
    /// on a route that honours it.
    Requested(CacheLifetime),
    Unknown,
}

/// The fields of a usage block that say how long its cache write lives.
/// Every field defaults, so a stored row that lacks one still reads: a reader
/// decodes this, never the whole `ApiUsage`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub(crate) struct WriteSplit {
    cache_creation_tokens: u32,
    cache_creation_1h_tokens: Option<u32>,
    cache_creation_30m_tokens: Option<u32>,
    cache_creation_12h_tokens: Option<u32>,
    cache_creation_requested_ttl: Option<CacheLifetime>,
}

impl From<&ApiUsage> for WriteSplit {
    fn from(usage: &ApiUsage) -> Self {
        Self {
            cache_creation_tokens: usage.cache_creation_tokens,
            cache_creation_1h_tokens: usage.cache_creation_1h_tokens,
            cache_creation_30m_tokens: usage.cache_creation_30m_tokens,
            cache_creation_12h_tokens: usage.cache_creation_12h_tokens,
            cache_creation_requested_ttl: usage.cache_creation_requested_ttl,
        }
    }
}

impl CacheTtl {
    /// The TTL of one call's cache write. Any token with a shorter lifetime
    /// makes the whole write that short: that part expires first.
    ///
    /// Anthropic splits off the 1-hour part, and the rest is 5 minutes.
    /// OpenAI's 30-minute and 12-hour parts cover the write, so a rest there
    /// has no known lifetime.
    pub(crate) fn of_write(usage: impl Into<WriteSplit>) -> Self {
        let usage: WriteSplit = usage.into();
        let written = usage.cache_creation_tokens;
        if written == 0 {
            return Self::Unknown;
        }
        let parts = [
            (
                CacheLifetime::ThirtyMinutes,
                usage.cache_creation_30m_tokens,
            ),
            (CacheLifetime::OneHour, usage.cache_creation_1h_tokens),
            (CacheLifetime::TwelveHours, usage.cache_creation_12h_tokens),
        ];
        if parts.iter().all(|(_, part)| part.is_none()) {
            return match usage.cache_creation_requested_ttl {
                Some(lifetime) => Self::Requested(lifetime),
                None => Self::Unknown,
            };
        }
        let named: u64 = parts
            .iter()
            .filter_map(|(_, part)| *part)
            .map(u64::from)
            .sum();
        // Parts that claim more than the write are no evidence of anything.
        let Some(rest) = u64::from(written).checked_sub(named) else {
            return Self::Unknown;
        };
        let shortest_named = parts
            .iter()
            .filter(|(_, part)| part.is_some_and(|n| n > 0))
            .map(|(lifetime, _)| *lifetime)
            .min();
        match (rest, usage.cache_creation_1h_tokens) {
            (0, _) => shortest_named.map_or(Self::Unknown, Self::Reported),
            (_, Some(_)) => Self::Reported(CacheLifetime::FiveMinutes),
            (_, None) => Self::Unknown,
        }
    }

    /// The lifetime, when evidence gave one.
    pub(crate) fn lifetime(self) -> Option<CacheLifetime> {
        match self {
            Self::Reported(lifetime) | Self::Requested(lifetime) => Some(lifetime),
            Self::Unknown => None,
        }
    }
}

/// How far back the repo's latest cache write may lie. Older evidence counts
/// as none. The bound also keeps the index walk short.
const REPO_WRITE_LOOKBACK_DAYS: i32 = 7;

/// The latest Claude Code cache write of a session's own, among the threads
/// on one repo. A sub-agent may run on another TTL, so its writes do not
/// count.
const LATEST_REPO_WRITE_SQL: &str = r#"
SELECT e.payload->'usage' AS usage
  FROM events e
  JOIN thread_summaries ts ON ts.thread_id = e.thread_id
 WHERE e.event_type = 'ContextCaptured'
   AND e.created > now() - make_interval(days => $1)
   AND ts.cc_repo_id IS NOT DISTINCT FROM $2
   AND e.payload->>'producer' = 'claude_code'
   AND COALESCE(e.payload->>'purpose', 'turn') = 'turn'
   AND e.payload->>'parent_tool_use_id' IS NULL
   AND (e.payload->'usage'->>'cache_creation_tokens')::bigint > 0
 ORDER BY e.created DESC
 LIMIT 1
"#;

/// The TTL Claude Code sessions on `repo_id` get, from the repo's latest
/// cache write. A new session has no write of its own yet. Billing is shared
/// across a workspace, and a repo's project settings can change the TTL, so
/// the repo is the closest evidence. Unknown when there is none, or it is
/// unreadable.
pub(crate) async fn repo_cache_ttl(pool: &sqlx::PgPool, repo_id: Option<&str>) -> CacheTtl {
    let row: Option<(sqlx::types::Json<WriteSplit>,)> = match sqlx::query_as(LATEST_REPO_WRITE_SQL)
        .bind(REPO_WRITE_LOOKBACK_DAYS)
        .bind(repo_id)
        .fetch_optional(pool)
        .await
    {
        Ok(row) => row,
        Err(e) => {
            crate::log!("[CacheTtl] Could not read the repo's latest cache write: {e}");
            return CacheTtl::Unknown;
        }
    };
    row.map_or(CacheTtl::Unknown, |(usage,)| CacheTtl::of_write(usage.0))
}

#[cfg(test)]
#[path = "cache_ttl_tests.rs"]
mod tests;
