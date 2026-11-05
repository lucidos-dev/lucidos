//! The Claude gateway: the Anthropic Messages API behind an OAuth device-code
//! sign-in, run by an organisation for its members. Lucidos signs in as its
//! own public client. It reaches the gateway as `ProviderKind::ClaudeGateway`,
//! apart from direct Anthropic, so each credential stays on its own host.
//! Design: `docs/plans/2026-10-17-claude-gateway-provider.md`.
//!
//! The sign-in is the OAuth device-code flow (RFC 8628) as a public client:
//! [`discovery`] finds the endpoints, [`device_flow`] gets the tokens,
//! [`account`] stores them and [`tokens`] presents them, refreshed.
//! [`revocation`] revokes them at sign-out. [`telemetry`] reports usage
//! where the gateway's managed settings ask.
//! [`served_models`] reads which models the gateway serves this account.
//! [`machine_managed_settings`] reads the gateway Claude Code is told to use
//! on this machine. [`gateway_managed_settings`] fetches the policy the gateway
//! itself sets for the signed-in user.

pub mod account;
pub mod device_flow;
pub mod discovery;
pub mod gateway_managed_settings;
pub mod machine_managed_settings;
#[cfg(test)]
pub(crate) mod mock_gateway;
pub(crate) mod revocation;
pub(crate) mod served_models;
pub mod telemetry;
mod token_endpoint;
pub mod tokens;

pub use token_endpoint::GrantedTokens;

use std::sync::Arc;

use crate::llm::ANTHROPIC_API_BASE_URL;

/// Where the user signs in to the Claude gateway, as every error names it.
pub(crate) const SIGN_IN_LOCATION: &str = "Settings → Models → Providers";

/// The error for a gateway `401`, which names the way to recover.
pub(crate) fn unauthorized(error_body: &str) -> String {
    format!(
        "Claude gateway 401: sign in to the Claude gateway again in {SIGN_IN_LOCATION}. A gateway \
         reachable only on a private network also answers this when that network has \
         dropped: {error_body}"
    )
}

/// What a Claude gateway request needs from the stored sign-in: its bearer,
/// and the gateway's managed settings for the user.
///
/// The bearer is valid when returned: an expired token is refreshed first. An
/// error reaches the user as the turn's failure, so it says how to recover.
#[async_trait::async_trait]
pub trait GatewayAccess: Send + Sync {
    async fn bearer(&self) -> Result<String, Box<dyn std::error::Error + Send + Sync>>;

    /// A new bearer after the gateway answered `401` to `rejected`, which
    /// happens when it ends a token before the stated expiry, or states none.
    async fn renew_rejected(
        &self,
        rejected: &str,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>>;

    /// The gateway's managed settings for the signed-in user, or `None` when
    /// it set no policy for them.
    async fn managed_settings(
        &self,
    ) -> Result<
        Option<gateway_managed_settings::GatewayManagedSettings>,
        Box<dyn std::error::Error + Send + Sync>,
    >;
}

/// Makes the gateway access for a configured gateway origin.
///
/// The engine supplies it once its event bus exists, because a gateway access
/// announces a sign-in the gateway rejected. The boot build has none yet.
pub type GatewayAccessFactory = Arc<dyn Fn(GatewayOrigin) -> Arc<dyn GatewayAccess> + Send + Sync>;

/// A validated Claude gateway origin, such as `https://gateway.example`.
///
/// The only way to name a gateway host. It uses `https`, or plain `http` on a
/// loopback host for tests. It carries no path, query or credentials. It is
/// never the direct Anthropic host, so a gateway bearer cannot reach it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GatewayOrigin(String);

impl GatewayOrigin {
    /// Parse and validate `url`, or say why it cannot be a gateway origin.
    pub fn parse(url: &str) -> Result<Self, String> {
        let url = url.trim();
        let refuse = |why: &str| Err(format!("Claude gateway URL '{url}' {why}"));
        let Ok(parsed) = reqwest::Url::parse(url) else {
            return refuse("is not a URL");
        };
        let Some(host) = parsed.host_str() else {
            return refuse("has no host");
        };
        let host = host.to_ascii_lowercase();
        match parsed.scheme() {
            "https" => {}
            "http" if is_loopback(&host) => {}
            _ => return refuse("must use https"),
        }
        if !parsed.username().is_empty() || parsed.password().is_some() {
            return refuse("must not carry credentials");
        }
        if !matches!(parsed.path(), "" | "/") || parsed.query().is_some() {
            return refuse("must be an origin, with no path or query");
        }
        if parsed.fragment().is_some() {
            return refuse("must not carry a fragment");
        }
        if Some(host.as_str()) == anthropic_api_host().as_deref() {
            return refuse("is the direct Anthropic API, not a gateway");
        }
        let port = parsed.port().map(|p| format!(":{p}")).unwrap_or_default();
        Ok(Self(format!("{}://{host}{port}", parsed.scheme())))
    }

