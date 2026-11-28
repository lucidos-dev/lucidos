use std::collections::{HashMap, HashSet};

use super::intelligence_ladder_policy::{is_estimated, SurfaceOffer};
use crate::llm::intelligence_rungs::{
    intelligence_rung_model_base, IntelligenceRung, LadderSurface,
};
use crate::llm::model_registry::{self, ModelRegistry, ProviderKind};

#[derive(Clone)]
pub struct ModelRoutes {
    pub registry: ModelRegistry,
    pub configured: Vec<ProviderKind>,
    pub provider: Option<std::sync::Arc<dyn crate::llm::LlmProvider>>,
}

pub struct ChatOffer {
    pub routes: ModelRoutes,
    pub labels: HashMap<String, String>,
    pub unavailable_routes: std::collections::HashSet<(ProviderKind, String)>,
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

pub(crate) fn upgraded_chat_ladder(
    offer: &impl SurfaceOffer,
    enabled: &[String],
) -> Option<Vec<IntelligenceRung>> {
    let enabled: HashSet<String> = enabled
        .iter()
        .map(|id| intelligence_rung_model_base(id))
        .collect();
    let shipped = LadderSurface::Chat.shipped_pairs();
    let shipped_on: HashSet<String> = shipped
        .iter()
        .filter(|p| p.on && offer.offers(&p.model, &p.effort))
        .map(|p| intelligence_rung_model_base(&p.model))
        .collect();
    let joining_enabled: Vec<IntelligenceRung> = offer
        .joining_intelligence_rungs()
        .into_iter()
        .filter(|p| enabled.contains(&intelligence_rung_model_base(&p.model)))
        .collect();
    let joining: Vec<IntelligenceRung> = joining_enabled
        .iter()
        .filter(|p| is_estimated(LadderSurface::Chat, p))
        .cloned()
        .collect();
    let listed: HashSet<String> = joining_enabled
        .iter()
        .map(|p| intelligence_rung_model_base(&p.model))
        .filter(|base| {
            shipped_on.contains(base)
                || joining
                    .iter()
                    .any(|p| intelligence_rung_model_base(&p.model) == *base)
        })
        .collect();
    if listed.is_empty() || listed == shipped_on {
        return None;
    }
    let mut pairs: Vec<IntelligenceRung> = shipped
        .iter()
        .map(|p| (IntelligenceRung::new(&p.model, &p.effort), p.on))
        .filter(|(pair, on)| {
            listed.contains(&intelligence_rung_model_base(&pair.model))
                && offer.offers(&pair.model, &pair.effort)
                && (*on || joining.contains(pair))
        })
        .map(|(pair, _)| pair)
        .collect();
    for pair in joining {
        if !pairs.contains(&pair) {
            pairs.push(pair);
        }
    }
    Some(pairs)
}

pub(super) fn automatic_chat_model(offer: &ChatOffer, shipped_default: &str) -> String {
    let general_defaults: &[_] = &[
        shipped_default,
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
    let mut others: Vec<_> = offer
        .routes
        .registry
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
            crate::llm::EFFORT_LADDER
                .iter()
                .any(|effort| offer.available(model, effort))
        })
        .unwrap_or_else(|| shipped_default.to_string())
}

pub(super) fn chat_route_effort(
    offer: &ChatOffer,
    model: &str,
    effort: Option<String>,
) -> Option<String> {
    effort.and_then(|effort| {
        match model_registry::resolve_route(&offer.routes.registry, model, None, |kind| {
            offer.routes.configured.contains(&kind)
        }) {
            Ok(route) => {
                crate::llm::reasoning::clamp_effort(&effort, route.provider, &route.wire_id)
                    .map(str::to_string)
            }
            Err(_) => Some(effort),
        }
    })
}
