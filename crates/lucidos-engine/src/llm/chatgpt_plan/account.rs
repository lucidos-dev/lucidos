//! The workspace's Sign in with ChatGPT registration and sign-in: at most one
//! row, in `chatgpt_plan_accounts`.
//!
//! The tokens stay out of `oauth_accounts` on purpose. Every reader of that
//! table hands a token on, and nothing outside this module reads this one.

use chrono::{DateTime, Utc};
use sqlx::PgPool;

use super::auth_server::AuthServer;
use super::id_token::Identity;
use super::token_endpoint::{revoke, GrantedTokens};
use crate::core::secret_store::{
    self, ChatGptPlanAccessToken, ChatGptPlanIdToken, ChatGptPlanRefreshToken, Secret, SecretColumn,
};
use crate::engine::event_bus::{BusEvent, EventBus, SystemEvent};
use crate::engine::thread_events::MessageOrigin;

type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// What a sign-in needs before it starts. It outlives a sign-out, and only
/// [`ChatGptPlanAccountStore::forget_account`] clears it.
#[derive(Clone)]
pub struct Registration {
    /// The `ext_agent_host_id`, a `urn:uuid:` that names no user.
    pub host_id: String,
    /// The client id OpenAI issued, once the first sign-in completed.
    pub client_id: Option<String>,
    /// The last `id_token`, sent back as `id_token_hint`.
    pub id_token: Option<String>,
}

impl std::fmt::Debug for Registration {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Registration")
            .field("host_id", &self.host_id)
            .field("client_id", &self.client_id)
            .field("id_token", &self.id_token.as_ref().map(|_| "<redacted>"))
            .finish()
    }
}

/// A signed-in account, tokens included. Engine-internal only.
#[derive(Clone)]
pub struct ChatGptPlanAccount {
    pub client_id: String,
    pub email: Option<String>,
    pub access_token: String,
    pub refresh_token: String,
    pub token_expiry: Option<DateTime<Utc>>,
    pub refresh_due_at: Option<DateTime<Utc>>,
    pub signed_in_at: DateTime<Utc>,
}

impl std::fmt::Debug for ChatGptPlanAccount {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ChatGptPlanAccount")
            .field("client_id", &self.client_id)
            .field("email", &self.email)
            .field("access_token", &"<redacted>")
            .field("refresh_token", &"<redacted>")
            .field("token_expiry", &self.token_expiry)
            .field("refresh_due_at", &self.refresh_due_at)
            .field("signed_in_at", &self.signed_in_at)
            .finish()
    }
}

impl ChatGptPlanAccount {
    /// Whether the token is due a refresh: at OpenAI's `earliest_refresh_at`,
    /// else a minute before it expires. A token with neither is taken as
    /// valid until OpenAI answers `401`.
    pub fn refresh_due(&self, now: DateTime<Utc>) -> bool {
        self.refresh_due_at
            .or_else(|| {
                self.token_expiry
                    .map(|expiry| expiry - chrono::Duration::seconds(REFRESH_MARGIN_SECS))
            })
            .is_some_and(|due| now >= due)
    }
}

/// How long before its expiry a token with no `earliest_refresh_at` refreshes.
const REFRESH_MARGIN_SECS: i64 = 60;

impl<'r> sqlx::FromRow<'r, sqlx::postgres::PgRow> for ChatGptPlanAccount {
    fn from_row(row: &'r sqlx::postgres::PgRow) -> Result<Self, sqlx::Error> {
        use sqlx::Row as _;
        Ok(Self {
            client_id: row.try_get("client_id")?,
            email: row.try_get("email")?,
            access_token: row
                .try_get::<Secret<ChatGptPlanAccessToken>, _>("access_token")?
                .into_inner(),
            refresh_token: row
                .try_get::<Secret<ChatGptPlanRefreshToken>, _>("refresh_token")?
                .into_inner(),
            token_expiry: row.try_get("token_expiry")?,
            refresh_due_at: row.try_get("refresh_due_at")?,
            signed_in_at: row.try_get("signed_in_at")?,
        })
    }
}

