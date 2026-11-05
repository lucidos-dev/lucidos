//! The workspace's sign-in to its Claude gateway: at most one, in
//! `claude_gateway_accounts`.
//!
//! The tokens stay out of `oauth_accounts` on purpose. Every reader of that
//! table hands a token on: as a script env var, to the `http_request` tool, to
//! the app proxy and to the access-token endpoint. Nothing outside this module
//! reads this table, so a gateway token reaches only the gateway.

use chrono::{DateTime, Utc};
use sqlx::PgPool;

use super::gateway_managed_settings::{Fetched, GatewayManagedSettings};
use super::revocation::{revoke, EndedSignIn};
use super::token_endpoint::jwt_email;
use super::{GatewayOrigin, GrantedTokens};
use crate::core::secret_store::{
    self, ClaudeGatewayAccessToken, ClaudeGatewayRefreshToken, Secret, SecretColumn,
};
use crate::engine::event_bus::{BusEvent, EventBus, SystemEvent};
use crate::engine::thread_events::MessageOrigin;

type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// The columns a [`ClaudeGatewayAccount`] reads.
const ACCOUNT_COLUMNS: &str = "issuer, email, access_token, refresh_token, token_expiry, \
     created_at, updated_at, managed_settings, managed_settings_etag, managed_settings_fetched_at, \
     managed_telemetry_since";

/// The stored sign-in, tokens included. Engine-internal only.
#[derive(Clone)]
pub struct ClaudeGatewayAccount {
    /// The gateway origin that issued the tokens. They go to no other host.
    pub issuer: String,
    pub email: Option<String>,
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub token_expiry: Option<DateTime<Utc>>,
    /// When the user signed in. A token refresh leaves it, so it names this
    /// sign-in apart from any later one.
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    /// The gateway's managed settings for this user. `None` when it has no
    /// policy for them, or none was fetched yet.
    pub managed_settings: Option<sqlx::types::Json<serde_json::Value>>,
    pub managed_settings_etag: Option<String>,
    /// When the managed settings were last fetched. `None` until the first.
    pub managed_settings_fetched_at: Option<DateTime<Utc>>,
    /// Since when the managed settings have asked for usage telemetry. `None`
    /// while they do not. A capture made earlier is never sent.
    pub managed_telemetry_since: Option<DateTime<Utc>>,
}

// Manual `FromRow`: both tokens are sealed at rest, and open as they decode.
impl<'r> sqlx::FromRow<'r, sqlx::postgres::PgRow> for ClaudeGatewayAccount {
    fn from_row(row: &'r sqlx::postgres::PgRow) -> Result<Self, sqlx::Error> {
        use sqlx::Row as _;
        Ok(Self {
            issuer: row.try_get("issuer")?,
            email: row.try_get("email")?,
            access_token: row
                .try_get::<Secret<ClaudeGatewayAccessToken>, _>("access_token")?
                .into_inner(),
            refresh_token: row
                .try_get::<Option<Secret<ClaudeGatewayRefreshToken>>, _>("refresh_token")?
                .map(Secret::into_inner),
            token_expiry: row.try_get("token_expiry")?,
            created_at: row.try_get("created_at")?,
            updated_at: row.try_get("updated_at")?,
            managed_settings: row.try_get("managed_settings")?,
            managed_settings_etag: row.try_get("managed_settings_etag")?,
            managed_settings_fetched_at: row.try_get("managed_settings_fetched_at")?,
            managed_telemetry_since: row.try_get("managed_telemetry_since")?,
        })
    }
}

impl ClaudeGatewayAccount {
    /// The gateway's managed settings, when it set a policy for this user.
    pub fn managed_settings(&self) -> Option<GatewayManagedSettings> {
        self.managed_settings
            .as_ref()
            .map(|document| GatewayManagedSettings::from_document(document))
    }
}

impl std::fmt::Debug for ClaudeGatewayAccount {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClaudeGatewayAccount")
            .field("issuer", &self.issuer)
            .field("email", &self.email)
            .field("access_token", &"<redacted>")
            .field(
                "refresh_token",
                &self.refresh_token.as_ref().map(|_| "<redacted>"),
            )
            .field("token_expiry", &self.token_expiry)
            .field("created_at", &self.created_at)
            .field("updated_at", &self.updated_at)
            .field("managed_settings", &self.managed_settings())
            .field("managed_settings_etag", &self.managed_settings_etag)
            .field(
                "managed_settings_fetched_at",
                &self.managed_settings_fetched_at,
            )
            .field("managed_telemetry_since", &self.managed_telemetry_since)
            .finish()
    }
}

/// The stored sign-in as the API reports it: who and where, never a token.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ClaudeGatewayAccountInfo {
    pub issuer: String,
    pub email: Option<String>,
    pub signed_in_at: DateTime<Utc>,
}

