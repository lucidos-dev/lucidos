//! Token revocation at sign-out (RFC 7009), best effort.
//!
//! A sign-out forgets the tokens first and revokes after, so a gateway that
//! cannot be reached never keeps the user signed in.

use super::discovery::discover;
use super::token_endpoint::{excerpt, http_client};
use super::{GatewayOrigin, GrantedTokens};

/// The tokens of a sign-in that has just ended, to revoke at their issuer.
pub(crate) struct EndedSignIn {
    pub issuer: String,
    pub access_token: String,
    pub refresh_token: Option<String>,
}

impl EndedSignIn {
    /// Tokens the gateway granted that no stored sign-in kept, because a
    /// sign-out or a URL change won the race to the row.
    pub(crate) fn unkept(issuer: &GatewayOrigin, tokens: &GrantedTokens) -> Self {
        Self {
            issuer: issuer.to_string(),
            access_token: tokens.access_token.clone(),
            refresh_token: tokens.refresh_token.clone(),
        }
    }
}

/// Revoke `ended`'s tokens at its issuer, when the issuer's discovery
/// document names a revocation endpoint. Every failure is logged and dropped.
pub(crate) async fn revoke(ended: EndedSignIn) {
    let Ok(origin) = GatewayOrigin::parse(&ended.issuer) else {
        return;
    };
    let endpoint = match discover(&origin).await {
        Ok(endpoints) => endpoints.revocation,
        Err(e) => {
            crate::log!("[ClaudeGateway] Could not revoke the sign-in at {origin}: {e}");
            return;
        }
    };
    let Some(endpoint) = endpoint else {
        return;
    };
    post(&endpoint, &[("token", ended.access_token.as_str())]).await;
    if let Some(refresh_token) = &ended.refresh_token {
        post(
            &endpoint,
            &[
                ("token", refresh_token.as_str()),
                ("token_type_hint", "refresh_token"),
            ],
        )
        .await;
    }
}

async fn post(endpoint: &str, form: &[(&str, &str)]) {
    match http_client().post(endpoint).form(form).send().await {
        Ok(resp) if resp.status().is_success() => {}
        Ok(resp) => {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            crate::log!(
                "[ClaudeGateway] {endpoint} answered {status} to a revocation: {}",
                excerpt(&body)
            );
        }
        Err(e) => crate::log!("[ClaudeGateway] Could not reach {endpoint} to revoke: {e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::claude_gateway::mock_gateway::MockGateway;

    fn ended(gateway: &MockGateway, refresh_token: Option<&str>) -> EndedSignIn {
        EndedSignIn {
            issuer: gateway.origin.to_string(),
            access_token: "gw-access".into(),
            refresh_token: refresh_token.map(str::to_string),
        }
    }

    /// The bearer first, then the refresh token with its hint, each from a
    /// public client with the Lucidos `User-Agent`.
    #[tokio::test]
    async fn sign_out_revokes_the_bearer_then_the_refresh_token() {
        let gateway = MockGateway::start().await;
        revoke(ended(&gateway, Some("gw-refresh"))).await;
        let revoked: Vec<_> = gateway
            .seen()
            .into_iter()
            .filter(|s| s.path == "/oauth/revoke")
            .collect();
        assert_eq!(revoked.len(), 2);
        assert_eq!(revoked[0].form["token"], "gw-access");
        assert!(!revoked[0].form.contains_key("token_type_hint"));
        assert_eq!(revoked[1].form["token"], "gw-refresh");
        assert_eq!(revoked[1].form["token_type_hint"], "refresh_token");
        assert!(revoked.iter().all(|s| !s.form.contains_key("client_id")
            && s.user_agent
                .as_deref()
                .is_some_and(|ua| ua.starts_with("Lucidos/"))));
    }

    #[tokio::test]
    async fn no_refresh_token_revokes_the_bearer_alone() {
        let gateway = MockGateway::start().await;
        revoke(ended(&gateway, None)).await;
        assert_eq!(
            gateway
                .paths()
                .iter()
                .filter(|p| *p == "/oauth/revoke")
                .count(),
            1
        );
    }

    /// An unreachable gateway is logged, never raised.
    #[tokio::test]
    async fn an_unreachable_gateway_is_only_logged() {
        revoke(EndedSignIn {
            issuer: "http://127.0.0.1:9".into(),
            access_token: "gw-access".into(),
            refresh_token: None,
        })
        .await;
    }
}
