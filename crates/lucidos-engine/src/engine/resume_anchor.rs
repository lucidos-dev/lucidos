use uuid::Uuid;

use super::event_bus::{BusEvent, EventBus};
use super::thread_events::{
    ActorMode, EngineReason, EventChannel, EventMeta, MessageOrigin, ThreadEvent,
};

pub type ChatResumeFuture = std::pin::Pin<
    Box<
        dyn std::future::Future<
                Output = Result<ResumeAnchorClaim, Box<dyn std::error::Error + Send + Sync>>,
            > + Send,
    >,
>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChatResumeAnchor {
    NewBoundary {
        interrupted_turn: Option<Uuid>,
        resumed_abort: Option<Uuid>,
    },
    ExistingTurn(Uuid),
}

impl ChatResumeAnchor {
    /// INVARIANT: The originating event of the turn the restart interrupted, which bounds
    /// the queued-message recovery from below (`chat::queued_recovery`).
    ///
    /// `ExistingTurn` already IS that turn's `request_event_id`, since it is
    /// read off the `ToolCalled{ask_user_question}` the resume answers.
    ///
    /// `None` means the interrupted turn is unknown. Only one caller produces
    /// it: a legacy `ToolCalled` with no `request_event_id`, via
    /// `agent_question::resume_anchor_for_ask`. Recovering with no lower bound
    /// would sweep the whole thread, so that resume recovers nothing.
    pub fn interrupted_turn(self) -> Option<Uuid> {
        match self {
            Self::NewBoundary {
                interrupted_turn, ..
            } => interrupted_turn,
            Self::ExistingTurn(request_event_id) => Some(request_event_id),
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum ContinueChatOutcome {
    Resumed,
    AlreadyResumed,
    NothingToContinue,
}

const RESUMED_ABORT_UNIQUE_INDEX: &str = "events_resumed_abort_unique";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResumeAnchorClaim {
    Anchored(Uuid),
    AlreadyResumed,
}

/// INVARIANT: Emit whatever timeline boundary `anchor` calls for and return the
/// `request_event_id` the re-entered turn must be stamped with.
///
/// The `ContinuationStarted` insert is the claim on the abort it resumes. Two
/// concurrent resumes of one abort both reach it, and the index refuses the
/// second, so only one rerun ever spawns.
///
/// Emit the resume boundary independently of spawning the agentic loop. Tests
/// can check the resulting events with a bare `EventBus`.
pub async fn emit_resume_anchor(
    bus: &EventBus,
    thread_id: Uuid,
    anchor: ChatResumeAnchor,
    engine_note: &str,
    channel: EventChannel,
    actor: Option<MessageOrigin>,
) -> Result<ResumeAnchorClaim, Box<dyn std::error::Error + Send + Sync>> {
    let resumed_abort = match anchor {
        ChatResumeAnchor::ExistingTurn(request_event_id) => {
            return Ok(ResumeAnchorClaim::Anchored(request_event_id))
        }
        ChatResumeAnchor::NewBoundary { resumed_abort, .. } => resumed_abort,
    };

    let emitted = bus
        .emit(BusEvent::Thread {
            thread_id,
            event: ThreadEvent::ContinuationStarted {
                branch: String::new(),
                origin: None,
                reason: None,
                resumed_abort_id: resumed_abort,
            },
            meta: EventMeta {
                channel: Some(channel),
                actor,
                ..EventMeta::NONE
            },
        })
        .await;
    let continuation_started_id = match emitted {
        Ok(result) => result.expect("ContinuationStarted is persisted").event_id,
        Err(e)
            if crate::engine::event_bus::violates_unique_index(&*e, RESUMED_ABORT_UNIQUE_INDEX) =>
        {
            return Ok(ResumeAnchorClaim::AlreadyResumed)
        }
        Err(e) => return Err(e),
    };

    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::PromptInjected {
            text: engine_note.to_string(),
            mode: ActorMode::Engine,
            origin: Some(MessageOrigin::engine(EngineReason::ContinuationStarted)),
            injected_message_id: None,
            delivered_event_id: None,
        },
        meta: EventMeta {
            channel: Some(channel),
            request_event_id: Some(continuation_started_id),
            actor: Some(MessageOrigin::engine(EngineReason::ContinuationStarted)),
            ..EventMeta::NONE
        },
    })
    .await?;

    Ok(ResumeAnchorClaim::Anchored(continuation_started_id))
}
