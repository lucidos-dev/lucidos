//! Serialize provider hold transitions with their event announcements.
use crate::engine::event_bus::{BusEvent, EventBus, SystemEvent};
use crate::engine::thread_events::MessageOrigin;
use crate::llm::spend_cap::{
    SpendCapHold, SpendCapHoldChange, SpendCapHoldEndCause, SpendCapReached,
};
use crate::llm::ProviderKind;

type BoxError = Box<dyn std::error::Error + Send + Sync>;

#[async_trait::async_trait]
impl crate::llm::spend_cap::SpendCapPolicy for EventBus {
    async fn check(
        &self,
        provider: Option<ProviderKind>,
        interactive: bool,
    ) -> Result<(), BoxError> {
        check(self, provider, interactive).await
    }

    async fn failed(&self, error: &(dyn std::error::Error + Send + Sync + 'static)) {
        failed(self, error).await;
    }

    async fn succeeded(&self, provider: Option<ProviderKind>) {
        succeeded(self, provider).await;
    }
}

async fn announce(
    bus: &EventBus,
    changes: impl IntoIterator<Item = SpendCapHoldChange>,
    actor: Option<MessageOrigin>,
) {
    for change in changes {
        let event = match change {
            SpendCapHoldChange::Started(hold) => SystemEvent::ModelSpendCapReached {
                provider: hold.provider,
                resets_at: hold.resets_at,
                held_until: hold.held_until,
            },
            SpendCapHoldChange::Ended { provider, cause } => SystemEvent::ModelSpendCapHoldEnded {
                provider: provider.as_str().to_owned(),
                cause,
                actor: actor.clone(),
            },
        };
        bus.emit_or_log(BusEvent::System(event), "[ModelCall] spend cap hold")
            .await;
    }
}

pub(crate) async fn check(
    bus: &EventBus,
    provider: Option<ProviderKind>,
    interactive: bool,
) -> Result<(), BoxError> {
    let mut holds = bus.spend_cap_holds().lock().await;
    announce(bus, holds.expire(chrono::Utc::now()), None).await;
    if !interactive {
        if let Some(refusal) = provider.and_then(|provider| holds.refusal(provider)) {
            return Err(refusal.into());
        }
    }
    Ok(())
}

pub(crate) async fn failed(
    bus: &EventBus,
    error: &(dyn std::error::Error + Send + Sync + 'static),
) {
    let Some(cap) = error.downcast_ref::<SpendCapReached>() else {
        return;
    };
    let mut holds = bus.spend_cap_holds().lock().await;
    let now = chrono::Utc::now();
    announce(bus, holds.expire(now), None).await;
    let change = holds.reached(cap.clone(), now);
    let until = match &change {
        Some(SpendCapHoldChange::Started(hold)) => Some(hold.held_until),
        _ => None,
    };
    announce(bus, change, None).await;
    drop(holds);
    if let Some(until) = until {
        let bus = bus.clone();
        tokio::spawn(async move {
            tokio::time::sleep((until - chrono::Utc::now()).to_std().unwrap_or_default()).await;
            // Expire even in an idle workspace, so open settings receive the
            // end event without needing a new model call or page refresh.
            status(&bus).await;
        });
    }
}

pub(crate) async fn succeeded(bus: &EventBus, provider: Option<ProviderKind>) {
    let Some(provider) = provider else { return };
    let mut holds = bus.spend_cap_holds().lock().await;
    announce(bus, holds.expire(chrono::Utc::now()), None).await;
    announce(
        bus,
        holds.end(provider, SpendCapHoldEndCause::Succeeded),
        None,
    )
    .await;
}

pub(crate) async fn status(bus: &EventBus) -> Vec<SpendCapHold> {
    let mut holds = bus.spend_cap_holds().lock().await;
    announce(bus, holds.expire(chrono::Utc::now()), None).await;
    holds.snapshots()
}

pub(crate) async fn clear(bus: &EventBus, provider: ProviderKind, actor: MessageOrigin) {
    let mut holds = bus.spend_cap_holds().lock().await;
    announce(bus, holds.expire(chrono::Utc::now()), None).await;
    announce(
        bus,
        holds.end(provider, SpendCapHoldEndCause::Cleared),
        Some(actor),
    )
    .await;
}
