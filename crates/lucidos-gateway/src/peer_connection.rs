use crate::auth;
use crate::gateway_protocol::{
    PairAnswer, GATEWAY_API_PREFIX, HEADER_GATEWAY_CREDENTIAL, PAIRING_PATH, PAIR_PATH,
};
use crate::peers::GATEWAY_HEALTH_PATH;
use crate::remote_gateways::{ExtraHeaders, RemoteGateway};
use std::time::Duration;
pub(crate) const PEER_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug)]
pub enum PairReply {
    Paired(PairAnswer),
    AwaitingOwner,
}

#[derive(Clone)]
pub struct PeerConnection {
    http: reqwest::Client,
    loopback: reqwest::Client,
    upgrade_http: reqwest::Client,
    upgrade_loopback: reqwest::Client,
}

impl PeerConnection {
    pub fn new() -> Result<Self, String> {
        let unavailable =
            |e: reqwest::Error| format!("could not start the remote gateway client: {e}");
        let http = || {
            reqwest::Client::builder()
                .https_only(true)
                .redirect(reqwest::redirect::Policy::none())
                .connect_timeout(PEER_TIMEOUT)
        };
        let loopback =
            || lucidos_local_token::client::client_builder().connect_timeout(PEER_TIMEOUT);
        Ok(Self {
            http: http().build().map_err(unavailable)?,
            loopback: loopback().build().map_err(unavailable)?,
            upgrade_http: http().http1_only().build().map_err(unavailable)?,
            upgrade_loopback: loopback().http1_only().build().map_err(unavailable)?,
        })
    }

    #[cfg(test)]
    pub(crate) fn trusting_loopback_root(root_pem: &str) -> Result<Self, String> {
        let mut connection = Self::new()?;
        let root =
            reqwest::Certificate::from_pem(root_pem.as_bytes()).map_err(|e| e.to_string())?;
        let loopback = || {
            reqwest::Client::builder()
                .no_proxy()
                .use_rustls_tls()
                .tls_built_in_root_certs(false)
                .redirect(reqwest::redirect::Policy::none())
                .connect_timeout(PEER_TIMEOUT)
                .add_root_certificate(root.clone())
        };
        connection.loopback = loopback().build().map_err(|e| e.to_string())?;
        connection.upgrade_loopback = loopback().http1_only().build().map_err(|e| e.to_string())?;
        Ok(connection)
    }

    fn endpoint(&self, origin: &str, upgrade: bool) -> (reqwest::Client, String) {
        if crate::remote_gateways::is_loopback_origin(origin) {
            let https = match origin.strip_prefix("http://") {
                Some(rest) => format!("https://{rest}"),
                None => origin.to_string(),
            };
            let client = if upgrade {
                &self.upgrade_loopback
            } else {
                &self.loopback
            };
            return (client.clone(), https);
        }
        let client = if upgrade {
            &self.upgrade_http
        } else {
            &self.http
        };
        (client.clone(), origin.to_string())
    }

    pub(crate) fn return_request(
        &self,
        origin: &str,
        path: &str,
    ) -> Result<reqwest::RequestBuilder, String> {
        let origin = crate::remote_gateways::valid_address(origin)?;
        if !path.starts_with('/') || path.starts_with("//") {
            return Err("A return request requires an origin-relative path.".into());
        }
        let (client, origin) = self.endpoint(&origin, false);
        Ok(client.post(format!("{origin}{path}")))
    }

    pub fn request(
        &self,
        gateway: &RemoteGateway,
        method: reqwest::Method,
        path: &str,
    ) -> Result<reqwest::RequestBuilder, String> {
        self.gateway_request(gateway, method, path, false)
    }

    pub(crate) fn upgrade_request(
        &self,
        gateway: &RemoteGateway,
        path: &str,
    ) -> Result<reqwest::RequestBuilder, String> {
        Ok(self
            .gateway_request(gateway, reqwest::Method::GET, path, true)?
            .timeout(PEER_TIMEOUT))
    }

    fn gateway_request(
        &self,
        gateway: &RemoteGateway,
        method: reqwest::Method,
        path: &str,
        upgrade: bool,
    ) -> Result<reqwest::RequestBuilder, String> {
        if !path.starts_with('/') || path.starts_with("//") {
            return Err("a peer request needs an origin-relative path".into());
        }
        let credential = auth::sensitive_credential(&gateway.credential)
            .ok_or("the remote gateway credential is not a header value")?;
        let (client, address) = self.endpoint(&gateway.address, upgrade);
        Ok(gateway.headers.apply(
            client
                .request(method, format!("{address}{path}"))
                .header(HEADER_GATEWAY_CREDENTIAL, credential),
        ))
    }

    pub async fn health(&self, origin: &str) -> Result<serde_json::Value, String> {
        self.health_with_headers(origin, &ExtraHeaders::default())
            .await
    }

    pub async fn health_with_headers(
        &self,
        origin: &str,
        headers: &ExtraHeaders,
    ) -> Result<serde_json::Value, String> {
        self.client_health(origin, headers)
            .await
            .map_err(|(_, reason)| reason)
    }

