//! The context-handling benchmark (ADR 0110).
//!
//! One configuration at a time, scored on five absolute axes. `analyse` and
//! `report` pool several runs, which is how a budget sweep is read.
//!
//! A binary and never a test. `make test` would otherwise pick the eval up and
//! spend four figures on a lint fix, which is ADR 0087 decision 15.
//! `scripts/check-eval-not-a-test.sh` enforces that in `make lint`.
//!
//! Run it through `scripts/eval-context-mode.sh`, which resolves the engine
//! binary, the eval root and the database base for you.

mod analyse;
mod arm;
mod assertions;
mod config;
mod driver;
#[cfg(feature = "fixture-gen")]
mod fixture_gen;
mod gateway;
mod guidance;
mod judge;
mod manipulation;
mod metrics;
mod probe;
mod replay;
mod results;
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

/// Where the handover the census scores is expected to be.
const HANDOVER_PATH: &str = "artifacts/build-health/HANDOVER.md";

#[derive(Parser)]
#[command(
    name = "lucidos-eval",
    about = "The ADR 0110 context-handling benchmark"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

/// The arms a run measures, when nothing says otherwise (ADR 0110 decision 1).
///
/// One configuration is the shape. Naming two still interleaves them and still
/// voids a divergent pair, so a baseline stays one flag away.
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
        /// Resume into an existing results file rather than starting one.
        #[arg(long)]
        run_id: Option<String>,
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
    /// Re-embed the seeded memory rows from `memory-seed.toml`.
    #[cfg(feature = "fixture-gen")]
    GenerateMemoryFixture,
}

/// Everything the harness resolves from its environment, in one place.
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
        let repo_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(Path::parent)
            .ok_or("the crate must sit two levels below the repository root")?
            .to_path_buf();
        let fixture_root = repo_root.join("eval/context-mode");
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
                repeat,
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
                    repeats,
                    arms: selected_arms(&arms)?,
                    window: checked_window(window)?,
                    sweep: SweepPins::from_env()?,
                },
                tasks,
                run_id,
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
        Command::Report { run_id } => {
            let paths = Paths::resolve()?;
            let analysis = analyse::analyse(&read_runs(&paths, &run_id)?)?;
            print_report(&analysis, &trims_by_pass(&paths, &run_id).await?);
            Ok(())
        }
    }
}

/// What a run measures, beside the fixture: the arms, the repeats, the window.
struct RunShape {
    config: RunConfig,
    repeats: u32,
    arms: Vec<Arm>,
    window: Option<i64>,
    /// The schedule the lean arm sweeps at, resolved once for the whole run.
    sweep: SweepPins,
}

/// The arms named on the command line, in the order they were named.
///
/// Order is the interleave order, so it is preserved rather than sorted. A
/// duplicate is refused instead of silently deduplicated: it would seed the
/// same workspace twice and double every count read off it.
fn selected_arms(names: &[String]) -> Fallible<Vec<Arm>> {
    let mut arms: Vec<Arm> = Vec::new();
    for name in names {
        let known = Arm::BOTH
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

/// Refuse a window whose message budget collapses (ADR 0110 decision 11).
///
/// The tools array and the system prompt are roughly 85,000 chars, and the
/// engine's budget is `(window - 8000) * 1.5` chars. Below about 64,000 tokens
/// the subtraction saturates to zero, so every round ships over budget however
/// hard the trimmer works. Such a run measures the fixed overhead.
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

/// Everything that must hold before a single token is spent.
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

/// The model under test when nothing pins one.
///
/// Named because the run label defaults to it too, and the two must not drift.
const DEFAULT_MODEL: &str = "claude-opus-5@default";

/// The pins ADR 0087's precondition P5 asks for, plus the declared window.
fn seed_pins(window: Option<i64>) -> workspace::SeedPins<'static> {
    workspace::SeedPins {
        model: leak_env("LUCIDOS_EVAL_MODEL", DEFAULT_MODEL),
        model_label: leak_env("LUCIDOS_EVAL_MODEL_LABEL", "Model under test"),
        model_provider: leak_env("LUCIDOS_EVAL_MODEL_PROVIDER", "vertex"),
        reasoning_effort: leak_env("LUCIDOS_EVAL_REASONING_EFFORT", "default"),
        context_window: window,
    }
}

/// What separates this run's workspaces from a concurrent run's.
///
/// `LUCIDOS_EVAL_RUN_LABEL` when an operator sets one, else the model pin. The
/// model default covers cross-provider runs with no configuration at all. The
/// override is for two runs of the SAME model that differ on another axis, such
/// as two windows of a budget sweep started together.
fn run_label() -> &'static workspace::RunLabel {
    static LABEL: std::sync::OnceLock<workspace::RunLabel> = std::sync::OnceLock::new();
    LABEL.get_or_init(|| {
        let source = set_env("LUCIDOS_EVAL_RUN_LABEL")
            .or_else(|| set_env("LUCIDOS_EVAL_MODEL"))
            .unwrap_or_else(|| DEFAULT_MODEL.to_string());
        workspace::RunLabel::derive(&source)
    })
}

