//! The home thread, end to end (ADR 0362, ADR 0411).
//!
//! Every workspace has one from boot. These tests check the wiring the unit
//! tests cannot. The thread list carries it from the first request, and the
//! archive and delete routes refuse it. Only a rename by hand names it, and
//! the retired switch cannot be written back.
//!
//! The home thread is workspace-wide state, so one test walks the whole
//! sequence in order rather than several tests racing on it.

use crate::support::{base_url, db_url, user_client};
use serde_json::json;

/// The key ADR 0411 retired, as clients used to send it: a wire contract.
const RETIRED_SWITCH: &str = "home_thread_enabled";

async fn thread_list(client: &reqwest::Client) -> serde_json::Value {
    client
        .get(format!("{}/api/v1/threads", base_url()))
        .send()
        .await
        .expect("thread list request failed")
        .json()
        .await
        .expect("Invalid JSON")
}

async fn home_rows(pool: &sqlx::PgPool) -> Vec<uuid::Uuid> {
    sqlx::query_scalar("SELECT thread_id FROM thread_summaries WHERE is_home")
        .fetch_all(pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn the_home_thread_exists_from_boot_and_cannot_be_switched_off() {
    let client = user_client().await;
    let pool = sqlx::PgPool::connect(&db_url())
        .await
        .expect("Failed to connect to E2E workspace database");

    // Boot made it before the router served, so the list carries it.
    let homes = home_rows(&pool).await;
    assert_eq!(homes.len(), 1, "{homes:?}");
    let home = homes[0].to_string();
    let body = thread_list(&client).await;
    assert_eq!(body["home_thread"]["thread_id"], home, "{body:?}");
    assert_eq!(body["home_thread"]["home"], true);
    assert_eq!(body["home_thread"]["channel"], "chat");

    // The welcome is drawn by the client, so Home holds no message of its own.
    let messages: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM events WHERE thread_id = $1 \
         AND event_type IN ('MessageReceived', 'TextStreamed', 'ResponseGenerated')",
    )
    .bind(homes[0])
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(messages, 0, "Home holds no message nobody wrote");

    // Archive and delete refuse it.
    for route in ["archive", "delete"] {
        let resp = client
            .post(format!("{}/api/v1/threads/{route}", base_url()))
            .json(&json!({ "thread_id": home }))
            .send()
            .await
            .unwrap_or_else(|e| panic!("{route} request failed: {e}"));
        let status = resp.status().as_u16();
        let body: serde_json::Value = resp.json().await.expect("Invalid JSON");
        assert_eq!(status, 409, "{route} must refuse the home thread: {body:?}");
        assert_eq!(body["reason"], "home_thread", "{route}: {body:?}");
    }

    // The retired switch is refused, so an old client cannot hide Home.
    let resp = client
        .put(format!(
            "{}/api/v1/preferences?key={RETIRED_SWITCH}",
            base_url()
        ))
        .json(&json!({ "value": "false" }))
        .send()
        .await
        .expect("preference write failed");
    let body: serde_json::Value = resp.json().await.expect("Invalid JSON");
    assert_eq!(
        body["success"], false,
        "the retired switch must be refused: {body:?}"
    );
    assert!(body.to_string().contains("ADR 0411"), "{body:?}");
    let stored: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM preferences WHERE key = $1")
        .bind(RETIRED_SWITCH)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(stored, 0);
    assert_eq!(thread_list(&client).await["home_thread"]["thread_id"], home);

    // Only the user names it: a rename lands, a suggestion is refused before
    // and after it, and the rename survives.
    assert_eq!(suggest_title(&client, &home).await, 409);
    rename(&client, &home, "Kitchen table").await;
    assert_eq!(
        thread_list(&client).await["home_thread"]["title"],
        "Kitchen table"
    );
    assert_eq!(suggest_title(&client, &home).await, 409);
    assert_eq!(
        thread_list(&client).await["home_thread"]["title"],
        "Kitchen table"
    );
    // Later suites expect the name it was born with.
    rename(&client, &home, "Home").await;
}

async fn rename(client: &reqwest::Client, thread_id: &str, title: &str) {
    let resp = client
        .post(format!("{}/api/v1/threads/rename", base_url()))
        .json(&json!({ "thread_id": thread_id, "title": title }))
        .send()
        .await
        .expect("rename request failed");
    assert_eq!(resp.status(), 200, "a rename by hand must land on Home");
}

/// The suggest-title status. Its refusal names the home thread.
async fn suggest_title(client: &reqwest::Client, thread_id: &str) -> u16 {
    let resp = client
        .post(format!("{}/api/v1/threads/suggest-title", base_url()))
        .json(&json!({ "thread_id": thread_id }))
        .send()
        .await
        .expect("suggest-title request failed");
    let status = resp.status().as_u16();
    let body = resp.text().await.expect("suggest-title body");
    if status == 409 {
        assert!(body.contains("home thread"), "{body}");
    }
    status
}
