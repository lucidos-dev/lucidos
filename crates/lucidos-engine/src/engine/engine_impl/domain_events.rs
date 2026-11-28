use super::super::*;
use crate::core::webhook_deliveries::{DeliveryKey, DeliveryOutcome};
use crate::engine::thread_events::MessageOrigin;

impl LucidosEngine {
    pub async fn emit_domain_event(
        &self,
        event_type: &str,
        payload: serde_json::Value,
        actor: MessageOrigin,
        emitting_trigger_id: Option<String>,
    ) -> Result<uuid::Uuid, Box<dyn std::error::Error + Send + Sync>> {
        let result = self
            .event_bus
            .emit_domain_event(event_type, payload, false, actor, emitting_trigger_id)
            .await?;
        Ok(result
            .expect("non-transient DomainEvent always returns EmitResult")
            .event_id)
    }

    pub async fn emit_webhook_delivery(
        &self,
        event_type: &str,
        payload: serde_json::Value,
        actor: MessageOrigin,
        delivery: &DeliveryKey,
    ) -> Result<DeliveryOutcome, Box<dyn std::error::Error + Send + Sync>> {
        self.event_bus
            .emit_webhook_delivery(event_type, payload, actor, delivery)
            .await
    }

    /// INVARIANT: Broadcast a domain event on SSE without writing it to the events table.
    /// Used for high-churn coordination signals (heartbeats, presenter↔remote
    /// state) where the audit trail isn't valuable.
    ///
    /// `emitting_trigger_id` means what it does on [`Self::emit_domain_event`].
    /// A transient frame reaches the trigger matcher too, so it needs the same
    /// marker.
    pub async fn broadcast_transient_domain_event(
        &self,
        event_type: &str,
        payload: serde_json::Value,
        actor: MessageOrigin,
        emitting_trigger_id: Option<String>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        self.event_bus
            .emit_domain_event(event_type, payload, true, actor, emitting_trigger_id)
            .await?;
        Ok(())
    }

    pub async fn record_trigger_executed(
        &self,
        trigger_id: &str,
        status: crate::triggers::TriggerRunStatus,
    ) {
        if let Err(error) = self
            .record_trigger_executed_at(trigger_id, status, chrono::Utc::now())
            .await
        {
            log_legacy!(
                "[Triggers] Failed to persist TriggerExecuted event: {}",
                error
            );
        }
    }

    pub(crate) async fn record_trigger_executed_at(
        &self,
        trigger_id: &str,
        status: crate::triggers::TriggerRunStatus,
        ran_at: chrono::DateTime<chrono::Utc>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        self.event_bus
            .record_trigger_executed_at(trigger_id, status, ran_at)
            .await?;
        let mut configs = self.trigger_configs.write().unwrap();
        if let Some(config) = configs.get_mut(trigger_id) {
            config.last_run = Some(ran_at);
            config.last_run_status = Some(status);
        }
        Ok(())
    }
}
