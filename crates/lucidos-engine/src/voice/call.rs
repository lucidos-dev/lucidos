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
//!
//! **What the thread is waiting on goes through a third seam.** A card the
//! caller can settle is read and resolved by a [`DecisionResolver`], never by
//! this file reaching into the engine. That is also what refuses a delegation
//! while the doer is parked inside a card of its own.

use std::collections::VecDeque;
use std::time::Instant;

use async_trait::async_trait;
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::broadcast::Receiver;
use uuid::Uuid;

use super::decision::{DecisionKind, DecisionResolver, OpenDecision, Resolution};
use super::doer::TurnStarter;
use super::provider::{SessionOpening, VoiceEvent, VoiceProvider, VoiceSession};
use super::wire::{ClientControl, ServerFrame};
use super::{build, language, resident};
use crate::engine::event_bus::{BusEvent, EmittedEvent, EventBus};
use crate::engine::thread_events::{
    AgentParticipant, CancelCause, EventMeta, MessageOrigin, ThreadEvent, VoiceSessionEndReason,
};
use crate::engine::{AuxCapture, ContextPurpose, LucidosEngine};
use crate::llm::tool_names;

type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// What the rendered history calls the talker.
///
/// Its own label, so the doer never reads a spoken turn as its own prior
/// turn (ADR 0150). The user never meets this name: they hear one entity, and
/// the transcript renders a spoken turn as Lucidos.
const TALKER_LABEL: &str = "Lucidos (aloud)";

/// What the talker is told when the doer's turn did not finish.
///
/// It is told what happened rather than handed a sentence, because the honesty
/// constraint decides the words. Saying nothing would leave the caller waiting
/// for an answer that is never coming.
const UNFINISHED: &str =
    "[ANSWER] That did not finish. Tell the caller so, and offer to try again.";

/// What the talker is told when the doer would not take an utterance at all.
///
/// A call reaches the Lucidos Agent and nothing else (ADR 0165). A compose
/// draft can move to a coding agent while a call is already up. Distinct from
/// [`UNFINISHED`], which reports work that started and stopped. Nothing started
/// here, so "try again" would be advice that cannot work.
///
/// Said rather than swallowed, because a caller left waiting in silence is the
/// one failure a call cannot recover from on its own.
const NOT_TAKEN: &str = "[ANSWER] That could not be started on this conversation. \
                         Tell the caller so, and say they can type it instead.";

/// What the talker is told once something waiting has been settled.
///
/// Appended rather than spoken. Whoever settled it already knows: the caller
/// either said so out loud or pressed it on screen. What it prevents is the
/// talker offering the same card again from a note it still holds.
///
/// It names neither route, because the engine sees one event either way and
/// must state no fact it was not given (ADR 0149).
const DECISION_SETTLED: &str = "\
[SETTLED] That is settled now, so do not put it to them again.";

/// What the talker is told when its `delegate` call landed.
///
/// Deliberately says nothing about timing or about who is doing the work. The
/// talker already told the caller it is on it, and a second promise here is
/// one more thing that can turn out false.
const DELEGATION_TAKEN: &str = "Taken. The answer will arrive separately.";

/// What the talker is told when a delegation could not start anything.
///
/// It states a FACT rather than a policy: the doer is blocked inside the very
/// card that is waiting, so there is no turn to start. The answer is what frees
/// it, which is why the note points back at the card.
const DELEGATION_PARKED: &str = "\
Not started. Lucidos is waiting on the caller's answer to what is already open, \
so nothing new can run until they settle it. Put that back to them, and answer \
it with what they say.";

/// What the talker is told when its `answer` call settled the card.
const ANSWER_TAKEN: &str = "Answered. That is settled now.";

/// What the talker is told when its `hang_up` call landed.
///
/// The goodbye is still being spoken when this goes over, so the talker really
/// does read it. It says nothing about timing, because the line closes when
/// that turn ends rather than now.
const HANGUP_TAKEN: &str = "Ending the call now.";

