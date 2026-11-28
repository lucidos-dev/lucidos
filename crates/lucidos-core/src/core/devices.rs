use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;

use super::{DevicePresenceStore, PinnedAppStore};
use crate::engine::event_bus::{BusEvent, EventBus, SystemEvent};
use crate::engine::thread_events::MessageOrigin;
use crate::gateway_devices::{self, DeviceIdentities};

/// INVARIANT: `name` and `pairing_label` come from the gateway, never from the
/// `devices` table (ADR 0825).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Device {
    pub id: String,
    pub name: Option<String>,
    pub pairing_label: Option<String>,
    pub user_agent: Option<String>,
    pub push_enabled: bool,
    pub last_seen_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
struct DeviceRow {
    id: String,
    user_agent: Option<String>,
    push_enabled: bool,
    last_seen_at: DateTime<Utc>,
    created_at: DateTime<Utc>,
}

impl DeviceRow {
    fn named(self, identities: &DeviceIdentities) -> Device {
        Device {
            name: identities.name_of(&self.id).map(str::to_string),
            pairing_label: identities.label_of(&self.id).map(str::to_string),
            id: self.id,
            user_agent: self.user_agent,
            push_enabled: self.push_enabled,
            last_seen_at: self.last_seen_at,
            created_at: self.created_at,
        }
    }
}

impl Device {
    fn label(&self) -> String {
        friendly_device_name(
            self.name.as_deref(),
            self.pairing_label.as_deref(),
            self.user_agent.as_deref(),
            &self.id,
        )
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct DeviceListing {
    #[serde(flatten)]
    pub device: Device,
    pub replaced_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SeenDevice {
    pub id: String,
    pub label: String,
    pub details: Option<String>,
    pub seen_secs_ago: i64,
    pub visible_now: bool,
}

const SEEN_DEVICE_SELECT: &str = "SELECT d.id, d.user_agent, \
        EXTRACT(EPOCH FROM now() - s.seen)::bigint AS seen_secs_ago, \
        COALESCE(p.visible_at > now() - make_interval(secs => $1), false) AS visible_now \
     FROM devices d \
     LEFT JOIN device_presence p ON p.device_id = d.id \
     CROSS JOIN LATERAL (SELECT GREATEST(d.last_seen_at, p.visible_at) AS seen) s";

fn presence_window_secs() -> f64 {
    super::device_presence::PRESENCE_STALE_AFTER.num_seconds() as f64
}

#[derive(sqlx::FromRow)]
struct SeenDeviceRow {
    id: String,
    user_agent: Option<String>,
    seen_secs_ago: i64,
    visible_now: bool,
}

impl SeenDevice {
    fn of(row: SeenDeviceRow, identities: &DeviceIdentities) -> Self {
        Self {
            label: friendly_device_name(
                identities.name_of(&row.id),
                identities.label_of(&row.id),
                row.user_agent.as_deref(),
                &row.id,
            ),
            details: row.user_agent.as_deref().map(parse_user_agent),
            id: row.id,
            seen_secs_ago: row.seen_secs_ago,
            visible_now: row.visible_now,
        }
    }
}

const MAX_ALIAS_HOPS: i32 = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum HandOver {
    Moved,
    AlreadyDone,
    NoSuchDevice,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NewIdRows {
    Orphans,
    /// INVARIANT: The new id is a registered device. Keep its rows, and drop only the
    /// old id's rows that would clash with them.
    Live,
}

pub struct DeviceStore;

impl DeviceStore {
    pub async fn native_push_devices(pool: &sqlx::PgPool) -> Result<Vec<String>, sqlx::Error> {
        sqlx::query_scalar("SELECT id FROM devices WHERE position($1 in user_agent) > 0")
            .bind(DESKTOP_APP_UA_TOKEN)
            .fetch_all(pool)
            .await
    }

    /// INVARIANT: Register or update a device (upsert by id). Returns `(device, inserted)`
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
            device: DeviceRow,
            inserted: bool,
        }

        let row: DeviceWithInsertFlag = sqlx::query_as(
            "INSERT INTO devices (id, user_agent, last_seen_at)
             VALUES ($1, $2, NOW())
             ON CONFLICT (id) DO UPDATE SET
                user_agent = COALESCE($2, devices.user_agent),
                last_seen_at = NOW()
             RETURNING id, user_agent, push_enabled, last_seen_at, created_at, (xmax = 0) AS inserted",
        )
        .bind(id)
        .bind(user_agent)
        .fetch_one(pool)
        .await?;
        Ok((row.device.named(&gateway_devices::current()), row.inserted))
    }

    /// INVARIANT: Does a row exist for this device id?
    ///
    /// The *attribution* probe, distinct from [`Self::friendly_name`] on purpose.
    /// `friendly_name` collapses "no such device" and "the database is down"
    /// into the same `None`, which is fine when the answer only picks a label
    /// but wrong when it decides whether to accept a request: a transient
    /// outage would then refuse the user's own chat sends. This returns the
    /// error so the caller can name its fallback, per the UNKNOWN-is-not-a-no
    /// rule in `.claude/rules/rust.md`.
    ///
    /// The one caller today is the human-attribution gate in
    /// `api::chat::require_human_mode_is_attributed`, which treats `Err` as
    /// attributed: a missed refusal costs a mis-labelled event, a false refusal
    /// costs the user their message.
    pub async fn is_registered(pool: &PgPool, id: &str) -> Result<bool, sqlx::Error> {
        sqlx::query_scalar::<_, bool>("SELECT EXISTS (SELECT 1 FROM devices WHERE id = $1)")
            .bind(id)
            .fetch_one(pool)
            .await
    }

    /// INVARIANT: The row a device id names for display: its own row, else the row its
    /// alias chain ends at. See [`Self::hand_over`].
    ///
    /// For display only. An alias never makes an id registered, stamped or the
    /// owner, so no gate may call this. [`Self::is_registered`] reads the id's
    /// own row and nothing else.
    async fn display_row(pool: &PgPool, id: &str) -> Result<Option<Device>, sqlx::Error> {
        let row: Option<DeviceRow> = sqlx::query_as(
            "WITH RECURSIVE chain (id, depth) AS (
                 SELECT $1::text, 0
                 UNION ALL
                 SELECT a.device_id, c.depth + 1
                 FROM chain c JOIN device_aliases a ON a.old_id = c.id
                 WHERE c.depth < $2
                   AND NOT EXISTS (SELECT 1 FROM devices d WHERE d.id = c.id)
             )
             SELECT d.id, d.user_agent, d.push_enabled, d.last_seen_at, d.created_at
             FROM chain c JOIN devices d ON d.id = c.id
             ORDER BY c.depth LIMIT 1",
        )
        .bind(id)
        .bind(MAX_ALIAS_HOPS)
        .fetch_optional(pool)
        .await?;
        Ok(row.map(|r| r.named(&gateway_devices::current())))
    }

    /// INVARIANT: The id a device answers to now: its own when it has a row, else the id
    /// its alias chain ends at. `None` when neither has a row.
    ///
    /// For addressing a device, such as a navigation target. Like
    /// [`Self::display_row`], no gate may call this.
    pub async fn current_id(pool: &PgPool, id: &str) -> Result<Option<String>, sqlx::Error> {
        Ok(Self::display_row(pool, id).await?.map(|d| d.id))
    }

    pub async fn friendly_name(pool: &PgPool, id: &str) -> String {
        let device = match Self::display_row(pool, id).await {
            Ok(d) => d,
            Err(e) => {
                log_legacy!("[Devices] friendly_name({}) failed: {}", id, e);
                None
            }
        };
        match device {
            Some(d) => d.label(),
            None => {
                let identities = gateway_devices::current();
                friendly_device_name(identities.name_of(id), identities.label_of(id), None, id)
            }
        }
    }

    pub async fn tooltip_info(pool: &PgPool, id: &str) -> Option<String> {
        let device = match Self::display_row(pool, id).await {
            Ok(d) => d,
            Err(e) => {
                log_legacy!("[Devices] tooltip_info({}) failed: {}", id, e);
                return None;
            }
        };
        let device = device?;
        let name = device.label();
        let ua = device.user_agent.as_deref().map(parse_user_agent);
        match ua {
            Some(parsed) => Some(format!("{}\n{}", name, parsed)),
            None => Some(name),
        }
    }

    pub async fn list(
        pool: &PgPool,
    ) -> Result<Vec<DeviceListing>, Box<dyn std::error::Error + Send + Sync>> {
        let identities = gateway_devices::current();
        let devices: Vec<Device> = sqlx::query_as::<_, DeviceRow>(
            "SELECT id, user_agent, push_enabled, last_seen_at, created_at
             FROM devices ORDER BY last_seen_at DESC",
        )
        .fetch_all(pool)
        .await?
        .into_iter()
        .map(|row| row.named(&identities))
        .collect();
        let resolved: Vec<(String, String)> = sqlx::query_as(
            "WITH RECURSIVE chain (old_id, id, depth) AS (
                 SELECT a.old_id, a.device_id, 1 FROM device_aliases a
                 WHERE NOT EXISTS (SELECT 1 FROM devices d WHERE d.id = a.old_id)
                 UNION ALL
                 SELECT c.old_id, a.device_id, c.depth + 1
                 FROM chain c JOIN device_aliases a ON a.old_id = c.id
                 WHERE c.depth < $1
                   AND NOT EXISTS (SELECT 1 FROM devices d WHERE d.id = c.id)
             )
             SELECT DISTINCT ON (c.old_id) c.old_id, c.id
             FROM chain c JOIN devices d ON d.id = c.id
             ORDER BY c.old_id, c.depth",
        )
        .bind(MAX_ALIAS_HOPS)
        .fetch_all(pool)
        .await?;
        let mut replaced: std::collections::HashMap<String, Vec<String>> =
            std::collections::HashMap::new();
        for (old_id, id) in resolved {
            replaced.entry(id).or_default().push(old_id);
        }
        Ok(devices
            .into_iter()
            .map(|device| DeviceListing {
                replaced_ids: replaced.remove(&device.id).unwrap_or_default(),
                device,
            })
            .collect())
    }

