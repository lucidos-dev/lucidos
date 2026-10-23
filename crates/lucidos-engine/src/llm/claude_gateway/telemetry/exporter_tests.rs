use std::sync::Arc;

use axum::http::StatusCode;
use serde_json::{json, Value};

use super::*;
use crate::engine::event_bus::EventBus;
use crate::engine::model_call::auxiliary_capture;
use crate::engine::thread_events::EventMeta;
use crate::engine::{ApiUsage, ContextPurpose};
use crate::llm::call_trace::CallFailure;
use crate::llm::claude_gateway::account::{ClaudeGatewayAccount, ClaudeGatewaySignOutReason};
use crate::llm::claude_gateway::gateway_managed_settings;
use crate::llm::claude_gateway::mock_gateway::{LogsRequest, MockGateway};
use crate::llm::claude_gateway::telemetry::otlp::tests::{protobuf_records, string_pairs};
use crate::llm::claude_gateway::tokens::StoredGatewayAccess;
use crate::llm::claude_gateway::GrantedTokens;
use crate::test_support::{setup_test_db, teardown_test_db};

/// Text a capture's section holds, which no record may carry.
const CONTENT_MARKER: &str = "content-marker-the-prompt-said-this";

struct Fixture {
    pool: PgPool,
    db_name: String,
    bus: EventBus,
    access: GatewayAccessFactory,
}

impl Fixture {
    async fn new() -> Self {
        let (pool, db_name) = setup_test_db().await;
        let (bus, _callback_rx) = EventBus::new(pool.clone());
        let refresh_lock = Arc::default();
        let (factory_pool, factory_bus) = (pool.clone(), bus.clone());
        let access: GatewayAccessFactory = Arc::new(move |origin| {
            Arc::new(StoredGatewayAccess::new(
                factory_pool.clone(),
                factory_bus.clone(),
                origin,
                Arc::clone(&refresh_lock),
            ))
        });
        Self {
            pool,
            db_name,
            bus,
            access,
        }
    }

    fn exporter(&self) -> Exporter {
        Exporter {
            pool: self.pool.clone(),
            access: self.access.clone(),
            engine_env: |_| None,
            retry_step: Duration::ZERO,
        }
    }

    /// An exporter in an engine whose environment opts out of telemetry.
    fn opted_out_exporter(&self) -> Exporter {
        Exporter {
            engine_env: |k| (k == "DISABLE_TELEMETRY").then(|| "1".to_string()),
            ..self.exporter()
        }
    }

    /// Sign in to `gateway`, whose managed settings are `policy`.
    async fn sign_in(&self, gateway: &MockGateway, policy: Value) -> ClaudeGatewayAccount {
        let tokens = GrantedTokens {
            access_token: "gw-token".into(),
            refresh_token: Some("r".into()),
            token_expiry: None,
            email: None,
        };
        ClaudeGatewayAccountStore::sign_in(&self.pool, &self.bus, &gateway.origin, &tokens, None)
            .await
            .unwrap();
        let account = ClaudeGatewayAccountStore::get(&self.pool)
            .await
            .unwrap()
            .unwrap();
        gateway.serve_managed_settings(StatusCode::OK, policy, None);
        gateway_managed_settings::refresh(&self.pool, &self.bus, &account, "gw-token").await;
        ClaudeGatewayAccountStore::get(&self.pool)
            .await
            .unwrap()
            .unwrap()
    }

    /// Record one model call served by `provider`, with a usage block when
    /// `reported` and a section holding [`CONTENT_MARKER`].
    async fn capture(&self, provider: Option<ProviderKind>, reported: bool) {
        self.capture_on(Uuid::new_v4(), EventMeta::NONE, provider, reported)
            .await;
    }

