//! Signing in to the Claude gateway, for the HTTP API and the CLI over it.
//!
//! One sign-in runs at a time, in memory only. Nothing is written until the
//! gateway grants tokens, so an engine restart mid-sign-in means starting
//! again. A change of `claude_gateway_url` signs the user out of the old
//! gateway, whoever wrote it.

use std::sync::{Arc, Mutex};

use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::PgPool;

use crate::core::prefs;
use crate::engine::event_bus::{BusEvent, EmittedEvent, EventBus, SystemEvent};
use crate::engine::thread_events::MessageOrigin;
use crate::llm::claude_gateway::account::{
    ClaudeGatewayAccount, ClaudeGatewayAccountInfo, ClaudeGatewayAccountStore,
    ClaudeGatewaySignOutReason,
};
use crate::llm::claude_gateway::revocation::{self, EndedSignIn};
use crate::llm::claude_gateway::telemetry::switch;
use crate::llm::claude_gateway::{
    device_flow, discovery, machine_managed_settings, GatewayOrigin, GrantedTokens,
};

/// Where the current sign-in stands. Carries what the user must see, never a
/// token or the device code.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "kebab-case")]
pub enum SignInState {
    Idle,
    /// Waiting for the user to approve `user_code` on the verification page.
    Pending {
        issuer: String,
        user_code: String,
        verification_uri: String,
        verification_uri_complete: Option<String>,
        expires_at: DateTime<Utc>,
    },
    /// The last sign-in ended without tokens. Kept until the next one starts.
    Failed {
        issuer: String,
        error: String,
    },
}

/// The configured URL, the stored sign-in and the sign-in in progress.
#[derive(Debug, Serialize)]
pub struct ClaudeGatewayStatus {
    pub url: Option<String>,
    /// The gateway Claude Code's managed settings name on this machine, offered
    /// when no URL is set. Never signed in to without the user.
    pub suggested_url: Option<String>,
    pub account: Option<ClaudeGatewayAccountInfo>,
    /// The gateway rejected the last sign-in, and none has been made since.
    pub sign_in_again: bool,
    /// The signed-in gateway's managed settings turn usage telemetry on, and
    /// the engine's environment does not opt out.
    pub telemetry: bool,
    pub sign_in: SignInState,
}

/// Whether usage telemetry goes out for `account`, under its gateway's policy
/// and the engine environment `engine_env`.
fn telemetry_on(account: &ClaudeGatewayAccount, engine_env: switch::EngineEnv) -> bool {
    GatewayOrigin::parse(&account.issuer).is_ok_and(|origin| {
        switch::target(&origin, account.managed_settings().as_ref(), engine_env).is_ok()
    })
}

/// The engine's one sign-in slot. Each start bumps `generation`, so a
/// superseded flow can never overwrite the state of the flow that replaced it.
///
/// `store` fences storing granted tokens against a sign-out. Aborting a flow
/// takes effect only at its next await, so without the fence a store already
/// sent could commit after the sign-out's delete.
#[derive(Default)]
pub struct ClaudeGatewaySignIn {
    inner: Mutex<Slot>,
    store: tokio::sync::Mutex<()>,
}

struct Slot {
    state: SignInState,
    generation: u64,
    task: Option<tokio::task::AbortHandle>,
}

impl Default for Slot {
    fn default() -> Self {
        Self {
            state: SignInState::Idle,
            generation: 0,
            task: None,
        }
    }
}

type BoxError = Box<dyn std::error::Error + Send + Sync>;

impl ClaudeGatewaySignIn {
    pub fn state(&self) -> SignInState {
        self.lock().state.clone()
    }

