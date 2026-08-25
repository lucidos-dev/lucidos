//! Per-thread cost, rounds, tools and notes, read from the arm's own database.
//!
//! Cost is computed here from event-store token counts and the pinned price
//! table, never from a provider bill (I8). A bill is not attributable per
//! thread, and the eval's whole cost claim is per thread.

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

/// Grouped domains whose reads recover something the prompt no longer carries.
///
/// A call to one of these after round 1 is what a lost fact costs: a round
/// spent going back for it. The rounds axis reports the count.
///
/// Curated, not derived. Most domains administer the workspace, and a read
/// there is not the agent going back for a dropped fact. Every other LLM domain
/// is named in `NOT_RECOVERY_DOMAINS`, in this file's test module. A new one
/// belongs to neither list, so it fails the drift test until somebody places it.
pub const RECOVERY_DOMAINS: [&str; 4] = ["events", "memory", "threads", "triggers"];

/// Recovery tools that belong to no domain, so no `action` dispatches them.
///
/// `read_file` leads this list on purpose. The audit found it the most common
/// recovery shape in the transcripts, and the old repeat metric ignored it.
pub const UNGROUPED_RECOVERY_TOOLS: [&str; 5] = [
    tool_names::READ_FILE,
    tool_names::GLOB_FILES,
    tool_names::GREP_FILES,
    tool_names::LOAD_KNOWHOW,
    tool_names::LIST_APPS,
];

/// The `producer` the engine stamps on a capture the agent itself made.
///
/// Any other value is auxiliary work: the title, the memory extractor, the
/// summariser. The engine writes it from `ContextProducer`, which renames to
/// snake_case on the wire.
pub const MAIN_LLM_PRODUCER: &str = "main_llm";

/// The four token counts one group of captures recorded.
///
/// Grouped because a price applies to all four together, and because the audit
/// found them summed per thread when they belong per model.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq)]
pub struct TokenCounts {
    pub cache_creation: i64,
    pub cache_read: i64,
    /// Everything the model processed on input, cached tokens INCLUDED.
    ///
    /// The name says total because it is one, and the two fields above it are
    /// parts of it rather than classes beside it. Both providers report input
    /// this way. Anthropic sums its three disjoint counts before storing, and
    /// OpenAI's `prompt_tokens` already covers `cached_tokens`.
    ///
    /// So never price this field. Call [`TokenCounts::input_split`], which is
    /// the only way to reach the disjoint form. Priced directly beside the two
    /// cache counts, every cached token is billed twice, once at the full rate.
    pub input_total: i64,
    pub output_tokens: i64,
}

impl TokenCounts {
    /// Take the cached parts out of the input total.
    pub fn input_split(&self) -> InputSplit {
        InputSplit::from_total(self.input_total, self.cache_read, self.cache_creation)
    }

    /// Whether this group recorded any usage at all.
    ///
    /// A reconstructed auxiliary capture carries no `usage` block, so every
    /// count is zero and there is nothing to price.
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

/// One model's captures on a thread, kept apart so each is priced at its rate.
///
/// A thread runs the agent on the model under test. Its title, memory and
/// summary work run on the auxiliary default, which is a different model at a
/// different price.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct ModelTokens {
    /// The model id the engine stamped on the capture.
    pub model: String,
    /// The engine's `producer`. See [`MAIN_LLM_PRODUCER`].
    pub producer: String,
    /// Captures in this group, which is where rounds are counted.
    pub captures: i64,
    pub counts: TokenCounts,
}

impl ModelTokens {
    /// Whether the agent itself made these calls.
    pub fn is_main_agent(&self) -> bool {
        self.producer == MAIN_LLM_PRODUCER
    }
}

/// Token totals and wall clock for one thread, split per model.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct ThreadTokens {
    /// Every model that captured on this thread, main agent and auxiliary.
    pub by_model: Vec<ModelTokens>,
    pub started: Option<DateTime<Utc>>,
    pub ended: Option<DateTime<Utc>>,
}

impl ThreadTokens {
    /// Rounds the agent took. An auxiliary call is spend, not a round.
    ///
    /// Counting the classifier, the title and the summariser as rounds would
    /// inflate the denominator of every per-round figure.
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

