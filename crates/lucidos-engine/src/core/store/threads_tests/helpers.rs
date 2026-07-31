pub(super) use crate::test_support::{setup_test_db, teardown_test_db};
pub(super) use sqlx::PgPool;
pub(super) use uuid::Uuid;

/// Insert a parent thread and a child thread referencing it. Returns
/// `(parent_id, child_id)`. `child_saved` controls whether the child
/// is_saved (the parent is never saved). Both have has_response=TRUE
/// so they show up in get_recent_threads / get_older_threads.
pub(super) async fn insert_parent_child(pool: &PgPool, child_saved: bool) -> (Uuid, Uuid) {
    let parent = Uuid::new_v4();
    let child = Uuid::new_v4();
    sqlx::query(
            "INSERT INTO thread_summaries (thread_id, title, source, message_count, last_activity, has_response, is_saved, parent_thread_id) \
             VALUES ($1, 'Parent thread', 'chat', 1, NOW(), TRUE, FALSE, NULL), \
                    ($2, 'Child thread',  'chat', 1, NOW(), TRUE, $3,   $1)"
        )
        .bind(parent)
        .bind(child)
        .bind(child_saved)
        .execute(pool)
        .await
        .expect("insert thread_summaries");
    (parent, child)
}

/// Insert a trigger-source thread with the given trigger_id/trigger_name and
/// last_activity offset. Returns the new thread id. has_response=TRUE so the
/// thread surfaces in `get_older_threads`.
pub(super) async fn insert_trigger_thread(
    pool: &PgPool,
    trigger_id: &str,
    trigger_name: &str,
    minutes_ago: i64,
) -> Uuid {
    let id = Uuid::new_v4();
    // The drawer sorts by last_user_action, so stagger it (not just
    // last_activity) to control the order these helpers' callers assert on.
    sqlx::query(
            "INSERT INTO thread_summaries \
             (thread_id, title, source, message_count, last_activity, last_user_action, last_agent_action, has_response, is_saved, trigger_id, trigger_name) \
             VALUES ($1, 'T', 'trigger', 1, NOW() - ($2 || ' minutes')::interval, NOW() - ($2 || ' minutes')::interval, NOW() - ($2 || ' minutes')::interval, TRUE, FALSE, $3, $4)",
        )
        .bind(id)
        .bind(minutes_ago.to_string())
        .bind(trigger_id)
        .bind(trigger_name)
        .execute(pool)
        .await
        .expect("insert trigger thread");
    id
}

/// Insert a CC-source thread bound to the given repo UUID with the given
/// last_activity offset. Returns the new thread id.
pub(super) async fn insert_cc_repo_thread(pool: &PgPool, repo_id: &str, minutes_ago: i64) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
            "INSERT INTO thread_summaries \
             (thread_id, title, source, message_count, last_activity, last_user_action, last_agent_action, has_response, is_saved, cc_repo_id) \
             VALUES ($1, 'CC', 'claude_code', 1, NOW() - ($2 || ' minutes')::interval, NOW() - ($2 || ' minutes')::interval, NOW() - ($2 || ' minutes')::interval, TRUE, FALSE, $3)",
        )
        .bind(id)
        .bind(minutes_ago.to_string())
        .bind(repo_id)
        .execute(pool)
        .await
        .expect("insert cc repo thread");
    id
}

/// Register a repo in the `repositories` table so `cc_repo_name` resolves.
pub(super) async fn insert_repository(pool: &PgPool, repo_id: Uuid, name: &str, path: &str) {
    sqlx::query("INSERT INTO repositories (id, name, path) VALUES ($1, $2, $3)")
        .bind(repo_id)
        .bind(name)
        .bind(path)
        .execute(pool)
        .await
        .expect("insert repository");
}

/// Seed the durable `repo_names` projection (what the EventBus
/// `RepositoryAdded` arm writes). Lets a test simulate "this repo's name was
/// recorded, then the repo was removed from the live registry".
pub(super) async fn insert_repo_name(pool: &PgPool, repo_id: Uuid, name: &str) {
    sqlx::query("INSERT INTO repo_names (id, name) VALUES ($1, $2) ON CONFLICT (id) DO UPDATE SET name = EXCLUDED.name")
        .bind(repo_id)
        .bind(name)
        .execute(pool)
        .await
        .expect("insert repo_name");
}

/// Remove a repo from the live `repositories` registry — mirrors
/// `RepositoryStore::remove`. The `repo_names` projection is left untouched.
pub(super) async fn delete_repository(pool: &PgPool, repo_id: Uuid) {
    sqlx::query("DELETE FROM repositories WHERE id = $1")
        .bind(repo_id)
        .execute(pool)
        .await
        .expect("delete repository");
}

