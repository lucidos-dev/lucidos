//! Adopt durable accepted inputs through the queue. Live wakeups
//! and the periodic drain both reconcile from Postgres, so losing the wakeup
//! after commit cannot strand an accepted send until the next restart.
use super::*;

/// A validation failure after acceptance still records the exact accepted
/// message. Persist this pair atomically so recovery never mistakes a failed
/// attachment read for a message that has not yet been dispatched.
pub(super) fn image_failure_events(
    request: &ThreadQueueRequest,
    error: String,
) -> Result<
    Vec<(ThreadEvent, crate::engine::thread_events::EventMeta)>,
    Box<dyn std::error::Error + Send + Sync>,
> {
    use crate::engine::thread_events::{EventChannel, EventMeta};
    let ThreadQueueRequest::AgentChat {
        message,
        event_id,
        image_hashes,
        model,
        reasoning_effort,
        provider,
        origin: Some(origin),
        mode,
        device_id,
        parent_thread_id,
        spawning_event_id,
        wake_mode,
        use_coding_agent,
        ..
    } = request
    else {
        return Err("saved_draft_queue_payload_invalid".into());
    };
    if *mode != crate::engine::thread_events::ActorMode::Human || origin.mode() != *mode {
        return Err("accepted_input_actor_invalid".into());
    }
    let id = Uuid::parse_str(
        event_id
            .as_deref()
            .ok_or("saved_draft_message_identity_missing")?,
    )?;
    let channel = Some(if *use_coding_agent == Some(true) {
        EventChannel::ClaudeCode
    } else {
        EventChannel::Chat
    });
    Ok(vec![
        (
            ThreadEvent::MessageReceived {
                provider: provider.clone(),
                voice_session_id: None,
                wake_mode: *wake_mode,
                text: message.clone(),
                user_image_hashes: image_hashes.clone(),
                device_id: device_id.clone(),
                image_description: None,
                parent_thread_id: *parent_thread_id,
                spawning_event_id: *spawning_event_id,
                mode: crate::engine::thread_events::ActorMode::Human,
                model: model.clone(),
                reasoning_effort: reasoning_effort.clone(),
                origin: Some(origin.clone()),
            },
            EventMeta {
                event_id: Some(id),
                channel,
                ..EventMeta::NONE
            },
        ),
        (
            ThreadEvent::ResponseFailed {
                error,
                agent_exit: None,
                provider_error: None,
                spend_cap: None,
            },
            EventMeta {
                request_event_id: Some(id),
                channel,
                ..EventMeta::NONE
            },
        ),
    ])
}

fn message_id(
    request: &ThreadQueueRequest,
) -> Option<Result<Uuid, Box<dyn std::error::Error + Send + Sync>>> {
    match request {
        ThreadQueueRequest::AgentChat {
            event_id: Some(id), ..
        } => Some(Uuid::parse_str(id).map_err(Into::into)),
        ThreadQueueRequest::AgentChat {
            origin: Some(MessageOrigin::SavedDraft { .. }),
            event_id,
            ..
        } => Some(
            event_id
                .as_deref()
                .ok_or_else(|| "saved_draft_message_identity_missing".into())
                .and_then(|id| Uuid::parse_str(id).map_err(Into::into)),
        ),
        _ => None,
    }
}

pub(super) fn validate_saved_request(
    value: &serde_json::Value,
    event_id: Uuid,
    thread_id: Uuid,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let request: ThreadQueueRequest = serde_json::from_value(value.clone())?;
    if !matches!(&request, ThreadQueueRequest::AgentChat {
        origin: Some(MessageOrigin::SavedDraft { submission_event_id, .. }), ..
    } if *submission_event_id == event_id)
        || request.thread_id() != Some(thread_id)
    {
        return Err("saved_draft_queue_identity_mismatch".into());
    }
    message_id(&request).ok_or("saved_draft_message_identity_missing")??;
    Ok(())
}

impl ThreadQueue {
    pub(super) async fn notify_invalid_accepted_input(
        &self,
        entry_id: Uuid,
        thread_id: Uuid,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let notice_id = Uuid::new_v5(&entry_id, b"invalid-saved-draft-request");
        self.bus.emit_system_once(notice_id, SystemEvent::NotificationCreated {
            id: notice_id.to_string(),
            title: "Accepted input needs recovery".into(),
            message: "Lucidos could not read an accepted input. Its durable request is retained in the Thread Queue for recovery.".into(),
            task_id: None, app_id: None, thread_id: Some(thread_id.to_string()), event_id: None,
            tap: crate::scheduler::notifications::Tap::Navigate {
                to: Box::new(crate::scheduler::notifications::NavigateUi {
                    target: crate::scheduler::notifications::NavigateTarget::ThreadQueue,
                    ..Default::default()
                }),
            },
            actor: None, audience: None, push_group: None,
        }).await?;
        Ok(())
    }

