//! Load and validate the checked-in fixture: tasks, facts, probes, completion
//! probes, answers and prices.
//!
//! Everything the measurement depends on is a file, and every file is hashed
//! onto the result rows. `fixture_hash` covers the five that define what is
//! measured, so a run with an edited probe is distinguishable from a run with
//! revised guidance (I6).
//!
//! One validation rule is newer than the rest and is the reason this file has a
//! banned-token list. ADR 0110 decision 5: no criterion may name an internal of
//! the context mode. It is checked rather than remembered, because ADR 0087
//! wrote the same rule down and eleven probes still shipped scoring a spelling.
//!
//! Validation is strict and runs before anything is seeded. A probe naming a
//! missing fact is a broken measurement, and so is one carrying both an
//! assertion and a judge flag. Finding either mid-run would void a repeat.
//!
//! Every table below denies an unknown key. A misspelled one would otherwise
//! deserialize to its default, silently, and `fixture_hash` would then stamp a
//! measurement nobody wrote as the pre-registered one.
//!
//! One exception, and it is the gap worth knowing about. [`Assertion`] is an
//! internally tagged enum. The attribute is not safe to add there unverified,
//! so a misspelled key inside an `assert` block still defaults in silence.
//! Tracked as `harden-0819-eval-assertion-unknown-keys` in the work tracker.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use lucidos_engine::core::preference_catalog;
use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::assertions::Assertion;

type Fallible<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

/// The seed tree, relative to the fixture root. The harness copies it over the
/// arm's `data/`, so a path in it is a path an assertion reads.
pub const SEED_TREE: &str = "fixtures/workspace";

