use crate::core::AuthType;
#[path = "openai/codex_detect.rs"]
pub mod codex_detect;

#[derive(Clone)]
pub enum AnthropicAuth {
    ApiKey(String),
    OAuthBearer(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnthropicAuthSource {
    Credential,
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

/// INVARIANT: Resolve one auth source for the direct Anthropic provider, web search
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenAiKeySource {
    Credential,
    Env,
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

pub const OPENAI_API_KEY_ENV: &str = "OPENAI_API_KEY";

/// INVARIANT: The OpenAI key an in-process caller uses: [`resolve_openai_api_key`] over
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

    fn kind_and_value(auth: &AnthropicAuth) -> (&'static str, &str) {
        match auth {
            AnthropicAuth::ApiKey(k) => ("api_key", k.as_str()),
            AnthropicAuth::OAuthBearer(t) => ("bearer", t.as_str()),
        }
    }

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

    #[test]
    fn env_key_configures_as_api_key() {
        let (auth, source) = resolve_anthropic_auth(None, Some("sk-ant-env".to_string()))
            .expect("the env var resolves");
        assert_eq!(kind_and_value(&auth), ("api_key", "sk-ant-env"));
        assert_eq!(source, AnthropicAuthSource::Env);
    }

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

    #[test]
    fn falls_back_to_env_when_no_credential() {
        let resolved = resolve_openai_api_key(
            None,
            Some("sk-env".to_string()),
            Some("sk-codex".to_string()),
        );
        assert_eq!(resolved, Some(("sk-env".to_string(), OpenAiKeySource::Env)));
    }

    #[test]
    fn falls_back_to_codex_when_no_credential_or_env() {
        let resolved = resolve_openai_api_key(None, None, Some("sk-codex".to_string()));
        assert_eq!(
            resolved,
            Some(("sk-codex".to_string(), OpenAiKeySource::CodexCli))
        );
    }

    #[test]
    fn blank_env_falls_back_to_codex() {
        let resolved =
            resolve_openai_api_key(None, Some("   ".to_string()), Some("sk-codex".to_string()));
        assert_eq!(
            resolved,
            Some(("sk-codex".to_string(), OpenAiKeySource::CodexCli))
        );
    }

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

    #[test]
    fn blank_credential_falls_back_to_env() {
        let resolved = resolve_openai_api_key(
            Some((AuthType::ApiKey, "   ".to_string())),
            Some("sk-env".to_string()),
            None,
        );
        assert_eq!(resolved, Some(("sk-env".to_string(), OpenAiKeySource::Env)));
    }

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
