//! Credential precedence shared by provider clients and their callers.

use crate::core::AuthType;
// Keep the existing source path while the OpenAI client still lives here.
#[path = "openai/codex_detect.rs"]
pub mod codex_detect;

/// Authentication for the direct Anthropic API. The two kinds map to different
/// HTTP headers (see `chat.rs`):
/// - [`AnthropicAuth::ApiKey`] → `x-api-key` (pay-per-token).
/// - [`AnthropicAuth::OAuthBearer`] → `Authorization: Bearer` with the
///   subscription beta header ([`super::anthropic::ANTHROPIC_OAUTH_BETA`]).
#[derive(Clone)]
pub enum AnthropicAuth {
    ApiKey(String),
    /// Subscription OAuth tokens expire without auto-refresh. A 401 prompts
    /// the user to paste a fresh token in Settings → Providers.
    OAuthBearer(String),
}

/// The startup log reports whether direct Anthropic auth came from stored
/// credentials or the env fallback. This type carries only the source.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnthropicAuthSource {
    /// A credential stored in Settings → Providers (service name `anthropic`).
    Credential,
    /// The `ANTHROPIC_API_KEY` launch environment variable (the fallback).
    Env,
}

impl std::fmt::Display for AnthropicAuthSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Credential => "stored credential (Settings → Providers)",
            Self::Env => "ANTHROPIC_API_KEY",
        })
    }
}

/// Resolve one auth source for the direct Anthropic provider, web search
/// and builtin app proxy.
///
/// A usable stored `anthropic` credential wins over `ANTHROPIC_API_KEY`.
/// The stored credential can carry an API key or Claude subscription token.
/// The env fallback always supplies an [`AnthropicAuth::ApiKey`].
///
/// Only `api_key` and `bearer` credentials are accepted. A different
/// `auth_type` logs and falls through to the env var. Blank values are
/// absent. With neither source configured, the router reports an error.
///
/// Callers read the environment and supply the values. Log the
/// [`AnthropicAuthSource`] only.
pub fn resolve_anthropic_auth(
    credential: Option<(AuthType, String)>,
    env_key: Option<String>,
) -> Option<(AnthropicAuth, AnthropicAuthSource)> {
    if let Some((auth_type, value)) = credential {
        match auth_type {
            AuthType::ApiKey | AuthType::Bearer => {
                let trimmed = value.trim();
                if !trimmed.is_empty() {
                    let auth = if matches!(auth_type, AuthType::Bearer) {
                        AnthropicAuth::OAuthBearer(trimmed.to_string())
                    } else {
                        AnthropicAuth::ApiKey(trimmed.to_string())
                    };
                    return Some((auth, AnthropicAuthSource::Credential));
                }
            }
            other => {
                log_legacy!(
                    "[Startup] Anthropic credential auth_type {} unsupported (expected api_key or bearer), ignoring it and falling back to ANTHROPIC_API_KEY",
                    other
                );
            }
        }
    }
    let trimmed = env_key?.trim().to_string();
    (!trimmed.is_empty()).then_some((AnthropicAuth::ApiKey(trimmed), AnthropicAuthSource::Env))
}

/// The startup log reports which source supplied the direct OpenAI key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenAiKeySource {
    /// A credential stored in Settings → Providers (service name `openai`).
    Credential,
    /// The `OPENAI_API_KEY` launch environment variable (the fallback).
    Env,
    /// Auto-detected from the Codex CLI's `auth.json` (`apikey` login) — the
    /// lowest-precedence source. See [`codex_detect`].
    CodexCli,
}

impl std::fmt::Display for OpenAiKeySource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Credential => "stored credential (Settings → Providers)",
            Self::Env => OPENAI_API_KEY_ENV,
            Self::CodexCli => "Codex CLI (~/.codex/auth.json)",
        })
    }
}

