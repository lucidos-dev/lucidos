//! The *intelligence ladder*: one *ladder surface*'s picks, each an
//! *intelligence rung* (one model at one effort), least to most intelligent.
//! The surfaces are chat (the Lucidos Agent), Claude Code and Codex.
//!
//! The *rung estimates* (`llm/rung_estimates.json`) hold every estimated
//! pair's bands, and each surface's shipped pairs in order with whether each
//! is on. An owner's override replaces the on rungs with an ordered list.
//! Cost per task is the token band times the price table's output price, so
//! a price change moves it (ADR 0500, ADR 0525).

use std::collections::HashMap;

use serde::Serialize;
use sqlx::PgPool;

use super::CodingAgent;
use crate::core::preference_catalog as prefs;
use crate::llm::model_prices::{self, ModelPrice};
use crate::llm::model_registry::{self, ModelRegistry, ProviderKind};

use crate::llm::intelligence_rungs::{intelligence_rung_model_base, CONTEXT_VARIANT};
pub use crate::llm::intelligence_rungs::{
    rung_estimates, EstimatedPair, IntelligenceRung, LadderSurface, RungEstimates, SurfacePair,
    SurfacePairs,
};

/// Whether `pick` is the model `current` runs, minus the *context variant*
/// `current` runs. A rung names a bare model, so such a pick means `current`
/// exactly: the variant is part of the prompt cache's key.
pub(crate) fn drops_context_variant(agent: CodingAgent, pick: &str, current: &str) -> bool {
    let runs_variant = |id: &str| model_run_by(agent, id).ends_with(CONTEXT_VARIANT);
    runs_variant(current)
        && !runs_variant(pick)
        && canonical_model(agent.into(), pick) == canonical_model(agent.into(), current)
}

/// The concrete model `model` runs on `agent`: an alias row the backend
/// resolved (`opus` running `claude-opus-5-5`) answers with that model.
pub(crate) fn model_run_by(agent: CodingAgent, model: &str) -> String {
    let options = super::coding_agent_model_options(agent);
    crate::llm::coding_agent_models::offered_row(&options, model)
        .map(|row| row.resolved_model.as_deref().unwrap_or(&row.value))
        .unwrap_or(model)
        .to_string()
}

/// Whether `a` and `b` run the same concrete model on `agent`, *context
/// variant* included: the variant is part of the prompt cache's key.
pub(crate) fn runs_same_model(agent: CodingAgent, a: &str, b: &str) -> bool {
    let run =
        |id: &str| crate::llm::intelligence_rungs::strip_version_pin(&model_run_by(agent, id));
    run(a) == run(b)
}

/// The model `model` runs on `surface`, as the rung estimates name it. Chat
/// picks name registry ids, so only the decorations come off.
fn canonical_model(surface: LadderSurface, model: &str) -> String {
    match surface.coding_agent() {
        Some(agent) => intelligence_rung_model_base(&model_run_by(agent, model)),
        None => intelligence_rung_model_base(model),
    }
}

fn same_intelligence_rung(
    surface: LadderSurface,
    a: &IntelligenceRung,
    b: &IntelligenceRung,
) -> bool {
    a.effort == b.effort && canonical_model(surface, &a.model) == canonical_model(surface, &b.model)
}

/// Whether `agent` offers `model` at `effort` to a new spawn.
pub fn is_offered(agent: CodingAgent, model: &str, effort: &str) -> bool {
    super::validate_coding_agent_model(agent, Some(model)).is_ok()
        && super::validate_coding_agent_effort(agent, Some(model), Some(effort)).is_ok()
}

/// What a surface reads from the world to serve its ladder.
pub trait SurfaceOffer {
    /// Whether the surface offers `model` at `effort` to a new pick.
    fn offers(&self, model: &str, effort: &str) -> bool;
    /// The model's picker label, else its id.
    fn label(&self, model: &str) -> String;
    /// Availability controls defaults, while offers controls catalogue and overrides.
    fn available(&self, model: &str, effort: &str) -> bool {
        self.offers(model, effort)
    }
    /// Whether the route in force for `model` bills nobody, so its cost reads
    /// "included".
    fn is_free(&self, model: &str) -> bool;
    /// The rung each offered model joins the ladder at, in display order. A
    /// model the ladder names nowhere else lists off on it, unbanded.
    fn joining_intelligence_rungs(&self) -> Vec<IntelligenceRung> {
        Vec::new()
    }
}

/// A coding agent's offer: its menu. Its cost reads at list price.
pub struct CodingAgentOffer(pub CodingAgent);

impl SurfaceOffer for CodingAgentOffer {
    fn offers(&self, model: &str, effort: &str) -> bool {
        is_offered(self.0, model, effort)
    }

    fn label(&self, model: &str) -> String {
        let options = super::coding_agent_model_options(self.0);
        crate::llm::coding_agent_models::offered_row(&options, model)
            .map(|row| row.label.clone())
            .unwrap_or_else(|| model.to_string())
    }

    fn is_free(&self, _model: &str) -> bool {
        false
    }
}

/// The model registry and the backends this engine reaches, which chat's
/// offer and every surface's pick filling read.
#[derive(Clone)]
pub struct ModelRoutes {
    pub registry: ModelRegistry,
    pub configured: Vec<ProviderKind>,
    pub provider: Option<std::sync::Arc<dyn crate::llm::LlmProvider>>,
}

/// Chat's offer: the registry's current models at the tiers one of their
/// routes takes, priced on the route the router would take.
pub struct ChatOffer {
    pub routes: ModelRoutes,
    /// Registry labels by model id.
    pub labels: HashMap<String, String>,
    /// Account exclusions by backend and registry model id, as the refusal ledger keys them.
    pub unavailable_routes: std::collections::HashSet<(ProviderKind, String)>,
}

