//! Signing in with ChatGPT, for the HTTP API and the CLI over it.
//!
//! One attempt runs at a time, in memory only. Its callback comes back through
//! the gateway-owned loopback port (ADR 0622), routed by `state` to the
//! [`PendingCallback`] the attempt opened. Nothing is stored until OpenAI
//! grants tokens, so a restart mid-sign-in means starting again.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::PgPool;

use crate::core::oauth_callback::{CallbackRoute, PendingCallback};
use crate::engine::event_bus::{BusEvent, EventBus, SystemEvent};
use crate::engine::thread_events::MessageOrigin;
use crate::llm::chatgpt_plan::account::{ChatGptPlanAccountInfo, SignOutOutcome};
use crate::llm::chatgpt_plan::{AuthServer, ChatGptPlanAccountStore, LimitPause, PendingSignIn};

type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// How long an attempt waits for the browser to come back.
const SIGN_IN_TIMEOUT: Duration = Duration::from_secs(10 * 60);

/// The provider name the callback page shows.
const CALLBACK_PROVIDER: &str = "ChatGPT";

/// Where the current attempt stands. Carries what the user must see, never
/// the authorize URL, its `state` or a token.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "kebab-case")]
pub enum SignInState {
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
    pub sign_in: SignInState,
    /// When plan requests resume after the usage limit, while they are paused.
    pub paused_until: Option<DateTime<Utc>>,
    /// How long the pause has left, by the engine's clock. A client times its
    /// re-read from this, since its own clock may differ.
    pub pause_remaining_ms: Option<i64>,
}

/// The engine's one Sign in with ChatGPT slot. Each start bumps `generation`,
/// so a superseded attempt can never overwrite the state of its replacement.
///
/// `store` fences a completing sign-in against a sign-out. Aborting an attempt
/// takes effect only at its next await, so without the fence a sign-in could
/// commit after the sign-out's delete.
pub struct ChatGptPlanSignIn {
    inner: Mutex<Slot>,
    store: tokio::sync::Mutex<()>,
    timeout: Duration,
}

struct Slot {
    state: SignInState,
    generation: u64,
    task: Option<tokio::task::AbortHandle>,
}

impl Default for ChatGptPlanSignIn {
    fn default() -> Self {
        Self::with_timeout(SIGN_IN_TIMEOUT)
    }
}

impl ChatGptPlanSignIn {
    fn with_timeout(timeout: Duration) -> Self {
        Self {
            inner: Mutex::new(Slot {
                state: SignInState::Idle,
                generation: 0,
                task: None,
            }),
            store: tokio::sync::Mutex::new(()),
            timeout,
        }
    }

    pub fn state(&self) -> SignInState {
        self.lock().state.clone()
    }

    pub async fn status(
        &self,
        pool: &PgPool,
        pause: &LimitPause,
    ) -> Result<ChatGptPlanStatus, sqlx::Error> {
        let account = ChatGptPlanAccountStore::get(pool).await?;
        let paused_until = pause.paused_until();
        let sign_in_again =
            account.is_none() && ChatGptPlanAccountStore::rejected_last(pool).await?;
        Ok(ChatGptPlanStatus {
            account: account.as_ref().map(ChatGptPlanAccountInfo::from),
            sign_in_again,
            registered: ChatGptPlanAccountStore::is_registered(pool).await?,
            sign_in: self.state(),
            paused_until,
            pause_remaining_ms: paused_until
                .map(|until| (until - Utc::now()).num_milliseconds().max(0)),
        })
    }

    /// Start an attempt whose callback comes back by `route`, superseding any
    /// running one. Refuses, before building any URL, a browser that cannot
    /// reach the loopback redirect URI.
    pub async fn start(
        self: &Arc<Self>,
        pool: &PgPool,
        event_bus: &EventBus,
        auth: AuthServer,
        workspace_name: &str,
        route: CallbackRoute,
        actor: Option<MessageOrigin>,
    ) -> Result<StartedSignIn, BoxError> {
        refuse_unreachable(&route)?;
        let begun = self.lock().generation;
        let pending = PendingSignIn::begin(pool, auth, workspace_name, &route).await?;
        // Open before the URL leaves the engine, so the callback cannot arrive
        // before anything waits for it.
        let callback = PendingCallback::open(pending.state(), CALLBACK_PROVIDER);
        let expires_at = Utc::now() + chrono::Duration::from_std(self.timeout).unwrap_or_default();
        let started = StartedSignIn {
            authorize_url: pending.authorize_url().to_string(),
            expires_at,
        };

        {
            let mut slot = self.lock();
            if slot.generation != begun {
                return Err(
                    "A sign-out or another sign-in replaced this sign-in before it began.".into(),
                );
            }
            if let Some(task) = slot.task.take() {
                task.abort();
            }
            slot.generation += 1;
            slot.state = SignInState::Pending { expires_at };
            let generation = slot.generation;
            let (this, pool, event_bus, actor) = (
                Arc::clone(self),
                pool.clone(),
                event_bus.clone(),
                actor.clone(),
            );
            let task = tokio::spawn(async move {
                this.run(generation, &pool, &event_bus, pending, callback, actor)
                    .await;
            });
            slot.task = Some(task.abort_handle());
        }
        event_bus
            .emit_or_log(
                BusEvent::System(SystemEvent::ChatGptPlanSignInStarted { actor }),
                "[ChatGptPlan] ChatGptPlanSignInStarted",
            )
            .await;
        Ok(started)
    }

