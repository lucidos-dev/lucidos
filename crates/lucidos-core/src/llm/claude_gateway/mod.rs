pub mod account;
pub mod device_flow;
pub mod discovery;
pub mod gateway_managed_settings;
pub mod machine_managed_settings;
#[cfg(any(test, feature = "test-support"))]
pub mod mock_gateway;
pub mod revocation;
pub mod served_models;
pub mod telemetry;
mod token_endpoint;
pub mod tokens;

pub use token_endpoint::GrantedTokens;

pub(crate) const SIGN_IN_LOCATION: &str = providers_settings!();

pub(crate) fn unauthorized(error_body: &str) -> String {
    format!(
        "Claude gateway 401: sign in to the Claude gateway again in {SIGN_IN_LOCATION}. A gateway \
         reachable only on a private network also answers this when that network has \
         dropped: {error_body}"
    )
}

pub(crate) use super::provider_factory::is_loopback;
pub use super::provider_factory::{GatewayAccess, GatewayAccessFactory, GatewayOrigin};

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

    #[test]
    fn plain_http_is_refused_except_on_loopback() {
        assert!(GatewayOrigin::parse("http://gateway.example").is_err());
        assert!(GatewayOrigin::parse("http://10.0.0.5").is_err());
        assert!(GatewayOrigin::parse("http://127.0.0.1:4100").is_ok());
        assert!(GatewayOrigin::parse("http://localhost:4100").is_ok());
        assert!(GatewayOrigin::parse("http://[::1]:4100").is_ok());
        assert!(GatewayOrigin::parse("ftp://gateway.example").is_err());
    }

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
