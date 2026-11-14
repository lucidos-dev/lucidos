//! Secret-free evidence of the backend a coding-agent process reports.
//!
//! Model ids, account directories and the engine's own provider settings are
//! deliberately not inputs. A caller must bind observations to the process
//! that produced the usage; these parsers establish no historical scope.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The backend and billing contract, independently of the coding agent/model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum BillingProvider {
    ClaudeGateway,
    Vertex,
    Bedrock,
    Foundry,
    Anthropic,
    AnthropicSubscription,
    Openai,
    ChatgptPlan,
}

impl BillingProvider {
    pub fn id(self) -> &'static str {
        match self {
            Self::ClaudeGateway => "claude-gateway",
            Self::Vertex => "vertex",
            Self::Bedrock => "bedrock",
            Self::Foundry => "foundry",
            Self::Anthropic => "anthropic",
            Self::AnthropicSubscription => "anthropic-subscription",
            Self::Openai => "openai",
            Self::ChatgptPlan => "chatgpt-plan",
        }
    }

    /// Plan-backed usage must be presented as list-price equivalent, rather
    /// than claiming that the token estimate was an additional charge.
    pub fn is_plan(self) -> bool {
        matches!(self, Self::AnthropicSubscription | Self::ChatgptPlan)
    }
}

/// The billing basis, without asserting an invoice amount or remaining quota.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum BillingMode {
    TokenMetered,
    Plan,
    #[default]
    Unknown,
}

/// The CLI response that supplied this observation. Never an auth file path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum BillingEvidenceSource {
    ClaudeInitialize,
    CodexAccount,
    Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum UnresolvedBillingReason {
    MissingEvidence,
    UnsupportedBackend,
    UnknownAuthMethod,
    UnknownEndpoint,
    ConflictingEvidence,
}

/// Sanitized config evidence; a sparse config needs the typed provider report.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodexEndpointConfig {
    Unknown,
    ExplicitDefaults,
    BuiltinCompatible,
}

/// `modelProvider/list` distinguishes builtin routing from arbitrary providers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodexProviderEndpoints {
    Unavailable,
    BuiltinDefaults,
    Other,
}

impl CodexEndpointConfig {
    pub fn read(response: &Value) -> Self {
        if BillingAttribution::codex_default_endpoints(response) {
            return Self::ExplicitDefaults;
        }
        let config = &response["config"];
        let optional_object = |v: &Value| v.is_null() || v.is_object();
        let optional_builtin = |v: &Value| {
            v.is_null()
                || v.as_str()
                    .is_some_and(|v| v.trim_end_matches('/') == "https://api.openai.com/v1")
        };
        if config.is_object()
            && config["chatgpt_base_url"]
                .as_str()
                .is_some_and(|v| v.trim_end_matches('/') == "https://chatgpt.com/backend-api")
            && optional_object(&config["model_providers"])
            && optional_object(&config["model_providers"]["openai"])
            && optional_builtin(&config["openai_base_url"])
            && optional_builtin(&config["model_providers"]["openai"]["base_url"])
        {
            Self::BuiltinCompatible
        } else {
            Self::Unknown
        }
    }

    pub fn is_default(self, providers: CodexProviderEndpoints) -> bool {
        matches!(
            (self, providers),
            (Self::ExplicitDefaults, CodexProviderEndpoints::Unavailable)
                | (
                    Self::ExplicitDefaults | Self::BuiltinCompatible,
                    CodexProviderEndpoints::BuiltinDefaults
                )
        )
    }
}

impl CodexProviderEndpoints {
    pub fn read(response: &Value) -> Self {
        let Some(data) = response["data"].as_array() else {
            return Self::Other;
        };
        let openai: Vec<_> = data.iter().filter(|row| row["id"] == "openai").collect();
        let [provider] = openai.as_slice() else {
            return Self::Other;
        };
        // The typed builtin has no auth or routing settings. Its null baseUrl
        // selects the auth-dependent builtin endpoint, not a missing report.
        if response["defaultProviderId"] == "openai"
            && provider["settings"] == serde_json::json!({"type": "openai"})
            && provider.get("baseUrl").is_some_and(|url| {
                url.is_null()
                    || url
                        .as_str()
                        .is_some_and(|url| url.trim_end_matches('/') == "https://api.openai.com/v1")
            })
        {
            Self::BuiltinDefaults
        } else {
            Self::Other
        }
    }
}

/// Only allowlisted values survive parsing: no raw response, credential,
/// account identifier, email or endpoint is serializable through this type.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "kebab-case")]
pub enum BillingAttribution {
    Known {
        provider: BillingProvider,
        source: BillingEvidenceSource,
    },
    Unresolved {
        reason: UnresolvedBillingReason,
        source: BillingEvidenceSource,
    },
}