    /// Wait for the callback of `generation`, then complete the sign-in unless
    /// a sign-out or a newer attempt replaced it.
    async fn run(
        &self,
        generation: u64,
        pool: &PgPool,
        event_bus: &EventBus,
        pending: PendingSignIn,
        callback: PendingCallback,
        actor: Option<MessageOrigin>,
    ) {
        let outcome = match tokio::time::timeout(self.timeout, callback.received()).await {
            Err(_) => Err("The sign-in timed out before ChatGPT sent the browser back.".into()),
            Ok(Err(refused)) => Err(refused.to_string()),
            Ok(Ok(grant)) => {
                let _store = self.store.lock().await;
                if self.lock().generation == generation {
                    pending
                        .complete(pool, event_bus, grant, actor.clone())
                        .await
                        .map(|_| ())
                        .map_err(|e| e.to_string())
                } else {
                    Err("A sign-out or a newer sign-in replaced this one.".to_string())
                }
            }
        };
        if let Err(error) = &outcome {
            crate::log!("[ChatGptPlan] Sign-in ended: {}", error);
        }
        if let Some(error) = self.finish(generation, outcome) {
            event_bus
                .emit_or_log(
                    BusEvent::System(SystemEvent::ChatGptPlanSignInFailed { error, actor }),
                    "[ChatGptPlan] ChatGptPlanSignInFailed",
                )
                .await;
        }
    }

    /// Stop any attempt in progress and forget the stored sign-in. With
    /// `forget_account`, forget the registration too, so the next sign-in may
    /// use any ChatGPT account. Returns what ended.
    pub async fn sign_out(
        &self,
        pool: &PgPool,
        event_bus: &EventBus,
        auth: &AuthServer,
        forget_account: bool,
        actor: Option<MessageOrigin>,
    ) -> Result<SignOutOutcome, BoxError> {
        let _store = self.store.lock().await;
        let stopped = {
            let mut slot = self.lock();
            if let Some(task) = slot.task.take() {
                task.abort();
            }
            slot.generation += 1;
            std::mem::replace(&mut slot.state, SignInState::Idle)
        };
        if let SignInState::Pending { .. } = stopped {
            event_bus
                .emit_or_log(
                    BusEvent::System(SystemEvent::ChatGptPlanSignInCanceled {
                        actor: actor.clone(),
                    }),
                    "[ChatGptPlan] ChatGptPlanSignInCanceled",
                )
                .await;
        }
        if forget_account {
            return ChatGptPlanAccountStore::forget_account(pool, event_bus, auth, actor).await;
        }
        let signed_out = ChatGptPlanAccountStore::sign_out(pool, event_bus, auth, actor).await?;
        Ok(SignOutOutcome {
            signed_out,
            account_forgotten: false,
        })
    }

    /// Record how the attempt of `generation` ended, unless a newer one
    /// replaced it. Returns the error to announce, for a current failure only.
    fn finish(&self, generation: u64, outcome: Result<(), String>) -> Option<String> {
        let mut slot = self.lock();
        if slot.generation != generation {
            return None;
        }
        slot.task = None;
        let (state, failure) = match outcome {
            Ok(()) => (SignInState::Idle, None),
            Err(error) => (
                SignInState::Failed {
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

/// Refuse a route whose browser cannot follow the loopback redirect URI.
///
/// OpenAI accepts only `http://127.0.0.1:14981/oauth/callback`, so the browser
/// must run where a Lucidos gateway holds that port.
fn refuse_unreachable(route: &CallbackRoute) -> Result<(), BoxError> {
    match route {
        CallbackRoute::Gateway(browser) if browser.local => Ok(()),
        CallbackRoute::Gateway(_) => Err(
            "Sign in with ChatGPT returns only to this computer's own Lucidos, so open this \
             workspace on a computer running Lucidos and sign in there."
                .into(),
        ),
        CallbackRoute::OwnListener => Err(
            "Sign in with ChatGPT needs the Lucidos gateway, which holds the sign-in port, \
             so start this workspace through the gateway."
                .into(),
        ),
    }
}

#[cfg(test)]
#[path = "chatgpt_plan_sign_in_tests.rs"]
mod tests;
