use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;

use super::PinnedAppStore;

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct Device {
    pub id: String,
    pub name: Option<String>,
    pub user_agent: Option<String>,
    pub push_enabled: bool,
    pub last_seen_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
}

pub struct DeviceStore;

impl DeviceStore {
    /// Defensive double-write — the migration owns this CREATE TABLE
    /// (see `20260517160627_consolidate_init_schema_tables.sql`). Slated
    /// for removal in `harden-init-schema-tables-vs-migrations-pattern-finish`.
    pub async fn init_schema(
        pool: &PgPool,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS devices (
                id TEXT PRIMARY KEY,
                name TEXT,
                user_agent TEXT,
                push_enabled BOOLEAN NOT NULL DEFAULT false,
                last_seen_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
                created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
            )",
        )
        .execute(pool)
        .await?;
        Ok(())
    }

    /// Register or update a device (upsert by id). Returns `(device, inserted)`
    /// where `inserted` is true iff a new row was created (false on
    /// last-seen-at refresh). Callers stamp `DeviceRegistered` audit events
    /// only when `inserted` is true so a page-load refresh doesn't append a
    /// row to the events table on every navigation.
    ///
    /// `xmax = 0` on PostgreSQL is the standard idiom for "INSERT path of an
    /// ON CONFLICT DO UPDATE" — the system column holds the deleting
    /// transaction id, which is 0 for a freshly inserted row and the
    /// current xid for an UPDATE.
    pub async fn register(
        pool: &PgPool,
        id: &str,
        user_agent: Option<&str>,
    ) -> Result<(Device, bool), Box<dyn std::error::Error + Send + Sync>> {
        #[derive(sqlx::FromRow)]
        struct DeviceWithInsertFlag {
            #[sqlx(flatten)]
            device: Device,
            inserted: bool,
        }

        let row: DeviceWithInsertFlag = sqlx::query_as(
            "INSERT INTO devices (id, user_agent, last_seen_at)
             VALUES ($1, $2, NOW())
             ON CONFLICT (id) DO UPDATE SET
                user_agent = COALESCE($2, devices.user_agent),
                last_seen_at = NOW()
             RETURNING id, name, user_agent, push_enabled, last_seen_at, created_at, (xmax = 0) AS inserted",
        )
        .bind(id)
        .bind(user_agent)
        .fetch_one(pool)
        .await?;
        Ok((row.device, row.inserted))
    }

    /// Get the display name for a device (falls back to truncated ID if no name set).
    /// DB errors are logged and treated as "device not found" — caller falls back to None.
    pub async fn display_name(pool: &PgPool, id: &str) -> Option<String> {
        let row: Option<(Option<String>,)> =
            match sqlx::query_as("SELECT name FROM devices WHERE id = $1")
                .bind(id)
                .fetch_optional(pool)
                .await
            {
                Ok(r) => r,
                Err(e) => {
                    log!("[Devices] display_name({}) failed: {}", id, e);
                    return None;
                }
            };
        let (name,) = row?;
        Some(resolve_device_name(name.as_deref(), id))
    }

    /// Build a rich tooltip string for a device: name + user agent summary.
    /// DB errors are logged and treated as "device not found" — caller falls back to None.
    pub async fn tooltip_info(pool: &PgPool, id: &str) -> Option<String> {
        let device: Option<Device> = match sqlx::query_as("SELECT * FROM devices WHERE id = $1")
            .bind(id)
            .fetch_optional(pool)
            .await
        {
            Ok(d) => d,
            Err(e) => {
                log!("[Devices] tooltip_info({}) failed: {}", id, e);
                return None;
            }
        };
        let device = device?;
        let name = resolve_device_name(device.name.as_deref(), id);
        let ua = device.user_agent.as_deref().map(parse_user_agent);
        match ua {
            Some(parsed) => Some(format!("{}\n{}", name, parsed)),
            None => Some(name),
        }
    }

    /// List all devices ordered by last_seen_at descending
    pub async fn list(
        pool: &PgPool,
    ) -> Result<Vec<Device>, Box<dyn std::error::Error + Send + Sync>> {
        let devices =
            sqlx::query_as::<_, Device>("SELECT * FROM devices ORDER BY last_seen_at DESC")
                .fetch_all(pool)
                .await?;
        Ok(devices)
    }

    /// Rename a device
    pub async fn rename(
        pool: &PgPool,
        id: &str,
        name: Option<&str>,
    ) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
        let result = sqlx::query("UPDATE devices SET name = $2 WHERE id = $1")
            .bind(id)
            .bind(name)
            .execute(pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }

    /// Delete a device and its per-device preferences
    pub async fn delete(
        pool: &PgPool,
        id: &str,
    ) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
        // Delete per-device preferences first
        sqlx::query("DELETE FROM preferences WHERE device_id = $1")
            .bind(id)
            .execute(pool)
            .await?;
        // Delete push subscriptions for this device
        sqlx::query("DELETE FROM push_subscriptions WHERE device_id = $1")
            .bind(id)
            .execute(pool)
            .await?;
        // Delete pinned App UIs for this device
        PinnedAppStore::delete_for_device(pool, id).await?;
        let result = sqlx::query("DELETE FROM devices WHERE id = $1")
            .bind(id)
            .execute(pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }

    /// Set push_enabled for a device
    pub async fn set_push_enabled(
        pool: &PgPool,
        id: &str,
        enabled: bool,
    ) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
        let result = sqlx::query("UPDATE devices SET push_enabled = $2 WHERE id = $1")
            .bind(id)
            .bind(enabled)
            .execute(pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }

    /// List IDs of currently push-enabled devices whose `last_seen_at` is
    /// older than `cutoff_days` days. Used by the daily prune to flip them
    /// to `push_enabled = false`, stopping push fan-out to phantom
    /// subscriptions (typically PWA reinstalls whose Apple/Google endpoint
    /// hasn't 410'd yet). Filtered to push-enabled at the SELECT layer so
    /// the caller never emits a no-op `DevicePushChanged` for rows already
    /// disabled.
    pub async fn list_stale_push_enabled(
        pool: &PgPool,
        cutoff_days: i64,
    ) -> Result<Vec<String>, Box<dyn std::error::Error + Send + Sync>> {
        let rows: Vec<(String,)> = sqlx::query_as(
            "SELECT id FROM devices
             WHERE push_enabled = true
               AND last_seen_at < NOW() - make_interval(days => $1::int)
             ORDER BY last_seen_at ASC",
        )
        .bind(cutoff_days)
        .fetch_all(pool)
        .await?;
        Ok(rows.into_iter().map(|(id,)| id).collect())
    }
}

