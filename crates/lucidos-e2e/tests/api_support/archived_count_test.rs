//! Coverage for `GET /api/v1/threads/archived-count` — the filter-scoped total
//! that drives the collapsed Archive badge. The badge must "respect the filter"
//! (count only archived threads matching the active channel/facet selection) and
//! exclude inbox + saved rows, so the number is stable regardless of how many
//! rows are paginated in.
//!
//! Isolation: each test binds its seeded rows to a FRESH `cc_repo_id` UUID and
//! queries with `sources=claude_code&repo_ids=<that uuid>`. The `sources` gate
//! is load-bearing: `channel_facet_filter_sql` narrows by `repo_ids` ONLY within
//! the claude_code channel, so a `repo_ids`-only query still counts archived+
//! unsaved chat / trigger rows left by other tests in the shared workspace (the
//! documented additive-union compose semantics — see `get_archived_count`).
//! Gating to the claude_code channel AND a fresh repo isolates the asserted
//! count to exactly this test's seeded set.

use crate::support::{base_url, db_url, http_client};

/// Seed a claude_code thread bound to `repo_id` with the given archive/saved
/// state. `has_response=TRUE` so it's a normal completed thread.
async fn seed_repo_thread(
    pool: &sqlx::PgPool,
    title: &str,
    repo_id: &str,
    archive_state: &str,
    is_saved: bool,
) {
    sqlx::query(
        "INSERT INTO thread_summaries \
            (thread_id, title, first_message, source, initiator, created_at, last_activity, \
             message_count, status, has_response, archive_state, is_saved, is_coding_agent, cc_repo_id) \
         VALUES ($1, $2, $2, 'claude_code', 'user', NOW(), NOW(), \
                 1, 'idle', TRUE, $3, $4, TRUE, $5)",
    )
    .bind(uuid::Uuid::new_v4())
    .bind(title)
    .bind(archive_state)
    .bind(is_saved)
    .bind(repo_id)
    .execute(pool)
    .await
    .expect("seed repo thread");
}

async fn archived_count(repo_id: &str) -> i64 {
    // `sources=claude_code` is load-bearing — see the module doc. Without it the
    // count also includes archived+unsaved chat / trigger rows from other tests,
    // because the repo facet only narrows the claude_code channel.
    let url = format!(
        "{}/api/v1/threads/archived-count?sources=claude_code&repo_ids={repo_id}",
        base_url()
    );
    let body: serde_json::Value = http_client()
        .get(&url)
        .send()
        .await
        .expect("archived-count request")
        .json()
        .await
        .expect("invalid JSON");
    body["count"].as_i64().expect("count is an integer")
}

#[tokio::test]
async fn archived_count_respects_repo_filter_and_excludes_inbox_and_saved() {
    let pool = sqlx::PgPool::connect(&db_url()).await.expect("connect db");
    let repo = uuid::Uuid::new_v4().to_string();

    // 5 archived (unsaved) → counted; 1 inbox → excluded (Review, not Archive);
    // 1 archived+saved → excluded (routes to Saved, not Archive).
    for i in 0..5 {
        seed_repo_thread(&pool, &format!("ac-arch-{i}"), &repo, "archived", false).await;
    }
    seed_repo_thread(&pool, "ac-inbox", &repo, "inbox", false).await;
    seed_repo_thread(&pool, "ac-saved", &repo, "archived", true).await;

    // Decoy: an archived+unsaved claude_code thread in a DIFFERENT repo must be
    // excluded by the repo_ids facet. This is what "respects the repo filter"
    // means within the claude_code channel — and proves the gate above narrows
    // by repo, not just by source.
    let other_repo = uuid::Uuid::new_v4().to_string();
    seed_repo_thread(&pool, "ac-other-repo", &other_repo, "archived", false).await;

    let count = archived_count(&repo).await;
    assert_eq!(
        count, 5,
        "archived-count must count only archived+unsaved threads matching the repo filter"
    );
}

#[tokio::test]
async fn archived_count_is_zero_for_a_repo_with_no_archived_threads() {
    let pool = sqlx::PgPool::connect(&db_url()).await.expect("connect db");
    let repo = uuid::Uuid::new_v4().to_string();
    // Only an inbox thread for this repo → nothing in the Archive pile.
    seed_repo_thread(&pool, "ac-none-inbox", &repo, "inbox", false).await;

    assert_eq!(archived_count(&repo).await, 0);
}
