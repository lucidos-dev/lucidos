//! The judge: two scoring jobs and one route classification, and nothing else.
//!
//! ADR 0087 decision 6 keeps the judge off the critical path. It scores the one
//! probe no assertion can express. It triages which threads a human reads. It
//! says whether a failing response admitted the gap. It never scores a probe an
//! assertion can express (I9), and it never sees the arm label.
//!
//! The judge calls its model through the engine's provider layer, so it reaches
//! every provider the engine reaches. [`Judge::connect`] gives the order that
//! picks one, and resolves it once before the first probe.

use std::collections::BTreeMap;
use std::sync::Arc;

use lucidos_engine::llm::model_registry::{self, ModelRegistry, ModelRouting, ProviderKind};
use lucidos_engine::llm::provider::{LlmProvider, Message, MessageContent};
use lucidos_engine::llm::provider_build::{
    build_active_provider, ProviderBuildContext, ProviderBuildOutcome,
};
use lucidos_engine::llm::vertex;
use serde::{Deserialize, Serialize};

use crate::arm::Arm;
use crate::config::JudgeConfig;

type Fallible<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

/// Env var that pins the judge to one provider.
pub const JUDGE_PROVIDER_VAR: &str = "LUCIDOS_EVAL_JUDGE_PROVIDER";

/// Every provider the judge can be pinned to, in the engine's own vocabulary.
const PROVIDER_KINDS: [ProviderKind; 7] = [
    ProviderKind::Vertex,
    ProviderKind::Anthropic,
    ProviderKind::OpenAi,
    ProviderKind::OpenRouter,
    ProviderKind::XAi,
    ProviderKind::OpenCodeFree,
    ProviderKind::Local,
];

/// The region the engine defaults to when nothing names one.
const DEFAULT_VERTEX_REGION: &str = "europe-west1";

/// The one question the preflight asks, when the judge session is built.
///
/// It checks that the model answers on the resolved provider, so it has to be
/// cheap and to need no judgement at all.
const PREFLIGHT_RUBRIC: &str = "Reply with one word: ready.";

/// Rubric that decides whether a trigger intent carries procedure (P05.5).
pub const RUBRIC_TRIGGER_PROCEDURE: &str = "trigger_intent_has_procedure";

/// Rubric that decides whether a response admitted it lacked the fact.
///
/// Route classification, not probe scoring: it separates `lost-loud` from
/// `lost-silent` after the assertion has already said the probe failed. I9 is
/// about which scorer decides pass and fail, and this one never does.
pub const RUBRIC_RESPONSE_DISCLAIMS: &str = "response_disclaims";

/// Rubric that scores a response's responsiveness for triage.
pub const RUBRIC_TRIAGE: &str = "triage";

/// One judge vote.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Vote {
    pub yes: bool,
}

/// The outcome of a judged probe.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JudgedProbe {
    pub probe: String,
    pub yes: bool,
    /// True when the votes were not unanimous.
    pub disagreed: bool,
}

/// Majority over an odd number of votes, plus whether they disagreed.
pub fn tally(probe: &str, votes: &[Vote]) -> JudgedProbe {
    let yes_count = votes.iter().filter(|v| v.yes).count();
    JudgedProbe {
        probe: probe.to_string(),
        yes: yes_count * 2 > votes.len(),
        disagreed: yes_count != 0 && yes_count != votes.len(),
    }
}

/// Whether a judged probe stays in the primary analysis.
///
/// Above the ceiling the probe is too noisy to carry a measurement, and the
/// results file says so rather than the number quietly standing.
pub fn keeps_primary_standing(judged: &[JudgedProbe], config: &JudgeConfig) -> bool {
    if judged.is_empty() {
        return true;
    }
    let disagreed = judged.iter().filter(|j| j.disagreed).count() as f64;
    disagreed / judged.len() as f64 <= config.disagreement_ceiling
}

