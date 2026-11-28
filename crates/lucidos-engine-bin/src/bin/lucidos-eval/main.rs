mod analyse;
mod arm;
mod assertions;
mod calls;
mod candidate;
mod command_judge;
mod config;
mod decision_check;
mod driver;
#[cfg(feature = "fixture-gen")]
mod fixture_gen;
mod gateway;
mod guidance;
mod judge;
mod manipulation;
mod metrics;
mod probe;
mod read_decision;
mod replay;
mod results;
mod snapshot;
mod wake_decision;
mod workspace;

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use chrono::Utc;
use clap::{Parser, Subcommand};
use sqlx::PgPool;

use arm::{Arm, SweepPins};
use assertions::AssertionContext;
use config::{Fixture, Probe, Task, TaskGraph};
use driver::{ArmEndpoint, DrivenTask};
use probe::Outcome;
use results::{
    CompletionOutcome, CompletionRow, ProbeRow, ResultRow, ResultsFile, RunConfig, RunRow,
    ThreadRow,
};

type Fallible<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

const HANDOVER_PATH: &str = "artifacts/build-health/HANDOVER.md";

#[derive(Parser)]
#[command(
    name = "lucidos-eval",
    about = "The ADR 0110 context-handling benchmark, and the command judge check"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

const DEFAULT_ARMS: &str = "lean";

#[derive(Subcommand)]
enum Command {
    /// Create and seed the named arms for one repeat, then compare the digests.
    Seed {
        #[arg(long, default_value_t = 1)]
        repeat: u32,
        #[arg(long, value_delimiter = ',', default_value = DEFAULT_ARMS)]
        arms: Vec<String>,
        /// Declare a smaller context window on the seeded model row.
        #[arg(long)]
        window: Option<i64>,
    },
    /// Seed, then run every task in every named arm, interleaved per task.
    Run {
        #[arg(long, default_value = "smoke")]
        config: String,
        #[arg(long, default_value_t = 1)]
        repeats: u32,
        /// Run only these tasks, in this order. Defaults to the whole set.
        #[arg(long, value_delimiter = ',')]
        tasks: Option<Vec<String>>,
        /// Which arms to measure. One is the shape; two is the comparison.
        #[arg(long, value_delimiter = ',', default_value = DEFAULT_ARMS)]
        arms: Vec<String>,
        /// The context window to declare, in tokens. Omitted, the engine infers
        /// it from the model id, which is that model's real window.
        #[arg(long)]
        window: Option<i64>,
        /// Resume into an existing results file rather than starting one. A
        /// repeat cut at a task boundary continues in the workspaces it left.
        #[arg(long)]
        run_id: Option<String>,
        /// End this invocation after this task. The same command with a later
        /// task, or none, continues the repeat without reseeding.
        #[arg(long)]
        stop_after: Option<String>,
        /// Retain completed arm workspaces after publishing durable inputs.
        #[arg(long)]
        keep_workspaces: bool,
    },
    /// Resolve every probe for a finished run and append the probe rows.
    Score {
        #[arg(long)]
        run_id: String,
        /// Also score the judged probes and the disclaimer route. Costs money.
        #[arg(long, default_value_t = false)]
        with_judge: bool,
    },
    /// The whole analysis as JSON. Pool a sweep by naming every run.
    Analyse {
        #[arg(long)]
        run_id: Vec<String>,
    },
    /// The one-page human summary of an analysed run.
    Report {
        #[arg(long)]
        run_id: Vec<String>,
    },
    /// Replay one thread's captured rounds out of its arm's event log.
    Replay {
        #[arg(long)]
        run_id: String,
        /// The thread, by id or by any prefix of one. Required unless --list.
        #[arg(long)]
        thread: Option<String>,
        /// One round. Defaults to every round of the thread.
        #[arg(long)]
        round: Option<usize>,
        /// Print this section's whole body rather than a preview of each.
        #[arg(long)]
        section: Option<String>,
        /// Print the captured payload as JSON instead of the rendering.
        #[arg(long, default_value_t = false)]
        raw: bool,
        /// List the run's threads and stop.
        #[arg(long, default_value_t = false)]
        list: bool,
    },
    /// Classify `eval/command-guard/cases.toml` with each candidate, through
    /// the command guard's own judge, and print the table. Spends real tokens.
    #[command(name = "command-judge")]
    JudgeCheck {
        /// Candidates as `model@tier`, comma separated.
        #[arg(long, value_delimiter = ',')]
        models: Vec<String>,
        /// Where each answer is appended, as JSON lines.
        #[arg(long, default_value = "/tmp/command-judge-results.jsonl")]
        out: PathBuf,
    },
    /// Decide `eval/read-decision/turns.toml` with each candidate, through the
    /// turn-end gate's own question and threshold, and print the table. Spends
    /// real tokens.
    #[command(name = read_decision::CHECK.command)]
    ReadDecisionCheck(DecisionCheckArgs),
    /// Decide `eval/wake-decision/turns.toml` with each candidate, through the
    /// turn-end gate's own state, question and threshold, and print the table.
    /// Spends real tokens.
    #[command(name = wake_decision::CHECK.command)]
    WakeDecisionCheck(DecisionCheckArgs),
    /// Re-embed the seeded memory rows from `memory-seed.toml`.
    #[cfg(feature = "fixture-gen")]
    GenerateMemoryFixture,
}

/// What both turn-end decision checks take.
#[derive(clap::Args)]
struct DecisionCheckArgs {
    /// Which backend answers: `chat`, or a System One row id such as `jev`,
    /// `openrouter` or `openai`. A row reads its key the way the engine does,
    /// from this workspace's database and environment.
    #[arg(long, default_value = decision_check::CHAT_BACKEND)]
    backend: String,
    /// Chat candidates as `model@tier`, comma separated. Only `chat` reads
    /// them.
    #[arg(long, value_delimiter = ',')]
    models: Vec<String>,
    /// Where each decision is appended, as JSON lines. Defaults to
    /// `/tmp/<check>-results.jsonl`.
    #[arg(long)]
    out: Option<PathBuf>,
    /// Another labelled set in the same shape, such as one kept outside the
    /// tree. Defaults to the shipped `eval/<check>/turns.toml`.
    #[arg(long)]
    set: Option<PathBuf>,
}

impl DecisionCheckArgs {
    fn paths(&self, check: &decision_check::Check) -> (PathBuf, PathBuf) {
        let set = self.set.clone().unwrap_or_else(|| check.shipped_set());
        let out = self.out.clone().unwrap_or_else(|| check.default_out());
        (set, out)
    }
}

fn repository_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("the root package sits two levels below the repository root")
}

fn eval_fixture_root() -> PathBuf {
    repository_root().join("eval")
}

struct Paths {
    repo_root: PathBuf,
    fixture_root: PathBuf,
    results_dir: PathBuf,
    eval_root: PathBuf,
    engine_bin: PathBuf,
    pg_base: String,
}

impl Paths {
    fn resolve() -> Fallible<Paths> {
        let repo_root = repository_root().to_path_buf();
        let fixture_root = eval_fixture_root().join("context-mode");
        Ok(Paths {
            results_dir: fixture_root.join("results"),
            eval_root: PathBuf::from(required_env(
                "LUCIDOS_EVAL_ROOT",
                "it names the directory the eval workspaces are created under",
            )?),
            engine_bin: PathBuf::from(required_env(
                "LUCIDOS_EVAL_ENGINE_BIN",
                "it names the engine binary under test",
            )?),
            pg_base: required_env(
                "LUCIDOS_EVAL_PG_BASE",
                "it is the connection string without a database name, and every arm's database \
                 hangs off it",
            )?,
            fixture_root,
            repo_root,
        })
    }
}

fn required_env(key: &str, why: &str) -> Fallible<String> {
    std::env::var(key).map_err(|_| format!("{key} is unset: {why}").into())
}

#[tokio::main]
async fn main() {
    if let Err(error) = dispatch().await {
        eprintln!("[eval] {error}");
        std::process::exit(1);
    }
}

