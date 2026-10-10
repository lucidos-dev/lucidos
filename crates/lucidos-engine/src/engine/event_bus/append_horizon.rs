//! The *in-flight append lock*, and the *committed horizon* it makes readable
//! (ADR 0364).
//!
//! An append draws its `sequence` at the INSERT and becomes visible at commit,
//! so a higher sequence can commit first. `MAX(sequence)` therefore promises
//! nothing about the rows below it. A reader keeping one global mark needs a
//! sequence at or below which nothing is still in flight.

use std::time::{Duration, Instant};

use sqlx::{PgPool, Postgres, Transaction};

use super::EventBus;

/// Advisory-lock class of the in-flight append lock. The two-key form keeps it
/// apart from the one-key locks elsewhere in the engine.
const APPEND_IN_FLIGHT_CLASS: i32 = 0x696e_666c;

/// How long a horizon read waits, in total, for the appends in flight when it
/// began. Each takes milliseconds, so running out means one of them is stuck.
const HORIZON_WAIT_TIMEOUT_MS: u64 = 10_000;

impl EventBus {
    /// Begin a transaction that appends an events row, holding its in-flight
    /// append lock until the transaction ends.
    ///
    /// **The lock must be the transaction's first lock.** A waiter then holds
    /// nothing, so it cannot close a deadlock cycle. Being first also puts it
    /// ahead of the INSERT that draws the sequence, which the horizon read needs.
    ///
    /// The key is this backend's pid, and a backend runs one transaction at a
    /// time, so appends never contend on it. Only a horizon read waits on it.
    pub(super) async fn begin_append(&self) -> Result<Transaction<'static, Postgres>, sqlx::Error> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("SELECT pg_advisory_xact_lock($1, pg_backend_pid())")
            .bind(APPEND_IN_FLIGHT_CLASS)
            .execute(&mut *tx)
            .await?;
        Ok(tx)
    }
}

/// The event store's **committed horizon**: a sequence at or below which every
/// events row has committed, or never will. An event-wait watermark is one.
///
/// It reads `MAX(sequence)`, then waits out every append in flight at that
/// moment. An uncommitted row at or below the maximum drew its sequence
/// earlier. Its append was therefore already in flight and holding its lock.
/// Any append that starts later draws a higher sequence.
///
/// Only the caller waits. An append queues behind a read only while the read
/// holds that backend's key, which is for the instant between grant and release.
pub(crate) async fn committed_event_horizon(pool: &PgPool) -> Result<i64, sqlx::Error> {
    committed_event_horizon_within(pool, HORIZON_WAIT_TIMEOUT_MS).await
}

/// [`committed_event_horizon`] with the wait bounded by `timeout_ms`. Running
/// out is an error, never a guess at a horizon.
pub(super) async fn committed_event_horizon_within(
    pool: &PgPool,
    timeout_ms: u64,
) -> Result<i64, sqlx::Error> {
    // Outside the waiting transaction, so that transaction holds no lock while
    // it waits.
    let horizon: i64 = sqlx::query_scalar("SELECT COALESCE(MAX(sequence), 0) FROM events")
        .fetch_one(pool)
        .await?;
    // After the maximum, never before: an append holding its lock at that
    // read must be in this list.
    let in_flight: Vec<i32> = sqlx::query_scalar(
        "SELECT objid::integer FROM pg_locks \
         WHERE locktype = 'advisory' AND granted AND objsubid = 2 \
           AND classid = $1::oid \
           AND database = (SELECT oid FROM pg_database WHERE datname = current_database())",
    )
    .bind(APPEND_IN_FLIGHT_CLASS)
    .fetch_all(pool)
    .await?;
    if in_flight.is_empty() {
        return Ok(horizon);
    }

    // `lock_timeout` bounds one statement, so each key gets only what is left
    // of one deadline. At least 1 ms, because zero would disable the timeout.
    // The checkout shares that deadline too. Dropping a checkout is safe,
    // where dropping a lock statement could strand its session lock.
    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    let checkout_left = deadline.saturating_duration_since(Instant::now());
    let mut tx = tokio::time::timeout(checkout_left, pool.begin())
        .await
        .map_err(|_| sqlx::Error::PoolTimedOut)??;
    for key in in_flight {
        let left_ms = deadline
            .saturating_duration_since(Instant::now())
            .as_millis()
            .max(1);
        sqlx::query("SELECT set_config('lock_timeout', $1, true)")
            .bind(format!("{left_ms}ms"))
            .execute(&mut *tx)
            .await?;
        // Released the moment it is granted, so the read never holds one key
        // while waiting on another. A held key would stall that backend's next
        // append, and a cycle through application code could then form.
        sqlx::query("SELECT pg_advisory_lock_shared($1, $2), pg_advisory_unlock_shared($1, $2)")
            .bind(APPEND_IN_FLIGHT_CLASS)
            .bind(key)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(horizon)
}
