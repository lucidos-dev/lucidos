//! One call, from the opening payload to the paired end event.
//!
//! Provider-agnostic above the seam and socket-agnostic below it: the loop
//! talks to a [`VoiceProvider`] on one side and a [`CallTransport`] on the
//! other. `api::voice` supplies a WebSocket transport, and the tests supply a
//! scripted one, so the loop is exercised without a socket or a credential.
//!
//! It takes the opening payload rather than building one. Assembling the
//! resident block reads half the workspace ([`opening_for`] is where that
//! happens), and driving a call needs none of it.
//!
//! **The thread is the third party in the room.** A finished utterance wakes
//! the thread's own agent through a [`TurnStarter`]. What that agent produces
//! comes back over the EventBus, so the loop selects over three things: the
//! caller, the talker, and the thread.

use std::collections::VecDeque;
use std::time::Instant;

use async_trait::async_trait;
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::broadcast::Receiver;
use uuid::Uuid;

use super::provider::{SessionOpening, VoiceEvent, VoiceProvider, VoiceSession};
use super::reasoner::TurnStarter;
use super::wire::{ClientControl, ServerFrame};
use super::{resident, TALKER_INSTRUCTIONS};
use crate::engine::event_bus::{BusEvent, EmittedEvent, EventBus};
use crate::engine::thread_events::{
    AgentParticipant, CancelCause, EventMeta, MessageOrigin, ThreadEvent, VoiceSessionEndReason,
};
use crate::engine::{AuxCapture, ContextPurpose, LucidosEngine};

type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// The voice the talker speaks in.
///
/// Fixed rather than configurable, because nothing can hear it yet: a setting
/// nobody can evaluate is a setting nobody can choose. It becomes one alongside
/// the resident block's own screen, in the phase that ships a client. Both
/// deferrals are recorded in
/// `docs/plans/2026-08-29-a-voice-session-opens-behind-one-seam.md`.
const TALKER_VOICE: &str = "marin";

/// What the rendered history calls the talker.
///
/// Its own label, so the reasoner never reads a spoken turn as its own prior
/// turn (ADR 0150). The user never meets this name: they hear one entity, and
/// the transcript renders a spoken turn as Lucidos.
const TALKER_LABEL: &str = "Lucidos (aloud)";

/// What the talker is told when the reasoner's turn did not finish.
///
/// It is told what happened rather than handed a sentence, because the honesty
/// constraint decides the words. Saying nothing would leave the caller waiting
/// for an answer that is never coming.
const UNFINISHED: &str =
    "[ANSWER] That did not finish. Tell the caller so, and offer to try again.";

/// What arrived from the caller.
#[derive(Debug, Clone, PartialEq)]
pub enum CallerFrame {
    /// Microphone audio, in the PCM the opening frame named.
    Audio(Vec<u8>),
    Control(ClientControl),
    /// A text frame that is not a control we know.
    Undecodable,
    /// The socket is gone.
    Closed,
}

/// The caller's end of a call. One implementation per way of reaching a person.
#[async_trait]
pub trait CallTransport: Send {
    async fn recv(&mut self) -> CallerFrame;
    async fn send_audio(&mut self, pcm: Vec<u8>) -> Result<(), BoxError>;
    async fn send_frame(&mut self, frame: ServerFrame) -> Result<(), BoxError>;
}

/// Who this call is for, and what it stamps on what it writes.
#[derive(Debug, Clone)]
pub struct CallSubject {
    pub thread_id: Uuid,
    pub session_id: Uuid,
    /// The device that placed the call. It rides the session events. It is also
    /// the actor on every message the caller speaks, exactly as it would be on
    /// one they typed from the same phone.
    pub actor: Option<MessageOrigin>,
}

