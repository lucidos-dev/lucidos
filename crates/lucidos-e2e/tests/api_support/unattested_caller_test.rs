//! An unattested caller against the real engine: invariants 1 and 3 of
//! `docs/plans/2026-11-10-unattested-callers-and-gateway-stamps.md`.
//!
//! The harness starts the e2e engine with a stamp key, so [`user_client`]
//! proves its device the way a gateway does. [`unattested_user_client`] names
//! the same registered device with no stamp, which any local process can do.

use crate::support::{
    base_url, db_url, http_client, register_device, seed_chat_thread_summary,
    unattested_user_client, unique_marker, user_client, E2E_DEVICE_ID,
};
use lucidos_engine::api::actor::NOT_ATTESTED_AS_OWNER;
use lucidos_engine::core::prefs;
use serde_json::{json, Value};
use uuid::Uuid;

/// The `reason` slug of a parked request's 202. A wire contract.
const WAITING_FOR_OWNER: &str = "waiting_for_owner";

async fn pool() -> sqlx::PgPool {
    sqlx::PgPool::connect(&db_url())
        .await
        .expect("Failed to connect to E2E workspace database")
}

/// The origin of the chat message carrying `marker`.
async fn message_origin(pool: &sqlx::PgPool, marker: &str) -> Value {
    for _ in 0..50 {
        let origin: Option<Value> = sqlx::query_scalar(
            "SELECT payload->'origin' FROM events \
             WHERE event_type = 'MessageReceived' AND payload::text LIKE '%' || $1 || '%' \
             ORDER BY sequence LIMIT 1",
        )
        .bind(marker)
        .fetch_optional(pool)
        .await
        .expect("events query failed");
        if let Some(origin) = origin {
            return origin;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    panic!("no MessageReceived carries {marker}");
}

async fn post_chat(builder: reqwest::RequestBuilder, body: Value) -> Value {
    let resp = builder
        .json(&body)
        .send()
        .await
        .expect("chat request failed");
    assert_eq!(resp.status(), 200, "the chat post must be accepted");
    resp.json().await.expect("chat reply is JSON")
}

/// Invariant 1: every claim a `curl` can make, stacked, builds no owner
/// origin. The same device with a real stamp is the device.
#[tokio::test]
async fn a_forged_curl_shaped_request_is_an_unattested_caller() {
    register_device(E2E_DEVICE_ID).await;
    let pool = pool().await;
    let token = lucidos_local_token::read().expect("the engine mints a local token at boot");
    let forged = unique_marker("forged-origin");

    post_chat(
        http_client()
            .post(format!("{}/api/v1/chat/stream", base_url()))
            .header(lucidos_local_token::HEADER_LOCAL_TOKEN, token)
            .header("x-lucidos-device-id", E2E_DEVICE_ID)
            .header(
                lucidos_gateway_stamp::HEADER_GATEWAY_STAMP,
                format!(
                    "v1.{}.{}",
                    lucidos_gateway_stamp::now_unix(),
                    "0".repeat(64)
                ),
            ),
        json!({
            "message": format!("Say exactly: \"ok {forged}\""),
            "mode": "human",
            "caller_workspace": "dev",
        }),
    )
    .await;
    let origin = message_origin(&pool, &forged).await;
    assert_eq!(origin["kind"], "unattested_caller", "{origin}");
    assert_eq!(origin["claimed_device_id"], E2E_DEVICE_ID, "{origin}");
    assert_eq!(origin["claimed_local_token"], true, "{origin}");
    assert_eq!(origin["claimed_workspace"], "dev", "{origin}");

    let stamped = unique_marker("stamped-origin");
    post_chat(
        user_client()
            .await
            .post(format!("{}/api/v1/chat/stream", base_url())),
        json!({ "message": format!("Say exactly: \"ok {stamped}\""), "mode": "human" }),
    )
    .await;
    let origin = message_origin(&pool, &stamped).await;
    assert_eq!(
        origin["kind"], "device",
        "a stamped device is the device: {origin}"
    );
    assert_eq!(origin["device_id"], E2E_DEVICE_ID, "{origin}");
}

/// One owner-only request. `marker` rides in its query, so the card it parks
/// on can be found.
struct Door {
    name: &'static str,
    method: reqwest::Method,
    path: String,
    body: Option<Value>,
}

impl Door {
    fn new(name: &'static str, method: reqwest::Method, path: &str, marker: &str) -> Self {
        let joint = if path.contains('?') { '&' } else { '?' };
        Self {
            name,
            method,
            path: format!("{path}{joint}e2e={marker}"),
            body: None,
        }
    }

    fn body(mut self, body: Value) -> Self {
        self.body = Some(body);
        self
    }

    async fn send(&self, client: &reqwest::Client) -> reqwest::Response {
        let mut builder = client.request(
            self.method.clone(),
            format!("{}/api/v1{}", base_url(), self.path),
        );
        if let Some(body) = &self.body {
            builder = builder.json(body);
        }
        builder.send().await.expect("door request failed")
    }
}

/// The card a door parked the request carrying `marker` on.
async fn parked_card(pool: &sqlx::PgPool, marker: &str) -> (Uuid, String) {
    sqlx::query_as(
        "SELECT thread_id, payload->>'tool_use_id' FROM events \
         WHERE event_type = 'UserQuestionAsked' \
           AND payload->'permission_request'->'unattested_act'->'request'->>'path' LIKE '%e2e=' || $1 || '%' \
         ORDER BY sequence DESC LIMIT 1",
    )
    .bind(marker)
    .fetch_optional(pool)
    .await
    .expect("events query failed")
    .unwrap_or_else(|| panic!("no permission request card parks the request {marker}"))
}

/// Answer the card from the stamped device, so the e2e workspace keeps no
/// open card, and check the act never ran.
async fn dismiss(pool: &sqlx::PgPool, thread_id: Uuid, tool_use_id: &str, door: &str) {
    let resp = user_client()
        .await
        .post(format!(
            "{}/api/v1/threads/{thread_id}/answer-question",
            base_url()
        ))
        .json(&json!({ "tool_use_id": tool_use_id, "answer": { "kind": "Canceled" } }))
        .send()
        .await
        .expect("answer request failed");
    assert!(
        resp.status().is_success(),
        "{door}: the stamped device dismisses the card, got {}",
        resp.status()
    );
    let granted: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM events \
         WHERE event_type = 'PermissionGranted' AND thread_id = $1 \
           AND payload->>'request_tool_use_id' = $2",
    )
    .bind(thread_id)
    .bind(tool_use_id)
    .fetch_one(pool)
    .await
    .expect("events query failed");
    assert_eq!(granted, 0, "{door}: a dismissed card grants nothing");
}

async fn get_json(client: &reqwest::Client, path: &str) -> Value {
    client
        .get(format!("{}/api/v1{path}", base_url()))
        .send()
        .await
        .expect("read failed")
        .json()
        .await
        .expect("read is JSON")
}

/// The part of the workspace a door's act would change. Other tests write the
/// same endpoints concurrently, so a landing reads only what this test's
/// doors alone write.
enum Landing {
    /// The one preference a door writes.
    Preference(&'static str),
    /// Whether a value only this test writes appears in a read.
    Mentions { read: &'static str, value: String },
}

impl Landing {
    async fn observe(&self, client: &reqwest::Client) -> Value {
        match self {
            Self::Preference(key) => get_json(client, &format!("/preferences?key={key}")).await
                ["preferences"][key]
                .clone(),
            Self::Mentions { read, value } => Value::Bool(
                get_json(client, read)
                    .await
                    .to_string()
                    .contains(value.as_str()),
            ),
        }
    }
}

/// Invariant 3: each owner-only door asks the owner and acts on nothing.
/// The before and after reads show the act did not land.
#[tokio::test]
async fn every_owner_only_door_asks_an_unattested_caller_and_acts_on_nothing() {
    use reqwest::Method;
    let pool = pool().await;
    let owner = user_client().await;
    let unattested = unattested_user_client().await;
    let marker = || unique_marker("door");

    let doomed = Uuid::new_v4();
    seed_chat_thread_summary(&pool, doomed, "idle").await;
    let trigger_name = unique_marker("e2e-unattested-grant");
    // No other test binds this address or grants this tool.
    let engine_bind = "100.64.0.99";
    let granted_tool = "Bash(e2e-unattested:*)";
    let landings = [
        Landing::Preference(prefs::COMMAND_GUARD_JUDGE.key()),
        Landing::Mentions {
            read: "/network-config",
            value: engine_bind.to_owned(),
        },
        Landing::Mentions {
            read: "/cc-allowed-tools",
            value: granted_tool.to_owned(),
        },
        Landing::Mentions {
            read: "/triggers",
            value: trigger_name.clone(),
        },
    ];
    let mut before = Vec::new();
    for landing in &landings {
        before.push(landing.observe(&owner).await);
    }

    let doors = [
        Door::new(
            "a human-only preference",
            Method::PUT,
            &format!("/preferences?key={}", prefs::COMMAND_GUARD_JUDGE.key()),
            &marker(),
        )
        .body(json!({ "value": "false" })),
        Door::new(
            "the network bind",
            Method::PUT,
            "/network-config",
            &marker(),
        )
        .body(json!({ "engine_bind": engine_bind })),
        Door::new(
            "a permission grant",
            Method::PUT,
            "/cc-allowed-tools",
            &marker(),
        )
        .body(json!({ "contents": format!("{granted_tool}\n") })),
        Door::new(
            "a trigger's coding-agent grant",
            Method::POST,
            "/triggers",
            &marker(),
        )
        .body(json!({
            "name": trigger_name,
            "run": { "type": "intent", "intent": "noop" },
            "cron_expressions": ["0 0 8 * * *"],
            "coding_agent_grant": true,
        })),
        Door::new("thread delete", Method::POST, "/threads/delete", &marker())
            .body(json!({ "thread_id": doomed })),
        Door::new(
            "Archive all",
            Method::POST,
            "/threads/archive-all",
            &marker(),
        )
        .body(json!({ "thread_ids": [doomed] })),
        Door::new(
            "closing the main check",
            Method::POST,
            "/main-check/close",
            &marker(),
        ),
    ];

    for door in &doors {
        let resp = door.send(&unattested).await;
        let status = resp.status();
        let body: Value = resp.json().await.unwrap_or_default();
        assert_eq!(
            status, 202,
            "{}: an unattested caller waits: {body}",
            door.name
        );
        assert_eq!(body["reason"], WAITING_FOR_OWNER, "{}: {body}", door.name);
        let marker = door
            .path
            .rsplit("e2e=")
            .next()
            .expect("every door carries a marker");
        let (thread_id, tool_use_id) = parked_card(&pool, marker).await;
        dismiss(&pool, thread_id, &tool_use_id, door.name).await;
    }

    // The first doors write the landings, in order.
    for (door, (landing, before)) in doors.iter().zip(landings.iter().zip(before)) {
        assert_eq!(
            landing.observe(&owner).await,
            before,
            "{}: an unattested caller's act must not land",
            door.name
        );
    }
    // The seeded thread has no events of its own, and its delete and Archive
    // all cards land in the home thread. So any event here is the act landing.
    let (rows, events): (i64, i64) = sqlx::query_as(
        "SELECT (SELECT COUNT(*) FROM thread_summaries WHERE thread_id = $1), \
                (SELECT COUNT(*) FROM events WHERE thread_id = $1)",
    )
    .bind(doomed)
    .fetch_one(&pool)
    .await
    .expect("thread query failed");
    assert_eq!(
        (rows, events),
        (1, 0),
        "the thread is neither deleted nor archived"
    );
}

/// Invariant 3: a door that cannot wait refuses an unattested caller outright,
/// and lets the same device through once stamped.
#[tokio::test]
async fn a_reveal_refuses_an_unattested_caller_and_passes_a_stamped_one() {
    let path = format!(
        "{}/api/v1/credential-reveal-token?id={}",
        base_url(),
        Uuid::new_v4()
    );
    let refused = unattested_user_client()
        .await
        .post(&path)
        .send()
        .await
        .expect("mint request failed");
    assert_eq!(refused.status(), 403);
    let text = refused.text().await.unwrap_or_default();
    assert!(text.contains(NOT_ATTESTED_AS_OWNER), "{text}");

    let stamped = user_client()
        .await
        .post(&path)
        .send()
        .await
        .expect("mint request failed");
    assert_eq!(
        stamped.status(),
        404,
        "a stamped device passes the gate and meets the missing credential"
    );
}

/// Invariant 3: only the owner answers a card, so an unattested answer is
/// refused, and the stamped device's answer lands.
#[tokio::test]
async fn an_unattested_caller_cannot_answer_a_card() {
    let pool = pool().await;
    let thread_id = Uuid::new_v4();
    let tool_use_id = format!("toolu_e2e_{}", Uuid::new_v4().simple());
    seed_chat_thread_summary(&pool, thread_id, "waiting_for_user_answer").await;
    sqlx::query(
        "INSERT INTO events (id, aggregate, aggregate_id, event_type, payload, created, thread_id) \
         VALUES ($1, 'thread', $2::text, 'UserQuestionAsked', $3, NOW(), $2)",
    )
    .bind(Uuid::new_v4())
    .bind(thread_id)
    .bind(json!({
        "tool_use_id": tool_use_id,
        "cc_session_id": "",
        "question": "Pick one:",
        "options": [{ "id": "opt-0", "label": "Yes" }, { "id": "opt-1", "label": "No" }],
        "channel": "chat",
    }))
    .execute(&pool)
    .await
    .expect("failed to insert UserQuestionAsked");

    let url = format!("{}/api/v1/threads/{thread_id}/answer-question", base_url());
    let answer = json!({ "tool_use_id": tool_use_id, "answer": { "kind": "Canceled" } });
    let refused = unattested_user_client()
        .await
        .post(&url)
        .json(&answer)
        .send()
        .await
        .expect("answer request failed");
    assert_eq!(refused.status(), 403, "an unattested answer is refused");

    let answered = user_client()
        .await
        .post(&url)
        .json(&answer)
        .send()
        .await
        .expect("answer request failed");
    assert!(
        answered.status().is_success(),
        "the stamped device answers, got {}",
        answered.status()
    );
}
