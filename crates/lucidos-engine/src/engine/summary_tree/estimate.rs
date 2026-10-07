//! What a Tree backfill would cost, read before anything is spent.
//!
//! **The counts are real, the sizes approximate.** One pass over `events`
//! counts each in-scope thread's entries the way [`super::log::project`] makes
//! them. A payload's stored size stands in for its entry's bytes, so the pass
//! takes about a second on millions of events. That size carries field
//! overhead and compression, so every figure is a range.
//!
//! **Token counts per call come from history where there is enough of it.**
//! [`measured_for_catalog`] averages real `ContextCaptured` rows for the
//! compactor, per model. A model with too little history uses its own seed,
//! measured per model and kept beside its price in [`COMPACTOR_MODELS`]. So two
//! models at one price are told apart by what each actually spends. Output
//! further scales with the reasoning tier: see [`scaled_output_tokens`].
//!
//! **A tier is priced as the tier the model runs at.** Routing snaps a tier
//! the model cannot run onto one it can, so the estimate clamps the same way.
//!
//! **Time follows the compactor's lanes.** Calls run [`START_LIMIT`] at once,
//! and a thread's leaves run one after another, so the longest thread sets a
//! floor. *Usable* is when the ready flag sets: the workspace tree and the
//! threads active within [`READY_DAYS`]. *Complete* is every tree. A model's
//! seconds per call come from its timed history where there is enough of it.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

use super::compactor_models::{MeasuredUsage, COMPACTOR_MODELS};
use super::limit::START_LIMIT;
use super::log::{ENTRY_EVENT_TYPES, TURN_END_EVENT_TYPES};
use super::store::{ready_window_sql, THREAD_IN_SCOPE_SQL};
use super::workspace_log::{sql_list, WORKSPACE_LEAF_EVENT_TYPES};
use super::{fold, prompt, CAP_CHARS, CONTEXT_BYTES, NODE_BYTES, READY_DAYS};
use crate::engine::LucidosEngine;
use crate::llm::model_registry::{
    provider_kind_for, resolve_route, ModelRegistry, ProviderKind, RouteEntry,
};
use crate::llm::reasoning::clamp_effort;
use crate::llm::EFFORT_LADDER;

/// A stored payload over this many bytes is surely an entry over
/// [`NODE_BYTES`]: its fields and metadata never take more than the slack.
const SURE_LONG_BYTES: i64 = NODE_BYTES as i64 + 400;

/// A large payload is stored compressed. Text this repetitive shrinks by at
/// most about this factor, so it bounds an entry's real size from above.
const COMPRESSION: i64 = 3;

const BYTES_PER_TOKEN: u64 = 4;

/// A single-line answer's own tokens, independent of reasoning tier: what
/// [`scaled_output_tokens`] falls back to at `none`, and the floor every
/// other tier is built on top of.
const LINE_TOKENS: u64 = 128;

/// Seconds one call takes, wall clock, for a model with no timed history.
const SECS_PER_CALL: Bounds<f64> = Bounds {
    low: 1.0,
    high: 4.0,
};

/// Days of activity the daily figure averages over.
const RECENT_DAYS: u64 = 7;

/// A model's measured history counting fewer calls than this is too thin to
/// trust: one unusually long or short call could swing the average either
/// way. Below it, the model's seed applies instead.
const MIN_MEASURED_CALLS: i64 = 20;

/// Entry types only a Lucidos Agent thread logs.
const AGENT_ONLY: &[&str] = &["ToolCalled", "ToolResult"];

/// Entry types only a coding-agent thread logs.
const CODING_ONLY: &[&str] = &["UserQuestionAsked", "UserQuestionAnswered"];

/// Deltas joined into another entry, so never one of their own.
const DELTAS: &[&str] = &["TextStreamed", "CodingAgentTextStreamed"];

const IDLED: &str = "CodingAgentIdled";

/// A low and a high figure.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Bounds<T> {
    pub low: T,
    pub high: T,
}

/// What `GET /api/v1/memory/tree-backfill/estimate` serves.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TreeBackfillEstimate {
    /// Compactor calls the backfill makes.
    pub calls: Bounds<u64>,
    /// Until turns read the trees, at the default seconds per call.
    pub usable_secs: Bounds<u64>,
    /// Until every tree is built, at the default seconds per call.
    pub complete_secs: Bounds<u64>,
    /// Calls a day once the backfill is done, from the last week's activity.
    pub daily_calls: Bounds<u64>,
    /// The cost of both, for each compactor model, at every reasoning tier
    /// the compactor preference accepts. A tier the model cannot run is
    /// priced as the tier routing snaps it to.
    pub costs: Vec<ModelCost>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ModelCost {
    pub model: String,
    /// [`TreeBackfillEstimate::usable_secs`] at this model's seconds per call.
    pub usable_secs: Bounds<u64>,
    /// [`TreeBackfillEstimate::complete_secs`] at this model's seconds per call.
    pub complete_secs: Bounds<u64>,
    pub by_effort: Vec<EffortCost>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EffortCost {
    pub effort: String,
    pub backfill_usd: Bounds<f64>,
    /// The single figure the panel leads with. From measured (or
    /// seeded) per-call tokens, not the low/high guesses that make the
    /// range: see [`central_usd`].
    pub backfill_usd_central: f64,
    pub daily_usd: Bounds<f64>,
    pub daily_usd_central: f64,
}

/// One tree's leaves, as the estimate weighs them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Leaves {
    pub(crate) count: u64,
    /// Leaves that surely, and that possibly, cost a call.
    pub(crate) calls: Bounds<u64>,
    /// Input bytes of those calls' messages, low and high.
    pub(crate) bytes: Bounds<u64>,
}

/// One tree's leaves: all of them, and those of the last [`RECENT_DAYS`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Tree {
    pub(crate) all: Leaves,
    pub(crate) recent: Leaves,
    /// The thread was active within [`READY_DAYS`], so the ready flag waits
    /// on its tree.
    pub(crate) ready_window: bool,
}

impl Tree {
    /// The calls of building the whole tree.
    fn backfill(&self) -> Work {
        tree_work(self.all, merge_count(self.all.count), self.all.count)
    }

    /// Its leaf calls, which run one after another.
    fn chain(&self) -> Bounds<u64> {
        self.all.calls
    }

    /// The calls its recent leaves cost: those leaves, and the merges they
    /// added to the tree.
    fn recent_work(&self) -> Work {
        let before = self.all.count - self.recent.count;
        tree_work(
            self.recent,
            merge_count(self.all.count) - merge_count(before),
            self.all.count,
        )
    }
}