    pub async fn recently_seen(
        pool: &PgPool,
        limit: i64,
        within_days: i32,
    ) -> Result<Vec<SeenDevice>, sqlx::Error> {
        let rows: Vec<SeenDeviceRow> = sqlx::query_as(&format!(
            "{SEEN_DEVICE_SELECT} \
             WHERE s.seen > now() - make_interval(days => $2) \
             ORDER BY s.seen DESC \
             LIMIT $3"
        ))
        .bind(presence_window_secs())
        .bind(within_days)
        .bind(limit)
        .fetch_all(pool)
        .await?;
        let identities = gateway_devices::current();
        Ok(rows
            .into_iter()
            .map(|row| SeenDevice::of(row, &identities))
            .collect())
    }

    pub async fn user_agent(pool: &PgPool, id: &str) -> Result<Option<String>, sqlx::Error> {
        Ok(
            sqlx::query_scalar::<_, Option<String>>("SELECT user_agent FROM devices WHERE id = $1")
                .bind(id)
                .fetch_optional(pool)
                .await?
                .flatten(),
        )
    }

    pub async fn seen(pool: &PgPool, id: &str) -> Result<Option<SeenDevice>, sqlx::Error> {
        let row: Option<SeenDeviceRow> =
            sqlx::query_as(&format!("{SEEN_DEVICE_SELECT} WHERE d.id = $2"))
                .bind(presence_window_secs())
                .bind(id)
                .fetch_optional(pool)
                .await?;
        Ok(row.map(|row| SeenDevice::of(row, &gateway_devices::current())))
    }

    pub async fn mark_seen(pool: &PgPool, id: &str) -> Result<(), sqlx::Error> {
        sqlx::query("UPDATE devices SET last_seen_at = NOW() WHERE id = $1")
            .bind(id)
            .execute(pool)
            .await?;
        Ok(())
    }