/// What the talker is told when a held answer never got the caller's words.
///
/// Two ways there: a second `answer` displaces the first, or the utterance
/// after it carried nothing. Owed either way, per [`Call::acknowledge`].
const NEVER_HEARD: &str = "\
That one was dropped: the caller's own words never came through for it. Ask \
them again if it still matters.";

/// What the talker is told when an answer spent the words its ask was waiting
/// for.
///
/// Appended, because the ask's own acknowledgement went out when it was made
/// and a tool call is answered once. Its reason follows, so the talker can
/// offer the caller the thing that did not run.
const ASK_LOST_ITS_WORDS: &str = "\
[NOT STARTED] The caller's words settled what was waiting, so the request \
below never started. Nothing is running for it. Offer it to them again if they \
still want it.";

/// Above this many characters, an answer is offered rather than delivered.
///
/// The doer writes for a reader: headings, tables, code, links. Read out,
/// any of it is unbearable, so the talker always says what an answer MEANS.
/// Past this length there is more meaning than a listener can hold, and the
/// talker gives the headline and offers the rest.
///
/// Roughly four spoken sentences. A guess until it has been heard, and the
/// place to revisit it is
/// `docs/plans/2026-08-29-a-spoken-turn-reads-as-spoken.md`.
const OFFER_THE_DETAIL_ABOVE_CHARS: usize = 400;

/// Hand the doer's answer to the talker, as something to say.
///
/// Never as something to read. The caller is on a phone, and the answer is a
/// document. Both framings carry the text in FULL, for two reasons. The talker
/// holds no tools, so a fact trimmed here is one it can only invent later (ADR
/// 0149). And "yes, go on" is answerable only from what it was given.
fn answer_to_say(text: &str) -> String {
    let opening = if text.chars().count() > OFFER_THE_DETAIL_ABOVE_CHARS {
        "[ANSWER] This came back for the caller. It is long, so say the short \
         version out loud, then ask whether they want the detail. Do not read \
         it out."
    } else {
        "[ANSWER] This came back for the caller. Say what it means out loud, in \
         your own words. Do not read it out."
    };
    format!("{}\n\n{}", opening, text)
}