/// A variable an operator actually set, treating blank as unset.
///
/// `env::var` answers `Ok("")` for an exported-but-empty variable, so a bare
/// `LUCIDOS_EVAL_RUN_LABEL=` would otherwise be a label: the digest of the
/// empty string, naming databases that say nothing about which model wrote
/// them. Every other reader here already treats blank as absent.
fn set_env(key: &str) -> Option<String> {
    std::env::var(key)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

/// Every model this run and its siblings put under test.
///
/// A run knows only its own pin, and runs go in parallel now. So the operator
/// declares the whole set in `LUCIDOS_EVAL_MODEL_SET`, and the judge is checked
/// against all of it. Unset, the set is this run's own model.
///
/// This run's own model is always in the set, declared or not. A set that
/// forgot it would let the judge be the very model it is grading.
fn models_under_test(model: &str) -> Vec<String> {
    // Empty only when a results file recorded no model, which no run writes.
    // Carrying it would put a nameless entry in the set the judge is checked
    // against.
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

/// State what this run pins, before it costs anything.
///
/// The window is the one that decides whether a cross-provider comparison means
/// anything. Left undeclared, the engine infers it per model, and `gpt-5.6`
/// infers far above Opus. The ceiling tasks then measure two different budgets
/// and report one number.
fn announce_run_shape(under_test: &[String], window: Option<i64>, sweep: SweepPins) {
    if under_test.len() > 1 {
        println!("[eval] models under test, this run and its siblings:");
        for model in under_test {
            println!("[eval]   {model}");
        }
    }
    // Printed rather than left to the preference rows, because the schedule is
    // what the lean arm is being measured at. A reader of the log can tell one
    // window of a budget sweep from one step of a schedule sweep.
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

/// Read a pin once and keep it for the process. The pins never change mid-run.
fn leak_env(key: &str, fallback: &'static str) -> &'static str {
    match std::env::var(key) {
        Ok(value) => Box::leak(value.into_boxed_str()),
        Err(_) => fallback,
    }
}

/// Connect to one arm's database.
///
/// The label is passed in rather than read from the environment. A post-run
/// command reads it off the results file, so `score` in a shell pinned to one
/// model cannot open another model's arms.
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

/// Seed every named arm of one repeat, and prove they came out identical (I1).
///
/// One arm has nothing to compare against, so the digest is computed and not
/// asserted. It still costs nothing and it still lands in the log, which is
/// what makes a seeding difference visible when a second arm joins later.
async fn seed_repeat(
    paths: &Paths,
    repeat: u32,
    arms: &[Arm],
    window: Option<i64>,
    sweep: SweepPins,
) -> Fallible<()> {
    let pins = seed_pins(window);
    let tls = workspace::EngineTls::from_env()?;
    let mut digests = Vec::new();
    for arm in arms.iter().copied() {
        let slug = workspace::arm_workspace_name(run_label(), arm, repeat);
        let ws = workspace::arm_workspace_path(&paths.eval_root, run_label(), arm, repeat);
        workspace::checked_eval_workspace(&ws)?;
        std::fs::create_dir_all(&ws)?;
        let database = workspace::arm_database_name(run_label(), arm, repeat);
        let url = workspace::arm_database_url(&paths.pg_base, run_label(), arm, repeat);
        // Both names, because the knowhow tells an operator to open the
        // database by hand and the label makes them run-specific.
        println!("[eval] {slug} seeds into database \"{database}\"");
        workspace::recreate_database(&paths.pg_base, &database)?;
        // The tree goes in BEFORE the first boot. The engine adopts `data/`
        // into the workspace repo as it comes up. Replacing the directory
        // afterwards would leave every fixture file an uncommitted delete.
        workspace::install_fixture_tree(&ws, &paths.fixture_root.join(config::SEED_TREE))?;
        workspace::migrate_by_booting(&paths.engine_bin, &ws, &url, &tls)?;
        workspace::apply_seed_sql(&url, &paths.fixture_root.join("fixtures/seed.sql"), &pins)?;
        let pool = arm_pool(paths, run_label(), arm, repeat).await?;
        workspace::write_arm_preference(&pool, arm, sweep).await?;
        let rows = workspace::read_seed_rows(&pool).await?;
        digests.push(workspace::SeedDigest {
            db: workspace::digest_rows(&rows),
            fs: workspace::fs_digest(&ws)?,
        });
        // Last, and best-effort. A seeded arm is browsable from here on, and a
        // gateway that is down costs the browsing and nothing else.
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

/// What the picker calls one arm.
///
/// It says which run, which arm and which repeat. The picker's list is the only
/// place these appear together, and one character of difference at a glance is
/// what the reader gets.
///
/// The label is in it because two providers run at once now. Their slugs
/// differ, so the picker lists both, and without the label here it lists them
/// under one identical name.
fn arm_display_name(label: &workspace::RunLabel, arm: Arm, repeat: u32) -> String {
    format!("Eval {label}: {arm} arm, repeat {repeat}")
}

/// Seed and drive one or more repeats, arms interleaved per task (I7).
async fn run_command(
    paths: &Paths,
    fixture: &Fixture,
    shape: RunShape,
    tasks: Option<Vec<String>>,
    run_id: Option<String>,
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
        // Zero when nothing was declared, which is the model's own window. The
        // analysis then reads what the engine actually resolved off the
        // threads, so the configuration is still labelled correctly.
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
        None => file.append(&ResultRow::Run(this_run))?,
    }

    for repeat in 1..=shape.repeats {
        if done.contains(&repeat) {
            println!("[eval] repeat {repeat} is already complete, skipping");
            continue;
        }
        run_repeat(paths, fixture, &file, &run_id, repeat, &selected, &shape).await?;
    }
    println!("[eval] results at {}", file.path().display());
    Ok(())
}

/// One repeat: every named arm, every task, interleaved.
async fn run_repeat(
    paths: &Paths,
    fixture: &Fixture,
    file: &ResultsFile,
    run_id: &str,
    repeat: u32,
    tasks: &[&Task],
    shape: &RunShape,
) -> Fallible<()> {
    seed_repeat(paths, repeat, &shape.arms, shape.window, shape.sweep).await?;
    let mut engines = Vec::new();
    let mut endpoints = Vec::new();
    let booted = boot_arms(paths, repeat, &shape.arms, &mut engines, &mut endpoints).await;

    // The engines are stopped whichever way the repeat ends, a half-boot
    // included. A manipulation failure aborts the repeat by design. A leaked
    // engine keeps writing to a workspace the next seed clears, and holds a
    // database that seed force-drops.
    let outcome = match booted {
        Ok(()) => drive_repeat(fixture, file, run_id, repeat, tasks, &endpoints).await,
        Err(error) => Err(error),
    };
    let stop_failures: Vec<String> = engines
        .into_iter()
        .filter_map(|engine| workspace::stop_engine(engine).err())
        .map(|error| error.to_string())
        .collect();
    // The repeat's own error wins. A failure to kill would otherwise hide why
    // the repeat aborted.
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

/// Boot an engine per arm and connect to it, leaving what booted in `engines`.
///
/// The caller stops them, so an error partway through does not leak the engine
/// that already started.
async fn boot_arms(
    paths: &Paths,
    repeat: u32,
    arms: &[Arm],
    engines: &mut Vec<workspace::BootedEngine>,
    endpoints: &mut Vec<(Arm, PathBuf, ArmEndpoint)>,
) -> Fallible<()> {
    let tls = workspace::EngineTls::from_env()?;
    for arm in arms.iter().copied() {
        let ws = workspace::arm_workspace_path(&paths.eval_root, run_label(), arm, repeat);
        let url = workspace::arm_database_url(&paths.pg_base, run_label(), arm, repeat);
        let port = claim_arm_port(arm, repeat).await?;
        // Full `ContextCaptured` bodies, always. A run whose captures are
        // truncated at 8 KB cannot be replayed, and replaying it is half of
        // what the benchmark is for.
        let engine = workspace::boot_engine(&paths.engine_bin, &ws, &url, port, &tls, true)?;
        let base_url = engine.base_url.clone();
        engines.push(engine);
        let pool = arm_pool(paths, run_label(), arm, repeat).await?;
        endpoints.push((arm, ws, ArmEndpoint::connect(&base_url, pool).await?));
    }
    Ok(())
}

/// Take the port this arm's engine binds, and clear whatever sat on it.
///
/// The registry holds a port once the seed registered the arm. Binding THAT one
/// is what makes `/eval-lean-1/` live during the run: the gateway probes the
/// port before it spawns, finds this engine, and adopts it. An unregistered arm
/// takes a free port and is simply not browsable.
///
/// The release is what lets a run follow a browse. Browsing lazy-starts a
/// gateway-owned engine on the same port, and nothing else stops it.
async fn claim_arm_port(arm: Arm, repeat: u32) -> Fallible<u16> {
    let slug = workspace::arm_workspace_name(run_label(), arm, repeat);
    let Some(port) = gateway::registered_port(&slug).await else {
        return workspace::free_port();
    };
    gateway::release_arm(&slug).await;
    // The gateway signals its engine and reaps it on another thread, so the
    // port is still held when the call returns. Booting now would just fail to
    // bind. A timeout is not fatal: `boot_engine` then reports the occupied
    // port itself, in the same breath as any other cause.
    if !workspace::wait_for_port_free(port, ARM_PORT_RELEASE_TIMEOUT) {
        println!("[eval] port {port} is still held after asking the gateway to release {slug}");
    }
    println!("[eval] {slug} boots on its registered port {port}");
    Ok(port)
}

/// How long to wait for a released arm's port. A graceful engine drain is
/// seconds, and past that the occupied port is worth reporting rather than
/// waiting on.
const ARM_PORT_RELEASE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// Every task of one repeat, in both arms, interleaved per task.
async fn drive_repeat(
    fixture: &Fixture,
    file: &ResultsFile,
    run_id: &str,
    repeat: u32,
    tasks: &[&Task],
    endpoints: &[(Arm, PathBuf, ArmEndpoint)],
) -> Fallible<()> {
    let graph = TaskGraph::new(&fixture.tasks);
    // A failure set per arm, because two arms are two worlds. Voiding T05 in
    // the lean arm because T02 failed in the control arm would throw away a
    // measurement the lean arm could still make. Keyed on the arms that ran,
    // so a single-configuration run carries exactly one set.
    let mut failed: BTreeMap<Arm, BTreeSet<String>> = endpoints
        .iter()
        .map(|(arm, _, _)| (*arm, BTreeSet::new()))
        .collect();
    for task in tasks {
        // Look the completion probe up once. Neither the scorer nor the row
        // writer should have to cope with a task the loader already guaranteed
        // carries one.
        let completion = fixture.completion_for(&task.id).ok_or_else(|| {
            format!(
                "task {} has no completion probe, which the loader should have refused",
                task.id
            )
        })?;
        let mut retrieved: BTreeMap<Arm, bool> = BTreeMap::new();
        let mut delivered: BTreeMap<Arm, CompletionOutcome> = BTreeMap::new();
        // Only the arms that ran get an entry, so a blocked arm cannot be read
        // as one that answered.
        let mut ran_empty: BTreeMap<Arm, bool> = BTreeMap::new();
        // Arms whose void probe rows are already on disk. A blocked arm writes
        // its own below, and the pair verdict must not write a second set over
        // it: that would double every count the probe census reads.
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
            // Snapshot after the thread stops writing. Title, memory and
            // summary work runs detached, so collecting the moment the driver
            // leaves records whichever of them had landed.
            let event_log_settled =
                driver::wait_until_settled(&endpoint.pool, driven.thread_id).await?;
            if !event_log_settled {
                println!(
                    "[eval]   {} was still writing events when the settle bound arrived, so \
                     its counts may be short",
                    task.id
                );
            }
            // The whole table, not one row. A thread bills the agent on the
            // model under test. Its title and memory work bill the auxiliary
            // default, and each is priced at its own rate.
            let metrics =
                metrics::collect(&endpoint.pool, driven.thread_id, &fixture.prices).await?;
            gate_manipulation(*arm, &endpoint.pool, driven.thread_id).await?;
            let handover_lines = handover_line_count(task, workspace_dir);
            retrieved.insert(*arm, metrics.memory_recalled);
            let ended_empty = driven.status == driver::TaskStatus::Empty;
            ran_empty.insert(*arm, ended_empty);
            // Scored HERE, against the workspace as this task left it. Every
            // later task rewrites the tree, so asking after the run would read
            // a world the task never saw.
            //
            // A thread that never ran is not asked at all. Its deliverable is
            // missing because nothing was attempted, and recording that as a
            // delivery failure is the false signal this void exists to stop.
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
                event_log_settled,
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

/// Did this arm deliver the task, read against the tree the task just left.
///
/// A probe with no assertion is a harness failure rather than a void. The
/// loader guarantees one, so reaching this means the fixture and the scorer
/// disagree, and recording that as "measured nothing" would hide it.
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

/// What both arms made of one task, as the pair verdict reads it.
struct PairOutcomes<'a> {
    /// Whether each arm's engine retrieved memory. Arms that ran only.
    retrieved: &'a BTreeMap<Arm, bool>,
    /// Whether each arm delivered, a blocked arm's `Void` included.
    delivered: &'a BTreeMap<Arm, CompletionOutcome>,
    /// Whether each arm's thread never produced anything. Arms that ran only.
    ran_empty: &'a BTreeMap<Arm, bool>,
    /// Arms whose void probe rows the run loop already wrote.
    probes_voided: &'a BTreeSet<Arm>,
}

/// Everything decided once every arm has run this task.
///
/// Three causes can void the pair, and each arm gets ONE set of void probe rows
/// however many of them fired. A second set doubles every count the probe
/// census reads. So an arm the run loop already voided is skipped by name,
/// rather than by a proxy for it.
///
/// Two of the three need two arms and cannot fire on a single-configuration
/// run. The third can: an arm whose every attempt came back empty never started
/// the task, whether or not anything ran beside it.
///
/// The completion rows are written here rather than in the arm loop, because a
/// classifier disagreement and an empty thread both void them, and both are
/// only known now. A divergence does not: those outcomes are what the
/// divergence is evidence of.
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
        // The arms this run measured, and never `Arm::BOTH`. Every arm that
        // reached this point has a delivery entry, blocked ones included. A
        // fixed pair would write void probe rows for an arm nobody ran.
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
            // A disagreement means the arms met different engines, so their
            // delivery is no more comparable than their retention. An empty
            // thread is worse: one arm was never asked, so the arm that did
            // deliver has nothing to be compared against.
            outcome: match classifier_disagreed || ran_empty {
                true => CompletionOutcome::Void,
                false => *outcome,
            },
        }))?;
    }
    Ok(())
}

/// Why this task cannot measure anything in this arm, if it cannot (I3).
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

/// T12's handover length, recorded as a covariate on the thread row.
///
/// The prompt caps it at 60 lines so a long document cannot score by accident.
/// Recording the real count is what lets a reader check that it held.
fn handover_line_count(task: &Task, workspace_dir: &Path) -> Option<i64> {
    if task.id != analyse::CENSUS_TASK {
        return None;
    }
    let path = workspace_dir.join("data").join(HANDOVER_PATH);
    std::fs::read_to_string(path)
        .ok()
        .map(|text| text.lines().count() as i64)
}

/// Every probe of a task that never ran, recorded as void rather than absent.
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

/// I2, per thread. A violation aborts the repeat rather than being recorded.
async fn gate_manipulation(arm: Arm, pool: &PgPool, thread_id: uuid::Uuid) -> Fallible<()> {
    let rounds = manipulation::captured_rounds(pool, thread_id).await?;
    let starts = manipulation::exchange_starts(pool, thread_id).await?;
    manipulation::check(arm, &manipulation::place_rounds(&rounds, &starts))
}

fn task_answers<'a>(fixture: &'a Fixture, task: &str) -> Option<&'a config::TaskAnswers> {
    fixture.answers.iter().find(|entry| entry.task == task)
}

