//! The System One rows a judgment site can pick (ADR 0363).
//!
//! Three are seeded: TypeSafe's Jev, and Cloudflare's Clef and Clef-flash on
//! Workers AI. The fourth is a custom endpoint the user points at, such as a
//! self-hosted open model. All four speak one wire, so a row is only a URL, a
//! model name and where its key lives.
//!
//! Each provider has its own master switch, defaulting to on, like every
//! other `provider_enabled_*` key. Clef and Clef-flash share Cloudflare's.

use std::time::Duration;

use sqlx::PgPool;

use super::{SystemOneProvider, JEV_DEFAULT_MODEL, TYPESAFE_API_BASE_URL};
use crate::core::prefs::{self, Flag, Pref};
use crate::core::{credential_scope_covers, CredentialStore};

/// The launch environment's key, read when no `typesafe` credential is stored.
pub const TYPESAFE_API_KEY_ENV: &str = "TYPESAFE_API_KEY";

/// The credential service name holding TypeSafe's key.
pub const TYPESAFE_CREDENTIAL_SERVICE: &str = "typesafe";

/// The credential service name holding a Workers AI token.
///
/// Its scope URL carries the account, as
/// `https://api.cloudflare.com/client/v4/accounts/<account id>/ai`, so one
/// credential holds everything Clef needs.
pub const CLOUDFLARE_WORKERS_AI_CREDENTIAL_SERVICE: &str = "cloudflare-workers-ai";

/// The credential service name holding the custom endpoint's optional key.
pub const SYSTEM_ONE_CUSTOM_CREDENTIAL_SERVICE: &str = "system-one-custom";

/// What every Workers AI scope URL starts with, before the account id.
pub(crate) const CLOUDFLARE_ACCOUNTS_PREFIX: &str =
    "https://api.cloudflare.com/client/v4/accounts/";

/// One System One row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SystemOneEndpoint {
    Jev,
    Clef,
    ClefFlash,
    Custom,
}

impl SystemOneEndpoint {
    pub const ALL: [Self; 4] = [Self::Jev, Self::Clef, Self::ClefFlash, Self::Custom];