/// Hold one call open until it ends, and record what it did.
///
/// Returns the reason it ended, or `None` when the talker never answered at
/// all. That case writes NO events: a start with no call behind it would make
/// the pair count sessions that never happened.
pub async fn run_call(
    bus: &EventBus,
    provider: &dyn VoiceProvider,
    transport: &mut dyn CallTransport,
    reasoner: &dyn TurnStarter,
    opening: SessionOpening,
    subject: CallSubject,
) -> Option<VoiceSessionEndReason> {
    let audio = opening.audio;

    // Subscribed before the talker can produce a word. Nothing the reasoner
    // emits can then land in the gap between opening a session and watching
    // for one.
    let thread = bus.subscribe();

    let mut session = match provider.open(opening).await {
        Ok(session) => session,
        Err(e) => {
            log!(
                "[Voice] {} could not open a session: {}",
                provider.name(),
                e
            );
            let _ = transport
                .send_frame(ServerFrame::Error {
                    message: "The voice service could not be reached. Try again in a moment."
                        .to_string(),
                })
                .await;
            return None;
        }
    };

    emit(
        bus,
        subject.thread_id,
        ThreadEvent::VoiceSessionStarted {
            session_id: subject.session_id,
        },
        EventMeta::with_actor(subject.actor.clone()),
    )
    .await;
    let started = Instant::now();
    if transport
        .send_frame(ServerFrame::SessionStarted {
            audio: audio.into(),
        })
        .await
        .is_err()
    {
        log!("[Voice] The caller was gone before the call opened");
    }

    let mut call = Call {
        bus: bus.clone(),
        provider,
        reasoner,
        capture: AuxCapture::new(bus, subject.thread_id, ContextPurpose::Voice),
        subject: subject.clone(),
        thread,
        talker_has_the_floor: false,
        interrupted: false,
        waiting_to_be_said: VecDeque::new(),
    };
    let reason = call.drive(&mut *session, transport).await;
    session.close().await;

    // Dropped on failure, deliberately. The call is already over, and the
    // reason is on its way into the event log. A caller who has gone missed
    // only a courtesy.
    let _ = transport
        .send_frame(ServerFrame::SessionEnded { reason })
        .await;
    emit(
        bus,
        subject.thread_id,
        ThreadEvent::VoiceSessionEnded {
            session_id: subject.session_id,
            reason,
            duration_secs: started.elapsed().as_secs(),
        },
        EventMeta::with_actor(subject.actor),
    )
    .await;
    Some(reason)
}

/// What one poll of the three inputs produced.
///
/// The select resolves to one of these and nothing else. Acting on it happens
/// after the select statement ends, where the talker session is free to be
/// borrowed again.
enum Step {
    /// Nothing to act on this round.
    Nothing,
    CallerAudio(Vec<u8>),
    BargeIn,
    Undecodable,
    Talker(VoiceEvent),
    /// The talker closed the session on its own side.
    TalkerGone,
    /// Boxed because the payload dwarfs every other variant here.
    Thread(Box<EmittedEvent>),
    Ended(VoiceSessionEndReason),
}

/// Everything one live call carries between its three inputs.
///
/// A struct rather than a column of arguments. The caller, the talker and the
/// thread all read and write the same floor state. Passing that around by hand
/// is how the two halves of one rule drift apart.
struct Call<'a> {
    bus: EventBus,
    provider: &'a dyn VoiceProvider,
    reasoner: &'a dyn TurnStarter,
    capture: AuxCapture,
    subject: CallSubject,
    thread: Receiver<EmittedEvent>,
    /// True while the talker owes the caller the rest of a sentence.
    talker_has_the_floor: bool,
    /// The caller cut in. Read by the turn end that follows it.
    interrupted: bool,
    /// Reasoner answers that arrived while the talker was speaking. A queue
    /// rather than a slot, because dropping one loses an answer the caller
    /// asked for and never hears.
    waiting_to_be_said: VecDeque<String>,
}

