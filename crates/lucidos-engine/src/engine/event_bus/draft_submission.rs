//! Transactional acceptance primitive. The submission service authenticates
//! the caller and resolves the send before constructing `PreparedDraftSubmission`.
//! This primitive rechecks owner authority on its transaction. No HTTP caller
//! can supply this structure. Dispatch happens only after this commits.

use super::*;
use crate::core::draft_submission::{
    DraftSubmissionAuthority, DraftSubmissionReceipt, SavedDraftRow, SavedDraftSnapshot,
    SubmitDraftRequest, SAVED_DRAFT_COLS,
};

use crate::engine::thread_events::{ActorMode, MessageOrigin};
use crate::engine::thread_queue::ThreadQueueRequest;

pub struct PreparedDraftSubmission {
    pub request: SubmitDraftRequest,
    pub snapshot: SavedDraftSnapshot,
    pub queue_request: ThreadQueueRequest,
    pub actor: MessageOrigin,
    pub authority: DraftSubmissionAuthority,
}

impl EventBus {
    /// Read a prior acceptance before resolving today's draft or settings.
    /// `actor` must come from authenticated transport provenance. This only
    /// returns existing work belonging to that submitter; it grants no authority
    /// for a new acceptance. A miss must still pass the transactional replay
    /// check in `accept_saved_draft` after preparation.
    pub async fn saved_draft_receipt(
        &self,
        request: &SubmitDraftRequest,
        actor: &MessageOrigin,
    ) -> Result<Option<DraftSubmissionReceipt>, Box<dyn std::error::Error + Send + Sync>> {
        read_saved_draft_receipt(&self.pool, request, actor).await
    }