    async fn delete_device_rows(
        conn: &mut sqlx::PgConnection,
        id: &str,
    ) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
        sqlx::query("DELETE FROM preferences WHERE device_id = $1")
            .bind(id)
            .execute(&mut *conn)
            .await?;
        sqlx::query("DELETE FROM push_subscriptions WHERE device_id = $1")
            .bind(id)
            .execute(&mut *conn)
            .await?;
        PinnedAppStore::delete_for_device(&mut *conn, id).await?;
        DevicePresenceStore::delete_for_device(&mut *conn, id).await?;
        let result = sqlx::query("DELETE FROM devices WHERE id = $1")
            .bind(id)
            .execute(conn)
            .await?;
        Ok(result.rows_affected() > 0)
    }

    /// INVARIANT: Delete one device and its per-device state in one transaction.
    ///
    /// Locks the device row before touching its children, the order
    /// [`Self::remove_one_off`] takes. The opposite order deadlocks against it.
    async fn delete_row(
        pool: &PgPool,
        id: &str,
    ) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
        let mut tx = pool.begin().await?;
        sqlx::query("SELECT 1 FROM devices WHERE id = $1 FOR UPDATE")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        let removed = Self::delete_device_rows(&mut tx, id).await?;
        tx.commit().await?;
        Ok(removed)
    }

    /// INVARIANT: Set push_enabled on a device row. **Private on purpose**:
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

    /// INVARIANT: Rename a device this workspace knows, on the gateway, so the
    /// name shows in every workspace (ADR 0825). `false` when no such device.
    pub async fn rename(
        pool: &PgPool,
        event_bus: &EventBus,
        id: &str,
        name: Option<&str>,
        actor: Option<MessageOrigin>,
    ) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
        let renamed = Self::is_registered(pool, id).await?;
        if renamed {
            gateway_devices::rename(id, name).await?;
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

    /// INVARIANT: Remove every *one-off device* and announce each removal. Returns the
    /// removed ids, oldest first.
    ///
    /// A one-off device was created more than `older_than_days` ago and never
    /// seen again after its first 24 hours. Nobody named it, it never paired
    /// through the gateway, and it holds no push state. A fresh browser profile
    /// mints a fresh device id, so every headless test run leaves one behind.
    /// The rule reads no user agent on purpose: emulated runs claim to be
    /// phones. Rationale in `docs/plans/2026-10-02-one-off-devices-expire.md`.
    ///
    /// The rows are locked as they are selected, and deleted in the same
    /// transaction. A device that registers mid-sweep is therefore never lost:
    /// either its refreshed `last_seen_at` excludes it, or its upsert waits for
    /// the commit and inserts it again as new.
    pub async fn remove_one_off(
        pool: &PgPool,
        event_bus: &EventBus,
        older_than_days: i32,
    ) -> Result<Vec<String>, Box<dyn std::error::Error + Send + Sync>> {
        let Some(identities) = gateway_devices::known() else {
            return Ok(Vec::new());
        };
        let identified = identities.identified_ids();
        let mut tx = pool.begin().await?;
        let ids: Vec<String> = sqlx::query_scalar(
            "SELECT d.id FROM devices d
             LEFT JOIN device_presence p ON p.device_id = d.id
             WHERE d.created_at < now() - make_interval(days => $1)
               AND GREATEST(d.last_seen_at, p.visible_at) < d.created_at + interval '1 day'
               AND NOT (d.id = ANY($2))
               AND NOT EXISTS (SELECT 1 FROM device_name_imports i WHERE i.device_id = d.id)
               AND NOT d.push_enabled
               AND NOT EXISTS (SELECT 1 FROM push_subscriptions s WHERE s.device_id = d.id)
             ORDER BY d.created_at
             FOR UPDATE OF d SKIP LOCKED",
        )
        .bind(older_than_days)
        .bind(&identified)
        .fetch_all(&mut *tx)
        .await?;
        for id in &ids {
            Self::delete_device_rows(&mut tx, id).await?;
        }
        tx.commit().await?;

        for id in &ids {
            event_bus
                .emit_or_log(
                    BusEvent::System(SystemEvent::DeviceDeleted {
                        device_id: id.clone(),
                        actor: None,
                    }),
                    "[Devices] DeviceDeleted",
                )
                .await;
        }
        Ok(ids)
    }

    /// INVARIANT: Move every trace of `old_id` onto `new_id`, and announce the result.
    ///
    /// The one migration path off the `localStorage` device id. A browser now
    /// takes its id from the *workspace gateway*. Without this it arrives as a
    /// new device and loses its push subscription and its preferences. It knows
    /// both ids for one page load, which is the window this uses. Rationale in
    /// `docs/plans/2026-08-22-one-device-identity-minted-at-the-gateway.md`.
    ///
    /// When the new id already has a row (a window that registered fresh
    /// before it could hand over), the two rows merge. The new id's own
    /// settings win, and everything it lacks comes over from the old id.
    ///
    /// Order is load-bearing. `push_subscriptions.device_id` is a foreign key
    /// onto `devices(id)` with no `ON UPDATE CASCADE`, so the parent key cannot
    /// be renamed in place. Make sure the new row exists, repoint the children,
    /// drop the old row last.
    ///
    /// The old id becomes an alias of the new one, so history recorded under
    /// it resolves to the device's current name. See [`Self::friendly_name`].
    ///
    /// `push_log` is left alone: it records what was sent to which id at the
    /// time, so it is history, like an event payload.
    pub async fn hand_over(
        pool: &PgPool,
        event_bus: &EventBus,
        old_id: &str,
        new_id: &str,
        actor: Option<MessageOrigin>,
    ) -> Result<HandOver, Box<dyn std::error::Error + Send + Sync>> {
        if old_id == new_id {
            return Ok(HandOver::AlreadyDone);
        }
        let mut tx = pool.begin().await?;

        let old_row: Option<String> =
            sqlx::query_scalar("SELECT id FROM devices WHERE id = $1 FOR UPDATE")
                .bind(old_id)
                .fetch_optional(&mut *tx)
                .await?;
        if old_row.is_none() {
            return Ok(HandOver::NoSuchDevice);
        }
        // INVARIANT: Under READ COMMITTED a concurrent `register(new_id)` can commit
        // after this read. The `devices` primary key then fails the copy below,
        // and the transaction aborts to a 500 the client retries as a merge.
        let new_rows = if Self::is_registered_in(&mut tx, new_id).await? {
            NewIdRows::Live
        } else {
            NewIdRows::Orphans
        };
        match new_rows {
            NewIdRows::Orphans => Self::copy_row_forward(&mut tx, old_id, new_id).await?,
            NewIdRows::Live => Self::merge_row_into(&mut tx, old_id, new_id).await?,
        }
        Self::move_preferences(&mut tx, old_id, new_id, new_rows).await?;
        sqlx::query("UPDATE push_subscriptions SET device_id = $2 WHERE device_id = $1")
            .bind(old_id)
            .bind(new_id)
            .execute(&mut *tx)
            .await?;
        PinnedAppStore::move_device(&mut tx, old_id, new_id, new_rows).await?;
        DevicePresenceStore::move_device(&mut tx, old_id, new_id, new_rows).await?;

        sqlx::query("DELETE FROM devices WHERE id = $1")
            .bind(old_id)
            .execute(&mut *tx)
            .await?;
        Self::record_alias(&mut tx, old_id, new_id).await?;
        tx.commit().await?;
        Self::carry_names_over(pool).await;

        event_bus
            .emit_or_log(
                BusEvent::System(SystemEvent::DeviceHandedOver {
                    old_device_id: old_id.to_string(),
                    device_id: new_id.to_string(),
                    actor,
                }),
                "[Devices] DeviceHandedOver",
            )
            .await;
        Ok(HandOver::Moved)
    }

    /// INVARIANT: A handed-over id passes its typed name on the gateway to the id it
    /// answers to now, when that id has none of its own. The alias row owes the
    /// copy until it lands, so a gateway that is down only delays it: boot and
    /// every gateway signal try again. Never fatal, since the rows already moved.
    pub async fn carry_names_over(pool: &PgPool) {
        if let Err(e) = Self::try_carry_names_over(pool).await {
            log_legacy!("[Devices] could not carry device names over yet: {e}");
        }
    }

    async fn try_carry_names_over(
        pool: &PgPool,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let owed: Vec<String> = sqlx::query_scalar(
            "SELECT old_id FROM device_aliases WHERE NOT name_carried ORDER BY created_at",
        )
        .fetch_all(pool)
        .await?;
        if owed.is_empty() {
            return Ok(());
        }
        gateway_devices::load().await?;
        for old_id in owed {
            let identities = gateway_devices::current();
            if let (Some(name), Some(new_id)) = (
                identities.name_of(&old_id),
                Self::current_id(pool, &old_id).await?,
            ) {
                if identities.name_of(&new_id).is_none() {
                    gateway_devices::rename(&new_id, Some(name)).await?;
                }
            }
            sqlx::query("UPDATE device_aliases SET name_carried = TRUE WHERE old_id = $1")
                .bind(&old_id)
                .execute(pool)
                .await?;
        }
        Ok(())
    }

    async fn is_registered_in(
        conn: &mut sqlx::PgConnection,
        id: &str,
    ) -> Result<bool, sqlx::Error> {
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM devices WHERE id = $1)")
            .bind(id)
            .fetch_one(conn)
            .await
    }

    async fn copy_row_forward(
        conn: &mut sqlx::PgConnection,
        old_id: &str,
        new_id: &str,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO devices (id, user_agent, push_enabled, last_seen_at, created_at)
             SELECT $2, user_agent, push_enabled, last_seen_at, created_at
             FROM devices WHERE id = $1",
        )
        .bind(old_id)
        .bind(new_id)
        .execute(conn)
        .await?;
        Ok(())
    }

    /// INVARIANT: The new row keeps what it has and takes what it lacks from the old
    /// row. Part of [`Self::hand_over`].
    ///
    /// Push stays on when either row had it: the old row's subscriptions move
    /// here, and a subscription on a device with push off never fires.
    async fn merge_row_into(
        conn: &mut sqlx::PgConnection,
        old_id: &str,
        new_id: &str,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "UPDATE devices n SET
                user_agent = COALESCE(n.user_agent, o.user_agent),
                push_enabled = n.push_enabled OR o.push_enabled,
                last_seen_at = GREATEST(n.last_seen_at, o.last_seen_at),
                created_at = LEAST(n.created_at, o.created_at)
             FROM devices o
             WHERE n.id = $2 AND o.id = $1",
        )
        .bind(old_id)
        .bind(new_id)
        .execute(conn)
        .await?;
        Ok(())
    }

    async fn move_preferences(
        conn: &mut sqlx::PgConnection,
        old_id: &str,
        new_id: &str,
        new_rows: NewIdRows,
    ) -> Result<(), sqlx::Error> {
        match new_rows {
            NewIdRows::Orphans => {
                sqlx::query("DELETE FROM preferences WHERE device_id = $1")
                    .bind(new_id)
                    .execute(&mut *conn)
                    .await?;
            }
            NewIdRows::Live => {
                sqlx::query(
                    "DELETE FROM preferences o WHERE o.device_id = $1
                     AND EXISTS (SELECT 1 FROM preferences n
                                 WHERE n.device_id = $2 AND n.key = o.key)",
                )
                .bind(old_id)
                .bind(new_id)
                .execute(&mut *conn)
                .await?;
            }
        }
        sqlx::query("UPDATE preferences SET device_id = $2 WHERE device_id = $1")
            .bind(old_id)
            .bind(new_id)
            .execute(conn)
            .await?;
        Ok(())
    }

    async fn record_alias(
        conn: &mut sqlx::PgConnection,
        old_id: &str,
        new_id: &str,
    ) -> Result<(), sqlx::Error> {
        sqlx::query("DELETE FROM device_aliases WHERE old_id = $1")
            .bind(new_id)
            .execute(&mut *conn)
            .await?;
        sqlx::query(
            "INSERT INTO device_aliases (old_id, device_id, name_carried) VALUES ($1, $2, FALSE)
             ON CONFLICT (old_id) DO UPDATE
             SET device_id = EXCLUDED.device_id, created_at = NOW(), name_carried = FALSE",
        )
        .bind(old_id)
        .bind(new_id)
        .execute(conn)
        .await?;
        Ok(())
    }

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

