//! The *handoff estimate*: does a handoff pay at a *handoff checkpoint*? Each
//! model call left saves the gap between this context and a fresh one. The
//! handoff costs the narrative and writing the fresh context.
//!
//! The engine alone decides, and it names no checkpoint: a flow passes the
//! name. The CLI prints the verdict and compares nothing (ADR 0505).
//!
//! The price of a handoff depends on the old thread's prompt cache. While it
//! is warm, the old agent writes the narrative and rereads its whole context
//! to do so. Once it is cold, the successor starts from a *handoff brief* and
//! the old thread wakes nothing, but staying would rewrite its context in full.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

use super::event_bus::{BusEvent, EventBus};
use super::handoff_history::{calls_per_unit, fresh_tokens_median, thread_repo_sql, HistoryKey};
use super::successor::{ThreadContext, MAIN_SESSION_CAPTURE};
use super::switch_estimate::cache_remaining_secs;
use super::thread_events::{EventMeta, MessageOrigin, ThreadEvent};
use crate::llm::model_prices::{price_of, ModelPrice};

/// Context size, in input tokens, at or under which a handoff never pays.
/// Estimates are noisy there, and a context this small still works well.
pub const HANDOFF_FLOOR_TOKENS: u64 = 100_000;

/// Main-session calls one unit of work takes, when history is thin.
pub const CALLS_PER_UNIT_FALLBACK: f64 = 40.0;

/// A successor's context at its tenth call, when history is thin.
pub const FRESH_TOKENS_FALLBACK: f64 = 60_000.0;

/// The output tokens a *handoff narrative* takes to write.
const NARRATIVE_OUTPUT_TOKENS: f64 = 4_000.0;

/// The checkpoint name the Successor card's reading pools its history under.
pub const CARD_CHECKPOINT: &str = "successor-card";

/// The longest checkpoint name a flow may pass.
const MAX_NAME_LEN: usize = 64;

/// A checkpoint's name, as a flow passes it: lowercase letters, digits and
/// hyphens, starting with a letter or digit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct CheckpointName(String);

impl CheckpointName {
    pub fn parse(raw: &str) -> Result<Self, String> {
        let well_formed = !raw.is_empty()
            && raw.len() <= MAX_NAME_LEN
            && raw.starts_with(|c: char| c.is_ascii_lowercase() || c.is_ascii_digit())
            && raw
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
        if well_formed {
            Ok(Self(raw.to_string()))
        } else {
            Err(format!(
                "A checkpoint name is 1 to {MAX_NAME_LEN} lowercase letters, digits and hyphens, \
                 starting with a letter or digit. Got `{raw}`."
            ))
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// How far a flow's work has come at a checkpoint: `done` units of `total`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckpointProgress {
    pub done: u32,
    pub total: u32,
}

impl CheckpointProgress {
    /// Parse `<done>/<total>`, with `total >= 1` and `done <= total`.
    pub fn parse(raw: &str) -> Result<Self, String> {
        let parsed = raw.split_once('/').and_then(|(done, total)| {
            Some((done.trim().parse().ok()?, total.trim().parse().ok()?))
        });
        match parsed {
            Some((done, total)) if total >= 1 && done <= total => Ok(Self { done, total }),
            _ => Err(format!(
                "The progress reads `<done>/<total>`, with 1 <= total and done <= total. Got `{raw}`."
            )),
        }
    }

    pub fn units_left(&self) -> u32 {
        self.total - self.done
    }
}

/// The wire form `parse` reads: `<done>/<total>`.
impl std::fmt::Display for CheckpointProgress {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/{}", self.done, self.total)
    }
}

/// One checkpoint run, as the flow names it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckpointRun {
    pub name: CheckpointName,
    pub progress: Option<CheckpointProgress>,
}

impl CheckpointRun {
    /// Parse the wire pair a flow passes.
    pub fn parse(name: &str, progress: Option<&str>) -> Result<Self, String> {
        Ok(Self {
            name: CheckpointName::parse(name)?,
            progress: progress.map(CheckpointProgress::parse).transpose()?,
        })
    }

    /// The units of work left: `total - done`, else one.
    pub fn units_left(&self) -> u32 {
        self.progress.map_or(1, |p| p.units_left())
    }
}

/// Whether the old thread's prompt cache is warm when a handoff starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum CacheWarmth {
    Warm,
    Cold,
}