/// The sign-in as the API reports it: who, never a token.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ChatGptPlanAccountInfo {
    pub email: Option<String>,
    pub signed_in_at: DateTime<Utc>,
}

impl From<&ChatGptPlanAccount> for ChatGptPlanAccountInfo {
    fn from(account: &ChatGptPlanAccount) -> Self {
        Self {
            email: account.email.clone(),
            signed_in_at: account.signed_in_at,
        }
    }
}

/// Why a ChatGPT plan sign-in ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ChatGptPlanSignOutReason {
    /// Someone signed out, from Settings, the CLI or the API.
    SignedOut,
    /// OpenAI refused the refresh token for good, so the user must sign in
    /// again.
    Rejected,
    /// Someone forgot the account and its registration, so the next sign-in
    /// may use any ChatGPT account.
    Forgotten,
}

impl ChatGptPlanSignOutReason {
    fn as_str(self) -> &'static str {
        match self {
            Self::SignedOut => "signed-out",
            Self::Rejected => "rejected",
            Self::Forgotten => "forgotten",
        }
    }
}

/// A completed sign-in, ready to store.
pub(super) struct CompletedSignIn {
    pub client_id: String,
    pub identity: Identity,
    pub id_token: String,
    pub tokens: GrantedTokens,
}

/// The only access to `chatgpt_plan_accounts`. A sign-in and a sign-out
/// announce themselves; minting the host id and a token refresh are silent.
pub struct ChatGptPlanAccountStore;

const ACCOUNT_COLUMNS: &str =
    "client_id, email, access_token, refresh_token, token_expiry, refresh_due_at, signed_in_at";

impl ChatGptPlanAccountStore {
    /// The registration, minting the host id on the first call.
    pub async fn registration(pool: &PgPool) -> Result<Registration, BoxError> {
        sqlx::query(
            "INSERT INTO chatgpt_plan_accounts (host_id) VALUES ($1)
             ON CONFLICT (singleton) DO NOTHING",
        )
        .bind(format!("urn:uuid:{}", uuid::Uuid::new_v4()))
        .execute(pool)
        .await?;
        let (host_id, client_id, id_token): (String, Option<String>, Option<String>) =
            sqlx::query_as("SELECT host_id, client_id, id_token FROM chatgpt_plan_accounts")
                .fetch_one(pool)
                .await?;
        Ok(Registration {
            host_id,
            client_id,
            id_token: id_token
                .map(|sealed| secret_store::open(&ChatGptPlanIdToken::SPEC, &sealed))
                .transpose()?,
        })
    }

    /// The signed-in account, if there is one.
    pub async fn get(pool: &PgPool) -> Result<Option<ChatGptPlanAccount>, sqlx::Error> {
        sqlx::query_as::<_, ChatGptPlanAccount>(&format!(
            "SELECT {ACCOUNT_COLUMNS} FROM chatgpt_plan_accounts WHERE access_token IS NOT NULL"
        ))
        .fetch_optional(pool)
        .await
    }