/// One text variant per vote, with the listed lines rotated.
///
/// Rotation rather than a shuffle, so a re-run of the same judged probe asks
/// the same questions. The point is defeating a position effect, which a fixed
/// rotation does as well as randomness and reproducibly.
pub fn shuffled_variants(text: &str, votes: u32) -> Vec<String> {
    let lines: Vec<&str> = text.lines().collect();
    (0..votes.max(1))
        .map(|vote| {
            if lines.len() < 2 {
                return text.to_string();
            }
            let offset = (vote as usize) % lines.len();
            let mut rotated: Vec<&str> = lines[offset..].to_vec();
            rotated.extend_from_slice(&lines[..offset]);
            rotated.join("\n")
        })
        .collect()
}

/// A thread the triage pass looked at.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TriageRow {
    pub thread_id: uuid::Uuid,
    pub arm: Arm,
    pub task: String,
    /// The judge's responsiveness score, one to three.
    pub judged: u8,
    /// Whether the programmatic score called this thread's task a success.
    pub programmatic_pass: bool,
}

impl TriageRow {
    /// Whether the two scorers disagree about this thread.
    ///
    /// A three is a clear success and a one is a clear failure. A two is the
    /// judge hedging, which is not a disagreement with anything.
    pub fn disagrees(&self) -> bool {
        match self.judged {
            3 => !self.programmatic_pass,
            1 => self.programmatic_pass,
            _ => false,
        }
    }
}

/// Choose the human-read sample: disagreements first, stratified by arm and
/// task, to the configured fraction of threads.
///
/// Stratification is round-robin over the (arm, task) buckets, so one noisy
/// task cannot take the whole sample. Ties inside a bucket break on thread id,
/// which keeps the selection reproducible.
pub fn triage_sample(rows: &[TriageRow], config: &JudgeConfig) -> Vec<uuid::Uuid> {
    let target = ((rows.len() as f64) * config.triage_fraction).ceil() as usize;
    if target == 0 {
        return Vec::new();
    }
    let mut buckets: BTreeMap<(String, String), Vec<&TriageRow>> = BTreeMap::new();
    for row in rows.iter().filter(|row| row.disagrees()) {
        buckets
            .entry((row.arm.as_str().to_string(), row.task.clone()))
            .or_default()
            .push(row);
    }
    for bucket in buckets.values_mut() {
        bucket.sort_by_key(|row| row.thread_id);
    }
    let mut selected = Vec::new();
    let mut depth = 0usize;
    loop {
        let mut took_any = false;
        for bucket in buckets.values() {
            if let Some(row) = bucket.get(depth) {
                selected.push(row.thread_id);
                took_any = true;
                if selected.len() == target {
                    return selected;
                }
            }
        }
        if !took_any {
            return selected;
        }
        depth += 1;
    }
}

/// Refuse a judge that is any model in the set under test.
///
/// A set, not one model, because runs go in parallel now. This run pins one
/// model, and a sibling run against another provider pins a different one. A
/// judge checked against this run alone still grades its own output when the
/// sibling is the one producing it.
///
/// The harness cannot see a sibling, so the operator declares the set and this
/// checks against all of it. Unset, the set is this run's own model, which is
/// the check as it was.
pub fn check_judge_is_independent(
    config: &JudgeConfig,
    models_under_test: &[&str],
) -> Fallible<()> {
    if !models_under_test.contains(&config.model.as_str()) {
        return Ok(());
    }
    let others = models_under_test.len() - 1;
    let scope = match others {
        0 => "is the model under test".to_string(),
        1 => "is one of the 2 models under test".to_string(),
        _ => format!("is one of the {} models under test", others + 1),
    };
    Err(format!(
        "judge_is_the_subject: the judge model `{}` {scope}. A model scoring its own output \
         is not an instrument.",
        config.model
    )
    .into())
}

/// Prompt for one judge call. Carries no arm label and no probe id.
pub fn judge_prompt(rubric: &str, subject: &str) -> String {
    format!("{rubric}\n\n--- subject ---\n{subject}\n--- end subject ---")
}

/// The judge's provider and configuration, proven once before the first probe.
///
/// Scoring runs for minutes and appends as it goes. A credential fault has to
/// fail at the start, not halfway through the results file. So does a provider
/// that serves no such model.
pub struct Judge<'a> {
    provider: Arc<dyn LlmProvider>,
    pub config: &'a JudgeConfig,
}