/// Everything the estimate is computed from.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Counts {
    pub(crate) threads: Vec<Tree>,
    pub(crate) workspace: Tree,
    /// Model-written nodes already stored, which the backfill skips.
    pub(crate) built: u64,
}

/// Calls and input bytes of some amount of compactor work.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Work {
    calls: Bounds<u64>,
    input_bytes: Bounds<u64>,
}

impl Work {
    fn add(&mut self, other: Work) {
        self.calls.low += other.calls.low;
        self.calls.high += other.calls.high;
        self.input_bytes.low += other.input_bytes.low;
        self.input_bytes.high += other.input_bytes.high;
    }

    fn input_tokens(&self) -> Bounds<u64> {
        Bounds {
            low: self.input_bytes.low / BYTES_PER_TOKEN,
            high: self.input_bytes.high / BYTES_PER_TOKEN,
        }
    }

    /// The calls' midpoint: the one call count the central figure uses,
    /// where the low/high bounds instead carry the range.
    fn central_calls(&self) -> u64 {
        (self.calls.low + self.calls.high) / 2
    }

    /// This work less `built` calls already made, its bytes in proportion.
    fn less(self, built: u64) -> Work {
        let keep = |calls: u64, bytes: u64| -> (u64, u64) {
            let left = calls.saturating_sub(built);
            let bytes = if calls == 0 {
                0
            } else {
                (bytes as u128 * left as u128 / calls as u128) as u64
            };
            (left, bytes)
        };
        let (low, low_bytes) = keep(self.calls.low, self.input_bytes.low);
        let (high, high_bytes) = keep(self.calls.high, self.input_bytes.high);
        Work {
            calls: Bounds { low, high },
            input_bytes: Bounds {
                low: low_bytes,
                high: high_bytes,
            },
        }
    }

    fn per_day(self) -> Work {
        let day = |n: u64| n / RECENT_DAYS;
        Work {
            calls: Bounds {
                low: day(self.calls.low),
                high: day(self.calls.high),
            },
            input_bytes: Bounds {
                low: day(self.input_bytes.low),
                high: day(self.input_bytes.high),
            },
        }
    }
}

/// Bytes every call sends besides its message: the system prompt and the
/// request's frame.
fn frame_bytes() -> u64 {
    (prompt::COMPACT.len() + prompt::request(&[], &prompt::compress_step("")).len()) as u64
}

/// The real context-tiling bytes a call at tree position `tree_len` sends,
/// assuming every stored node is [`NODE_BYTES`] big (the compactor's cap on a
/// written line). Calls [`fold::context_tiling`], the exact function the
/// compactor itself calls to build a node's context, so this is no longer a
/// flat `0..CONTEXT_BYTES` guess: a small tree gets a small figure, and only a
/// tree past the budget converges on [`CONTEXT_BYTES`].
fn context_tiling_bytes(tree_len: u64) -> u64 {
    fold::context_tiling(tree_len, |_| Some(NODE_BYTES), CONTEXT_BYTES).len() as u64
        * NODE_BYTES as u64
}

/// The merges a full tree over `n` leaves holds (`shape::complete_addresses`
/// less its leaves).
pub(crate) fn merge_count(n: u64) -> u64 {
    let (mut span, mut merges) = (2, 0);
    while span <= n {
        merges += n / span;
        span *= 2;
    }
    merges
}

/// A tree's leaf calls, and `merges` merge calls at most. At best each pair
/// of leaves that costs no call merges for free too. `tree_len` is the whole
/// tree's leaf count, which sets how deep a call's context tiling reaches.
fn tree_work(leaves: Leaves, merges: u64, tree_len: u64) -> Work {
    let frame = frame_bytes();
    let context_bytes = context_tiling_bytes(tree_len);
    let free_pairs = leaves.count.saturating_sub(leaves.calls.high) / 2;
    let merges = Bounds {
        low: merges.saturating_sub(free_pairs),
        high: merges,
    };
    let merge_bytes = Bounds {
        low: frame + NODE_BYTES as u64,
        high: frame + 2 * NODE_BYTES as u64 + context_bytes,
    };
    Work {
        calls: Bounds {
            low: leaves.calls.low + merges.low,
            high: leaves.calls.high + merges.high,
        },
        input_bytes: Bounds {
            low: leaves.calls.low * frame + leaves.bytes.low + merges.low * merge_bytes.low,
            high: leaves.calls.high * (frame + context_bytes)
                + leaves.bytes.high
                + merges.high * merge_bytes.high,
        },
    }
}

/// A relative thinking-budget weight for each reasoning tier, used only to
/// scale a measured tier's average onto another tier.
///
/// `none` is true zero: no thinking is requested at that tier, so its weight
/// must be lower than every other, not
/// [`crate::llm::thinking_budget_for_effort`]'s catch-all fallback.
///
/// Every other tier keeps that function's numeric ceiling: the one real,
/// code-defined scale between tiers this crate has. Only the budget-token
/// Claude path exposes a number today. Adaptive Claude and Gemini's
/// qualitative tiers do not, so for them this ratio is the estimate's one
/// remaining guess, not its direction.
fn tier_weight(effort: &str) -> u32 {
    match effort {
        "none" => 0,
        other => crate::llm::thinking_budget_for_effort(other),
    }
}

/// Output tokens of one call at `effort`, from a `low`-tier baseline (today's
/// `reasoning_summary_compaction` default, which is what historical,
/// tier-unlabeled usage implicitly measures). The line itself
/// ([`LINE_TOKENS`]) does not scale; only the thinking portion above it does,
/// by the ratio of `effort`'s weight to `low`'s. Monotonic in
/// [`EFFORT_LADDER`] order, since [`tier_weight`] is.
fn scaled_output_tokens(baseline_low_output: u64, effort: &str) -> u64 {
    let thinking = baseline_low_output.saturating_sub(LINE_TOKENS);
    let low_weight = f64::from(tier_weight("low"));
    let weight = f64::from(tier_weight(effort));
    LINE_TOKENS + ((thinking as f64) * weight / low_weight).round() as u64
}

/// Real per-call compactor usage and time for one model, from
/// `ContextCaptured` history: count and averages of the compactor's calls, per
/// model in `$1`. Only timed calls count toward the time.
///
/// The first two conditions are `idx_events_summary_compaction_captures`'s
/// predicate, spelled exactly as its migration does. Change one and the read
/// falls back to scanning every event.
///
/// A row with no `reasoning_effort` predates the field and ran at the
/// preference's then-default, `low`. A labelled higher tier must not dilute
/// that baseline. `avg(bigint)` is `numeric`, which sqlx cannot decode into
/// `f64`, so each average casts to FLOAT8.
const MEASURED_USAGE_SQL: &str = "SELECT payload->>'model', count(*), \
       avg((payload->'usage'->>'input_tokens')::bigint)::float8, \
       avg((payload->'usage'->>'output_tokens')::bigint)::float8, \
       count(payload->'duration_ms'), \
       avg((payload->>'duration_ms')::float8) \
     FROM events \
     WHERE event_type = 'ContextCaptured' \
       AND payload->>'purpose' = 'summary_compaction' \
       AND payload->>'model' = ANY($1) \
       AND payload ? 'usage' \
       AND COALESCE(payload->>'reasoning_effort', 'low') = 'low' \
     GROUP BY 1";