async fn dispatch() -> Fallible<()> {
    let cli = Cli::parse();
    match cli.command {
        #[cfg(feature = "fixture-gen")]
        Command::GenerateMemoryFixture => {
            let paths = Paths::resolve()?;
            fixture_gen::generate(&paths.fixture_root.join("fixtures"))
        }
        Command::Seed {
            repeat,
            arms,
            window,
        } => {
            let paths = Paths::resolve()?;
            let fixture = Fixture::load(&paths.fixture_root)?;
            preflight(&paths, &fixture)?;
            let arms = selected_arms(&arms)?;
            seed_repeat(
                &paths,
                checked_repeat(repeat)?,
                &arms,
                checked_window(window)?,
                SweepPins::from_env()?,
            )
            .await?;
            println!("[eval] repeat {repeat} is seeded and every digest agrees");
            Ok(())
        }
        Command::Run {
            config,
            repeats,
            tasks,
            arms,
            window,
            run_id,
            stop_after,
            keep_workspaces,
        } => {
            let paths = Paths::resolve()?;
            let fixture = Fixture::load(&paths.fixture_root)?;
            preflight(&paths, &fixture)?;
            let config =
                RunConfig::parse(&config).ok_or("config must be smoke, pilot or confirmatory")?;
            run_command(
                &paths,
                &fixture,
                RunShape {
                    config,
                    repeats: checked_repeat(repeats)?,
                    arms: selected_arms(&arms)?,
                    window: checked_window(window)?,
                    sweep: SweepPins::from_env()?,
                },
                tasks,
                run_id,
                stop_after,
                keep_workspaces,
            )
            .await
        }
        Command::Score { run_id, with_judge } => {
            let paths = Paths::resolve()?;
            let fixture = Fixture::load(&paths.fixture_root)?;
            score_command(&paths, &fixture, &run_id, with_judge).await
        }
        Command::Analyse { run_id } => {
            let paths = Paths::resolve()?;
            let analysis = analyse::analyse(&read_runs(&paths, &run_id)?)?;
            println!("{}", serde_json::to_string_pretty(&analysis)?);
            Ok(())
        }
        Command::Replay {
            run_id,
            thread,
            round,
            section,
            raw,
            list,
        } => {
            let paths = Paths::resolve()?;
            replay_command(
                &paths,
                &run_id,
                thread.as_deref(),
                list,
                &replay::Options {
                    round,
                    section: section.as_deref(),
                    raw,
                },
            )
            .await
        }
        Command::JudgeCheck { models, out } => {
            let set = eval_fixture_root().join("command-guard/cases.toml");
            command_judge::run(&set, &models, &out).await
        }
        Command::ReadDecisionCheck(args) => {
            let (set, out) = args.paths(&read_decision::CHECK);
            read_decision::run(&set, &args.backend, &args.models, &out).await
        }
        Command::WakeDecisionCheck(args) => {
            let (set, out) = args.paths(&wake_decision::CHECK);
            wake_decision::run(&set, &args.backend, &args.models, &out).await
        }
        Command::Report { run_id } => {
            let paths = Paths::resolve()?;
            let fixture = Fixture::load(&paths.fixture_root)?;
            let analysis = analyse::analyse(&read_runs(&paths, &run_id)?)?;
            print_report(
                &analysis,
                &trims_by_pass(&paths, &run_id).await?,
                &calls_by_arm(&paths, &run_id, &fixture).await?,
            );
            Ok(())
        }
    }
}

struct RunShape {
    config: RunConfig,
    repeats: u32,
    arms: Vec<Arm>,
    window: Option<i64>,
    sweep: SweepPins,
}

fn selected_arms(names: &[String]) -> Fallible<Vec<Arm>> {
    let mut arms: Vec<Arm> = Vec::new();
    for name in names {
        let known = Arm::ALL
            .iter()
            .map(|arm| arm.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        let arm = Arm::parse(name)
            .ok_or_else(|| format!("{name} is not an arm. The arms are {known}"))?;
        if arms.contains(&arm) {
            return Err(format!("the {arm} arm is named twice").into());
        }
        arms.push(arm);
    }
    if arms.is_empty() {
        return Err("name at least one arm".into());
    }
    Ok(arms)
}

fn checked_repeat(repeat: u32) -> Fallible<u32> {
    match repeat <= workspace::MAX_REPEAT {
        true => Ok(repeat),
        false => Err(format!(
            "repeat {repeat} is above {}, and an arm's database name would no longer fit \
             a Postgres identifier",
            workspace::MAX_REPEAT
        )
        .into()),
    }
}

fn checked_window(window: Option<i64>) -> Fallible<Option<i64>> {
    match window {
        Some(window) if window < analyse::MIN_CONTEXT_WINDOW => Err(format!(
            "a {window}-token window leaves no room for messages: the tools array and the \
             system prompt are about 85,000 chars against a budget of (window - 8000) * 1.5. \
             The smallest window worth measuring is {}.",
            analyse::MIN_CONTEXT_WINDOW
        )
        .into()),
        window => Ok(window),
    }
}

fn preflight(paths: &Paths, fixture: &Fixture) -> Fallible<()> {
    fixture.validate()?;
    fixture.check_models_priced(seed_pins(None).model)?;
    manipulation::preflight(manipulation::flag_availability_in_checkout(
        &paths.repo_root,
    )?)
}

fn read_runs(paths: &Paths, run_ids: &[String]) -> Fallible<Vec<ResultRow>> {
    if run_ids.is_empty() {
        return Err("name at least one --run-id".into());
    }
    let mut rows = Vec::new();
    for run_id in run_ids {
        rows.extend(results::read_rows(&ResultsFile::path_for(
            &paths.results_dir,
            run_id,
        ))?);
    }
    Ok(rows)
}

const DEFAULT_MODEL: &str = "claude-opus-5-5";

fn seed_pins(window: Option<i64>) -> workspace::SeedPins<'static> {
    workspace::SeedPins {
        model: leak_env("LUCIDOS_EVAL_MODEL", DEFAULT_MODEL),
        model_label: leak_env("LUCIDOS_EVAL_MODEL_LABEL", "Model under test"),
        model_provider: leak_env("LUCIDOS_EVAL_MODEL_PROVIDER", "vertex"),
        reasoning_effort: leak_env("LUCIDOS_EVAL_REASONING_EFFORT", "default"),
        background_model: lucidos_engine::core::prefs::BACKGROUND_MODEL,
        context_window: window,
    }
}

fn run_label() -> &'static workspace::RunLabel {
    static LABEL: std::sync::OnceLock<workspace::RunLabel> = std::sync::OnceLock::new();
    LABEL.get_or_init(|| {
        let source = set_env("LUCIDOS_EVAL_RUN_LABEL")
            .or_else(|| set_env("LUCIDOS_EVAL_MODEL"))
            .unwrap_or_else(|| DEFAULT_MODEL.to_string());
        workspace::RunLabel::derive(&source)
    })
}

fn set_env(key: &str) -> Option<String> {
    std::env::var(key)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn models_under_test(model: &str) -> Vec<String> {
    let mut models: Vec<String> = match model.is_empty() {
        true => Vec::new(),
        false => vec![model.to_string()],
    };
    let Some(declared) = set_env("LUCIDOS_EVAL_MODEL_SET") else {
        return models;
    };
    for other in declared.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        if !models.iter().any(|m| m == other) {
            models.push(other.to_string());
        }
    }
    models
}

fn announce_run_shape(under_test: &[String], window: Option<i64>, sweep: SweepPins) {
    if under_test.len() > 1 {
        println!("[eval] models under test, this run and its siblings:");
        for model in under_test {
            println!("[eval]   {model}");
        }
    }
    println!(
        "[eval] sweep schedule: a result expires after {} round(s), and the sweep runs every \
         {} round(s)",
        sweep.expire_after_rounds, sweep.sweep_every_rounds
    );
    match window {
        Some(window) => println!(
            "[eval] context window pinned at {window} tokens, seeded onto the model row so the \
             engine's budget follows it"
        ),
        None => println!(
            "[eval] NO context window pinned, so the engine infers one from the model id. Two \
             providers compared this way are not budget-equal, and the ceiling tasks measure \
             whatever each model's own window is. Pass --window to hold them equal."
        ),
    }
}