    /// Start a sign-in to `origin`, superseding any running one. Returns the
    /// pending state, which holds the code the user must enter.
    pub async fn start(
        self: &Arc<Self>,
        pool: &PgPool,
        event_bus: &EventBus,
        origin: GatewayOrigin,
        actor: Option<MessageOrigin>,
    ) -> Result<SignInState, BoxError> {
        let begun = self.lock().generation;
        let endpoints = discovery::discover(&origin).await?;
        let authorization = device_flow::start(&endpoints).await?;
        let pending = SignInState::Pending {
            issuer: origin.to_string(),
            user_code: authorization.user_code.clone(),
            verification_uri: authorization.verification_uri.clone(),
            verification_uri_complete: authorization.verification_uri_complete.clone(),
            expires_at: Utc::now()
                + chrono::Duration::from_std(authorization.expires_in).unwrap_or_default(),
        };

        {
            let mut slot = self.lock();
            if slot.generation != begun {
                return Err(format!(
                    "A sign-out or another sign-in replaced the sign-in to {origin} before it began."
                )
                .into());
            }
            if let Some(task) = slot.task.take() {
                task.abort();
            }
            slot.generation += 1;
            slot.state = pending.clone();
            let generation = slot.generation;
            let (this, pool, event_bus, origin, actor) = (
                Arc::clone(self),
                pool.clone(),
                event_bus.clone(),
                origin.clone(),
                actor.clone(),
            );
            let task = tokio::spawn(async move {
                this.run(
                    generation,
                    &pool,
                    &event_bus,
                    &origin,
                    endpoints,
                    authorization,
                    actor,
                )
                .await;
            });
            slot.task = Some(task.abort_handle());
        }
        event_bus
            .emit_or_log(
                BusEvent::System(SystemEvent::ClaudeGatewaySignInStarted {
                    issuer: origin.to_string(),
                    actor,
                }),
                "[ClaudeGateway] ClaudeGatewaySignInStarted",
            )
            .await;
        Ok(pending)
    }

    /// Wait for the user to approve the sign-in of `generation`, then store
    /// its tokens unless a sign-out or a newer sign-in replaced it.
    #[allow(clippy::too_many_arguments)]
    async fn run(
        &self,
        generation: u64,
        pool: &PgPool,
        event_bus: &EventBus,
        origin: &GatewayOrigin,
        endpoints: discovery::GatewayEndpoints,
        authorization: device_flow::DeviceAuthorization,
        actor: Option<MessageOrigin>,
    ) {
        let outcome = match device_flow::wait_for_tokens(&endpoints, &authorization).await {
            Ok(tokens) => {
                let _store = self.store.lock().await;
                if self.lock().generation == generation {
                    store_sign_in(pool, event_bus, origin, &tokens, actor.clone()).await
                } else {
                    tokio::spawn(revocation::revoke(EndedSignIn::unkept(origin, &tokens)));
                    Err("A sign-out or a newer sign-in replaced this one.".to_string())
                }
            }
            Err(e) => Err(e.to_string()),
        };
        if let Err(error) = &outcome {
            crate::log!("[ClaudeGateway] Sign-in to {} ended: {}", origin, error);
        }
        if let Some(error) = self.finish(generation, origin, outcome) {
            event_bus
                .emit_or_log(
                    BusEvent::System(SystemEvent::ClaudeGatewaySignInFailed {
                        issuer: origin.to_string(),
                        error,
                        actor,
                    }),
                    "[ClaudeGateway] ClaudeGatewaySignInFailed",
                )
                .await;
        }
    }

    /// Stop any sign-in in progress and forget the stored one. Returns whether
    /// there was a stored sign-in.
    pub async fn sign_out(
        &self,
        pool: &PgPool,
        event_bus: &EventBus,
        actor: Option<MessageOrigin>,
    ) -> Result<bool, sqlx::Error> {
        let _store = self.store.lock().await;
        let stopped = {
            let mut slot = self.lock();
            if let Some(task) = slot.task.take() {
                task.abort();
            }
            slot.generation += 1;
            std::mem::replace(&mut slot.state, SignInState::Idle)
        };
        if let SignInState::Pending { issuer, .. } = stopped {
            event_bus
                .emit_or_log(
                    BusEvent::System(SystemEvent::ClaudeGatewaySignInCanceled {
                        issuer,
                        actor: actor.clone(),
                    }),
                    "[ClaudeGateway] ClaudeGatewaySignInCanceled",
                )
                .await;
        }
        ClaudeGatewayAccountStore::sign_out(
            pool,
            event_bus,
            ClaudeGatewaySignOutReason::SignedOut,
            actor,
        )
        .await
    }

    pub async fn status(&self, pool: &PgPool) -> Result<ClaudeGatewayStatus, sqlx::Error> {
        let stored = ClaudeGatewayAccountStore::get(pool).await?;
        let telemetry = stored
            .as_ref()
            .is_some_and(|account| telemetry_on(account, switch::process_env));
        let account = stored.as_ref().map(ClaudeGatewayAccountInfo::from);
        let sign_in_again =
            account.is_none() && ClaudeGatewayAccountStore::rejected_last(pool).await?;
        Ok(ClaudeGatewayStatus {
            url: prefs::CLAUDE_GATEWAY_URL.try_read(pool).await?,
            suggested_url: machine_managed_settings::suggested_url(),
            account,
            sign_in_again,
            telemetry,
            sign_in: self.state(),
        })
    }