    /// A process error can follow a durable failed turn. Release capacity
    /// only when this exact input and its terminal outcome both survived.
    pub(super) async fn recorded_chat_outcome(&self, thread_id: Uuid, message_id: Uuid) -> bool {
        let recorded = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM events WHERE id=$1 AND thread_id=$2)",
        )
        .bind(message_id)
        .bind(thread_id)
        .fetch_one(&self.pool)
        .await
        .unwrap_or(false);
        recorded
            && crate::engine::thread_events::has_terminator_for(&self.pool, thread_id, message_id)
                .await
    }

    /// A chat request may target a thread that existed before acceptance.
    /// Its message identity decides whether execution crossed into thread
    /// recovery. Legacy requests without an identity use the spawn fallback.
    pub(super) async fn queued_chat_message_recorded(
        &self,
        request: &ThreadQueueRequest,
    ) -> Option<Result<bool, Box<dyn std::error::Error + Send + Sync>>> {
        let id = match message_id(request)? {
            Ok(id) => id,
            Err(error) => return Some(Err(error)),
        };
        Some(
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM events WHERE id=$1)")
                .bind(id)
                .fetch_one(&self.pool)
                .await
                .map_err(Into::into),
        )
    }

    pub(super) async fn load_accepted_inputs(
        &self,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        #[derive(sqlx::FromRow)]
        struct Row {
            id: Uuid,
            event_id: Uuid,
            thread_id: Uuid,
            summary: String,
            request: serde_json::Value,
            queued_at: DateTime<Utc>,
            accepted_request: serde_json::Value,
            ordinary: bool,
        }
        let rows: Vec<Row> = sqlx::query_as(
            "SELECT q.id,e.id AS event_id,q.thread_id,q.summary,q.request,q.queued_at, e.payload->'data'->'request' AS accepted_request, \
             (e.event_type='OrdinarySubmissionAccepted') AS ordinary \
             FROM thread_queue q JOIN events e ON \
               (e.event_type='DraftSubmissionAccepted' AND e.payload->'data'->'receipt'->>'queue_entry_id'=q.id::text) \
               OR (e.event_type='OrdinarySubmissionAccepted' AND e.payload->'data'->>'entry_id'=q.id::text) \
             WHERE q.status='queued' \
             AND e.sequence > COALESCE((SELECT event_sequence FROM snapshot_restore_boundary WHERE singleton),0) \
             ORDER BY q.sequence",
        ).fetch_all(&self.pool).await?;
        let mut completed = Vec::new();
        let mut invalid = Vec::new();
        {
            let mut state = self.state.lock().await;
            for row in rows {
                if state.active.contains_key(&row.id)
                    || state.accepted_user_inputs.contains(&row.id)
                    || state.queued.iter().any(|entry| entry.id == row.id)
                {
                    continue;
                }
                if row.request != row.accepted_request {
                    invalid.push((
                        row.id,
                        row.thread_id,
                        "saved_draft_queue_payload_mismatch".into(),
                    ));
                    continue;
                }
                let request: ThreadQueueRequest = match serde_json::from_value(row.request) {
                    Ok(request) => request,
                    Err(error) => {
                        invalid.push((row.id, row.thread_id, error.to_string()));
                        continue;
                    }
                };
                let valid_origin = if row.ordinary {
                    matches!(&request, ThreadQueueRequest::AgentChat { mode: crate::engine::thread_events::ActorMode::Human, origin: Some(origin), .. } if origin.mode() == crate::engine::thread_events::ActorMode::Human)
                } else {
                    matches!(&request, ThreadQueueRequest::AgentChat {
                    origin: Some(MessageOrigin::SavedDraft { submission_event_id, .. }), ..
                } if *submission_event_id==row.event_id)
                };
                if !valid_origin || request.thread_id() != Some(row.thread_id) {
                    invalid.push((
                        row.id,
                        row.thread_id,
                        "saved_draft_queue_identity_mismatch".into(),
                    ));
                    continue;
                }
                let id = match message_id(&request) {
                    Some(Ok(id)) => id,
                    _ => {
                        invalid.push((
                            row.id,
                            row.thread_id,
                            "saved_draft_message_identity_missing".into(),
                        ));
                        continue;
                    }
                };
                // Re-read under the manager lock: the initial scan may have
                // raced another drain through admission and completion.
                let (queued, recorded): (bool, bool) = sqlx::query_as(
                    "SELECT EXISTS(SELECT 1 FROM thread_queue WHERE id=$1 AND status='queued'), \
                     EXISTS(SELECT 1 FROM events WHERE id=$2)",
                )
                .bind(row.id)
                .bind(id)
                .fetch_one(&self.pool)
                .await?;
                if !queued {
                    continue;
                }
                if recorded {
                    completed.push(row.id);
                    continue;
                }
                if row.ordinary {
                    state.accepted_user_inputs.insert(row.id);
                }
                state.queued.push_back(QueueEntry {
                    id: row.id,
                    kind: request.kind(),
                    trigger_id: None,
                    trigger_name: None,
                    thread_id: Some(row.thread_id),
                    summary: row.summary,
                    request,
                    queued_at: row.queued_at,
                    completion_tx: None,
                });
            }
        }
        for (entry_id, thread_id, error) in invalid {
            log!(
                "[ThreadQueue] Accepted input {} needs recovery: {}",
                entry_id,
                error
            );
            self.notify_invalid_accepted_input(entry_id, thread_id)
                .await?;
        }
        for entry_id in completed {
            self.bus
                .emit(BusEvent::System(SystemEvent::ThreadQueueCompleted {
                    entry_id,
                }))
                .await?;
        }
        Ok(())
    }
}
