//! The HTTP side of the gateway's OAuth endpoints: one client, and one reader
//! of a token endpoint's answer, shared by the device flow and the refresh.

use std::time::Duration;

use base64::Engine as _;
use chrono::{DateTime, Utc};
use serde::Deserialize;

use crate::core::oauth::TokenResponse;

type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// Longest raw body echoed into an error. A proxy in front of the gateway can
/// answer with a whole HTML page.
const MAX_ERROR_BODY_CHARS: usize = 500;

/// The client for every gateway call but the Messages stream. It names Lucidos
/// in `User-Agent`, and it follows no redirect: a `307` would resend a token
/// to whichever host the redirect named.
pub(super) fn http_client() -> &'static reqwest::Client {
    static CLIENT: std::sync::LazyLock<reqwest::Client> = std::sync::LazyLock::new(|| {
        reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .user_agent(crate::llm::lucidos_user_agent())
            .redirect(reqwest::redirect::Policy::none())
            .build()
            // Only fails on TLS-backend init.
            .expect("Claude gateway HTTP client builder")
    });
    &CLIENT
}

/// Tokens the gateway granted, as Lucidos stores them.
#[derive(Clone)]
pub struct GrantedTokens {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub token_expiry: Option<DateTime<Utc>>,
    /// The account that approved the sign-in, shown in Settings and never used
    /// to decide anything. The token response's own `email` extension, else
    /// the ID token's claim, else the access token's.
    pub email: Option<String>,
}

impl std::fmt::Debug for GrantedTokens {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GrantedTokens")
            .field("access_token", &"<redacted>")
            .field(
                "refresh_token",
                &self.refresh_token.as_ref().map(|_| "<redacted>"),
            )
            .field("token_expiry", &self.token_expiry)
            .field("email", &self.email)
            .finish()
    }
}

/// An OAuth error answer (RFC 6749 §5.2), such as `authorization_pending`.
#[derive(Debug, Deserialize)]
pub(super) struct OAuthRefusal {
    pub error: String,
    pub error_description: Option<String>,
}

impl std::fmt::Display for OAuthRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.error_description {
            Some(description) => write!(f, "{}: {description}", self.error),
            None => f.write_str(&self.error),
        }
    }
}

/// What a token endpoint answered.
#[derive(Debug)]
pub(super) enum TokenReply {
    Granted(GrantedTokens),
    Refused(OAuthRefusal),
}

/// POST a form to a token endpoint. An OAuth error comes back as
/// [`TokenReply::Refused`], so the caller can act on its code. Anything else
/// that is not a grant is an `Err`.
pub(super) async fn post_token_form(
    token_url: &str,
    form: &[(&str, &str)],
) -> Result<TokenReply, BoxError> {
    let resp = http_client()
        .post(token_url)
        .header("Accept", "application/json")
        .form(form)
        .send()
        .await?;
    let status = resp.status();
    let body = resp.text().await?;
    if status.is_success() {
        let value: serde_json::Value = serde_json::from_str(&body)
            .map_err(|e| format!("the token endpoint answered {status} with no JSON: {e}"))?;
        return Ok(TokenReply::Granted(granted_from(value)?));
    }
    match serde_json::from_str::<OAuthRefusal>(&body) {
        Ok(refusal) => Ok(TokenReply::Refused(refusal)),
        Err(_) => Err(format!("the token endpoint answered {status}: {}", excerpt(&body)).into()),
    }
}

/// The start of a raw error body, short enough to echo into an error.
pub(super) fn excerpt(body: &str) -> &str {
    let body = body.trim();
    &body[..body.floor_char_boundary(MAX_ERROR_BODY_CHARS)]
}

fn granted_from(value: serde_json::Value) -> Result<GrantedTokens, BoxError> {
    let email = value["email"]
        .as_str()
        .map(str::trim)
        .filter(|email| !email.is_empty())
        .map(str::to_string)
        .or_else(|| value["id_token"].as_str().and_then(jwt_email))
        .or_else(|| value["access_token"].as_str().and_then(jwt_email));
    let tokens: TokenResponse = serde_json::from_value(value)
        .map_err(|e| format!("the token endpoint's answer is not a token: {e}"))?;
    Ok(GrantedTokens {
        token_expiry: tokens.expiry_from(Utc::now()),
        access_token: tokens.access_token,
        refresh_token: tokens.refresh_token.filter(|t| !t.is_empty()),
        email,
    })
}

