pub mod account;
mod auth_server;
pub mod id_token;
pub use super::chatgpt_plan_pause as limit_pause;
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

pub use super::provider_factory::{ChatGptPlanAccess, ChatGptPlanTokens};
