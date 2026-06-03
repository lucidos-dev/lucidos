//! The `notifications` projection: `EventBus::update_system_projection`
//! materializes `NotificationCreated` rows into the `notifications` table.

use uuid::Uuid;

use super::super::{EventBus, SystemEvent};

impl EventBus {
    // ---- System projection ----

    pub(crate) async fn update_system_projection(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        event_id: Uuid,
        event: &SystemEvent,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        match event {
            SystemEvent::NotificationCreated {
                id,
                title,
                message,
                task_id,
                app_id,
                thread_id,
                event_id: link_event_id,
                tap,
                actor: _,
            } => {
                let notification_id = Uuid::parse_str(id).unwrap_or(event_id);
                let task_uuid = task_id.as_ref().and_then(|s| Uuid::parse_str(s).ok());
                let thread_uuid = thread_id.as_ref().and_then(|s| Uuid::parse_str(s).ok());
                let event_uuid = link_event_id.as_ref().and_then(|s| Uuid::parse_str(s).ok());
                sqlx::query(
                    "INSERT INTO notifications (id, task_id, app_id, thread_id, event_id, title, message, read, created_at, tap) \
                     VALUES ($1, $2, $3, $4, $5, $6, $7, false, NOW(), $8)"
                )
                .bind(notification_id)
                .bind(task_uuid)
                .bind(app_id.as_deref())
                .bind(thread_uuid)
                .bind(event_uuid)
                .bind(title)
                .bind(message)
                .bind(sqlx::types::Json(tap))
                .execute(&mut **tx)
                .await?;
                Ok(())
            }
            _ => Ok(()),
        }
    }
}