impl Default for BillingAttribution {
    fn default() -> Self {
        Self::Unresolved {
            reason: UnresolvedBillingReason::MissingEvidence,
            source: BillingEvidenceSource::Unavailable,
        }
    }
}

impl BillingAttribution {
    /// The complete status section is built inside the serving CLI, after
    /// wrapper and managed settings. Endpoint overrides appear as base URL
    /// rows. Missing/changed schemas fail closed; no URL is retained.
    pub fn claude_default_endpoint(status: &Value) -> bool {
        let Some(sections) = status["sections"].as_array() else {
            return false;
        };
        let sessions: Vec<_> = sections
            .iter()
            .filter(|s| s["title"] == "Session")
            .collect();
        let [session] = sessions.as_slice() else {
            return false;
        };
        let Some(rows) = session["rows"].as_array() else {
            return false;
        };
        rows.iter().all(|row| {
            row.is_object()
                && row["value"].is_string()
                && row.get("label").is_none_or(|label| {
                    label
                        .as_str()
                        .is_some_and(|label| !label.to_ascii_lowercase().contains("base url"))
                })
        }) && ["Version", "Session ID", "cwd"].iter().all(|label| {
            rows.iter().any(|row| {
                row["label"] == *label && row["value"].as_str().is_some_and(|v| !v.is_empty())
            })
        })
    }

    pub fn claude_endpoint_checked(self, status_is_default: bool) -> Self {
        if !status_is_default
            && matches!(self, Self::Known { provider, .. } if provider != BillingProvider::ClaudeGateway)
        {
            Self::unresolved(
                UnresolvedBillingReason::UnknownEndpoint,
                BillingEvidenceSource::ClaudeInitialize,
            )
        } else {
            self
        }
    }

    pub fn billing_mode(&self) -> BillingMode {
        match self {
            Self::Known { provider, .. } if provider.is_plan() => BillingMode::Plan,
            Self::Known {
                provider: BillingProvider::ClaudeGateway,
                ..
            }
            | Self::Unresolved { .. } => BillingMode::Unknown,
            Self::Known { .. } => BillingMode::TokenMetered,
        }
    }
    /// Config and account are read from the same running app-server. A custom
    /// endpoint must not inherit the builtin provider's name or billing mode.
    pub fn codex_endpoint_checked(self, config_is_default: bool) -> Self {
        if !config_is_default && matches!(self, Self::Known { .. }) {
            Self::unresolved(
                UnresolvedBillingReason::UnknownEndpoint,
                BillingEvidenceSource::CodexAccount,
            )
        } else {
            self
        }
    }

