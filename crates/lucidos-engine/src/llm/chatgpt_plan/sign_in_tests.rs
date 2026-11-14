use std::collections::HashMap;

use axum::http::StatusCode;
use sqlx::PgPool;

use super::*;
use crate::core::oauth_callback::BrowserContext;
use crate::llm::chatgpt_plan::account::SignOutOutcome;
use crate::llm::chatgpt_plan::id_token::tests::signed_id_token;
use crate::llm::chatgpt_plan::mock_auth::MockAuth;
use crate::test_support::{setup_test_db, teardown_test_db};

/// The redirect URI OpenAI's docs require, pinned as a wire contract.
const REDIRECT_URI: &str = "http://127.0.0.1:14981/oauth/callback";

fn laptop() -> Option<MessageOrigin> {
    Some(MessageOrigin::Device {
        device_id: "laptop".to_string(),
    })
}

struct Fixture {
    pool: PgPool,
    db_name: String,
    bus: EventBus,
    mock: MockAuth,
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
        }
    }

    async fn begin(&self) -> PendingSignIn {
        self.begin_by(&CallbackRoute::OwnListener).await
    }

    async fn begin_by(&self, route: &CallbackRoute) -> PendingSignIn {
        PendingSignIn::begin(&self.pool, self.mock.auth.clone(), "dev", route)
            .await
            .unwrap()
    }

    /// Queue a grant whose `id_token` answers `pending` for `client_id`.
    fn grant_for(&self, pending: &PendingSignIn, client_id: &str) {
        let nonce = param(pending, "nonce").unwrap();
        self.mock.reply_to_token(
            StatusCode::OK,
            serde_json::json!({
                "access_token": "access-1",
                "refresh_token": "refresh-1",
                "id_token": signed_id_token(self.mock.auth.origin(), client_id, &nonce),
                "token_type": "Bearer",
                "expires_in": 3600,
                "earliest_refresh_at": chrono::Utc::now().timestamp() + 3240,
            }),
        );
    }

    async fn complete(
        &self,
        pending: PendingSignIn,
        client_id: Option<&str>,
    ) -> Result<ChatGptPlanAccountInfo, BoxError> {
        pending
            .complete(
                &self.pool,
                &self.bus,
                AuthorizationGrant {
                    code: "the-code".into(),
                    client_id: client_id.map(str::to_string),
                },
                laptop(),
            )
            .await
    }

    /// A first sign-in, completed as `oaiapp_1`.
    async fn sign_in_first(&self) {
        let pending = self.begin().await;
        self.grant_for(&pending, "oaiapp_1");
        self.complete(pending, Some("oaiapp_1")).await.unwrap();
    }

    async fn stored_client_id(&self) -> Option<String> {
        sqlx::query_scalar("SELECT client_id FROM chatgpt_plan_accounts")
            .fetch_one(&self.pool)
            .await
            .unwrap()
    }

    /// The revoke requests the mock received, once the first has arrived.
    async fn revoked(&self) -> Vec<HashMap<String, String>> {
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
        while self.mock.revoke_forms().is_empty() && tokio::time::Instant::now() < deadline {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        self.mock.revoke_forms()
    }

    async fn done(self) {
        self.pool.close().await;
        teardown_test_db(&self.db_name).await;
    }
}

fn params(pending: &PendingSignIn) -> HashMap<String, String> {
    reqwest::Url::parse(pending.authorize_url())
        .unwrap()
        .query_pairs()
        .into_owned()
        .collect()
}

fn param(pending: &PendingSignIn, key: &str) -> Option<String> {
    params(pending).remove(key)
}

/// The first sign-in registers through the dynamic entrypoint, and every
/// later request uses the id OpenAI issued. The entrypoint is never stored.
#[tokio::test]
async fn the_first_sign_in_registers_and_stores_the_issued_client_id() {
    let f = Fixture::new().await;
    let pending = f.begin().await;
    let sent = params(&pending);
    assert!(pending
        .authorize_url()
        .starts_with(&format!("{}/api/accounts/authorize?", f.mock.auth.origin())));
    assert_eq!(sent["client_id"], DYNAMIC_CLIENT_ID);
    assert_eq!(sent["agent_name_hint"], "Lucidos (dev)");
    assert_eq!(sent["response_type"], "code");
    assert_eq!(sent["redirect_uri"], REDIRECT_URI);
    assert_eq!(
        sent["scope"],
        "openid profile email offline_access resource.invoke chatgpt.tokens.use.direct"
    );
    assert_eq!(sent["resource"], "https://api.openai.com/v1");
    assert_eq!(sent["code_challenge_method"], "S256");
    assert_eq!(sent["state"], pending.state());
    assert!(sent["ext_agent_host_id"].starts_with("urn:uuid:"));
    assert!(!sent.contains_key("id_token_hint"));

    f.grant_for(&pending, "oaiapp_1");
    let challenge = sent["code_challenge"].clone();
    let info = f.complete(pending, Some("oaiapp_1")).await.unwrap();
    assert_eq!(info.email.as_deref(), Some("user@example.com"));

    let forms = f.mock.token_forms();
    assert_eq!(forms.len(), 1);
    let form = &forms[0];
    assert_eq!(form["grant_type"], "authorization_code");
    assert_eq!(form["client_id"], "oaiapp_1");
    assert_eq!(form["code"], "the-code");
    assert_eq!(form["redirect_uri"], REDIRECT_URI);
    assert_eq!(form["resource"], "https://api.openai.com/v1");
    assert_eq!(
        crate::core::oauth::Pkce::challenge_for(&form["code_verifier"]),
        challenge
    );
    assert_eq!(f.stored_client_id().await.as_deref(), Some("oaiapp_1"));
    f.done().await;
}

