//! Which agent runs an existing thread's input (ADR 0678).

use uuid::Uuid;

/// The agent that runs every input on an existing thread. The thread decides,
/// never the caller.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ThreadAgent {
    LucidosAgent,
    CodingAgent,
}

/// The agent of a sent thread, read from its `source`. `None` for a missing row
/// or a composing draft, whose source can still change before its first send.
/// Never read `is_coding_agent` for this: it drifts.
pub(crate) async fn thread_agent(
    pool: &sqlx::PgPool,
    thread_id: Uuid,
) -> Result<Option<ThreadAgent>, sqlx::Error> {
    let source: Option<Option<String>> = sqlx::query_scalar(
        "SELECT source FROM thread_summaries WHERE thread_id = $1 AND state <> 'composing'",
    )
    .bind(thread_id)
    .fetch_optional(pool)
    .await?;
    Ok(source.map(|source| match source.as_deref() {
        Some("claude_code") => ThreadAgent::CodingAgent,
        _ => ThreadAgent::LucidosAgent,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::event_bus::EventBus;
    use crate::test_support::{
        seed_chat_thread, seed_coding_thread, setup_test_db, teardown_test_db,
    };

    #[tokio::test]
    async fn the_source_decides_and_a_missing_row_is_unknown() {
        let (pool, db) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let coding = seed_coding_thread(&bus, None).await;
        let chat = seed_chat_thread(&bus, "hello").await;
        assert_eq!(
            thread_agent(&pool, coding).await.unwrap(),
            Some(ThreadAgent::CodingAgent)
        );
        assert_eq!(
            thread_agent(&pool, chat).await.unwrap(),
            Some(ThreadAgent::LucidosAgent)
        );
        assert_eq!(thread_agent(&pool, Uuid::new_v4()).await.unwrap(), None);

        sqlx::query("UPDATE thread_summaries SET is_coding_agent = false WHERE thread_id = $1")
            .bind(coding)
            .execute(&pool)
            .await
            .unwrap();
        assert_eq!(
            thread_agent(&pool, coding).await.unwrap(),
            Some(ThreadAgent::CodingAgent),
            "a drifted is_coding_agent does not move the lane"
        );
        teardown_test_db(&db).await;
    }
}
