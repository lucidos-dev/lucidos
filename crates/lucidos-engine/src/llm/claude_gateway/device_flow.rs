//! The OAuth device authorization grant (RFC 8628), as a public client.
//!
//! The user opens the gateway's verification page on any device and enters
//! the user code, while the engine polls the token endpoint. Nothing here is
//! stored: an engine restart mid-sign-in only means starting again.

use std::time::Duration;

use serde::Deserialize;

use super::discovery::GatewayEndpoints;
use super::token_endpoint::{
    http_client, post_token_form, GrantedTokens, OAuthRefusal, TokenReply,
};

type BoxError = Box<dyn std::error::Error + Send + Sync>;

const DEVICE_CODE_GRANT: &str = "urn:ietf:params:oauth:grant-type:device_code";

/// The scopes the gateway advertises. Lucidos asks for no more.
const SCOPES: &str = "openid profile email";

/// The protocol's `surface` extension: which client application signs in, so
/// a gateway can tell Lucidos sessions apart (Claude Code sends `claude_code`).
const SURFACE: &str = "lucidos";

/// The poll interval when the gateway names none (RFC 8628 §3.2).
const DEFAULT_INTERVAL: Duration = Duration::from_secs(5);

/// The longest a code may live, and the longest poll interval, whatever the
/// gateway says. A larger value would overflow the deadline or stall the poll.
const MAX_CODE_LIFETIME: Duration = Duration::from_secs(60 * 60);

/// How much each `slow_down` adds to the interval (RFC 8628 §3.5).
const SLOW_DOWN_STEP: Duration = Duration::from_secs(5);

/// A started sign-in: what the user must see, and the code the engine polls
/// with.
pub struct DeviceAuthorization {
    device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    /// The verification page with the user code filled in, when the gateway
    /// offers one.
    pub verification_uri_complete: Option<String>,
    pub expires_in: Duration,
    pub interval: Duration,
}

impl std::fmt::Debug for DeviceAuthorization {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DeviceAuthorization")
            .field("device_code", &"<redacted>")
            .field("user_code", &self.user_code)
            .field("verification_uri", &self.verification_uri)
            .field("verification_uri_complete", &self.verification_uri_complete)
            .field("expires_in", &self.expires_in)
            .field("interval", &self.interval)
            .finish()
    }
}

#[derive(Deserialize)]
struct AuthorizationResponse {
    device_code: String,
    user_code: String,
    verification_uri: String,
    verification_uri_complete: Option<String>,
    expires_in: u64,
    interval: Option<u64>,
}

/// Ask the gateway for a device code and a user code.
pub async fn start(endpoints: &GatewayEndpoints) -> Result<DeviceAuthorization, BoxError> {
    let resp = http_client()
        .post(&endpoints.device_authorization)
        .header("Accept", "application/json")
        .form(&[("scope", SCOPES), ("surface", SURFACE)])
        .send()
        .await?;
    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        let reason = serde_json::from_str::<OAuthRefusal>(&body)
            .map_or_else(|_| status.to_string(), |r| r.to_string());
        return Err(format!("The Claude gateway refused to start a sign-in: {reason}").into());
    }
    let answer: AuthorizationResponse = resp
        .json()
        .await
        .map_err(|e| format!("The Claude gateway's sign-in answer is malformed: {e}"))?;
    for uri in std::iter::once(&answer.verification_uri).chain(&answer.verification_uri_complete) {
        if !is_web_link(uri) {
            return Err(
                format!("The Claude gateway's verification page is not a web link: {uri}").into(),
            );
        }
    }
    Ok(DeviceAuthorization {
        device_code: answer.device_code,
        user_code: answer.user_code,
        verification_uri: answer.verification_uri,
        verification_uri_complete: answer.verification_uri_complete,
        expires_in: Duration::from_secs(answer.expires_in).min(MAX_CODE_LIFETIME),
        interval: answer
            .interval
            .map_or(DEFAULT_INTERVAL, Duration::from_secs)
            .min(MAX_CODE_LIFETIME),
    })
}