impl<'a> Judge<'a> {
    /// Build the judge's provider from this machine's credentials.
    ///
    /// Which provider serves the judge model, in order:
    ///
    /// 1. `LUCIDOS_EVAL_JUDGE_PROVIDER`, when it is set. A value that is not a
    ///    provider name errors, and names the six that are.
    /// 2. Otherwise the engine's own routing, from the model id. The judge
    ///    reads no workspace database, so that is the prefix heuristic: a
    ///    `claude-` id goes to Vertex and a `gpt-` id to OpenAI.
    /// 3. When that provider is not configured here, and exactly one other is,
    ///    the judge uses that one and prints which.
    ///
    /// One cheap call then proves the pairing, so the printed line means the
    /// judge works rather than that a credential exists.
    pub async fn connect(config: &'a JudgeConfig) -> Fallible<Self> {
        let pinned = provider_override(std::env::var(JUDGE_PROVIDER_VAR).ok().as_deref())?;
        let registry = judge_registry(&config.model, pinned)?;
        let context = judge_context(&config.model, &registry);
        let provider = match build_active_provider(None, &context).await? {
            ProviderBuildOutcome::Install { llm, .. } => llm,
            ProviderBuildOutcome::FailFast => return Err(no_provider_message().into()),
        };
        let configured = provider.configured_providers().unwrap_or_default();
        let resolved = resolve_provider(&registry, &config.model, pinned, &configured)?;
        let judge = Self { provider, config };
        judge.preflight(resolved).await?;
        println!(
            "[eval] the judge runs {} on the {} provider.",
            config.model,
            resolved.as_str()
        );
        Ok(judge)
    }

    /// Ask the model one cheap question, to prove it answers on this provider.
    ///
    /// A configured provider is a credential, not a catalogue. Nothing offline
    /// knows which model ids a provider serves, so a prefix rule here would
    /// refuse combinations that work: OpenRouter serving `anthropic/claude-*`,
    /// or a local endpoint serving anything at all. One call is the only check
    /// that stays true as the catalogues move.
    async fn preflight(&self, resolved: ProviderKind) -> Fallible<()> {
        judge_call(self, PREFLIGHT_RUBRIC, "")
            .await
            .map_err(|err| {
                // The provider's own error goes last. It is often several lines,
                // so anything after it reads as debris.
                format!(
                    "the judge model `{}` did not answer on the {} provider. Either set \
                 {JUDGE_PROVIDER_VAR} to a provider that serves it, or pin a model that one \
                 does. The provider said: {err}",
                    self.config.model,
                    resolved.as_str()
                )
            })?;
        Ok(())
    }
}

/// Read the pinned provider out of the env value.
///
/// `ProviderKind::parse` falls back to Vertex on an unknown string, so a typo
/// would quietly route somewhere nobody asked for. The round-trip through
/// `as_str` is what turns that into an error.
fn provider_override(raw: Option<&str>) -> Fallible<Option<ProviderKind>> {
    let name = raw.unwrap_or_default().trim();
    if name.is_empty() {
        return Ok(None);
    }
    let kind = ProviderKind::parse(name);
    if kind.as_str() != name {
        return Err(format!(
            "{JUDGE_PROVIDER_VAR} is `{name}`, which is not a provider. Use one of: {}.",
            provider_list(&PROVIDER_KINDS)
        )
        .into());
    }
    Ok(Some(kind))
}

/// The registry the judge routes on: one row for the pinned provider, and
/// nothing else. Empty when nothing is pinned, which leaves the decision to the
/// engine's prefix heuristic.
fn judge_registry(model: &str, pinned: Option<ProviderKind>) -> Fallible<ModelRegistry> {
    let registry = model_registry::empty();
    if let Some(provider) = pinned {
        route_to(&registry, model, provider)?;
    }
    Ok(registry)
}

/// Point one model id at one provider. The registry handle is shared with the
/// built router, so a row written after the build still routes.
fn route_to(registry: &ModelRegistry, model: &str, provider: ProviderKind) -> Fallible<()> {
    registry
        .write()
        .map_err(|_| "the judge's model registry lock is poisoned")?
        .insert(
            model.to_string(),
            ModelRouting {
                provider,
                context_window: None,
            },
        );
    Ok(())
}

