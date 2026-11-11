//! System projections: `EventBus::update_system_projection` materializes
//! `NotificationCreated` rows into the `notifications` table, the
//! `ThreadQueue*` events into the `thread_queue` table, and `RepositoryAdded`
//! into the durable `repo_names` table (so removed repos keep a resolvable
//! name), and `AppDeleted` into `app_kinds` (so a removed widget stays one).
//! `McpServerStopped` and `McpServerRemoved` clear the server from every
//! `thread_summaries.enabled_mcp_servers` (ADR 0430).

use uuid::Uuid;

use super::super::{EventBus, SystemEvent};

impl EventBus {
    /// Fill in the audience of a `NotificationCreated` that names none, from
    /// the root thread of the thread it opens (ADR 0623). The event then carries
    /// it, so a replay projects the same row. A lookup that fails leaves the
    /// whole team: a notification must never silently reach nobody.
    pub(crate) async fn with_notification_audience(&self, event: SystemEvent) -> SystemEvent {
        let SystemEvent::NotificationCreated {
            id,
            title,
            message,
            task_id,
            app_id,
            thread_id,
            event_id,
            tap,
            actor,
            audience: None,
            push_group,
        } = event
        else {
            return event;
        };
        let link_event = event_id.as_deref().and_then(|e| Uuid::parse_str(e).ok());
        let audience = crate::engine::thread_people::audience_of_notification(
            self.pool(),
            &tap,
            link_event,
            &crate::gateway_operators::current(),
        )
        .await
        .unwrap_or_else(|e| {
            log!(
                "[EventBus] Cannot read a notification's audience (team): {}",
                e
            );
            crate::engine::thread_people::Audience::Team
        })
        .device_ids();
        SystemEvent::NotificationCreated {
            id,
            title,
            message,
            task_id,
            app_id,
            thread_id,
            event_id,
            tap,
            actor,
            audience,
            push_group,
        }
    }

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
                audience,
                push_group,
            } => {
                let notification_id = Uuid::parse_str(id).unwrap_or(event_id);
                let task_uuid = task_id.as_ref().and_then(|s| Uuid::parse_str(s).ok());
                let thread_uuid = thread_id.as_ref().and_then(|s| Uuid::parse_str(s).ok());
                let event_uuid = link_event_id.as_ref().and_then(|s| Uuid::parse_str(s).ok());
                sqlx::query(
                    "INSERT INTO notifications (id, task_id, app_id, thread_id, event_id, title, message, read, created_at, tap, audience, push_group) \
                     VALUES ($1, $2, $3, $4, $5, $6, $7, false, NOW(), $8, $9, $10)"
                )
                .bind(notification_id)
                .bind(task_uuid)
                .bind(app_id.as_deref())
                .bind(thread_uuid)
                .bind(event_uuid)
                .bind(title)
                .bind(message)
                .bind(sqlx::types::Json(tap))
                .bind(audience.as_deref())
                .bind(push_group.as_ref().map(sqlx::types::Json))
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
                // `admitted_at` keeps the FIRST admission's time. A trigger
                // fire re-emits this event once it creates its thread (see
                // `ThreadQueue::record_entry_thread`). Restamping there would
                // restart the age the panel shows for a running entry. A
                // genuine re-admission still gets a fresh stamp: the
                // `ThreadQueued` upsert above nulls the column first.
                sqlx::query(
                    "UPDATE thread_queue SET status = 'admitted', \
                     admitted_at = COALESCE(admitted_at, NOW()), \
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
            // Stop and Remove switch the server off in every thread (ADR 0430).
            SystemEvent::McpServerStopped { server_id, .. }
            | SystemEvent::McpServerRemoved { server_id, .. } => {
                sqlx::query(
                    "UPDATE thread_summaries \
                     SET enabled_mcp_servers = array_remove(enabled_mcp_servers, $1) \
                     WHERE $1 = ANY(enabled_mcp_servers)",
                )
                .bind(server_id)
                .execute(&mut **tx)
                .await?;
                Ok(())
            }
            SystemEvent::AppDeleted {
                app_id,
                kind: Some(kind),
                ..
            } => {
                // Stored in the spelling serde writes, and read back through serde.
                let kind = serde_json::to_value(kind)?;
                sqlx::query(
                    "INSERT INTO app_kinds (app_id, kind) VALUES ($1, $2) \
                     ON CONFLICT (app_id) DO UPDATE SET kind = EXCLUDED.kind",
                )
                .bind(app_id)
                .bind(kind.as_str())
                .execute(&mut **tx)
                .await?;
                Ok(())
            }
            _ => Ok(()),
        }
    }
}
