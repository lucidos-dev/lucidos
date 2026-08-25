//! The five axes, per configuration, in absolute numbers (ADR 0110).
//!
//! A *configuration* is what a run measured: an arm and a declared context
//! window. Everything below is grouped by one, and nothing below is a ratio.
//! Pooling several runs is how a budget sweep is read, so this module takes a
//! list of runs and groups rather than merging.
//!
//! One thing it refuses outright: pooling runs that measured different things
//! (I6). That is the fixture, the guidance, the prices and the model, because a
//! sweep across two of any of them is not a budget curve.
//!
//! Two arms out of order inside one run is also refused (I7). Two arms from
//! SEPARATE runs cannot be caught that way, so the comparison is printed with
//! `interleaved` false rather than withheld.
//!
//! ADR 0087's bar lived here: a verdict, four graduation conditions, six kills,
//! a stratified secondary and a power calculation. All of it is retired. It
//! graded a mechanism ADR 0109 replaced, and it was frozen at a confirmatory
//! run that never happened.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::arm::Arm;
use crate::metrics::TokenCounts;
use crate::probe::Outcome;
use crate::results::{
    CompletionOutcome, CompletionRow, ProbeRow, ResultRow, RunConfig, RunRow, ThreadRow,
    EMPTY_STATUS,
};

type Fallible<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

/// The task whose probes ask what the final handover carried.
pub const CENSUS_TASK: &str = "T12";

/// How far delivery or fidelity may fall before a budget stops holding.
///
/// Five points, borrowed from ADR 0087's completion margin and argued the same
/// way. It is 0.7 of the 14 tasks and 3.3 of the 65 fidelity items. So one
/// unlucky probe cannot move the answer and a pattern can.
pub const QUALITY_TOLERANCE: f64 = 0.05;

/// The smallest context window a run may declare.
///
/// The tools array and the system prompt are roughly 85,000 chars, and the
/// engine's budget is `(window - 8000) * 1.5` chars. Below about 64,000 tokens
/// the message budget is zero, so every round ships over budget and the run
/// measures the overhead rather than the agent. See ADR 0110 decision 11.
pub const MIN_CONTEXT_WINDOW: i64 = 72_000;

/// One measured configuration: the pins a run put under test.
///
/// The window is on it because a budget sweep is several runs of the same arm,
/// and the whole point is telling them apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Configuration {
    pub arm: Arm,
    /// The context window the engine resolved, in tokens. Zero when no thread
    /// captured one, which is what a run that never reached the model leaves.
    pub context_window: i64,
}

impl Configuration {
    pub fn label(&self) -> String {
        match self.context_window {
            0 => format!("{} arm, window unrecorded", self.arm),
            window => format!("{} arm, {}k window", self.arm, window / 1_000),
        }
    }
}

/// A count over a denominator, kept together so a reader sees the sample.
///
/// A bare proportion hides how much it rests on. Three of four and thirty of
/// forty read identically as 0.75 and mean very different things.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct Rate {
    pub passed: usize,
    pub scored: usize,
    pub rate: f64,
}

impl Rate {
    fn of(passed: usize, scored: usize) -> Rate {
        Rate {
            passed,
            scored,
            rate: match scored {
                0 => 0.0,
                scored => passed as f64 / scored as f64,
            },
        }
    }
}

/// Axis 1: did the configuration do the work, and did the facts survive.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Quality {
    /// One completion probe per task: the deliverable exists and is right.
    pub delivery: Rate,
    /// The planted probes: a fact the task needed survived.
    pub fidelity: Rate,
    /// The final handover's own probes, which are fidelity on the last task.
    pub handover: Rate,
    /// Every failed fidelity probe, split by how it failed. Wrong and silent is
    /// the one that matters, and the split is what tells it from the others.
    pub failures: BTreeMap<String, usize>,
    /// Fidelity per recovery tier, so a loss says how cheap the fact was to
    /// re-derive.
    pub by_tier: BTreeMap<u8, Rate>,
}

/// Axis 2: what it cost.
///
/// **Cache read and cache creation are reported apart, per round.** One dollar
/// figure hides the defect they expose. A healthy thread reads its prefix back
/// at 0.1x and writes only the delta, so reads climb and writes stay small. A
/// thread whose message array is edited in place writes the whole array at
/// 1.25x every round and never reads it back. The frozen read is the tell.
/// See `knowhow/context-mode-eval-mechanics.md`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Cost {
    /// False when the price table prices the model under test at zero. Every
    /// dollar below is then a real 0.00 rather than a cheap run, and saying so
    /// is the difference between a measurement and a blank.
    ///
    /// Judged on main-agent spend alone. An arm on a free model still bills its
    /// title work to a priced auxiliary model. That spend must not read as a
    /// measured cost axis.
    pub measured: bool,
    pub usd: f64,
    /// The part of `usd` the auxiliary models caused, at their own rates.
    ///
    /// Title, memory and summary work runs on the extractor default rather than
    /// the model under test. It is inside the totals, because the arm caused it,
    /// and reported apart, because it prices at another rate.
    pub usd_auxiliary: f64,
    pub usd_per_task: f64,
    pub usd_per_round: f64,
    /// Input the provider read fresh, with the cached counts taken out.
    ///
    /// Fresh, cache read and cache creation are disjoint and add up to
    /// everything the model processed. Reporting the raw stored total here
    /// instead would overlap the two below it and read as three times the
    /// traffic.
    pub fresh_input: i64,
    pub output_tokens: i64,
    pub cache_read: i64,
    pub cache_creation: i64,
    /// The part of the four counts above the auxiliary models processed.
    pub auxiliary_tokens: TokenCounts,
    pub fresh_input_per_round: f64,
    /// Cached tokens read per round. Frozen across a thread's rounds means the
    /// prefix stopped matching, and the cache is doing nothing.
    pub cache_read_per_round: f64,
    /// Cached tokens written per round. Growing with the context means the
    /// array is being re-created rather than extended.
    pub cache_creation_per_round: f64,
}

impl Cost {
    /// What the model under test cost, with auxiliary work taken out.
    pub fn usd_main_agent(&self) -> f64 {
        self.usd - self.usd_auxiliary
    }
}

/// Axis 3: how many rounds it took, and what it spent them on.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Rounds {
    pub total: i64,
    pub per_task_median: f64,
    pub per_task_max: i64,
    /// Calls to a recovery tool after round 1, over every thread. What a fact
    /// leaving the prompt costs.
    pub recovery_calls: i64,
    /// Of those, the ones fetching a handle the thread already fetched. The
    /// thrash signal.
    pub repeat_recoveries: i64,
}

/// Axis 4: how long it took.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Timing {
    pub total_secs: i64,
    pub per_task_median: f64,
    pub per_task_max: i64,
}

/// Axis 5: how full the requests got, and against what.
///
/// Peak and mean say different things and both are here. A high mean is a
/// configuration that carried a lot all along. A high peak over a low mean is
/// one round that spiked.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Utilisation {
    pub context_window: i64,
    /// The largest request any thread sent, in the engine's own estimate.
    pub peak_tokens: i64,
    /// The mean over every thread's own mean, so a long thread does not
    /// outvote a short one.
    pub mean_tokens: f64,
    /// What was left at the peak. Negative means the request was bigger than
    /// the window the engine believed it had, which the trimmer then fixed.
    pub headroom_at_peak: i64,
    /// The peak as a share of the window. The one number saying whether this
    /// configuration was ever under pressure at all.
    pub peak_share: f64,
    pub trimmed_rounds: i64,
    pub trimmed_threads: usize,
}

/// Axis 6: how the model wrote its working understanding.
///
/// Collected per thread from the first run and aggregated by none of them, so
/// the design question these three were added to answer went unanswered.
///
/// The mode claims the document rides along with a tool call rather than
/// costing a round of its own. `beside_a_call` is that claim as one rate.
/// Totals carry a per-task mean beside them, because two arms rarely run the
/// same number of threads once a pair is voided.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Document {
    pub writes: i64,
    pub writes_per_task: f64,
    /// Of the writes, the ones that shared their reply with a tool call.
    pub beside_a_call: Rate,
    pub items_held_open: i64,
    pub items_held_open_per_task: f64,
}

/// One task's own numbers, so the report can show where a run went wrong.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TaskRow {
    pub task: String,
    pub delivered: Option<bool>,
    pub fidelity: Rate,
    pub rounds: i64,
    pub usd: f64,
    pub wall_secs: i64,
    pub peak_tokens: i64,
    pub trimmed_rounds: i64,
}