impl CacheWarmth {
    fn of_remaining_secs(remaining_secs: Option<i64>) -> Self {
        match remaining_secs {
            Some(_) => Self::Warm,
            None => Self::Cold,
        }
    }
}

/// Whether `thread_id`'s prompt cache is warm now.
pub async fn cache_warmth(pool: &PgPool, thread_id: Uuid) -> Result<CacheWarmth, sqlx::Error> {
    Ok(CacheWarmth::of_remaining_secs(
        cache_remaining_secs(pool, thread_id).await?,
    ))
}

/// What the estimate weighed, so a reader can check the verdict.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct HandoffEstimate {
    pub pays: bool,
    pub warmth: CacheWarmth,
    pub units_left: u32,
    pub calls_per_unit: f64,
    pub calls_left: f64,
    pub fresh_tokens: f64,
    /// USD staying saves by handing off: the calls left on a fresh context,
    /// and, on a cold cache, the next call's full rewrite of the old one.
    pub saving_usd: f64,
    /// USD the handoff costs: the fresh context's write, and on a warm cache
    /// the narrative turn, which rereads the old context and writes the
    /// narrative.
    pub cost_usd: f64,
}

/// The estimate itself, a pure function of its inputs.
pub(crate) fn estimate(
    context_tokens: u64,
    price: &ModelPrice,
    units_left: u32,
    calls_per_unit: f64,
    fresh_tokens: f64,
    warmth: CacheWarmth,
) -> HandoffEstimate {
    let calls_left = f64::from(units_left) * calls_per_unit;
    let per_token = |share: f64| price.input * share / 1e6;
    let saved_per_call = (context_tokens as f64 - fresh_tokens).max(0.0);
    let mut saving_usd = calls_left * saved_per_call * per_token(price.cache.read);
    let mut cost_usd = fresh_tokens * per_token(price.cache.write_5m);
    match warmth {
        CacheWarmth::Warm => {
            cost_usd += context_tokens as f64 * per_token(price.cache.read)
                + NARRATIVE_OUTPUT_TOKENS * price.output / 1e6;
        }
        CacheWarmth::Cold => {
            saving_usd += context_tokens as f64 * per_token(price.cache.write_5m);
        }
    }
    HandoffEstimate {
        pays: context_tokens > HANDOFF_FLOOR_TOKENS && saving_usd > cost_usd,
        warmth,
        units_left,
        calls_per_unit,
        calls_left,
        fresh_tokens,
        saving_usd,
        cost_usd,
    }
}

/// The engine's verdict for one thread at one checkpoint run.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct HandoffVerdict {
    pub name: CheckpointName,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub progress: Option<CheckpointProgress>,
    /// `None` when the engine could not make a reading: no context size yet,
    /// or a model with no price row. The agent then carries on.
    pub pays: Option<bool>,
    pub estimate: Option<HandoffEstimate>,
}

/// What a checkpoint run read off its thread, which its record keeps so a
/// later median can pool on it.
#[derive(Debug, Clone, Default, PartialEq)]
struct Readings {
    context_tokens: Option<u64>,
    model: Option<String>,
    reasoning_effort: Option<String>,
}

/// The verdict for `thread_id` at `run`. A thread that cannot hand off never
/// pays.
pub async fn handoff_verdict(
    pool: &PgPool,
    thread_id: Uuid,
    run: &CheckpointRun,
) -> Result<HandoffVerdict, sqlx::Error> {
    let readings = read_thread(pool, thread_id).await?;
    verdict_from(pool, thread_id, run, &readings).await
}

async fn verdict_from(
    pool: &PgPool,
    thread_id: Uuid,
    run: &CheckpointRun,
    readings: &Readings,
) -> Result<HandoffVerdict, sqlx::Error> {
    let unknown = HandoffVerdict {
        name: run.name.clone(),
        progress: run.progress,
        pays: None,
        estimate: None,
    };
    let Some(inputs) = estimate_inputs(pool, thread_id, run.name.as_str(), readings).await? else {
        return Ok(unknown);
    };
    let warmth = cache_warmth(pool, thread_id).await?;
    let mut estimate = inputs.estimate(run.units_left(), warmth);
    let context = super::successor::thread_context(pool, thread_id).await?;
    estimate.pays &= context.can_hand_off && !holds_open_change(pool, thread_id).await?;
    Ok(HandoffVerdict {
        pays: Some(estimate.pays),
        estimate: Some(estimate),
        ..unknown
    })
}

