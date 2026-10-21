//! The stored sign-in as a [`GatewayAccess`]: it presents the token only
//! to the gateway that issued it, refreshes it before it expires, and reads
//! that gateway's managed settings.
//!
//! The refresh is the gateway's own, not `core::oauth::refresh_oauth_if_needed`.
//! That one reads a stored OAuth client with a `client_id` and a fixed
//! `token_url`. The gateway is a public client with no `client_id`, and its
//! token endpoint comes from discovery.

use std::sync::Arc;

use sqlx::PgPool;

use super::account::{ClaudeGatewayAccount, ClaudeGatewayAccountStore, RejectedToken};
use super::discovery::discover;
use super::gateway_managed_settings::{self, GatewayManagedSettings};
use super::revocation::{self, EndedSignIn};
use super::token_endpoint::{post_token_form, TokenReply};
use super::{GatewayAccess, GatewayOrigin, SIGN_IN_LOCATION};
use crate::engine::event_bus::EventBus;

type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// A token this close to its expiry is refreshed first.
const REFRESH_MARGIN_SECS: i64 = 60;

/// The gateway access behind the gateway provider.
pub struct StoredGatewayAccess {
    pool: PgPool,
    event_bus: EventBus,
    origin: GatewayOrigin,
    /// Serialises refreshes. A gateway may rotate the refresh token, so two
    /// turns refreshing at once would leave one holding a spent token. Every
    /// source the engine builds shares one, since a provider rebuild makes a
    /// new source while turns on the old one still run.
    refresh_lock: Arc<RefreshLock>,
}

/// The lock every [`StoredGatewayAccess`] of one engine shares.
pub type RefreshLock = tokio::sync::Mutex<()>;

impl StoredGatewayAccess {
    /// A source for the gateway at `origin`, the configured gateway URL.
    pub fn new(
        pool: PgPool,
        event_bus: EventBus,
        origin: GatewayOrigin,
        refresh_lock: Arc<RefreshLock>,
    ) -> Self {
        Self {
            pool,
            event_bus,
            origin,
            refresh_lock,
        }
    }

    /// The stored sign-in, if `origin` issued it. A sign-in to any other
    /// gateway is refused, so its token never reaches this one.
    async fn account_for_origin(&self) -> Result<ClaudeGatewayAccount, BoxError> {
        let account = ClaudeGatewayAccountStore::get(&self.pool)
            .await
            .map_err(|e| format!("Could not read the Claude gateway sign-in: {e}"))?
            .ok_or_else(not_signed_in)?;
        if account.issuer != self.origin.as_str() {
            return Err(format!(
                "Lucidos is signed in to the Claude gateway at {}, not the one configured at {}. \
                 Sign in again in {SIGN_IN_LOCATION}.",
                account.issuer, self.origin
            )
            .into());
        }
        Ok(account)
    }

    async fn refresh(&self, account: ClaudeGatewayAccount) -> Result<String, BoxError> {
        let Some(refresh_token) = account.refresh_token.as_deref() else {
            return Err(sign_in_again());
        };
        let endpoints = discover(&self.origin)
            .await
            .map_err(|e| unreachable_gateway(&*e))?;
        let form = [
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token),
        ];
        let refusal = match post_token_form(&endpoints.token, &form).await {
            Ok(TokenReply::Granted(tokens)) => {
                let stored = ClaudeGatewayAccountStore::update_tokens(
                    &self.pool,
                    &account.issuer,
                    refresh_token,
                    &tokens,
                )
                .await
                .map_err(|e| format!("Could not store the renewed Claude gateway sign-in: {e}"))?;
                if !stored {
                    tokio::spawn(revocation::revoke(EndedSignIn::unkept(
                        &self.origin,
                        &tokens,
                    )));
                    return Err(not_signed_in());
                }
                crate::log!("[ClaudeGateway] Refreshed the token for {}", account.issuer);
                let (pool, event_bus, bearer) = (
                    self.pool.clone(),
                    self.event_bus.clone(),
                    tokens.access_token.clone(),
                );
                tokio::spawn(async move {
                    gateway_managed_settings::refresh(&pool, &event_bus, &account, &bearer).await;
                });
                return Ok(tokens.access_token);
            }
            Ok(TokenReply::Refused(refusal)) => refusal,
            Err(e) => return Err(unreachable_gateway(&*e)),
        };
        crate::log!(
            "[ClaudeGateway] {} refused the token refresh: {}",
            account.issuer,
            refusal
        );
        if refusal.error != "invalid_grant" {
            return Err(format!(
                "The Claude gateway refused to renew the sign-in ({refusal}). Sign in again in \
                 {SIGN_IN_LOCATION}."
            )
            .into());
        }
        self.forget(RejectedToken::Refresh(refresh_token)).await;
        Err(sign_in_again())
    }

    /// Forget the sign-in holding `rejected`, so Settings says to sign in
    /// again. A failure to forget still leaves the turn's own error.
    async fn forget(&self, rejected: RejectedToken<'_>) {
        if let Err(e) =
            ClaudeGatewayAccountStore::forget_rejected(&self.pool, &self.event_bus, rejected).await
        {
            crate::log!(
                "[ClaudeGateway] Could not forget the rejected sign-in: {}",
                e
            );
        }
    }
}