    /// Record how the sign-in of `generation` ended, unless a newer one
    /// replaced it. Returns the error to announce, for a current failure only.
    fn finish(
        &self,
        generation: u64,
        origin: &GatewayOrigin,
        outcome: Result<(), String>,
    ) -> Option<String> {
        let mut slot = self.lock();
        if slot.generation != generation {
            return None;
        }
        slot.task = None;
        let (state, failure) = match outcome {
            Ok(()) => (SignInState::Idle, None),
            Err(error) => (
                SignInState::Failed {
                    issuer: origin.to_string(),
                    error: error.clone(),
                },
                Some(error),
            ),
        };
        slot.state = state;
        failure
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Slot> {
        // The slot holds no invariant a panic could break halfway.
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// The gateway `claude_gateway_url` names, or why there is none.
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

/// Store granted tokens, unless the URL moved to another gateway meanwhile.
/// The tokens belong to `origin` only.
async fn store_sign_in(
    pool: &PgPool,
    event_bus: &EventBus,
    origin: &GatewayOrigin,
    tokens: &GrantedTokens,
    actor: Option<MessageOrigin>,
) -> Result<(), String> {
    match configured_origin(pool).await {
        Ok(configured) if configured == *origin => {}
        _ => {
            tokio::spawn(revocation::revoke(EndedSignIn::unkept(origin, tokens)));
            return Err(format!(
                "The Claude gateway URL changed during the sign-in to {origin}. Sign in again."
            ));
        }
    }
    ClaudeGatewayAccountStore::sign_in(pool, event_bus, origin, tokens, actor)
        .await
        .map_err(|e| format!("Could not store the Claude gateway sign-in: {e}"))
}

/// Sign out of the stored gateway when `claude_gateway_url` stops naming it.
///
/// A subscriber rather than a step in the preference write, so every writer
/// is covered, the reset to default included.
pub(crate) fn spawn_url_change_sign_out(
    mut rx: tokio::sync::broadcast::Receiver<EmittedEvent>,
    pool: PgPool,
    event_bus: EventBus,
) {
    tokio::spawn(async move {
        loop {
            let (value, actor) = match rx.recv().await {
                Ok(emitted) => match emitted.typed {
                    BusEvent::System(SystemEvent::PreferencesChanged { key, value, actor })
                        if key == prefs::CLAUDE_GATEWAY_URL.key() =>
                    {
                        (value, actor)
                    }
                    _ => continue,
                },
                Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                    crate::log!(
                        "[ClaudeGateway] URL watch lagged by {} events; checking the sign-in",
                        n
                    );
                    // An unreadable URL is unknown, not unset, so it signs nobody out.
                    match prefs::CLAUDE_GATEWAY_URL.try_read(&pool).await {
                        Ok(url) => (url, None),
                        Err(e) => {
                            crate::log!("[ClaudeGateway] URL watch could not read the URL: {}", e);
                            continue;
                        }
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            };
            sign_out_unless_issued_by(&pool, &event_bus, value.as_deref(), actor).await;
        }
    });
}

async fn sign_out_unless_issued_by(
    pool: &PgPool,
    event_bus: &EventBus,
    url: Option<&str>,
    actor: Option<MessageOrigin>,
) {
    let account = match ClaudeGatewayAccountStore::get(pool).await {
        Ok(Some(account)) => account,
        Ok(None) => return,
        Err(e) => {
            crate::log!(
                "[ClaudeGateway] URL watch could not read the sign-in: {}",
                e
            );
            return;
        }
    };
    let configured = url.and_then(|url| GatewayOrigin::parse(url).ok());
    if configured.is_some_and(|origin| origin.as_str() == account.issuer) {
        return;
    }
    crate::log!(
        "[ClaudeGateway] The gateway URL no longer names {}; signing out",
        account.issuer
    );
    let signed_out = ClaudeGatewayAccountStore::sign_out(
        pool,
        event_bus,
        ClaudeGatewaySignOutReason::UrlChanged,
        actor,
    )
    .await;
    if let Err(e) = signed_out {
        crate::log!("[ClaudeGateway] Sign-out after a URL change failed: {}", e);
    }
}

#[cfg(test)]
#[path = "claude_gateway_sign_in_tests.rs"]
mod tests;
