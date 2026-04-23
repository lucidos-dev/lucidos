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

    /// Register or update a device (upsert by id)
    pub async fn register(
        pool: &PgPool,
        id: &str,
        user_agent: Option<&str>,
    ) -> Result<Device, Box<dyn std::error::Error + Send + Sync>> {
        let device = sqlx::query_as::<_, Device>(
            "INSERT INTO devices (id, user_agent, last_seen_at)
             VALUES ($1, $2, NOW())
             ON CONFLICT (id) DO UPDATE SET
                user_agent = COALESCE($2, devices.user_agent),
                last_seen_at = NOW()
             RETURNING *",
        )
        .bind(id)
        .bind(user_agent)
        .fetch_one(pool)
        .await?;
        Ok(device)
    }

    /// Get the display name for a device (falls back to truncated ID if no name set).
    pub async fn display_name(pool: &PgPool, id: &str) -> Option<String> {
        let row: Option<(Option<String>,)> =
            sqlx::query_as("SELECT name FROM devices WHERE id = $1")
                .bind(id)
                .fetch_optional(pool)
                .await
                .ok()?;
        let (name,) = row?;
        Some(resolve_device_name(name.as_deref(), id))
    }

    /// Build a rich tooltip string for a device: name + user agent summary.
    pub async fn tooltip_info(pool: &PgPool, id: &str) -> Option<String> {
        let device: Option<Device> = sqlx::query_as("SELECT * FROM devices WHERE id = $1")
            .bind(id)
            .fetch_optional(pool)
            .await
            .ok()?;
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
}

/// Resolve a display name for a device: prefer the stored name, fall back to
/// `device-<short id>` derived from the first 8 chars of the device ID.
fn resolve_device_name(stored: Option<&str>, id: &str) -> String {
    if let Some(n) = stored {
        if !n.is_empty() {
            return n.to_string();
        }
    }
    let short = &id[..id.floor_char_boundary(8.min(id.len()))];
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
