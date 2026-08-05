//! Notification storage

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

use crate::engine::event_bus::{BusEvent, EventBus, SystemEvent};
use crate::engine::thread_events::MessageOrigin;

/// Where a tap on this notification (OS push, in-app toast) should land.
///
/// Discriminated union — `kind` selects the variant. `Modal` (default) opens
/// the inbox modal so the user reads the message and chooses what to do next.
/// `Navigate` delegates to the same target/sub-field router the `navigate_ui`
/// LLM tool uses, so any UI surface reachable by `navigate_ui` is reachable by
/// a notification tap with no per-target wrapper variant. Every notification is
/// openable — there is no passive/button-less kind.
///
/// The old passive `None` kind (`{"kind":"none"}`) is RETIRED: nothing produces
/// it anymore, and the custom `Deserialize` below coerces any historical
/// `{"kind":"none"}` event/row to `Modal` (see that impl for why it can't just
/// be deleted). Removed by `docs/plans/2026-07-02-remove-notification-tap-none.md`.
///
/// Wire shape (JSON):
/// - `{"kind":"modal"}`
/// - `{"kind":"navigate","to":{"target":"app","app_id":"habit-tracker"}}`
///
/// Required sub-fields on `NavigateUi.to` (e.g. `app_id` for `target=app`,
/// `id` for `target=thread`) are validated by the page-side router, not by
/// this Rust type — the LLM tool definition documents them.
///
/// # Strict — no tolerance for the legacy four-string form
///
/// Inbound `Tap` deserialization is strict: only the canonical object form
/// is accepted. The pre-`20260522152123_notification_tap_jsonb.sql` bare
/// strings (`"modal"` / `"open_app"` / `"open_thread"` / `"none"`) are
/// rejected by serde with a clear "missing field 'kind'" error — surfacing
/// as `400 Bad Request` on the HTTP `notifications` POST and as a
/// `send_notification` LLM tool error on bad LLM output. (The retired
/// `{"kind":"none"}` OBJECT is the one exception — coerced to `Modal`, not
/// rejected; the bare string `"none"` is still rejected.)
///
/// Workspaces that still have triggers or apps emitting the old strings
/// must migrate them via `system-knowhow/migrate-tap-shape.md` (detected by
/// `system-knowhow/workspace-audit.md`). Historical `NotificationCreated`
/// event payloads with the old form remain in the events table forever
/// (event-sourcing immutability), but the projection is built once by the
/// JSONB migration and incremental updates only consume new events with the
/// canonical shape — normal operation never re-deserializes the old strings.
/// A force projection rebuild on a pre-migration workspace will fail loudly,
/// at which point the workspace owner runs `migrate-tap-shape.md` and
/// rebuilds.
#[derive(Debug, Clone, Serialize, PartialEq, Eq, Default)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Tap {
    #[default]
    Modal,
    Navigate {
        to: NavigateUi,
    },
}

// Custom Deserialize (not derived) so the RETIRED `{"kind":"none"}` kind coerces
// to `Modal` rather than erroring. `Tap::None` (the old passive kind) was removed
// so every notification is openable — but immutable `NotificationCreated` event
// payloads, and any `notifications` rows a projection rebuild replays, still carry
// `{"kind":"none"}` forever. So the read boundary MUST tolerate it. This is
// PERMANENT old-data tolerance (a parser for a legacy wire shape), NOT a temporary
// measure — so it carries no `docs/temporary-measures.md` row.
//
// It delegates to a private derived helper that keeps serde's full strictness:
// legacy bare strings ("modal"/"none"/"open_thread") and unknown object kinds are
// still rejected loudly (the migrate-tap-shape guard). ONLY the well-formed
// `{"kind":"none"}` object is coerced — to `Modal`, which then re-serializes as
// `{"kind":"modal"}`, so a rebuilt row never re-emits `none`.
impl<'de> Deserialize<'de> for Tap {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(tag = "kind", rename_all = "snake_case")]
        enum TapWire {
            Modal,
            None,
            Navigate { to: NavigateUi },
        }
        Ok(match TapWire::deserialize(deserializer)? {
            TapWire::Modal | TapWire::None => Tap::Modal,
            TapWire::Navigate { to } => Tap::Navigate { to },
        })
    }
}

