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

pub const IDLE_STATUS: &str = "idle";

pub const WOKEN_STATUS: &str = "idle-after-wake";

pub const PARKED_STATUS: &str = "parked";

pub const EMPTY_STATUS: &str = "empty-completion";

pub const FINISHED_STATUSES: [&str; 2] = [IDLE_STATUS, WOKEN_STATUS];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunConfig {
    Smoke,
    Pilot,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CompletionOutcome {
    Pass,
    Fail,
    Void,
}

impl CompletionOutcome {
    pub fn is_scored(self) -> bool {
        self != CompletionOutcome::Void
    }
}

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
    pub prices_hash: String,
    pub read_by_id: bool,
    pub config: RunConfig,
    pub repeats: u32,
    pub tasks: Vec<String>,
    pub arms: Vec<Arm>,
    #[serde(default)]
    pub context_window: i64,
    #[serde(default)]
    pub run_label: String,
    #[serde(default)]
    pub expire_after_rounds: usize,
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
    #[serde(alias = "input_tokens")]
    pub input_total: i64,
    pub output_tokens: i64,
    #[serde(default)]
    pub auxiliary_tokens: TokenCounts,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event_log_settled: Option<bool>,
    pub todo_writes: i64,
    #[serde(default)]
    pub document_writes: i64,
    #[serde(default)]
    pub writes_with_a_tool_call: i64,
    #[serde(default)]
    pub items_held_open: i64,
    pub recovery_calls: BTreeMap<String, i64>,
    pub wall_secs: i64,
    pub usd: f64,
    #[serde(default)]
    pub usd_auxiliary: f64,
    pub status: String,
    pub unscripted_answers: u32,
    pub started: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub handover_lines: Option<i64>,
    #[serde(default)]
    pub memory_recalled: bool,
    #[serde(default)]
    pub repeat_recoveries: i64,
    #[serde(default)]
    pub trimmed_rounds: i64,
    #[serde(default)]
    pub peak_request_tokens: i64,
    #[serde(default)]
    pub mean_request_tokens: i64,
    #[serde(default)]
    pub context_window: i64,
    #[serde(default)]
    pub empty_completions: i64,
    #[serde(default)]
    pub empty_retries: i64,
    #[serde(default)]
    pub refusals: i64,
    #[serde(default)]
    pub refusal_retries: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub followup_sequence: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compaction_caught_up: Option<bool>,
}

