//! What the two turn-end decision checks share: the backend a candidate runs
//! on, the row each decision writes, and the table row a candidate scores.
//! `read_decision` and `wake_decision` each own their labelled set and the
//! engine entry point that decides one case.
//!
//! The number that decides first is a missed yes: a reply the user needed, or
//! a report the parent had to act on. Noise, a yes the case did not need,
//! decides second.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use lucidos_engine::engine::OpeningSource;
use lucidos_engine::llm::judgment::{ChatJudgmentProvider, JudgmentProvider, SystemOneEndpoint};
use lucidos_engine::llm::LlmProvider;
use serde::Serialize;

use crate::candidate::{candidate, percentile, pinned_provider, provider_for};
use crate::Fallible;

/// The `--backend` value that asks chat models.
pub const CHAT_BACKEND: &str = "chat";

/// How long one System One call may take. Generous, so a slow answer is
/// measured rather than cut.
const SYSTEM_ONE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

/// The engine's chat backend for one decision, so the eval asks chat the way
/// the gate does.
pub(crate) type ChatJudge = fn(Arc<dyn LlmProvider>, Option<String>) -> ChatJudgmentProvider;

/// What a check is called, and what its table calls a missed yes and noise.
pub(crate) struct Check {
    /// The subcommand, which also names the check's folder under `eval/`.
    pub(crate) command: &'static str,
    pub(crate) name: &'static str,
    pub(crate) chat_judge: ChatJudge,
    pub(crate) missed: &'static str,
    pub(crate) noise: &'static str,
}

impl Check {
    /// The shipped labelled set, `eval/<command>/turns.toml`.
    pub(crate) fn shipped_set(&self) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join(format!("../../eval/{}/turns.toml", self.command))
    }

    /// Where a run appends its decisions unless told otherwise.
    pub(crate) fn default_out(&self) -> PathBuf {
        PathBuf::from(format!("/tmp/{}-results.jsonl", self.command))
    }
}

/// One labelled case a check decides.
pub(crate) trait Labelled {
    /// The case's shape, for the per-kind breakdown.
    fn kind(&self) -> &str;
    /// Whether the right answer is yes.
    fn expected(&self) -> bool;
    /// Refuse a case the gate would never ask about, before any call.
    fn validate(&self) -> Fallible<()>;
    /// Decide the case on `provider`, through the engine's entry point.
    async fn decide(&self, provider: &dyn JudgmentProvider) -> Fallible<Answer>;
}

/// One decision a backend gave.
pub(crate) struct Answer {
    pub(crate) yes: bool,
    pub(crate) probability: Option<f64>,
    pub(crate) input_tokens: u32,
    pub(crate) output_tokens: u32,
}

/// What opened a turn, as a labelled set writes it: `user`, `child-report`,
/// `trigger` or `agent`.
pub(crate) fn opening_source(source: &str) -> Fallible<OpeningSource> {
    Ok(match source {
        "user" => OpeningSource::UserMessage,
        "child-report" => OpeningSource::ChildThreadReport,
        "trigger" => OpeningSource::TriggerRun,
        "agent" => OpeningSource::AgentOrEngine,
        other => return Err(format!("unknown source {other:?}").into()),
    })
}

/// One decision, as written to the results file.
#[derive(Debug, Serialize)]
struct Decided<'a> {
    config: &'a str,
    /// The case's position in its set, so repeat runs pair up by case.
    case: usize,
    kind: &'a str,
    expected: bool,
    answer: Option<bool>,
    /// The probability of yes, so another threshold can be scored offline.
    probability: Option<f64>,
    error: Option<String>,
    latency_ms: u128,
    input_tokens: u32,
    output_tokens: u32,
}

/// The candidates `backend` names, each with the provider that answers it.
async fn candidates(
    check: &Check,
    backend: &str,
    models: &[String],
) -> Fallible<Vec<(String, Box<dyn JudgmentProvider>)>> {
    if backend == CHAT_BACKEND {
        let pinned = pinned_provider()?;
        let mut built = Vec::with_capacity(models.len());
        for spec in models {
            let candidate = candidate(spec);
            let provider = provider_for(&candidate, pinned).await?;
            let judge = (check.chat_judge)(provider, candidate.effort.map(str::to_string));
            built.push((spec.clone(), Box::new(judge) as Box<dyn JudgmentProvider>));
        }
        return Ok(built);
    }
    let endpoint = SystemOneEndpoint::from_id(backend)
        .ok_or_else(|| format!("unknown backend {backend:?}: chat, or a System One row id"))?;
    // The row's key resolves as the engine's does: the stored credential,
    // then the environment. An agent's read-only `LUCIDOS_DATABASE_URL` names
    // the workspace first: an agent's `DATABASE_URL` is the user's own project
    // setting (ADR 0481). By hand, `DATABASE_URL` or libpq's `PG*` do.
    let url = std::env::var("LUCIDOS_DATABASE_URL").or_else(|_| std::env::var("DATABASE_URL"));
    let options = match url {
        Ok(url) => url.parse()?,
        Err(_) => sqlx::postgres::PgConnectOptions::new(),
    };
    let pool = sqlx::PgPool::connect_with(options).await?;
    let provider = endpoint.provider(&pool, SYSTEM_ONE_TIMEOUT).await?;
    eprintln!("[eval] {} on {}", check.name, endpoint.label());
    Ok(vec![(endpoint.id().to_string(), provider)])
}

