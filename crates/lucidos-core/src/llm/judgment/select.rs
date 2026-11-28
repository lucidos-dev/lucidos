use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use sqlx::PgPool;

use super::endpoint::{typesafe_api_key, SystemOneEndpoint};
use super::JudgmentProvider;
use crate::core::prefs::{self, Optional, Pref, Text};
use crate::llm::ProviderKind;

pub const WAKE_DECISION_DEFAULT: SystemOneEndpoint = SystemOneEndpoint::OpenAi;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JudgmentSite {
    CommandGuard,
    QueryClassification,
    MemoryFind,
    ReadDecision,
    /// INVARIANT: The turn-end gate's *wake decision*: whether a child's report wakes its
    /// parent. Every doubt wakes, so a missing answer is a wake.
    WakeDecision,
    ExpectedWakeDecision,
    PermissionAnswer,
}

impl JudgmentSite {
    pub const ALL: [Self; 7] = [
        Self::CommandGuard,
        Self::QueryClassification,
        Self::MemoryFind,
        Self::ReadDecision,
        Self::WakeDecision,
        Self::ExpectedWakeDecision,
        Self::PermissionAnswer,
    ];

    pub const fn preference_key(self) -> &'static Pref<Optional> {
        match self {
            Self::CommandGuard => &prefs::JUDGMENT_COMMAND_GUARD,
            Self::QueryClassification => &prefs::JUDGMENT_QUERY_CLASSIFICATION,
            Self::MemoryFind => &prefs::JUDGMENT_MEMORY_FIND,
            Self::ReadDecision => &prefs::JUDGMENT_READ_DECISION,
            Self::WakeDecision => &prefs::JUDGMENT_WAKE_DECISION,
            Self::ExpectedWakeDecision => &prefs::JUDGMENT_EXPECTED_WAKE_DECISION,
            Self::PermissionAnswer => &prefs::JUDGMENT_PERMISSION_ANSWER,
        }
    }

    /// INVARIANT: The chat model the site runs on while no System One row answers it.
    pub const fn chat_model_key(self) -> &'static Pref<Text> {
        match self {
            Self::CommandGuard => &prefs::MODEL_COMMAND_JUDGE,
            Self::QueryClassification => &prefs::MODEL_QUERY_CLASSIFICATION,
            Self::MemoryFind => &prefs::MODEL_MEMORY_FIND,
            Self::ReadDecision => &prefs::MODEL_READ_DECISION,
            Self::WakeDecision => &prefs::MODEL_WAKE_DECISION,
            Self::ExpectedWakeDecision => &prefs::MODEL_EXPECTED_WAKE_DECISION,
            Self::PermissionAnswer => &prefs::MODEL_PERMISSION_ANSWER,
        }
    }

    pub const fn default_endpoint(self) -> Option<SystemOneEndpoint> {
        match self {
            Self::WakeDecision => Some(WAKE_DECISION_DEFAULT),
            Self::ReadDecision
            | Self::ExpectedWakeDecision
            | Self::PermissionAnswer
            | Self::CommandGuard
            | Self::QueryClassification
            | Self::MemoryFind => None,
        }
    }

    const fn label(self) -> &'static str {
        match self {
            Self::CommandGuard => "command guard",
            Self::QueryClassification => "query classification",
            Self::MemoryFind => "memory find",
            Self::ReadDecision => "read decision",
            Self::WakeDecision => "wake decision",
            Self::ExpectedWakeDecision => "expected wake decision",
            Self::PermissionAnswer => "permission answer",
        }
    }

    fn warned(self) -> &'static AtomicBool {
        static COMMAND_GUARD: AtomicBool = AtomicBool::new(false);
        static QUERY_CLASSIFICATION: AtomicBool = AtomicBool::new(false);
        static MEMORY_FIND: AtomicBool = AtomicBool::new(false);
        static READ_DECISION: AtomicBool = AtomicBool::new(false);
        static WAKE_DECISION: AtomicBool = AtomicBool::new(false);
        static EXPECTED_WAKE_DECISION: AtomicBool = AtomicBool::new(false);
        static PERMISSION_ANSWER: AtomicBool = AtomicBool::new(false);
        match self {
            Self::CommandGuard => &COMMAND_GUARD,
            Self::QueryClassification => &QUERY_CLASSIFICATION,
            Self::MemoryFind => &MEMORY_FIND,
            Self::ReadDecision => &READ_DECISION,
            Self::WakeDecision => &WAKE_DECISION,
            Self::ExpectedWakeDecision => &EXPECTED_WAKE_DECISION,
            Self::PermissionAnswer => &PERMISSION_ANSWER,
        }
    }
}

pub fn picked_endpoint(value: Option<&str>) -> Option<SystemOneEndpoint> {
    value.and_then(SystemOneEndpoint::from_id)
}

/// INVARIANT: Whether `endpoint`'s master switch is on, or `None` when it could not be
/// read.
///
/// **The unknown is returned rather than resolved here**, because the callers
/// collapse it in opposite directions and each has to name its own side. One
/// function per direction would let a new caller pick the wrong one silently.
async fn provider_switch(pool: &PgPool, endpoint: SystemOneEndpoint) -> Option<bool> {
    match endpoint.switch_key().try_read(pool).await {
        Ok(on) => Some(on),
        Err(e) => {
            log_legacy!(
                "[Judgment] Could not read {}: {}",
                endpoint.switch_key().key(),
                e
            );
            None
        }
    }
}