    /// As [`Self::capture`], on `thread_id` and under `meta`.
    async fn capture_on(
        &self,
        thread_id: Uuid,
        meta: EventMeta,
        provider: Option<ProviderKind>,
        reported: bool,
    ) {
        let usage = reported.then_some(ApiUsage {
            input_tokens: 120,
            output_tokens: 30,
            cache_read_tokens: 20,
            cache_creation_tokens: 0,
            modality: None,
            service_tier: None,
            cache_creation_1h_tokens: None,
        });
        let mut event =
            auxiliary_capture(ContextPurpose::Title, "claude-opus-5-5", 40, usage, false);
        if let ThreadEvent::ContextCaptured {
            provider: served_by,
            sections,
            ttft_ms,
            request_id,
            ..
        } = &mut event
        {
            *served_by = provider.map(|kind| kind.as_str().to_string());
            sections[0].content = Some(CONTENT_MARKER.into());
            *ttft_ms = Some(25);
            *request_id = Some("req_capture".into());
        }
        self.bus
            .emit_or_log(
                BusEvent::Thread {
                    thread_id,
                    event,
                    meta,
                },
                "[Test] ContextCaptured",
            )
            .await;
    }

    async fn gateway_captures(&self, count: usize) {
        for _ in 0..count {
            self.capture(Some(ProviderKind::ClaudeGateway), true).await;
        }
    }

    /// Record one failed model call served by `provider`.
    async fn failure(&self, provider: Option<ProviderKind>) {
        self.bus
            .emit_or_log(
                BusEvent::Thread {
                    thread_id: Uuid::new_v4(),
                    event: ThreadEvent::ModelCallFailed {
                        model: "claude-opus-5-5".into(),
                        purpose: ContextPurpose::Title,
                        provider: provider.map(|kind| kind.as_str().to_string()),
                        reasoning_effort: None,
                        duration_ms: 90,
                        attempts: 4,
                        failure: CallFailure::Http,
                        status_code: Some(429),
                        request_id: Some("req_failure".into()),
                    },
                    meta: EventMeta::NONE,
                },
                "[Test] ModelCallFailed",
            )
            .await;
    }

    async fn gateway_failure(&self) {
        self.failure(Some(ProviderKind::ClaudeGateway)).await;
    }

    /// Store an origin event of `thread_id`, as a message that started a turn.
    async fn origin(&self, thread_id: Uuid) -> Uuid {
        let id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO events (id, aggregate, aggregate_id, event_type, payload, thread_id)
             VALUES ($1, 'thread', $2, 'MessageReceived', '{}'::jsonb, $3)",
        )
        .bind(id)
        .bind(thread_id.to_string())
        .bind(thread_id)
        .execute(&self.pool)
        .await
        .unwrap();
        id
    }

    async fn done(self) {
        self.pool.close().await;
        teardown_test_db(&self.db_name).await;
    }
}

fn telemetry_policy() -> Value {
    json!({"env": {"CLAUDE_CODE_ENABLE_TELEMETRY": "1", "OTEL_LOGS_EXPORTER": "otlp"}})
}

/// The `event.sequence` of every record in `requests`, all protobuf.
fn sequences(requests: &[LogsRequest]) -> Vec<String> {
    requests
        .iter()
        .flat_map(|request| protobuf_records(&request.body))
        .map(|record| {
            string_pairs(&record, 6)
                .into_iter()
                .find(|(key, _)| key == "event.sequence")
                .map(|(_, value)| value)
                .unwrap()
        })
        .collect()
}

/// Every record in `requests` as its attribute pairs, all protobuf.
fn records(requests: &[LogsRequest]) -> Vec<Vec<(String, String)>> {
    requests
        .iter()
        .flat_map(|request| protobuf_records(&request.body))
        .map(|record| string_pairs(&record, 6))
        .collect()
}

fn attribute<'a>(record: &'a [(String, String)], key: &str) -> Option<&'a str> {
    record
        .iter()
        .find(|(k, _)| k == key)
        .map(|(_, value)| value.as_str())
}