fn leak_env(key: &str, fallback: &'static str) -> &'static str {
    match std::env::var(key) {
        Ok(value) => Box::leak(value.into_boxed_str()),
        Err(_) => fallback,
    }
}

async fn arm_pool(
    paths: &Paths,
    label: &workspace::RunLabel,
    arm: Arm,
    repeat: u32,
) -> Fallible<PgPool> {
    let url = workspace::arm_database_url(&paths.pg_base, label, arm, repeat);
    sqlx::postgres::PgPoolOptions::new()
        .max_connections(4)
        .connect(&url)
        .await
        .map_err(|e| format!("cannot connect to the {arm} arm's database: {e}").into())
}

async fn seed_repeat(
    paths: &Paths,
    repeat: u32,
    arms: &[Arm],
    window: Option<i64>,
    sweep: SweepPins,
) -> Fallible<()> {
    let pins = seed_pins(window);
    let mut digests = Vec::new();
    for arm in arms.iter().copied() {
        let slug = workspace::arm_workspace_name(run_label(), arm, repeat);
        let ws = workspace::arm_workspace_path(&paths.eval_root, run_label(), arm, repeat);
        gateway::check_seed_registration(&slug).await?;
        workspace::create_seed_workspace(&ws)?;
        let database = workspace::arm_database_name(run_label(), arm, repeat);
        let url = workspace::arm_database_url(&paths.pg_base, run_label(), arm, repeat);
        println!("[eval] {slug} seeds into database \"{database}\"");
        workspace::create_database(&paths.pg_base, &database)?;
        workspace::install_fixture_tree(&ws, &paths.fixture_root.join(config::SEED_TREE))?;
        workspace::migrate_by_booting(&paths.engine_bin, &ws, &url)?;
        workspace::apply_seed_sql(&url, &paths.fixture_root.join("fixtures/seed.sql"), &pins)?;
        let pool = arm_pool(paths, run_label(), arm, repeat).await?;
        workspace::write_arm_preference(&pool, arm, sweep).await?;
        let rows = workspace::read_seed_rows(&pool).await?;
        digests.push(workspace::SeedDigest {
            db: workspace::digest_rows(&rows),
            fs: workspace::fs_digest(&ws)?,
        });
        gateway::register_arm(&ws, &slug, &arm_display_name(run_label(), arm, repeat)).await;
    }
    match digests.as_slice() {
        [only] => {
            println!("[eval] seed digest {}", only.db);
            Ok(())
        }
        [first, rest @ ..] => rest
            .iter()
            .try_for_each(|other| workspace::compare_digests(first, other)),
        [] => Err("no arm was seeded".into()),
    }
}

fn arm_display_name(label: &workspace::RunLabel, arm: Arm, repeat: u32) -> String {
    format!("Eval {label}: {arm} arm, repeat {repeat}")
}

async fn run_command(
    paths: &Paths,
    fixture: &Fixture,
    shape: RunShape,
    tasks: Option<Vec<String>>,
    run_id: Option<String>,
    stop_after: Option<String>,
    keep_workspaces: bool,
) -> Fallible<()> {
    let run_id = run_id.unwrap_or_else(|| uuid::Uuid::new_v4().simple().to_string());
    let file = ResultsFile::open(&paths.results_dir, &run_id)?;
    let existing = file.read_all()?;
    let done = results::completed_repeats(&existing);
    let selected: Vec<&Task> = match &tasks {
        Some(ids) => ids
            .iter()
            .map(|id| {
                fixture
                    .task(id)
                    .ok_or_else(|| -> Box<dyn std::error::Error + Send + Sync> {
                        format!("{id} is not a task").into()
                    })
            })
            .collect::<Fallible<Vec<_>>>()?,
        None => fixture.tasks.iter().collect(),
    };
    let end = match &stop_after {
        Some(id) => {
            selected
                .iter()
                .position(|task| &task.id == id)
                .ok_or_else(|| format!("--stop-after {id} is not one of this run's tasks"))?
                + 1
        }
        None => selected.len(),
    };
    let pins = seed_pins(shape.window);
    let under_test = models_under_test(pins.model);
    judge::check_judge_is_independent(
        &fixture.judge,
        &under_test.iter().map(String::as_str).collect::<Vec<_>>(),
    )?;
    announce_run_shape(&under_test, shape.window, shape.sweep);

    let this_run = RunRow {
        run_id: run_id.clone(),
        started: Utc::now(),
        engine_commit: engine_commit(&paths.repo_root),
        model: pins.model.to_string(),
        fixture_hash: fixture.fixture_hash.clone(),
        guidance_hash: fixture.guidance_hash.clone(),
        prices_hash: fixture.prices_hash.clone(),
        read_by_id: read_by_id_available(),
        config: shape.config,
        repeats: shape.repeats,
        tasks: selected.iter().map(|t| t.id.clone()).collect(),
        arms: shape.arms.clone(),
        context_window: shape.window.unwrap_or_default(),
        run_label: run_label().as_str().to_string(),
        expire_after_rounds: shape.sweep.expire_after_rounds,
        sweep_every_rounds: shape.sweep.sweep_every_rounds,
    };
    match existing.iter().find_map(|row| match row {
        ResultRow::Run(run) => Some(run),
        _ => None,
    }) {
        Some(recorded) => results::check_resume_matches(recorded, &this_run)?,
        None => file.append(&ResultRow::Run(this_run.clone()))?,
    }

    for repeat in 1..=shape.repeats {
        if done.contains(&repeat) {
            println!("[eval] repeat {repeat} is already complete, skipping");
            continue;
        }
        let progress = results::repeat_progress(&existing, &this_run, repeat)?;
        if progress.reached >= end {
            return Err(format!(
                "repeat {repeat} already ran past {}, so this invocation has nothing to drive. \
                 Name a later --stop-after.",
                selected[end - 1].id
            )
            .into());
        }
        let stretch = Stretch {
            repeat,
            tasks: &selected[..end],
            progress,
        };
        run_repeat(paths, fixture, &file, &run_id, stretch, &shape).await?;
        if let Some(last) = &stop_after {
            println!(
                "[eval] stopped after {last} in repeat {repeat}. The same command continues \
                 from there, with a later --stop-after or none"
            );
            break;
        }
    }
    if stop_after.is_none() {
        capture_run_inputs(paths, fixture, &run_id).await?;
        if !keep_workspaces {
            for repeat in 1..=shape.repeats {
                for arm in &shape.arms {
                    if snapshot::Snapshot::read(
                        &paths.results_dir,
                        &run_id,
                        run_label().as_str(),
                        *arm,
                        repeat,
                    )?
                    .is_none()
                    {
                        return Err("completed arm has no durable snapshot; cleanup refused".into());
                    }
                    let ws =
                        workspace::arm_workspace_path(&paths.eval_root, run_label(), *arm, repeat);
                    let slug = workspace::arm_workspace_name(run_label(), *arm, repeat);
                    gateway::pin_run_owner(&paths.results_dir, &run_id, &ws, &slug, true)?;
                }
            }
            let mut failures = Vec::new();
            for repeat in 1..=shape.repeats {
                for arm in &shape.arms {
                    let ws =
                        workspace::arm_workspace_path(&paths.eval_root, run_label(), *arm, repeat);
                    let slug = workspace::arm_workspace_name(run_label(), *arm, repeat);
                    if let Err(error) =
                        gateway::cleanup_arm(&paths.results_dir, &run_id, &ws, &slug).await
                    {
                        failures.push(format!("{slug}: {error}"));
                    }
                }
            }
            if !failures.is_empty() {
                return Err(format!(
                    "run inputs saved; cleanup incomplete: {}",
                    failures.join("; ")
                )
                .into());
            }
        }
    }
    println!("[eval] results at {}", file.path().display());
    Ok(())
}

struct Stretch<'a> {
    repeat: u32,
    tasks: &'a [&'a Task],
    progress: results::RepeatProgress,
}

async fn run_repeat(
    paths: &Paths,
    fixture: &Fixture,
    file: &ResultsFile,
    run_id: &str,
    stretch: Stretch<'_>,
    shape: &RunShape,
) -> Fallible<()> {
    let Stretch {
        repeat,
        tasks,
        progress,
    } = stretch;
    let remaining = &tasks[progress.reached..];
    match progress.reached {
        0 => seed_repeat(paths, repeat, &shape.arms, shape.window, shape.sweep).await?,
        _ => println!(
            "[eval] repeat {repeat} continues at {} in the workspaces an earlier invocation left",
            remaining[0].id
        ),
    }
    for arm in &shape.arms {
        let ws = workspace::arm_workspace_path(&paths.eval_root, run_label(), *arm, repeat);
        let slug = workspace::arm_workspace_name(run_label(), *arm, repeat);
        gateway::pin_run_owner(&paths.results_dir, run_id, &ws, &slug, progress.reached > 0)?;
    }
    let mut engines = Vec::new();
    let mut endpoints = Vec::new();
    let booted = boot_arms(paths, repeat, &shape.arms, &mut engines, &mut endpoints).await;

    let outcome = match booted {
        Ok(()) => {
            async {
                let next = &remaining[0].id;
                check_workspaces_resumable(&endpoints, &progress, run_id, repeat, next).await?;
                await_tree_backfill(&endpoints).await?;
                drive_repeat(
                    fixture,
                    file,
                    run_id,
                    repeat,
                    remaining,
                    progress.failed,
                    &endpoints,
                )
                .await
            }
            .await
        }
        Err(error) => Err(error),
    };
    let stop_failures: Vec<String> = engines
        .into_iter()
        .filter_map(|engine| workspace::stop_engine(engine).err())
        .map(|error| error.to_string())
        .collect();
    outcome?;
    if stop_failures.is_empty() {
        return Ok(());
    }
    Err(format!(
        "cannot stop this repeat's engines: {}",
        stop_failures.join("; ")
    )
    .into())
}

async fn boot_arms(
    paths: &Paths,
    repeat: u32,
    arms: &[Arm],
    engines: &mut Vec<workspace::BootedEngine>,
    endpoints: &mut Vec<(Arm, PathBuf, ArmEndpoint)>,
) -> Fallible<()> {
    for arm in arms.iter().copied() {
        let ws = workspace::arm_workspace_path(&paths.eval_root, run_label(), arm, repeat);
        let url = workspace::arm_database_url(&paths.pg_base, run_label(), arm, repeat);
        let port = claim_arm_port(arm, repeat).await?;
        let engine = workspace::boot_engine(&paths.engine_bin, &ws, &url, port, true)?;
        let base_url = engine.base_url.clone();
        engines.push(engine);
        let pool = arm_pool(paths, run_label(), arm, repeat).await?;
        endpoints.push((arm, ws, ArmEndpoint::connect(&base_url, pool).await?));
    }
    Ok(())
}

async fn await_tree_backfill(endpoints: &[(Arm, PathBuf, ArmEndpoint)]) -> Fallible<()> {
    for (arm, _, endpoint) in endpoints {
        if !arm.expects_tree_memory() {
            continue;
        }
        let waited = std::time::Instant::now();
        if !driver::wait_until_tree_ready(&endpoint.pool).await {
            return Err(format!(
                "tree_not_ready: the {arm} arm's engine never reported its summary trees ready, \
                 so every turn would run Classic under a Tree label. Read its engine log."
            )
            .into());
        }
        println!(
            "[eval] {arm} arm: summary trees ready after {}s",
            waited.elapsed().as_secs()
        );
    }
    Ok(())
}