    /// Every token the thread caused, whoever spent it.
    pub fn combined(&self) -> TokenCounts {
        self.fold(|_| true)
    }

    /// The title, the memory extractor and the summariser.
    ///
    /// The agent's own share is [`ThreadTokens::combined`] less this. Only one
    /// of the two is stored on a result row, so there is one place they can
    /// disagree, and it is none.
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

/// The three input classes as disjoint counts, which is the only shape a price
/// applies to.
///
/// The fields are private and [`InputSplit::from_total`] is the sole
/// constructor, so no caller can reach a dollar figure while still holding the
/// overlapping total. That is the whole point of the type, and the subtraction
/// below is the only one in the crate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InputSplit {
    fresh: i64,
    cache_read: i64,
    cache_creation: i64,
}

impl InputSplit {
    /// Split a stored input total, which contains both cached counts.
    ///
    /// Saturating and floored at zero. A row claiming more cached than it
    /// processed is corrupt, not a negative count.
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

    /// Input the provider read fresh, billed at the full input rate.
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

/// What the run records for one thread, beside its probe outcomes.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct ThreadMetrics {
    pub tokens: ThreadTokens,
    pub todo_writes: i64,
    /// How the model wrote its working understanding.
    pub document: DocumentWrites,
    pub recovery_calls: BTreeMap<String, i64>,
    /// Recovery calls after round 2 that re-fetch a handle already fetched.
    pub repeat_recoveries: i64,
    /// Rounds where the engine trimmed the context to fit the budget.
    pub trimmed_rounds: i64,
    /// How full this thread's requests got, and against what window.
    pub utilisation: Utilisation,
    pub spend: Spend,
    pub memory_recalled: bool,
}

/// Dollars for one thread, with auxiliary work told apart.
///
/// The headline stays combined, because the run pays for every token it
/// caused. The split is beside it so a cost difference between arms can be read
/// as the agent's or as its title and memory work.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq)]
pub struct Spend {
    /// Every dollar the thread caused, each group at its own model's rate.
    pub total: f64,
    /// The part of `total` spent on title, memory and summary work.
    pub auxiliary: f64,
}

/// How the model wrote its working understanding, per thread.
///
/// Three questions the design turns on, and none was measurable before it. The
/// record's rates were all taken when a write cost a whole round, so none of
/// them is a prediction about this design.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq)]
pub struct DocumentWrites {
    /// Replies that carried a span.
    pub writes: i64,
    /// Writes that shared their reply with a tool call, which is the whole
    /// point of moving the document out of one.
    pub writes_with_a_tool_call: i64,
    /// Addresses the model held open.
    pub items_held_open: i64,
}

/// The document-behaviour query, lifted out so a test can read its shape.
///
/// `beside_a_call` ends a write's interval at the next round, and a round is a
/// main-LLM capture. An auxiliary one, the title or the memory extractor, lands
/// wherever its detached task happened to finish. Left unfiltered it closed the
/// interval early and dropped a tool call that really did share the reply.
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

/// Count the three, from the event log alone.
///
/// A write shares its reply with a tool call when a `ToolCalled` lands between
/// it and the next round's `ContextCaptured`. The loop parses the span the
/// moment the reply arrives, before it runs anything. That ordering is what
/// "in the same reply" means on the wire.
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