/// Everything the engine's provider build needs, with no database and no mock.
fn judge_context(model: &str, registry: &ModelRegistry) -> ProviderBuildContext {
    // The engine falls back to a `gcloud` subprocess last. The eval does not
    // need it: `project_from_files` reads the same two files directly.
    let project_id = std::env::var("VERTEX_PROJECT_ID")
        .ok()
        .filter(|id| !id.trim().is_empty())
        .or_else(vertex::adc::project_from_files)
        .unwrap_or_default();
    let token_cache = (!project_id.is_empty()).then(|| Arc::new(std::sync::Mutex::new(None)));
    let region =
        std::env::var("VERTEX_REGION").unwrap_or_else(|_| DEFAULT_VERTEX_REGION.to_string());
    ProviderBuildContext {
        default_model: model.to_string(),
        model_is_mock: false,
        vertex_project_id: project_id,
        vertex_location: vertex::location_handle(region),
        vertex_token_cache: token_cache,
        model_registry: registry.clone(),
        boot_without_provider: false,
    }
}

/// Which provider actually serves the judge, or an error naming what is missing.
fn resolve_provider(
    registry: &ModelRegistry,
    model: &str,
    pinned: Option<ProviderKind>,
    configured: &[ProviderKind],
) -> Fallible<ProviderKind> {
    let routed = model_registry::provider_kind_for(registry, model);
    if configured.contains(&routed) {
        return Ok(routed);
    }
    // One configured provider is one intention, so route to it rather than
    // failing over a default nobody chose. A pin is a choice, and stands.
    if let ([only], None) = (configured, pinned) {
        route_to(registry, model, *only)?;
        println!(
            "[eval] {} is not configured here, so the judge uses {}, the only one that is.",
            routed.as_str(),
            only.as_str()
        );
        return Ok(*only);
    }
    Err(missing_provider_message(model, routed, pinned, configured).into())
}

