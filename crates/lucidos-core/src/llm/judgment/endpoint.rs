use std::time::Duration;

use sqlx::PgPool;

use super::{
    JudgmentProvider, OpenAiDecisionsProvider, SystemOneProvider, JEV_DEFAULT_MODEL,
    TYPESAFE_API_BASE_URL,
};
use crate::core::prefs::{self, Flag, Pref};
use crate::core::{credential_scope_covers, CredentialStore};
use crate::llm::{ProviderKind, OPENROUTER_API_KEY_ENV, OPENROUTER_BASE_URL};

pub const TYPESAFE_API_KEY_ENV: &str = "TYPESAFE_API_KEY";

pub const TYPESAFE_CREDENTIAL_SERVICE: &str = "typesafe";

pub const CLOUDFLARE_WORKERS_AI_CREDENTIAL_SERVICE: &str = "cloudflare-workers-ai";

pub const SYSTEM_ONE_CUSTOM_CREDENTIAL_SERVICE: &str = "system-one-custom";

pub const OPENROUTER_JEV_MODEL: &str = "~typesafe/jev-latest";

pub const CLOUDFLARE_ACCOUNTS_PREFIX: &str = "https://api.cloudflare.com/client/v4/accounts/";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SystemOneEndpoint {
    Jev,
    OpenRouter,
    Clef,
    ClefFlash,
    OpenAi,
    Custom,
}

impl SystemOneEndpoint {
    pub const ALL: [Self; 6] = [
        Self::Jev,
        Self::OpenRouter,
        Self::Clef,
        Self::ClefFlash,
        Self::OpenAi,
        Self::Custom,
    ];

    pub const fn id(self) -> &'static str {
        match self {
            Self::Jev => "jev",
            Self::OpenRouter => "openrouter",
            Self::Clef => "clef",
            Self::ClefFlash => "clef-flash",
            Self::OpenAi => "openai",
            Self::Custom => "custom",
        }
    }

    pub fn from_id(value: &str) -> Option<Self> {
        let value = value.trim();
        Self::ALL
            .into_iter()
            .find(|endpoint| endpoint.id().eq_ignore_ascii_case(value))
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Jev => "TypeSafe (Jev)",
            Self::OpenRouter => "OpenRouter System One",
            Self::Clef => "Cloudflare Clef",
            Self::ClefFlash => "Cloudflare Clef-flash",
            Self::OpenAi => "OpenAI Decisions",
            Self::Custom => "the custom System One endpoint",
        }
    }

    pub const fn switch_key(self) -> &'static Pref<Flag> {
        match self {
            Self::Jev => &prefs::PROVIDER_ENABLED_TYPESAFE,
            Self::OpenRouter => &prefs::PROVIDER_ENABLED_OPENROUTER,
            Self::Clef | Self::ClefFlash => &prefs::PROVIDER_ENABLED_CLOUDFLARE_WORKERS_AI,
            Self::OpenAi => &prefs::PROVIDER_ENABLED_OPENAI,
            Self::Custom => &prefs::PROVIDER_ENABLED_SYSTEM_ONE_CUSTOM,
        }
    }

    pub const fn credential_service(self) -> &'static str {
        match self {
            Self::Jev => TYPESAFE_CREDENTIAL_SERVICE,
            Self::OpenRouter => ProviderKind::OpenRouter.as_str(),
            Self::Clef | Self::ClefFlash => CLOUDFLARE_WORKERS_AI_CREDENTIAL_SERVICE,
            Self::OpenAi => ProviderKind::OpenAi.as_str(),
            Self::Custom => SYSTEM_ONE_CUSTOM_CREDENTIAL_SERVICE,
        }
    }

    pub const fn chat_provider(self) -> Option<ProviderKind> {
        match self {
            Self::OpenRouter => Some(ProviderKind::OpenRouter),
            Self::OpenAi => Some(ProviderKind::OpenAi),
            Self::Jev | Self::Clef | Self::ClefFlash | Self::Custom => None,
        }
    }

    /// INVARIANT: Build this row's provider, or say what is missing.
    ///
    /// The reason is for a log line, so it names where to fix it and never
    /// carries the key.
    pub async fn provider(
        self,
        pool: &PgPool,
        timeout: Duration,
    ) -> Result<Box<dyn JudgmentProvider>, String> {
        let build_failed = |e| format!("could not build the {} client: {}", self.label(), e);
        if self == Self::OpenAi {
            let (key, _source) =
                crate::llm::openai::openai_api_key(pool)
                    .await
                    .ok_or_else(|| {
                        format!(
                            "no OpenAI key is set. Store one as the 'openai' credential, set {}, \
                         or log the Codex CLI in with an API key",
                            crate::llm::OPENAI_API_KEY_ENV
                        )
                    })?;
            let provider = OpenAiDecisionsProvider::new(key, timeout).map_err(build_failed)?;
            return Ok(Box::new(provider));
        }
        let (url, model, api_key) = self.system_one_request(pool).await?;
        let provider =
            SystemOneProvider::new(url, model, api_key, timeout).map_err(build_failed)?;
        Ok(Box::new(provider))
    }

    pub(crate) async fn system_one_request(
        self,
        pool: &PgPool,
    ) -> Result<(String, String, Option<String>), String> {
        Ok(match self {
            Self::OpenAi => return Err("OpenAI Decisions does not speak System One".to_string()),
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
            Self::OpenRouter => {
                let key = openrouter_api_key(pool).await.ok_or_else(|| {
                    format!(
                        "no OpenRouter key is set. Store one as the 'openrouter' credential or set {}",
                        OPENROUTER_API_KEY_ENV
                    )
                })?;
                let model = prefs::SYSTEM_ONE_OPENROUTER_MODEL
                    .read(pool)
                    .await
                    .unwrap_or_else(|| OPENROUTER_JEV_MODEL.to_string());
                (openrouter_system_one_url(), model, Some(key))
            }
            Self::Clef | Self::ClefFlash => {
                let model = if self == Self::Clef {
                    "clef"
                } else {
                    "clef-flash"
                };
                let (credential, token) = stored_credential(pool, self.credential_service())
                    .await
                    .ok_or("no Cloudflare Workers AI token is stored")?;
                let base = credential
                    .base_urls
                    .first()
                    .map(String::as_str)
                    .unwrap_or("");
                let url = workers_ai_url(base, model).ok_or(concat!(
                    "the Cloudflare Workers AI credential names no account. Save it again in ",
                    providers_settings!(),
                ))?;
                (url, model.to_string(), Some(token))
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
                let key = stored_credential(pool, self.credential_service())
                    .await
                    .filter(|(c, _)| credential_scope_covers(&c.base_urls, &url))
                    .map(|(_, key)| key);
                (url, model, key)
            }
        })
    }
}

