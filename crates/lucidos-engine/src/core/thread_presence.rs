//! Thread presence tracking — which device is focused on which thread.
//!
//! A device emits ThreadFocused when the user starts viewing a thread (frontend
//! focusedThreadId becomes non-null AND the page is visible). It emits
//! ThreadUnfocused when the page is hidden, the window blurs, or the user
//! switches to a different thread.
//!
//! Presence is high-churn and not interesting to replay, so the underlying
//! events are transient (broadcast over EventBus, never persisted to the
//! events table). The truth lives in this projection.
//!
//! Heartbeats keep `focused_at` fresh; queries treat rows older than
//! [`PRESENCE_STALE_AFTER`] as stale (browser likely died).

use chrono::Duration;
use sqlx::PgPool;
use uuid::Uuid;

/// Rows older than this are considered stale and ignored by queries.
/// The frontend re-emits ThreadFocused every 30s while focused, so anything
/// older than 2 minutes means the browser stopped reporting (crash, network
/// drop, killed tab).
pub const PRESENCE_STALE_AFTER: Duration = Duration::seconds(120);

pub struct ThreadPresenceStore;

impl ThreadPresenceStore {
    /// Record that a device is now focused on a thread. Idempotent — also
    /// used as the heartbeat refresh by re-emitting from the frontend.
    /// Returns `true` when this is a real state change (new device or a
    /// different thread than before) and `false` for a heartbeat refresh.
    /// Callers use the return to decide whether to broadcast on EventBus.
    pub async fn record_focused(
        pool: &PgPool,
        device_id: &str,
        thread_id: Uuid,
    ) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
        let previous: Option<Uuid> =
            sqlx::query_scalar("SELECT thread_id FROM thread_presence WHERE device_id = $1")
                .bind(device_id)
                .fetch_optional(pool)
                .await?;

