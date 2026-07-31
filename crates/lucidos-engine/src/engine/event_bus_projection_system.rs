//! System projections: `EventBus::update_system_projection` materializes
//! `NotificationCreated` rows into the `notifications` table, the
//! `ThreadQueue*` events into the `thread_queue` table, and `RepositoryAdded`
//! into the durable `repo_names` table (so removed repos keep a resolvable
//! name).

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
            SystemEvent::ThreadQueued {
                entry_id,
                kind,
                trigger_id,
                trigger_name,
                thread_id,
                summary,
                request,
                requeued: _,
                actor: _,
            } => {
                // Upsert: the boot sweep re-queues an 'admitted' entry whose
                // work died with the previous engine process by re-emitting
                // ThreadQueued for the same entry_id. queued_at resets so the
                // backlog-age notification doesn't fire spuriously right
                // after a restart.
                sqlx::query(
                    "INSERT INTO thread_queue (id, kind, trigger_id, trigger_name, thread_id, summary, request, status, queued_at) \
                     VALUES ($1, $2, $3, $4, $5, $6, $7, 'queued', NOW()) \
                     ON CONFLICT (id) DO UPDATE SET status = 'queued', queued_at = NOW(), admitted_at = NULL",
                )
                .bind(entry_id)
                .bind(kind.as_str())
                .bind(trigger_id.as_deref())
                .bind(trigger_name.as_deref())
                .bind(thread_id)
                .bind(summary)
                .bind(request)
                .execute(&mut **tx)
                .await?;
                Ok(())
            }
            SystemEvent::ThreadQueueAdmitted {
                entry_id,
                thread_id,
                actor: _,
            } => {
                sqlx::query(
                    "UPDATE thread_queue SET status = 'admitted', admitted_at = NOW(), \
                     thread_id = COALESCE($2, thread_id) WHERE id = $1",
                )
                .bind(entry_id)
                .bind(thread_id)
                .execute(&mut **tx)
                .await?;
                Ok(())
            }
            SystemEvent::ThreadQueueDropped { entry_id, .. }
            | SystemEvent::ThreadQueueCompleted { entry_id } => {
                sqlx::query("DELETE FROM thread_queue WHERE id = $1")
                    .bind(entry_id)
                    .execute(&mut **tx)
                    .await?;
                Ok(())
            }
            SystemEvent::RepositoryAdded { repo_id, name, .. } => {
                // Durable repo-name projection: retain the name so a later
                // RepositoryRemoved (which deliberately does NOT touch this
                // table) can't erase it — the filter / thread rows keep
                // resolving the historical name instead of the raw UUID.
                // Upsert covers a rename re-add at the same path (same id).
                if let Ok(uuid) = Uuid::parse_str(repo_id) {
                    sqlx::query(
                        "INSERT INTO repo_names (id, name) VALUES ($1, $2) \
                         ON CONFLICT (id) DO UPDATE SET name = EXCLUDED.name",
                    )
                    .bind(uuid)
                    .bind(name)
                    .execute(&mut **tx)
                    .await?;
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }
}
