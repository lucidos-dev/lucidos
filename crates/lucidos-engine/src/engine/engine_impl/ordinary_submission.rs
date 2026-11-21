use crate::engine::event_bus::{ordinary_message_id, OrdinaryAcceptance, OrdinarySubmissionIntent};
use crate::engine::thread_events::MessageOrigin;
use crate::engine::thread_queue::ThreadQueueRequest;
use crate::engine::{InputPosition, LucidosEngine};

impl LucidosEngine {
    /// Accept a fully resolved ordinary input. Transports authenticate before
    /// calling this; EventBus retains its input turn for durable queue dispatch.
    pub async fn accept_ordinary_submission(
        &self,
        request: ThreadQueueRequest,
        actor: MessageOrigin,
        chain: Option<&str>,
        sequence: Option<i64>,
        expected_epoch: Option<i64>,
        intent: OrdinarySubmissionIntent,
    ) -> Result<OrdinaryAcceptance, Box<dyn std::error::Error + Send + Sync>> {
        let id = ordinary_message_id(&request)?;
        let position = InputPosition::from_wire(chain, sequence, Some(id))?;
        self.event_bus
            .accept_ordinary_submission(
                request,
                actor,
                &self.input_order,
                position,
                expected_epoch,
                intent,
            )
            .await
    }
}