impl ChatOffer {
    /// The offer with the registry's labels, read from the `models` table.
    pub async fn load(pool: &PgPool, routes: ModelRoutes) -> Self {
        let labels = crate::core::ModelStore::list(pool)
            .await
            .inspect_err(|e| log_legacy!("[IntelligenceLadder] reading model labels failed: {e}"))
            .unwrap_or_default()
            .into_iter()
            .map(|m| (m.id, m.label))
            .collect();
        let mut unavailable_routes = std::collections::HashSet::new();
        let mut checked_routes = std::collections::HashSet::new();
        let entries: Vec<_> = routes
            .registry
            .read()
            .expect("model registry lock")
            .iter()
            .flat_map(|(model, row)| {
                row.routes
                    .iter()
                    .cloned()
                    .map(|route| (model.clone(), route))
                    .collect::<Vec<_>>()
            })
            .collect();
        if let Some(provider) = &routes.provider {
            let checks = entries
                .into_iter()
                .filter(|(model, route)| {
                    routes.configured.contains(&route.provider)
                        && checked_routes.insert((route.provider, model.clone()))
                })
                .map(|(model, route)| async move {
                    let available = provider
                        .route_available(route.provider, &model, &route.wire_id)
                        .await;
                    (model, route, available)
                });
            for (model, route, available) in futures::future::join_all(checks).await {
                match available {
                    Ok(true) => {}
                    Ok(false) => {
                        unavailable_routes.insert((route.provider, model));
                    }
                    Err(e) => {
                        log_legacy!(
                            "[IntelligenceLadder] availability unknown for {}: {e}",
                            route.wire_id
                        );
                        // An unreadable listing is unknown. Keep configured-route
                        // eligibility; only an authoritative refusal removes it.
                    }
                }
            }
        }
        Self {
            routes,
            labels,
            unavailable_routes,
        }
    }
}

impl SurfaceOffer for ChatOffer {
    fn available(&self, model: &str, effort: &str) -> bool {
        let Some(row) = model_registry::routing_for(&self.routes.registry, model) else {
            return false;
        };
        row.successor.is_none()
            && model_registry::resolve_route(&self.routes.registry, model, None, |kind| {
                self.routes.configured.contains(&kind)
            })
            .is_ok_and(|route| {
                !self
                    .unavailable_routes
                    .contains(&(route.provider, model.to_string()))
                    && crate::llm::reasoning::supported_efforts(route.provider, &route.wire_id)
                        .contains(&effort)
            })
    }

    fn offers(&self, model: &str, effort: &str) -> bool {
        model_registry::routing_for(&self.routes.registry, model).is_some_and(|row| {
            row.successor.is_none()
                && row.routes.iter().any(|route| {
                    crate::llm::reasoning::supported_efforts(route.provider, &route.wire_id)
                        .contains(&effort)
                })
        })
    }

    fn label(&self, model: &str) -> String {
        self.labels
            .get(model)
            .cloned()
            .unwrap_or_else(|| model.to_string())
    }

    fn is_free(&self, model: &str) -> bool {
        let configured = &self.routes.configured;
        model_registry::resolve_route(&self.routes.registry, model, None, |kind| {
            configured.contains(&kind)
        })
        .is_ok_and(|route| route.provider.is_free())
    }

    /// Every current registry model on the rung it joins at, each base model
    /// once, ordered by label.
    fn joining_intelligence_rungs(&self) -> Vec<IntelligenceRung> {
        let mut ids: Vec<String> = self
            .routes
            .registry
            .read()
            .map(|map| map.keys().cloned().collect())
            .unwrap_or_default();
        ids.sort_by_key(|id| (self.label(id), id.clone()));
        let mut seen = std::collections::HashSet::new();
        ids.iter()
            .filter_map(|id| chat_joining_intelligence_rung(&self.routes.registry, id))
            .filter(|pair| self.offers(&pair.model, &pair.effort))
            .filter(|pair| seen.insert(intelligence_rung_model_base(&pair.model)))
            .collect()
    }
}

/// The rung a chat model joins the ladder at: its *default effort*, else the
/// tier nearest `medium` its first route takes. It names the bare model when
/// the registry has a row for it, since a `[1m]` twin is the same rung.
/// `None` without a registry row.
pub fn chat_joining_intelligence_rung(
    registry: &ModelRegistry,
    model: &str,
) -> Option<IntelligenceRung> {
    let routing = model_registry::routing_for(registry, model)?;
    let effort = match routing.default_effort {
        Some(effort) => effort,
        None => {
            let route = routing.routes.first()?;
            crate::llm::reasoning::clamp_effort("medium", route.provider, &route.wire_id)?
        }
    };
    let base = intelligence_rung_model_base(model);
    let named = match model_registry::routing_for(registry, &base) {
        Some(_) => base,
        None => model.to_string(),
    };
    Some(IntelligenceRung::new(&named, effort))
}

/// The rungs `surface` binds agents to, least to most intelligent: the
/// owner's override when one is stored, else the shipped rungs that are on.
/// Either way only offered pairs survive, in the stored order.
pub fn effective_ladder(
    surface: LadderSurface,
    override_value: Option<&str>,
    offered: impl Fn(&str, &str) -> bool,
) -> Vec<IntelligenceRung> {
    match parsed_override(surface, override_value) {
        Some(pairs) => pairs
            .into_iter()
            .filter(|(model, effort)| offered(model, effort))
            .map(|(model, effort)| IntelligenceRung::new(model, effort))
            .collect(),
        None => shipped_on(surface, offered),
    }
}

fn parsed_override(surface: LadderSurface, value: Option<&str>) -> Option<Vec<(&str, &str)>> {
    value.and_then(|v| prefs::parse_intelligence_ladder(v, surface.efforts()).ok())
}

fn shipped_pair(pair: &SurfacePair) -> IntelligenceRung {
    IntelligenceRung::new(&pair.model, &pair.effort)
}

/// The rungs Copy from Lucidos Agent turns on for `target`: the chat rungs that
/// are on and that `target` offers, least to most intelligent on `target`'s
/// profile. A pair with no estimate follows the banded ones, in chat order.
pub fn copied_from_chat(
    target: LadderSurface,
    chat_on: &[IntelligenceRung],
    offered: impl Fn(&str, &str) -> bool,
) -> Vec<IntelligenceRung> {
    let mut kept: Vec<IntelligenceRung> = Vec::new();
    for pair in chat_on.iter().filter(|p| offered(&p.model, &p.effort)) {
        if !kept.iter().any(|k| same_intelligence_rung(target, k, pair)) {
            kept.push(pair.clone());
        }
    }
    let shipped_at = |pair: &IntelligenceRung| {
        target
            .shipped_pairs()
            .iter()
            .position(|p| same_intelligence_rung(target, &shipped_pair(p), pair))
    };
    kept.sort_by_key(|pair| {
        (
            intelligence_band(target, pair).unwrap_or(i32::MAX),
            shipped_at(pair).unwrap_or(usize::MAX),
        )
    });
    kept
}

/// The frontier of the offered subset, including its available shipped default.
pub(crate) fn shipped_on(
    surface: LadderSurface,
    offered: impl Fn(&str, &str) -> bool,
) -> Vec<IntelligenceRung> {
    surface
        .shipped_pairs()
        .iter()
        .filter(|p| {
            offered(&p.model, &p.effort)
                && (p.pinned || !p.dominators.iter().any(|d| offered(&d.model, &d.effort)))
        })
        .map(shipped_pair)
        .collect()
}

