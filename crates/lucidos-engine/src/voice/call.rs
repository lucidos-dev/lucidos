use std::collections::{HashSet, VecDeque};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::broadcast::Receiver;
use uuid::Uuid;

use super::decision::{DecisionKind, DecisionResolver, OpenDecision, Resolution};
use super::doer::TurnStarter;
use super::provider::{wait_until, SessionOpening, VoiceEvent, VoiceProvider, VoiceSession};
use super::wire::{ClientControl, ServerFrame};
use super::{build, language, resident, EARLIER_TAG, WORKING_NOTE_TAG};
use crate::core::store::messages::spoken_merge::join_spoken;
use crate::engine::event_bus::{BusEvent, EmittedEvent, EventBus, LagReplay, ReplayScope};
use crate::engine::permission::legacy::LegacyCard;
use crate::engine::thread_events::{
    AgentParticipant, EventMeta, MessageOrigin, ThreadEvent, VoiceSessionEndReason,
};
use crate::engine::{AuxCapture, ContextPurpose, LucidosEngine};
use crate::llm::tool_names;

type BoxError = Box<dyn std::error::Error + Send + Sync>;

const TALKER_LABEL: &str = "Lucidos (aloud)";

const UNFINISHED: &str =
    "[ANSWER] That did not finish. Tell the caller so, and offer to try again.";

const NOT_TAKEN: &str = "[ANSWER] That could not be started on this conversation. \
                         Tell the caller so, and say they can type it instead.";

const DECISION_SETTLED: &str = "\
[SETTLED] That is settled now, so do not put it to them again.";

const DELEGATION_TAKEN: &str = "Taken. The answer will arrive separately.";

const DELEGATION_PARKED: &str = "\
Not started. Lucidos is waiting on the caller's answer to what is already open, \
so nothing new can run until they settle it. Put that back to them, and answer \
it with what they say.";

const DELEGATION_PARKED_ON_SCREEN: &str = "\
Not started. Lucidos needs the caller's say-so on what is already open, and \
that one is settled on their screen rather than out loud. Tell them what is \
waiting and that the card is there, then wait for them.";

const ANSWER_TAKEN: &str = "Answered. That is settled now.";

const HANGUP_TAKEN: &str = "Ending the call now.";

const NEVER_HEARD: &str = "\
That one was dropped: the caller's own words never came through for it. Ask \
them again if it still matters.";

const ASK_LOST_ITS_WORDS: &str = "\
[NOT STARTED] The caller's words settled what was waiting, so the request you \
made never started. Nothing is running for it. Offer it to them again if they \
still want it.";

/// INVARIANT: What the talker is told when an ask waited for words that never came.
///
/// The other end of [`ASK_LOST_ITS_WORDS`]: there the words were spent, here
/// they never arrived at all. A delegation frame carries no text, so an ask
/// with no transcript behind it has nothing to run on.
///
/// Spoken rather than appended, unlike its sibling. Nothing else reaches the
/// caller here: they were told it was on it, and they are sitting in silence
/// waiting for work that never started.
const ASK_NEVER_GOT_ITS_WORDS: &str = "\
[ANSWER] None of what they just said came through, so nothing was started for \
it. Tell them you did not catch it, and ask them to say it again.";

/// INVARIANT: How long a caller may wait with nobody answering them.
///
/// **This is not the forbidden silence timer, and the subject is the
/// difference.** That rule is about deciding when a PERSON stopped talking, and
/// nothing here decides that: the words this hands over are the words the
/// provider already transcribed. What it measures is OUR failure to respond.
///
/// A caller asking a question and hearing nothing until they hang up is the one
/// outcome a call may not have. The talker is the fast path to an answer, and
/// past this bound it has demonstrably not taken it, so the doer does.
///
/// Re-armed on every word the caller says, so a long sentence is never cut in
/// half by it. It runs from the last thing they said.
const CALLER_WAITED_LONG_ENOUGH: Duration = Duration::from_secs(6);

const TURNS_ONE_OPENER_BUYS: u8 = 8;

const NOBODY_ANSWERED: &str =
    "the talker said nothing, so the caller's question came straight here";

const VOICE_IS_NOT_ANSWERING: &str =
    "The voice is not answering. Your question went to Lucidos anyway, \
                                      and the answer will appear in this conversation.";

const OFFER_THE_DETAIL_ABOVE_CHARS: usize = 400;

/// INVARIANT: Hand the doer's answer to the talker, as something to say.
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

fn answer_to_keep(text: &str) -> String {
    format!(
        "{} This came back for something asked before this call. It is on \
         screen. Do not bring it up. Pass it on only if they ask what came back.\n\n{}",
        EARLIER_TAG, text
    )
}

const UNFINISHED_EARLIER: &str = "[EARLIER] Something asked before this call did \
not finish. Do not bring it up. Say so only if they ask about it.";

fn decision_to_ask(decision: &OpenDecision, holds_the_answer_tool: bool) -> String {
    let opening = match decision.kind {
        DecisionKind::Question => "[QUESTION] The work is waiting on the caller's answer.",
        DecisionKind::CommandPermission
        | DecisionKind::McpPermission
        | DecisionKind::CodingAgentPermission => {
            "[PERMISSION] Lucidos needs the caller's say-so before it can carry on."
        }
        DecisionKind::PermissionRequest { .. } => {
            "[PERMISSION] Lucidos asks the caller for a permission."
        }
    };
    const HAND_ITS_ID_BACK: &str = "They answer by saying which one they want, \
                                    and you hand its id back. Never say an id out \
                                    loud.";
    let how = match (holds_the_answer_tool, decision.kind) {
        (true, DecisionKind::Question) => HAND_ITS_ID_BACK.to_string(),
        (true, DecisionKind::CommandPermission)
        | (true, DecisionKind::McpPermission)
        | (true, DecisionKind::CodingAgentPermission) => {
            format!(
                "{} {}",
                HAND_ITS_ID_BACK,
                super::PERMISSION_NEEDS_THEIR_WORDS
            )
        }
        // INVARIANT: Either talker: the caller's own transcript or a tap settles it, never
        // a pick.
        (_, DecisionKind::PermissionRequest { words_answer, .. }) => format!(
            "{} Never say an id out loud.",
            super::permission_request_read_back(words_answer)
        ),
        (false, DecisionKind::Question) => {
            "They answer by saying which one they want, in their own words. Hand \
             what they said over, which is what settles this. Never say an id out \
             loud."
                .to_string()
        }
        (false, DecisionKind::CommandPermission)
        | (false, DecisionKind::McpPermission)
        | (false, DecisionKind::CodingAgentPermission) => {
            "This one they settle on their screen rather than out loud. Tell them \
             the card is there. Never say an id out loud."
                .to_string()
        }
    };
    // INVARIANT: The prompt itself is NEVER cut, unlike everything else the talker reads.
    // A truncated question is a different question, and the talker is about to
    // state it as the one being asked.
    format!(
        "{} Put this to them out loud, in your own words, and read them the \
         choices. {}\n\n{}\n{}",
        opening,
        how,
        super::fenced_card_text(&decision.prompt),
        super::choices_for(&decision.choices),
    )
}

#[derive(Debug, Clone, PartialEq)]
pub enum CallerFrame {
    Audio(Vec<u8>),
    Control(ClientControl),
    Undecodable,
    Closed,
}

#[async_trait]
pub trait CallTransport: Send {
    async fn recv(&mut self) -> CallerFrame;
    async fn send_audio(&mut self, pcm: Vec<u8>) -> Result<(), BoxError>;
    async fn send_frame(&mut self, frame: ServerFrame) -> Result<(), BoxError>;
}

