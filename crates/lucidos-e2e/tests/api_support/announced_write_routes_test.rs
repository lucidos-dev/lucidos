//! The routes that replace an agent's direct SQL write each announce the
//! change (ADR 0760). A direct write emitted nothing,
//! so every client that follows events showed stale data.

use crate::support::{base_url, db_url, unique_marker, user_client, E2E_DEVICE_ID};
use lucidos_engine::core::prefs;

/// How many persisted events of `event_type` carry `needle` in their payload.
async fn events_naming(pool: &sqlx::PgPool, event_type: &str, needle: &str) -> i64 {
    sqlx::query_scalar(
        "SELECT COUNT(*) FROM events WHERE event_type = $1 AND position($2 in payload::text) > 0",
    )
    .bind(event_type)
    .bind(needle)
    .fetch_one(pool)
    .await
    .unwrap_or_else(|e| panic!("counting {event_type} failed: {e}"))
}

#[tokio::test]
async fn a_pin_and_an_unpin_each_announce_themselves() {
    let pool = sqlx::PgPool::connect(&db_url()).await.unwrap();
    let owner = user_client().await;
    let app_id = unique_marker("pinned-app");
    let url = format!("{}/api/v1/pinned-apps", base_url());
    let body = serde_json::json!({ "app_id": app_id, "device_id": E2E_DEVICE_ID });

    let pinned = owner.post(&url).json(&body).send().await.unwrap();
    assert_eq!(pinned.status(), 200);
    let listed: serde_json::Value = owner
        .get(format!("{url}?device_id={E2E_DEVICE_ID}"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let unpinned = owner.delete(&url).json(&body).send().await.unwrap();
    assert_eq!(unpinned.status(), 200);

    assert!(listed.to_string().contains(&app_id), "{listed}");
    assert_eq!(events_naming(&pool, "PinnedAppPinned", &app_id).await, 1);
    assert_eq!(events_naming(&pool, "PinnedAppUnpinned", &app_id).await, 1);
}

#[tokio::test]
async fn a_preference_reset_announces_the_return_to_the_default() {
    let pool = sqlx::PgPool::connect(&db_url()).await.unwrap();
    let owner = user_client().await;
    let key = prefs::EXTERNAL_LINK_TARGET.key();
    let url = format!("{}/api/v1/preferences?key={key}", base_url());
    let resets = "SELECT COUNT(*) FROM events WHERE event_type = 'PreferencesChanged' \
                  AND payload->'data'->>'key' = $1 AND payload->'data'->>'value' IS NULL";
    let count = || async {
        sqlx::query_scalar::<_, i64>(resets)
            .bind(key)
            .fetch_one(&pool)
            .await
            .unwrap()
    };

    let set = owner
        .put(&url)
        .json(&serde_json::json!({ "value": "ask" }))
        .send()
        .await
        .unwrap();
    assert_eq!(set.status(), 200);
    let before = count().await;
    let reset = owner.delete(&url).send().await.unwrap();
    assert_eq!(reset.status(), 200);

    assert_eq!(count().await, before + 1);
}

/// The generated command reaches the gated route. The CLI's machine-local
/// token passes the mutating gate, which answers 401 only to no credential.
/// The owner gate then refuses a human-only key with 403, exactly as on `set`.
#[test]
fn the_cli_reset_command_meets_the_route_gate() {
    let out = crate::lucidos_cli_test::lucidos_cmd()
        .args(["preferences", "reset", "--key"])
        .arg(prefs::SNAPSHOT_RESTORE_BY_AGENTS.key())
        .output()
        .expect("lucidos preferences reset runs");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success() && stderr.contains("403"), "{stderr}");
}

#[tokio::test]
async fn forgetting_a_browser_login_removes_its_row() {
    let pool = sqlx::PgPool::connect(&db_url()).await.unwrap();
    let owner = user_client().await;
    let domain = format!("{}.example.com", unique_marker("login"));
    sqlx::query("INSERT INTO browser_logins (domain, label) VALUES ($1, $1)")
        .bind(&domain)
        .execute(&pool)
        .await
        .unwrap();

    let forgot = owner
        .post(format!("{}/api/v1/browser/forget-login", base_url()))
        .json(&serde_json::json!({ "domain": domain }))
        .send()
        .await
        .unwrap();
    assert_eq!(forgot.status(), 200);

    let left: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM browser_logins WHERE domain = $1")
        .bind(&domain)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(left, 0);
}

/// An unknown id changes nothing and fails with the tool's own sentence, so a
/// script that replaced a SQL delete sees its command fail.
#[tokio::test]
async fn correcting_an_unknown_memory_id_fails_with_the_tools_sentence() {
    let owner = user_client().await;
    let id = uuid::Uuid::new_v4();
    let response = owner
        .post(format!("{}/api/v1/memory/correct-by-id", base_url()))
        .json(&serde_json::json!({ "id": id }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::NOT_FOUND);
    let answer: serde_json::Value = response.json().await.unwrap();
    let error = answer["error"].as_str().unwrap_or_default();
    assert!(error.contains(&id.to_string()), "{answer}");

    let invalid = owner
        .post(format!("{}/api/v1/memory/correct-by-id", base_url()))
        .json(&serde_json::json!({ "id": "not-a-uuid" }))
        .send()
        .await
        .unwrap();
    assert_eq!(invalid.status(), reqwest::StatusCode::BAD_REQUEST);
}