    /// The origin with no trailing slash, as the gateway's discovery document
    /// names its `issuer`.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The Messages API root on this gateway (`{base}/messages`).
    pub fn api_base_url(&self) -> String {
        format!("{}/v1", self.0)
    }

    /// Whether `url` lies on this origin, so a token sent there stays on the
    /// gateway.
    pub fn owns(&self, url: &str) -> bool {
        let Ok(parsed) = reqwest::Url::parse(url) else {
            return false;
        };
        let Some(host) = parsed.host_str() else {
            return false;
        };
        let port = parsed.port().map(|p| format!(":{p}")).unwrap_or_default();
        let origin = format!("{}://{}{port}", parsed.scheme(), host.to_ascii_lowercase());
        origin == self.0
    }
}

impl std::fmt::Display for GatewayOrigin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

pub(crate) fn is_loopback(host: &str) -> bool {
    let bare = host.trim_start_matches('[').trim_end_matches(']');
    bare == "localhost"
        || bare
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
}

fn anthropic_api_host() -> Option<String> {
    reqwest::Url::parse(ANTHROPIC_API_BASE_URL)
        .ok()?
        .host_str()
        .map(str::to_ascii_lowercase)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_https_origin_parses_and_normalises() {
        let origin = GatewayOrigin::parse(" https://Gateway.Example/ ").unwrap();
        assert_eq!(origin.as_str(), "https://gateway.example");
        assert_eq!(origin.api_base_url(), "https://gateway.example/v1");
        let with_port = GatewayOrigin::parse("https://gateway.example:8443").unwrap();
        assert_eq!(with_port.as_str(), "https://gateway.example:8443");
    }

    /// A bearer credential never travels over plain http, except to a loopback
    /// host, where the tests run their mock gateway.
    #[test]
    fn plain_http_is_refused_except_on_loopback() {
        assert!(GatewayOrigin::parse("http://gateway.example").is_err());
        assert!(GatewayOrigin::parse("http://10.0.0.5").is_err());
        assert!(GatewayOrigin::parse("http://127.0.0.1:4100").is_ok());
        assert!(GatewayOrigin::parse("http://localhost:4100").is_ok());
        assert!(GatewayOrigin::parse("http://[::1]:4100").is_ok());
        assert!(GatewayOrigin::parse("ftp://gateway.example").is_err());
    }

    /// The direct Anthropic host is never a gateway, so the gateway bearer
    /// cannot be sent there.
    #[test]
    fn the_direct_anthropic_host_is_refused() {
        let err = GatewayOrigin::parse("https://api.anthropic.com").unwrap_err();
        assert!(err.contains("direct Anthropic API"), "{err}");
        assert!(GatewayOrigin::parse("https://API.anthropic.com/").is_err());
    }

    #[test]
    fn owns_only_urls_on_the_same_origin() {
        let origin = GatewayOrigin::parse("https://gateway.example").unwrap();
        assert!(origin.owns("https://gateway.example/oauth/token"));
        assert!(origin.owns("https://GATEWAY.example:443/oauth/token"));
        assert!(!origin.owns("http://gateway.example/oauth/token"));
        assert!(!origin.owns("https://gateway.example:8443/oauth/token"));
        assert!(!origin.owns("https://gateway.example.evil/oauth/token"));
        assert!(!origin.owns("https://idp.example/oauth/token"));
        assert!(!origin.owns("not a url"));
    }

    #[test]
    fn a_path_query_fragment_or_credential_is_refused() {
        for url in [
            "https://gateway.example/v1",
            "https://gateway.example/?a=b",
            "https://gateway.example/#x",
            "https://user:pw@gateway.example",
            "not a url",
            "",
        ] {
            assert!(GatewayOrigin::parse(url).is_err(), "{url} must be refused");
        }
    }
}
