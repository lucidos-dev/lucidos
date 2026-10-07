//! The workspace's *home thread* (ADR 0362): one chat thread that never ends.
//!
//! It sits behind the experimental `home_thread_enabled` switch, off by
//! default. Boot creates it only while the switch is on, and turning the switch
//! on creates it. It is marked by `thread_summaries.is_home`, unique in the
//! database, so a workspace holds one at most.
//!
//! **[`home_thread_id`] and [`is_home_thread`] are the gate.** While the switch
//! is off they answer "none", so the drawer, reach, voice and the agent's
//! notice all lose it at once. Archive, delete, triage and titling read the
//! `is_home` column instead ([`is_marked_home_thread`]), so a hidden home
//! thread stays protected and comes back whole.
//!
//! **Only the user names it.** A rename by hand lands. Nothing titles it
//! automatically: no generated title and no suggestion, since one topic in a
//! thread that never ends is not its name.
//!
//! Reach widens for it and authority does not. `api::thread_reach` owns that
//! rule, and this module only answers which thread is home.

use uuid::Uuid;

use crate::core::prefs;
use crate::engine::event_bus::{BusEvent, EventBus};
use crate::engine::thread_events::{EventMeta, ThreadEvent};

/// What the home thread is called when it is created. The user may rename it,
/// and nothing else does.
pub const HOME_THREAD_TITLE: &str = "Home";

/// The home thread's id, or `None` while the switch is off or before one
/// exists.
pub async fn home_thread_id(pool: &sqlx::PgPool) -> Result<Option<Uuid>, sqlx::Error> {
    if !prefs::HOME_THREAD_ENABLED.try_read(pool).await? {
        return Ok(None);
    }
    marked_home_thread_id(pool).await
}

/// Is `thread_id` the home thread? A missing row is not, and nothing is while
/// the switch is off.
pub async fn is_home_thread(pool: &sqlx::PgPool, thread_id: Uuid) -> Result<bool, sqlx::Error> {
    Ok(home_thread_id(pool).await? == Some(thread_id))
}

/// Does `thread_id` carry the home marker, whatever the switch says? The
/// protections ask this, so they hold while the thread is hidden.
pub async fn is_marked_home_thread(
    pool: &sqlx::PgPool,
    thread_id: Uuid,
) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM thread_summaries WHERE thread_id = $1 AND is_home)",
    )
    .bind(thread_id)
    .fetch_one(pool)
    .await
}

/// The marked row, whatever the switch says. Only [`ensure_home_thread`] reads
/// it, so it never creates a second home while one is hidden.
async fn marked_home_thread_id(pool: &sqlx::PgPool) -> Result<Option<Uuid>, sqlx::Error> {
    sqlx::query_scalar("SELECT thread_id FROM thread_summaries WHERE is_home")
        .fetch_optional(pool)
        .await
}

/// Create the home thread unless the workspace already has one, and return its
/// id. Safe to call on every boot. It ignores the switch: callers decide.
///
/// Two callers racing both emit. The unique marker refuses the loser's row,
/// which rolls its event back, so the loser reads the winner's id instead.
pub async fn ensure_home_thread(
    bus: &EventBus,
    pool: &sqlx::PgPool,
) -> Result<Uuid, Box<dyn std::error::Error + Send + Sync>> {
    if let Some(id) = marked_home_thread_id(pool).await? {
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
    match (emitted, marked_home_thread_id(pool).await?) {
        (Ok(_), _) => {
            crate::log!("[HomeThread] Created the home thread {}", thread_id);
            Ok(thread_id)
        }
        (Err(_), Some(winner)) => Ok(winner),
        (Err(e), None) => Err(e),
    }
}

/// Create the home thread at boot, while the switch is on.
pub async fn ensure_home_thread_if_enabled(
    bus: &EventBus,
    pool: &sqlx::PgPool,
) -> Result<Option<Uuid>, Box<dyn std::error::Error + Send + Sync>> {
    if !prefs::HOME_THREAD_ENABLED.try_read(pool).await? {
        return Ok(None);
    }
    ensure_home_thread(bus, pool).await.map(Some)
}

/// Turn the switch on and create the home thread, for tests that need one.
#[cfg(test)]
pub(crate) async fn enabled_home_thread(bus: &EventBus, pool: &sqlx::PgPool) -> Uuid {
    crate::core::PreferenceStore::set(pool, bus, prefs::HOME_THREAD_ENABLED.key(), "true", None)
        .await
        .expect("turn the home thread switch on");
    ensure_home_thread(bus, pool)
        .await
        .expect("create the home thread")
}

#[cfg(test)]
#[path = "home_thread_tests.rs"]
mod tests;
