use serde::Serialize;
use sqlx::PgPool;

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct PinnedAppUi {
    pub app_id: String,
    pub ui_id: String,
}

pub struct PinnedAppStore;

impl PinnedAppStore {
    /// Defensive double-write — the migration owns this CREATE TABLE
    /// (see `20260517160627_consolidate_init_schema_tables.sql`). Slated
    /// for removal in `harden-init-schema-tables-vs-migrations-pattern-finish`.
    pub async fn init_schema(
        pool: &PgPool,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS pinned_apps (
                id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
                app_id TEXT NOT NULL,
                ui_id TEXT NOT NULL DEFAULT 'main',
                device_id TEXT NOT NULL,
                created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
                UNIQUE (app_id, ui_id, device_id)
            )",
        )
        .execute(pool)
        .await?;
        Ok(())
    }

    /// List pinned app UIs for a device, ordered by creation time
    pub async fn list_for_device(
        pool: &PgPool,
        device_id: &str,
    ) -> Result<Vec<PinnedAppUi>, Box<dyn std::error::Error + Send + Sync>> {
        let rows = sqlx::query_as::<_, PinnedAppUi>(
            "SELECT app_id, ui_id FROM pinned_apps WHERE device_id = $1 ORDER BY created_at ASC",
        )
        .bind(device_id)
        .fetch_all(pool)
        .await?;
        Ok(rows)
    }

    /// Pin an app UI for a device (idempotent — ignores if already pinned).
    /// Returns `true` if a new row was inserted, `false` if the
    /// `(app_id, ui_id, device_id)` triple already existed. Callers gate
    /// `PinnedAppPinned` audit emits on the bool so duplicate clicks don't
    /// flood the events table.
    pub async fn pin(
        pool: &PgPool,
        app_id: &str,
        ui_id: &str,
        device_id: &str,
    ) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
        let result = sqlx::query(
            "INSERT INTO pinned_apps (app_id, ui_id, device_id)
             VALUES ($1, $2, $3)
             ON CONFLICT (app_id, ui_id, device_id) DO NOTHING",
        )
        .bind(app_id)
        .bind(ui_id)
        .bind(device_id)
        .execute(pool)
        .await?;
        Ok(result.rows_affected() > 0)
    }

    /// Unpin an app UI for a device
    pub async fn unpin(
        pool: &PgPool,
        app_id: &str,
        ui_id: &str,
        device_id: &str,
    ) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
        let result = sqlx::query(
            "DELETE FROM pinned_apps WHERE app_id = $1 AND ui_id = $2 AND device_id = $3",
        )
        .bind(app_id)
        .bind(ui_id)
        .bind(device_id)
        .execute(pool)
        .await?;
        Ok(result.rows_affected() > 0)
    }

    /// Delete all pinned app UIs for a device (used when deleting a device)
    pub async fn delete_for_device(
        pool: &PgPool,
        device_id: &str,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        sqlx::query("DELETE FROM pinned_apps WHERE device_id = $1")
            .bind(device_id)
            .execute(pool)
            .await?;
        Ok(())
    }
}
