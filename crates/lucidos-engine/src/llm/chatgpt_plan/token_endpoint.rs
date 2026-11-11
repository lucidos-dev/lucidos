//! OpenAI's token endpoint: the code exchange, the refresh, and revocation.

use chrono::{DateTime, Utc};

use super::auth_server::{http_client, AuthServer};
use crate::core::oauth::TokenResponse;

type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// Longest raw body echoed into an error.
const MAX_ERROR_BODY_CHARS: usize = 500;

/// The refresh error codes that end a sign-in. OpenAI's errors page lists
/// them as "unusable refresh token": clear the tokens and sign in again.
const SIGN_IN_ENDING_REFRESH_CODES: [&str; 6] = [
    "invalid_grant",
    "invalid_refresh_token",
    "token_expired",
    "refresh_token_expired",
    "refresh_token_invalidated",
    "refresh_token_reused",
];

/// Tokens the token endpoint granted.
#[derive(Clone)]
pub(super) struct GrantedTokens {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub id_token: Option<String>,
    pub token_expiry: Option<DateTime<Utc>>,
    /// OpenAI's `earliest_refresh_at`. A refresh waits for it.
    pub refresh_due_at: Option<DateTime<Utc>>,
}

impl std::fmt::Debug for GrantedTokens {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let redacted = |token: &Option<String>| token.as_ref().map(|_| "<redacted>");
        f.debug_struct("GrantedTokens")
            .field("access_token", &"<redacted>")
            .field("refresh_token", &redacted(&self.refresh_token))
            .field("id_token", &redacted(&self.id_token))
            .field("token_expiry", &self.token_expiry)
            .field("refresh_due_at", &self.refresh_due_at)
            .finish()
    }
}

/// An error answer that names a code, in OAuth's shape (`"error": "code"`)
/// or OpenAI's (`"error": {"code": ...}`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct TokenRefusal {
    pub status: u16,
    pub code: String,
    pub description: Option<String>,
}

impl TokenRefusal {
    fn from_body(status: u16, body: &str) -> Option<Self> {
        let value: serde_json::Value = serde_json::from_str(body).ok()?;
        let error = value.get("error")?;
        let text = |v: Option<&serde_json::Value>| {
            v.and_then(serde_json::Value::as_str)
                .map(str::to_string)
                .filter(|s| !s.is_empty())
        };
        let (code, description) = match error.as_str() {
            Some(code) => (code.to_string(), text(value.get("error_description"))),
            None => (text(error.get("code"))?, text(error.get("message"))),
        };
        Some(Self {
            status,
            code,
            description,
        })
    }

    /// Whether a refresh refused with this code can never succeed, so the
    /// tokens are cleared.
    pub fn ends_sign_in(&self) -> bool {
        SIGN_IN_ENDING_REFRESH_CODES.contains(&self.code.as_str())
    }
}

impl std::fmt::Display for TokenRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} ({})", self.code, self.status)?;
        if let Some(description) = &self.description {
            write!(f, ": {description}")?;
        }
        Ok(())
    }
}

/// What the token endpoint answered.
#[derive(Debug)]
pub(super) enum TokenReply {
    Granted(GrantedTokens),
    Refused(TokenRefusal),
}

/// POST `form` to the token endpoint. An answer naming an error code is
/// [`TokenReply::Refused`]. A network failure or an answer with no code is
/// an `Err`: temporary, so nothing is cleared for it.
pub(super) async fn post_token_form(
    auth: &AuthServer,
    form: &[(&str, &str)],
) -> Result<TokenReply, BoxError> {
    let resp = http_client()
        .post(auth.token_url())
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
    TokenRefusal::from_body(status.as_u16(), &body)
        .map(TokenReply::Refused)
        .ok_or_else(|| {
            let body = body.trim();
            let excerpt = &body[..body.floor_char_boundary(MAX_ERROR_BODY_CHARS)];
            format!("the token endpoint answered {status}: {excerpt}").into()
        })
}

fn granted_from(value: serde_json::Value) -> Result<GrantedTokens, BoxError> {
    let text = |key: &str| {
        value[key]
            .as_str()
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    };
    let id_token = text("id_token");
    let refresh_due_at = value["earliest_refresh_at"]
        .as_i64()
        .and_then(|secs| DateTime::from_timestamp(secs, 0));
    let tokens: TokenResponse = serde_json::from_value(value)
        .map_err(|e| format!("the token endpoint's answer is not a token: {e}"))?;
    Ok(GrantedTokens {
        token_expiry: tokens.expiry_from(Utc::now()),
        access_token: tokens.access_token,
        refresh_token: tokens.refresh_token.filter(|t| !t.is_empty()),
        id_token,
        refresh_due_at,
    })
}

/// Revoke `token` at OpenAI, best effort: a sign-out has already forgotten it.
pub(super) async fn revoke(auth: AuthServer, client_id: String, token: String) {
    let form = [
        ("token", token.as_str()),
        ("token_type_hint", "refresh_token"),
        ("client_id", client_id.as_str()),
    ];
    let outcome = http_client()
        .post(auth.revoke_url())
        .form(&form)
        .send()
        .await
        .map(|resp| resp.status());
    match outcome {
        Ok(status) if status.is_success() => {}
        Ok(status) => crate::log!("[ChatGptPlan] Revoking the refresh token answered {status}"),
        Err(e) => crate::log!("[ChatGptPlan] Could not revoke the refresh token: {e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_grant_reads_the_earliest_refresh_time_and_the_id_token() {
        let granted = granted_from(serde_json::json!({
            "access_token": "a",
            "refresh_token": "r",
            "id_token": "i",
            "expires_in": 3600,
            "earliest_refresh_at": 1_900_000_000,
        }))
        .unwrap();
        assert_eq!(granted.refresh_token.as_deref(), Some("r"));
        assert_eq!(granted.id_token.as_deref(), Some("i"));
        assert_eq!(granted.refresh_due_at.unwrap().timestamp(), 1_900_000_000);
        assert!((granted.token_expiry.unwrap() - Utc::now()).num_seconds() > 3500);
    }

    /// The errors page names the codes but not the body shape, so both the
    /// OAuth shape and OpenAI's object shape are read.
    #[test]
    fn a_refusal_reads_its_code_from_either_shape() {
        let oauth = TokenRefusal::from_body(
            400,
            r#"{"error": "invalid_grant", "error_description": "spent"}"#,
        )
        .unwrap();
        assert_eq!(oauth.code, "invalid_grant");
        assert_eq!(oauth.description.as_deref(), Some("spent"));
        let object = TokenRefusal::from_body(
            401,
            r#"{"error": {"code": "refresh_token_reused", "message": "reused"}}"#,
        )
        .unwrap();
        assert_eq!(object.code, "refresh_token_reused");
        assert_eq!(object.description.as_deref(), Some("reused"));
        for no_code in [r#"{"detail": "busy"}"#, "<html>", r#"{"error": {}}"#] {
            assert_eq!(TokenRefusal::from_body(502, no_code), None, "{no_code}");
        }
    }

    #[test]
    fn debug_never_shows_a_token() {
        let granted = GrantedTokens {
            access_token: "secret-access".into(),
            refresh_token: Some("secret-refresh".into()),
            id_token: Some("secret-id".into()),
            token_expiry: None,
            refresh_due_at: None,
        };
        let shown = format!("{granted:?}");
        assert!(!shown.contains("secret"), "{shown}");
    }
}