/// The models [`MEASURED_USAGE_SQL`] reads: the compactor's, which the
/// estimate prices.
fn compactor_model_ids() -> Vec<&'static str> {
    COMPACTOR_MODELS.iter().map(|m| m.id).collect()
}

/// What the compactor's history says about each model.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Measured {
    pub(crate) usage: HashMap<String, MeasuredUsage>,
    /// Seconds one call took on average.
    pub(crate) secs_per_call: HashMap<String, f64>,
}

impl Measured {
    /// This model's seconds per call: measured, or the default range.
    fn secs_per_call(&self, model: &str) -> Bounds<f64> {
        self.secs_per_call
            .get(model)
            .map_or(SECS_PER_CALL, |&secs| Bounds {
                low: secs,
                high: secs,
            })
    }
}

/// Measured usage and time for every compactor model, in one query. Each
/// needs at least [`MIN_MEASURED_CALLS`] calls: fewer are not enough to trust
/// an average from, and the model's seed or the default time applies instead.
pub(crate) async fn measured_for_catalog(pool: &PgPool) -> Result<Measured, sqlx::Error> {
    type Row = (String, i64, Option<f64>, Option<f64>, i64, Option<f64>);
    let rows: Vec<Row> = sqlx::query_as(MEASURED_USAGE_SQL)
        .bind(compactor_model_ids())
        .fetch_all(pool)
        .await?;
    let mut measured = Measured::default();
    for (model, calls, avg_input, avg_output, timed, avg_ms) in rows {
        if calls >= MIN_MEASURED_CALLS {
            let usage = MeasuredUsage {
                avg_input: avg_input.unwrap_or_default().round() as u64,
                avg_output_low: avg_output.unwrap_or_default().round() as u64,
            };
            measured.usage.insert(model.clone(), usage);
        }
        if let (true, Some(ms)) = (timed >= MIN_MEASURED_CALLS, avg_ms) {
            measured.secs_per_call.insert(model, ms / 1000.0);
        }
    }
    Ok(measured)
}

/// Seconds to run `calls` [`START_LIMIT`] at a time, never under the longest
/// `chain` of calls that run one after another. Built nodes shrink `calls`, so
/// they cap the chain too: no chain outlasts the calls still to make.
fn run_secs(calls: Bounds<u64>, chain: Bounds<u64>, secs: Bounds<f64>) -> Bounds<u64> {
    let rounds = |calls: u64, chain: u64| calls.div_ceil(START_LIMIT as u64).max(chain.min(calls));
    let secs_for = |rounds: u64, secs: f64| (rounds as f64 * secs).ceil() as u64;
    Bounds {
        low: secs_for(rounds(calls.low, chain.low), secs.low),
        high: secs_for(rounds(calls.high, chain.high), secs.high),
    }
}

/// The longest chain among `trees`, low and high.
fn longest_chain<'a>(trees: impl Iterator<Item = &'a Tree>) -> Bounds<u64> {
    trees.fold(Bounds::default(), |longest, tree| Bounds {
        low: longest.low.max(tree.chain().low),
        high: longest.high.max(tree.chain().high),
    })
}

/// The estimate, from the counts and whatever measured usage is on file.
/// Pure, so the arithmetic is tested alone. `registry` and `configured` pick
/// the route each model's calls take, which decides the tiers it can run.
pub(crate) fn estimate(
    counts: &Counts,
    measured: &Measured,
    registry: &ModelRegistry,
    configured: &[ProviderKind],
) -> TreeBackfillEstimate {
    let mut gross = Work::default();
    let mut recent = Work::default();
    let mut ready = counts.workspace.backfill();
    for tree in counts.threads.iter().chain([&counts.workspace]) {
        gross.add(tree.backfill());
        recent.add(tree.recent_work());
    }
    let recent_threads = || counts.threads.iter().filter(|t| t.ready_window);
    for tree in recent_threads() {
        ready.add(tree.backfill());
    }
    let work = gross.less(counts.built);
    let ready = ready.less(counts.built);
    let daily = recent.per_day();

    // A workspace leaf reads no other workspace line, so only threads chain.
    let complete_chain = longest_chain(counts.threads.iter());
    let ready_chain = longest_chain(recent_threads());
    let times = |secs: Bounds<f64>| {
        (
            run_secs(ready.calls, ready_chain, secs),
            run_secs(work.calls, complete_chain, secs),
        )
    };
    let (usable_secs, complete_secs) = times(SECS_PER_CALL);

    TreeBackfillEstimate {
        calls: work.calls,
        usable_secs,
        complete_secs,
        daily_calls: daily.calls,
        costs: COMPACTOR_MODELS
            .iter()
            .map(|model| {
                let (input_price, output_price) = model.price;
                let usage = measured.usage.get(model.id).copied().unwrap_or(model.seed);
                let (usable_secs, complete_secs) = times(measured.secs_per_call(model.id));
                let route = route_for(registry, model.id, configured);
                let by_effort = EFFORT_LADDER
                    .iter()
                    .map(|&effort| {
                        let runs_at =
                            clamp_effort(effort, route.provider, &route.wire_id).unwrap_or(effort);
                        let output_per_call = scaled_output_tokens(usage.avg_output_low, runs_at);
                        let backfill_usd =
                            ranged_usd(input_price, output_price, output_per_call, &work);
                        let daily_usd =
                            ranged_usd(input_price, output_price, output_per_call, &daily);
                        EffortCost {
                            effort: effort.to_string(),
                            backfill_usd_central: central_usd(
                                input_price,
                                output_price,
                                &usage,
                                output_per_call,
                                &work,
                            )
                            .clamp(backfill_usd.low, backfill_usd.high),
                            daily_usd_central: central_usd(
                                input_price,
                                output_price,
                                &usage,
                                output_per_call,
                                &daily,
                            )
                            .clamp(daily_usd.low, daily_usd.high),
                            backfill_usd,
                            daily_usd,
                        }
                    })
                    .collect();
                ModelCost {
                    model: model.id.to_string(),
                    usable_secs,
                    complete_secs,
                    by_effort,
                }
            })
            .collect(),
    }
}