async fn check_workspaces_resumable(
    endpoints: &[(Arm, PathBuf, ArmEndpoint)],
    progress: &results::RepeatProgress,
    run_id: &str,
    repeat: u32,
    next: &str,
) -> Fallible<()> {
    for (arm, _, endpoint) in endpoints {
        if let Some(thread_id) = progress.last_threads.get(arm) {
            if !driver::thread_exists(&endpoint.pool, *thread_id).await? {
                return Err(format!(
                    "workspace_reseeded: the {arm} arm no longer holds thread {thread_id}, the \
                     last one this run drove there. Another run under the same label reseeded \
                     it. Start a new run instead."
                )
                .into());
            }
        }
        let marker = driver::task_marker(run_id, repeat, *arm, next);
        if let Some(thread_id) = driver::find_thread_by_marker(&endpoint.pool, &marker).await? {
            return Err(format!(
                "cut_off: the {arm} arm already holds thread {thread_id} for {next}, with no \
                 row for it, so an earlier invocation stopped inside {next}. Start a new run \
                 instead."
            )
            .into());
        }
    }
    Ok(())
}

async fn claim_arm_port(arm: Arm, repeat: u32) -> Fallible<u16> {
    let slug = workspace::arm_workspace_name(run_label(), arm, repeat);
    let Some(port) = gateway::registered_port(&slug).await else {
        return workspace::free_port();
    };
    gateway::release_arm(&slug).await;
    if !workspace::wait_for_port_free(port, ARM_PORT_RELEASE_TIMEOUT) {
        println!("[eval] port {port} is still held after asking the gateway to release {slug}");
    }
    println!("[eval] {slug} boots on its registered port {port}");
    Ok(port)
}

const ARM_PORT_RELEASE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

async fn drive_repeat(
    fixture: &Fixture,
    file: &ResultsFile,
    run_id: &str,
    repeat: u32,
    tasks: &[&Task],
    mut failed: BTreeMap<Arm, BTreeSet<String>>,
    endpoints: &[(Arm, PathBuf, ArmEndpoint)],
) -> Fallible<()> {
    let graph = TaskGraph::new(&fixture.tasks);
    for task in tasks {
        let completion = fixture.completion_for(&task.id).ok_or_else(|| {
            format!(
                "task {} has no completion probe, which the loader should have refused",
                task.id
            )
        })?;
        let mut retrieved: BTreeMap<Arm, bool> = BTreeMap::new();
        let mut delivered: BTreeMap<Arm, CompletionOutcome> = BTreeMap::new();
        let mut ran_empty: BTreeMap<Arm, bool> = BTreeMap::new();
        let mut probes_voided: BTreeSet<Arm> = BTreeSet::new();
        for (arm, workspace_dir, endpoint) in endpoints {
            println!(
                "[eval] repeat {repeat}, {arm} arm, {}: {}",
                task.id, task.title
            );
            let arm_failures = &failed[arm];
            let blocked =
                task_is_blocked(&graph, arm_failures, task, endpoint, workspace_dir).await?;
            if let Some(reason) = blocked {
                println!("[eval]   voided: {reason}");
                failed
                    .get_mut(arm)
                    .expect("every arm that runs is seeded")
                    .insert(task.id.clone());
                for row in void_rows(fixture, run_id, repeat, *arm, task) {
                    file.append(&ResultRow::Probe(row))?;
                }
                probes_voided.insert(*arm);
                delivered.insert(*arm, CompletionOutcome::Void);
                continue;
            }
            manipulation::check_tree_path(
                *arm,
                lucidos_engine::engine::summary_tree::tree_ready(&endpoint.pool).await,
            )?;
            let marker = driver::task_marker(run_id, repeat, *arm, &task.id);
            let driven = driver::drive_task(
                endpoint,
                task,
                &marker,
                &fixture.scope_rule,
                task_answers(fixture, &task.id),
            )
            .await?;
            if !driven.completed() {
                failed
                    .get_mut(arm)
                    .expect("every arm that runs is seeded")
                    .insert(task.id.clone());
            }
            let event_log_settled =
                driver::wait_until_settled(&endpoint.pool, driven.thread_id).await?;
            if !event_log_settled {
                println!(
                    "[eval]   {} was still writing events when the settle bound arrived, so \
                     its counts may be short",
                    task.id
                );
            }
            let settled = Settled {
                event_log: event_log_settled,
                compaction: match arm.expects_tree_memory() {
                    true => Some(compaction_caught_up(&endpoint.pool, &task.id).await?),
                    false => None,
                },
            };
            let metrics =
                metrics::collect(&endpoint.pool, driven.thread_id, &fixture.prices).await?;
            gate_manipulation(*arm, &endpoint.pool, driven.thread_id).await?;
            manipulation::check_no_recall_on_tree(*arm, metrics.memory_recalled)?;
            let handover_lines = handover_line_count(task, workspace_dir);
            retrieved.insert(*arm, metrics.memory_recalled);
            let ended_empty = driven.status == driver::TaskStatus::Empty;
            ran_empty.insert(*arm, ended_empty);
            delivered.insert(
                *arm,
                match ended_empty {
                    true => CompletionOutcome::Void,
                    false => score_completion(completion, endpoint, workspace_dir, &driven).await?,
                },
            );
            file.append(&ResultRow::Thread(thread_row(
                run_id,
                repeat,
                *arm,
                &driven,
                &metrics,
                handover_lines,
                settled,
            )))?;
        }
        write_pair_verdict(
            fixture,
            file,
            run_id,
            repeat,
            task,
            completion,
            &PairOutcomes {
                retrieved: &retrieved,
                delivered: &delivered,
                ran_empty: &ran_empty,
                probes_voided: &probes_voided,
            },
        )?;
    }
    Ok(())
}

async fn score_completion(
    probe: &config::CompletionProbe,
    endpoint: &ArmEndpoint,
    workspace_dir: &Path,
    driven: &DrivenTask,
) -> Fallible<CompletionOutcome> {
    let assertion = probe.assert.as_ref().ok_or_else(|| {
        format!(
            "completion probe {} has no assertion, which the loader should have refused",
            probe.id
        )
    })?;
    let world = ThreadWorld::load(
        &endpoint.pool,
        driven.thread_id,
        TurnTwo::just_driven(driven.followup_sequence),
    )
    .await?;
    let data_dir = workspace_dir.join("data");
    let passed = assertion.evaluate(&AssertionContext {
        data_dir: &data_dir,
        final_response: &world.final_response,
        tool_calls: &world.tool_calls,
        events: &world.events,
        round_two_sequence: world.round_two_sequence,
        followup_sequence: world.followup_sequence,
    })?;
    if !passed {
        println!(
            "[eval]   {} did not deliver: {}",
            probe.id, probe.deliverable
        );
    }
    Ok(match passed {
        true => CompletionOutcome::Pass,
        false => CompletionOutcome::Fail,
    })
}

struct PairOutcomes<'a> {
    retrieved: &'a BTreeMap<Arm, bool>,
    delivered: &'a BTreeMap<Arm, CompletionOutcome>,
    ran_empty: &'a BTreeMap<Arm, bool>,
    probes_voided: &'a BTreeSet<Arm>,
}

fn write_pair_verdict(
    fixture: &Fixture,
    file: &ResultsFile,
    run_id: &str,
    repeat: u32,
    task: &Task,
    completion: &config::CompletionProbe,
    outcomes: &PairOutcomes,
) -> Fallible<()> {
    let classifier_disagreed = analyse::retrieval_disagreed(outcomes.retrieved);
    let ran_empty = analyse::empty_completion_voided(outcomes.ran_empty);
    let mut reasons = Vec::new();
    if classifier_disagreed {
        reasons.push(
            "the query classifier retrieved memory in one arm only, so this pair would \
             measure the classifier rather than the configuration",
        );
    }
    if ran_empty {
        reasons.push(
            "an arm ended every attempt at a turn with no text and no tool call, so this \
             pair would score a thread that never started the task",
        );
    }
    if analyse::completion_diverged(outcomes.delivered) {
        reasons.push(
            "one arm delivered this task and the other did not, so its cost and its probes \
             compare two different amounts of work",
        );
    }
    if !reasons.is_empty() {
        println!("[eval]   voided: {}", reasons.join("; and "));
        for arm in outcomes.delivered.keys().copied() {
            if outcomes.probes_voided.contains(&arm) {
                continue;
            }
            for row in void_rows(fixture, run_id, repeat, arm, task) {
                file.append(&ResultRow::Probe(row))?;
            }
        }
    }
    for (arm, outcome) in outcomes.delivered {
        file.append(&ResultRow::Completion(CompletionRow {
            run_id: run_id.to_string(),
            repeat,
            arm: *arm,
            task: task.id.clone(),
            probe: completion.id.clone(),
            outcome: match classifier_disagreed || ran_empty {
                true => CompletionOutcome::Void,
                false => *outcome,
            },
        }))?;
    }
    Ok(())
}