/// The files `fixture_hash` covers, in the order it hashes them.
pub const HASHED_FIXTURE_FILES: [&str; 5] = [
    "tasks.toml",
    "facts.toml",
    "probes.toml",
    "answers.toml",
    "completion.toml",
];

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TasksFile {
    /// Appended to every prompt in both arms. See the file's own header for
    /// why it is central rather than repeated per task.
    pub scope_rule: String,
    pub task: Vec<Task>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Task {
    pub id: String,
    pub title: String,
    /// Sent verbatim, with `{marker}` replaced per run.
    pub prompt: String,
    /// The whole task's budget, both turns of a two-turn task included.
    pub timeout_minutes: u64,
    /// A second user message, sent into the same thread once turn one has
    /// genuinely finished.
    ///
    /// It is what makes a task measure a turn boundary. The engine trims
    /// context between turns by dropping history from the oldest end, and it
    /// says nothing when it does. What survives into turn two is the question.
    /// The driver holds the follow-up back until turn one is idle and holds no
    /// event wait. Posted mid-round it would blur the two turns into one.
    #[serde(default)]
    pub followup: Option<String>,
    /// Tasks whose failure voids this one's probes. Transitively closed by
    /// [`TaskGraph`], so a task names only its direct dependencies.
    #[serde(default)]
    pub depends_on: Vec<String>,
    /// Checked before the task is sent. A failure voids the task itself.
    #[serde(default)]
    pub preconditions: Vec<Assertion>,
}

impl Task {
    pub fn timeout(&self) -> std::time::Duration {
        std::time::Duration::from_secs(self.timeout_minutes * 60)
    }

    /// The prompt with its run marker substituted and the scope rule appended.
    ///
    /// One place, so the rule cannot drift per task. It goes last, because a
    /// standing instruction ahead of the request would read as part of it.
    pub fn rendered_prompt(&self, marker: &str, scope_rule: &str) -> String {
        render(&self.prompt, marker, scope_rule)
    }

    /// The follow-up, rendered exactly as the first prompt is.
    ///
    /// The scope rule goes on both turns rather than on turn one alone. The
    /// cross-turn trim drops the oldest history first. So a rule carried only
    /// by turn one is a rule the context ceiling can take away, in one arm and
    /// not the other. That is the asymmetry the central rule exists to prevent.
    pub fn rendered_followup(&self, marker: &str, scope_rule: &str) -> Option<String> {
        self.followup
            .as_deref()
            .map(|followup| render(followup, marker, scope_rule))
    }
}

fn render(prompt: &str, marker: &str, scope_rule: &str) -> String {
    format!(
        "{}\n\n{}",
        prompt.replace("{marker}", marker).trim_end(),
        scope_rule.trim()
    )
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FactsFile {
    pub fact: Vec<Fact>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Fact {
    pub id: String,
    pub statement: String,
    /// The register's canonical tier. A probe may override it, since
    /// one fact is tier 4 in the turn that states it and tier 3 afterwards.
    pub tier: u8,
    pub established_in: String,
    /// How the establishing task puts the fact in front of the model.
    ///
    /// Validation reads it to pick the haystack, so a probed fact nobody
    /// states is refused rather than run.
    #[serde(default)]
    pub established_by: Established,
    /// What counts as stating the fact, with its specific value, in T12's
    /// handover. This is the primary endpoint's whole definition.
    pub census_regex: String,
    /// The loose form of the fact, read to prove the establishing task said it.
    ///
    /// A prompt states the fact, and `census_regex` wants the exact value the
    /// model works out from it. T02 lists eight builds and never says 37.5%, so
    /// the strict regex would refuse a fact the prompt plainly establishes.
    pub stated_regex: String,
    /// What an `ask_user_question` must be about for the `asked` outcome.
    pub topic_regex: String,
}

/// The route by which a fact reaches the model in the task that establishes it.
///
/// Four routes, and the register declares which one. `Prompt` is the common
/// case and the default. `Answer` is a scripted reply to `ask_user_question`,
/// so the fact arrives mid-thread. `Work` is a fact the thread establishes by
/// doing the task at all, such as a script failing or an event being emitted.
/// `Seed` is a fact written in a seeded document, which the task asks the agent
/// to read: no prompt states it, and unlike `Work` there is text to check.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Established {
    #[default]
    Prompt,
    Answer,
    Work,
    Seed,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProbesFile {
    pub probe: Vec<Probe>,
    pub judge: JudgeConfig,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Probe {
    pub id: String,
    pub task: String,
    /// `None` for the probes that measure behaviour rather than a fact.
    #[serde(default)]
    pub fact: Option<String>,
    #[serde(default)]
    pub tier: Option<u8>,
    /// The tempting alternative an agent without the fact reaches for. Prose,
    /// recorded so a reader can tell a probe from a loudness test.
    pub wrong_default: String,
    #[serde(default)]
    pub assert: Option<Assertion>,
    /// Names a rubric in `[judge.rubric]`. Mutually exclusive with `assert`.
    #[serde(default)]
    pub judge: Option<String>,
    /// Which judge answer counts as this probe passing.
    ///
    /// A rubric asks one question, and the probe says which answer it wants.
    /// P05.5 asks whether an intent carries procedure, and passes on `no`.
    #[serde(default)]
    pub judge_passes_when: JudgeAnswer,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompletionFile {
    pub completion: Vec<CompletionProbe>,
}

/// One task's completion probe: did the job get done.
///
/// Deliberately NOT a [`Probe`]. A probe asks whether the agent still knew a
/// fact, which measures the mode. This asks whether the deliverable exists and
/// is right, which measures the agent. The missing fields are the point: no
/// `fact` and no `tier`, so a completion outcome cannot
/// reach a retention rate or the tier breakdown.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompletionProbe {
    pub id: String,
    pub task: String,
    /// What the task's deliverable IS, in the task's own terms.
    ///
    /// Stands where a probe's `wrong_default` stands, and for the same reason:
    /// a scorer nobody can read is a scorer nobody can check. There is no
    /// tempting wrong answer to name here, so this names the right one.
    pub deliverable: String,
    #[serde(default)]
    pub assert: Option<Assertion>,
    /// Accepted so the refusal can explain itself, and never scored.
    ///
    /// A judged probe is scored by `score`, after the run. A completion probe
    /// is scored the moment its own task ends, because every later task
    /// rewrites the tree. Nothing can be both, so this is a hard refusal rather
    /// than a gap to fill later. All fourteen deliverables are expressible as
    /// assertions, so nothing is lost.
    #[serde(default)]
    pub judge: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum JudgeAnswer {
    #[default]
    Yes,
    No,
}

impl JudgeAnswer {
    /// Whether a judge verdict means this probe passed.
    pub fn passed(self, judge_said_yes: bool) -> bool {
        match self {
            JudgeAnswer::Yes => judge_said_yes,
            JudgeAnswer::No => !judge_said_yes,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JudgeConfig {
    /// Must not be the model under test. Checked against the run's model.
    pub model: String,
    /// How many shuffled calls vote on a judged probe.
    pub votes: u32,
    /// Above this disagreement rate the probe leaves the primary analysis.
    pub disagreement_ceiling: f64,
    /// Fraction of threads the triage pass selects for the human read.
    pub triage_fraction: f64,
    pub rubric: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AnswersFile {
    #[serde(default)]
    pub answers: Vec<TaskAnswers>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskAnswers {
    pub task: String,
    pub scripted: Vec<ScriptedAnswer>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScriptedAnswer {
    /// Regex the question text must match for this reply to be the answer.
    pub matches: String,
    /// The reply. Matched against the offered option labels first, and sent as
    /// free text when none of them fit.
    pub answer: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PricesFile {
    pub model: Vec<ModelPrice>,
}

/// Preference keys naming a model the eval's own workload runs on.
///
/// The seed writes `chat_model` and deletes every other preference row, so each
/// of these falls back to the catalog's default. Two are deliberately absent:
/// `model_conversation_summary` inherits `model_memory` rather than naming a
/// model, and `model_image_description` fires on an upload no task makes.
const AUXILIARY_MODEL_KEYS: [&str; 2] = ["model_title", "model_memory"];

/// Per-million-token prices for one model, pinned so a result is reproducible.
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ModelPrice {
    pub id: String,
    pub input_per_mtok: f64,
    pub output_per_mtok: f64,
    pub cache_read_per_mtok: f64,
    pub cache_creation_per_mtok: f64,
}

/// The whole loaded fixture, validated.
#[derive(Debug, Clone)]
pub struct Fixture {
    pub tasks: Vec<Task>,
    pub facts: Vec<Fact>,
    pub probes: Vec<Probe>,
    /// One per task, in `completion.toml`. Kept apart from `probes` because a
    /// completion outcome measures the agent and a probe measures the mode.
    pub completion: Vec<CompletionProbe>,
    pub judge: JudgeConfig,
    pub answers: Vec<TaskAnswers>,
    /// Appended to every prompt, in both arms.
    pub scope_rule: String,
    pub prices: Vec<ModelPrice>,
    /// Every file in the seed tree, as a path relative to the arm's `data/`.
    /// Validation reads it to tell a precondition on a seeded input from one on
    /// a behaviour a probe scores.
    pub seed_files: Vec<String>,
    /// Where those paths resolve, so validation can read a seeded document and
    /// not just name it.
    pub seed_root: PathBuf,
    /// Covers tasks, facts, probes and answers together.
    pub fixture_hash: String,
    /// Covers the engine's own `NOTE_GUIDANCE`, read out of the checkout.
    ///
    /// It hashed `eval/context-mode/guidance.md` until ADR 0109 moved the text
    /// into the engine. That file is now deleted: a hash over a copy no model
    /// reads labels nothing. See `crate::guidance`.
    pub guidance_hash: String,
    /// Covers `prices.toml` alone (I8).
    pub prices_hash: String,
}

impl Fixture {
    pub fn load(root: &Path) -> Fallible<Fixture> {
        let tasks: TasksFile = read_toml(&root.join("tasks.toml"))?;
        let facts: FactsFile = read_toml(&root.join("facts.toml"))?;
        let probes: ProbesFile = read_toml(&root.join("probes.toml"))?;
        let answers: AnswersFile = read_toml(&root.join("answers.toml"))?;
        let completion: CompletionFile = read_toml(&root.join("completion.toml"))?;
        let prices: PricesFile = read_toml(&root.join("prices.toml"))?;
        // ADR 0109: the engine owns the guidance it delivers, so the hash
        // covers what shipped rather than the fixture's stale copy. See
        // `crate::guidance`.
        let guidance = crate::guidance::guidance_for_this_build()?;

        let mut hasher = Sha256::new();
        for name in HASHED_FIXTURE_FILES {
            hasher.update(name.as_bytes());
            hasher.update([0x1f]);
            hasher.update(std::fs::read(root.join(name))?);
            hasher.update([0x1e]);
        }
        let fixture_hash = hex(hasher.finalize().as_slice());

        let fixture = Fixture {
            seed_files: seed_files(&root.join(SEED_TREE))?,
            seed_root: root.join(SEED_TREE),
            scope_rule: tasks.scope_rule,
            tasks: tasks.task,
            facts: facts.fact,
            probes: probes.probe,
            completion: completion.completion,
            judge: probes.judge,
            answers: answers.answers,
            prices: prices.model,
            guidance_hash: hex(Sha256::digest(guidance.as_bytes()).as_slice()),
            prices_hash: hex(Sha256::digest(std::fs::read(root.join("prices.toml"))?).as_slice()),
            fixture_hash,
        };
        fixture.validate()?;
        Ok(fixture)
    }

    /// Refuse a run whose spend the price table cannot express (I8).
    ///
    /// `metrics::usd` refuses an unpriced model too, but it first runs once a
    /// task has been driven, so the run dies having already spent. Asked here,
    /// before anything boots, the same question costs nothing.
    ///
    /// The auxiliary models come from the engine's own catalog, not a list kept
    /// here. A changed default then reaches the harness with the engine that
    /// changed it.
    pub fn check_models_priced(&self, under_test: &str) -> Fallible<()> {
        let mut wanted = BTreeSet::from([under_test.to_string()]);
        for key in AUXILIARY_MODEL_KEYS {
            let spec = preference_catalog::lookup(key).ok_or_else(|| {
                format!(
                    "the engine's preference catalog no longer states {key:?}, so the harness \
                     cannot tell which model the auxiliary calls will bill"
                )
            })?;
            wanted.insert(spec.default.to_string());
        }
        let missing: Vec<&str> = wanted
            .iter()
            .map(String::as_str)
            .filter(|model| !self.prices.iter().any(|price| price.id == *model))
            .collect();
        if missing.is_empty() {
            return Ok(());
        }
        Err(format!(
            "prices.toml has no row for {}. A run bills the model under test and the engine's \
             auxiliary defaults, and every one of them is priced before the run spends.",
            missing.join(", ")
        )
        .into())
    }

    pub fn task(&self, id: &str) -> Option<&Task> {
        self.tasks.iter().find(|t| t.id == id)
    }

    pub fn fact(&self, id: &str) -> Option<&Fact> {
        self.facts.iter().find(|f| f.id == id)
    }

    pub fn probes_for<'a>(&'a self, task: &'a str) -> impl Iterator<Item = &'a Probe> {
        self.probes.iter().filter(move |p| p.task == task)
    }

    /// The one completion probe for a task. Validation guarantees it is there.
    pub fn completion_for(&self, task: &str) -> Option<&CompletionProbe> {
        self.completion.iter().find(|c| c.task == task)
    }

    /// The census probes, derived from the fact register rather than restated.
    ///
    /// The primary endpoint is one probe per fact against T12's handover, and
    /// each fact already carries the regex that defines "stated with its
    /// specific value". Declaring them in `probes.toml` too would be two
    /// sources for one number.
    ///
    /// Only facts the census task could know reach it: those established in a
    /// task it depends on, plus its own. A fact established downstream is a
    /// probe neither arm can pass. It would scale the primary endpoint's
    /// denominator while measuring nothing. The scope is structural rather than
    /// a per-fact opt-out. A flag on a pre-registered endpoint is a lever
    /// somebody can pull once they have seen the result.
    pub fn census_probes(&self, handover_path: &str) -> Vec<Probe> {
        let graph = TaskGraph::new(&self.tasks);
        let mut known = graph.upstream_of(crate::analyse::CENSUS_TASK);
        known.insert(crate::analyse::CENSUS_TASK.to_string());
        self.facts
            .iter()
            .filter(|fact| known.contains(&fact.established_in))
            .map(|fact| Probe {
                id: format!("P12.{}", fact.id),
                task: crate::analyse::CENSUS_TASK.to_string(),
                fact: Some(fact.id.clone()),
                tier: Some(fact.tier),
                wrong_default: format!("the handover omits {}, or states it vaguely", fact.id),
                assert: Some(Assertion::FileMatches {
                    path: handover_path.to_string(),
                    regex: fact.census_regex.clone(),
                }),
                judge: None,
                judge_passes_when: JudgeAnswer::default(),
            })
            .collect()
    }

    /// Every string a human reads as a criterion, so one scan covers them all.
    ///
    /// Task prompts and the scope rule are in here on purpose. A prompt naming
    /// a verb would teach the agent to use it, which is the same defect as a
    /// probe scoring it, one step earlier.
    fn criterion_texts(&self) -> Vec<(String, String)> {
        let mut texts: Vec<(String, String)> =
            vec![("the scope rule".to_string(), self.scope_rule.clone())];
        for task in &self.tasks {
            texts.push((format!("task {}'s prompt", task.id), task.prompt.clone()));
            if let Some(followup) = &task.followup {
                texts.push((format!("task {}'s follow-up", task.id), followup.clone()));
            }
        }
        for fact in &self.facts {
            texts.push((format!("fact {}", fact.id), fact.statement.clone()));
        }
        for probe in &self.probes {
            texts.push((
                format!("probe {}'s wrong_default", probe.id),
                probe.wrong_default.clone(),
            ));
            texts.extend(rendered_assertion(
                &format!("probe {}'s assertion", probe.id),
                probe.assert.as_ref(),
            ));
        }
        for completion in &self.completion {
            texts.push((
                format!("completion probe {}'s deliverable", completion.id),
                completion.deliverable.clone(),
            ));
            texts.extend(rendered_assertion(
                &format!("completion probe {}'s assertion", completion.id),
                completion.assert.as_ref(),
            ));
        }
        for (name, rubric) in &self.judge.rubric {
            texts.push((format!("judge rubric {name}"), rubric.clone()));
        }
        for entry in &self.answers {
            for scripted in &entry.scripted {
                texts.push((
                    format!("a scripted answer for {}", entry.task),
                    scripted.answer.clone(),
                ));
            }
        }
        texts
    }
}

/// An assertion as one scannable string, or nothing when it does not render.
///
/// Serialized rather than walked, so a new variant is covered the day it is
/// added. A variant that cannot serialize is not silently skipped: the loader
/// already refuses an invalid assertion, and this reads the same tree.
fn rendered_assertion(label: &str, assertion: Option<&Assertion>) -> Option<(String, String)> {
    let rendered = serde_json::to_string(assertion?).ok()?;
    Some((label.to_string(), rendered))
}

/// Words no criterion may contain (ADR 0110 decision 5).
///
/// Every one names an internal of the context mode: a verb, a surface it
/// renders, or a route the retired outcome vocabulary attributed a pass to. A
/// criterion that says any of them is scoring the mechanism rather than the
/// work, and it stops meaning anything the day the mechanism changes.
///
/// Matched case-insensitively, and deliberately as PHRASES where the bare word
/// is ordinary English. The seeded corpus lives in `artifacts/ledger-migration`
/// and several probes name its paths, so `ledger` alone would refuse the
/// fixture for saying where a document is.
/// A RETIRED internal stays on the list. A criterion naming one scores a
/// mechanism that is gone, so it reads 0 for every arm. The run then reports
/// a difference nobody made.
pub const BANNED_MECHANISM_TOKENS: [&str; 14] = [
    "keep open",
    "keep_open",
    "working understanding",
    "context ledger",
    "context panel",
    "curated bod",
    "context_mode",
    "self-curated",
    "sweep",
    "from-notes",
    "unknown-pass",
    "scratchpad",
    "keep_in_context",
    "dismiss_from_context",
];

impl Fixture {
    /// Criteria that name an internal of the mode, described.
    fn mechanism_in_a_criterion(&self) -> Vec<String> {
        let mut problems = Vec::new();
        for (where_it_is, text) in self.criterion_texts() {
            let haystack = text.to_ascii_lowercase();
            for token in BANNED_MECHANISM_TOKENS {
                if haystack.contains(token) {
                    problems.push(format!(
                        "{where_it_is} names `{token}`, which is an internal of the context \
                         mode. ADR 0110 decision 5: a criterion scores whether the work was \
                         done and whether the facts the task needed survived, never how"
                    ));
                }
            }
        }
        problems
    }

    /// Every structural rule the fixture has to satisfy before a run starts.
    pub fn validate(&self) -> Fallible<()> {
        let mut problems = Vec::new();
        let task_ids: BTreeSet<&str> = self.tasks.iter().map(|t| t.id.as_str()).collect();
        let fact_ids: BTreeSet<&str> = self.facts.iter().map(|f| f.id.as_str()).collect();

        let mut seen_tasks = BTreeSet::new();
        for task in &self.tasks {
            if !seen_tasks.insert(task.id.as_str()) {
                problems.push(format!("task {} is declared twice", task.id));
            }
            for dependency in &task.depends_on {
                if !task_ids.contains(dependency.as_str()) {
                    problems.push(format!(
                        "task {} depends on {dependency}, which is not a task",
                        task.id
                    ));
                }
            }
            for precondition in &task.preconditions {
                if let Err(e) = precondition.validate() {
                    problems.push(format!("task {} has an invalid precondition: {e}", task.id));
                }
            }
        }

        for fact in &self.facts {
            for (label, pattern) in [
                ("census_regex", &fact.census_regex),
                ("stated_regex", &fact.stated_regex),
                ("topic_regex", &fact.topic_regex),
            ] {
                if let Err(e) = regex::Regex::new(pattern) {
                    problems.push(format!("fact {}'s {label} does not compile: {e}", fact.id));
                }
            }
            if !task_ids.contains(fact.established_in.as_str()) {
                problems.push(format!(
                    "fact {} is established in {}, which is not a task",
                    fact.id, fact.established_in
                ));
            }
            if !(1..=4).contains(&fact.tier) {
                problems.push(format!(
                    "fact {} has tier {}, not 1 to 4",
                    fact.id, fact.tier
                ));
            }
        }

        let mut seen_probes = BTreeSet::new();
        for probe in &self.probes {
            if !seen_probes.insert(probe.id.as_str()) {
                problems.push(format!("probe {} is declared twice", probe.id));
            }
            if !task_ids.contains(probe.task.as_str()) {
                problems.push(format!(
                    "probe {} is on {}, which is not a task",
                    probe.id, probe.task
                ));
            }
            if let Some(fact) = &probe.fact {
                if !fact_ids.contains(fact.as_str()) {
                    problems.push(format!(
                        "probe {} names fact {fact}, which is not in the register",
                        probe.id
                    ));
                }
            }
            // I9: a probe declares exactly one scorer.
            match (&probe.assert, &probe.judge) {
                (Some(_), Some(_)) => problems.push(format!(
                    "probe {} declares both `assert` and `judge`. I9: the judge never scores \
                     what an assertion can express, so a probe declares exactly one",
                    probe.id
                )),
                (None, None) => problems.push(format!(
                    "probe {} declares neither `assert` nor `judge`, so nothing scores it",
                    probe.id
                )),
                (Some(assertion), None) => {
                    if let Err(e) = assertion.validate() {
                        problems.push(format!("probe {}'s assertion is invalid: {e}", probe.id));
                    }
                }
                (None, Some(rubric)) => {
                    if !self.judge.rubric.contains_key(rubric) {
                        problems.push(format!(
                            "probe {} names judge rubric {rubric}, which `[judge.rubric]` does \
                             not define",
                            probe.id
                        ));
                    }
                }
            }
        }

        // Exactly one per task, both directions. With none, the task's delivery
        // goes unmeasured and the completion rate's denominator quietly stops
        // being the task count. With two, it is counted twice.
        let mut seen_completion = BTreeSet::new();
        let mut covered: BTreeSet<&str> = BTreeSet::new();
        for completion in &self.completion {
            if !seen_completion.insert(completion.id.as_str()) {
                problems.push(format!(
                    "completion probe {} is declared twice",
                    completion.id
                ));
            }
            if !task_ids.contains(completion.task.as_str()) {
                problems.push(format!(
                    "completion probe {} is on {}, which is not a task",
                    completion.id, completion.task
                ));
            } else if !covered.insert(completion.task.as_str()) {
                problems.push(format!(
                    "task {} has more than one completion probe, so its delivery would be \
                     counted twice",
                    completion.task
                ));
            }
            // One scorer, and it is an assertion. A judged probe is scored
            // after the run. A completion probe is scored the moment its own
            // task ends, so a judged one could never be right.
            if completion.judge.is_some() {
                problems.push(format!(
                    "completion probe {} names a judge rubric. A completion probe is scored \
                     when its own task ends and the judge runs after the whole run, so it \
                     takes an `assert`",
                    completion.id
                ));
            }
            match &completion.assert {
                None => problems.push(format!(
                    "completion probe {} declares no `assert`, so nothing scores it",
                    completion.id
                )),
                Some(assertion) => {
                    if let Err(e) = assertion.validate() {
                        problems.push(format!(
                            "completion probe {}'s assertion is invalid: {e}",
                            completion.id
                        ));
                    }
                }
            }
        }
        for task in &self.tasks {
            if !covered.contains(task.id.as_str()) {
                problems.push(format!(
                    "task {} has no completion probe, so nothing asks whether its job got done",
                    task.id
                ));
            }
        }

        for entry in &self.answers {
            if !task_ids.contains(entry.task.as_str()) {
                problems.push(format!("answers name {}, which is not a task", entry.task));
            }
            for scripted in &entry.scripted {
                if let Err(e) = regex::Regex::new(&scripted.matches) {
                    problems.push(format!(
                        "an answer for {} has an invalid match regex: {e}",
                        entry.task
                    ));
                }
            }
        }

        // The scope rule reaches every prompt, so a fact in it reaches every
        // probe. That would turn the register into reading comprehension.
        if self.scope_rule.trim().is_empty() {
            problems.push("tasks.toml declares an empty scope_rule".to_string());
        }
        for fact in &self.facts {
            if regex::Regex::new(&fact.census_regex)
                .is_ok_and(|pattern| pattern.is_match(&self.scope_rule))
            {
                problems.push(format!(
                    "the scope rule states fact {}, so every probe on it would be answered by \
                     the prompt: {}",
                    fact.id, fact.statement
                ));
            }
        }

        // A probe on a fact nobody stated measures nothing, because the model
        // cannot lose what it was never told. F05 was probed on three tasks and
        // said in none, and the pilot read the misses as retention.
        let probed: BTreeSet<&str> = self
            .probes
            .iter()
            .filter_map(|p| p.fact.as_deref())
            .collect();
        for fact in &self.facts {
            if !probed.contains(fact.id.as_str()) {
                continue;
            }
            let Ok(stated) = regex::Regex::new(&fact.stated_regex) else {
                continue;
            };
            let (said, route) = match fact.established_by {
                Established::Prompt => (
                    self.tasks
                        .iter()
                        .find(|task| task.id == fact.established_in)
                        .is_some_and(|task| {
                            stated.is_match(&task.prompt)
                                || task.followup.as_deref().is_some_and(|f| stated.is_match(f))
                        }),
                    "its prompt",
                ),
                Established::Answer => (
                    self.answers
                        .iter()
                        .filter(|entry| entry.task == fact.established_in)
                        .flat_map(|entry| &entry.scripted)
                        .any(|scripted| stated.is_match(&scripted.answer)),
                    "a scripted answer",
                ),
                Established::Work => (true, "the work"),
                Established::Seed => (self.stated_in_seed_tree(fact), "a seeded document"),
            };
            if !said {
                problems.push(format!(
                    "fact {} is probed, and {} never states it in {route}. A probe on a fact \
                     nobody said measures nothing: {}",
                    fact.id, fact.established_in, fact.statement
                ));
            }
        }

        problems.extend(self.unsatisfiable_preconditions());
        problems.extend(self.mechanism_in_a_criterion());

        if self.prices.is_empty() {
            problems.push("prices.toml declares no model".to_string());
        }

        if problems.is_empty() {
            Ok(())
        } else {
            Err(format!("fixture is invalid:\n  {}", problems.join("\n  ")).into())
        }
    }
}

impl Fixture {
    /// Whether some seeded document states the fact, with its value.
    ///
    /// The census regex is the strict one, and a seeded fact is checked against
    /// it rather than against the looser note regex. The document is the only
    /// place the value is written. A corpus that merely names the topic leaves
    /// every probe on the fact measuring the model's prior.
    fn stated_in_seed_tree(&self, fact: &Fact) -> bool {
        let Ok(pattern) = regex::Regex::new(&fact.census_regex) else {
            return false;
        };
        self.seed_files.iter().any(|path| {
            std::fs::read(self.seed_root.join(path))
                .is_ok_and(|bytes| pattern.is_match(&String::from_utf8_lossy(&bytes)))
        })
    }

    /// Preconditions that gate on something the fixture cannot deliver.
    ///
    /// A precondition may read the task's INPUTS and nothing else: a file the
    /// seed tree lays down, or a deliverable a task it depends on asserts on
    /// completion. Gate on anything else and the task is voided exactly when
    /// the miss is the interesting result, asymmetrically between arms. T06 and
    /// T08 gated on a knowhow file the T05 agent chooses to write and P05.4
    /// scores, so one arm declining voided four tasks. What this catches is a
    /// path or an event type that no seed file and no depended-on completion
    /// assertion supplies.
    ///
    /// What it does not catch, all deliberate. Two different globs are never
    /// compared, so a precondition spells its path as the completion probe
    /// does. An `any` on the guarantee side promises nothing, so a completion
    /// probe written that way gates nothing downstream. Counts are compared on
    /// neither side, so a precondition wanting eight events or three files
    /// passes on a probe asserting one. Existence is the only question, so a
    /// gate saying a file must NOT be there reads here as no gate at all.
    /// Nothing asks whether a probe SCORES the path, only whether the fixture
    /// can produce it.
    fn unsatisfiable_preconditions(&self) -> Vec<String> {
        let graph = TaskGraph::new(&self.tasks);
        let mut problems = Vec::new();
        for task in &self.tasks {
            if task.preconditions.is_empty() {
                continue;
            }
            let upstream = graph.upstream_of(&task.id);
            let mut guaranteed = Reads::default();
            for assertion in upstream
                .iter()
                .filter_map(|id| self.completion_for(id))
                .filter_map(|completion| completion.assert.as_ref())
            {
                let reads = Reads::of(assertion);
                guaranteed.paths.extend(reads.paths);
                guaranteed.events.extend(reads.events);
            }
            for precondition in &task.preconditions {
                let Some(offender) = unsatisfiable_leaf(
                    precondition,
                    &self.seed_files,
                    &guaranteed.paths,
                    &guaranteed.events,
                ) else {
                    continue;
                };
                problems.push(format!(
                    "task {}'s precondition on {offender} is not satisfiable: the seed tree does \
                     not lay it down and no task {} depends on asserts it on completion. A \
                     precondition gates on the task's inputs, never on a behaviour a probe scores",
                    task.id, task.id
                ));
            }
        }
        problems
    }
}

/// Which tasks a set of failures voids, transitively (I3).
#[derive(Debug, Clone)]
pub struct TaskGraph {
    edges: BTreeMap<String, Vec<String>>,
}

impl TaskGraph {
    pub fn new(tasks: &[Task]) -> Self {
        TaskGraph {
            edges: tasks
                .iter()
                .map(|t| (t.id.clone(), t.depends_on.clone()))
                .collect(),
        }
    }

    /// Every task the given one depends on, directly or through another.
    pub fn upstream_of(&self, task: &str) -> BTreeSet<String> {
        let mut found = BTreeSet::new();
        let mut pending = vec![task.to_string()];
        while let Some(current) = pending.pop() {
            for dependency in self.edges.get(&current).into_iter().flatten() {
                if found.insert(dependency.clone()) {
                    pending.push(dependency.clone());
                }
            }
        }
        found
    }

    /// Whether any task this one depends on is in `failed`.
    pub fn is_voided_by(&self, task: &str, failed: &BTreeSet<String>) -> bool {
        self.upstream_of(task).iter().any(|up| failed.contains(up))
    }
}

/// Every file under the seed tree, as a path relative to it.
///
/// The harness copies the tree over the arm's `data/`, so these are exactly the
/// paths an assertion sees before the first prompt is sent.
fn seed_files(root: &Path) -> Fallible<Vec<String>> {
    if !root.is_dir() {
        return Err(format!("the fixture has no seed tree at {}", root.display()).into());
    }
    crate::assertions::matching_files(root, "**")?
        .iter()
        .map(|path| {
            path.strip_prefix(root)
                .map(|relative| relative.to_string_lossy().into_owned())
                .map_err(|e| format!("{} is not under the seed tree: {e}", path.display()).into())
        })
        .collect()
}

/// The paths and event types one assertion PROVES are there, split by which.
///
/// Only a leaf whose success proves the resource exists counts. A `glob_count`
/// with no `min`, an `event_count` of zero, an empty `event_payloads_cover`, a
/// table-width check and a `file_not_matches` all hold over an empty tree. Each
/// therefore promises nothing as a guarantee, and gates on nothing as a
/// precondition.
#[derive(Default)]
struct Reads<'a> {
    paths: Vec<&'a str>,
    events: Vec<&'a str>,
}

impl<'a> Reads<'a> {
    /// Read one assertion, descending only into `All`.
    ///
    /// `Any` is skipped on purpose. On the guarantee side it promises no single
    /// branch. On the precondition side nothing but a leaf gets here, because
    /// [`unsatisfiable_leaf`] takes both `all` and `any` itself.
    fn of(assertion: &'a Assertion) -> Reads<'a> {
        let mut reads = Reads::default();
        reads.walk(assertion);
        reads
    }

    fn walk(&mut self, assertion: &'a Assertion) {
        match assertion {
            Assertion::All { of } => of.iter().for_each(|a| self.walk(a)),
            Assertion::FileExists { path }
            | Assertion::FileMatches { path, .. }
            | Assertion::FileEndsWith { path, .. }
            | Assertion::ResponseQuotesFile { path, .. }
            | Assertion::GlobCount {
                path,
                min: Some(1..),
                ..
            } => self.paths.push(path),
            Assertion::FileModifiedAfterEvent { path, event_type } => {
                self.paths.push(path);
                self.events.push(event_type);
            }
            Assertion::EventExists { event_type }
            | Assertion::EventCount {
                event_type,
                count: 1..,
            } => self.events.push(event_type),
            Assertion::EventPayloadsCover {
                event_type,
                regexes,
            } if !regexes.is_empty() => self.events.push(event_type),
            Assertion::Any { .. }
            | Assertion::GlobCount { .. }
            | Assertion::EventCount { .. }
            | Assertion::EventPayloadsCover { .. }
            | Assertion::MarkdownTableMaxColumns { .. }
            | Assertion::FileNotMatches { .. }
            | Assertion::ToolCallCount { .. }
            | Assertion::ResponseMatches { .. }
            | Assertion::ResponseNotMatches { .. } => {}
        }
    }
}

/// Whether a completion probe's path guarantees a file the precondition matches.
///
/// Two forms, and both are exact rather than clever. The same pattern on both
/// sides is the common case. A concrete path on the guarantee side is checked
/// against the precondition's glob. Two DIFFERENT globs are not compared,
/// because deciding whether one glob's language sits inside another's is not
/// worth faking. Write the precondition's path as the completion probe writes it.
fn guarantees_path(guarantee: &str, wanted: &str) -> bool {
    guarantee == wanted
        || (!guarantee.contains('*') && crate::assertions::glob_match(wanted, guarantee))
}

/// The first leaf of a precondition that nothing can satisfy, described.
///
/// This owns both combinators, so a nested one still reaches its own arm. An
/// `all` fails on its first failing branch. An `any` holds as soon as one branch
/// does. So it fails only when every branch fails, and it then reports itself
/// rather than an arbitrary branch.
fn unsatisfiable_leaf(
    precondition: &Assertion,
    seed_files: &[String],
    paths: &[&str],
    events: &[&str],
) -> Option<String> {
    match precondition {
        Assertion::All { of } => of
            .iter()
            .find_map(|branch| unsatisfiable_leaf(branch, seed_files, paths, events)),
        Assertion::Any { of } => of
            .iter()
            .all(|branch| unsatisfiable_leaf(branch, seed_files, paths, events).is_some())
            .then(|| "every branch of an `any`".to_string()),
        leaf => {
            let reads = Reads::of(leaf);
            if let Some(path) = reads.paths.iter().find(|wanted| {
                !seed_files
                    .iter()
                    .any(|seeded| crate::assertions::glob_match(wanted, seeded))
                    && !paths.iter().any(|got| guarantees_path(got, wanted))
            }) {
                return Some(format!("the path `{path}`"));
            }
            reads
                .events
                .iter()
                .find(|wanted| !events.contains(*wanted))
                .map(|event| format!("the event `{event}`"))
        }
    }
}

fn read_toml<T: serde::de::DeserializeOwned>(path: &Path) -> Fallible<T> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    toml::from_str(&text).map_err(|e| format!("cannot parse {}: {e}", path.display()).into())
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes.iter().fold(String::with_capacity(64), |mut acc, b| {
        let _ = write!(acc, "{b:02x}");
        acc
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The checked-in fixture's directory, which several tests load.
    fn fixture_root() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(std::path::Path::parent)
            .expect("the crate sits two levels below the repository root")
            .join("eval/context-mode")
    }

    /// The row the price table is missing would otherwise surface from
    /// `metrics::usd`, one driven task into a run that had already spent.
    #[test]
    fn the_checked_in_prices_cover_the_model_under_test_and_the_auxiliaries() {
        Fixture::load(&fixture_root())
            .expect("the checked-in fixture must be valid")
            .check_models_priced(crate::DEFAULT_MODEL)
            .expect("prices.toml prices every model a default run bills");
    }

    /// A model nobody priced fails before the boot, and the refusal names it.
    #[test]
    fn an_unpriced_model_under_test_is_refused_by_name() {
        let error = Fixture::load(&fixture_root())
            .unwrap()
            .check_models_priced("nobody/priced-this")
            .unwrap_err()
            .to_string();
        assert!(error.contains("nobody/priced-this"), "{error}");
        assert!(error.contains("prices.toml has no row"), "{error}");
    }

    /// The keys are read off the engine's catalog, so one that was renamed has
    /// to fail here rather than quietly price nothing.
    #[test]
    fn every_auxiliary_model_key_still_names_a_model() {
        for key in AUXILIARY_MODEL_KEYS {
            let spec = preference_catalog::lookup(key)
                .unwrap_or_else(|| panic!("the catalog no longer states {key:?}"));
            assert!(
                !spec.default.starts_with('('),
                "{key} now inherits another key rather than naming a model: {}",
                spec.default
            );
        }
    }

    fn tasks() -> Vec<Task> {
        [
            ("T01", vec![]),
            ("T02", vec!["T01"]),
            ("T04", vec![]),
            ("T05", vec!["T01", "T02", "T04"]),
            ("T12", vec!["T05"]),
        ]
        .into_iter()
        .map(|(id, deps)| Task {
            id: id.to_string(),
            title: id.to_string(),
            prompt: "{marker} do it".to_string(),
            timeout_minutes: 10,
            followup: None,
            depends_on: deps.into_iter().map(str::to_string).collect(),
            preconditions: vec![],
        })
        .collect()
    }

    #[test]
    fn the_marker_is_substituted_into_the_prompt() {
        let task = &tasks()[0];
        assert_eq!(
            task.rendered_prompt("eval-7f", "stay in scope"),
            "eval-7f do it\n\nstay in scope"
        );
    }

    /// One place, both arms, every task. Fourteen copies could drift, and a rule
    /// that reached one arm only would be the asymmetry it exists to remove.
    ///
    /// A follow-up carries it too. The cross-turn trim drops the oldest history
    /// first, so a rule sent only in turn one can be evicted before turn two.
    #[test]
    fn every_task_gets_the_same_scope_rule_appended() {
        let fixture = Fixture::load(&fixture_root()).unwrap();
        assert!(!fixture.scope_rule.trim().is_empty());
        for task in &fixture.tasks {
            let sent = [
                Some(task.rendered_prompt("eval-7f", &fixture.scope_rule)),
                task.rendered_followup("eval-7f", &fixture.scope_rule),
            ];
            for rendered in sent.iter().flatten() {
                assert!(
                    rendered.ends_with(fixture.scope_rule.trim()),
                    "{} sends a message not ending with the scope rule",
                    task.id
                );
            }
            let written = [Some(task.prompt.as_str()), task.followup.as_deref()];
            for text in written.iter().flatten() {
                assert!(
                    !text.contains(fixture.scope_rule.trim()),
                    "{} restates the scope rule in its own prompt",
                    task.id
                );
            }
        }
    }

    /// The rule reaches every prompt, so a fact in it answers every probe on
    /// that fact before the agent starts.
    #[test]
    fn a_scope_rule_stating_a_fact_is_refused() {
        let mut fixture = Fixture::load(&fixture_root()).unwrap();
        let fact = fixture.facts[0].clone();
        fixture.scope_rule = format!("Do what this asks. Also: {}", fact.statement);
        let err = fixture.validate().unwrap_err().to_string();
        assert!(
            err.contains("the scope rule states fact"),
            "expected a fact leak to be refused, got: {err}"
        );
    }

    /// The checked-in rule is clean, which is the direction that matters.
    #[test]
    fn the_checked_in_scope_rule_states_no_fact() {
        Fixture::load(&fixture_root()).expect("the fixture validates, scope rule included");
    }

    #[test]
    fn an_empty_scope_rule_is_refused() {
        let mut fixture = Fixture::load(&fixture_root()).unwrap();
        fixture.scope_rule = "  \n ".to_string();
        let err = fixture.validate().unwrap_err().to_string();
        assert!(err.contains("empty scope_rule"));
    }

    #[test]
    fn upstream_is_transitive() {
        let graph = TaskGraph::new(&tasks());
        let upstream = graph.upstream_of("T12");
        assert!(upstream.contains("T05"));
        assert!(upstream.contains("T02"));
        assert!(upstream.contains("T01"));
        assert!(upstream.contains("T04"));
    }

    /// I3, the graph half: T02 failing reaches T05.
    #[test]
    fn a_failed_upstream_task_voids_the_tasks_below_it() {
        let graph = TaskGraph::new(&tasks());
        let failed: BTreeSet<String> = ["T02".to_string()].into_iter().collect();
        assert!(graph.is_voided_by("T05", &failed));
        assert!(graph.is_voided_by("T12", &failed));
        assert!(!graph.is_voided_by("T04", &failed));
        assert!(!graph.is_voided_by("T01", &failed));
    }

    #[test]
    fn timeouts_are_read_in_minutes() {
        assert_eq!(tasks()[0].timeout().as_secs(), 600);
    }

    /// Borrow the register's entry for a fact, to doctor it in place.
    fn fact_mut<'a>(fixture: &'a mut Fixture, id: &str) -> &'a mut Fact {
        fixture
            .facts
            .iter_mut()
            .find(|fact| fact.id == id)
            .expect("the register carries it")
    }

    /// The check that would have caught F05 the day it was written.
    ///
    /// F05 was probed on three tasks and stated in none. A probe on a fact
    /// nobody said measures nothing, because the model cannot lose what it was
    /// never told. T11 then hid the hole behind a memory precondition, so the
    /// pilot reported a void rather than a defect.
    ///
    /// The predicate is `stated_regex`, the loose form of the fact. A prompt
    /// states the fact, and `census_regex` wants the exact value the model
    /// works out from it.
    #[test]
    fn a_probed_fact_the_establishing_prompt_never_states_is_refused() {
        let mut fixture = Fixture::load(&fixture_root()).unwrap();
        fact_mut(&mut fixture, "F03").stated_regex = "nothing any prompt says".to_string();
        let err = fixture.validate().unwrap_err().to_string();
        assert!(err.contains("fact F03 is probed"), "{err}");
        assert!(err.contains("in its prompt"), "{err}");
    }

    /// F18 arrives as the scripted reply to T09's question, not in the prompt.
    /// So the haystack is the answer, and a wrong answer is refused too.
    #[test]
    fn an_answer_established_fact_is_read_from_the_scripted_reply() {
        let mut fixture = Fixture::load(&fixture_root()).unwrap();
        assert_eq!(
            fact_mut(&mut fixture, "F18").established_by,
            Established::Answer
        );
        for entry in &mut fixture.answers {
            for scripted in &mut entry.scripted {
                scripted.answer = "by day".to_string();
            }
        }
        let err = fixture.validate().unwrap_err().to_string();
        assert!(err.contains("fact F18 is probed"), "{err}");
        assert!(err.contains("in a scripted answer"), "{err}");
    }

    /// Some facts are established by doing the task: a script fails, an event
    /// lands, a knowhow file gets written. Nobody says them, and the register
    /// declares that rather than the check pretending otherwise.
    #[test]
    fn a_work_established_fact_has_no_text_to_check() {
        let mut fixture = Fixture::load(&fixture_root()).unwrap();
        let fact = fact_mut(&mut fixture, "F12");
        assert_eq!(fact.established_by, Established::Work);
        fact.stated_regex = "nothing any prompt says".to_string();
        fixture
            .validate()
            .expect("the work states it, not a prompt");
    }

    /// The whole checked-in fixture has to load and validate.
    ///
    /// This is what makes I9 a running check rather than an intention: every
    /// probe in `probes.toml` goes through the one-scorer rule on the way in.
    ///
    /// The census is twenty of twenty-three facts, and the gap is the point.
    /// F21 to F23 are established in T13 and T14, which T12 does not depend on,
    /// so the handover has no route to them.
    #[test]
    fn the_checked_in_fixture_loads_and_validates() {
        let root = fixture_root();
        let fixture = Fixture::load(&root).expect("the checked-in fixture must be valid");
        assert_eq!(fixture.tasks.len(), 14, "the task set is fourteen threads");
        assert_eq!(
            fixture.facts.len(),
            23,
            "the fact register is twenty-three facts"
        );
        assert_eq!(
            fixture
                .census_probes("artifacts/build-health/HANDOVER.md")
                .len(),
            20
        );
    }

    /// The census covers what the handover could carry, and stops there.
    ///
    /// T11 is the one task T12 does not depend on, so a fact established there
    /// is one the handover has no route to. Left in, it is a probe both arms
    /// fail. The primary endpoint's denominator would then grow with the
    /// fixture rather than with what is measured.
    #[test]
    fn a_fact_the_census_task_cannot_know_is_left_out() {
        let mut fixture = Fixture::load(&fixture_root()).unwrap();
        let handover = "artifacts/build-health/HANDOVER.md";
        assert_eq!(fixture.census_probes(handover).len(), 20);

        fact_mut(&mut fixture, "F03").established_in = "T11".to_string();
        let census = fixture.census_probes(handover);
        assert_eq!(census.len(), 19);
        assert!(!census
            .iter()
            .any(|probe| probe.fact.as_deref() == Some("F03")));
    }

    /// One completion probe per task, both directions, over the real fixture.
    /// A task with none leaves its delivery unmeasured, which is the whole gap
    /// this file was added to close.
    #[test]
    fn every_task_has_exactly_one_completion_probe() {
        let fixture = Fixture::load(&fixture_root()).unwrap();
        assert_eq!(fixture.completion.len(), fixture.tasks.len());
        for task in &fixture.tasks {
            assert!(
                fixture.completion_for(&task.id).is_some(),
                "{} has no completion probe",
                task.id
            );
        }
    }

    #[test]
    fn a_task_with_no_completion_probe_is_refused() {
        let mut fixture = Fixture::load(&fixture_root()).unwrap();
        let dropped = fixture.completion.remove(0);
        let err = fixture.validate().unwrap_err().to_string();
        assert!(err.contains(&format!("task {} has no completion probe", dropped.task)));
    }

    #[test]
    fn a_task_with_two_completion_probes_is_refused() {
        let mut fixture = Fixture::load(&fixture_root()).unwrap();
        let mut second = fixture.completion[0].clone();
        second.id = "C99".to_string();
        fixture.completion.push(second);
        let err = fixture.validate().unwrap_err().to_string();
        assert!(err.contains("more than one completion probe"));
    }

    #[test]
    fn a_completion_probe_with_no_assertion_is_refused() {
        let mut fixture = Fixture::load(&fixture_root()).unwrap();
        fixture.completion[0].assert = None;
        let err = fixture.validate().unwrap_err().to_string();
        assert!(err.contains("declares no `assert`"));
    }

    /// A judged probe is scored after the run, and a completion probe is
    /// scored when its own task ends. Nothing can be both.
    #[test]
    fn a_judged_completion_probe_is_refused() {
        let mut fixture = Fixture::load(&fixture_root()).unwrap();
        fixture.completion[0].judge = Some("triage".to_string());
        let err = fixture.validate().unwrap_err().to_string();
        assert!(err.contains("names a judge rubric"));
    }

    /// Borrow a task, to doctor its preconditions in place.
    fn task_mut<'a>(fixture: &'a mut Fixture, id: &str) -> &'a mut Task {
        fixture
            .tasks
            .iter_mut()
            .find(|task| task.id == id)
            .expect("the task set carries it")
    }

    /// The defect this check exists for, in the shape it shipped in.
    ///
    /// T06 gated on a knowhow file. F14 marks it `established_by = "work"`, so
    /// the T05 agent chooses whether to write it, and P05.4 scores that choice.
    /// The gate voided T06, T07, T08 and T10 for an arm that declined.
    #[test]
    fn a_precondition_on_a_scored_behaviour_is_refused() {
        let mut fixture = Fixture::load(&fixture_root()).unwrap();
        task_mut(&mut fixture, "T06").preconditions = vec![Assertion::GlobCount {
            path: "knowhow/**/*.md".to_string(),
            min: Some(1),
            max: None,
        }];
        let err = fixture.validate().unwrap_err().to_string();
        assert!(
            err.contains("task T06's precondition on the path `knowhow/**/*.md`"),
            "{err}"
        );
        assert!(err.contains("never on a behaviour a probe scores"), "{err}");
    }

    /// The event half of the same rule. Nothing seeds events, so the only route
    /// is a depended-on task's completion assertion.
    #[test]
    fn a_precondition_on_an_unproduced_event_is_refused() {
        let mut fixture = Fixture::load(&fixture_root()).unwrap();
        task_mut(&mut fixture, "T06").preconditions = vec![Assertion::EventExists {
            event_type: "KnowhowWritten".to_string(),
        }];
        let err = fixture.validate().unwrap_err().to_string();
        assert!(
            err.contains("task T06's precondition on the event `KnowhowWritten`"),
            "{err}"
        );
    }

    /// A precondition on a task's real input passes, by both routes.
    ///
    /// The seed tree lays down the collect scripts. T04's completion probe
    /// asserts the collected files, and T06 depends on T04.
    #[test]
    fn a_precondition_on_a_seeded_or_asserted_input_is_accepted() {
        let mut fixture = Fixture::load(&fixture_root()).unwrap();
        task_mut(&mut fixture, "T06").preconditions = vec![
            Assertion::FileExists {
                path: "artifacts/build-health/collect.py".to_string(),
            },
            Assertion::GlobCount {
                path: "artifacts/build-health/*-collected.json".to_string(),
                min: Some(2),
                max: None,
            },
        ];
        fixture
            .validate()
            .expect("both gate on inputs the fixture delivers");
    }

    /// The guarantee has to come from a task this one DEPENDS on. Only C07
    /// asserts `weekly.md`, and T04 runs long before T07 does.
    #[test]
    fn a_precondition_guaranteed_only_by_a_task_downstream_is_refused() {
        let mut fixture = Fixture::load(&fixture_root()).unwrap();
        task_mut(&mut fixture, "T04").preconditions = vec![Assertion::FileExists {
            path: "artifacts/build-health/weekly.md".to_string(),
        }];
        let err = fixture.validate().unwrap_err().to_string();
        assert!(
            err.contains("task T04's precondition on the path `artifacts/build-health/weekly.md`"),
            "{err}"
        );
    }

    /// Wrapping the same gate in `all` and `any` must not smuggle it past.
    ///
    /// The combinators nest, so a check that read only the outermost one would
    /// hand the author a one-word way around it.
    #[test]
    fn a_scored_behaviour_nested_under_a_combinator_is_still_refused() {
        let knowhow = Assertion::GlobCount {
            path: "knowhow/**/*.md".to_string(),
            min: Some(1),
            max: None,
        };
        let seeded = Assertion::FileExists {
            path: "artifacts/build-health/collect.py".to_string(),
        };
        for wrapped in [
            Assertion::All {
                of: vec![seeded.clone(), knowhow.clone()],
            },
            Assertion::All {
                of: vec![Assertion::Any {
                    of: vec![knowhow.clone()],
                }],
            },
            Assertion::Any {
                of: vec![knowhow.clone(), knowhow.clone()],
            },
        ] {
            let mut fixture = Fixture::load(&fixture_root()).unwrap();
            task_mut(&mut fixture, "T06").preconditions = vec![wrapped.clone()];
            let err = fixture.validate().unwrap_err().to_string();
            assert!(
                err.contains("task T06's precondition on"),
                "{wrapped:?} must be refused, got: {err}"
            );
        }

        // One satisfiable branch is all an `any` needs.
        let mut fixture = Fixture::load(&fixture_root()).unwrap();
        task_mut(&mut fixture, "T06").preconditions = vec![Assertion::Any {
            of: vec![knowhow, seeded],
        }];
        fixture.validate().expect("the seeded branch carries it");
    }

    /// Borrow a completion probe, to doctor its assertion in place.
    fn completion_mut<'a>(fixture: &'a mut Fixture, id: &str) -> &'a mut CompletionProbe {
        fixture
            .completion
            .iter_mut()
            .find(|probe| probe.id == id)
            .expect("the completion set carries it")
    }

    /// A completion probe that passes over an empty tree guarantees nothing.
    ///
    /// Each form below holds with no file and no event, so reading one as a
    /// guarantee would wave through the precondition it appears to cover. The
    /// task would then be voided at run time, which is the whole failure.
    #[test]
    fn a_vacuous_completion_assertion_guarantees_nothing() {
        let collected = "artifacts/build-health/*-collected.json".to_string();
        let vacuous = [
            Assertion::GlobCount {
                path: collected.clone(),
                min: None,
                max: Some(9),
            },
            Assertion::GlobCount {
                path: collected.clone(),
                min: Some(0),
                max: None,
            },
            Assertion::MarkdownTableMaxColumns {
                path: collected,
                max: 4,
            },
        ];
        for assertion in vacuous {
            let mut fixture = Fixture::load(&fixture_root()).unwrap();
            completion_mut(&mut fixture, "C04").assert = Some(assertion.clone());
            let err = fixture.validate().unwrap_err().to_string();
            assert!(
                err.contains("task T05's precondition on the path"),
                "{assertion:?} must not carry T05, got: {err}"
            );
        }

        // The event half: a count of zero passes with no event at all.
        let mut fixture = Fixture::load(&fixture_root()).unwrap();
        completion_mut(&mut fixture, "C05").assert = Some(Assertion::EventCount {
            event_type: "TriggerCreated".to_string(),
            count: 0,
        });
        let err = fixture.validate().unwrap_err().to_string();
        assert!(
            err.contains("task T09's precondition on the event `TriggerCreated`"),
            "{err}"
        );
    }

    /// A concrete completion path satisfies a precondition's glob over it. T03
    /// gates on `*-health.md`, and C02 asserts `2026-01-05-health.md`.
    #[test]
    fn a_concrete_completion_path_satisfies_a_glob_precondition() {
        assert!(guarantees_path(
            "artifacts/build-health/2026-01-05-health.md",
            "artifacts/build-health/*-health.md"
        ));
        assert!(guarantees_path(
            "artifacts/build-health/*-collected.json",
            "artifacts/build-health/*-collected.json"
        ));
        // Two different globs are not compared, which the doc comment says.
        assert!(!guarantees_path(
            "artifacts/build-health/*-health.md",
            "artifacts/**/*.md"
        ));
    }

    /// The seed tree is read, and `.gitkeep` is not a knowhow entry.
    #[test]
    fn the_seed_tree_is_read_and_carries_no_knowhow_entry() {
        let fixture = Fixture::load(&fixture_root()).unwrap();
        assert!(fixture
            .seed_files
            .contains(&"artifacts/build-health/collect.py".to_string()));
        assert!(
            !fixture
                .seed_files
                .iter()
                .any(|path| crate::assertions::glob_match("knowhow/**/*.md", path)),
            "a seeded knowhow entry would answer P05.4 and P06.1 before the run"
        );
    }

    /// ADR 0110 decision 5, from the failing direction, on every surface.
    ///
    /// A criterion naming a verb, a surface or a retired route is scoring the
    /// mechanism rather than the work. The rule was written down once before
    /// and eleven probes still shipped scoring a spelling, which is why it is
    /// a check.
    #[test]
    fn a_criterion_naming_a_mode_internal_is_refused() {
        /// Where the offending text goes, and the doctoring that puts it there.
        type Case = (&'static str, &'static dyn Fn(&mut Fixture));

        let cases: [Case; 5] = [
            ("probe P02.1's wrong_default", &|fixture: &mut Fixture| {
                fixture.probes[0].wrong_default = "it forgot to keep open the read".into();
            }),
            ("task T01's prompt", &|fixture: &mut Fixture| {
                task_mut(fixture, "T01").prompt = "{marker} use the working understanding".into();
            }),
            ("the scope rule", &|fixture: &mut Fixture| {
                fixture.scope_rule = "Do what this asks. Read the context panel.".into();
            }),
            (
                "completion probe C01's deliverable",
                &|fixture: &mut Fixture| {
                    completion_mut(fixture, "C01").deliverable =
                        "the curated bodies survive".into();
                },
            ),
            ("judge rubric triage", &|fixture: &mut Fixture| {
                fixture.judge.rubric.insert(
                    "triage".into(),
                    "Did the sweep take it? Reply yes or no.".into(),
                );
            }),
        ];
        for (where_it_is, doctor) in cases {
            let mut fixture = Fixture::load(&fixture_root()).unwrap();
            doctor(&mut fixture);
            let err = fixture.validate().unwrap_err().to_string();
            assert!(
                err.contains(where_it_is) && err.contains("internal of the context mode"),
                "expected {where_it_is} to be refused, got: {err}"
            );
        }
    }

    /// An assertion is scanned too, so a regex cannot smuggle a verb past.
    #[test]
    fn an_assertion_naming_a_mode_internal_is_refused() {
        let mut fixture = Fixture::load(&fixture_root()).unwrap();
        fixture.probes[0].assert = Some(Assertion::ResponseMatches {
            regex: "(?i)keep open".into(),
        });
        let err = fixture.validate().unwrap_err().to_string();
        assert!(err.contains("assertion names `keep open`"), "{err}");
    }

    /// The corpus lives in `artifacts/ledger-migration`, and several probes
    /// name its paths. A bare `ledger` in the list would refuse the fixture for
    /// saying where a document is, so the token is the two-word phrase.
    #[test]
    fn the_seeded_corpus_path_is_not_mistaken_for_a_mode_internal() {
        assert!(BANNED_MECHANISM_TOKENS.contains(&"context ledger"));
        assert!(!BANNED_MECHANISM_TOKENS.contains(&"ledger"));
        Fixture::load(&fixture_root()).expect("the checked-in fixture names no mode internal");
    }

    /// I9, from the failing direction.
    #[test]
    fn a_probe_declaring_both_scorers_is_refused() {
        let root = fixture_root();
        let mut fixture = Fixture::load(&root).unwrap();
        let probe = fixture
            .probes
            .iter_mut()
            .find(|p| p.assert.is_some())
            .expect("some probe carries an assertion");
        probe.judge = Some("triage".to_string());
        let err = fixture.validate().unwrap_err().to_string();
        assert!(err.contains("declares both `assert` and `judge`"));
    }

    #[test]
    fn a_probe_declaring_no_scorer_is_refused() {
        let root = fixture_root();
        let mut fixture = Fixture::load(&root).unwrap();
        let probe = fixture
            .probes
            .iter_mut()
            .find(|p| p.assert.is_some())
            .expect("some probe carries an assertion");
        probe.assert = None;
        let err = fixture.validate().unwrap_err().to_string();
        assert!(err.contains("declares neither"));
    }

    /// The ceiling is crossed by material, not by how much the model writes.
    ///
    /// The engine reserves the response and converts the rest at 1.5 chars per
    /// token, leaving roughly 172,700 chars for messages. This corpus exceeds
    /// that on its own, so the claim is checkable rather than asserted.
    #[test]
    fn the_ceiling_corpus_can_fill_the_context_window() {
        const CORPUS: &str = "artifacts/ledger-migration";
        const MAX_BYTES: u64 = 48_000;
        const MIN_TOTAL: u64 = 180_000;

        let dir = fixture_root().join(SEED_TREE).join(CORPUS);
        let mut docs: Vec<(String, u64)> = std::fs::read_dir(&dir)
            .expect("the ceiling corpus is seeded")
            .map(|entry| {
                let entry = entry.expect("the corpus directory is readable");
                let size = entry.metadata().expect("each document has metadata").len();
                (entry.file_name().to_string_lossy().into_owned(), size)
            })
            .collect();
        docs.sort();

        let names: Vec<&str> = docs.iter().map(|(name, _)| name.as_str()).collect();
        assert_eq!(
            names,
            [
                "01-vendor-replay-api.md",
                "02-ci-log-2026-07-14.md",
                "03-incident-postmortem.md",
                "04-cutover-runbook.md",
                "05-field-dictionary.md",
            ]
        );

        // `read_file` truncates at 50,000 bytes, so a document near that cap
        // would lose its tail and take a planted fact with it.
        for (name, size) in &docs {
            assert!(
                *size < MAX_BYTES,
                "{name} is {size} bytes, too close to the read tool's cap"
            );
        }
        let total: u64 = docs.iter().map(|(_, size)| size).sum();
        assert!(
            total > MIN_TOTAL,
            "the corpus is {total} bytes, too small to cross the ceiling"
        );
    }

    /// A lost fact must be answered by a guess, never by a misread.
    ///
    /// Each phrase is the round number a model reaches for when the document
    /// is gone: a day, five hundred, half an hour. None of them is seeded
    /// anywhere, so a probe matching one is measuring the model's prior.
    #[test]
    fn no_seeded_file_states_a_plausible_wrong_default() {
        const WRONG: [&str; 5] = ["24 hour", "24-hour", "500 record", "30 minute", "30-minute"];

        let root = fixture_root();
        let fixture = Fixture::load(&root).unwrap();
        let seed = root.join(SEED_TREE);
        for path in &fixture.seed_files {
            let bytes = std::fs::read(seed.join(path)).expect("a seeded file is readable");
            let text = String::from_utf8_lossy(&bytes).to_lowercase();
            for phrase in WRONG {
                assert!(
                    !text.contains(phrase),
                    "{path} states \"{phrase}\", which a probe would read as a hit"
                );
            }
        }
    }
}