/// The route a model's calls take: the one the router resolves against the
/// configured providers. With none configured, the model's own first route,
/// so an estimate read before setup still prices something.
fn route_for(registry: &ModelRegistry, model: &str, configured: &[ProviderKind]) -> RouteEntry {
    resolve_route(registry, model, None, |kind| configured.contains(&kind))
        .or_else(|_| resolve_route(registry, model, None, |_| true))
        .unwrap_or_else(|_| RouteEntry::new(provider_kind_for(registry, model), model))
}

/// The low/high range: measured (or seeded) output tokens a call, over the
/// work's own call-count range. Input stays the deterministic byte-based
/// estimate; only the call count and the per-call token figures move the
/// range now, not a second independent output-token guess.
fn ranged_usd(
    input_price: f64,
    output_price: f64,
    output_per_call: u64,
    work: &Work,
) -> Bounds<f64> {
    let usd = |calls: u64, input_tokens: u64| {
        (input_tokens as f64 * input_price + (calls * output_per_call) as f64 * output_price)
            / 1_000_000.0
    };
    let input = work.input_tokens();
    Bounds {
        low: usd(work.calls.low, input.low),
        high: usd(work.calls.high, input.high),
    }
}

/// The one central figure: the work's midpoint call count, times the
/// measured (or seeded) per-call input and output tokens. Neither side
/// comes from a low/high guess, so the caller clamps the result into
/// `ranged_usd`'s own range: that range is the byte-size heuristic, a
/// different measurement from this one, and the two are not guaranteed to
/// agree once real usage history exists.
fn central_usd(
    input_price: f64,
    output_price: f64,
    usage: &MeasuredUsage,
    output_per_call: u64,
    work: &Work,
) -> f64 {
    let calls = work.central_calls();
    (calls * usage.avg_input) as f64 * input_price / 1_000_000.0
        + (calls * output_per_call) as f64 * output_price / 1_000_000.0
}

/// One thread's events in one window: the recent days, or before them.
#[derive(sqlx::FromRow)]
struct ThreadRow {
    thread_id: Uuid,
    is_coding_agent: bool,
    /// Active within [`READY_DAYS`], as the compactor's seeding reads it.
    ready_window: bool,
    recent: bool,
    /// Entries other than replies.
    entries: i64,
    replies: i64,
    idles: i64,
    sure_long: i64,
    maybe_long: i64,
    long_replies: i64,
    sure_bytes: i64,
    maybe_bytes: i64,
}

impl ThreadRow {
    /// This window's thread leaves, and the turns it adds to the workspace.
    /// A coding agent's idle with no reply before it closes a turn of its own.
    fn leaves(&self) -> (Leaves, Leaves) {
        let idle_turns = if self.is_coding_agent {
            (self.idles - self.replies).max(0) as u64
        } else {
            0
        };
        let turns = self.replies as u64 + idle_turns;
        let thread = Leaves {
            count: self.entries as u64 + turns,
            calls: Bounds {
                low: self.sure_long as u64,
                high: self.maybe_long as u64 + idle_turns,
            },
            bytes: Bounds {
                low: self.sure_bytes as u64,
                high: self.maybe_bytes as u64,
            },
        };
        let workspace = Leaves {
            count: turns,
            calls: Bounds {
                low: self.long_replies as u64,
                high: turns,
            },
            bytes: Bounds {
                low: self.long_replies as u64 * NODE_BYTES as u64,
                high: turns * (4 * NODE_BYTES + CONTEXT_BYTES) as u64,
            },
        };
        (thread, workspace)
    }
}

fn add_leaves(into: &mut Leaves, more: Leaves) {
    into.count += more.count;
    into.calls.low += more.calls.low;
    into.calls.high += more.calls.high;
    into.bytes.low += more.bytes.low;
    into.bytes.high += more.bytes.high;
}

/// Read the counts. The three reads are independent, so they run at once.
pub(crate) async fn counts(pool: &PgPool) -> Result<Counts, sqlx::Error> {
    let (rows, (artifacts, recent_artifacts), built) =
        tokio::try_join!(thread_rows(pool), artifact_writes(pool), built_nodes(pool))?;

    let mut threads: HashMap<Uuid, Tree> = HashMap::new();
    let mut counts = Counts {
        built,
        ..Counts::default()
    };
    for row in &rows {
        let (thread, workspace) = row.leaves();
        let tree = threads.entry(row.thread_id).or_default();
        tree.ready_window = row.ready_window;
        add_leaves(&mut tree.all, thread);
        add_leaves(&mut counts.workspace.all, workspace);
        if row.recent {
            add_leaves(&mut tree.recent, thread);
            add_leaves(&mut counts.workspace.recent, workspace);
        }
    }
    counts.threads = threads.into_values().collect();
    add_leaves(&mut counts.workspace.all, artifact_leaves(artifacts));
    add_leaves(
        &mut counts.workspace.recent,
        artifact_leaves(recent_artifacts),
    );
    Ok(counts)
}

/// One scan of the thread events, grouped by thread and by whether the
/// event is recent.
async fn thread_rows(pool: &PgPool) -> Result<Vec<ThreadRow>, sqlx::Error> {
    let replies: Vec<&str> = TURN_END_EVENT_TYPES
        .iter()
        .copied()
        .filter(|t| *t != IDLED)
        .collect();
    let entry_types: Vec<&str> = ENTRY_EVENT_TYPES
        .iter()
        .copied()
        .filter(|t| !DELTAS.contains(t))
        .collect();
    let mut read: Vec<&str> = entry_types.clone();
    read.extend(TURN_END_EVENT_TYPES);
    let size = "pg_column_size(e.payload)";
    let entry = format!(
        "(e.event_type IN ({}) \
          AND (s.is_coding_agent OR e.event_type NOT IN ({})) \
          AND (NOT s.is_coding_agent OR e.event_type NOT IN ({})))",
        sql_list(&entry_types),
        sql_list(CODING_ONLY),
        sql_list(AGENT_ONLY),
    );
    let reply = format!("e.event_type IN ({})", sql_list(&replies));
    let long_candidate = format!("({entry} OR {reply})");
    sqlx::query_as(&format!(
        "SELECT s.thread_id, s.is_coding_agent, \
           {} AS ready_window, \
           e.created > now() - make_interval(days => $1) AS recent, \
           count(*) FILTER (WHERE {entry}) AS entries, \
           count(*) FILTER (WHERE {reply}) AS replies, \
           count(*) FILTER (WHERE e.event_type = '{IDLED}') AS idles, \
           count(*) FILTER (WHERE {long_candidate} AND {size} > $2) AS sure_long, \
           count(*) FILTER (WHERE {long_candidate} AND {size} > $3) AS maybe_long, \
           count(*) FILTER (WHERE {reply} AND {size} > $2) AS long_replies, \
           COALESCE(sum(LEAST({size}, $4)) FILTER ( \
             WHERE {long_candidate} AND {size} > $2), 0)::bigint AS sure_bytes, \
           COALESCE(sum(LEAST({size} * $5, $4)) FILTER ( \
             WHERE {long_candidate} AND {size} > $3), 0)::bigint AS maybe_bytes \
         FROM thread_summaries s JOIN events e ON e.thread_id = s.thread_id \
         WHERE {THREAD_IN_SCOPE_SQL} AND e.event_type IN ({}) \
         GROUP BY 1, 2, 3, 4",
        ready_window_sql(6),
        sql_list(&read),
    ))
    .bind(RECENT_DAYS as i32)
    .bind(SURE_LONG_BYTES as i32)
    .bind(NODE_BYTES as i32)
    .bind(CAP_CHARS as i32)
    .bind(COMPRESSION as i32)
    .bind(READY_DAYS)
    .fetch_all(pool)
    .await
}

