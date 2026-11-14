use axum::http::StatusCode;
use chrono::{Duration, Utc};

use super::*;
use crate::core::secret_store::SEALED_PREFIX;
use crate::engine::thread_events::MessageOrigin;
use crate::llm::chatgpt_plan::account::{ChatGptPlanAccountInfo, CompletedSignIn};
use crate::llm::chatgpt_plan::id_token::Identity;
use crate::llm::chatgpt_plan::mock_auth::MockAuth;
use crate::llm::chatgpt_plan::token_endpoint::GrantedTokens;
use crate::test_support::{setup_test_db, teardown_test_db};

/// Tokens due a refresh `due_in` seconds from now.
fn granted(access: &str, refresh: &str, due_in: i64) -> GrantedTokens {
    GrantedTokens {
        access_token: access.into(),
        refresh_token: Some(refresh.into()),
        id_token: Some("id-token".into()),
        token_expiry: Some(Utc::now() + Duration::seconds(due_in + 360)),
        refresh_due_at: Some(Utc::now() + Duration::seconds(due_in)),
    }
}

fn renewed_reply(access: &str, refresh: &str) -> serde_json::Value {
    serde_json::json!({
        "access_token": access,
        "refresh_token": refresh,
        "id_token": "id-token-2",
        "expires_in": 3600,
        "earliest_refresh_at": Utc::now().timestamp() + 3240,
    })
}

struct Fixture {
    pool: PgPool,
    db_name: String,
    bus: EventBus,
    mock: MockAuth,
    refresh_lock: Arc<RefreshLock>,
}

impl Fixture {
    async fn new() -> Self {
        let (pool, db_name) = setup_test_db().await;
        let (bus, _callback_rx) = EventBus::new(pool.clone());
        Self {
            pool,
            db_name,
            bus,
            mock: MockAuth::start().await,
            refresh_lock: Arc::default(),
        }
    }

    async fn sign_in(&self, tokens: GrantedTokens) {
        ChatGptPlanAccountStore::registration(&self.pool)
            .await
            .unwrap();
        let signed_in = CompletedSignIn {
            client_id: "oaiapp_1".into(),
            identity: Identity {
                issuer: self.mock.auth.origin().into(),
                subject: "user-123".into(),
                email: Some("user@example.com".into()),
            },
            id_token: "id-token".into(),
            tokens,
        };
        ChatGptPlanAccountStore::sign_in(&self.pool, &self.bus, &signed_in, None)
            .await
            .unwrap();
    }

    fn source(&self) -> StoredChatGptPlanTokens {
        self.source_at(self.mock.auth.clone())
    }

    fn source_at(&self, auth: AuthServer) -> StoredChatGptPlanTokens {
        StoredChatGptPlanTokens::new(
            self.pool.clone(),
            self.bus.clone(),
            auth,
            self.refresh_lock.clone(),
        )
    }

