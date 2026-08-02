use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;

use super::PinnedAppStore;
use crate::engine::event_bus::{BusEvent, EventBus, SystemEvent};
use crate::engine::thread_events::MessageOrigin;

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct Device {
    pub id: String,
    pub name: Option<String>,
    pub user_agent: Option<String>,
    pub push_enabled: bool,
    pub last_seen_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
}

/// The registry of devices that have connected to this workspace.
///
/// **No caller can skip the event.** [`Self::register`], [`Self::rename`],
/// [`Self::set_push_enabled`] and [`Self::delete`] are the only reachable
/// mutators; the raw row writes are private to this module.
/// `Device{Registered,Renamed,PushChanged,Deleted}` is what reloads the
/// Settings devices list on every other device.
///
/// Same shape as `RepositoryStore`; see `core::announced_surfaces`.
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
    /// last-seen-at refresh).
    ///
    /// **Private on purpose**: [`Self::register`] is the reachable mutator, and
    /// it emits `DeviceRegistered` only when `inserted` is true, so a page-load
    /// refresh does not append a row to the events table on every navigation.
    ///
    /// `xmax = 0` on PostgreSQL is the standard idiom for "INSERT path of an
    /// ON CONFLICT DO UPDATE" — the system column holds the deleting
    /// transaction id, which is 0 for a freshly inserted row and the
    /// current xid for an UPDATE.
    async fn upsert_row(
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

    /// Rename a device row. **Private on purpose**: [`Self::rename`] emits.
    async fn rename_row(
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

    /// Delete a device and everything scoped to it. **Private on purpose**:
    /// [`Self::delete`] emits.
    ///
    /// The cascade (per-device preferences, push subscriptions, pinned apps) is
    /// deliberately silent. `DeviceDeleted` is the announcement for all of it:
    /// the device is gone, so a `PreferencesChanged` or `PinnedAppUnpinned` per
    /// row would describe changes to a device no client still tracks.
    async fn delete_row(
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

    /// Set push_enabled on a device row. **Private on purpose**:
    /// [`Self::set_push_enabled`] emits.
    ///
    /// Returns `None` when no such device exists, `Some(changed)` otherwise.
    /// `rows_affected` cannot answer "changed": Postgres writes a new tuple
    /// version even when the value is identical. The self-join reads the
    /// pre-update value in the same statement.
    async fn set_push_enabled_row(
        pool: &PgPool,
        id: &str,
        enabled: bool,
    ) -> Result<Option<bool>, Box<dyn std::error::Error + Send + Sync>> {
        let changed: Option<bool> = sqlx::query_scalar(
            "UPDATE devices AS d SET push_enabled = $2 \
             FROM (SELECT id, push_enabled FROM devices WHERE id = $1) AS prior \
             WHERE d.id = prior.id \
             RETURNING (prior.push_enabled IS DISTINCT FROM $2)",
        )
        .bind(id)
        .bind(enabled)
        .fetch_optional(pool)
        .await?;
        Ok(changed)
    }

    /// Register a device and announce it. The only way to add one.
    ///
    /// `DeviceRegistered` fires only on a genuinely new device, never on the
    /// last-seen-at refresh every page load performs.
    pub async fn register(
        pool: &PgPool,
        event_bus: &EventBus,
        id: &str,
        user_agent: Option<&str>,
        actor: Option<MessageOrigin>,
    ) -> Result<(Device, bool), Box<dyn std::error::Error + Send + Sync>> {
        let (device, inserted) = Self::upsert_row(pool, id, user_agent).await?;
        if inserted {
            event_bus
                .emit_or_log(
                    BusEvent::System(SystemEvent::DeviceRegistered {
                        device_id: device.id.clone(),
                        user_agent: device.user_agent.clone(),
                        actor,
                    }),
                    "[Devices] DeviceRegistered",
                )
                .await;
        }
        Ok((device, inserted))
    }

    /// Rename a device and announce it. Announces only when a row existed.
    pub async fn rename(
        pool: &PgPool,
        event_bus: &EventBus,
        id: &str,
        name: Option<&str>,
        actor: Option<MessageOrigin>,
    ) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
        let renamed = Self::rename_row(pool, id, name).await?;
        if renamed {
            event_bus
                .emit_or_log(
                    BusEvent::System(SystemEvent::DeviceRenamed {
                        device_id: id.to_string(),
                        name: name.map(str::to_string),
                        actor,
                    }),
                    "[Devices] DeviceRenamed",
                )
                .await;
        }
        Ok(renamed)
    }

    /// Flip a device's push flag and announce it.
    ///
    /// Returns whether the device exists (the HTTP handler reports "Device not
    /// found" on `false`), but announces only when the flag actually MOVED.
    /// The stale-device prune already avoided no-op announcements by filtering
    /// `push_enabled = true` at its SELECT; enforcing it in the write path
    /// covers the HTTP handler too, which has no such filter.
    pub async fn set_push_enabled(
        pool: &PgPool,
        event_bus: &EventBus,
        id: &str,
        enabled: bool,
        actor: Option<MessageOrigin>,
    ) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
        let outcome = Self::set_push_enabled_row(pool, id, enabled).await?;
        if outcome == Some(true) {
            event_bus
                .emit_or_log(
                    BusEvent::System(SystemEvent::DevicePushChanged {
                        device_id: id.to_string(),
                        push_enabled: enabled,
                        actor,
                    }),
                    "[Devices] DevicePushChanged",
                )
                .await;
        }
        Ok(outcome.is_some())
    }

    /// Delete a device and announce it. The only way to remove one; the
    /// per-device cascade rides along under this single event (see
    /// [`Self::delete_row`]).
    pub async fn delete(
        pool: &PgPool,
        event_bus: &EventBus,
        id: &str,
        actor: Option<MessageOrigin>,
    ) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
        let removed = Self::delete_row(pool, id).await?;
        if removed {
            event_bus
                .emit_or_log(
                    BusEvent::System(SystemEvent::DeviceDeleted {
                        device_id: id.to_string(),
                        actor,
                    }),
                    "[Devices] DeviceDeleted",
                )
                .await;
        }
        Ok(removed)
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

/// Product token the Tauri native desktop client appends to its registered
/// user-agent string (see `registrationUserAgent` in
/// `crates/lucidos-app/src/utils/platform.ts`). The WKWebView's real UA is
/// indistinguishable from Safari, so this token is the only signal that lets the
/// agent's device context tell the desktop app from a browser — keep the literal
/// in sync with the frontend constant.
const DESKTOP_APP_UA_TOKEN: &str = "Lucidos-Desktop";

/// Parse a raw user-agent string into a short "Browser/Version on OS" summary.
/// A user-agent carrying the [`DESKTOP_APP_UA_TOKEN`] is rendered as the Lucidos
/// native desktop app instead of a browser, so the agent gives native-OS (not
/// browser-permission) notification advice in the desktop client.
fn parse_user_agent(ua: &str) -> String {
    let os = parse_os(ua);

    if ua.contains(DESKTOP_APP_UA_TOKEN) {
        return format!("Lucidos desktop app on {}", os);
    }

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

    format!("{} on {}", browser, os)
}

/// Map a raw user-agent to a short OS label.
fn parse_os(ua: &str) -> &'static str {
    if ua.contains("iPhone") {
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
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_user_agent_renders_desktop_token_as_lucidos_desktop_app() {
        // The Tauri client registers a Safari-like UA with the desktop-app token
        // appended; it must read as the desktop app, NOT Safari, so the agent
        // gives native-OS notification advice instead of browser-permission advice.
        let ua = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 \
                  (KHTML, like Gecko) Version/18.0 Safari/605.1.15 Lucidos-Desktop";
        assert_eq!(parse_user_agent(ua), "Lucidos desktop app on macOS");
    }

    #[test]
    fn parse_user_agent_leaves_browsers_unchanged() {
        // A real browser / PWA UA (no token) keeps the "Browser/Version on OS" shape.
        assert_eq!(
            parse_user_agent(
                "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 \
                 (KHTML, like Gecko) Chrome/149.0.0.0 Safari/537.36"
            ),
            "Chrome/149.0.0.0 on macOS"
        );
        assert_eq!(
            parse_user_agent(
                "Mozilla/5.0 (iPhone; CPU iPhone OS 18_0 like Mac OS X) \
                 AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.0 Mobile/15E148 Safari/604.1"
            ),
            "Safari/604.1 on iOS"
        );
    }

    async fn backdate_last_seen(pool: &PgPool, id: &str, days_ago: i64) {
        sqlx::query("UPDATE devices SET last_seen_at = NOW() - make_interval(days => $1::int) WHERE id = $2")
            .bind(days_ago)
            .bind(id)
            .execute(pool)
            .await
            .unwrap();
    }

    /// The load-bearing guarantee: a device write and its announcement are one
    /// operation, so the Settings devices list on every OTHER device reloads.
    /// The one write that must stay silent is the last-seen-at refresh the
    /// frontend performs on every page load: announcing it would append an
    /// events row per navigation.
    #[tokio::test]
    async fn register_announces_a_new_device_but_not_a_last_seen_refresh() {
        let (pool, db_name) = crate::test_support::setup_test_db().await;
        let (bus, _callback_rx) = EventBus::new(pool.clone());
        async fn emitted(pool: &PgPool, event_type: &str) -> i64 {
            sqlx::query_scalar("SELECT count(*) FROM events WHERE event_type = $1")
                .bind(event_type)
                .fetch_one(pool)
                .await
                .unwrap()
        }

        let (_, inserted) = DeviceStore::register(&pool, &bus, "d1", Some("UA"), None)
            .await
            .unwrap();
        assert!(inserted);
        assert_eq!(emitted(&pool, "DeviceRegistered").await, 1);

        let (_, inserted) = DeviceStore::register(&pool, &bus, "d1", Some("UA"), None)
            .await
            .unwrap();
        assert!(!inserted);
        assert_eq!(
            emitted(&pool, "DeviceRegistered").await,
            1,
            "a page-load last-seen refresh must not announce"
        );

        DeviceStore::rename(&pool, &bus, "d1", Some("My MacBook"), None)
            .await
            .unwrap();
        assert_eq!(emitted(&pool, "DeviceRenamed").await, 1);

        DeviceStore::set_push_enabled(&pool, &bus, "d1", true, None)
            .await
            .unwrap();
        assert_eq!(emitted(&pool, "DevicePushChanged").await, 1);

        // Re-asserting the current value still reports the device exists (the
        // HTTP handler renders `false` as "Device not found"), but announces
        // nothing.
        assert!(DeviceStore::set_push_enabled(&pool, &bus, "d1", true, None)
            .await
            .unwrap());
        assert_eq!(
            emitted(&pool, "DevicePushChanged").await,
            1,
            "a no-op toggle must not announce"
        );

        assert!(DeviceStore::delete(&pool, &bus, "d1", None).await.unwrap());
        assert_eq!(emitted(&pool, "DeviceDeleted").await, 1);
        assert!(!DeviceStore::delete(&pool, &bus, "d1", None).await.unwrap());
        assert_eq!(
            emitted(&pool, "DeviceDeleted").await,
            1,
            "second delete removes nothing and therefore announces nothing"
        );

        crate::test_support::teardown_test_db(&db_name).await;
    }

    /// Deleting a device takes its pins with it under the single DeviceDeleted
    /// event. A PinnedAppUnpinned per app would describe a device no client
    /// still tracks, so the cascade is deliberately silent.
    #[tokio::test]
    async fn delete_cascades_pins_silently_under_device_deleted() {
        let (pool, db_name) = crate::test_support::setup_test_db().await;
        let (bus, _callback_rx) = EventBus::new(pool.clone());

        DeviceStore::register(&pool, &bus, "d1", Some("UA"), None)
            .await
            .unwrap();
        PinnedAppStore::pin(&pool, &bus, "habit-tracker", "main", "d1", None)
            .await
            .unwrap();
        assert_eq!(
            PinnedAppStore::list_for_device(&pool, "d1")
                .await
                .unwrap()
                .len(),
            1
        );

        DeviceStore::delete(&pool, &bus, "d1", None).await.unwrap();
        assert!(PinnedAppStore::list_for_device(&pool, "d1")
            .await
            .unwrap()
            .is_empty());
        let unpinned: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM events WHERE event_type = 'PinnedAppUnpinned'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(unpinned, 0, "the cascade rides under DeviceDeleted");

        crate::test_support::teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn list_stale_push_enabled_filters_by_age_and_push_state() {
        let (pool, db_name) = crate::test_support::setup_test_db().await;
        let (bus, _callback_rx) = EventBus::new(pool.clone());

        // Push-enabled + old → returned
        DeviceStore::register(&pool, &bus, "old-on", Some("UA"), None)
            .await
            .unwrap();
        DeviceStore::set_push_enabled(&pool, &bus, "old-on", true, None)
            .await
            .unwrap();
        backdate_last_seen(&pool, "old-on", 45).await;

        // Push-enabled + recent → excluded (last_seen is today)
        DeviceStore::register(&pool, &bus, "fresh-on", Some("UA"), None)
            .await
            .unwrap();
        DeviceStore::set_push_enabled(&pool, &bus, "fresh-on", true, None)
            .await
            .unwrap();

        // Push-disabled + old → excluded (filtered at SELECT to avoid no-op events)
        DeviceStore::register(&pool, &bus, "old-off", Some("UA"), None)
            .await
            .unwrap();
        backdate_last_seen(&pool, "old-off", 45).await;

        // Right on the cutoff (29 days) → excluded
        DeviceStore::register(&pool, &bus, "almost-on", Some("UA"), None)
            .await
            .unwrap();
        DeviceStore::set_push_enabled(&pool, &bus, "almost-on", true, None)
            .await
            .unwrap();
        backdate_last_seen(&pool, "almost-on", 29).await;

        let stale = DeviceStore::list_stale_push_enabled(&pool, 30)
            .await
            .unwrap();
        assert_eq!(stale, vec!["old-on".to_string()]);

        crate::test_support::teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn list_stale_push_enabled_returns_empty_when_nothing_stale() {
        let (pool, db_name) = crate::test_support::setup_test_db().await;
        let (bus, _callback_rx) = EventBus::new(pool.clone());
        DeviceStore::register(&pool, &bus, "fresh", Some("UA"), None)
            .await
            .unwrap();
        DeviceStore::set_push_enabled(&pool, &bus, "fresh", true, None)
            .await
            .unwrap();

        let stale = DeviceStore::list_stale_push_enabled(&pool, 30)
            .await
            .unwrap();
        assert!(stale.is_empty());

        crate::test_support::teardown_test_db(&db_name).await;
    }
}