/// `on` plus every other offered shipped pair. An off pair follows the
/// shipped pair before it, so it keeps its place in the shipped order.
fn with_off_pairs(
    surface: LadderSurface,
    on: Vec<IntelligenceRung>,
    offered: impl Fn(&str, &str) -> bool,
) -> Vec<(IntelligenceRung, bool)> {
    let mut all: Vec<(IntelligenceRung, bool)> = on.into_iter().map(|p| (p, true)).collect();
    let mut cursor = 0;
    for shipped in surface.shipped_pairs() {
        let pair = shipped_pair(shipped);
        match all
            .iter()
            .position(|(p, _)| same_intelligence_rung(surface, p, &pair))
        {
            Some(at) => cursor = at + 1,
            None if offered(&pair.model, &pair.effort) => {
                all.insert(cursor, (pair, false));
                cursor += 1;
            }
            None => {}
        }
    }
    all
}

/// The estimate for `pair`, seeing through an alias, a `[1m]` twin and a
/// version pin.
fn estimate_for(surface: LadderSurface, pair: &IntelligenceRung) -> Option<&'static EstimatedPair> {
    rung_estimates().pairs.iter().find(|e| {
        same_intelligence_rung(surface, &IntelligenceRung::new(&e.model, &e.effort), pair)
    })
}

/// `pair`'s band on `surface`'s intelligence profile. `None` without an
/// estimate.
fn intelligence_band(surface: LadderSurface, pair: &IntelligenceRung) -> Option<i32> {
    estimate_for(surface, pair).map(|e| surface.profile_band(e))
}

/// Whether `on` climbs: every estimated rung sits no lower on `surface`'s
/// intelligence profile than the estimated rung before it. A rung with no
/// estimate has no place to be out of.
pub fn climbs(surface: LadderSurface, on: &[IntelligenceRung]) -> bool {
    on.iter()
        .filter_map(|pair| intelligence_band(surface, pair))
        .is_sorted()
}

/// `on` ordered least to most intelligent on `surface`'s profile. Equal bands
/// keep their order, and a rung with no estimate follows the banded ones.
pub fn sorted_by_intelligence(
    surface: LadderSurface,
    on: &[IntelligenceRung],
) -> Vec<IntelligenceRung> {
    let mut sorted = on.to_vec();
    sorted.sort_by_key(|pair| intelligence_band(surface, pair).unwrap_or(i32::MAX));
    sorted
}

/// Whether `pair` has an estimate, whatever the reference rung is.
pub(crate) fn is_estimated(surface: LadderSurface, pair: &IntelligenceRung) -> bool {
    estimate_for(surface, pair).is_some()
}

/// An intelligence, cost or time per task as a multiple of the reference
/// rung's.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Band {
    pub multiple: f64,
    /// How a person reads it: `2.5×`, `1.0×`, `0.02×`, or `included`.
    pub label: String,
}

/// The steps per doubling a token or cost band counts in.
pub(crate) const COST_STEPS_PER_DOUBLING: i32 = 6;

/// The steps per doubling a time band counts in.
pub(crate) const TIME_STEPS_PER_DOUBLING: i32 = 3;

impl Band {
    /// The one label rule. From half the reference up a band reads to
    /// `decimals`. Below that it takes two decimals, or more where two would
    /// round it to zero. Trailing zeros go, but one decimal always stays, so
    /// the reference reads `1.0×` and never a bare `1×`.
    fn from_multiple(multiple: f64, decimals: usize) -> Self {
        let decimals = if multiple >= 0.5 {
            decimals
        } else {
            // A multiple below 0.5 has a small positive -log10, so the cast
            // never truncates or wraps.
            let first_digit = (-multiple.log10()).ceil() as usize;
            first_digit.max(2)
        };
        let fixed = format!("{multiple:.decimals$}");
        let trimmed = fixed.trim_end_matches('0');
        let label = if trimmed.ends_with('.') {
            format!("{trimmed}0")
        } else {
            trimmed.to_string()
        };
        Self {
            multiple,
            label: format!("{label}×"),
        }
    }

    /// A cost band: `steps` above the reference, read in tenths.
    pub fn from_cost_steps(steps: i32) -> Self {
        Self::from_steps(steps, COST_STEPS_PER_DOUBLING)
    }

    /// A time band: `steps` above the reference, read in tenths.
    pub fn from_time_steps(steps: i32) -> Self {
        Self::from_steps(steps, TIME_STEPS_PER_DOUBLING)
    }

    fn from_steps(steps: i32, per_doubling: i32) -> Self {
        Self::from_multiple(2f64.powf(f64::from(steps) / f64::from(per_doubling)), 1)
    }

    /// An intelligence band: the rung's score steps over the reference's.
    /// It reads in hundredths, since one step near the reference is 0.04×.
    /// A non-positive score has no multiple, so it ships no band.
    pub fn from_intelligence(band: i32, reference: i32) -> Option<Self> {
        (band > 0 && reference > 0)
            .then(|| Self::from_multiple(f64::from(band) / f64::from(reference), 2))
    }

    /// The cost on a free route: a task adds nothing to anybody's bill.
    pub fn included() -> Self {
        Self {
            multiple: 0.0,
            label: "included".to_string(),
        }
    }
}

/// The model and optional effort a surface runs when nobody picked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DefaultImplementer {
    pub model: String,
    pub effort: Option<String>,
}

impl DefaultImplementer {
    pub(crate) fn intelligence_rung(&self) -> Option<IntelligenceRung> {
        self.effort
            .as_deref()
            .map(|e| IntelligenceRung::new(&self.model, e))
    }
}

/// The backend's default implementer, from its preferences.
///
/// A stored model the backend no longer offers would break every spawn with
/// no pick, so it reads as no default, logged. An effort the model does not
/// take is dropped the same way.
pub async fn default_implementer(pool: &PgPool, agent: CodingAgent) -> Option<DefaultImplementer> {
    let stored = match agent {
        CodingAgent::ClaudeCode => DefaultImplementer {
            model: prefs::MODEL_CLAUDE_CODE.read(pool).await,
            effort: Some(prefs::REASONING_CLAUDE_CODE.read(pool).await),
        },
        CodingAgent::Codex => DefaultImplementer {
            model: prefs::MODEL_CODEX.read(pool).await,
            effort: Some(prefs::REASONING_CODEX.read(pool).await),
        },
    };
    offered_default(agent, stored)
}