async fn task_is_blocked(
    graph: &TaskGraph,
    failed: &BTreeSet<String>,
    task: &Task,
    endpoint: &ArmEndpoint,
    workspace_dir: &Path,
) -> Fallible<Option<String>> {
    if graph.is_voided_by(&task.id, failed) {
        return Ok(Some("an upstream task failed".to_string()));
    }
    let data_dir = workspace_dir.join("data");
    let events = metrics::workspace_events(&endpoint.pool).await?;
    let context = AssertionContext {
        data_dir: &data_dir,
        final_response: "",
        tool_calls: &[],
        events: &events,
        round_two_sequence: None,
        followup_sequence: None,
    };
    for precondition in &task.preconditions {
        if !precondition.evaluate(&context)? {
            return Ok(Some(format!("a precondition failed: {precondition:?}")));
        }
    }
    Ok(None)
}

fn handover_line_count(task: &Task, workspace_dir: &Path) -> Option<i64> {
    if task.id != analyse::CENSUS_TASK {
        return None;
    }
    let path = workspace_dir.join("data").join(HANDOVER_PATH);
    std::fs::read_to_string(path)
        .ok()
        .map(|text| text.lines().count() as i64)
}

fn void_rows(fixture: &Fixture, run_id: &str, repeat: u32, arm: Arm, task: &Task) -> Vec<ProbeRow> {
    let mut probes: Vec<Probe> = fixture.probes_for(&task.id).cloned().collect();
    if task.id == analyse::CENSUS_TASK {
        probes.extend(fixture.census_probes(HANDOVER_PATH));
    }
    probes
        .into_iter()
        .map(|probe| ProbeRow {
            run_id: run_id.to_string(),
            repeat,
            arm,
            task: task.id.clone(),
            tier: probe.tier,
            fact: probe.fact,
            probe: probe.id,
            outcome: Outcome::Void,
        })
        .collect()
}

async fn gate_manipulation(arm: Arm, pool: &PgPool, thread_id: uuid::Uuid) -> Fallible<()> {
    let rounds = manipulation::captured_rounds(pool, thread_id).await?;
    let starts = manipulation::exchange_starts(pool, thread_id).await?;
    manipulation::check(arm, &manipulation::place_rounds(&rounds, &starts))
}

fn task_answers<'a>(fixture: &'a Fixture, task: &str) -> Option<&'a config::TaskAnswers> {
    fixture.answers.iter().find(|entry| entry.task == task)
}

#[derive(Debug, Clone, Copy)]
struct Settled {
    event_log: bool,
    compaction: Option<bool>,
}

async fn compaction_caught_up(pool: &PgPool, task: &str) -> Fallible<bool> {
    let caught_up = driver::wait_until_compacted(pool).await?;
    if !caught_up {
        println!(
            "[eval]   the compactor had not caught up with {task} when its bound arrived, so \
             the next task reads part-built trees"
        );
    }
    Ok(caught_up)
}

fn thread_row(
    run_id: &str,
    repeat: u32,
    arm: Arm,
    driven: &DrivenTask,
    metrics: &metrics::ThreadMetrics,
    handover_lines: Option<i64>,
    settled: Settled,
) -> ThreadRow {
    let combined = metrics.tokens.combined();
    ThreadRow {
        run_id: run_id.to_string(),
        repeat,
        arm,
        task: driven.task.clone(),
        thread_id: driven.thread_id,
        rounds: metrics.tokens.rounds(),
        cache_creation: combined.cache_creation,
        cache_read: combined.cache_read,
        input_total: combined.input_total,
        output_tokens: combined.output_tokens,
        auxiliary_tokens: metrics.tokens.auxiliary(),
        event_log_settled: Some(settled.event_log),
        todo_writes: metrics.todo_writes,
        document_writes: metrics.document.writes,
        writes_with_a_tool_call: metrics.document.writes_with_a_tool_call,
        items_held_open: metrics.document.items_held_open,
        recovery_calls: metrics.recovery_calls.clone(),
        repeat_recoveries: metrics.repeat_recoveries,
        trimmed_rounds: metrics.trimmed_rounds,
        peak_request_tokens: metrics.utilisation.peak_request_tokens,
        mean_request_tokens: metrics.utilisation.mean_request_tokens,
        context_window: metrics.utilisation.context_window,
        wall_secs: metrics.tokens.wall_secs(),
        usd: metrics.spend.total,
        usd_auxiliary: metrics.spend.auxiliary,
        status: driven.row_status(),
        unscripted_answers: driven.unscripted_answers,
        started: metrics.tokens.started,
        handover_lines,
        memory_recalled: metrics.memory_recalled,
        empty_completions: driven.empty_completions as i64,
        empty_retries: driven.empty_retries as i64,
        refusals: driven.refusals as i64,
        refusal_retries: driven.refusal_retries as i64,
        followup_sequence: driven.followup_sequence,
        compaction_caught_up: settled.compaction,
    }
}

async fn score_command(
    paths: &Paths,
    fixture: &Fixture,
    run_id: &str,
    with_judge: bool,
) -> Fallible<()> {
    let file = ResultsFile::open(&paths.results_dir, run_id)?;
    let rows = file.read_all()?;
    let threads: Vec<ThreadRow> = rows
        .iter()
        .filter_map(|row| match row {
            ResultRow::Thread(thread) => Some(thread.clone()),
            _ => None,
        })
        .collect();
    if threads.is_empty() {
        return Err(format!("run {run_id} has no thread rows to score").into());
    }
    if with_judge {
        let recorded_model = rows.iter().find_map(|row| match row {
            ResultRow::Run(run) => Some(run.model.clone()),
            _ => None,
        });
        let under_test = models_under_test(recorded_model.as_deref().unwrap_or_default());
        judge::check_judge_is_independent(
            &fixture.judge,
            &under_test.iter().map(String::as_str).collect::<Vec<_>>(),
        )?;
    }
    let already_scored: BTreeSet<(u32, Arm, String)> = rows
        .iter()
        .filter_map(|row| match row {
            ResultRow::Probe(probe) => Some((probe.repeat, probe.arm, probe.probe.clone())),
            _ => None,
        })
        .collect();
    let judge = match with_judge {
        true => Some(judge::Judge::connect(&fixture.judge).await?),
        false => None,
    };
    let graph = TaskGraph::new(&fixture.tasks);
    let mut failed: BTreeMap<(u32, Arm), BTreeSet<String>> = BTreeMap::new();
    for thread in threads.iter().filter(|t| !t.finished()) {
        failed
            .entry((thread.repeat, thread.arm))
            .or_default()
            .insert(thread.task.clone());
    }
    let completions: Vec<&CompletionRow> = rows
        .iter()
        .filter_map(|row| match row {
            ResultRow::Completion(done) => Some(done),
            _ => None,
        })
        .collect();
    let by_reference: Vec<&ThreadRow> = threads.iter().collect();
    let voided: BTreeSet<analyse::PairKey> = analyse::classifier_voided_pairs(&by_reference)
        .union(&analyse::completion_diverged_pairs(&completions))
        .cloned()
        .collect::<BTreeSet<_>>()
        .union(&analyse::empty_completion_pairs(&by_reference))
        .cloned()
        .collect();
    let census = fixture.census_probes(HANDOVER_PATH);
    let labels = RunLabels::read(paths, &[run_id.to_string()])?;
    let mut skipped_judge = 0usize;
    let mut judged: Vec<judge::JudgedProbe> = Vec::new();
    let mut triage: Vec<judge::TriageRow> = Vec::new();
    let no_failures = BTreeSet::new();

    for thread in &threads {
        if voided.contains(&analyse::pair_key(
            &thread.run_id,
            thread.repeat,
            &thread.task,
        )) {
            continue;
        }
        let label = labels.of(&thread.run_id);
        let mut applicable: Vec<Probe> = fixture.probes_for(&thread.task).cloned().collect();
        if thread.task == analyse::CENSUS_TASK {
            applicable.extend(census.iter().cloned());
        }
        let turn_two = TurnTwo::of(
            thread.followup_sequence,
            fixture
                .task(&thread.task)
                .is_some_and(|task| task.followup.is_some()),
        );
        let saved = snapshot::Snapshot::read(
            &paths.results_dir,
            run_id,
            label.as_str(),
            thread.arm,
            thread.repeat,
        )?;
        let saved_data;
        let (world, data_dir) = if let Some(mut saved) = saved {
            saved_data = saved.data_dir()?;
            let inputs = saved
                .threads
                .remove(&thread.thread_id)
                .ok_or("snapshot is missing the scored thread")?;
            (inputs.world, saved_data.path().to_path_buf())
        } else {
            let pool = arm_pool(paths, label, thread.arm, thread.repeat).await?;
            (
                ThreadWorld::load(&pool, thread.thread_id, turn_two).await?,
                workspace::arm_workspace_path(&paths.eval_root, label, thread.arm, thread.repeat)
                    .join("data"),
            )
        };
        let scoring = ScoringContext {
            fixture,
            world: &world,
            data_dir: &data_dir,
            thread,
            graph: &graph,
            failed: failed
                .get(&(thread.repeat, thread.arm))
                .unwrap_or(&no_failures),
            judge: judge.as_ref(),
        };
        let mut all_passed = true;
        for probe in &applicable {
            if already_scored.contains(&(thread.repeat, thread.arm, probe.id.clone())) {
                continue;
            }
            if probe.judge.is_some() && judge.is_none() {
                skipped_judge += 1;
                continue;
            }
            let (row, verdict) = score_probe(&scoring, probe).await?;
            if let Some(verdict) = verdict {
                judged.push(verdict);
            }
            if row.outcome.is_scored() && !row.outcome.is_pass() {
                all_passed = false;
                report_failure(fixture, probe, &row);
            }
            file.append(&ResultRow::Probe(row))?;
        }
        if let Some(session) = judge.as_ref() {
            triage.push(judge::TriageRow {
                thread_id: thread.thread_id,
                arm: thread.arm,
                task: thread.task.clone(),
                judged: judge::judge_score(
                    session,
                    rubric(fixture, judge::RUBRIC_TRIAGE)?,
                    &world.final_response,
                )
                .await?,
                programmatic_pass: all_passed,
            });
        }
    }

    if let Some(session) = judge.as_ref() {
        if !judge::keeps_primary_standing(&judged, session.config) {
            println!(
                "[eval] the judged probes disagreed above the {:.0}% ceiling, so they leave \
                 the primary analysis.",
                session.config.disagreement_ceiling * 100.0
            );
        }
        let sample = judge::triage_sample(&triage, session.config);
        println!("[eval] human-read sample, {} threads:", sample.len());
        for thread_id in sample {
            println!("  {thread_id}");
        }
    }
    if skipped_judge > 0 {
        println!(
            "[eval] {skipped_judge} judged probes were not scored: pass --with-judge to run \
             them. The analysis is incomplete without them."
        );
        println!(
            "[eval] the disclaimer route was not judged either, so every unexplained failure \
             is recorded as lost-silent rather than lost-loud."
        );
    }
    Ok(())
}