    async fn account(&self) -> Option<ChatGptPlanAccount> {
        ChatGptPlanAccountStore::get(&self.pool).await.unwrap()
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
async fn a_token_not_yet_due_is_handed_out_without_a_refresh() {
    let f = Fixture::new().await;
    f.sign_in(granted("fresh", "r1", 600)).await;
    assert_eq!(f.source().access_token().await.unwrap(), "fresh");
    assert!(f.mock.token_forms().is_empty());
    f.done().await;
}

#[tokio::test]
async fn no_sign_in_says_where_to_sign_in() {
    let f = Fixture::new().await;
    let err = f.source().access_token().await.unwrap_err().to_string();
    assert!(
        err.contains("not signed in") && err.contains(SIGN_IN_LOCATION),
        "{err}"
    );
    f.done().await;
}

/// A token past `earliest_refresh_at` refreshes once, silently, and the
/// rotated refresh token is stored with its access token.
#[tokio::test]
async fn a_due_token_refreshes_and_stores_the_rotated_refresh_token() {
    let f = Fixture::new().await;
    f.sign_in(granted("stale", "r1", -10)).await;
    f.mock
        .reply_to_token(StatusCode::OK, renewed_reply("renewed", "r2"));

    assert_eq!(f.source().access_token().await.unwrap(), "renewed");
    let forms = f.mock.token_forms();
    assert_eq!(forms.len(), 1);
    assert_eq!(forms[0]["grant_type"], "refresh_token");
    assert_eq!(forms[0]["client_id"], "oaiapp_1");
    assert_eq!(forms[0]["refresh_token"], "r1");
    let account = f.account().await.unwrap();
    assert_eq!(account.access_token, "renewed");
    assert_eq!(account.refresh_token, "r2");
    assert!(!account.refresh_due(Utc::now()));
    assert_eq!(
        f.emitted("ChatGptPlanSignedIn").await,
        1,
        "a refresh is silent"
    );
    f.done().await;
}

/// Many calls at a due token, on two sources, spend the refresh token once.
/// The mock refuses a reused refresh token as OpenAI does.
#[tokio::test]
async fn concurrent_calls_refresh_exactly_once() {
    let f = Fixture::new().await;
    f.sign_in(granted("stale", "r1", -10)).await;
    f.mock
        .reply_to_token(StatusCode::OK, renewed_reply("renewed", "r2"));
    let (first, second) = (f.source(), f.source());
    let calls = (0..8).map(|i| {
        let source = if i % 2 == 0 { &first } else { &second };
        source.access_token()
    });
    for token in futures::future::join_all(calls).await {
        assert_eq!(token.unwrap(), "renewed");
    }
    assert_eq!(f.mock.token_forms().len(), 1);
    assert_eq!(f.emitted("ChatGptPlanSignedOut").await, 0);
    f.done().await;
}

/// A token OpenAI rejected refreshes once, and a second caller holding the
/// same rejected token takes the renewed one.
#[tokio::test]
async fn a_rejected_token_renews_once() {
    let f = Fixture::new().await;
    f.sign_in(granted("ended", "r1", 600)).await;
    f.mock
        .reply_to_token(StatusCode::OK, renewed_reply("renewed", "r2"));
    let source = f.source();
    assert_eq!(source.after_rejection("ended").await.unwrap(), "renewed");
    assert_eq!(source.after_rejection("ended").await.unwrap(), "renewed");
    assert_eq!(f.mock.token_forms().len(), 1);
    f.done().await;
}

/// Every refresh code OpenAI documents as terminal clears the tokens, keeps
/// the registration, and says to sign in again.
#[tokio::test]
async fn a_terminal_refresh_error_clears_the_tokens_and_keeps_the_registration() {
    for code in [
        "invalid_grant",
        "invalid_refresh_token",
        "token_expired",
        "refresh_token_expired",
        "refresh_token_invalidated",
        "refresh_token_reused",
    ] {
        let f = Fixture::new().await;
        f.sign_in(granted("stale", "r1", -10)).await;
        let host_id = ChatGptPlanAccountStore::registration(&f.pool)
            .await
            .unwrap()
            .host_id;
        f.mock.reply_to_token(
            StatusCode::BAD_REQUEST,
            serde_json::json!({"error": {"code": code, "message": "no"}}),
        );
        let err = f.source().access_token().await.unwrap_err().to_string();
        assert!(err.contains("Sign in with ChatGPT again"), "{code}: {err}");
        assert!(f.account().await.is_none(), "{code}");
        let registration = ChatGptPlanAccountStore::registration(&f.pool)
            .await
            .unwrap();
        assert_eq!(registration.host_id, host_id, "{code}");
        assert_eq!(
            registration.client_id.as_deref(),
            Some("oaiapp_1"),
            "{code}"
        );
        assert_eq!(f.emitted("ChatGptPlanSignedOut").await, 1, "{code}");
        assert!(
            ChatGptPlanAccountStore::rejected_last(&f.pool)
                .await
                .unwrap(),
            "{code}"
        );
        f.done().await;
    }
}

/// A refusal OpenAI does not document as terminal, a server error with no
/// code, and an unreachable host all keep the tokens.
#[tokio::test]
async fn a_temporary_refresh_failure_keeps_the_tokens() {
    let replies = [
        (
            StatusCode::BAD_REQUEST,
            serde_json::json!({"error": "invalid_client"}),
        ),
        (
            StatusCode::SERVICE_UNAVAILABLE,
            serde_json::json!({"error": "temporarily_unavailable"}),
        ),
        (
            StatusCode::BAD_GATEWAY,
            serde_json::json!({"detail": "upstream"}),
        ),
    ];
    for (status, body) in replies {
        let f = Fixture::new().await;
        f.sign_in(granted("stale", "r1", -10)).await;
        f.mock.reply_to_token(status, body.clone());
        assert!(f.source().access_token().await.is_err(), "{body}");
        assert_eq!(f.account().await.unwrap().refresh_token, "r1", "{body}");
        assert_eq!(f.emitted("ChatGptPlanSignedOut").await, 0, "{body}");
        f.done().await;
    }

    let f = Fixture::new().await;
    f.sign_in(granted("stale", "r1", -10)).await;
    let unreachable = AuthServer::parse("http://127.0.0.1:1").unwrap();
    let err = f.source_at(unreachable).access_token().await.unwrap_err();
    assert!(err.to_string().contains("Try again"), "{err}");
    assert!(f.account().await.is_some());
    f.done().await;
}

/// A refusal of an old refresh token leaves a sign-in made since.
#[tokio::test]
async fn forgetting_a_rejected_token_keeps_a_newer_sign_in() {
    let f = Fixture::new().await;
    f.sign_in(granted("new", "r-new", 600)).await;
    assert!(
        !ChatGptPlanAccountStore::forget_rejected(&f.pool, &f.bus, "r-old")
            .await
            .unwrap()
    );
    assert!(f.account().await.is_some());
    assert_eq!(f.emitted("ChatGptPlanSignedOut").await, 0);
    f.done().await;
}

/// A refresh that spent an old refresh token stores nothing over a newer
/// sign-in.
#[tokio::test]
async fn a_refresh_never_overwrites_a_newer_sign_in() {
    let f = Fixture::new().await;
    f.sign_in(granted("new", "r-new", 600)).await;
    let stored = ChatGptPlanAccountStore::update_tokens(
        &f.pool,
        "r-old",
        &granted("from-old", "r-old-2", 600),
    )
    .await
    .unwrap();
    assert!(!stored);
    assert_eq!(f.account().await.unwrap().access_token, "new");
    f.done().await;
}

/// A refresh that loses the race to a sign-out revokes the token OpenAI just
/// rotated to. No grant stays live for a forgotten sign-in.
#[tokio::test]
async fn a_refresh_that_loses_to_a_sign_out_revokes_its_rotated_token() {
    let f = Fixture::new().await;
    f.sign_in(granted("stale", "r1", -10)).await;
    let account = f.account().await.unwrap();
    ChatGptPlanAccountStore::sign_out(&f.pool, &f.bus, &f.mock.auth, None)
        .await
        .unwrap();
    f.mock
        .reply_to_token(StatusCode::OK, renewed_reply("renewed", "r2"));
    let err = f.source().refresh(account).await.unwrap_err();
    assert!(err.to_string().contains("not signed in"), "{err}");

    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    while f.mock.revoke_forms().len() < 2 && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    let mut revoked: Vec<String> = f
        .mock
        .revoke_forms()
        .iter()
        .map(|form| form["token"].clone())
        .collect();
    revoked.sort();
    assert_eq!(revoked, ["r1", "r2"]);
    f.done().await;
}

/// All three tokens are sealed at rest, after a sign-in and after a refresh.
#[tokio::test]
async fn the_stored_tokens_are_sealed() {
    let f = Fixture::new().await;
    let sealed = || async {
        let (id, access, refresh): (String, String, String) = sqlx::query_as(
            "SELECT id_token, access_token, refresh_token FROM chatgpt_plan_accounts",
        )
        .fetch_one(&f.pool)
        .await
        .unwrap();
        [id, access, refresh]
            .iter()
            .all(|token| token.starts_with(SEALED_PREFIX))
    };
    f.sign_in(granted("stale", "r1", -10)).await;
    assert!(sealed().await, "after the sign-in");
    f.mock
        .reply_to_token(StatusCode::OK, renewed_reply("renewed", "r2"));
    f.source().access_token().await.unwrap();
    assert!(sealed().await, "after the refresh");
    f.done().await;
}

/// A sign-out clears the tokens, keeps the registration, announces once with
/// no token, and revokes the refresh token at OpenAI.
#[tokio::test]
async fn a_sign_out_announces_without_tokens_and_revokes() {
    let f = Fixture::new().await;
    f.sign_in(granted("secret-access", "secret-refresh", 600))
        .await;
    let actor = Some(MessageOrigin::Device {
        device_id: "laptop".into(),
    });
    assert!(
        ChatGptPlanAccountStore::sign_out(&f.pool, &f.bus, &f.mock.auth, actor.clone())
            .await
            .unwrap()
    );
    assert!(
        !ChatGptPlanAccountStore::sign_out(&f.pool, &f.bus, &f.mock.auth, actor)
            .await
            .unwrap()
    );
    assert!(f.account().await.is_none());
    assert_eq!(f.emitted("ChatGptPlanSignedOut").await, 1);
    assert!(!ChatGptPlanAccountStore::rejected_last(&f.pool)
        .await
        .unwrap());
    let payloads: Vec<String> =
        sqlx::query_scalar("SELECT payload::text FROM events WHERE event_type LIKE 'ChatGptPlan%'")
            .fetch_all(&f.pool)
            .await
            .unwrap();
    assert!(
        payloads.iter().all(|p| !p.contains("secret")),
        "{payloads:?}"
    );
    assert!(
        payloads.iter().any(|p| p.contains("laptop")),
        "{payloads:?}"
    );

    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    while f.mock.revoke_forms().is_empty() && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    let revoked = f.mock.revoke_forms();
    assert_eq!(revoked.len(), 1);
    assert_eq!(revoked[0]["token"], "secret-refresh");
    assert_eq!(revoked[0]["client_id"], "oaiapp_1");
    f.done().await;
}

/// The plan tokens live apart from `oauth_accounts`, so no reader of that
/// table can hand them out.
#[tokio::test]
async fn no_generic_oauth_reader_sees_the_plan_token() {
    let f = Fixture::new().await;
    f.sign_in(granted("secret-access", "secret-refresh", 600))
        .await;
    assert!(crate::core::OAuthStore::list_all_with_tokens(&f.pool)
        .await
        .unwrap()
        .is_empty());
    f.done().await;
}

#[tokio::test]
async fn debug_and_the_reported_account_never_show_a_token() {
    let f = Fixture::new().await;
    f.sign_in(granted("secret-access", "secret-refresh", 600))
        .await;
    let account = f.account().await.unwrap();
    let registration = ChatGptPlanAccountStore::registration(&f.pool)
        .await
        .unwrap();
    let info = serde_json::to_string(&ChatGptPlanAccountInfo::from(&account)).unwrap();
    for shown in [format!("{account:?}"), format!("{registration:?}"), info] {
        assert!(!shown.contains("secret"), "{shown}");
        assert!(!shown.contains("id-token"), "{shown}");
    }
    f.done().await;
}
