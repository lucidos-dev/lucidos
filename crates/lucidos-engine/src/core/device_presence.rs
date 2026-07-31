//! Device presence tracking — which devices currently have a visible Lucidos tab.
//!
//! A device emits `DeviceVisible` when ANY top-level Lucidos tab becomes
//! visible, and `DeviceHidden` when the last visible tab goes away
//! (visibilitychange to 'hidden', window blur, beforeunload). This module
//! answers a single question: "which devices might be active right now?"
//! — the candidate list that the PresenceCheck protocol pings (see
//! `system-knowhow/notifications.md` §3).
//!
//! Used by the OS push fan-out: empty candidate list → push_allowed=true
//! directly (no point pinging). One or more candidates → run the
//! PresenceCheck and let pongs make the call.
//!
//! Underlying events are transient (broadcast over EventBus, never
//! persisted to the events table) and rows older than
//! [`PRESENCE_STALE_AFTER`] are ignored by queries.
//!
//! Heartbeats: the frontend re-emits `DeviceVisible` every 30s while
//! visible so a long-running session refreshes `visible_at` and rows
//! don't go stale.

use chrono::Duration;
use sqlx::PgPool;

/// Rows older than this are considered stale and ignored by queries. The
/// frontend re-emits DeviceVisible every 30s while visible, so anything older
/// than 2 minutes means the browser stopped reporting (crash, network drop,
/// killed tab).
pub(crate) const PRESENCE_STALE_AFTER: Duration = Duration::seconds(120);

pub struct DevicePresenceStore;