fn thread_row(
    run_id: &str,
    repeat: u32,
    arm: Arm,
    driven: &DrivenTask,
    metrics: &metrics::ThreadMetrics,
    handover_lines: Option<i64>,
    event_log_settled: bool,
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
        event_log_settled: Some(event_log_settled),
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
        followup_sequence: driven.followup_sequence,
    }
}

/// Resolve every probe of a finished run against the arms' databases.
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
    // Here too, not only in `run`. This is where the judge actually spends
    // tokens, and it is a separate invocation, so a set declared only now would
    // otherwise never be consulted. The model comes off the file rather than
    // the environment, for the same reason the label does.
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
    // Already-scored probes are skipped rather than appended again. The file is
    // append-only, so a second `score` would double every count the analysis
    // reads. This is the same resumability contract the run loop has.
    let already_scored: BTreeSet<(u32, Arm, String)> = rows
        .iter()
        .filter_map(|row| match row {
            ResultRow::Probe(probe) => Some((probe.repeat, probe.arm, probe.probe.clone())),
            _ => None,
        })
        .collect();
    // Built before the first probe, so a missing credential fails the command
    // rather than a probe halfway down the append-only file.
    let judge = match with_judge {
        true => Some(judge::Judge::connect(&fixture.judge).await?),
        false => None,
    };
    let graph = TaskGraph::new(&fixture.tasks);
    // Per repeat and arm, because the two arms are two worlds and a repeat is
    // its own sequence run. A pooled set would void a task in every arm
    // because one arm flaked.
    let mut failed: BTreeMap<(u32, Arm), BTreeSet<String>> = BTreeMap::new();
    for thread in threads.iter().filter(|t| !t.finished()) {
        failed
            .entry((thread.repeat, thread.arm))
            .or_default()
            .insert(thread.task.clone());
    }
    // The run loop already recorded these pairs void. This loop skips their
    // threads whole, so a pair that measured the classifier or a scope
    // difference reaches neither the judge nor the triage sample.
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
    // Read off the file, so scoring opens the databases this run wrote.
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
        let pool = arm_pool(paths, label, thread.arm, thread.repeat).await?;
        let data_dir =
            workspace::arm_workspace_path(&paths.eval_root, label, thread.arm, thread.repeat)
                .join("data");
        let mut applicable: Vec<Probe> = fixture.probes_for(&thread.task).cloned().collect();
        if thread.task == analyse::CENSUS_TASK {
            applicable.extend(census.iter().cloned());
        }
        // Read once per thread rather than once per probe. Both are whole-scan
        // queries, and T10 alone can carry sixty rounds of rows.
        let turn_two = TurnTwo::of(
            thread.followup_sequence,
            fixture
                .task(&thread.task)
                .is_some_and(|task| task.followup.is_some()),
        );
        let world = ThreadWorld::load(&pool, thread.thread_id, turn_two).await?;
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