/// One record per gateway capture with usage, posted to the gateway with the
/// bearer and a real `Content-Length`, and no content in the body.
#[tokio::test]
async fn each_gateway_capture_is_sent_once_without_content() {
    let f = Fixture::new().await;
    let gateway = MockGateway::start().await;
    f.sign_in(&gateway, telemetry_policy()).await;
    let exporter = f.exporter();
    assert_eq!(exporter.export_once().await.unwrap(), Pass::Done, "seeds");

    f.gateway_captures(2).await;
    f.capture(Some(ProviderKind::Vertex), true).await;
    f.capture(None, true).await;
    f.capture(Some(ProviderKind::ClaudeGateway), false).await;
    assert_eq!(exporter.export_once().await.unwrap(), Pass::Done);
    assert_eq!(exporter.export_once().await.unwrap(), Pass::Done);

    let requests = gateway.logs_requests();
    let [request] = requests.as_slice() else {
        panic!("expected one export, saw {}", requests.len());
    };
    assert_eq!(request.header("authorization"), Some("Bearer gw-token"));
    assert_eq!(
        request.header("content-type"),
        Some("application/x-protobuf")
    );
    assert_eq!(
        request.header("content-length"),
        Some(request.body.len().to_string().as_str())
    );
    assert!(request
        .header("user-agent")
        .is_some_and(|ua| ua.starts_with("Lucidos/")));
    assert_eq!(protobuf_records(&request.body).len(), 2);
    assert!(!String::from_utf8_lossy(&request.body).contains(CONTENT_MARKER));
    f.done().await;
}

#[tokio::test]
async fn the_json_encoding_carries_no_content_either() {
    let f = Fixture::new().await;
    let gateway = MockGateway::start().await;
    let mut policy = telemetry_policy();
    policy["env"]["OTEL_EXPORTER_OTLP_PROTOCOL"] = "http/json".into();
    f.sign_in(&gateway, policy).await;
    let exporter = f.exporter();
    exporter.export_once().await.unwrap();
    f.gateway_captures(1).await;
    exporter.export_once().await.unwrap();

    let requests = gateway.logs_requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].header("content-type"), Some("application/json"));
    let body: Value = serde_json::from_slice(&requests[0].body).unwrap();
    let records = &body["resourceLogs"][0]["scopeLogs"][0]["logRecords"];
    assert_eq!(records.as_array().unwrap().len(), 1);
    assert!(!body.to_string().contains(CONTENT_MARKER));
    f.done().await;
}

/// Capture three, send, restart the exporter, capture one more: the gateway
/// sees four records, none twice.
#[tokio::test]
async fn a_restart_neither_drops_nor_resends() {
    let f = Fixture::new().await;
    let gateway = MockGateway::start().await;
    f.sign_in(&gateway, telemetry_policy()).await;
    f.exporter().export_once().await.unwrap();
    f.gateway_captures(3).await;
    f.exporter().export_once().await.unwrap();

    let restarted = f.exporter();
    restarted.export_once().await.unwrap();
    f.gateway_captures(1).await;
    restarted.export_once().await.unwrap();

    let mut sent = sequences(&gateway.logs_requests());
    assert_eq!(sent.len(), 4, "{sent:?}");
    sent.dedup();
    assert_eq!(sent.len(), 4, "a record went twice");
    f.done().await;
}