#[async_trait::async_trait]
impl GatewayAccess for StoredGatewayAccess {
    async fn bearer(&self) -> Result<String, BoxError> {
        let account = self.account_for_origin().await?;
        if !expiring(&account) {
            return Ok(account.access_token);
        }
        let _refreshing = self.refresh_lock.lock().await;
        // Another turn may have refreshed while this one waited.
        let account = self.account_for_origin().await?;
        if !expiring(&account) {
            return Ok(account.access_token);
        }
        self.refresh(account).await
    }

    async fn managed_settings(&self) -> Result<Option<GatewayManagedSettings>, BoxError> {
        Ok(self.account_for_origin().await?.managed_settings())
    }

    async fn renew_rejected(&self, rejected: &str) -> Result<String, BoxError> {
        let _refreshing = self.refresh_lock.lock().await;
        let account = self.account_for_origin().await?;
        // Another turn may have renewed it while this one waited.
        if account.access_token != rejected {
            return Ok(account.access_token);
        }
        if account.refresh_token.is_none() {
            self.forget(RejectedToken::Access(rejected)).await;
            return Err(sign_in_again());
        }
        self.refresh(account).await
    }
}

/// Whether the token expires within the margin. A token with no stated expiry
/// is taken as valid until the gateway answers `401`, which renews it.
fn expiring(account: &ClaudeGatewayAccount) -> bool {
    account.token_expiry.is_some_and(|expiry| {
        expiry < chrono::Utc::now() + chrono::Duration::seconds(REFRESH_MARGIN_SECS)
    })
}

fn not_signed_in() -> BoxError {
    format!("Lucidos is not signed in to the Claude gateway. Sign in in {SIGN_IN_LOCATION}.").into()
}

fn sign_in_again() -> BoxError {
    format!("The Claude gateway sign-in has expired. Sign in to the Claude gateway again in {SIGN_IN_LOCATION}.").into()
}

