//! One Sign in with ChatGPT attempt: the authorize URL, then the callback's
//! code exchanged for tokens.
//!
//! An attempt in progress lives in memory only. A restart means starting
//! again, which costs the user one click.

use sqlx::PgPool;

use super::account::{ChatGptPlanAccountInfo, ChatGptPlanAccountStore, CompletedSignIn};
use super::auth_server::AuthServer;
use super::id_token;
use super::token_endpoint::{post_token_form, revoke, GrantedTokens, TokenReply};
use crate::core::oauth::{default_redirect_uri, random_url_token, AuthorizationGrant, Pkce};
use crate::core::oauth_callback::CallbackRoute;
use crate::engine::event_bus::EventBus;
use crate::engine::thread_events::MessageOrigin;
use crate::llm::openai::OPENAI_DEFAULT_BASE_URL;

type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// The first-sign-in entrypoint OpenAI's docs name. It registers the client
/// and is never stored or sent to the token endpoint.
pub const DYNAMIC_CLIENT_ID: &str = "dynamic_agent_client";

/// The scopes the docs list for plan usage.
const SCOPES: &str =
    "openid profile email offline_access resource.invoke chatgpt.tokens.use.direct";

/// A first sign-in whose callback lacks the issued client id. OpenAI sends
/// it, and a gateway from before ADR 0636 drops it while relaying.
const RELAY_DROPPED_CLIENT_ID: &str = "A Lucidos gateway that relayed OpenAI's callback did not \
     pass on the issued client id, most likely because it is an older version. Update Lucidos on \
     the computer whose browser approved the sign-in, and on the computer hosting this workspace \
     if it is remote. Signing in again works only after that.";

/// The name a workspace shows in ChatGPT Settings > Usage.
pub fn agent_name_hint(workspace_name: &str) -> String {
    format!("Lucidos ({workspace_name})")
}

/// Which client an attempt signs in as.
#[derive(Debug, Clone, PartialEq, Eq)]
enum SignInClient {
    /// The first sign-in: registers through [`DYNAMIC_CLIENT_ID`].
    Register,
    /// A later sign-in, with the client id OpenAI issued.
    Issued(String),
}

/// An attempt waiting for its callback.
pub struct PendingSignIn {
    auth: AuthServer,
    state: String,
    nonce: String,
    pkce: Pkce,
    client: SignInClient,
    authorize_url: String,
}

impl std::fmt::Debug for PendingSignIn {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PendingSignIn")
            .field("auth", &self.auth)
            .field("client", &self.client)
            .field("pkce", &self.pkce)
            .finish_non_exhaustive()
    }
}

impl PendingSignIn {
    /// Begin an attempt: mint the host id if this is the first, and build the
    /// authorize URL. `route` decides the `state` the callback is routed by.
    pub async fn begin(
        pool: &PgPool,
        auth: AuthServer,
        workspace_name: &str,
        route: &CallbackRoute,
    ) -> Result<Self, BoxError> {
        let registration = ChatGptPlanAccountStore::registration(pool).await?;
        let state = route.state_for(&random_url_token());
        let nonce = random_url_token();
        let pkce = Pkce::generate();
        let client = registration
            .client_id
            .clone()
            .map_or(SignInClient::Register, SignInClient::Issued);
        let mut params: Vec<(&str, String)> = vec![
            ("response_type", "code".into()),
            ("redirect_uri", default_redirect_uri()),
            ("scope", SCOPES.into()),
            ("resource", OPENAI_DEFAULT_BASE_URL.into()),
            ("state", state.clone()),
            ("nonce", nonce.clone()),
            ("code_challenge_method", "S256".into()),
            ("code_challenge", pkce.challenge().into()),
            ("ext_agent_host_id", registration.host_id),
        ];
        match &client {
            SignInClient::Register => {
                params.push(("client_id", DYNAMIC_CLIENT_ID.into()));
                params.push(("agent_name_hint", agent_name_hint(workspace_name)));
            }
            SignInClient::Issued(client_id) => {
                params.push(("client_id", client_id.clone()));
                if let Some(hint) = registration.id_token {
                    params.push(("id_token_hint", hint));
                }
            }
        }
        let authorize_url = reqwest::Url::parse_with_params(&auth.authorize_url(), &params)?.into();
        Ok(Self {
            auth,
            state,
            nonce,
            pkce,
            client,
            authorize_url,
        })
    }