/// The cursor moves only after a `2xx`. A failed batch goes again on the
/// next pass, and a `503` is retried within the pass.
#[tokio::test]
async fn a_failed_send_keeps_the_cursor() {
    let f = Fixture::new().await;
    let gateway = MockGateway::start().await;
    f.sign_in(&gateway, telemetry_policy()).await;
    let exporter = f.exporter();
    exporter.export_once().await.unwrap();
    f.gateway_captures(1).await;

    gateway.reply_to_logs(StatusCode::INTERNAL_SERVER_ERROR);
    let err = exporter.export_once().await.unwrap_err().to_string();
    assert!(err.contains("500"), "{err}");
    assert_eq!(gateway.logs_requests().len(), 1, "a 500 is not retried");

    gateway.reply_to_logs(StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(exporter.export_once().await.unwrap(), Pass::Done);
    let requests = gateway.logs_requests();
    assert_eq!(requests.len(), 3, "the 503 was retried");
    assert_eq!(sequences(&requests[2..]), sequences(&requests[..1]));
    exporter.export_once().await.unwrap();
    assert_eq!(gateway.logs_requests().len(), 3, "sent once it succeeded");
    f.done().await;
}

/// A batch the collector refuses for good is dropped, as OTLP/HTTP says,
/// rather than blocking every later record behind it.
#[tokio::test]
async fn a_refused_batch_is_dropped() {
    let f = Fixture::new().await;
    let gateway = MockGateway::start().await;
    f.sign_in(&gateway, telemetry_policy()).await;
    let exporter = f.exporter();
    exporter.export_once().await.unwrap();
    f.gateway_captures(2).await;

    gateway.reply_to_logs(StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(exporter.export_once().await.unwrap(), Pass::Done);
    assert_eq!(gateway.logs_requests().len(), 1, "a 413 is not retried");

    f.gateway_captures(1).await;
    exporter.export_once().await.unwrap();
    let requests = gateway.logs_requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(sequences(&requests[1..]).len(), 1, "only the new record");
    f.done().await;
}

/// A `401` from the gateway renews the bearer once and sends again.
#[tokio::test]
async fn a_401_renews_the_bearer_and_sends_again() {
    let f = Fixture::new().await;
    let gateway = MockGateway::start().await;
    f.sign_in(&gateway, telemetry_policy()).await;
    let exporter = f.exporter();
    exporter.export_once().await.unwrap();
    f.gateway_captures(1).await;

    gateway.reply_to_logs(StatusCode::UNAUTHORIZED);
    gateway.reply_to_token(
        StatusCode::OK,
        json!({"access_token": "gw-token-2", "refresh_token": "r2", "expires_in": 3600}),
    );
    assert_eq!(exporter.export_once().await.unwrap(), Pass::Done);
    let requests = gateway.logs_requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(
        requests[1].header("authorization"),
        Some("Bearer gw-token-2")
    );
    assert_eq!(sequences(&requests[1..]), sequences(&requests[..1]));
    f.done().await;
}

/// A row that does not read back still counts toward a full batch. So the
/// cursor stops at the last row fetched, and the next pass sends the rest.
#[tokio::test]
async fn an_unreadable_row_in_a_full_batch_skips_nothing_after_it() {
    let f = Fixture::new().await;
    let gateway = MockGateway::start().await;
    f.sign_in(&gateway, telemetry_policy()).await;
    let exporter = f.exporter();
    exporter.export_once().await.unwrap();

    let gateway_provider = ProviderKind::ClaudeGateway.as_str();
    sqlx::query(
        "INSERT INTO events (id, aggregate, aggregate_id, event_type, payload)
         VALUES (gen_random_uuid(), 'thread', 'unreadable', 'ContextCaptured',
                 jsonb_build_object('provider', $1::text, 'usage', '{}'::jsonb))",
    )
    .bind(gateway_provider)
    .execute(&f.pool)
    .await
    .unwrap();
    f.gateway_captures(1).await;
    let readable: Value = sqlx::query_scalar(
        "SELECT payload FROM events WHERE event_type = 'ContextCaptured'
         ORDER BY sequence DESC LIMIT 1",
    )
    .fetch_one(&f.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO events (id, aggregate, aggregate_id, event_type, payload)
         SELECT gen_random_uuid(), 'thread', 'bulk', 'ContextCaptured', $1
         FROM generate_series(1, $2)",
    )
    .bind(&readable)
    .bind(BATCH_LIMIT as i32)
    .execute(&f.pool)
    .await
    .unwrap();

    assert_eq!(exporter.export_once().await.unwrap(), Pass::More);
    assert_eq!(exporter.export_once().await.unwrap(), Pass::Done);
    assert_eq!(
        sequences(&gateway.logs_requests()).len(),
        BATCH_LIMIT as usize + 1,
        "every readable capture went"
    );
    f.done().await;
}

/// A capture whose sequence commits after a higher one is still sent: the
/// pass reads only up to the committed horizon.
#[tokio::test]
async fn a_capture_that_commits_late_is_still_sent() {
    let f = Fixture::new().await;
    let gateway = MockGateway::start().await;
    f.sign_in(&gateway, telemetry_policy()).await;
    let exporter = Arc::new(f.exporter());
    exporter.export_once().await.unwrap();

    let mut late = f.bus.begin_append_for_test().await.unwrap();
    let mut event = auxiliary_capture(
        ContextPurpose::Title,
        "claude-opus-5-5",
        40,
        Some(ApiUsage {
            input_tokens: 10,
            output_tokens: 1,
            cache_read_tokens: 0,
            cache_creation_tokens: 0,
            modality: None,
            service_tier: None,
            cache_creation_1h_tokens: None,
        }),
        false,
    );
    if let ThreadEvent::ContextCaptured { provider, .. } = &mut event {
        *provider = Some(ProviderKind::ClaudeGateway.as_str().to_string());
    }
    let thread_id = Uuid::new_v4();
    let late_sequence: i64 = sqlx::query_scalar(
        "INSERT INTO events (id, aggregate, aggregate_id, event_type, payload, thread_id)
         VALUES ($1, 'thread', $2, 'ContextCaptured', $3, $4) RETURNING sequence",
    )
    .bind(Uuid::new_v4())
    .bind(thread_id.to_string())
    .bind(event.to_payload(&EventMeta::NONE))
    .bind(thread_id)
    .fetch_one(&mut *late)
    .await
    .unwrap();
    f.gateway_captures(1).await;

    let pass = tokio::spawn({
        let exporter = Arc::clone(&exporter);
        async move { exporter.export_once().await.map_err(|e| e.to_string()) }
    });
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(
        !pass.is_finished(),
        "the pass waits for the append in flight"
    );
    late.commit().await.unwrap();
    assert_eq!(pass.await.unwrap().unwrap(), Pass::Done);

    let sent = sequences(&gateway.logs_requests());
    assert_eq!(sent.len(), 2, "{sent:?}");
    assert!(sent.contains(&late_sequence.to_string()), "{sent:?}");
    f.done().await;
}

/// Records from one sign-in never reach another gateway: a sign-out skips
/// what was not sent, and the next gateway sees only its own.
#[tokio::test]
async fn records_never_reach_another_gateway() {
    let f = Fixture::new().await;
    let first = MockGateway::start().await;
    f.sign_in(&first, telemetry_policy()).await;
    let exporter = f.exporter();
    exporter.export_once().await.unwrap();
    f.gateway_captures(2).await;
    f.gateway_failure().await;
    ClaudeGatewayAccountStore::sign_out(
        &f.pool,
        &f.bus,
        ClaudeGatewaySignOutReason::SignedOut,
        None,
    )
    .await
    .unwrap();

    let second = MockGateway::start().await;
    f.sign_in(&second, telemetry_policy()).await;
    exporter.export_once().await.unwrap();
    assert!(first.logs_requests().is_empty());
    assert!(second.logs_requests().is_empty(), "nothing from before");

    f.gateway_captures(1).await;
    exporter.export_once().await.unwrap();
    assert_eq!(sequences(&second.logs_requests()).len(), 1);
    f.done().await;
}

/// A sign-in that replaces another without a sign-out still keeps the old
/// sign-in's unsent records away from the new gateway.
#[tokio::test]
async fn a_replacing_sign_in_skips_the_old_records() {
    let f = Fixture::new().await;
    let first = MockGateway::start().await;
    f.sign_in(&first, telemetry_policy()).await;
    let exporter = f.exporter();
    exporter.export_once().await.unwrap();
    f.gateway_captures(1).await;
    f.gateway_failure().await;

    let second = MockGateway::start().await;
    f.sign_in(&second, telemetry_policy()).await;
    exporter.export_once().await.unwrap();
    assert!(second.logs_requests().is_empty());
    assert!(first.logs_requests().is_empty());
    f.done().await;
}

/// A collector the policy names on another host gets the policy's headers
/// and never the gateway bearer. The gateway gets nothing.
#[tokio::test]
async fn a_collector_on_another_host_never_sees_the_bearer() {
    let f = Fixture::new().await;
    let gateway = MockGateway::start().await;
    let collector = MockGateway::start().await;
    let mut policy = telemetry_policy();
    policy["env"]["OTEL_EXPORTER_OTLP_ENDPOINT"] = collector.origin.to_string().into();
    policy["env"]["OTEL_EXPORTER_OTLP_HEADERS"] = "x-collector-key=abc".into();
    f.sign_in(&gateway, policy).await;
    let exporter = f.exporter();
    exporter.export_once().await.unwrap();
    f.gateway_captures(1).await;
    exporter.export_once().await.unwrap();

    assert!(gateway.logs_requests().is_empty());
    let requests = collector.logs_requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].header("authorization"), None);
    assert_eq!(requests[0].header("x-collector-key"), Some("abc"));
    f.done().await;
}

