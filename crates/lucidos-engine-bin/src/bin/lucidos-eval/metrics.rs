use std::collections::{BTreeMap, HashSet};

use chrono::{DateTime, Utc};
use lucidos_engine::capability_manifest::{self, Domain};
use lucidos_engine::llm::tool_names;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sqlx::{PgPool, Row};
use uuid::Uuid;

use crate::assertions::{EventRow, ToolCall};
use crate::config::ModelPrice;

type Fallible<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

pub const RECOVERY_DOMAINS: [&str; 5] = ["events", "memory", "recall", "threads", "triggers"];

pub const UNGROUPED_RECOVERY_TOOLS: [&str; 5] = [
    tool_names::READ_FILE,
    tool_names::GLOB_FILES,
    tool_names::GREP_FILES,
    tool_names::LOAD_KNOWHOW,
    tool_names::LIST_APPS,
];

pub const MAIN_LLM_PRODUCER: &str = "main_llm";

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq)]
pub struct TokenCounts {
    pub cache_creation: i64,
    pub cache_read: i64,
    pub input_total: i64,
    pub output_tokens: i64,
}

impl TokenCounts {
    pub fn input_split(&self) -> InputSplit {
        InputSplit::from_total(self.input_total, self.cache_read, self.cache_creation)
    }

    pub fn is_zero(&self) -> bool {
        self.cache_creation == 0
            && self.cache_read == 0
            && self.input_total == 0
            && self.output_tokens == 0
    }