/// A later sign-in sends the issued id with the last ID token as a hint, and
/// keeps the host id. A callback that omits the client id is accepted.
#[tokio::test]
async fn a_later_sign_in_reuses_the_issued_client_id() {
    let f = Fixture::new().await;
    let first = f.begin().await;
    let host_id = param(&first, "ext_agent_host_id").unwrap();
    f.grant_for(&first, "oaiapp_1");
    f.complete(first, Some("oaiapp_1")).await.unwrap();
    ChatGptPlanAccountStore::sign_out(&f.pool, &f.bus, &f.mock.auth, None)
        .await
        .unwrap();

    let later = f.begin().await;
    let sent = params(&later);
    assert_eq!(sent["client_id"], "oaiapp_1");
    assert_eq!(sent["ext_agent_host_id"], host_id);
    assert!(sent.contains_key("id_token_hint"));
    assert!(!sent.contains_key("agent_name_hint"));
    f.grant_for(&later, "oaiapp_1");
    f.complete(later, None).await.unwrap();
    let forms = f.mock.token_forms();
    assert_eq!(forms.last().unwrap()["client_id"], "oaiapp_1");
    assert!(forms
        .iter()
        .all(|form| form["client_id"] != DYNAMIC_CLIENT_ID));
    f.done().await;
}

/// A gateway from before ADR 0636 forwards only `code` and `state`, which
/// parse to a grant with no client id. The refusal names the gateway, and
/// nothing reaches the token endpoint or the store.
#[tokio::test]
async fn a_first_sign_in_relayed_by_an_old_gateway_names_the_gateway() {
    let f = Fixture::new().await;
    let route = CallbackRoute::Gateway(BrowserContext {
        prefix: "/dev@cloud/".into(),
        local: true,
        origin: None,
    });
    let pending = f.begin_by(&route).await;
    assert!(pending.state().starts_with("/dev@cloud/~"));
    let err = f.complete(pending, None).await.unwrap_err();
    assert_eq!(err.to_string(), RELAY_DROPPED_CLIENT_ID);
    assert!(f.mock.token_forms().is_empty());
    assert_eq!(f.stored_client_id().await, None);
    f.done().await;
}

/// A callback naming another client id, or the registration entrypoint, is
/// refused before any token request.
#[tokio::test]
async fn a_mismatched_client_id_is_refused_without_a_token_request() {
    let f = Fixture::new().await;
    let pending = f.begin().await;
    assert!(f.complete(pending, Some(DYNAMIC_CLIENT_ID)).await.is_err());
    assert!(f.mock.token_forms().is_empty());

    f.sign_in_first().await;
    let pending = f.begin().await;
    let err = f.complete(pending, Some("oaiapp_2")).await.unwrap_err();
    assert!(err.to_string().contains("different client id"), "{err}");
    assert_eq!(f.mock.token_forms().len(), 1, "only the first sign-in's");
    assert_eq!(f.stored_client_id().await.as_deref(), Some("oaiapp_1"));
    f.done().await;
}

/// The registered client id cannot bring in another ChatGPT account.
#[tokio::test]
async fn a_later_sign_in_as_another_account_is_refused() {
    use crate::llm::chatgpt_plan::id_token::tests::signed_id_token_for;
    let f = Fixture::new().await;
    f.sign_in_first().await;
    let later = f.begin().await;
    let nonce = param(&later, "nonce").unwrap();
    f.mock.reply_to_token(
        StatusCode::OK,
        serde_json::json!({
            "access_token": "access-2",
            "refresh_token": "refresh-2",
            "id_token": signed_id_token_for("someone-else", f.mock.auth.origin(), "oaiapp_1", &nonce),
            "expires_in": 3600,
        }),
    );
    let err = f.complete(later, None).await.unwrap_err();
    assert!(err.to_string().contains("another ChatGPT account"), "{err}");
    let kept = ChatGptPlanAccountStore::get(&f.pool)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(kept.access_token, "access-1");
    let revoked = f.revoked().await;
    assert_eq!(revoked.len(), 1, "the refused grant, never the kept one");
    assert_eq!(revoked[0]["token"], "refresh-2");
    f.done().await;
}

