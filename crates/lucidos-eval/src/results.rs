//! The results file: append-only JSONL, one file per invocation, three row
//! kinds.
//!
//! Append-only is what makes a run resumable. A repeat whose rows are already
//! present and complete is skipped, so an interrupted night continues rather
//! than restarting and paying twice.

use std::collections::{BTreeMap, BTreeSet};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::arm::Arm;
use crate::metrics::{InputSplit, TokenCounts};
use crate::probe::Outcome;

type Fallible<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

/// The projection's word for a thread doing nothing right now.
pub const IDLE_STATUS: &str = "idle";

/// A thread that parked on an event wait, woke, and then finished.
///
/// Its own word, because the projection calls both the park and the finish
/// `idle`. Only the driver watched the wake happen, so only the driver can
/// write this down.
pub const WOKEN_STATUS: &str = "idle-after-wake";

/// A thread still holding an event wait when its deadline arrived.
pub const PARKED_STATUS: &str = "parked";

/// A thread whose every attempt at a turn came back with no text and no tool
/// call, so it never started its task.
///
/// Its own word for the same reason a park has one. The projection calls this
/// thread `idle` too, and recording that would hide the whole defect: a thread
/// that did nothing read as a task that was attempted and did not deliver.
pub const EMPTY_STATUS: &str = "empty-completion";

/// Row statuses that mean the task ran to the end.
pub const FINISHED_STATUSES: [&str; 2] = [IDLE_STATUS, WOKEN_STATUS];

/// How big a run was meant to be. A label, and nothing reads it as a gate.
///
/// It gated the retired verdict, which only a `confirmatory` run could print.
/// ADR 0110 leaves it as what a reader calls the run in the report's first
/// line. A resume still refuses to change it under you.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunConfig {
    /// A few tasks, one repeat, to debug the harness.
    Smoke,
    /// The whole task set, one or a few repeats.
    Pilot,
    /// The sized run, whose numbers are meant to be read.
    Confirmatory,
}

