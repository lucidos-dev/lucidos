use super::{BusEvent, EmitResult, EventBus, SystemEvent};
use crate::core::webhook_deliveries::{DeliveryKey, DeliveryOutcome};
use crate::engine::thread_events::MessageOrigin;
use crate::engine::trigger_context::{current_event_trigger_depth, TriggerRunStatus};

impl EventBus {
    pub async fn record_trigger_executed_at(
        &self,
        trigger_id: &str,
        status: TriggerRunStatus,
        ran_at: chrono::DateTime<chrono::Utc>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        self.emit(BusEvent::System(SystemEvent::TriggerExecuted {
            trigger_id: trigger_id.to_string(),
            payload: serde_json::json!({
                "trigger_id": trigger_id,
                "last_run": ran_at.to_rfc3339(),
                "status": status.as_str(),
            }),
        }))
        .await?;
        Ok(())
    }

    pub async fn emit_domain_event(
        &self,
        event_type: &str,
        payload: serde_json::Value,
        transient: bool,
        actor: MessageOrigin,
        emitting_trigger_id: Option<String>,
    ) -> Result<Option<EmitResult>, Box<dyn std::error::Error + Send + Sync>> {
        let event = domain_event(event_type, payload, transient, actor)?;
        self.emit_as_trigger(BusEvent::System(event), emitting_trigger_id)
            .await
    }

    pub async fn emit_webhook_delivery(
        &self,
        event_type: &str,
        payload: serde_json::Value,
        actor: MessageOrigin,
        delivery: &DeliveryKey,
    ) -> Result<DeliveryOutcome, Box<dyn std::error::Error + Send + Sync>> {
        let event = domain_event(event_type, payload, false, actor)?;
        self.append_webhook_delivery(event, delivery).await
    }
}

fn domain_event(
    event_type: &str,
    payload: serde_json::Value,
    transient: bool,
    actor: MessageOrigin,
) -> Result<SystemEvent, Box<dyn std::error::Error + Send + Sync>> {
    // INVARIANT: In the write path so no caller can skip it. The HTTP route checked
    // the name; the LLM `emit_event` tool did not, and a reserved name from
    // either writes the same permanent row.
    crate::core::event_subscription::validate_emittable_event_type(event_type)?;
    // INVARIANT: `to_payload` drops the caller's `actor`. Logged here rather than
    // there, because every reader of the event calls `to_payload` again.
    if let Some(claimed) = payload.get(SystemEvent::ACTOR_KEY) {
        log_legacy!(
            "[Events] {} carried its own `actor` ({}); recorded the engine's instead ({})",
            event_type,
            claimed,
            serde_json::to_value(&actor).unwrap_or_default()
        );
    }
    Ok(SystemEvent::DomainEvent {
        event_type: event_type.to_string(),
        payload,
        depth: current_event_trigger_depth(),
        transient,
        actor: Some(actor),
    })
}
