pub mod account;
mod auth_server;
pub mod id_token;
pub mod limit_pause;
#[cfg(any(test, feature = "test-support"))]
pub mod mock_auth;
pub(crate) mod served_models;
pub mod sign_in;
mod token_endpoint;
pub mod tokens;

pub use super::chatgpt_plan_error::{ChatGptPlanError, ChatGptPlanFailure};
pub use account::{ChatGptPlanAccountStore, ChatGptPlanSignOutReason};
pub use auth_server::{AuthServer, OPENAI_AUTH_ORIGIN};
pub use limit_pause::LimitPause;
pub use sign_in::PendingSignIn;
pub use tokens::{RefreshLock, StoredChatGptPlanTokens};

/// INVARIANT: Whether `url` is a loopback test mock: plain http on an IPv4 loopback
/// address. The only host besides OpenAI's own a plan credential may reach.
pub(crate) fn is_loopback_mock(url: &reqwest::Url) -> bool {
    url.scheme() == "http"
        && url
            .host_str()
            .and_then(|host| host.parse::<std::net::Ipv4Addr>().ok())
            .is_some_and(|ip| ip.is_loopback())
}

pub const TOOL_NAMESPACE: &str = "lucidos";

/// INVARIANT: What every plan provider build shares, so a rebuild keeps the refresh
/// lock and the limit pause.
#[derive(Clone)]
pub struct ChatGptPlanAccess {
    pub tokens: std::sync::Arc<dyn ChatGptPlanTokens>,
    pub pause: std::sync::Arc<LimitPause>,
}

#[async_trait::async_trait]
pub trait ChatGptPlanTokens: Send + Sync {
    async fn access_token(&self) -> Result<String, Box<dyn std::error::Error + Send + Sync>>;

    async fn after_rejection(
        &self,
        rejected: &str,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>>;
}
