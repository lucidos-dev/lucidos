use serde::Serialize;
use sqlx::PgPool;
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

/// Common SELECT columns for Change queries, joining thread_summaries for thread_title.
const CHANGE_SELECT: &str = "SELECT c.id, c.request_id, c.thread_id, c.branch_name, c.repo_root, \
    c.description, c.file_count, c.files, c.requires_restart, c.status, c.created_at, \
    c.resolved_at, c.merge_worktree_path, c.merge_temp_branch, c.hardened, \
    c.pre_merge_sha, c.post_merge_sha, ts.title AS thread_title, c.commits \
    FROM changes c LEFT JOIN thread_summaries ts ON c.thread_id = ts.thread_id";

/// Project a ChangeProposed event into the changes table.
/// `hardened` is set atomically at insert time — callers must declare
/// the hardening status up front (no separate `set_hardened` needed).
#[allow(clippy::too_many_arguments)]
pub async fn apply_change_proposed(
    pool: &PgPool,
    change_id: Uuid,
    request_id: Uuid,
    thread_id: Option<Uuid>,
    branch_name: &str,
    repo_root: &str,
    description: &str,
    files: &[String],
    requires_restart: bool,
    hardened: bool,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO changes (id, request_id, thread_id, branch_name, repo_root, description, file_count, files, requires_restart, hardened)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)"
    )
    .bind(change_id)
    .bind(request_id)
    .bind(thread_id)
    .bind(branch_name)
    .bind(repo_root)
    .bind(description)
    .bind(files.len() as i32)
    .bind(files)
    .bind(requires_restart)
    .bind(hardened)
    .execute(pool)
    .await?;
    Ok(())
}

/// Project a ChangeApplied event into the changes table. The commit subjects
/// are persisted so the restart-required toast can list them after page reload
/// (not only while the live SSE event is in scope).
pub async fn apply_change_applied(
    pool: &PgPool,
    change_id: Uuid,
    commits: &[String],
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE changes SET status = 'applied', resolved_at = NOW(), commits = $2 WHERE id = $1",
    )
    .bind(change_id)
    .bind(commits)
    .execute(pool)
    .await?;
    Ok(())
}

/// Project a ChangeDiscarded event into the changes table.
pub async fn apply_change_discarded(pool: &PgPool, change_id: Uuid) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE changes SET status = 'discarded', resolved_at = NOW() WHERE id = $1")
        .bind(change_id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Update an existing pending change with fresh metadata (description, files, requires_restart, hardened).
/// Called when CC continues working after the initial change was proposed.
/// `hardened` is updated atomically — if CC committed after the last harden, this downgrades to false.
pub async fn update_pending(
    pool: &PgPool,
    id: Uuid,
    description: &str,
    files: &[String],
    requires_restart: bool,
    hardened: bool,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE changes SET description = $1, file_count = $2, files = $3, requires_restart = $4, hardened = $5 \
         WHERE id = $6 AND status = 'pending'"
    )
    .bind(description)
    .bind(files.len() as i32)
    .bind(files)
    .bind(requires_restart)
    .bind(hardened)
    .bind(id)
    .execute(pool)
    .await?;
    Ok(())
}

/// Get all pending changes (status = 'pending').
pub async fn list_pending(pool: &PgPool) -> Result<Vec<Change>, sqlx::Error> {
    sqlx::query_as::<_, Change>(&format!(
        "{CHANGE_SELECT} WHERE c.status = 'pending' ORDER BY c.created_at ASC"
    ))
    .fetch_all(pool)
    .await
}

/// Get branch names that have completed changes (applied or discarded).
pub async fn list_completed_branches(pool: &PgPool) -> Result<Vec<String>, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT DISTINCT branch_name FROM changes WHERE status IN ('applied', 'discarded')",
    )
    .fetch_all(pool)
    .await
}

