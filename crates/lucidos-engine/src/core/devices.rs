use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;

use super::{DevicePresenceStore, PinnedAppStore};
use crate::engine::event_bus::{BusEvent, EventBus, SystemEvent};
use crate::engine::thread_events::MessageOrigin;

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct Device {
    pub id: String,
    /// The name someone typed on the Devices row.
    pub name: Option<String>,
    /// The name the device was given when it paired, as the workspace gateway
    /// forwards it. `None` for a client that never came through a gateway.
    pub pairing_label: Option<String>,
    pub user_agent: Option<String>,
    pub push_enabled: bool,
    pub last_seen_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
}

/// A device as the agent context lists it. See [`DeviceStore::recently_seen`].
#[derive(Debug, Clone, PartialEq)]
pub struct SeenDevice {
    pub id: String,
    /// The name Settings → Devices shows.
    pub label: String,
    /// Browser and OS, parsed from the user agent.
    pub details: Option<String>,
    pub seen_secs_ago: i64,
    /// A visible heartbeat within `PRESENCE_STALE_AFTER`.
    pub visible_now: bool,
}

/// "Seen" is the newer of the last page load (`last_seen_at`) and the last
/// visible heartbeat (`device_presence.visible_at`). `$1` is the presence
/// window in seconds. Ages use the database clock (ADR 0053).
const SEEN_DEVICE_SELECT: &str = "SELECT d.id, d.name, d.pairing_label, d.user_agent, \
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
    name: Option<String>,
    pairing_label: Option<String>,
    user_agent: Option<String>,
    seen_secs_ago: i64,
    visible_now: bool,
}