    /// Whether a sign-in is stored, without opening a token.
    pub async fn is_signed_in(pool: &PgPool) -> Result<bool, sqlx::Error> {
        sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM chatgpt_plan_accounts WHERE access_token IS NOT NULL)",
        )
        .fetch_one(pool)
        .await
    }

    /// Store a completed sign-in and announce it.
    ///
    /// Refused when the registration moved on since the sign-in began: another
    /// client id was stored, or the account is not the registered one.
    pub(super) async fn sign_in(
        pool: &PgPool,
        event_bus: &EventBus,
        signed_in: &CompletedSignIn,
        actor: Option<MessageOrigin>,
    ) -> Result<(), BoxError> {
        let refresh_token = signed_in
            .tokens
            .refresh_token
            .as_deref()
            .ok_or("OpenAI granted no refresh token")?;
        let stored = sqlx::query(
            "UPDATE chatgpt_plan_accounts SET
                 client_id = $1, issuer = $2, subject = $3, email = $4, id_token = $5,
                 access_token = $6, refresh_token = $7, token_expiry = $8,
                 refresh_due_at = $9, signed_in_at = NOW(), updated_at = NOW()
             WHERE (client_id IS NULL OR client_id = $1)
               AND (subject IS NULL OR (issuer = $2 AND subject = $3))",
        )
        .bind(&signed_in.client_id)
        .bind(&signed_in.identity.issuer)
        .bind(&signed_in.identity.subject)
        .bind(signed_in.identity.email.as_deref())
        .bind(Secret::<ChatGptPlanIdToken>::new(&*signed_in.id_token))
        .bind(Secret::<ChatGptPlanAccessToken>::new(
            &*signed_in.tokens.access_token,
        ))
        .bind(Secret::<ChatGptPlanRefreshToken>::new(refresh_token))
        .bind(signed_in.tokens.token_expiry)
        .bind(signed_in.tokens.refresh_due_at)
        .execute(pool)
        .await?;
        if stored.rows_affected() == 0 {
            return Err(
                "This workspace is registered with another ChatGPT account. Sign in \
                        with that account."
                    .into(),
            );
        }
        event_bus
            .emit_or_log(
                BusEvent::System(SystemEvent::ChatGptPlanSignedIn {
                    email: signed_in.identity.email.clone(),
                    actor,
                }),
                "[ChatGptPlan] ChatGptPlanSignedIn",
            )
            .await;
        Ok(())
    }

    /// Clear the sign-in, keep the registration, and announce it. Returns
    /// whether there was one. Its refresh token is then revoked at `auth` in
    /// the background, best effort.
    pub async fn sign_out(
        pool: &PgPool,
        event_bus: &EventBus,
        auth: &AuthServer,
        actor: Option<MessageOrigin>,
    ) -> Result<bool, BoxError> {
        let Some(ended) = Self::clear_tokens(pool, None).await? else {
            return Ok(false);
        };
        Self::announce_sign_out(event_bus, ChatGptPlanSignOutReason::SignedOut, actor).await;
        ended.revoke_at(auth);
        Ok(true)
    }

    /// Clear the sign-in AND the registration, and announce it. Returns what
    /// ended, so the next sign-in registers afresh with any ChatGPT account.
    /// A stored refresh token is revoked at `auth` in the background.
    ///
    /// The host id stays: it names no user.
    pub async fn forget_account(
        pool: &PgPool,
        event_bus: &EventBus,
        auth: &AuthServer,
        actor: Option<MessageOrigin>,
    ) -> Result<SignOutOutcome, BoxError> {
        let Some((client_id, sealed)) = Self::clear_registration(pool).await? else {
            return Ok(SignOutOutcome::default());
        };
        Self::announce_sign_out(event_bus, ChatGptPlanSignOutReason::Forgotten, actor).await;
        let signed_out = sealed.is_some();
        if let Some(sealed) = sealed {
            EndedSignIn {
                client_id,
                refresh_token: secret_store::open(&ChatGptPlanRefreshToken::SPEC, &sealed),
            }
            .revoke_at(auth);
        }
        Ok(SignOutOutcome {
            signed_out,
            account_forgotten: true,
        })
    }

    /// Whether a registration from an earlier sign-in is kept, which only
    /// that account may sign in with until it is forgotten.
    pub async fn is_registered(pool: &PgPool) -> Result<bool, sqlx::Error> {
        sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM chatgpt_plan_accounts WHERE client_id IS NOT NULL)",
        )
        .fetch_one(pool)
        .await
    }

    /// Clear a sign-in whose refresh token OpenAI refused for good, and
    /// announce it. A sign-in made since, with another token, is kept.
    pub(super) async fn forget_rejected(
        pool: &PgPool,
        event_bus: &EventBus,
        rejected_refresh_token: &str,
    ) -> Result<bool, BoxError> {
        if Self::clear_tokens(pool, Some(rejected_refresh_token))
            .await?
            .is_none()
        {
            return Ok(false);
        }
        Self::announce_sign_out(event_bus, ChatGptPlanSignOutReason::Rejected, None).await;
        Ok(true)
    }

    /// Whether the last sign-in ended because OpenAI refused it, with no
    /// sign-in since. Read from the events, so it survives a restart.
    pub async fn rejected_last(pool: &PgPool) -> Result<bool, sqlx::Error> {
        let reason: Option<Option<String>> = sqlx::query_scalar(
            "SELECT payload->'data'->>'reason' FROM events
             WHERE aggregate = 'chatgpt_plan'
               AND event_type IN ('ChatGptPlanSignedIn', 'ChatGptPlanSignedOut')
             ORDER BY created DESC, sequence DESC
             LIMIT 1",
        )
        .fetch_optional(pool)
        .await?;
        Ok(reason.flatten().as_deref() == Some(ChatGptPlanSignOutReason::Rejected.as_str()))
    }

    async fn announce_sign_out(
        event_bus: &EventBus,
        reason: ChatGptPlanSignOutReason,
        actor: Option<MessageOrigin>,
    ) {
        event_bus
            .emit_or_log(
                BusEvent::System(SystemEvent::ChatGptPlanSignedOut { reason, actor }),
                "[ChatGptPlan] ChatGptPlanSignedOut",
            )
            .await;
    }

    /// Clear the tokens, only while the row holds `rejected` when one is
    /// given. Returns the ended sign-in.
    ///
    /// A sealed token carries a random nonce, so SQL cannot compare it. The
    /// row is locked, opened and compared here.
    async fn clear_tokens(
        pool: &PgPool,
        rejected: Option<&str>,
    ) -> Result<Option<EndedSignIn>, sqlx::Error> {
        let mut tx = pool.begin().await?;
        let Some((client_id, sealed)) = sqlx::query_as::<_, (String, String)>(
            "SELECT client_id, refresh_token FROM chatgpt_plan_accounts
             WHERE refresh_token IS NOT NULL FOR UPDATE",
        )
        .fetch_optional(&mut *tx)
        .await?
        else {
            return Ok(None);
        };
        let refresh_token = secret_store::open(&ChatGptPlanRefreshToken::SPEC, &sealed);
        // A token that does not open is not the one OpenAI refused: the
        // engine opened that one to present it.
        let holds_rejected = match (rejected, &refresh_token) {
            (None, _) => true,
            (Some(rejected), Ok(stored)) => stored == rejected,
            (Some(_), Err(_)) => false,
        };
        if !holds_rejected {
            return Ok(None);
        }
        sqlx::query(
            "UPDATE chatgpt_plan_accounts SET access_token = NULL, refresh_token = NULL,
                 token_expiry = NULL, refresh_due_at = NULL, signed_in_at = NULL,
                 updated_at = NOW()",
        )
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(Some(EndedSignIn {
            client_id,
            refresh_token,
        }))
    }

    /// Clear the registration and any sign-in. Returns the forgotten client id
    /// and sealed refresh token, or `None` when nothing was registered.
    async fn clear_registration(
        pool: &PgPool,
    ) -> Result<Option<(String, Option<String>)>, sqlx::Error> {
        let mut tx = pool.begin().await?;
        let Some(forgotten) = sqlx::query_as::<_, (String, Option<String>)>(
            "SELECT client_id, refresh_token FROM chatgpt_plan_accounts
             WHERE client_id IS NOT NULL FOR UPDATE",
        )
        .fetch_optional(&mut *tx)
        .await?
        else {
            return Ok(None);
        };
        sqlx::query(
            "UPDATE chatgpt_plan_accounts SET client_id = NULL, issuer = NULL, subject = NULL,
                 email = NULL, id_token = NULL, access_token = NULL, refresh_token = NULL,
                 token_expiry = NULL, refresh_due_at = NULL, signed_in_at = NULL,
                 updated_at = NOW()",
        )
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(Some(forgotten))
    }

    /// Store refreshed tokens for the sign-in that held `spent_refresh_token`,
    /// all in one write. Returns `false` when that sign-in is gone, so a
    /// refresh racing a sign-out or a newer sign-in cannot overwrite it.
    pub(super) async fn update_tokens(
        pool: &PgPool,
        spent_refresh_token: &str,
        tokens: &GrantedTokens,
    ) -> Result<bool, sqlx::Error> {
        let mut tx = pool.begin().await?;
        let stored: Option<String> = sqlx::query_scalar(
            "SELECT refresh_token FROM chatgpt_plan_accounts
             WHERE refresh_token IS NOT NULL FOR UPDATE",
        )
        .fetch_optional(&mut *tx)
        .await?;
        let holds_spent = stored
            .and_then(|sealed| secret_store::open(&ChatGptPlanRefreshToken::SPEC, &sealed).ok())
            .is_some_and(|stored| stored == spent_refresh_token);
        if !holds_spent {
            return Ok(false);
        }
        sqlx::query(
            "UPDATE chatgpt_plan_accounts
             SET access_token = $1,
                 refresh_token = COALESCE($2, refresh_token),
                 id_token = COALESCE($3, id_token),
                 token_expiry = $4,
                 refresh_due_at = $5,
                 updated_at = NOW()",
        )
        .bind(Secret::<ChatGptPlanAccessToken>::new(&*tokens.access_token))
        .bind(
            tokens
                .refresh_token
                .as_deref()
                .map(Secret::<ChatGptPlanRefreshToken>::new),
        )
        .bind(
            tokens
                .id_token
                .as_deref()
                .map(Secret::<ChatGptPlanIdToken>::new),
        )
        .bind(tokens.token_expiry)
        .bind(tokens.refresh_due_at)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(true)
    }
}