    // Older servers can omit inherited endpoint overrides from config/read.
    // Absence is not evidence of default routing.
    pub fn codex_default_endpoints(response: &Value) -> bool {
        let config = &response["config"];
        if !config.is_object() {
            return false;
        }
        let builtin = |value: &Value, builtin: &str| match value {
            Value::String(value) => value.trim_end_matches('/') == builtin,
            _ => false,
        };
        builtin(&config["openai_base_url"], "https://api.openai.com/v1")
            && builtin(
                &config["model_providers"]["openai"]["base_url"],
                "https://api.openai.com/v1",
            )
            && builtin(
                &config["chatgpt_base_url"],
                "https://chatgpt.com/backend-api",
            )
    }
    pub fn provider_id(&self) -> &'static str {
        match self {
            Self::Known { provider, .. } => provider.id(),
            Self::Unresolved { .. } => "unknown",
        }
    }

    fn known(provider: BillingProvider, source: BillingEvidenceSource) -> Self {
        Self::Known { provider, source }
    }

    fn unresolved(reason: UnresolvedBillingReason, source: BillingEvidenceSource) -> Self {
        Self::Unresolved { reason, source }
    }

    /// The account in the *running process's* initialize control reply.
    /// Third-party transport wins over account/key fields: managed policy can
    /// override a key or subscription present in the same environment.
    pub fn claude_initialize(body: &Value) -> Self {
        use BillingEvidenceSource::ClaudeInitialize as Source;
        use BillingProvider as P;
        let account = &body["account"];
        let provider = match account["apiProvider"].as_str() {
            Some("gateway") => P::ClaudeGateway,
            Some("vertex") => P::Vertex,
            Some("bedrock") => P::Bedrock,
            Some("foundry") => P::Foundry,
            Some("firstParty") => {
                match account["apiKeySource"].as_str() {
                    Some("ANTHROPIC_API_KEY" | "/login managed key") => P::Anthropic,
                    None | Some("none" | "oauth")
                        if matches!(
                            account["tokenSource"].as_str(),
                            Some("claude-login" | "CLAUDE_CODE_OAUTH_TOKEN")
                        ) =>
                    {
                        P::AnthropicSubscription
                    }
                    // `none` alone also describes an unknown bearer token;
                    // apiKeyHelper may front an arbitrary endpoint.
                    _ => {
                        return Self::unresolved(UnresolvedBillingReason::UnknownAuthMethod, Source)
                    }
                }
            }
            None => return Self::unresolved(UnresolvedBillingReason::MissingEvidence, Source),
            _ => return Self::unresolved(UnresolvedBillingReason::UnsupportedBackend, Source),
        };
        Self::known(provider, Source)
    }

    /// Combine the effective thread provider with account/read from the same
    /// app-server. `openai` in a transcript alone does not distinguish a key
    /// from a ChatGPT plan. Unknown/custom providers are never called OpenAI.
    pub fn codex_account(model_provider: Option<&str>, body: &Value) -> Self {
        use BillingEvidenceSource::CodexAccount as Source;
        use UnresolvedBillingReason as R;
        match model_provider {
            None => Self::unresolved(R::MissingEvidence, Source),
            Some("openai") => match body["account"]["type"].as_str() {
                Some("apiKey") if body["requiresOpenaiAuth"].as_bool() == Some(true) => {
                    Self::known(BillingProvider::Openai, Source)
                }
                Some("chatgpt") if body["requiresOpenaiAuth"].as_bool() == Some(true) => {
                    Self::known(BillingProvider::ChatgptPlan, Source)
                }
                _ => Self::unresolved(R::UnknownAuthMethod, Source),
            },
            Some(_) => Self::unresolved(R::UnsupportedBackend, Source),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn claude_endpoint_evidence_requires_a_complete_effective_status() {
        let session = |extra: Value| {
            json!({"sections":[{"title":"Session", "rows":[
                {"label":"Version", "value":"2.1.311"},
                {"label":"Session ID", "value":"fixture-session"},
                {"label":"cwd", "value":"/tmp/fixture"}, extra
            ]}]})
        };
        assert!(BillingAttribution::claude_default_endpoint(&session(
            json!({"label":"Auth token", "value":"fixture"})
        )));
        for status in [
            json!({}),
            json!({"sections":[]}),
            session(json!({"label":"Anthropic base URL","value":"https://example.com/private"})),
            session(json!({"label":"Vertex base URL","value":"https://example.com/private"})),
            session(json!(null)),
        ] {
            assert!(!BillingAttribution::claude_default_endpoint(&status));
        }
    }

    #[test]
    fn managed_transport_wins_over_a_subscription_or_key() {
        for (transport, expected) in [
            ("gateway", "claude-gateway"),
            ("vertex", "vertex"),
            ("bedrock", "bedrock"),
            ("foundry", "foundry"),
        ] {
            let evidence = BillingAttribution::claude_initialize(&json!({"account": {
                "apiProvider": transport, "apiKeySource": "ANTHROPIC_API_KEY",
                "subscriptionType": "max", "tokenSource": "claude-login"
            }}));
            assert_eq!(evidence.provider_id(), expected);
        }
    }

    #[test]
    fn first_party_auth_needs_positive_evidence() {
        for (key_source, subscription, expected) in [
            ("ANTHROPIC_API_KEY", "max", "anthropic"),
            ("/login managed key", "", "anthropic"),
            ("none", "max", "unknown"),
            ("oauth", "pro", "unknown"),
            ("none", "Claude API", "unknown"),
            ("apiKeyHelper", "max", "unknown"),
            ("", "", "unknown"),
        ] {
            let evidence = BillingAttribution::claude_initialize(&json!({"account": {
                "apiProvider": "firstParty", "apiKeySource": key_source,
                "subscriptionType": subscription
            }}));
            assert_eq!(evidence.provider_id(), expected);
        }
        for token_source in ["claude-login", "CLAUDE_CODE_OAUTH_TOKEN"] {
            let evidence = BillingAttribution::claude_initialize(&json!({"account": {
                "apiProvider": "firstParty", "apiKeySource": "none",
                "tokenSource": token_source, "subscriptionType": "Claude API"
            }}));
            assert_eq!(evidence.provider_id(), "anthropic-subscription");
            assert_eq!(evidence.billing_mode(), BillingMode::Plan);
        }
    }

    #[test]
    fn codex_openai_transport_alone_does_not_prove_billing() {
        for (auth, expected) in [
            ("apiKey", "openai"),
            ("chatgpt", "chatgpt-plan"),
            ("future-auth", "unknown"),
        ] {
            let body = json!({"requiresOpenaiAuth": true, "account": {"type": auth}});
            assert_eq!(
                BillingAttribution::codex_account(Some("openai"), &body).provider_id(),
                expected
            );
            assert_eq!(
                BillingAttribution::codex_account(Some("custom"), &body).provider_id(),
                "unknown"
            );
        }
        assert_eq!(
            BillingAttribution::codex_account(Some("openai"), &json!({})).provider_id(),
            "unknown"
        );
    }

    #[test]
    fn serialization_is_allowlisted_and_roundtrips() {
        let evidence = BillingAttribution::claude_initialize(&json!({"account": {
            "apiProvider": "gateway", "email": "person@example.com",
            "accessToken": "fixture-secret", "endpoint": "https://example.com/private"
        }}));
        let wire = serde_json::to_string(&evidence).unwrap();
        for forbidden in ["person@", "fixture-secret", "https:"] {
            assert!(!wire.contains(forbidden));
        }
        assert_eq!(
            serde_json::from_str::<BillingAttribution>(&wire).unwrap(),
            evidence
        );
        assert!(BillingProvider::ChatgptPlan.is_plan());
        assert!(BillingProvider::AnthropicSubscription.is_plan());
        assert!(!BillingProvider::Openai.is_plan());
        assert_eq!(BillingAttribution::default().provider_id(), "unknown");
    }

    #[test]
    fn codex_typed_builtin_report_resolves_sparse_native_config() {
        let config = CodexEndpointConfig::read(&json!({"config": {
            "chatgpt_base_url": "https://chatgpt.com/backend-api"
        }}));
        let providers = json!({"defaultProviderId":"openai", "data":[{
            "id":"openai", "baseUrl":null, "settings":{"type":"openai"}
        }]});
        assert!(config.is_default(CodexProviderEndpoints::read(&providers)));
        assert!(!config.is_default(CodexProviderEndpoints::Unavailable));
        for auth in ["apiKey", "chatgpt"] {
            let attribution = BillingAttribution::codex_account(
                Some("openai"),
                &json!({"requiresOpenaiAuth":true,"account":{"type":auth}}),
            )
            .codex_endpoint_checked(config.is_default(CodexProviderEndpoints::read(&providers)));
            assert_eq!(
                attribution.provider_id(),
                if auth == "apiKey" {
                    "openai"
                } else {
                    "chatgpt-plan"
                }
            );
        }
        for invalid in [
            json!({}),
            json!({"defaultProviderId":"custom","data":providers["data"]}),
            json!({"defaultProviderId":"openai","data":[]}),
            json!({"defaultProviderId":"openai","data":[providers["data"][0],providers["data"][0]]}),
            json!({"defaultProviderId":"openai","data":[{"id":"openai","settings":{"type":"openai"}}]}),
            json!({"defaultProviderId":"openai","data":[{"id":"openai","baseUrl":null,"settings":{"type":"customGateway"}}]}),
            json!({"defaultProviderId":"openai","data":[{"id":"openai","baseUrl":"https://example.com/v1","settings":{"type":"openai"}}]}),
        ] {
            assert!(!config.is_default(CodexProviderEndpoints::read(&invalid)));
        }
        for invalid in [
            json!({}),
            json!({"config":{}}),
            json!({"config":{"chatgpt_base_url":"https://example.com"}}),
            json!({"config":{"chatgpt_base_url":"https://chatgpt.com/backend-api","openai_base_url":"https://example.com/v1"}}),
            json!({"config":{"chatgpt_base_url":"https://chatgpt.com/backend-api","model_providers":[]}}),
            json!({"config":{"chatgpt_base_url":"https://chatgpt.com/backend-api","model_providers":{"openai":{"base_url":"https://example.com/v1"}}}}),
        ] {
            assert!(!CodexEndpointConfig::read(&invalid)
                .is_default(CodexProviderEndpoints::read(&providers)));
        }
    }

    #[test]
    fn codex_custom_endpoints_do_not_inherit_builtin_billing() {
        let account = json!({"requiresOpenaiAuth": true, "account": {"type": "apiKey"}});
        for config in [
            json!({}),
            json!({"config": {}}),
            json!({"config": {"openai_base_url": null}}),
            json!({"config": {"openai_base_url": "https://example.com/v1"}}),
            json!({"config": {"model_providers": {"openai": {"base_url": "https://example.com/v1"}}}}),
        ] {
            let evidence = BillingAttribution::codex_account(Some("openai"), &account)
                .codex_endpoint_checked(BillingAttribution::codex_default_endpoints(&config));
            assert_eq!(evidence.provider_id(), "unknown");
        }
        let explicit = json!({"config": {
            "openai_base_url": "https://api.openai.com/v1",
            "model_providers": {"openai": {"base_url": "https://api.openai.com/v1"}},
            "chatgpt_base_url": "https://chatgpt.com/backend-api"
        }});
        assert!(BillingAttribution::codex_default_endpoints(&explicit));
        assert!(
            CodexEndpointConfig::read(&explicit).is_default(CodexProviderEndpoints::Unavailable)
        );
        assert!(!CodexEndpointConfig::read(&explicit).is_default(CodexProviderEndpoints::Other));
    }
}
