//! Reads the gateway's OAuth endpoints from its discovery document
//! (RFC 8414), and refuses one that could send a token anywhere else.

use serde::Deserialize;

use super::token_endpoint::http_client;
use super::GatewayOrigin;

type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// The only gateway protocol this client speaks.
const GATEWAY_PROTOCOL_VERSION: u64 = 1;

/// Where the discovery document lives, relative to the origin.
const DISCOVERY_PATH: &str = "/.well-known/oauth-authorization-server";

/// The gateway's OAuth endpoints. All lie on the gateway origin, so the
/// device code and the tokens never leave it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GatewayEndpoints {
    pub device_authorization: String,
    pub token: String,
    /// Where sign-out revokes the tokens (RFC 7009), when the gateway offers it.
    pub revocation: Option<String>,
}

#[derive(Deserialize)]
struct DiscoveryDocument {
    issuer: String,
    device_authorization_endpoint: Option<String>,
    token_endpoint: Option<String>,
    revocation_endpoint: Option<String>,
    gateway_protocol_version: Option<serde_json::Value>,
}

/// Fetch and check `origin`'s discovery document.
///
/// Refuses a document whose `issuer` is not `origin`, whose protocol version is
/// not the one this client speaks, or whose endpoints lie on another origin.
pub async fn discover(origin: &GatewayOrigin) -> Result<GatewayEndpoints, BoxError> {
    let url = format!("{origin}{DISCOVERY_PATH}");
    let resp = http_client()
        .get(&url)
        .header("Accept", "application/json")
        .send()
        .await
        .map_err(|e| format!("could not reach {url}: {e}"))?;
    let status = resp.status();
    if !status.is_success() {
        return Err(format!("{url} answered {status}, so {origin} is not a Claude gateway").into());
    }
    let document: DiscoveryDocument = resp
        .json()
        .await
        .map_err(|e| format!("{url} is not an OAuth discovery document: {e}"))?;
    check(origin, document).map_err(Into::into)
}

fn check(origin: &GatewayOrigin, document: DiscoveryDocument) -> Result<GatewayEndpoints, String> {
    let issuer = GatewayOrigin::parse(&document.issuer).ok();
    if issuer.as_ref() != Some(origin) {
        return Err(format!(
            "{origin} names its issuer '{}', so it cannot vouch for its own tokens",
            document.issuer
        ));
    }
    let version = document
        .gateway_protocol_version
        .as_ref()
        .and_then(serde_json::Value::as_u64);
    if version != Some(GATEWAY_PROTOCOL_VERSION) {
        let shown = document
            .gateway_protocol_version
            .map_or_else(|| "none".to_string(), |v| v.to_string());
        return Err(format!(
            "{origin} speaks Claude gateway protocol {shown}; Lucidos speaks \
             {GATEWAY_PROTOCOL_VERSION}"
        ));
    }
    let endpoint = |name: &str, value: Option<String>| match value {
        Some(url) if origin.owns(&url) => Ok(url),
        Some(url) => Err(format!("{origin} puts its {name} on another origin: {url}")),
        None => Err(format!("{origin} names no {name}")),
    };
    Ok(GatewayEndpoints {
        device_authorization: endpoint(
            "device_authorization_endpoint",
            document.device_authorization_endpoint,
        )?,
        token: endpoint("token_endpoint", document.token_endpoint)?,
        revocation: same_origin_revocation(origin, document.revocation_endpoint),
    })
}