/// Check if a pending change exists for a given branch name.
pub async fn has_pending_for_branch(pool: &PgPool, branch_name: &str) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM changes WHERE branch_name = $1 AND status = 'pending')",
    )
    .bind(branch_name)
    .fetch_one(pool)
    .await
}

/// Check if any OTHER pending change exists for a given branch (excluding a specific change ID).
pub async fn other_pending_for_branch(
    pool: &PgPool,
    branch_name: &str,
    exclude_id: Uuid,
) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM changes WHERE branch_name = $1 AND status = 'pending' AND id != $2)")
        .bind(branch_name)
        .bind(exclude_id)
        .fetch_one(pool)
        .await
}

/// Get a pending change by branch name (if one exists).
pub async fn get_pending_by_branch(
    pool: &PgPool,
    branch_name: &str,
) -> Result<Option<Change>, sqlx::Error> {
    sqlx::query_as::<_, Change>(&format!(
        "{CHANGE_SELECT} WHERE c.branch_name = $1 AND c.status = 'pending' LIMIT 1"
    ))
    .bind(branch_name)
    .fetch_optional(pool)
    .await
}

/// Get all pending changes for a specific thread.
pub async fn pending_for_thread(
    pool: &PgPool,
    thread_id: Uuid,
) -> Result<Vec<Change>, sqlx::Error> {
    sqlx::query_as::<_, Change>(&format!(
        "{CHANGE_SELECT} WHERE c.thread_id = $1 AND c.status = 'pending'"
    ))
    .bind(thread_id)
    .fetch_all(pool)
    .await
}

/// Get a single change by ID.
pub async fn get_by_id(pool: &PgPool, id: Uuid) -> Result<Option<Change>, sqlx::Error> {
    sqlx::query_as::<_, Change>(&format!("{CHANGE_SELECT} WHERE c.id = $1"))
        .bind(id)
        .fetch_optional(pool)
        .await
}

/// Get recently applied changes (status = 'applied' or 'reverted'), most recent first.
/// Supports cursor-based pagination: `limit` controls page size, `before` fetches older items.
pub async fn list_recently_applied(
    pool: &PgPool,
    limit: i64,
    before: Option<chrono::DateTime<chrono::Utc>>,
) -> Result<Vec<Change>, sqlx::Error> {
    if let Some(before_ts) = before {
        sqlx::query_as::<_, Change>(
            &format!("{CHANGE_SELECT} WHERE c.status IN ('applied', 'reverted') AND c.resolved_at < $1 ORDER BY c.resolved_at DESC LIMIT $2")
        )
        .bind(before_ts)
        .bind(limit)
        .fetch_all(pool)
        .await
    } else {
        sqlx::query_as::<_, Change>(
            &format!("{CHANGE_SELECT} WHERE c.status IN ('applied', 'reverted') ORDER BY c.resolved_at DESC LIMIT $1")
        )
        .bind(limit)
        .fetch_all(pool)
        .await
    }
}

/// Whether any changes requiring restart have been applied since a given timestamp.
/// Used to derive `restart_required` from persistent data instead of in-memory state.
pub async fn requires_restart_since(pool: &PgPool, since: chrono::DateTime<chrono::Utc>) -> bool {
    sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM changes WHERE status = 'applied' AND requires_restart = true AND resolved_at > $1)"
    )
    .bind(since)
    .fetch_one(pool)
    .await
    .unwrap_or(false)
}

/// Whether any applied changes since a given timestamp contain frontend files
/// (.ts, .tsx, .css, .html, .js, .jsx). Used by the API to tell the client
/// it should reload to pick up new code.
pub async fn client_update_since(pool: &PgPool, since: chrono::DateTime<chrono::Utc>) -> bool {
    sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(
            SELECT 1 FROM changes
            WHERE status = 'applied' AND resolved_at > $1
            AND EXISTS(
                SELECT 1 FROM unnest(files) f(name)
                WHERE name ~ '\\.(ts|tsx|css|html|js|jsx)$'
            )
        )",
    )
    .bind(since)
    .fetch_one(pool)
    .await
    .unwrap_or(false)
}

