use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

use super::event_bus::{BusEvent, EventBus};
use super::handoff_history::{calls_per_unit, fresh_tokens_median, thread_repo_sql, HistoryKey};
use super::switch_estimate::{cache_remaining, CacheRemaining};
use super::thread_events::{EventMeta, MessageOrigin, ThreadEvent};
use crate::engine::handoff_state::ThreadContext;
use crate::engine::session_capture_sql::current_session_capture_sql;
use crate::llm::model_prices::{price_of, ModelPrice};
use crate::runtime::intelligence_ladder::{IntelligenceLadder, IntelligenceRung};
use crate::runtime::CodingAgent;

pub use crate::engine::thread_events::HANDOFF_FLOOR_TOKENS;

pub const CALLS_PER_UNIT_FALLBACK: f64 = 40.0;

pub const FRESH_TOKENS_FALLBACK: f64 = 60_000.0;

const NARRATIVE_OUTPUT_TOKENS: f64 = 4_000.0;

pub const CARD_CHECKPOINT: &str = "successor-card";

const MAX_NAME_LEN: usize = 64;

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

pub use crate::engine::thread_events::checkpoint::CheckpointProgress;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckpointRun {
    pub name: CheckpointName,
    pub progress: Option<CheckpointProgress>,
}

impl CheckpointRun {
    pub fn parse(name: &str, progress: Option<&str>) -> Result<Self, String> {
        Ok(Self {
            name: CheckpointName::parse(name)?,
            progress: progress.map(CheckpointProgress::parse).transpose()?,
        })
    }