fn unreachable_gateway(e: &(dyn std::error::Error + Send + Sync)) -> BoxError {
    format!(
        "Could not renew the Claude gateway sign-in: {e}. If the gateway is on a private \
         network, check that you are connected to it."
    )
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::secret_store::SEALED_PREFIX;
    use crate::llm::claude_gateway::account::ClaudeGatewaySignOutReason;
    use crate::llm::claude_gateway::mock_gateway::MockGateway;
    use crate::llm::claude_gateway::GrantedTokens;
    use crate::test_support::{setup_test_db, teardown_test_db};
    use axum::http::StatusCode;
    use chrono::{Duration, Utc};

    fn granted(access: &str, refresh: Option<&str>, expires_in: i64) -> GrantedTokens {
        GrantedTokens {
            access_token: access.into(),
            refresh_token: refresh.map(str::to_string),
            token_expiry: Some(Utc::now() + Duration::seconds(expires_in)),
            email: Some("user@example.com".into()),
        }
    }

    struct Fixture {
        pool: PgPool,
        db_name: String,
        bus: EventBus,
        gateway: MockGateway,
        refresh_lock: Arc<RefreshLock>,
    }

    impl Fixture {
        async fn new() -> Self {
            let (pool, db_name) = setup_test_db().await;
            let (bus, _callback_rx) = EventBus::new(pool.clone());
            let gateway = MockGateway::start().await;
            Self {
                pool,
                db_name,
                bus,
                gateway,
                refresh_lock: Arc::default(),
            }
        }

        async fn sign_in(&self, issuer: &GatewayOrigin, tokens: GrantedTokens) {
            ClaudeGatewayAccountStore::sign_in(&self.pool, &self.bus, issuer, &tokens, None)
                .await
                .unwrap();
        }

        fn source(&self) -> StoredGatewayAccess {
            StoredGatewayAccess::new(
                self.pool.clone(),
                self.bus.clone(),
                self.gateway.origin.clone(),
                self.refresh_lock.clone(),
            )
        }

        async fn emitted(&self, event_type: &str) -> i64 {
            sqlx::query_scalar("SELECT count(*) FROM events WHERE event_type = $1")
                .bind(event_type)
                .fetch_one(&self.pool)
                .await
                .unwrap()
        }

        async fn done(self) {
            self.pool.close().await;
            teardown_test_db(&self.db_name).await;
        }
    }

    #[tokio::test]
    async fn a_fresh_token_is_presented_without_a_refresh() {
        let f = Fixture::new().await;
        f.sign_in(&f.gateway.origin, granted("fresh", Some("r"), 3600))
            .await;
        assert_eq!(f.source().bearer().await.unwrap(), "fresh");
        assert!(f.gateway.paths().is_empty(), "no refresh, no request");
        f.done().await;
    }

    #[tokio::test]
    async fn no_sign_in_says_where_to_sign_in() {
        let f = Fixture::new().await;
        let err = f.source().bearer().await.unwrap_err().to_string();
        assert!(
            err.contains("not signed in") && err.contains(SIGN_IN_LOCATION),
            "{err}"
        );
        f.done().await;
    }

    /// A token is never presented to a gateway other than its issuer, even
    /// before it expires.
    #[tokio::test]
    async fn a_sign_in_to_another_gateway_is_refused() {
        let f = Fixture::new().await;
        let other = GatewayOrigin::parse("https://gateway.example").unwrap();
        f.sign_in(&other, granted("theirs", Some("r"), 3600)).await;
        let err = f.source().bearer().await.unwrap_err().to_string();
        assert!(
            err.contains("https://gateway.example") && err.contains("Sign in again"),
            "{err}"
        );
        assert!(f.gateway.paths().is_empty());
        f.done().await;
    }

    /// An expired token refreshes once before the call, and the rotated
    /// refresh token is stored.
    #[tokio::test]
    async fn an_expired_token_refreshes_before_the_call() {
        let f = Fixture::new().await;
        f.sign_in(&f.gateway.origin, granted("stale", Some("r1"), -10))
            .await;
        f.gateway.reply_to_token(
            StatusCode::OK,
            serde_json::json!({"access_token": "renewed", "refresh_token": "r2", "expires_in": 3600}),
        );

        assert_eq!(f.source().bearer().await.unwrap(), "renewed");
        let forms = f.gateway.token_forms();
        assert_eq!(forms.len(), 1);
        assert_eq!(forms[0]["grant_type"], "refresh_token");
        assert_eq!(forms[0]["refresh_token"], "r1");
        assert!(
            !forms[0].contains_key("client_id"),
            "a public client sends none"
        );

        let stored = ClaudeGatewayAccountStore::get(&f.pool)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(stored.access_token, "renewed");
        assert_eq!(stored.refresh_token.as_deref(), Some("r2"));
        assert_eq!(
            f.emitted("ClaudeGatewaySignedIn").await,
            1,
            "a refresh is silent"
        );
        f.done().await;
    }

    /// Both tokens are sealed at rest, after a sign-in and after a refresh
    /// rewrites them (ADR 0509).
    #[tokio::test]
    async fn the_stored_tokens_are_sealed() {
        let f = Fixture::new().await;
        let raw = || async {
            sqlx::query_as::<_, (String, Option<String>)>(
                "SELECT access_token, refresh_token FROM claude_gateway_accounts",
            )
            .fetch_one(&f.pool)
            .await
            .unwrap()
        };
        let sealed = |(access, refresh): (String, Option<String>)| {
            access.starts_with(SEALED_PREFIX)
                && refresh.is_some_and(|refresh| refresh.starts_with(SEALED_PREFIX))
        };
        f.sign_in(&f.gateway.origin, granted("stale", Some("r1"), -10))
            .await;
        assert!(sealed(raw().await), "after the sign-in");
        f.gateway.reply_to_token(
            StatusCode::OK,
            serde_json::json!({"access_token": "renewed", "refresh_token": "r2", "expires_in": 3600}),
        );
        assert_eq!(f.source().bearer().await.unwrap(), "renewed");
        assert!(sealed(raw().await), "after the refresh");
        f.done().await;
    }

    /// Two turns needing a refresh at once spend the refresh token once,
    /// even on two sources: a provider rebuild makes a new one mid-turn.
    #[tokio::test]
    async fn concurrent_calls_refresh_once() {
        let f = Fixture::new().await;
        f.sign_in(&f.gateway.origin, granted("stale", Some("r1"), 30))
            .await;
        f.gateway.reply_to_token(
            StatusCode::OK,
            serde_json::json!({"access_token": "renewed", "expires_in": 3600}),
        );
        let (first, second) = (f.source(), f.source());
        let (a, b) = tokio::join!(first.bearer(), second.bearer());
        assert_eq!(
            (a.unwrap(), b.unwrap()),
            ("renewed".into(), "renewed".into())
        );
        assert_eq!(f.gateway.token_forms().len(), 1);
        let stored = ClaudeGatewayAccountStore::get(&f.pool)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            stored.refresh_token.as_deref(),
            Some("r1"),
            "kept when none is rotated in"
        );
        f.done().await;
    }

    /// A rejected refresh token ends the sign-in and says to sign in again,
    /// rather than failing silently or falling back.
    #[tokio::test]
    async fn invalid_grant_signs_out_and_says_sign_in_again() {
        let f = Fixture::new().await;
        f.sign_in(&f.gateway.origin, granted("stale", Some("r1"), -10))
            .await;
        f.gateway.refuse_token("invalid_grant");

        let err = f.source().bearer().await.unwrap_err().to_string();
        assert!(err.contains("Sign in to the Claude gateway again"), "{err}");
        assert!(ClaudeGatewayAccountStore::get(&f.pool)
            .await
            .unwrap()
            .is_none());
        assert_eq!(f.emitted("ClaudeGatewaySignedOut").await, 1);
        assert!(ClaudeGatewayAccountStore::rejected_last(&f.pool)
            .await
            .unwrap());
        f.done().await;
    }

    #[tokio::test]
    async fn another_refusal_keeps_the_sign_in() {
        let f = Fixture::new().await;
        f.sign_in(&f.gateway.origin, granted("stale", Some("r1"), -10))
            .await;
        f.gateway.refuse_token("temporarily_unavailable");
        let err = f.source().bearer().await.unwrap_err().to_string();
        assert!(err.contains("temporarily_unavailable"), "{err}");
        assert!(ClaudeGatewayAccountStore::get(&f.pool)
            .await
            .unwrap()
            .is_some());
        f.done().await;
    }

    #[tokio::test]
    async fn an_expired_token_with_no_refresh_token_says_sign_in_again() {
        let f = Fixture::new().await;
        f.sign_in(&f.gateway.origin, granted("stale", None, -10))
            .await;
        let err = f.source().bearer().await.unwrap_err().to_string();
        assert!(err.contains("again"), "{err}");
        assert!(f.gateway.paths().is_empty());
        f.done().await;
    }

    /// The refresh rejected the OLD token. A sign-in made since holds a new
    /// one, and survives.
    #[tokio::test]
    async fn forgetting_a_rejected_token_keeps_a_newer_sign_in() {
        let f = Fixture::new().await;
        f.sign_in(&f.gateway.origin, granted("new", Some("r-new"), 3600))
            .await;
        let forgot = ClaudeGatewayAccountStore::forget_rejected(
            &f.pool,
            &f.bus,
            RejectedToken::Refresh("r-old"),
        )
        .await
        .unwrap();
        assert!(!forgot);
        assert!(ClaudeGatewayAccountStore::get(&f.pool)
            .await
            .unwrap()
            .is_some());
        assert_eq!(f.emitted("ClaudeGatewaySignedOut").await, 0);
        f.done().await;
    }

    /// A token the gateway ended early refreshes, even before its expiry.
    #[tokio::test]
    async fn a_rejected_token_renews() {
        let f = Fixture::new().await;
        f.sign_in(&f.gateway.origin, granted("ended", Some("r1"), 3600))
            .await;
        f.gateway.reply_to_token(
            StatusCode::OK,
            serde_json::json!({"access_token": "renewed", "expires_in": 3600}),
        );
        let source = f.source();
        assert_eq!(source.renew_rejected("ended").await.unwrap(), "renewed");
        assert_eq!(
            source.renew_rejected("ended").await.unwrap(),
            "renewed",
            "a second turn takes the renewed token"
        );
        assert_eq!(f.gateway.token_forms().len(), 1);
        f.done().await;
    }

    /// A rejected token with nothing to renew it ends the sign-in, so
    /// Settings says to sign in again rather than "signed in".
    #[tokio::test]
    async fn a_rejected_token_with_no_refresh_token_signs_out() {
        let f = Fixture::new().await;
        f.sign_in(&f.gateway.origin, granted("ended", None, 3600))
            .await;
        let err = f.source().renew_rejected("ended").await.unwrap_err();
        assert!(err.to_string().contains("again"), "{err}");
        assert!(ClaudeGatewayAccountStore::get(&f.pool)
            .await
            .unwrap()
            .is_none());
        assert!(ClaudeGatewayAccountStore::rejected_last(&f.pool)
            .await
            .unwrap());
        f.done().await;
    }

    /// A refresh that spent the old refresh token stores nothing over a
    /// sign-in made since.
    #[tokio::test]
    async fn a_refresh_never_overwrites_a_newer_sign_in() {
        let f = Fixture::new().await;
        f.sign_in(&f.gateway.origin, granted("new", Some("r-new"), 3600))
            .await;
        let stored = ClaudeGatewayAccountStore::update_tokens(
            &f.pool,
            f.gateway.origin.as_str(),
            "r-old",
            &granted("from-old", Some("r-old-2"), 3600),
        )
        .await
        .unwrap();
        assert!(!stored);
        let account = ClaudeGatewayAccountStore::get(&f.pool)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(account.access_token, "new");
        assert_eq!(account.refresh_token.as_deref(), Some("r-new"));
        f.done().await;
    }

    /// A sign-in and a sign-out announce once each, carrying no token. A
    /// repeated sign-out announces nothing.
    #[tokio::test]
    async fn sign_in_and_sign_out_announce_without_tokens() {
        let f = Fixture::new().await;
        f.sign_in(
            &f.gateway.origin,
            granted("secret-access", Some("secret-refresh"), 3600),
        )
        .await;
        assert!(ClaudeGatewayAccountStore::sign_out(
            &f.pool,
            &f.bus,
            ClaudeGatewaySignOutReason::SignedOut,
            None,
        )
        .await
        .unwrap());
        assert!(!ClaudeGatewayAccountStore::sign_out(
            &f.pool,
            &f.bus,
            ClaudeGatewaySignOutReason::SignedOut,
            None,
        )
        .await
        .unwrap());
        assert_eq!(f.emitted("ClaudeGatewaySignedIn").await, 1);
        assert_eq!(f.emitted("ClaudeGatewaySignedOut").await, 1);
        assert!(!ClaudeGatewayAccountStore::rejected_last(&f.pool)
            .await
            .unwrap());
        let payloads: Vec<String> = sqlx::query_scalar(
            "SELECT payload::text FROM events WHERE event_type LIKE 'ClaudeGateway%'",
        )
        .fetch_all(&f.pool)
        .await
        .unwrap();
        assert!(
            payloads.iter().all(|p| !p.contains("secret")),
            "{payloads:?}"
        );
        assert!(
            payloads
                .iter()
                .all(|p| p.contains(f.gateway.origin.as_str())),
            "{payloads:?}"
        );
        f.done().await;
    }

    /// A sign-out revokes both tokens at the issuer after forgetting them. A
    /// rejected sign-in holds nothing worth revoking.
    #[tokio::test]
    async fn a_sign_out_revokes_and_a_rejection_does_not() {
        let f = Fixture::new().await;
        f.sign_in(&f.gateway.origin, granted("access", Some("refresh"), 3600))
            .await;
        ClaudeGatewayAccountStore::sign_out(
            &f.pool,
            &f.bus,
            ClaudeGatewaySignOutReason::SignedOut,
            None,
        )
        .await
        .unwrap();
        let revoked = || {
            f.gateway
                .seen()
                .into_iter()
                .filter(|s| s.path == "/oauth/revoke")
                .map(|s| s.form["token"].clone())
                .collect::<Vec<_>>()
        };
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
        while revoked().len() < 2 && tokio::time::Instant::now() < deadline {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        assert_eq!(revoked(), vec!["access", "refresh"]);

        f.sign_in(&f.gateway.origin, granted("ended", Some("r2"), 3600))
            .await;
        ClaudeGatewayAccountStore::forget_rejected(&f.pool, &f.bus, RejectedToken::Refresh("r2"))
            .await
            .unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        assert_eq!(revoked().len(), 2, "a rejected sign-in is not revoked");
        f.done().await;
    }

    /// The gateway token lives apart from `oauth_accounts`. So no reader of
    /// that table can hand it out: script env vars, `http_request`, the app
    /// proxy and the access-token endpoint all read only there.
    #[tokio::test]
    async fn no_generic_oauth_reader_sees_the_gateway_token() {
        let f = Fixture::new().await;
        f.sign_in(
            &f.gateway.origin,
            granted("secret-access", Some("secret-refresh"), 3600),
        )
        .await;
        assert!(crate::core::OAuthStore::list_all_with_tokens(&f.pool)
            .await
            .unwrap()
            .is_empty());
        assert!(crate::core::OAuthStore::list(&f.pool)
            .await
            .unwrap()
            .is_empty());
        f.done().await;
    }

    #[test]
    fn debug_never_shows_a_token() {
        let account = ClaudeGatewayAccount {
            issuer: "https://gateway.example".into(),
            email: None,
            access_token: "secret-access".into(),
            refresh_token: Some("secret-refresh".into()),
            token_expiry: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            managed_settings: None,
            managed_settings_etag: None,
            managed_settings_fetched_at: None,
            managed_telemetry_since: None,
        };
        let shown = format!("{account:?}");
        assert!(!shown.contains("secret"), "{shown}");
    }

    /// A sign-in stored with no email reports the one its access token names,
    /// and a stored email still wins.
    #[test]
    fn the_reported_account_falls_back_to_the_access_token_email() {
        use crate::llm::claude_gateway::account::ClaudeGatewayAccountInfo;
        use crate::llm::claude_gateway::token_endpoint::tests::id_token;
        let mut account = ClaudeGatewayAccount {
            issuer: "https://gateway.example".into(),
            email: None,
            access_token: id_token(serde_json::json!({"email": "access@example.com"})),
            refresh_token: None,
            token_expiry: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            managed_settings: None,
            managed_settings_etag: None,
            managed_settings_fetched_at: None,
            managed_telemetry_since: None,
        };
        let info = ClaudeGatewayAccountInfo::from(&account);
        assert_eq!(info.email.as_deref(), Some("access@example.com"));
        account.email = Some("stored@example.com".into());
        let info = ClaudeGatewayAccountInfo::from(&account);
        assert_eq!(info.email.as_deref(), Some("stored@example.com"));
    }
}