/// The verification page is shown as a link, so it must open a web page and
/// nothing else.
fn is_web_link(uri: &str) -> bool {
    reqwest::Url::parse(uri).is_ok_and(|u| matches!(u.scheme(), "https" | "http"))
}

/// Poll the token endpoint until the user approves, declines, or the code
/// expires. A failed request is tried again on the next tick, since a
/// gateway on a private network can drop out for a moment.
pub async fn wait_for_tokens(
    endpoints: &GatewayEndpoints,
    authorization: &DeviceAuthorization,
) -> Result<GrantedTokens, BoxError> {
    poll(endpoints, authorization, SLOW_DOWN_STEP).await
}

async fn poll(
    endpoints: &GatewayEndpoints,
    authorization: &DeviceAuthorization,
    slow_down_step: Duration,
) -> Result<GrantedTokens, BoxError> {
    let deadline = tokio::time::Instant::now() + authorization.expires_in;
    let mut interval = authorization.interval;
    let form = [
        ("grant_type", DEVICE_CODE_GRANT),
        ("device_code", authorization.device_code.as_str()),
    ];
    let mut last_failure: Option<BoxError> = None;
    loop {
        tokio::time::sleep(interval).await;
        if tokio::time::Instant::now() >= deadline {
            return Err(match last_failure {
                Some(e) => format!(
                    "The Claude gateway sign-in code expired while the gateway could not be \
                     reached: {e}. Start again."
                )
                .into(),
                None => EXPIRED.into(),
            });
        }
        let refusal = match post_token_form(&endpoints.token, &form).await {
            Ok(TokenReply::Granted(tokens)) => return Ok(tokens),
            Ok(TokenReply::Refused(refusal)) => refusal,
            Err(e) => {
                crate::log!(
                    "[ClaudeGateway] The sign-in poll failed, trying again: {}",
                    e
                );
                last_failure = Some(e);
                continue;
            }
        };
        last_failure = None;
        match refusal.error.as_str() {
            "authorization_pending" => {}
            "slow_down" => interval += slow_down_step,
            "expired_token" => return Err(EXPIRED.into()),
            "access_denied" => return Err("The Claude gateway sign-in was declined.".into()),
            _ => return Err(format!("The Claude gateway refused the sign-in: {refusal}").into()),
        }
    }
}