/// Print what a failing probe was reaching for, and what it fell to instead.
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

/// Where a thread's turn-two boundary comes from.
///
/// It is the driver's own record wherever there is one. A re-posted turn leaves
/// several prompts on the thread. Counting them can then land on a turn-one
/// attempt, and read most of turn one as turn two.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TurnTwo {
    /// The task ran one turn, so there is no boundary.
    None,
    /// The driver recorded it while it drove the turn.
    Recorded(i64),
    /// A run recorded before the driver wrote it down, on a task the fixture
    /// says has a follow-up. Those runs posted one prompt per turn, so the
    /// second prompt is the boundary.
    CountPrompts,
}

impl TurnTwo {
    /// What a thread row and its task say the boundary is.
    fn of(recorded: Option<i64>, task_has_followup: bool) -> TurnTwo {
        match (recorded, task_has_followup) {
            (Some(sequence), _) => TurnTwo::Recorded(sequence),
            (None, true) => TurnTwo::CountPrompts,
            (None, false) => TurnTwo::None,
        }
    }

    /// The boundary of a task the driver has just finished driving. It records
    /// one whenever there was a turn two, so nothing needs counting.
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

/// One thread's rows, read once and shared by every probe on it.
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

/// Everything scoring a probe needs that is the same for every probe of one
/// thread. Built once per thread, so the scorer below takes it and the single
/// probe under test.
struct ScoringContext<'a> {
    fixture: &'a Fixture,
    world: &'a ThreadWorld,
    data_dir: &'a Path,
    thread: &'a ThreadRow,
    graph: &'a TaskGraph,
    /// Tasks the thread's own arm and repeat already failed, which void the
    /// probes downstream of them.
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
        // The fixture guarantees exactly one scorer (I9), so the remaining
        // shapes are a judged probe and nothing else.
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

/// What the judge is shown for a rubric. Never the arm, and never the probe id.
///
/// The procedure rubric is shown the trigger's intent, not its whole payload.
/// The rubric asks about prose, and the judge's variants rotate lines, which
/// would leave a JSON blob unparseable and the question unanswerable.
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

/// The `run.intent` string out of a `TriggerCreated` payload.
///
/// `None` when the payload has another shape, which leaves the caller showing
/// the whole thing rather than showing the judge nothing.
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

/// The one-page human summary: absolute numbers first, comparisons last.
///
/// A reader should be able to answer "what did this run do" from the first
/// block, without holding a second configuration in their head. ADR 0110
/// decision 2.
fn print_report(analysis: &analyse::Analysis, trims: &Trims) {
    println!(
        "Context-handling benchmark, {} run",
        analysis.config.as_str()
    );
    // Where the arms ran. Reading a thread by hand means opening its database,
    // and the label is what makes that name run-specific.
    for label in &analysis.run_labels {
        println!(
            "  workspaces       eval-{label}-<arm>-<repeat>, in database \
             \"lucidos_eval-{label}-<arm>-<repeat>\""
        );
    }
    print_pairs(&analysis.pairs);
    print_empty_completions(&analysis.empty_completions);

    for result in &analysis.configurations {
        print_configuration(result);
    }
    for sweep in &analysis.sweeps {
        print_sweep(sweep);
    }
    print_trims(trims);
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

/// One configuration's five axes, then its per-task table.
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
        // A model under test priced at zero produces a real 0.00 rather than a
        // cheap run. Saying which is the difference between a figure and a
        // blank. The auxiliary line below still prints, because that model may
        // well be priced.
        false => println!(
            "    dollars          not measured: this model is priced at zero in prices.toml"
        ),
    }
    // Each model at its own rate, so the two are not one number. Auxiliary work
    // is the title, memory and summary calls the thread caused, which run on
    // the extractor default rather than the model under test.
    println!(
        "    by producer      ${:.2} main agent, ${:.2} auxiliary",
        cost.usd_main_agent(),
        cost.usd_auxiliary
    );
    // Fresh, read and write are disjoint. Fresh is what is left once the two
    // cached counts come out of the total the provider reports.
    println!(
        "    tokens           {} fresh in, {} out, {} cache read, {} cache write",
        cost.fresh_input, cost.output_tokens, cost.cache_read, cost.cache_creation
    );
    // The same four counts, so the line above can be read against this one.
    // Printing the stored input total here instead would put a number carrying
    // its cached tokens beside a number with them taken out.
    let auxiliary = cost.auxiliary_tokens.input_split();
    println!(
        "    of which aux     {} fresh in, {} out, {} cache read, {} cache write",
        auxiliary.fresh(),
        cost.auxiliary_tokens.output_tokens,
        auxiliary.cache_read(),
        auxiliary.cache_creation()
    );
    println!("    fresh per round  {:.0}", cost.fresh_input_per_round);
    // Apart, and never summed. A read is 0.1x and a write is 1.25x. So a
    // configuration re-creating its array every round can cost several times
    // one that extends it, while both report similar token totals.
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
    // The mode's own claim, as one rate. A write beside a tool call rode along
    // with a round the thread was taking anyway. A write alone cost a round.
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

/// Which trim passes fired, and how often, over every captured round.
///
/// The utilisation axis says how many rounds trimmed. This says which passes
/// did it, which is the part that decides whether anything was lost silently.
/// Only pass 5 removes a message; every pass above it leaves a stub.
fn print_trims(trims: &Trims) {
    println!("\n== trims by pass ==");
    println!(
        "  read {} of {} thread(s){}",
        trims.read,
        trims.threads,
        match trims.read < trims.threads {
            // Said plainly, because the alternative is a table that looks
            // complete. A re-seeded arm database takes the earlier runs with it.
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

/// A rate with its sample beside it, never a bare proportion.
fn print_rate(label: &str, rate: analyse::Rate) {
    println!(
        "{label:<20} {:>6.1}%  ({} of {})",
        rate.rate * 100.0,
        rate.passed,
        rate.scored
    );
}

/// The quality-and-cost curve against budget, and the number it exists for.
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

/// Two configurations side by side, in differences.
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

/// How many pairs measured anything, and what removed the rest.
///
/// Only a two-arm run can void a pair, so a single-configuration run prints
/// this with every cause at zero. That is a measurement rather than a gap.
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

/// Turns that came back with nothing, printed beside the pair census.
///
/// Always printed, so a zero is a measurement rather than a missing line. The
/// warning is what stops an unrecovered one reading as a clean run.
fn print_empty_completions(empty: &analyse::EmptyCompletions) {
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

/// Every captured round's trim passes, pooled over the runs being reported.
///
/// A run whose arm database is unreachable contributes nothing rather than
/// failing the report. An older run contributes nothing either: its rows carry
/// no `trim_passes`, and reading that as "no pass fired" would be a claim they
/// never made.
async fn trims_by_pass(paths: &Paths, run_ids: &[String]) -> Fallible<Trims> {
    let mut trims = Trims::default();
    // Per run, because a pooled sweep can carry a different label per run.
    let labels = RunLabels::read(paths, run_ids)?;
    for thread in threads_of(paths, run_ids)? {
        trims.threads += 1;
        let Ok(pool) = arm_pool(paths, labels.of(&thread.run_id), thread.arm, thread.repeat).await
        else {
            continue;
        };
        let rounds = replay::rounds(&pool, thread.thread_id).await?;
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

/// Which trim passes fired, and over how much of the run the reader could see.
///
/// The coverage is not decoration. An arm's database is force-dropped and
/// re-seeded by the NEXT run, and every run of one arm and repeat uses the same
/// database name. So pooling a sweep leaves only the last run's threads in
/// place, and the counts silently cover one run out of four.
#[derive(Default)]
struct Trims {
    by_pass: BTreeMap<u64, usize>,
    /// Threads the results files name.
    threads: usize,
    /// Threads whose rounds were still readable.
    read: usize,
}

/// Every thread row of the named runs, in the order they were driven.
/// Which label named each pooled run's workspaces, keyed by run id.
///
/// A post-run command reads the label off the file rather than off its own
/// environment. Two reasons, and both are silent when they go wrong. A `score`
/// run from a shell pinned to another model would open that model's arms. And
/// `report` pools a sweep, whose runs can carry different labels. One label for
/// every thread then reads the wrong database for all but one of them.
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

    /// A run with no row of its own falls back to the unlabelled name. That is
    /// what a file recorded before the label existed actually created.
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

/// Replay one thread out of its own arm's database, or list what ran.
///
/// The thread is named by id or by any prefix, because nobody types a uuid.
/// An ambiguous prefix is refused rather than resolved to the first match: a
/// replay of the wrong thread reads exactly like a replay of the right one.
async fn replay_command(
    paths: &Paths,
    run_id: &str,
    thread: Option<&str>,
    list: bool,
    options: &replay::Options<'_>,
) -> Fallible<()> {
    let rows = threads_of(paths, &[run_id.to_string()])?;
    // Read off the file, so the replay opens the database this run wrote.
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
    let pool = arm_pool(paths, labels.of(&row.run_id), row.arm, row.repeat).await?;
    let rounds = replay::rounds(&pool, row.thread_id).await?;
    let label = format!("{} {} repeat {}", row.arm, row.task, row.repeat);
    replay::print_rounds(&label, &rounds, options);
    Ok(())
}

/// Precondition P3: whether `query_events` can read an event by id.
///
/// Recorded rather than enforced. Without it a noted pointer resolves as a
/// newest-first window, so the run is legitimate as a pilot and never as the
/// confirmatory.
fn read_by_id_available() -> bool {
    std::env::var("LUCIDOS_EVAL_READ_BY_ID")
        .map(|value| value == "1" || value.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}
