//! The stored sign-in as a [`ChatGptPlanTokens`]: it hands out the access
//! token and refreshes it, one refresh at a time per engine.

use std::sync::Arc;

use sqlx::PgPool;

use super::account::{ChatGptPlanAccount, ChatGptPlanAccountStore};
use super::auth_server::AuthServer;
use super::token_endpoint::{post_token_form, revoke, TokenReply};
use super::ChatGptPlanTokens;
use crate::engine::event_bus::EventBus;

const SIGN_IN_LOCATION: &str = providers_settings!();

type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// The lock every [`StoredChatGptPlanTokens`] of one engine shares.
pub type RefreshLock = tokio::sync::Mutex<()>;

/// The token source behind the plan provider.
pub struct StoredChatGptPlanTokens {
    pool: PgPool,
    event_bus: EventBus,
    auth: AuthServer,
    /// Serialises refreshes. OpenAI rotates the refresh token and refuses a
    /// spent one as `refresh_token_reused`, which ends the sign-in. A provider
    /// rebuild makes a new source while turns on the old one still run, so
    /// every source shares this one.
    refresh_lock: Arc<RefreshLock>,
}

impl StoredChatGptPlanTokens {
    pub fn new(
        pool: PgPool,
        event_bus: EventBus,
        auth: AuthServer,
        refresh_lock: Arc<RefreshLock>,
    ) -> Self {
        Self {
            pool,
            event_bus,
            auth,
            refresh_lock,
        }
    }

    async fn account(&self) -> Result<ChatGptPlanAccount, BoxError> {
        ChatGptPlanAccountStore::get(&self.pool)
            .await
            .map_err(|e| format!("Could not read the ChatGPT sign-in: {e}"))?
            .ok_or_else(not_signed_in)
    }

    /// Spend the refresh token once. The caller holds the refresh lock.
    async fn refresh(&self, account: ChatGptPlanAccount) -> Result<String, BoxError> {
        let form = [
            ("grant_type", "refresh_token"),
            ("client_id", account.client_id.as_str()),
            ("refresh_token", account.refresh_token.as_str()),
        ];
        let refusal = match post_token_form(&self.auth, &form).await {
            Ok(TokenReply::Granted(tokens)) => {
                let stored = ChatGptPlanAccountStore::update_tokens(
                    &self.pool,
                    &account.refresh_token,
                    &tokens,
                )
                .await;
                if !matches!(stored, Ok(true)) {
                    // A sign-out or another sign-in won the race, or the write
                    // failed. The rotated grant must not stay live at OpenAI.
                    if let Some(rotated) = tokens.refresh_token {
                        tokio::spawn(revoke(self.auth.clone(), account.client_id, rotated));
                    }
                }
                if !stored
                    .map_err(|e| format!("Could not store the renewed ChatGPT sign-in: {e}"))?
                {
                    return Err(not_signed_in());
                }
                crate::log!("[ChatGptPlan] Refreshed the token");
                return Ok(tokens.access_token);
            }
            Ok(TokenReply::Refused(refusal)) => refusal,
            Err(e) => {
                return Err(
                    format!("Could not renew the ChatGPT sign-in: {e}. Try again shortly.").into(),
                )
            }
        };
        crate::log!("[ChatGptPlan] OpenAI refused the token refresh: {refusal}");
        if !refusal.ends_sign_in() {
            return Err(format!(
                "OpenAI refused to renew the ChatGPT sign-in ({refusal}). Try again, or sign in \
                 again in {SIGN_IN_LOCATION}."
            )
            .into());
        }
        if let Err(e) = ChatGptPlanAccountStore::forget_rejected(
            &self.pool,
            &self.event_bus,
            &account.refresh_token,
        )
        .await
        {
            crate::log!("[ChatGptPlan] Could not clear the refused sign-in: {e}");
        }
        Err(sign_in_again())
    }
}

#[async_trait::async_trait]
impl ChatGptPlanTokens for StoredChatGptPlanTokens {
    async fn access_token(&self) -> Result<String, BoxError> {
        let account = self.account().await?;
        if !account.refresh_due(chrono::Utc::now()) {
            return Ok(account.access_token);
        }
        let _refreshing = self.refresh_lock.lock().await;
        // Another turn may have refreshed while this one waited.
        let account = self.account().await?;
        if !account.refresh_due(chrono::Utc::now()) {
            return Ok(account.access_token);
        }
        self.refresh(account).await
    }

    async fn after_rejection(&self, rejected: &str) -> Result<String, BoxError> {
        let _refreshing = self.refresh_lock.lock().await;
        let account = self.account().await?;
        // Another turn may have renewed it while this one waited.
        if account.access_token != rejected {
            return Ok(account.access_token);
        }
        self.refresh(account).await
    }
}

fn not_signed_in() -> BoxError {
    format!("Lucidos is not signed in with ChatGPT. Sign in in {SIGN_IN_LOCATION}.").into()
}

fn sign_in_again() -> BoxError {
    format!("The ChatGPT sign-in has ended. Sign in with ChatGPT again in {SIGN_IN_LOCATION}.")
        .into()
}

#[cfg(test)]
#[path = "tokens_tests.rs"]
mod tests;
