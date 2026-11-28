pub(super) use crate::test_support::{setup_test_db, teardown_test_db};
pub(super) use sqlx::PgPool;
pub(super) use uuid::Uuid;

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

pub(super) async fn insert_trigger_thread(
    pool: &PgPool,
    trigger_id: &str,
    trigger_name: &str,
    minutes_ago: i64,
) -> Uuid {
    let id = Uuid::new_v4();
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

pub(super) async fn insert_repository(pool: &PgPool, repo_id: Uuid, name: &str, path: &str) {
    sqlx::query("INSERT INTO repositories (id, name, path) VALUES ($1, $2, $3)")
        .bind(repo_id)
        .bind(name)
        .bind(path)
        .execute(pool)
        .await
        .expect("insert repository");
}

pub(super) async fn insert_repo_name(pool: &PgPool, repo_id: Uuid, name: &str) {
    sqlx::query("INSERT INTO repo_names (id, name) VALUES ($1, $2) ON CONFLICT (id) DO UPDATE SET name = EXCLUDED.name")
        .bind(repo_id)
        .bind(name)
        .execute(pool)
        .await
        .expect("insert repo_name");
}

pub(super) async fn delete_repository(pool: &PgPool, repo_id: Uuid) {
    sqlx::query("DELETE FROM repositories WHERE id = $1")
        .bind(repo_id)
        .execute(pool)
        .await
        .expect("delete repository");
}

pub(super) async fn insert_change(pool: &PgPool, thread_id: Uuid, repo_root: &str) {
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

pub(super) async fn initialize_memory_entries(pool: &PgPool) {
    crate::core::store::ensure_memory_entries_table(pool)
        .await
        .expect("init pgvector schema");
}