/// `stored`, kept only as far as `agent` offers it.
fn offered_default(agent: CodingAgent, stored: DefaultImplementer) -> Option<DefaultImplementer> {
    if let Err(e) = super::validate_coding_agent_model(agent, Some(&stored.model)) {
        log_legacy!("[IntelligenceLadder] ignoring the default implementer: {e}");
        return None;
    }
    let effort = stored.effort.filter(|effort| {
        super::validate_coding_agent_effort(agent, Some(&stored.model), Some(effort))
            .inspect_err(|e| log_legacy!("[IntelligenceLadder] ignoring the default effort: {e}"))
            .is_ok()
    });
    Some(DefaultImplementer {
        model: stored.model,
        effort,
    })
}

/// The effort a pick naming only `model` runs at: the model registry's answer
/// for Claude Code, the menu file's for Codex.
pub(crate) fn model_default_effort(
    agent: CodingAgent,
    registry: &ModelRegistry,
    model: &str,
) -> Option<&'static str> {
    match agent {
        CodingAgent::ClaudeCode => model_registry::default_effort(
            registry,
            &crate::llm::intelligence_rungs::strip_version_pin(&model_run_by(agent, model)),
        ),
        CodingAgent::Codex => super::codex::codex_model_options()
            .iter()
            .find(|row| row.value == model)
            .and_then(|row| row.default_effort.as_deref()),
    }
}

/// Fill what a pick left out, after the thread's own memory had its say.
///
/// No model takes the default implementer whole, or its model beside a
/// picked effort. A model with no effort runs at its *default effort*
/// (`model_default_effort`), unless the backend refuses that tier.
pub fn fill_pick(
    agent: CodingAgent,
    model: Option<String>,
    effort: Option<String>,
    default: Option<&DefaultImplementer>,
    default_effort: impl Fn(&str) -> Option<&'static str>,
) -> (Option<String>, Option<String>) {
    match (model, effort) {
        (None, None) => (
            default.map(|d| d.model.clone()),
            default.and_then(|d| d.effort.clone()),
        ),
        (None, Some(effort)) => (default.map(|d| d.model.clone()), Some(effort)),
        (Some(model), None) => {
            let effort = default_effort(&model)
                .filter(|e| {
                    super::validate_coding_agent_effort(agent, Some(&model), Some(e)).is_ok()
                })
                .map(str::to_string);
            (Some(model), effort)
        }
        picked => picked,
    }
}

/// The effort a *model swap* records beside `served`, the model the backend
/// ran in place of the pick. The pick's effort suited another model, so the
/// served model runs at its own configured effort: the default implementer's
/// when that runs `served`, else the model's default effort.
pub fn fallback_effort(
    agent: CodingAgent,
    served: &str,
    default: Option<&DefaultImplementer>,
    default_effort: impl Fn(&str) -> Option<&'static str>,
) -> Option<String> {
    match default.filter(|d| runs_same_model(agent, &d.model, served)) {
        Some(default) => default.effort.clone(),
        None => fill_pick(agent, Some(served.to_string()), None, None, default_effort).1,
    }
}

/// Resolve automatic defaults without rewriting owner preferences or thread pins.
pub async fn default_chat_selection(pool: &PgPool, offer: &ChatOffer) -> DefaultImplementer {
    let registry = &offer.routes.registry;
    let stored = prefs::CHAT_MODEL.stored(pool).await;
    let provider_default = offer.routes.provider.as_ref().map(|p| p.default_model());
    let pinned = stored.or_else(|| {
        provider_default
            .filter(|m| *m != prefs::CHAT_MODEL.default_text())
            .map(str::to_string)
    });
    let automatic = pinned.is_none();
    let model = match pinned {
        Some(model) => model,
        None => {
            // Defaults are curated by capability and cost, never release recency alone.
            let general_defaults: &[_] = &[
                prefs::CHAT_MODEL.default_text(),
                "gpt-6.2-sol",
                "gpt-6.1-sol",
                "claude-sonnet-5-5",
            ];
            let vertex_defaults: &[_] = &["gemini-4-neon", "gemini-4-carbon"];
            let defaults = if offer.routes.configured == [ProviderKind::Vertex] {
                [vertex_defaults, general_defaults]
            } else {
                [general_defaults, vertex_defaults]
            };
            let mut candidates: Vec<String> = defaults
                .into_iter()
                .flatten()
                .copied()
                .chain(["gemini-3.8-flash"])
                .map(str::to_string)
                .collect();
            let mut others: Vec<_> = registry
                .read()
                .expect("model registry lock")
                .keys()
                .cloned()
                .collect();
            others.sort();
            candidates.extend(others);
            candidates
                .into_iter()
                .find(|model| {
                    prefs::REASONING_EFFORTS
                        .iter()
                        .any(|effort| offer.available(model, effort))
                })
                .unwrap_or_else(|| prefs::CHAT_MODEL.default_text().to_string())
        }
    };
    let effort = chat_effort_for(pool, registry, &model)
        .await
        .or_else(|| automatic.then(|| "medium".to_string()));
    let effort = effort.and_then(|effort| {
        match model_registry::resolve_route(registry, &model, None, |kind| {
            offer.routes.configured.contains(&kind)
        }) {
            Ok(route) => {
                crate::llm::reasoning::clamp_effort(&effort, route.provider, &route.wire_id)
                    .map(str::to_string)
            }
            Err(_) => Some(effort),
        }
    });
    DefaultImplementer { model, effort }
}

/// The tier a new chat thread on `model` runs at: the account's stored tier,
/// else the model's *default effort*.
async fn chat_effort_for(pool: &PgPool, registry: &ModelRegistry, model: &str) -> Option<String> {
    match crate::core::PreferenceStore::user_chat_effort(pool, model).await {
        Some(effort) => Some(effort),
        None => model_registry::default_effort(registry, model).map(str::to_string),
    }
}

/// Fill a chat pick the way a new chat thread fills it: a missing model is
/// the account's chat model, and a missing effort its tier for the model.
async fn fill_chat_pick(
    pool: &PgPool,
    offer: &ChatOffer,
    model: Option<&str>,
    effort: Option<&str>,
) -> (Option<String>, Option<String>) {
    if model.is_none() {
        let default = default_chat_selection(pool, offer).await;
        return (
            Some(default.model),
            effort.map(str::to_string).or(default.effort),
        );
    }
    let model = model.expect("model provided").to_string();
    let effort = match effort {
        Some(effort) => Some(effort.to_string()),
        None => chat_effort_for(pool, &offer.routes.registry, &model).await,
    };
    (Some(model), effort)
}