    pub async fn client_health(
        &self,
        origin: &str,
        headers: &ExtraHeaders,
    ) -> Result<serde_json::Value, (crate::remote_workspaces::ReasonKind, String)> {
        use crate::remote_workspaces::ReasonKind;
        let (client, called) = self.endpoint(origin, false);
        let mut response = headers
            .apply(client.get(format!("{called}{GATEWAY_HEALTH_PATH}")))
            .timeout(PEER_TIMEOUT)
            .send()
            .await
            .map_err(|e| {
                (
                    crate::remote_workspaces::transport_reason(&e),
                    format!("could not reach {origin}: {}", reached_error(&e)),
                )
            })?;
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
            return Err((ReasonKind::Incompatible, format!("this address answered with a sign-in page at {host}; add proxy headers or use the tailnet address")));
        }
        if !status.is_success() {
            return Err((
                ReasonKind::Incompatible,
                format!(
                    "{origin} answered with HTTP {}, not with Lucidos's health",
                    response.status().as_u16()
                ),
            ));
        }
        let invalid = || {
            (
                ReasonKind::Incompatible,
                format!("{origin} answered with invalid or oversized Lucidos health"),
            )
        };
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| invalid())? {
            if bytes.len() + chunk.len() > crate::remote_browser::MAX_CLIENT_HEALTH_BYTES {
                return Err(invalid());
            }
            bytes.extend_from_slice(&chunk);
        }
        serde_json::from_slice(&bytes).map_err(|_| {
            (
                ReasonKind::Incompatible,
                format!("{origin} answered, but not with Lucidos's health"),
            )
        })
    }

    pub async fn pair_with_headers(
        &self,
        origin: &str,
        code: &str,
        label: &str,
        headers: &ExtraHeaders,
    ) -> Result<PairReply, String> {
        let (client, called) = self.endpoint(origin, false);
        let response = headers
            .apply(client.post(format!("{called}{GATEWAY_API_PREFIX}{PAIR_PATH}")))
            .timeout(PEER_TIMEOUT)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(serde_json::json!({ "code": code, "label": label }).to_string())
            .send()
            .await
            .map_err(|e| format!("could not reach {origin}: {}", reached_error(&e)))?;
        if response.status() == reqwest::StatusCode::ACCEPTED {
            return Ok(PairReply::AwaitingOwner);
        }
        if !response.status().is_success() {
            return Err(refusal(response).await);
        }
        read_json(response)
            .await
            .map(PairReply::Paired)
            .map_err(|e| format!("{origin} paired, but its answer was unreadable: {e}"))
    }

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

    pub async fn destination_identity(
        &self,
        gateway: &RemoteGateway,
    ) -> Result<Option<String>, String> {
        let response = self
            .request(
                gateway,
                reqwest::Method::GET,
                &format!("{GATEWAY_API_PREFIX}{PAIRING_PATH}"),
            )?
            .timeout(PEER_TIMEOUT)
            .send()
            .await
            .map_err(|_| "could not read the remote gateway identity")?;
        if !response.status().is_success() {
            return Err(refusal(response).await);
        }
        let body: serde_json::Value = read_json(response).await?;
        if body.get("pairing_id").and_then(|v| v.as_str()) != Some(&gateway.pairing_id) {
            return Err("remote identity did not match this pairing".into());
        }
        match body.get("gateway_id") {
            None | Some(serde_json::Value::Null) => Ok(None),
            Some(serde_json::Value::String(id)) if !id.trim().is_empty() => Ok(Some(id.clone())),
            _ => Err("remote gateway identity was invalid".into()),
        }
    }

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

fn reached_error(e: &reqwest::Error) -> String {
    if e.is_redirect() {
        return "it redirected elsewhere, and a gateway credential never follows a redirect"
            .to_string();
    }
    crate::external_address::error_chain(e)
}

async fn read_json<T: serde::de::DeserializeOwned>(
    response: reqwest::Response,
) -> Result<T, String> {
    let bytes = response.bytes().await.map_err(|e| e.to_string())?;
    serde_json::from_slice(&bytes).map_err(|e| e.to_string())
}

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
    #[tokio::test]
    async fn gateway_health_refuses_oversized_chunked_json() {
        use axum::response::IntoResponse;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let app = axum::Router::new().fallback(|| async {
            let json = serde_json::to_vec(&serde_json::json!({"padding": "x".repeat(crate::remote_browser::MAX_CLIENT_HEALTH_BYTES + 1)})).unwrap();
            let chunks: Vec<_> = json.chunks(1024).map(|c| Ok::<_, std::io::Error>(axum::body::Bytes::copy_from_slice(c))).collect();
            axum::body::Body::from_stream(futures::stream::iter(chunks)).into_response()
        });
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let result = super::PeerConnection::new()
            .unwrap()
            .client_health(
                &format!("http://{address}"),
                &super::ExtraHeaders::default(),
            )
            .await;
        server.abort();
        assert!(
            result.is_err(),
            "untrusted gateway health must have a body limit"
        );
    }

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