/// INVARIANT: What to call a device: the one rule every engine surface names it by.
///
/// The order: the typed name, then the pairing label, then the browser and
/// machine the user-agent names. That last one carries the short id, as in
/// "Chrome on Mac (109371a3)". Failing all three, it is `device-` and the id. The frontend's `deviceFriendlyName`
/// applies the same order, so a device is called one thing on every screen.
/// Never build a device name anywhere else.
pub(crate) fn friendly_device_name(
    name: Option<&str>,
    pairing_label: Option<&str>,
    user_agent: Option<&str>,
    id: &str,
) -> String {
    if let Some(n) = [name, pairing_label]
        .into_iter()
        .flatten()
        .find(|n| !n.trim().is_empty())
    {
        return n.to_string();
    }
    let short = &id[..id.floor_char_boundary(8)];
    match user_agent.and_then(user_agent_device_label) {
        Some(label) => format!("{label} ({short})"),
        None => format!("device-{short}"),
    }
}

/// INVARIANT: A name read off a device's stored user-agent, for a device nobody named.
/// Mirrors `userAgentDeviceLabel` and `suggestDeviceLabel` in the frontend's
/// `utils/deviceLabel.ts`: Chromium forks are asked about before Chrome, and
/// Safari last, since every browser above it also says Safari.
fn user_agent_device_label(ua: &str) -> Option<String> {
    let has = |token: &str| ua.contains(token);
    let platform = if has("iPhone") {
        Some("iPhone")
    } else if has("iPad") {
        Some("iPad")
    } else if has("Android") {
        Some("Android")
    } else if has("CrOS") {
        Some("Chromebook")
    } else if has("Macintosh") || has("Mac OS X") {
        Some("Mac")
    } else if has("Windows") {
        Some("Windows")
    } else if has("Linux") {
        Some("Linux")
    } else {
        None
    };
    if has(DESKTOP_APP_UA_TOKEN) {
        return Some(match platform {
            Some(p) => format!("Lucidos app on {p}"),
            None => "Lucidos app".to_string(),
        });
    }
    let browser = if ["Edg/", "EdgA/", "EdgiOS/", "Edge/"].iter().any(|t| has(t)) {
        Some("Edge")
    } else if has("OPR/") || has("Opera/") || has("Opera ") {
        Some("Opera")
    } else if has("SamsungBrowser/") {
        Some("Samsung Internet")
    } else if has("Firefox/") || has("FxiOS/") {
        Some("Firefox")
    } else if has("CriOS/") || has("Chrome/") || has("Chromium/") {
        Some("Chrome")
    } else if has("Safari/") {
        Some("Safari")
    } else {
        None
    };
    match (browser, platform) {
        (Some(b), Some(p)) => Some(format!("{b} on {p}")),
        (Some(one), None) | (None, Some(one)) => Some(one.to_string()),
        (None, None) => None,
    }
}

