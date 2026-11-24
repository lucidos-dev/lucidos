//! The *intelligence ladder*: one *ladder surface*'s picks, each an
//! *intelligence rung* (one model at one effort), least to most intelligent.
//! The surfaces are chat (the Lucidos Agent), Claude Code and Codex.
//!
//! The *rung estimates* (`llm/rung_estimates.json`) hold every estimated
//! pair's bands, and each surface's shipped pairs in order with whether each
//! is on. An owner's override replaces the on rungs with an ordered list.
//! Cost per task is the token band times the price table's output price, so
//! a price change moves it (ADR 0500, ADR 0525).

use sqlx::PgPool;

use super::CodingAgent;
use crate::core::preference_catalog as prefs;
use crate::llm::model_registry::{self, ModelRegistry};

pub use crate::llm::intelligence_rungs::{
    rung_estimates, EstimatedPair, IntelligenceRung, LadderSurface, RungEstimates, SurfacePair,
    SurfacePairs,
};

use super::chat_ladder_policy::{automatic_chat_model, chat_route_effort};
pub use super::chat_ladder_policy::{chat_joining_intelligence_rung, ChatOffer, ModelRoutes};
pub use super::intelligence_ladder_policy::{
    assemble, climbs, copied_from_chat, effective_ladder, fallback_effort, fill_pick, is_offered,
    sorted_by_intelligence, Band, CodingAgentOffer, DefaultImplementer, IntelligenceLadder,
    LongPromptShare, OnIntelligenceRung, ServedIntelligenceRung, SurfaceOffer,
};
pub(crate) use super::intelligence_ladder_policy::{
    drops_context_variant, is_estimated, model_default_effort, model_run_by, runs_same_model,
    shipped_on,
};
#[cfg(test)]
use super::intelligence_ladder_policy::{
    estimate_for, steps_of, COST_STEPS_PER_DOUBLING, TIME_STEPS_PER_DOUBLING,
};
use super::intelligence_ladder_policy::{other_coding_agents, LONG_PROMPT_TOKENS};
#[cfg(test)]
use crate::llm::{intelligence_rungs::CONTEXT_VARIANT, model_prices, model_registry::ProviderKind};
#[cfg(test)]
use std::collections::HashMap;

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
        None => automatic_chat_model(offer, prefs::CHAT_MODEL.default_text()),
    };
    let effort = chat_effort_for(pool, registry, &model)
        .await
        .or_else(|| automatic.then(|| "medium".to_string()));
    let effort = chat_route_effort(offer, &model, effort);
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