    /// Authorize and commit a resolved saved send. The append horizon
    /// lock precedes the thread lock, and the draft row stays locked through
    /// receipt, queue and compose projection. A replay consults the immutable
    /// receipt before examining today's draft, so later edits survive retries.
    pub async fn accept_saved_draft(
        &self,
        prepared: PreparedDraftSubmission,
    ) -> Result<DraftSubmissionReceipt, Box<dyn std::error::Error + Send + Sync>> {
        prepared.request.validate()?;
        if prepared.actor.mode() != ActorMode::Agent || prepared.actor.source_thread_id().is_none()
        {
            return Err("draft_submitter_unattributed".into());
        }
        let caller = prepared
            .actor
            .source_thread_id()
            .ok_or("draft_submitter_unattributed")?;
        let depth = current_event_trigger_depth();
        let emitting_trigger_id = current_trigger_id();
        // A permission's spend lands on the thread that granted it.
        let granting_thread = match &prepared.authority {
            DraftSubmissionAuthority::Permission { permission_id } => Some(
                crate::engine::permission::permission_row(&self.pool, *permission_id)
                    .await?
                    .and_then(|p| p.granted_in_thread_id)
                    .ok_or("draft_owner_authority_missing")?,
            ),
            _ => None,
        };
        let mut append = self.begin_append().await?;
        // Caller authority and target compose state share one lock ordering.
        // Opposite-direction sends must not each hold the other's thread.
        let mut threads = vec![caller, prepared.request.thread_id];
        threads.extend(granting_thread);
        threads.sort_unstable();
        threads.dedup();
        for thread in threads {
            Self::lock_thread_append(&mut append.tx, thread).await?;
        }
        // A retry key is workspace-wide. Different target locks must not let
        // concurrent reuse race past the receipt check.
        sqlx::query("SELECT pg_advisory_xact_lock($1, hashtext($2))")
            .bind(0x6472_6166_i32)
            .bind(prepared.request.retry_key.to_string())
            .execute(&mut *append.tx)
            .await?;
        if let Some(receipt) =
            read_saved_draft_receipt(&mut *append.tx, &prepared.request, &prepared.actor).await?
        {
            append.tx.rollback().await?;
            return Ok(receipt);
        }
        let sql = format!("SELECT {SAVED_DRAFT_COLS} FROM thread_summaries WHERE thread_id=$1 AND state IN ('composing','active') FOR UPDATE");
        let raw: SavedDraftRow = sqlx::query_as(&sql)
            .bind(prepared.request.thread_id)
            .fetch_optional(&mut *append.tx)
            .await?
            .ok_or("draft_missing")?;
        if !prepared.request.names_current(&raw) {
            return Err("draft_snapshot_stale".into());
        }
        let current = SavedDraftSnapshot::try_from(raw)?;
        if current != prepared.snapshot {
            return Err("draft_snapshot_stale".into());
        }
        match &prepared.queue_request {
            ThreadQueueRequest::AgentChat {
                user_addressing,
                message,
                thread_id,
                image_hashes,
                device_id,
                app_context,
                file_context,
                url_context,
                conflict_change_id,
                parent_thread_id,
                spawning_event_id,
                wake_mode,
                integrated,
                spawning_trigger_id,
                mode,
                event_id,
                model,
                reasoning_effort,
                provider,
                use_coding_agent,
                cc_model,
                coding_agent,
                ..
            } if *thread_id == prepared.request.thread_id
                && message == &current.report().text
                && image_hashes == &current.report().image_hashes
                && user_addressing.is_none()
                && app_context.is_none()
                && file_context.is_none()
                && url_context.is_none()
                && conflict_change_id.is_none()
                && device_id.is_none()
                && parent_thread_id.is_none()
                && spawning_event_id.is_none()
                && wake_mode.is_none()
                && !integrated
                && spawning_trigger_id.is_none()
                && *mode == ActorMode::Human
                && event_id
                    .as_deref()
                    .is_some_and(|id| Uuid::parse_str(id).is_ok_and(|id| !id.is_nil())) =>
            {
                // Defaults are resolved by the service; explicit saved picks
                // must survive preparation unchanged. Coding effort shares
                // the wire field with chat effort, but uses its own selection.
                let coding = *use_coding_agent == Some(true);
                let selection = current.selection();
                let unchanged = |saved: &Option<String>, queued: &Option<String>| {
                    saved
                        .as_ref()
                        .is_none_or(|value| queued.as_ref() == Some(value))
                };
                let channel_matches = current
                    .report()
                    .mode
                    .as_deref()
                    .is_none_or(|mode| (mode == "claude_code") == coding);
                let settings_match = if coding {
                    selection.cc_model == *cc_model
                        && selection.cc_reasoning_effort == *reasoning_effort
                        && selection
                            .coding_agent
                            .as_ref()
                            .is_none_or(|agent| coding_agent.as_ref() == Some(agent))
                } else {
                    unchanged(&selection.model, model)
                        && unchanged(&selection.reasoning_effort, reasoning_effort)
                        && unchanged(&selection.provider, provider)
                };
                if !channel_matches || !settings_match {
                    return Err("draft_queue_payload_mismatch".into());
                }
                let (state, source, bound_backend): (String, String, Option<String>) =
                    sqlx::query_as(
                        "SELECT state,source,coding_agent FROM thread_summaries WHERE thread_id=$1",
                    )
                    .bind(prepared.request.thread_id)
                    .fetch_one(&mut *append.tx)
                    .await?;
                let active = state == "active";
                if active
                    && (crate::engine::thread_lifecycle::ThreadType::from_source(&source)
                        == crate::engine::thread_lifecycle::ThreadType::CodingAgent)
                        != coding
                {
                    return Err("draft_channel_locked".into());
                }
                if coding {
                    let backend = coding_agent.ok_or("draft_coding_backend_missing")?;
                    let bound = if active {
                        // NULL alone is the legacy Claude Code binding. Unknown
                        // stored values must not inherit parse()'s tolerant fallback.
                        Some(match bound_backend.as_deref() {
                            None | Some("claude-code") => crate::runtime::CodingAgent::ClaudeCode,
                            Some("codex") => crate::runtime::CodingAgent::Codex,
                            Some(_) => return Err("draft_coding_backend_unsupported".into()),
                        })
                    } else {
                        None
                    };
                    if bound.is_some_and(|bound| bound != backend) {
                        return Err("draft_coding_backend_locked".into());
                    }
                    // A Rust-only prepared payload still cannot smuggle a pick
                    // the runtime would replace, trim, or silently discard.
                    current.resolve_coding_selection(bound, backend)?;
                }
            }
            _ => return Err("draft_queue_payload_mismatch".into()),
        }
        let spent = match &prepared.authority {
            DraftSubmissionAuthority::StandingInstruction {
                instruction_event_id,
            } => {
                let actual = crate::api::standing_instruction::standing_instruction_event(
                    &mut append.tx,
                    caller,
                )
                .await?;
                if actual != Some(*instruction_event_id) {
                    return Err("draft_owner_authority_missing".into());
                }
                None
            }
            DraftSubmissionAuthority::Permission { permission_id } => {
                use crate::engine::permission::{live_permission, Action, Lifetime};
                // Only a once grant sends a draft, so the spend below ends it.
                let permission = live_permission(
                    &mut *append.tx,
                    &crate::api::permission_requests::thread_user(caller),
                    Action::SubmitDraft,
                    Some(&prepared.request.thread_id.to_string()),
                )
                .await?
                .filter(|p| {
                    p.permission_id == *permission_id && p.lifetime == Lifetime::Once.wire()
                })
                .ok_or("draft_owner_authority_missing")?;
                let granting_thread = permission
                    .granted_in_thread_id
                    .ok_or("draft_owner_authority_missing")?;
                let te = ThreadEvent::PermissionSpent {
                    permission_id: *permission_id,
                };
                let id = Uuid::new_v4();
                let meta = EventMeta::with_actor(Some(prepared.actor.clone()));
                let seq = self
                    .persist(
                        &mut append.tx,
                        &append.ticket,
                        NewEventRow {
                            id,
                            aggregate: "thread",
                            aggregate_id: &granting_thread.to_string(),
                            event_type: te.event_type(),
                            payload: &te.to_payload(&meta),
                            trigger_chain_depth: thread_trigger_chain_depth(depth),
                            emitting_trigger_id: emitting_trigger_id.as_deref(),
                        },
                    )
                    .await?;
                self.update_thread_projection(&mut append.tx, granting_thread, &te, &meta)
                    .await?;
                Some((id, seq, granting_thread, te, meta))
            }
            DraftSubmissionAuthority::OwnerApproval { .. } => {
                return Err("draft_owner_authority_missing".into());
            }
        };
        let receipt = DraftSubmissionReceipt {
            thread_id: prepared.request.thread_id,
            revision: prepared.request.expected_revision,
            epoch: prepared.request.expected_epoch,
            retry_key: prepared.request.retry_key,
            event_id: Uuid::new_v4(),
            queue_entry_id: Uuid::new_v4(),
        };
        let mut queue_request = prepared.queue_request;
        queue_request.stamp_caller_depth(depth);
        if let ThreadQueueRequest::AgentChat { origin, .. } = &mut queue_request {
            *origin = Some(MessageOrigin::SavedDraft {
                submission_event_id: receipt.event_id,
                submitted_by_thread_id: caller,
            });
        }
        // Join while the target append lock still serializes ordinary and
        // saved acceptance. Rollback drops this place without publishing it.
        let registration = self.register_accepted_input_turn(
            receipt.queue_entry_id,
            receipt.thread_id,
            match &queue_request {
                ThreadQueueRequest::AgentChat {
                    event_id: Some(id), ..
                } => Uuid::parse_str(id)?,
                _ => return Err("draft_payload_invalid".into()),
            },
            self.input_order.join(receipt.thread_id),
            &self.input_order,
        );
        let event = SystemEvent::DraftSubmissionAccepted {
            receipt: receipt.clone(),
            snapshot: current.report().clone(),
            request: Box::new(queue_request),
            authority: prepared.authority,
            actor: Some(prepared.actor),
        };
        let committed = self
            .commit_system_event(
                append,
                receipt.event_id,
                event,
                depth,
                emitting_trigger_id.clone(),
            )
            .await?;
        registration.commit();
        if let Some((event_id, seq, thread_id, event, meta)) = spent {
            committed.ticket.send(
                &self.event_tx,
                EmittedEvent {
                    event_id,
                    seq: Some(seq),
                    created: Utc::now(),
                    typed: BusEvent::Thread {
                        thread_id,
                        event,
                        meta,
                    },
                    aggregate: None,
                    depth,
                    emitting_trigger_id,
                },
            );
        }
        self.publish_system_event(committed);
        Ok(receipt)
    }
}