/// Everything one configuration did.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConfigurationResult {
    pub configuration: Configuration,
    /// Sequence runs behind these numbers. One repeat is one run.
    pub repeats: usize,
    pub threads: usize,
    pub quality: Quality,
    pub cost: Cost,
    pub rounds: Rounds,
    pub timing: Timing,
    pub utilisation: Utilisation,
    pub document: Document,
    /// Per task, ordered as the fixture declares them.
    pub tasks: Vec<TaskRow>,
}

/// One row of the budget sweep, reduced to what the curve needs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SweepRow {
    pub context_window: i64,
    pub delivery: f64,
    pub fidelity: f64,
    pub usd: f64,
    pub rounds: i64,
    pub peak_tokens: i64,
    pub peak_share: f64,
    pub trimmed_rounds: i64,
    /// Whether quality held here, against the largest window in this sweep.
    pub holds: bool,
}

/// The quality-and-cost curve against budget, for one arm.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Sweep {
    pub arm: Arm,
    /// Largest window first, so the reference row is at the top.
    pub rows: Vec<SweepRow>,
    /// The window every other row is judged against.
    pub reference_window: i64,
    /// The headline: the smallest window where delivery and fidelity both held.
    ///
    /// `None` when the sweep has one window, because there is nothing to
    /// compare, and when no smaller window held.
    pub smallest_holding: Option<i64>,
}

/// How many task-arm pairs measured anything, and what removed the rest.
///
/// Only a two-arm run can void a pair, so a single-configuration run reports
/// attempted and effective as the same number.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PairCensus {
    pub attempted: usize,
    pub empty_completion: usize,
    pub classifier_disagreement: usize,
    pub completion_divergence: usize,
    pub upstream_failure: usize,
    pub effective: usize,
}

/// Turns the model ended with nothing, and what the re-posts recovered.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct EmptyCompletions {
    pub turns: i64,
    pub retries: i64,
    pub recovered_threads: usize,
    pub unrecovered_threads: usize,
}

/// Two configurations side by side, in differences rather than ratios.
///
/// Printed only when a run carried two arms. A difference leaves both absolute
/// figures readable beside it. A ratio replaces them with one number whose
/// denominator you then have to go and find.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Comparison {
    pub left: Configuration,
    pub right: Configuration,
    /// Right minus left, in points.
    pub delivery_points: f64,
    pub fidelity_points: f64,
    /// Right minus left.
    pub usd: f64,
    pub rounds: i64,
    pub peak_tokens: i64,
    pub document_writes: i64,
    pub items_held_open: i64,
    /// Right minus left, in points. The share of document writes that shared a
    /// reply with a tool call, so cost no round of their own.
    pub beside_a_call_points: f64,
    /// Whether the two arms ran inside one invocation, task by task.
    ///
    /// False when they came from separate runs, which the baseline workflow
    /// produces. Provider drift then lands on one side of the comparison, and
    /// I7's interleave check cannot see it: each run has one arm per task, so
    /// there is nothing out of order to find. Reported rather than refused,
    /// because the two absolute blocks above it are still good.
    pub interleaved: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Analysis {
    pub config: RunConfig,
    /// One block per configuration, largest window first.
    pub configurations: Vec<ConfigurationResult>,
    /// One per arm that ran at more than one window.
    pub sweeps: Vec<Sweep>,
    /// Populated only when a run carried two arms at one window.
    pub comparisons: Vec<Comparison>,
    pub pairs: PairCensus,
    pub empty_completions: EmptyCompletions,
    /// What each pooled run named its arm workspaces after, deduplicated.
    ///
    /// The report prints these so an operator can find the databases again.
    /// Empty for a run recorded before the label existed, whose workspaces
    /// carried no label at all.
    pub run_labels: Vec<String>,
}

/// Run the whole analysis over one or more results files' rows.
pub fn analyse(rows: &[ResultRow]) -> Fallible<Analysis> {
    let runs: Vec<&RunRow> = rows
        .iter()
        .filter_map(|row| match row {
            ResultRow::Run(run) => Some(run),
            _ => None,
        })
        .collect();
    let first = *runs
        .first()
        .ok_or("no run row: these results carry nothing to analyse")?;
    check_one_fixture(&runs)?;

    let threads: Vec<&ThreadRow> = rows
        .iter()
        .filter_map(|row| match row {
            ResultRow::Thread(thread) => Some(thread),
            _ => None,
        })
        .collect();
    let probes: Vec<&ProbeRow> = rows
        .iter()
        .filter_map(|row| match row {
            ResultRow::Probe(probe) => Some(probe),
            _ => None,
        })
        .collect();
    let completions: Vec<&CompletionRow> = rows
        .iter()
        .filter_map(|row| match row {
            ResultRow::Completion(done) => Some(done),
            _ => None,
        })
        .collect();
    check_arms_interleave(&threads)?;

    let windows = windows_by_run(&runs, &threads);
    let classifier = classifier_voided_pairs(&threads);
    let diverged = completion_diverged_pairs(&completions);
    let empty = empty_completion_pairs(&threads);
    let voided: BTreeSet<PairKey> = classifier
        .union(&diverged)
        .cloned()
        .collect::<BTreeSet<_>>()
        .union(&empty)
        .cloned()
        .collect();

    let task_order: Vec<String> = first.tasks.clone();
    let mut configurations: Vec<ConfigurationResult> = configurations_present(&threads, &windows)
        .into_iter()
        .map(|configuration| {
            configuration_result(
                configuration,
                &threads,
                &probes,
                &completions,
                &windows,
                &task_order,
            )
        })
        .collect();
    // Largest window first, so the reference the sweep is judged against is the
    // first block a reader meets.
    configurations.sort_by(|a, b| {
        b.configuration
            .context_window
            .cmp(&a.configuration.context_window)
            .then(a.configuration.arm.cmp(&b.configuration.arm))
    });

    let runs_by_configuration: BTreeMap<Configuration, BTreeSet<String>> = configurations
        .iter()
        .map(|result| {
            (
                result.configuration,
                runs_of(result.configuration, &threads, &windows),
            )
        })
        .collect();
    Ok(Analysis {
        config: first.config,
        sweeps: sweeps(&configurations),
        comparisons: comparisons(&configurations, &runs_by_configuration),
        pairs: pair_census(
            &threads,
            &probes,
            &Voided {
                all: &voided,
                classifier: &classifier,
                empty: &empty,
            },
        ),
        empty_completions: empty_completions(&threads),
        run_labels: runs
            .iter()
            .map(|run| run.run_label.clone())
            .filter(|label| !label.is_empty())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect(),
        configurations,
    })
}

/// The window each run declared, keyed by run id.
///
/// The run row is the intent and a thread's captures are what happened, so the
/// thread wins where the two disagree. A run whose threads never captured
/// anything falls back to what it asked for.
fn windows_by_run(runs: &[&RunRow], threads: &[&ThreadRow]) -> BTreeMap<String, i64> {
    let mut windows: BTreeMap<String, i64> = runs
        .iter()
        .map(|run| (run.run_id.clone(), run.context_window))
        .collect();
    for thread in threads.iter().filter(|t| t.context_window > 0) {
        windows.insert(thread.run_id.clone(), thread.context_window);
    }
    windows
}

fn configuration_of(thread: &ThreadRow, windows: &BTreeMap<String, i64>) -> Configuration {
    Configuration {
        arm: thread.arm,
        context_window: windows.get(&thread.run_id).copied().unwrap_or_default(),
    }
}