/// Tokens per thread, grouped by the model and producer that spent them.
///
/// **A round is a main-LLM call, and the token groups are every call.** The two
/// differ because an auxiliary call is real spend the run should be charged for,
/// and is not a round the agent took.
///
/// The grouping is what lets each model be priced at its own rate. Summed flat,
/// a Gemini title is billed as Opus, and the arm making more auxiliary calls is
/// flattered by exactly that error.
pub async fn thread_tokens(pool: &PgPool, thread_id: Uuid) -> Fallible<ThreadTokens> {
    let rows = sqlx::query(
        // `sum()` over bigint returns NUMERIC, not INT8, so every total is cast
        // back. Without the cast the read fails on the first thread that
        // produced usage. No test caught it: the unit tests build ThreadTokens
        // directly rather than through this query.
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
        let started: Option<DateTime<Utc>> = row.try_get("started")?;
        let ended: Option<DateTime<Utc>> = row.try_get("ended")?;
        tokens.started = earliest(tokens.started, started);
        tokens.ended = latest(tokens.ended, ended);
        tokens.by_model.push(ModelTokens {
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
        });
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

/// Rounds where the engine had to trim the context to make it fit.
///
/// The only event-visible sign that a thread reached the ceiling. It covers the
/// in-turn trim alone: the cross-turn one runs at turn setup, before any capture
/// is written, and it emits nothing. So zero here means "this turn never
/// overflowed", not "nothing was ever dropped".
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

/// Sequence at which round 2 began, or `None` for a single-round thread.
///
/// Everything the harness calls a recovery is measured after this point,
/// because round 1 carries the payload in every configuration.
///
/// `main_llm` only. The title generator fires on a thread's second message, so
/// an auxiliary capture can land where round 2 should be. The floor would then
/// sit before the agent's second call and count round 1's work as a recovery.
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

/// Sequence of the thread's nth prompt, counting from one.
///
/// A prompt is a `MessageReceived`, which on an eval thread is the driver's own
/// post and nothing else. A wake writes `UserPromptInjected` and a scripted
/// answer writes `UserQuestionAnswered`.
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

/// The last sequence written on a thread, or zero for a thread with none.
///
/// Every event counts, whoever wrote it. Auxiliary work is exactly what this
/// watches for, and it writes a `ContextCaptured` and often a title beside it.
/// A filter on producer would leave the harness blind to the second.
pub async fn highest_sequence(pool: &PgPool, thread_id: Uuid) -> Fallible<i64> {
    let highest: Option<i64> =
        sqlx::query_scalar("SELECT max(sequence) FROM events WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_one(pool)
            .await?;
    Ok(highest.unwrap_or(0))
}

/// The turn-two boundary of a run recorded before the driver wrote it down.
///
/// Those runs posted exactly one prompt per turn, so the second prompt is the
/// follow-up. A run that re-posts an empty completion breaks that premise. The
/// driver now records [`crate::driver::DrivenTask::followup_sequence`], and
/// this is only the fallback for the older files.
pub async fn second_prompt_sequence(pool: &PgPool, thread_id: Uuid) -> Fallible<Option<i64>> {
    prompt_sequence(pool, thread_id, 2).await
}

/// Whether the engine retrieved memory on this thread at all.
///
/// The engine emits `MemoryRecalled` only when its query classifier answered
/// yes. An absent event means retrieval was skipped, so the thread carried no
/// memory payload. The classifier is an LLM call and the two arms can disagree
/// about the same task, which is why the harness records this.
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

/// Recovery calls after round 2 began, counted per canonical name.
///
/// Classified in Rust over the rows [`tool_calls`] already fetched, so the SQL
/// no longer carries a copy of the tool vocabulary that can go stale. A thread
/// with no second round has nothing to measure, exactly as the old query's
/// empty boundary gave it.
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

/// Every `TodoListWritten` on the thread, oldest first.
///
/// Both halves matter: the count is a cost metric, and the payloads are what
/// the `from-notes` route reads.
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

/// Every `ToolCalled` on the thread, oldest first.
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

/// Every event in the workspace, oldest first. What the assertions read.
///
/// Workspace-wide rather than per thread. The event store is the workspace's
/// memory, so a later task re-testing an earlier fact has to see the event that
/// established it. Tool calls stay per thread, because those are behaviour.
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

/// Dollars for one thread's token counts at the pinned prices (I8).
///
/// Each group is priced at its own model's row. A thread bills the agent on the
/// model under test, and its title and memory work on the auxiliary default.
/// Pricing them together charges Gemini tokens at Opus rates.
///
/// Fresh input, output, cache read and cache creation, each priced per million.
/// The fresh count comes from [`TokenCounts::input_split`], because the stored
/// input total already contains the two cached counts. Nothing is rounded until
/// it is printed, so summing per-thread figures gives the per-run figure
/// exactly.
///
/// An observed model with no `prices.toml` row fails the run, as that file's
/// header requires. A group that recorded no usage is skipped first. Zero
/// tokens cost zero at any rate, and a reconstructed capture names a model
/// nobody priced.
pub fn usd(tokens: &ThreadTokens, prices: &[ModelPrice]) -> Fallible<Spend> {
    let mut spend = Spend::default();
    for group in &tokens.by_model {
        if group.counts.is_zero() {
            continue;
        }
        let price = prices
            .iter()
            .find(|price| price.id == group.model)
            .ok_or_else(|| {
                format!(
                    "prices.toml has no row for model {:?}, seen on a {:?} capture",
                    group.model, group.producer
                )
            })?;
        let dollars = price_counts(&group.counts, price);
        spend.total += dollars;
        if !group.is_main_agent() {
            spend.auxiliary += dollars;
        }
    }
    Ok(spend)
}

/// One group's dollars at one model's rates.
fn price_counts(counts: &TokenCounts, price: &ModelPrice) -> f64 {
    const PER_MTOK: f64 = 1_000_000.0;
    let input = counts.input_split();
    (input.fresh() as f64 * price.input_per_mtok
        + counts.output_tokens as f64 * price.output_per_mtok
        + input.cache_read() as f64 * price.cache_read_per_mtok
        + input.cache_creation() as f64 * price.cache_creation_per_mtok)
        / PER_MTOK
}

/// One recovery call, resolved against the tool surface the engine ships.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Recovery {
    /// The canonical name, with the action folded in: `events(query)`. A flat
    /// alias and its grouped form report under the same name.
    pub name: String,
    /// What the call went after, so the same fetch twice is one handle.
    pub handle: String,
}

/// Whether a tool call recovers something, and what it went back for.
///
/// One classifier serves both the per-tool counts and the repeat count, so the
/// two columns cannot disagree about the vocabulary. That was the audit's
/// finding: a flat list of eleven names, written before the tools were grouped
/// behind an `action`, silently counted zero for the new shape.
///
/// A grouped call resolves through the capability manifest. Its domain has to
/// be a recovery domain, and its operation has to be non-mutating.
/// `events(action="emit")` is a write and fails the second test.
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

/// The canonical name of a grouped call, when the call is a recovery read.
///
/// A flat legacy name resolves through its operation's alias, so an old row
/// still counts. Anything else reads the call's own `action` argument.
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

/// The call's arguments as one comparable string, with `action` taken out.
///
/// `action` already sits in the canonical name, so keeping it here would double
/// it. What is left is what the call went after: a path, a doc id, a query.
/// `serde_json` holds an object's keys sorted, so two identical calls render
/// identically whatever order the model wrote them in.
///
/// Every remaining argument counts, pagination included. See
/// [`repeat_recoveries`] for what that costs the count, and why it stays.
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

/// Recovery calls after round 2 that fetch a handle the thread already fetched.
///
/// One re-fetch is the ordinary price of a fact that left the prompt. This
/// counts the ones after it, where the agent is paying for the same thing over
/// and over. That is the thrash signal, and it is the sharpest thing the rounds
/// axis reports.
///
/// Round 1 is excluded because the full payload is there in every
/// configuration, so a call in it is a fetch rather than a re-fetch. The first
/// fetch of a handle is never counted, wherever it happened.
///
/// The count is a floor. Two `read_file` calls on one path with different
/// `offset` values render as different handles, so a paged re-read is not a
/// repeat. Keying on the path alone would catch that and break the honest case,
/// counting a page-through as thrash. Both arms run this rule (ADR 0087 I7).
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
        // `insert` answers "was this already there", which is exactly the
        // question, and it records the first fetch in the same step.
        if !seen.insert(recovery.handle) && call.sequence > round_two {
            repeats += 1;
        }
    }
    repeats
}

