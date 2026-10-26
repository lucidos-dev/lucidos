//! The *switch estimate*: what changing a live thread's model or effort costs.
//! Both rewrite the messages cache once, so the one-time cost is the context
//! written at the new price, less the read it replaces. The figure is
//! approximate: the tools and system prompt may stay cached.
//!
//! A coding-agent thread's current pair is what it runs on: its model
//! setting and its *effort in force*. A chat thread's is what its latest turn
//! ran.
//!
//! The browser shows the figure and prices nothing itself. The same TTL rule
//! says how long a thread's cache stays warm, for the browser's countdown. A
//! cache whose TTL no evidence gives is neither warm nor cold (ADR 0575).

use serde::Serialize;
use sqlx::PgPool;
use uuid::Uuid;

use super::cache_ttl::{CacheTtl, WriteSplit};
use super::handoff_estimate::thread_model;
use super::successor::{
    current_session_capture_sql, last_cache_refresh_started_sql, thread_context,
};
use crate::llm::model_prices::{price_of, ModelPrice};
use crate::runtime::intelligence_ladder::{model_run_by, runs_same_model};
use crate::runtime::CodingAgent;

/// Whose model ids a switch speaks. A coding agent names models as its own
/// menu does. The Lucidos Agent names them as the price table does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SwitchSubject {
    CodingAgent(CodingAgent),
    LucidosAgent,
}

impl SwitchSubject {
    fn run_model(self, model: &str) -> String {
        match self {
            Self::CodingAgent(agent) => model_run_by(agent, model),
            Self::LucidosAgent => model.to_string(),
        }
    }

    fn same_model(self, a: &str, b: &str) -> bool {
        match self {
            Self::CodingAgent(agent) => runs_same_model(agent, a, b),
            Self::LucidosAgent => a == b,
        }
    }
}

/// The model and effort a pick would commit. An absent field is unchanged.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SwitchTarget {
    pub model: Option<String>,
    pub effort: Option<String>,
}

/// What the switch would cost.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct SwitchEstimate {
    /// The input tokens of the thread's latest main-session model call.
    pub context_tokens: Option<u64>,
    /// Whether the switch rewrites a warm cache.
    pub rebuilds_cache: CacheRebuild,
    /// USD, one time, should it rewrite. `None` when no rewrite happens or a
    /// model has no price.
    pub extra_cost_usd: Option<f64>,
}

/// Whether a switch rewrites the thread's prompt cache.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum CacheRebuild {
    /// The cache is warm.
    Yes,
    /// A cold cache, an empty thread or a no-op pick.
    No,
    /// The cache may still be warm: no evidence gives its TTL.
    Unknown,
}

/// How warm a thread's prompt cache is now, as evidence shows it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CacheRemaining {
    /// Warm for this many more whole seconds.
    Warm(i64),
    /// Its lifetime passed, or the thread never wrote a cache.
    Cold,
    /// It wrote a cache whose lifetime no evidence gives.
    Unknown,
}

/// The estimate itself, a pure function of its inputs.
pub(crate) fn estimate(
    context_tokens: Option<u64>,
    warmth: CacheRemaining,
    changes: bool,
    ttl: CacheTtl,
    from: Option<&ModelPrice>,
    to: Option<&ModelPrice>,
) -> SwitchEstimate {
    let rebuilds_cache = match warmth {
        _ if !changes || context_tokens.is_none_or(|t| t == 0) => CacheRebuild::No,
        CacheRemaining::Warm(_) => CacheRebuild::Yes,
        CacheRemaining::Cold => CacheRebuild::No,
        CacheRemaining::Unknown => CacheRebuild::Unknown,
    };
    let extra_cost_usd = match (rebuilds_cache, context_tokens, from, to) {
        (CacheRebuild::Yes | CacheRebuild::Unknown, Some(tokens), Some(from), Some(to)) => {
            // A write with no known lifetime bills at the plain write rate,
            // as `ApiUsage::usd_on` prices one.
            let write_share = ttl
                .lifetime()
                .map_or(to.cache.write_5m, |lifetime| lifetime.write_share(to.cache));
            let write = to.input * write_share;
            let read = from.input * from.cache.read;
            Some(tokens as f64 * (write - read).max(0.0) / 1e6)
        }
        _ => None,
    };
    SwitchEstimate {
        context_tokens,
        rebuilds_cache,
        extra_cost_usd,
    }
}

/// The model and effort `thread_id`'s latest turn in its current session ran
/// at: the current pair of a thread with no live session to ask.
pub(crate) async fn last_turn_settings(
    pool: &PgPool,
    thread_id: Uuid,
) -> Result<(Option<String>, Option<String>), sqlx::Error> {
    let row: Option<(Option<String>, Option<String>)> = sqlx::query_as(&format!(
        "SELECT payload->>'model', payload->>'reasoning_effort' FROM events \
          WHERE thread_id = $1 AND {} \
          ORDER BY sequence DESC LIMIT 1",
        current_session_capture_sql("$1")
    ))
    .bind(thread_id)
    .fetch_optional(pool)
    .await?;
    Ok(row.unwrap_or((None, None)))
}

