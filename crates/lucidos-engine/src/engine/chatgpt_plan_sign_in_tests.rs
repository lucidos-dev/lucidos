use std::time::Duration;

use axum::http::StatusCode;

use super::*;
use crate::core::oauth_callback::{deliver_callback, BrowserContext, CallbackDelivery};
use crate::llm::chatgpt_plan::id_token::tests::signed_id_token;
use crate::llm::chatgpt_plan::mock_auth::MockAuth;
use crate::llm::chatgpt_plan::ChatGptPlanError;
use crate::test_support::{setup_test_db, teardown_test_db};

fn laptop() -> Option<MessageOrigin> {
    Some(MessageOrigin::Device {
        device_id: "laptop".to_string(),
    })
}

fn through_gateway(local: bool) -> CallbackRoute {
    CallbackRoute::Gateway(BrowserContext {
        prefix: format!("/plan-{}/", uuid::Uuid::new_v4().simple()),
        local,
        origin: Some("https://host.example".into()),
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

    async fn start(
        &self,
        sign_in: &Arc<ChatGptPlanSignIn>,
        route: CallbackRoute,
    ) -> Result<StartedSignIn, BoxError> {
        sign_in
            .start(
                &self.pool,
                &self.bus,
                self.mock.auth.clone(),
                "dev",
                route,
                laptop(),
            )
            .await
    }

    /// Queue a grant answering `started` as client `oaiapp_1`.
    fn grant_for(&self, started: &StartedSignIn) {
        let nonce = param(started, "nonce");
        self.mock.reply_to_token(
            StatusCode::OK,
            serde_json::json!({
                "access_token": "access-1",
                "refresh_token": "refresh-1",
                "id_token": signed_id_token(self.mock.auth.origin(), "oaiapp_1", &nonce),
                "token_type": "Bearer",
                "expires_in": 3600,
            }),
        );
    }

    async fn emitted(&self, event_type: &str) -> i64 {
        sqlx::query_scalar("SELECT COUNT(*) FROM events WHERE event_type = $1")
            .bind(event_type)
            .fetch_one(&self.pool)
            .await
            .unwrap()
    }

    /// The count once `event_type` has landed. A failure settles the state
    /// before its event is written, so a bare count can read zero.
    async fn emitted_eventually(&self, event_type: &str) -> i64 {
        for _ in 0..200 {
            let count = self.emitted(event_type).await;
            if count > 0 {
                return count;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        panic!("{event_type} was never emitted");
    }

    async fn registrations(&self) -> i64 {
        sqlx::query_scalar("SELECT COUNT(*) FROM chatgpt_plan_accounts")
            .fetch_one(&self.pool)
            .await
            .unwrap()
    }

    async fn done(self) {
        self.pool.close().await;
        teardown_test_db(&self.db_name).await;
    }
}

fn param(started: &StartedSignIn, key: &str) -> String {
    reqwest::Url::parse(&started.authorize_url)
        .unwrap()
        .query_pairs()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.into_owned())
        .unwrap()
}

fn callback(started: &StartedSignIn, client_id: &str) -> String {
    format!(
        "code=the-code&state={}&client_id={client_id}",
        urlencoding::encode(&param(started, "state"))
    )
}

/// Wait for the attempt to leave `Pending`.
async fn settled(sign_in: &ChatGptPlanSignIn) -> SignInState {
    for _ in 0..200 {
        let state = sign_in.state();
        if !matches!(state, SignInState::Pending { .. }) {
            return state;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("the sign-in never settled");
}

#[tokio::test]
async fn a_browser_with_no_local_gateway_is_refused_before_any_url() {
    let f = Fixture::new().await;
    let sign_in = Arc::new(ChatGptPlanSignIn::default());
    let error = f
        .start(&sign_in, through_gateway(false))
        .await
        .unwrap_err()
        .to_string();
    assert!(error.contains("on a computer running Lucidos"), "{error}");
    assert_eq!(f.registrations().await, 0, "no host id was minted");
    assert_eq!(sign_in.state(), SignInState::Idle);
    assert_eq!(f.emitted("ChatGptPlanSignInStarted").await, 0);
    f.done().await;
}

#[tokio::test]
async fn an_engine_without_a_gateway_is_refused() {
    let f = Fixture::new().await;
    let sign_in = Arc::new(ChatGptPlanSignIn::default());
    let error = f
        .start(&sign_in, CallbackRoute::OwnListener)
        .await
        .unwrap_err()
        .to_string();
    assert!(error.contains("Lucidos gateway"), "{error}");
    assert_eq!(f.registrations().await, 0);
    f.done().await;
}

/// The gateway's internal delivery hands the plan `state` and its issued
/// `client_id` to the attempt, which signs in.
#[tokio::test]
async fn a_delivered_callback_completes_the_sign_in() {
    let f = Fixture::new().await;
    let sign_in = Arc::new(ChatGptPlanSignIn::default());
    let started = f.start(&sign_in, through_gateway(true)).await.unwrap();
    assert!(matches!(sign_in.state(), SignInState::Pending { .. }));
    assert_eq!(f.emitted("ChatGptPlanSignInStarted").await, 1);
    f.grant_for(&started);

    let delivered = deliver_callback(&callback(&started, "oaiapp_1"));
    assert!(matches!(
        delivered,
        CallbackDelivery::Accepted { ok: true, .. }
    ));
    assert_eq!(settled(&sign_in).await, SignInState::Idle);
    let account = ChatGptPlanAccountStore::get(&f.pool)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(account.client_id, "oaiapp_1");
    assert_eq!(f.emitted("ChatGptPlanSignedIn").await, 1);
    f.done().await;
}

#[tokio::test]
async fn a_refused_callback_fails_the_attempt_and_says_why() {
    let f = Fixture::new().await;
    let sign_in = Arc::new(ChatGptPlanSignIn::default());
    let started = f.start(&sign_in, through_gateway(true)).await.unwrap();
    let state = param(&started, "state");
    let query = format!("error=access_denied&state={}", urlencoding::encode(&state));
    assert!(matches!(
        deliver_callback(&query),
        CallbackDelivery::Accepted { ok: false, .. }
    ));
    let SignInState::Failed { error } = settled(&sign_in).await else {
        panic!("a denial fails the attempt");
    };
    assert!(error.contains("access_denied"), "{error}");
    assert_eq!(f.emitted_eventually("ChatGptPlanSignInFailed").await, 1);
    assert!(ChatGptPlanAccountStore::get(&f.pool)
        .await
        .unwrap()
        .is_none());
    f.done().await;
}

#[tokio::test]
async fn an_attempt_nobody_finishes_times_out() {
    let f = Fixture::new().await;
    let sign_in = Arc::new(ChatGptPlanSignIn::with_timeout(Duration::from_millis(50)));
    let started = f.start(&sign_in, through_gateway(true)).await.unwrap();
    let SignInState::Failed { error } = settled(&sign_in).await else {
        panic!("a timeout fails the attempt");
    };
    assert!(error.contains("timed out"), "{error}");
    assert!(
        matches!(
            deliver_callback(&callback(&started, "oaiapp_1")),
            CallbackDelivery::Unclaimed { .. }
        ),
        "a timed-out attempt takes no callback"
    );
    f.done().await;
}

/// A newer start supersedes the running attempt, whose callback then finds
/// no slot.
#[tokio::test]
async fn a_new_start_supersedes_the_running_attempt() {
    let f = Fixture::new().await;
    let sign_in = Arc::new(ChatGptPlanSignIn::default());
    let first = f.start(&sign_in, through_gateway(true)).await.unwrap();
    let second = f.start(&sign_in, through_gateway(true)).await.unwrap();
    assert_ne!(param(&first, "state"), param(&second, "state"));
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(matches!(
        deliver_callback(&callback(&first, "oaiapp_1")),
        CallbackDelivery::Unclaimed { .. }
    ));
    assert!(matches!(sign_in.state(), SignInState::Pending { .. }));
    f.done().await;
}

#[tokio::test]
async fn sign_out_cancels_the_attempt_and_closes_its_slot() {
    let f = Fixture::new().await;
    let sign_in = Arc::new(ChatGptPlanSignIn::default());
    let started = f.start(&sign_in, through_gateway(true)).await.unwrap();
    let ended = sign_in
        .sign_out(&f.pool, &f.bus, &f.mock.auth, false, laptop())
        .await
        .unwrap();
    assert_eq!(ended, SignOutOutcome::default());
    assert_eq!(sign_in.state(), SignInState::Idle);
    assert_eq!(f.emitted("ChatGptPlanSignInCanceled").await, 1);
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(matches!(
        deliver_callback(&callback(&started, "oaiapp_1")),
        CallbackDelivery::Unclaimed { .. }
    ));
    f.done().await;
}

/// The status carries neither the authorize URL nor its `state`.
#[tokio::test]
async fn the_pending_state_carries_no_url_or_state() {
    let f = Fixture::new().await;
    let sign_in = Arc::new(ChatGptPlanSignIn::default());
    let started = f.start(&sign_in, through_gateway(true)).await.unwrap();
    let body = serde_json::to_string(&sign_in.state()).unwrap();
    assert!(body.contains("\"state\":\"pending\""), "{body}");
    let nonce = param(&started, "state");
    let nonce = nonce.rsplit('~').next().unwrap();
    assert!(!body.contains(nonce), "{body}");
    assert!(!body.contains("authorize"), "{body}");
    f.done().await;
}

/// The status names the account, a running pause and the attempt, and never a
/// token.
#[tokio::test]
async fn the_status_reports_account_and_pause_and_never_a_token() {
    let f = Fixture::new().await;
    let sign_in = Arc::new(ChatGptPlanSignIn::default());
    let pause = LimitPause::unannounced();
    let idle = sign_in.status(&f.pool, &pause).await.unwrap();
    assert_eq!(idle.account, None);
    assert_eq!(idle.paused_until, None);
    assert_eq!(idle.pause_remaining_ms, None);
    assert!(!idle.sign_in_again);
    assert!(!idle.registered);

    crate::test_support::seed_chatgpt_plan_sign_in(&f.pool, "access-secret").await;
    let limit = ChatGptPlanError::from_error_object(&serde_json::json!({
        "code": "subscription_sharing_usage_limit_exceeded",
        "message": "m",
    }))
    .unwrap();
    pause.observe(&limit).await;
    let status = sign_in.status(&f.pool, &pause).await.unwrap();
    assert_eq!(
        status.account.as_ref().and_then(|a| a.email.as_deref()),
        Some("user@example.com")
    );
    assert_eq!(status.paused_until, pause.paused_until());
    assert!(status.pause_remaining_ms.is_some_and(|ms| ms > 0));
    assert_eq!(status.sign_in, SignInState::Idle);
    assert!(status.registered);
    let body = serde_json::to_string(&status).unwrap();
    for secret in ["access-secret", "refresh-token", "id-token"] {
        assert!(!body.contains(secret), "{body}");
    }
    f.done().await;
}

/// A started attempt answers with the page to open and when it ends, only.
#[tokio::test]
async fn a_started_attempt_serializes_the_page_and_expiry_only() {
    let f = Fixture::new().await;
    let sign_in = Arc::new(ChatGptPlanSignIn::default());
    let started = f.start(&sign_in, through_gateway(true)).await.unwrap();
    let body = serde_json::to_value(&started).unwrap();
    let mut keys: Vec<_> = body.as_object().unwrap().keys().cloned().collect();
    keys.sort();
    assert_eq!(keys, ["authorize_url", "expires_at"]);
    f.done().await;
}