/// With telemetry off, captures are skipped, and a policy that turns it on
/// later does not send them.
#[tokio::test]
async fn captures_made_while_telemetry_is_off_are_never_sent() {
    let f = Fixture::new().await;
    let gateway = MockGateway::start().await;
    let account = f.sign_in(&gateway, json!({"env": {}})).await;
    let exporter = f.exporter();
    exporter.export_once().await.unwrap();
    f.gateway_captures(2).await;
    f.gateway_failure().await;
    exporter.export_once().await.unwrap();
    assert!(gateway.logs_requests().is_empty());

    gateway.serve_managed_settings(StatusCode::OK, telemetry_policy(), None);
    gateway_managed_settings::refresh(&f.pool, &f.bus, &account, "gw-token").await;
    exporter.export_once().await.unwrap();
    assert!(gateway.logs_requests().is_empty());
    f.gateway_captures(1).await;
    exporter.export_once().await.unwrap();
    assert_eq!(sequences(&gateway.logs_requests()).len(), 1);
    f.done().await;
}

/// A policy that turns telemetry on before the exporter's next pass still
/// leaves the captures made while it was off unsent.
#[tokio::test]
async fn telemetry_turned_on_before_a_pass_sends_nothing_older() {
    let f = Fixture::new().await;
    let gateway = MockGateway::start().await;
    let account = f.sign_in(&gateway, json!({"env": {}})).await;
    let exporter = f.exporter();
    exporter.export_once().await.unwrap();
    f.gateway_captures(2).await;
    f.gateway_failure().await;

    gateway.serve_managed_settings(StatusCode::OK, telemetry_policy(), None);
    gateway_managed_settings::refresh(&f.pool, &f.bus, &account, "gw-token").await;
    exporter.export_once().await.unwrap();
    assert!(gateway.logs_requests().is_empty());

    f.gateway_captures(1).await;
    exporter.export_once().await.unwrap();
    assert_eq!(sequences(&gateway.logs_requests()).len(), 1);
    f.done().await;
}