    /// The value a `judgment_*` preference stores to pick this row.
    pub const fn id(self) -> &'static str {
        match self {
            Self::Jev => "jev",
            Self::Clef => "clef",
            Self::ClefFlash => "clef-flash",
            Self::Custom => "custom",
        }
    }

    /// The row a stored value names, or `None`.
    ///
    /// Case and surrounding space are forgiven, because a human types this
    /// value. Nothing else is: an unrecognized word is not a guess to resolve.
    pub fn from_id(value: &str) -> Option<Self> {
        let value = value.trim();
        Self::ALL
            .into_iter()
            .find(|endpoint| endpoint.id().eq_ignore_ascii_case(value))
    }

    /// The name in a log line.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Jev => "TypeSafe (Jev)",
            Self::Clef => "Cloudflare Clef",
            Self::ClefFlash => "Cloudflare Clef-flash",
            Self::Custom => "the custom System One endpoint",
        }
    }

    /// The master switch over this row's provider.
    pub const fn switch_key(self) -> &'static Pref<Flag> {
        match self {
            Self::Jev => &prefs::PROVIDER_ENABLED_TYPESAFE,
            Self::Clef | Self::ClefFlash => &prefs::PROVIDER_ENABLED_CLOUDFLARE_WORKERS_AI,
            Self::Custom => &prefs::PROVIDER_ENABLED_SYSTEM_ONE_CUSTOM,
        }
    }

    pub const fn credential_service(self) -> &'static str {
        match self {
            Self::Jev => TYPESAFE_CREDENTIAL_SERVICE,
            Self::Clef | Self::ClefFlash => CLOUDFLARE_WORKERS_AI_CREDENTIAL_SERVICE,
            Self::Custom => SYSTEM_ONE_CUSTOM_CREDENTIAL_SERVICE,
        }
    }

    /// Build this row's provider, or say what is missing.
    ///
    /// The reason is for a log line, so it names where to fix it and never
    /// carries the key.
    pub async fn provider(
        self,
        pool: &PgPool,
        timeout: Duration,
    ) -> Result<SystemOneProvider, String> {
        let (url, model, api_key) = match self {
            Self::Jev => {
                let key = typesafe_api_key(pool).await.ok_or_else(|| {
                    format!(
                        "no TypeSafe key is set. Store one as the '{}' credential or set {}",
                        TYPESAFE_CREDENTIAL_SERVICE, TYPESAFE_API_KEY_ENV
                    )
                })?;
                (
                    format!("{TYPESAFE_API_BASE_URL}/systemone"),
                    JEV_DEFAULT_MODEL.to_string(),
                    Some(key),
                )
            }
            Self::Clef | Self::ClefFlash => {
                let model = if self == Self::Clef {
                    "clef"
                } else {
                    "clef-flash"
                };
                let credential = stored_credential(pool, self.credential_service())
                    .await
                    .ok_or("no Cloudflare Workers AI token is stored")?;
                let base = credential
                    .base_urls
                    .first()
                    .map(String::as_str)
                    .unwrap_or("");
                let url = workers_ai_url(base, model).ok_or(
                    "the Cloudflare Workers AI credential names no account. Save it again in \
                     Settings → Models → Providers",
                )?;
                (url, model.to_string(), Some(credential.auth_value))
            }
            Self::Custom => {
                let url = prefs::SYSTEM_ONE_CUSTOM_URL
                    .read(pool)
                    .await
                    .filter(|u| u.starts_with("http://") || u.starts_with("https://"))
                    .ok_or("the custom System One endpoint has no http(s) URL set")?;
                let model = prefs::SYSTEM_ONE_CUSTOM_MODEL
                    .read(pool)
                    .await
                    .ok_or("the custom System One endpoint has no model set")?;
                // The key goes only where its scope covers the URL, as the
                // Local provider's does. A moved URL must not carry it along.
                let key = stored_credential(pool, self.credential_service())
                    .await
                    .filter(|c| credential_scope_covers(&c.base_urls, &url))
                    .map(|c| c.auth_value);
                (url, model, key)
            }
        };
        SystemOneProvider::new(url, model, api_key, timeout)
            .map_err(|e| format!("could not build the {} client: {}", self.label(), e))
    }
}

/// The Clef request URL under one Workers AI scope URL, or `None` when the
/// scope does not name an account.
pub(crate) fn workers_ai_url(scope: &str, model: &str) -> Option<String> {
    let scope = scope.trim().trim_end_matches('/');
    let account = scope
        .strip_prefix(CLOUDFLARE_ACCOUNTS_PREFIX)?
        .strip_suffix("/ai")?;
    let well_formed = !account.is_empty() && account.chars().all(|c| c.is_ascii_alphanumeric());
    well_formed.then(|| format!("{scope}/run/@cf/cloudflare/{model}"))
}

/// The TypeSafe key, from the stored credential or the launch environment.
///
/// Returns `None` rather than an error, because no key is the ordinary state
/// of a workspace and not a fault.
pub(crate) async fn typesafe_api_key(pool: &PgPool) -> Option<String> {
    if let Some(credential) = stored_credential(pool, TYPESAFE_CREDENTIAL_SERVICE).await {
        return Some(credential.auth_value);
    }
    std::env::var(TYPESAFE_API_KEY_ENV)
        .ok()
        .filter(|k| !k.trim().is_empty())
}

/// A stored credential with a non-blank secret, or `None`. A read error logs.
async fn stored_credential(pool: &PgPool, service: &str) -> Option<crate::core::Credential> {
    match CredentialStore::get(pool, service).await {
        Ok(Some(c)) if !c.auth_value.trim().is_empty() => Some(c),
        Ok(_) => None,
        Err(e) => {
            log!(
                "[Judgment] Could not read the '{}' credential: {}",
                service,
                e
            );
            None
        }
    }
}

#[cfg(test)]
#[path = "endpoint_tests.rs"]
mod tests;