impl From<SeenDeviceRow> for SeenDevice {
    fn from(row: SeenDeviceRow) -> Self {
        Self {
            label: friendly_device_name(
                row.name.as_deref(),
                row.pairing_label.as_deref(),
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

/// What a hand-over did, so the caller can answer without guessing.
///
/// `AlreadyDone` and `NoSuchDevice` are both ordinary outcomes rather than
/// errors. A client retries the call on a later load, and by then the first
/// attempt has usually landed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum HandOver {
    /// Every row moved from the old id to the new one.
    Moved,
    /// The new id already has a row, so there is nothing to move onto it.
    AlreadyDone,
    /// The old id has no row. Nothing to hand over.
    NoSuchDevice,
}

/// The registry of devices that have connected to this workspace.
///
/// **No caller can skip the event.** [`Self::register`], [`Self::rename`],
/// [`Self::set_push_enabled`], [`Self::delete`], [`Self::remove_one_off`] and
/// [`Self::hand_over`] are the only reachable mutators; the raw row writes are
/// private to this module. `Device{Registered,Renamed,PushChanged,Deleted,
/// HandedOver}` is what reloads the Settings devices list on every other device.
///
/// Same shape as `RepositoryStore`; see `core::announced_surfaces`.
pub struct DeviceStore;

impl DeviceStore {
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
        pairing_label: Option<&str>,
    ) -> Result<(Device, bool), Box<dyn std::error::Error + Send + Sync>> {
        #[derive(sqlx::FromRow)]
        struct DeviceWithInsertFlag {
            #[sqlx(flatten)]
            device: Device,
            inserted: bool,
        }

        let row: DeviceWithInsertFlag = sqlx::query_as(
            "INSERT INTO devices (id, user_agent, pairing_label, last_seen_at)
             VALUES ($1, $2, $3, NOW())
             ON CONFLICT (id) DO UPDATE SET
                user_agent = COALESCE($2, devices.user_agent),
                pairing_label = COALESCE($3, devices.pairing_label),
                last_seen_at = NOW()
             RETURNING id, name, pairing_label, user_agent, push_enabled, last_seen_at, created_at, (xmax = 0) AS inserted",
        )
        .bind(id)
        .bind(user_agent)
        .bind(pairing_label)
        .fetch_one(pool)
        .await?;
        Ok((row.device, row.inserted))
    }

    /// Does a row exist for this device id?
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

    /// What to call a device, read from its row. See [`friendly_device_name`].
    ///
    /// Always answers. A device with no row, or a failed read, gets the short-id
    /// name, since a label is display metadata and must never fail an action.
    pub async fn friendly_name(pool: &PgPool, id: &str) -> String {
        let row: Option<(Option<String>, Option<String>, Option<String>)> = match sqlx::query_as(
            "SELECT name, pairing_label, user_agent FROM devices WHERE id = $1",
        )
        .bind(id)
        .fetch_optional(pool)
        .await
        {
            Ok(r) => r,
            Err(e) => {
                log!("[Devices] friendly_name({}) failed: {}", id, e);
                None
            }
        };
        let (name, pairing_label, user_agent) = row.unwrap_or_default();
        friendly_device_name(
            name.as_deref(),
            pairing_label.as_deref(),
            user_agent.as_deref(),
            id,
        )
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
        let name = friendly_device_name(
            device.name.as_deref(),
            device.pairing_label.as_deref(),
            device.user_agent.as_deref(),
            id,
        );
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

    /// The devices seen most recently, newest first, at most `limit` of them
    /// and none unseen for `within_days`. The agent context lists these.
    /// See [`SEEN_DEVICE_SELECT`] for what "seen" means.
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
        Ok(rows.into_iter().map(SeenDevice::from).collect())
    }

    /// One device, seen the same way as [`Self::recently_seen`]. `None` when
    /// no such device exists.
    pub async fn seen(pool: &PgPool, id: &str) -> Result<Option<SeenDevice>, sqlx::Error> {
        let row: Option<SeenDeviceRow> =
            sqlx::query_as(&format!("{SEEN_DEVICE_SELECT} WHERE d.id = $2"))
                .bind(presence_window_secs())
                .bind(id)
                .fetch_optional(pool)
                .await?;
        Ok(row.map(SeenDevice::from))
    }

    /// Record that a device was seen now. Called when a device hides Lucidos:
    /// the hide deletes its presence row, and without this stamp its age would
    /// fall back to its last page load.
    pub async fn mark_seen(pool: &PgPool, id: &str) -> Result<(), sqlx::Error> {
        sqlx::query("UPDATE devices SET last_seen_at = NOW() WHERE id = $1")
            .bind(id)
            .execute(pool)
            .await?;
        Ok(())
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

    /// Delete a device and everything scoped to it, inside the caller's
    /// transaction. **Private on purpose**: [`Self::delete`] and
    /// [`Self::remove_one_off`] emit.
    ///
    /// The cascade (per-device preferences, push subscriptions, pinned apps,
    /// presence) is deliberately silent. `DeviceDeleted` is the announcement for
    /// all of it: the device is gone, so a `PreferencesChanged` or
    /// `PinnedAppUnpinned` per row would describe changes to a device no client
    /// still tracks.
    ///
    /// The children go first: `push_subscriptions.device_id` is a foreign key
    /// onto `devices(id)`.
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

    /// Delete one device and its per-device state in one transaction.
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
    /// `pairing_label` is stored when present and kept when absent, so a
    /// request that did not come through the gateway never erases it.
    ///
    /// `DeviceRegistered` fires only on a genuinely new device, never on the
    /// last-seen-at refresh every page load performs.
    pub async fn register(
        pool: &PgPool,
        event_bus: &EventBus,
        id: &str,
        user_agent: Option<&str>,
        pairing_label: Option<&str>,
        actor: Option<MessageOrigin>,
    ) -> Result<(Device, bool), Box<dyn std::error::Error + Send + Sync>> {
        let (device, inserted) = Self::upsert_row(pool, id, user_agent, pairing_label).await?;
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

    /// Remove every *one-off device* and announce each removal. Returns the
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
        let mut tx = pool.begin().await?;
        let ids: Vec<String> = sqlx::query_scalar(
            "SELECT d.id FROM devices d
             LEFT JOIN device_presence p ON p.device_id = d.id
             WHERE d.created_at < now() - make_interval(days => $1)
               AND GREATEST(d.last_seen_at, p.visible_at) < d.created_at + interval '1 day'
               AND btrim(COALESCE(d.name, '')) = ''
               AND d.pairing_label IS NULL
               AND NOT d.push_enabled
               AND NOT EXISTS (SELECT 1 FROM push_subscriptions s WHERE s.device_id = d.id)
             ORDER BY d.created_at
             FOR UPDATE OF d SKIP LOCKED",
        )
        .bind(older_than_days)
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

    /// Move every trace of `old_id` onto `new_id`, and announce the result.
    ///
    /// The one migration path off the `localStorage` device id. A browser now
    /// takes its id from the *workspace gateway*. Without this it arrives as a
    /// new device and loses its push subscription and its preferences. It knows
    /// both ids for one page load, which is the window this uses. Rationale in
    /// `docs/plans/2026-08-22-one-device-identity-minted-at-the-gateway.md`.
    ///
    /// Order is load-bearing. `push_subscriptions.device_id` is a foreign key
    /// onto `devices(id)` with no `ON UPDATE CASCADE`, so the parent key cannot
    /// be renamed in place. Copy the row forward, repoint the children, drop
    /// the old row last.
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

        // This check is a fast path, NOT the guarantee. Under READ COMMITTED a
        // `SELECT EXISTS` takes no lock on a row that is not there. So a
        // concurrent `register(new_id)` can still commit before the insert
        // below. What actually holds is the `devices` primary key: the insert
        // blocks, then fails, and the transaction aborts to a 500 the client
        // retries on its next load.
        let new_exists: bool =
            sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM devices WHERE id = $1)")
                .bind(new_id)
                .fetch_one(&mut *tx)
                .await?;
        if new_exists {
            return Ok(HandOver::AlreadyDone);
        }
        let copied = sqlx::query(
            "INSERT INTO devices (id, name, pairing_label, user_agent, push_enabled, last_seen_at, created_at)
             SELECT $2, name, pairing_label, user_agent, push_enabled, last_seen_at, created_at
             FROM devices WHERE id = $1",
        )
        .bind(old_id)
        .bind(new_id)
        .execute(&mut *tx)
        .await?;
        if copied.rows_affected() == 0 {
            return Ok(HandOver::NoSuchDevice);
        }

        // `preferences` carries no foreign key, so a client that wrote one
        // before registering can leave rows under an id with no device. They
        // belong to a device that does not exist, so they are cleared rather
        // than merged: `(key, COALESCE(device_id, ''))` would collide.
        sqlx::query("DELETE FROM preferences WHERE device_id = $1")
            .bind(new_id)
            .execute(&mut *tx)
            .await?;
        sqlx::query("UPDATE preferences SET device_id = $2 WHERE device_id = $1")
            .bind(old_id)
            .bind(new_id)
            .execute(&mut *tx)
            .await?;
        // Its foreign key is why the parent row was copied first: there can be
        // no orphan here to clear.
        sqlx::query("UPDATE push_subscriptions SET device_id = $2 WHERE device_id = $1")
            .bind(old_id)
            .bind(new_id)
            .execute(&mut *tx)
            .await?;
        // Through their owning stores, matching what `delete_row` does with its
        // own cascade. `core::announced_surfaces` is the reason: this module owns
        // neither table. A module writing another's table behind its back is
        // exactly what that registry exists to refuse.
        PinnedAppStore::move_device(&mut tx, old_id, new_id).await?;
        DevicePresenceStore::move_device(&mut tx, old_id, new_id).await?;

        sqlx::query("DELETE FROM devices WHERE id = $1")
            .bind(old_id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;

        // One event for the whole move, matching `delete`'s cascade rule. The
        // per-row changes describe a device no client tracks any more.
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

/// What to call a device: the one rule every engine surface names it by.
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

/// A name read off a device's stored user-agent, for a device nobody named.
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
    use crate::core::prefs;

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

    /// A device nobody named and nothing paired is called by what its
    /// user-agent says, with the short id so two alike stay apart.
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

    /// A paired device nobody renamed is called by its pairing label, and a
    /// typed name still wins over it.
    #[tokio::test]
    async fn friendly_name_uses_the_pairing_label_and_keeps_it_and_the_typed_name() {
        let (pool, db_name) = crate::test_support::setup_test_db().await;
        let (bus, _callback_rx) = EventBus::new(pool.clone());

        DeviceStore::register(
            &pool,
            &bus,
            "phone-1",
            Some("UA"),
            Some("Safari on iPhone"),
            None,
        )
        .await
        .unwrap();
        assert_eq!(
            DeviceStore::friendly_name(&pool, "phone-1").await,
            "Safari on iPhone"
        );

        // A later request that did not come through the gateway carries no label.
        DeviceStore::register(&pool, &bus, "phone-1", Some("UA"), None, None)
            .await
            .unwrap();
        assert_eq!(
            DeviceStore::friendly_name(&pool, "phone-1").await,
            "Safari on iPhone",
            "a request without the label must not erase it"
        );

        DeviceStore::rename(&pool, &bus, "phone-1", Some("Work phone"), None)
            .await
            .unwrap();
        DeviceStore::register(
            &pool,
            &bus,
            "phone-1",
            Some("UA"),
            Some("Safari on iPhone"),
            None,
        )
        .await
        .unwrap();
        assert_eq!(
            DeviceStore::friendly_name(&pool, "phone-1").await,
            "Work phone",
            "the typed name wins over the pairing label"
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

    /// The agent's Known devices list reads the two records the engine already
    /// keeps: the page-load `last_seen_at` and the visible heartbeat. The newer
    /// one orders the list, and a device unseen past the window is left out.
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
        // The laptop has not reloaded in two days, but it is showing Lucidos.
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

    /// Hiding Lucidos deletes the presence row. The device was still seen at
    /// that moment, so its age must not fall back to a page load days ago.
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

        let (_, inserted) = DeviceStore::register(&pool, &bus, "d1", Some("UA"), None, None)
            .await
            .unwrap();
        assert!(inserted);
        assert_eq!(emitted(&pool, "DeviceRegistered").await, 1);

        let (_, inserted) = DeviceStore::register(&pool, &bus, "d1", Some("UA"), None, None)
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

        DeviceStore::register(&pool, &bus, "d1", Some("UA"), None, None)
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

    /// Seed one row in every table the hand-over must move. A table added to
    /// the schema and forgotten here shows up as a surviving old-id row.
    async fn seed_device_state(pool: &PgPool, bus: &EventBus, id: &str) {
        DeviceStore::register(pool, bus, id, Some("UA"), Some("Safari on iPhone"), None)
            .await
            .unwrap();
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
             VALUES ('https://push.example/sub', 'k', 'a', $1)",
        )
        .bind(id)
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

    /// The whole point of the migration: a browser that changed id keeps its
    /// push subscription, its name and its device-scoped preferences.
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
        let device: Device = sqlx::query_as("SELECT * FROM devices WHERE id = 'new'")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(device.name.as_deref(), Some("My iPhone"));
        assert_eq!(device.pairing_label.as_deref(), Some("Safari on iPhone"));
        assert!(device.push_enabled, "the push flag rides across");

        crate::test_support::teardown_test_db(&db_name).await;
    }

    /// One event for the whole move, matching the `DeviceDeleted` cascade rule.
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

    /// Replaying the claim must not move a second device onto the same id. The
    /// client retries on a later load, so this is an ordinary path.
    #[tokio::test]
    async fn hand_over_refuses_once_the_new_id_has_a_row() {
        let (pool, db_name) = crate::test_support::setup_test_db().await;
        let (bus, _callback_rx) = EventBus::new(pool.clone());
        seed_device_state(&pool, &bus, "old").await;
        DeviceStore::register(&pool, &bus, "new", Some("UA"), None, None)
            .await
            .unwrap();

        let outcome = DeviceStore::hand_over(&pool, &bus, "old", "new", None)
            .await
            .unwrap();
        assert_eq!(outcome, HandOver::AlreadyDone);
        assert_eq!(
            rows_for(&pool, "devices", "old").await,
            1,
            "a refused hand-over changes nothing"
        );
        assert_eq!(rows_for(&pool, "push_subscriptions", "old").await, 1);

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

    /// An orphan preference under the target id would collide with the
    /// `(key, COALESCE(device_id, ''))` unique index, aborting the whole move.
    /// It belongs to a device that does not exist, so it is cleared.
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

        // Push-enabled + old → returned
        DeviceStore::register(&pool, &bus, "old-on", Some("UA"), None, None)
            .await
            .unwrap();
        DeviceStore::set_push_enabled(&pool, &bus, "old-on", true, None)
            .await
            .unwrap();
        backdate_last_seen(&pool, "old-on", 45).await;

        // Push-enabled + recent → excluded (last_seen is today)
        DeviceStore::register(&pool, &bus, "fresh-on", Some("UA"), None, None)
            .await
            .unwrap();
        DeviceStore::set_push_enabled(&pool, &bus, "fresh-on", true, None)
            .await
            .unwrap();

        // Push-disabled + old → excluded (filtered at SELECT to avoid no-op events)
        DeviceStore::register(&pool, &bus, "old-off", Some("UA"), None, None)
            .await
            .unwrap();
        backdate_last_seen(&pool, "old-off", 45).await;

        // One day short of the 30-day cutoff (29 days) → excluded
        DeviceStore::register(&pool, &bus, "almost-on", Some("UA"), None, None)
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
        DeviceStore::register(&pool, &bus, "fresh", Some("UA"), None, None)
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

    /// Ages a device: created `created_hours_ago`, last seen `seen_hours_ago`.
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

    /// Registers a device that was used for 20 hours a month ago, then never again.
    async fn seed_one_off(pool: &PgPool, bus: &EventBus, id: &str) {
        DeviceStore::register(pool, bus, id, Some(DESKTOP_SAFARI), None, None)
            .await
            .unwrap();
        age_device(pool, id, MONTH_HOURS, MONTH_HOURS - 20).await;
    }

    /// Only a device never seen past its first day, old enough, and holding no
    /// name, pairing or push is removed. The user agent plays no part: the
    /// removed row claims to be desktop Safari.
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

        DeviceStore::register(&pool, &bus, "paired", None, Some("My iPhone"), None)
            .await
            .unwrap();
        age_device(&pool, "paired", MONTH_HOURS, MONTH_HOURS).await;

        seed_one_off(&pool, &bus, "push-on").await;
        DeviceStore::set_push_enabled(&pool, &bus, "push-on", true, None)
            .await
            .unwrap();

        seed_one_off(&pool, &bus, "push-sub").await;
        sqlx::query(
            "INSERT INTO push_subscriptions (endpoint, p256dh, auth, device_id)
             VALUES ('https://push.example/one-off', 'k', 'a', 'push-sub')",
        )
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

    /// Each removal announces once, unattributed, and takes the device's state
    /// with it. A re-run finds nothing, and the device can still come back.
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
            DeviceStore::register(&pool, &bus, "one-off", Some(DESKTOP_SAFARI), None, None)
                .await
                .unwrap();
        assert!(inserted, "a returning device registers again as new");

        crate::test_support::teardown_test_db(&db_name).await;
    }

    /// The Remove button clears the same state the sweep does, presence
    /// included: `device_presence` has no foreign key to cascade it.
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