/// Captures an opted-out engine made after its last pass, before it stopped,
/// stay unsent when the next engine runs without the opt-out.
#[tokio::test]
async fn captures_made_while_opted_out_are_never_sent_by_a_later_run() {
    let f = Fixture::new().await;
    let gateway = MockGateway::start().await;
    f.sign_in(&gateway, telemetry_policy()).await;
    f.exporter().export_once().await.unwrap();

    f.opted_out_exporter().export_once().await.unwrap();
    f.gateway_captures(2).await;
    f.gateway_failure().await;

    let next_run = f.exporter();
    next_run.export_once().await.unwrap();
    assert!(gateway.logs_requests().is_empty());
    f.gateway_captures(1).await;
    next_run.export_once().await.unwrap();
    assert_eq!(sequences(&gateway.logs_requests()).len(), 1);
    f.done().await;
}

/// An opted-out engine that stops before its first pass still keeps its
/// captures from the next run: the opt-out is recorded at startup.
#[tokio::test]
async fn an_opted_out_start_is_recorded_before_any_pass() {
    let f = Fixture::new().await;
    let gateway = MockGateway::start().await;
    f.sign_in(&gateway, telemetry_policy()).await;
    f.exporter().export_once().await.unwrap();

    f.opted_out_exporter().record_opt_out().await.unwrap();
    f.gateway_captures(2).await;
    f.gateway_failure().await;

    let next_run = f.exporter();
    next_run.export_once().await.unwrap();
    assert!(gateway.logs_requests().is_empty());
    f.gateway_captures(1).await;
    next_run.export_once().await.unwrap();
    assert_eq!(sequences(&gateway.logs_requests()).len(), 1);
    f.done().await;
}

