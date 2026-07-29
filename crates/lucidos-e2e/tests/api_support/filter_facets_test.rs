//! Coverage for the drawer "Show" filter's backend surface:
//! `GET /api/v1/threads/filter-facets` (complete option lists) and the
//! `app_ids` narrowing param on `GET /api/v1/threads/older`.
//!
//! Strategy mirrors `threads_list_test.rs`: seed `thread_summaries` rows
//! directly with unique markers, then assert the endpoints surface them. App
//! threads are identified by `coding_agent_kind='app'` + a
//! `<ws>/data/apps/<id>` folder; the app id is the last path segment.

use crate::support::{base_url, db_url, http_client, unique_marker};

/// Seed an app coding-agent thread on `data/apps/<app_id>` with the given
/// archive state and an age offset (older = larger `minutes_ago`).
async fn seed_app_thread(
    pool: &sqlx::PgPool,
    title: &str,
    app_id: &str,
    minutes_ago: i64,
    archived: bool,
) -> uuid::Uuid {
    let id = uuid::Uuid::new_v4();
    let folder = format!("/ws/data/apps/{app_id}");
    let archive_state = if archived { "archived" } else { "inbox" };
    sqlx::query(
        "INSERT INTO thread_summaries \
            (thread_id, title, first_message, source, initiator, created_at, last_activity, \
             message_count, status, has_response, archive_state, coding_agent_kind, coding_agent_folder) \
         VALUES ($1, $2, $2, 'claude_code', 'user', NOW(), NOW() - ($3 || ' minutes')::interval, \
                 1, 'idle', TRUE, $4, 'app', $5)",
    )
    .bind(id)
    .bind(title)
    .bind(minutes_ago.to_string())
    .bind(archive_state)
    .bind(folder)
    .execute(pool)
    .await
    .expect("seed app thread");
    id
}

#[tokio::test]
async fn filter_facets_lists_app_with_only_archived_thread() {
    let client = http_client();
    let pool = sqlx::PgPool::connect(&db_url()).await.expect("connect db");
    let marker = unique_marker("facets-app").replace('-', "");

    // App whose ONLY thread is archived must still appear as a facet — the
    // dropdown lists every app that has a session, archived or not.
    seed_app_thread(&pool, &format!("{marker}-arch"), &marker, 90, true).await;

    let url = format!("{}/api/v1/threads/filter-facets", base_url());
    let resp = client.get(&url).send().await.expect("facets request");
    assert_eq!(resp.status(), 200, "filter-facets should return 200");
    let body: serde_json::Value = resp.json().await.expect("invalid JSON");

    for key in ["triggers", "repos", "apps"] {
        assert!(body[key].is_array(), "facets.{key} must be an array");
    }
    let app_ids: Vec<&str> = body["apps"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|f| f["id"].as_str())
        .collect();
    assert!(
        app_ids.contains(&marker.as_str()),
        "archived-only app `{marker}` must appear in facets.apps, got {app_ids:?}"
    );
}

#[tokio::test]
async fn older_app_ids_fetches_archived_and_excludes_prefix_matches() {
    let client = http_client();
    let pool = sqlx::PgPool::connect(&db_url()).await.expect("connect db");
    let base = unique_marker("facets-older").replace('-', "");
    let app = base.clone();
    let app_prefix = format!("{base}extra"); // e.g. "<id>" vs "<id>extra"

    let active = seed_app_thread(&pool, &format!("{base}-active"), &app, 30, false).await;
    let archived = seed_app_thread(&pool, &format!("{base}-arch"), &app, 90, true).await;
    let _other = seed_app_thread(&pool, &format!("{base}-other"), &app_prefix, 20, false).await;

    // `before` in the future so all three are "older" than the cursor.
    let before = "2099-01-01T00:00:00Z";
    let url = format!(
        "{}/api/v1/threads/older?before={before}&limit=50&app_ids={app}",
        base_url()
    );
    let body: serde_json::Value = client
        .get(&url)
        .send()
        .await
        .expect("older app_ids request")
        .json()
        .await
        .expect("invalid JSON");

    let ids: Vec<&str> = body["threads"]
        .as_array()
        .expect("threads array")
        .iter()
        .filter_map(|t| t["thread_id"].as_str())
        .collect();

    assert!(
        ids.contains(&active.to_string().as_str()),
        "active app thread must be fetched"
    );
    assert!(
        ids.contains(&archived.to_string().as_str()),
        "archived app thread must be fetched (not hidden)"
    );
    assert!(
        !ids.iter().any(|id| *id == _other.to_string()),
        "prefix-named app must NOT match exact app_id filter"
    );
}
