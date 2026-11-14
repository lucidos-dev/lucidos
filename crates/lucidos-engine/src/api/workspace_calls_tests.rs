//! The outbound workspace-call route: who may call, and what verdict rides.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, RwLock};

use axum::http::HeaderMap;
use uuid::Uuid;

use super::*;
use crate::api::actor::{init_agent_origin_secret, mint_agent_origin_token};
use crate::engine::thread_events::{EngineReason, MessageOrigin, SpawnVerdict};
use crate::test_support::{setup_test_db, teardown_test_db};

type Seen = Arc<Mutex<Option<HeaderMap>>>;

/// A stand-in gateway on loopback that records the headers of the call.
async fn stand_in_gateway() -> (String, Seen) {
    let seen: Seen = Default::default();
    let record = seen.clone();
    let app = axum::Router::new().fallback(move |headers: HeaderMap| {
        let record = record.clone();
        async move {
            *record.lock().unwrap() = Some(headers);
            axum::Json(serde_json::json!({ "thread_id": "spawned" }))
        }
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (port.to_string(), seen)
}

fn token_headers(thread_id: Option<Uuid>, trigger: Option<&str>) -> HeaderMap {
    init_agent_origin_secret("subprocess-test-secret".to_string());
    let token = mint_agent_origin_token(thread_id, 0, trigger).expect("secret installed");
    let mut headers = HeaderMap::new();
    headers.insert(
        crate::api::actor::HEADER_AGENT_ORIGIN_TOKEN,
        token.parse().unwrap(),
    );
    headers
}

async fn seed_thread(pool: &sqlx::PgPool, origin: &MessageOrigin) -> Uuid {
    let thread_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO events (id, aggregate, aggregate_id, event_type, payload, created, thread_id) \
         VALUES ($1, 'thread', $2, 'MessageReceived', $3, NOW(), $2::uuid)",
    )
    .bind(Uuid::new_v4())
    .bind(thread_id.to_string())
    .bind(serde_json::json!({ "origin": origin }))
    .execute(pool)
    .await
    .unwrap();
    thread_id
}

fn no_triggers() -> RwLock<HashMap<String, crate::triggers::TriggerConfig>> {
    RwLock::new(HashMap::new())
}

async fn body_of(response: Response) -> serde_json::Value {
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

fn verdict_seen(seen: &Seen) -> Option<SpawnVerdict> {
    let headers = seen.lock().unwrap().clone()?;
    let value = headers.get(lucidos_gateway_stamp::HEADER_CALLER_VERDICT)?;
    SpawnVerdict::from_header(value.to_str().ok()?)
}

#[tokio::test]
async fn a_caller_with_no_origin_token_is_refused() {
    let (pool, db) = setup_test_db().await;
    let (port, seen) = stand_in_gateway().await;
    let response = outbound_call(
        &pool,
        &no_triggers(),
        Some(&port),
        "work2",
        &HeaderMap::new(),
        b"{}",
    )
    .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert_eq!(
        body_of(response).await["reason"],
        REASON_ORIGIN_TOKEN_REQUIRED
    );
    assert!(
        seen.lock().unwrap().is_none(),
        "nothing reaches the gateway"
    );
    pool.close().await;
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn with_no_gateway_the_caller_hears_to_post_directly() {
    let (pool, db) = setup_test_db().await;
    let thread = seed_thread(
        &pool,
        &MessageOrigin::Device {
            device_id: "d1".into(),
        },
    )
    .await;
    let response = outbound_call(
        &pool,
        &no_triggers(),
        None,
        "work2",
        &token_headers(Some(thread), None),
        b"{}",
    )
    .await;
    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert_eq!(
        body_of(response).await["reason"],
        lucidos_gateway_stamp::REASON_NO_GATEWAY
    );
    pool.close().await;
    teardown_test_db(&db).await;
}

/// The verdict is the calling thread's own, read from the verified token.
#[tokio::test]
async fn the_call_carries_the_calling_threads_verdict() {
    let _ = crate::api::gateway_stamp::test_support::stamp_for("d1");
    let (pool, db) = setup_test_db().await;
    let owned = seed_thread(
        &pool,
        &MessageOrigin::Device {
            device_id: "d1".into(),
        },
    )
    .await;
    let triggered = seed_thread(
        &pool,
        &MessageOrigin::engine(EngineReason::Scheduler {
            trigger_id: "trig-gone".into(),
            trigger_name: Some("Nightly".into()),
            actor_email: None,
        }),
    )
    .await;

    let (port, seen) = stand_in_gateway().await;
    let response = outbound_call(
        &pool,
        &no_triggers(),
        Some(&port),
        "work2",
        &token_headers(Some(owned), None),
        br#"{"message": "hi"}"#,
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(body_of(response).await["thread_id"], "spawned");
    assert_eq!(verdict_seen(&seen), Some(SpawnVerdict::Allowed));
    let headers = seen.lock().unwrap().clone().unwrap();
    assert_eq!(
        headers[lucidos_gateway_stamp::HEADER_CALLER_THREAD],
        owned.to_string()
    );

    let (port, seen) = stand_in_gateway().await;
    outbound_call(
        &pool,
        &no_triggers(),
        Some(&port),
        "work2",
        &token_headers(Some(triggered), None),
        b"{}",
    )
    .await;
    assert!(
        matches!(verdict_seen(&seen), Some(SpawnVerdict::Refused { ref reason }) if reason.contains("Nightly")),
        "{:?}",
        verdict_seen(&seen)
    );

    pool.close().await;
    teardown_test_db(&db).await;
}

/// The CLI builds its URL with `workspace_call_path`, so the route matches it.
#[tokio::test]
async fn the_route_matches_the_path_the_cli_builds() {
    use tower::ServiceExt as _;
    let app = axum::Router::new().route(
        ROUTE,
        axum::routing::post(|Path(target): Path<String>| async move { target }),
    );
    let path = lucidos_gateway_stamp::workspace_call_path("work2").unwrap();
    let response = app
        .oneshot(
            axum::http::Request::post(path)
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(&body[..], b"work2");
}

/// The body names the mode, and nothing signs it, so a subprocess that
/// claims `human` is sent as an agent.
#[test]
fn a_subprocess_never_calls_as_a_human() {
    let mode = |m: &str| subprocess_mode(&serde_json::json!({ "mode": m }));
    assert_eq!(mode("human"), ActorMode::Agent);
    assert_eq!(mode("engine"), ActorMode::Engine);
    assert_eq!(mode("agent"), ActorMode::Agent);
    assert_eq!(subprocess_mode(&serde_json::json!({})), ActorMode::Agent);
}