/// Navigation payload — mirrors the `navigate_ui` LLM tool arg shape. Used
/// as the `to` of `Tap::Navigate`. Required sub-fields are not enforced at
/// the Rust layer; the page-side router rejects malformed nav targets.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct NavigateUi {
    pub target: NavigateTarget,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settings_view: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_path: Option<String>,
    /// First line to select in the file preview, 1-based. Only meaningful with
    /// `target: File`. The page-side router validates and silently ignores a
    /// line it can't use (0, negative, past the end of the file) rather than
    /// refusing to open the file: a citation's line number is the part that
    /// goes stale, and the file is still what the reader asked for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
    /// Last line of the selected range, 1-based and INCLUSIVE. Omit for a
    /// single line. Only meaningful alongside `line`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line_end: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
}

/// Navigation target enum — every UI surface a notification tap can deep-link
/// to. Wire format: kebab-case (e.g. `"new-app"`). Mirrors the `navigate_ui`
/// LLM tool's `target` enum.
///
/// Has a `Default` impl (`Thread`) so callers can use the
/// `NavigateUi { target: …, ..Default::default() }` shorthand. The default
/// target value is rarely the right pick — `handleNavigationRequest` surfaces
/// a `Navigation target missing …` toast at runtime when a navigate-kind tap
/// lacks the required sub-field for whatever target was chosen, which is the
/// failure mode this would have masked.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum NavigateTarget {
    Files,
    Apps,
    AppStore,
    Plugins,
    Triggers,
    ThreadQueue,
    Changes,
    Notifications,
    Settings,
    App,
    File,
    Trigger,
    #[default]
    Thread,
    NewApp,
    NewTrigger,
    NewChat,
    Url,
}

/// A notification sent to the user
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct Notification {
    pub id: Uuid,
    pub task_id: Option<Uuid>,
    pub app_id: Option<String>,
    /// Originating thread, when the notification has one. Drives the inbox
    /// modal's "Open thread" button. Engine sets this from `link_thread` —
    /// the same value that powers push deep-linking and presence-based push
    /// suppression.
    pub thread_id: Option<Uuid>,
    /// Specific event UUID inside `thread_id` to deep-link to. When set, both
    /// the inbox modal's "Open thread" button and a tap with
    /// `Tap::Navigate { to: { target: Thread, .. } }` push scroll and briefly
    /// pulse this event on land. Typically points at the `UserQuestionAsked`
    /// or `CodingAgentPermissionRequest` row the user should answer.
    pub event_id: Option<Uuid>,
    pub title: String,
    pub message: String,
    pub read: bool,
    pub created_at: DateTime<Utc>,
    /// Where a tap lands. Stored as JSONB; see [`Tap`] for the wire shape.
    /// The `Json<Tap>` newtype handles the JSONB encode/decode; the field
    /// reads back as a plain `Tap` to call sites via the `Deref` extraction
    /// below in the `FromRow` decode.
    #[sqlx(json)]
    pub tap: Tap,
}

/// Storage for notifications
pub struct NotificationStore;

impl NotificationStore {
    /// Defensive double-write — the migration owns this CREATE TABLE
    /// (see `20260517160627_consolidate_init_schema_tables.sql`). Slated
    /// for removal in `harden-init-schema-tables-vs-migrations-pattern-finish`.
    /// The `tap` column shape (TEXT → JSONB) is owned by the dedicated
    /// migrations; this defensive create only spins up the bare row schema.
    pub async fn init_schema(pool: &PgPool) -> Result<(), sqlx::Error> {
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS notifications (
                id UUID PRIMARY KEY,
                task_id UUID,
                app_id TEXT,
                thread_id UUID,
                event_id UUID,
                title TEXT NOT NULL,
                message TEXT NOT NULL,
                read BOOLEAN DEFAULT false,
                created_at TIMESTAMPTZ DEFAULT NOW()
            )
            "#,
        )
        .execute(pool)
        .await?;