#[derive(sqlx::FromRow)]
struct SavedDraftReceiptRow {
    thread_id: Uuid,
    revision: i64,
    epoch: i64,
    event_id: Uuid,
    queue_entry_id: Uuid,
    actor: serde_json::Value,
}

async fn read_saved_draft_receipt<'e>(
    executor: impl sqlx::Executor<'e, Database = sqlx::Postgres>,
    request: &SubmitDraftRequest,
    actor: &MessageOrigin,
) -> Result<Option<DraftSubmissionReceipt>, Box<dyn std::error::Error + Send + Sync>> {
    request.validate()?;
    if actor.mode() != ActorMode::Agent || actor.source_thread_id().is_none() {
        return Err("draft_submitter_unattributed".into());
    }
    let row: Option<SavedDraftReceiptRow> = sqlx::query_as(
        "SELECT d.thread_id,d.revision,d.epoch,d.event_id,d.queue_entry_id,\
         e.payload->'data'->'actor' AS actor FROM draft_submissions d \
         JOIN events e ON e.id=d.event_id WHERE retry_key=$1",
    )
    .bind(request.retry_key)
    .fetch_optional(executor)
    .await?;
    let Some(row) = row else { return Ok(None) };
    let submitter: MessageOrigin = serde_json::from_value(row.actor)?;
    if row.thread_id != request.thread_id
        || row.revision != request.expected_revision
        || row.epoch != request.expected_epoch
        || submitter.source_thread_id() != actor.source_thread_id()
    {
        return Err("draft_retry_key_conflict".into());
    }
    Ok(Some(DraftSubmissionReceipt {
        thread_id: row.thread_id,
        revision: row.revision,
        epoch: row.epoch,
        retry_key: request.retry_key,
        event_id: row.event_id,
        queue_entry_id: row.queue_entry_id,
    }))
}

#[cfg(test)]
mod tests;