/// Artifact writes that get a workspace leaf, all and recent. The workspace
/// log's own filter applies: binaries and bookkeeping get no leaf.
async fn artifact_writes(pool: &PgPool) -> Result<(u64, u64), sqlx::Error> {
    let artifact_types: Vec<&str> = WORKSPACE_LEAF_EVENT_TYPES
        .iter()
        .copied()
        .filter(|t| !TURN_END_EVENT_TYPES.contains(t))
        .collect();
    let writes: Vec<(Option<String>, bool)> = sqlx::query_as(&format!(
        "SELECT COALESCE(payload->'data', payload)->>'artifact_path', \
           created > now() - make_interval(days => $1) \
         FROM events WHERE event_type IN ({})",
        sql_list(&artifact_types)
    ))
    .bind(RECENT_DAYS as i32)
    .fetch_all(pool)
    .await?;
    let (mut artifacts, mut recent_artifacts) = (0, 0);
    for (path, recent) in writes {
        let Some(path) = path else { continue };
        let path = LucidosEngine::canonicalize_artifact_path(&path);
        if LucidosEngine::should_skip_artifact_for_memory(path) {
            continue;
        }
        artifacts += 1;
        recent_artifacts += u64::from(recent);
    }
    Ok((artifacts, recent_artifacts))
}

/// Model-written nodes already stored.
async fn built_nodes(pool: &PgPool) -> Result<u64, sqlx::Error> {
    let built: i64 =
        sqlx::query_scalar("SELECT count(*) FROM summary_tree_nodes WHERE model IS NOT NULL")
            .fetch_one(pool)
            .await?;
    Ok(built as u64)
}