/// What the estimate reads off a thread and its history, before the units
/// left and the warmth are known.
struct EstimateInputs {
    context_tokens: u64,
    price: ModelPrice,
    calls_per_unit: f64,
    fresh_tokens: f64,
}

impl EstimateInputs {
    fn estimate(&self, units_left: u32, warmth: CacheWarmth) -> HandoffEstimate {
        estimate(
            self.context_tokens,
            &self.price,
            units_left,
            self.calls_per_unit,
            self.fresh_tokens,
            warmth,
        )
    }
}

/// `None` when the engine cannot make a reading: no context size yet, or a
/// model with no price row.
async fn estimate_inputs(
    pool: &PgPool,
    thread_id: Uuid,
    checkpoint: &str,
    readings: &Readings,
) -> Result<Option<EstimateInputs>, sqlx::Error> {
    let (Some(context_tokens), Some(model)) = (readings.context_tokens, &readings.model) else {
        return Ok(None);
    };
    let Some(price) = price_of(model) else {
        return Ok(None);
    };
    let key = HistoryKey {
        repo: thread_repo(pool, thread_id).await?,
        name: checkpoint.to_string(),
        model: model.clone(),
        reasoning_effort: readings.reasoning_effort.clone(),
    };
    let calls_per_unit = calls_per_unit(pool, &key, context_tokens)
        .await?
        .unwrap_or(CALLS_PER_UNIT_FALLBACK);
    let fresh_tokens = fresh_tokens_median(pool)
        .await?
        .unwrap_or(FRESH_TOKENS_FALLBACK);
    Ok(Some(EstimateInputs {
        context_tokens,
        price,
        calls_per_unit,
        fresh_tokens,
    }))
}

/// The tokens a *Stay here* hides the Successor card for: it comes back once
/// the thread has grown this much past the size it was declined at.
pub const OFFER_DECLINE_HOLD_TOKENS: u64 = 100_000;

/// Whether a decline at `declined_at` still hides the card at `current`
/// tokens. A context that shrank below it (a compaction) lifts the hold.
pub(crate) fn offer_held_back(declined_at: Option<u64>, current: u64) -> bool {
    declined_at
        .is_some_and(|d| (d..=d.saturating_add(OFFER_DECLINE_HOLD_TOKENS)).contains(&current))
}

/// The Successor card's reading: whether a handoff pays on either cache
/// state, so the card flips from warm to cold with no second read.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct CardVerdict {
    /// The card shows while the cache is warm.
    pub warm: bool,
    /// The card shows once the cache is cold.
    pub cold: bool,
    /// When the cache goes cold, `None` when it already is.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cold_at: Option<DateTime<Utc>>,
    /// How many times a fresh thread's context the old one holds, for the
    /// card's "each reply costs about Nx".
    pub fresh_ratio: f64,
}

/// The Successor card's verdict for `thread_id`, `None` when the engine can
/// make no reading. The card never shows on a thread that cannot hand off,
/// one holding an open change, or one the user declined it on lately.
pub async fn card_verdict(
    pool: &PgPool,
    thread_id: Uuid,
    context: &ThreadContext,
) -> Result<Option<CardVerdict>, sqlx::Error> {
    let readings = Readings {
        context_tokens: context.input_tokens,
        model: thread_model(pool, thread_id).await?,
        reasoning_effort: thread_effort_in_force(pool, thread_id).await?,
    };
    let Some(inputs) = estimate_inputs(pool, thread_id, CARD_CHECKPOINT, &readings).await? else {
        return Ok(None);
    };
    let held = offer_held_back(
        super::successor::latest_offer_decline(pool, thread_id).await?,
        inputs.context_tokens,
    );
    let eligible = context.can_hand_off && !held && !holds_open_change(pool, thread_id).await?;
    let shows = |warmth| eligible && inputs.estimate(1, warmth).pays;
    let remaining_secs = cache_remaining_secs(pool, thread_id).await?;
    Ok(Some(CardVerdict {
        warm: shows(CacheWarmth::Warm),
        cold: shows(CacheWarmth::Cold),
        cold_at: remaining_secs.map(|secs| Utc::now() + chrono::Duration::seconds(secs)),
        fresh_ratio: inputs.context_tokens as f64 / inputs.fresh_tokens.max(1.0),
    }))
}

