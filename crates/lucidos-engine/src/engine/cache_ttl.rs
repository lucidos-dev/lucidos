//! How long a prompt cache lives.
//!
//! For a Claude Code session, billing and settings decide it (ADR 0479), so
//! only the usage block says what a session got. The keep-alive times its
//! beats from it, and a new session's context states it (ADR 0470). A
//! Lucidos Agent beat takes it from the provider (ADR 0556).

use crate::llm::model_prices::CacheShares;

/// How long a session's prompt cache lives, as its latest cache write shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CacheTtl {
    FiveMinutes,
    /// OpenAI's minimum from GPT-5.6 on. No Claude Code session writes it.
    ThirtyMinutes,
    OneHour,
}

impl CacheTtl {
    /// The TTL of a write of `written` tokens, `written_1h` of them to the
    /// 1-hour cache. `None` for no write, or one the provider did not split.
    /// Any 5-minute token makes it 5 minutes: that part expires first.
    pub(crate) fn observed(written: i64, written_1h: Option<i64>) -> Option<Self> {
        let written_1h = written_1h.filter(|_| written > 0)?;
        Some(if written_1h >= written {
            Self::OneHour
        } else {
            Self::FiveMinutes
        })
    }

    /// The lifetime, in seconds.
    pub(crate) fn lifetime_secs(self) -> f64 {
        match self {
            Self::FiveMinutes => 300.0,
            Self::ThirtyMinutes => 1800.0,
            Self::OneHour => 3600.0,
        }
    }

    /// What a write at this TTL costs, as a share of the input price.
    pub(crate) fn write_share(self, cache: CacheShares) -> f64 {
        match self {
            Self::FiveMinutes | Self::ThirtyMinutes => cache.write_5m,
            Self::OneHour => cache.write_1h,
        }
    }

    /// The lifetime, in the words the session context states it.
    pub(crate) fn lifetime_words(self) -> &'static str {
        match self {
            Self::FiveMinutes => "5 minutes",
            Self::ThirtyMinutes => "30 minutes",
            Self::OneHour => "1 hour",
        }
    }

    /// The longest blocking call whose next request still lands inside the
    /// cache, with room for the call's own output and turn overhead.
    pub(crate) fn blocking_cap_words(self) -> &'static str {
        match self {
            Self::FiveMinutes => "4 minutes",
            Self::ThirtyMinutes => "25 minutes",
            Self::OneHour => "50 minutes",
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
SELECT (e.payload->'usage'->>'cache_creation_tokens')::bigint AS written,
       (e.payload->'usage'->>'cache_creation_1h_tokens')::bigint AS written_1h
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
/// the repo is the closest evidence. `None` when unknown or unreadable.
pub(crate) async fn repo_cache_ttl(pool: &sqlx::PgPool, repo_id: Option<&str>) -> Option<CacheTtl> {
    let row: Option<(i64, Option<i64>)> = match sqlx::query_as(LATEST_REPO_WRITE_SQL)
        .bind(REPO_WRITE_LOOKBACK_DAYS)
        .bind(repo_id)
        .fetch_optional(pool)
        .await
    {
        Ok(row) => row,
        Err(e) => {
            crate::log!("[CacheTtl] Could not read the repo's latest cache write: {e}");
            return None;
        }
    };
    let (written, written_1h) = row?;
    CacheTtl::observed(written, written_1h)
}

#[cfg(test)]
#[path = "cache_ttl_tests.rs"]
mod tests;