impl ThreadRow {
    pub fn finished(&self) -> bool {
        FINISHED_STATUSES.contains(&self.status.as_str())
    }

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

pub struct ResultsFile {
    path: PathBuf,
}

impl ResultsFile {
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

#[derive(Debug, Default, PartialEq)]
pub struct RepeatProgress {
    pub reached: usize,
    pub failed: BTreeMap<Arm, BTreeSet<String>>,
    pub last_threads: BTreeMap<Arm, Uuid>,
}

pub fn repeat_progress(rows: &[ResultRow], run: &RunRow, repeat: u32) -> Fallible<RepeatProgress> {
    let mut verdicts: BTreeSet<(Arm, &str)> = BTreeSet::new();
    let mut threads: BTreeMap<(Arm, &str), &ThreadRow> = BTreeMap::new();
    let mut touched: BTreeSet<&str> = BTreeSet::new();
    for row in rows {
        match row {
            ResultRow::Completion(done) if done.repeat == repeat => {
                verdicts.insert((done.arm, &done.task));
                touched.insert(&done.task);
            }
            ResultRow::Thread(thread) if thread.repeat == repeat => {
                threads.insert((thread.arm, &thread.task), thread);
                touched.insert(&thread.task);
            }
            ResultRow::Probe(probe) if probe.repeat == repeat => {
                touched.insert(&probe.task);
            }
            _ => {}
        }
    }
    let reached = run
        .tasks
        .iter()
        .take_while(|task| {
            run.arms
                .iter()
                .all(|arm| verdicts.contains(&(*arm, task.as_str())))
        })
        .count();
    let done = &run.tasks[..reached];
    if let Some(cut) = touched.iter().find(|task| !done.iter().any(|t| t == *task)) {
        return Err(format!(
            "cut_off: task {cut} of repeat {repeat} has rows but not a verdict from every arm, \
             so an earlier invocation stopped inside it. Its workspaces hold half a task, and \
             no later task can be dropped into them. Start a new run instead."
        )
        .into());
    }
    let mut progress = RepeatProgress {
        reached,
        ..RepeatProgress::default()
    };
    for arm in run.arms.iter().copied() {
        let failed = progress.failed.entry(arm).or_default();
        for task in done {
            match threads.get(&(arm, task.as_str())) {
                Some(thread) => {
                    progress.last_threads.insert(arm, thread.thread_id);
                    if !FINISHED_STATUSES.contains(&thread.status.as_str()) {
                        failed.insert(task.clone());
                    }
                }
                None => {
                    failed.insert(task.clone());
                }
            }
        }
    }
    Ok(progress)
}

pub fn completed_repeats(rows: &[ResultRow]) -> BTreeSet<u32> {
    let Some(run) = rows.iter().find_map(|row| match row {
        ResultRow::Run(run) => Some(run),
        _ => None,
    }) else {
        return BTreeSet::new();
    };
    let repeats: BTreeSet<u32> = rows
        .iter()
        .filter_map(|row| match row {
            ResultRow::Completion(done) => Some(done.repeat),
            _ => None,
        })
        .collect();
    repeats
        .into_iter()
        .filter(|repeat| {
            repeat_progress(rows, run, *repeat)
                .is_ok_and(|progress| progress.reached == run.tasks.len())
        })
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
            arms: vec![Arm::CONTROL, Arm::LEAN],
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
            refusals: 0,
            refusal_retries: 0,
            followup_sequence: None,
            compaction_caught_up: None,
        })
    }

    fn probe_row() -> ResultRow {
        ResultRow::Probe(ProbeRow {
            run_id: "abc".into(),
            repeat: 1,
            arm: Arm::LEAN,
            task: "T05".into(),
            probe: "P05.1".into(),
            fact: Some("F16".into()),
            tier: Some(3),
            outcome: Outcome::LostSilent,
        })
    }

    #[test]
    fn every_row_kind_round_trips_through_json() {
        for row in [run_row(), thread_row(1, Arm::LEAN, "T01"), probe_row()] {
            let text = serde_json::to_string(&row).unwrap();
            assert_eq!(serde_json::from_str::<ResultRow>(&text).unwrap(), row);
        }
    }

    #[test]
    fn a_thread_row_written_before_the_retrieval_field_still_parses() {
        let mut row: serde_json::Value =
            serde_json::to_value(thread_row(1, Arm::LEAN, "T01")).unwrap();
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

    #[test]
    fn a_thread_row_written_before_the_utilisation_fields_still_parses() {
        let mut row: serde_json::Value =
            serde_json::to_value(thread_row(1, Arm::LEAN, "T01")).unwrap();
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

    #[test]
    fn a_thread_row_written_before_the_auxiliary_split_still_parses() {
        let mut row: serde_json::Value =
            serde_json::to_value(thread_row(1, Arm::LEAN, "T01")).unwrap();
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

    #[test]
    fn the_old_input_field_name_still_reads() {
        let mut row: serde_json::Value =
            serde_json::to_value(thread_row(1, Arm::LEAN, "T01")).unwrap();
        let object = row.as_object_mut().expect("a thread row is an object");
        let total = object.remove("input_total").expect("the field is written");
        object.insert("input_tokens".to_string(), total);

        let parsed: ResultRow = serde_json::from_value(row).unwrap();
        match parsed {
            ResultRow::Thread(thread) => assert_eq!(thread.input_total, 3),
            other => panic!("expected a thread row and got {other:?}"),
        }
    }

    #[test]
    fn a_thread_that_woke_and_finished_counts_as_finished() {
        let row = |status: &str| ThreadRow {
            status: status.into(),
            ..match thread_row(1, Arm::LEAN, "T08") {
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

    #[test]
    fn a_thread_row_written_before_the_repost_fields_still_parses() {
        let mut row: serde_json::Value =
            serde_json::to_value(thread_row(1, Arm::LEAN, "T14")).unwrap();
        let object = row.as_object_mut().expect("a thread row is an object");
        object
            .remove("empty_completions")
            .expect("the field is written");
        for field in ["empty_retries", "refusals", "refusal_retries"] {
            object.remove(field).expect("the field is written");
        }
        assert!(!object.contains_key("followup_sequence"));

        let parsed: ResultRow = serde_json::from_value(row).unwrap();
        match parsed {
            ResultRow::Thread(thread) => {
                assert_eq!(thread.empty_completions, 0);
                assert_eq!(thread.empty_retries, 0);
                assert_eq!((thread.refusals, thread.refusal_retries), (0, 0));
                assert_eq!(thread.followup_sequence, None);
            }
            other => panic!("expected a thread row and got {other:?}"),
        }
    }

    #[test]
    fn a_thread_row_written_before_the_settle_wait_reads_as_unrecorded() {
        let mut row: serde_json::Value =
            serde_json::to_value(thread_row(1, Arm::LEAN, "T14")).unwrap();
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
            arm: Arm::LEAN,
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

    #[test]
    fn only_a_scored_completion_outcome_counts() {
        assert!(CompletionOutcome::Pass.is_scored());
        assert!(CompletionOutcome::Fail.is_scored());
        assert!(!CompletionOutcome::Void.is_scored());
    }

    #[test]
    fn a_results_file_written_before_completion_rows_still_parses() {
        let rows = [run_row(), thread_row(1, Arm::LEAN, "T01"), probe_row()];
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
            rows.push(thread_row(1, Arm::CONTROL, task));
        }
        assert!(completed_repeats(&rows).is_empty());
    }

    #[test]
    fn a_repeat_with_every_arm_and_task_is_skipped_on_resume() {
        let mut rows = vec![run_row()];
        rows.extend(finished_task(1, "T01"));
        rows.extend(finished_task(1, "T02"));
        rows.extend(finished_task(2, "T01"));
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
                assert_eq!(run.expire_after_rounds, 0);
                assert_eq!(run.sweep_every_rounds, 0);
            }
            _ => unreachable!("that line is a run row"),
        }
    }

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

    #[test]
    fn a_repeat_whose_last_task_was_voided_is_still_complete() {
        let mut rows = vec![run_row()];
        rows.extend(finished_task(1, "T01"));
        for arm in [Arm::CONTROL, Arm::LEAN] {
            rows.push(verdict_row(1, arm, "T02", CompletionOutcome::Void));
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
    fn a_repeat_missing_its_last_verdict_is_not_complete() {
        let mut rows = vec![run_row()];
        rows.extend(finished_task(1, "T01"));
        for arm in [Arm::CONTROL, Arm::LEAN] {
            rows.push(thread_row(1, arm, "T02"));
        }
        assert!(completed_repeats(&rows).is_empty());
    }

    fn verdict_row(repeat: u32, arm: Arm, task: &str, outcome: CompletionOutcome) -> ResultRow {
        ResultRow::Completion(CompletionRow {
            run_id: "abc".into(),
            repeat,
            arm,
            task: task.into(),
            probe: format!("C{}", &task[1..]),
            outcome,
        })
    }

    fn finished_task(repeat: u32, task: &str) -> Vec<ResultRow> {
        let mut rows = Vec::new();
        for arm in [Arm::CONTROL, Arm::LEAN] {
            rows.push(thread_row(repeat, arm, task));
        }
        for arm in [Arm::CONTROL, Arm::LEAN] {
            rows.push(verdict_row(repeat, arm, task, CompletionOutcome::Pass));
        }
        rows
    }

    #[test]
    fn a_repeat_with_no_rows_starts_at_the_first_task() {
        let progress = repeat_progress(&[run_row()], &bare_run_row(), 1).unwrap();
        assert_eq!(progress.reached, 0);
        assert!(progress.last_threads.is_empty());
        assert!(progress.failed.values().all(BTreeSet::is_empty));
    }

    #[test]
    fn a_repeat_stopped_at_a_task_boundary_continues_after_it() {
        let mut rows = vec![run_row()];
        rows.extend(finished_task(1, "T01"));
        let progress = repeat_progress(&rows, &bare_run_row(), 1).unwrap();
        assert_eq!(progress.reached, 1);
        assert_eq!(progress.last_threads.len(), 2);
        assert!(progress.failed.values().all(BTreeSet::is_empty));
    }

    #[test]
    fn a_resume_rebuilds_which_tasks_failed_in_each_arm() {
        let mut rows = vec![run_row()];
        let ResultRow::Thread(timed_out) = thread_row(1, Arm::CONTROL, "T01") else {
            unreachable!("thread_row builds a thread row")
        };
        rows.push(ResultRow::Thread(ThreadRow {
            status: "timeout".into(),
            ..timed_out
        }));
        rows.push(verdict_row(1, Arm::CONTROL, "T01", CompletionOutcome::Fail));
        rows.push(verdict_row(1, Arm::LEAN, "T01", CompletionOutcome::Void));
        let progress = repeat_progress(&rows, &bare_run_row(), 1).unwrap();
        assert_eq!(progress.reached, 1);
        for arm in [Arm::CONTROL, Arm::LEAN] {
            assert!(progress.failed[&arm].contains("T01"), "{arm}");
        }
        assert!(!progress.last_threads.contains_key(&Arm::LEAN));
    }

    #[test]
    fn a_repeat_cut_off_inside_a_task_is_refused() {
        let mut rows = vec![run_row()];
        rows.extend(finished_task(1, "T01"));
        rows.push(thread_row(1, Arm::CONTROL, "T02"));
        let err = repeat_progress(&rows, &bare_run_row(), 1)
            .unwrap_err()
            .to_string();
        assert!(err.contains("cut_off: task T02 of repeat 1"), "{err}");
    }

    #[test]
    fn another_repeats_rows_do_not_move_this_one() {
        let mut rows = vec![run_row()];
        rows.extend(finished_task(1, "T01"));
        rows.push(thread_row(1, Arm::LEAN, "T02"));
        assert_eq!(
            repeat_progress(&rows, &bare_run_row(), 2).unwrap().reached,
            0
        );
    }

    #[test]
    fn a_file_with_no_run_row_completes_nothing() {
        let rows = vec![thread_row(1, Arm::LEAN, "T01")];
        assert!(completed_repeats(&rows).is_empty());
    }

    #[test]
    fn a_written_file_reads_back_as_the_rows_that_went_in() {
        let dir = tempfile::tempdir().unwrap();
        let file = ResultsFile::open(&dir.path().join("results"), "abc").unwrap();
        let written = vec![run_row(), thread_row(1, Arm::LEAN, "T01"), probe_row()];
        for row in &written {
            file.append(row).unwrap();
        }
        assert_eq!(file.read_all().unwrap(), written);
    }

    #[test]
    fn a_corrupt_line_is_an_error_and_never_a_smaller_sample() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("abc.jsonl");
        std::fs::write(&path, "{\"kind\":\"nonsense\"}\n").unwrap();
        let err = read_rows(&path).unwrap_err().to_string();
        assert!(err.contains("is not a result row"));
    }

    #[test]
    fn a_missing_file_reads_as_no_rows_rather_than_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("missing.jsonl");
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