/// The `email` claim of a token the gateway issued, ID or access. The signature
/// is not checked: the token came straight from the token endpoint over TLS
/// (OIDC Core §3.1.3.7), and the claim is only displayed.
pub(super) fn jwt_email(token: &str) -> Option<String> {
    jwt_claims(token)?["email"]
        .as_str()
        .map(str::trim)
        .filter(|email| !email.is_empty())
        .map(str::to_string)
}

/// The claims of a JWT, unverified, or `None` for a token that is not one.
/// Only for reading what the gateway itself issued over TLS.
pub(super) fn jwt_claims(token: &str) -> Option<serde_json::Value> {
    let payload = token.split('.').nth(1)?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload.trim_end_matches('='))
        .ok()?;
    serde_json::from_slice::<serde_json::Value>(&bytes)
        .ok()
        .filter(serde_json::Value::is_object)
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;

    /// An unsigned ID token carrying `claims`, as a test gateway issues it.
    pub(crate) fn id_token(claims: serde_json::Value) -> String {
        let encode = |v: &serde_json::Value| {
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(v.to_string())
        };
        format!(
            "{}.{}.sig",
            encode(&serde_json::json!({"alg": "none"})),
            encode(&claims)
        )
    }

    #[test]
    fn a_grant_reads_its_expiry_and_the_id_token_email() {
        let granted = granted_from(serde_json::json!({
            "access_token": "a",
            "refresh_token": "r",
            "expires_in": 3600,
            "id_token": id_token(serde_json::json!({"email": "user@example.com"})),
        }))
        .unwrap();
        assert_eq!(granted.access_token, "a");
        assert_eq!(granted.refresh_token.as_deref(), Some("r"));
        assert_eq!(granted.email.as_deref(), Some("user@example.com"));
        let left = granted.token_expiry.unwrap() - Utc::now();
        assert!(left.num_seconds() > 3500, "{left}");
    }

    /// The protocol's `email` extension names the approving account, ahead of
    /// the ID token. A blank one is no answer.
    #[test]
    fn the_email_extension_wins_over_the_id_token() {
        let id_token = id_token(serde_json::json!({"email": "claim@example.com"}));
        let granted = granted_from(serde_json::json!({
            "access_token": "a",
            "email": "approver@example.com",
            "id_token": id_token,
        }))
        .unwrap();
        assert_eq!(granted.email.as_deref(), Some("approver@example.com"));
        let blank = granted_from(serde_json::json!({
            "access_token": "a",
            "email": "  ",
            "id_token": id_token,
        }))
        .unwrap();
        assert_eq!(blank.email.as_deref(), Some("claim@example.com"));
    }

    /// A gateway that sends neither the extension nor an ID token may still
    /// name the account in its access token, as telemetry reads it.
    #[test]
    fn the_access_token_names_the_account_last() {
        let access = id_token(serde_json::json!({"email": "access@example.com"}));
        let granted = granted_from(serde_json::json!({"access_token": access})).unwrap();
        assert_eq!(granted.email.as_deref(), Some("access@example.com"));
        let with_id = granted_from(serde_json::json!({
            "access_token": access,
            "id_token": id_token(serde_json::json!({"email": "claim@example.com"})),
        }))
        .unwrap();
        assert_eq!(with_id.email.as_deref(), Some("claim@example.com"));
    }

    /// An empty refresh token is not a token, so a refresh keeps the stored
    /// one rather than erasing it.
    #[test]
    fn an_empty_refresh_token_is_none() {
        let granted =
            granted_from(serde_json::json!({"access_token": "a", "refresh_token": ""})).unwrap();
        assert!(granted.refresh_token.is_none());
        assert!(granted.token_expiry.is_none());
        assert!(granted.email.is_none());
    }

    #[test]
    fn a_malformed_or_blank_token_gives_no_email() {
        let blank = id_token(serde_json::json!({"email": " "}));
        for token in ["", "one-part", "a.!!!.c", "a.e30.c", &blank] {
            assert_eq!(jwt_email(token), None, "{token}");
        }
    }

    #[test]
    fn debug_never_shows_a_token() {
        let granted = GrantedTokens {
            access_token: "secret-access".into(),
            refresh_token: Some("secret-refresh".into()),
            token_expiry: None,
            email: None,
        };
        let shown = format!("{granted:?}");
        assert!(!shown.contains("secret"), "{shown}");
    }
}
