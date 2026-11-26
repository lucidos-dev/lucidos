use chrono::Duration;
use sqlx::PgPool;

use super::devices::NewIdRows;

/// INVARIANT: Rows older than this are considered stale and ignored by queries. The
/// frontend re-emits DeviceVisible every 30s while visible, so anything older
/// than 2 minutes means the browser stopped reporting (crash, network drop,
/// killed tab).
pub(crate) const PRESENCE_STALE_AFTER: Duration = Duration::seconds(120);

pub struct DevicePresenceStore;

impl DevicePresenceStore {
    pub async fn record_visible(
        pool: &PgPool,
        device_id: &str,
    ) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
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

    pub async fn candidates(
        pool: &PgPool,
    ) -> Result<Vec<String>, Box<dyn std::error::Error + Send + Sync>> {
        // INVARIANT: Compute the staleness cutoff with the DATABASE clock, not the
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

    /// INVARIANT: Drop a deleted device's presence row. Called only from the device
    /// store's delete cascade, inside its transaction. `device_id` carries no
    /// foreign key, so nothing else would clear it.
    ///
    /// No announcement is owed: `DeviceDeleted` covers the whole device.
    pub async fn delete_for_device(
        conn: &mut sqlx::PgConnection,
        device_id: &str,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        sqlx::query("DELETE FROM device_presence WHERE device_id = $1")
            .bind(device_id)
            .execute(conn)
            .await?;
        Ok(())
    }

    /// INVARIANT: Move a device's presence row to a new id. Called only from
    /// `DeviceStore::hand_over`.
    ///
    /// Takes a connection rather than the pool, because the hand-over is one
    /// transaction and a pool call would run outside it. `device_id` is the
    /// primary key, so one side's row must go first. `new_rows` says which.
    ///
    /// No announcement is owed. This table is a projection of DeviceVisible /
    /// DeviceHidden, and the move reports neither: the tab's visibility is
    /// unchanged, only the id naming it.
    pub async fn move_device(
        conn: &mut sqlx::PgConnection,
        old_id: &str,
        new_id: &str,
        new_rows: NewIdRows,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        match new_rows {
            NewIdRows::Orphans => {
                sqlx::query("DELETE FROM device_presence WHERE device_id = $1")
                    .bind(new_id)
                    .execute(&mut *conn)
                    .await?;
            }
            NewIdRows::Live => {
                sqlx::query(
                    "DELETE FROM device_presence WHERE device_id = $1
                     AND EXISTS (SELECT 1 FROM device_presence WHERE device_id = $2)",
                )
                .bind(old_id)
                .bind(new_id)
                .execute(&mut *conn)
                .await?;
            }
        }
        sqlx::query("UPDATE device_presence SET device_id = $2 WHERE device_id = $1")
            .bind(old_id)
            .bind(new_id)
            .execute(conn)
            .await?;
        Ok(())
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