/// Resolve a display name for a device: prefer the stored name, fall back to
/// `device-<short id>` derived from the first 8 chars of the device ID.
pub(crate) fn resolve_device_name(stored: Option<&str>, id: &str) -> String {
    if let Some(n) = stored {
        if !n.is_empty() {
            return n.to_string();
        }
    }
    let short = &id[..id.floor_char_boundary(8)];
    format!("device-{}", short)
}

/// Parse a raw user-agent string into a short "Browser/Version on OS" summary.
fn parse_user_agent(ua: &str) -> String {
    let browser = ["Chrome", "Firefox", "Safari", "Edge", "Opera"]
        .iter()
        .find_map(|name| {
            ua.find(name).map(|start| {
                let rest = &ua[start..];
                let end = rest.find(|c: char| c.is_whitespace()).unwrap_or(rest.len());
                let token = &rest[..end];
                if token.contains('/') {
                    token.to_string()
                } else {
                    (*name).to_string()
                }
            })
        })
        .unwrap_or_else(|| "Unknown browser".to_string());

    let os = if ua.contains("iPhone") {
        "iOS"
    } else if ua.contains("iPad") {
        "iPadOS"
    } else if ua.contains("Mac") {
        "macOS"
    } else if ua.contains("Android") {
        "Android"
    } else if ua.contains("Windows") {
        "Windows"
    } else if ua.contains("Linux") {
        "Linux"
    } else {
        "Unknown OS"
    };

    format!("{} on {}", browser, os)
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn backdate_last_seen(pool: &PgPool, id: &str, days_ago: i64) {
        sqlx::query("UPDATE devices SET last_seen_at = NOW() - make_interval(days => $1::int) WHERE id = $2")
            .bind(days_ago)
            .bind(id)
            .execute(pool)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn list_stale_push_enabled_filters_by_age_and_push_state() {
        let (pool, db_name) = crate::test_support::setup_test_db().await;

        // Push-enabled + old → returned
        DeviceStore::register(&pool, "old-on", Some("UA")).await.unwrap();
        DeviceStore::set_push_enabled(&pool, "old-on", true).await.unwrap();
        backdate_last_seen(&pool, "old-on", 45).await;

        // Push-enabled + recent → excluded (last_seen is today)
        DeviceStore::register(&pool, "fresh-on", Some("UA")).await.unwrap();
        DeviceStore::set_push_enabled(&pool, "fresh-on", true).await.unwrap();

        // Push-disabled + old → excluded (filtered at SELECT to avoid no-op events)
        DeviceStore::register(&pool, "old-off", Some("UA")).await.unwrap();
        backdate_last_seen(&pool, "old-off", 45).await;

        // Right on the cutoff (29 days) → excluded
        DeviceStore::register(&pool, "almost-on", Some("UA")).await.unwrap();
        DeviceStore::set_push_enabled(&pool, "almost-on", true).await.unwrap();
        backdate_last_seen(&pool, "almost-on", 29).await;

        let stale = DeviceStore::list_stale_push_enabled(&pool, 30).await.unwrap();
        assert_eq!(stale, vec!["old-on".to_string()]);

        crate::test_support::teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn list_stale_push_enabled_returns_empty_when_nothing_stale() {
        let (pool, db_name) = crate::test_support::setup_test_db().await;
        DeviceStore::register(&pool, "fresh", Some("UA")).await.unwrap();
        DeviceStore::set_push_enabled(&pool, "fresh", true).await.unwrap();

        let stale = DeviceStore::list_stale_push_enabled(&pool, 30).await.unwrap();
        assert!(stale.is_empty());

        crate::test_support::teardown_test_db(&db_name).await;
    }
}