/// Hand the talker something the thread is waiting on, as something to put to
/// the caller.
///
/// Spoken rather than appended, because the turn behind it is parked on a
/// person. Nothing else is coming, so a talker that stays quiet leaves the
/// caller waiting for an answer that never arrives.
///
/// It says where the answer goes, and that is now HERE. The caller settles it
/// out loud, and the talker hands back the id of the choice they picked.
fn decision_to_ask(decision: &OpenDecision) -> String {
    // Exhaustive, so a fourth kind has to decide what the caller hears rather
    // than inheriting the permission wording by default.
    let opening = match decision.kind {
        DecisionKind::Question => "[QUESTION] The work is waiting on the caller's answer.",
        DecisionKind::CommandPermission
        | DecisionKind::McpPermission
        | DecisionKind::CodingAgentPermission => {
            "[PERMISSION] Lucidos needs the caller's say-so before it can carry on."
        }
    };
    // The prompt itself is NEVER cut, unlike everything else the talker reads.
    // A truncated question is a different question, and the talker is about to
    // state it as the one being asked.
    format!(
        "{} Put this to them out loud, in your own words, and read them the \
         choices. They answer by saying which one they want, and you hand its \
         id back. Never say an id out loud.\n\n{}\n\n{}",
        opening,
        decision.prompt,
        super::choices_for(&decision.choices),
    )
}

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
    doer: &dyn TurnStarter,
    decisions: &dyn DecisionResolver,
    opening: SessionOpening,
    subject: CallSubject,
) -> Option<VoiceSessionEndReason> {
    let audio = opening.audio;

    // Subscribed before the talker can produce a word. Nothing the doer
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
        doer,
        decisions,
        capture: AuxCapture::new(bus, subject.thread_id, ContextPurpose::Voice),
        subject: subject.clone(),
        thread,
        talker_has_the_floor: false,
        interrupted: false,
        relaying: false,
        waiting_to_be_said: VecDeque::new(),
        pending_utterance: None,
        pending_delegation: None,
        pending_answer: None,
        hanging_up: false,
        delegated_this_turn: false,
    };
    let reason = call.drive(&mut *session, transport).await;
    // Before the session closes, and for EVERY end reason. A hangup, a dropped
    // socket and a provider failure all leave the same thing held, and the
    // caller said it whether or not anybody answered.
    call.write_down_whatever_is_left().await;
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
    doer: &'a dyn TurnStarter,
    /// What this call can do about what is waiting on its own thread.
    decisions: &'a dyn DecisionResolver,
    capture: AuxCapture,
    subject: CallSubject,
    thread: Receiver<EmittedEvent>,
    /// True while the talker owes the caller the rest of a sentence.
    talker_has_the_floor: bool,
    /// The caller cut in. Read by the turn end that follows it.
    interrupted: bool,
    /// The reply the talker is composing was handed to it, so a running round
    /// already knows what it says. Set by [`Call::say`], read and cleared by
    /// the turn end that follows.
    relaying: bool,
    /// Doer answers that arrived while the talker was speaking. A queue
    /// rather than a slot, because dropping one loses an answer the caller
    /// asked for and never hears.
    waiting_to_be_said: VecDeque<String>,
    /// A finished utterance not yet written down.
    ///
    /// Held because how it is recorded depends on what the talker does next.
    /// Delegated, it becomes a `MessageReceived` and runs a turn. Handled
    /// alone, it becomes a `SpokenMessageReceived` and runs nothing. Recording
    /// it on arrival would mean choosing before the answer is known.
    pending_utterance: Option<String>,
    /// The talker's reason from a `delegate` call with no utterance to pair
    /// with yet.
    ///
    /// **Sticky on purpose, and it outlives the turn that made it.** The
    /// transcript and the tool call come from two models on one socket, so a
    /// short fast reply produces the call first. Cleared at the turn's end,
    /// this would drop the caller's real question into a row that starts
    /// nothing. That is the failure the whole tool exists to end.
    pending_delegation: Option<String>,
    /// An `answer` call whose choice sends the caller's own words, waiting for
    /// the transcript to catch up.
    ///
    /// The same race the ask has, held the same way. Only the one choice that
    /// carries a transcript can end up here: every other choice names
    /// everything it needs, so it settles the moment it arrives.
    pending_answer: Option<PendingAnswer>,
    /// The talker called `hang_up`, and the goodbye is still being spoken.
    ///
    /// The call ends at that turn's end rather than on the tool call. So the
    /// caller hears the whole of it and the thread keeps the row. Cleared by a
    /// barge-in: a caller talking over the goodbye was not done.
    hanging_up: bool,
    /// The talker already asked once in the turn it is speaking now. Cleared
    /// when that turn ends, so the next one may ask again.
    delegated_this_turn: bool,
}