/// Resolve the OpenAI API key used to construct `OpenAiProvider`.
///
/// Precedence starts with a usable stored `openai` credential.
/// `OPENAI_API_KEY` is next, then the Codex CLI's `auth.json` key
/// (`codex_key`, see [`codex_detect`]).
///
/// The env fallback preserves existing deployments. Codex users signed in
/// with an API key can use OpenAI models without extra configuration.
///
/// Only `api_key` and `bearer` credentials are accepted. Other auth types
/// log and fall through. Blank values are absent. With no key, the router
/// reports an error. The caller reads the environment and the Codex file.
pub fn resolve_openai_api_key(
    credential: Option<(AuthType, String)>,
    env_key: Option<String>,
    codex_key: Option<String>,
) -> Option<(String, OpenAiKeySource)> {
    if let Some((auth_type, value)) = credential {
        match auth_type {
            AuthType::ApiKey | AuthType::Bearer => {
                let trimmed = value.trim();
                if !trimmed.is_empty() {
                    return Some((trimmed.to_string(), OpenAiKeySource::Credential));
                }
            }
            other => {
                log_legacy!(
                    "[Startup] OpenAI credential auth_type {} unsupported (expected api_key or bearer) — ignoring it and falling back to OPENAI_API_KEY / Codex CLI",
                    other
                );
            }
        }
    }
    if let Some(value) = env_key {
        let trimmed = value.trim();
        if !trimmed.is_empty() {
            return Some((trimmed.to_string(), OpenAiKeySource::Env));
        }
    }
    if let Some(value) = codex_key {
        let trimmed = value.trim();
        if !trimmed.is_empty() {
            return Some((trimmed.to_string(), OpenAiKeySource::CodexCli));
        }
    }
    None
}

/// The launch environment's OpenAI key, read below a stored `openai` credential.
pub const OPENAI_API_KEY_ENV: &str = "OPENAI_API_KEY";

/// The OpenAI key an in-process caller uses: [`resolve_openai_api_key`] over
/// the stored `openai` credential, [`OPENAI_API_KEY_ENV`] and the Codex login.
///
/// A credential that cannot be read logs and falls through to the other two,
/// as the chat provider's build does. `None` means no source holds a key.
pub async fn openai_api_key(pool: &sqlx::PgPool) -> Option<(String, OpenAiKeySource)> {
    let service = crate::llm::ProviderKind::OpenAi.as_str();
    let credential = match crate::core::CredentialStore::get(pool, service).await {
        Ok(Some(c)) => c
            .secret_or_log(pool, "[OpenAI]")
            .await
            .map(|secret| (c.auth_type, secret)),
        Ok(None) => None,
        Err(e) => {
            log_legacy!(
                "[OpenAI] Could not read the '{}' credential: {}",
                service,
                e
            );
            None
        }
    };
    resolve_openai_api_key(
        credential,
        std::env::var(OPENAI_API_KEY_ENV).ok(),
        codex_detect::load(),
    )
}

/// Resolve a bearer-style key for an OpenAI-compatible backend.
/// A usable `api_key` or `bearer` credential wins over the env fallback.
/// Ignore other auth types and blank values. Return the trimmed key, or
/// `None` when neither source is configured.
pub fn resolve_bearer_key(
    credential: Option<(AuthType, String)>,
    env_key: Option<String>,
) -> Option<String> {
    if let Some((auth_type, value)) = credential {
        if matches!(auth_type, AuthType::ApiKey | AuthType::Bearer) {
            let trimmed = value.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_string());
            }
        }
    }
    env_key.and_then(|v| {
        let trimmed = v.trim();
        (!trimmed.is_empty()).then(|| trimmed.to_string())
    })
}

#[cfg(test)]
mod anthropic_tests {
    use super::*;

    /// `(kind, value)` for a resolved auth. Lets the tests assert on the variant
    /// without deriving `PartialEq`/`Debug` on a secret-carrying enum.
    fn kind_and_value(auth: &AnthropicAuth) -> (&'static str, &str) {
        match auth {
            AnthropicAuth::ApiKey(k) => ("api_key", k.as_str()),
            AnthropicAuth::OAuthBearer(t) => ("bearer", t.as_str()),
        }
    }

    /// A usable stored credential wins over the env var, and reports itself as
    /// the credential source.
    #[test]
    fn credential_beats_env() {
        let (auth, source) = resolve_anthropic_auth(
            Some((AuthType::ApiKey, "sk-ant-stored".to_string())),
            Some("sk-ant-env".to_string()),
        )
        .expect("a stored credential resolves");
        assert_eq!(kind_and_value(&auth), ("api_key", "sk-ant-stored"));
        assert_eq!(source, AnthropicAuthSource::Credential);
    }

