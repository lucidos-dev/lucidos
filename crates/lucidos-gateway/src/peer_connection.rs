//! The only outbound gateway-to-gateway transport. Redirects are refused;
//! streaming calls have no total timeout. Interactive calls set their own.
use crate::auth;
use crate::gateway_protocol::{
    PairAnswer, GATEWAY_API_PREFIX, HEADER_GATEWAY_CREDENTIAL, PAIRING_PATH, PAIR_PATH,
};
use crate::peers::GATEWAY_HEALTH_PATH;
use crate::remote_gateways::{ExtraHeaders, RemoteGateway};
use std::time::Duration;
const PEER_TIMEOUT: Duration = Duration::from_secs(10);

/// A's client for B: `https` only, certificates checked, no redirects.
#[derive(Clone)]
pub struct PeerConnection {
    http: std::sync::Arc<std::sync::RwLock<reqwest::Client>>,
}

impl PeerConnection {
    pub fn new() -> Result<Self, String> {
        Self::build(None)
    }

    /// A client that also trusts `pem`, for a test's self-signed stand-in.
    #[cfg(test)]
    pub fn trusting(pem: &[u8]) -> Self {
        Self::build(Some(reqwest::Certificate::from_pem(pem).unwrap())).unwrap()
    }

    fn build(extra_root: Option<reqwest::Certificate>) -> Result<Self, String> {
        let mut builder = reqwest::Client::builder()
            .https_only(true)
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(PEER_TIMEOUT);
        if let Some(root) = extra_root {
            builder = builder.add_root_certificate(root);
        }
        let http = builder
            .build()
            .map_err(|e| format!("could not start the remote gateway client: {e}"))?;
        Ok(Self {
            http: std::sync::Arc::new(std::sync::RwLock::new(http)),
        })
    }

    fn transport(&self) -> Result<reqwest::Client, String> {
        self.http
            .read()
            .map(|client| client.clone())
            .map_err(|_| "the peer connection is unavailable".to_string())
    }

    pub(crate) fn client(&self) -> Option<Self> {
        self.transport().ok().map(|_| self.clone())
    }

    #[cfg(test)]
    pub fn trust_for_test(&self, pem: &[u8]) {
        let trusted = Self::trusting(pem);
        *self.http.write().unwrap() = trusted.transport().unwrap();
    }

    /// Build a request to this remote's origin, with its gateway credential.
    /// No caller can supply a different destination URL.
    pub fn request(
        &self,
        gateway: &RemoteGateway,
        method: reqwest::Method,
        path: &str,
    ) -> Result<reqwest::RequestBuilder, String> {
        if !path.starts_with('/') || path.starts_with("//") {
            return Err("a peer request needs an origin-relative path".into());
        }
        let credential = auth::sensitive_credential(&gateway.credential)
            .ok_or("the remote gateway credential is not a header value")?;
        Ok(gateway.headers.apply(
            self.transport()?
                .request(method, format!("{}{path}", gateway.address))
                .header(HEADER_GATEWAY_CREDENTIAL, credential),
        ))
    }

    /// B's public health body.
    pub async fn health(&self, origin: &str) -> Result<serde_json::Value, String> {
        self.health_with_headers(origin, &ExtraHeaders::default())
            .await
    }