impl DevicePresenceStore {
    /// Record that a device's tab is now visible. Idempotent — also used as
    /// the heartbeat refresh by re-emitting from the frontend. Returns `true`
    /// when the row went from absent to present (real state change), `false`
    /// when this is a heartbeat refresh on an existing row.
    pub async fn record_visible(
        pool: &PgPool,
        device_id: &str,
    ) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
        // `xmax = 0` distinguishes the INSERT path of an ON CONFLICT DO UPDATE
        // from the UPDATE path: the system column holds the deleting xid (0
        // for a freshly inserted row, current xid for an UPDATE). Same idiom
        // as `devices::DeviceStore::register`. Single atomic query — no
        // SELECT-then-UPSERT race between the existence check and the upsert.
        let inserted: bool = sqlx::query_scalar::<_, bool>(
            "INSERT INTO device_presence (device_id, visible_at) \
             VALUES ($1, NOW()) \
             ON CONFLICT (device_id) DO UPDATE SET visible_at = NOW() \
             RETURNING (xmax = 0) AS inserted",
        )
        .bind(device_id)
        .fetch_one(pool)
        .await?;
        Ok(inserted)
    }

    /// Record that a device's tab is no longer visible. Returns `true` if a
    /// row was actually deleted, `false` if the unfocus was a no-op (already
    /// hidden).
    pub async fn record_hidden(
        pool: &PgPool,
        device_id: &str,
    ) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
        let result = sqlx::query("DELETE FROM device_presence WHERE device_id = $1")
            .bind(device_id)
            .execute(pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }

    /// Return device_ids that pinged visible within the last
    /// `PRESENCE_STALE_AFTER` window. Used by `send_push_to_all_with_app`
    /// to decide whether to RUN the PresenceCheck protocol — empty list
    /// → skip the ping and set `push_allowed = true` directly. See
    /// `system-knowhow/notifications.md` §3.
    pub async fn candidates(
        pool: &PgPool,
    ) -> Result<Vec<String>, Box<dyn std::error::Error + Send + Sync>> {
        // Compute the staleness cutoff with the DATABASE clock, not the
        // process clock: `record_visible` writes `visible_at = NOW()` (the DB's
        // clock), so the comparison must use that same clock. Computing the
        // cutoff from `chrono::Utc::now()` here would mix two clocks — a fresh
        // row would look stale (or vice-versa) whenever the engine host and
        // Postgres disagree on the time (Docker VM clock drift in tests; a
        // separate DB host with NTP skew in production). `$1` is the
        // `PRESENCE_STALE_AFTER` window in seconds, kept as the single source
        // of truth for the threshold.
        let stale_secs = PRESENCE_STALE_AFTER.num_seconds();
        let rows: Vec<(String,)> = sqlx::query_as(
            "SELECT device_id FROM device_presence \
             WHERE visible_at > NOW() - ($1::bigint * INTERVAL '1 second')",
        )
        .bind(stale_secs)
        .fetch_all(pool)
        .await?;
        Ok(rows.into_iter().map(|(d,)| d).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{setup_test_db, teardown_test_db};

    #[tokio::test]
    async fn record_visible_inserts_and_reports_state_change() {
        let (pool, db) = setup_test_db().await;
        let changed = DevicePresenceStore::record_visible(&pool, "dev-1")
            .await
            .unwrap();
        assert!(changed, "first record_visible must report a state change");
        assert!(!DevicePresenceStore::candidates(&pool)
            .await
            .unwrap()
            .is_empty());
        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn record_visible_again_is_a_heartbeat() {
        let (pool, db) = setup_test_db().await;
        DevicePresenceStore::record_visible(&pool, "dev-1")
            .await
            .unwrap();
        let changed = DevicePresenceStore::record_visible(&pool, "dev-1")
            .await
            .unwrap();
        assert!(
            !changed,
            "re-recording the same device is a heartbeat, not a state change"
        );
        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn record_hidden_removes_row() {
        let (pool, db) = setup_test_db().await;
        DevicePresenceStore::record_visible(&pool, "dev-1")
            .await
            .unwrap();
        let removed = DevicePresenceStore::record_hidden(&pool, "dev-1")
            .await
            .unwrap();
        assert!(removed, "hide on a visible row must report removal");
        assert!(DevicePresenceStore::candidates(&pool)
            .await
            .unwrap()
            .is_empty());
        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn candidates_empty_when_no_devices() {
        let (pool, db) = setup_test_db().await;
        assert!(DevicePresenceStore::candidates(&pool)
            .await
            .unwrap()
            .is_empty());
        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn candidates_lists_every_visible_device() {
        let (pool, db) = setup_test_db().await;
        DevicePresenceStore::record_visible(&pool, "dev-1")
            .await
            .unwrap();
        DevicePresenceStore::record_visible(&pool, "dev-2")
            .await
            .unwrap();
        assert!(!DevicePresenceStore::candidates(&pool)
            .await
            .unwrap()
            .is_empty());
        // Hide one; the other still keeps the candidate list non-empty.
        DevicePresenceStore::record_hidden(&pool, "dev-1")
            .await
            .unwrap();
        assert!(!DevicePresenceStore::candidates(&pool)
            .await
            .unwrap()
            .is_empty());
        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn stale_rows_are_excluded_from_candidates() {
        let (pool, db) = setup_test_db().await;
        // Insert a row past the staleness threshold.
        sqlx::query(
            "INSERT INTO device_presence (device_id, visible_at) \
             VALUES ($1, NOW() - INTERVAL '1 hour')",
        )
        .bind("dev-stale")
        .execute(&pool)
        .await
        .unwrap();
        assert!(
            DevicePresenceStore::candidates(&pool)
                .await
                .unwrap()
                .is_empty(),
            "stale rows must not count as visible"
        );
        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn record_visible_refreshes_visible_at() {
        let (pool, db) = setup_test_db().await;
        // Pre-insert a stale row.
        sqlx::query(
            "INSERT INTO device_presence (device_id, visible_at) \
             VALUES ($1, NOW() - INTERVAL '1 hour')",
        )
        .bind("dev-1")
        .execute(&pool)
        .await
        .unwrap();
        DevicePresenceStore::record_visible(&pool, "dev-1")
            .await
            .unwrap();
        assert!(
            !DevicePresenceStore::candidates(&pool)
                .await
                .unwrap()
                .is_empty(),
            "heartbeat should refresh visible_at so the row is no longer stale"
        );
        teardown_test_db(&db).await;
    }
}
