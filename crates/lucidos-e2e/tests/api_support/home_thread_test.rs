//! The home thread, end to end (ADR 0362, invariant I10).
//!
//! It sits behind the experimental `home_thread_enabled` switch, off by
//! default. These tests check the wiring the unit tests cannot: the switch's
//! write yields one thread, the thread list carries or hides it, and the
//! archive and delete routes refuse it. Only a rename by hand names it.
//!
//! The switch is workspace-wide state, so one test walks the whole sequence
//! in order rather than several tests racing on it.

use crate::support::{base_url, db_url, user_client};
use lucidos_engine::core::prefs;
use serde_json::json;

async fn set_home_switch(client: &reqwest::Client, on: bool) {
    let resp = client
        .put(format!(
            "{}/api/v1/preferences?key={}",
            base_url(),
            prefs::HOME_THREAD_ENABLED.key()
        ))
        .json(&json!({ "value": if on { "true" } else { "false" } }))
        .send()
        .await
        .expect("preference write failed");
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.expect("Invalid JSON");
    assert_eq!(body["success"], true, "{body:?}");
}

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

/// No list in the body names this thread.
fn lists_mention(body: &serde_json::Value, id: &str) -> bool {
    [
        "saved",
        "archive",
        "active_threads",
        "composing",
        "family_threads",
    ]
    .iter()
    .filter_map(|key| body[*key].as_array())
    .flatten()
    .any(|t| t["thread_id"] == id)
}

#[tokio::test]
async fn the_home_thread_switch_creates_hides_and_restores_one_thread() {
    let client = user_client().await;
    let pool = sqlx::PgPool::connect(&db_url())
        .await
        .expect("Failed to connect to E2E workspace database");

    // Off by default: no list carries a home thread. The row itself may
    // exist, hidden: a model call no thread caused creates it (ADR 0381), as
    // the proxy cost test does.
    let hidden = home_rows(&pool).await;
    assert!(hidden.len() <= 1, "{hidden:?}");
    assert!(thread_list(&client).await["home_thread"].is_null());

    // On: the write creates it unless a hidden one exists, which it adopts.
    // The list carries it.
    set_home_switch(&client, true).await;
    let homes = home_rows(&pool).await;
    assert_eq!(homes.len(), 1, "{homes:?}");
    if !hidden.is_empty() {
        assert_eq!(homes, hidden, "turning it on must adopt the hidden one");
    }
    let home = homes[0].to_string();
    let body = thread_list(&client).await;
    assert_eq!(body["home_thread"]["thread_id"], home, "{body:?}");
    assert_eq!(body["home_thread"]["home"], true);
    assert_eq!(body["home_thread"]["channel"], "chat");

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

    // Off: hidden from every list, even when asked for by focus, and kept.
    set_home_switch(&client, false).await;
    let body = client
        .get(format!("{}/api/v1/threads?focused={home}", base_url()))
        .send()
        .await
        .expect("thread list request failed")
        .json::<serde_json::Value>()
        .await
        .expect("Invalid JSON");
    assert!(body["home_thread"].is_null(), "{body:?}");
    assert!(body.get("focused_thread").is_none(), "{body:?}");
    assert!(!lists_mention(&body, &home), "{body:?}");
    let flat: serde_json::Value = client
        .get(format!("{}/api/v1/threads/list?limit=1000", base_url()))
        .send()
        .await
        .expect("flat list request failed")
        .json()
        .await
        .expect("Invalid JSON");
    assert!(
        !flat
            .as_array()
            .unwrap()
            .iter()
            .any(|t| t["thread_id"] == home),
        "the flat list must hide it too"
    );
    assert_eq!(home_rows(&pool).await.len(), 1, "off never removes the row");

    // On again: the same thread, never a second one.
    set_home_switch(&client, true).await;
    assert_eq!(home_rows(&pool).await, homes);
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