fn rubric<'a>(fixture: &'a Fixture, name: &str) -> Fallible<&'a str> {
    fixture
        .judge
        .rubric
        .get(name)
        .map(String::as_str)
        .ok_or_else(|| format!("`[judge.rubric]` has no {name} entry").into())
}

fn report_failure(fixture: &Fixture, probe: &Probe, row: &ProbeRow) {
    let statement = probe
        .fact
        .as_deref()
        .and_then(|id| fixture.fact(id))
        .map(|fact| fact.statement.as_str())
        .unwrap_or("behaviour rather than a fact");
    println!(
        "[eval]   {} {} ({statement}); the tempting wrong answer is: {}",
        probe.id,
        row.outcome.as_str(),
        probe.wrong_default
    );
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TurnTwo {
    None,
    Recorded(i64),
    CountPrompts,
}

impl TurnTwo {
    fn of(recorded: Option<i64>, task_has_followup: bool) -> TurnTwo {
        match (recorded, task_has_followup) {
            (Some(sequence), _) => TurnTwo::Recorded(sequence),
            (None, true) => TurnTwo::CountPrompts,
            (None, false) => TurnTwo::None,
        }
    }

    fn just_driven(recorded: Option<i64>) -> TurnTwo {
        TurnTwo::of(recorded, false)
    }

    async fn resolve(self, pool: &PgPool, thread_id: uuid::Uuid) -> Fallible<Option<i64>> {
        match self {
            TurnTwo::None => Ok(None),
            TurnTwo::Recorded(sequence) => Ok(Some(sequence)),
            TurnTwo::CountPrompts => metrics::second_prompt_sequence(pool, thread_id).await,
        }
    }
}

async fn capture_run_inputs(paths: &Paths, fixture: &Fixture, run_id: &str) -> Fallible<()> {
    let ids = vec![run_id.to_string()];
    let labels = RunLabels::read(paths, &ids)?;
    let label = labels.of(run_id);
    let mut groups: BTreeMap<(Arm, u32), Vec<ThreadRow>> = BTreeMap::new();
    for thread in threads_of(paths, &ids)? {
        groups
            .entry((thread.arm, thread.repeat))
            .or_default()
            .push(thread);
    }
    for ((arm, repeat), threads) in groups {
        if let Some(saved) =
            snapshot::Snapshot::read(&paths.results_dir, run_id, label.as_str(), arm, repeat)?
        {
            if threads
                .iter()
                .any(|t| !saved.threads.contains_key(&t.thread_id))
            {
                return Err("published snapshot is missing run threads; arms retained".into());
            }
            continue;
        }
        let pool = arm_pool(paths, label, arm, repeat).await?;
        let data = workspace::arm_workspace_path(&paths.eval_root, label, arm, repeat).join("data");
        let mut saved = snapshot::Snapshot::new(run_id, label.as_str(), arm, repeat, &data)?;
        for thread in threads {
            let turn_two = TurnTwo::of(
                thread.followup_sequence,
                fixture
                    .task(&thread.task)
                    .is_some_and(|task| task.followup.is_some()),
            );
            let world = ThreadWorld::load(&pool, thread.thread_id, turn_two).await?;
            let rounds = replay::rounds(&pool, thread.thread_id).await?;
            saved
                .threads
                .insert(thread.thread_id, snapshot::ThreadInputs { world, rounds });
        }
        saved.calls = calls::by_purpose(&pool, &fixture.prices).await?;
        if arm.expects_tree_memory() {
            saved.tree = Some(calls::tree_nodes(&pool).await?);
        }
        saved.publish(&paths.results_dir)?;
    }
    Ok(())
}

#[derive(serde::Serialize, serde::Deserialize)]
struct ThreadWorld {
    tool_calls: Vec<assertions::ToolCall>,
    events: Vec<assertions::EventRow>,
    round_two_sequence: Option<i64>,
    followup_sequence: Option<i64>,
    final_response: String,
    questions: Vec<String>,
}

impl ThreadWorld {
    async fn load(
        pool: &PgPool,
        thread_id: uuid::Uuid,
        followup: TurnTwo,
    ) -> Fallible<ThreadWorld> {
        let events = metrics::workspace_events(pool).await?;
        Ok(ThreadWorld {
            tool_calls: metrics::tool_calls(pool, thread_id).await?,
            round_two_sequence: metrics::round_two_sequence(pool, thread_id).await?,
            followup_sequence: followup.resolve(pool, thread_id).await?,
            final_response: driver::final_response(pool, thread_id).await?,
            questions: events
                .iter()
                .filter(|event| event.event_type == "UserQuestionAsked")
                .map(|event| event.payload.clone())
                .collect(),
            events,
        })
    }
}

struct ScoringContext<'a> {
    fixture: &'a Fixture,
    world: &'a ThreadWorld,
    data_dir: &'a Path,
    thread: &'a ThreadRow,
    graph: &'a TaskGraph,
    failed: &'a BTreeSet<String>,
    judge: Option<&'a judge::Judge<'a>>,
}

async fn score_probe(
    ctx: &ScoringContext<'_>,
    probe: &Probe,
) -> Fallible<(ProbeRow, Option<judge::JudgedProbe>)> {
    let ScoringContext {
        fixture,
        world,
        data_dir,
        thread,
        graph,
        failed,
        judge,
    } = *ctx;
    let context = AssertionContext {
        data_dir,
        final_response: &world.final_response,
        tool_calls: &world.tool_calls,
        events: &world.events,
        round_two_sequence: world.round_two_sequence,
        followup_sequence: world.followup_sequence,
    };
    let mut verdict = None;
    let passed = match (&probe.assert, &probe.judge) {
        (Some(assertion), _) => assertion.evaluate(&context)?,
        (None, Some(name)) => match judge {
            Some(session) => {
                let subject = judge_subject(name, &world.events, &world.final_response);
                let mut votes = Vec::new();
                for variant in judge::shuffled_variants(&subject, session.config.votes) {
                    votes.push(judge::judge_vote(session, rubric(fixture, name)?, &variant).await?);
                }
                let tallied = judge::tally(&probe.id, &votes);
                let passed = probe.judge_passes_when.passed(tallied.yes);
                verdict = Some(tallied);
                passed
            }
            None => false,
        },
        (None, None) => false,
    };
    let fact = probe.fact.as_deref().and_then(|id| fixture.fact(id));
    let disclaimed = match (passed, judge) {
        (false, Some(session)) => {
            judge::judge_vote(
                session,
                rubric(fixture, judge::RUBRIC_RESPONSE_DISCLAIMS)?,
                &world.final_response,
            )
            .await?
            .yes
        }
        _ => false,
    };
    let outcome = probe::resolve(&probe::ProbeInputs {
        upstream_failed: graph.is_voided_by(&thread.task, failed) || !thread.finished(),
        passed,
        fact,
        questions: &world.questions,
        disclaimed,
    });
    Ok((
        ProbeRow {
            run_id: thread.run_id.clone(),
            repeat: thread.repeat,
            arm: thread.arm,
            task: thread.task.clone(),
            probe: probe.id.clone(),
            fact: probe.fact.clone(),
            tier: probe.tier.or(fact.map(|f| f.tier)),
            outcome,
        },
        verdict,
    ))
}

fn judge_subject(rubric: &str, events: &[assertions::EventRow], final_response: &str) -> String {
    if rubric == judge::RUBRIC_TRIGGER_PROCEDURE {
        let payload = events
            .iter()
            .filter(|event| event.event_type == "TriggerCreated")
            .map(|event| event.payload.as_str())
            .next_back()
            .unwrap_or_default();
        return trigger_intent(payload).unwrap_or_else(|| payload.to_string());
    }
    final_response.to_string()
}