/// Record `run` on `thread_id` as a `HandoffCheckpointReached`, and return
/// its verdict. The record is the sample a later median reads.
pub async fn record_checkpoint(
    bus: &EventBus,
    pool: &PgPool,
    thread_id: Uuid,
    run: &CheckpointRun,
    offered: bool,
    actor: Option<MessageOrigin>,
) -> Result<HandoffVerdict, Box<dyn std::error::Error + Send + Sync>> {
    let readings = read_thread(pool, thread_id).await?;
    let verdict = verdict_from(pool, thread_id, run, &readings).await?;
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::HandoffCheckpointReached {
            name: run.name.as_str().to_string(),
            progress: run.progress,
            context_tokens: readings.context_tokens,
            model: readings.model,
            reasoning_effort: readings.reasoning_effort,
            offered,
            pays: verdict.pays,
        },
        meta: EventMeta::with_actor(actor),
    })
    .await?;
    Ok(verdict)
}

async fn read_thread(pool: &PgPool, thread_id: Uuid) -> Result<Readings, sqlx::Error> {
    Ok(Readings {
        context_tokens: super::successor::thread_context(pool, thread_id)
            .await?
            .input_tokens,
        model: thread_model(pool, thread_id).await?,
        reasoning_effort: thread_effort_in_force(pool, thread_id).await?,
    })
}

/// Whether `thread_id` holds a change the user may be applying: a pending
/// one, one applying now, or one in the landing lane. A handoff starts new
/// work on the branch, so an Apply waiting on this hardening would fail.
async fn holds_open_change(pool: &PgPool, thread_id: Uuid) -> Result<bool, sqlx::Error> {
    let open: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM changes WHERE thread_id = $1 AND status = $2) \
             OR EXISTS (SELECT 1 FROM thread_summaries \
                         WHERE thread_id = $1 AND apply_in_flight)",
    )
    .bind(thread_id)
    .bind(crate::core::changes::ChangeStatus::Pending)
    .fetch_one(pool)
    .await?;
    Ok(open || super::successor::awaits_landing(pool, thread_id).await?)
}

/// The repository `thread_id` works in, which a median pools on.
async fn thread_repo(pool: &PgPool, thread_id: Uuid) -> Result<Option<String>, sqlx::Error> {
    let sql = format!(
        "SELECT {} FROM thread_summaries t WHERE t.thread_id = $1",
        thread_repo_sql("t")
    );
    Ok(sqlx::query_scalar::<_, Option<String>>(&sql)
        .bind(thread_id)
        .fetch_optional(pool)
        .await?
        .flatten())
}

/// The model of `thread_id`'s latest main-session call.
pub(crate) async fn thread_model(
    pool: &PgPool,
    thread_id: Uuid,
) -> Result<Option<String>, sqlx::Error> {
    let sql = format!(
        "SELECT payload->>'model' FROM events \
         WHERE thread_id = $1 AND {MAIN_SESSION_CAPTURE} \
         ORDER BY sequence DESC LIMIT 1"
    );
    Ok(sqlx::query_scalar::<_, Option<String>>(&sql)
        .bind(thread_id)
        .fetch_optional(pool)
        .await?
        .flatten())
}

/// `thread_id`'s *effort in force*: the reasoning effort its latest
/// main-session call ran at. Before a call records one, the effort its
/// settings last named. A settings event can be a pick no call ran at yet.
pub(crate) async fn thread_effort_in_force(
    pool: &PgPool,
    thread_id: Uuid,
) -> Result<Option<String>, sqlx::Error> {
    let sql = format!(
        "SELECT COALESCE( \
           (SELECT payload->>'reasoning_effort' FROM events \
             WHERE thread_id = $1 AND {MAIN_SESSION_CAPTURE} \
             ORDER BY sequence DESC LIMIT 1), \
           (SELECT payload->>'reasoning_effort' FROM events \
             WHERE thread_id = $1 AND event_type = 'CodingAgentSettingsChanged' \
               AND payload->>'reasoning_effort' IS NOT NULL \
             ORDER BY sequence DESC LIMIT 1))"
    );
    sqlx::query_scalar::<_, Option<String>>(&sql)
        .bind(thread_id)
        .fetch_one(pool)
        .await
}

#[cfg(test)]
#[path = "handoff_estimate_tests.rs"]
mod tests;
