use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use lucidos_engine::core::prefs::{self, Pref, Text};
use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::assertions::Assertion;

type Fallible<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

pub const SEED_TREE: &str = "fixtures/workspace";

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
    pub scope_rule: String,
    pub task: Vec<Task>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Task {
    pub id: String,
    pub title: String,
    pub prompt: String,
    pub timeout_minutes: u64,
    #[serde(default)]
    pub followup: Option<String>,
    #[serde(default)]
    pub depends_on: Vec<String>,
    #[serde(default)]
    pub preconditions: Vec<Assertion>,
}

impl Task {
    pub fn timeout(&self) -> std::time::Duration {
        std::time::Duration::from_secs(self.timeout_minutes * 60)
    }

    pub fn rendered_prompt(&self, marker: &str, scope_rule: &str) -> String {
        render(&self.prompt, marker, scope_rule)
    }

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
    pub tier: u8,
    pub established_in: String,
    #[serde(default)]
    pub established_by: Established,
    pub census_regex: String,
    pub stated_regex: String,
    pub topic_regex: String,
}

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
    #[serde(default)]
    pub fact: Option<String>,
    #[serde(default)]
    pub tier: Option<u8>,
    pub wrong_default: String,
    #[serde(default)]
    pub assert: Option<Assertion>,
    #[serde(default)]
    pub judge: Option<String>,
    #[serde(default)]
    pub judge_passes_when: JudgeAnswer,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompletionFile {
    pub completion: Vec<CompletionProbe>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompletionProbe {
    pub id: String,
    pub task: String,
    pub deliverable: String,
    #[serde(default)]
    pub assert: Option<Assertion>,
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
    pub model: String,
    pub votes: u32,
    pub disagreement_ceiling: f64,
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
    pub matches: String,
    pub answer: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PricesFile {
    pub model: Vec<ModelPrice>,
}

const AUXILIARY_MODELS: [&Pref<Text>; 1] = [&prefs::MODEL_TITLE];

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ModelPrice {
    pub id: String,
    pub input_per_mtok: f64,
    pub output_per_mtok: f64,
    pub cache_read_per_mtok: f64,
    pub cache_creation_per_mtok: f64,
}

#[derive(Debug, Clone)]
pub struct Fixture {
    pub tasks: Vec<Task>,
    pub facts: Vec<Fact>,
    pub probes: Vec<Probe>,
    pub completion: Vec<CompletionProbe>,
    pub judge: JudgeConfig,
    pub answers: Vec<TaskAnswers>,
    pub scope_rule: String,
    pub prices: Vec<ModelPrice>,
    pub seed_files: Vec<String>,
    pub seed_root: PathBuf,
    pub fixture_hash: String,
    pub guidance_hash: String,
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

    pub fn check_models_priced(&self, under_test: &str) -> Fallible<()> {
        let mut wanted = BTreeSet::from([under_test.to_string()]);
        for pref in AUXILIARY_MODELS {
            wanted.insert(pref.default_text().to_string());
        }
        wanted.insert(prefs::BACKGROUND_MODEL.to_string());
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

    pub fn completion_for(&self, task: &str) -> Option<&CompletionProbe> {
        self.completion.iter().find(|c| c.task == task)
    }

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

fn rendered_assertion(label: &str, assertion: Option<&Assertion>) -> Option<(String, String)> {
    let rendered = serde_json::to_string(assertion?).ok()?;
    Some((label.to_string(), rendered))
}

pub const BANNED_MECHANISM_TOKENS: [&str; 18] = [
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
    "summary tree",
    "memory view",
    "view snapshot",
    "compactor",
];

impl Fixture {
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
    fn stated_in_seed_tree(&self, fact: &Fact) -> bool {
        let Ok(pattern) = regex::Regex::new(&fact.census_regex) else {
            return false;
        };
        self.seed_files.iter().any(|path| {
            std::fs::read(self.seed_root.join(path))
                .is_ok_and(|bytes| pattern.is_match(&String::from_utf8_lossy(&bytes)))
        })
    }

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

    pub fn is_voided_by(&self, task: &str, failed: &BTreeSet<String>) -> bool {
        self.upstream_of(task).iter().any(|up| failed.contains(up))
    }
}

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

#[derive(Default)]
struct Reads<'a> {
    paths: Vec<&'a str>,
    events: Vec<&'a str>,
}

impl<'a> Reads<'a> {
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

fn guarantees_path(guarantee: &str, wanted: &str) -> bool {
    guarantee == wanted
        || (!guarantee.contains('*') && crate::assertions::glob_match(wanted, guarantee))
}

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

    fn fixture_root() -> std::path::PathBuf {
        crate::eval_fixture_root().join("context-mode")
    }

    #[test]
    fn the_checked_in_prices_cover_the_model_under_test_and_the_auxiliaries() {
        Fixture::load(&fixture_root())
            .expect("the checked-in fixture must be valid")
            .check_models_priced(crate::DEFAULT_MODEL)
            .expect("prices.toml prices every model a default run bills");
    }

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

    #[test]
    fn every_auxiliary_model_key_still_names_a_model() {
        for pref in AUXILIARY_MODELS {
            assert!(
                matches!(pref.spec.default, prefs::PrefDefault::Value(_)),
                "{} now inherits another key rather than naming a model: {}",
                pref.key(),
                pref.spec.default_label()
            );
        }
    }

    #[test]
    fn the_seed_pins_every_memory_task_to_the_background_model() {
        let seed = std::fs::read_to_string(fixture_root().join("fixtures/seed.sql"))
            .expect("the checked-in seed");
        for pref in [
            &prefs::MODEL_MEMORY,
            &prefs::MODEL_QUERY_CLASSIFICATION,
            &prefs::MODEL_CONVERSATION_SUMMARY,
            &prefs::MODEL_MEMORY_FIND,
        ] {
            let row = format!("('{}',", pref.key());
            let line = seed
                .lines()
                .find(|line| line.trim_start().starts_with(&row))
                .unwrap_or_else(|| panic!("seed.sql does not pin {}", pref.key()));
            assert!(line.contains(":'background_model'"), "{line}");
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

    fn fact_mut<'a>(fixture: &'a mut Fixture, id: &str) -> &'a mut Fact {
        fixture
            .facts
            .iter_mut()
            .find(|fact| fact.id == id)
            .expect("the register carries it")
    }

    #[test]
    fn a_probed_fact_the_establishing_prompt_never_states_is_refused() {
        let mut fixture = Fixture::load(&fixture_root()).unwrap();
        fact_mut(&mut fixture, "F03").stated_regex = "nothing any prompt says".to_string();
        let err = fixture.validate().unwrap_err().to_string();
        assert!(err.contains("fact F03 is probed"), "{err}");
        assert!(err.contains("in its prompt"), "{err}");
    }

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

    #[test]
    fn a_judged_completion_probe_is_refused() {
        let mut fixture = Fixture::load(&fixture_root()).unwrap();
        fixture.completion[0].judge = Some("triage".to_string());
        let err = fixture.validate().unwrap_err().to_string();
        assert!(err.contains("names a judge rubric"));
    }

    fn task_mut<'a>(fixture: &'a mut Fixture, id: &str) -> &'a mut Task {
        fixture
            .tasks
            .iter_mut()
            .find(|task| task.id == id)
            .expect("the task set carries it")
    }

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

        let mut fixture = Fixture::load(&fixture_root()).unwrap();
        task_mut(&mut fixture, "T06").preconditions = vec![Assertion::Any {
            of: vec![knowhow, seeded],
        }];
        fixture.validate().expect("the seeded branch carries it");
    }

    fn completion_mut<'a>(fixture: &'a mut Fixture, id: &str) -> &'a mut CompletionProbe {
        fixture
            .completion
            .iter_mut()
            .find(|probe| probe.id == id)
            .expect("the completion set carries it")
    }

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
        assert!(!guarantees_path(
            "artifacts/build-health/*-health.md",
            "artifacts/**/*.md"
        ));
    }

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

    #[test]
    fn a_criterion_naming_a_mode_internal_is_refused() {
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

    #[test]
    fn an_assertion_naming_a_mode_internal_is_refused() {
        let mut fixture = Fixture::load(&fixture_root()).unwrap();
        fixture.probes[0].assert = Some(Assertion::ResponseMatches {
            regex: "(?i)keep open".into(),
        });
        let err = fixture.validate().unwrap_err().to_string();
        assert!(err.contains("assertion names `keep open`"), "{err}");
    }

    #[test]
    fn the_seeded_corpus_path_is_not_mistaken_for_a_mode_internal() {
        assert!(BANNED_MECHANISM_TOKENS.contains(&"context ledger"));
        assert!(!BANNED_MECHANISM_TOKENS.contains(&"ledger"));
        Fixture::load(&fixture_root()).expect("the checked-in fixture names no mode internal");
    }

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