/// One intelligence rung as the route and the CLI show it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ServedIntelligenceRung {
    pub model: String,
    pub effort: String,
    /// The model's picker label, else its id.
    pub label: String,
    /// On the effective ladder, so an agent may pick it.
    pub on: bool,
    /// The pair has an estimate, whatever the reference rung is.
    pub estimated: bool,
    /// Cost per task against the reference rung, or `included` on a route
    /// that bills nobody. Absent without an estimate or a price.
    pub cost: Option<Band>,
    /// Intelligence against the reference rung. Absent without an estimate.
    pub intelligence: Option<Band>,
    /// Time per task against the reference rung. Absent without an estimate.
    pub time: Option<Band>,
    pub is_default: bool,
}

/// One intelligence rung that is on, as the rung control lists it: the pair
/// an agent may run on and its model's picker label.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct OnIntelligenceRung {
    pub model: String,
    pub effort: String,
    pub label: String,
    pub estimated: bool,
    pub intelligence: Option<Band>,
    pub cost: Option<Band>,
    pub time: Option<Band>,
}

/// One surface's ladder, served by `GET /api/v1/intelligence-ladder`. The
/// wire keeps the UI's word, `rungs`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct IntelligenceLadder {
    pub surface: LadderSurface,
    /// The backend of a coding-agent surface. Absent for chat.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub coding_agent: Option<CodingAgent>,
    pub source: String,
    pub estimated: String,
    /// True when the owner's override replaced the shipped rungs.
    pub customised: bool,
    /// The rungs that are on climb on this surface's intelligence profile.
    /// False offers the owner Sort by intelligence.
    pub climbs: bool,
    /// The shipped rungs that are on and offered, which an override equal to
    /// them only restates.
    #[serde(rename = "shipped_rungs")]
    pub shipped_intelligence_rungs: Vec<IntelligenceRung>,
    #[serde(rename = "default_rung")]
    pub default_intelligence_rung: Option<IntelligenceRung>,
    /// The rung the bands compare against. Absent without an estimate.
    pub reference: Option<IntelligenceRung>,
    /// The long-prompt share Claude Code's bands price in. Absent for chat,
    /// whose calls it does not measure.
    pub long_prompt_share: Option<LongPromptShare>,
    /// The rungs that are on, in order, with every other offered shipped pair
    /// in its shipped place.
    #[serde(rename = "rungs")]
    pub intelligence_rungs: Vec<ServedIntelligenceRung>,
}

impl IntelligenceLadder {
    /// The rungs an agent may pick, in order.
    pub fn pairs(&self) -> Vec<IntelligenceRung> {
        self.intelligence_rungs
            .iter()
            .filter(|r| r.on)
            .map(|r| IntelligenceRung::new(&r.model, &r.effort))
            .collect()
    }

    /// The rungs that are on, in order, for the rung control's list.
    pub fn on_intelligence_rungs(&self) -> Vec<OnIntelligenceRung> {
        self.intelligence_rungs
            .iter()
            .filter(|r| r.on)
            .map(|r| OnIntelligenceRung {
                model: r.model.clone(),
                effort: r.effort.clone(),
                label: r.label.clone(),
                estimated: r.estimated,
                intelligence: r.intelligence.clone(),
                cost: r.cost.clone(),
                time: r.time.clone(),
            })
            .collect()
    }

    /// Where `pick` sits in the served rungs, on or off, seeing through a
    /// `[1m]` twin and an alias.
    pub fn position_of(&self, pick: &IntelligenceRung) -> Option<usize> {
        self.intelligence_rungs.iter().position(|r| {
            same_intelligence_rung(
                self.surface,
                &IntelligenceRung::new(&r.model, &r.effort),
                pick,
            )
        })
    }

    /// Where `pick` sits among the rungs that are on, as [`Self::pairs`]
    /// lists them. `None` when `pick` is not on.
    pub fn on_position_of(&self, pick: &IntelligenceRung) -> Option<usize> {
        let at = self
            .position_of(pick)
            .filter(|&at| self.intelligence_rungs[at].on)?;
        Some(
            self.intelligence_rungs[..at]
                .iter()
                .filter(|r| r.on)
                .count(),
        )
    }

    /// Whether a rung that is on runs `model`, seeing through a `[1m]` twin
    /// and an alias. The chat picker lists exactly these models.
    pub fn lists_model(&self, model: &str) -> bool {
        let model = canonical_model(self.surface, model);
        self.intelligence_rungs
            .iter()
            .any(|r| r.on && canonical_model(self.surface, &r.model) == model)
    }

    /// The rungs that are on with `pair` turned on too, in served order.
    /// `None` when `pair` is not a served rung, which means it is not offered.
    pub fn on_with(&self, pair: &IntelligenceRung) -> Option<Vec<IntelligenceRung>> {
        let at = self.position_of(pair)?;
        Some(
            self.intelligence_rungs
                .iter()
                .enumerate()
                .filter(|(i, r)| r.on || *i == at)
                .map(|(_, r)| IntelligenceRung::new(&r.model, &r.effort))
                .collect(),
        )
    }

    /// The rungs that are on, less every one that runs `model`.
    pub fn on_without_model(&self, model: &str) -> Vec<IntelligenceRung> {
        let model = canonical_model(self.surface, model);
        self.pairs()
            .into_iter()
            .filter(|p| canonical_model(self.surface, &p.model) != model)
            .collect()
    }

    /// The rungs that are on once an edit turns on exactly `asked`, in its
    /// order, named as the served rungs name them. Refuses an empty list, a
    /// pair this surface does not offer, a repeat, and leaving the default off.
    /// With no default rung, `default_model` must still run on some rung.
    pub fn intelligence_rungs_set_to(
        &self,
        asked: &[IntelligenceRung],
        default_model: Option<&str>,
    ) -> Result<Vec<IntelligenceRung>, String> {
        let ladder = format!("the {} intelligence ladder", self.surface.display_name());
        if asked.is_empty() {
            return Err(format!("Name at least one rung for {ladder}."));
        }
        let mut positions = Vec::with_capacity(asked.len());
        for pair in asked {
            let at = self
                .position_of(pair)
                .ok_or_else(|| format!("{pair} is not offered on {ladder}."))?;
            if positions.contains(&at) {
                return Err(format!("{pair} is named twice."));
            }
            positions.push(at);
        }
        if let Some(default) = &self.default_intelligence_rung {
            if self
                .position_of(default)
                .is_some_and(|at| !positions.contains(&at))
            {
                return Err(format!(
                    "{default} is the default rung of {ladder}, so it stays on. Pick another \
                     default first."
                ));
            }
        } else if let Some(model) = default_model {
            let model = canonical_model(self.surface, model);
            let runs_default = |&at: &usize| {
                canonical_model(self.surface, &self.intelligence_rungs[at].model) == model
            };
            if !positions.iter().any(runs_default) {
                return Err(format!(
                    "'{model}' is the default model of {ladder}, so a rung that runs it stays \
                     on. Pick another default first."
                ));
            }
        }
        Ok(positions
            .into_iter()
            .map(|at| {
                let intelligence_rung = &self.intelligence_rungs[at];
                IntelligenceRung::new(&intelligence_rung.model, &intelligence_rung.effort)
            })
            .collect())
    }