fn trigger_intent(payload: &str) -> Option<String> {
    let parsed: serde_json::Value = serde_json::from_str(payload).ok()?;
    for prefix in [&parsed, parsed.get("data")?] {
        if let Some(intent) = prefix
            .get("run")
            .and_then(|run| run.get("intent"))
            .and_then(|intent| intent.as_str())
        {
            return Some(intent.to_string());
        }
    }
    None
}

fn print_report(analysis: &analyse::Analysis, trims: &Trims, calls: &[ArmCalls]) {
    println!(
        "Context-handling benchmark, {} run",
        analysis.config.as_str()
    );
    for label in &analysis.run_labels {
        println!(
            "  workspaces       eval-{label}-<arm>-<repeat>, in database \
             \"lucidos_eval-{label}-<arm>-<repeat>\""
        );
    }
    print_pairs(&analysis.pairs);
    print_empty_completions(&analysis.empty_completions);
    print_refusals(&analysis.refusals);

    for result in &analysis.configurations {
        print_configuration(result);
    }
    for sweep in &analysis.sweeps {
        print_sweep(sweep);
    }
    print_trims(trims);
    print_calls(calls);
    for comparison in &analysis.comparisons {
        print_comparison(comparison);
    }
    if analysis.comparisons.is_empty() && analysis.sweeps.is_empty() {
        println!(
            "\nOne configuration, so nothing is compared. Run a second window for a sweep, or \
             a second arm for a side by side."
        );
    }
}

struct ArmCalls {
    arm: Arm,
    repeat: u32,
    by_purpose: Option<Vec<calls::PurposeCalls>>,
    tree: Option<calls::TreeNodes>,
}

async fn calls_by_arm(
    paths: &Paths,
    run_ids: &[String],
    fixture: &Fixture,
) -> Fallible<Vec<ArmCalls>> {
    let labels = RunLabels::read(paths, run_ids)?;
    let mut one_thread: BTreeMap<(String, Arm, u32), uuid::Uuid> = BTreeMap::new();
    for thread in threads_of(paths, run_ids)? {
        one_thread
            .entry((thread.run_id, thread.arm, thread.repeat))
            .or_insert(thread.thread_id);
    }
    let mut arms = Vec::new();
    for ((run_id, arm, repeat), thread_id) in one_thread {
        let mut found = ArmCalls {
            arm,
            repeat,
            by_purpose: None,
            tree: None,
        };
        if let Some(saved) = snapshot::Snapshot::read(
            &paths.results_dir,
            &run_id,
            labels.of(&run_id).as_str(),
            arm,
            repeat,
        )? {
            if !saved.threads.contains_key(&thread_id) {
                return Err("snapshot is missing the reported thread".into());
            }
            found.by_purpose = Some(saved.calls);
            found.tree = saved.tree;
        } else if let Ok(pool) = arm_pool(paths, labels.of(&run_id), arm, repeat).await {
            if metrics::highest_sequence(&pool, thread_id).await? > 0 {
                found.by_purpose = Some(calls::by_purpose(&pool, &fixture.prices).await?);
                if arm.expects_tree_memory() {
                    found.tree = Some(calls::tree_nodes(&pool).await?);
                }
            }
        }
        arms.push(found);
    }
    Ok(arms)
}

fn print_calls(arms: &[ArmCalls]) {
    println!("\n== model calls, whole workspace ==");
    for arm in arms {
        println!("  {} arm, repeat {}", arm.arm, arm.repeat);
        let Some(groups) = &arm.by_purpose else {
            println!("    unreadable: a later run re-seeded this database");
            continue;
        };
        println!(
            "    {:<22} {:<24} {:>6} {:>10} {:>8} {:>11} {:>11} {:>8}",
            "purpose", "model", "calls", "fresh in", "out", "cache read", "cache write", "usd"
        );
        for group in groups {
            let input = group.counts.input_split();
            println!(
                "    {:<22} {:<24} {:>6} {:>10} {:>8} {:>11} {:>11} {:>8.2}",
                group.purpose,
                group.model,
                group.calls,
                input.fresh(),
                group.counts.output_tokens,
                input.cache_read(),
                input.cache_creation(),
                group.usd
            );
        }
        let auxiliary: Vec<&calls::PurposeCalls> = calls::auxiliary(groups).collect();
        println!(
            "    {} call(s), {} of them auxiliary at ${:.2}, ${:.2} in all",
            groups.iter().map(|g| g.calls).sum::<i64>(),
            auxiliary.iter().map(|g| g.calls).sum::<i64>(),
            auxiliary.iter().map(|g| g.usd).sum::<f64>(),
            groups.iter().map(|g| g.usd).sum::<f64>()
        );
        if let Some(tree) = arm.tree {
            println!(
                "    summary tree nodes   {} written by a model, {} stored verbatim",
                tree.written, tree.free
            );
        }
    }
}

fn print_configuration(result: &analyse::ConfigurationResult) {
    let quality = &result.quality;
    println!("\n== {} ==", result.configuration.label());
    println!(
        "  {} thread(s) over {} repeat(s)",
        result.threads, result.repeats
    );

    println!("  QUALITY");
    print_rate("    delivery", quality.delivery);
    print_rate("    fidelity", quality.fidelity);
    print_rate("    handover", quality.handover);
    let failures: Vec<String> = quality
        .failures
        .iter()
        .map(|(name, count)| format!("{name} {count}"))
        .collect();
    println!("    failures         {}", failures.join(", "));
    for (tier, rate) in &quality.by_tier {
        print_rate(&format!("    tier {tier}"), *rate);
    }

    let cost = &result.cost;
    println!("  COST");
    match cost.measured {
        true => println!(
            "    dollars          ${:.2} total, ${:.2} per task, ${:.3} per round",
            cost.usd, cost.usd_per_task, cost.usd_per_round
        ),
        false => println!(
            "    dollars          not measured: this model is priced at zero in prices.toml"
        ),
    }
    println!(
        "    by producer      ${:.2} main agent, ${:.2} auxiliary",
        cost.usd_main_agent(),
        cost.usd_auxiliary
    );
    println!(
        "    tokens           {} fresh in, {} out, {} cache read, {} cache write",
        cost.fresh_input, cost.output_tokens, cost.cache_read, cost.cache_creation
    );
    let auxiliary = cost.auxiliary_tokens.input_split();
    println!(
        "    of which aux     {} fresh in, {} out, {} cache read, {} cache write",
        auxiliary.fresh(),
        cost.auxiliary_tokens.output_tokens,
        auxiliary.cache_read(),
        auxiliary.cache_creation()
    );
    println!("    fresh per round  {:.0}", cost.fresh_input_per_round);
    println!(
        "    cache per round  {:.0} read, {:.0} write",
        cost.cache_read_per_round, cost.cache_creation_per_round
    );

    let rounds = &result.rounds;
    println!("  ROUNDS");
    println!(
        "    per task         {:.1} median, {} max, {} total",
        rounds.per_task_median, rounds.per_task_max, rounds.total
    );
    println!(
        "    re-fetches       {} recovery call(s) after round 1, {} of them repeats",
        rounds.recovery_calls, rounds.repeat_recoveries
    );

    let timing = &result.timing;
    println!("  WALL TIME");
    println!(
        "    per task         {:.0}s median, {}s max, {}s total",
        timing.per_task_median, timing.per_task_max, timing.total_secs
    );

    let used = &result.utilisation;
    println!("  CONTEXT UTILISATION");
    println!(
        "    window           {} tokens declared",
        used.context_window
    );
    println!(
        "    request size     {} peak, {:.0} mean",
        used.peak_tokens, used.mean_tokens
    );
    println!(
        "    at the peak      {} headroom, {:.1}% of the window used",
        used.headroom_at_peak,
        used.peak_share * 100.0
    );
    println!(
        "    trims            {} round(s) over {} thread(s)",
        used.trimmed_rounds, used.trimmed_threads
    );

    let document = &result.document;
    println!("  DOCUMENT");
    println!(
        "    writes           {} total, {:.1} per task",
        document.writes, document.writes_per_task
    );
    print_rate("    beside a call", document.beside_a_call);
    println!(
        "    held open        {} item(s), {:.1} per task",
        document.items_held_open, document.items_held_open_per_task
    );

    println!("  PER TASK");
    println!(
        "    {:<5} {:<9} {:<11} {:>6} {:>9} {:>7} {:>10} {:>6}",
        "task", "delivered", "fidelity", "rounds", "usd", "secs", "peak", "trims"
    );
    for task in &result.tasks {
        println!(
            "    {:<5} {:<9} {:<11} {:>6} {:>9.2} {:>7} {:>10} {:>6}",
            task.task,
            match task.delivered {
                Some(true) => "yes",
                Some(false) => "no",
                None => "void",
            },
            format!("{} of {}", task.fidelity.passed, task.fidelity.scored),
            task.rounds,
            task.usd,
            task.wall_secs,
            task.peak_tokens,
            task.trimmed_rounds,
        );
    }
}