/// Provider names in the engine's vocabulary, for a message.
fn provider_list(kinds: &[ProviderKind]) -> String {
    kinds
        .iter()
        .map(|kind| kind.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

/// What to set to configure one provider. Names the credential the machine
/// running the eval is missing, rather than a key it may never have.
fn how_to_configure(kind: ProviderKind) -> &'static str {
    match kind {
        ProviderKind::Vertex => {
            "set VERTEX_PROJECT_ID, or run `gcloud auth application-default login`"
        }
        ProviderKind::Anthropic => "set ANTHROPIC_API_KEY",
        ProviderKind::OpenAi => "set OPENAI_API_KEY",
        ProviderKind::OpenRouter => "set LUCIDOS_OPENROUTER_API_KEY",
        ProviderKind::XAi => "set LUCIDOS_XAI_API_KEY",
        ProviderKind::OpenCodeFree => "set LUCIDOS_OPENCODE_FREE=1 (keyless, no account)",
        ProviderKind::Local => "set LUCIDOS_LOCAL_BASE_URL",
    }
}

/// The failure when this machine has no provider at all.
fn no_provider_message() -> String {
    let options = PROVIDER_KINDS
        .iter()
        .map(|kind| format!("{} ({})", kind.as_str(), how_to_configure(*kind)))
        .collect::<Vec<_>>()
        .join("; ");
    format!("no model provider is configured, so the judge cannot run. Configure one: {options}.")
}

/// The failure when the provider the judge wants is not one of this machine's.
fn missing_provider_message(
    model: &str,
    routed: ProviderKind,
    pinned: Option<ProviderKind>,
    configured: &[ProviderKind],
) -> String {
    let wanted = pinned.unwrap_or(routed);
    let head = match pinned {
        Some(kind) => format!("{JUDGE_PROVIDER_VAR} pins the judge to {}", kind.as_str()),
        None => format!("the judge model `{model}` routes to {}", routed.as_str()),
    };
    format!(
        "{head}, which is not configured here. Configured providers: {}. Either set \
         {JUDGE_PROVIDER_VAR} to one of those, or {} to reach {}.",
        provider_list(configured),
        how_to_configure(wanted),
        wanted.as_str()
    )
}

/// Ask the judge model one yes or no question.
pub async fn judge_vote(judge: &Judge<'_>, rubric: &str, subject: &str) -> Fallible<Vote> {
    Ok(Vote {
        yes: parse_vote(&judge_call(judge, rubric, subject).await?),
    })
}

/// Ask the judge to score one response for responsiveness, one to three.
pub async fn judge_score(judge: &Judge<'_>, rubric: &str, subject: &str) -> Fallible<u8> {
    Ok(parse_score(&judge_call(judge, rubric, subject).await?))
}

/// Send one rubric to the judge model and return its reply text.
///
/// The only function in this crate that calls a model provider, and the reason
/// the I4 lint forbids a test from naming it.
pub async fn judge_call(judge: &Judge<'_>, rubric: &str, subject: &str) -> Fallible<String> {
    let message = Message {
        role: "user".to_string(),
        content: MessageContent::Text(judge_prompt(rubric, subject)),
    };
    let response = judge
        .provider
        .chat(
            vec![message],
            Vec::new(),
            Some(&judge.config.model),
            None,
            None,
            None,
        )
        .await?;
    Ok(response.content.unwrap_or_default())
}

/// Read a one-to-three score out of the judge's reply.
///
/// An unreadable reply is a two: the judge hedging, which selects nothing for
/// the human sample. Reading it as a one or a three would invent a
/// disagreement out of a parse failure.
pub fn parse_score(text: &str) -> u8 {
    text.chars()
        .find_map(|c| c.to_digit(10))
        .filter(|digit| (1..=3).contains(digit))
        .map(|digit| digit as u8)
        .unwrap_or(2)
}

/// Read a yes or no out of the judge's reply, defaulting to no.
///
/// A reply the judge did not shape as an answer is not evidence for the claim,
/// so it counts against it.
pub fn parse_vote(text: &str) -> bool {
    text.trim()
        .to_ascii_lowercase()
        .trim_start_matches(|c: char| !c.is_alphanumeric())
        .starts_with("yes")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> JudgeConfig {
        JudgeConfig {
            model: "judge-model".into(),
            votes: 3,
            disagreement_ceiling: 0.20,
            triage_fraction: 0.10,
            rubric: BTreeMap::new(),
        }
    }

    fn votes(pattern: &[bool]) -> Vec<Vote> {
        pattern.iter().map(|yes| Vote { yes: *yes }).collect()
    }

    /// The judge model `probes.toml` pins, which routes to Vertex by shape.
    const MODEL: &str = "claude-haiku-4-5";

    fn routes_to(registry: &ModelRegistry, model: &str) -> ProviderKind {
        model_registry::provider_kind_for(registry, model)
    }

    #[test]
    fn every_provider_name_is_accepted_and_a_typo_names_the_ones_that_are() {
        for kind in PROVIDER_KINDS {
            assert_eq!(provider_override(Some(kind.as_str())).unwrap(), Some(kind));
        }
        assert_eq!(provider_override(None).unwrap(), None);
        assert_eq!(provider_override(Some("   ")).unwrap(), None);
        for typo in ["Vertex", "gemini", "claude"] {
            let err = provider_override(Some(typo)).unwrap_err().to_string();
            assert!(err.contains(JUDGE_PROVIDER_VAR), "{err}");
            assert!(err.contains("vertex, anthropic, openai"), "{err}");
        }
    }

    #[test]
    fn the_pin_decides_and_without_one_the_model_id_does() {
        let pinned = judge_registry(MODEL, Some(ProviderKind::Anthropic)).unwrap();
        assert_eq!(routes_to(&pinned, MODEL), ProviderKind::Anthropic);
        let bare = judge_registry(MODEL, None).unwrap();
        assert_eq!(routes_to(&bare, MODEL), ProviderKind::Vertex);
        assert_eq!(routes_to(&bare, "gpt-5.6"), ProviderKind::OpenAi);
    }

    #[test]
    fn the_routed_provider_wins_whenever_this_machine_has_it() {
        let registry = judge_registry(MODEL, None).unwrap();
        let configured = [ProviderKind::Vertex, ProviderKind::OpenAi];
        let resolved = resolve_provider(&registry, MODEL, None, &configured).unwrap();
        assert_eq!(resolved, ProviderKind::Vertex);
    }

    #[test]
    fn the_only_configured_provider_serves_a_judge_nobody_pinned() {
        let registry = judge_registry(MODEL, None).unwrap();
        let resolved =
            resolve_provider(&registry, MODEL, None, &[ProviderKind::Anthropic]).unwrap();
        assert_eq!(resolved, ProviderKind::Anthropic);
        // The router reads the same handle, so the fallback has to reach it.
        assert_eq!(routes_to(&registry, MODEL), ProviderKind::Anthropic);
    }

    #[test]
    fn a_missing_provider_is_named_with_what_this_machine_lacks() {
        let registry = judge_registry(MODEL, None).unwrap();
        let configured = [ProviderKind::OpenAi, ProviderKind::XAi];
        let err = resolve_provider(&registry, MODEL, None, &configured)
            .unwrap_err()
            .to_string();
        assert!(err.contains(MODEL), "{err}");
        assert!(err.contains("openai, xai"), "{err}");
        assert!(
            err.contains("gcloud auth application-default login"),
            "{err}"
        );
        // The old message sent every machine after a key it may never have.
        assert!(!err.contains("ANTHROPIC_API_KEY"), "{err}");
    }

    #[test]
    fn a_pin_stands_even_when_it_is_the_provider_that_is_missing() {
        let pinned = Some(ProviderKind::OpenAi);
        let registry = judge_registry(MODEL, pinned).unwrap();
        let err = resolve_provider(&registry, MODEL, pinned, &[ProviderKind::Vertex])
            .unwrap_err()
            .to_string();
        assert!(err.contains(JUDGE_PROVIDER_VAR), "{err}");
        assert!(err.contains("OPENAI_API_KEY"), "{err}");
        assert!(err.contains("Configured providers: vertex"), "{err}");
    }

    #[test]
    fn a_machine_with_no_provider_is_told_every_way_to_get_one() {
        let message = no_provider_message();
        for needle in [
            "VERTEX_PROJECT_ID",
            "gcloud auth application-default login",
            "ANTHROPIC_API_KEY",
            "OPENAI_API_KEY",
            "LUCIDOS_OPENROUTER_API_KEY",
            "LUCIDOS_XAI_API_KEY",
            "LUCIDOS_LOCAL_BASE_URL",
        ] {
            assert!(message.contains(needle), "{message}");
        }
    }

    #[test]
    fn a_majority_decides_and_unanimity_is_not_a_disagreement() {
        let unanimous = tally("P05.5", &votes(&[true, true, true]));
        assert!(unanimous.yes);
        assert!(!unanimous.disagreed);
        let split = tally("P05.5", &votes(&[true, true, false]));
        assert!(split.yes);
        assert!(split.disagreed);
        let against = tally("P05.5", &votes(&[false, true, false]));
        assert!(!against.yes);
        assert!(against.disagreed);
    }

    #[test]
    fn a_probe_over_the_disagreement_ceiling_leaves_the_primary_analysis() {
        let judged: Vec<JudgedProbe> = (0..10)
            .map(|i| JudgedProbe {
                probe: format!("P05.5-{i}"),
                yes: true,
                disagreed: i < 3,
            })
            .collect();
        assert!(!keeps_primary_standing(&judged, &config()));
        let quieter: Vec<JudgedProbe> = (0..10)
            .map(|i| JudgedProbe {
                probe: format!("P05.5-{i}"),
                yes: true,
                disagreed: i < 2,
            })
            .collect();
        assert!(keeps_primary_standing(&quieter, &config()));
    }

    #[test]
    fn every_variant_carries_the_same_lines_in_a_different_order() {
        let variants = shuffled_variants("a\nb\nc", 3);
        assert_eq!(variants.len(), 3);
        assert_eq!(variants[0], "a\nb\nc");
        assert_eq!(variants[1], "b\nc\na");
        assert_eq!(variants[2], "c\na\nb");
        for variant in &variants {
            let mut lines: Vec<&str> = variant.lines().collect();
            lines.sort();
            assert_eq!(lines, vec!["a", "b", "c"]);
        }
    }

    #[test]
    fn a_one_line_subject_is_repeated_rather_than_mangled() {
        assert_eq!(shuffled_variants("one line", 3), vec!["one line"; 3]);
    }

    #[test]
    fn the_prompt_never_carries_the_arm() {
        let prompt = judge_prompt("does it carry procedure?", "run the collector nightly");
        assert!(!prompt.contains("lean"));
        assert!(!prompt.contains("control"));
        assert!(prompt.contains("run the collector nightly"));
    }

    #[test]
    fn a_reply_that_is_not_an_answer_counts_against_the_claim() {
        assert!(parse_vote("yes"));
        assert!(parse_vote("  Yes, it does."));
        assert!(parse_vote("**yes**"));
        assert!(!parse_vote("no"));
        assert!(!parse_vote("I am not sure."));
        assert!(!parse_vote(""));
    }

    #[test]
    fn an_unreadable_score_is_a_hedge_and_never_a_disagreement() {
        assert_eq!(parse_score("3"), 3);
        assert_eq!(parse_score("  1 "), 1);
        assert_eq!(parse_score("score: 2"), 2);
        assert_eq!(parse_score("I would say it is fine"), 2);
        assert_eq!(parse_score("9"), 2);
    }

    #[test]
    fn the_judge_may_not_be_the_model_under_test() {
        check_judge_is_independent(&config(), &["model-under-test"]).unwrap();
        let err = check_judge_is_independent(&config(), &["judge-model"])
            .unwrap_err()
            .to_string();
        assert!(err.contains("judge_is_the_subject"), "{err}");
        assert!(err.contains("is the model under test"), "{err}");
    }

    /// The parallel-run case. This run pins something else, so the per-run
    /// check passes. The judge is still grading its own output, over in the
    /// sibling run that pinned it.
    #[test]
    fn the_judge_may_not_be_any_model_in_a_concurrent_set() {
        let set = ["model-under-test", "judge-model"];
        let err = check_judge_is_independent(&config(), &set)
            .unwrap_err()
            .to_string();
        assert!(err.contains("judge_is_the_subject"), "{err}");
        assert!(err.contains("one of the 2 models under test"), "{err}");

        // A set that does not name the judge is still allowed, however big.
        check_judge_is_independent(&config(), &["a", "b", "c"]).unwrap();
    }

    fn row(task: &str, arm: Arm, judged: u8, programmatic_pass: bool, seed: u128) -> TriageRow {
        TriageRow {
            thread_id: uuid::Uuid::from_u128(seed),
            arm,
            task: task.into(),
            judged,
            programmatic_pass,
        }
    }

    #[test]
    fn only_a_clear_judge_score_can_disagree() {
        assert!(row("T02", Arm::Lean, 3, false, 1).disagrees());
        assert!(row("T02", Arm::Lean, 1, true, 2).disagrees());
        assert!(!row("T02", Arm::Lean, 2, true, 3).disagrees());
        assert!(!row("T02", Arm::Lean, 3, true, 4).disagrees());
    }

    #[test]
    fn the_sample_is_a_tenth_of_the_threads_spread_across_the_buckets() {
        let mut rows = Vec::new();
        for index in 0..40u128 {
            let arm = if index % 2 == 0 {
                Arm::Control
            } else {
                Arm::Lean
            };
            let task = if index < 20 { "T02" } else { "T05" };
            rows.push(row(task, arm, 3, false, index + 1));
        }
        let selected = triage_sample(&rows, &config());
        assert_eq!(selected.len(), 4);
        let mut unique = selected.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(unique.len(), 4, "the sample must not repeat a thread");
    }

    #[test]
    fn threads_the_scorers_agree_on_are_never_sampled() {
        let rows: Vec<TriageRow> = (0..40u128)
            .map(|index| row("T02", Arm::Lean, 3, true, index + 1))
            .collect();
        assert!(triage_sample(&rows, &config()).is_empty());
    }
}
