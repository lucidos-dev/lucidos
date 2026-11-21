//! Event boundary and anchor for re-entering a chat turn.

use uuid::Uuid;

use super::event_bus::{BusEvent, EventBus};
use super::thread_events::{
    ActorMode, EngineReason, EventChannel, EventMeta, MessageOrigin, ThreadEvent,
};

/// Type-erased return of `spawn_chat_resume`, yielding the
/// anchor claim. Boxed rather than opaque to break a mutual async recursion
/// — see that method's doc comment for why the erasure is load-bearing.
pub type ChatResumeFuture = std::pin::Pin<
    Box<
        dyn std::future::Future<
                Output = Result<ResumeAnchorClaim, Box<dyn std::error::Error + Send + Sync>>,
            > + Send,
    >,
>;

/// Where a re-entered chat turn is anchored in the timeline — the one
/// difference between the two callers of `spawn_chat_resume`.
///
/// The anchor is the `request_event_id` every event of the re-entered turn is
/// stamped with, so it decides which exchange the resumed work renders under.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChatResumeAnchor {
    /// Manual **Continue** after an interrupted turn (`continue_chat`). The
    /// previous `ResponseAborted` closed its exchange. The new attempt emits
    /// `ContinuationStarted` and a `PromptInjected` reminder about prior tool
    /// effects.
    ///
    /// `interrupted_turn` is that run's originating event, and it is the lower
    /// bound of the queued-message recovery: see
    /// [`ChatResumeAnchor::interrupted_turn`].
    ///
    /// `resumed_abort` is that run's `ResponseAborted`. The boundary claims it,
    /// so a second resume of the same abort is refused at the insert.
    NewBoundary {
        interrupted_turn: Option<Uuid>,
        resumed_abort: Option<Uuid>,
    },
    /// Continue the turn when the user answers a question that survived a
    /// restart. Carry the original `request_event_id` so events group under the
    /// existing question card. This path emits no continuation boundary or
    /// reminder.
    ExistingTurn(Uuid),
}

impl ChatResumeAnchor {
    /// The originating event of the turn the restart interrupted, which bounds
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

/// What `continue_chat` did, for its callers to log or report.
#[derive(Debug, PartialEq, Eq)]
pub enum ContinueChatOutcome {
    /// The rerun was spawned.
    Resumed,
    /// Another resume already claimed the latest `ResponseAborted`. The click
    /// is a no-op, typically a double Continue.
    AlreadyResumed,
    /// The thread waits on no answered card and has no `ResponseAborted`, or
    /// its turn has no originating event. Nothing to continue.
    NothingToContinue,
}

/// The unique index that lets one `ContinuationStarted` claim each abort.
const RESUMED_ABORT_UNIQUE_INDEX: &str = "events_resumed_abort_unique";

/// What [`emit_resume_anchor`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResumeAnchorClaim {
    /// The re-entered turn must be stamped with this `request_event_id`.
    Anchored(Uuid),
    /// Another resume already claimed this abort, and nothing was emitted.
    AlreadyResumed,
}

/// Emit whatever timeline boundary `anchor` calls for and return the
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

    // ContinuationStarted opens the new exchange in the timeline.
    let emitted = bus
        .emit(BusEvent::Thread {
            thread_id,
            event: ThreadEvent::ContinuationStarted {
                branch: String::new(),
                origin: None,
                // Chat rerun carries its own engine note, not a CC
                // continuation reason.
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

    // PromptInjected carries the engine note for the audit trail.
    // request_event_id points at ContinuationStarted so the resume
    // exchange groups this event as its own step in the timeline.
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
