use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::PgPool;
use std::sync::Arc;

use crate::core::{oauth_callback::CallbackRoute, prefs};
use crate::llm::provider_factory::{
    ChatGptPlanAccountInfo, ClaudeGatewayAccountInfo, GatewayOrigin, ProviderBuildContext,
    ProviderFactory, SignOutOutcome,
};

use super::{event_bus::EventBus, thread_events::MessageOrigin};

type BoxError = Box<dyn std::error::Error + Send + Sync>;

#[async_trait]
pub trait WorkspaceProviders: ProviderFactory {
    fn bind_workspace(&self, pool: &PgPool, bus: &EventBus, context: &mut ProviderBuildContext);
    fn start_gateway_watchers(&self, pool: &PgPool, bus: &EventBus, context: &ProviderBuildContext);
    async fn start_gateway_telemetry(
        &self,
        pool: &PgPool,
        bus: &EventBus,
        context: &ProviderBuildContext,
    );

    async fn claude_gateway_status(
        &self,
        pool: &PgPool,
    ) -> Result<ClaudeGatewayStatus, sqlx::Error>;
    async fn claude_gateway_sign_in(
        &self,
        pool: &PgPool,
        bus: &EventBus,
        origin: GatewayOrigin,
        actor: Option<MessageOrigin>,
    ) -> Result<ClaudeGatewaySignInState, BoxError>;
    async fn claude_gateway_sign_out(
        &self,
        pool: &PgPool,
        bus: &EventBus,
        actor: Option<MessageOrigin>,
    ) -> Result<bool, sqlx::Error>;

    async fn chatgpt_plan_status(
        &self,
        pool: &PgPool,
        context: &ProviderBuildContext,
    ) -> Result<ChatGptPlanStatus, sqlx::Error>;
    async fn chatgpt_plan_sign_in(
        &self,
        pool: &PgPool,
        bus: &EventBus,
        workspace: &str,
        route: CallbackRoute,
        actor: Option<MessageOrigin>,
    ) -> Result<StartedSignIn, BoxError>;
    async fn chatgpt_plan_sign_out(
        &self,
        pool: &PgPool,
        bus: &EventBus,
        forget_account: bool,
        actor: Option<MessageOrigin>,
    ) -> Result<SignOutOutcome, BoxError>;

    async fn gateway_has_matching_account(
        &self,
        pool: &PgPool,
        origin: &GatewayOrigin,
    ) -> Result<bool, sqlx::Error>;
    fn gateway_proxy_access(
        &self,
        context: &ProviderBuildContext,
        origin: GatewayOrigin,
    ) -> Arc<dyn GatewayProxyAccess>;
}

#[async_trait]
pub trait GatewayProxyAccess: Send + Sync {
    async fn renew_rejected(&self, rejected: &str) -> Result<(), BoxError>;
    async fn wire_id_and_bearer(&self, model: &str) -> Result<(String, String), BoxError>;
}

/// INVARIANT: Where the current sign-in stands. Carries what the user must see, never a
/// token or the device code.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "kebab-case")]
pub enum ClaudeGatewaySignInState {
    Idle,
    Pending {
        issuer: String,
        user_code: String,
        verification_uri: String,
        verification_uri_complete: Option<String>,
        expires_at: DateTime<Utc>,
    },
    Failed {
        issuer: String,
        error: String,
    },
}

#[derive(Debug, Serialize)]
pub struct ClaudeGatewayStatus {
    pub url: Option<String>,
    /// INVARIANT: The gateway Claude Code's managed settings name on this machine, offered
    /// when no URL is set. Never signed in to without the user.
    pub suggested_url: Option<String>,
    pub account: Option<ClaudeGatewayAccountInfo>,
    pub sign_in_again: bool,
    pub telemetry: bool,
    pub sign_in: ClaudeGatewaySignInState,
}

/// Where the current attempt stands. Carries what the user must see, never
/// the authorize URL, its `state` or a token.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "kebab-case")]
pub enum ChatGptPlanSignInState {
    Idle,
    /// Waiting for the browser to come back from ChatGPT.
    Pending {
        expires_at: DateTime<Utc>,
    },
    /// The last attempt ended without a sign-in. Kept until the next starts.
    Failed {
        error: String,
    },
}

/// A started attempt: the page the browser must open.
#[derive(Debug, Serialize)]
pub struct StartedSignIn {
    pub authorize_url: String,
    pub expires_at: DateTime<Utc>,
}

/// The stored sign-in, the attempt in progress and the limit pause. Never a
/// token.
#[derive(Debug, Serialize)]
pub struct ChatGptPlanStatus {
    pub account: Option<ChatGptPlanAccountInfo>,
    /// OpenAI refused the last sign-in for good, and none has been made since.
    pub sign_in_again: bool,
    /// A registration from an earlier sign-in is kept, so only that account
    /// may sign in until the user forgets it.
    pub registered: bool,
    pub sign_in: ChatGptPlanSignInState,
    /// When plan requests resume after the usage limit, while they are paused.
    pub paused_until: Option<DateTime<Utc>>,
    /// How long the pause has left, by the engine's clock. A client times its
    /// re-read from this, since its own clock may differ.
    pub pause_remaining_ms: Option<i64>,
}

pub async fn configured_origin(pool: &PgPool) -> Result<GatewayOrigin, BoxError> {
    let url = prefs::CLAUDE_GATEWAY_URL
        .try_read(pool)
        .await?
        .ok_or_else(|| {
            format!(
                "No Claude gateway URL is set. Set the '{}' preference first.",
                prefs::CLAUDE_GATEWAY_URL.key()
            )
        })?;
    Ok(GatewayOrigin::parse(&url)?)
}