#[derive(Debug, Clone)]
pub struct CallSubject {
    pub thread_id: Uuid,
    pub session_id: Uuid,
    pub actor: Option<MessageOrigin>,
}

/// INVARIANT: Hold one call open until it ends, and record what it did.
///
/// Returns the reason it ended, or `None` when the talker never answered at
/// all. That case writes NO events: a start with no call behind it would make
/// the pair count sessions that never happened.
///
/// Four of the arguments are the SEAMS this file is built on, and each is a
/// separate thing a call can be given: the talker, the caller, the doer, and
/// what is waiting. Bundling them would hide which ones a test is standing in
/// for, which is the whole point of having them.
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

    // INVARIANT: Subscribed before the talker can produce a word. Nothing the doer
    // emits can then land in the gap between opening a session and watching
    // for one.
    let (thread, replay) = LagReplay::subscribe(bus, ReplayScope::Thread(subject.thread_id), None);

    let mut session = match provider.open(opening).await {
        Ok(session) => session,
        Err(e) => {
            log_legacy!(
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
        log_legacy!("[Voice] The caller was gone before the call opened");
    }

    let mut call = Call {
        bus: bus.clone(),
        provider,
        doer,
        decisions,
        capture: AuxCapture::new(bus, subject.thread_id, ContextPurpose::Voice),
        subject: subject.clone(),
        thread,
        replay,
        talker_has_the_floor: false,
        interrupted: false,
        reply_was_cut_off: CutReply::Running,
        relaying: false,
        waiting_to_be_said: VecDeque::new(),
        spoken_so_far: SpokenTurn::default(),
        last_turn_transcript: String::new(),
        talker_answered_them: false,
        undelivered_words: None,
        continuing: None,
        noted_this_turn: false,
        seen_on_this_call: HashSet::new(),
        pending_delegation: None,
        ask_owed_by: None,
        pending_answer: None,
        hanging_up: false,
        delegated_this_turn: false,
        heard_so_far: String::new(),
        answer_owed_by: None,
        told_the_caller: false,
        floor: Floor::Shut,
        audience: Audience::Undecided,
        mute_held_for: 0,
        the_caller_has_spoken: false,
        held: VecDeque::new(),
    };
    let reason = call.drive(&mut *session, transport).await;
    call.forget_the_hold().await;
    // INVARIANT: Closed FIRST, and that ordering is the whole of it. A provider with no
    // turn-end frame holds the caller's last sentence until the socket goes.
    // The flush below used to run while those words were still upstream.
    // `VoiceSession::close` hands them back.
    call.take_what_was_still_held(session.close().await).await;
    // INVARIANT: Both flushes run for EVERY end reason. A hangup, a dropped socket and a
    // provider failure all leave the same two things unwritten: the caller's
    // unclosed partial, and a talker turn the provider never ended.
    //
    // **The reply first, and only here is the order a choice.** Everywhere
    // else a row goes down at its own turn end, so the clock decides. These
    // two go down at one moment, and a talker still holding the floor began
    // its reply before the partial that landed over it.
    //
    // A reply whose turn already ended was written then, and this writes
    // nothing.
    call.write_down_whatever_was_said().await;
    call.close_the_unfinished_partial().await;

    // INVARIANT: Dropped on failure, deliberately. The call is already over, and the
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

enum Step {
    CallerAudio(Vec<u8>),
    BargeIn,
    CallerStartedSpeaking,
    Undecodable,
    Talker(VoiceEvent),
    TalkerGone,
    Thread(Box<Result<EmittedEvent, RecvError>>),
    NobodyAnsweredTheCaller,
    AskNeverGotItsWords,
    Ended(VoiceSessionEndReason),
}

#[derive(Default)]
struct SpokenTurn {
    words: String,
    began: Option<Instant>,
}

impl SpokenTurn {
    fn push(&mut self, delta: &str) {
        if !delta.trim().is_empty() {
            self.began.get_or_insert_with(Instant::now);
        }
        self.words.push_str(delta);
    }

    fn take(&mut self) -> (String, Option<f64>) {
        let age = self.began.take().map(|at| at.elapsed().as_secs_f64());
        (std::mem::take(&mut self.words), age)
    }
}

struct Call<'a> {
    bus: EventBus,
    provider: &'a dyn VoiceProvider,
    doer: &'a dyn TurnStarter,
    decisions: &'a dyn DecisionResolver,
    capture: AuxCapture,
    subject: CallSubject,
    thread: Receiver<EmittedEvent>,
    replay: LagReplay,
    /// INVARIANT: True while the talker owes the caller the rest of a sentence.
    ///
    /// Claimed by its WORDS, and by [`Call::say`] asking it for some. Never by
    /// its audio: a provider that streams audio between turns would take the
    /// floor once and hold it until the line closed. Released by the turn end
    /// alone, so a reply the caller is still hearing is never spoken over.
    talker_has_the_floor: bool,
    interrupted: bool,
    reply_was_cut_off: CutReply,
    relaying: bool,
    waiting_to_be_said: VecDeque<String>,
    spoken_so_far: SpokenTurn,
    /// INVARIANT: What the provider called the talker's last turn, for the one reply that
    /// streamed no deltas at all.
    ///
    /// The deltas are what the caller HEARD, so they are the row. This is the
    /// fallback for a provider that reports a finished turn without having
    /// streamed it, which would otherwise lose the reply outright.
    last_turn_transcript: String,
    talker_answered_them: bool,
    undelivered_words: Option<String>,
    /// INVARIANT: The last ask the doer took.
    ///
    /// The rest of that ask's sentence is offered to its turn, and never put
    /// on [`Call::undelivered_words`], where a later ask would run it again.
    /// `None` when the last ask started nothing, and once its turn ended.
    continuing: Option<AskedTurn>,
    noted_this_turn: bool,
    seen_on_this_call: HashSet<Uuid>,
    /// INVARIANT: The talker's reason from a `delegate` call with no utterance to pair
    /// with yet.
    ///
    /// **Empty on a protocol whose ask composes no words.** The `Some` is what
    /// says an ask is held, so the string may be blank and the option may not.
    ///
    /// **Sticky on purpose, and it outlives the turn that made it.** The
    /// transcript and the tool call come from two models on one socket, so a
    /// short fast reply produces the call first. Cleared at the turn's end,
    /// this would drop the caller's real question into a row that starts
    /// nothing. That is the failure the whole tool exists to end.
    ///
    /// Sticky, and still not forever: see [`Call::ask_owed_by`]. Write it
    /// through [`Call::hold_the_ask`] and read it back through
    /// [`Call::take_the_ask`], so the two can never disagree.
    pending_delegation: Option<String>,
    /// INVARIANT: When a held ask gives up on the words it was made for.
    ///
    /// A tool call and a transcript come from two models on one socket, so an
    /// ask waiting for the transcript is ordinary. Waiting for one that is not
    /// coming is not: the caller was told it was on it, and nothing is running.
    /// A Live delegation frame carries no text at all, so this is the only
    /// thing standing between that caller and silence.
    ///
    /// [`CALLER_WAITED_LONG_ENOUGH`] again, because it answers the same
    /// question about the same person.
    ask_owed_by: Option<tokio::time::Instant>,
    pending_answer: Option<PendingAnswer>,
    /// INVARIANT: The talker called `hang_up`, and the goodbye is still being spoken.
    ///
    /// The call ends at that turn's end rather than on the tool call. So the
    /// caller hears the whole of it and the thread keeps the row. Cleared by a
    /// barge-in: a caller talking over the goodbye was not done.
    hanging_up: bool,
    delegated_this_turn: bool,
    /// INVARIANT: What the provider has heard of the caller SO FAR, before it says the
    /// turn finished.
    ///
    /// The caller's half of [`Self::spoken_so_far`], held for the same reason.
    /// A protocol with no turn-end frame reports the caller through these
    /// partials alone. A call that stops first has nothing else to write a row
    /// from. Superseded by the finished transcript, which covers the same
    /// stretch.
    heard_so_far: String,
    answer_owed_by: Option<tokio::time::Instant>,
    told_the_caller: bool,
    floor: Floor,
    audience: Audience,
    mute_held_for: u8,
    the_caller_has_spoken: bool,
    held: VecDeque<VoiceEvent>,
}

const HELD_AUDIO_BYTES: usize = 1024 * 1024;

fn held_audio_bytes(event: &VoiceEvent) -> usize {
    match event {
        VoiceEvent::Audio(pcm) => pcm.len(),
        _ => 0,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Opener {
    CallerWords,
    ProviderHeardThem,
    DeviceHeardThem,
    Engine,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Floor {
    Shut,
    Held,
    Open { turns_left: u8 },
}

impl Floor {
    fn is_open(self) -> bool {
        matches!(self, Floor::Open { .. })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Audience {
    Undecided,
    Nobody,
    TheCaller,
}

fn the_sentence_is_unfinished(transcript: &str) -> bool {
    let words = transcript.trim_end_matches(|c: char| {
        c.is_whitespace() || matches!(c, '"' | '\'' | ')' | ']' | '}' | '»' | '”' | '’')
    });
    match words.chars().next_back() {
        None => false,
        Some(last) => !matches!(last, '.' | '!' | '?' | '…'),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CutReply {
    Running,
    Cut,
    /// INVARIANT: The same, with that row already written.
    ///
    /// **What the provider reports at the turn's end is then read for
    /// nothing.** It covers the WHOLE reply, tail included, and the caller
    /// stopped listening part-way through. Taken as the next stretch's
    /// fallback, it writes the reply a second time, in full, saying nobody
    /// cut it off.
    CutAndWritten,
}

impl CutReply {
    fn is_cut(self) -> bool {
        self != CutReply::Running
    }

    fn and_cut(self) -> Self {
        match self {
            CutReply::CutAndWritten => CutReply::CutAndWritten,
            _ => CutReply::Cut,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct AskedTurn {
    delegation: Uuid,
    turn: Uuid,
}

impl AskedTurn {
    fn new(delegation: Uuid) -> Self {
        AskedTurn {
            delegation,
            turn: delegation,
        }
    }

    fn follow(&mut self, event: &ThreadEvent, request_event_id: Option<Uuid>) {
        if let (
            ThreadEvent::PromptInjected {
                injected_message_id: Some(injected),
                ..
            },
            Some(turn),
        ) = (event, request_event_id)
        {
            if *injected == self.delegation {
                self.turn = turn;
            }
        }
    }

    fn ended_by(&self, request_event_id: Option<Uuid>) -> bool {
        request_event_id == Some(self.turn)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CallerWords {
    OwnSentence,
    RestOfAsked(Option<Uuid>),
}

struct PendingAnswer {
    tool_call_id: String,
    choice_id: String,
}

impl Call<'_> {
    async fn drive(
        &mut self,
        session: &mut dyn VoiceSession,
        transport: &mut dyn CallTransport,
    ) -> VoiceSessionEndReason {
        loop {
            let owed = self.answer_owed_by;
            let asked = self.ask_owed_by;
            let step = tokio::select! {
                from_caller = transport.recv() => from_caller.into(),
                from_talker = session.next() => match from_talker {
                    Some(event) => Step::Talker(event),
                    None => Step::TalkerGone,
                },
                from_thread = self.thread.recv() => Step::Thread(Box::new(from_thread)),
                _ = wait_until(owed) => Step::NobodyAnsweredTheCaller,
                _ = wait_until(asked) => Step::AskNeverGotItsWords,
            };

            match step {
                Step::CallerAudio(pcm) => {
                    if let Err(e) = session.push_audio(&pcm).await {
                        log_legacy!("[Voice] The talker stopped taking audio: {}", e);
                        return provider_failed(transport).await;
                    }
                }
                Step::BargeIn => self.the_caller_cut_in(session).await,
                Step::CallerStartedSpeaking => self.the_device_heard_them(),
                Step::Undecodable => {
                    let _ = transport
                        .send_frame(ServerFrame::Error {
                            message: "That control was not one this call understands.".to_string(),
                        })
                        .await;
                }
                Step::Talker(event) => {
                    let a_turn_ended = matches!(event, VoiceEvent::TalkerTurnEnded { .. });
                    let mut next = self.hold(event);
                    while let Some(event) = next.take().or_else(|| self.released()) {
                        match self.forward(event, session, transport).await {
                            None => {}
                            Some(VoiceSessionEndReason::ProviderFailed) => {
                                return provider_failed(transport).await
                            }
                            Some(reason) => return reason,
                        }
                    }
                    // INVARIANT: A turn end the hold kept gave the floor back without a
                    // forwarded turn end, so what queued behind it goes now.
                    if a_turn_ended && self.floor == Floor::Held {
                        self.say_what_is_waiting(session).await;
                    }
                }
                Step::TalkerGone => {
                    log_legacy!(
                        "[Voice] {} ended the session on its side",
                        self.provider.name()
                    );
                    return provider_failed(transport).await;
                }
                Step::Thread(received) => {
                    let Some(frames) = self.replay.read(*received, self.bus.pool()).await else {
                        return VoiceSessionEndReason::EngineShutdown;
                    };
                    for emitted in frames {
                        self.on_thread_event(emitted, session).await;
                    }
                }
                Step::NobodyAnsweredTheCaller => {
                    self.nobody_answered_the_caller(session, transport).await
                }
                Step::AskNeverGotItsWords => self.the_ask_never_got_its_words(session).await,
                Step::Ended(reason) => return reason,
            }
        }
    }

    /// INVARIANT: Send one talker event on to the caller, and write down what it means.
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
            // INVARIANT: Forwarded, and it claims NOTHING. A Live talker streams audio
            // whether or not it is speaking. A floor taken here is one held for
            // the whole call, with every doer answer queued behind it unheard.
            // The floor reads words, on the arm below.
            //
            // The narrow cost is on Realtime, which interleaves the two. A
            // response's first audio frame now leads its first worded delta,
            // by however long the provider takes to send one.
            //
            // Dropped outright once the caller has cut this reply off. See
            // [`Call::reply_was_cut_off`].
            VoiceEvent::Audio(pcm) => {
                if self.reply_was_cut_off.is_cut() || !self.the_caller_hears_this() {
                    return None;
                }
                return delivered(transport.send_audio(pcm).await);
            }
            // INVARIANT: Held as well as forwarded, exactly as the talker's deltas are.
            // A partial is not a sentence, so it gets no row of its own. It is
            // still the only account of the caller a turn-less protocol gives
            // before it closes. See [`Call::heard_so_far`].
            //
            // Concatenated raw, never space-joined: a delta carries its own
            // spacing, and one can split a word in two.
            VoiceEvent::UserTranscript { text } => {
                self.heard_so_far.push_str(&text);
                self.caller_is_owed_an_answer();
                // INVARIANT: Words while the line is closing say they were not done, and
                // the client sends no barge-in for somebody who never stopped.
                // So this is the only signal that reaches a goodbye the talker
                // began over the top of them.
                if !text.trim().is_empty() {
                    self.the_caller_was_not_done();
                    self.open_the_floor(Opener::CallerWords);
                }
                ServerFrame::UserTranscript { text }
            }
            VoiceEvent::CallerStartedSpeaking => {
                self.open_the_floor(Opener::ProviderHeardThem);
                return None;
            }
            VoiceEvent::UserTurnEnded { transcript } => {
                self.caller_finished(session, transcript, CallerWords::OwnSentence)
                    .await
            }
            VoiceEvent::UserTurnContinued { transcript } => {
                let words = CallerWords::RestOfAsked(self.turn_to_continue());
                self.caller_finished(session, transcript, words).await
            }
            VoiceEvent::TalkerTranscript { text } => {
                // INVARIANT: **Both of these run for a turn nobody hears**, which is what
                // keeps the floor honest. The provider really is mid-turn, so
                // an answer handed over now has to queue behind it. Skipped,
                // `Call::say` would spend its relay flag on the turn it is
                // speaking over, and mute the answer that follows.
                //
                // Both read off its WORDS, and a BLANK delta carries none.
                // Audio the caller cannot hear is silence, and so is a stream
                // of empty deltas: both providers forward one exactly as it
                // arrives. Acting on those is how a mute talker would hold the
                // bound and the floor for a whole call.
                if !text.trim().is_empty() {
                    self.the_talker_said_a_word();
                    self.talker_has_the_floor = true;
                }
                if self.reply_was_cut_off.is_cut() || !self.the_caller_hears_this() {
                    return None;
                }
                if !text.trim().is_empty() {
                    self.answer_is_no_longer_owed();
                }
                // INVARIANT: Held as well as forwarded. This is the only account of the
                // reply the call holds before the turn ends, and a call can end
                // first. See [`Call::spoken_so_far`].
                self.talker_said(&text);
                ServerFrame::TalkerTranscript { text }
            }
            VoiceEvent::DelegationRequested {
                tool_call_id,
                reason,
            } => {
                self.delegated(session, &tool_call_id, reason).await;
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
                self.answer_is_no_longer_owed();
                // INVARIANT: Held, NOT acted on. A tool call lands while the talker is
                // still speaking, which is what makes delegation free and what
                // would cut the goodbye off mid-word here. The turn's end is
                // where the line closes.
                self.hanging_up = true;
                return None;
            }
            // INVARIANT: **The talker's turn ended.** On a provider that states its own
            // turn ends this really is one, and on a Live call it is a hole in
            // the words. Either way the words so far are final, so they are
            // written down here and the row is dated by this moment.
            VoiceEvent::TalkerTurnEnded { transcript, usage } => {
                // INVARIANT: Recorded HERE, where the number arrives, and never with the
                // row. A cut reply's row went down at the caller's own turn
                // end, before this report existed. Read at the row alone, what
                // the turn cost went nowhere. Audio has no chars, so the
                // estimate is zero and the turn's own report is the only real
                // number.
                //
                // Recorded for a turn nobody heard as well. We were billed for
                // it whoever it reached.
                self.capture
                    .record_usage(self.provider.model(), 0, Some(usage))
                    .await;
                let heard = self.the_caller_hears_this();
                let holding = !heard
                    && self.floor.is_open()
                    && self.mute_held_for < TURNS_ONE_OPENER_BUYS
                    && the_sentence_is_unfinished(&transcript);
                self.mute_held_for = if holding { self.mute_held_for + 1 } else { 0 };
                if !heard {
                    log_legacy!(
                        "[Voice] The floor was shut, so nobody heard the talker{}: {}",
                        if holding {
                            ", and its sentence is held"
                        } else {
                            ""
                        },
                        super::clip(transcript.trim(), super::READ_ALOUD_CHARS)
                    );
                }
                let cut = std::mem::replace(&mut self.reply_was_cut_off, CutReply::Running);
                let owed_a_row = heard && cut != CutReply::CutAndWritten;
                if owed_a_row && !transcript.trim().is_empty() {
                    self.last_turn_transcript = transcript.clone();
                    self.talker_answered_them = true;
                }
                let relaying = std::mem::take(&mut self.relaying);
                if heard && !relaying && !cut.is_cut() && !transcript.trim().is_empty() {
                    self.doer
                        .overheard(self.subject.thread_id, &transcript)
                        .await;
                }
                self.write_down_the_reply().await;
                if heard && !transcript.trim().is_empty() {
                    self.the_talker_spent_a_turn();
                }
                self.audience = if holding {
                    Audience::Nobody
                } else {
                    Audience::Undecided
                };
                // INVARIANT: The reply is over, so its cut is spent with it. Cleared here
                // rather than with the row, because a cut that wrote no row
                // would otherwise mark the next reply.
                self.interrupted = false;
                // INVARIANT: **The caller's held words are NOT spent here.** A pause is
                // not a move of the conversation, on either side. The talker
                // routinely asks for the doer a beat after one: it says "Okay.",
                // draws breath, and asks. Taking the words here left that ask
                // with nothing to pair with, so the doer never ran and the
                // caller was told it had. See
                // `docs/plans/2026-09-16-a-pause-spends-nothing.md`.
                //
                // `pending_delegation` is left alone for the same reason: a
                // transcript still in flight belongs to it.
                if transcript.trim().is_empty() {
                    log_legacy!("[Voice] A talker turn ended with nothing said");
                } else if heard {
                    // INVARIANT: The caller was answered, so nothing is owed. Said again
                    // here as well as on the first delta, because a transcript
                    // that lands late re-arms the bound after the reply.
                    //
                    // Not for a turn nobody heard. They are still owed, and
                    // the bound is what sends them to the doer instead.
                    self.answer_is_no_longer_owed();
                }
                self.delegated_this_turn = false;
                self.talker_has_the_floor = false;
                if self.hanging_up {
                    log_legacy!("[Voice] The caller said they were done, so the talker rang off");
                    let _ = transport.send_frame(ServerFrame::TalkerTurnEnded).await;
                    return Some(VoiceSessionEndReason::AgentHangup);
                }
                self.say_what_is_waiting(session).await;
                ServerFrame::TalkerTurnEnded
            }
            VoiceEvent::Interrupted => {
                self.interrupted = true;
                // INVARIANT: The provider saw the cut itself, so the rest of this reply is
                // over here too. Realtime reports one, and a Live talker never
                // does: on that protocol the barge-in control is the only word
                // we get.
                //
                // It never walks a cut BACK, because the row it names may
                // already be down. Realtime reports the cut beside the turn
                // end, and the caller's own words can land between the two.
                self.reply_was_cut_off = self.reply_was_cut_off.and_cut();
                self.the_caller_was_not_done();
                ServerFrame::Interrupted
            }
            VoiceEvent::Failed { message } => {
                log_legacy!(
                    "[Voice] {} failed mid-call: {}",
                    self.provider.name(),
                    message
                );
                return Some(VoiceSessionEndReason::ProviderFailed);
            }
        };
        delivered(transport.send_frame(frame).await)
    }

    async fn the_caller_cut_in(&mut self, session: &mut dyn VoiceSession) {
        if !self.talker_has_the_floor || self.reply_was_cut_off.is_cut() {
            return;
        }
        self.reply_was_cut_off = self.reply_was_cut_off.and_cut();
        self.interrupted = true;
        self.the_caller_was_not_done();
        if let Err(e) = session.cancel().await {
            log_legacy!("[Voice] Could not interrupt the talker: {}", e);
        }
    }

    fn the_caller_was_not_done(&mut self) {
        if self.hanging_up {
            log_legacy!("[Voice] The caller cut in over the goodbye, so the call stays up");
            self.hanging_up = false;
        }
    }

    async fn on_thread_event(&mut self, emitted: EmittedEvent, session: &mut dyn VoiceSession) {
        let BusEvent::Thread {
            thread_id,
            event,
            meta,
        } = &emitted.typed
        else {
            return;
        };
        if *thread_id != self.subject.thread_id {
            return;
        }
        self.seen_on_this_call.insert(emitted.event_id);
        if let (
            ThreadEvent::PromptInjected {
                injected_message_id: Some(injected),
                ..
            },
            Some(turn),
        ) = (event, meta.request_event_id)
        {
            if self.seen_on_this_call.contains(injected) {
                self.seen_on_this_call.insert(turn);
            }
        }
        // INVARIANT: An unstamped terminal cannot be placed, so it is spoken as it
        // always was. Kept silent, an answer the caller is owed is lost.
        let begun_on_this_call = meta
            .request_event_id
            .is_none_or(|turn| self.seen_on_this_call.contains(&turn));
        if let Some(asked) = self.continuing.as_mut() {
            asked.follow(event, meta.request_event_id);
        }
        if matches!(
            event,
            ThreadEvent::ResponseGenerated { .. }
                | ThreadEvent::ResponseFailed { .. }
                | ThreadEvent::ResponseAborted { .. }
                | ThreadEvent::ResponseCanceled { .. }
        ) {
            self.noted_this_turn = false;
            if self
                .continuing
                .is_some_and(|asked| asked.ended_by(meta.request_event_id))
            {
                self.continuing = None;
            }
        }
        match event {
            ThreadEvent::ToolCalled { name, .. } if name == tool_names::ASK_USER_QUESTION => {}
            ThreadEvent::ToolCalled { .. } if self.noted_this_turn => {}
            ThreadEvent::ToolCalled { name, .. } => {
                self.noted_this_turn = true;
                let note = format!("{} Using {}.", WORKING_NOTE_TAG, name);
                self.append(session, &note).await;
            }
            ThreadEvent::UserQuestionAsked {
                tool_use_id,
                question,
                options,
                multi_select,
                permission_request,
                ..
            } => {
                let open = match permission_request {
                    Some(request) => OpenDecision::permission_request(
                        tool_use_id,
                        question,
                        options,
                        request.into(),
                    ),
                    None => OpenDecision::question(tool_use_id, question, options, *multi_select),
                };
                self.ask(session, open).await;
            }
            ThreadEvent::CommandPermissionRequested { .. }
            | ThreadEvent::McpPermissionRequested { .. }
            | ThreadEvent::CodingAgentPermissionRequest { .. } => {
                let sequence = emitted.seq.unwrap_or_default();
                if let Some(card) = LegacyCard::from_event(*thread_id, sequence, event) {
                    self.ask(session, OpenDecision::tool_permission(&card))
                        .await;
                }
            }
            ThreadEvent::UserQuestionAnswered { .. }
            | ThreadEvent::CommandPermissionResolved { .. }
            | ThreadEvent::McpPermissionResolved { .. }
            | ThreadEvent::CodingAgentPermissionResolved { .. } => {
                self.append(session, DECISION_SETTLED).await;
            }
            ThreadEvent::ResponseGenerated { text, .. }
                if !text.trim().is_empty() && !begun_on_this_call =>
            {
                self.append(session, &answer_to_keep(text.trim())).await;
            }
            ThreadEvent::ResponseGenerated { text, .. } if !text.trim().is_empty() => {
                let answer = answer_to_say(text.trim());
                self.say(session, answer).await;
            }
            ThreadEvent::ResponseFailed { .. } | ThreadEvent::ResponseAborted { .. }
                if !begun_on_this_call =>
            {
                self.append(session, UNFINISHED_EARLIER).await;
            }
            ThreadEvent::ResponseFailed { .. } | ThreadEvent::ResponseAborted { .. } => {
                self.say(session, UNFINISHED.to_string()).await;
            }
            ThreadEvent::ResponseCanceled { cause, .. }
                if !cause.is_redirect() && !begun_on_this_call =>
            {
                self.append(session, UNFINISHED_EARLIER).await;
            }
            ThreadEvent::ResponseCanceled { cause, .. } if !cause.is_redirect() => {
                self.say(session, UNFINISHED.to_string()).await;
            }
            _ => {}
        }
    }

    /// INVARIANT: The talker asked for the doer.
    ///
    /// **Never a new turn while this thread's doer is parked.** The doer is
    /// blocked inside the very card that is waiting, so there is no turn to
    /// start. What happens instead depends on the talker and on the card, and
    /// [`Call::settle_or_refuse`] decides.
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
        if let Some(parked) = self.decisions.parked_on(self.subject.thread_id).await {
            self.settle_or_refuse(session, tool_call_id, &reason, parked)
                .await;
            return;
        }
        self.acknowledge(session, tool_call_id, DELEGATION_TAKEN)
            .await;
        if self.delegated_this_turn {
            log_legacy!(
                "[Voice] The talker asked twice in one turn, ignoring: {}",
                reason
            );
            return;
        }
        // INVARIANT: Disarmed only where the ask was TAKEN, matching [`Call::answer`]. An
        // ignored duplicate starts nothing, so nobody is answering the caller
        // and the bound is still theirs. Disarming above this line left their
        // second question with no ask and no net under it.
        self.answer_is_no_longer_owed();
        log_legacy!(
            "[Voice] The talker asked for the doer: {}",
            if reason.trim().is_empty() {
                "it said nothing about why"
            } else {
                reason.trim()
            }
        );
        self.delegated_this_turn = true;
        self.hold_the_ask(reason);
        self.settle_the_delegation(session).await;
    }

    async fn settle_or_refuse(
        &mut self,
        session: &mut dyn VoiceSession,
        tool_call_id: &str,
        reason: &str,
        parked: OpenDecision,
    ) {
        let holds_the_answer_tool = self.provider.holds_the_answer_tool();
        let their_words = (!holds_the_answer_tool)
            .then(|| parked.their_words_choice())
            .flatten();
        let Some(choice) = their_words else {
            let note = if holds_the_answer_tool {
                DELEGATION_PARKED
            } else {
                DELEGATION_PARKED_ON_SCREEN
            };
            log_legacy!(
                "[Voice] Not delegating {:?}: this thread's doer is parked on a card",
                reason
            );
            self.acknowledge(session, tool_call_id, note).await;
            return;
        };
        log_legacy!(
            "[Voice] The ask settles what is waiting, in the caller's own words: {}",
            choice
        );
        // INVARIANT: Everything an answer needs comes with this call. A transcript still
        // in flight is held and settled on the utterance it was made for. The
        // words are spent, so no later ask runs on them.
        //
        // A held one acknowledges NOTHING yet, deliberately: see the
        // `NeedsTheirWords` arm. The tool call is answered when it settles, or
        // dropped with a note on the retry that still had no words.
        self.answer(session, tool_call_id.to_string(), choice.to_string(), false)
            .await;
    }

    fn hold_the_ask(&mut self, reason: String) {
        self.pending_delegation = Some(reason);
        self.ask_owed_by = Some(tokio::time::Instant::now() + CALLER_WAITED_LONG_ENOUGH);
    }

    fn take_the_ask(&mut self) -> Option<String> {
        self.ask_owed_by = None;
        self.pending_delegation.take()
    }

    /// INVARIANT: Give up on an ask whose words never came, and say so out loud.
    ///
    /// Left held, it waits for a transcript that is not coming, and the caller
    /// waits with it. Worse, it survives to claim a LATER sentence, which then
    /// runs under a reason taken from the one that never arrived.
    ///
    /// Said rather than appended, because nothing else reaches this caller.
    /// They were told it was on it, and a caller left in silence is the one
    /// outcome a call may not have.
    async fn the_ask_never_got_its_words(&mut self, session: &mut dyn VoiceSession) {
        let Some(stale) = self.take_the_ask() else {
            return;
        };
        log_legacy!(
            "[Voice] An ask never got the caller's words, so nothing ran: {}",
            stale
        );
        self.say(session, ASK_NEVER_GOT_ITS_WORDS.to_string()).await;
    }

    async fn answer(
        &mut self,
        session: &mut dyn VoiceSession,
        tool_call_id: String,
        choice_id: String,
        retry: bool,
    ) {
        let spoken = self.undelivered_words.clone().unwrap_or_default();
        let outcome = self
            .decisions
            .resolve(
                self.subject.thread_id,
                &choice_id,
                &spoken,
                self.subject.actor.clone(),
            )
            .await;
        if matches!(
            outcome,
            Resolution::Settled | Resolution::SettledWithTheirWords
        ) {
            self.answer_is_no_longer_owed();
        }
        match outcome {
            Resolution::Settled => self.acknowledge(session, &tool_call_id, ANSWER_TAKEN).await,
            Resolution::SettledWithTheirWords => {
                self.undelivered_words = None;
                // INVARIANT: An ask still waiting for those same words is now waiting for
                // nothing. Left sticky it pairs with a LATER utterance. That
                // wakes the doer on words asking for something else, under a
                // reason taken from the sentence just spent.
                //
                // Its "Taken." went out when it was made, and a tool call is
                // answered once. So the correction is appended instead, which
                // is what lets the talker tell the caller.
                if let Some(stale) = self.take_the_ask() {
                    log_legacy!("[Voice] The answer spent the words an ask was waiting for");
                    let note = match stale.trim() {
                        "" => ASK_LOST_ITS_WORDS.to_string(),
                        why => format!("{}\n\n{}", ASK_LOST_ITS_WORDS, why),
                    };
                    self.append(session, &note).await;
                }
                self.acknowledge(session, &tool_call_id, ANSWER_TAKEN).await;
            }
            Resolution::NeedsTheirWords if !retry => {
                let held = PendingAnswer {
                    tool_call_id,
                    choice_id,
                };
                if let Some(displaced) = self.pending_answer.replace(held) {
                    self.acknowledge(session, &displaced.tool_call_id, NEVER_HEARD)
                        .await;
                }
            }
            // INVARIANT: A retry that STILL has no words. Held again it would sit until
            // some later, unrelated sentence settled the card with it, and a
            // card cannot be unsettled. Dropped and said so instead.
            Resolution::NeedsTheirWords => {
                self.acknowledge(session, &tool_call_id, NEVER_HEARD).await;
            }
            Resolution::Refused(why) => self.acknowledge(session, &tool_call_id, &why).await,
        }
    }

    /// INVARIANT: Settle a held answer against the next thing the caller said.
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

    /// INVARIANT: Tell the talker one of its tool calls landed.
    ///
    /// Owed for every tool and every outcome. An unresolved call leaves a
    /// dangling item in the talker's history, and it reads that as work it
    /// never heard back about.
    async fn acknowledge(&self, session: &mut dyn VoiceSession, tool_call_id: &str, note: &str) {
        if let Err(e) = session.resolve_tool_call(tool_call_id, note).await {
            log_legacy!(
                "[Voice] The talker would not take the acknowledgement: {}",
                e
            );
        }
    }

    async fn ask(&mut self, session: &mut dyn VoiceSession, decision: OpenDecision) {
        let framing = decision_to_ask(&decision, self.provider.holds_the_answer_tool());
        self.say(session, framing).await;
    }

    async fn settle_the_delegation(&mut self, session: &mut dyn VoiceSession) {
        if self.undelivered_words.is_none() || self.pending_delegation.is_none() {
            return;
        }
        let transcript = self.undelivered_words.take().unwrap_or_default();
        let reason = self.take_the_ask().unwrap_or_default();
        self.heard_so_far.clear();
        self.answer_is_no_longer_owed();

        // INVARIANT: The wake first, so the doer's history already carries the reason by
        // the time the turn reading that history starts.
        let delegation = emit(
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
        let Some(delegation) = delegation else {
            log_legacy!("[Voice] The delegation was not recorded, so no turn was started");
            self.continuing = None;
            self.undelivered_words = Some(transcript);
            self.say(session, NOT_TAKEN.to_string()).await;
            return;
        };
        // INVARIANT: Seen here as well as on the subscription, so the pickup naming it
        // promotes its turn whichever reaches the call first.
        self.seen_on_this_call.insert(delegation);
        let taken = self
            .doer
            .wake(
                self.subject.thread_id,
                delegation,
                &transcript,
                self.subject.actor.clone(),
            )
            .await;
        self.continuing = taken.then(|| AskedTurn::new(delegation));
        if !taken {
            self.undelivered_words = Some(transcript);
            self.say(session, NOT_TAKEN.to_string()).await;
        }
    }

    fn turn_to_continue(&self) -> Option<Uuid> {
        self.continuing
            .filter(|_| self.pending_delegation.is_none())
            .map(|asked| asked.turn)
    }

    async fn caller_finished(
        &mut self,
        session: &mut dyn VoiceSession,
        transcript: String,
        words: CallerWords,
    ) -> ServerFrame {
        if !transcript.trim().is_empty() {
            // INVARIANT: Said again here as well as on the partial, because a provider
            // can report a finished turn having streamed no partial for it.
            self.open_the_floor(Opener::CallerWords);
            // INVARIANT: The talker answered whatever came before, so those words are its
            // business rather than the doer's. Left on the pile, a later ask
            // would run on a question already answered plus the new one.
            //
            // The rest of an asked sentence is not the caller's next one, so
            // it spends nothing. That holds with no turn to offer it to: an ask
            // the doer refused put the sentence's start back on the pile.
            if words == CallerWords::OwnSentence && std::mem::take(&mut self.talker_answered_them) {
                self.undelivered_words = None;
            }
            if self.reply_was_cut_off.is_cut() {
                self.write_down_the_reply().await;
            }
            self.write_a_spoken_row(transcript.clone()).await;
            match words {
                // INVARIANT: Offered after the row, so a turn not running yet reads them
                // from history. Not piled and not owed: that turn answers them.
                CallerWords::RestOfAsked(Some(asked_turn)) => {
                    self.doer
                        .continued(self.subject.thread_id, asked_turn, &transcript)
                        .await;
                }
                CallerWords::RestOfAsked(None) | CallerWords::OwnSentence => {
                    self.caller_said_more(&transcript);
                    self.caller_is_owed_an_answer();
                }
            }
        }
        self.heard_so_far.clear();
        // INVARIANT: The answer first: it is the one thing that can SPEND the transcript.
        // A delegation pairing with words already sent as an answer would run
        // that answer as a turn of its own.
        //
        // Outside the guard above, so a wordless turn settles a held answer
        // too. It settles it by giving up, which is the point: an answer held
        // past the utterance it was made for would eventually settle a card
        // with a sentence about something else.
        self.settle_the_pending_answer(session).await;
        self.settle_the_delegation(session).await;
        ServerFrame::UserTurnEnded { transcript }
    }

    async fn close_the_unfinished_partial(&mut self) {
        let partial = std::mem::take(&mut self.heard_so_far);
        if partial.trim().is_empty() {
            return;
        }
        self.write_a_spoken_row(partial.clone()).await;
        self.caller_said_more(&partial);
    }

    async fn write_a_spoken_row(&mut self, transcript: String) {
        if transcript.trim().is_empty() {
            return;
        }
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

    async fn everything_the_caller_said(&mut self) -> String {
        self.close_the_unfinished_partial().await;
        self.undelivered_words.take().unwrap_or_default()
    }

    fn caller_said_more(&mut self, text: &str) {
        let said = self.undelivered_words.take().unwrap_or_default();
        let joined = join_spoken(&said, text);
        if !joined.is_empty() {
            self.undelivered_words = Some(joined);
        }
    }

    fn talker_said(&mut self, delta: &str) {
        self.spoken_so_far.push(delta);
        if !delta.trim().is_empty() {
            self.talker_answered_them = true;
        }
    }

    fn open_the_floor(&mut self, opener: Opener) {
        if opener == Opener::CallerWords {
            self.the_caller_has_spoken = true;
        }
        if !self.floor.is_open() {
            log_legacy!("[Voice] The floor opened on {:?}", opener);
        }
        self.floor = Floor::Open {
            turns_left: TURNS_ONE_OPENER_BUYS,
        };
    }

    fn the_device_heard_them(&mut self) {
        if self.the_caller_has_spoken {
            self.open_the_floor(Opener::DeviceHeardThem);
        } else if self.floor == Floor::Shut {
            log_legacy!("[Voice] A sound before the caller's first words, so the talker is held");
            self.floor = Floor::Held;
        }
    }

    fn hold(&mut self, event: VoiceEvent) -> Option<VoiceEvent> {
        if self.floor != Floor::Held {
            return Some(event);
        }
        let ends_the_hold = match &event {
            VoiceEvent::Audio(_) => {
                let bytes: usize = self.held.iter().map(held_audio_bytes).sum();
                bytes + held_audio_bytes(&event) > HELD_AUDIO_BYTES
            }
            VoiceEvent::TalkerTranscript { text } => {
                if !text.trim().is_empty() {
                    self.talker_has_the_floor = true;
                }
                false
            }
            VoiceEvent::TalkerTurnEnded { transcript, .. } => {
                self.talker_has_the_floor = false;
                let turns_held = self
                    .held
                    .iter()
                    .filter(|held| matches!(held, VoiceEvent::TalkerTurnEnded { .. }))
                    .count();
                !the_sentence_is_unfinished(transcript)
                    || turns_held + 1 >= usize::from(TURNS_ONE_OPENER_BUYS)
            }
            _ => return Some(event),
        };
        self.held.push_back(event);
        if ends_the_hold {
            self.floor = Floor::Shut;
        }
        None
    }

    fn released(&mut self) -> Option<VoiceEvent> {
        if self.floor == Floor::Held {
            return None;
        }
        self.held.pop_front()
    }

    async fn forget_the_hold(&mut self) {
        if self.held.is_empty() {
            return;
        }
        self.talker_has_the_floor = false;
        for event in std::mem::take(&mut self.held) {
            if let VoiceEvent::TalkerTurnEnded { usage, .. } = event {
                self.capture
                    .record_usage(self.provider.model(), 0, Some(usage))
                    .await;
            }
        }
        log_legacy!("[Voice] Nobody spoke over it, so the talker's held output went unheard");
    }

    fn the_talker_spent_a_turn(&mut self) {
        let Floor::Open { turns_left } = self.floor else {
            return;
        };
        self.floor = match turns_left.saturating_sub(1) {
            0 => {
                log_legacy!(
                    "[Voice] The talker has had {} turns on one opener, so it is \
                     not heard again until the caller speaks",
                    TURNS_ONE_OPENER_BUYS
                );
                Floor::Shut
            }
            left => Floor::Open { turns_left: left },
        };
    }

    fn the_caller_hears_this(&self) -> bool {
        match self.audience {
            Audience::Nobody => false,
            Audience::TheCaller => true,
            Audience::Undecided => self.floor.is_open(),
        }
    }

    /// INVARIANT: Settle who this turn is for, on the first word of it.
    ///
    /// One-shot per turn: the turn end is what makes it undecided again. So a
    /// caller speaking mid-turn opens the floor for the NEXT turn, never for
    /// the rest of the one they spoke over.
    fn the_talker_said_a_word(&mut self) {
        if self.audience == Audience::Undecided {
            self.audience = if self.floor.is_open() {
                Audience::TheCaller
            } else {
                Audience::Nobody
            };
        }
    }

    fn caller_is_owed_an_answer(&mut self) {
        self.answer_owed_by = Some(tokio::time::Instant::now() + CALLER_WAITED_LONG_ENOUGH);
    }

    fn answer_is_no_longer_owed(&mut self) {
        self.answer_owed_by = None;
    }

    async fn nobody_answered_the_caller(
        &mut self,
        session: &mut dyn VoiceSession,
        transport: &mut dyn CallTransport,
    ) {
        self.answer_is_no_longer_owed();
        let said = self.everything_the_caller_said().await;
        if said.trim().is_empty() {
            return;
        }
        // INVARIANT: Taken above the seam, so the provider must stop holding its own copy.
        // A turn-less one accumulates until something asks, and would hand the
        // same sentence over again at its next boundary or at close. That is
        // one breath and two rows, which is the whole defect.
        session.caller_words_were_taken().await;
        // INVARIANT: Refused for the same reason `delegated` refuses: the doer is blocked
        // inside the very card that is waiting, so there is no turn to start.
        // The caller is still told, because they are still owed an answer.
        //
        // Never settles the card, whatever the talker holds. Nobody answered,
        // so nothing judged those words a move, and a card is not settled by a
        // sentence the talker did not hand over.
        if self
            .decisions
            .parked_on(self.subject.thread_id)
            .await
            .is_some()
        {
            log_legacy!(
                "[Voice] Nothing answered {:?}, and the doer is parked on a card",
                said
            );
            self.undelivered_words = Some(said);
            self.tell_the_caller(transport).await;
            return;
        }
        log_legacy!(
            "[Voice] Nothing answered {:?} in {}s, so the doer takes it",
            said,
            CALLER_WAITED_LONG_ENOUGH.as_secs()
        );
        self.undelivered_words = Some(said);
        // INVARIANT: Never over a sticky ask. The talker's own reason outlives the turn
        // that made it, exactly so a transcript still in flight can pair with
        // it. Clobbering it here would spend the caller's words under the wrong
        // reason and leave that ask to claim a later, unrelated sentence.
        if self.pending_delegation.is_none() {
            self.hold_the_ask(NOBODY_ANSWERED.to_string());
        }
        self.settle_the_delegation(session).await;
        self.tell_the_caller(transport).await;
    }

    async fn tell_the_caller(&mut self, transport: &mut dyn CallTransport) {
        if self.told_the_caller {
            return;
        }
        self.told_the_caller = true;
        let _ = transport
            .send_frame(ServerFrame::Error {
                message: VOICE_IS_NOT_ANSWERING.to_string(),
            })
            .await;
    }

    async fn take_what_was_still_held(&mut self, held: Vec<VoiceEvent>) {
        for event in held {
            match event {
                VoiceEvent::UserTurnEnded { transcript } => {
                    self.write_a_spoken_row(transcript.clone()).await;
                    self.caller_said_more(&transcript);
                    self.heard_so_far.clear();
                }
                VoiceEvent::UserTurnContinued { transcript } => {
                    self.write_a_spoken_row(transcript.clone()).await;
                    match self.turn_to_continue() {
                        Some(asked_turn) => {
                            self.doer
                                .continued(self.subject.thread_id, asked_turn, &transcript)
                                .await
                        }
                        None => self.caller_said_more(&transcript),
                    }
                    self.heard_so_far.clear();
                }
                VoiceEvent::UserTranscript { text } => self.heard_so_far.push_str(&text),
                VoiceEvent::TalkerTranscript { text } if !self.reply_was_cut_off.is_cut() => {
                    self.talker_said(&text)
                }
                // INVARIANT: Kept as the FALLBACK, never folded into the deltas. Those
                // already carry this turn, so replacing them here would drop
                // what the caller actually heard. A reply already written is
                // read for nothing: the drained end covers the whole of it,
                // tail included, and the caller stopped listening part-way.
                VoiceEvent::TalkerTurnEnded { transcript, usage } => {
                    // INVARIANT: The spend is recorded wherever the turn end arrives, and
                    // this is the other place it can: a socket that closed over
                    // a running response hands it back here.
                    self.capture
                        .record_usage(self.provider.model(), 0, Some(usage))
                        .await;
                    if !transcript.trim().is_empty()
                        && self.reply_was_cut_off != CutReply::CutAndWritten
                    {
                        self.last_turn_transcript = transcript;
                    }
                }
                _ => {}
            }
        }
    }

    async fn write_down_whatever_was_said(&mut self) {
        self.interrupted |= self.talker_has_the_floor;
        self.write_down_the_reply().await;
    }

    async fn say(&mut self, session: &mut dyn VoiceSession, note: String) {
        if self.the_talker_is_mid_turn() {
            self.waiting_to_be_said.push_back(note);
            return;
        }
        self.forget_the_hold().await;
        // INVARIANT: Nothing to write down here. Both sides record at their own turn
        // ends, and the floor check above means no talker turn is in flight.
        //
        // Claimed BEFORE the request, not when the first audio arrives. A
        // second answer landing in that window would otherwise be spoken over
        // the one already on its way.
        self.talker_has_the_floor = true;
        self.relaying = true;
        if let Err(e) = session.speak(&note).await {
            log_legacy!("[Voice] The talker would not take the answer: {}", e);
            self.talker_has_the_floor = false;
            self.relaying = false;
            return;
        }
        self.open_the_floor(Opener::Engine);
        // INVARIANT: A run the engine handed over is a new one, whatever the talker was
        // half way through saying to nobody. Left held, the mute would eat the
        // one answer we asked for.
        self.audience = Audience::Undecided;
        self.mute_held_for = 0;
    }

    async fn say_what_is_waiting(&mut self, session: &mut dyn VoiceSession) {
        if self.the_talker_is_mid_turn() {
            return;
        }
        if let Some(note) = self.waiting_to_be_said.pop_front() {
            self.say(session, note).await;
        }
    }

    /// INVARIANT: Whether the talker is mid-turn, so an answer must wait.
    ///
    /// Output still draining from a decided hold counts: the provider's turn
    /// behind it is in flight, and forgetting it would drop what the caller
    /// is about to hear.
    fn the_talker_is_mid_turn(&self) -> bool {
        self.talker_has_the_floor || (self.floor != Floor::Held && !self.held.is_empty())
    }

    async fn append(&mut self, session: &mut dyn VoiceSession, note: &str) {
        if let Err(e) = session.append_context(note).await {
            log_legacy!("[Voice] The talker would not take a note: {}", e);
        }
    }

    async fn write_down_the_reply(&mut self) {
        // INVARIANT: The turn is accounted for now, however it ended. Forgotten here,
        // so nothing downstream writes the same words a second time.
        let (streamed, age) = self.spoken_so_far.take();
        let reported = std::mem::take(&mut self.last_turn_transcript);
        if !self.the_caller_hears_this() {
            return;
        }
        let (transcript, spoken_secs_before) = if streamed.trim().is_empty() {
            (reported, None)
        } else {
            (streamed.trim().to_string(), age)
        };
        if transcript.trim().is_empty() {
            return;
        }
        // INVARIANT: READ, never taken. The flag belongs to the REPLY, and a cut one is
        // written mid-turn by the caller's own turn end: taking it there left
        // the row right and the turn that follows saying nobody cut in.
        // Taking it here instead leaked it onto the next reply, which a cut
        // that wrote nothing never cleared. The turn end owns it, below.
        let interrupted = self.interrupted;
        if self.reply_was_cut_off == CutReply::Cut {
            self.reply_was_cut_off = CutReply::CutAndWritten;
        }
        emit(
            &self.bus,
            self.subject.thread_id,
            ThreadEvent::SpokenReplyGenerated {
                session_id: self.subject.session_id,
                text: transcript,
                interrupted,
                spoken_secs_before,
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
            CallerFrame::Control(ClientControl::CallerStartedSpeaking) => {
                Step::CallerStartedSpeaking
            }
            CallerFrame::Control(ClientControl::HangUp) => {
                Step::Ended(VoiceSessionEndReason::Hangup)
            }
            CallerFrame::Undecodable => Step::Undecodable,
            CallerFrame::Closed => Step::Ended(VoiceSessionEndReason::Disconnected),
        }
    }
}

fn delivered(sent: Result<(), BoxError>) -> Option<VoiceSessionEndReason> {
    match sent {
        Ok(()) => None,
        Err(e) => {
            log_legacy!("[Voice] The caller stopped receiving: {}", e);
            Some(VoiceSessionEndReason::Disconnected)
        }
    }
}

async fn provider_failed(transport: &mut dyn CallTransport) -> VoiceSessionEndReason {
    let _ = transport
        .send_frame(ServerFrame::Error {
            message: "The voice service stopped responding.".to_string(),
        })
        .await;
    VoiceSessionEndReason::ProviderFailed
}

async fn emit(
    bus: &EventBus,
    thread_id: Uuid,
    event: ThreadEvent,
    meta: EventMeta,
) -> Option<Uuid> {
    let ctx = format!("[Voice] {}", event.event_type());
    match bus
        .emit(BusEvent::Thread {
            thread_id,
            event,
            meta,
        })
        .await
    {
        Ok(result) => result.map(|r| r.event_id),
        Err(e) => {
            log_legacy!("[EventBus] {} emit failed: {}", ctx, e);
            None
        }
    }
}

pub async fn opening_for(engine: &LucidosEngine, thread_id: Uuid) -> SessionOpening {
    let pool = engine.pool();
    let language = language::for_workspace(pool).await;
    let literacy = crate::core::technical_literacy::read(pool).await;
    SessionOpening {
        instructions: super::instructions_for(language.as_ref(), literacy),
        resident_block: resident::build_block(engine, thread_id).await,
        voice: build::talker_voice(pool).await,
        transcriber: build::transcriber_model(pool).await,
        audio: Default::default(),
        language,
    }
}

#[cfg(test)]
#[path = "call_tests.rs"]
mod tests;