    /// A `bearer` credential is a Claude subscription OAuth token, so it maps to
    /// `OAuthBearer` (which sends `Authorization: Bearer` plus the OAuth beta
    /// header), not to `x-api-key`.
    #[test]
    fn bearer_credential_resolves_as_oauth() {
        let (auth, source) = resolve_anthropic_auth(
            Some((AuthType::Bearer, "oauth-token-xyz".to_string())),
            None,
        )
        .expect("a bearer credential resolves");
        assert_eq!(kind_and_value(&auth), ("bearer", "oauth-token-xyz"));
        assert_eq!(source, AnthropicAuthSource::Credential);
    }

    /// The gap this fallback closes: no stored credential, but a key exported in
    /// the environment configures Anthropic. It must be an API key, never a
    /// bearer token.
    #[test]
    fn env_key_configures_as_api_key() {
        let (auth, source) = resolve_anthropic_auth(None, Some("sk-ant-env".to_string()))
            .expect("the env var resolves");
        assert_eq!(kind_and_value(&auth), ("api_key", "sk-ant-env"));
        assert_eq!(source, AnthropicAuthSource::Env);
    }

    /// Whitespace is trimmed off both sources.
    #[test]
    fn values_are_trimmed() {
        let (auth, _) = resolve_anthropic_auth(None, Some("  sk-ant-env  ".to_string()))
            .expect("the env var resolves");
        assert_eq!(kind_and_value(&auth), ("api_key", "sk-ant-env"));

        let (auth, _) = resolve_anthropic_auth(
            Some((AuthType::ApiKey, "  sk-ant-stored\n".to_string())),
            None,
        )
        .expect("the credential resolves");
        assert_eq!(kind_and_value(&auth), ("api_key", "sk-ant-stored"));
    }

    /// A blank credential value is treated as absent, so it can't shadow a good
    /// env key.
    #[test]
    fn blank_credential_falls_through_to_env() {
        let (auth, source) = resolve_anthropic_auth(
            Some((AuthType::ApiKey, "   ".to_string())),
            Some("sk-ant-env".to_string()),
        )
        .expect("the env var resolves past a blank credential");
        assert_eq!(kind_and_value(&auth), ("api_key", "sk-ant-env"));
        assert_eq!(source, AnthropicAuthSource::Env);
    }

    /// An unsupported `auth_type` (a `password` JSON blob, say) is logged and
    /// skipped rather than disabling Anthropic: the env var still applies.
    #[test]
    fn unsupported_auth_type_falls_through_to_env() {
        let (auth, source) = resolve_anthropic_auth(
            Some((AuthType::Password, "{\"user\":\"x\"}".to_string())),
            Some("sk-ant-env".to_string()),
        )
        .expect("the env var resolves past an unusable credential");
        assert_eq!(kind_and_value(&auth), ("api_key", "sk-ant-env"));
        assert_eq!(source, AnthropicAuthSource::Env);
    }

    /// Neither source configured, or only blank ones, resolves to `None` so the
    /// "no provider configured" path is unchanged.
    #[test]
    fn neither_source_is_none() {
        assert!(resolve_anthropic_auth(None, None).is_none());
        assert!(resolve_anthropic_auth(None, Some("   ".to_string())).is_none());
        assert!(resolve_anthropic_auth(
            Some((AuthType::ApiKey, String::new())),
            Some(String::new())
        )
        .is_none());
        assert!(
            resolve_anthropic_auth(Some((AuthType::Password, "blob".to_string())), None).is_none()
        );
    }

    /// The source `Display` names the env var, so the startup log can say where
    /// the auth came from without ever printing the secret.
    #[test]
    fn source_display_names_the_env_var_not_the_secret() {
        assert_eq!(AnthropicAuthSource::Env.to_string(), "ANTHROPIC_API_KEY");
        assert!(AnthropicAuthSource::Credential
            .to_string()
            .contains("stored credential"));
    }
}

#[cfg(test)]
mod openai_tests {
    use super::*;

    /// A stored OpenAI credential wins over both fallback keys.
    #[test]
    fn stored_credential_takes_precedence_over_env() {
        let resolved = resolve_openai_api_key(
            Some((AuthType::ApiKey, "sk-stored".to_string())),
            Some("sk-env".to_string()),
            Some("sk-codex".to_string()),
        );
        assert_eq!(
            resolved,
            Some(("sk-stored".to_string(), OpenAiKeySource::Credential))
        );
    }

