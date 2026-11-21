use std::time::Duration;

use axum::http::StatusCode;

use super::*;
use crate::llm::claude_gateway::account::RejectedToken;
use crate::llm::claude_gateway::mock_gateway::MockGateway;
use crate::test_support::{
    seed_claude_gateway_sign_in, seed_preference, setup_test_db, teardown_test_db,
};

const GATEWAY: &str = "https://gateway.example";

fn laptop() -> Option<MessageOrigin> {
    Some(MessageOrigin::Device {
        device_id: "laptop".to_string(),
    })
}

fn grant() -> serde_json::Value {
    serde_json::json!({"access_token": "a", "refresh_token": "r", "expires_in": 3600})
}

/// The next system event `pick` accepts, within five seconds.
async fn next_event<T>(
    rx: &mut tokio::sync::broadcast::Receiver<EmittedEvent>,
    pick: impl Fn(SystemEvent) -> Option<T>,
) -> T {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Ok(EmittedEvent {
                typed: BusEvent::System(event),
                ..
            }) = rx.recv().await
            {
                if let Some(found) = pick(event) {
                    return found;
                }
            }
        }
    })
    .await
    .expect("the event arrives")
}

/// The slot's state once it leaves `Pending`, within five seconds.
async fn settled(sign_in: &ClaudeGatewaySignIn) -> SignInState {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let state = sign_in.state();
            if !matches!(state, SignInState::Pending { .. }) {
                return state;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the sign-in settles")
}

async fn gateway_at_url(pool: &PgPool) -> MockGateway {
    let gateway = MockGateway::start().await;
    seed_preference(
        pool,
        prefs::CLAUDE_GATEWAY_URL.key(),
        gateway.origin.as_str(),
    )
    .await
    .unwrap();
    gateway
}

/// The user sees the code at once. Once they approve, the sign-in is stored
/// and announced with the device that started it.
#[tokio::test]
async fn an_approved_sign_in_is_stored_and_announced_with_its_actor() {
    let (pool, db) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());
    let gateway = gateway_at_url(&pool).await;
    gateway.refuse_token("authorization_pending");
    gateway.reply_to_token(StatusCode::OK, grant());
    let mut rx = bus.subscribe();
    let sign_in = Arc::new(ClaudeGatewaySignIn::default());

    let pending = sign_in
        .start(&pool, &bus, gateway.origin.clone(), laptop())
        .await
        .unwrap();
    let SignInState::Pending {
        issuer, user_code, ..
    } = pending
    else {
        panic!("expected a pending sign-in: {pending:?}");
    };
    assert_eq!(issuer, gateway.origin.as_str());
    assert_eq!(user_code, "ABCD-EFGH");

    let actor = next_event(&mut rx, |event| match event {
        SystemEvent::ClaudeGatewaySignedIn { actor, .. } => Some(actor),
        _ => None,
    })
    .await;
    assert_eq!(actor, laptop());
    assert_eq!(settled(&sign_in).await, SignInState::Idle);
    let account = ClaudeGatewayAccountStore::get(&pool)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(account.issuer, gateway.origin.as_str());
    teardown_test_db(&db).await;
}

/// Starting a sign-in announces it, and stopping it before approval
/// announces that too, so another open Settings card follows both.
#[tokio::test]
async fn starting_and_stopping_a_sign_in_are_announced() {
    let (pool, db) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());
    let gateway = gateway_at_url(&pool).await;
    gateway.set_device_field("interval", serde_json::json!(600));
    let mut rx = bus.subscribe();
    let sign_in = Arc::new(ClaudeGatewaySignIn::default());

    sign_in
        .start(&pool, &bus, gateway.origin.clone(), laptop())
        .await
        .unwrap();
    let started = next_event(&mut rx, |event| match event {
        SystemEvent::ClaudeGatewaySignInStarted { issuer, actor } => Some((issuer, actor)),
        _ => None,
    })
    .await;
    assert_eq!(started, (gateway.origin.to_string(), laptop()));

    assert!(!sign_in.sign_out(&pool, &bus, laptop()).await.unwrap());
    let canceled = next_event(&mut rx, |event| match event {
        SystemEvent::ClaudeGatewaySignInCanceled { issuer, actor } => Some((issuer, actor)),
        _ => None,
    })
    .await;
    assert_eq!(canceled, (gateway.origin.to_string(), laptop()));
    teardown_test_db(&db).await;
}