/// How full this thread's requests got, and against what.
///
/// Every figure is the engine's own, read off `ContextCaptured`. The engine
/// estimates a request's tokens at a measured 2.5 chars per token and records
/// the window it resolved, so the harness re-derives neither.
///
/// The peak is per thread rather than per run, because the peak is what decides
/// whether a thread ever felt its budget. Averaging it across a run would hide
/// the one task that did.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq)]
pub struct Utilisation {
    pub peak_request_tokens: i64,
    pub mean_request_tokens: i64,
    /// The window the engine resolved for the model under test. Zero when the
    /// thread captured nothing, which is what a thread that never ran did.
    pub context_window: i64,
}

/// Read the peak, the mean and the window off one thread's captures.
///
/// `producer = main_llm` is load-bearing here for the same reason it is in the
/// validity gate. An auxiliary call writes its own `ContextCaptured` for the
/// classifier, the title and the summariser, and those are small. Counted in,
/// they drag the mean down and describe a request nobody made.
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

/// Collect everything the results file records for one thread.
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

    /// The LLM domains a read of which is not context recovery.
    ///
    /// It is what a reader checks to see that a domain was excluded on purpose.
    /// Making the drift test exhaustive is its only job, so it lives here: I4
    /// allows one test region per file, and this is that region.
    const NOT_RECOVERY_DOMAINS: [&str; 10] = [
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
    ];

    /// The model under test.
    fn main_price() -> ModelPrice {
        ModelPrice {
            id: "test-model".into(),
            input_per_mtok: 15.0,
            output_per_mtok: 75.0,
            cache_read_per_mtok: 1.5,
            cache_creation_per_mtok: 18.75,
        }
    }

    /// The cheap model the title and the memory extractor run on.
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

    /// A thread whose only captures are the agent's own, on the priced model.
    fn main_thread(counts: TokenCounts) -> ThreadTokens {
        ThreadTokens {
            by_model: vec![group("test-model", MAIN_LLM_PRODUCER, counts)],
            ..ThreadTokens::default()
        }
    }

    /// I8: the arithmetic against a thread computed by hand.
    ///
    /// The 560,000 total is 500,000 cached reads, 40,000 cache writes and
    /// 20,000 fresh. Fresh at $15 is $0.30. 4,000 output at $75 is $0.30. The
    /// reads at $1.50 are $0.75, and the writes at $18.75 are $0.75.
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

    /// The defect this type exists to prevent: cached tokens billed twice.
    ///
    /// 100,000 processed, of which 90,000 were read from cache and 8,000 were
    /// written to it. Only 2,000 were fresh. Priced flat, the whole total lands
    /// at the input rate as well as at the cache rates. That is what overstated
    /// every dollar figure this harness printed before run 6.
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

        // 2,000 fresh at $15 is $0.03. 1,000 output at $75 is $0.075. 90,000
        // reads at $1.50 are $0.135. 8,000 writes at $18.75 are $0.15.
        let total = usd(&tokens, &prices()).expect("priced").total;
        assert!((total - 0.39).abs() < 1e-9, "expected 0.39 and got {total}");

        // Flat pricing would have charged the whole total at the input rate.
        let flat = total + 98_000.0 * 15.0 / 1_000_000.0;
        assert!(flat > total * 4.0, "the flat form is the 4x overstatement");
    }

    /// Audit finding 1: a Gemini title used to be billed at the Opus rate.
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

        // Main: 100,000 fresh at $15 is $1.50, 10,000 output at $75 is $0.75.
        // Auxiliary: 20,000 at $0.50 is $0.01, 2,000 at $3.00 is $0.006.
        let spend = usd(&tokens, &prices()).expect("priced");
        let main_agent = spend.total - spend.auxiliary;
        assert!((main_agent - 2.25).abs() < 1e-9, "{spend:?}");
        assert!((spend.auxiliary - 0.016).abs() < 1e-9, "{spend:?}");
        assert!((spend.total - 2.266).abs() < 1e-9, "{spend:?}");

        // The defect, restated: the same tokens at the main model's rate.
        let as_main = price_counts(&tokens.auxiliary(), &main_price());
        assert!(
            as_main > spend.auxiliary * 20.0,
            "the old form was {as_main}"
        );
    }

    /// Nothing falls between the two subtotals, in tokens or in dollars.
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

    /// The `prices.toml` rule: a missing row fails rather than defaulting.
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

    /// A reconstructed auxiliary capture carries no usage block at all.
    ///
    /// Its model is whatever the engine wrote down, often `unknown`. Zero
    /// tokens cost zero at any rate, so demanding a row for it would fail a run
    /// over spend nobody recorded.
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

    /// The span covers every group, and a query returns one row per group.
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

    /// The shape the old flat list could not see. A grouped read dispatches on
    /// `action`, so `triggers` alone said nothing about what the call did.
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

    /// The old list counted this as a recovery read. It is a write.
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

    /// An old row names the flat tool. It has to keep counting, and under the
    /// same name as the grouped form, or the two shapes split one metric.
    #[test]
    fn a_flat_alias_and_its_grouped_form_are_one_name() {
        let flat = recovery_name(&call(1, "query_events", r#"{"limit":10}"#));
        let grouped = recovery_name(&call(2, "events", r#"{"action":"query","limit":10}"#));
        assert_eq!(flat, Some("events(query)".to_string()));
        assert_eq!(flat, grouped);
    }

    /// A domain read that is workspace administration, not context recovery.
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

    /// A grouped call naming no action resolves to no operation.
    #[test]
    fn a_grouped_call_with_no_action_is_not_classified() {
        assert_eq!(recovery_name(&call(1, "events", r#"{"limit":10}"#)), None);
        assert_eq!(recovery_name(&call(1, "memory", "not json")), None);
    }

    /// The audit's most common recovery shape, and the one the old repeat
    /// metric ignored outright.
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

    /// Two paths are two handles. The argument is what was fetched.
    #[test]
    fn reading_two_paths_is_never_a_repeat() {
        let calls = [
            call(30, "read_file", r#"{"path":"notes.md"}"#),
            call(40, "read_file", r#"{"path":"plan.md"}"#),
            call(50, "load_knowhow", r#"{"id":"js-sdk"}"#),
        ];
        assert_eq!(repeat_recoveries(&calls, Some(20)), 0);
    }

    /// Round 1 carries the full payload in both arms, so a call there is a
    /// fetch rather than a re-fetch. It still records the handle.
    #[test]
    fn a_repeat_inside_round_one_does_not_count_but_is_remembered() {
        let calls = [
            call(5, "events", r#"{"action":"query","event_id":"evt-abc"}"#),
            call(8, "events", r#"{"action":"query","event_id":"evt-abc"}"#),
            call(30, "events", r#"{"action":"query","event_id":"evt-abc"}"#),
        ];
        assert_eq!(repeat_recoveries(&calls, Some(20)), 1);
    }

    /// A single-round thread has no round 2, so nothing is measurable.
    #[test]
    fn a_thread_that_never_reached_round_two_reports_nothing() {
        let calls = [
            call(10, "load_knowhow", r#"{"id":"js-sdk"}"#),
            call(12, "load_knowhow", r#"{"id":"js-sdk"}"#),
        ];
        assert_eq!(repeat_recoveries(&calls, None), 0);
        assert!(recovery_calls(&calls, None).is_empty());
    }

    /// Two tools naming the same string are two handles, not one.
    #[test]
    fn the_same_string_under_two_tools_is_two_handles() {
        let calls = [
            call(30, "load_knowhow", r#"{"id":"deploy"}"#),
            call(40, "read_file", r#"{"id":"deploy"}"#),
        ];
        assert_eq!(repeat_recoveries(&calls, Some(20)), 0);
    }

    /// The counts and the repeats read one classifier, so a call either feeds
    /// both columns or neither.
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

    /// The drift guard. A new LLM domain has to be placed on one side, so the
    /// vocabulary cannot go stale in silence the way the flat list did.
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

    /// Every action of a recovery domain classifies by its `mutating` flag, so
    /// a new read is counted and a new write is not.
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

    /// The query needs a database, so the regression is on its shape. What it
    /// protects against is an auxiliary capture interleaved between a document
    /// write and the tool call that shared its reply.
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

    /// An ungrouped tool that grew a domain would be decided by the grouped
    /// branch instead, and drop out of the count.
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