/// Run every candidate `backend` names over `cases`, append each decision to
/// `out` as JSON lines, and print the table.
pub(crate) async fn run<C: Labelled>(
    check: &Check,
    cases: &[C],
    backend: &str,
    models: &[String],
    out: &Path,
) -> Fallible<()> {
    for case in cases {
        case.validate()
            .map_err(|e| format!("a {} case: {e}", case.kind()))?;
    }
    let candidates = candidates(check, backend, models).await?;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(out)?;
    println!(
        "| Config | Answered | Agreement | {missed} | {missed} by kind | {noise} \
         | {noise} by kind | p50 ms | p95 ms | Tokens in / out |",
        missed = check.missed,
        noise = check.noise,
    );
    println!("|---|---|---|---|---|---|---|---|---|---|");
    for (spec, provider) in &candidates {
        let mut rows = Vec::with_capacity(cases.len());
        for (index, case) in cases.iter().enumerate() {
            let started = Instant::now();
            let (answer, error) = match case.decide(provider.as_ref()).await {
                Ok(answer) => (Some(answer), None),
                Err(err) => (None, Some(err.to_string())),
            };
            let row = Decided {
                config: spec,
                case: index,
                kind: case.kind(),
                expected: case.expected(),
                answer: answer.as_ref().map(|a| a.yes),
                probability: answer.as_ref().and_then(|a| a.probability),
                error,
                latency_ms: started.elapsed().as_millis(),
                input_tokens: answer.as_ref().map_or(0, |a| a.input_tokens),
                output_tokens: answer.as_ref().map_or(0, |a| a.output_tokens),
            };
            writeln!(file, "{}", serde_json::to_string(&row)?)?;
            rows.push(row);
        }
        println!("{}", summary_row(spec, &rows));
    }
    Ok(())
}

/// `kind wrong/all` for each kind among `rows` that answered wrong at least
/// once.
fn wrong_by_kind(rows: &[&Decided<'_>]) -> String {
    let mut kinds: BTreeMap<&str, (usize, usize)> = BTreeMap::new();
    for r in rows {
        let (wrong, all) = kinds.entry(r.kind).or_default();
        *all += 1;
        *wrong += usize::from(r.answer == Some(!r.expected));
    }
    kinds
        .iter()
        .filter(|(_, (wrong, _))| *wrong > 0)
        .map(|(kind, (wrong, all))| format!("{kind} {wrong}/{all}"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// One table row for a candidate.
fn summary_row(spec: &str, rows: &[Decided<'_>]) -> String {
    let answered: Vec<&Decided<'_>> = rows.iter().filter(|r| r.answer.is_some()).collect();
    let (yes, noise): (Vec<&Decided<'_>>, Vec<&Decided<'_>>) =
        answered.iter().partition(|r| r.expected);
    let agree = answered
        .iter()
        .filter(|r| r.answer == Some(r.expected))
        .count();
    let missed = yes.iter().filter(|r| r.answer == Some(false)).count();
    let passed = noise.iter().filter(|r| r.answer == Some(true)).count();
    let mut latencies: Vec<u128> = answered.iter().map(|r| r.latency_ms).collect();
    latencies.sort_unstable();
    let tokens_in: u32 = answered.iter().map(|r| r.input_tokens).sum();
    let tokens_out: u32 = answered.iter().map(|r| r.output_tokens).sum();
    let mean = |total: u32| total.checked_div(answered.len() as u32).unwrap_or(0);
    format!(
        "| {spec} | {}/{} | {agree}/{} | {missed}/{} | {} | {passed}/{} | {} | {} | {} | {} / {} |",
        answered.len(),
        rows.len(),
        answered.len(),
        yes.len(),
        wrong_by_kind(&yes),
        noise.len(),
        wrong_by_kind(&noise),
        percentile(&latencies, 0.5),
        percentile(&latencies, 0.95),
        mean(tokens_in),
        mean(tokens_out),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(kind: &'static str, expected: bool, answer: Option<bool>) -> Decided<'static> {
        Decided {
            config: "c",
            case: 0,
            kind,
            expected,
            answer,
            probability: None,
            error: None,
            latency_ms: 10,
            input_tokens: 100,
            output_tokens: 2,
        }
    }

    /// A missed yes and a passed noise case each count once, under their
    /// kind, and an unanswered case counts in neither.
    #[test]
    fn the_row_counts_misses_and_noise_by_kind() {
        let rows = [
            row("pipeline", true, Some(false)),
            row("pipeline", true, Some(true)),
            row("fan-in", false, Some(true)),
            row("fan-in", false, Some(false)),
            row("research", false, None),
        ];
        assert_eq!(
            summary_row("c", &rows),
            "| c | 4/5 | 2/4 | 1/2 | pipeline 1/2 | 1/2 | fan-in 1/2 | 10 | 10 | 100 / 2 |"
        );
    }
}