        sqlx::query(
            "INSERT INTO thread_presence (device_id, thread_id, focused_at) \
             VALUES ($1, $2, NOW()) \
             ON CONFLICT (device_id) DO UPDATE SET \
                 thread_id = EXCLUDED.thread_id, \
                 focused_at = NOW()",
        )
        .bind(device_id)
        .bind(thread_id)
        .execute(pool)
        .await?;
        Ok(previous != Some(thread_id))
    }

    /// Record that a device is no longer focused on a thread. Only deletes
    /// the row if the stored thread_id matches — prevents a stale unfocus
    /// from clobbering a more recent focus on a different thread.
    /// Returns `true` if a row was actually deleted, `false` if the unfocus
    /// was a no-op (already unfocused or focused elsewhere).
    pub async fn record_unfocused(
        pool: &PgPool,
        device_id: &str,
        thread_id: Uuid,
    ) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
        let result =
            sqlx::query("DELETE FROM thread_presence WHERE device_id = $1 AND thread_id = $2")
                .bind(device_id)
                .bind(thread_id)
                .execute(pool)
                .await?;
        Ok(result.rows_affected() > 0)
    }

    /// Return device_ids currently focused on the given thread (excluding stale rows).
    pub async fn devices_focused_on(
        pool: &PgPool,
        thread_id: Uuid,
    ) -> Result<Vec<String>, Box<dyn std::error::Error + Send + Sync>> {
        let cutoff = chrono::Utc::now() - PRESENCE_STALE_AFTER;
        let rows: Vec<(String,)> = sqlx::query_as(
            "SELECT device_id FROM thread_presence \
             WHERE thread_id = $1 AND focused_at > $2",
        )
        .bind(thread_id)
        .bind(cutoff)
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
    async fn record_focused_inserts_row() {
        let (pool, db) = setup_test_db().await;
        let thread = Uuid::new_v4();
        let changed = ThreadPresenceStore::record_focused(&pool, "dev-1", thread)
            .await
            .unwrap();
        assert!(changed, "first focus must report a state change");
        let devices = ThreadPresenceStore::devices_focused_on(&pool, thread)
            .await
            .unwrap();
        assert_eq!(devices, vec!["dev-1".to_string()]);
        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn record_focused_replaces_previous_thread_for_same_device() {
        let (pool, db) = setup_test_db().await;
        let thread_a = Uuid::new_v4();
        let thread_b = Uuid::new_v4();
        ThreadPresenceStore::record_focused(&pool, "dev-1", thread_a)
            .await
            .unwrap();
        let changed = ThreadPresenceStore::record_focused(&pool, "dev-1", thread_b)
            .await
            .unwrap();
        assert!(changed, "switching threads must report a state change");
        let on_a = ThreadPresenceStore::devices_focused_on(&pool, thread_a)
            .await
            .unwrap();
        let on_b = ThreadPresenceStore::devices_focused_on(&pool, thread_b)
            .await
            .unwrap();
        assert!(on_a.is_empty(), "previous focus should be replaced");
        assert_eq!(on_b, vec!["dev-1".to_string()]);
        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn record_focused_for_same_thread_is_a_heartbeat() {
        let (pool, db) = setup_test_db().await;
        let thread = Uuid::new_v4();
        ThreadPresenceStore::record_focused(&pool, "dev-1", thread)
            .await
            .unwrap();
        let changed = ThreadPresenceStore::record_focused(&pool, "dev-1", thread)
            .await
            .unwrap();
        assert!(
            !changed,
            "re-focusing the same thread is a heartbeat, not a state change"
        );
        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn devices_focused_on_returns_all_focused_devices() {
        let (pool, db) = setup_test_db().await;
        let thread = Uuid::new_v4();
        ThreadPresenceStore::record_focused(&pool, "dev-1", thread)
            .await
            .unwrap();
        ThreadPresenceStore::record_focused(&pool, "dev-2", thread)
            .await
            .unwrap();
        let mut devices = ThreadPresenceStore::devices_focused_on(&pool, thread)
            .await
            .unwrap();
        devices.sort();
        assert_eq!(devices, vec!["dev-1".to_string(), "dev-2".to_string()]);
        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn record_unfocused_removes_row() {
        let (pool, db) = setup_test_db().await;
        let thread = Uuid::new_v4();
        ThreadPresenceStore::record_focused(&pool, "dev-1", thread)
            .await
            .unwrap();
        let removed = ThreadPresenceStore::record_unfocused(&pool, "dev-1", thread)
            .await
            .unwrap();
        assert!(removed, "unfocus on a focused row must report removal");
        let devices = ThreadPresenceStore::devices_focused_on(&pool, thread)
            .await
            .unwrap();
        assert!(devices.is_empty());
        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn record_unfocused_for_different_thread_is_noop() {
        let (pool, db) = setup_test_db().await;
        let current = Uuid::new_v4();
        let stale = Uuid::new_v4();
        ThreadPresenceStore::record_focused(&pool, "dev-1", current)
            .await
            .unwrap();
        // A late-arriving Unfocused for the previous thread must NOT remove the
        // newer focus on `current`.
        let removed = ThreadPresenceStore::record_unfocused(&pool, "dev-1", stale)
            .await
            .unwrap();
        assert!(
            !removed,
            "unfocus on a non-matching thread must report no-op"
        );
        let devices = ThreadPresenceStore::devices_focused_on(&pool, current)
            .await
            .unwrap();
        assert_eq!(devices, vec!["dev-1".to_string()]);
        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn stale_rows_are_excluded_from_query() {
        let (pool, db) = setup_test_db().await;
        let thread = Uuid::new_v4();
        // Insert a row with focused_at well past the staleness threshold.
        sqlx::query(
            "INSERT INTO thread_presence (device_id, thread_id, focused_at) \
             VALUES ($1, $2, NOW() - INTERVAL '1 hour')",
        )
        .bind("dev-stale")
        .bind(thread)
        .execute(&pool)
        .await
        .unwrap();
        // And a fresh row.
        ThreadPresenceStore::record_focused(&pool, "dev-fresh", thread)
            .await
            .unwrap();
        let devices = ThreadPresenceStore::devices_focused_on(&pool, thread)
            .await
            .unwrap();
        assert_eq!(
            devices,
            vec!["dev-fresh".to_string()],
            "stale rows must not be returned"
        );
        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn record_focused_refreshes_focused_at() {
        let (pool, db) = setup_test_db().await;
        let thread = Uuid::new_v4();
        // Pre-insert a stale row.
        sqlx::query(
            "INSERT INTO thread_presence (device_id, thread_id, focused_at) \
             VALUES ($1, $2, NOW() - INTERVAL '1 hour')",
        )
        .bind("dev-1")
        .bind(thread)
        .execute(&pool)
        .await
        .unwrap();
        // Heartbeat: re-record same device + thread.
        ThreadPresenceStore::record_focused(&pool, "dev-1", thread)
            .await
            .unwrap();
        let devices = ThreadPresenceStore::devices_focused_on(&pool, thread)
            .await
            .unwrap();
        assert_eq!(
            devices,
            vec!["dev-1".to_string()],
            "heartbeat should refresh focused_at so the row is no longer stale"
        );
        teardown_test_db(&db).await;
    }
}