/// Project a ChangeReverted event into the changes table.
pub async fn apply_change_reverted(pool: &PgPool, change_id: Uuid) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE changes SET status = 'reverted', resolved_at = NOW() WHERE id = $1")
        .bind(change_id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Store the pre/post merge SHA range so revert can find the exact commits.
pub async fn set_merge_shas(
    pool: &PgPool,
    id: Uuid,
    pre_sha: &str,
    post_sha: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE changes SET pre_merge_sha = $1, post_merge_sha = $2 WHERE id = $3")
        .bind(pre_sha)
        .bind(post_sha)
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Store the merge worktree path and temp branch for a change with an in-progress merge.
pub async fn set_merge_worktree(
    pool: &PgPool,
    id: Uuid,
    path: &str,
    branch: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE changes SET merge_worktree_path = $1, merge_temp_branch = $2 WHERE id = $3",
    )
    .bind(path)
    .bind(branch)
    .bind(id)
    .execute(pool)
    .await?;
    Ok(())
}

/// Clear the merge worktree fields (after cleanup).
pub async fn clear_merge_worktree(pool: &PgPool, id: Uuid) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE changes SET merge_worktree_path = NULL, merge_temp_branch = NULL WHERE id = $1",
    )
    .bind(id)
    .execute(pool)
    .await?;
    Ok(())
}

/// Mark a change as hardened (hardening pass completed).
pub async fn set_hardened(pool: &PgPool, id: Uuid) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE changes SET hardened = true WHERE id = $1")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

/// List pending + recently applied changes for a specific repo path.
/// Returns (pending, applied, has_more) where has_more indicates pagination availability.
pub async fn list_for_repo(
    pool: &PgPool,
    repo_root: &str,
    applied_limit: i64,
    before: Option<chrono::DateTime<chrono::Utc>>,
) -> Result<(Vec<Change>, Vec<Change>, bool), sqlx::Error> {
    let pending = sqlx::query_as::<_, Change>(&format!(
        "{CHANGE_SELECT} WHERE c.status = 'pending' AND c.repo_root = $1 ORDER BY c.created_at ASC"
    ))
    .bind(repo_root)
    .fetch_all(pool)
    .await?;

    let mut applied = if let Some(before_ts) = before {
        sqlx::query_as::<_, Change>(
            &format!("{CHANGE_SELECT} WHERE c.status IN ('applied', 'reverted') AND c.repo_root = $1 AND c.resolved_at < $2 ORDER BY c.resolved_at DESC LIMIT $3")
        )
        .bind(repo_root)
        .bind(before_ts)
        .bind(applied_limit + 1)
        .fetch_all(pool)
        .await?
    } else {
        sqlx::query_as::<_, Change>(
            &format!("{CHANGE_SELECT} WHERE c.status IN ('applied', 'reverted') AND c.repo_root = $1 ORDER BY c.resolved_at DESC LIMIT $2")
        )
        .bind(repo_root)
        .bind(applied_limit + 1)
        .fetch_all(pool)
        .await?
    };

    let has_more = applied.len() as i64 > applied_limit;
    if has_more {
        applied.truncate(applied_limit as usize);
    }

    Ok((pending, applied, has_more))
}