    pub fn plus(self, other: TokenCounts) -> TokenCounts {
        TokenCounts {
            cache_creation: self.cache_creation.saturating_add(other.cache_creation),
            cache_read: self.cache_read.saturating_add(other.cache_read),
            input_total: self.input_total.saturating_add(other.input_total),
            output_tokens: self.output_tokens.saturating_add(other.output_tokens),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct ModelTokens {
    pub model: String,
    pub producer: String,
    pub captures: i64,
    pub counts: TokenCounts,
}

impl ModelTokens {
    pub fn is_main_agent(&self) -> bool {
        self.producer == MAIN_LLM_PRODUCER
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct ThreadTokens {
    pub by_model: Vec<ModelTokens>,
    pub started: Option<DateTime<Utc>>,
    pub ended: Option<DateTime<Utc>>,
}

impl ThreadTokens {
    fn add(
        &mut self,
        group: ModelTokens,
        started: Option<DateTime<Utc>>,
        ended: Option<DateTime<Utc>>,
    ) {
        if group.is_main_agent() {
            self.started = earliest(self.started, started);
            self.ended = latest(self.ended, ended);
        }
        self.by_model.push(group);
    }

    pub fn rounds(&self) -> i64 {
        self.by_model
            .iter()
            .filter(|group| group.is_main_agent())
            .map(|group| group.captures)
            .sum()
    }

    pub fn wall_secs(&self) -> i64 {
        match (self.started, self.ended) {
            (Some(start), Some(end)) => (end - start).num_seconds(),
            _ => 0,
        }
    }

    pub fn combined(&self) -> TokenCounts {
        self.fold(|_| true)
    }

    pub fn auxiliary(&self) -> TokenCounts {
        self.fold(|group| !group.is_main_agent())
    }

    fn fold(&self, keep: impl Fn(&ModelTokens) -> bool) -> TokenCounts {
        self.by_model
            .iter()
            .filter(|group| keep(group))
            .fold(TokenCounts::default(), |acc, group| acc.plus(group.counts))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InputSplit {
    fresh: i64,
    cache_read: i64,
    cache_creation: i64,
}

impl InputSplit {
    pub fn from_total(input_total: i64, cache_read: i64, cache_creation: i64) -> Self {
        Self {
            fresh: input_total
                .saturating_sub(cache_read)
                .saturating_sub(cache_creation)
                .max(0),
            cache_read,
            cache_creation,
        }
    }

    pub fn fresh(&self) -> i64 {
        self.fresh
    }

    pub fn cache_read(&self) -> i64 {
        self.cache_read
    }

    pub fn cache_creation(&self) -> i64 {
        self.cache_creation
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct ThreadMetrics {
    pub tokens: ThreadTokens,
    pub todo_writes: i64,
    pub document: DocumentWrites,
    pub recovery_calls: BTreeMap<String, i64>,
    pub repeat_recoveries: i64,
    pub trimmed_rounds: i64,
    pub utilisation: Utilisation,
    pub spend: Spend,
    pub memory_recalled: bool,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq)]
pub struct Spend {
    pub total: f64,
    pub auxiliary: f64,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq)]
pub struct DocumentWrites {
    pub writes: i64,
    pub writes_with_a_tool_call: i64,
    pub items_held_open: i64,
}

const DOCUMENT_WRITES_SQL: &str = "SELECT \
   (SELECT count(*) FROM events \
     WHERE thread_id = $1 AND event_type = 'WorkingUnderstandingWritten')::int8 \
     AS writes, \
   (SELECT count(*) FROM events w \
     WHERE w.thread_id = $1 AND w.event_type = 'WorkingUnderstandingWritten' \
       AND EXISTS ( \
         SELECT 1 FROM events t \
          WHERE t.thread_id = $1 AND t.event_type = 'ToolCalled' \
            AND t.sequence > w.sequence \
            AND t.sequence < COALESCE(( \
              SELECT min(c.sequence) FROM events c \
               WHERE c.thread_id = $1 AND c.event_type = 'ContextCaptured' \
                 AND c.payload->>'producer' = 'main_llm' \
                 AND c.sequence > w.sequence), 9223372036854775807)))::int8 \
     AS beside_a_call, \
   (SELECT count(*) FROM events \
     WHERE thread_id = $1 AND event_type = 'ContextKeptOpen')::int8 AS held";

pub async fn document_writes(pool: &PgPool, thread_id: Uuid) -> Fallible<DocumentWrites> {
    let row = sqlx::query(DOCUMENT_WRITES_SQL)
        .bind(thread_id)
        .fetch_one(pool)
        .await?;
    Ok(DocumentWrites {
        writes: row.try_get("writes")?,
        writes_with_a_tool_call: row.try_get("beside_a_call")?,
        items_held_open: row.try_get("held")?,
    })
}

pub async fn thread_tokens(pool: &PgPool, thread_id: Uuid) -> Fallible<ThreadTokens> {
    let rows = sqlx::query(
        "SELECT COALESCE(payload->>'model', '')                                   AS model, \
                COALESCE(payload->>'producer', '')                                AS producer, \
                count(*)::int8                                                    AS captures, \
                sum((payload->'usage'->>'cache_creation_tokens')::bigint)::bigint AS cache_creation, \
                sum((payload->'usage'->>'cache_read_tokens')::bigint)::bigint     AS cache_read, \
                sum((payload->'usage'->>'input_tokens')::bigint)::bigint          AS input_total, \
                sum((payload->'usage'->>'output_tokens')::bigint)::bigint         AS output_tokens, \
                min(created)                                                      AS started, \
                max(created)                                                      AS ended \
           FROM events \
          WHERE event_type = 'ContextCaptured' AND thread_id = $1 \
          GROUP BY 1, 2 ORDER BY 1, 2",
    )
    .bind(thread_id)
    .fetch_all(pool)
    .await?;
    let mut tokens = ThreadTokens::default();
    for row in rows {
        let group = ModelTokens {
            model: row.try_get("model")?,
            producer: row.try_get("producer")?,
            captures: row.try_get("captures")?,
            counts: TokenCounts {
                cache_creation: row
                    .try_get::<Option<i64>, _>("cache_creation")?
                    .unwrap_or(0),
                cache_read: row.try_get::<Option<i64>, _>("cache_read")?.unwrap_or(0),
                input_total: row.try_get::<Option<i64>, _>("input_total")?.unwrap_or(0),
                output_tokens: row.try_get::<Option<i64>, _>("output_tokens")?.unwrap_or(0),
            },
        };
        tokens.add(group, row.try_get("started")?, row.try_get("ended")?);
    }
    Ok(tokens)
}

fn earliest(a: Option<DateTime<Utc>>, b: Option<DateTime<Utc>>) -> Option<DateTime<Utc>> {
    match (a, b) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (found, None) | (None, found) => found,
    }
}

fn latest(a: Option<DateTime<Utc>>, b: Option<DateTime<Utc>>) -> Option<DateTime<Utc>> {
    match (a, b) {
        (Some(a), Some(b)) => Some(a.max(b)),
        (found, None) | (None, found) => found,
    }
}

pub async fn trimmed_rounds(pool: &PgPool, thread_id: Uuid) -> Fallible<i64> {
    let row: (i64,) = sqlx::query_as(
        "SELECT count(*) FROM events \
          WHERE event_type = 'ContextCaptured' AND thread_id = $1 \
            AND payload->>'producer' = 'main_llm' \
            AND payload->>'trimmed' = 'true'",
    )
    .bind(thread_id)
    .fetch_one(pool)
    .await?;
    Ok(row.0)
}

pub async fn round_two_sequence(pool: &PgPool, thread_id: Uuid) -> Fallible<Option<i64>> {
    let row: Option<(i64,)> = sqlx::query_as(
        "SELECT sequence FROM events \
          WHERE event_type = 'ContextCaptured' AND thread_id = $1 \
            AND payload->>'producer' = 'main_llm' \
          ORDER BY sequence OFFSET 1 LIMIT 1",
    )
    .bind(thread_id)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|r| r.0))
}

pub async fn prompt_sequence(pool: &PgPool, thread_id: Uuid, nth: usize) -> Fallible<Option<i64>> {
    let row: Option<(i64,)> = sqlx::query_as(
        "SELECT sequence FROM events \
          WHERE event_type = 'MessageReceived' AND thread_id = $1 \
          ORDER BY sequence OFFSET $2 LIMIT 1",
    )
    .bind(thread_id)
    .bind(nth.saturating_sub(1) as i64)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|r| r.0))
}

pub async fn highest_sequence(pool: &PgPool, thread_id: Uuid) -> Fallible<i64> {
    let highest: Option<i64> =
        sqlx::query_scalar("SELECT max(sequence) FROM events WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_one(pool)
            .await?;
    Ok(highest.unwrap_or(0))
}

pub async fn second_prompt_sequence(pool: &PgPool, thread_id: Uuid) -> Fallible<Option<i64>> {
    prompt_sequence(pool, thread_id, 2).await
}

pub async fn memory_recalled(pool: &PgPool, thread_id: Uuid) -> Fallible<bool> {
    let row: Option<(i64,)> = sqlx::query_as(
        "SELECT sequence FROM events \
          WHERE event_type = 'MemoryRecalled' AND thread_id = $1 LIMIT 1",
    )
    .bind(thread_id)
    .fetch_optional(pool)
    .await?;
    Ok(row.is_some())
}

pub fn recovery_calls(
    calls: &[ToolCall],
    round_two_sequence: Option<i64>,
) -> BTreeMap<String, i64> {
    let Some(round_two) = round_two_sequence else {
        return BTreeMap::new();
    };
    let mut counts = BTreeMap::new();
    for call in calls.iter().filter(|call| call.sequence > round_two) {
        if let Some(recovery) = classify_recovery(call) {
            *counts.entry(recovery.name).or_insert(0) += 1;
        }
    }
    counts
}

pub async fn todo_writes(pool: &PgPool, thread_id: Uuid) -> Fallible<Vec<EventRow>> {
    let rows = sqlx::query(
        "SELECT sequence, payload::text AS payload, created FROM events \
          WHERE event_type = 'TodoListWritten' AND thread_id = $1 ORDER BY sequence",
    )
    .bind(thread_id)
    .fetch_all(pool)
    .await?;
    rows.into_iter()
        .map(|row| {
            Ok(EventRow {
                sequence: row.try_get("sequence")?,
                event_type: "TodoListWritten".to_string(),
                payload: row.try_get("payload")?,
                created: row.try_get("created")?,
            })
        })
        .collect()
}

pub async fn tool_calls(pool: &PgPool, thread_id: Uuid) -> Fallible<Vec<ToolCall>> {
    let rows = sqlx::query(
        "SELECT sequence, payload->>'name' AS name, COALESCE(payload->'args', '{}'::jsonb)::text \
                AS args \
           FROM events \
          WHERE event_type = 'ToolCalled' AND thread_id = $1 ORDER BY sequence",
    )
    .bind(thread_id)
    .fetch_all(pool)
    .await?;
    rows.into_iter()
        .map(|row| {
            Ok(ToolCall {
                sequence: row.try_get("sequence")?,
                name: row
                    .try_get::<Option<String>, _>("name")?
                    .unwrap_or_default(),
                args: row.try_get("args")?,
            })
        })
        .collect()
}

pub async fn workspace_events(pool: &PgPool) -> Fallible<Vec<EventRow>> {
    let rows = sqlx::query(
        "SELECT sequence, event_type, payload::text AS payload, created FROM events \
          ORDER BY sequence",
    )
    .fetch_all(pool)
    .await?;
    rows.into_iter()
        .map(|row| {
            Ok(EventRow {
                sequence: row.try_get("sequence")?,
                event_type: row.try_get("event_type")?,
                payload: row.try_get("payload")?,
                created: row.try_get("created")?,
            })
        })
        .collect()
}

pub fn usd(tokens: &ThreadTokens, prices: &[ModelPrice]) -> Fallible<Spend> {
    let mut spend = Spend::default();
    for group in &tokens.by_model {
        let dollars = priced(&group.model, &group.producer, &group.counts, prices)?;
        spend.total += dollars;
        if !group.is_main_agent() {
            spend.auxiliary += dollars;
        }
    }
    Ok(spend)
}

pub fn priced(
    model: &str,
    seen_on: &str,
    counts: &TokenCounts,
    prices: &[ModelPrice],
) -> Fallible<f64> {
    if counts.is_zero() {
        return Ok(0.0);
    }
    let price = prices
        .iter()
        .find(|price| price.id == model)
        .ok_or_else(|| {
            format!("prices.toml has no row for model {model:?}, seen on a {seen_on:?} capture")
        })?;
    Ok(price_counts(counts, price))
}

fn price_counts(counts: &TokenCounts, price: &ModelPrice) -> f64 {
    const PER_MTOK: f64 = 1_000_000.0;
    let input = counts.input_split();
    (input.fresh() as f64 * price.input_per_mtok
        + counts.output_tokens as f64 * price.output_per_mtok
        + input.cache_read() as f64 * price.cache_read_per_mtok
        + input.cache_creation() as f64 * price.cache_creation_per_mtok)
        / PER_MTOK
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Recovery {
    pub name: String,
    pub handle: String,
}

pub fn classify_recovery(call: &ToolCall) -> Option<Recovery> {
    let parsed = serde_json::from_str::<Value>(&call.args).ok();
    let args = parsed.as_ref().and_then(Value::as_object);
    let name = match capability_manifest::domain_for_tool(&call.name) {
        Some(domain) => grouped_recovery_name(domain, &call.name, args)?,
        None => UNGROUPED_RECOVERY_TOOLS
            .contains(&call.name.as_str())
            .then(|| call.name.clone())?,
    };
    let handle = format!("{name}|{}", handle_arguments(args));
    Some(Recovery { name, handle })
}

fn grouped_recovery_name(
    domain: &Domain,
    tool: &str,
    args: Option<&Map<String, Value>>,
) -> Option<String> {
    if !RECOVERY_DOMAINS.contains(&domain.name) {
        return None;
    }
    let on_llm = || domain.operations.iter().filter(|op| op.on_llm(domain));
    let operation = match on_llm().find(|op| op.llm_alias == Some(tool)) {
        Some(operation) => operation,
        None => {
            let action = args?.get("action")?.as_str()?;
            on_llm().find(|op| op.action == action)?
        }
    };
    (!operation.mutating).then(|| format!("{}({})", domain.name, operation.action))
}

fn handle_arguments(args: Option<&Map<String, Value>>) -> String {
    let Some(args) = args else {
        return String::new();
    };
    args.iter()
        .filter(|(key, _)| key.as_str() != "action")
        .map(|(key, value)| match value.as_str() {
            Some(text) => format!("{key}={}", text.trim()),
            None => format!("{key}={value}"),
        })
        .collect::<Vec<_>>()
        .join(",")
}

pub fn repeat_recoveries(calls: &[ToolCall], round_two_sequence: Option<i64>) -> i64 {
    let Some(round_two) = round_two_sequence else {
        return 0;
    };
    let mut seen: HashSet<String> = HashSet::new();
    let mut repeats = 0;
    for call in calls {
        let Some(recovery) = classify_recovery(call) else {
            continue;
        };
        if !seen.insert(recovery.handle) && call.sequence > round_two {
            repeats += 1;
        }
    }
    repeats
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq)]
pub struct Utilisation {
    pub peak_request_tokens: i64,
    pub mean_request_tokens: i64,
    pub context_window: i64,
}

pub async fn utilisation(pool: &PgPool, thread_id: Uuid) -> Fallible<Utilisation> {
    let row = sqlx::query(
        "SELECT max((payload->>'estimated_total_tokens')::bigint)::bigint  AS peak, \
                avg((payload->>'estimated_total_tokens')::bigint)::bigint  AS mean, \
                max((payload->>'context_window')::bigint)::bigint          AS window \
           FROM events \
          WHERE event_type = 'ContextCaptured' AND thread_id = $1 \
            AND payload->>'producer' = 'main_llm'",
    )
    .bind(thread_id)
    .fetch_one(pool)
    .await?;
    Ok(Utilisation {
        peak_request_tokens: row.try_get::<Option<i64>, _>("peak")?.unwrap_or(0),
        mean_request_tokens: row.try_get::<Option<i64>, _>("mean")?.unwrap_or(0),
        context_window: row.try_get::<Option<i64>, _>("window")?.unwrap_or(0),
    })
}

pub async fn collect(
    pool: &PgPool,
    thread_id: Uuid,
    prices: &[ModelPrice],
) -> Fallible<ThreadMetrics> {
    let tokens = thread_tokens(pool, thread_id).await?;
    let spend = usd(&tokens, prices)?;
    let calls = tool_calls(pool, thread_id).await?;
    let round_two = round_two_sequence(pool, thread_id).await?;
    Ok(ThreadMetrics {
        todo_writes: todo_writes(pool, thread_id).await?.len() as i64,
        document: document_writes(pool, thread_id).await?,
        recovery_calls: recovery_calls(&calls, round_two),
        repeat_recoveries: repeat_recoveries(&calls, round_two),
        trimmed_rounds: trimmed_rounds(pool, thread_id).await?,
        utilisation: utilisation(pool, thread_id).await?,
        memory_recalled: memory_recalled(pool, thread_id).await?,
        tokens,
        spend,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOT_RECOVERY_DOMAINS: [&str; 11] = [
        "changes",
        "env_vars",
        "mcp",
        "models",
        "notifications",
        "plugins",
        "preferences",
        "repositories",
        "thread_queue",
        "trigger_groups",
        "widgets",
    ];

    fn main_price() -> ModelPrice {
        ModelPrice {
            id: "test-model".into(),
            input_per_mtok: 15.0,
            output_per_mtok: 75.0,
            cache_read_per_mtok: 1.5,
            cache_creation_per_mtok: 18.75,
        }
    }

    fn aux_price() -> ModelPrice {
        ModelPrice {
            id: "test-aux".into(),
            input_per_mtok: 0.5,
            output_per_mtok: 3.0,
            cache_read_per_mtok: 0.05,
            cache_creation_per_mtok: 0.0,
        }
    }

    fn prices() -> Vec<ModelPrice> {
        vec![main_price(), aux_price()]
    }

    fn group(model: &str, producer: &str, counts: TokenCounts) -> ModelTokens {
        ModelTokens {
            model: model.to_string(),
            producer: producer.to_string(),
            captures: 1,
            counts,
        }
    }

    fn main_thread(counts: TokenCounts) -> ThreadTokens {
        ThreadTokens {
            by_model: vec![group("test-model", MAIN_LLM_PRODUCER, counts)],
            ..ThreadTokens::default()
        }
    }

    #[test]
    fn cost_matches_a_hand_computed_thread() {
        let tokens = main_thread(TokenCounts {
            cache_creation: 40_000,
            cache_read: 500_000,
            input_total: 560_000,
            output_tokens: 4_000,
        });
        let total = usd(&tokens, &prices()).expect("priced").total;
        assert!((total - 2.10).abs() < 1e-9, "expected 2.10 and got {total}");
    }

    #[test]
    fn cached_input_is_not_billed_at_the_fresh_rate_as_well() {
        let tokens = main_thread(TokenCounts {
            cache_creation: 8_000,
            cache_read: 90_000,
            input_total: 100_000,
            output_tokens: 1_000,
        });
        let fresh = tokens.combined().input_split().fresh();
        assert_eq!(fresh, 2_000, "2,000 were fresh");

        let total = usd(&tokens, &prices()).expect("priced").total;
        assert!((total - 0.39).abs() < 1e-9, "expected 0.39 and got {total}");

        let flat = total + 98_000.0 * 15.0 / 1_000_000.0;
        assert!(flat > total * 4.0, "the flat form is the 4x overstatement");
    }

    #[test]
    fn each_model_is_priced_at_its_own_rate() {
        let tokens = ThreadTokens {
            by_model: vec![
                group(
                    "test-model",
                    MAIN_LLM_PRODUCER,
                    TokenCounts {
                        input_total: 100_000,
                        output_tokens: 10_000,
                        ..TokenCounts::default()
                    },
                ),
                group(
                    "test-aux",
                    "auxiliary",
                    TokenCounts {
                        input_total: 20_000,
                        output_tokens: 2_000,
                        ..TokenCounts::default()
                    },
                ),
            ],
            ..ThreadTokens::default()
        };

        let spend = usd(&tokens, &prices()).expect("priced");
        let main_agent = spend.total - spend.auxiliary;
        assert!((main_agent - 2.25).abs() < 1e-9, "{spend:?}");
        assert!((spend.auxiliary - 0.016).abs() < 1e-9, "{spend:?}");
        assert!((spend.total - 2.266).abs() < 1e-9, "{spend:?}");

        let as_main = price_counts(&tokens.auxiliary(), &main_price());
        assert!(
            as_main > spend.auxiliary * 20.0,
            "the old form was {as_main}"
        );
    }

    #[test]
    fn the_split_sums_to_the_combined_total() {
        let tokens = ThreadTokens {
            by_model: vec![
                group(
                    "test-model",
                    MAIN_LLM_PRODUCER,
                    TokenCounts {
                        cache_creation: 3,
                        cache_read: 40,
                        input_total: 500,
                        output_tokens: 6_000,
                    },
                ),
                group(
                    "test-aux",
                    "auxiliary",
                    TokenCounts {
                        cache_creation: 70_000,
                        cache_read: 800_000,
                        input_total: 9_000_000,
                        output_tokens: 100,
                    },
                ),
            ],
            ..ThreadTokens::default()
        };
        let main_counts = tokens.by_model[0].counts;
        let aux_counts = tokens.by_model[1].counts;
        assert_eq!(tokens.auxiliary(), aux_counts, "the auxiliary group alone");
        let combined = main_counts.plus(aux_counts);
        assert_eq!(tokens.combined(), combined, "and both groups together");

        let spend = usd(&tokens, &prices()).expect("priced");
        let aux_dollars = price_counts(&aux_counts, &aux_price());
        assert!((spend.auxiliary - aux_dollars).abs() < 1e-9, "{spend:?}");
        let sum = price_counts(&main_counts, &main_price()) + aux_dollars;
        assert!((spend.total - sum).abs() < 1e-9, "{spend:?}");
    }

    #[test]
    fn a_model_with_no_price_row_fails_the_run() {
        let tokens = ThreadTokens {
            by_model: vec![group(
                "a-model-nobody-priced",
                "auxiliary",
                TokenCounts {
                    input_total: 10,
                    ..TokenCounts::default()
                },
            )],
            ..ThreadTokens::default()
        };
        let error = usd(&tokens, &prices()).expect_err("an unpriced model fails");
        assert!(
            error.to_string().contains("a-model-nobody-priced"),
            "the message names the model: {error}"
        );
    }

    #[test]
    fn a_capture_that_recorded_no_usage_needs_no_price_row() {
        let tokens = ThreadTokens {
            by_model: vec![group("unknown", "auxiliary", TokenCounts::default())],
            ..ThreadTokens::default()
        };
        assert_eq!(usd(&tokens, &prices()).expect("priced"), Spend::default());
    }

    #[test]
    fn rounds_count_main_llm_captures_alone() {
        let tokens = ThreadTokens {
            by_model: vec![
                ModelTokens {
                    captures: 6,
                    ..group("test-model", MAIN_LLM_PRODUCER, TokenCounts::default())
                },
                ModelTokens {
                    captures: 3,
                    ..group("test-aux", "auxiliary", TokenCounts::default())
                },
            ],
            ..ThreadTokens::default()
        };
        assert_eq!(
            tokens.rounds(),
            6,
            "the three auxiliary calls are not rounds"
        );
    }

    #[test]
    fn a_row_claiming_more_cached_than_it_processed_floors_at_zero() {
        let counts = TokenCounts {
            cache_read: 900,
            cache_creation: 500,
            input_total: 1_000,
            output_tokens: 0,
        };
        assert_eq!(counts.input_split().fresh(), 0, "never negative");
    }

    #[test]
    fn a_thread_with_no_rounds_costs_nothing() {
        let spend = usd(&ThreadTokens::default(), &prices()).expect("priced");
        assert_eq!(spend, Spend::default());
    }

    #[test]
    fn wall_clock_is_the_span_of_the_captured_rounds() {
        let start = DateTime::parse_from_rfc3339("2026-01-05T08:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let tokens = ThreadTokens {
            started: Some(start),
            ended: Some(start + chrono::Duration::seconds(412)),
            ..ThreadTokens::default()
        };
        assert_eq!(tokens.wall_secs(), 412);
    }

    #[test]
    fn the_span_widens_to_the_earliest_and_latest_group() {
        let start = DateTime::parse_from_rfc3339("2026-01-05T08:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let later = start + chrono::Duration::seconds(90);
        assert_eq!(earliest(Some(later), Some(start)), Some(start));
        assert_eq!(latest(Some(start), Some(later)), Some(later));
        assert_eq!(earliest(None, Some(start)), Some(start));
        assert_eq!(latest(Some(start), None), Some(start));
        assert_eq!(earliest(None, None), None);
    }

    #[test]
    fn auxiliary_work_after_the_turn_does_not_stretch_the_wall_clock() {
        let start = DateTime::parse_from_rfc3339("2026-01-05T08:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let mut tokens = ThreadTokens::default();
        tokens.add(
            group("test-model", MAIN_LLM_PRODUCER, TokenCounts::default()),
            Some(start),
            Some(start + chrono::Duration::seconds(60)),
        );
        tokens.add(
            group("test-aux", "auxiliary", TokenCounts::default()),
            Some(start + chrono::Duration::seconds(30)),
            Some(start + chrono::Duration::seconds(400)),
        );
        assert_eq!(tokens.wall_secs(), 60);
        assert_eq!(
            tokens.by_model.len(),
            2,
            "the auxiliary group is still priced"
        );
    }

    #[test]
    fn a_thread_that_never_captured_a_round_has_no_wall_clock() {
        assert_eq!(ThreadTokens::default().wall_secs(), 0);
    }

    fn call(sequence: i64, name: &str, args: &str) -> ToolCall {
        ToolCall {
            sequence,
            name: name.to_string(),
            args: args.to_string(),
        }
    }

    fn recovery_name(call: &ToolCall) -> Option<String> {
        classify_recovery(call).map(|r| r.name)
    }

    #[test]
    fn a_grouped_read_counts_under_its_domain_and_action() {
        assert_eq!(
            recovery_name(&call(1, "triggers", r#"{"action":"list"}"#)),
            Some("triggers(list)".to_string())
        );
        assert_eq!(
            recovery_name(&call(1, "memory", r#"{"action":"search","q":"deploy"}"#)),
            Some("memory(search)".to_string())
        );
        assert_eq!(
            recovery_name(&call(1, "threads", r#"{"action":"count"}"#)),
            Some("threads(count)".to_string())
        );
    }

    #[test]
    fn emitting_an_event_is_a_write_not_a_recovery() {
        assert_eq!(
            recovery_name(&call(1, "events", r#"{"action":"emit"}"#)),
            None
        );
        assert_eq!(
            recovery_name(&call(1, "emit_event", r#"{"summary":"x"}"#)),
            None
        );
        assert_eq!(
            recovery_name(&call(1, "memory", r#"{"action":"correct"}"#)),
            None
        );
        assert_eq!(
            recovery_name(&call(1, "triggers", r#"{"action":"delete","id":"t1"}"#)),
            None
        );
    }

    #[test]
    fn a_flat_alias_and_its_grouped_form_are_one_name() {
        let flat = recovery_name(&call(1, "query_events", r#"{"limit":10}"#));
        let grouped = recovery_name(&call(2, "events", r#"{"action":"query","limit":10}"#));
        assert_eq!(flat, Some("events(query)".to_string()));
        assert_eq!(flat, grouped);
    }

    #[test]
    fn a_read_outside_the_recovery_domains_is_not_recovery() {
        assert_eq!(
            recovery_name(&call(1, "preferences", r#"{"action":"get"}"#)),
            None
        );
        assert_eq!(
            recovery_name(&call(1, "changes", r#"{"action":"list"}"#)),
            None
        );
    }

    #[test]
    fn a_grouped_call_with_no_action_is_not_classified() {
        assert_eq!(recovery_name(&call(1, "events", r#"{"limit":10}"#)), None);
        assert_eq!(recovery_name(&call(1, "memory", "not json")), None);
    }

    #[test]
    fn re_reading_one_path_after_round_two_is_a_repeat() {
        let calls = [
            call(10, "read_file", r#"{"path":"notes.md"}"#),
            call(30, "read_file", r#"{"path":"notes.md"}"#),
            call(40, "read_file", r#"{"path":"notes.md"}"#),
        ];
        assert_eq!(repeat_recoveries(&calls, Some(20)), 2);
        assert_eq!(
            recovery_calls(&calls, Some(20)),
            [("read_file".to_string(), 2)].into_iter().collect()
        );
    }

    #[test]
    fn reading_two_paths_is_never_a_repeat() {
        let calls = [
            call(30, "read_file", r#"{"path":"notes.md"}"#),
            call(40, "read_file", r#"{"path":"plan.md"}"#),
            call(50, "load_knowhow", r#"{"id":"js-sdk"}"#),
        ];
        assert_eq!(repeat_recoveries(&calls, Some(20)), 0);
    }

    #[test]
    fn a_repeat_inside_round_one_does_not_count_but_is_remembered() {
        let calls = [
            call(5, "events", r#"{"action":"query","event_id":"evt-abc"}"#),
            call(8, "events", r#"{"action":"query","event_id":"evt-abc"}"#),
            call(30, "events", r#"{"action":"query","event_id":"evt-abc"}"#),
        ];
        assert_eq!(repeat_recoveries(&calls, Some(20)), 1);
    }

    #[test]
    fn a_thread_that_never_reached_round_two_reports_nothing() {
        let calls = [
            call(10, "load_knowhow", r#"{"id":"js-sdk"}"#),
            call(12, "load_knowhow", r#"{"id":"js-sdk"}"#),
        ];
        assert_eq!(repeat_recoveries(&calls, None), 0);
        assert!(recovery_calls(&calls, None).is_empty());
    }

    #[test]
    fn the_same_string_under_two_tools_is_two_handles() {
        let calls = [
            call(30, "load_knowhow", r#"{"id":"deploy"}"#),
            call(40, "read_file", r#"{"id":"deploy"}"#),
        ];
        assert_eq!(repeat_recoveries(&calls, Some(20)), 0);
    }

    #[test]
    fn one_classifier_serves_both_recovery_columns() {
        let calls = [
            call(30, "triggers", r#"{"action":"list"}"#),
            call(40, "triggers", r#"{"action":"list"}"#),
            call(50, "events", r#"{"action":"emit","summary":"done"}"#),
        ];
        assert_eq!(
            recovery_calls(&calls, Some(20)),
            [("triggers(list)".to_string(), 2)].into_iter().collect()
        );
        assert_eq!(repeat_recoveries(&calls, Some(20)), 1);
    }

    #[test]
    fn every_llm_domain_sits_on_one_side_of_the_recovery_line() {
        for domain in capability_manifest::domains().iter().filter(|d| d.llm) {
            let recovery = RECOVERY_DOMAINS.contains(&domain.name);
            let administrative = NOT_RECOVERY_DOMAINS.contains(&domain.name);
            assert!(
                recovery ^ administrative,
                "domain {:?} is in neither list or in both. Place it.",
                domain.name
            );
        }
    }

    #[test]
    fn a_recovery_domain_action_classifies_by_its_mutating_flag() {
        for name in RECOVERY_DOMAINS {
            let domain = capability_manifest::domains()
                .iter()
                .find(|d| d.name == name)
                .unwrap_or_else(|| panic!("no domain named {name:?}"));
            for operation in domain.operations.iter().filter(|op| op.on_llm(domain)) {
                let args = format!(r#"{{"action":"{}"}}"#, operation.action);
                let grouped = call(1, domain.tool_name, &args);
                assert_eq!(
                    classify_recovery(&grouped).is_some(),
                    !operation.mutating,
                    "{}({}) classified against its mutating flag",
                    domain.name,
                    operation.action
                );
            }
        }
    }

    #[test]
    fn the_document_write_boundary_is_a_main_llm_capture() {
        let (_, boundary) = DOCUMENT_WRITES_SQL
            .split_once("SELECT min(c.sequence)")
            .expect("the boundary subquery is no longer where the test looks");
        let clause = format!("c.payload->>'producer' = '{MAIN_LLM_PRODUCER}'");
        assert!(
            boundary.contains(&clause),
            "a title or memory capture would close the write's interval early"
        );
    }

    #[test]
    fn the_ungrouped_recovery_tools_belong_to_no_domain() {
        for tool in UNGROUPED_RECOVERY_TOOLS {
            assert!(
                capability_manifest::domain_for_tool(tool).is_none(),
                "{tool:?} now resolves to a domain, so the grouped branch decides it"
            );
        }
    }
}
