//! Whether one call site runs on Jev, and the provider when it does.
//!
//! **Every doubt resolves to `chat`.** An unset preference, an unreadable one,
//! a value nobody recognizes, a missing key, a client that will not build: all
//! of them return `None`, and the caller runs the path it has always run.
//! Installing a TypeSafe key therefore changes nothing on its own, which is
//! the promise this module exists to keep.
//!
//! **Two switches, and both must say yes.** The master switch over the whole
//! provider is `provider_enabled_typesafe`, set on Settings → Models →
//! Providers. Under it sits one preference per call site. Absent means on for
//! the master and `chat` for a site, so an untouched workspace is unchanged.
//!
//! **The `judge` tool has one switch, not two, and that is deliberate.** A
//! [`JudgmentSite`] is a decision the engine already makes on a chat model, so
//! its preference chooses which backend answers. The tool replaces nothing: it
//! is a capability that exists or does not, like `generate_image` behind a
//! configured image provider. So [`judgment_available`] asks the key and the
//! master switch, and there is no third preference to set.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use sqlx::PgPool;

use super::JevProvider;
use crate::core::{
    CredentialStore, PreferenceStore, PREF_JUDGMENT_COMMAND_GUARD,
    PREF_JUDGMENT_QUERY_CLASSIFICATION, PREF_PROVIDER_ENABLED_TYPESAFE,
};
use crate::llm::provider_build::switch_is_on;

/// The launch environment's key, read when no `typesafe` credential is stored.
pub const TYPESAFE_API_KEY_ENV: &str = "TYPESAFE_API_KEY";

/// The credential service name holding the key.
pub const TYPESAFE_CREDENTIAL_SERVICE: &str = "typesafe";

/// The preference value that opts a call site in. Anything else means `chat`.
const JEV: &str = "jev";

/// A classification call site that can run on Jev.
///
/// Each owns one preference, so opting one in never moves the other. That
/// split is deliberate: the command guard is a safety gate and the user may
/// well want it on a different footing from memory retrieval.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JudgmentSite {
    /// The command guard's judge over the ambiguous middle (ADR 0002).
    CommandGuard,
    /// The three yes/no questions in front of memory retrieval.
    QueryClassification,
}

impl JudgmentSite {
    pub const fn preference_key(self) -> &'static str {
        match self {
            Self::CommandGuard => PREF_JUDGMENT_COMMAND_GUARD,
            Self::QueryClassification => PREF_JUDGMENT_QUERY_CLASSIFICATION,
        }
    }

    /// The name in a log line. Not user-facing.
    const fn label(self) -> &'static str {
        match self {
            Self::CommandGuard => "command guard",
            Self::QueryClassification => "query classification",
        }
    }

    /// Whether a misconfiguration has already been logged for this site.
    fn warned(self) -> &'static AtomicBool {
        static COMMAND_GUARD: AtomicBool = AtomicBool::new(false);
        static QUERY_CLASSIFICATION: AtomicBool = AtomicBool::new(false);
        match self {
            Self::CommandGuard => &COMMAND_GUARD,
            Self::QueryClassification => &QUERY_CLASSIFICATION,
        }
    }
}

/// Whether the stored preference value opts this site in.
///
/// Case and surrounding space are forgiven, because a human types this value.
/// Nothing else is: an unrecognized word is not a guess to resolve, so it
/// reads as `chat`.
pub fn wants_jev(value: Option<&str>) -> bool {
    value
        .map(str::trim)
        .is_some_and(|v| v.eq_ignore_ascii_case(JEV))
}

/// Whether the TypeSafe master switch is on (Settings → Models → Providers),
/// or `None` when the row could not be read.
///
/// Absent means on, like every other `provider_enabled_*` key, so a workspace
/// that never touched the switch runs exactly what it ran before it existed.
///
/// **The unknown is returned rather than resolved here**, because the callers
/// collapse it in opposite directions and each has to name its own side. One
/// function per direction would let a new caller pick the wrong one silently.
async fn typesafe_switch(pool: &PgPool) -> Option<bool> {
    match PreferenceStore::get(pool, PREF_PROVIDER_ENABLED_TYPESAFE).await {
        Ok(value) => Some(switch_is_on(value.as_deref())),
        Err(e) => {
            log!(
                "[Judgment] Could not read {}: {}",
                PREF_PROVIDER_ENABLED_TYPESAFE,
                e
            );
            None
        }
    }
}