impl Call<'_> {
    /// Pump between the caller, the talker and the thread until one stops.
    async fn drive(
        &mut self,
        session: &mut dyn VoiceSession,
        transport: &mut dyn CallTransport,
    ) -> VoiceSessionEndReason {
        loop {
            let step = tokio::select! {
                from_caller = transport.recv() => from_caller.into(),
                from_talker = session.next() => match from_talker {
                    Some(event) => Step::Talker(event),
                    None => Step::TalkerGone,
                },
                from_thread = self.thread.recv() => thread_step(from_thread),
            };

            match step {
                Step::Nothing => {}
                Step::CallerAudio(pcm) => {
                    if let Err(e) = session.push_audio(&pcm).await {
                        log!("[Voice] The talker stopped taking audio: {}", e);
                        return provider_failed(transport).await;
                    }
                }
                Step::BargeIn => {
                    if let Err(e) = session.cancel().await {
                        log!("[Voice] Could not interrupt the talker: {}", e);
                    }
                }
                Step::Undecodable => {
                    let _ = transport
                        .send_frame(ServerFrame::Error {
                            message: "That control was not one this call understands.".to_string(),
                        })
                        .await;
                }
                Step::Talker(event) => match self.forward(event, session, transport).await {
                    None => {}
                    Some(VoiceSessionEndReason::ProviderFailed) => {
                        return provider_failed(transport).await
                    }
                    Some(reason) => return reason,
                },
                Step::TalkerGone => {
                    log!(
                        "[Voice] {} ended the session on its side",
                        self.provider.name()
                    );
                    return provider_failed(transport).await;
                }
                Step::Thread(emitted) => self.on_thread_event(*emitted, session).await,
                Step::Ended(reason) => return reason,
            }
        }
    }

    /// Send one talker event on to the caller, and write down what it means.
    ///
    /// `None` means carry on. `Some(reason)` ends the call, and WHICH reason
    /// matters: a send that fails means the caller is gone, not that the talker
    /// broke. Reporting a dropped phone as `provider_failed` puts the blame in
    /// the event log, where a trigger can match on it.
    async fn forward(
        &mut self,
        event: VoiceEvent,
        session: &mut dyn VoiceSession,
        transport: &mut dyn CallTransport,
    ) -> Option<VoiceSessionEndReason> {
        let frame = match event {
            VoiceEvent::Audio(pcm) => {
                self.talker_has_the_floor = true;
                return delivered(transport.send_audio(pcm).await);
            }
            VoiceEvent::UserTurnEnded { transcript } => {
                self.reasoner
                    .heard(
                        self.subject.thread_id,
                        &transcript,
                        self.subject.actor.clone(),
                    )
                    .await;
                ServerFrame::UserTurnEnded { transcript }
            }
            VoiceEvent::TalkerTranscript { text } => {
                self.talker_has_the_floor = true;
                ServerFrame::TalkerTranscript { text }
            }
            VoiceEvent::TalkerTurnEnded { transcript, usage } => {
                // Audio has no chars, so the estimate is zero and `usage`
                // carries the only real number. A rollup reading measured spend
                // reads that.
                self.capture
                    .record_usage(self.provider.model(), 0, Some(usage))
                    .await;
                self.record_spoken(transcript).await;
                self.talker_has_the_floor = false;
                self.say_what_is_waiting(session).await;
                ServerFrame::TalkerTurnEnded
            }
            VoiceEvent::Interrupted => {
                self.interrupted = true;
                ServerFrame::Interrupted
            }
            VoiceEvent::Failed { message } => {
                log!(
                    "[Voice] {} failed mid-call: {}",
                    self.provider.name(),
                    message
                );
                return Some(VoiceSessionEndReason::ProviderFailed);
            }
        };
        delivered(transport.send_frame(frame).await)
    }

    /// What one event on this thread means to the talker.
    ///
    /// Progress is appended silently, so the talker can answer "what are you
    /// doing?" truthfully without narrating every step unasked. An answer is
    /// spoken, because it is the thing the caller is waiting for.
    async fn on_thread_event(&mut self, emitted: EmittedEvent, session: &mut dyn VoiceSession) {
        let BusEvent::Thread {
            thread_id, event, ..
        } = &emitted.typed
        else {
            return;
        };
        if *thread_id != self.subject.thread_id {
            return;
        }
        match event {
            ThreadEvent::ToolCalled { name, .. } => {
                let note = format!("[WORKING] Using {}.", name);
                self.append(session, &note).await;
            }
            ThreadEvent::ResponseGenerated { text, .. } if !text.trim().is_empty() => {
                let answer = format!("[ANSWER] Say this to the caller: {}", text.trim());
                self.say(session, answer).await;
            }
            ThreadEvent::ResponseFailed { .. } | ThreadEvent::ResponseAborted { .. } => {
                self.say(session, UNFINISHED.to_string()).await;
            }
            // A cancel splits on WHY, and only one half is worth saying.
            //
            // `SupersededByFollowup` is the caller talking over the answer,
            // which is how people talk. The turn that replaced it is already
            // running, so "that did not finish" would talk over the real answer
            // on its way. A Stop is the other half: nothing is coming, and a
            // caller left waiting in silence is the failure this reports.
            ThreadEvent::ResponseCanceled { cause, .. }
                if *cause != CancelCause::SupersededByFollowup =>
            {
                self.say(session, UNFINISHED.to_string()).await;
            }
            _ => {}
        }
    }

    /// Say this next, or queue it while the talker is mid-sentence.
    ///
    /// Two replies at once is the failure a listener cannot recover from. The
    /// floor is checked here and nowhere else.
    async fn say(&mut self, session: &mut dyn VoiceSession, note: String) {
        if self.talker_has_the_floor {
            self.waiting_to_be_said.push_back(note);
            return;
        }
        // Claimed BEFORE the request, not when the first audio arrives. A
        // second answer landing in that window would otherwise be spoken over
        // the one already on its way.
        self.talker_has_the_floor = true;
        if let Err(e) = session.speak(&note).await {
            log!("[Voice] The talker would not take the answer: {}", e);
            self.talker_has_the_floor = false;
        }
    }

    /// Release the next queued answer, now that the talker has stopped.
    async fn say_what_is_waiting(&mut self, session: &mut dyn VoiceSession) {
        if let Some(note) = self.waiting_to_be_said.pop_front() {
            self.say(session, note).await;
        }
    }

    async fn append(&mut self, session: &mut dyn VoiceSession, note: &str) {
        if let Err(e) = session.append_context(note).await {
            log!("[Voice] The talker would not take a progress note: {}", e);
        }
    }

    /// Write down what the caller just heard.
    ///
    /// Attributed to the talker, so `history.rs` gives it its own speaker label
    /// and the reasoner never reads it as its own prior turn (ADR 0150).
    ///
    /// A reply with no words is not written. A cancelled response can end
    /// before the talker said anything, and an empty row would claim the caller
    /// heard something they did not.
    async fn record_spoken(&mut self, transcript: String) {
        let interrupted = std::mem::take(&mut self.interrupted);
        if transcript.trim().is_empty() {
            return;
        }
        emit(
            &self.bus,
            self.subject.thread_id,
            ThreadEvent::SpokenReplyGenerated {
                session_id: self.subject.session_id,
                text: transcript,
                interrupted,
            },
            EventMeta::NONE.authored_by(AgentParticipant::Guest {
                label: TALKER_LABEL.to_string(),
            }),
        )
        .await;
    }
}