/// Insert a `changes` row binding a thread to a `repo_root` path. Powers the
/// repo-name scavenge backfill: a pre-`RepositoryAdded` repo's name is
/// recoverable from this path's basename even when no event recorded it.
pub(super) async fn insert_change(pool: &PgPool, thread_id: Uuid, repo_root: &str) {
    // `branch_name` is unique per pending change (`idx_changes_unique_pending_branch`),
    // so derive a fresh one per row.
    let request_id = Uuid::new_v4();
    sqlx::query("INSERT INTO changes (request_id, branch_name, repo_root, thread_id) VALUES ($1, $2, $3, $4)")
        .bind(request_id)
        .bind(format!("branch-{request_id}"))
        .bind(repo_root)
        .bind(thread_id)
        .execute(pool)
        .await
        .expect("insert change");
}

/// Insert an app coding-agent thread operating on `data/apps/<app_id>` with the
/// given last_activity offset and archive state. Returns the new thread id.
pub(super) async fn insert_app_thread(
    pool: &PgPool,
    app_id: &str,
    minutes_ago: i64,
    archived: bool,
) -> Uuid {
    let id = Uuid::new_v4();
    let folder = format!("/ws/data/apps/{app_id}");
    let archive_state = if archived { "archived" } else { "inbox" };
    sqlx::query(
            "INSERT INTO thread_summaries \
             (thread_id, title, source, message_count, last_activity, last_user_action, last_agent_action, has_response, is_saved, \
              archive_state, coding_agent_kind, coding_agent_folder) \
             VALUES ($1, 'App', 'claude_code', 1, NOW() - ($2 || ' minutes')::interval, NOW() - ($2 || ' minutes')::interval, NOW() - ($2 || ' minutes')::interval, TRUE, FALSE, \
                     $3, 'app', $4)",
        )
        .bind(id)
        .bind(minutes_ago.to_string())
        .bind(archive_state)
        .bind(folder)
        .execute(pool)
        .await
        .expect("insert app thread");
    id
}

/// Insert a trigger-source thread row with NULL trigger_id/trigger_name —
/// the state every legacy thread is in after the broken
/// `20260429214800_addtriggeridtothreadsummaries.sql` backfill, which only
/// reads `payload->>'trigger_id'` and skips the legacy `task_id`/`task_name`
/// pair. Returns the new thread id.
pub(super) async fn insert_null_trigger_thread(pool: &PgPool, minutes_ago: i64) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
            "INSERT INTO thread_summaries \
             (thread_id, title, source, message_count, last_activity, last_user_action, last_agent_action, has_response, is_saved, trigger_id, trigger_name) \
             VALUES ($1, 'T', 'trigger', 1, NOW() - ($2 || ' minutes')::interval, NOW() - ($2 || ' minutes')::interval, NOW() - ($2 || ' minutes')::interval, TRUE, FALSE, NULL, NULL)",
        )
        .bind(id)
        .bind(minutes_ago.to_string())
        .execute(pool)
        .await
        .expect("insert null-trigger thread");
    id
}

/// Insert a `TriggerStarted` event for the given thread with a raw payload.
/// Lets the test mimic legacy (`task_id`) vs modern (`trigger_id`) shapes.
pub(super) async fn insert_trigger_started_event(
    pool: &PgPool,
    thread_id: Uuid,
    payload: serde_json::Value,
) {
    sqlx::query(
        "INSERT INTO events (id, event_type, payload, aggregate, aggregate_id, thread_id) \
             VALUES ($1, 'TriggerStarted', $2, 'thread', $3, $4)",
    )
    .bind(Uuid::new_v4())
    .bind(payload)
    .bind(thread_id.to_string())
    .bind(thread_id)
    .execute(pool)
    .await
    .expect("insert TriggerStarted");
}

pub(super) async fn fetch_trigger_pair(
    pool: &PgPool,
    thread_id: Uuid,
) -> (Option<String>, Option<String>) {
    sqlx::query_as::<_, (Option<String>, Option<String>)>(
        "SELECT trigger_id, trigger_name FROM thread_summaries WHERE thread_id = $1",
    )
    .bind(thread_id)
    .fetch_one(pool)
    .await
    .expect("fetch trigger pair")
}

pub(super) async fn insert_thread(pool: &PgPool, id: Uuid, title: &str) {
    sqlx::query(
            "INSERT INTO thread_summaries (thread_id, title, source, message_count, last_activity, has_response) \
             VALUES ($1, $2, 'chat', 0, NOW(), TRUE)"
        )
        .bind(id)
        .bind(title)
        .execute(pool)
        .await
        .expect("insert thread_summaries");
}

pub(super) async fn insert_message(pool: &PgPool, thread_id: Uuid, event_type: &str, text: &str) {
    sqlx::query(
        "INSERT INTO events (id, event_type, payload, thread_id, aggregate, aggregate_id) \
             VALUES ($1, $2, $3, $4, 'thread', $4::text)",
    )
    .bind(Uuid::new_v4())
    .bind(event_type)
    .bind(serde_json::json!({ "text": text }))
    .bind(thread_id)
    .execute(pool)
    .await
    .expect("insert event");
}

/// `memory_entries` is created by `PgVectorIndex::new` rather than a migration,
/// so any test that joins it must initialize the index first.
pub(super) async fn ensure_memory_entries_table(pool: &PgPool) {
    crate::memory::pgvector::PgVectorIndex::new(pool.clone())
        .await
        .expect("init pgvector schema");
}
