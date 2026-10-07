//! Which backend answers one judgment site: its chat model, or a System One
//! row the user picked for it.
//!
//! **Every doubt resolves to `chat`.** An unset preference, an unreadable one,
//! a value nobody recognizes, a switched-off provider, a missing key, a client
//! that will not build: all of them return `None`, and the site asks its chat
//! model. So storing a credential changes no site on its own. That is the
//! promise this module keeps (ADR 0363, I13 and I14 of
//! `docs/plans/2026-10-04-tree-memory-module-and-the-home-thread.md`).
//!
//! **Two switches, and both must say yes.** Each System One provider has a
//! master switch on Settings → Models → Providers. Under it sits one preference
//! per site. Absent means on for the master and `chat` for a site.
//!
//! **The `judge` tool has one switch, not two, and that is deliberate.** A
//! [`JudgmentSite`] is a decision the engine already makes on a chat model, so
//! its preference chooses which backend answers. The tool replaces nothing: it
//! is a capability that exists or does not, like `generate_image` behind a
//! configured image provider. So [`judgment_available`] asks Jev's key and
//! master switch, and there is no third preference to set.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use sqlx::PgPool;

use super::endpoint::{typesafe_api_key, SystemOneEndpoint};
use super::SystemOneProvider;
use crate::core::prefs::{self, Optional, Pref};

/// A call site that asks typed questions.
///
/// Each owns one preference, so moving one never moves another. That split is
/// deliberate: the command guard is a safety gate and the user may well want
/// it on a different footing from memory retrieval.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JudgmentSite {
    /// The command guard's judge over the ambiguous middle (ADR 0002).
    CommandGuard,
    /// The three yes/no questions in front of memory retrieval.
    QueryClassification,
    /// The Tree memory module's `find`, which walks a summary tree by asking
    /// choice questions over its lines (ADR 0362). Nothing asks it yet.
    MemoryFind,
}

impl JudgmentSite {
    pub const ALL: [Self; 3] = [
        Self::CommandGuard,
        Self::QueryClassification,
        Self::MemoryFind,
    ];

    pub const fn preference_key(self) -> &'static Pref<Optional> {
        match self {
            Self::CommandGuard => &prefs::JUDGMENT_COMMAND_GUARD,
            Self::QueryClassification => &prefs::JUDGMENT_QUERY_CLASSIFICATION,
            Self::MemoryFind => &prefs::JUDGMENT_MEMORY_FIND,
        }
    }

    /// The name in a log line. Not user-facing.
    const fn label(self) -> &'static str {
        match self {
            Self::CommandGuard => "command guard",
            Self::QueryClassification => "query classification",
            Self::MemoryFind => "memory find",
        }
    }

    /// Whether a misconfiguration has already been logged for this site.
    fn warned(self) -> &'static AtomicBool {
        static COMMAND_GUARD: AtomicBool = AtomicBool::new(false);
        static QUERY_CLASSIFICATION: AtomicBool = AtomicBool::new(false);
        static MEMORY_FIND: AtomicBool = AtomicBool::new(false);
        match self {
            Self::CommandGuard => &COMMAND_GUARD,
            Self::QueryClassification => &QUERY_CLASSIFICATION,
            Self::MemoryFind => &MEMORY_FIND,
        }
    }
}

/// The System One row a stored site preference picks, or `None` for chat.
pub fn picked_endpoint(value: Option<&str>) -> Option<SystemOneEndpoint> {
    value.and_then(SystemOneEndpoint::from_id)
}

/// Whether `endpoint`'s master switch is on, or `None` when it could not be
/// read.
///
/// **The unknown is returned rather than resolved here**, because the callers
/// collapse it in opposite directions and each has to name its own side. One
/// function per direction would let a new caller pick the wrong one silently.
async fn provider_switch(pool: &PgPool, endpoint: SystemOneEndpoint) -> Option<bool> {
    match endpoint.switch_key().try_read(pool).await {
        Ok(on) => Some(on),
        Err(e) => {
            log!(
                "[Judgment] Could not read {}: {}",
                endpoint.switch_key().key(),
                e
            );
            None
        }
    }
}

/// The System One provider for `site`, or `None` to ask its chat model.
pub async fn system_one_for(
    pool: &PgPool,
    site: JudgmentSite,
    timeout: Duration,
) -> Option<SystemOneProvider> {
    let preference = match site.preference_key().try_stored(pool).await {
        Ok(value) => value,
        Err(e) => {
            log!(
                "[Judgment] Could not read {}: {}. Running the {} on its chat model",
                site.preference_key().key(),
                e,
                site.label()
            );
            return None;
        }
    };
    let endpoint = picked_endpoint(preference.as_deref())?;

    // Read second, so a workspace with every site on chat pays no extra query.
    // An unknown reads as OFF: the fallback is a working chat model.
    if !provider_switch(pool, endpoint).await.unwrap_or(false) {
        return None;
    }

    match endpoint.provider(pool, timeout).await {
        Ok(provider) => Some(provider),
        Err(reason) => {
            warn_once(
                site,
                format!(
                    "[Judgment] {} picks {}, but {}. Running it on its chat model meanwhile",
                    site.preference_key().key(),
                    endpoint.label(),
                    reason
                ),
            );
            None
        }
    }
}

/// Whether the `judge` tool is offered to this workspace.
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

/// The provider behind the `judge` tool, or the reason to tell the agent.
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
) -> Result<SystemOneProvider, &'static str> {
    if !provider_switch(pool, SystemOneEndpoint::Jev)
        .await
        .unwrap_or(true)
    {
        return Err("TypeSafe is switched off in Settings → Models → Providers");
    }
    if typesafe_api_key(pool).await.is_none() {
        return Err("no TypeSafe API key is stored in Settings → Models → Providers");
    }
    SystemOneEndpoint::Jev
        .provider(pool, timeout)
        .await
        .map_err(|e| {
            log!("[Judgment] Jev for the judge tool: {}", e);
            "the TypeSafe client could not be built, see the engine log"
        })
}

/// Log a misconfiguration the first time this site hits it.
///
/// The command guard runs in front of every ambiguous command, so a line per
/// call would bury the log in the one message worth reading.
fn warn_once(site: JudgmentSite, message: String) {
    if !site.warned().swap(true, Ordering::Relaxed) {
        log!("{}", message);
    }
}

#[cfg(test)]
#[path = "select_tests.rs"]
mod tests;