/// An answer waiting on the caller's words, and the tool call that owes an
/// acknowledgement for it.
struct PendingAnswer {
    tool_call_id: String,
    choice_id: String,
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
                // Held rather than recorded. Which row it becomes depends on
                // whether the talker asks for the doer, and it may already
                // have. A second utterance arriving first flushes the one
                // before it, so nothing is overwritten.
                self.write_down_whatever_is_left().await;
                // A transcript with no words is not held at all. Held, it
                // would pair with a waiting ask and spend it on nothing: a
                // `WorkDelegated` with no turn behind it, and the caller's
                // real words then arriving with no ask left to claim them.
                if !transcript.trim().is_empty() {
                    self.pending_utterance = Some(transcript.clone());
                }
                // The answer first: it is the one thing that can SPEND the
                // transcript. A delegation pairing with words already sent as
                // an answer would run that answer as a turn of its own.
                //
                // Outside the guard above, so a wordless turn settles a held
                // answer too. It settles it by giving up, which is the point:
                // an answer held past the utterance it was made for would
                // eventually settle a card with a sentence about something
                // else.
                self.settle_the_pending_answer(session).await;
                self.settle_the_pending_utterance(session).await;
                ServerFrame::UserTurnEnded { transcript }
            }
            VoiceEvent::TalkerTranscript { text } => {
                self.talker_has_the_floor = true;
                ServerFrame::TalkerTranscript { text }
            }
            VoiceEvent::DelegationRequested {
                tool_call_id,
                reason,
            } => {
                self.delegated(session, &tool_call_id, reason).await;
                // No frame. The caller hears one answer, and which model
                // produced it is nothing they can act on.
                return None;
            }
            VoiceEvent::AnswerRequested {
                tool_call_id,
                choice_id,
            } => {
                self.answer(session, tool_call_id, choice_id, false).await;
                return None;
            }
            VoiceEvent::HangupRequested { tool_call_id } => {
                self.acknowledge(session, &tool_call_id, HANGUP_TAKEN).await;
                // Held, NOT acted on. A tool call lands while the talker is
                // still speaking, which is what makes delegation free and what
                // would cut the goodbye off mid-word here. The turn's end is
                // where the line closes.
                self.hanging_up = true;
                return None;
            }
            VoiceEvent::TalkerTurnEnded { transcript, usage } => {
                // Audio has no chars, so the estimate is zero and `usage`
                // carries the only real number. A rollup reading measured spend
                // reads that.
                self.capture
                    .record_usage(self.provider.model(), 0, Some(usage))
                    .await;
                // The talker answered and asked for nothing, so the utterance
                // it answered is its alone. `pending_delegation` is left as it
                // is: a transcript still in flight belongs to it.
                //
                // The utterance goes down BEFORE the reply, because the caller
                // spoke first. Both rows leave this one handler, so the order
                // here is the order the transcript reads. The other way round
                // put every answer above its question.
                self.write_down_whatever_is_left().await;
                self.record_spoken(transcript).await;
                // The next turn may ask again, and must be able to.
                self.delegated_this_turn = false;
                self.talker_has_the_floor = false;
                // The goodbye is said and written down, so the line can close.
                // The caller heard all of it, which is the whole reason the
                // hangup waited for this.
                if self.hanging_up {
                    log!("[Voice] The caller said they were done, so the talker rang off");
                    let _ = transport.send_frame(ServerFrame::TalkerTurnEnded).await;
                    return Some(VoiceSessionEndReason::AgentHangup);
                }
                self.say_what_is_waiting(session).await;
                ServerFrame::TalkerTurnEnded
            }
            VoiceEvent::Interrupted => {
                self.interrupted = true;
                // The caller talked over the goodbye, so they were not done.
                // Their intent is the only thing that ends a call (ADR 0170),
                // and taking the floor back is them saying otherwise.
                if self.hanging_up {
                    log!("[Voice] The caller cut in over the goodbye, so the call stays up");
                    self.hanging_up = false;
                }
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
    ///
    /// Anything WAITING on the caller is spoken for the same reason, and it is
    /// the stronger case: the turn behind it is parked on a person, so no
    /// answer follows it at all. Four surfaces qualify, and each gets its own
    /// arm: a question card, and a permission card in each of its three lanes.
    /// Their resolutions share one arm, because settled is settled.
    ///
    /// Spoken, not read: see [`answer_to_say`] and [`decision_to_ask`].
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
            // Silent, and the only tool that is. The `UserQuestionAsked` a
            // beat behind it carries the question itself, which is the whole
            // of what the caller needs. Beside that, `[WORKING] Using
            // ask_user_question.` is only a tool name the talker is told
            // never to say. Compared against the tool layer's own constant,
            // so a rename cannot leave the suppression pointing at nothing.
            ThreadEvent::ToolCalled { name, .. } if name == tool_names::ASK_USER_QUESTION => {}
            ThreadEvent::ToolCalled { name, .. } => {
                let note = format!("[WORKING] Using {}.", name);
                self.append(session, &note).await;
            }
            ThreadEvent::UserQuestionAsked {
                tool_use_id,
                question,
                options,
                multi_select,
                ..
            } => {
                let open = OpenDecision::question(tool_use_id, question, options, *multi_select);
                self.ask(session, open).await;
            }
            ThreadEvent::CommandPermissionRequested {
                request_id,
                tool_name,
                command,
                summary,
                ..
            } => {
                let open =
                    OpenDecision::command_permission(request_id, tool_name, command, summary);
                self.ask(session, open).await;
            }
            ThreadEvent::McpPermissionRequested {
                request_id,
                server_id,
                server_name,
                tool_name,
                arguments_summary,
                ..
            } => {
                let open = OpenDecision::mcp_permission(
                    request_id,
                    server_id,
                    server_name,
                    tool_name,
                    arguments_summary,
                );
                self.ask(session, open).await;
            }
            ThreadEvent::CodingAgentPermissionRequest {
                request_id,
                tool_name,
                input,
                summary,
                ..
            } => {
                let open =
                    OpenDecision::coding_agent_permission(request_id, tool_name, input, summary);
                self.ask(session, open).await;
            }
            // Settled, whichever way and by whichever surface. Appended rather
            // than spoken: the caller either said it or pressed it, so saying
            // it back is news to nobody.
            ThreadEvent::UserQuestionAnswered { .. }
            | ThreadEvent::CommandPermissionResolved { .. }
            | ThreadEvent::McpPermissionResolved { .. }
            | ThreadEvent::CodingAgentPermissionResolved { .. } => {
                self.append(session, DECISION_SETTLED).await;
            }
            ThreadEvent::ResponseGenerated { text, .. } if !text.trim().is_empty() => {
                let answer = answer_to_say(text.trim());
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

    /// The talker asked for the doer.
    ///
    /// **Refused while this thread's doer is parked**, and at no other time.
    /// The doer is blocked inside the very card that is waiting, so there is no
    /// turn to start. The utterance is not paired and the doer is not woken, so
    /// what the caller said is still written down on the next flush.
    ///
    /// Read-then-act, like `doer_for` on the typed path: a card landing between
    /// the read and the pairing is not caught. Closing that would mean holding
    /// a lock across the wake, and the loser is one superseded card rather than
    /// a wrong action.
    ///
    /// **One ask per talker turn, and the rest are acknowledged only.** A model
    /// that calls a tool twice in one response is asking about one utterance,
    /// and both extra shapes are bugs.
    ///
    /// A duplicate arriving BEFORE the transcript would overwrite the reason.
    /// One arriving after the first has already paired would outlive the turn
    /// as a stale ask. It would then wake the doer on the NEXT utterance,
    /// which the talker may be handling alone.
    async fn delegated(
        &mut self,
        session: &mut dyn VoiceSession,
        tool_call_id: &str,
        reason: String,
    ) {
        if self.decisions.doer_is_parked(self.subject.thread_id).await {
            log!(
                "[Voice] Not delegating {:?}: this thread's doer is parked on a card",
                reason
            );
            self.acknowledge(session, tool_call_id, DELEGATION_PARKED)
                .await;
            return;
        }
        // Owed whatever happens next, duplicate or not.
        self.acknowledge(session, tool_call_id, DELEGATION_TAKEN)
            .await;
        if self.delegated_this_turn {
            log!(
                "[Voice] The talker asked twice in one turn, ignoring: {}",
                reason
            );
            return;
        }
        log!("[Voice] The talker asked for the doer: {}", reason);
        self.delegated_this_turn = true;
        self.pending_delegation = Some(reason);
        self.settle_the_pending_utterance(session).await;
    }

    /// The talker answered something waiting on the caller.
    ///
    /// The choice id came from the engine, and the engine looks it up again
    /// before acting. One it never issued, and one whose card has since
    /// settled, are both refused with a note saying so, never guessed at.
    ///
    /// `retry` marks the second and last attempt at a held answer. A first
    /// attempt with no words waits for them; a retry with none gives up.
    async fn answer(
        &mut self,
        session: &mut dyn VoiceSession,
        tool_call_id: String,
        choice_id: String,
        retry: bool,
    ) {
        // The caller's own words as held, for the one choice that sends them.
        // A paraphrase would be a different answer (ADR 0149).
        let spoken = self.pending_utterance.clone().unwrap_or_default();
        let outcome = self
            .decisions
            .resolve(
                self.subject.thread_id,
                &choice_id,
                &spoken,
                self.subject.actor.clone(),
            )
            .await;
        match outcome {
            Resolution::Settled => self.acknowledge(session, &tool_call_id, ANSWER_TAKEN).await,
            Resolution::SettledWithTheirWords => {
                // Spent. Those words ARE the answer's row, exactly as a typed
                // answer is, so holding them on would write the same sentence
                // down a second time.
                self.pending_utterance = None;
                // An ask still waiting for those same words is now waiting for
                // nothing. Left sticky it pairs with a LATER utterance. That
                // wakes the doer on words asking for something else, under a
                // reason taken from the sentence just spent.
                //
                // Its "Taken." went out when it was made, and a tool call is
                // answered once. So the correction is appended instead, which
                // is what lets the talker tell the caller.
                if let Some(stale) = self.pending_delegation.take() {
                    log!("[Voice] The answer spent the words an ask was waiting for");
                    let note = format!("{}\n\n{}", ASK_LOST_ITS_WORDS, stale);
                    self.append(session, &note).await;
                }
                self.acknowledge(session, &tool_call_id, ANSWER_TAKEN).await;
            }
            Resolution::NeedsTheirWords if !retry => {
                // Nothing is acknowledged yet: the call is held, and settles on
                // the next thing the caller says, whatever it is.
                let held = PendingAnswer {
                    tool_call_id,
                    choice_id,
                };
                if let Some(displaced) = self.pending_answer.replace(held) {
                    self.acknowledge(session, &displaced.tool_call_id, NEVER_HEARD)
                        .await;
                }
            }
            // A retry that STILL has no words. Held again it would sit until
            // some later, unrelated sentence settled the card with it, and a
            // card cannot be unsettled. Dropped and said so instead.
            Resolution::NeedsTheirWords => {
                self.acknowledge(session, &tool_call_id, NEVER_HEARD).await;
            }
            Resolution::Refused(why) => self.acknowledge(session, &tool_call_id, &why).await,
        }
    }

    /// Settle a held answer against the next thing the caller said.
    ///
    /// **Once, whatever that turn carried.** The held call is bounded to the
    /// utterance following it, which is the one the talker was answering for.
    /// Anything later is a different sentence, and settling a card with it is
    /// not something the caller can undo.
    async fn settle_the_pending_answer(&mut self, session: &mut dyn VoiceSession) {
        let Some(held) = self.pending_answer.take() else {
            return;
        };
        self.answer(session, held.tool_call_id, held.choice_id, true)
            .await;
    }

    /// Tell the talker one of its tool calls landed.
    ///
    /// Owed for every tool and every outcome. An unresolved call leaves a
    /// dangling item in the talker's history, and it reads that as work it
    /// never heard back about.
    async fn acknowledge(&self, session: &mut dyn VoiceSession, tool_call_id: &str, note: &str) {
        if let Err(e) = session.resolve_tool_call(tool_call_id, note).await {
            log!(
                "[Voice] The talker would not take the acknowledgement: {}",
                e
            );
        }
    }

    /// Put an open decision to the caller, out loud.
    async fn ask(&mut self, session: &mut dyn VoiceSession, decision: OpenDecision) {
        self.say(session, decision_to_ask(&decision)).await;
    }

    /// Pair a held utterance with a held ask, and send both on.
    ///
    /// Called from BOTH sides, so the order the two frames arrive in decides
    /// nothing. The transcript comes from a different model than the tool
    /// call, and on a short fast reply the call really does land first.
    ///
    /// Does nothing until it has both. An utterance with no ask is written
    /// down elsewhere, when the talker's turn ends or the call does.
    ///
    /// **A doer that refuses is handled here, not ignored.** It writes nothing
    /// when it refuses, so the caller's words would otherwise vanish and leave
    /// a `WorkDelegated` beside no record of what was said.
    async fn settle_the_pending_utterance(&mut self, session: &mut dyn VoiceSession) {
        if self.pending_utterance.is_none() || self.pending_delegation.is_none() {
            return;
        }
        let transcript = self.pending_utterance.take().unwrap_or_default();
        let reason = self.pending_delegation.take().unwrap_or_default();

        // The wake first, so the doer's history already carries the reason by
        // the time the turn reading that history starts.
        emit(
            &self.bus,
            self.subject.thread_id,
            ThreadEvent::WorkDelegated {
                session_id: self.subject.session_id,
                reason,
            },
            EventMeta::NONE.authored_by(AgentParticipant::Guest {
                label: TALKER_LABEL.to_string(),
            }),
        )
        .await;
        let taken = self
            .doer
            .wake(
                self.subject.thread_id,
                self.subject.session_id,
                &transcript,
                self.subject.actor.clone(),
            )
            .await;
        if !taken {
            // Put it back so the one writer of a spoken row stays the one
            // writer, rather than a second emit growing beside it.
            self.pending_utterance = Some(transcript);
            self.write_down_whatever_is_left().await;
            self.say(session, NOT_TAKEN.to_string()).await;
        }
    }

    /// Write down a held utterance that started no turn.
    ///
    /// Two ways to get here: the talker answered it alone, or the doer refused
    /// it. Either way no turn is running, and the words are the caller's.
    ///
    /// `SpokenMessageReceived`, which is `Metadata` and starts nothing. A
    /// `MessageReceived` here would leave the thread claiming a turn that will
    /// never run, because that variant is `EventClass::Start`.
    ///
    /// The caller's own actor, not the talker's. Whoever handled it, the
    /// caller said it.
    async fn write_down_whatever_is_left(&mut self) {
        // Never empty: `UserTurnEnded` refuses to hold a wordless transcript.
        let Some(transcript) = self.pending_utterance.take() else {
            return;
        };
        emit(
            &self.bus,
            self.subject.thread_id,
            ThreadEvent::SpokenMessageReceived {
                session_id: self.subject.session_id,
                text: transcript,
            },
            EventMeta::with_actor(self.subject.actor.clone()),
        )
        .await;
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
        // The turn about to start is a relay of what it was handed, so no
        // running round needs to be told about it.
        self.relaying = true;
        if let Err(e) = session.speak(&note).await {
            log!("[Voice] The talker would not take the answer: {}", e);
            self.talker_has_the_floor = false;
            self.relaying = false;
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
    /// and the doer never reads it as its own prior turn (ADR 0150).
    ///
    /// A reply with no words is not written. A cancelled response can end
    /// before the talker said anything, and an empty row would claim the caller
    /// heard something they did not.
    async fn record_spoken(&mut self, transcript: String) {
        let interrupted = std::mem::take(&mut self.interrupted);
        let relaying = std::mem::take(&mut self.relaying);
        if transcript.trim().is_empty() {
            return;
        }
        // Offer the talker's OWN words to a turn already running. It then
        // learns what the caller was told in its name, rather than reading it
        // next round. A relay is skipped: the round wrote that answer itself.
        if !relaying {
            self.doer
                .overheard(self.subject.thread_id, &transcript)
                .await;
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
    let pool = engine.pool();
    let language = language::for_workspace(pool).await;
    SessionOpening {
        instructions: super::instructions_for(language.as_ref()),
        resident_block: resident::build_block(engine, thread_id).await,
        // Both resolve their catalog default, so neither is guarded here. The
        // voice was a const while nothing could hear it: a setting nobody can
        // evaluate is a setting nobody can choose. A client ships now.
        voice: build::talker_voice(pool).await,
        transcriber: build::transcriber_model(pool).await,
        audio: Default::default(),
        language,
    }
}

#[cfg(test)]
#[path = "call_tests.rs"]
mod tests;