const EXPIRED: &str =
    "The Claude gateway sign-in code expired before it was approved. Start again.";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::claude_gateway::discovery::discover;
    use crate::llm::claude_gateway::mock_gateway::MockGateway;
    use axum::http::StatusCode;

    async fn started() -> (MockGateway, GatewayEndpoints, DeviceAuthorization) {
        let gateway = MockGateway::start().await;
        let endpoints = discover(&gateway.origin).await.unwrap();
        let authorization = start(&endpoints).await.unwrap();
        (gateway, endpoints, authorization)
    }

    fn grant() -> serde_json::Value {
        serde_json::json!({"access_token": "a", "refresh_token": "r", "expires_in": 3600})
    }

    #[tokio::test]
    async fn start_shows_the_user_code_and_hides_the_device_code() {
        let (gateway, _, authorization) = started().await;
        assert_eq!(authorization.user_code, "ABCD-EFGH");
        assert_eq!(
            authorization.verification_uri,
            format!("{}/device", gateway.origin)
        );
        assert_eq!(authorization.interval, Duration::ZERO);
        assert_eq!(authorization.expires_in, Duration::from_secs(600));
        assert!(!format!("{authorization:?}").contains("device-code"));

        let seen = gateway.seen();
        let request = seen
            .iter()
            .find(|s| s.path == "/oauth/device_authorization")
            .unwrap();
        assert_eq!(request.form.get("scope").map(String::as_str), Some(SCOPES));
        assert_eq!(
            request.form.get("surface").map(String::as_str),
            Some("lucidos")
        );
        assert!(
            !request.form.contains_key("client_id"),
            "a public client sends none"
        );
        assert!(request
            .user_agent
            .as_deref()
            .unwrap()
            .starts_with("Lucidos/"));
    }

    /// A huge lifetime or interval would overflow the deadline or stall the
    /// poll, so both are capped.
    #[tokio::test]
    async fn the_gateway_cannot_name_an_endless_code() {
        let gateway = MockGateway::start().await;
        gateway.set_device_field("expires_in", u64::MAX.into());
        gateway.set_device_field("interval", u64::MAX.into());
        let endpoints = discover(&gateway.origin).await.unwrap();
        let authorization = start(&endpoints).await.unwrap();
        assert_eq!(authorization.expires_in, MAX_CODE_LIFETIME);
        assert_eq!(authorization.interval, MAX_CODE_LIFETIME);
    }

    #[tokio::test]
    async fn a_verification_page_that_is_not_a_web_link_is_refused() {
        let gateway = MockGateway::start().await;
        gateway.set_device_field("verification_uri", "javascript:alert(1)".into());
        let endpoints = discover(&gateway.origin).await.unwrap();
        let err = start(&endpoints).await.unwrap_err().to_string();
        assert!(err.contains("not a web link"), "{err}");
    }

    #[tokio::test]
    async fn polling_waits_through_pending_and_slow_down() {
        let (gateway, endpoints, authorization) = started().await;
        gateway.refuse_token("authorization_pending");
        gateway.refuse_token("slow_down");
        gateway.reply_to_token(StatusCode::OK, grant());

        let tokens = poll(&endpoints, &authorization, Duration::ZERO)
            .await
            .unwrap();
        assert_eq!(tokens.access_token, "a");
        let forms = gateway.token_forms();
        assert_eq!(forms.len(), 3);
        assert!(forms
            .iter()
            .all(|f| f["grant_type"] == DEVICE_CODE_GRANT && f["device_code"] == "device-code"));
    }

    /// A gateway that drops out mid-flow is polled again, and the sign-in
    /// still lands.
    #[tokio::test]
    async fn polling_survives_a_failed_request() {
        let (gateway, endpoints, authorization) = started().await;
        gateway.reply_to_token(StatusCode::BAD_GATEWAY, "upstream down".into());
        gateway.reply_to_token(StatusCode::OK, grant());
        let tokens = poll(&endpoints, &authorization, Duration::ZERO)
            .await
            .unwrap();
        assert_eq!(tokens.access_token, "a");
        assert_eq!(gateway.token_forms().len(), 2);
    }

    /// RFC 8628 §3.5: each `slow_down` lengthens the interval for the rest of
    /// the flow.
    #[tokio::test]
    async fn slow_down_lengthens_the_interval() {
        let step = Duration::from_millis(40);
        let (gateway, endpoints, authorization) = started().await;
        gateway.refuse_token("slow_down");
        gateway.refuse_token("authorization_pending");
        gateway.reply_to_token(StatusCode::OK, grant());
        let began = tokio::time::Instant::now();
        poll(&endpoints, &authorization, step).await.unwrap();
        assert!(began.elapsed() >= step * 2, "{:?}", began.elapsed());
    }

    #[tokio::test]
    async fn a_declined_or_expired_sign_in_ends_the_wait() {
        for (error, says) in [("access_denied", "declined"), ("expired_token", "expired")] {
            let (gateway, endpoints, authorization) = started().await;
            gateway.refuse_token(error);
            let err = poll(&endpoints, &authorization, Duration::ZERO)
                .await
                .unwrap_err()
                .to_string();
            assert!(err.contains(says), "{error}: {err}");
        }
    }

    #[tokio::test]
    async fn the_code_expires_on_the_engine_clock_too() {
        let gateway = MockGateway::start().await;
        gateway.set_device_field("expires_in", 0.into());
        let endpoints = discover(&gateway.origin).await.unwrap();
        let authorization = start(&endpoints).await.unwrap();
        let err = poll(&endpoints, &authorization, Duration::ZERO)
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("expired"), "{err}");
        assert!(
            gateway.token_forms().is_empty(),
            "an expired code is never sent"
        );
    }
}