/// Forgetting the account clears the registration, so the next sign-in
/// registers afresh and any ChatGPT account may use it.
#[tokio::test]
async fn a_forgotten_account_lets_another_account_sign_in() {
    use crate::llm::chatgpt_plan::id_token::tests::signed_id_token_for;
    let f = Fixture::new().await;
    f.sign_in_first().await;
    assert!(ChatGptPlanAccountStore::is_registered(&f.pool)
        .await
        .unwrap());

    let outcome = ChatGptPlanAccountStore::forget_account(&f.pool, &f.bus, &f.mock.auth, laptop())
        .await
        .unwrap();
    assert_eq!(
        outcome,
        SignOutOutcome {
            signed_out: true,
            account_forgotten: true,
        }
    );
    assert!(!ChatGptPlanAccountStore::is_registered(&f.pool)
        .await
        .unwrap());
    assert_eq!(f.stored_client_id().await, None);
    assert_eq!(f.revoked().await[0]["token"], "refresh-1");

    let later = f.begin().await;
    assert_eq!(
        param(&later, "client_id").as_deref(),
        Some(DYNAMIC_CLIENT_ID)
    );
    assert_eq!(param(&later, "id_token_hint"), None);
    let nonce = param(&later, "nonce").unwrap();
    f.mock.reply_to_token(
        StatusCode::OK,
        serde_json::json!({
            "access_token": "access-2",
            "refresh_token": "refresh-2",
            "id_token": signed_id_token_for("someone-else", f.mock.auth.origin(), "oaiapp_2", &nonce),
            "expires_in": 3600,
        }),
    );
    f.complete(later, Some("oaiapp_2")).await.unwrap();
    assert_eq!(f.stored_client_id().await.as_deref(), Some("oaiapp_2"));

    let again = ChatGptPlanAccountStore::forget_account(&f.pool, &f.bus, &f.mock.auth, laptop())
        .await
        .unwrap();
    assert!(again.account_forgotten);
    let nothing = ChatGptPlanAccountStore::forget_account(&f.pool, &f.bus, &f.mock.auth, laptop())
        .await
        .unwrap();
    assert_eq!(nothing, SignOutOutcome::default());
    f.done().await;
}

/// An ID token for another attempt stores nothing.
#[tokio::test]
async fn an_id_token_with_another_nonce_stores_nothing() {
    let f = Fixture::new().await;
    let pending = f.begin().await;
    f.mock.reply_to_token(
        StatusCode::OK,
        serde_json::json!({
            "access_token": "access-1",
            "refresh_token": "refresh-1",
            "id_token": signed_id_token(f.mock.auth.origin(), "oaiapp_1", "another-nonce"),
            "expires_in": 3600,
        }),
    );
    assert!(f.complete(pending, Some("oaiapp_1")).await.is_err());
    assert!(ChatGptPlanAccountStore::get(&f.pool)
        .await
        .unwrap()
        .is_none());
    assert_eq!(f.stored_client_id().await, None);
    f.done().await;
}

/// The host id is minted once, survives sign-out, and names no user.
#[tokio::test]
async fn the_host_id_is_minted_once_and_survives_sign_out() {
    let f = Fixture::new().await;
    let first = ChatGptPlanAccountStore::registration(&f.pool)
        .await
        .unwrap()
        .host_id;
    let uuid = first.strip_prefix("urn:uuid:").unwrap();
    assert_eq!(
        uuid::Uuid::parse_str(uuid).unwrap().get_version_num(),
        4,
        "random, so derived from nothing"
    );
    f.sign_in_first().await;
    ChatGptPlanAccountStore::sign_out(&f.pool, &f.bus, &f.mock.auth, None)
        .await
        .unwrap();
    let after = ChatGptPlanAccountStore::registration(&f.pool)
        .await
        .unwrap();
    assert_eq!(after.host_id, first);
    assert_eq!(after.client_id.as_deref(), Some("oaiapp_1"));
    f.done().await;
}

/// A sign-in announces once with the device that started it, and carries no
/// token.
#[tokio::test]
async fn a_sign_in_is_announced_with_its_actor_and_no_token() {
    let f = Fixture::new().await;
    f.sign_in_first().await;
    let payloads: Vec<String> = sqlx::query_scalar(
        "SELECT payload::text FROM events WHERE event_type = 'ChatGptPlanSignedIn'",
    )
    .fetch_all(&f.pool)
    .await
    .unwrap();
    assert_eq!(payloads.len(), 1);
    assert!(payloads[0].contains("laptop"), "{}", payloads[0]);
    assert!(payloads[0].contains("user@example.com"), "{}", payloads[0]);
    for secret in ["access-1", "refresh-1", "eyJ"] {
        assert!(!payloads[0].contains(secret), "{}", payloads[0]);
    }
    f.done().await;
}

#[tokio::test]
async fn debug_never_shows_the_pkce_verifier() {
    let f = Fixture::new().await;
    let pending = f.begin().await;
    let shown = format!("{pending:?}");
    assert!(!shown.contains(pending.pkce.verifier()), "{shown}");
    assert!(!shown.contains(&pending.nonce), "{shown}");
    f.done().await;
}
