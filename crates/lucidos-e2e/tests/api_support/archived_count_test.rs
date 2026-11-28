use crate::support::{base_url, db_url, http_client};

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

    for i in 0..5 {
        seed_repo_thread(&pool, &format!("ac-arch-{i}"), &repo, "archived", false).await;
    }
    seed_repo_thread(&pool, "ac-inbox", &repo, "inbox", false).await;
    seed_repo_thread(&pool, "ac-saved", &repo, "inbox", true).await;

    let other_repo = uuid::Uuid::new_v4().to_string();
    seed_repo_thread(&pool, "ac-other-repo", &other_repo, "archived", false).await;

    let count = archived_count(&repo).await;
    assert_eq!(
        count, 5,
        "archived-count must count only archived threads matching the repo filter"
    );
}

#[tokio::test]
async fn archived_count_is_zero_for_a_repo_with_no_archived_threads() {
    let pool = sqlx::PgPool::connect(&db_url()).await.expect("connect db");
    let repo = uuid::Uuid::new_v4().to_string();
    seed_repo_thread(&pool, "ac-none-inbox", &repo, "inbox", false).await;

    assert_eq!(archived_count(&repo).await, 0);
}
