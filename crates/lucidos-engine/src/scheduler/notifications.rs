//! Notification storage

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

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
    pub title: String,
    pub message: String,
    pub read: bool,
    pub created_at: DateTime<Utc>,
}

/// Storage for notifications
pub struct NotificationStore;

impl NotificationStore {
    /// Initialize the notifications table
    pub async fn init_schema(pool: &PgPool) -> Result<(), sqlx::Error> {
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS notifications (
                id UUID PRIMARY KEY,
                task_id UUID,
                app_id TEXT,
                thread_id UUID,
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
    pub async fn insert(
        pool: &PgPool,
        title: &str,
        message: &str,
        task_id: Option<Uuid>,
        app_id: Option<&str>,
        thread_id: Option<Uuid>,
    ) -> Result<Notification, sqlx::Error> {
        Self::insert_with_timestamp(pool, title, message, task_id, app_id, thread_id, Utc::now())
            .await
    }

    /// Insert a notification with a custom timestamp (for backdating)
    pub async fn insert_with_timestamp(
        pool: &PgPool,
        title: &str,
        message: &str,
        task_id: Option<Uuid>,
        app_id: Option<&str>,
        thread_id: Option<Uuid>,
        created_at: DateTime<Utc>,
    ) -> Result<Notification, sqlx::Error> {
        let id = Uuid::new_v4();

        sqlx::query(
            r#"
            INSERT INTO notifications (id, task_id, app_id, thread_id, title, message, read, created_at)
            VALUES ($1, $2, $3, $4, $5, $6, false, $7)
            "#,
        )
        .bind(id)
        .bind(task_id)
        .bind(app_id)
        .bind(thread_id)
        .bind(title)
        .bind(message)
        .bind(created_at)
        .execute(pool)
        .await?;

        Ok(Notification {
            id,
            task_id,
            app_id: app_id.map(|s| s.to_string()),
            thread_id,
            title: title.to_string(),
            message: message.to_string(),
            read: false,
            created_at,
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
                    SELECT id, task_id, app_id, thread_id, title, message, read, created_at
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
                    SELECT id, task_id, app_id, thread_id, title, message, read, created_at
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
                    SELECT id, task_id, app_id, thread_id, title, message, read, created_at
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
                    SELECT id, task_id, app_id, thread_id, title, message, read, created_at
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
            SELECT id, task_id, app_id, thread_id, title, message, read, created_at
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

    /// Mark a notification as read
    pub async fn mark_read(pool: &PgPool, notification_id: Uuid) -> Result<bool, sqlx::Error> {
        let result = sqlx::query(
            r#"
            UPDATE notifications
            SET read = true
            WHERE id = $1
            "#,
        )
        .bind(notification_id)
        .execute(pool)
        .await?;

        Ok(result.rows_affected() > 0)
    }

    /// Mark all notifications as read
    pub async fn mark_all_read(pool: &PgPool) -> Result<u64, sqlx::Error> {
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

    /// Get notification by ID
    pub async fn get_by_id(
        pool: &PgPool,
        notification_id: Uuid,
    ) -> Result<Option<Notification>, sqlx::Error> {
        sqlx::query_as::<_, Notification>(
            r#"
            SELECT id, task_id, app_id, thread_id, title, message, read, created_at
            FROM notifications
            WHERE id = $1
            "#,
        )
        .bind(notification_id)
        .fetch_optional(pool)
        .await
    }
}
