//! The Lucidos Agent's intelligence ladder as the only switch for what the
//! model picker lists (ADR 0525). This module turns one model on or off it and copies it
//! onto a coding agent's ladder. It also runs the one-time upgrade from the
//! model registry's `enabled` flag it replaced.

use std::collections::HashSet;

use sqlx::PgPool;

use super::intelligence_ladder::{
    self as ladder, intelligence_rung_model_base, ChatOffer, IntelligenceRung, LadderSurface,
    ModelRoutes, SurfaceOffer,
};
use crate::core::preference_catalog as prefs;
use crate::core::PreferenceStore;
use crate::engine::event_bus::EventBus;
use crate::engine::thread_events::MessageOrigin;

/// What turning a model on or off the chat ladder did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChatLadderChange {
    /// The model already was as asked.
    Unchanged,
    /// The model joined at this rung.
    TurnedOn(IntelligenceRung),
    /// These rungs, every one that ran the model, went off.
    TurnedOff(Vec<IntelligenceRung>),
}

impl ChatLadderChange {
    /// The change in a sentence, for a tool result or a log line.
    pub fn describe(&self, model: &str) -> String {
        match self {
            Self::Unchanged => format!("'{model}' already was as asked on the Lucidos Agent's ladder."),
            Self::TurnedOn(pair) => format!(
                "'{model}' is on the Lucidos Agent's ladder at {pair}, so the model picker lists it."
            ),
            Self::TurnedOff(pairs) => format!(
                "'{model}' is off the Lucidos Agent's ladder, so the model picker no longer lists it. \
                 Rungs turned off: {}.",
                override_value(pairs)
            ),
        }
    }
}