    /// With no credential the OPENAI_API_KEY env fallback is preserved, and it
    /// wins over the Codex key (env is an explicit launch choice).
    #[test]
    fn falls_back_to_env_when_no_credential() {
        let resolved = resolve_openai_api_key(
            None,
            Some("sk-env".to_string()),
            Some("sk-codex".to_string()),
        );
        assert_eq!(resolved, Some(("sk-env".to_string(), OpenAiKeySource::Env)));
    }

    /// The Codex key is used only when neither a credential nor the env var is
    /// present — the lowest-precedence auto-detected source.
    #[test]
    fn falls_back_to_codex_when_no_credential_or_env() {
        let resolved = resolve_openai_api_key(None, None, Some("sk-codex".to_string()));
        assert_eq!(
            resolved,
            Some(("sk-codex".to_string(), OpenAiKeySource::CodexCli))
        );
    }

    /// A blank env key does not shadow a usable Codex key.
    #[test]
    fn blank_env_falls_back_to_codex() {
        let resolved =
            resolve_openai_api_key(None, Some("   ".to_string()), Some("sk-codex".to_string()));
        assert_eq!(
            resolved,
            Some(("sk-codex".to_string(), OpenAiKeySource::CodexCli))
        );
    }

    /// A `bearer` credential carries a usable OpenAI key just like `api_key`.
    #[test]
    fn bearer_credential_is_accepted() {
        let resolved = resolve_openai_api_key(
            Some((AuthType::Bearer, "sk-bearer".to_string())),
            None,
            None,
        );
        assert_eq!(
            resolved,
            Some(("sk-bearer".to_string(), OpenAiKeySource::Credential))
        );
    }

    /// An unsupported auth type falls through to the env var.
    #[test]
    fn unsupported_auth_type_falls_back_to_env() {
        let resolved = resolve_openai_api_key(
            Some((
                AuthType::Password,
                r#"{"username":"a","password":"b"}"#.to_string(),
            )),
            Some("sk-env".to_string()),
            None,
        );
        assert_eq!(resolved, Some(("sk-env".to_string(), OpenAiKeySource::Env)));
    }

    /// A blank/whitespace credential value is treated as absent so a stray empty
    /// row doesn't shadow a real env key.
    #[test]
    fn blank_credential_falls_back_to_env() {
        let resolved = resolve_openai_api_key(
            Some((AuthType::ApiKey, "   ".to_string())),
            Some("sk-env".to_string()),
            None,
        );
        assert_eq!(resolved, Some(("sk-env".to_string(), OpenAiKeySource::Env)));
    }

    /// None configured (or all blank) → None, so OpenAI models surface a clear
    /// "not configured" error instead of constructing a dead provider.
    #[test]
    fn none_configured_returns_none() {
        assert_eq!(resolve_openai_api_key(None, None, None), None);
        assert_eq!(
            resolve_openai_api_key(
                Some((AuthType::ApiKey, String::new())),
                Some("  ".to_string()),
                Some("   ".to_string())
            ),
            None
        );
    }

    /// `resolve_bearer_key` (used for OpenRouter) prefers a usable credential,
    /// falls back to the env var, and treats blanks/unsupported auth as absent.
    #[test]
    fn resolve_bearer_key_prefers_credential_then_env() {
        assert_eq!(
            resolve_bearer_key(
                Some((AuthType::ApiKey, "sk-or-cred".to_string())),
                Some("sk-or-env".to_string())
            ),
            Some("sk-or-cred".to_string())
        );
        assert_eq!(
            resolve_bearer_key(None, Some("sk-or-env".to_string())),
            Some("sk-or-env".to_string())
        );
        assert_eq!(
            resolve_bearer_key(Some((AuthType::Bearer, "  ".to_string())), None),
            None
        );
        // A non-key auth_type is ignored; falls through to the env var.
        assert_eq!(
            resolve_bearer_key(
                Some((AuthType::Password, "{}".to_string())),
                Some("sk-or-env".to_string())
            ),
            Some("sk-or-env".to_string())
        );
        assert_eq!(resolve_bearer_key(None, None), None);
    }
}
