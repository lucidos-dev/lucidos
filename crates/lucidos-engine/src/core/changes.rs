use serde::Serialize;
use sqlx::PgPool;
use std::collections::HashMap;
use uuid::Uuid;

/// A change as returned by API queries.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct Change {
    pub id: Uuid,
    pub request_id: Uuid,
    pub thread_id: Option<Uuid>,
    pub branch_name: String,
    pub repo_root: String,
    pub description: String,
    pub file_count: i32,
    pub files: Vec<String>,
    pub requires_restart: bool,
    pub status: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub resolved_at: Option<chrono::DateTime<chrono::Utc>>,
    pub merge_worktree_path: Option<String>,
    pub merge_temp_branch: Option<String>,
    pub hardened: bool,
    pub pre_merge_sha: Option<String>,
    pub post_merge_sha: Option<String>,
    pub thread_title: Option<String>,
    pub commits: Vec<String>,
}

/// One thread's contribution to the current restart-required toast: the
/// originating thread's title and the list of commit subjects that will land
/// once the engine restarts.
#[derive(Debug, Clone, Serialize)]
pub struct RestartGroup {
    pub thread_id: Option<Uuid>,
    pub thread_title: Option<String>,
    pub commits: Vec<String>,
}

/// Fill `thread_title` on each Change by looking up the originating thread's
/// title in `thread_summaries`. The in-memory ChangesProjection doesn't track
/// titles (separate aggregate), so callers that serialize `Change` to JSON
/// for the UI call this once per response. Single batch query, no N+1.
pub async fn enrich_thread_titles(
    pool: &PgPool,
    changes: &mut [Change],
) -> Result<(), sqlx::Error> {
    enrich_titles(pool, changes, |c| c.thread_id, |c, t| c.thread_title = Some(t)).await
}

/// Same as `enrich_thread_titles` but for `RestartGroup`.
pub async fn enrich_restart_group_titles(
    pool: &PgPool,
    groups: &mut [RestartGroup],
) -> Result<(), sqlx::Error> {
    enrich_titles(pool, groups, |g| g.thread_id, |g, t| g.thread_title = Some(t)).await
}

async fn enrich_titles<T>(
    pool: &PgPool,
    items: &mut [T],
    get_id: impl Fn(&T) -> Option<Uuid>,
    set_title: impl Fn(&mut T, String),
) -> Result<(), sqlx::Error> {
    let titles = fetch_titles_for(pool, items.iter().filter_map(&get_id)).await?;
    if titles.is_empty() {
        return Ok(());
    }
    for item in items.iter_mut() {
        if let Some(tid) = get_id(item) {
            if let Some(t) = titles.get(&tid) {
                set_title(item, t.clone());
            }
        }
    }
    Ok(())
}

async fn fetch_titles_for(
    pool: &PgPool,
    thread_ids: impl Iterator<Item = Uuid>,
) -> Result<HashMap<Uuid, String>, sqlx::Error> {
    let ids: Vec<Uuid> = thread_ids.collect();
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    let rows: Vec<(Uuid, Option<String>)> = sqlx::query_as(
        "SELECT thread_id, title FROM thread_summaries WHERE thread_id = ANY($1)",
    )
    .bind(&ids)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .filter_map(|(id, title)| title.map(|t| (id, t)))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{setup_test_db, teardown_test_db};

    async fn insert_thread_summary(pool: &PgPool, thread_id: Uuid, title: &str) {
        sqlx::query(
            "INSERT INTO thread_summaries (thread_id, title, source, message_count, last_activity, has_response, is_pinned) \
             VALUES ($1, $2, 'chat', 0, NOW(), false, false)"
        )
        .bind(thread_id).bind(title)
        .execute(pool).await.expect("insert thread_summary");
    }

    fn make_change(thread_id: Option<Uuid>) -> Change {
        Change {
            id: Uuid::new_v4(),
            request_id: Uuid::new_v4(),
            thread_id,
            branch_name: "branch-x".into(),
            repo_root: "/repo".into(),
            description: "desc".into(),
            file_count: 1,
            files: vec!["a.rs".into()],
            requires_restart: false,
            status: "pending".into(),
            created_at: chrono::Utc::now(),
            resolved_at: None,
            merge_worktree_path: None,
            merge_temp_branch: None,
            hardened: false,
            pre_merge_sha: None,
            post_merge_sha: None,
            thread_title: None,
            commits: vec![],
        }
    }

    /// `enrich_thread_titles` populates `thread_title` from `thread_summaries`
    /// in a single batch query, leaves `None` for changes whose thread has no
    /// title (or no thread_id), and is a no-op when called with no changes.
    #[tokio::test]
    async fn enrich_thread_titles_populates_from_summaries() {
        let (pool, db) = setup_test_db().await;

        let with_title = Uuid::new_v4();
        let no_title = Uuid::new_v4();
        insert_thread_summary(&pool, with_title, "Refactor auth").await;
        // no_title thread is intentionally never inserted

        let mut changes = vec![
            make_change(Some(with_title)),
            make_change(Some(no_title)),
            make_change(None),
        ];

        enrich_thread_titles(&pool, &mut changes)
            .await
            .expect("enrich");

        assert_eq!(changes[0].thread_title.as_deref(), Some("Refactor auth"));
        assert_eq!(
            changes[1].thread_title, None,
            "thread without summary stays None"
        );
        assert_eq!(
            changes[2].thread_title, None,
            "no thread_id stays None"
        );

        // Empty input is a no-op (and doesn't issue any query)
        let mut empty: Vec<Change> = vec![];
        enrich_thread_titles(&pool, &mut empty)
            .await
            .expect("empty no-op");

        teardown_test_db(&db).await;
    }
}