/// Get all changes that have an active merge worktree (for startup cleanup).
pub async fn with_merge_worktree(pool: &PgPool) -> Result<Vec<Change>, sqlx::Error> {
    sqlx::query_as::<_, Change>(&format!(
        "{CHANGE_SELECT} WHERE c.merge_worktree_path IS NOT NULL AND c.status = 'pending'"
    ))
    .fetch_all(pool)
    .await
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

/// Restart groups for applied-but-not-yet-restarted changes since `since`.
/// One group per originating thread; commits from multiple changes on the
/// same thread are concatenated in apply order.
pub async fn restart_groups_since(
    pool: &PgPool,
    since: chrono::DateTime<chrono::Utc>,
) -> Result<Vec<RestartGroup>, sqlx::Error> {
    let rows = sqlx::query_as::<
        _,
        (
            Option<Uuid>,
            Option<String>,
            Vec<String>,
            chrono::DateTime<chrono::Utc>,
        ),
    >(
        "SELECT c.thread_id, ts.title, c.commits, c.resolved_at \
         FROM changes c LEFT JOIN thread_summaries ts ON c.thread_id = ts.thread_id \
         WHERE c.status = 'applied' AND c.requires_restart = true AND c.resolved_at > $1 \
         ORDER BY c.resolved_at ASC",
    )
    .bind(since)
    .fetch_all(pool)
    .await?;

    let mut groups: Vec<RestartGroup> = Vec::new();
    for (thread_id, title, commits, _resolved_at) in rows {
        if let Some(g) = groups.iter_mut().find(|g| g.thread_id == thread_id) {
            for c in commits {
                if !g.commits.contains(&c) {
                    g.commits.push(c);
                }
            }
            if g.thread_title.is_none() {
                g.thread_title = title;
            }
        } else {
            groups.push(RestartGroup {
                thread_id,
                thread_title: title,
                commits,
            });
        }
    }
    Ok(groups)
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

    async fn insert_applied_change_requiring_restart(
        pool: &PgPool,
        thread_id: Uuid,
        commits: &[&str],
    ) -> Uuid {
        let id = Uuid::new_v4();
        let commits_vec: Vec<String> = commits.iter().map(|s| s.to_string()).collect();
        sqlx::query(
            "INSERT INTO changes (id, request_id, thread_id, branch_name, repo_root, description, file_count, files, requires_restart, hardened, status, resolved_at, commits) \
             VALUES ($1, $2, $3, $4, '/repo', 'desc', 1, ARRAY['file.rs'], true, true, 'applied', NOW(), $5)"
        )
        .bind(id)
        .bind(Uuid::new_v4())
        .bind(thread_id)
        .bind(format!("branch-{}", id.as_simple()))
        .bind(&commits_vec)
        .execute(pool).await.expect("insert change");
        id
    }

    /// `restart_groups_since` returns one group per originating thread,
    /// populated with the thread's title and merged commit list, for changes
    /// applied after the cutoff timestamp.
    #[tokio::test]
    async fn restart_groups_since_returns_thread_title_and_commits() {
        let (pool, db) = setup_test_db().await;

        let thread_a = Uuid::new_v4();
        let thread_b = Uuid::new_v4();
        insert_thread_summary(&pool, thread_a, "Fix toast detail").await;
        insert_thread_summary(&pool, thread_b, "Add restart panel").await;

        // Cutoff: anything resolved after this counts.
        let since = chrono::Utc::now() - chrono::Duration::seconds(10);

        insert_applied_change_requiring_restart(&pool, thread_a, &["fix: a1", "fix: a2"]).await;
        insert_applied_change_requiring_restart(&pool, thread_a, &["fix: a3"]).await;
        insert_applied_change_requiring_restart(&pool, thread_b, &["feat: b1"]).await;

        let groups = restart_groups_since(&pool, since)
            .await
            .expect("restart_groups_since");

        assert_eq!(groups.len(), 2, "one group per thread, got {:?}", groups);
        let g_a = groups
            .iter()
            .find(|g| g.thread_id == Some(thread_a))
            .expect("group A");
        let g_b = groups
            .iter()
            .find(|g| g.thread_id == Some(thread_b))
            .expect("group B");
        assert_eq!(g_a.thread_title.as_deref(), Some("Fix toast detail"));
        assert_eq!(
            g_a.commits,
            vec![
                "fix: a1".to_string(),
                "fix: a2".to_string(),
                "fix: a3".to_string()
            ]
        );
        assert_eq!(g_b.thread_title.as_deref(), Some("Add restart panel"));
        assert_eq!(g_b.commits, vec!["feat: b1".to_string()]);

        teardown_test_db(&db).await;
    }

    /// Changes resolved before the cutoff are excluded — i.e. changes from
    /// a previous engine session that were already applied-and-restarted.
    #[tokio::test]
    async fn restart_groups_since_filters_by_cutoff() {
        let (pool, db) = setup_test_db().await;
        let thread = Uuid::new_v4();
        insert_thread_summary(&pool, thread, "old work").await;
        insert_applied_change_requiring_restart(&pool, thread, &["old commit"]).await;

        // Cutoff in the future — nothing should match.
        let future = chrono::Utc::now() + chrono::Duration::hours(1);
        let groups = restart_groups_since(&pool, future)
            .await
            .expect("restart_groups_since");
        assert!(
            groups.is_empty(),
            "no groups when cutoff is after all resolved_at: {:?}",
            groups
        );

        teardown_test_db(&db).await;
    }

    async fn insert_pending_change(
        pool: &PgPool,
        thread_id: Option<Uuid>,
        branch: &str,
        repo_root: &str,
    ) -> Uuid {
        let id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO changes (id, request_id, thread_id, branch_name, repo_root, description, file_count, files, requires_restart, hardened) \
             VALUES ($1, $2, $3, $4, $5, 'desc', 1, ARRAY['file.rs'], false, true)"
        )
        .bind(id)
        .bind(Uuid::new_v4())
        .bind(thread_id)
        .bind(branch)
        .bind(repo_root)
        .execute(pool).await.expect("insert change");
        id
    }

    /// `list_pending` (and every other CHANGE_SELECT consumer) joins
    /// thread_summaries to populate `thread_title`. Smoke-tests CHANGE_SELECT
    /// against the live schema so a column rename or join typo fails here
    /// instead of taking down /api/changes in production.
    #[tokio::test]
    async fn list_pending_returns_thread_title_via_join() {
        let (pool, db) = setup_test_db().await;

        let thread = Uuid::new_v4();
        insert_thread_summary(&pool, thread, "Refactor auth").await;
        let with_thread = insert_pending_change(&pool, Some(thread), "branch-a", "/repo").await;
        let without_thread = insert_pending_change(&pool, None, "branch-b", "/repo").await;

        let pending = list_pending(&pool).await.expect("list_pending");

        let a = pending
            .iter()
            .find(|c| c.id == with_thread)
            .expect("change with thread");
        assert_eq!(a.thread_title.as_deref(), Some("Refactor auth"));

        let b = pending
            .iter()
            .find(|c| c.id == without_thread)
            .expect("change without thread");
        assert_eq!(b.thread_title, None);

        teardown_test_db(&db).await;
    }

    /// `apply_change_applied` records the merged commits so they survive
    /// page reload (not just available in the live SSE event).
    #[tokio::test]
    async fn apply_change_applied_persists_commits() {
        let (pool, db) = setup_test_db().await;

        let change_id = Uuid::new_v4();
        let commits = vec!["feat: x".to_string(), "fix: y".to_string()];
        sqlx::query(
            "INSERT INTO changes (id, request_id, branch_name, repo_root, description, file_count, files, requires_restart, hardened) \
             VALUES ($1, $2, $3, '/repo', 'desc', 1, ARRAY['f.rs'], true, true)"
        )
        .bind(change_id).bind(Uuid::new_v4()).bind("branch-x")
        .execute(&pool).await.unwrap();

        apply_change_applied(&pool, change_id, &commits)
            .await
            .expect("apply_change_applied");

        let stored: Vec<String> = sqlx::query_scalar("SELECT commits FROM changes WHERE id = $1")
            .bind(change_id)
            .fetch_one(&pool)
            .await
            .expect("read commits back");
        assert_eq!(stored, commits);

        teardown_test_db(&db).await;
    }
}
