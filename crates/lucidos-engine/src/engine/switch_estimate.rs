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
//! says how long a thread's cache stays warm, for the browser's countdown.

use serde::Serialize;
use sqlx::PgPool;
use uuid::Uuid;

use super::cache_ttl::CacheTtl;
use super::handoff_estimate::thread_model;
use super::successor::{last_cache_refresh_started_sql, thread_context, MAIN_SESSION_CAPTURE};
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
    /// The switch rewrites a warm cache. A cold cache, an empty thread and a
    /// no-op pick never do.
    pub rebuilds_cache: bool,
    /// USD, one time. `None` when no rewrite happens or a model has no price.
    pub extra_cost_usd: Option<f64>,
}

/// The estimate itself, a pure function of its inputs.
pub(crate) fn estimate(
    context_tokens: Option<u64>,
    cache_warm: bool,
    changes: bool,
    ttl: CacheTtl,
    from: Option<&ModelPrice>,
    to: Option<&ModelPrice>,
) -> SwitchEstimate {
    let rebuilds_cache = changes && cache_warm && context_tokens.is_some_and(|t| t > 0);
    let extra_cost_usd = match (rebuilds_cache, context_tokens, from, to) {
        (true, Some(tokens), Some(from), Some(to)) => {
            let write = to.input * ttl.write_share(to.cache);
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

/// The model and effort `thread_id`'s latest turn ran at: the current pair of
/// a thread with no session to ask.
pub(crate) async fn last_turn_settings(
    pool: &PgPool,
    thread_id: Uuid,
) -> Result<(Option<String>, Option<String>), sqlx::Error> {
    let row: Option<(Option<String>, Option<String>)> = sqlx::query_as(&format!(
        "SELECT payload->>'model', payload->>'reasoning_effort' FROM events \
          WHERE thread_id = $1 AND {MAIN_SESSION_CAPTURE} \
          ORDER BY sequence DESC LIMIT 1"
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
    let cache_warm = warm_remaining_secs(age_secs, ttl).is_some();
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
        cache_warm,
        model_changes || effort_changes,
        ttl,
        from.as_ref(),
        to.as_ref(),
    ))
}

/// Seconds a cache last written `age_secs` ago stays warm. `None` once it is
/// cold, or when there is no call to age.
fn warm_remaining_secs(age_secs: Option<f64>, ttl: CacheTtl) -> Option<f64> {
    age_secs
        .map(|age| ttl.lifetime_secs() - age)
        .filter(|remaining| *remaining > 0.0)
}

/// Whole seconds `thread_id`'s cache stays warm, `None` when it is cold or the
/// thread has made no main-session call. The age is computed in SQL, so no
/// host clock enters.
pub(crate) async fn cache_remaining_secs(
    pool: &PgPool,
    thread_id: Uuid,
) -> Result<Option<i64>, sqlx::Error> {
    let (age_secs, ttl) = cache_age_and_ttl(pool, thread_id).await?;
    Ok(warm_remaining_secs(age_secs, ttl)
        .map(|remaining| remaining.floor() as i64)
        .filter(|secs| *secs > 0))
}

/// Seconds since the thread's cache last refreshed (a turn request or a
/// keep-alive copy beat), and the TTL its latest cache write shows. The TTL
/// is 5 minutes when the write does not split it. A thread that never wrote to a prompt cache has no age: Codex threads and
/// providers without a cache write report none, so nothing of theirs is warm.
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
    let write: Option<(i64, Option<i64>)> = sqlx::query_as(&format!(
        "SELECT (payload->'usage'->>'cache_creation_tokens')::bigint, \
                (payload->'usage'->>'cache_creation_1h_tokens')::bigint FROM events \
          WHERE thread_id = $1 AND {MAIN_SESSION_CAPTURE} \
            AND (payload->'usage'->>'cache_creation_tokens')::bigint > 0 \
          ORDER BY sequence DESC LIMIT 1"
    ))
    .bind(thread_id)
    .fetch_optional(pool)
    .await?;
    let Some((written, written_1h)) = write else {
        return Ok((None, CacheTtl::FiveMinutes));
    };
    let ttl = CacheTtl::observed(written, written_1h).unwrap_or(CacheTtl::FiveMinutes);
    Ok((age, ttl))
}

#[cfg(test)]
#[path = "switch_estimate_tests.rs"]
mod tests;