/// A sign-out that lands while granted tokens are being stored waits for
/// the store, then deletes it: the user is never left signed in.
#[tokio::test]
async fn a_sign_out_during_the_store_still_signs_out() {
    let (pool, db) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());
    let gateway = gateway_at_url(&pool).await;
    gateway.reply_to_token(StatusCode::OK, grant());
    let sign_in = Arc::new(ClaudeGatewaySignIn::default());

    let held = sign_in.store.lock().await;
    sign_in
        .start(&pool, &bus, gateway.origin.clone(), laptop())
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        while gateway.token_forms().is_empty() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the grant arrives");
    tokio::time::sleep(Duration::from_millis(100)).await;
    let signing_out = tokio::spawn({
        let (sign_in, pool, bus) = (Arc::clone(&sign_in), pool.clone(), bus.clone());
        async move { sign_in.sign_out(&pool, &bus, laptop()).await.unwrap() }
    });
    tokio::time::sleep(Duration::from_millis(100)).await;
    drop(held);
    signing_out.await.unwrap();
    assert!(ClaudeGatewayAccountStore::get(&pool)
        .await
        .unwrap()
        .is_none());
    teardown_test_db(&db).await;
}

/// A declined sign-in says why, announces it, and stores nothing.
#[tokio::test]
async fn a_declined_sign_in_reports_why_and_stores_nothing() {
    let (pool, db) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());
    let gateway = gateway_at_url(&pool).await;
    gateway.refuse_token("access_denied");
    let mut rx = bus.subscribe();
    let sign_in = Arc::new(ClaudeGatewaySignIn::default());

    sign_in
        .start(&pool, &bus, gateway.origin.clone(), laptop())
        .await
        .unwrap();
    let announced = next_event(&mut rx, |event| match event {
        SystemEvent::ClaudeGatewaySignInFailed {
            issuer,
            error,
            actor,
        } => Some((issuer, error, actor)),
        _ => None,
    })
    .await;
    let SignInState::Failed { issuer, error } = settled(&sign_in).await else {
        panic!("expected a failed sign-in");
    };
    assert_eq!(issuer, gateway.origin.as_str());
    assert!(!error.is_empty());
    assert_eq!(announced, (issuer, error, laptop()));
    assert!(ClaudeGatewayAccountStore::get(&pool)
        .await
        .unwrap()
        .is_none());
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn a_sign_in_without_a_url_says_to_set_one() {
    let (pool, db) = setup_test_db().await;
    let err = configured_origin(&pool).await.unwrap_err().to_string();
    assert!(err.contains(prefs::CLAUDE_GATEWAY_URL.key()), "{err}");
    teardown_test_db(&db).await;
}

/// Tokens granted by a gateway the URL no longer names are dropped.
#[tokio::test]
async fn tokens_from_a_gateway_the_url_moved_off_are_not_stored() {
    let (pool, db) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());
    seed_preference(
        &pool,
        prefs::CLAUDE_GATEWAY_URL.key(),
        "https://other.example",
    )
    .await
    .unwrap();
    let tokens = GrantedTokens {
        access_token: "a".to_string(),
        refresh_token: None,
        token_expiry: None,
        email: None,
    };
    let origin = GatewayOrigin::parse(GATEWAY).unwrap();

    let err = store_sign_in(&pool, &bus, &origin, &tokens, None)
        .await
        .unwrap_err();
    assert!(err.contains("changed during the sign-in"), "{err}");
    assert!(ClaudeGatewayAccountStore::get(&pool)
        .await
        .unwrap()
        .is_none());
    teardown_test_db(&db).await;
}

/// Signing out stops a sign-in in progress and forgets the stored one.
#[tokio::test]
async fn sign_out_stops_a_pending_sign_in_and_forgets_the_account() {
    let (pool, db) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());
    let gateway = gateway_at_url(&pool).await;
    gateway.set_device_field("interval", serde_json::json!(600));
    seed_claude_gateway_sign_in(&pool, gateway.origin.as_str()).await;
    let sign_in = Arc::new(ClaudeGatewaySignIn::default());
    sign_in
        .start(&pool, &bus, gateway.origin.clone(), laptop())
        .await
        .unwrap();

    assert!(sign_in.sign_out(&pool, &bus, laptop()).await.unwrap());
    assert_eq!(sign_in.state(), SignInState::Idle);
    assert!(ClaudeGatewayAccountStore::get(&pool)
        .await
        .unwrap()
        .is_none());
    assert!(!sign_in.sign_out(&pool, &bus, laptop()).await.unwrap());
    teardown_test_db(&db).await;
}

/// The status names the URL and the account, never a token.
#[tokio::test]
async fn the_status_reports_the_account_without_its_tokens() {
    let (pool, db) = setup_test_db().await;
    seed_preference(&pool, prefs::CLAUDE_GATEWAY_URL.key(), GATEWAY)
        .await
        .unwrap();
    seed_claude_gateway_sign_in(&pool, GATEWAY).await;

    let status = ClaudeGatewaySignIn::default().status(&pool).await.unwrap();
    assert_eq!(status.url.as_deref(), Some(GATEWAY));
    assert_eq!(status.account.unwrap().issuer, GATEWAY);
    assert!(!status.sign_in_again);
    assert_eq!(status.sign_in, SignInState::Idle);
    let body = serde_json::to_string(&ClaudeGatewaySignIn::default().status(&pool).await.unwrap())
        .unwrap();
    assert!(!body.contains("gw-token"), "{body}");
    teardown_test_db(&db).await;
}