    /// The `state` the callback must carry back.
    pub fn state(&self) -> &str {
        &self.state
    }

    /// Where the browser goes to approve the sign-in.
    pub fn authorize_url(&self) -> &str {
        &self.authorize_url
    }

    /// The client id this attempt's tokens are for. A callback naming another
    /// one is refused before any token request.
    fn issued_client_id(&self, from_callback: Option<&str>) -> Result<String, BoxError> {
        let from_callback = from_callback.map(str::trim).filter(|id| !id.is_empty());
        match (&self.client, from_callback) {
            (_, Some(DYNAMIC_CLIENT_ID)) => {
                Err("OpenAI returned the registration entrypoint, not an issued client id.".into())
            }
            (SignInClient::Register, Some(issued)) => Ok(issued.to_string()),
            // Why no fallback stands in: ADR 0636 § Notes.
            (SignInClient::Register, None) => Err(RELAY_DROPPED_CLIENT_ID.into()),
            (SignInClient::Issued(stored), None) => Ok(stored.clone()),
            (SignInClient::Issued(stored), Some(issued)) if issued == stored => Ok(stored.clone()),
            (SignInClient::Issued(_), Some(_)) => Err(
                "OpenAI returned a different client id than this workspace registered, so the \
                 sign-in was refused."
                    .into(),
            ),
        }
    }

    /// Exchange the callback's code, verify the `id_token`, store the sign-in
    /// and announce it with `actor`. A first sign-in's callback always carries
    /// the issued client id, and a later one may omit it.
    pub async fn complete(
        self,
        pool: &PgPool,
        event_bus: &EventBus,
        callback: AuthorizationGrant,
        actor: Option<MessageOrigin>,
    ) -> Result<ChatGptPlanAccountInfo, BoxError> {
        let client_id = self.issued_client_id(callback.client_id.as_deref())?;
        let redirect_uri = default_redirect_uri();
        let form = [
            ("grant_type", "authorization_code"),
            ("client_id", client_id.as_str()),
            ("code", callback.code.as_str()),
            ("code_verifier", self.pkce.verifier()),
            ("redirect_uri", redirect_uri.as_str()),
            ("resource", OPENAI_DEFAULT_BASE_URL),
        ];
        let tokens = match post_token_form(&self.auth, &form).await {
            Ok(TokenReply::Granted(tokens)) => tokens,
            Ok(TokenReply::Refused(refusal)) => {
                return Err(format!("OpenAI refused the sign-in: {refusal}").into())
            }
            Err(e) => return Err(format!("Could not complete the ChatGPT sign-in: {e}").into()),
        };
        let refresh_token = tokens.refresh_token.clone();
        let stored = self
            .store(pool, event_bus, client_id.clone(), tokens, actor)
            .await;
        if let (Err(_), Some(token)) = (&stored, refresh_token) {
            // A grant Lucidos refuses must not stay live at OpenAI.
            tokio::spawn(revoke(self.auth.clone(), client_id, token));
        }
        stored?;
        let account = ChatGptPlanAccountStore::get(pool)
            .await?
            .ok_or("the ChatGPT sign-in ended as it was stored")?;
        crate::log!("[ChatGptPlan] Signed in");
        Ok(ChatGptPlanAccountInfo::from(&account))
    }

    /// Verify the granted `id_token` and store the sign-in.
    async fn store(
        &self,
        pool: &PgPool,
        event_bus: &EventBus,
        client_id: String,
        tokens: GrantedTokens,
        actor: Option<MessageOrigin>,
    ) -> Result<(), BoxError> {
        let id_token = tokens
            .id_token
            .clone()
            .ok_or("OpenAI granted no ID token")?;
        let identity = id_token::verify(&self.auth, &id_token, &client_id, &self.nonce).await?;
        let signed_in = CompletedSignIn {
            client_id,
            identity,
            id_token,
            tokens,
        };
        ChatGptPlanAccountStore::sign_in(pool, event_bus, &signed_in, actor).await
    }
}

#[cfg(test)]
#[path = "sign_in_tests.rs"]
mod tests;
