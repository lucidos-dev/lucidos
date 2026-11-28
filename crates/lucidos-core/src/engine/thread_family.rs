use sqlx::PgPool;
use uuid::Uuid;

const MAX_FAMILY_LINKS: i32 = 64;

pub async fn family_root(pool: &PgPool, thread_id: Uuid) -> Result<Uuid, sqlx::Error> {
    let root: Option<Uuid> = sqlx::query_scalar(
        "WITH RECURSIVE up(thread_id, next_id, links) AS ( \
            SELECT thread_id, COALESCE(parent_thread_id, successor_of), 0 FROM thread_summaries \
                WHERE thread_id = $1 \
            UNION ALL \
            SELECT t.thread_id, COALESCE(t.parent_thread_id, t.successor_of), up.links + 1 \
            FROM thread_summaries t JOIN up ON t.thread_id = up.next_id \
            WHERE up.links < $2 \
        ) \
        SELECT thread_id FROM up ORDER BY links DESC LIMIT 1",
    )
    .bind(thread_id)
    .bind(MAX_FAMILY_LINKS)
    .fetch_optional(pool)
    .await?;
    Ok(root.unwrap_or(thread_id))
}

#[cfg(test)]
#[path = "thread_family_tests.rs"]
mod tests;
