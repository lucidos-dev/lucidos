use super::event_bus::EventBus;
use crate::core::snapshot::{first_boot::VerifiedRestoreBoot, BoxError};

#[async_trait::async_trait]
pub trait RestoreSettlement: Sync {
    async fn settle(
        &self,
        bus: &EventBus,
        restore_boot: Option<VerifiedRestoreBoot<'_>>,
    ) -> Result<(), BoxError>;
}

/// INVARIANT: End archived authority before announcing the restore.
pub async fn settle_restored_authority(bus: &EventBus) -> Result<(), BoxError> {
    let pool = bus.pool();
    settle_restored_threads(pool, bus).await?;
    super::event_wait::settle_restored_waits(pool, bus).await?;
    super::apply_all_driver::settle_restored_batches(pool, bus).await?;
    super::standing_apply::settle_restored_standing_applies(pool, bus).await?;
    super::landing_lane::settle_restored_lane(pool, bus).await?;
    super::main_check::settle_restored_checks(pool, bus).await?;
    Ok(())
}

#[cfg(any(test, feature = "test-support"))]
pub(super) struct NormalRestoreSettlement;

#[cfg(any(test, feature = "test-support"))]
#[async_trait::async_trait]
impl RestoreSettlement for NormalRestoreSettlement {
    async fn settle(
        &self,
        bus: &EventBus,
        restore_boot: Option<VerifiedRestoreBoot<'_>>,
    ) -> Result<(), BoxError> {
        assert!(restore_boot.is_none());
        settle_restored_authority(bus).await
    }
}

#[cfg(feature = "test-support")]
#[doc(hidden)]
pub use lucidos_owner_lock::OwnerLock as TestOwnerLock;

/// INVARIANT: End restored in-flight turns before any boot subscriber or reconstructor.
/// Failures propagate to boot; a persisted terminal makes crash retries idempotent.
pub async fn settle_restored_threads(
    pool: &sqlx::PgPool,
    bus: &crate::engine::event_bus::EventBus,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    use crate::engine::event_bus::BusEvent;
    use crate::engine::thread_events::{AbortCause, EventMeta, ThreadEvent};
    let Some(boundary) = crate::core::snapshot::boundary::read(pool).await? else {
        return Ok(());
    };
    let threads: Vec<(uuid::Uuid, String)> = sqlx::query_as(
        "SELECT t.thread_id, t.source FROM thread_summaries t \
         WHERE t.status IN ('running','waiting_for_user_answer','paused') \
         AND EXISTS (SELECT 1 FROM events e WHERE e.aggregate_id = t.thread_id::text AND e.sequence <= $1) \
         AND NOT EXISTS (SELECT 1 FROM events e WHERE e.aggregate_id = t.thread_id::text \
           AND e.sequence > $1 AND (e.event_type IN ('MessageReceived','CodingAgentUserMessageSent', \
             'TriggerStarted','ChildThreadCompleted','ContinuationStarted','OrphanRecoveryStarted') \
             OR (e.event_type = 'ResponseAborted' AND e.payload->>'cause' = 'snapshot_restored')))"
    ).bind(boundary.event_sequence).fetch_all(pool).await?;
    for (thread_id, source) in threads {
        // INVARIANT: An open card would still take an answer, and an answer resumes the
        // archived turn. Close it first, as Stop does.
        if let Some(tool_use_id) =
            crate::engine::agent_question::try_lookup_pending_question_tool_use_id(pool, thread_id)
                .await?
        {
            let channel = crate::engine::agent_question::lookup_question_channel(
                pool,
                thread_id,
                &tool_use_id,
            )
            .await?;
            let closed = bus
                .emit(BusEvent::Thread {
                    thread_id,
                    event: ThreadEvent::UserQuestionAnswered {
                        compose: None,
                        tool_use_id,
                        answer: crate::engine::thread_events::AnswerKind::Canceled,
                        quiet: false,
                    },
                    meta: EventMeta {
                        channel: Some(
                            channel
                                .unwrap_or(crate::engine::thread_events::EventChannel::ClaudeCode),
                        ),
                        ..EventMeta::NONE
                    },
                })
                .await?;
            if closed.is_none() {
                return Err("snapshot_restore_question_settlement_rejected".into());
            }
        }
        // INVARIANT: Recovery needs the emit result: a logged/dropped terminal cannot
        // authorize normal boot against an unsettled archived turn.
        let persisted = bus
            .emit(BusEvent::Thread {
                thread_id,
                event: ThreadEvent::ResponseAborted {
                    cause: AbortCause::SnapshotRestored,
                    text: "This turn was interrupted by a workspace restore.".into(),
                    images: vec![],
                    model: None,
                    reasoning_effort: None,
                },
                meta: EventMeta {
                    channel: crate::engine::thread_events::EventChannel::from_wire(&source),
                    ..EventMeta::NONE
                },
            })
            .await?;
        if persisted.is_none() {
            return Err("snapshot_restore_thread_settlement_rejected".into());
        }
    }
    let applying: Vec<uuid::Uuid> = sqlx::query_scalar(
        "SELECT t.thread_id FROM thread_summaries t WHERE t.apply_in_flight \
         AND NOT EXISTS (SELECT 1 FROM events e WHERE e.aggregate_id = t.thread_id::text \
           AND e.event_type = 'ApplyNowStarted' AND e.sequence > $1)",
    )
    .bind(boundary.event_sequence)
    .fetch_all(pool)
    .await?;
    for thread_id in applying {
        if bus
            .emit(BusEvent::Thread {
                thread_id,
                event: ThreadEvent::ApplyNowEnded {
                    outcome: crate::engine::thread_events::ApplyNowOutcome::Interrupted,
                },
                meta: EventMeta::NONE,
            })
            .await?
            .is_none()
        {
            return Err("snapshot_restore_apply_settlement_rejected".into());
        }
    }
    Ok(())
}

#[macro_export]
macro_rules! turn_terminal_events {
    () => {
        "'ResponseGenerated','ResponseCanceled','ResponseAborted','ResponseFailed',\
         'CodingAgentIdled','SessionEnded'"
    };
}

#[macro_export]
macro_rules! turn_activity_events {
    () => {
        "'TextStreamed','Thinking','ThoughtStreamed','ToolCalled','ToolResult',\
         'CodingAgentTextStreamed','CodingAgentToolCalled','CodingAgentToolResult'"
    };
}

pub use crate::{turn_activity_events, turn_terminal_events};