/// INVARIANT: Product token the Tauri native desktop client appends to its registered
/// user-agent string (see `registrationUserAgent` in
/// `crates/lucidos-app/src/utils/platform.ts`). The WKWebView's real UA is
/// indistinguishable from Safari, so this token is the only signal that lets the
/// agent's device context tell the desktop app from a browser — keep the literal
/// in sync with the frontend constant.
const DESKTOP_APP_UA_TOKEN: &str = "Lucidos-Desktop";

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
    use crate::core::prefs;

    #[test]
    fn parse_user_agent_renders_desktop_token_as_lucidos_desktop_app() {
        let ua = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 \
                  (KHTML, like Gecko) Version/18.0 Safari/605.1.15 Lucidos-Desktop";
        assert_eq!(parse_user_agent(ua), "Lucidos desktop app on macOS");
    }

    #[test]
    fn parse_user_agent_leaves_browsers_unchanged() {
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

    #[test]
    fn friendly_device_name_prefers_the_typed_name_then_the_pairing_label() {
        let id = "ab2c03f77d715bce";
        assert_eq!(
            friendly_device_name(Some("My iPhone"), Some("Safari on iPhone"), None, id),
            "My iPhone"
        );
        assert_eq!(
            friendly_device_name(None, Some("Safari on iPhone"), None, id),
            "Safari on iPhone"
        );
        assert_eq!(
            friendly_device_name(Some("  "), Some("Safari on iPhone"), None, id),
            "Safari on iPhone",
            "a blank typed name is no name"
        );
        assert_eq!(
            friendly_device_name(None, None, None, id),
            "device-ab2c03f7"
        );
        assert_eq!(
            friendly_device_name(None, Some(""), None, id),
            "device-ab2c03f7"
        );
    }

    #[test]
    fn friendly_device_name_falls_back_to_the_user_agent_before_the_id() {
        let id = "109371a3-ee53-42fa-b34b-c168237467b7";
        let name = |ua: &str| friendly_device_name(None, None, Some(ua), id);
        assert_eq!(
            name(
                "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 \
                  (KHTML, like Gecko) Chrome/153.0.8010.12 Safari/537.36"
            ),
            "Chrome on Mac (109371a3)"
        );
        assert_eq!(
            name(
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
                  (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36 Edg/120.0.0.0"
            ),
            "Edge on Windows (109371a3)",
            "a Chromium fork is named before the Chrome it carries"
        );
        assert_eq!(
            name(
                "Mozilla/5.0 (iPhone; CPU iPhone OS 18_0 like Mac OS X) AppleWebKit/605.1.15 \
                  (KHTML, like Gecko) CriOS/130.0 Mobile/15E148 Safari/604.1"
            ),
            "Chrome on iPhone (109371a3)"
        );
        assert_eq!(
            name(
                "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 \
                  (KHTML, like Gecko) Version/18.0 Safari/605.1.15 Lucidos-Desktop"
            ),
            "Lucidos app on Mac (109371a3)"
        );
        assert_eq!(
            name("curl/8.7.1"),
            "device-109371a3",
            "a user-agent naming no browser and no machine says nothing"
        );
        assert_eq!(
            friendly_device_name(Some("My MacBook"), None, Some("Chrome/1"), id),
            "My MacBook"
        );
    }

    #[tokio::test]
    async fn friendly_name_reads_the_gateway_name_then_its_label_then_the_user_agent() {
        let (pool, db_name) = crate::test_support::setup_test_db().await;
        let (bus, _callback_rx) = EventBus::new(pool.clone());
        let ua = "Mozilla/5.0 (iPhone) Safari/604.1";
        for id in ["gw-named-1", "gw-labelled-1", "gw-unknown-1"] {
            DeviceStore::register(&pool, &bus, id, Some(ua), None)
                .await
                .unwrap();
        }
        gateway_devices::seed_for_test("gw-named-1", Some("Work phone"), Some("Safari on iPhone"));
        gateway_devices::seed_for_test("gw-labelled-1", None, Some("Safari on iPhone"));

        assert_eq!(
            DeviceStore::friendly_name(&pool, "gw-named-1").await,
            "Work phone"
        );
        assert_eq!(
            DeviceStore::friendly_name(&pool, "gw-labelled-1").await,
            "Safari on iPhone"
        );
        assert_eq!(
            DeviceStore::friendly_name(&pool, "gw-unknown-1").await,
            "Safari on iPhone (gw-unkno)"
        );
        let listed = DeviceStore::list(&pool).await.unwrap();
        let named = listed.iter().find(|l| l.device.id == "gw-named-1").unwrap();
        assert_eq!(named.device.name.as_deref(), Some("Work phone"));
        assert_eq!(
            named.device.pairing_label.as_deref(),
            Some("Safari on iPhone")
        );
        assert_eq!(
            DeviceStore::friendly_name(&pool, "no-such-device").await,
            "device-no-such-",
            "an unknown device still gets a name"
        );

        crate::test_support::teardown_test_db(&db_name).await;
    }

    async fn backdate_last_seen(pool: &PgPool, id: &str, days_ago: i64) {
        sqlx::query("UPDATE devices SET last_seen_at = NOW() - make_interval(days => $1::int) WHERE id = $2")
            .bind(days_ago)
            .bind(id)
            .execute(pool)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn recently_seen_orders_by_the_newer_record_and_drops_stale_devices() {
        use crate::test_support::seed_device;
        let (pool, db_name) = crate::test_support::setup_test_db().await;
        let mac = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) Chrome/149.0.0.0 Safari/537.36";
        let iphone = "Mozilla/5.0 (iPhone; CPU iPhone OS 18_0 like Mac OS X) Safari/604.1";
        seed_device(&pool, "laptop", Some(mac), Some("My MacBook")).await;
        seed_device(&pool, "phone", Some(iphone), None).await;
        seed_device(&pool, "tablet", None, Some("Old tablet")).await;
        backdate_last_seen(&pool, "laptop", 2).await;
        backdate_last_seen(&pool, "phone", 1).await;
        backdate_last_seen(&pool, "tablet", 40).await;
        DevicePresenceStore::record_visible(&pool, "laptop")
            .await
            .unwrap();

        let seen = DeviceStore::recently_seen(&pool, 5, 30).await.unwrap();
        let ids: Vec<&str> = seen.iter().map(|d| d.id.as_str()).collect();
        assert_eq!(
            ids,
            ["laptop", "phone"],
            "newest first, stale tablet dropped"
        );
        assert_eq!(seen[0].label, "My MacBook");
        assert_eq!(
            seen[0].details.as_deref(),
            Some("Chrome/149.0.0.0 on macOS")
        );
        assert!(seen[0].visible_now);
        assert!(
            seen[0].seen_secs_ago < 60,
            "the heartbeat is the newer record"
        );
        assert_eq!(
            seen[1].label, "Safari on iPhone (phone)",
            "an unnamed device is called by its browser and machine"
        );
        assert!(!seen[1].visible_now);
        assert!(seen[1].seen_secs_ago >= 86_000);

        let one = DeviceStore::recently_seen(&pool, 1, 30).await.unwrap();
        assert_eq!(one.len(), 1, "the limit bounds the list");

        pool.close().await;
        crate::test_support::teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn a_device_that_hides_lucidos_stays_seen_at_the_hide() {
        let (pool, db_name) = crate::test_support::setup_test_db().await;
        let (bus, _callback_rx) = EventBus::new(pool.clone());
        crate::test_support::seed_device(&pool, "phone", None, Some("My iPhone")).await;
        backdate_last_seen(&pool, "phone", 3).await;
        for event in [
            SystemEvent::DeviceVisible {
                device_id: "phone".into(),
            },
            SystemEvent::DeviceHidden {
                device_id: "phone".into(),
            },
        ] {
            bus.emit(BusEvent::System(event)).await.unwrap();
        }

        let seen = DeviceStore::seen(&pool, "phone").await.unwrap().unwrap();
        assert!(
            seen.seen_secs_ago < 60,
            "seen at the hide, not at the page load: {seen:?}"
        );
        assert!(!seen.visible_now);
        assert!(
            DevicePresenceStore::candidates(&pool)
                .await
                .unwrap()
                .is_empty(),
            "a hidden device is no PresenceCheck candidate"
        );

        pool.close().await;
        crate::test_support::teardown_test_db(&db_name).await;
    }

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

    async fn seed_device_state(pool: &PgPool, bus: &EventBus, id: &str) {
        DeviceStore::register(pool, bus, id, Some("UA"), None)
            .await
            .unwrap();
        gateway_devices::seed_for_test(id, None, Some("Safari on iPhone"));
        DeviceStore::set_push_enabled(pool, bus, id, true, None)
            .await
            .unwrap();
        DeviceStore::rename(pool, bus, id, Some("My iPhone"), None)
            .await
            .unwrap();
        PinnedAppStore::pin(pool, bus, "habit-tracker", "main", id, None)
            .await
            .unwrap();
        sqlx::query("INSERT INTO preferences (key, value, device_id) VALUES ($1, '125', $2)")
            .bind(prefs::UI_SCALE.key())
            .bind(id)
            .execute(pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO push_subscriptions (endpoint, p256dh, auth, device_id)
             VALUES ('https://push.example/sub', 'k', $2, $1)",
        )
        .bind(id)
        .bind(crate::core::secret_store::Secret::<
            crate::core::secret_store::PushAuth,
        >::new("a"))
        .execute(pool)
        .await
        .unwrap();
        sqlx::query("INSERT INTO device_presence (device_id) VALUES ($1)")
            .bind(id)
            .execute(pool)
            .await
            .unwrap();
    }

    async fn rows_for(pool: &PgPool, table: &str, id: &str) -> i64 {
        let column = if table == "devices" {
            "id"
        } else {
            "device_id"
        };
        sqlx::query_scalar(&format!("SELECT count(*) FROM {table} WHERE {column} = $1"))
            .bind(id)
            .fetch_one(pool)
            .await
            .unwrap()
    }

    const DEVICE_TABLES: &[&str] = &[
        "devices",
        "preferences",
        "pinned_apps",
        "device_presence",
        "push_subscriptions",
    ];

    #[tokio::test]
    async fn hand_over_moves_every_table_and_leaves_nothing_behind() {
        let (pool, db_name) = crate::test_support::setup_test_db().await;
        let (bus, _callback_rx) = EventBus::new(pool.clone());
        seed_device_state(&pool, &bus, "old").await;

        let outcome = DeviceStore::hand_over(&pool, &bus, "old", "new", None)
            .await
            .unwrap();
        assert_eq!(outcome, HandOver::Moved);

        for table in DEVICE_TABLES {
            assert_eq!(
                rows_for(&pool, table, "new").await,
                1,
                "{table} must carry the new id"
            );
            assert_eq!(
                rows_for(&pool, table, "old").await,
                0,
                "{table} must keep nothing under the old id"
            );
        }
        let device = DeviceStore::display_row(&pool, "new")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            device.name.as_deref(),
            Some("My iPhone"),
            "the name moves on the gateway"
        );
        assert!(device.push_enabled, "the push flag rides across");

        crate::test_support::teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn hand_over_announces_once() {
        let (pool, db_name) = crate::test_support::setup_test_db().await;
        let (bus, _callback_rx) = EventBus::new(pool.clone());
        seed_device_state(&pool, &bus, "old").await;

        DeviceStore::hand_over(&pool, &bus, "old", "new", None)
            .await
            .unwrap();

        let announced: i64 =
            sqlx::query_scalar("SELECT count(*) FROM events WHERE event_type = 'DeviceHandedOver'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(announced, 1);
        let registered: i64 =
            sqlx::query_scalar("SELECT count(*) FROM events WHERE event_type = 'DeviceRegistered'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(
            registered, 1,
            "the copied row is a move, not a second registration"
        );

        crate::test_support::teardown_test_db(&db_name).await;
    }

    async fn device_pref(pool: &PgPool, key: &str, id: &str) -> Option<String> {
        sqlx::query_scalar("SELECT value FROM preferences WHERE key = $1 AND device_id = $2")
            .bind(key)
            .bind(id)
            .fetch_optional(pool)
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn hand_over_onto_a_registered_id_merges_and_the_new_id_wins() {
        let (pool, db_name) = crate::test_support::setup_test_db().await;
        let (bus, _callback_rx) = EventBus::new(pool.clone());
        seed_device_state(&pool, &bus, "old").await;
        sqlx::query(
            "INSERT INTO preferences (key, value, device_id) VALUES ($1, 'tokyo-night', 'old')",
        )
        .bind(prefs::THEME.key())
        .execute(&pool)
        .await
        .unwrap();
        DeviceStore::register(&pool, &bus, "new", Some("UA"), None)
            .await
            .unwrap();
        sqlx::query("INSERT INTO preferences (key, value, device_id) VALUES ($1, 'mono', 'new')")
            .bind(prefs::THEME.key())
            .execute(&pool)
            .await
            .unwrap();
        PinnedAppStore::pin(&pool, &bus, "habit-tracker", "main", "new", None)
            .await
            .unwrap();
        sqlx::query("INSERT INTO device_presence (device_id) VALUES ('new')")
            .execute(&pool)
            .await
            .unwrap();

        let outcome = DeviceStore::hand_over(&pool, &bus, "old", "new", None)
            .await
            .unwrap();
        assert_eq!(outcome, HandOver::Moved);

        for table in DEVICE_TABLES {
            assert_eq!(
                rows_for(&pool, table, "old").await,
                0,
                "{table} keeps nothing under the old id"
            );
        }
        assert_eq!(rows_for(&pool, "push_subscriptions", "new").await, 1);
        assert_eq!(
            rows_for(&pool, "pinned_apps", "new").await,
            1,
            "a pin both had stays one pin"
        );
        assert_eq!(rows_for(&pool, "device_presence", "new").await, 1);
        assert_eq!(
            device_pref(&pool, prefs::THEME.key(), "new")
                .await
                .as_deref(),
            Some("mono"),
            "the new id's own value is never overwritten"
        );
        assert_eq!(
            device_pref(&pool, prefs::UI_SCALE.key(), "new")
                .await
                .as_deref(),
            Some("125"),
            "a value only the old id had comes over"
        );
        let device = DeviceStore::display_row(&pool, "new")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            device.name.as_deref(),
            Some("My iPhone"),
            "a nameless new row takes the old name"
        );
        assert!(device.push_enabled, "the moved subscription keeps push on");

        crate::test_support::teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn hand_over_merge_keeps_the_name_the_new_row_already_has() {
        let (pool, db_name) = crate::test_support::setup_test_db().await;
        let (bus, _callback_rx) = EventBus::new(pool.clone());
        seed_device_state(&pool, &bus, "old").await;
        DeviceStore::register(&pool, &bus, "new", Some("UA"), None)
            .await
            .unwrap();
        DeviceStore::rename(&pool, &bus, "new", Some("Desk Mac"), None)
            .await
            .unwrap();

        DeviceStore::hand_over(&pool, &bus, "old", "new", None)
            .await
            .unwrap();

        assert_eq!(DeviceStore::friendly_name(&pool, "new").await, "Desk Mac");
        crate::test_support::teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn a_replaced_id_resolves_to_the_current_name_through_a_chain() {
        let (pool, db_name) = crate::test_support::setup_test_db().await;
        let (bus, _callback_rx) = EventBus::new(pool.clone());
        seed_device_state(&pool, &bus, "first").await;

        DeviceStore::hand_over(&pool, &bus, "first", "second", None)
            .await
            .unwrap();
        assert_eq!(
            DeviceStore::friendly_name(&pool, "first").await,
            "My iPhone"
        );
        DeviceStore::hand_over(&pool, &bus, "second", "third", None)
            .await
            .unwrap();
        DeviceStore::rename(&pool, &bus, "third", Some("Phone"), None)
            .await
            .unwrap();

        for old in ["first", "second"] {
            assert_eq!(DeviceStore::friendly_name(&pool, old).await, "Phone");
            assert!(
                DeviceStore::tooltip_info(&pool, old)
                    .await
                    .is_some_and(|t| t.starts_with("Phone")),
                "{old}'s tooltip names the current device"
            );
            assert!(
                !DeviceStore::is_registered(&pool, old).await.unwrap(),
                "an alias never makes {old} registered"
            );
        }
        assert_eq!(
            DeviceStore::current_id(&pool, "first").await.unwrap(),
            Some("third".to_string())
        );
        let listed = DeviceStore::list(&pool).await.unwrap();
        let third = listed.iter().find(|l| l.device.id == "third").unwrap();
        let mut replaced = third.replaced_ids.clone();
        replaced.sort();
        assert_eq!(
            replaced,
            ["first", "second"],
            "the client names old turns from this"
        );
        crate::test_support::teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn an_id_that_comes_back_is_no_longer_an_alias() {
        let (pool, db_name) = crate::test_support::setup_test_db().await;
        let (bus, _callback_rx) = EventBus::new(pool.clone());
        seed_device_state(&pool, &bus, "a").await;

        DeviceStore::hand_over(&pool, &bus, "a", "b", None)
            .await
            .unwrap();
        DeviceStore::hand_over(&pool, &bus, "b", "a", None)
            .await
            .unwrap();

        assert_eq!(DeviceStore::friendly_name(&pool, "b").await, "My iPhone");
        assert_eq!(DeviceStore::friendly_name(&pool, "a").await, "My iPhone");
        let looped: i64 =
            sqlx::query_scalar("SELECT count(*) FROM device_aliases WHERE old_id = 'a'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(looped, 0);
        crate::test_support::teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn a_name_the_gateway_could_not_take_is_carried_over_on_the_next_try() {
        let (pool, db_name) = crate::test_support::setup_test_db().await;
        let (bus, _callback_rx) = EventBus::new(pool.clone());
        seed_device_state(&pool, &bus, "old").await;
        DeviceStore::hand_over(&pool, &bus, "old", "new", None)
            .await
            .unwrap();
        let owed = || async {
            sqlx::query_scalar::<_, bool>(
                "SELECT NOT name_carried FROM device_aliases WHERE old_id = 'old'",
            )
            .fetch_one(&pool)
            .await
            .unwrap()
        };
        assert!(
            !owed().await,
            "a hand-over that reached the gateway owes nothing"
        );

        gateway_devices::seed_for_test("new", None, None);
        gateway_devices::seed_for_test("old", Some("Desk Mac"), None);
        sqlx::query("UPDATE device_aliases SET name_carried = FALSE")
            .execute(&pool)
            .await
            .unwrap();
        DeviceStore::carry_names_over(&pool).await;

        assert_eq!(DeviceStore::friendly_name(&pool, "new").await, "Desk Mac");
        assert!(!owed().await);
        crate::test_support::teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn the_alias_migration_backfills_past_hand_overs() {
        let (pool, db_name) = crate::test_support::setup_test_db().await;
        let (bus, _callback_rx) = EventBus::new(pool.clone());
        seed_device_state(&pool, &bus, "first").await;
        DeviceStore::hand_over(&pool, &bus, "first", "second", None)
            .await
            .unwrap();
        DeviceStore::hand_over(&pool, &bus, "second", "third", None)
            .await
            .unwrap();
        sqlx::query("DELETE FROM device_aliases")
            .execute(&pool)
            .await
            .unwrap();
        for gone in ["first", "second"] {
            gateway_devices::seed_for_test(gone, None, None);
        }
        assert!(DeviceStore::friendly_name(&pool, "first")
            .await
            .starts_with("device-"));

        sqlx::raw_sql(include_str!(
            "../../migrations/20261120135610_add_device_aliases.sql"
        ))
        .execute(&pool)
        .await
        .unwrap();

        assert_eq!(
            DeviceStore::friendly_name(&pool, "first").await,
            "My iPhone"
        );
        assert_eq!(
            DeviceStore::friendly_name(&pool, "second").await,
            "My iPhone"
        );
        crate::test_support::teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn hand_over_reports_an_unknown_old_id_rather_than_failing() {
        let (pool, db_name) = crate::test_support::setup_test_db().await;
        let (bus, _callback_rx) = EventBus::new(pool.clone());

        let outcome = DeviceStore::hand_over(&pool, &bus, "never-existed", "new", None)
            .await
            .unwrap();
        assert_eq!(outcome, HandOver::NoSuchDevice);
        assert_eq!(
            rows_for(&pool, "devices", "new").await,
            0,
            "nothing is conjured for a claim with no source"
        );

        crate::test_support::teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn hand_over_clears_orphan_rows_sitting_under_the_new_id() {
        let (pool, db_name) = crate::test_support::setup_test_db().await;
        let (bus, _callback_rx) = EventBus::new(pool.clone());
        seed_device_state(&pool, &bus, "old").await;
        sqlx::query("INSERT INTO preferences (key, value, device_id) VALUES ($1, '75', 'new')")
            .bind(prefs::UI_SCALE.key())
            .execute(&pool)
            .await
            .unwrap();

        let outcome = DeviceStore::hand_over(&pool, &bus, "old", "new", None)
            .await
            .unwrap();
        assert_eq!(outcome, HandOver::Moved);
        let value: String = sqlx::query_scalar(
            "SELECT value FROM preferences WHERE key = $1 AND device_id = 'new'",
        )
        .bind(prefs::UI_SCALE.key())
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(value, "125", "the real device's value wins over the orphan");

        crate::test_support::teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn hand_over_to_the_same_id_is_a_no_op() {
        let (pool, db_name) = crate::test_support::setup_test_db().await;
        let (bus, _callback_rx) = EventBus::new(pool.clone());
        seed_device_state(&pool, &bus, "same").await;

        let outcome = DeviceStore::hand_over(&pool, &bus, "same", "same", None)
            .await
            .unwrap();
        assert_eq!(outcome, HandOver::AlreadyDone);
        assert_eq!(rows_for(&pool, "devices", "same").await, 1);

        crate::test_support::teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn list_stale_push_enabled_filters_by_age_and_push_state() {
        let (pool, db_name) = crate::test_support::setup_test_db().await;
        let (bus, _callback_rx) = EventBus::new(pool.clone());

        DeviceStore::register(&pool, &bus, "old-on", Some("UA"), None)
            .await
            .unwrap();
        DeviceStore::set_push_enabled(&pool, &bus, "old-on", true, None)
            .await
            .unwrap();
        backdate_last_seen(&pool, "old-on", 45).await;

        DeviceStore::register(&pool, &bus, "fresh-on", Some("UA"), None)
            .await
            .unwrap();
        DeviceStore::set_push_enabled(&pool, &bus, "fresh-on", true, None)
            .await
            .unwrap();

        DeviceStore::register(&pool, &bus, "old-off", Some("UA"), None)
            .await
            .unwrap();
        backdate_last_seen(&pool, "old-off", 45).await;

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

    async fn age_device(pool: &PgPool, id: &str, created_hours_ago: i32, seen_hours_ago: i32) {
        sqlx::query(
            "UPDATE devices SET created_at = now() - make_interval(hours => $2), \
             last_seen_at = now() - make_interval(hours => $3) WHERE id = $1",
        )
        .bind(id)
        .bind(created_hours_ago)
        .bind(seen_hours_ago)
        .execute(pool)
        .await
        .unwrap();
    }

    const MONTH_HOURS: i32 = 30 * 24;
    const DESKTOP_SAFARI: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) \
        AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.0 Safari/605.1.15";

    async fn seed_one_off(pool: &PgPool, bus: &EventBus, id: &str) {
        DeviceStore::register(pool, bus, id, Some(DESKTOP_SAFARI), None)
            .await
            .unwrap();
        age_device(pool, id, MONTH_HOURS, MONTH_HOURS - 20).await;
    }

    #[tokio::test]
    async fn remove_one_off_takes_only_unclaimed_devices_never_seen_after_their_first_day() {
        let (pool, db_name) = crate::test_support::setup_test_db().await;
        let (bus, _callback_rx) = EventBus::new(pool.clone());

        seed_one_off(&pool, &bus, "one-off").await;

        seed_one_off(&pool, &bus, "came-back").await;
        age_device(&pool, "came-back", MONTH_HOURS, MONTH_HOURS - 24).await;

        seed_one_off(&pool, &bus, "young").await;
        age_device(&pool, "young", 6 * 24, 6 * 24).await;

        seed_one_off(&pool, &bus, "visible-now").await;
        DevicePresenceStore::record_visible(&pool, "visible-now")
            .await
            .unwrap();

        seed_one_off(&pool, &bus, "named").await;
        DeviceStore::rename(&pool, &bus, "named", Some("My MacBook"), None)
            .await
            .unwrap();

        DeviceStore::register(&pool, &bus, "paired", None, None)
            .await
            .unwrap();
        gateway_devices::seed_for_test("paired", None, Some("Desk Mac"));
        age_device(&pool, "paired", MONTH_HOURS, MONTH_HOURS).await;

        seed_one_off(&pool, &bus, "held").await;
        sqlx::query("INSERT INTO device_name_imports (device_id, name) VALUES ('held', 'Old Mac')")
            .execute(&pool)
            .await
            .unwrap();

        seed_one_off(&pool, &bus, "push-on").await;
        DeviceStore::set_push_enabled(&pool, &bus, "push-on", true, None)
            .await
            .unwrap();

        seed_one_off(&pool, &bus, "push-sub").await;
        sqlx::query(
            "INSERT INTO push_subscriptions (endpoint, p256dh, auth, device_id)
             VALUES ('https://push.example/one-off', 'k', $1, 'push-sub')",
        )
        .bind(crate::core::secret_store::Secret::<
            crate::core::secret_store::PushAuth,
        >::new("a"))
        .execute(&pool)
        .await
        .unwrap();

        let removed = DeviceStore::remove_one_off(&pool, &bus, 7).await.unwrap();
        assert_eq!(removed, vec!["one-off".to_string()]);
        for kept in [
            "came-back",
            "young",
            "visible-now",
            "named",
            "paired",
            "held",
            "push-on",
            "push-sub",
        ] {
            assert_eq!(
                rows_for(&pool, "devices", kept).await,
                1,
                "{kept} must stay"
            );
        }

        crate::test_support::teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn remove_one_off_announces_once_and_clears_the_device_state() {
        let (pool, db_name) = crate::test_support::setup_test_db().await;
        let (bus, _callback_rx) = EventBus::new(pool.clone());
        seed_one_off(&pool, &bus, "one-off").await;
        PinnedAppStore::pin(&pool, &bus, "habit-tracker", "main", "one-off", None)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO preferences (key, value, device_id) VALUES ($1, '125', 'one-off')",
        )
        .bind(prefs::UI_SCALE.key())
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO device_presence (device_id, visible_at) \
             VALUES ('one-off', now() - make_interval(hours => $1))",
        )
        .bind(MONTH_HOURS)
        .execute(&pool)
        .await
        .unwrap();

        let removed = DeviceStore::remove_one_off(&pool, &bus, 7).await.unwrap();
        assert_eq!(removed, vec!["one-off".to_string()]);
        for table in ["devices", "preferences", "pinned_apps", "device_presence"] {
            assert_eq!(
                rows_for(&pool, table, "one-off").await,
                0,
                "{table} must keep nothing for a removed device"
            );
        }
        let deleted: Vec<serde_json::Value> = sqlx::query_scalar(
            "SELECT payload->'data' FROM events WHERE event_type = 'DeviceDeleted'",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(deleted.len(), 1, "one announcement per removal");
        assert_eq!(deleted[0]["device_id"], "one-off");
        assert!(
            deleted[0].get("actor").is_none(),
            "an engine sweep names no actor: {}",
            deleted[0]
        );

        let again = DeviceStore::remove_one_off(&pool, &bus, 7).await.unwrap();
        assert!(again.is_empty(), "a re-run removes nothing");
        let (_, inserted) =
            DeviceStore::register(&pool, &bus, "one-off", Some(DESKTOP_SAFARI), None)
                .await
                .unwrap();
        assert!(inserted, "a returning device registers again as new");

        crate::test_support::teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn delete_clears_every_per_device_table() {
        let (pool, db_name) = crate::test_support::setup_test_db().await;
        let (bus, _callback_rx) = EventBus::new(pool.clone());
        seed_device_state(&pool, &bus, "d1").await;

        assert!(DeviceStore::delete(&pool, &bus, "d1", None).await.unwrap());
        for table in DEVICE_TABLES {
            assert_eq!(
                rows_for(&pool, table, "d1").await,
                0,
                "{table} must keep nothing for a deleted device"
            );
        }

        crate::test_support::teardown_test_db(&db_name).await;
    }
}