/// The policy that decides may still be on its way after a sign-in. The
/// captures wait for it rather than being skipped.
#[tokio::test]
async fn captures_wait_for_the_first_policy() {
    let f = Fixture::new().await;
    let gateway = MockGateway::start().await;
    let tokens = GrantedTokens {
        access_token: "gw-token".into(),
        refresh_token: None,
        token_expiry: None,
        email: None,
    };
    ClaudeGatewayAccountStore::sign_in(&f.pool, &f.bus, &gateway.origin, &tokens, None)
        .await
        .unwrap();
    let exporter = f.exporter();
    exporter.export_once().await.unwrap();
    f.gateway_captures(1).await;
    f.gateway_failure().await;
    exporter.export_once().await.unwrap();

    let account = ClaudeGatewayAccountStore::get(&f.pool)
        .await
        .unwrap()
        .unwrap();
    gateway.serve_managed_settings(StatusCode::OK, telemetry_policy(), None);
    gateway_managed_settings::refresh(&f.pool, &f.bus, &account, "gw-token").await;
    exporter.export_once().await.unwrap();
    assert_eq!(sequences(&gateway.logs_requests()).len(), 2);
    f.done().await;
}

/// Telemetry failure never touches a turn: with the background exporter
/// failing against an unreachable collector, a gateway call still records
/// its capture at once.
#[tokio::test]
async fn an_unreachable_collector_never_delays_a_call() {
    let f = Fixture::new().await;
    let gateway = MockGateway::start().await;
    let mut policy = telemetry_policy();
    policy["env"]["OTEL_EXPORTER_OTLP_ENDPOINT"] = "http://127.0.0.1:9".into();
    f.sign_in(&gateway, policy).await;
    f.exporter().export_once().await.unwrap();
    spawn(f.exporter(), f.bus.subscribe());

    let recorded = tokio::time::timeout(Duration::from_secs(5), async {
        for _ in 0..3 {
            f.gateway_captures(1).await;
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await;
    assert!(recorded.is_ok(), "recording a call waited on the collector");
    let stored: i64 =
        sqlx::query_scalar("SELECT count(*) FROM events WHERE event_type = 'ContextCaptured'")
            .fetch_one(&f.pool)
            .await
            .unwrap();
    assert_eq!(stored, 3);
    let err = f.exporter().export_once().await.unwrap_err().to_string();
    assert!(err.contains("could not reach"), "{err}");
    f.done().await;
}

/// A failed call goes out as `api_error` and never as `api_request`, in the
/// same batch as a capture and under its own sequence. A failure from another
/// provider, or from no router choice, is not sent.
#[tokio::test]
async fn a_failed_gateway_call_is_sent_as_an_api_error_only() {
    let f = Fixture::new().await;
    let gateway = MockGateway::start().await;
    f.sign_in(&gateway, telemetry_policy()).await;
    let exporter = f.exporter();
    exporter.export_once().await.unwrap();

    f.gateway_captures(1).await;
    f.gateway_failure().await;
    f.failure(Some(ProviderKind::Vertex)).await;
    f.failure(None).await;
    assert_eq!(exporter.export_once().await.unwrap(), Pass::Done);

    let sent = records(&gateway.logs_requests());
    let names: Vec<_> = sent
        .iter()
        .map(|record| attribute(record, "event.name").unwrap())
        .collect();
    assert_eq!(names, ["api_request", "api_error"]);
    let mut sequences: Vec<_> = sent
        .iter()
        .map(|record| attribute(record, "event.sequence").unwrap())
        .collect();
    sequences.dedup();
    assert_eq!(sequences.len(), 2, "each row has its own sequence");

    let (request, error) = (&sent[0], &sent[1]);
    assert_eq!(attribute(request, "request_id"), Some("req_capture"));
    assert_eq!(attribute(request, "ttft_ms"), Some("25"));
    assert_eq!(attribute(error, "error"), Some("rate_limit"));
    assert_eq!(attribute(error, "status_code"), Some("429"));
    assert_eq!(attribute(error, "attempt"), Some("4"));
    assert_eq!(attribute(error, "duration_ms"), Some("90"));
    assert_eq!(attribute(error, "request_id"), Some("req_failure"));
    assert_eq!(
        attribute(error, "input_tokens"),
        None,
        "a failure is not billed"
    );
    f.done().await;
}

/// A failure alone wakes the exporter.
#[test]
fn a_failed_gateway_call_wakes_the_exporter() {
    let failed = |provider: Option<ProviderKind>| BusEvent::Thread {
        thread_id: Uuid::new_v4(),
        event: ThreadEvent::ModelCallFailed {
            model: "claude-opus-5-5".into(),
            purpose: ContextPurpose::Title,
            provider: provider.map(|kind| kind.as_str().to_string()),
            reasoning_effort: None,
            duration_ms: 1,
            attempts: 1,
            failure: CallFailure::Other,
            status_code: None,
            request_id: None,
        },
        meta: EventMeta::NONE,
    };
    assert!(wakes_the_exporter(&failed(Some(
        ProviderKind::ClaudeGateway
    ))));
    assert!(!wakes_the_exporter(&failed(Some(ProviderKind::Vertex))));
    assert!(!wakes_the_exporter(&failed(None)));
}

/// A turn's row names the origin it was written under. A follow-up queued
/// behind the turn is newer, and must not take the turn's `prompt.id`
/// (`engine::in_flight_request_event_id`).
#[tokio::test]
async fn a_turn_keeps_its_own_origin_when_a_follow_up_is_newer() {
    let f = Fixture::new().await;
    let gateway = MockGateway::start().await;
    f.sign_in(&gateway, telemetry_policy()).await;
    let exporter = f.exporter();
    exporter.export_once().await.unwrap();

    let thread_id = Uuid::new_v4();
    let turn_origin = f.origin(thread_id).await;
    f.origin(thread_id).await;
    let meta = EventMeta {
        request_event_id: Some(turn_origin),
        ..EventMeta::NONE
    };
    f.capture_on(thread_id, meta, Some(ProviderKind::ClaudeGateway), true)
        .await;
    exporter.export_once().await.unwrap();

    let sent = records(&gateway.logs_requests());
    assert_eq!(sent.len(), 1);
    assert_eq!(
        attribute(&sent[0], "prompt.id"),
        Some(turn_origin.to_string().as_str())
    );
    f.done().await;
}

/// An auxiliary call has no origin of its own, so it takes the thread's newest
/// one before it. A call on a thread with none sends no `prompt.id`.
#[tokio::test]
async fn an_auxiliary_call_takes_the_newest_earlier_origin() {
    let f = Fixture::new().await;
    let gateway = MockGateway::start().await;
    f.sign_in(&gateway, telemetry_policy()).await;
    let exporter = f.exporter();
    exporter.export_once().await.unwrap();

    let thread_id = Uuid::new_v4();
    f.origin(thread_id).await;
    let newest = f.origin(thread_id).await;
    f.capture_on(
        thread_id,
        EventMeta::NONE,
        Some(ProviderKind::ClaudeGateway),
        true,
    )
    .await;
    f.origin(thread_id).await;
    f.gateway_captures(1).await;
    exporter.export_once().await.unwrap();

    let sent = records(&gateway.logs_requests());
    assert_eq!(sent.len(), 2);
    assert_eq!(
        attribute(&sent[0], "prompt.id"),
        Some(newest.to_string().as_str()),
        "a later origin is not this call's"
    );
    assert_eq!(attribute(&sent[1], "prompt.id"), None);
    f.done().await;
}