/// An artifact leaf carries the file's content, capped, so it always costs a
/// call.
fn artifact_leaves(n: u64) -> Leaves {
    Leaves {
        count: n,
        calls: Bounds { low: n, high: n },
        bytes: Bounds {
            low: n * NODE_BYTES as u64,
            high: n * (CAP_CHARS + CONTEXT_BYTES) as u64,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_count_matches_the_tree_shape() {
        for n in 0..300 {
            let shape = super::super::shape::complete_addresses(n);
            assert_eq!(merge_count(n), shape.len() as u64 - n, "n = {n}");
        }
    }

    fn leaves(count: u64, sure: u64, maybe: u64) -> Leaves {
        Leaves {
            count,
            calls: Bounds {
                low: sure,
                high: maybe,
            },
            bytes: Bounds {
                low: sure * 1_000,
                high: maybe * 3_000,
            },
        }
    }

    fn tree(all: Leaves, recent: Leaves) -> Tree {
        Tree {
            all,
            recent,
            ready_window: false,
        }
    }

    fn old(all: Leaves) -> Tree {
        tree(all, Leaves::default())
    }

    /// No registry row: every model routes by its id's shape, as on a
    /// workspace whose rows are missing.
    fn estimate_measured(
        counts: &Counts,
        usage: &HashMap<String, MeasuredUsage>,
    ) -> TreeBackfillEstimate {
        estimate(
            counts,
            &Measured {
                usage: usage.clone(),
                ..Measured::default()
            },
            &crate::llm::model_registry::empty(),
            &ProviderKind::ALL,
        )
    }

    fn estimate_with(counts: &Counts) -> TreeBackfillEstimate {
        estimate_measured(counts, &HashMap::new())
    }

    fn seed_of(model: &str) -> MeasuredUsage {
        COMPACTOR_MODELS
            .iter()
            .find(|m| m.id == model)
            .unwrap_or_else(|| panic!("{model} is not a compactor model"))
            .seed
    }

    fn cost_at<'a>(e: &'a TreeBackfillEstimate, model: &str, effort: &str) -> &'a EffortCost {
        e.costs
            .iter()
            .find(|c| c.model == model)
            .unwrap_or_else(|| panic!("{model} not priced"))
            .by_effort
            .iter()
            .find(|c| c.effort == effort)
            .unwrap_or_else(|| panic!("{model} has no {effort} row"))
    }

    #[test]
    fn every_low_bound_stays_under_its_high_bound() {
        let counts = Counts {
            threads: vec![
                tree(leaves(10, 2, 4), leaves(4, 1, 2)),
                old(leaves(1, 0, 0)),
                tree(leaves(64, 30, 40), leaves(10, 2, 5)),
            ],
            workspace: tree(leaves(20, 5, 20), leaves(7, 1, 7)),
            built: 0,
        };
        let e = estimate_with(&counts);
        for b in [e.calls, e.complete_secs, e.daily_calls] {
            assert!(b.low <= b.high, "{b:?}");
        }
        for c in &e.costs {
            for ec in &c.by_effort {
                assert!(ec.backfill_usd.low <= ec.backfill_usd.high, "{ec:?}");
                assert!(ec.daily_usd.low <= ec.daily_usd.high, "{ec:?}");
                assert!(
                    ec.backfill_usd_central >= ec.backfill_usd.low - 1e-9,
                    "{ec:?}"
                );
                assert!(
                    ec.backfill_usd_central <= ec.backfill_usd.high + 1e-9,
                    "{ec:?}"
                );
            }
        }
        assert!(e.calls.low > 0);
    }

    /// Two long leaves cost two leaf calls and their one merge. Four short
    /// ones cost one merge at best and three at worst.
    #[test]
    fn short_leaves_merge_free_only_in_the_low_bound() {
        let long = tree_work(leaves(2, 2, 2), merge_count(2), 2);
        assert_eq!(long.calls, Bounds { low: 3, high: 3 });
        let short = tree_work(leaves(4, 0, 0), merge_count(4), 4);
        assert_eq!(short.calls, Bounds { low: 1, high: 3 });
    }

    #[test]
    fn built_nodes_shrink_the_estimate() {
        let counts = Counts {
            threads: vec![old(leaves(16, 8, 16))],
            ..Counts::default()
        };
        let fresh = estimate_with(&counts);
        let half = estimate_with(&Counts {
            built: fresh.calls.low / 2,
            ..counts.clone()
        });
        assert!(half.calls.low < fresh.calls.low);
        assert!(half.calls.high < fresh.calls.high);
        let done = estimate_with(&Counts {
            built: fresh.calls.high,
            ..counts
        });
        assert_eq!(done.calls, Bounds { low: 0, high: 0 });
        assert_eq!(done.usable_secs, Bounds { low: 0, high: 0 });
        assert_eq!(done.complete_secs, Bounds { low: 0, high: 0 });
    }

    /// Seven new one-leaf threads add no merge between them; a week of new
    /// leaves on one long thread adds the merges they close.
    #[test]
    fn daily_merges_stay_inside_each_tree() {
        let single = leaves(1, 1, 1);
        let seven = Counts {
            threads: vec![tree(single, single); 7],
            ..Counts::default()
        };
        assert_eq!(
            estimate_with(&seven).daily_calls,
            Bounds { low: 1, high: 1 }
        );

        let long = Counts {
            threads: vec![tree(leaves(16, 16, 16), leaves(8, 8, 8))],
            ..Counts::default()
        };
        let added = merge_count(16) - merge_count(8);
        let per_day = (8 + added) / RECENT_DAYS;
        assert_eq!(
            estimate_with(&long).daily_calls,
            Bounds {
                low: per_day,
                high: per_day
            }
        );
    }

    /// A partial batch of calls still takes a round.
    #[test]
    fn a_few_calls_still_take_time() {
        let few = Counts {
            threads: vec![old(leaves(2, 2, 2))],
            ..Counts::default()
        };
        assert!(estimate_with(&few).complete_secs.low >= SECS_PER_CALL.low as u64);
    }

    #[test]
    fn a_cheaper_model_costs_less() {
        let e = estimate_with(&Counts {
            threads: vec![old(leaves(100, 50, 80))],
            ..Counts::default()
        });
        let usd = |model: &str| cost_at(&e, model, "low").backfill_usd.high;
        assert!(usd("gemini-3.8-flash") < usd("gpt-6.1-sol"));
        assert!(usd("gemini-3.8-flash") < usd("claude-sonnet-5-5"));
    }

    /// GPT-6.1 Sol and Sonnet 5.5 share a list price but not a spend: Sonnet
    /// 5.5 sends more input and writes more output per call. One shared token
    /// guess priced them identically, so this pins that the seeds tell them
    /// apart, in the direction the measurements point.
    #[test]
    fn two_models_at_one_price_are_told_apart_by_their_seeds() {
        let sol = COMPACTOR_MODELS.iter().find(|m| m.id == "gpt-6.1-sol");
        let sonnet = COMPACTOR_MODELS
            .iter()
            .find(|m| m.id == "claude-sonnet-5-5");
        let price = |m: Option<&super::super::compactor_models::CompactorModel>| m.map(|m| m.price);
        assert_eq!(price(sol), price(sonnet), "the premise: one list price");
        assert_ne!(seed_of("gpt-6.1-sol"), seed_of("claude-sonnet-5-5"));

        let e = estimate_with(&Counts {
            threads: vec![tree(leaves(100, 50, 80), leaves(30, 15, 24))],
            ..Counts::default()
        });
        for effort in EFFORT_LADDER {
            let sol = cost_at(&e, "gpt-6.1-sol", effort);
            let sonnet = cost_at(&e, "claude-sonnet-5-5", effort);
            assert!(
                sol.backfill_usd_central < sonnet.backfill_usd_central,
                "at {effort}: {sol:?} vs {sonnet:?}"
            );
            assert!(
                sol.daily_usd_central < sonnet.daily_usd_central,
                "at {effort}"
            );
        }
    }

    /// Routing snaps `none` up to `low` on a model that always reasons, so
    /// the estimate must not price a `none` it will never send.
    #[test]
    fn a_tier_the_model_cannot_run_is_priced_as_the_tier_it_runs_at() {
        let e = estimate_with(&Counts {
            threads: vec![old(leaves(100, 50, 80))],
            ..Counts::default()
        });
        for cost in &e.costs {
            let none = cost_at(&e, &cost.model, "none");
            let low = cost_at(&e, &cost.model, "low");
            assert_eq!(none.backfill_usd, low.backfill_usd, "{}", cost.model);
            assert_eq!(
                none.backfill_usd_central, low.backfill_usd_central,
                "{}",
                cost.model
            );
        }
    }

    /// Prices follow the route the call takes. Through OpenRouter, GPT-6.1 Sol
    /// tops out at `high`. So `xhigh` and `max` price as `high` there, though
    /// its first route (OpenAI) offers them.
    #[test]
    fn tiers_are_priced_on_the_configured_route() {
        use crate::llm::model_registry::ModelRouting;
        let registry: ModelRegistry = std::sync::Arc::new(std::sync::RwLock::new(
            [(
                "gpt-6.1-sol".to_string(),
                ModelRouting {
                    routes: vec![
                        RouteEntry::new(ProviderKind::OpenAi, "gpt-6.1-sol"),
                        RouteEntry::new(ProviderKind::OpenRouter, "openai/gpt-6.1-sol"),
                    ],
                    preferred: None,
                    vision: false,
                },
            )]
            .into_iter()
            .collect(),
        ));
        let counts = Counts {
            threads: vec![old(leaves(100, 50, 80))],
            ..Counts::default()
        };
        // Enough thinking output that the tiers above `low` price apart.
        let measured = HashMap::from([(
            "gpt-6.1-sol".to_string(),
            MeasuredUsage {
                avg_input: 3_000,
                avg_output_low: 1_000,
            },
        )]);
        let measured = Measured {
            usage: measured,
            ..Measured::default()
        };
        let cost = |configured: &[ProviderKind], effort: &str| {
            let e = estimate(&counts, &measured, &registry, configured);
            cost_at(&e, "gpt-6.1-sol", effort).backfill_usd.high
        };
        let openrouter = [ProviderKind::OpenRouter];
        assert_eq!(cost(&openrouter, "xhigh"), cost(&openrouter, "high"));
        assert_eq!(cost(&openrouter, "max"), cost(&openrouter, "high"));
        let openai = [ProviderKind::OpenAi];
        assert!(cost(&openai, "max") > cost(&openai, "high"));
    }

    /// The price table is one array ([`COMPACTOR_MODELS`]), so this can only
    /// fail if `estimate()` stops deriving `costs` from it.
    #[test]
    fn costs_cover_exactly_the_compactor_models() {
        let e = estimate_with(&Counts::default());
        let compactor_ids: Vec<&str> = COMPACTOR_MODELS.iter().map(|m| m.id).collect();
        let cost_ids: Vec<&str> = e.costs.iter().map(|c| c.model.as_str()).collect();
        assert_eq!(cost_ids, compactor_ids);
    }

    /// A higher reasoning tier never costs less, for the same model and the
    /// same work: thinking tokens bill as output, so more thinking budget
    /// never prices under less.
    #[test]
    fn a_higher_tier_never_costs_less() {
        let e = estimate_with(&Counts {
            threads: vec![old(leaves(200, 100, 160))],
            ..Counts::default()
        });
        for cost in &e.costs {
            let mut last = 0.0;
            for effort in EFFORT_LADDER {
                let row = cost.by_effort.iter().find(|c| c.effort == *effort).unwrap();
                assert!(
                    row.backfill_usd.high >= last,
                    "{}: {effort} costs less than the tier before it",
                    cost.model
                );
                last = row.backfill_usd.high;
            }
        }
    }

    /// Measured history replaces the seed for a model with enough calls. A
    /// bigger measured average costs more than the same model's seeded run.
    #[test]
    fn measured_usage_overrides_the_seed() {
        let counts = Counts {
            threads: vec![old(leaves(100, 50, 80))],
            ..Counts::default()
        };
        let seed = seed_of("gemini-3.8-flash");
        let mut measured = HashMap::new();
        measured.insert(
            "gemini-3.8-flash".to_string(),
            MeasuredUsage {
                avg_input: seed.avg_input * 10,
                avg_output_low: seed.avg_output_low * 10,
            },
        );
        let e = estimate_measured(&counts, &measured);
        let measured_cost = cost_at(&e, "gemini-3.8-flash", "low").backfill_usd_central;
        let e_seeded = estimate_with(&counts);
        let seeded_cost = cost_at(&e_seeded, "gemini-3.8-flash", "low").backfill_usd_central;
        assert!(measured_cost > seeded_cost);
    }

    /// The central figure comes from measured per-call tokens; the range
    /// comes from the byte-size heuristic. The two methods can disagree.
    /// A measured average far outside the heuristic must still clamp into
    /// the displayed range, rather than draw outside it.
    #[test]
    fn a_central_figure_never_draws_outside_its_own_range() {
        let counts = Counts {
            threads: vec![old(leaves(100, 50, 80))],
            ..Counts::default()
        };
        let mut measured = HashMap::new();
        measured.insert(
            "gemini-3.8-flash".to_string(),
            MeasuredUsage {
                avg_input: 100_000_000,
                avg_output_low: 100_000_000,
            },
        );
        let estimate_high = estimate_measured(&counts, &measured);
        let high = cost_at(&estimate_high, "gemini-3.8-flash", "low");
        assert_eq!(high.backfill_usd_central, high.backfill_usd.high);

        measured.insert(
            "gemini-3.8-flash".to_string(),
            MeasuredUsage {
                avg_input: 0,
                avg_output_low: 0,
            },
        );
        let estimate_low = estimate_measured(&counts, &measured);
        let low = cost_at(&estimate_low, "gemini-3.8-flash", "low");
        assert_eq!(low.backfill_usd_central, low.backfill_usd.low);
    }

    fn recent_thread(all: Leaves) -> Tree {
        Tree {
            ready_window: true,
            ..old(all)
        }
    }

    /// Usable waits on the workspace and the recent threads only. So a long
    /// old thread moves the time to complete, never the time to usable.
    #[test]
    fn an_old_thread_moves_only_the_time_to_complete() {
        let base = Counts {
            threads: vec![recent_thread(leaves(40, 20, 30))],
            workspace: old(leaves(20, 10, 20)),
            built: 0,
        };
        let with_old = Counts {
            threads: vec![
                recent_thread(leaves(40, 20, 30)),
                old(leaves(4_000, 2_000, 3_000)),
            ],
            ..base.clone()
        };
        let (base, with_old) = (estimate_with(&base), estimate_with(&with_old));
        assert_eq!(with_old.usable_secs, base.usable_secs);
        assert!(with_old.complete_secs.low > base.complete_secs.low);
        for e in [&base, &with_old] {
            assert!(e.usable_secs.low <= e.complete_secs.low, "{e:?}");
            assert!(e.usable_secs.high <= e.complete_secs.high, "{e:?}");
        }
    }

    /// Calls run [`START_LIMIT`] at once, but one thread's leaves run one
    /// after another, so its leaf calls set a floor.
    #[test]
    fn the_longest_thread_sets_a_floor_on_the_time_to_complete() {
        let parallel = Counts {
            threads: vec![old(leaves(1, 1, 1)); 640],
            ..Counts::default()
        };
        let rounds = 640u64.div_ceil(START_LIMIT as u64);
        assert_eq!(
            estimate_with(&parallel).complete_secs.low,
            rounds * SECS_PER_CALL.low as u64
        );

        let serial = Counts {
            threads: vec![old(leaves(1_000, 500, 900))],
            ..Counts::default()
        };
        let e = estimate_with(&serial);
        assert_eq!(e.complete_secs.low, 500 * SECS_PER_CALL.low as u64);
        assert_eq!(e.complete_secs.high, 900 * SECS_PER_CALL.high as u64);
    }

    /// A model's timed history replaces the default seconds per call, for
    /// that model only.
    #[test]
    fn measured_call_time_sets_that_models_durations() {
        let counts = Counts {
            threads: vec![old(leaves(1_000, 500, 900))],
            ..Counts::default()
        };
        let measured = Measured {
            secs_per_call: HashMap::from([("gemini-3.8-flash".to_string(), 2.5)]),
            ..Measured::default()
        };
        let e = estimate(
            &counts,
            &measured,
            &crate::llm::model_registry::empty(),
            &ProviderKind::ALL,
        );
        let model = |id: &str| e.costs.iter().find(|c| c.model == id).unwrap();
        assert_eq!(
            model("gemini-3.8-flash").complete_secs,
            Bounds {
                low: 1_250,
                high: 2_250
            }
        );
        assert_eq!(model("gpt-6.1-sol").complete_secs, e.complete_secs);
    }

    #[test]
    fn context_tiling_bytes_is_bounded_by_the_budget_and_grows_with_the_tree() {
        assert_eq!(context_tiling_bytes(0), 0);
        assert!(context_tiling_bytes(4) <= context_tiling_bytes(4_000));
        assert!(context_tiling_bytes(4_000) <= CONTEXT_BYTES as u64);
    }

    /// One compactor call's capture, shaped as `AuxCapture` writes it.
    async fn insert_compaction_capture(
        pool: &PgPool,
        model: &str,
        effort: Option<&str>,
        input_tokens: u64,
        output_tokens: u64,
    ) {
        insert_timed_capture(pool, model, effort, input_tokens, output_tokens, None).await;
    }

    async fn insert_timed_capture(
        pool: &PgPool,
        model: &str,
        effort: Option<&str>,
        input_tokens: u64,
        output_tokens: u64,
        duration_ms: Option<u64>,
    ) {
        let mut payload = serde_json::json!({
            "type": "ContextCaptured",
            "producer": "auxiliary",
            "purpose": "summary_compaction",
            "model": model,
            "context_window": 0,
            "sections": [{ "name": "Summary Compaction Request", "budget_delta_chars": 0, "role": "user" }],
            "estimated_total_tokens": 0,
            "usage": {
                "input_tokens": input_tokens,
                "output_tokens": output_tokens,
                "cache_read_tokens": 0,
                "cache_creation_tokens": 0,
            },
        });
        if let Some(effort) = effort {
            payload["reasoning_effort"] = effort.into();
        }
        if let Some(ms) = duration_ms {
            payload["duration_ms"] = ms.into();
        }
        sqlx::query(
            "INSERT INTO events (id, event_type, payload, created) VALUES ($1, 'ContextCaptured', $2, now())",
        )
        .bind(uuid::Uuid::new_v4())
        .bind(payload)
        .execute(pool)
        .await
        .expect("insert");
    }

    /// Postgres's `avg(bigint)` is `numeric`, a wire type sqlx cannot decode
    /// into `f64` uncast. A zero-row average is NULL, so the bug only shows
    /// with real rows. One query reads every model: a labelled higher tier
    /// stays out of the baseline, and a thin model gets no entry.
    #[tokio::test]
    async fn measured_usage_reads_every_model_in_one_query() {
        let (pool, db_name) = crate::test_support::setup_test_db().await;
        for i in 0..MIN_MEASURED_CALLS {
            let effort = (i % 2 == 0).then_some("low");
            insert_compaction_capture(&pool, "gemini-3.8-flash", effort, 3000, 400).await;
            insert_compaction_capture(&pool, "claude-sonnet-5-5", None, 2000, 300).await;
        }
        insert_compaction_capture(&pool, "gemini-3.8-flash", Some("high"), 90_000, 9_000).await;
        for _ in 1..MIN_MEASURED_CALLS {
            insert_compaction_capture(&pool, "gpt-6.1-sol", None, 5000, 500).await;
        }

        let measured = measured_for_catalog(&pool).await.expect("query decodes");

        let usage = |model: &str| measured.usage.get(model).copied();
        let flash = MeasuredUsage {
            avg_input: 3000,
            avg_output_low: 400,
        };
        let sonnet = MeasuredUsage {
            avg_input: 2000,
            avg_output_low: 300,
        };
        assert_eq!(usage("gemini-3.8-flash"), Some(flash));
        assert_eq!(usage("claude-sonnet-5-5"), Some(sonnet));
        assert_eq!(usage("gpt-6.1-sol"), None, "too thin to trust");
        crate::test_support::teardown_test_db(&db_name).await;
    }

    /// Seconds per call average only the timed calls, and a model needs
    /// enough of them before its own time replaces the default.
    #[tokio::test]
    async fn measured_call_time_averages_only_timed_calls() {
        let (pool, db_name) = crate::test_support::setup_test_db().await;
        for i in 0..MIN_MEASURED_CALLS {
            let ms = if i % 2 == 0 { 2_000 } else { 3_000 };
            insert_timed_capture(&pool, "gemini-3.8-flash", None, 3000, 400, Some(ms)).await;
            insert_timed_capture(&pool, "gpt-6.1-sol", None, 3000, 400, None).await;
        }
        insert_timed_capture(&pool, "claude-sonnet-5-5", None, 3000, 400, Some(9_000)).await;

        let measured = measured_for_catalog(&pool).await.expect("read");
        assert_eq!(measured.secs_per_call.get("gemini-3.8-flash"), Some(&2.5));
        assert_eq!(measured.secs_per_call.get("gpt-6.1-sol"), None, "untimed");
        assert_eq!(
            measured.secs_per_call.get("claude-sonnet-5-5"),
            None,
            "thin"
        );
        assert!(measured.usage.contains_key("gpt-6.1-sol"));
        crate::test_support::teardown_test_db(&db_name).await;
    }

    /// On a history where compaction captures are a sliver, the planner must
    /// reach them through the partial index, never a scan of every event.
    #[tokio::test]
    async fn measured_usage_reads_through_its_index_on_a_large_history() {
        let (pool, db_name) = crate::test_support::setup_test_db().await;
        sqlx::query(
            "INSERT INTO events (id, event_type, payload, created) \
             SELECT gen_random_uuid(), \
                    CASE WHEN n % 2 = 0 THEN 'ContextCaptured' ELSE 'CodingAgentToolResult' END, \
                    jsonb_build_object( \
                      'producer', 'claude_code', \
                      'model', 'gemini-3.8-flash', \
                      'usage', jsonb_build_object('input_tokens', n, 'output_tokens', n), \
                      'content', repeat('x', 200)), \
                    now() \
             FROM generate_series(1, 50000) AS n",
        )
        .execute(&pool)
        .await
        .expect("filler history");
        for _ in 0..MIN_MEASURED_CALLS {
            insert_compaction_capture(&pool, "gemini-3.8-flash", None, 3000, 400).await;
        }
        sqlx::query("ANALYZE events")
            .execute(&pool)
            .await
            .expect("analyze");

        let plan: Vec<String> = sqlx::query_scalar(&format!("EXPLAIN {MEASURED_USAGE_SQL}"))
            .bind(compactor_model_ids())
            .fetch_all(&pool)
            .await
            .expect("explain");
        let plan = plan.join("\n");
        assert!(
            plan.contains("idx_events_summary_compaction_captures"),
            "{plan}"
        );
        assert!(!plan.contains("Seq Scan on events"), "{plan}");

        let measured = measured_for_catalog(&pool).await.expect("read");
        assert_eq!(
            measured.usage.get("gemini-3.8-flash").map(|u| u.avg_input),
            Some(3000),
            "filler without the compaction purpose stays out of the average"
        );
        crate::test_support::teardown_test_db(&db_name).await;
    }
}