fn print_trims(trims: &Trims) {
    println!("\n== trims by pass ==");
    println!(
        "  read {} of {} thread(s){}",
        trims.read,
        trims.threads,
        match trims.read < trims.threads {
            true => "   the rest were re-seeded away by a later run",
            false => "",
        }
    );
    if trims.by_pass.is_empty() {
        println!("  no trim fired in what could be read");
        return;
    }
    for (pass, rounds) in &trims.by_pass {
        println!(
            "  pass {pass}   {rounds} round(s){}",
            match pass {
                5 => "   the only pass that loses anything silently",
                _ => "",
            }
        );
    }
}

fn print_rate(label: &str, rate: analyse::Rate) {
    println!(
        "{label:<20} {:>6.1}%  ({} of {})",
        rate.rate * 100.0,
        rate.passed,
        rate.scored
    );
}

fn print_sweep(sweep: &analyse::Sweep) {
    println!("\n== budget sweep, {} arm ==", sweep.arm);
    println!(
        "  {:<10} {:>9} {:>9} {:>9} {:>7} {:>10} {:>7} {:>6} {:>6}",
        "window", "delivery", "fidelity", "usd", "rounds", "peak", "used", "trims", "holds"
    );
    for row in &sweep.rows {
        println!(
            "  {:<10} {:>8.1}% {:>8.1}% {:>9.2} {:>7} {:>10} {:>6.1}% {:>6} {:>6}",
            row.context_window,
            row.delivery * 100.0,
            row.fidelity * 100.0,
            row.usd,
            row.rounds,
            row.peak_tokens,
            row.peak_share * 100.0,
            row.trimmed_rounds,
            match row.holds {
                true => "yes",
                false => "no",
            },
        );
    }
    println!(
        "  judged against the {} window, within {:.0} points on delivery and fidelity",
        sweep.reference_window,
        analyse::QUALITY_TOLERANCE * 100.0
    );
    match sweep.smallest_holding {
        Some(window) => println!("  SMALLEST BUDGET THAT HELD: {window} tokens"),
        None => println!("  no smaller budget held quality"),
    }
}

fn print_comparison(comparison: &analyse::Comparison) {
    println!(
        "\n== {} against {} ==",
        comparison.right.label(),
        comparison.left.label()
    );
    println!(
        "  delivery           {:+.1} points",
        comparison.delivery_points
    );
    println!(
        "  fidelity           {:+.1} points",
        comparison.fidelity_points
    );
    println!("  dollars            {:+.2}", comparison.usd);
    println!("  rounds             {:+}", comparison.rounds);
    println!("  peak request       {:+} tokens", comparison.peak_tokens);
    println!(
        "  document writes    {:+}, {:+.1} points beside a call",
        comparison.document_writes, comparison.beside_a_call_points
    );
    println!("  items held open    {:+}", comparison.items_held_open);
    if !comparison.interleaved {
        println!(
            "  NOT INTERLEAVED    these two arms came from separate runs, so provider drift \
             lands on one side of every line above. Read the two absolute blocks instead."
        );
    }
}

fn print_pairs(pairs: &analyse::PairCensus) {
    println!(
        "  pairs kept         {} of {} ({} voided by an empty completion, {} by classifier \
         disagreement, {} by a scope divergence, {} by a failed task)",
        pairs.effective,
        pairs.attempted,
        pairs.empty_completion,
        pairs.classifier_disagreement,
        pairs.completion_divergence,
        pairs.upstream_failure
    );
}

fn print_empty_completions(empty: &analyse::RepostTally) {
    println!(
        "  empty completions  {} attempt(s) with no text and no tool call, {} re-posted",
        empty.turns, empty.retries
    );
    println!(
        "  empty recovered    {} thread(s) recovered by a re-post, {} never did",
        empty.recovered_threads, empty.unrecovered_threads
    );
    if empty.unrecovered_threads > 0 {
        println!(
            "  NOT A CLEAN RUN    {} thread(s) came back empty and never finished. Every \
             figure below covers the tasks that ran.",
            empty.unrecovered_threads
        );
    }
}

fn print_refusals(refusals: &[analyse::ArmRefusals]) {
    println!("  refusals           turns refused with no output, every attempt counted");
    for arm in refusals {
        let total = &arm.total;
        println!(
            "    {:<17}{} refused, {} re-posted, {} thread(s) recovered, {} never did",
            arm.arm.to_string(),
            total.turns,
            total.retries,
            total.recovered_threads,
            total.unrecovered_threads
        );
        for task in &arm.by_task {
            let tally = &task.tally;
            println!(
                "      {:<15}{} refused, {} re-posted, {} thread(s) recovered, {} never did",
                task.task,
                tally.turns,
                tally.retries,
                tally.recovered_threads,
                tally.unrecovered_threads
            );
        }
    }
}

fn engine_commit(repo_root: &Path) -> String {
    std::process::Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(repo_root)
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .unwrap_or_else(|| "unknown".to_string())
}

async fn trims_by_pass(paths: &Paths, run_ids: &[String]) -> Fallible<Trims> {
    let mut trims = Trims::default();
    let labels = RunLabels::read(paths, run_ids)?;
    for thread in threads_of(paths, run_ids)? {
        trims.threads += 1;
        let rounds = if let Some(mut saved) = snapshot::Snapshot::read(
            &paths.results_dir,
            &thread.run_id,
            labels.of(&thread.run_id).as_str(),
            thread.arm,
            thread.repeat,
        )? {
            saved
                .threads
                .remove(&thread.thread_id)
                .ok_or("snapshot is missing trim inputs")?
                .rounds
        } else {
            let Ok(pool) =
                arm_pool(paths, labels.of(&thread.run_id), thread.arm, thread.repeat).await
            else {
                continue;
            };
            replay::rounds(&pool, thread.thread_id).await?
        };
        if rounds.is_empty() {
            continue;
        }
        trims.read += 1;
        for (pass, count) in replay::trim_passes(&rounds) {
            *trims.by_pass.entry(pass).or_default() += count;
        }
    }
    Ok(trims)
}

#[derive(Default)]
struct Trims {
    by_pass: BTreeMap<u64, usize>,
    threads: usize,
    read: usize,
}

struct RunLabels(BTreeMap<String, workspace::RunLabel>);

impl RunLabels {
    fn read(paths: &Paths, run_ids: &[String]) -> Fallible<RunLabels> {
        Ok(RunLabels(
            read_runs(paths, run_ids)?
                .into_iter()
                .filter_map(|row| match row {
                    ResultRow::Run(run) => {
                        Some((run.run_id, workspace::RunLabel::recorded(&run.run_label)))
                    }
                    _ => None,
                })
                .collect(),
        ))
    }

    fn of(&self, run_id: &str) -> &workspace::RunLabel {
        static UNLABELLED: std::sync::OnceLock<workspace::RunLabel> = std::sync::OnceLock::new();
        self.0
            .get(run_id)
            .unwrap_or_else(|| UNLABELLED.get_or_init(|| workspace::RunLabel::recorded("")))
    }
}

fn threads_of(paths: &Paths, run_ids: &[String]) -> Fallible<Vec<ThreadRow>> {
    Ok(read_runs(paths, run_ids)?
        .into_iter()
        .filter_map(|row| match row {
            ResultRow::Thread(thread) => Some(thread),
            _ => None,
        })
        .collect())
}

async fn replay_command(
    paths: &Paths,
    run_id: &str,
    thread: Option<&str>,
    list: bool,
    options: &replay::Options<'_>,
) -> Fallible<()> {
    let rows = threads_of(paths, &[run_id.to_string()])?;
    let labels = RunLabels::read(paths, &[run_id.to_string()])?;
    if list || thread.is_none() {
        println!("{:<6} {:<8} {:<5} thread", "repeat", "arm", "task");
        for row in &rows {
            println!(
                "{:<6} {:<8} {:<5} {}",
                row.repeat,
                row.arm.as_str(),
                row.task,
                row.thread_id
            );
        }
        if !list {
            println!("\nname one with --thread, by id or by any prefix of it");
        }
        return Ok(());
    }
    let wanted = thread.unwrap_or_default();
    let matched: Vec<&ThreadRow> = rows
        .iter()
        .filter(|row| row.thread_id.to_string().starts_with(wanted))
        .collect();
    let row = match matched.as_slice() {
        [only] => *only,
        [] => return Err(format!("no thread of run {run_id} starts with {wanted}").into()),
        many => {
            return Err(format!(
                "{} threads of run {run_id} start with {wanted}. Name more of it.",
                many.len()
            )
            .into())
        }
    };
    let rounds = if let Some(mut saved) = snapshot::Snapshot::read(
        &paths.results_dir,
        &row.run_id,
        labels.of(&row.run_id).as_str(),
        row.arm,
        row.repeat,
    )? {
        saved
            .threads
            .remove(&row.thread_id)
            .ok_or("snapshot is missing replay inputs")?
            .rounds
    } else {
        let pool = arm_pool(paths, labels.of(&row.run_id), row.arm, row.repeat).await?;
        replay::rounds(&pool, row.thread_id).await?
    };
    let label = format!("{} {} repeat {}", row.arm, row.task, row.repeat);
    replay::print_rounds(&label, &rounds, options);
    Ok(())
}

fn read_by_id_available() -> bool {
    std::env::var("LUCIDOS_EVAL_READ_BY_ID")
        .map(|value| value == "1" || value.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}