fn configurations_present(
    threads: &[&ThreadRow],
    windows: &BTreeMap<String, i64>,
) -> Vec<Configuration> {
    threads
        .iter()
        .map(|thread| configuration_of(thread, windows))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

/// Which run ids belong to one configuration.
fn runs_of(
    configuration: Configuration,
    threads: &[&ThreadRow],
    windows: &BTreeMap<String, i64>,
) -> BTreeSet<String> {
    threads
        .iter()
        .filter(|thread| configuration_of(thread, windows) == configuration)
        .map(|thread| thread.run_id.clone())
        .collect()
}

fn configuration_result(
    configuration: Configuration,
    threads: &[&ThreadRow],
    probes: &[&ProbeRow],
    completions: &[&CompletionRow],
    windows: &BTreeMap<String, i64>,
    task_order: &[String],
) -> ConfigurationResult {
    let run_ids = runs_of(configuration, threads, windows);
    let mine: Vec<&ThreadRow> = threads
        .iter()
        .copied()
        .filter(|thread| configuration_of(thread, windows) == configuration)
        .collect();
    let my_probes: Vec<&ProbeRow> = probes
        .iter()
        .copied()
        .filter(|probe| probe.arm == configuration.arm && run_ids.contains(&probe.run_id))
        .collect();
    let my_completions: Vec<&CompletionRow> = completions
        .iter()
        .copied()
        .filter(|done| done.arm == configuration.arm && run_ids.contains(&done.run_id))
        .collect();

    ConfigurationResult {
        configuration,
        repeats: mine
            .iter()
            .map(|thread| (thread.run_id.as_str(), thread.repeat))
            .collect::<BTreeSet<_>>()
            .len(),
        threads: mine.len(),
        quality: quality(&my_probes, &my_completions),
        cost: cost(&mine),
        rounds: rounds(&mine),
        timing: timing(&mine),
        utilisation: utilisation(configuration.context_window, &mine),
        document: document(&mine),
        tasks: task_rows(&mine, &my_probes, &my_completions, task_order),
    }
}

/// Whether a probe belongs to the handover rather than to a planted fact.
fn is_handover(probe: &ProbeRow) -> bool {
    probe.task == CENSUS_TASK
}

fn quality(probes: &[&ProbeRow], completions: &[&CompletionRow]) -> Quality {
    let scored: Vec<&&ProbeRow> = probes.iter().filter(|p| p.outcome.is_scored()).collect();
    let count = |predicate: &dyn Fn(&ProbeRow) -> bool| -> Rate {
        let picked: Vec<&&&ProbeRow> = scored.iter().filter(|p| predicate(p)).collect();
        Rate::of(
            picked.iter().filter(|p| p.outcome.is_pass()).count(),
            picked.len(),
        )
    };
    let mut failures: BTreeMap<String, usize> =
        [Outcome::Asked, Outcome::LostLoud, Outcome::LostSilent]
            .into_iter()
            .map(|outcome| (outcome.as_str().to_string(), 0))
            .collect();
    let mut by_tier_counts: BTreeMap<u8, (usize, usize)> = BTreeMap::new();
    for probe in &scored {
        if !probe.outcome.is_pass() {
            *failures
                .entry(probe.outcome.as_str().to_string())
                .or_default() += 1;
        }
        if let Some(tier) = probe.tier {
            let cell = by_tier_counts.entry(tier).or_insert((0, 0));
            cell.1 += 1;
            if probe.outcome.is_pass() {
                cell.0 += 1;
            }
        }
    }
    Quality {
        delivery: Rate::of(
            completions
                .iter()
                .filter(|done| done.outcome == CompletionOutcome::Pass)
                .count(),
            completions
                .iter()
                .filter(|done| done.outcome.is_scored())
                .count(),
        ),
        fidelity: count(&|probe| !is_handover(probe)),
        handover: count(&is_handover),
        failures,
        by_tier: by_tier_counts
            .into_iter()
            .map(|(tier, (passed, scored))| (tier, Rate::of(passed, scored)))
            .collect(),
    }
}

fn cost(threads: &[&ThreadRow]) -> Cost {
    let usd: f64 = threads.iter().map(|t| t.usd).sum();
    let usd_auxiliary: f64 = threads.iter().map(|t| t.usd_auxiliary).sum();
    let total_rounds: i64 = threads.iter().map(|t| t.rounds).sum();
    let splits: Vec<_> = threads.iter().map(|t| t.input_split()).collect();
    let fresh: i64 = splits.iter().map(|s| s.fresh()).sum();
    let cache_read: i64 = splits.iter().map(|s| s.cache_read()).sum();
    let cache_creation: i64 = splits.iter().map(|s| s.cache_creation()).sum();
    let auxiliary_tokens = threads.iter().fold(TokenCounts::default(), |total, t| {
        total.plus(t.auxiliary_tokens)
    });
    let mut cost = Cost {
        measured: false,
        usd,
        usd_auxiliary,
        usd_per_task: divide(usd, threads.len() as i64),
        usd_per_round: divide(usd, total_rounds),
        fresh_input: fresh,
        output_tokens: threads.iter().map(|t| t.output_tokens).sum(),
        cache_read,
        cache_creation,
        auxiliary_tokens,
        fresh_input_per_round: divide(fresh as f64, total_rounds),
        cache_read_per_round: divide(cache_read as f64, total_rounds),
        cache_creation_per_round: divide(cache_creation as f64, total_rounds),
    };
    // Main-agent dollars, never the total. A free model under test with a
    // priced title model would otherwise report a measured cost axis.
    cost.measured = cost.usd_main_agent() > 0.0;
    cost
}

fn rounds(threads: &[&ThreadRow]) -> Rounds {
    let per_task: Vec<f64> = threads.iter().map(|t| t.rounds as f64).collect();
    Rounds {
        total: threads.iter().map(|t| t.rounds).sum(),
        per_task_median: median(&per_task),
        per_task_max: threads.iter().map(|t| t.rounds).max().unwrap_or(0),
        recovery_calls: threads
            .iter()
            .map(|t| t.recovery_calls.values().sum::<i64>())
            .sum(),
        repeat_recoveries: threads.iter().map(|t| t.repeat_recoveries).sum(),
    }
}

fn document(threads: &[&ThreadRow]) -> Document {
    let writes: i64 = threads.iter().map(|t| t.document_writes).sum();
    let beside: i64 = threads.iter().map(|t| t.writes_with_a_tool_call).sum();
    let held: i64 = threads.iter().map(|t| t.items_held_open).sum();
    Document {
        writes,
        writes_per_task: divide(writes as f64, threads.len() as i64),
        beside_a_call: Rate::of(beside as usize, writes as usize),
        items_held_open: held,
        items_held_open_per_task: divide(held as f64, threads.len() as i64),
    }
}

fn timing(threads: &[&ThreadRow]) -> Timing {
    let per_task: Vec<f64> = threads.iter().map(|t| t.wall_secs as f64).collect();
    Timing {
        total_secs: threads.iter().map(|t| t.wall_secs).sum(),
        per_task_median: median(&per_task),
        per_task_max: threads.iter().map(|t| t.wall_secs).max().unwrap_or(0),
    }
}

fn utilisation(context_window: i64, threads: &[&ThreadRow]) -> Utilisation {
    let peak = threads
        .iter()
        .map(|t| t.peak_request_tokens)
        .max()
        .unwrap_or(0);
    let means: Vec<f64> = threads
        .iter()
        .filter(|t| t.mean_request_tokens > 0)
        .map(|t| t.mean_request_tokens as f64)
        .collect();
    Utilisation {
        context_window,
        peak_tokens: peak,
        mean_tokens: mean(&means),
        headroom_at_peak: context_window - peak,
        peak_share: divide(peak as f64, context_window),
        trimmed_rounds: threads.iter().map(|t| t.trimmed_rounds).sum(),
        trimmed_threads: threads.iter().filter(|t| t.trimmed_rounds > 0).count(),
    }
}

fn task_rows(
    threads: &[&ThreadRow],
    probes: &[&ProbeRow],
    completions: &[&CompletionRow],
    task_order: &[String],
) -> Vec<TaskRow> {
    let mut ordered: Vec<String> = task_order.to_vec();
    for thread in threads {
        if !ordered.contains(&thread.task) {
            ordered.push(thread.task.clone());
        }
    }
    ordered
        .into_iter()
        .filter_map(|task| {
            let mine: Vec<&&ThreadRow> = threads.iter().filter(|t| t.task == task).collect();
            let scored: Vec<&&ProbeRow> = probes
                .iter()
                .filter(|p| p.task == task && p.outcome.is_scored())
                .collect();
            let delivered = completions
                .iter()
                .filter(|c| c.task == task && c.outcome.is_scored())
                .map(|c| c.outcome == CompletionOutcome::Pass)
                .next_back();
            if mine.is_empty() && scored.is_empty() && delivered.is_none() {
                return None;
            }
            Some(TaskRow {
                task,
                delivered,
                fidelity: Rate::of(
                    scored.iter().filter(|p| p.outcome.is_pass()).count(),
                    scored.len(),
                ),
                rounds: mine.iter().map(|t| t.rounds).sum(),
                usd: mine.iter().map(|t| t.usd).sum(),
                wall_secs: mine.iter().map(|t| t.wall_secs).sum(),
                peak_tokens: mine
                    .iter()
                    .map(|t| t.peak_request_tokens)
                    .max()
                    .unwrap_or(0),
                trimmed_rounds: mine.iter().map(|t| t.trimmed_rounds).sum(),
            })
        })
        .collect()
}

/// One sweep per arm that ran at more than one window.
///
/// One window is not a sweep. Reported as one, it would print a
/// "smallest budget that held" over a set of size one. That number is just the
/// only window there was.
fn sweeps(configurations: &[ConfigurationResult]) -> Vec<Sweep> {
    let mut by_arm: BTreeMap<Arm, Vec<&ConfigurationResult>> = BTreeMap::new();
    for result in configurations {
        by_arm
            .entry(result.configuration.arm)
            .or_default()
            .push(result);
    }
    by_arm
        .into_iter()
        .filter(|(_, results)| results.len() > 1)
        .map(|(arm, mut results)| {
            results.sort_by_key(|r| std::cmp::Reverse(r.configuration.context_window));
            let reference = results[0];
            let rows: Vec<SweepRow> = results
                .iter()
                .map(|result| SweepRow {
                    context_window: result.configuration.context_window,
                    delivery: result.quality.delivery.rate,
                    fidelity: result.quality.fidelity.rate,
                    usd: result.cost.usd,
                    rounds: result.rounds.total,
                    peak_tokens: result.utilisation.peak_tokens,
                    peak_share: result.utilisation.peak_share,
                    trimmed_rounds: result.utilisation.trimmed_rounds,
                    holds: holds_against(result, reference),
                })
                .collect();
            Sweep {
                arm,
                reference_window: reference.configuration.context_window,
                // The smallest window that held, and never the smallest that
                // ran. A row that failed does not disqualify a smaller row that
                // passed: the table shows both, and a non-monotone sweep is a
                // result worth seeing rather than one to smooth over.
                smallest_holding: rows
                    .iter()
                    .filter(|row| row.holds)
                    .map(|row| row.context_window)
                    .min(),
                rows,
            }
        })
        .collect()
}

/// Whether quality held at this window, against the sweep's largest.
///
/// A reference that scored nothing cannot be held against. Its rates are both
/// 0.0, which every other row clears, so a broken reference run would report
/// the tightest budget in the sweep as holding.
fn holds_against(result: &ConfigurationResult, reference: &ConfigurationResult) -> bool {
    if reference.quality.delivery.scored == 0 || reference.quality.fidelity.scored == 0 {
        return false;
    }
    result.quality.delivery.rate >= reference.quality.delivery.rate - QUALITY_TOLERANCE
        && result.quality.fidelity.rate >= reference.quality.fidelity.rate - QUALITY_TOLERANCE
}

/// Two arms at one window, differenced. Empty on a single-configuration run.
fn comparisons(
    configurations: &[ConfigurationResult],
    runs_by_configuration: &BTreeMap<Configuration, BTreeSet<String>>,
) -> Vec<Comparison> {
    let mut by_window: BTreeMap<i64, Vec<&ConfigurationResult>> = BTreeMap::new();
    for result in configurations {
        by_window
            .entry(result.configuration.context_window)
            .or_default()
            .push(result);
    }
    by_window
        .into_values()
        .filter(|results| results.len() == 2)
        .map(|results| {
            let (left, right) = (results[0], results[1]);
            let shared = |configuration| {
                runs_by_configuration
                    .get(configuration)
                    .cloned()
                    .unwrap_or_default()
            };
            Comparison {
                interleaved: !shared(&left.configuration)
                    .intersection(&shared(&right.configuration))
                    .collect::<Vec<_>>()
                    .is_empty(),
                left: left.configuration,
                right: right.configuration,
                delivery_points: (right.quality.delivery.rate - left.quality.delivery.rate) * 100.0,
                fidelity_points: (right.quality.fidelity.rate - left.quality.fidelity.rate) * 100.0,
                usd: right.cost.usd - left.cost.usd,
                rounds: right.rounds.total - left.rounds.total,
                peak_tokens: right.utilisation.peak_tokens - left.utilisation.peak_tokens,
                document_writes: right.document.writes - left.document.writes,
                items_held_open: right.document.items_held_open - left.document.items_held_open,
                beside_a_call_points: (right.document.beside_a_call.rate
                    - left.document.beside_a_call.rate)
                    * 100.0,
            }
        })
        .collect()
}

/// Every turn that came back with nothing, and what became of its thread.
///
/// The turns are attempts and the recovery counts are threads, because that is
/// what each is honestly known at. A thread carries one status, so it recovered
/// or it did not; a turn can be attempted three times.
///
/// Recovery is `finished()`, never "the status is not `empty-completion`". A
/// re-post that then timed out recovered nothing.
fn empty_completions(threads: &[&ThreadRow]) -> EmptyCompletions {
    let mut summary = EmptyCompletions::default();
    for thread in threads {
        summary.turns += thread.empty_completions;
        summary.retries += thread.empty_retries;
        if thread.empty_completions == 0 {
            continue;
        }
        match thread.finished() {
            true => summary.recovered_threads += 1,
            false => summary.unrecovered_threads += 1,
        }
    }
    summary
}

/// I6: every run row in the analysis has to agree on what was measured.
///
/// Six things define that, and a sweep pools several runs, so all six are
/// checked rather than the fixture alone. A sweep whose model changed halfway
/// is a model comparison wearing a budget curve's clothes.
///
/// `engine_commit` is deliberately not here. Rebuilding between runs is normal
/// and mostly harmless, and `guidance_hash` already catches the one engine
/// change that alters what the model is told.
pub fn check_one_fixture(runs: &[&RunRow]) -> Fallible<()> {
    let differences: Vec<String> = [
        ("fixture_hash", pinned(runs, |run| run.fixture_hash.clone())),
        (
            "guidance_hash",
            pinned(runs, |run| run.guidance_hash.clone()),
        ),
        ("prices_hash", pinned(runs, |run| run.prices_hash.clone())),
        ("model", pinned(runs, |run| run.model.clone())),
        // Six, not four. `guidance_hash` catches a moved schedule only while
        // the prompt quotes both numbers, and it names neither when it does.
        (
            "expire_after_rounds",
            pinned(runs, |run| run.expire_after_rounds.to_string()),
        ),
        (
            "sweep_every_rounds",
            pinned(runs, |run| run.sweep_every_rounds.to_string()),
        ),
    ]
    .into_iter()
    .filter(|(_, values)| values.len() > 1)
    .map(|(field, values)| format!("{field} differs: {}", values.join(", ")))
    .collect();
    if differences.is_empty() {
        return Ok(());
    }
    Err(format!(
        "pooled_runs_disagree: these results measured different things, so they cannot be \
         pooled. A probe or a price edited after seeing data is not a measurement, and a \
         sweep across two models is not a budget curve. {}",
        differences.join("; ")
    )
    .into())
}

/// The distinct values one pinned field took across the pooled runs.
///
/// Rendered rather than borrowed, so a number pins the same way a hash does.
fn pinned(runs: &[&RunRow], field: fn(&RunRow) -> String) -> Vec<String> {
    runs.iter()
        .map(|run| field(run))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

/// I7: inside each repeat, two arms ran each task back to back.
///
/// Provider drift is the confound with no defence except timing. A
/// single-configuration run has nothing to interleave, so every task appears
/// once and the check passes without asserting anything.
pub fn check_arms_interleave(threads: &[&ThreadRow]) -> Fallible<()> {
    let mut by_repeat: BTreeMap<(&str, u32), Vec<&&ThreadRow>> = BTreeMap::new();
    for thread in threads {
        by_repeat
            .entry((thread.run_id.as_str(), thread.repeat))
            .or_default()
            .push(thread);
    }
    for ((run_id, repeat), mut rows) in by_repeat {
        rows.retain(|row| row.started.is_some());
        if rows.len() < 2 {
            continue;
        }
        rows.sort_by_key(|row| row.started);
        let ordering: Vec<&str> = rows.iter().map(|row| row.task.as_str()).collect();
        for (index, task) in ordering.iter().enumerate() {
            if ordering.iter().filter(|other| *other == task).count() < 2 {
                continue;
            }
            let paired = index
                .checked_sub(1)
                .and_then(|before| ordering.get(before))
                .is_some_and(|other| other == task)
                || ordering.get(index + 1).is_some_and(|other| other == task);
            if !paired {
                return Err(format!(
                    "arms_not_interleaved: in run {run_id} repeat {repeat}, task {task} does \
                     not sit beside its other arm in start order. Provider drift then lands on \
                     one arm only. Order was: {}",
                    ordering.join(", ")
                )
                .into());
            }
        }
    }
    Ok(())
}

/// What identifies one task-arm pair: one task of one repeat, in both arms.
pub type PairKey = (String, u32, String);

pub fn pair_key(run_id: &str, repeat: u32, task: &str) -> PairKey {
    (run_id.to_string(), repeat, task.to_string())
}

/// Whether the arms disagreed about retrieval on one task of one repeat.
///
/// The engine's query classifier decides whether memory is retrieved at all,
/// and it is an LLM call. A task one arm retrieved for and the other did not
/// measures that call rather than the configuration. So the pair is voided.
///
/// Both arms skipping is agreement, and is kept. An arm missing entirely is not
/// a disagreement either: one arm cannot disagree with nothing.
pub fn retrieval_disagreed(by_arm: &BTreeMap<Arm, bool>) -> bool {
    matches!(
        (by_arm.get(&Arm::Control), by_arm.get(&Arm::Lean)),
        (Some(control), Some(lean)) if control != lean
    )
}

/// Whether a turn came back with nothing in either arm of one pair.
///
/// Either arm is enough, unlike a disagreement. A disagreement needs two
/// opinions, and this needs one thread that never started the task.
pub fn empty_completion_voided(by_arm: &BTreeMap<Arm, bool>) -> bool {
    by_arm.values().any(|empty| *empty)
}

/// Every pair an arm's thread never produced anything on.
pub fn empty_completion_pairs(threads: &[&ThreadRow]) -> BTreeSet<PairKey> {
    threads
        .iter()
        .filter(|thread| thread.status == EMPTY_STATUS)
        .map(|thread| pair_key(&thread.run_id, thread.repeat, &thread.task))
        .collect()
}

/// Every pair the arms disagreed about, read back off the thread rows.
pub fn classifier_voided_pairs(threads: &[&ThreadRow]) -> BTreeSet<PairKey> {
    let mut by_pair: BTreeMap<PairKey, BTreeMap<Arm, bool>> = BTreeMap::new();
    for thread in threads {
        by_pair
            .entry(pair_key(&thread.run_id, thread.repeat, &thread.task))
            .or_default()
            .insert(thread.arm, thread.memory_recalled);
    }
    by_pair
        .into_iter()
        .filter(|(_, by_arm)| retrieval_disagreed(by_arm))
        .map(|(pair, _)| pair)
        .collect()
}

/// Whether the arms disagreed about delivering one task of one repeat.
///
/// Both arms have to have been scored on it. A void is not a disagreement, and
/// one arm cannot disagree with nothing.
pub fn completion_diverged(by_arm: &BTreeMap<Arm, CompletionOutcome>) -> bool {
    matches!(
        (by_arm.get(&Arm::Control), by_arm.get(&Arm::Lean)),
        (Some(control), Some(lean))
            if control.is_scored() && lean.is_scored() && control != lean
    )
}

/// Every pair one arm delivered and the other did not.
pub fn completion_diverged_pairs(completions: &[&CompletionRow]) -> BTreeSet<PairKey> {
    let mut by_pair: BTreeMap<PairKey, BTreeMap<Arm, CompletionOutcome>> = BTreeMap::new();
    for row in completions {
        by_pair
            .entry(pair_key(&row.run_id, row.repeat, &row.task))
            .or_default()
            .insert(row.arm, row.outcome);
    }
    by_pair
        .into_iter()
        .filter(|(_, by_arm)| completion_diverged(by_arm))
        .map(|(pair, _)| pair)
        .collect()
}

/// The voided pairs, and which cause each is attributed to.
struct Voided<'a> {
    all: &'a BTreeSet<PairKey>,
    classifier: &'a BTreeSet<PairKey>,
    empty: &'a BTreeSet<PairKey>,
}

/// Split every attempted pair into what it measured and why it did not.
fn pair_census(threads: &[&ThreadRow], probes: &[&ProbeRow], voided: &Voided) -> PairCensus {
    let mut attempted: BTreeSet<PairKey> = threads
        .iter()
        .map(|t| pair_key(&t.run_id, t.repeat, &t.task))
        .collect();
    let probed: BTreeSet<PairKey> = probes
        .iter()
        .map(|p| pair_key(&p.run_id, p.repeat, &p.task))
        .collect();
    attempted.extend(probed.iter().cloned());
    // Read off the probe rows, because a task voided before it ran leaves no
    // thread row at all.
    let mut scored: BTreeMap<PairKey, BTreeSet<Arm>> = BTreeMap::new();
    for probe in probes.iter().filter(|p| p.outcome.is_scored()) {
        scored
            .entry(pair_key(&probe.run_id, probe.repeat, &probe.task))
            .or_default()
            .insert(probe.arm);
    }
    // How many arms a pair needs before it counts as measured, PER RUN. A run
    // of one configuration wants one, and asking for two would report every
    // pair as an upstream failure.
    //
    // Per run rather than pooled, because a sweep pools several single-arm runs
    // and a baseline is another one beside them. Pooled, one two-arm run in the
    // set would raise the bar for every single-arm run and write off all of it.
    let mut wanted: BTreeMap<&str, BTreeSet<Arm>> = BTreeMap::new();
    for thread in threads {
        wanted
            .entry(thread.run_id.as_str())
            .or_default()
            .insert(thread.arm);
    }
    let arms_wanted =
        |run_id: &str| -> usize { wanted.get(run_id).map_or(1, |arms| arms.len().max(1)) };
    let upstream_failure = probed
        .iter()
        .filter(|pair| !voided.all.contains(*pair))
        .filter(|pair| {
            scored
                .get(*pair)
                .is_none_or(|arms| arms.len() < arms_wanted(&pair.0))
        })
        .count();
    let empty_completion = voided.empty.len();
    let classifier_disagreement = voided.classifier.difference(voided.empty).count();
    PairCensus {
        effective: attempted
            .len()
            .saturating_sub(voided.all.len())
            .saturating_sub(upstream_failure),
        attempted: attempted.len(),
        empty_completion,
        classifier_disagreement,
        completion_divergence: voided
            .all
            .len()
            .saturating_sub(empty_completion)
            .saturating_sub(classifier_disagreement),
        upstream_failure,
    }
}

fn divide(total: f64, by: i64) -> f64 {
    match by {
        0 => 0.0,
        by => total / by as f64,
    }
}

fn mean(values: &[f64]) -> f64 {
    match values.len() {
        0 => 0.0,
        n => values.iter().sum::<f64>() / n as f64,
    }
}

pub fn median(values: &[f64]) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let middle = sorted.len() / 2;
    if sorted.len().is_multiple_of(2) {
        (sorted[middle - 1] + sorted[middle]) / 2.0
    } else {
        sorted[middle]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};

    fn run_row(run_id: &str, window: i64, arms: Vec<Arm>) -> ResultRow {
        ResultRow::Run(RunRow {
            run_id: run_id.into(),
            started: Utc.timestamp_opt(1_767_225_600, 0).unwrap(),
            engine_commit: "deadbeef".into(),
            model: "model-under-test".into(),
            fixture_hash: "f1".into(),
            guidance_hash: "g1".into(),
            prices_hash: "p1".into(),
            read_by_id: true,
            config: RunConfig::Smoke,
            repeats: 1,
            tasks: vec!["T01".into(), "T02".into()],
            arms,
            context_window: window,
            run_label: "test-model".into(),
            expire_after_rounds: 5,
            sweep_every_rounds: 10,
        })
    }

    /// One thread, with the handful of fields the axes read.
    struct Thread {
        run_id: &'static str,
        arm: Arm,
        task: &'static str,
        rounds: i64,
        usd: f64,
        peak: i64,
        window: i64,
        trimmed: i64,
    }

    fn thread_row(spec: Thread) -> ResultRow {
        ResultRow::Thread(ThreadRow {
            run_id: spec.run_id.into(),
            repeat: 1,
            arm: spec.arm,
            task: spec.task.into(),
            thread_id: uuid::Uuid::from_u128(spec.task.len() as u128),
            rounds: spec.rounds,
            cache_creation: 10,
            cache_read: 20,
            input_total: 1_000,
            output_tokens: 100,
            auxiliary_tokens: TokenCounts::default(),
            event_log_settled: Some(true),
            todo_writes: 0,
            document_writes: 0,
            writes_with_a_tool_call: 0,
            items_held_open: 0,
            recovery_calls: [("read_file".to_string(), 2)].into_iter().collect(),
            wall_secs: 30,
            usd: spec.usd,
            usd_auxiliary: 0.0,
            status: "idle".into(),
            unscripted_answers: 0,
            started: Some(Utc.timestamp_opt(1_767_225_600, 0).unwrap()),
            handover_lines: None,
            memory_recalled: true,
            repeat_recoveries: 1,
            trimmed_rounds: spec.trimmed,
            peak_request_tokens: spec.peak,
            mean_request_tokens: spec.peak / 2,
            context_window: spec.window,
            empty_completions: 0,
            empty_retries: 0,
            followup_sequence: None,
        })
    }

    /// The same thread, unwrapped, for the axis functions that take rows.
    fn thread(spec: Thread) -> ThreadRow {
        match thread_row(spec) {
            ResultRow::Thread(row) => row,
            other => panic!("expected a thread row and got {other:?}"),
        }
    }

    fn one_thread(usd: f64) -> Thread {
        Thread {
            run_id: "r1",
            arm: Arm::Lean,
            task: "T01",
            rounds: 4,
            usd,
            peak: 0,
            window: 200_000,
            trimmed: 0,
        }
    }

    /// A free model under test does not become measured through its title work.
    ///
    /// `stealth/ox-alpha` is priced at zero, so its cost axis measures nothing.
    /// Its title and memory work still bills a priced auxiliary model, and that
    /// spend must not be read as a cost measurement.
    #[test]
    fn auxiliary_spend_alone_is_not_a_measured_cost_axis() {
        let mut row = thread(one_thread(0.4));
        row.usd_auxiliary = 0.4;
        row.auxiliary_tokens = TokenCounts {
            cache_creation: 0,
            cache_read: 0,
            input_total: 900,
            output_tokens: 60,
        };

        let cost = cost(&[&row]);
        assert!(
            !cost.measured,
            "every main-agent dollar here is a real zero"
        );
        assert_eq!(cost.usd, 0.4, "the arm still caused the spend");
        assert_eq!(cost.usd_auxiliary, 0.4);
        assert_eq!(cost.auxiliary_tokens.input_total, 900);
    }

    /// A priced model under test stays measured, and the split still reports.
    #[test]
    fn a_priced_main_model_reports_measured_with_the_split_beside_it() {
        let mut row = thread(one_thread(2.5));
        row.usd_auxiliary = 0.02;

        let cost = cost(&[&row]);
        assert!(cost.measured);
        assert_eq!(cost.usd, 2.5, "the headline is main plus auxiliary");
        assert_eq!(cost.usd_auxiliary, 0.02);
        assert_eq!(cost.usd_per_round, 0.625, "per round counts every dollar");
    }

    /// Vary only the three document fields, so nothing else can move the axis.
    fn document_thread(writes: i64, beside: i64, held: i64) -> ThreadRow {
        let mut row = thread(one_thread(1.0));
        row.document_writes = writes;
        row.writes_with_a_tool_call = beside;
        row.items_held_open = held;
        row
    }

    /// The three fields were collected per thread and aggregated nowhere, so
    /// every run reported them as nothing at all.
    #[test]
    fn the_document_axis_totals_the_three_fields_it_is_given() {
        let rows = [document_thread(3, 2, 5), document_thread(1, 1, 0)];
        let borrowed: Vec<&ThreadRow> = rows.iter().collect();

        let document = document(&borrowed);
        assert_eq!(document.writes, 4);
        assert_eq!(document.writes_per_task, 2.0);
        assert_eq!(document.beside_a_call.passed, 3);
        assert_eq!(
            document.beside_a_call.scored, 4,
            "scored against the writes"
        );
        assert_eq!(document.beside_a_call.rate, 0.75);
        assert_eq!(document.items_held_open, 5);
        assert_eq!(document.items_held_open_per_task, 2.5);
    }

    /// A configuration whose model never wrote a document reports a real zero,
    /// not a divide-by-zero.
    #[test]
    fn a_configuration_that_wrote_no_document_reports_zero() {
        let row = document_thread(0, 0, 0);

        let document = document(&[&row]);
        assert_eq!(document.writes, 0);
        assert_eq!(document.beside_a_call.rate, 0.0);
        assert_eq!(document.beside_a_call.scored, 0);
    }

    /// The axis has to reach the comparison, which is where two arms are read
    /// against each other. That is the whole reason the fields were collected.
    ///
    /// Only the three document fields vary between the arms here. Control
    /// writes twice and never beside a call; lean writes once and always
    /// beside one.
    #[test]
    fn the_comparison_carries_the_document_deltas() {
        let mut rows = vec![run_row("r1", 200_000, vec![Arm::Control, Arm::Lean])];
        let mut at = 1_767_225_600;
        for task in ["T01", "T02"] {
            for arm in [Arm::Control, Arm::Lean] {
                let mut thread = thread_row(Thread {
                    run_id: "r1",
                    arm,
                    task,
                    rounds: 5,
                    usd: 3.0,
                    peak: 100_000,
                    window: 200_000,
                    trimmed: 0,
                });
                if let ResultRow::Thread(row) = &mut thread {
                    row.started = Some(Utc.timestamp_opt(at, 0).unwrap());
                    row.thread_id = uuid::Uuid::from_u128(at as u128);
                    let (writes, beside, held) = match arm {
                        Arm::Lean => (1, 1, 3),
                        _ => (2, 0, 0),
                    };
                    row.document_writes = writes;
                    row.writes_with_a_tool_call = beside;
                    row.items_held_open = held;
                }
                at += 60;
                rows.push(thread);
                rows.push(completion_row("r1", arm, task, CompletionOutcome::Pass));
                rows.push(probe_row("r1", arm, task, "P.1", Outcome::Pass));
            }
        }

        let analysis = analyse(&rows).unwrap();
        let axis = |arm: Arm| {
            analysis
                .configurations
                .iter()
                .find(|result| result.configuration.arm == arm)
                .map(|result| result.document.clone())
                .unwrap_or_else(|| panic!("no {arm:?} configuration"))
        };
        assert_eq!(axis(Arm::Control).writes, 4);
        assert_eq!(axis(Arm::Control).beside_a_call.rate, 0.0);
        assert_eq!(axis(Arm::Lean).writes, 2);
        assert_eq!(axis(Arm::Lean).beside_a_call.rate, 1.0);
        assert_eq!(axis(Arm::Lean).items_held_open, 6);

        let comparison = &analysis.comparisons[0];
        // A delta is right minus left, and which arm lands on which side is
        // the enum's ordering rather than this test's business.
        let toward_lean = match comparison.right.arm {
            Arm::Lean => 1,
            _ => -1,
        };
        assert_eq!(comparison.document_writes, -2 * toward_lean);
        assert_eq!(comparison.items_held_open, 6 * toward_lean);
        assert_eq!(comparison.beside_a_call_points, 100.0 * toward_lean as f64);
    }

    fn probe_row(run_id: &str, arm: Arm, task: &str, probe: &str, outcome: Outcome) -> ResultRow {
        ResultRow::Probe(ProbeRow {
            run_id: run_id.into(),
            repeat: 1,
            arm,
            task: task.into(),
            probe: probe.into(),
            fact: Some("F01".into()),
            tier: Some(1),
            outcome,
        })
    }

    fn completion_row(run_id: &str, arm: Arm, task: &str, outcome: CompletionOutcome) -> ResultRow {
        ResultRow::Completion(CompletionRow {
            run_id: run_id.into(),
            repeat: 1,
            arm,
            task: task.into(),
            probe: format!("C{}", &task[1..]),
            outcome,
        })
    }

    /// One arm, one window: the shape ADR 0110 makes the default.
    fn one_configuration() -> Vec<ResultRow> {
        vec![
            run_row("r1", 200_000, vec![Arm::Lean]),
            thread_row(Thread {
                run_id: "r1",
                arm: Arm::Lean,
                task: "T01",
                rounds: 4,
                usd: 2.0,
                peak: 120_000,
                window: 200_000,
                trimmed: 0,
            }),
            thread_row(Thread {
                run_id: "r1",
                arm: Arm::Lean,
                task: "T02",
                rounds: 8,
                usd: 6.0,
                peak: 180_000,
                window: 200_000,
                trimmed: 3,
            }),
            probe_row("r1", Arm::Lean, "T02", "P02.1", Outcome::Pass),
            probe_row("r1", Arm::Lean, "T02", "P02.2", Outcome::LostSilent),
            completion_row("r1", Arm::Lean, "T01", CompletionOutcome::Pass),
            completion_row("r1", Arm::Lean, "T02", CompletionOutcome::Pass),
        ]
    }

    /// The invariant the whole reshaping rests on: one arm is a full report.
    #[test]
    fn one_arm_produces_every_axis_and_no_comparison() {
        let analysis = analyse(&one_configuration()).unwrap();
        assert_eq!(analysis.configurations.len(), 1);
        assert!(analysis.comparisons.is_empty());
        assert!(analysis.sweeps.is_empty());

        let result = &analysis.configurations[0];
        assert_eq!(result.configuration.arm, Arm::Lean);
        assert_eq!(result.configuration.context_window, 200_000);
        assert_eq!(result.quality.delivery, Rate::of(2, 2));
        assert_eq!(result.quality.fidelity, Rate::of(1, 2));
        assert!(result.cost.measured);
        assert_eq!(result.cost.usd, 8.0);
        assert_eq!(result.rounds.total, 12);
        assert_eq!(result.rounds.recovery_calls, 4);
        assert_eq!(result.timing.total_secs, 60);
        assert_eq!(result.utilisation.peak_tokens, 180_000);
        assert_eq!(result.utilisation.headroom_at_peak, 20_000);
        assert_eq!(result.utilisation.trimmed_rounds, 3);
        assert_eq!(result.tasks.len(), 2);
    }

    /// A single-configuration run cannot void a pair, so nothing is removed and
    /// the effective count is the attempted one.
    #[test]
    fn one_arm_voids_no_pair() {
        let analysis = analyse(&one_configuration()).unwrap();
        assert_eq!(analysis.pairs.attempted, 2);
        assert_eq!(analysis.pairs.effective, 2);
        assert_eq!(analysis.pairs.classifier_disagreement, 0);
        assert_eq!(analysis.pairs.upstream_failure, 0);
    }

    /// A failure is split three ways, and the split is what a reader acts on.
    #[test]
    fn the_failure_split_names_how_each_probe_failed() {
        let mut rows = one_configuration();
        rows.push(probe_row("r1", Arm::Lean, "T02", "P02.3", Outcome::Asked));
        rows.push(probe_row(
            "r1",
            Arm::Lean,
            "T02",
            "P02.4",
            Outcome::LostLoud,
        ));
        let analysis = analyse(&rows).unwrap();
        let failures = &analysis.configurations[0].quality.failures;
        assert_eq!(failures.get("lost-silent"), Some(&1));
        assert_eq!(failures.get("asked"), Some(&1));
        assert_eq!(failures.get("lost-loud"), Some(&1));
    }

    /// Two runs of one arm at two windows pool into one sweep with two rows.
    fn two_windows() -> Vec<ResultRow> {
        let mut rows = one_configuration();
        rows.extend([
            run_row("r2", 96_000, vec![Arm::Lean]),
            thread_row(Thread {
                run_id: "r2",
                arm: Arm::Lean,
                task: "T01",
                rounds: 5,
                usd: 2.5,
                peak: 90_000,
                window: 96_000,
                trimmed: 1,
            }),
            thread_row(Thread {
                run_id: "r2",
                arm: Arm::Lean,
                task: "T02",
                rounds: 11,
                usd: 7.0,
                peak: 94_000,
                window: 96_000,
                trimmed: 6,
            }),
            probe_row("r2", Arm::Lean, "T02", "P02.1", Outcome::Pass),
            probe_row("r2", Arm::Lean, "T02", "P02.2", Outcome::LostSilent),
            completion_row("r2", Arm::Lean, "T01", CompletionOutcome::Pass),
            completion_row("r2", Arm::Lean, "T02", CompletionOutcome::Pass),
        ]);
        rows
    }

    #[test]
    fn two_windows_pool_into_one_sweep_of_two_rows() {
        let analysis = analyse(&two_windows()).unwrap();
        assert_eq!(analysis.configurations.len(), 2);
        assert_eq!(analysis.sweeps.len(), 1);

        let sweep = &analysis.sweeps[0];
        assert_eq!(sweep.reference_window, 200_000);
        let windows: Vec<i64> = sweep.rows.iter().map(|row| row.context_window).collect();
        assert_eq!(windows, vec![200_000, 96_000]);
        // Both windows scored the same, so the smaller one held and it is the
        // answer the sweep exists to produce.
        assert_eq!(sweep.smallest_holding, Some(96_000));
    }

    /// The tolerance is what decides a row, so a real drop has to fail it.
    #[test]
    fn a_budget_that_loses_quality_does_not_hold() {
        let mut rows = two_windows();
        for row in rows.iter_mut() {
            if let ResultRow::Completion(done) = row {
                if done.run_id == "r2" {
                    done.outcome = CompletionOutcome::Fail;
                }
            }
        }
        let analysis = analyse(&rows).unwrap();
        let sweep = &analysis.sweeps[0];
        assert!(!sweep.rows[1].holds);
        assert_eq!(sweep.smallest_holding, Some(200_000));
    }

    /// Two arms at one window produce a comparison and no sweep.
    ///
    /// The threads are built task by task, arm within task, and stamped in that
    /// order. That is what I7 asks for, and building them arm by arm makes
    /// `check_arms_interleave` refuse the whole analysis.
    #[test]
    fn two_arms_at_one_window_are_compared_and_not_swept() {
        let mut rows = vec![run_row("r1", 200_000, vec![Arm::Control, Arm::Lean])];
        let mut at = 1_767_225_600;
        for task in ["T01", "T02"] {
            for arm in [Arm::Control, Arm::Lean] {
                let mut thread = thread_row(Thread {
                    run_id: "r1",
                    arm,
                    task,
                    rounds: 5,
                    usd: 3.0,
                    peak: 100_000,
                    window: 200_000,
                    trimmed: 0,
                });
                if let ResultRow::Thread(row) = &mut thread {
                    row.started = Some(Utc.timestamp_opt(at, 0).unwrap());
                    row.thread_id = uuid::Uuid::from_u128(at as u128);
                }
                at += 60;
                rows.push(thread);
                rows.push(completion_row("r1", arm, task, CompletionOutcome::Pass));
                rows.push(probe_row("r1", arm, task, "P.1", Outcome::Pass));
            }
        }
        let analysis = analyse(&rows).unwrap();
        assert_eq!(analysis.configurations.len(), 2);
        assert!(analysis.sweeps.is_empty());
        assert_eq!(analysis.comparisons.len(), 1);
        assert_eq!(analysis.comparisons[0].usd, 0.0);
    }

    /// The window comes off the thread rows when they disagree with the run.
    ///
    /// The run row records what was asked for, and a seed that silently failed
    /// would leave the two apart. What the engine resolved is the truth.
    #[test]
    fn a_thread_that_resolved_another_window_wins() {
        let mut rows = one_configuration();
        if let Some(ResultRow::Run(run)) = rows.first_mut() {
            run.context_window = 0;
        }
        let analysis = analyse(&rows).unwrap();
        assert_eq!(
            analysis.configurations[0].configuration.context_window,
            200_000
        );
    }

    /// I6, from the failing direction.
    #[test]
    fn two_fixtures_cannot_be_pooled() {
        let mut rows = two_windows();
        if let Some(ResultRow::Run(run)) = rows.iter_mut().find_map(|row| match row {
            ResultRow::Run(run) if run.run_id == "r2" => Some(ResultRow::Run(run.clone())),
            _ => None,
        }) {
            let mut changed = run.clone();
            changed.fixture_hash = "f2".into();
            for row in rows.iter_mut() {
                if let ResultRow::Run(existing) = row {
                    if existing.run_id == "r2" {
                        *existing = changed.clone();
                    }
                }
            }
        }
        let error = analyse(&rows).unwrap_err().to_string();
        assert!(error.contains("pooled_runs_disagree"), "{error}");
        assert!(error.contains("fixture_hash differs"), "{error}");
    }

    #[test]
    fn rows_with_no_run_row_are_refused() {
        let error = analyse(&[]).unwrap_err().to_string();
        assert!(error.contains("no run row"), "{error}");
    }

    /// A reference that scored nothing is not a bar every smaller window
    /// clears. Its rates are both zero, so without the guard the tightest
    /// budget in a sweep would read as holding on a broken reference run.
    #[test]
    fn a_reference_that_scored_nothing_holds_nobody() {
        let mut rows = two_windows();
        // Strip the reference run's probes and completions, leaving its threads.
        rows.retain(|row| match row {
            ResultRow::Probe(probe) => probe.run_id != "r1",
            ResultRow::Completion(done) => done.run_id != "r1",
            _ => true,
        });
        let analysis = analyse(&rows).unwrap();
        let sweep = &analysis.sweeps[0];
        assert_eq!(sweep.reference_window, 200_000);
        assert!(sweep.rows.iter().all(|row| !row.holds));
        assert_eq!(sweep.smallest_holding, None);
    }

    /// A sweep of single-arm runs pooled beside a two-arm baseline.
    ///
    /// The arms a pair needs is per run. Pooled, one two-arm run would raise
    /// the bar for every single-arm run beside it. Every one of their pairs
    /// would then read as an upstream failure, which looks like a broken sweep.
    #[test]
    fn a_single_arm_run_is_not_failed_by_a_two_arm_run_beside_it() {
        let mut rows = one_configuration();
        rows.push(run_row("r2", 200_000, vec![Arm::Control, Arm::Lean]));
        let mut at = 1_767_312_000;
        for task in ["T01", "T02"] {
            for arm in [Arm::Control, Arm::Lean] {
                let mut thread = thread_row(Thread {
                    run_id: "r2",
                    arm,
                    task,
                    rounds: 5,
                    usd: 3.0,
                    peak: 100_000,
                    window: 200_000,
                    trimmed: 0,
                });
                if let ResultRow::Thread(row) = &mut thread {
                    row.started = Some(Utc.timestamp_opt(at, 0).unwrap());
                    row.thread_id = uuid::Uuid::from_u128(at as u128);
                }
                at += 60;
                rows.push(thread);
                rows.push(completion_row("r2", arm, task, CompletionOutcome::Pass));
                rows.push(probe_row("r2", arm, task, "P.1", Outcome::Pass));
            }
        }
        let analysis = analyse(&rows).unwrap();
        // r1 scores one probed task and r2 scores two, so three pairs measured
        // something. Nothing here is an upstream failure.
        assert_eq!(analysis.pairs.upstream_failure, 0);
        assert_eq!(analysis.pairs.effective, analysis.pairs.attempted);
    }

    /// A re-post that then timed out recovered nothing.
    ///
    /// Recovery reads `finished()`, never "the status is not empty-completion".
    /// Reading the status alone counts that thread as rescued and drops the
    /// warning line with it.
    #[test]
    fn a_thread_whose_retry_timed_out_never_recovered() {
        let empty = |status: &str| ThreadRow {
            status: status.into(),
            empty_completions: 1,
            empty_retries: 2,
            ..match thread_row(Thread {
                run_id: "r1",
                arm: Arm::Lean,
                task: "T01",
                rounds: 1,
                usd: 0.0,
                peak: 0,
                window: 200_000,
                trimmed: 0,
            }) {
                ResultRow::Thread(thread) => thread,
                _ => unreachable!("thread_row builds a thread row"),
            }
        };
        let timed_out = empty("timeout");
        let recovered = empty("idle");
        let summary = empty_completions(&[&timed_out, &recovered]);
        assert_eq!(summary.recovered_threads, 1);
        assert_eq!(summary.unrecovered_threads, 1);
        assert_eq!(summary.turns, 2);
    }

    /// One empty arm voids the pair, whichever arm it is, and even when the
    /// other arm never ran. A disagreement needs two opinions; this needs one
    /// thread that never started the task.
    #[test]
    fn one_empty_arm_voids_the_pair_whichever_arm_it_is() {
        for arm in Arm::BOTH {
            let by_arm: BTreeMap<Arm, bool> = [(arm, true)].into_iter().collect();
            assert!(empty_completion_voided(&by_arm), "{arm} alone must void it");
        }
        let neither: BTreeMap<Arm, bool> = Arm::BOTH.into_iter().map(|arm| (arm, false)).collect();
        assert!(!empty_completion_voided(&neither));
        assert!(!empty_completion_voided(&BTreeMap::new()));
    }

    /// Retrieval in one arm and not the other voids that pair. Both arms
    /// skipping is agreement and is kept, because the mode genuinely saves
    /// less on such a turn.
    #[test]
    fn arms_that_disagree_on_retrieval_void_that_pair() {
        let pair = |control: Option<bool>, lean: Option<bool>| -> BTreeMap<Arm, bool> {
            [(Arm::Control, control), (Arm::Lean, lean)]
                .into_iter()
                .filter_map(|(arm, value)| value.map(|value| (arm, value)))
                .collect()
        };
        assert!(retrieval_disagreed(&pair(Some(true), Some(false))));
        assert!(retrieval_disagreed(&pair(Some(false), Some(true))));
        assert!(!retrieval_disagreed(&pair(Some(true), Some(true))));
        assert!(!retrieval_disagreed(&pair(Some(false), Some(false))));
        // One arm cannot disagree with nothing.
        assert!(!retrieval_disagreed(&pair(Some(true), None)));
    }

    /// A task one arm delivered and the other did not is a divergence. A void
    /// is not: the arms did not both answer, so there is nothing to disagree
    /// about.
    #[test]
    fn a_task_one_arm_delivered_and_the_other_did_not_is_a_divergence() {
        let pair = |control, lean| -> BTreeMap<Arm, CompletionOutcome> {
            [(Arm::Control, control), (Arm::Lean, lean)]
                .into_iter()
                .collect()
        };
        assert!(completion_diverged(&pair(
            CompletionOutcome::Pass,
            CompletionOutcome::Fail
        )));
        assert!(!completion_diverged(&pair(
            CompletionOutcome::Pass,
            CompletionOutcome::Pass
        )));
        assert!(!completion_diverged(&pair(
            CompletionOutcome::Pass,
            CompletionOutcome::Void
        )));
    }

    /// I7, from the failing direction.
    ///
    /// One arm running the whole sequence and then the other puts a task's two
    /// threads far apart in time. Provider drift then lands on one arm alone.
    #[test]
    fn one_arm_running_the_whole_sequence_first_fails_the_interleave_check() {
        let mut rows = vec![run_row("r1", 200_000, vec![Arm::Control, Arm::Lean])];
        let mut at = 1_767_225_600;
        for arm in [Arm::Control, Arm::Lean] {
            for task in ["T01", "T02"] {
                let mut thread = thread_row(Thread {
                    run_id: "r1",
                    arm,
                    task,
                    rounds: 5,
                    usd: 1.0,
                    peak: 100_000,
                    window: 200_000,
                    trimmed: 0,
                });
                if let ResultRow::Thread(row) = &mut thread {
                    row.started = Some(Utc.timestamp_opt(at, 0).unwrap());
                    row.thread_id = uuid::Uuid::from_u128(at as u128);
                }
                at += 60;
                rows.push(thread);
            }
        }
        let error = analyse(&rows).unwrap_err().to_string();
        assert!(error.contains("arms_not_interleaved"), "{error}");
    }

    /// Two arms from separate runs are compared and told apart from two that
    /// interleaved. The interleave check cannot see this: each run has one arm
    /// per task, so nothing is out of order to find.
    #[test]
    fn two_arms_from_separate_runs_are_flagged_as_not_interleaved() {
        let mut rows = one_configuration();
        rows.push(run_row("r2", 200_000, vec![Arm::Control]));
        for task in ["T01", "T02"] {
            rows.push(thread_row(Thread {
                run_id: "r2",
                arm: Arm::Control,
                task,
                rounds: 5,
                usd: 3.0,
                peak: 100_000,
                window: 200_000,
                trimmed: 0,
            }));
            rows.push(completion_row(
                "r2",
                Arm::Control,
                task,
                CompletionOutcome::Pass,
            ));
            rows.push(probe_row("r2", Arm::Control, task, "P.1", Outcome::Pass));
        }
        let analysis = analyse(&rows).unwrap();
        assert_eq!(analysis.comparisons.len(), 1);
        assert!(!analysis.comparisons[0].interleaved);
    }

    /// A sweep whose model changed halfway is a model comparison wearing a
    /// budget curve's clothes, so pooling refuses it.
    #[test]
    fn a_sweep_across_two_models_cannot_be_pooled() {
        let mut rows = two_windows();
        for row in rows.iter_mut() {
            if let ResultRow::Run(run) = row {
                if run.run_id == "r2" {
                    run.model = "another-model".into();
                }
            }
        }
        let error = analyse(&rows).unwrap_err().to_string();
        assert!(error.contains("pooled_runs_disagree"), "{error}");
        assert!(error.contains("model differs"), "{error}");
    }

    /// A budget curve swept at two schedules is two designs on one axis, and
    /// the report would read the schedule off whichever run it printed.
    #[test]
    fn a_sweep_across_two_schedules_cannot_be_pooled() {
        for (label, moved) in [("expire_after_rounds", true), ("sweep_every_rounds", false)] {
            let mut rows = two_windows();
            for row in rows.iter_mut() {
                if let ResultRow::Run(run) = row {
                    if run.run_id == "r2" && moved {
                        run.expire_after_rounds = 3;
                    } else if run.run_id == "r2" {
                        run.sweep_every_rounds = 8;
                    }
                }
            }
            let error = analyse(&rows).unwrap_err().to_string();
            assert!(error.contains("pooled_runs_disagree"), "{error}");
            assert!(error.contains(&format!("{label} differs")), "{error}");
        }
    }

    /// The floor exists because the engine's budget saturates to zero below it.
    #[test]
    fn the_minimum_window_leaves_room_for_messages() {
        const RESERVE: i64 = 8_000;
        const FIXED_OVERHEAD_CHARS: i64 = 85_000;
        let budget_chars = (MIN_CONTEXT_WINDOW - RESERVE) * 3 / 2;
        assert!(
            budget_chars > FIXED_OVERHEAD_CHARS,
            "a {MIN_CONTEXT_WINDOW}-token window gives {budget_chars} chars, and the tools \
             array plus the system prompt already take about {FIXED_OVERHEAD_CHARS}"
        );
    }
}
