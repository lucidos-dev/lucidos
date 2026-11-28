use uuid::Uuid;

use crate::engine::event_bus::{BusEvent, EventBus};
use crate::engine::thread_events::{EventMeta, ThreadEvent};

pub use crate::engine::thread_events::HOME_THREAD_TITLE;

pub async fn home_thread_id(pool: &sqlx::PgPool) -> Result<Option<Uuid>, sqlx::Error> {
    sqlx::query_scalar("SELECT thread_id FROM thread_summaries WHERE is_home")
        .fetch_optional(pool)
        .await
}

pub async fn is_home_thread(pool: &sqlx::PgPool, thread_id: Uuid) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM thread_summaries WHERE thread_id = $1 AND is_home)",
    )
    .bind(thread_id)
    .fetch_one(pool)
    .await
}

pub async fn spawner_is_home(pool: &sqlx::PgPool, parent_id: Uuid) -> bool {
    is_home_thread(pool, parent_id).await.unwrap_or_else(|e| {
        crate::log_legacy!(
            "[FanOut] Home check for {} failed, so its child wakes it: {}",
            parent_id,
            e
        );
        false
    })
}

/// INVARIANT: Create the home thread unless the workspace already has one, and return its
/// id. Safe to call on every boot.
///
/// Two callers racing both emit. The unique marker refuses the loser's row,
/// which rolls its event back, so the loser reads the winner's id instead.
pub async fn ensure_home_thread(
    bus: &EventBus,
    pool: &sqlx::PgPool,
) -> Result<Uuid, Box<dyn std::error::Error + Send + Sync>> {
    if let Some(id) = home_thread_id(pool).await? {
        return Ok(id);
    }
    let thread_id = Uuid::new_v4();
    let emitted = bus
        .emit(BusEvent::Thread {
            thread_id,
            event: ThreadEvent::HomeThreadCreated,
            meta: EventMeta::NONE,
        })
        .await;
    match (emitted, home_thread_id(pool).await?) {
        (Ok(_), _) => {
            crate::log_legacy!("[HomeThread] Created the home thread {}", thread_id);
            Ok(thread_id)
        }
        (Err(_), Some(winner)) => Ok(winner),
        (Err(e), None) => Err(e),
    }
}

#[cfg(test)]
#[path = "home_thread_tests.rs"]
mod tests;