    /// The rungs that are on, less every one that runs `model`. Refuses a
    /// model no rung that is on runs, the default's model, and the last model.
    pub fn intelligence_rungs_without(
        &self,
        model: &str,
        default_model: Option<&str>,
    ) -> Result<Vec<IntelligenceRung>, String> {
        let ladder = self.ladder_name();
        if !self.lists_model(model) {
            return Err(format!("No rung that is on runs '{model}' on {ladder}."));
        }
        self.refuse_the_default(model, default_model)?;
        let pairs = self.on_without_model(model);
        if pairs.is_empty() {
            return Err(format!(
                "'{model}' is the only model on {ladder}. Turn another model on first."
            ));
        }
        Ok(pairs)
    }

    /// Refuses `model` when it is the default's model, whether or not a rung
    /// that is on runs it.
    pub fn refuse_the_default(
        &self,
        model: &str,
        default_model: Option<&str>,
    ) -> Result<(), String> {
        if default_model.is_some_and(|default| {
            canonical_model(self.surface, default) == canonical_model(self.surface, model)
        }) {
            return Err(format!(
                "'{model}' runs the default rung of {}. Pick another default first.",
                self.ladder_name()
            ));
        }
        Ok(())
    }

    fn ladder_name(&self) -> String {
        format!("the {} intelligence ladder", self.surface.display_name())
    }

    /// The rungs to offer in place of `refused` after a swap: at most four
    /// that are on, run another model, and sit nearest it in served order.
    /// Up to two sit above it and two below, and the other side fills a
    /// shortfall. Nearest first, the higher rung first on a tie. An effort
    /// no rung offers sits at the model's lowest rung. A model the backend
    /// also refused is never offered.
    pub fn swap_suggestions(
        &self,
        refused: &IntelligenceRung,
        also_refused: impl Fn(&str) -> bool,
    ) -> Vec<IntelligenceRung> {
        const PER_SIDE: usize = 2;
        let refused_model = canonical_model(self.surface, &refused.model);
        let runs_refused = |intelligence_rung: &ServedIntelligenceRung| {
            canonical_model(self.surface, &intelligence_rung.model) == refused_model
        };
        let at = self
            .position_of(refused)
            .or_else(|| self.intelligence_rungs.iter().position(runs_refused))
            .unwrap_or(self.intelligence_rungs.len());
        let others = |range: &mut dyn Iterator<Item = usize>| -> Vec<usize> {
            range
                .filter(|&i| {
                    let intelligence_rung = &self.intelligence_rungs[i];
                    intelligence_rung.on
                        && !runs_refused(intelligence_rung)
                        && !also_refused(&intelligence_rung.model)
                })
                .collect()
        };
        let above = others(&mut (at + 1..self.intelligence_rungs.len()));
        let below = others(&mut (0..at).rev());
        let take_above = above
            .len()
            .min((2 * PER_SIDE).saturating_sub(below.len().min(PER_SIDE)));
        let take_below = below.len().min(2 * PER_SIDE - take_above);
        let mut picked: Vec<usize> = above[..take_above]
            .iter()
            .chain(&below[..take_below])
            .copied()
            .collect();
        picked.sort_by_key(|&i| (i.abs_diff(at), i < at));
        picked
            .into_iter()
            .map(|i| {
                let intelligence_rung = &self.intelligence_rungs[i];
                IntelligenceRung::new(&intelligence_rung.model, &intelligence_rung.effort)
            })
            .collect()
    }

    /// Refuse a pick that is not a rung that is on, naming every such rung.
    pub fn check_pick(&self, pick: &IntelligenceRung) -> Result<(), String> {
        if self
            .position_of(pick)
            .is_some_and(|at| self.intelligence_rungs[at].on)
        {
            return Ok(());
        }
        Err(format!(
            "{pick} is not a rung on the {} intelligence ladder. {}",
            self.surface.display_name(),
            self.intelligence_rung_list()
        ))
    }

    /// Refuse an agent's pick that is not a rung that is on, or that runs a
    /// model the backend `refused`: it substituted another model, and the
    /// owner has not answered yet. The refusal names the swap.
    pub fn check_runnable_pick(
        &self,
        pick: &IntelligenceRung,
        refused: impl Fn(CodingAgent, &str) -> bool,
    ) -> Result<(), String> {
        self.check_pick(pick)?;
        match self.coding_agent {
            Some(agent) if refused(agent, &pick.model) => Err(format!(
                "{} swapped {} for another model on this account, so no agent picks it \
                 until it serves again or the owner takes it off the ladder. {}",
                self.surface.display_name(),
                pick.model,
                self.intelligence_rung_list_where(|p| !refused(agent, &p.model))
            )),
            _ => Ok(()),
        }
    }

    /// Every rung that is on, as a refusal names them.
    fn intelligence_rung_list(&self) -> String {
        self.intelligence_rung_list_where(|_| true)
    }

    /// The rungs that are on and `keep` holds, as a refusal names them.
    fn intelligence_rung_list_where(&self, keep: impl Fn(&IntelligenceRung) -> bool) -> String {
        let on = self
            .pairs()
            .iter()
            .filter(|p| keep(p))
            .map(IntelligenceRung::to_string)
            .collect::<Vec<_>>()
            .join(", ");
        format!(
            "Pick one of: {on}. Run `lucidos intelligence-ladder --surface {}` to see each \
             rung's intelligence, cost and time.",
            self.surface.as_wire()
        )
    }
}

/// The share of calls whose prompt passes the long-prompt line, 0 to 1.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct LongPromptShare {
    /// What the bands use: the owner's setting, else the measured share.
    pub effective: f64,
    /// The owner's `long_prompt_share`, when set.
    pub set: Option<f64>,
    /// Measured from the workspace's recent Claude Code calls. Absent
    /// without any.
    pub measured: Option<f64>,
    /// The prompt size past which a call counts as long.
    pub line_tokens: i64,
}

/// How much a model's average call costs at `share` over the long-prompt
/// line, as a multiple of its base-tier cost. Only a long tier that starts at
/// [`LONG_PROMPT_TOKENS`] counts, since `share` is measured against that line.
fn long_prompt_weight(price: &ModelPrice, share: f64) -> f64 {
    price
        .long
        .filter(|long| long.threshold_tokens == LONG_PROMPT_TOKENS.unsigned_abs())
        .map_or(1.0, |long| 1.0 - share + share * long.factor)
}