impl RunConfig {
    pub fn as_str(self) -> &'static str {
        match self {
            RunConfig::Smoke => "smoke",
            RunConfig::Pilot => "pilot",
            RunConfig::Confirmatory => "confirmatory",
        }
    }

    pub fn parse(s: &str) -> Option<RunConfig> {
        match s.trim().to_ascii_lowercase().as_str() {
            "smoke" => Some(RunConfig::Smoke),
            "pilot" => Some(RunConfig::Pilot),
            "confirmatory" => Some(RunConfig::Confirmatory),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ResultRow {
    Run(RunRow),
    Thread(ThreadRow),
    Probe(ProbeRow),
    Completion(CompletionRow),
}

/// What one task's completion probe resolved to.
///
/// Three values, and none of them is a *probe outcome*. Delivery and fidelity
/// are separate axes, so they share no vocabulary and no rate. `Void` means the
/// same thing here as there: this measured nothing, and it is never a failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CompletionOutcome {
    /// The deliverable is there and is right.
    Pass,
    /// The task ran and did not deliver.
    Fail,
    /// An upstream task failed, or the pair measured something other than the
    /// configuration, so delivery was not asked.
    Void,
}

impl CompletionOutcome {
    pub fn is_scored(self) -> bool {
        self != CompletionOutcome::Void
    }
}

/// One task's delivery, in one arm of one repeat.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CompletionRow {
    pub run_id: String,
    pub repeat: u32,
    pub arm: Arm,
    pub task: String,
    pub probe: String,
    pub outcome: CompletionOutcome,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RunRow {
    pub run_id: String,
    pub started: DateTime<Utc>,
    pub engine_commit: String,
    pub model: String,
    pub fixture_hash: String,
    pub guidance_hash: String,
    /// I8: the price table is part of what makes a cost figure reproducible.
    pub prices_hash: String,
    /// Precondition P3. A run without read-by-id is a pilot at best.
    pub read_by_id: bool,
    pub config: RunConfig,
    pub repeats: u32,
    pub tasks: Vec<String>,
    pub arms: Vec<Arm>,
    /// The context window this run declared, in tokens (ADR 0110 decision 9).
    ///
    /// Part of the configuration, so a budget sweep can pool several runs and
    /// still tell them apart. Defaulted, so a run recorded before the sweep
    /// existed reads zero and groups on its own.
    #[serde(default)]
    pub context_window: i64,
    /// What this run's arm workspaces and databases were named after.
    ///
    /// `eval-<run_label>-<arm>-<repeat>`, so a reader of the file can find the
    /// databases again and a concurrent run against another provider never
    /// collides with this one. Defaulted, so a run recorded before the label
    /// existed reads empty, which is what its unlabelled names were.
    #[serde(default)]
    pub run_label: String,
    /// How old a result got before a sweep could take it, in rounds.
    ///
    /// This and the interval below are the schedule the lean arm ran at, and
    /// two schedules are two designs. `guidance_hash` covers them only while
    /// the prompt quotes both. It also names nothing a reader can act on when
    /// it differs, so the numbers are recorded beside it.
    ///
    /// Defaulted, so a run recorded before the schedule was stamped reads
    /// zero. No run can pin zero, so it reads as unrecorded.
    #[serde(default)]
    pub expire_after_rounds: usize,
    /// How often the sweep ran, in rounds.
    #[serde(default)]
    pub sweep_every_rounds: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ThreadRow {
    pub run_id: String,
    pub repeat: u32,
    pub arm: Arm,
    pub task: String,
    pub thread_id: Uuid,
    pub rounds: i64,
    pub cache_creation: i64,
    pub cache_read: i64,
    /// Everything the model processed on input, cached tokens INCLUDED. Split
    /// it with [`ThreadRow::input_split`] before pricing any of it.
    #[serde(alias = "input_tokens")]
    pub input_total: i64,
    pub output_tokens: i64,
    /// The part of the four counts above spent on auxiliary work.
    ///
    /// The title, the memory extractor and the summariser, which run on a
    /// cheaper model than the one under test. Recorded because an arm making
    /// more auxiliary calls should be readable as such. Defaulted, so a run
    /// recorded before the split existed reads zero.
    #[serde(default)]
    pub auxiliary_tokens: TokenCounts,
    /// Whether the thread had stopped writing when it was snapshotted.
    ///
    /// Auxiliary work runs detached, so a snapshot taken the moment the driver
    /// leaves can miss it. `false` says the harness waited and the log was
    /// still growing, which puts every count on this row in doubt.
    ///
    /// Named for the log rather than for the auxiliary work, because the log is
    /// what was watched. A call in flight writes nothing, so `true` is elapsed
    /// quiet and not a completion the harness could join.
    ///
    /// `None` on a run recorded before the harness waited at all. Those runs
    /// snapshotted at once, so a late title landed after the row was written.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event_log_settled: Option<bool>,
    pub todo_writes: i64,
    /// Replies that carried a working-understanding span.
    ///
    /// This and the two below are defaulted, so a run recorded before they
    /// existed still parses. [`crate::metrics::DocumentWrites`] carries why
    /// they are the three worth counting.
    #[serde(default)]
    pub document_writes: i64,
    /// Writes that shared their reply with a tool call. Moving the document out
    /// of a tool call worked only if this tracks the line above.
    #[serde(default)]
    pub writes_with_a_tool_call: i64,
    /// Addresses the model held open, over the whole thread.
    #[serde(default)]
    pub items_held_open: i64,
    pub recovery_calls: BTreeMap<String, i64>,
    pub wall_secs: i64,
    /// Every dollar the thread caused, each model at its own pinned rate.
    pub usd: f64,
    /// The part of `usd` spent on auxiliary work, at the auxiliary model's rate.
    ///
    /// Defaulted, so a run recorded before the split existed reads zero. Such a
    /// run priced every token at the main model's rate, which is the defect
    /// this field exists to make visible.
    #[serde(default)]
    pub usd_auxiliary: f64,
    pub status: String,
    pub unscripted_answers: u32,
    /// When the task's first round was captured. I7 reads this to prove the
    /// arms interleaved.
    pub started: Option<DateTime<Utc>>,
    /// T12's handover length, recorded as a covariate so a long document
    /// cannot score by accident without a reader noticing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub handover_lines: Option<i64>,
    /// Whether the engine retrieved memory on this thread at all.
    ///
    /// The query classifier decides that, and it is an LLM call. When the two
    /// arms disagree on one task, the pair measures the classifier rather than
    /// the flag, and `analyse` voids it. The field defaults, because a run
    /// recorded before it existed must still parse. Both arms then read false,
    /// which is agreement and voids nothing.
    #[serde(default)]
    pub memory_recalled: bool,
    /// Recovery calls after round 2 that re-fetch a handle the thread already
    /// fetched. Defaulted, so a run recorded before it existed still parses.
    #[serde(default)]
    pub repeat_recoveries: i64,
    /// Rounds where the engine trimmed the context to fit the budget.
    ///
    /// The only field that says whether this thread reached the ceiling at all.
    /// Defaulted, so a run recorded before it existed reads zero, which is what
    /// those runs measured.
    #[serde(default)]
    pub trimmed_rounds: i64,
    /// The largest request this thread sent, in the engine's own token
    /// estimate. The peak is what decides whether a thread ever felt its
    /// budget, so it is recorded per thread rather than averaged away.
    #[serde(default)]
    pub peak_request_tokens: i64,
    /// The mean request over this thread's rounds, in the same units.
    ///
    /// Reported beside the peak because the two say different things. A high
    /// mean is a thread that carried a lot all along. A high peak over a low
    /// mean is one round that spiked.
    #[serde(default)]
    pub mean_request_tokens: i64,
    /// The context window the engine resolved for this thread's model.
    ///
    /// Read off the thread's own captures, not from the run row. A mis-seeded
    /// window then shows up as a thread disagreeing with its own run.
    #[serde(default)]
    pub context_window: i64,
    /// Turns the model ended with no text and no tool call, every attempt
    /// counted. Above zero with a finished status, a re-post recovered it.
    #[serde(default)]
    pub empty_completions: i64,
    /// Prompts the driver re-posted to recover one of those turns.
    #[serde(default)]
    pub empty_retries: i64,
    /// Sequence of the prompt that opened turn two, as the driver posted it.
    ///
    /// Recorded rather than counted, because a re-posted turn leaves more than
    /// one prompt behind it. Absent on a one-turn task, and on a run recorded
    /// before the driver wrote it: the scorer falls back to counting there,
    /// which is what those runs meant.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub followup_sequence: Option<i64>,
}

impl ThreadRow {
    /// Whether the driver saw this task through to the end, a wake included.
    ///
    /// The scoring layer voids every probe of a thread that did not finish.
    /// Reading the status by hand would void a woken thread's whole task, which
    /// is the measurement this change exists to keep.
    pub fn finished(&self) -> bool {
        FINISHED_STATUSES.contains(&self.status.as_str())
    }

    /// Take the cached parts out of the input total.
    pub fn input_split(&self) -> InputSplit {
        InputSplit::from_total(self.input_total, self.cache_read, self.cache_creation)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProbeRow {
    pub run_id: String,
    pub repeat: u32,
    pub arm: Arm,
    pub task: String,
    pub probe: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fact: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tier: Option<u8>,
    pub outcome: Outcome,
}

/// Append-only writer over one run's file.
pub struct ResultsFile {
    path: PathBuf,
}

impl ResultsFile {
    /// Path of the results file for a run id, under the results directory.
    pub fn path_for(results_dir: &Path, run_id: &str) -> PathBuf {
        results_dir.join(format!("{run_id}.jsonl"))
    }

    pub fn open(results_dir: &Path, run_id: &str) -> Fallible<ResultsFile> {
        std::fs::create_dir_all(results_dir)?;
        Ok(ResultsFile {
            path: Self::path_for(results_dir, run_id),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Append one row and flush it.
    ///
    /// Flushed per row on purpose. A killed run must leave every completed
    /// repeat on disk, or the resume skips nothing and the night is paid twice.
    pub fn append(&self, row: &ResultRow) -> Fallible<()> {
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?;
        writeln!(file, "{}", serde_json::to_string(row)?)?;
        file.flush()?;
        Ok(())
    }

    pub fn read_all(&self) -> Fallible<Vec<ResultRow>> {
        read_rows(&self.path)
    }
}

/// Read every row of a results file, refusing a line that does not parse.
///
/// A silently skipped line would drop a repeat from the analysis, which reads
/// as a smaller sample rather than as a broken file.
pub fn read_rows(path: &Path) -> Fallible<Vec<ResultRow>> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let file = std::fs::File::open(path)?;
    let mut rows = Vec::new();
    for (index, line) in BufReader::new(file).lines().enumerate() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let row: ResultRow = serde_json::from_str(&line)
            .map_err(|e| format!("{}:{} is not a result row: {e}", path.display(), index + 1))?;
        rows.push(row);
    }
    Ok(rows)
}

/// Refuse a resume whose arguments contradict the run already in the file.
///
/// The recorded row is what the analysis reads, and it is written once. A
/// resume under a different config, task set, model or fixture would append
/// rows the analysis then reports under the first run's metadata.
pub fn check_resume_matches(recorded: &RunRow, wanted: &RunRow) -> Fallible<()> {
    let differences: Vec<String> = [
        ("config", recorded.config.as_str(), wanted.config.as_str()),
        ("model", &recorded.model, &wanted.model),
        ("fixture_hash", &recorded.fixture_hash, &wanted.fixture_hash),
        (
            "guidance_hash",
            &recorded.guidance_hash,
            &wanted.guidance_hash,
        ),
        ("prices_hash", &recorded.prices_hash, &wanted.prices_hash),
        // The label decides which databases the rows came out of. Resuming
        // under a different one reads a different set of workspaces and files
        // the result under this run's metadata.
        ("run_label", &recorded.run_label, &wanted.run_label),
    ]
    .into_iter()
    .filter(|(_, was, now)| was != now)
    .map(|(field, was, now)| format!("{field} was {was} and is now {now}"))
    .chain((recorded.tasks != wanted.tasks).then(|| {
        format!(
            "tasks were {} and are now {}",
            recorded.tasks.join(","),
            wanted.tasks.join(",")
        )
    }))
    // The window and the arms are the rest of the configuration, and the
    // analysis groups on them. Resuming under either changed would append rows
    // the report then files under the first run's configuration.
    .chain((recorded.context_window != wanted.context_window).then(|| {
        format!(
            "the context window was {} and is now {}",
            recorded.context_window, wanted.context_window
        )
    }))
    .chain((recorded.arms != wanted.arms).then(|| {
        let names = |arms: &[Arm]| {
            arms.iter()
                .map(|arm| arm.as_str())
                .collect::<Vec<_>>()
                .join(",")
        };
        format!(
            "arms were {} and are now {}",
            names(&recorded.arms),
            names(&wanted.arms)
        )
    }))
    // The schedule is the rest of the design. Resuming under a different one
    // appends rows swept at a schedule the run row does not name.
    .chain(schedule_change(recorded, wanted))
    .collect();
    if differences.is_empty() {
        return Ok(());
    }
    Err(format!(
        "resume_mismatch: run {} was recorded with different arguments, and the analysis \
         reads the recorded ones. Start a new run instead. {}",
        recorded.run_id,
        differences.join("; ")
    )
    .into())
}

/// How the sweep schedule moved between the recorded run and this one.
///
/// One sentence covering both numbers, because they are one schedule and a
/// reader comparing two runs wants to see the pair.
fn schedule_change(recorded: &RunRow, wanted: &RunRow) -> Option<String> {
    let both = |run: &RunRow| format!("{}/{}", run.expire_after_rounds, run.sweep_every_rounds);
    (recorded.expire_after_rounds != wanted.expire_after_rounds
        || recorded.sweep_every_rounds != wanted.sweep_every_rounds)
        .then(|| {
            format!(
                "the sweep schedule (expire/every, in rounds) was {} and is now {}",
                both(recorded),
                both(wanted)
            )
        })
}

/// Repeats already finished in these rows, so a resume can skip them.
///
/// A repeat counts as complete when every arm has reached a verdict on every
/// task the run declared. A partly written repeat is re-run from the start,
/// because its workspaces carry state no later task can be dropped into.
///
/// A **voided** task counts as reached. It writes probe rows and no thread row.
/// Requiring a thread row read a finished repeat as unfinished and re-ran a
/// whole sequence run, which is the most expensive thing this file prevents.
pub fn completed_repeats(rows: &[ResultRow]) -> BTreeSet<u32> {
    let Some(run) = rows.iter().find_map(|row| match row {
        ResultRow::Run(run) => Some(run),
        _ => None,
    }) else {
        return BTreeSet::new();
    };
    let mut seen: BTreeMap<u32, BTreeSet<(String, String)>> = BTreeMap::new();
    for row in rows {
        let reached = match row {
            ResultRow::Thread(thread) => Some((thread.repeat, thread.arm, thread.task.clone())),
            ResultRow::Probe(probe) => Some((probe.repeat, probe.arm, probe.task.clone())),
            ResultRow::Completion(done) => Some((done.repeat, done.arm, done.task.clone())),
            ResultRow::Run(_) => None,
        };
        if let Some((repeat, arm, task)) = reached {
            seen.entry(repeat)
                .or_default()
                .insert((arm.as_str().to_string(), task));
        }
    }
    let expected: BTreeSet<(String, String)> = run
        .arms
        .iter()
        .flat_map(|arm| {
            run.tasks
                .iter()
                .map(move |task| (arm.as_str().to_string(), task.clone()))
        })
        .collect();
    seen.into_iter()
        .filter(|(_, done)| expected.is_subset(done))
        .map(|(repeat, _)| repeat)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run_row() -> ResultRow {
        ResultRow::Run(RunRow {
            run_id: "abc".into(),
            started: Utc::now(),
            engine_commit: "deadbeef".into(),
            model: "model-under-test".into(),
            fixture_hash: "f1".into(),
            guidance_hash: "g1".into(),
            prices_hash: "p1".into(),
            read_by_id: false,
            config: RunConfig::Smoke,
            repeats: 2,
            tasks: vec!["T01".into(), "T02".into()],
            arms: Arm::BOTH.to_vec(),
            context_window: 200_000,
            run_label: "test-model".into(),
            expire_after_rounds: 5,
            sweep_every_rounds: 10,
        })
    }

    fn thread_row(repeat: u32, arm: Arm, task: &str) -> ResultRow {
        ResultRow::Thread(ThreadRow {
            run_id: "abc".into(),
            repeat,
            arm,
            task: task.into(),
            thread_id: Uuid::from_u128(repeat as u128),
            rounds: 3,
            cache_creation: 1,
            cache_read: 2,
            input_total: 3,
            output_tokens: 4,
            auxiliary_tokens: TokenCounts::default(),
            event_log_settled: Some(true),
            todo_writes: 0,
            document_writes: 0,
            writes_with_a_tool_call: 0,
            items_held_open: 0,
            trimmed_rounds: 0,
            recovery_calls: BTreeMap::new(),
            repeat_recoveries: 0,
            peak_request_tokens: 0,
            mean_request_tokens: 0,
            context_window: 200_000,
            wall_secs: 5,
            usd: 0.5,
            usd_auxiliary: 0.0,
            status: "idle".into(),
            unscripted_answers: 0,
            started: None,
            handover_lines: None,
            memory_recalled: true,
            empty_completions: 0,
            empty_retries: 0,
            followup_sequence: None,
        })
    }

    fn probe_row() -> ResultRow {
        ResultRow::Probe(ProbeRow {
            run_id: "abc".into(),
            repeat: 1,
            arm: Arm::Lean,
            task: "T05".into(),
            probe: "P05.1".into(),
            fact: Some("F16".into()),
            tier: Some(3),
            outcome: Outcome::LostSilent,
        })
    }

    #[test]
    fn every_row_kind_round_trips_through_json() {
        for row in [run_row(), thread_row(1, Arm::Lean, "T01"), probe_row()] {
            let text = serde_json::to_string(&row).unwrap();
            assert_eq!(serde_json::from_str::<ResultRow>(&text).unwrap(), row);
        }
    }

    /// The file is append-only and older runs must keep parsing. A row from
    /// before the field reads as no retrieval, which is agreement in both arms.
    #[test]
    fn a_thread_row_written_before_the_retrieval_field_still_parses() {
        let mut row: serde_json::Value =
            serde_json::to_value(thread_row(1, Arm::Lean, "T01")).unwrap();
        row.as_object_mut()
            .expect("a thread row is an object")
            .remove("memory_recalled")
            .expect("the field is written");
        let parsed: ResultRow = serde_json::from_value(row).unwrap();
        match parsed {
            ResultRow::Thread(thread) => assert!(!thread.memory_recalled),
            other => panic!("expected a thread row and got {other:?}"),
        }
    }

    /// Same append-only rule for the utilisation fields. A run recorded before
    /// they existed reads zero, which is honestly what it measured: nobody was
    /// recording how full those requests were.
    #[test]
    fn a_thread_row_written_before_the_utilisation_fields_still_parses() {
        let mut row: serde_json::Value =
            serde_json::to_value(thread_row(1, Arm::Lean, "T01")).unwrap();
        let object = row.as_object_mut().expect("a thread row is an object");
        for field in [
            "repeat_recoveries",
            "peak_request_tokens",
            "mean_request_tokens",
            "context_window",
        ] {
            object.remove(field).expect("the field is written");
        }

        let parsed: ResultRow = serde_json::from_value(row).unwrap();
        match parsed {
            ResultRow::Thread(thread) => {
                assert_eq!(thread.repeat_recoveries, 0);
                assert_eq!(thread.peak_request_tokens, 0);
                assert_eq!(thread.context_window, 0);
            }
            other => panic!("expected a thread row and got {other:?}"),
        }
    }

    /// A run recorded before per-model pricing reads no auxiliary split.
    ///
    /// That is honest about what it measured. Such a run priced every token at
    /// the main model's rate, so it could not tell auxiliary spend apart.
    #[test]
    fn a_thread_row_written_before_the_auxiliary_split_still_parses() {
        let mut row: serde_json::Value =
            serde_json::to_value(thread_row(1, Arm::Lean, "T01")).unwrap();
        let object = row.as_object_mut().expect("a thread row is an object");
        for field in ["auxiliary_tokens", "usd_auxiliary"] {
            object.remove(field).expect("the field is written");
        }

        let parsed: ResultRow = serde_json::from_value(row).unwrap();
        match parsed {
            ResultRow::Thread(thread) => {
                assert_eq!(thread.auxiliary_tokens, TokenCounts::default());
                assert_eq!(thread.usd_auxiliary, 0.0);
                assert_eq!(thread.usd, 0.5, "the combined total is untouched");
            }
            other => panic!("expected a thread row and got {other:?}"),
        }
    }

    /// Rows written before `input_tokens` was renamed still load.
    #[test]
    fn the_old_input_field_name_still_reads() {
        let mut row: serde_json::Value =
            serde_json::to_value(thread_row(1, Arm::Lean, "T01")).unwrap();
        let object = row.as_object_mut().expect("a thread row is an object");
        let total = object.remove("input_total").expect("the field is written");
        object.insert("input_tokens".to_string(), total);

        let parsed: ResultRow = serde_json::from_value(row).unwrap();
        match parsed {
            ResultRow::Thread(thread) => assert_eq!(thread.input_total, 3),
            other => panic!("expected a thread row and got {other:?}"),
        }
    }

    /// A woken thread ran its task to the end, so its probes must still score.
    /// A park, a timeout and an empty thread did not, and theirs are voided.
    #[test]
    fn a_thread_that_woke_and_finished_counts_as_finished() {
        let row = |status: &str| ThreadRow {
            status: status.into(),
            ..match thread_row(1, Arm::Lean, "T08") {
                ResultRow::Thread(thread) => thread,
                _ => unreachable!("thread_row builds a thread row"),
            }
        };
        assert!(row("idle").finished());
        assert!(row(WOKEN_STATUS).finished());
        assert!(!row(PARKED_STATUS).finished());
        assert!(!row(EMPTY_STATUS).finished());
        assert!(!row("timeout").finished());
        assert!(!row("failed").finished());
    }

    /// A run recorded before the retry existed reads as no empty completions and no
    /// boundary. Both are what those runs measured: one prompt per turn, and no
    /// re-post to move it.
    #[test]
    fn a_thread_row_written_before_the_empty_completion_fields_still_parses() {
        let mut row: serde_json::Value =
            serde_json::to_value(thread_row(1, Arm::Lean, "T14")).unwrap();
        let object = row.as_object_mut().expect("a thread row is an object");
        object
            .remove("empty_completions")
            .expect("the field is written");
        object
            .remove("empty_retries")
            .expect("the field is written");
        // The boundary is skipped when absent, so an old row looks the same.
        assert!(!object.contains_key("followup_sequence"));

        let parsed: ResultRow = serde_json::from_value(row).unwrap();
        match parsed {
            ResultRow::Thread(thread) => {
                assert_eq!(thread.empty_completions, 0);
                assert_eq!(thread.empty_retries, 0);
                assert_eq!(thread.followup_sequence, None);
            }
            other => panic!("expected a thread row and got {other:?}"),
        }
    }

    /// A run recorded before the harness waited reads as unrecorded, never as
    /// a boundary that was reached. Those runs snapshotted at once.
    #[test]
    fn a_thread_row_written_before_the_settle_wait_reads_as_unrecorded() {
        let mut row: serde_json::Value =
            serde_json::to_value(thread_row(1, Arm::Lean, "T14")).unwrap();
        let object = row.as_object_mut().expect("a thread row is an object");
        object
            .remove("event_log_settled")
            .expect("the field is written");

        let parsed: ResultRow = serde_json::from_value(row).unwrap();
        match parsed {
            ResultRow::Thread(thread) => assert_eq!(thread.event_log_settled, None),
            other => panic!("expected a thread row and got {other:?}"),
        }
    }

    fn completion_row() -> ResultRow {
        ResultRow::Completion(CompletionRow {
            run_id: "abc".into(),
            repeat: 1,
            arm: Arm::Lean,
            task: "T06".into(),
            probe: "C06".into(),
            outcome: CompletionOutcome::Fail,
        })
    }

    #[test]
    fn a_completion_row_names_its_kind_and_its_outcome() {
        let text = serde_json::to_string(&completion_row()).unwrap();
        assert!(text.contains("\"kind\":\"completion\""));
        assert!(text.contains("\"outcome\":\"fail\""));
        assert_eq!(
            serde_json::from_str::<ResultRow>(&text).unwrap(),
            completion_row()
        );
    }

    /// A void measured nothing. Pass and fail are the two that count.
    #[test]
    fn only_a_scored_completion_outcome_counts() {
        assert!(CompletionOutcome::Pass.is_scored());
        assert!(CompletionOutcome::Fail.is_scored());
        assert!(!CompletionOutcome::Void.is_scored());
    }

    /// The file is append-only, so a run recorded before completion probes
    /// existed still reads. It simply says nothing about delivery.
    #[test]
    fn a_results_file_written_before_completion_rows_still_parses() {
        let rows = [run_row(), thread_row(1, Arm::Lean, "T01"), probe_row()];
        let text: Vec<String> = rows
            .iter()
            .map(|row| serde_json::to_string(row).unwrap())
            .collect();
        for line in &text {
            assert!(!line.contains("\"kind\":\"completion\""));
            serde_json::from_str::<ResultRow>(line).expect("an older row still parses");
        }
    }

    #[test]
    fn a_probe_row_names_its_outcome_the_way_the_plan_writes_it() {
        let text = serde_json::to_string(&probe_row()).unwrap();
        assert!(text.contains("\"kind\":\"probe\""));
        assert!(text.contains("\"outcome\":\"lost-silent\""));
    }

    #[test]
    fn a_repeat_missing_one_arm_is_not_complete() {
        let mut rows = vec![run_row()];
        for task in ["T01", "T02"] {
            rows.push(thread_row(1, Arm::Control, task));
        }
        assert!(completed_repeats(&rows).is_empty());
    }

    #[test]
    fn a_repeat_with_every_arm_and_task_is_skipped_on_resume() {
        let mut rows = vec![run_row()];
        for arm in Arm::BOTH {
            for task in ["T01", "T02"] {
                rows.push(thread_row(1, arm, task));
            }
        }
        rows.push(thread_row(2, Arm::Lean, "T01"));
        let complete = completed_repeats(&rows);
        assert!(complete.contains(&1));
        assert!(!complete.contains(&2));
    }

    fn bare_run_row() -> RunRow {
        match run_row() {
            ResultRow::Run(run) => run,
            _ => unreachable!("run_row builds a run row"),
        }
    }

    #[test]
    fn a_resume_with_the_same_arguments_is_allowed() {
        check_resume_matches(&bare_run_row(), &bare_run_row()).unwrap();
    }

    #[test]
    fn a_resume_that_changed_the_configuration_is_refused() {
        let wanted = RunRow {
            config: RunConfig::Confirmatory,
            ..bare_run_row()
        };
        let err = check_resume_matches(&bare_run_row(), &wanted)
            .unwrap_err()
            .to_string();
        assert!(err.contains("resume_mismatch"));
        assert!(err.contains("config was smoke and is now confirmatory"));
    }

    /// The label decides which databases the rows are read out of, so resuming
    /// under a new one measures a different set of workspaces.
    #[test]
    fn a_resume_that_changed_the_run_label_is_refused() {
        let wanted = RunRow {
            run_label: "gpt-5-6-sol".into(),
            ..bare_run_row()
        };
        let err = check_resume_matches(&bare_run_row(), &wanted)
            .unwrap_err()
            .to_string();
        assert!(err.contains("resume_mismatch"), "{err}");
        assert!(
            err.contains("run_label was test-model and is now gpt-5-6-sol"),
            "{err}"
        );
    }

    /// A run recorded before the label existed still parses, and reads as the
    /// unlabelled workspaces it actually created.
    #[test]
    fn a_run_recorded_before_the_run_label_parses_as_unlabelled() {
        let line = r#"{"kind":"run","run_id":"abc","started":"2026-01-01T00:00:00Z",
            "engine_commit":"deadbeef","model":"m","fixture_hash":"f1",
            "guidance_hash":"g1","prices_hash":"p1","read_by_id":true,
            "config":"smoke","repeats":1,"tasks":["T01"],"arms":["control"]}"#;
        let row: ResultRow = serde_json::from_str(&line.replace('\n', "")).unwrap();
        match row {
            ResultRow::Run(run) => {
                assert!(run.run_label.is_empty());
                assert_eq!(run.context_window, 0);
                // Zero is no schedule, so it reads as unrecorded rather than
                // as a run that swept every zero rounds.
                assert_eq!(run.expire_after_rounds, 0);
                assert_eq!(run.sweep_every_rounds, 0);
            }
            _ => unreachable!("that line is a run row"),
        }
    }

    /// Two schedules are two designs, and the rows do not say which they were
    /// swept at. Pooling them into one file reports a design nothing ran.
    #[test]
    fn a_resume_that_changed_the_sweep_schedule_is_refused() {
        for wanted in [
            RunRow {
                expire_after_rounds: 3,
                ..bare_run_row()
            },
            RunRow {
                sweep_every_rounds: 8,
                ..bare_run_row()
            },
        ] {
            let err = check_resume_matches(&bare_run_row(), &wanted)
                .unwrap_err()
                .to_string();
            assert!(err.contains("resume_mismatch"), "{err}");
            assert!(err.contains("the sweep schedule"), "{err}");
        }
    }

    /// The refusal quotes both numbers, so an operator can see which moved.
    #[test]
    fn the_schedule_refusal_names_the_pair_on_both_sides() {
        let wanted = RunRow {
            sweep_every_rounds: 8,
            ..bare_run_row()
        };
        let err = check_resume_matches(&bare_run_row(), &wanted)
            .unwrap_err()
            .to_string();
        assert!(err.contains("was 5/10 and is now 5/8"), "{err}");
    }

    #[test]
    fn a_resume_that_changed_the_fixture_or_the_task_set_is_refused() {
        for wanted in [
            RunRow {
                fixture_hash: "f2".into(),
                ..bare_run_row()
            },
            RunRow {
                tasks: vec!["T01".into()],
                ..bare_run_row()
            },
        ] {
            assert!(check_resume_matches(&bare_run_row(), &wanted).is_err());
        }
    }

    /// A voided task leaves probe rows and no thread row. The repeat is still
    /// finished, and re-running it would pay for a whole sequence run twice.
    #[test]
    fn a_repeat_whose_last_task_was_voided_is_still_complete() {
        let mut rows = vec![run_row()];
        for arm in Arm::BOTH {
            rows.push(thread_row(1, arm, "T01"));
            rows.push(ResultRow::Probe(ProbeRow {
                run_id: "abc".into(),
                repeat: 1,
                arm,
                task: "T02".into(),
                probe: "P02.1".into(),
                fact: None,
                tier: None,
                outcome: Outcome::Void,
            }));
        }
        assert!(completed_repeats(&rows).contains(&1));
    }

    #[test]
    fn a_file_with_no_run_row_completes_nothing() {
        let rows = vec![thread_row(1, Arm::Lean, "T01")];
        assert!(completed_repeats(&rows).is_empty());
    }

    #[test]
    fn a_written_file_reads_back_as_the_rows_that_went_in() {
        let dir = std::env::temp_dir().join(format!("lucidos-eval-{}", Uuid::new_v4().simple()));
        let file = ResultsFile::open(&dir, "abc").unwrap();
        let written = vec![run_row(), thread_row(1, Arm::Lean, "T01"), probe_row()];
        for row in &written {
            file.append(row).unwrap();
        }
        assert_eq!(file.read_all().unwrap(), written);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_corrupt_line_is_an_error_and_never_a_smaller_sample() {
        let dir = std::env::temp_dir().join(format!("lucidos-eval-{}", Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("abc.jsonl");
        std::fs::write(&path, "{\"kind\":\"nonsense\"}\n").unwrap();
        let err = read_rows(&path).unwrap_err().to_string();
        assert!(err.contains("is not a result row"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_missing_file_reads_as_no_rows_rather_than_an_error() {
        let path = std::env::temp_dir().join(format!("lucidos-eval-{}.jsonl", Uuid::new_v4()));
        assert!(read_rows(&path).unwrap().is_empty());
    }

    #[test]
    fn the_three_configurations_round_trip_through_their_names() {
        for config in [RunConfig::Smoke, RunConfig::Pilot, RunConfig::Confirmatory] {
            assert_eq!(RunConfig::parse(config.as_str()), Some(config));
        }
        assert_eq!(RunConfig::parse("dress-rehearsal"), None);
    }
}