/// The estimate for switching `thread_id` to `target`. `current` is the
/// model and effort the thread runs on.
pub(crate) async fn switch_estimate(
    pool: &PgPool,
    thread_id: Uuid,
    subject: SwitchSubject,
    current: (Option<String>, Option<String>),
    target: &SwitchTarget,
) -> Result<SwitchEstimate, sqlx::Error> {
    let (current_model, current_effort) = current;
    let current_model = match current_model {
        Some(model) => Some(model),
        None => thread_model(pool, thread_id).await?,
    };
    let model_changes = match (&current_model, &target.model) {
        (Some(current), Some(target)) => !subject.same_model(current, target),
        _ => false,
    };
    let effort_changes = match (&current_effort, &target.effort) {
        (Some(current), Some(target)) => current != target,
        _ => false,
    };
    let context_tokens = thread_context(pool, thread_id).await?.input_tokens;
    let (age_secs, ttl) = cache_age_and_ttl(pool, thread_id).await?;
    let warmth = remaining_of(age_secs, ttl);
    let price = |model: &Option<String>| {
        model
            .as_deref()
            .and_then(|model| price_of(&subject.run_model(model)))
    };
    let from = price(&current_model);
    let to = match &target.model {
        Some(_) => price(&target.model),
        None => from,
    };
    Ok(estimate(
        context_tokens,
        warmth,
        model_changes || effort_changes,
        ttl,
        from.as_ref(),
        to.as_ref(),
    ))
}

/// How warm a cache last refreshed `age_secs` ago is, on a cache of `ttl`.
/// With no call to age it is cold, and with no known lifetime unknown.
fn remaining_of(age_secs: Option<f64>, ttl: CacheTtl) -> CacheRemaining {
    let Some(age) = age_secs else {
        return CacheRemaining::Cold;
    };
    let Some(lifetime) = ttl.lifetime() else {
        return CacheRemaining::Unknown;
    };
    match (lifetime.lifetime_secs() - age).floor() as i64 {
        secs if secs > 0 => CacheRemaining::Warm(secs),
        _ => CacheRemaining::Cold,
    }
}

/// How warm `thread_id`'s cache is now. Cold when the thread has made no
/// main-session call. The age is computed in SQL, so no host clock enters.
pub(crate) async fn cache_remaining(
    pool: &PgPool,
    thread_id: Uuid,
) -> Result<CacheRemaining, sqlx::Error> {
    let (age_secs, ttl) = cache_age_and_ttl(pool, thread_id).await?;
    Ok(remaining_of(age_secs, ttl))
}

/// Seconds since the thread's cache last refreshed (a turn request or a
/// keep-alive copy beat), and the TTL its latest cache write shows. A thread
/// that never wrote to a prompt cache has no age, so nothing of its is warm.
async fn cache_age_and_ttl(
    pool: &PgPool,
    thread_id: Uuid,
) -> Result<(Option<f64>, CacheTtl), sqlx::Error> {
    let age: Option<f64> = sqlx::query_scalar(&format!(
        "SELECT EXTRACT(EPOCH FROM now() - {})::float8",
        last_cache_refresh_started_sql("$1")
    ))
    .bind(thread_id)
    .fetch_one(pool)
    .await?;
    let Some(write) = last_cache_write(pool, thread_id).await? else {
        return Ok((None, CacheTtl::Unknown));
    };
    Ok((age, write.ttl))
}

/// The latest write to the prompt cache in a thread's current session.
pub(crate) struct CacheWrite {
    pub ttl: CacheTtl,
    pub model: Option<String>,
}

/// The latest cache write of `thread_id`'s current session, `None` when it
/// never wrote one.
pub(crate) async fn last_cache_write(
    pool: &PgPool,
    thread_id: Uuid,
) -> Result<Option<CacheWrite>, sqlx::Error> {
    let write: Option<(sqlx::types::Json<WriteSplit>, Option<String>)> = sqlx::query_as(&format!(
        "SELECT payload->'usage', payload->>'model' FROM events \
          WHERE thread_id = $1 AND {} \
            AND (payload->'usage'->>'cache_creation_tokens')::bigint > 0 \
          ORDER BY sequence DESC LIMIT 1",
        current_session_capture_sql("$1")
    ))
    .bind(thread_id)
    .fetch_optional(pool)
    .await?;
    Ok(write.map(|(usage, model)| CacheWrite {
        ttl: CacheTtl::of_write(usage.0),
        model,
    }))
}

#[cfg(test)]
#[path = "switch_estimate_tests.rs"]
mod tests;