    pub fn units_left(&self) -> u32 {
        self.progress.map_or(1, |p| p.units_left())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum CacheWarmth {
    Warm,
    Cold,
    Unknown,
}

impl CacheWarmth {
    fn of_remaining(remaining: CacheRemaining) -> Self {
        match remaining {
            CacheRemaining::Warm(_) => Self::Warm,
            CacheRemaining::Cold => Self::Cold,
            CacheRemaining::Unknown => Self::Unknown,
        }
    }
}

pub async fn cache_warmth(pool: &PgPool, thread_id: Uuid) -> Result<CacheWarmth, sqlx::Error> {
    Ok(CacheWarmth::of_remaining(
        cache_remaining(pool, thread_id).await?,
    ))
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct HandoffEstimate {
    pub pays: bool,
    pub warmth: CacheWarmth,
    pub units_left: u32,
    pub calls_per_unit: f64,
    pub calls_left: f64,
    pub fresh_tokens: f64,
    pub saving_usd: f64,
    pub cost_usd: f64,
}

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
        // INVARIANT: Neither a narrative turn's warm read nor a cold rewrite can be
        // claimed, so only the calls left weigh. The handoff takes the brief.
        CacheWarmth::Unknown => {}
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OtherBackendRungs {
    pub coding_agent: CodingAgent,
    pub floor_rung: IntelligenceRung,
    pub risk_rung: Option<IntelligenceRung>,
}

impl OtherBackendRungs {
    pub fn on_ladder(
        ladder: &IntelligenceLadder,
        refused: impl Fn(CodingAgent, &str) -> bool,
    ) -> Option<Self> {
        if !ladder.climbs {
            return None;
        }
        let coding_agent = ladder.coding_agent?;
        let runs = |rung: &&IntelligenceRung| !refused(coding_agent, &rung.model);
        let floor = ladder.default_intelligence_rung.as_ref().filter(runs)?;
        let at = ladder.on_position_of(floor)?;
        let on = ladder.pairs();
        Some(Self {
            coding_agent,
            floor_rung: floor.clone(),
            risk_rung: on
                .get(at + 1)
                .filter(|_| at + 2 < on.len())
                .filter(runs)
                .cloned(),
        })
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct FloorRungs {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub floor_rung: Option<IntelligenceRung>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub risk_rung: Option<IntelligenceRung>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub other_backends: Vec<OtherBackendRungs>,
}

impl FloorRungs {
    pub(crate) fn offered(
        on: &[IntelligenceRung],
        default_at: Option<usize>,
        thread_at: Option<usize>,
    ) -> Self {
        let (Some(default_at), Some(thread_at)) = (default_at, thread_at) else {
            return Self::default();
        };
        let below = |at: usize| on.get(at).filter(|_| thread_at < at).cloned();
        let risk_at = default_at + 1;
        Self {
            other_backends: Vec::new(),
            floor_rung: below(default_at),
            risk_rung: below(risk_at).filter(|_| risk_at + 1 < on.len()),
        }
    }

    pub fn on_ladder(
        ladder: &IntelligenceLadder,
        thread: Option<&IntelligenceRung>,
        refused: impl Fn(CodingAgent, &str) -> bool,
    ) -> Self {
        if !ladder.climbs {
            return Self::default();
        }
        let at = |rung: &IntelligenceRung| ladder.on_position_of(rung);
        let offered = Self::offered(
            &ladder.pairs(),
            ladder.default_intelligence_rung.as_ref().and_then(at),
            thread.and_then(at),
        );
        let runs = |rung: &IntelligenceRung| {
            ladder
                .coding_agent
                .is_none_or(|agent| !refused(agent, &rung.model))
        };
        Self {
            floor_rung: offered.floor_rung.filter(runs),
            risk_rung: offered.risk_rung.filter(runs),
            ..offered
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct HandoffVerdict {
    pub name: CheckpointName,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub progress: Option<CheckpointProgress>,
    pub pays: Option<bool>,
    pub estimate: Option<HandoffEstimate>,
    pub past_ceiling: bool,
    #[serde(flatten)]
    pub rungs: FloorRungs,
}

#[derive(Debug, Clone, Default, PartialEq)]
struct Readings {
    context_tokens: Option<u64>,
    model: Option<String>,
    reasoning_effort: Option<String>,
}

pub async fn handoff_verdict(
    pool: &PgPool,
    thread_id: Uuid,
    run: &CheckpointRun,
) -> Result<HandoffVerdict, sqlx::Error> {
    let readings = read_thread(pool, thread_id).await?;
    verdict_from(pool, thread_id, run, &readings, FloorRungs::default()).await
}

async fn verdict_from(
    pool: &PgPool,
    thread_id: Uuid,
    run: &CheckpointRun,
    readings: &Readings,
    offered: FloorRungs,
) -> Result<HandoffVerdict, sqlx::Error> {
    let context = crate::engine::handoff_state::thread_context(pool, thread_id).await?;
    let eligible = context.can_hand_off && !holds_open_change(pool, thread_id).await?;
    let unknown = HandoffVerdict {
        name: run.name.clone(),
        progress: run.progress,
        pays: None,
        estimate: None,
        past_ceiling: super::context_ceiling::checkpoint_past_ceiling(pool, thread_id).await?,
        rungs: if eligible {
            offered
        } else {
            FloorRungs::default()
        },
    };
    let Some(inputs) = estimate_inputs(pool, thread_id, run.name.as_str(), readings).await? else {
        return Ok(unknown);
    };
    let warmth = cache_warmth(pool, thread_id).await?;
    let mut estimate = inputs.estimate(run.units_left(), warmth);
    estimate.pays &= eligible;
    Ok(HandoffVerdict {
        pays: Some(estimate.pays),
        estimate: Some(estimate),
        ..unknown
    })
}

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

pub const OFFER_DECLINE_HOLD_TOKENS: u64 = 100_000;

pub(crate) fn offer_held_back(declined_at: Option<u64>, current: u64) -> bool {
    declined_at
        .is_some_and(|d| (d..=d.saturating_add(OFFER_DECLINE_HOLD_TOKENS)).contains(&current))
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct CardVerdict {
    pub warm: bool,
    pub cold: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cold_at: Option<DateTime<Utc>>,
    pub fresh_ratio: f64,
}

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
        crate::engine::handoff_state::latest_offer_decline(pool, thread_id).await?,
        inputs.context_tokens,
    );
    let eligible = context.can_hand_off && !held && !holds_open_change(pool, thread_id).await?;
    let shows = |warmth| eligible && inputs.estimate(1, warmth).pays;
    let remaining = cache_remaining(pool, thread_id).await?;
    let after_warm = match remaining {
        CacheRemaining::Unknown => CacheWarmth::Unknown,
        CacheRemaining::Warm(_) | CacheRemaining::Cold => CacheWarmth::Cold,
    };
    let cold_at = match remaining {
        CacheRemaining::Warm(secs) => Some(Utc::now() + chrono::Duration::seconds(secs)),
        CacheRemaining::Cold | CacheRemaining::Unknown => None,
    };
    Ok(Some(CardVerdict {
        warm: shows(CacheWarmth::Warm),
        cold: shows(after_warm),
        cold_at,
        fresh_ratio: inputs.context_tokens as f64 / inputs.fresh_tokens.max(1.0),
    }))
}

pub async fn record_checkpoint(
    bus: &EventBus,
    pool: &PgPool,
    thread_id: Uuid,
    run: &CheckpointRun,
    offered: bool,
    floor: Option<FloorRungs>,
    actor: Option<MessageOrigin>,
) -> Result<HandoffVerdict, Box<dyn std::error::Error + Send + Sync>> {
    let readings = read_thread(pool, thread_id).await?;
    let default_floor = floor.is_some();
    let verdict = verdict_from(pool, thread_id, run, &readings, floor.unwrap_or_default()).await?;
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
            default_floor,
            floor_rung: verdict.rungs.floor_rung.clone(),
            risk_rung: verdict.rungs.risk_rung.clone(),
        },
        meta: EventMeta::with_actor(actor),
    })
    .await?;
    Ok(verdict)
}

async fn read_thread(pool: &PgPool, thread_id: Uuid) -> Result<Readings, sqlx::Error> {
    Ok(Readings {
        context_tokens: crate::engine::handoff_state::thread_context(pool, thread_id)
            .await?
            .input_tokens,
        model: thread_model(pool, thread_id).await?,
        reasoning_effort: thread_effort_in_force(pool, thread_id).await?,
    })
}

/// INVARIANT: Whether `thread_id` holds a change the user may be applying: a pending
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
    Ok(open || crate::engine::handoff_state::awaits_landing(pool, thread_id).await?)
}

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

pub(crate) async fn thread_model(
    pool: &PgPool,
    thread_id: Uuid,
) -> Result<Option<String>, sqlx::Error> {
    let sql = format!(
        "SELECT payload->>'model' FROM events \
         WHERE thread_id = $1 AND {} \
         ORDER BY sequence DESC LIMIT 1",
        current_session_capture_sql("$1")
    );
    Ok(sqlx::query_scalar::<_, Option<String>>(&sql)
        .bind(thread_id)
        .fetch_optional(pool)
        .await?
        .flatten())
}

pub(crate) async fn thread_effort_in_force(
    pool: &PgPool,
    thread_id: Uuid,
) -> Result<Option<String>, sqlx::Error> {
    let sql = format!(
        "SELECT COALESCE( \
           (SELECT payload->>'reasoning_effort' FROM events \
             WHERE thread_id = $1 AND {} \
             ORDER BY sequence DESC LIMIT 1), \
           (SELECT payload->>'reasoning_effort' FROM events \
             WHERE thread_id = $1 AND event_type = 'CodingAgentSettingsChanged' \
               AND payload->>'reasoning_effort' IS NOT NULL \
             ORDER BY sequence DESC LIMIT 1))",
        current_session_capture_sql("$1")
    );
    sqlx::query_scalar::<_, Option<String>>(&sql)
        .bind(thread_id)
        .fetch_one(pool)
        .await
}

#[cfg(test)]
#[path = "handoff_estimate_tests.rs"]
mod tests;
