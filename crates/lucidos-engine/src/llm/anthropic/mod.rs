//! Direct Anthropic provider — hits `api.anthropic.com/v1/messages`.
//!
//! Shares the entire wire format (request building, cache-control, SSE parsing)
//! with the Vertex Claude path via [`crate::llm::anthropic_wire`]; this module
//! only owns the direct transport: the endpoint, the auth headers, and the
//! retry loop. Wired into [`crate::llm::routing::RoutingProvider`] for any model
//! the [`crate::llm::model_registry`] maps to `ProviderKind::Anthropic`.

use crate::core::AuthType;
use crate::llm::provider::{LlmProvider, LlmResponse, Message, TokenCallback, ToolDefinition};
use async_trait::async_trait;
use std::time::Duration;

/// `pub(crate)` so `llm::web_search::anthropic` can share this module's auth
/// header and API-version constants instead of restating them.
pub(crate) mod chat;

/// Beta flag that authorizes a Claude subscription OAuth bearer token to call
/// the Messages API. Required alongside `Authorization: Bearer` for OAuth auth —
/// shared by the direct provider (`chat.rs`) and the builtin `anthropic` proxy
/// (`api::proxy_builtin`), so an OAuth credential works through either path.
pub const ANTHROPIC_OAUTH_BETA: &str = "oauth-2025-04-20";

/// Authentication for the direct Anthropic API. The two kinds map to different
/// HTTP headers (see `chat.rs`):
/// - [`AnthropicAuth::ApiKey`] → `x-api-key` (pay-per-token).
/// - [`AnthropicAuth::OAuthBearer`] → `Authorization: Bearer` + the
///   `anthropic-beta: oauth-2025-04-20` header (Claude subscription).
#[derive(Clone)]
pub enum AnthropicAuth {
    ApiKey(String),
    /// v1: no auto-refresh. Subscription OAuth tokens are short-lived; when one
    /// expires the call 401s and the UI tells the user to re-paste a fresh token
    /// in Settings → Providers. Auto-refresh is a follow-up.
    OAuthBearer(String),
}

/// Where the auth the engine builds [`AnthropicProvider`] from came from,
/// surfaced in the startup log so an operator can tell whether the stored
/// credential or the env fallback is in effect. Never carries the secret: only
/// the name of the source.
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

/// Resolve the auth used to construct [`AnthropicProvider`] (and the Anthropic
/// web-search backend, and the builtin `anthropic` app proxy, all of which must
/// agree on which source won).
///
/// Precedence, highest first: a usable stored `anthropic` credential (Settings →
/// Providers) then the `ANTHROPIC_API_KEY` launch env var. The stored credential
/// is the configured path and the only one that can carry a Claude subscription
/// OAuth token; the env var is the convenience fallback for a machine that
/// already exports a key, the parallel of `OPENAI_API_KEY` for OpenAI. An
/// env-sourced key is always an [`AnthropicAuth::ApiKey`]: an exported key is a
/// pay-per-token API key, never a subscription token, and sending one as a
/// bearer would 401.
///
/// Only `api_key` / `bearer` credentials carry usable Anthropic auth. Any other
/// `auth_type` (e.g. a `password` JSON blob) is ignored with a log line and the
/// resolver falls through to the env var, rather than disabling Anthropic
/// outright. Blank/whitespace values on either source are treated as absent.
/// Returns `None` when neither is configured, in which case Anthropic models
/// surface a clear error from `RoutingProvider`.
///
/// Pure over its inputs (the `std::env::var` read happens in the caller) so the
/// precedence stays unit-testable without mutating process env.
///
/// The returned auth is a secret: log the [`AnthropicAuthSource`], never the
/// value.
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
                log!(
                    "[Startup] Anthropic credential auth_type {} unsupported (expected api_key or bearer), ignoring it and falling back to ANTHROPIC_API_KEY",
                    other
                );
            }
        }
    }
    let trimmed = env_key?.trim().to_string();
    (!trimmed.is_empty()).then_some((AnthropicAuth::ApiKey(trimmed), AnthropicAuthSource::Env))
}

pub struct AnthropicProvider {
    auth: AnthropicAuth,
    model: String,
    /// Client without a per-request timeout — streaming applies per-chunk
    /// timeouts inside `parse_claude_stream` instead.
    streaming_client: reqwest::Client,
}

impl AnthropicProvider {
    /// Build the provider; returns `Err` if the reqwest builder rejects the
    /// configuration so the engine can fail at startup with a logged reason.
    pub fn new(
        auth: AnthropicAuth,
        model: String,
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let streaming_client = reqwest::Client::builder()
            .pool_idle_timeout(Duration::from_secs(30))
            .build()?;
        Ok(Self {
            auth,
            model,
            streaming_client,
        })
    }
}

#[async_trait]
impl LlmProvider for AnthropicProvider {
    fn default_model(&self) -> &str {
        &self.model
    }

    async fn chat(
        &self,
        messages: Vec<Message>,
        tools: Vec<ToolDefinition>,
        model_override: Option<&str>,
        system_prompt: Option<&str>,
        on_token: Option<TokenCallback>,
        reasoning_effort: Option<&str>,
    ) -> Result<LlmResponse, Box<dyn std::error::Error + Send + Sync>> {
        let model = model_override.unwrap_or(&self.model);
        self.chat_anthropic(
            messages,
            tools,
            model,
            system_prompt,
            on_token,
            reasoning_effort,
        )
        .await
    }
}

#[cfg(test)]
mod tests {
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