pub(crate) fn openrouter_system_one_url() -> String {
    format!("{OPENROUTER_BASE_URL}/systemone")
}

async fn openrouter_api_key(pool: &PgPool) -> Option<String> {
    let service = SystemOneEndpoint::OpenRouter.credential_service();
    let credential = match CredentialStore::get(pool, service).await {
        Ok(Some(c)) => c
            .secret_or_log(pool, "[Judgment]")
            .await
            .map(|secret| (c.auth_type, secret)),
        Ok(None) => None,
        Err(e) => {
            log_legacy!(
                "[Judgment] Could not read the '{}' credential: {}",
                service,
                e
            );
            None
        }
    };
    crate::llm::resolve_bearer_key(credential, std::env::var(OPENROUTER_API_KEY_ENV).ok())
}

pub(crate) fn workers_ai_url(scope: &str, model: &str) -> Option<String> {
    let scope = scope.trim().trim_end_matches('/');
    let account = scope
        .strip_prefix(CLOUDFLARE_ACCOUNTS_PREFIX)?
        .strip_suffix("/ai")?;
    let well_formed = !account.is_empty() && account.chars().all(|c| c.is_ascii_alphanumeric());
    well_formed.then(|| format!("{scope}/run/@cf/cloudflare/{model}"))
}

pub(crate) async fn typesafe_api_key(pool: &PgPool) -> Option<String> {
    if let Some((_, key)) = stored_credential(pool, TYPESAFE_CREDENTIAL_SERVICE).await {
        return Some(key);
    }
    std::env::var(TYPESAFE_API_KEY_ENV)
        .ok()
        .filter(|k| !k.trim().is_empty())
}

async fn stored_credential(
    pool: &PgPool,
    service: &str,
) -> Option<(crate::core::Credential, String)> {
    match CredentialStore::get(pool, service).await {
        Ok(Some(c)) => {
            let secret = c.secret_or_log(pool, "[Judgment]").await?;
            (!secret.trim().is_empty()).then_some((c, secret))
        }
        Ok(None) => None,
        Err(e) => {
            log_legacy!(
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