/// An override as the preference stores it.
fn override_value(pairs: &[IntelligenceRung]) -> String {
    pairs
        .iter()
        .map(IntelligenceRung::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

/// Turn `model` on the chat ladder at the rung it joins at, or turn off
/// every rung that runs it. A change writes the whole on-list as the owner's
/// override. Refuses a model with no registry row, a retired one, and taking
/// off the last model on the ladder.
pub async fn set_model_on_chat_ladder(
    pool: &PgPool,
    event_bus: &EventBus,
    routes: &ModelRoutes,
    model: &str,
    on: bool,
    actor: Option<MessageOrigin>,
) -> Result<ChatLadderChange, String> {
    let served = ladder::load(pool, routes, LadderSurface::Chat).await;
    if served.lists_model(model) == on {
        return Ok(ChatLadderChange::Unchanged);
    }
    let (pairs, change) = if on {
        let joining = ladder::chat_joining_intelligence_rung(&routes.registry, model)
            .ok_or_else(|| format!("No model '{model}' in the registry."))?;
        let pairs = served.on_with(&joining).ok_or_else(|| {
            format!("'{model}' is retired or takes no tier, so it cannot join the Lucidos Agent's ladder.")
        })?;
        (pairs, ChatLadderChange::TurnedOn(joining))
    } else {
        let pairs = served.on_without_model(model);
        if pairs.is_empty() {
            return Err(format!(
                "'{model}' is the only model on the Lucidos Agent's ladder. Turn another model \
                 on first."
            ));
        }
        let off = served
            .pairs()
            .into_iter()
            .filter(|p| !pairs.contains(p))
            .collect();
        (pairs, ChatLadderChange::TurnedOff(off))
    };
    PreferenceStore::set(
        pool,
        event_bus,
        prefs::INTELLIGENCE_LADDER_CHAT.key(),
        &override_value(&pairs),
        actor,
    )
    .await
    .map_err(|e| format!("Could not save the Lucidos Agent's ladder: {e}"))?;
    Ok(change)
}

/// Delete a user model, taking it off the chat ladder first. Refuses when it
/// is the ladder's last model, and then deletes nothing. Callers refuse a
/// builtin before this.
pub async fn delete_user_model(
    pool: &PgPool,
    event_bus: &EventBus,
    routes: &ModelRoutes,
    model: &str,
    actor: Option<MessageOrigin>,
) -> Result<bool, String> {
    set_model_on_chat_ladder(pool, event_bus, routes, model, false, actor.clone()).await?;
    crate::core::ModelStore::delete(pool, event_bus, model, actor)
        .await
        .map_err(|e| format!("Failed to delete model '{model}': {e}"))
}

/// Copy from Lucidos Agent: write the chat rungs that are on into `target`'s
/// override, once. Only pairs `target` offers survive, ordered on its
/// profile. A copy equal to the shipped ladder clears the override instead.
/// Returns the rungs now on. The chat override is only read.
pub async fn copy_chat_ladder_to(
    pool: &PgPool,
    event_bus: &EventBus,
    routes: &ModelRoutes,
    target: LadderSurface,
    actor: Option<MessageOrigin>,
) -> Result<Vec<IntelligenceRung>, String> {
    let agent = target
        .coding_agent()
        .ok_or("The Lucidos Agent's ladder cannot be copied onto itself.")?;
    let offered = |model: &str, effort: &str| ladder::is_offered(agent, model, effort);
    let chat = ladder::load(pool, routes, LadderSurface::Chat).await;
    let pairs = ladder::copied_from_chat(target, &chat.pairs(), offered);
    if pairs.is_empty() {
        return Err(format!(
            "{} runs none of the rungs on the Lucidos Agent's ladder, so nothing was copied.",
            agent.display_name()
        ));
    }
    let key = target.override_pref().key();
    let saved = if pairs == ladder::shipped_on(target, offered) {
        PreferenceStore::delete(pool, event_bus, key, actor)
            .await
            .map(|_| ())
    } else {
        PreferenceStore::set(pool, event_bus, key, &override_value(&pairs), actor).await
    };
    saved.map_err(|e| {
        format!(
            "Could not save the {} intelligence ladder: {e}",
            agent.display_name()
        )
    })?;
    Ok(pairs)
}

/// Sort by intelligence: write `surface`'s rungs that are on into its
/// override, least to most intelligent on its profile. The same rungs stay
/// on, and the off ones keep their shipped place. A sort equal to the shipped
/// ladder clears the override instead. Returns the rungs now on.
pub async fn sort_ladder(
    pool: &PgPool,
    event_bus: &EventBus,
    routes: &ModelRoutes,
    surface: LadderSurface,
    actor: Option<MessageOrigin>,
) -> Result<Vec<IntelligenceRung>, String> {
    let served = ladder::load(pool, routes, surface).await;
    let pairs = ladder::sorted_by_intelligence(surface, &served.pairs());
    if pairs.is_empty() {
        return Err(format!(
            "No rung is on the {} intelligence ladder, so nothing was sorted.",
            surface.display_name()
        ));
    }
    let key = surface.override_pref().key();
    let saved = if pairs == served.shipped_intelligence_rungs {
        PreferenceStore::delete(pool, event_bus, key, actor)
            .await
            .map(|_| ())
    } else {
        PreferenceStore::set(pool, event_bus, key, &override_value(&pairs), actor).await
    };
    saved.map_err(|e| {
        format!(
            "Could not save the {} intelligence ladder: {e}",
            surface.display_name()
        )
    })?;
    Ok(pairs)
}

/// The chat override a workspace upgrading from `enabled` starts with. `None`
/// when the models it had on are the shipped ladder's, or none can join.
///
/// Each model that was on keeps its shipped rungs that are on, plus the rung
/// it joins at, in shipped order. A model the shipped pairs never name joins
/// after them, unbanded. A joining rung with no estimate stays off, and a
/// model with only such a rung lists nothing.
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
        .filter(|p| ladder::is_estimated(LadderSurface::Chat, p))
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

/// Turn the models a workspace had on before the chat ladder replaced
/// `enabled` into its chat override, once. The migration snapshots them into
/// `chat_ladder_upgrade_models`, and this empties it. An override the owner
/// already set is never overwritten.
pub async fn upgrade_from_enabled(pool: &PgPool, event_bus: &EventBus, routes: &ModelRoutes) {
    let enabled: Vec<String> = match sqlx::query_scalar(
        "SELECT model_id FROM chat_ladder_upgrade_models ORDER BY model_id",
    )
    .fetch_all(pool)
    .await
    {
        Ok(enabled) => enabled,
        Err(e) => {
            log!("[IntelligenceLadder] Reading the enabled models to upgrade failed: {e}");
            return;
        }
    };
    if enabled.is_empty() {
        return;
    }
    if prefs::INTELLIGENCE_LADDER_CHAT.read(pool).await.is_none() {
        let offer = ChatOffer::load(pool, routes.clone()).await;
        if let Some(pairs) = upgraded_chat_ladder(&offer, &enabled) {
            if let Err(e) = PreferenceStore::set(
                pool,
                event_bus,
                prefs::INTELLIGENCE_LADDER_CHAT.key(),
                &override_value(&pairs),
                None,
            )
            .await
            {
                log!("[IntelligenceLadder] Saving the upgraded chat ladder failed: {e}");
                return;
            }
            log!(
                "[IntelligenceLadder] The chat ladder now lists the {} models that were enabled",
                enabled.len()
            );
        }
    }
    if let Err(e) = sqlx::query("DELETE FROM chat_ladder_upgrade_models")
        .execute(pool)
        .await
    {
        log!("[IntelligenceLadder] Clearing the chat ladder upgrade failed: {e}");
    }
}

#[cfg(test)]
#[path = "chat_ladder_tests.rs"]
mod tests;
