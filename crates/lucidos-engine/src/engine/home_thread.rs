//! The workspace's *home thread* (ADR 0362, ADR 0411): one chat thread that
//! never ends.
//!
//! Every workspace has one. Boot creates it, and so does the first model call
//! no thread made, whichever comes first. It is marked by
//! `thread_summaries.is_home`, unique in the database, so a workspace holds one
//! at most.
//!
//! **Only the user names it.** A rename by hand lands. Nothing titles it
//! automatically: no generated title and no suggestion, since one topic in a
//! thread that never ends is not its name.
//!
//! Reach widens for it and authority does not. `api::thread_reach` owns that
//! rule, and this module only answers which thread is home.

use uuid::Uuid;

use crate::engine::event_bus::{BusEvent, EventBus};
use crate::engine::thread_events::{EventMeta, ThreadEvent};

/// What the home thread is called when it is created. The user may rename it,
/// and nothing else does.
pub const HOME_THREAD_TITLE: &str = "Home";

/// The home thread's id, or `None` before one exists.
pub async fn home_thread_id(pool: &sqlx::PgPool) -> Result<Option<Uuid>, sqlx::Error> {
    sqlx::query_scalar("SELECT thread_id FROM thread_summaries WHERE is_home")
        .fetch_optional(pool)
        .await
}

/// Is `thread_id` the home thread? A missing row is not.
pub async fn is_home_thread(pool: &sqlx::PgPool, thread_id: Uuid) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM thread_summaries WHERE thread_id = $1 AND is_home)",
    )
    .bind(thread_id)
    .fetch_one(pool)
    .await
}

/// Create the home thread unless the workspace already has one, and return its
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
            crate::log!("[HomeThread] Created the home thread {}", thread_id);
            Ok(thread_id)
        }
        (Err(_), Some(winner)) => Ok(winner),
        (Err(e), None) => Err(e),
    }
}

#[cfg(test)]
#[path = "home_thread_tests.rs"]
mod tests;