#[cfg(test)]
impl ChatGptPlanAccountStore {
    /// Store a sign-in holding `access_token`, valid for an hour, for tests
    /// outside this module.
    pub(crate) async fn seed_sign_in(pool: &PgPool, event_bus: &EventBus, access_token: &str) {
        Self::registration(pool).await.expect("mint the host id");
        let signed_in = CompletedSignIn {
            client_id: "oaiapp_test".into(),
            identity: Identity {
                issuer: "https://auth.openai.com".into(),
                subject: "user-test".into(),
                email: Some("user@example.com".into()),
            },
            id_token: "id-token".into(),
            tokens: GrantedTokens {
                access_token: access_token.into(),
                refresh_token: Some("refresh-token".into()),
                id_token: None,
                token_expiry: Some(Utc::now() + chrono::Duration::hours(1)),
                refresh_due_at: Some(Utc::now() + chrono::Duration::minutes(50)),
            },
        };
        Self::sign_in(pool, event_bus, &signed_in, None)
            .await
            .expect("store the sign-in");
    }
}

/// What a sign-out ended, as the API reports it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize)]
pub struct SignOutOutcome {
    pub signed_out: bool,
    pub account_forgotten: bool,
}

/// A sign-in that a sign-out ended.
struct EndedSignIn {
    client_id: String,
    refresh_token: Result<String, BoxError>,
}

impl EndedSignIn {
    /// Revoke the ended refresh token in the background, best effort.
    fn revoke_at(self, auth: &AuthServer) {
        match self.refresh_token {
            Ok(token) => {
                tokio::spawn(revoke(auth.clone(), self.client_id, token));
            }
            Err(e) => crate::log!("[ChatGptPlan] Signed out without revoking: {e}"),
        }
    }
}