    pub async fn health_with_headers(
        &self,
        origin: &str,
        headers: &ExtraHeaders,
    ) -> Result<serde_json::Value, String> {
        let response = headers
            .apply(
                self.transport()?
                    .get(format!("{origin}{GATEWAY_HEALTH_PATH}")),
            )
            .timeout(PEER_TIMEOUT)
            .send()
            .await
            .map_err(|e| format!("could not reach {origin}: {}", reached_error(&e)))?;
        let status = response.status();
        let html = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v.to_ascii_lowercase().contains("text/html"));
        if status.is_redirection() || (html && matches!(status.as_u16(), 401 | 403)) {
            let host = reqwest::Url::parse(origin)
                .ok()
                .and_then(|u| u.host_str().map(str::to_owned))
                .unwrap_or_default();
            return Err(format!("this address answered with a sign-in page at {host}; add proxy headers or use the tailnet address"));
        }
        if !status.is_success() {
            return Err(format!(
                "{origin} answered with HTTP {}, not with Lucidos's health",
                response.status().as_u16()
            ));
        }
        read_json(response)
            .await
            .map_err(|_| format!("{origin} answered, but not with Lucidos's health"))
    }

    /// Redeem `code` at B.
    pub async fn pair_with_headers(
        &self,
        origin: &str,
        code: &str,
        label: &str,
        headers: &ExtraHeaders,
    ) -> Result<PairAnswer, String> {
        let response = headers
            .apply(
                self.transport()?
                    .post(format!("{origin}{GATEWAY_API_PREFIX}{PAIR_PATH}")),
            )
            .timeout(PEER_TIMEOUT)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(serde_json::json!({ "code": code, "label": label }).to_string())
            .send()
            .await
            .map_err(|e| format!("could not reach {origin}: {}", reached_error(&e)))?;
        if !response.status().is_success() {
            return Err(refusal(response).await);
        }
        read_json(response)
            .await
            .map_err(|e| format!("{origin} paired, but its answer was unreadable: {e}"))
    }

    /// Recover first with the candidate: B may have committed before A crashed.
    /// Only a 401 permits retrying the durable intent with the old credential.
    pub async fn rotate(&self, gateway: &RemoteGateway, candidate: &str) -> Result<u32, String> {
        let mut proposed = gateway.clone();
        proposed.credential = candidate.to_owned();
        let response = self
            .request(
                &proposed,
                reqwest::Method::GET,
                &format!("{GATEWAY_API_PREFIX}{PAIRING_PATH}"),
            )?
            .timeout(PEER_TIMEOUT)
            .send()
            .await
            .map_err(|_| "could not reach the remote gateway during rotation".to_string())?;
        if response.status().is_success() {
            let identity: serde_json::Value = read_json(response)
                .await
                .map_err(|_| "remote rotation identity was unreadable")?;
            if identity.get("pairing_id").and_then(|v| v.as_str()) != Some(&gateway.pairing_id) {
                return Err("remote rotation identity did not match this pairing".into());
            }
            return rotation_protocol(&identity);
        }
        if response.status() != reqwest::StatusCode::UNAUTHORIZED {
            return Err(format!(
                "remote rotation recovery answered HTTP {}",
                response.status().as_u16()
            ));
        }
        let response = self
            .request(
                gateway,
                reqwest::Method::POST,
                &format!(
                    "{GATEWAY_API_PREFIX}{}",
                    crate::gateway_protocol::ROTATE_PATH
                ),
            )?
            .timeout(PEER_TIMEOUT)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(serde_json::json!({"candidate": candidate}).to_string())
            .send()
            .await
            .map_err(|_| "could not reach the remote gateway during rotation".to_string())?;
        if matches!(response.status().as_u16(), 404 | 405 | 501) {
            return Err("the remote gateway does not support credential rotation yet; update it to enable automatic retries".into());
        }
        if !response.status().is_success() {
            return Err(format!(
                "remote credential rotation answered HTTP {}",
                response.status().as_u16()
            ));
        }
        // Confirm the new credential and retire B's old-key overlap before completion.
        let response = self
            .request(
                &proposed,
                reqwest::Method::GET,
                &format!("{GATEWAY_API_PREFIX}{PAIRING_PATH}"),
            )?
            .timeout(PEER_TIMEOUT)
            .send()
            .await
            .map_err(|_| "remote rotation confirmation could not be reached".to_string())?;
        if !response.status().is_success() {
            return Err(format!(
                "remote rotation confirmation answered HTTP {}",
                response.status().as_u16()
            ));
        }
        let identity: serde_json::Value = read_json(response)
            .await
            .map_err(|_| "remote rotation identity was unreadable")?;
        if identity.get("pairing_id").and_then(|v| v.as_str()) != Some(&gateway.pairing_id) {
            return Err("remote rotation identity did not match this pairing".into());
        }
        rotation_protocol(&identity)
    }

    /// Ask B to forget this pairing.
    pub async fn leave(&self, gateway: &RemoteGateway) -> Result<(), String> {
        let request = self
            .request(
                gateway,
                reqwest::Method::DELETE,
                &format!("{GATEWAY_API_PREFIX}{PAIRING_PATH}"),
            )?
            .timeout(PEER_TIMEOUT);
        let response = request
            .send()
            .await
            .map_err(|e| format!("could not reach {}: {}", gateway.address, reached_error(&e)))?;
        if response.status().is_success() {
            Ok(())
        } else {
            Err(refusal(response).await)
        }
    }
}

/// The reason a request failed, naming a refused redirect plainly.
fn reached_error(e: &reqwest::Error) -> String {
    if e.is_redirect() {
        return "it redirected elsewhere, and a gateway credential never follows a redirect"
            .to_string();
    }
    crate::external_address::error_chain(e)
}

/// A JSON body, without reqwest's `json` feature.
async fn read_json<T: serde::de::DeserializeOwned>(
    response: reqwest::Response,
) -> Result<T, String> {
    let bytes = response.bytes().await.map_err(|e| e.to_string())?;
    serde_json::from_slice(&bytes).map_err(|e| e.to_string())
}

/// B's own sentence for a refusal, else the status.
async fn refusal(response: reqwest::Response) -> String {
    let status = response.status();
    read_json::<serde_json::Value>(response)
        .await
        .ok()
        .and_then(|b| b.get("error").and_then(|e| e.as_str()).map(str::to_string))
        .unwrap_or_else(|| format!("the other gateway answered HTTP {}", status.as_u16()))
}

fn rotation_protocol(identity: &serde_json::Value) -> Result<u32, String> {
    identity
        .get("gateway_protocol")
        .and_then(|value| value.as_u64())
        .and_then(|value| u32::try_from(value).ok())
        .filter(|version| *version >= 2)
        .ok_or_else(|| "remote rotation protocol was unreadable".into())
}

#[cfg(test)]
mod tests {
    #[test]
    fn rotation_preserves_the_remote_protocol() {
        assert_eq!(
            super::rotation_protocol(&serde_json::json!({"gateway_protocol": 9})).unwrap(),
            9
        );
        assert!(super::rotation_protocol(&serde_json::json!({"gateway_protocol": 1})).is_err());
        assert!(super::rotation_protocol(&serde_json::json!({})).is_err());
    }
}