/// The revocation endpoint, when it lies on the gateway origin. One elsewhere
/// is ignored rather than refused: revoking is best effort, and signing in
/// does not need it.
fn same_origin_revocation(origin: &GatewayOrigin, endpoint: Option<String>) -> Option<String> {
    let url = endpoint?;
    if origin.owns(&url) {
        return Some(url);
    }
    crate::log!("[ClaudeGateway] {origin} puts its revocation_endpoint on another origin, so sign-out will not revoke: {url}");
    None
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use axum::http::StatusCode;

    /// The discovery document a well-formed gateway at `origin` serves.
    pub(crate) fn document(origin: &str) -> serde_json::Value {
        serde_json::json!({
            "issuer": origin,
            "device_authorization_endpoint": format!("{origin}/oauth/device_authorization"),
            "token_endpoint": format!("{origin}/oauth/token"),
            "revocation_endpoint": format!("{origin}/oauth/revoke"),
            "grant_types_supported": [
                "urn:ietf:params:oauth:grant-type:device_code",
                "refresh_token"
            ],
            "token_endpoint_auth_methods_supported": ["none"],
            "gateway_protocol_version": 1
        })
    }

    fn checked(
        origin: &str,
        edit: impl FnOnce(&mut serde_json::Value),
    ) -> Result<GatewayEndpoints, String> {
        let origin_value = GatewayOrigin::parse(origin).unwrap();
        let mut value = document(origin);
        edit(&mut value);
        check(&origin_value, serde_json::from_value(value).unwrap())
    }

    #[test]
    fn a_well_formed_document_gives_both_endpoints() {
        let endpoints = checked("https://gateway.example", |_| {}).unwrap();
        assert_eq!(
            endpoints,
            GatewayEndpoints {
                device_authorization: "https://gateway.example/oauth/device_authorization".into(),
                token: "https://gateway.example/oauth/token".into(),
                revocation: Some("https://gateway.example/oauth/revoke".into()),
            }
        );
    }

    /// Sign-out sends both tokens to the revocation endpoint, so one on
    /// another origin is never used. Signing in still works without it.
    #[test]
    fn a_revocation_endpoint_on_another_origin_is_ignored() {
        let endpoints = checked("https://gateway.example", |d| {
            d["revocation_endpoint"] = "https://idp.example/oauth/revoke".into()
        })
        .unwrap();
        assert_eq!(endpoints.revocation, None);
        let none = checked("https://gateway.example", |d| {
            d.as_object_mut().unwrap().remove("revocation_endpoint");
        })
        .unwrap();
        assert_eq!(none.revocation, None);
    }

    /// RFC 8414 §3.3: the issuer must be the URL the document was read from.
    /// A trailing slash or a case difference is the same origin.
    #[test]
    fn the_issuer_must_be_the_configured_origin() {
        assert!(checked("https://gateway.example", |d| d["issuer"] =
            "https://GATEWAY.example/".into())
        .is_ok());
        let err = checked("https://gateway.example", |d| {
            d["issuer"] = "https://other.example".into()
        })
        .unwrap_err();
        assert!(err.contains("issuer"), "{err}");
    }

    #[test]
    fn only_protocol_version_1_is_spoken() {
        for version in [
            serde_json::json!(2),
            serde_json::json!("1"),
            serde_json::Value::Null,
        ] {
            let err = checked("https://gateway.example", |d| {
                d["gateway_protocol_version"] = version.clone()
            })
            .unwrap_err();
            assert!(err.contains("protocol"), "{version}: {err}");
        }
    }

    /// The device code and the refresh token go to these endpoints, so one on
    /// another origin would carry them off the gateway.
    #[test]
    fn an_endpoint_on_another_origin_is_refused() {
        let err = checked("https://gateway.example", |d| {
            d["token_endpoint"] = "https://idp.example/oauth/token".into()
        })
        .unwrap_err();
        assert!(err.contains("another origin"), "{err}");
        let err = checked("https://gateway.example", |d| {
            d.as_object_mut()
                .unwrap()
                .remove("device_authorization_endpoint");
        })
        .unwrap_err();
        assert!(
            err.contains("names no device_authorization_endpoint"),
            "{err}"
        );
    }

    #[tokio::test]
    async fn discover_reads_the_well_known_path() {
        let gateway = super::super::mock_gateway::MockGateway::start().await;
        let endpoints = discover(&gateway.origin).await.unwrap();
        assert_eq!(endpoints.token, format!("{}/oauth/token", gateway.origin));
        assert_eq!(gateway.paths(), vec![DISCOVERY_PATH.to_string()]);
    }

    #[tokio::test]
    async fn a_host_with_no_document_is_not_a_gateway() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let app = axum::Router::new().fallback(|| async { StatusCode::NOT_FOUND });
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let origin = GatewayOrigin::parse(&format!("http://{addr}")).unwrap();
        let err = discover(&origin).await.unwrap_err().to_string();
        assert!(err.contains("is not a Claude gateway"), "{err}");
    }
}
