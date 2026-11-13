//! System projections: `EventBus::update_system_projection` materializes
//! `NotificationCreated` rows into the `notifications` table, the
//! `ThreadQueue*` events into the `thread_queue` table, and `RepositoryAdded`
//! into the durable `repo_names` table (so removed repos keep a resolvable
//! name), and `AppDeleted` into `app_kinds` (so a removed widget stays one).
//! `McpServerStopped` and `McpServerRemoved` clear the server from every
//! `thread_summaries.enabled_mcp_servers` (ADR 0430).

use uuid::Uuid;

use super::super::{EventBus, SystemEvent};

struct QueuedProjection<'a> {
    entry_id: Uuid,
    kind: crate::engine::thread_queue::ThreadQueueKind,
    trigger_id: Option<&'a str>,
    trigger_name: Option<&'a str>,
    thread_id: Option<Uuid>,
    summary: &'a str,
    request: &'a serde_json::Value,
}

async fn project_queued(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    queued: QueuedProjection<'_>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // Upsert also serves boot requeue: the original immutable request stays,
    // while admission time resets for the new queue wait.
    sqlx::query(
        "INSERT INTO thread_queue (id, kind, trigger_id, trigger_name, thread_id, summary, request, status, queued_at) \
         VALUES ($1,$2,$3,$4,$5,$6,$7,'queued',NOW()) \
         ON CONFLICT (id) DO UPDATE SET status='queued', queued_at=NOW(), admitted_at=NULL",
    ).bind(queued.entry_id).bind(queued.kind.as_str()).bind(queued.trigger_id)
        .bind(queued.trigger_name).bind(queued.thread_id).bind(queued.summary).bind(queued.request)
        .execute(&mut **tx).await?;
    Ok(())
}

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
        let audience = crate::engine::thread_users::audience_of_notification(
            self.pool(),
            &tap,
            link_event,
            &crate::gateway_users::current(),
        )
        .await
        .unwrap_or_else(|e| {
            log!(
                "[EventBus] Cannot read a notification's audience (team): {}",
                e
            );
            crate::engine::thread_users::Audience::Team
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
            SystemEvent::OrdinarySubmissionAccepted {
                thread_id,
                entry_id,
                request,
                compose,
                ..
            } => {
                // A projection rebuild applies the accepted report once, never
                // over a newer compose epoch. Live acceptance consumed this
                // slot already on the same transaction.
                if let Some(report) = compose {
                    sqlx::query("UPDATE thread_summaries SET compose_text=$2, compose_images=$3, compose_mode=$4, compose_selection=$5, compose_epoch=$6 WHERE thread_id=$1 AND compose_epoch < $6")
                        .bind(thread_id).bind(&report.text)
                        .bind(serde_json::json!(report.image_hashes))
                        .bind(&report.mode).bind(&report.selection).bind(report.compose_epoch)
                        .execute(&mut **tx).await?;
                }
                let request_json = serde_json::to_value(request)?;
                project_queued(
                    tx,
                    QueuedProjection {
                        entry_id: *entry_id,
                        kind: request.kind(),
                        trigger_id: None,
                        trigger_name: None,
                        thread_id: Some(*thread_id),
                        summary: &request.summary(None),
                        request: &request_json,
                    },
                )
                .await
            }
            SystemEvent::DraftSubmissionAccepted {
                receipt,
                snapshot,
                request,
                ..
            } => {
                let inserted = sqlx::query(
                    "INSERT INTO draft_submissions (retry_key, thread_id, revision, epoch, event_id, queue_entry_id) \
                     VALUES ($1,$2,$3,$4,$5,$6) ON CONFLICT (retry_key) DO NOTHING",
                )
                .bind(receipt.retry_key).bind(receipt.thread_id)
                .bind(receipt.revision).bind(receipt.epoch)
                .bind(event_id).bind(receipt.queue_entry_id)
                .execute(&mut **tx).await?.rows_affected();
                if inserted == 0 {
                    let existing: (Uuid, i64, i64, Uuid, Uuid) = sqlx::query_as(
                        "SELECT thread_id, revision, epoch, event_id, queue_entry_id FROM draft_submissions WHERE retry_key=$1",
                    ).bind(receipt.retry_key).fetch_one(&mut **tx).await?;
                    if existing
                        != (
                            receipt.thread_id,
                            receipt.revision,
                            receipt.epoch,
                            event_id,
                            receipt.queue_entry_id,
                        )
                    {
                        return Err("draft_retry_key_conflict".into());
                    }
                    // Replaying this acceptance must not resurrect completed
                    // work or consume a draft written after it.
                    return Ok(());
                }
                sqlx::query(
                    "UPDATE thread_summaries SET compose_text='', compose_images='[]'::jsonb, \
                     compose_mode=NULL, compose_selection=NULL, compose_epoch=compose_epoch+1 \
                     WHERE thread_id=$1 AND summary_version=$2 AND compose_epoch=$3",
                )
                .bind(receipt.thread_id)
                .bind(snapshot.summary_version)
                .bind(snapshot.compose_epoch)
                .execute(&mut **tx)
                .await?;
                let request_json = serde_json::to_value(request)?;
                project_queued(
                    tx,
                    QueuedProjection {
                        entry_id: receipt.queue_entry_id,
                        kind: request.kind(),
                        trigger_id: None,
                        trigger_name: None,
                        thread_id: Some(receipt.thread_id),
                        summary: &request.summary(None),
                        request: &request_json,
                    },
                )
                .await
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
                project_queued(
                    tx,
                    QueuedProjection {
                        entry_id: *entry_id,
                        kind: *kind,
                        trigger_id: trigger_id.as_deref(),
                        trigger_name: trigger_name.as_deref(),
                        thread_id: *thread_id,
                        summary,
                        request,
                    },
                )
                .await
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
            SystemEvent::PermissionGrantsChanged {
                grant_file,
                patterns,
                source,
                granted_in_thread_id,
                ..
            } => {
                crate::engine::permission::projection::write_standing_set(
                    tx,
                    *grant_file,
                    patterns,
                    *source,
                    *granted_in_thread_id,
                )
                .await
            }
            _ => Ok(()),
        }
    }
}