/// `ratio` in cost band steps.
fn steps_of(ratio: f64) -> f64 {
    f64::from(COST_STEPS_PER_DOUBLING) * ratio.log2()
}

/// Cost per task at list price, in band steps of an absolute scale: the
/// token band plus the output price, weighted by the long-prompt share.
/// Only a difference of two of them means anything. `None` without a price.
pub(crate) fn list_cost_steps(estimate: &EstimatedPair, share: f64) -> Option<f64> {
    let price = model_prices::price_of(&estimate.model)?;
    Some(
        f64::from(estimate.token_band)
            + steps_of(price.output)
            + steps_of(long_prompt_weight(&price, share)),
    )
}

/// Snap a step difference onto the band grid.
fn grid_steps(steps: f64) -> i32 {
    // Cost steps between two priced pairs stay within a few dozen, so the
    // cast never truncates.
    steps.round() as i32
}

/// The reference rung's bands, in the units each band divides by.
struct ReferenceBands {
    pair: IntelligenceRung,
    intelligence: i32,
    cost_steps: Option<f64>,
    time: i32,
}

/// Assemble the served ladder from what the caller resolved.
///
/// `reference` is the surface's own default rung when it has an estimate,
/// else Claude Code's: every band counts from the same estimates.
pub fn assemble(
    surface: LadderSurface,
    offer: &impl SurfaceOffer,
    override_value: Option<&str>,
    default_intelligence_rung: Option<IntelligenceRung>,
    reference: Option<IntelligenceRung>,
    long_prompt_share: Option<LongPromptShare>,
) -> IntelligenceLadder {
    let share = long_prompt_share.map_or(0.0, |s| s.effective);
    let offered = |model: &str, effort: &str| offer.offers(model, effort);
    let reference = reference.and_then(|pair| {
        estimate_for(surface, &pair).map(|e| ReferenceBands {
            intelligence: surface.profile_band(e),
            cost_steps: list_cost_steps(e, share),
            time: e.time_band,
            pair: IntelligenceRung::new(&e.model, &e.effort),
        })
    });
    let available = |model: &str, effort: &str| offer.available(model, effort);
    let mut defaults = shipped_on(surface, available);
    let available_default = default_intelligence_rung
        .as_ref()
        .filter(|p| available(&p.model, &p.effort));
    if let Some(pair) = available_default {
        if !defaults
            .iter()
            .any(|p| same_intelligence_rung(surface, p, pair))
        {
            defaults.push(pair.clone());
            defaults = sorted_by_intelligence(surface, &defaults);
        }
    }
    let on = if parsed_override(surface, override_value).is_some() {
        let mut on = effective_ladder(surface, override_value, offered);
        if let Some(pair) = available_default
            .filter(|pair| !on.iter().any(|p| same_intelligence_rung(surface, p, pair)))
        {
            let at = intelligence_band(surface, pair)
                .and_then(|rank| {
                    on.iter().position(|p| {
                        intelligence_band(surface, p).is_some_and(|other| other > rank)
                    })
                })
                .unwrap_or(on.len());
            on.insert(at, pair.clone());
        }
        on
    } else {
        defaults.clone()
    };
    let climbs = climbs(surface, &on);
    let mut all = with_off_pairs(surface, on, offered);
    for pair in offer.joining_intelligence_rungs() {
        let model = canonical_model(surface, &pair.model);
        if !all
            .iter()
            .any(|(p, _)| canonical_model(surface, &p.model) == model)
        {
            all.push((pair, false));
        }
    }
    let intelligence_rungs = all
        .into_iter()
        .map(|(pair, on)| {
            let estimate = estimate_for(surface, &pair);
            let bands = estimate.zip(reference.as_ref());
            let cost = if offer.is_free(&pair.model) {
                Some(Band::included())
            } else {
                bands.and_then(|(e, base)| {
                    let steps = list_cost_steps(e, share)? - base.cost_steps?;
                    Some(Band::from_cost_steps(grid_steps(steps)))
                })
            };
            ServedIntelligenceRung {
                label: offer.label(&pair.model),
                on,
                estimated: estimate.is_some(),
                intelligence: bands.and_then(|(e, base)| {
                    Band::from_intelligence(surface.profile_band(e), base.intelligence)
                }),
                cost,
                time: bands.map(|(e, base)| Band::from_time_steps(e.time_band - base.time)),
                is_default: default_intelligence_rung
                    .as_ref()
                    .is_some_and(|d| same_intelligence_rung(surface, d, &pair)),
                model: pair.model,
                effort: pair.effort,
            }
        })
        .collect();
    let estimates = rung_estimates();
    IntelligenceLadder {
        surface,
        coding_agent: surface.coding_agent(),
        source: estimates.source.clone(),
        estimated: estimates.estimated.clone(),
        customised: parsed_override(surface, override_value).is_some(),
        climbs,
        shipped_intelligence_rungs: defaults,
        default_intelligence_rung,
        reference: reference.map(|base| base.pair),
        long_prompt_share,
        intelligence_rungs,
    }
}

/// Where a Claude Code prompt starts billing at the long-prompt tier.
pub(crate) const LONG_PROMPT_TOKENS: i64 = 100_000;

/// How far back the measured share looks.
const LONG_PROMPT_WINDOW_DAYS: i32 = 30;

/// The share of the workspace's recent Claude Code calls whose prompt passed
/// [`LONG_PROMPT_TOKENS`]. `None` without any call, or when the query fails.
pub(crate) async fn measure_long_prompt_share(pool: &PgPool) -> Option<f64> {
    sqlx::query_scalar::<_, Option<f64>>(
        "SELECT (count(*) FILTER (WHERE (payload->'usage'->>'input_tokens')::bigint > $1))::float8 \
                / NULLIF(count(*), 0) \
         FROM events \
         WHERE event_type = 'ContextCaptured' \
           AND created > now() - make_interval(days => $2) \
           AND payload->>'producer' = 'claude_code'",
    )
    .bind(LONG_PROMPT_TOKENS)
    .bind(LONG_PROMPT_WINDOW_DAYS)
    .fetch_one(pool)
    .await
    .inspect_err(|e| log_legacy!("[IntelligenceLadder] measuring the long-prompt share failed: {e}"))
    .ok()
    .flatten()
}

/// How long a measured share serves before it is measured again.
const MEASURED_SHARE_TTL: std::time::Duration = std::time::Duration::from_secs(3600);

