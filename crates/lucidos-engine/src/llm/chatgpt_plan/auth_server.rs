//! OpenAI's auth host, where the sign-in, the token refresh and the `id_token`
//! keys live. A plan token and a refresh token go to no other host.

use std::time::Duration;

type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// OpenAI's auth origin, the only real host the sign-in talks to.
pub const OPENAI_AUTH_ORIGIN: &str = "https://auth.openai.com";

/// The auth origin a sign-in uses: OpenAI's, or a loopback mock in tests.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuthServer {
    origin: String,
}

impl AuthServer {
    pub fn openai() -> Self {
        Self {
            origin: OPENAI_AUTH_ORIGIN.to_string(),
        }
    }

    /// `origin` when it is OpenAI's or a loopback mock, else why not.
    pub fn parse(origin: &str) -> Result<Self, BoxError> {
        let origin = origin.trim().trim_end_matches('/');
        if origin == OPENAI_AUTH_ORIGIN {
            return Ok(Self::openai());
        }
        match reqwest::Url::parse(origin) {
            Ok(url) if super::is_loopback_mock(&url) && url.path() == "/" => Ok(Self {
                origin: url.origin().ascii_serialization(),
            }),
            _ => Err(
                format!("a ChatGPT sign-in goes only to {OPENAI_AUTH_ORIGIN}, not {origin}").into(),
            ),
        }
    }

    pub fn origin(&self) -> &str {
        &self.origin
    }

    pub(super) fn authorize_url(&self) -> String {
        format!("{}/api/accounts/authorize", self.origin)
    }

    pub(super) fn token_url(&self) -> String {
        format!("{}/api/accounts/oauth/token", self.origin)
    }

    pub(super) fn revoke_url(&self) -> String {
        format!("{}/api/accounts/oauth/revoke", self.origin)
    }

    pub(super) fn discovery_url(&self) -> String {
        format!("{}/.well-known/openid-configuration", self.origin)
    }
}

/// The client for every auth host call and the plan's model listing. It
/// follows no redirect, so a `307` cannot resend a code or a token elsewhere.
pub(super) fn http_client() -> &'static reqwest::Client {
    static CLIENT: std::sync::LazyLock<reqwest::Client> = std::sync::LazyLock::new(|| {
        reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .user_agent(crate::llm::lucidos_user_agent())
            .redirect(reqwest::redirect::Policy::none())
            .build()
            // Only fails on TLS-backend init.
            .expect("ChatGPT plan auth HTTP client builder")
    });
    &CLIENT
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_openai_or_a_loopback_mock_is_an_auth_server() {
        assert_eq!(
            AuthServer::parse("https://auth.openai.com/").unwrap(),
            AuthServer::openai()
        );
        assert_eq!(
            AuthServer::parse("http://127.0.0.1:4100").unwrap().origin(),
            "http://127.0.0.1:4100"
        );
        for refused in [
            "https://auth.openai.com.example",
            "http://auth.openai.com",
            "https://auth.example",
            "http://localhost:4100",
            "http://10.0.0.1:4100",
            "http://127.0.0.1:4100/elsewhere",
            "not a url",
        ] {
            assert!(AuthServer::parse(refused).is_err(), "{refused}");
        }
    }
}