impl From<&ClaudeGatewayAccount> for ClaudeGatewayAccountInfo {
    fn from(account: &ClaudeGatewayAccount) -> Self {
        Self {
            issuer: account.issuer.clone(),
            // A refresh never rewrites `email`, so a row stored with none may
            // still name its account in the access token.
            email: account
                .email
                .clone()
                .or_else(|| jwt_email(&account.access_token)),
            signed_in_at: account.created_at,
        }
    }
}

/// Why a Claude gateway sign-in ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ClaudeGatewaySignOutReason {
    /// Someone signed out, from Settings, the CLI or the API.
    SignedOut,
    /// `claude_gateway_url` stopped naming the gateway that issued the tokens.
    UrlChanged,
    /// The gateway rejected a token nothing can renew, so the user must sign
    /// in again.
    Rejected,
}

impl ClaudeGatewaySignOutReason {
    fn as_str(self) -> &'static str {
        match self {
            Self::SignedOut => "signed-out",
            Self::UrlChanged => "url-changed",
            Self::Rejected => "rejected",
        }
    }
}

/// The token the gateway rejected. A sign-in is forgotten only while it
/// still holds that token.
#[derive(Clone, Copy)]
pub enum RejectedToken<'a> {
    Refresh(&'a str),
    Access(&'a str),
}

/// The only access to `claude_gateway_accounts`. A sign-in and a sign-out
/// announce themselves; a token refresh is silent.
pub struct ClaudeGatewayAccountStore;

impl ClaudeGatewayAccountStore {
    pub async fn get(pool: &PgPool) -> Result<Option<ClaudeGatewayAccount>, sqlx::Error> {
        sqlx::query_as::<_, ClaudeGatewayAccount>(&format!(
            "SELECT {ACCOUNT_COLUMNS} FROM claude_gateway_accounts"
        ))
        .fetch_optional(pool)
        .await
    }

    /// The stored sign-in, when its managed settings are due a fetch: never
    /// fetched, or fetched over an hour ago. The age is the database's own,
    /// so the engine clock cannot skew it.
    pub(super) async fn managed_settings_due(
        pool: &PgPool,
    ) -> Result<Option<ClaudeGatewayAccount>, sqlx::Error> {
        sqlx::query_as::<_, ClaudeGatewayAccount>(&format!(
            "SELECT {ACCOUNT_COLUMNS} FROM claude_gateway_accounts
             WHERE managed_settings_fetched_at IS NULL
                OR managed_settings_fetched_at < NOW() - INTERVAL '1 hour'"
        ))
        .fetch_optional(pool)
        .await
    }

    /// Store what a managed settings fetch for `account` found, and announce
    /// a changed policy. Stores nothing once that sign-in has ended, so a
    /// fetch racing a sign-out or a new sign-in cannot attach its policy.
    pub(super) async fn store_managed_settings(
        pool: &PgPool,
        event_bus: &EventBus,
        account: &ClaudeGatewayAccount,
        fetched: Fetched,
    ) -> Result<(), sqlx::Error> {
        let (document, etag) = match fetched {
            Fetched::Unchanged => {
                sqlx::query(
                    "UPDATE claude_gateway_accounts SET managed_settings_fetched_at = NOW()
                     WHERE issuer = $1 AND created_at = $2",
                )
                .bind(&account.issuer)
                .bind(account.created_at)
                .execute(pool)
                .await?;
                return Ok(());
            }
            Fetched::NoPolicy => (None, None),
            Fetched::Policy { document, etag } => (Some(document), etag),
        };
        let policy = document.as_ref().map(GatewayManagedSettings::from_document);
        let telemetry = policy
            .as_ref()
            .is_some_and(GatewayManagedSettings::telemetry_requested);
        // Telemetry applies from when a policy first asked for it. The first
        // policy after a sign-in decides from the sign-in on, since the
        // exporter holds every capture until it arrives.
        let changed: Option<bool> = sqlx::query_scalar(
            "WITH previous AS (
                 SELECT managed_settings, managed_settings_fetched_at, managed_telemetry_since
                 FROM claude_gateway_accounts
                 WHERE issuer = $1 AND created_at = $2
                 FOR UPDATE
             )
             UPDATE claude_gateway_accounts
             SET managed_settings = $3,
                 managed_settings_etag = $4,
                 managed_settings_fetched_at = NOW(),
                 managed_telemetry_since = CASE
                     WHEN NOT $5 THEN NULL
                     WHEN previous.managed_settings_fetched_at IS NULL THEN created_at
                     ELSE COALESCE(previous.managed_telemetry_since, NOW())
                 END
             FROM previous
             WHERE issuer = $1 AND created_at = $2
             RETURNING previous.managed_settings IS DISTINCT FROM $3::jsonb",
        )
        .bind(&account.issuer)
        .bind(account.created_at)
        .bind(document.as_ref().map(sqlx::types::Json))
        .bind(etag)
        .bind(telemetry)
        .fetch_optional(pool)
        .await?;
        if changed != Some(true) {
            return Ok(());
        }
        crate::log!(
            "[ClaudeGateway] {} changed its managed settings: {:?}",
            account.issuer,
            policy
        );
        event_bus
            .emit_or_log(
                BusEvent::System(SystemEvent::ClaudeGatewayManagedSettingsChanged {
                    issuer: account.issuer.clone(),
                    policy: policy.is_some(),
                    telemetry,
                    available_models: policy
                        .as_ref()
                        .and_then(GatewayManagedSettings::available_models),
                }),
                "[ClaudeGateway] ClaudeGatewayManagedSettingsChanged",
            )
            .await;
        Ok(())
    }

    /// Store a completed sign-in, replacing any earlier one, and announce it.
    pub async fn sign_in(
        pool: &PgPool,
        event_bus: &EventBus,
        issuer: &GatewayOrigin,
        tokens: &GrantedTokens,
        actor: Option<MessageOrigin>,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO claude_gateway_accounts
                 (issuer, email, access_token, refresh_token, token_expiry)
             VALUES ($1, $2, $3, $4, $5)
             ON CONFLICT (singleton) DO UPDATE SET
                 issuer = EXCLUDED.issuer,
                 email = EXCLUDED.email,
                 access_token = EXCLUDED.access_token,
                 refresh_token = EXCLUDED.refresh_token,
                 token_expiry = EXCLUDED.token_expiry,
                 created_at = NOW(),
                 updated_at = NOW(),
                 managed_settings = NULL,
                 managed_settings_etag = NULL,
                 managed_settings_fetched_at = NULL,
                 managed_telemetry_since = NULL",
        )
        .bind(issuer.as_str())
        .bind(tokens.email.as_deref())
        .bind(Secret::<ClaudeGatewayAccessToken>::new(
            &*tokens.access_token,
        ))
        .bind(refresh_secret(tokens))
        .bind(tokens.token_expiry)
        .execute(pool)
        .await?;
        event_bus
            .emit_or_log(
                BusEvent::System(SystemEvent::ClaudeGatewaySignedIn {
                    issuer: issuer.to_string(),
                    email: tokens.email.clone(),
                    actor,
                }),
                "[ClaudeGateway] ClaudeGatewaySignedIn",
            )
            .await;
        Ok(())
    }

    /// Forget the sign-in and announce it with `reason`. Returns whether there
    /// was one, and announces only then. Its tokens are then revoked at the
    /// issuer in the background, best effort.
    pub async fn sign_out(
        pool: &PgPool,
        event_bus: &EventBus,
        reason: ClaudeGatewaySignOutReason,
        actor: Option<MessageOrigin>,
    ) -> Result<bool, sqlx::Error> {
        let removed = Self::delete_row(pool, None).await?;
        let issuer = removed.as_ref().map(|removed| removed.issuer.as_str());
        Self::announce_sign_out(event_bus, issuer, reason, actor).await;
        let signed_out = removed.is_some();
        if let Some(ended) = removed.and_then(|removed| removed.revocable) {
            tokio::spawn(revoke(ended));
        }
        Ok(signed_out)
    }

    /// Forget a sign-in whose token the gateway rejected, and announce it with
    /// no actor. A sign-in made since, with a new token, is kept.
    pub async fn forget_rejected(
        pool: &PgPool,
        event_bus: &EventBus,
        rejected: RejectedToken<'_>,
    ) -> Result<bool, sqlx::Error> {
        let removed = Self::delete_row(pool, Some(rejected)).await?;
        Self::announce_sign_out(
            event_bus,
            removed.as_ref().map(|removed| removed.issuer.as_str()),
            ClaudeGatewaySignOutReason::Rejected,
            None,
        )
        .await;
        Ok(removed.is_some())
    }

    /// Whether the last sign-in ended because the gateway rejected it, with no
    /// sign-in since. Read from the events, so it survives a restart.
    pub async fn rejected_last(pool: &PgPool) -> Result<bool, sqlx::Error> {
        let reason: Option<Option<String>> = sqlx::query_scalar(
            "SELECT payload->'data'->>'reason' FROM events
             WHERE aggregate = 'claude_gateway'
               AND event_type IN ('ClaudeGatewaySignedIn', 'ClaudeGatewaySignedOut')
             ORDER BY created DESC, sequence DESC
             LIMIT 1",
        )
        .fetch_optional(pool)
        .await?;
        Ok(reason.flatten().as_deref() == Some(ClaudeGatewaySignOutReason::Rejected.as_str()))
    }

    async fn announce_sign_out(
        event_bus: &EventBus,
        removed_issuer: Option<&str>,
        reason: ClaudeGatewaySignOutReason,
        actor: Option<MessageOrigin>,
    ) {
        let Some(issuer) = removed_issuer else {
            return;
        };
        event_bus
            .emit_or_log(
                BusEvent::System(SystemEvent::ClaudeGatewaySignedOut {
                    issuer: issuer.to_string(),
                    reason,
                    actor,
                }),
                "[ClaudeGateway] ClaudeGatewaySignedOut",
            )
            .await;
    }

    /// Delete the row, only if it holds the `rejected` token when one is
    /// given. Returns the removed sign-in.
    ///
    /// A sealed token carries a random nonce, so SQL cannot compare it. The
    /// row is locked, opened and compared here, then deleted.
    async fn delete_row(
        pool: &PgPool,
        rejected: Option<RejectedToken<'_>>,
    ) -> Result<Option<RemovedSignIn>, sqlx::Error> {
        let mut tx = pool.begin().await?;
        let Some((issuer, access_token, refresh_token)) =
            sqlx::query_as::<_, (String, String, Option<String>)>(
                "SELECT issuer, access_token, refresh_token FROM claude_gateway_accounts
                 FOR UPDATE",
            )
            .fetch_optional(&mut *tx)
            .await?
        else {
            return Ok(None);
        };
        let tokens = open_tokens(&access_token, refresh_token.as_deref());
        // A token that does not open is not the one the gateway rejected:
        // the engine opened that one to present it.
        let holds_rejected = match (rejected, &tokens) {
            (None, _) => true,
            (Some(_), Err(_)) => false,
            (Some(RejectedToken::Access(token)), Ok((access, _))) => access == token,
            (Some(RejectedToken::Refresh(token)), Ok((_, refresh))) => {
                refresh.as_deref() == Some(token)
            }
        };
        if !holds_rejected {
            return Ok(None);
        }
        sqlx::query("DELETE FROM claude_gateway_accounts")
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        let revocable = match tokens {
            Ok((access_token, refresh_token)) => Some(EndedSignIn {
                issuer: issuer.clone(),
                access_token,
                refresh_token,
            }),
            Err(e) => {
                crate::log!(
                    "[ClaudeGateway] Forgot the sign-in to {issuer} without revoking it: {e}"
                );
                None
            }
        };
        Ok(Some(RemovedSignIn { issuer, revocable }))
    }

    /// Store refreshed tokens for the sign-in that held `spent_refresh_token`.
    /// Returns `false` when that sign-in is gone, so a refresh racing a
    /// sign-out or a newer sign-in cannot overwrite it. A `None` refresh token
    /// keeps the stored one.
    pub(super) async fn update_tokens(
        pool: &PgPool,
        issuer: &str,
        spent_refresh_token: &str,
        tokens: &GrantedTokens,
    ) -> Result<bool, sqlx::Error> {
        let mut tx = pool.begin().await?;
        let stored: Option<(String, Option<String>)> =
            sqlx::query_as("SELECT issuer, refresh_token FROM claude_gateway_accounts FOR UPDATE")
                .fetch_optional(&mut *tx)
                .await?;
        let holds_spent = stored.is_some_and(|(stored_issuer, refresh_token)| {
            stored_issuer == issuer
                && refresh_token
                    .and_then(|sealed| {
                        secret_store::open(&ClaudeGatewayRefreshToken::SPEC, &sealed).ok()
                    })
                    .is_some_and(|refresh_token| refresh_token == spent_refresh_token)
        });
        if !holds_spent {
            return Ok(false);
        }
        sqlx::query(
            "UPDATE claude_gateway_accounts
             SET access_token = $1,
                 token_expiry = $2,
                 refresh_token = COALESCE($3, refresh_token),
                 updated_at = NOW()",
        )
        .bind(Secret::<ClaudeGatewayAccessToken>::new(
            &*tokens.access_token,
        ))
        .bind(tokens.token_expiry)
        .bind(refresh_secret(tokens))
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(true)
    }
}

/// A sign-in [`ClaudeGatewayAccountStore::delete_row`] removed. Its tokens are
/// revocable only when they opened.
struct RemovedSignIn {
    issuer: String,
    revocable: Option<EndedSignIn>,
}

/// Open a stored access token and refresh token.
fn open_tokens(
    access_token: &str,
    refresh_token: Option<&str>,
) -> Result<(String, Option<String>), BoxError> {
    let access_token = secret_store::open(&ClaudeGatewayAccessToken::SPEC, access_token)?;
    let refresh_token = refresh_token
        .map(|sealed| secret_store::open(&ClaudeGatewayRefreshToken::SPEC, sealed))
        .transpose()?;
    Ok((access_token, refresh_token))
}

fn refresh_secret(tokens: &GrantedTokens) -> Option<Secret<ClaudeGatewayRefreshToken>> {
    tokens.refresh_token.as_deref().map(Secret::new)
}
