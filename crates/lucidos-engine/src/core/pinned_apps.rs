use serde::Serialize;
use sqlx::PgPool;

use crate::engine::event_bus::{BusEvent, EventBus, SystemEvent};
use crate::engine::thread_events::MessageOrigin;

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct PinnedAppUi {
    pub app_id: String,
    pub ui_id: String,
}

/// Per-device pinned app UIs.
///
/// **No caller can skip the event.** [`Self::pin`] and [`Self::unpin`] are the
/// only reachable single-row mutators, and they emit
/// `PinnedApp{Pinned,Unpinned}` themselves. Pins are device-scoped, so the
/// event is what lets a second tab on the SAME device (and the agent's own
/// pin/unpin tool) reflect the change instead of sitting stale.
///
/// [`Self::delete_for_device`] is the one deliberate exception: see its doc.
/// Same shape as `RepositoryStore`; see `core::announced_surfaces`.
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

    /// Insert a pin row (idempotent). **Private on purpose**: [`Self::pin`] is
    /// the reachable mutator, and it emits.
    async fn insert_row(
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

    /// Delete a pin row. **Private on purpose**: [`Self::unpin`] is the
    /// reachable mutator, and it emits.
    async fn delete_row(
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

    /// Pin an app UI for a device and announce it. Idempotent: a duplicate
    /// click inserts nothing and therefore announces nothing, so repeated taps
    /// cannot flood the events table.
    pub async fn pin(
        pool: &PgPool,
        event_bus: &EventBus,
        app_id: &str,
        ui_id: &str,
        device_id: &str,
        actor: Option<MessageOrigin>,
    ) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
        let pinned = Self::insert_row(pool, app_id, ui_id, device_id).await?;
        if pinned {
            event_bus
                .emit_or_log(
                    BusEvent::System(SystemEvent::PinnedAppPinned {
                        app_id: app_id.to_string(),
                        device_id: device_id.to_string(),
                        actor,
                    }),
                    "[PinnedApps] PinnedAppPinned",
                )
                .await;
        }
        Ok(pinned)
    }

    /// Unpin an app UI for a device and announce it. Announces only when a row
    /// was actually removed.
    pub async fn unpin(
        pool: &PgPool,
        event_bus: &EventBus,
        app_id: &str,
        ui_id: &str,
        device_id: &str,
        actor: Option<MessageOrigin>,
    ) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
        let unpinned = Self::delete_row(pool, app_id, ui_id, device_id).await?;
        if unpinned {
            event_bus
                .emit_or_log(
                    BusEvent::System(SystemEvent::PinnedAppUnpinned {
                        app_id: app_id.to_string(),
                        device_id: device_id.to_string(),
                        actor,
                    }),
                    "[PinnedApps] PinnedAppUnpinned",
                )
                .await;
        }
        Ok(unpinned)
    }

    /// Delete every pin for a device, silently. Called only from
    /// `DeviceStore::delete`, whose `DeviceDeleted` is the announcement: the
    /// device is gone, so an unpin event per app would describe changes to a
    /// device no client still tracks. Registered as the one `pinned_apps`
    /// exemption in `core::announced_surfaces`.
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

    /// Move every pin from one device id to another, silently. Called only from
    /// `DeviceStore::hand_over`, whose `DeviceHandedOver` is the announcement:
    /// the pins are unchanged, only the id naming them is, so a pin event per
    /// app would report changes nobody made. Registered as a `pinned_apps`
    /// exemption in `core::announced_surfaces`.
    ///
    /// Takes a connection rather than the pool, because the hand-over is one
    /// transaction and a pool call would run outside it. Rows already under
    /// `new_id` belong to a device that does not exist, so they are cleared
    /// first: `UNIQUE (app_id, ui_id, device_id)` would otherwise abort the move.
    pub async fn move_device(
        conn: &mut sqlx::PgConnection,
        old_id: &str,
        new_id: &str,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        sqlx::query("DELETE FROM pinned_apps WHERE device_id = $1")
            .bind(new_id)
            .execute(&mut *conn)
            .await?;
        sqlx::query("UPDATE pinned_apps SET device_id = $2 WHERE device_id = $1")
            .bind(old_id)
            .bind(new_id)
            .execute(conn)
            .await?;
        Ok(())
    }
}