pub async fn system_one_for(
    pool: &PgPool,
    site: JudgmentSite,
    timeout: Duration,
) -> Option<Box<dyn JudgmentProvider>> {
    system_one_or_default_for(pool, site, timeout, &[]).await
}

/// INVARIANT: [`system_one_for`], with an unset preference running on the site's
/// [`JudgmentSite::default_endpoint`] while `served`, the running chat
/// provider's configured backends, holds that row's chat provider.
///
/// A chat model the user pinned for the site is a choice of chat. The default
/// never moves it, so its text never reaches the default's vendor.
///
/// The mock provider serves nothing, so a test engine never reaches a vendor.
pub async fn system_one_or_default_for(
    pool: &PgPool,
    site: JudgmentSite,
    timeout: Duration,
    served: &[ProviderKind],
) -> Option<Box<dyn JudgmentProvider>> {
    let preference = match site.preference_key().try_stored(pool).await {
        Ok(value) => value.filter(|v| !v.trim().is_empty()),
        Err(e) => {
            log_legacy!(
                "[Judgment] Could not read {}: {}. Running the {} on its chat model",
                site.preference_key().key(),
                e,
                site.label()
            );
            return None;
        }
    };
    let picked = preference.is_some();
    let endpoint = match preference {
        Some(value) => picked_endpoint(Some(&value))?,
        None => {
            let endpoint = site.default_endpoint().filter(|endpoint| {
                endpoint
                    .chat_provider()
                    .is_some_and(|k| served.contains(&k))
            })?;
            if !chat_model_unpinned(pool, site).await {
                return None;
            }
            endpoint
        }
    };

    // INVARIANT: Read second, so a workspace with every site on chat pays no extra query.
    // An unknown reads as OFF: the fallback is a working chat model.
    if !provider_switch(pool, endpoint).await.unwrap_or(false) {
        return None;
    }

    match endpoint.provider(pool, timeout).await {
        Ok(provider) => Some(provider),
        Err(reason) => {
            let how = if picked { "picks" } else { "defaults to" };
            warn_once(
                site,
                format!(
                    "[Judgment] {} {} {}, but {}. Running it on its chat model meanwhile",
                    site.preference_key().key(),
                    how,
                    endpoint.label(),
                    reason
                ),
            );
            None
        }
    }
}

async fn chat_model_unpinned(pool: &PgPool, site: JudgmentSite) -> bool {
    match site.chat_model_key().try_stored(pool).await {
        Ok(stored) => stored.is_none_or(|model| model.trim().is_empty()),
        Err(e) => {
            log_legacy!(
                "[Judgment] Could not read {}: {}. Running the {} on its chat model",
                site.chat_model_key().key(),
                e,
                site.label()
            );
            false
        }
    }
}

/// INVARIANT: Whether the `judge` tool is offered to this workspace.
///
/// Jev's master switch plus a resolvable key, and nothing else. Read once per
/// turn by `read_turn_capabilities`, which is why it stops at a credential
/// lookup and never builds a client.
///
/// **The key is read first, because almost no workspace has one.** A definite
/// no settles the gate in one query, where reading the switch first would cost
/// every keyless workspace a second one on every turn.
///
/// **An unreadable switch resolves ON**, the opposite of what a judgment site
/// does with the same row. Shutting this gate withdraws a capability and
/// rewrites the turn's tools cache tier, which `read_turn_capabilities` says
/// never to do on an unknown. An absent key still closes it, so an unknown
/// never opens the gate alone.
pub async fn judgment_available(pool: &PgPool) -> bool {
    typesafe_api_key(pool).await.is_some()
        && provider_switch(pool, SystemOneEndpoint::Jev)
            .await
            .unwrap_or(true)
}

/// INVARIANT: The provider behind the `judge` tool, or the reason to tell the agent.
///
/// The reason is returned rather than logged, because this caller's reader is
/// the model. A silent `None` would leave it retrying a tool that cannot work.
///
/// It asks the same two questions [`judgment_available`] does and collapses the
/// unknown the same way, so the gate and the handler cannot disagree. The order
/// is reversed on purpose. A switched-off provider is the more actionable thing
/// to name when both are true, and the extra query costs nothing here.
pub async fn jev_for_agent(
    pool: &PgPool,
    timeout: Duration,
) -> Result<Box<dyn JudgmentProvider>, &'static str> {
    if !provider_switch(pool, SystemOneEndpoint::Jev)
        .await
        .unwrap_or(true)
    {
        return Err(concat!(
            "TypeSafe is switched off in ",
            providers_settings!()
        ));
    }
    if typesafe_api_key(pool).await.is_none() {
        return Err(concat!(
            "no TypeSafe API key is stored in ",
            providers_settings!()
        ));
    }
    SystemOneEndpoint::Jev
        .provider(pool, timeout)
        .await
        .map_err(|e| {
            log_legacy!("[Judgment] Jev for the judge tool: {}", e);
            "the TypeSafe client could not be built, see the engine log"
        })
}

fn warn_once(site: JudgmentSite, message: String) {
    if !site.warned().swap(true, Ordering::Relaxed) {
        log_legacy!("{}", message);
    }
}

#[cfg(test)]
#[path = "select_tests.rs"]
mod tests;
