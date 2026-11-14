//! The *ChatGPT plan provider*: the Lucidos Agent on a user's ChatGPT plan,
//! through Sign in with ChatGPT. Decision: ADR 0636.
//!
//! The wire is [`OpenAiProvider`](crate::llm::OpenAiProvider) on its
//! `ChatGptPlan` endpoint. This module owns what that endpoint takes in place
//! of an API key: the sign-in, the stored tokens and their refresh. It also
//! owns the typed failures the endpoint answers with.

pub mod account;
mod auth_server;
mod error;
pub(crate) mod id_token;
pub mod limit_pause;
#[cfg(test)]
pub(crate) mod mock_auth;
pub(crate) mod served_models;
pub mod sign_in;
mod token_endpoint;
pub mod tokens;

pub use account::{ChatGptPlanAccountStore, ChatGptPlanSignOutReason};
pub use auth_server::{AuthServer, OPENAI_AUTH_ORIGIN};
pub use error::{ChatGptPlanError, ChatGptPlanFailure};
pub use limit_pause::LimitPause;
pub use sign_in::PendingSignIn;
pub use tokens::{RefreshLock, StoredChatGptPlanTokens};

/// Whether `url` is a loopback test mock: plain http on an IPv4 loopback
/// address. The only host besides OpenAI's own a plan credential may reach.
pub(crate) fn is_loopback_mock(url: &reqwest::Url) -> bool {
    url.scheme() == "http"
        && url
            .host_str()
            .and_then(|host| host.parse::<std::net::Ipv4Addr>().ok())
            .is_some_and(|ip| ip.is_loopback())
}

/// The namespace every Lucidos function tool rides in on the plan route. The
/// docs require tools in a namespace, and a returned call names it.
pub const TOOL_NAMESPACE: &str = "lucidos";

/// What every plan provider build shares, so a rebuild keeps the refresh
/// lock and the limit pause.
#[derive(Clone)]
pub struct ChatGptPlanAccess {
    pub tokens: std::sync::Arc<dyn ChatGptPlanTokens>,
    pub pause: std::sync::Arc<LimitPause>,
}

/// Where a plan request gets its bearer token. The account store implements
/// it, and refreshes behind it.
#[async_trait::async_trait]
pub trait ChatGptPlanTokens: Send + Sync {
    /// A current access token for the signed-in account.
    async fn access_token(&self) -> Result<String, Box<dyn std::error::Error + Send + Sync>>;

    /// OpenAI rejected `rejected` as a bearer. Returns a fresh token, from a
    /// refresh unless another caller already replaced that token.
    async fn after_rejection(
        &self,
        rejected: &str,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>>;
}