        // Index for efficient unread queries
        sqlx::query(
            r#"
            CREATE INDEX IF NOT EXISTS idx_notifications_unread
            ON notifications (read, created_at DESC)
            "#,
        )
        .execute(pool)
        .await?;

        // Standalone created_at index for "all" filter cursor pagination
        sqlx::query(
            r#"
            CREATE INDEX IF NOT EXISTS idx_notifications_created_at
            ON notifications (created_at DESC)
            "#,
        )
        .execute(pool)
        .await?;

        Ok(())
    }

    /// Insert a new notification
    #[allow(clippy::too_many_arguments)]
    pub async fn insert(
        pool: &PgPool,
        title: &str,
        message: &str,
        task_id: Option<Uuid>,
        app_id: Option<&str>,
        thread_id: Option<Uuid>,
        event_id: Option<Uuid>,
        tap: Tap,
    ) -> Result<Notification, sqlx::Error> {
        Self::insert_with_timestamp(
            pool,
            title,
            message,
            task_id,
            app_id,
            thread_id,
            event_id,
            tap,
            Utc::now(),
        )
        .await
    }

    /// Insert a notification with a custom timestamp (for backdating)
    // One arg per persisted column; matches the `notifications` row schema 1:1.
    #[allow(clippy::too_many_arguments)]
    pub async fn insert_with_timestamp(
        pool: &PgPool,
        title: &str,
        message: &str,
        task_id: Option<Uuid>,
        app_id: Option<&str>,
        thread_id: Option<Uuid>,
        event_id: Option<Uuid>,
        tap: Tap,
        created_at: DateTime<Utc>,
    ) -> Result<Notification, sqlx::Error> {
        let id = Uuid::new_v4();

        sqlx::query(
            r#"
            INSERT INTO notifications (id, task_id, app_id, thread_id, event_id, title, message, read, created_at, tap)
            VALUES ($1, $2, $3, $4, $5, $6, $7, false, $8, $9)
            "#,
        )
        .bind(id)
        .bind(task_id)
        .bind(app_id)
        .bind(thread_id)
        .bind(event_id)
        .bind(title)
        .bind(message)
        .bind(created_at)
        .bind(sqlx::types::Json(&tap))
        .execute(pool)
        .await?;

        Ok(Notification {
            id,
            task_id,
            app_id: app_id.map(|s| s.to_string()),
            thread_id,
            event_id,
            title: title.to_string(),
            message: message.to_string(),
            read: false,
            created_at,
            tap,
        })
    }

    /// Count unread notifications (no cap — returns the real total)
    pub async fn count_unread(pool: &PgPool) -> Result<i64, sqlx::Error> {
        let (count,): (i64,) =
            sqlx::query_as("SELECT COUNT(*) FROM notifications WHERE read = false")
                .fetch_one(pool)
                .await?;
        Ok(count)
    }

    /// Get notifications with optional filter and cursor pagination.
    ///
    /// `filter` — `"unread"` returns only unread; anything else returns all.
    /// `before_ts` — cursor: only rows with `created_at < $ts` (for infinite scroll).
    /// `limit` — max rows to return.
    pub async fn get_filtered(
        pool: &PgPool,
        filter: &str,
        limit: i64,
        before_ts: Option<DateTime<Utc>>,
    ) -> Result<Vec<Notification>, sqlx::Error> {
        let unread_only = filter == "unread";

        match (unread_only, before_ts) {
            (true, Some(ts)) => {
                sqlx::query_as::<_, Notification>(
                    r#"
                    SELECT id, task_id, app_id, thread_id, event_id, title, message, read, created_at, tap
                    FROM notifications
                    WHERE read = false AND created_at < $1
                    ORDER BY created_at DESC
                    LIMIT $2
                    "#,
                )
                .bind(ts)
                .bind(limit)
                .fetch_all(pool)
                .await
            }
            (true, None) => {
                sqlx::query_as::<_, Notification>(
                    r#"
                    SELECT id, task_id, app_id, thread_id, event_id, title, message, read, created_at, tap
                    FROM notifications
                    WHERE read = false
                    ORDER BY created_at DESC
                    LIMIT $1
                    "#,
                )
                .bind(limit)
                .fetch_all(pool)
                .await
            }
            (false, Some(ts)) => {
                sqlx::query_as::<_, Notification>(
                    r#"
                    SELECT id, task_id, app_id, thread_id, event_id, title, message, read, created_at, tap
                    FROM notifications
                    WHERE created_at < $1
                    ORDER BY created_at DESC
                    LIMIT $2
                    "#,
                )
                .bind(ts)
                .bind(limit)
                .fetch_all(pool)
                .await
            }
            (false, None) => {
                sqlx::query_as::<_, Notification>(
                    r#"
                    SELECT id, task_id, app_id, thread_id, event_id, title, message, read, created_at, tap
                    FROM notifications
                    ORDER BY created_at DESC
                    LIMIT $1
                    "#,
                )
                .bind(limit)
                .fetch_all(pool)
                .await
            }
        }
    }

    /// Get notifications created before a specific timestamp (for time travel)
    pub async fn get_all_before(
        pool: &PgPool,
        before: DateTime<Utc>,
        limit: i64,
    ) -> Result<Vec<Notification>, sqlx::Error> {
        sqlx::query_as::<_, Notification>(
            r#"
            SELECT id, task_id, app_id, thread_id, event_id, title, message, read, created_at, tap
            FROM notifications
            WHERE created_at <= $1
            ORDER BY created_at DESC
            LIMIT $2
            "#,
        )
        .bind(before)
        .bind(limit)
        .fetch_all(pool)
        .await
    }

    /// Flip one notification to read. Returns `true` only when the row moved
    /// from unread to read.
    ///
    /// **Private on purpose**: [`Self::mark_read`] is the reachable mutator,
    /// and it emits.
    async fn mark_read_row(pool: &PgPool, notification_id: Uuid) -> Result<bool, sqlx::Error> {
        let result = sqlx::query(
            r#"
            UPDATE notifications
            SET read = true
            WHERE id = $1 AND read = false
            "#,
        )
        .bind(notification_id)
        .execute(pool)
        .await?;

        Ok(result.rows_affected() > 0)
    }

    /// Flip every unread notification to read, returning how many moved.
    ///
    /// **Private on purpose**: [`Self::mark_all_read`] is the reachable
    /// mutator, and it emits.
    async fn mark_all_read_rows(pool: &PgPool) -> Result<u64, sqlx::Error> {
        let result = sqlx::query(
            r#"
            UPDATE notifications
            SET read = true
            WHERE read = false
            "#,
        )
        .execute(pool)
        .await?;

        Ok(result.rows_affected())
    }

    /// Mark a notification read and announce it. The only way to flip one.
    ///
    /// `NotificationRead` fires only when the row really moved from unread to
    /// read, so a redundant write (the service worker POSTs read on tap AND the
    /// in-app dispatch also marks read) cannot fan out a duplicate SSE frame.
    pub async fn mark_read(
        pool: &PgPool,
        event_bus: &EventBus,
        notification_id: Uuid,
        actor: Option<MessageOrigin>,
    ) -> Result<bool, sqlx::Error> {
        let marked = Self::mark_read_row(pool, notification_id).await?;
        if marked {
            event_bus
                .emit_or_log(
                    BusEvent::System(SystemEvent::NotificationRead {
                        id: notification_id.to_string(),
                        actor,
                    }),
                    "[Notifications] NotificationRead",
                )
                .await;
        }
        Ok(marked)
    }

    /// Mark every unread notification read and announce it once. Returns how
    /// many moved; announces nothing when the inbox was already clear.
    pub async fn mark_all_read(
        pool: &PgPool,
        event_bus: &EventBus,
        actor: Option<MessageOrigin>,
    ) -> Result<u64, sqlx::Error> {
        let count = Self::mark_all_read_rows(pool).await?;
        if count > 0 {
            event_bus
                .emit_or_log(
                    BusEvent::System(SystemEvent::NotificationsAllRead { actor }),
                    "[Notifications] NotificationsAllRead",
                )
                .await;
        }
        Ok(count)
    }

    /// Get notification by ID
    pub async fn get_by_id(
        pool: &PgPool,
        notification_id: Uuid,
    ) -> Result<Option<Notification>, sqlx::Error> {
        sqlx::query_as::<_, Notification>(
            r#"
            SELECT id, task_id, app_id, thread_id, event_id, title, message, read, created_at, tap
            FROM notifications
            WHERE id = $1
            "#,
        )
        .bind(notification_id)
        .fetch_optional(pool)
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tap_modal_serializes_with_kind_only() {
        let v = serde_json::to_value(Tap::Modal).unwrap();
        assert_eq!(v, serde_json::json!({"kind": "modal"}));
    }

    #[test]
    fn tap_navigate_thread_with_id_and_event_id() {
        let t = Tap::Navigate {
            to: NavigateUi {
                target: NavigateTarget::Thread,
                id: Some("11111111-2222-3333-4444-555555555555".into()),
                event_id: Some("aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee".into()),
                ..Default::default()
            },
        };
        let v = serde_json::to_value(&t).unwrap();
        assert_eq!(v["kind"], "navigate");
        assert_eq!(v["to"]["target"], "thread");
        assert_eq!(v["to"]["id"], "11111111-2222-3333-4444-555555555555");
        assert_eq!(v["to"]["event_id"], "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee");
        assert!(v["to"].get("settings_view").is_none());
        assert!(v["to"].get("file_path").is_none());
    }

    #[test]
    fn tap_navigate_app_uses_app_id() {
        let t = Tap::Navigate {
            to: NavigateUi {
                target: NavigateTarget::App,
                app_id: Some("habit-tracker".into()),
                ..Default::default()
            },
        };
        let v = serde_json::to_value(&t).unwrap();
        assert_eq!(v["kind"], "navigate");
        assert_eq!(v["to"]["target"], "app");
        assert_eq!(v["to"]["app_id"], "habit-tracker");
    }

    #[test]
    fn tap_navigate_kebab_case_targets() {
        // new-app / new-trigger / new-chat / thread-queue all serialize
        // kebab-case on the wire.
        for (target, wire) in [
            (NavigateTarget::NewApp, "new-app"),
            (NavigateTarget::NewTrigger, "new-trigger"),
            (NavigateTarget::NewChat, "new-chat"),
            (NavigateTarget::ThreadQueue, "thread-queue"),
        ] {
            let v = serde_json::to_value(target).unwrap();
            assert_eq!(v, serde_json::Value::String(wire.into()));
        }
    }

    #[test]
    fn tap_deserialize_modal() {
        let t: Tap = serde_json::from_value(serde_json::json!({"kind": "modal"})).unwrap();
        assert_eq!(t, Tap::Modal);
    }

    #[test]
    fn tap_deserialize_none_coerces_to_modal() {
        // The retired `{"kind":"none"}` kind is no longer produced, but historical
        // NotificationCreated events + notifications rows still carry it. It must
        // deserialize (as Modal) and re-serialize as `{"kind":"modal"}`, so a
        // projection rebuild never re-emits `none`.
        let t: Tap = serde_json::from_value(serde_json::json!({"kind": "none"})).unwrap();
        assert_eq!(t, Tap::Modal);
        assert_eq!(
            serde_json::to_value(&t).unwrap(),
            serde_json::json!({"kind": "modal"})
        );
    }

    #[test]
    fn tap_deserialize_navigate() {
        let v = serde_json::json!({
            "kind": "navigate",
            "to": {"target": "thread", "id": "abc"}
        });
        let t: Tap = serde_json::from_value(v).unwrap();
        match t {
            Tap::Navigate { to } => {
                assert_eq!(to.target, NavigateTarget::Thread);
                assert_eq!(to.id.as_deref(), Some("abc"));
            }
            _ => panic!("expected Navigate"),
        }
    }

    // Strict deserialization — only the canonical {kind, to?} object form is
    // accepted. Legacy bare strings ('modal' / 'none' / 'open_app' /
    // 'open_thread') are rejected; workspaces with stale taps must migrate
    // via system-knowhow/migrate-tap-shape.md.

    #[test]
    fn tap_deserializes_canonical_modal_object() {
        let v: Tap = serde_json::from_str(r#"{"kind":"modal"}"#).unwrap();
        assert_eq!(v, Tap::Modal);
    }

    #[test]
    fn tap_deserializes_canonical_navigate_object() {
        let v: Tap = serde_json::from_str(
            r#"{"kind":"navigate","to":{"target":"thread","id":"t-9","event_id":"e-7"}}"#,
        )
        .unwrap();
        match v {
            Tap::Navigate { to } => {
                assert_eq!(to.target, NavigateTarget::Thread);
                assert_eq!(to.id.as_deref(), Some("t-9"));
                assert_eq!(to.event_id.as_deref(), Some("e-7"));
            }
            _ => panic!("expected Navigate"),
        }
    }

    #[test]
    fn tap_rejects_legacy_bare_string_modal() {
        let r: Result<Tap, _> = serde_json::from_str("\"modal\"");
        assert!(
            r.is_err(),
            "expected legacy bare-string `\"modal\"` to be rejected"
        );
    }

    #[test]
    fn tap_rejects_legacy_bare_string_open_thread() {
        let r: Result<Tap, _> = serde_json::from_str("\"open_thread\"");
        assert!(
            r.is_err(),
            "expected legacy bare-string `\"open_thread\"` to be rejected"
        );
    }

    #[test]
    fn tap_rejects_legacy_bare_string_none_but_coerces_the_object() {
        // The retired `none` OBJECT (`{"kind":"none"}`) is coerced to Modal (see
        // tap_deserialize_none_coerces_to_modal), but the legacy BARE STRING
        // `"none"` must still be rejected loudly (the migrate-tap-shape guard) —
        // the two must never be conflated.
        let bare: Result<Tap, _> = serde_json::from_str("\"none\"");
        assert!(
            bare.is_err(),
            "expected legacy bare-string `\"none\"` to be rejected"
        );
        let obj: Tap = serde_json::from_str(r#"{"kind":"none"}"#).unwrap();
        assert_eq!(
            obj,
            Tap::Modal,
            "the `{{kind:none}}` object must coerce to Modal"
        );
    }

    #[test]
    fn tap_rejects_unknown_kind() {
        let r: Result<Tap, _> = serde_json::from_str(r#"{"kind":"open_anywhere"}"#);
        assert!(r.is_err());
    }

    #[test]
    fn tap_serializes_to_canonical_form_only() {
        // Outbound is always the structured form.
        assert_eq!(
            serde_json::to_string(&Tap::Modal).unwrap(),
            r#"{"kind":"modal"}"#
        );
    }

    #[test]
    fn tap_default_is_modal() {
        assert_eq!(Tap::default(), Tap::Modal);
    }
}
