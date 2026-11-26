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

pub const CENSUS_TASK: &str = "T12";

pub const QUALITY_TOLERANCE: f64 = 0.05;

pub const MIN_CONTEXT_WINDOW: i64 = 72_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Configuration {
    pub arm: Arm,
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

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Quality {
    pub delivery: Rate,
    pub fidelity: Rate,
    pub handover: Rate,
    pub failures: BTreeMap<String, usize>,
    pub by_tier: BTreeMap<u8, Rate>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Cost {
    pub measured: bool,
    pub usd: f64,
    pub usd_auxiliary: f64,
    pub usd_per_task: f64,
    pub usd_per_round: f64,
    pub fresh_input: i64,
    pub output_tokens: i64,
    pub cache_read: i64,
    pub cache_creation: i64,
    pub auxiliary_tokens: TokenCounts,
    pub fresh_input_per_round: f64,
    pub cache_read_per_round: f64,
    pub cache_creation_per_round: f64,
}

impl Cost {
    pub fn usd_main_agent(&self) -> f64 {
        self.usd - self.usd_auxiliary
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Rounds {
    pub total: i64,
    pub per_task_median: f64,
    pub per_task_max: i64,
    pub recovery_calls: i64,
    pub repeat_recoveries: i64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Timing {
    pub total_secs: i64,
    pub per_task_median: f64,
    pub per_task_max: i64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Utilisation {
    pub context_window: i64,
    pub peak_tokens: i64,
    pub mean_tokens: f64,
    pub headroom_at_peak: i64,
    pub peak_share: f64,
    pub trimmed_rounds: i64,
    pub trimmed_threads: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Document {
    pub writes: i64,
    pub writes_per_task: f64,
    pub beside_a_call: Rate,
    pub items_held_open: i64,
    pub items_held_open_per_task: f64,
}

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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConfigurationResult {
    pub configuration: Configuration,
    pub repeats: usize,
    pub threads: usize,
    pub quality: Quality,
    pub cost: Cost,
    pub rounds: Rounds,
    pub timing: Timing,
    pub utilisation: Utilisation,
    pub document: Document,
    pub tasks: Vec<TaskRow>,
}

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
    pub holds: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Sweep {
    pub arm: Arm,
    pub rows: Vec<SweepRow>,
    pub reference_window: i64,
    pub smallest_holding: Option<i64>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PairCensus {
    pub attempted: usize,
    pub empty_completion: usize,
    pub classifier_disagreement: usize,
    pub completion_divergence: usize,
    pub upstream_failure: usize,
    pub effective: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RepostTally {
    pub turns: i64,
    pub retries: i64,
    pub recovered_threads: usize,
    pub unrecovered_threads: usize,
}

impl RepostTally {
    fn add(&mut self, thread: &ThreadRow, turns: i64, retries: i64) {
        self.turns += turns;
        self.retries += retries;
        if turns == 0 {
            return;
        }
        match thread.finished() {
            true => self.recovered_threads += 1,
            false => self.unrecovered_threads += 1,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ArmRefusals {
    pub arm: Arm,
    pub total: RepostTally,
    pub by_task: Vec<TaskRefusals>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TaskRefusals {
    pub task: String,
    pub tally: RepostTally,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Comparison {
    pub left: Configuration,
    pub right: Configuration,
    pub delivery_points: f64,
    pub fidelity_points: f64,
    pub usd: f64,
    pub rounds: i64,
    pub peak_tokens: i64,
    pub document_writes: i64,
    pub items_held_open: i64,
    pub beside_a_call_points: f64,
    pub interleaved: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Analysis {
    pub config: RunConfig,
    pub configurations: Vec<ConfigurationResult>,
    pub sweeps: Vec<Sweep>,
    pub comparisons: Vec<Comparison>,
    pub pairs: PairCensus,
    pub empty_completions: RepostTally,
    #[serde(default)]
    pub refusals: Vec<ArmRefusals>,
    pub run_labels: Vec<String>,
}

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
        refusals: refusals(&threads, &task_order),
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

fn ordered_tasks(threads: &[&ThreadRow], task_order: &[String]) -> Vec<String> {
    let mut ordered: Vec<String> = task_order.to_vec();
    for thread in threads {
        if !ordered.contains(&thread.task) {
            ordered.push(thread.task.clone());
        }
    }
    ordered
}

fn task_rows(
    threads: &[&ThreadRow],
    probes: &[&ProbeRow],
    completions: &[&CompletionRow],
    task_order: &[String],
) -> Vec<TaskRow> {
    ordered_tasks(threads, task_order)
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

fn holds_against(result: &ConfigurationResult, reference: &ConfigurationResult) -> bool {
    if reference.quality.delivery.scored == 0 || reference.quality.fidelity.scored == 0 {
        return false;
    }
    result.quality.delivery.rate >= reference.quality.delivery.rate - QUALITY_TOLERANCE
        && result.quality.fidelity.rate >= reference.quality.fidelity.rate - QUALITY_TOLERANCE
}

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

fn empty_completions(threads: &[&ThreadRow]) -> RepostTally {
    let mut summary = RepostTally::default();
    for thread in threads {
        summary.add(thread, thread.empty_completions, thread.empty_retries);
    }
    summary
}

fn refusals(threads: &[&ThreadRow], task_order: &[String]) -> Vec<ArmRefusals> {
    let mut by_arm: BTreeMap<Arm, (RepostTally, BTreeMap<&str, RepostTally>)> = BTreeMap::new();
    for thread in threads {
        let (total, by_task) = by_arm.entry(thread.arm).or_default();
        total.add(thread, thread.refusals, thread.refusal_retries);
        by_task.entry(thread.task.as_str()).or_default().add(
            thread,
            thread.refusals,
            thread.refusal_retries,
        );
    }
    let ordered = ordered_tasks(threads, task_order);
    by_arm
        .into_iter()
        .map(|(arm, (total, by_task))| ArmRefusals {
            arm,
            total,
            by_task: ordered
                .iter()
                .filter_map(|task| {
                    let tally = by_task.get(task.as_str())?;
                    (tally.turns > 0).then(|| TaskRefusals {
                        task: task.clone(),
                        tally: tally.clone(),
                    })
                })
                .collect(),
        })
        .collect()
}

pub fn check_one_fixture(runs: &[&RunRow]) -> Fallible<()> {
    let differences: Vec<String> = [
        ("fixture_hash", pinned(runs, |run| run.fixture_hash.clone())),
        (
            "guidance_hash",
            pinned(runs, |run| run.guidance_hash.clone()),
        ),
        ("prices_hash", pinned(runs, |run| run.prices_hash.clone())),
        ("model", pinned(runs, |run| run.model.clone())),
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

fn pinned(runs: &[&RunRow], field: fn(&RunRow) -> String) -> Vec<String> {
    runs.iter()
        .map(|run| field(run))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

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

pub type PairKey = (String, u32, String);

pub fn pair_key(run_id: &str, repeat: u32, task: &str) -> PairKey {
    (run_id.to_string(), repeat, task.to_string())
}

pub fn retrieval_disagreed(by_arm: &BTreeMap<Arm, bool>) -> bool {
    by_arm.iter().any(|(arm, recalled)| {
        by_arm
            .iter()
            .any(|(other, theirs)| other.memory == arm.memory && theirs != recalled)
    })
}

pub fn empty_completion_voided(by_arm: &BTreeMap<Arm, bool>) -> bool {
    by_arm.values().any(|empty| *empty)
}

pub fn empty_completion_pairs(threads: &[&ThreadRow]) -> BTreeSet<PairKey> {
    threads
        .iter()
        .filter(|thread| thread.status == EMPTY_STATUS)
        .map(|thread| pair_key(&thread.run_id, thread.repeat, &thread.task))
        .collect()
}

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

pub fn completion_diverged(by_arm: &BTreeMap<Arm, CompletionOutcome>) -> bool {
    let mut scored = by_arm.values().filter(|outcome| outcome.is_scored());
    scored
        .next()
        .is_some_and(|first| scored.any(|other| other != first))
}

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

struct Voided<'a> {
    all: &'a BTreeSet<PairKey>,
    classifier: &'a BTreeSet<PairKey>,
    empty: &'a BTreeSet<PairKey>,
}

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
    let mut scored: BTreeMap<PairKey, BTreeSet<Arm>> = BTreeMap::new();
    for probe in probes.iter().filter(|p| p.outcome.is_scored()) {
        scored
            .entry(pair_key(&probe.run_id, probe.repeat, &probe.task))
            .or_default()
            .insert(probe.arm);
    }
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
            refusals: 0,
            refusal_retries: 0,
            followup_sequence: None,
            compaction_caught_up: None,
        })
    }

    fn thread(spec: Thread) -> ThreadRow {
        match thread_row(spec) {
            ResultRow::Thread(row) => row,
            other => panic!("expected a thread row and got {other:?}"),
        }
    }

    fn one_thread(usd: f64) -> Thread {
        Thread {
            run_id: "r1",
            arm: Arm::LEAN,
            task: "T01",
            rounds: 4,
            usd,
            peak: 0,
            window: 200_000,
            trimmed: 0,
        }
    }

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

    fn document_thread(writes: i64, beside: i64, held: i64) -> ThreadRow {
        let mut row = thread(one_thread(1.0));
        row.document_writes = writes;
        row.writes_with_a_tool_call = beside;
        row.items_held_open = held;
        row
    }

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

    #[test]
    fn a_configuration_that_wrote_no_document_reports_zero() {
        let row = document_thread(0, 0, 0);

        let document = document(&[&row]);
        assert_eq!(document.writes, 0);
        assert_eq!(document.beside_a_call.rate, 0.0);
        assert_eq!(document.beside_a_call.scored, 0);
    }

    #[test]
    fn the_comparison_carries_the_document_deltas() {
        let mut rows = vec![run_row("r1", 200_000, vec![Arm::CONTROL, Arm::LEAN])];
        let mut at = 1_767_225_600;
        for task in ["T01", "T02"] {
            for arm in [Arm::CONTROL, Arm::LEAN] {
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
                        Arm::LEAN => (1, 1, 3),
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
        assert_eq!(axis(Arm::CONTROL).writes, 4);
        assert_eq!(axis(Arm::CONTROL).beside_a_call.rate, 0.0);
        assert_eq!(axis(Arm::LEAN).writes, 2);
        assert_eq!(axis(Arm::LEAN).beside_a_call.rate, 1.0);
        assert_eq!(axis(Arm::LEAN).items_held_open, 6);

        let comparison = &analysis.comparisons[0];
        let toward_lean = match comparison.right.arm {
            Arm::LEAN => 1,
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

    fn one_configuration() -> Vec<ResultRow> {
        vec![
            run_row("r1", 200_000, vec![Arm::LEAN]),
            thread_row(Thread {
                run_id: "r1",
                arm: Arm::LEAN,
                task: "T01",
                rounds: 4,
                usd: 2.0,
                peak: 120_000,
                window: 200_000,
                trimmed: 0,
            }),
            thread_row(Thread {
                run_id: "r1",
                arm: Arm::LEAN,
                task: "T02",
                rounds: 8,
                usd: 6.0,
                peak: 180_000,
                window: 200_000,
                trimmed: 3,
            }),
            probe_row("r1", Arm::LEAN, "T02", "P02.1", Outcome::Pass),
            probe_row("r1", Arm::LEAN, "T02", "P02.2", Outcome::LostSilent),
            completion_row("r1", Arm::LEAN, "T01", CompletionOutcome::Pass),
            completion_row("r1", Arm::LEAN, "T02", CompletionOutcome::Pass),
        ]
    }

    #[test]
    fn one_arm_produces_every_axis_and_no_comparison() {
        let analysis = analyse(&one_configuration()).unwrap();
        assert_eq!(analysis.configurations.len(), 1);
        assert!(analysis.comparisons.is_empty());
        assert!(analysis.sweeps.is_empty());

        let result = &analysis.configurations[0];
        assert_eq!(result.configuration.arm, Arm::LEAN);
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

    #[test]
    fn one_arm_voids_no_pair() {
        let analysis = analyse(&one_configuration()).unwrap();
        assert_eq!(analysis.pairs.attempted, 2);
        assert_eq!(analysis.pairs.effective, 2);
        assert_eq!(analysis.pairs.classifier_disagreement, 0);
        assert_eq!(analysis.pairs.upstream_failure, 0);
    }

    #[test]
    fn the_failure_split_names_how_each_probe_failed() {
        let mut rows = one_configuration();
        rows.push(probe_row("r1", Arm::LEAN, "T02", "P02.3", Outcome::Asked));
        rows.push(probe_row(
            "r1",
            Arm::LEAN,
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

    fn two_windows() -> Vec<ResultRow> {
        let mut rows = one_configuration();
        rows.extend([
            run_row("r2", 96_000, vec![Arm::LEAN]),
            thread_row(Thread {
                run_id: "r2",
                arm: Arm::LEAN,
                task: "T01",
                rounds: 5,
                usd: 2.5,
                peak: 90_000,
                window: 96_000,
                trimmed: 1,
            }),
            thread_row(Thread {
                run_id: "r2",
                arm: Arm::LEAN,
                task: "T02",
                rounds: 11,
                usd: 7.0,
                peak: 94_000,
                window: 96_000,
                trimmed: 6,
            }),
            probe_row("r2", Arm::LEAN, "T02", "P02.1", Outcome::Pass),
            probe_row("r2", Arm::LEAN, "T02", "P02.2", Outcome::LostSilent),
            completion_row("r2", Arm::LEAN, "T01", CompletionOutcome::Pass),
            completion_row("r2", Arm::LEAN, "T02", CompletionOutcome::Pass),
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
        assert_eq!(sweep.smallest_holding, Some(96_000));
    }

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

    #[test]
    fn two_arms_at_one_window_are_compared_and_not_swept() {
        let mut rows = vec![run_row("r1", 200_000, vec![Arm::CONTROL, Arm::LEAN])];
        let mut at = 1_767_225_600;
        for task in ["T01", "T02"] {
            for arm in [Arm::CONTROL, Arm::LEAN] {
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

    #[test]
    fn a_reference_that_scored_nothing_holds_nobody() {
        let mut rows = two_windows();
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

    #[test]
    fn a_single_arm_run_is_not_failed_by_a_two_arm_run_beside_it() {
        let mut rows = one_configuration();
        rows.push(run_row("r2", 200_000, vec![Arm::CONTROL, Arm::LEAN]));
        let mut at = 1_767_312_000;
        for task in ["T01", "T02"] {
            for arm in [Arm::CONTROL, Arm::LEAN] {
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
        assert_eq!(analysis.pairs.upstream_failure, 0);
        assert_eq!(analysis.pairs.effective, analysis.pairs.attempted);
    }

    #[test]
    fn a_thread_whose_retry_timed_out_never_recovered() {
        let empty = |status: &str| ThreadRow {
            status: status.into(),
            empty_completions: 1,
            empty_retries: 2,
            ..match thread_row(Thread {
                run_id: "r1",
                arm: Arm::LEAN,
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

    fn refused(arm: Arm, task: &'static str, refusals: i64, status: &str) -> ThreadRow {
        ThreadRow {
            status: status.into(),
            refusals,
            refusal_retries: refusals.min(2),
            ..thread(Thread {
                arm,
                task,
                ..one_thread(0.0)
            })
        }
    }

    #[test]
    fn refusals_are_counted_per_arm_and_per_task_recovered_or_not() {
        let rows = [
            refused(Arm::LEAN, "T09", 1, "idle"),
            refused(Arm::LEAN, "T13", 3, "failed"),
            refused(Arm::LEAN, "T01", 0, "idle"),
            refused(Arm::LEAN_TREE, "T09", 0, "idle"),
        ];
        let threads: Vec<&ThreadRow> = rows.iter().collect();
        let order = ["T01", "T09", "T13"].map(String::from);
        let by_arm = refusals(&threads, &order);

        assert_eq!(by_arm.len(), 2, "every arm that ran gets an entry");
        let lean = by_arm.iter().find(|a| a.arm == Arm::LEAN).unwrap();
        assert_eq!(
            lean.total,
            RepostTally {
                turns: 4,
                retries: 3,
                recovered_threads: 1,
                unrecovered_threads: 1,
            }
        );
        let tasks: Vec<&str> = lean.by_task.iter().map(|t| t.task.as_str()).collect();
        assert_eq!(tasks, ["T09", "T13"], "only tasks with a refusal, in order");
        assert_eq!(lean.by_task[0].tally.recovered_threads, 1);
        assert_eq!(lean.by_task[1].tally.unrecovered_threads, 1);

        let tree = by_arm.iter().find(|a| a.arm == Arm::LEAN_TREE).unwrap();
        assert_eq!(tree.total, RepostTally::default());
        assert!(tree.by_task.is_empty());
    }

    #[test]
    fn a_recovered_refusal_reaches_the_analysis() {
        let mut rows = one_configuration();
        for row in rows.iter_mut() {
            if let ResultRow::Thread(thread) = row {
                if thread.task == "T02" {
                    thread.refusals = 1;
                    thread.refusal_retries = 1;
                }
            }
        }
        let analysis = analyse(&rows).unwrap();
        assert_eq!(analysis.refusals.len(), 1);
        let lean = &analysis.refusals[0];
        assert_eq!((lean.total.turns, lean.total.recovered_threads), (1, 1));
        assert_eq!(lean.by_task[0].task, "T02");
        assert_eq!(analysis.pairs.upstream_failure, 0);
    }

    #[test]
    fn a_refusal_is_never_counted_as_an_empty_completion() {
        let row = refused(Arm::LEAN, "T09", 1, "idle");
        assert_eq!(empty_completions(&[&row]), RepostTally::default());
    }

    #[test]
    fn one_empty_arm_voids_the_pair_whichever_arm_it_is() {
        for arm in Arm::ALL {
            let by_arm: BTreeMap<Arm, bool> = [(arm, true)].into_iter().collect();
            assert!(empty_completion_voided(&by_arm), "{arm} alone must void it");
        }
        let neither: BTreeMap<Arm, bool> = Arm::ALL.into_iter().map(|arm| (arm, false)).collect();
        assert!(!empty_completion_voided(&neither));
        assert!(!empty_completion_voided(&BTreeMap::new()));
    }

    #[test]
    fn arms_that_disagree_on_retrieval_void_that_pair() {
        let pair = |control: Option<bool>, lean: Option<bool>| -> BTreeMap<Arm, bool> {
            [(Arm::CONTROL, control), (Arm::LEAN, lean)]
                .into_iter()
                .filter_map(|(arm, value)| value.map(|value| (arm, value)))
                .collect()
        };
        assert!(retrieval_disagreed(&pair(Some(true), Some(false))));
        assert!(retrieval_disagreed(&pair(Some(false), Some(true))));
        assert!(!retrieval_disagreed(&pair(Some(true), Some(true))));
        assert!(!retrieval_disagreed(&pair(Some(false), Some(false))));
        assert!(!retrieval_disagreed(&pair(Some(true), None)));
    }

    #[test]
    fn retrieval_is_compared_within_one_memory_module_only() {
        let classic_and_tree: BTreeMap<Arm, bool> = [(Arm::LEAN, true), (Arm::LEAN_TREE, false)]
            .into_iter()
            .collect();
        assert!(!retrieval_disagreed(&classic_and_tree));
        let three: BTreeMap<Arm, bool> = [
            (Arm::CONTROL, true),
            (Arm::LEAN, false),
            (Arm::LEAN_TREE, false),
        ]
        .into_iter()
        .collect();
        assert!(retrieval_disagreed(&three));
    }

    #[test]
    fn a_tree_arm_can_diverge_from_its_classic_twin() {
        let by_arm: BTreeMap<Arm, CompletionOutcome> = [
            (Arm::LEAN, CompletionOutcome::Pass),
            (Arm::LEAN_TREE, CompletionOutcome::Fail),
        ]
        .into_iter()
        .collect();
        assert!(completion_diverged(&by_arm));
    }

    #[test]
    fn a_task_one_arm_delivered_and_the_other_did_not_is_a_divergence() {
        let pair = |control, lean| -> BTreeMap<Arm, CompletionOutcome> {
            [(Arm::CONTROL, control), (Arm::LEAN, lean)]
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

    #[test]
    fn one_arm_running_the_whole_sequence_first_fails_the_interleave_check() {
        let mut rows = vec![run_row("r1", 200_000, vec![Arm::CONTROL, Arm::LEAN])];
        let mut at = 1_767_225_600;
        for arm in [Arm::CONTROL, Arm::LEAN] {
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

    #[test]
    fn two_arms_from_separate_runs_are_flagged_as_not_interleaved() {
        let mut rows = one_configuration();
        rows.push(run_row("r2", 200_000, vec![Arm::CONTROL]));
        for task in ["T01", "T02"] {
            rows.push(thread_row(Thread {
                run_id: "r2",
                arm: Arm::CONTROL,
                task,
                rounds: 5,
                usd: 3.0,
                peak: 100_000,
                window: 200_000,
                trimmed: 0,
            }));
            rows.push(completion_row(
                "r2",
                Arm::CONTROL,
                task,
                CompletionOutcome::Pass,
            ));
            rows.push(probe_row("r2", Arm::CONTROL, task, "P.1", Outcome::Pass));
        }
        let analysis = analyse(&rows).unwrap();
        assert_eq!(analysis.comparisons.len(), 1);
        assert!(!analysis.comparisons[0].interleaved);
    }

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
