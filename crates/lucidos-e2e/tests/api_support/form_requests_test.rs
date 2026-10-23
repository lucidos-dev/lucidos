//! E2E for *form requests*: `GET /api/v1/form-requests/pending` and
//! `POST /api/v1/form-requests/{request_id}/cancel`, plus the credential save
//! that answers one.
//!
//! A real request needs a model to call `request_credential`, so the request
//! row is inserted directly. The handlers read it back from the events table,
//! exactly as they read one the agentic loop wrote.

use crate::support::{
    base_url, count_events_of_type, db_url, seed_chat_thread_summary, unique_marker, user_client,
};
use serde_json::json;
use uuid::Uuid;

/// A `CredentialRequested` for `service` on a fresh chat thread. Returns the
/// thread and the request id.
async fn insert_credential_request(pool: &sqlx::PgPool, service: &str) -> (Uuid, Uuid) {
    let thread_id = Uuid::new_v4();
    let request_id = Uuid::new_v4();
    seed_chat_thread_summary(pool, thread_id, "idle").await;
    let payload = json!({
        "request_id": request_id,
        "payload": json!({
            "service": service,
            "prompt": "Paste your API key.",
            "auth_type": "api_key",
            "base_urls": ["https://api.example.com"],
        })
        .to_string(),
    });
    sqlx::query(
        "INSERT INTO events (id, aggregate, aggregate_id, event_type, payload, created, thread_id) \
         VALUES ($1, 'thread', $2::text, 'CredentialRequested', $3, NOW(), $2)",
    )
    .bind(Uuid::new_v4())
    .bind(thread_id)
    .bind(payload)
    .execute(pool)
    .await
    .expect("failed to insert CredentialRequested");
    (thread_id, request_id)
}

async fn pending_ids(client: &reqwest::Client) -> Vec<String> {
    let resp = client
        .get(format!("{}/api/v1/form-requests/pending", base_url()))
        .send()
        .await
        .expect("pending request failed");
    assert_eq!(resp.status(), 200);
    let body: Vec<serde_json::Value> = resp.json().await.expect("pending body");
    body.iter()
        .filter_map(|p| p["request_id"].as_str().map(str::to_string))
        .collect()
}

async fn cancel(client: &reqwest::Client, request_id: Uuid) -> reqwest::Response {
    client
        .post(format!(
            "{}/api/v1/form-requests/{request_id}/cancel",
            base_url()
        ))
        .send()
        .await
        .expect("cancel request failed")
}

#[tokio::test]
async fn an_open_request_is_listed_until_canceled_and_cancels_once() {
    let client = user_client().await;
    let pool = sqlx::PgPool::connect(&db_url()).await.expect("db connect");
    let (thread_id, request_id) =
        insert_credential_request(&pool, &unique_marker("e2e-form")).await;

    let resp = client
        .get(format!("{}/api/v1/form-requests/pending", base_url()))
        .send()
        .await
        .expect("pending request failed");
    let body: Vec<serde_json::Value> = resp.json().await.expect("pending body");
    let listed = body
        .iter()
        .find(|p| p["request_id"] == request_id.to_string())
        .expect("the open request is listed");
    assert_eq!(listed["thread_id"], thread_id.to_string());
    assert_eq!(
        listed["event"]["type"], "CredentialRequested",
        "served as the stream frame it stands in for"
    );

    let resp = cancel(&client, request_id).await;
    assert_eq!(resp.status(), 200);
    assert_eq!(
        resp.json::<serde_json::Value>().await.unwrap()["resolved"],
        true
    );
    assert!(!pending_ids(&client).await.contains(&request_id.to_string()));

    // A second Cancel (another device, a double tap) closes nothing new.
    let resp = cancel(&client, request_id).await;
    assert_eq!(resp.status(), 200);
    assert_eq!(
        resp.json::<serde_json::Value>().await.unwrap()["resolved"],
        false
    );
    assert_eq!(
        count_events_of_type(&pool, thread_id, "FormRequestResolved").await,
        1
    );
}

#[tokio::test]
async fn saving_the_credential_answers_the_request() {
    let client = user_client().await;
    let pool = sqlx::PgPool::connect(&db_url()).await.expect("db connect");
    let service = unique_marker("e2e-form-save");
    let (thread_id, request_id) = insert_credential_request(&pool, &service).await;

    let resp = client
        .post(format!("{}/api/v1/credentials", base_url()))
        .json(&json!({
            "service_name": service,
            "base_urls": ["https://api.example.com"],
            "auth_type": "api_key",
            "auth_value": "k1",
            "form_request_id": request_id,
        }))
        .send()
        .await
        .expect("create failed");
    assert_eq!(resp.status(), 200);
    assert_eq!(
        resp.json::<serde_json::Value>().await.unwrap()["success"],
        true
    );

    assert!(!pending_ids(&client).await.contains(&request_id.to_string()));
    let outcome: Option<String> = sqlx::query_scalar(
        "SELECT payload->>'outcome' FROM events \
         WHERE event_type = 'FormRequestResolved' AND payload->>'request_id' = $1",
    )
    .bind(request_id.to_string())
    .fetch_optional(&pool)
    .await
    .unwrap();
    assert_eq!(outcome.as_deref(), Some("completed"));
    assert_eq!(
        count_events_of_type(&pool, thread_id, "FormRequestResolved").await,
        1
    );
}

#[tokio::test]
async fn canceling_an_unknown_request_is_a_404() {
    let client = user_client().await;
    let resp = cancel(&client, Uuid::new_v4()).await;
    assert_eq!(resp.status(), 404);
}