/// Telemetry reads on only when the stored policy asks for it and the
/// engine's environment does not opt out.
#[tokio::test]
async fn the_status_says_when_the_gateway_collects_telemetry() {
    let (pool, db) = setup_test_db().await;
    seed_claude_gateway_sign_in(&pool, GATEWAY).await;
    let stored = || async {
        ClaudeGatewayAccountStore::get(&pool)
            .await
            .unwrap()
            .unwrap()
    };
    let no_env: switch::EngineEnv = |_| None;
    assert!(!telemetry_on(&stored().await, no_env), "no policy");

    sqlx::query("UPDATE claude_gateway_accounts SET managed_settings = $1")
        .bind(serde_json::json!({"env": {
            "CLAUDE_CODE_ENABLE_TELEMETRY": "1",
            "OTEL_LOGS_EXPORTER": "otlp"
        }}))
        .execute(&pool)
        .await
        .unwrap();
    assert!(telemetry_on(&stored().await, no_env));
    let opted_out: switch::EngineEnv = |k| (k == "DISABLE_TELEMETRY").then(|| "1".into());
    assert!(!telemetry_on(&stored().await, opted_out));
    teardown_test_db(&db).await;
}

/// A URL that no longer names the stored gateway signs out, with the actor of
/// the write. The same origin spelled another way keeps the sign-in.
#[tokio::test]
async fn changing_the_url_signs_out_of_the_old_gateway() {
    let (pool, db) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());
    seed_claude_gateway_sign_in(&pool, GATEWAY).await;

    sign_out_unless_issued_by(&pool, &bus, Some("https://GATEWAY.example/"), None).await;
    assert!(ClaudeGatewayAccountStore::get(&pool)
        .await
        .unwrap()
        .is_some());

    spawn_url_change_sign_out(bus.subscribe(), pool.clone(), bus.clone());
    let mut rx = bus.subscribe();
    crate::core::PreferenceStore::set(
        &pool,
        &bus,
        prefs::CLAUDE_GATEWAY_URL.key(),
        "https://other.example",
        laptop(),
    )
    .await
    .unwrap();
    let (reason, actor) = next_event(&mut rx, |event| match event {
        SystemEvent::ClaudeGatewaySignedOut { reason, actor, .. } => Some((reason, actor)),
        _ => None,
    })
    .await;
    assert_eq!(reason, ClaudeGatewaySignOutReason::UrlChanged);
    assert_eq!(actor, laptop());
    assert!(ClaudeGatewayAccountStore::get(&pool)
        .await
        .unwrap()
        .is_none());
    teardown_test_db(&db).await;
}

/// Clearing the URL signs out too.
#[tokio::test]
async fn clearing_the_url_signs_out() {
    let (pool, db) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());
    seed_claude_gateway_sign_in(&pool, GATEWAY).await;

    sign_out_unless_issued_by(&pool, &bus, None, None).await;
    assert!(ClaudeGatewayAccountStore::get(&pool)
        .await
        .unwrap()
        .is_none());
    teardown_test_db(&db).await;
}

/// A sign-in the gateway rejected reads "sign in again" until the next
/// sign-in, while a sign-out by the user does not.
#[tokio::test]
async fn a_rejected_sign_in_reads_sign_in_again_until_the_next_one() {
    let (pool, db) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());
    let origin = GatewayOrigin::parse(GATEWAY).unwrap();
    let tokens = GrantedTokens {
        access_token: "a".to_string(),
        refresh_token: Some("r".to_string()),
        token_expiry: None,
        email: None,
    };
    let sign_in = ClaudeGatewaySignIn::default();
    let sign_in_again = || async { sign_in.status(&pool).await.unwrap().sign_in_again };

    ClaudeGatewayAccountStore::sign_in(&pool, &bus, &origin, &tokens, None)
        .await
        .unwrap();
    ClaudeGatewayAccountStore::forget_rejected(&pool, &bus, RejectedToken::Refresh("r"))
        .await
        .unwrap();
    assert!(sign_in_again().await);

    ClaudeGatewayAccountStore::sign_in(&pool, &bus, &origin, &tokens, None)
        .await
        .unwrap();
    assert!(!sign_in_again().await);

    sign_in.sign_out(&pool, &bus, laptop()).await.unwrap();
    assert!(!sign_in_again().await);
    teardown_test_db(&db).await;
}