impl From<CallerFrame> for Step {
    fn from(frame: CallerFrame) -> Self {
        match frame {
            CallerFrame::Audio(pcm) => Step::CallerAudio(pcm),
            CallerFrame::Control(ClientControl::BargeIn) => Step::BargeIn,
            CallerFrame::Control(ClientControl::HangUp) => {
                Step::Ended(VoiceSessionEndReason::Hangup)
            }
            CallerFrame::Undecodable => Step::Undecodable,
            CallerFrame::Closed => Step::Ended(VoiceSessionEndReason::Disconnected),
        }
    }
}

/// One read of the thread's traffic, as a step.
fn thread_step(received: Result<EmittedEvent, RecvError>) -> Step {
    match received {
        Ok(emitted) => Step::Thread(Box::new(emitted)),
        // A busy call missed some of the thread's traffic. Narration is
        // best-effort, so carry on rather than ending a call over a lost
        // progress note.
        Err(RecvError::Lagged(missed)) => {
            log!("[Voice] The call missed {} thread events", missed);
            Step::Nothing
        }
        // The bus is gone, so the engine is going down under the call.
        Err(RecvError::Closed) => Step::Ended(VoiceSessionEndReason::EngineShutdown),
    }
}

/// Read one send to the caller as a reason to stop, or as nothing at all.
fn delivered(sent: Result<(), BoxError>) -> Option<VoiceSessionEndReason> {
    match sent {
        Ok(()) => None,
        Err(e) => {
            log!("[Voice] The caller stopped receiving: {}", e);
            Some(VoiceSessionEndReason::Disconnected)
        }
    }
}

/// Tell the caller the talker gave up, then report it.
///
/// Only for a provider failure. A caller who is already gone cannot read an
/// error frame, and sending one would say the talker broke when it did not.
async fn provider_failed(transport: &mut dyn CallTransport) -> VoiceSessionEndReason {
    let _ = transport
        .send_frame(ServerFrame::Error {
            message: "The voice service stopped responding.".to_string(),
        })
        .await;
    VoiceSessionEndReason::ProviderFailed
}

async fn emit(bus: &EventBus, thread_id: Uuid, event: ThreadEvent, meta: EventMeta) {
    let ctx = format!("[Voice] {}", event.event_type());
    bus.emit_or_log(
        BusEvent::Thread {
            thread_id,
            event,
            // No channel on any of these. Voice is a mode of a chat thread
            // (ADR 0148), and stamping one here is how a fourth `EventChannel`
            // starts.
            meta,
        },
        &ctx,
    )
    .await;
}

/// What a call on this thread opens with: the stable persona, and the resident
/// block built fresh from the workspace as it is now.
///
/// Separate from [`run_call`] because it reads half the workspace, and driving
/// a call reads none of it.
pub async fn opening_for(engine: &LucidosEngine, thread_id: Uuid) -> SessionOpening {
    SessionOpening {
        instructions: TALKER_INSTRUCTIONS.to_string(),
        resident_block: resident::build_block(engine, thread_id).await,
        voice: TALKER_VOICE.to_string(),
        audio: Default::default(),
    }
}

#[cfg(test)]
#[path = "call_tests.rs"]
mod tests;