/// The TypeSafe key, from the stored credential or the launch environment.
///
/// Returns `None` rather than an error, because no key is the ordinary state
/// of a workspace and not a fault.
async fn api_key(pool: &PgPool) -> Option<String> {
    match CredentialStore::get(pool, TYPESAFE_CREDENTIAL_SERVICE).await {
        Ok(Some(c)) if !c.auth_value.trim().is_empty() => return Some(c.auth_value),
        Ok(_) => {}
        Err(e) => log!("[Judgment] Could not read the TypeSafe credential: {}", e),
    }
    std::env::var(TYPESAFE_API_KEY_ENV)
        .ok()
        .filter(|k| !k.trim().is_empty())
}

/// The Jev provider for `site`, or `None` to run the site's own path.
pub async fn jev_for(pool: &PgPool, site: JudgmentSite, timeout: Duration) -> Option<JevProvider> {
    let preference = match PreferenceStore::get(pool, site.preference_key()).await {
        Ok(value) => value,
        Err(e) => {
            log!(
                "[Judgment] Could not read {}: {}. Running the {} on its own path",
                site.preference_key(),
                e,
                site.label()
            );
            return None;
        }
    };
    if !wants_jev(preference.as_deref()) {
        return None;
    }

    // Read second, so a workspace with both sites on chat pays no extra query.
    // The master switch only decides anything once a site has asked for Jev.
    // An unknown reads as OFF: the fallback is a working chat path.
    if !typesafe_switch(pool).await.unwrap_or(false) {
        return None;
    }

    let Some(key) = api_key(pool).await else {
        warn_once(
            site,
            format!(
                "[Judgment] {} asks for Jev, but no TypeSafe key is set. \
                 Store one as the '{}' credential or set {}. Running its own path meanwhile",
                site.preference_key(),
                TYPESAFE_CREDENTIAL_SERVICE,
                TYPESAFE_API_KEY_ENV
            ),
        );
        return None;
    };

    match JevProvider::new(key, timeout) {
        Ok(provider) => Some(provider),
        Err(e) => {
            warn_once(
                site,
                format!(
                    "[Judgment] Could not build the Jev client for the {}: {}",
                    site.label(),
                    e
                ),
            );
            None
        }
    }
}

/// Whether the `judge` tool is offered to this workspace.
///
/// The master switch plus a resolvable key, and nothing else. Read once per
/// turn by `read_turn_capabilities`, which is why it stops at a credential
/// lookup and never builds a client.
///
/// **The key is read first, because almost no workspace has one.** A definite
/// no settles the gate in one query, where reading the switch first would cost
/// every keyless workspace a second one on every turn.
///
/// **An unreadable switch resolves ON**, the opposite of what a classification
/// site does with the same row. Shutting this gate withdraws a capability and
/// rewrites the turn's tools cache tier, which `read_turn_capabilities` says
/// never to do on an unknown. An absent key still closes it, so an unknown
/// never opens the gate alone.
pub async fn judgment_available(pool: &PgPool) -> bool {
    api_key(pool).await.is_some() && typesafe_switch(pool).await.unwrap_or(true)
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
pub async fn jev_for_agent(pool: &PgPool, timeout: Duration) -> Result<JevProvider, &'static str> {
    if !typesafe_switch(pool).await.unwrap_or(true) {
        return Err("TypeSafe is switched off in Settings → Models → Providers");
    }
    let Some(key) = api_key(pool).await else {
        return Err("no TypeSafe API key is stored in Settings → Models → Providers");
    };
    JevProvider::new(key, timeout).map_err(|e| {
        log!(
            "[Judgment] Could not build the Jev client for the judge tool: {}",
            e
        );
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
