use uuid::Uuid;

pub const APPROVED_PLAN_REENTRY_TEXT: &str = "The user approved the plan and chose to \
     build in this session. No source file changed. Start the first phase now. If the user \
     told you to wait, say so in one line and end the turn.";

pub fn no_source_change(changed_files: Option<&[String]>) -> bool {
    match changed_files {
        None => false,
        Some([]) => true,
        Some(files) => crate::engine::git_ops::lands_plan_files_only(files),
    }
}

/// INVARIANT: Whether the ending turn approved the plan's full scope, left nothing to
/// wait for, and opened no card. Call it before the turn's own idle is
/// written, since it counts approvals after the latest idle. An unreadable
/// answer is no, so a failed read never sends the agent back.
pub async fn approved_this_turn_and_idle(pool: &sqlx::PgPool, thread_id: Uuid) -> bool {
    sqlx::query_scalar::<_, bool>(&format!(
        "SELECT EXISTS (SELECT 1 FROM events \
           WHERE thread_id = $1 AND event_type = 'PlanApproved' \
             AND payload->>'kind' = 'implementation' AND payload->>'scope' = 'implement' AND payload->>'inherited_from_event_id' IS NULL \
             AND sequence > COALESCE((SELECT MAX(sequence) FROM events \
               WHERE thread_id = $1 AND event_type = 'CodingAgentIdled'), -1)) \
         AND NOT EXISTS (SELECT 1 FROM events q \
           WHERE q.thread_id = $1 AND q.event_type = 'UserQuestionAsked' \
             AND {scope} \
             AND NOT EXISTS (SELECT 1 FROM events a \
               WHERE a.thread_id = $1 AND a.event_type = 'UserQuestionAnswered' \
                 AND a.payload->>'tool_use_id' = q.payload->>'tool_use_id')) \
         AND NOT {pending}",
        scope = crate::engine::session_scope::in_current_session_sql("$1", "q.sequence"),
        pending = crate::engine::handoff_sql::pending_handoff_sql("$1"),
    ))
    .bind(thread_id)
    .fetch_one(pool)
    .await
    .unwrap_or_else(|e| {
        crate::log!(
            "[TurnEnd] Plan approval of {} unreadable: {}",
            thread_id,
            lucidos_diagnostic_log::private(&e)
        );
        false
    })
}

impl crate::engine::LucidosEngine {
    pub async fn send_approved_plan_reentry(
        &self,
        thread_id: Uuid,
        msg_tx: &tokio::sync::mpsc::UnboundedSender<crate::engine::AgentUserInput>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        use crate::engine::event_bus::BusEvent;
        use crate::engine::thread_events::{
            ActorMode, EngineReason, EventChannel, EventMeta, MessageOrigin, ThreadEvent,
        };
        let origin = MessageOrigin::engine(EngineReason::ApprovedPlanUnstarted);
        let anchor = self
            .event_bus
            .emit(BusEvent::Thread {
                thread_id,
                event: ThreadEvent::PromptInjected {
                    text: APPROVED_PLAN_REENTRY_TEXT.to_string(),
                    mode: ActorMode::Engine,
                    origin: Some(origin.clone()),
                    injected_message_id: None,
                    delivered_event_id: None,
                },
                meta: EventMeta {
                    channel: Some(EventChannel::ClaudeCode),
                    actor: Some(origin),
                    ..EventMeta::NONE
                },
            })
            .await?;
        msg_tx
            .send(crate::engine::AgentUserInput {
                text: APPROVED_PLAN_REENTRY_TEXT.to_string(),
                images: None,
                origin_event_id: anchor.map(|a| a.event_id),
                kind: crate::engine::AgentInputKind::ReentryFromEngine,
                completion: None,
            })
            .map_err(|_| "Session channel closed".into())
    }
}

#[cfg(test)]
#[path = "approved_plan_tests.rs"]
mod tests;
