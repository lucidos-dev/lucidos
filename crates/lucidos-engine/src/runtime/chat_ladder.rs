use sqlx::PgPool;

use super::chat_ladder_policy::upgraded_chat_ladder;
#[cfg(test)]
use super::intelligence_ladder::SurfaceOffer;
use super::intelligence_ladder::{
    self as ladder, ChatOffer, IntelligenceRung, LadderSurface, ModelRoutes,
};
use crate::core::preference_catalog as prefs;
use crate::core::PreferenceStore;
use crate::engine::event_bus::EventBus;
use crate::engine::thread_events::MessageOrigin;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChatLadderChange {
    Unchanged,
    TurnedOn(IntelligenceRung),
    TurnedOff(Vec<IntelligenceRung>),
}

impl ChatLadderChange {
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

fn override_value(pairs: &[IntelligenceRung]) -> String {
    pairs
        .iter()
        .map(IntelligenceRung::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

pub async fn set_model_on_chat_ladder(
    pool: &PgPool,
    event_bus: &EventBus,
    routes: &ModelRoutes,
    model: &str,
    on: bool,
    actor: Option<MessageOrigin>,
) -> Result<ChatLadderChange, String> {
    let served = ladder::load(pool, routes, LadderSurface::Chat).await;
    let default = if on {
        None
    } else {
        default_model(pool, routes, &served).await
    };
    // INVARIANT: Before the unchanged answer: a default off the ladder stays undeletable.
    served.refuse_the_default(model, default.as_deref())?;
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
        let pairs = served.intelligence_rungs_without(model, default.as_deref())?;
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

pub async fn set_default_intelligence_rung(
    pool: &PgPool,
    event_bus: &EventBus,
    routes: &ModelRoutes,
    surface: LadderSurface,
    pick: &IntelligenceRung,
    actor: MessageOrigin,
) -> Result<(), String> {
    let served = ladder::load(pool, routes, surface).await;
    let at = served.position_of(pick).ok_or_else(|| {
        format!(
            "{pick} is not offered on the {} intelligence ladder.",
            surface.display_name()
        )
    })?;
    let rung = &served.intelligence_rungs[at];
    let save = |key: &'static str, value: String| {
        let actor = actor.clone();
        async move { PreferenceStore::set(pool, event_bus, key, &value, Some(actor)).await }
    };

    let (model_key, effort_key, effort_value) = match surface {
        LadderSurface::Chat => {
            let stored = prefs::CHAT_REASONING_EFFORTS
                .read(pool)
                .await
                .unwrap_or_default();
            let mut efforts = prefs::parse_model_efforts(&stored)?;
            efforts.retain(|(model, _)| *model != rung.model);
            let mut efforts: Vec<_> = efforts
                .into_iter()
                .map(|(model, effort)| format!("{model}={effort}"))
                .collect();
            efforts.push(format!("{}={}", rung.model, rung.effort));
            (
                prefs::CHAT_MODEL.key(),
                prefs::CHAT_REASONING_EFFORTS.key(),
                efforts.join(", "),
            )
        }
        LadderSurface::ClaudeCode => (
            prefs::MODEL_CLAUDE_CODE.key(),
            prefs::REASONING_CLAUDE_CODE.key(),
            rung.effort.clone(),
        ),
        LadderSurface::Codex => (
            prefs::MODEL_CODEX.key(),
            prefs::REASONING_CODEX.key(),
            rung.effort.clone(),
        ),
    };

    if !rung.on {
        let key = surface.override_pref();
        if let Some(stored) = key.read(pool).await {
            if let Ok(override_pairs) = prefs::parse_intelligence_ladder(&stored, surface.efforts())
            {
                let mut pairs: Vec<_> = override_pairs
                    .into_iter()
                    .map(|(model, effort)| IntelligenceRung::new(model, effort))
                    .collect();
                // INVARIANT: Work from the stored list so unavailable rungs and the old
                // default's read-time placement do not change what is saved.
                let insert_at = pairs
                    .iter()
                    .position(|pair| {
                        served
                            .position_of(pair)
                            .is_some_and(|position| position > at)
                    })
                    .unwrap_or(pairs.len());
                pairs.insert(insert_at, IntelligenceRung::new(&rung.model, &rung.effort));
                save(key.key(), override_value(&pairs))
                    .await
                    .map_err(|e| e.to_string())?;
            }
        }
    }
    save(effort_key, effort_value)
        .await
        .map_err(|e| e.to_string())?;
    save(model_key, rung.model.clone())
        .await
        .map_err(|e| e.to_string())
}

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
    let shipped = ladder::shipped_on(target, offered);
    save_override(pool, event_bus, target, &shipped, &pairs, actor).await?;
    Ok(pairs)
}

async fn save_override(
    pool: &PgPool,
    event_bus: &EventBus,
    surface: LadderSurface,
    shipped: &[IntelligenceRung],
    pairs: &[IntelligenceRung],
    actor: Option<MessageOrigin>,
) -> Result<(), String> {
    let key = surface.override_pref().key();
    let saved = if pairs == shipped {
        PreferenceStore::delete(pool, event_bus, key, actor)
            .await
            .map(|_| ())
    } else {
        PreferenceStore::set(pool, event_bus, key, &override_value(pairs), actor).await
    };
    saved.map_err(|e| {
        format!(
            "Could not save the {} intelligence ladder: {e}",
            surface.display_name()
        )
    })
}

async fn default_model(
    pool: &PgPool,
    routes: &ModelRoutes,
    served: &ladder::IntelligenceLadder,
) -> Option<String> {
    match (&served.default_intelligence_rung, served.surface) {
        (Some(default), _) => Some(default.model.clone()),
        (None, LadderSurface::Chat) => {
            let offer = ChatOffer::load(pool, routes.clone()).await;
            Some(ladder::default_chat_selection(pool, &offer).await.model)
        }
        (None, _) => None,
    }
}

pub async fn set_intelligence_rungs(
    pool: &PgPool,
    event_bus: &EventBus,
    routes: &ModelRoutes,
    surface: LadderSurface,
    asked: &[IntelligenceRung],
    actor: Option<MessageOrigin>,
) -> Result<Vec<IntelligenceRung>, String> {
    let served = ladder::load(pool, routes, surface).await;
    let pairs = served
        .intelligence_rungs_set_to(asked, default_model(pool, routes, &served).await.as_deref())?;
    save_override(
        pool,
        event_bus,
        surface,
        &served.shipped_intelligence_rungs,
        &pairs,
        actor,
    )
    .await?;
    Ok(pairs)
}

pub async fn remove_model_from_ladder(
    pool: &PgPool,
    event_bus: &EventBus,
    routes: &ModelRoutes,
    surface: LadderSurface,
    model: &str,
    actor: Option<MessageOrigin>,
) -> Result<Vec<IntelligenceRung>, String> {
    let served = ladder::load(pool, routes, surface).await;
    let pairs = served
        .intelligence_rungs_without(model, default_model(pool, routes, &served).await.as_deref())?;
    save_override(
        pool,
        event_bus,
        surface,
        &served.shipped_intelligence_rungs,
        &pairs,
        actor,
    )
    .await?;
    Ok(pairs)
}

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
    save_override(
        pool,
        event_bus,
        surface,
        &served.shipped_intelligence_rungs,
        &pairs,
        actor,
    )
    .await?;
    Ok(pairs)
}

pub async fn upgrade_from_enabled(pool: &PgPool, event_bus: &EventBus, routes: &ModelRoutes) {
    let enabled: Vec<String> = match sqlx::query_scalar(
        "SELECT model_id FROM chat_ladder_upgrade_models ORDER BY model_id",
    )
    .fetch_all(pool)
    .await
    {
        Ok(enabled) => enabled,
        Err(e) => {
            log_legacy!("[IntelligenceLadder] Reading the enabled models to upgrade failed: {e}");
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
                log_legacy!("[IntelligenceLadder] Saving the upgraded chat ladder failed: {e}");
                return;
            }
            log_legacy!(
                "[IntelligenceLadder] The chat ladder now lists the {} models that were enabled",
                enabled.len()
            );
        }
    }
    if let Err(e) = sqlx::query("DELETE FROM chat_ladder_upgrade_models")
        .execute(pool)
        .await
    {
        log_legacy!("[IntelligenceLadder] Clearing the chat ladder upgrade failed: {e}");
    }
}

#[cfg(test)]
#[path = "chat_ladder_tests.rs"]
mod tests;