static MEASURED_SHARE: std::sync::Mutex<Option<(std::time::Instant, Option<f64>)>> =
    std::sync::Mutex::new(None);

/// [`measure_long_prompt_share`], cached for [`MEASURED_SHARE_TTL`]: it scans
/// a month of captures, and the ladder is read on every plan card.
async fn measured_long_prompt_share(pool: &PgPool) -> Option<f64> {
    let cached = *MEASURED_SHARE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((_, share)) = cached.filter(|(at, _)| at.elapsed() < MEASURED_SHARE_TTL) {
        return share;
    }
    let share = measure_long_prompt_share(pool).await;
    *MEASURED_SHARE.lock().unwrap_or_else(|e| e.into_inner()) =
        Some((std::time::Instant::now(), share));
    share
}

/// The long-prompt share the bands use: the owner's setting, else measured.
pub async fn long_prompt_share(pool: &PgPool) -> LongPromptShare {
    let measured = measured_long_prompt_share(pool).await;
    let set = prefs::LONG_PROMPT_SHARE
        .read(pool)
        .await
        .and_then(|v| v.parse::<f64>().ok())
        .map(|percent| percent / 100.0);
    LongPromptShare {
        effective: set.or(measured).unwrap_or(0.0),
        set,
        measured,
        line_tokens: LONG_PROMPT_TOKENS,
    }
}

/// The default rung of a coding agent, from its default implementer.
async fn default_coding_intelligence_rung(
    pool: &PgPool,
    agent: CodingAgent,
) -> Option<IntelligenceRung> {
    default_implementer(pool, agent)
        .await
        .and_then(|d| d.intelligence_rung())
}

/// `surface`'s effective ladder, read from the workspace's preferences.
pub async fn load(
    pool: &PgPool,
    routes: &ModelRoutes,
    surface: LadderSurface,
) -> IntelligenceLadder {
    load_resolved(pool, routes, surface, None).await
}

/// Load a surface against a caller-selected reference, including an absent one.
pub async fn load_with_reference(
    pool: &PgPool,
    routes: &ModelRoutes,
    surface: LadderSurface,
    reference: Option<IntelligenceRung>,
) -> IntelligenceLadder {
    load_resolved(pool, routes, surface, Some(reference)).await
}

async fn load_resolved(
    pool: &PgPool,
    routes: &ModelRoutes,
    surface: LadderSurface,
    reference: Option<Option<IntelligenceRung>>,
) -> IntelligenceLadder {
    let chat_offer = if surface.coding_agent().is_none() {
        Some(ChatOffer::load(pool, routes.clone()).await)
    } else {
        None
    };
    let default = match &chat_offer {
        Some(offer) => default_chat_selection(pool, offer)
            .await
            .intelligence_rung(),
        None => {
            default_coding_intelligence_rung(pool, surface.coding_agent().expect("coding surface"))
                .await
        }
    };
    let reference = match reference {
        Some(reference) => reference,
        None => match default.as_ref().filter(|pair| is_estimated(surface, pair)) {
            Some(pair) => Some(pair.clone()),
            None => default_coding_intelligence_rung(pool, CodingAgent::ClaudeCode).await,
        },
    };
    let override_value = surface.override_pref().read(pool).await;
    match chat_offer {
        Some(offer) => assemble(
            surface,
            &offer,
            override_value.as_deref(),
            default,
            reference,
            None,
        ),
        None => assemble(
            surface,
            &CodingAgentOffer(surface.coding_agent().expect("coding surface")),
            override_value.as_deref(),
            default,
            reference,
            Some(long_prompt_share(pool).await),
        ),
    }
}

/// Other installed backends, with the spawn's binary probe supplied by the caller.
pub(crate) fn other_coding_agents(
    current: CodingAgent,
    installed: impl Fn(CodingAgent) -> bool,
) -> Vec<CodingAgent> {
    [CodingAgent::ClaudeCode, CodingAgent::Codex]
        .into_iter()
        .filter(|a| *a != current && installed(*a))
        .collect()
}

/// Other installed backends measured against the current ladder's reference.
pub async fn other_backend_ladders(
    pool: &PgPool,
    routes: &ModelRoutes,
    current: &IntelligenceLadder,
) -> Vec<IntelligenceLadder> {
    let Some(agent) = current.coding_agent else {
        return Vec::new();
    };
    let installed = crate::runtime::installed_coding_agents(pool).await;
    let mut ladders = Vec::new();
    for other in other_coding_agents(agent, |a| installed.contains(&a)) {
        ladders
            .push(load_with_reference(pool, routes, other.into(), current.reference.clone()).await);
    }
    ladders
}

/// Whether `agent` substituted another model for `model` on the live account,
/// and `model` has not served since. Only Claude Code swaps.
pub fn is_refused(agent: CodingAgent, model: &str) -> bool {
    agent == CodingAgent::ClaudeCode && super::cc_model_discovery::is_refused(model)
}

/// The reason slug an off-ladder refusal carries, for a client to tell it
/// from its siblings.
pub const OFF_LADDER_REASON: &str = "off-intelligence-ladder";

/// Refuse an agent's explicit pick that is not a rung on `surface`'s
/// effective ladder. A request naming neither model nor effort picks
/// nothing, so it passes: the new thread runs on the surface's default.
///
/// The pick is filled as the new thread fills it, so a model-only pick is
/// judged at the effort it would run at. A pick that still lacks one half is
/// refused, since no rung names half a pair.
pub async fn check_agent_pick(
    pool: &PgPool,
    routes: &ModelRoutes,
    surface: LadderSurface,
    model: Option<&str>,
    effort: Option<&str>,
) -> Result<(), String> {
    if model.is_none() && effort.is_none() {
        return Ok(());
    }
    let filled = match surface.coding_agent() {
        Some(agent) => {
            let default = default_implementer(pool, agent).await;
            fill_pick(
                agent,
                model.map(str::to_string),
                effort.map(str::to_string),
                default.as_ref(),
                |m| model_default_effort(agent, &routes.registry, m),
            )
        }
        None => {
            fill_chat_pick(
                pool,
                &ChatOffer::load(pool, routes.clone()).await,
                model,
                effort,
            )
            .await
        }
    };
    let ladder = load(pool, routes, surface).await;
    match filled {
        (Some(model), Some(effort)) => {
            ladder.check_runnable_pick(&IntelligenceRung::new(&model, &effort), is_refused)
        }
        _ => Err(format!(
            "A pick on the {} intelligence ladder names both a model and an effort. {}",
            surface.display_name(),
            ladder.intelligence_rung_list()
        )),
    }
}

#[cfg(test)]
#[path = "intelligence_ladder_tests.rs"]
pub(crate) mod tests;
