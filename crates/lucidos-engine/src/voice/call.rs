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
//! this file reaching into the engine. It is also what a delegation meets while
//! the doer is parked inside a card of its own: see [`Call::settle_or_refuse`],
//! which is how a talker holding no answering tool settles one anyway.
//!
//! **Naming the thread is the fourth.** A [`ThreadNamer`] is asked once the
//! call has an exchange in it, and decides everything else on the far side of
//! a spawn. The loop is never held up for a name.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::broadcast::Receiver;
use uuid::Uuid;

use super::decision::{DecisionKind, DecisionResolver, OpenDecision, Resolution};
use super::doer::TurnStarter;
use super::naming::ThreadNamer;
use super::provider::{wait_until, SessionOpening, VoiceEvent, VoiceProvider, VoiceSession};
use super::wire::{ClientControl, ServerFrame};
use super::{build, language, resident};
use crate::core::store::messages::spoken_merge::join_spoken;
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
///
/// For a talker that HOLDS the answering tool. One that does not reads
/// [`DELEGATION_PARKED_ON_SCREEN`], because this note's last sentence asks for
/// a tool it has never been given.
const DELEGATION_PARKED: &str = "\
Not started. Lucidos is waiting on the caller's answer to what is already open, \
so nothing new can run until they settle it. Put that back to them, and answer \
it with what they say.";

/// The same refusal, for a talker with no way to settle the card in front of it.
///
/// Reached only by a permission card, which takes a decision rather than words.
/// A question card never lands here: its free-text choice is what
/// `Call::delegated` presses instead.
///
/// It sends the caller to the screen, which is the only true thing left. The
/// note above would be a promise this talker cannot keep, and a talker that
/// keeps promising is what one reported call was
/// (`docs/plans/2026-09-17-a-live-caller-settles-what-is-waiting.md`).
const DELEGATION_PARKED_ON_SCREEN: &str = "\
Not started. Lucidos needs the caller's say-so on what is already open, and \
that one is settled on their screen rather than out loud. Tell them what is \
waiting and that the card is there, then wait for them.";

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
[NOT STARTED] The caller's words settled what was waiting, so the request you \
made never started. Nothing is running for it. Offer it to them again if they \
still want it.";

/// What the talker is told when an ask waited for words that never came.
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

/// How long a caller may wait with nobody answering them.
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

/// How many turns the talker may be heard for on one opener.
///
/// **An answer, not a call.** A Live turn ends at every 700 ms hole in the
/// talker's words, so one spoken answer spans several of them (ADR 0187). That
/// is why the floor survives a turn end. It is not a reason to survive the
/// whole call, and held that far it licensed a recitation the caller could not
/// stop: see [`Floor`].
///
/// **Counted in turns, because no silence window separates the two.** The
/// reported recitation paused for up to 2.3 seconds between turns, and a real
/// answer on the same thread paused for eighteen. A window short enough to cut
/// the first truncates the second. Turn count does separate them: the longest
/// real answer in that thread ran to six turns, the recitation to eighteen.
///
/// Eight leaves margin over that six. Past it the caller has heard a monologue
/// rather than an answer, and one word from them buys the next one.
const TURNS_ONE_OPENER_BUYS: u8 = 8;

/// The reason on a wake nobody asked for.
///
/// It says what happened rather than inventing an intent. The talker composed
/// no reason, because the talker did nothing at all.
const NOBODY_ANSWERED: &str =
    "the talker said nothing, so the caller's question came straight here";

/// What the caller is shown when the talker has gone quiet on them.
///
/// Shown, because there is nothing else left: the one way to reach a caller's
/// EAR is the talker, and the talker is what failed. So the transcript carries
/// it, and it says where the answer will appear.
const VOICE_IS_NOT_ANSWERING: &str =
    "The voice is not answering. Your question went to Lucidos anyway, \
                                      and the answer will appear in this conversation.";

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
/// out loud, and how depends on what the talker holds.
///
/// **A talker holding the answering tool hands back a choice id.** One holding
/// none cannot. It is told the thing it CAN do instead: hand their words over,
/// which is what settles a question card (ADR 0205).
///
/// A permission card is the one such a talker cannot settle at all. The framing
/// names the screen there, rather than offering a choice nothing can press.
fn decision_to_ask(decision: &OpenDecision, holds_the_answer_tool: bool) -> String {
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
    let how = match (holds_the_answer_tool, decision.kind) {
        (true, _) => {
            "They answer by saying which one they want, and you hand its id back. \
             Never say an id out loud."
        }
        (false, DecisionKind::Question) => {
            "They answer by saying which one they want, in their own words. Hand \
             what they said over, which is what settles this. Never say an id out \
             loud."
        }
        // Exhaustive on this side too, like the opening above it. A fifth kind
        // has to say whether a tool-less talker can settle it, rather than
        // inheriting the screen by default.
        (false, DecisionKind::CommandPermission)
        | (false, DecisionKind::McpPermission)
        | (false, DecisionKind::CodingAgentPermission) => {
            "This one they settle on their screen rather than out loud. Tell them \
             the card is there. Never say an id out loud."
        }
    };
    // The prompt itself is NEVER cut, unlike everything else the talker reads.
    // A truncated question is a different question, and the talker is about to
    // state it as the one being asked.
    format!(
        "{} Put this to them out loud, in your own words, and read them the \
         choices. {}\n\n{}\n\n{}",
        opening,
        how,
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
///
/// Five of the arguments are the SEAMS this file is built on, and each is a
/// separate thing a call can be given: the talker, the caller, the doer, what
/// is waiting, and what names the thread. Bundling them would hide which ones
/// a test is standing in for, which is the whole point of having them.
#[allow(clippy::too_many_arguments)]
pub async fn run_call(
    bus: &EventBus,
    provider: &dyn VoiceProvider,
    transport: &mut dyn CallTransport,
    doer: &dyn TurnStarter,
    decisions: &dyn DecisionResolver,
    namer: &dyn ThreadNamer,
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
        namer,
        asked_for_a_name: false,
        capture: AuxCapture::new(bus, subject.thread_id, ContextPurpose::Voice),
        subject: subject.clone(),
        thread,
        talker_has_the_floor: false,
        interrupted: false,
        reply_was_cut_off: CutReply::Running,
        relaying: false,
        waiting_to_be_said: VecDeque::new(),
        spoken_so_far: SpokenTurn::default(),
        last_turn_transcript: String::new(),
        talker_answered_them: false,
        undelivered_words: None,
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
    };
    let reason = call.drive(&mut *session, transport).await;
    // Closed FIRST, and that ordering is the whole of it. A provider with no
    // turn-end frame holds the caller's last sentence until the socket goes.
    // The flush below used to run while those words were still upstream.
    // `VoiceSession::close` hands them back.
    call.take_what_was_still_held(session.close().await).await;
    // Both flushes run for EVERY end reason. A hangup, a dropped socket and a
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
    /// The caller opened their mouth, measured on their own device.
    CallerStartedSpeaking,
    Undecodable,
    Talker(VoiceEvent),
    /// The talker closed the session on its own side.
    TalkerGone,
    /// Boxed because the payload dwarfs every other variant here.
    Thread(Box<EmittedEvent>),
    /// The caller has been waiting [`CALLER_WAITED_LONG_ENOUGH`] with nothing
    /// answering them.
    NobodyAnsweredTheCaller,
    /// A held ask has waited that same bound for words that never came.
    AskNeverGotItsWords,
    Ended(VoiceSessionEndReason),
}

/// The talker's words for one turn, and the moment they started arriving.
///
/// **One value, and that is the point.** The row needs both, and a row written
/// without its moment reads where it was recorded rather than where it was
/// said. Taking them apart is how that happens, so [`SpokenTurn::take`] hands
/// back the pair or nothing (ADR 0206).
#[derive(Default)]
struct SpokenTurn {
    words: String,
    began: Option<Instant>,
}

impl SpokenTurn {
    /// Take a delta, stamping the moment the first WORDED one arrived.
    ///
    /// Words are what the caller hears, so they are what the age measures. A
    /// provider opening a turn with whitespace would otherwise start the clock
    /// before anything was said.
    fn push(&mut self, delta: &str) {
        if !delta.trim().is_empty() {
            self.began.get_or_insert_with(Instant::now);
        }
        self.words.push_str(delta);
    }

    /// The words and how long ago they began, leaving the stretch empty.
    ///
    /// The age comes off the monotonic clock, never a wall clock. The row's
    /// `created` is Postgres's, and the transcript subtracts one from the other
    /// (ADR 0053).
    fn take(&mut self) -> (String, Option<f64>) {
        let age = self.began.take().map(|at| at.elapsed().as_secs_f64());
        (std::mem::take(&mut self.words), age)
    }
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
    /// What names the thread, once this call has something to name it by.
    namer: &'a dyn ThreadNamer,
    /// Whether this call has already asked for a name.
    ///
    /// Every utterance after the first one answers a reply too, and asking on
    /// each would put a read behind every sentence the caller says. The name is
    /// settled by the first ask.
    ///
    /// Not the idempotency rule, which is the engine's: a thread that already
    /// has a name is never renamed, whoever asks.
    asked_for_a_name: bool,
    capture: AuxCapture,
    subject: CallSubject,
    thread: Receiver<EmittedEvent>,
    /// True while the talker owes the caller the rest of a sentence.
    ///
    /// Claimed by its WORDS, and by [`Call::say`] asking it for some. Never by
    /// its audio: a provider that streams audio between turns would take the
    /// floor once and hold it until the line closed. Released by the turn end
    /// alone, so a reply the caller is still hearing is never spoken over.
    talker_has_the_floor: bool,
    /// The caller cut in. Read by the turn end that follows it.
    interrupted: bool,
    /// What the caller did to the reply the talker is speaking NOW.
    ///
    /// Set by their barge-in, and by a provider that reports the cut itself.
    /// Back to [`CutReply::Running`] when the talker's words stop, which is
    /// where the reply ends. Distinct from [`Call::interrupted`], which
    /// belongs to the ROW and is spent when the row is written.
    reply_was_cut_off: CutReply,
    /// The reply the talker is composing was handed to it, so a running round
    /// already knows what it says. Set by [`Call::say`], read and cleared by
    /// the turn end that follows.
    relaying: bool,
    /// Doer answers that arrived while the talker was speaking. A queue
    /// rather than a slot, because dropping one loses an answer the caller
    /// asked for and never hears.
    waiting_to_be_said: VecDeque<String>,
    /// What the talker has said during the turn now running, and when it
    /// started saying it.
    ///
    /// Built from the transcript deltas as they pass through, and written out
    /// by the turn's own end. One turn is one row, so `created` is when the
    /// words stopped rather than a move of the conversation later (ADR 0201).
    ///
    /// The deltas rather than the turns' own transcripts, because each of
    /// those is trimmed. Joining them puts a space before a full stop, or
    /// loses the one between two words. A delta carries its own spacing.
    ///
    /// Cleared by [`Call::write_down_the_reply`], the one writer of the row.
    spoken_so_far: SpokenTurn,
    /// What the provider called the talker's last turn, for the one reply that
    /// streamed no deltas at all.
    ///
    /// The deltas are what the caller HEARD, so they are the row. This is the
    /// fallback for a provider that reports a finished turn without having
    /// streamed it, which would otherwise lose the reply outright.
    last_turn_transcript: String,
    /// The talker has spoken since the caller last finished a sentence.
    ///
    /// So whatever they said before that is the talker's business rather than
    /// the doer's, and [`Call::undelivered_words`] is dropped at their next
    /// sentence. Without it a later ask runs on a question already answered,
    /// glued to the new one.
    ///
    /// **Not spent by a pause** (ADR 0191). It says the talker answered, and a
    /// talker that says "Okay.", draws breath and then asks has still answered
    /// nothing. That is why the caller's own next sentence spends it.
    talker_answered_them: bool,
    /// Words the caller has said that the doer has not been given yet.
    ///
    /// A COPY of text already on the thread, never the only record of it.
    /// Every caller utterance is written the instant it finishes, so nothing
    /// here is owed a row. What it answers is a different question: a
    /// delegation arriving later needs the words to run the doer on, and the
    /// two frames come from two models on one socket.
    ///
    /// Spent by the delegation that runs on it, and by the end of the call.
    undelivered_words: Option<String>,
    /// The talker's reason from a `delegate` call with no utterance to pair
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
    /// When a held ask gives up on the words it was made for.
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
    /// What the provider has heard of the caller SO FAR, before it says the
    /// turn finished.
    ///
    /// The caller's half of [`Self::spoken_so_far`], held for the same reason.
    /// A protocol with no turn-end frame reports the caller through these
    /// partials alone. A call that stops first has nothing else to write a row
    /// from. Superseded by the finished transcript, which covers the same
    /// stretch.
    heard_so_far: String,
    /// When the caller stops being owed an answer by nobody in particular.
    ///
    /// Armed by the caller saying anything, and disarmed by the conversation
    /// moving: the talker speaking, a delegation, an answer, or the doer being
    /// woken. See [`CALLER_WAITED_LONG_ENOUGH`].
    answer_owed_by: Option<tokio::time::Instant>,
    /// The caller has already been told the voice is not answering. Once per
    /// call: a second card says nothing the first did not.
    told_the_caller: bool,
    /// Whether the talker may be heard, and for how many more turns.
    ///
    /// Opened by three things, and spent by the answer it bought. See
    /// [`Floor`], [`Call::open_the_floor`] and [`Call::the_talker_spent_a_turn`].
    ///
    /// Read per turn by [`Audience`]. While it is shut, the talker is talking
    /// to nobody.
    floor: Floor,
    /// Who the turn the talker is speaking now is for.
    ///
    /// Decided by that turn's first WORD, and back to [`Audience::Undecided`]
    /// at its end. A mute mid-sentence is the exception: see
    /// [`Call::mute_held_for`].
    audience: Audience,
    /// How many turns running an unheard sentence has held the mute.
    ///
    /// **A mute covers a sentence, and a Live turn is smaller than one.** So
    /// the latch survives a turn end while the words are unfinished. See
    /// [`the_sentence_is_unfinished`] for why, and for what it reads.
    ///
    /// Counted so a talker that never punctuates cannot mute the rest of the
    /// call. `TURNS_ONE_OPENER_BUYS` is the length ADR 0213 measured one answer
    /// at, and a mute has no business outlasting the answer it covers.
    mute_held_for: u8,
}

/// Whether the talker may be heard at all, and for how much longer.
///
/// **One opener buys one answer** (ADR 0213). A call opens `Shut`. Three things
/// open it, each with a fresh budget: the caller saying anything, the provider
/// hearing them start, and the engine handing the talker something to say.
///
/// The budget is what a held floor was missing. Held for the call, the caller's
/// first hello licensed every later turn. One reported call spent forty-nine
/// seconds reciting the thread's own history at somebody who had said one word
/// (`docs/plans/2026-09-17-one-opener-buys-one-answer.md`).
///
/// **`Open` always has a turn left in it.** [`Call::the_talker_spent_a_turn`]
/// is the only writer besides the openers, and it moves to `Shut` rather than
/// leaving a budget of nought behind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Floor {
    /// Nothing the talker produces reaches the caller or the thread.
    Shut,
    /// It does, for this many more turns.
    Open { turns_left: u8 },
}

impl Floor {
    /// Whether an undecided turn starting now would be heard.
    fn is_open(self) -> bool {
        self != Floor::Shut
    }
}

/// Who hears the turn the talker is speaking now.
///
/// **A call opens with the floor shut, and this is the shutter** (ADR 0211).
/// The talker is handed the whole of this conversation at open. On a Live
/// session that block arrives as steering. The model reads its last line as a
/// turn nobody answered, and answers it before the caller has said hello.
///
/// One reported call spent fourteen seconds that way. The caller said nothing
/// and the thread took five rows, the last four of them the thread's own
/// history read back out
/// (`docs/plans/2026-09-17-a-call-opens-silent-until-the-caller-speaks.md`).
///
/// **Decided per SENTENCE, by its first word.** Half a babbled sentence must
/// not start playing because the caller spoke over the other half. Words alone
/// decide it, never audio. A Live talker streams audio between turns, so a
/// turn read off the stream is decided at call open and never again (ADR
/// 0187).
///
/// Per turn was the first shape, and a turn is the wrong unit. A Live one ends
/// at every 700 ms hole in the words. So the latch expired mid-sentence, and
/// the caller heard the back half of a dropped one (ADR 0218). See
/// [`the_sentence_is_unfinished`] and [`Call::mute_held_for`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Audience {
    /// No turn is running, so the next word decides.
    Undecided,
    /// Nobody. It began before the caller's first word and nothing invited it,
    /// so it is played to no one and written nowhere.
    Nobody,
    /// The caller, which is every turn once they have spoken, and every turn
    /// the engine asked for.
    TheCaller,
}

/// Whether these words stop mid-sentence, so more of them is coming.
///
/// **What a mute has to cover is a sentence** (ADR 0218). A Live turn ends at
/// every 700 ms hole in the words (ADR 0187), so one sentence spans several of
/// them. Ended per turn, a mute lets the caller hear the back half of a
/// sentence whose front half was dropped. ADR 0211 named that outcome when it
/// rejected re-asking per delta, and the per-turn latch produced it anyway.
///
/// **The words decide, and nothing else may.** ADR 0213 measured that no
/// silence window separates a recitation from a real answer, so no clock,
/// duration or frame count belongs here.
///
/// Closing quotes and brackets come off first, so a quoted sentence ends where
/// its full stop is. Nothing said finishes nothing, which leaves a wordless
/// turn deciding no mute either way.
fn the_sentence_is_unfinished(transcript: &str) -> bool {
    let words = transcript.trim_end_matches(|c: char| {
        c.is_whitespace() || matches!(c, '"' | '\'' | ')' | ']' | '}' | '»' | '”' | '’')
    });
    match words.chars().next_back() {
        None => false,
        Some(last) => !matches!(last, '.' | '!' | '?' | '…'),
    }
}

/// What the caller did to the reply the talker is speaking now.
///
/// **A Live talker cannot be cancelled**, so the engine is what makes a cut
/// real: past one, the rest of that reply reaches neither the caller's ear nor
/// its row. Left through, the client's own stop-playback throws the queue away,
/// the talker keeps streaming, and the caller hears the reply resume as a
/// fragment.
///
/// A cut also says the reply is OVER, so the caller's next finished words read
/// below its row rather than above them. A turn landing inside a reply that is
/// still running says nothing of the sort: that reply writes itself at its own
/// turn end, like every other row (ADR 0201).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CutReply {
    /// Nobody cut in, so the reply is heard and written in full.
    Running,
    /// They cut it off, and the row for what they DID hear is still open.
    Cut,
    /// The same, with that row already written.
    ///
    /// **What the provider reports at the turn's end is then read for
    /// nothing.** It covers the WHOLE reply, tail included, and the caller
    /// stopped listening part-way through. Taken as the next stretch's
    /// fallback, it writes the reply a second time, in full, saying nobody
    /// cut it off.
    CutAndWritten,
}

impl CutReply {
    /// Is the rest of this reply withheld from the caller?
    fn is_cut(self) -> bool {
        self != CutReply::Running
    }

    /// The caller cut this reply off, whoever said so first.
    ///
    /// Saturating: a reply whose row is already down stays
    /// [`CutReply::CutAndWritten`]. Two surfaces report one cut, the caller's
    /// barge-in and the provider's own frame, and the row can go down between
    /// them.
    fn and_cut(self) -> Self {
        match self {
            CutReply::CutAndWritten => CutReply::CutAndWritten,
            _ => CutReply::Cut,
        }
    }
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
            let owed = self.answer_owed_by;
            let asked = self.ask_owed_by;
            let step = tokio::select! {
                from_caller = transport.recv() => from_caller.into(),
                from_talker = session.next() => match from_talker {
                    Some(event) => Step::Talker(event),
                    None => Step::TalkerGone,
                },
                from_thread = self.thread.recv() => thread_step(from_thread),
                _ = wait_until(owed) => Step::NobodyAnsweredTheCaller,
                _ = wait_until(asked) => Step::AskNeverGotItsWords,
            };

            match step {
                Step::Nothing => {}
                Step::CallerAudio(pcm) => {
                    if let Err(e) = session.push_audio(&pcm).await {
                        log!("[Voice] The talker stopped taking audio: {}", e);
                        return provider_failed(transport).await;
                    }
                }
                Step::BargeIn => self.the_caller_cut_in(session).await,
                // The floor and nothing else, exactly as the provider's own
                // signal does. It says the caller opened their mouth, which no
                // transcriber can withhold (ADR 0211).
                //
                // The device measured it, so it reaches every provider. The
                // Live one states no such frame, and its talker answers the
                // caller before its transcriber reports them.
                Step::CallerStartedSpeaking => self.open_the_floor(),
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
                Step::NobodyAnsweredTheCaller => {
                    self.nobody_answered_the_caller(session, transport).await
                }
                Step::AskNeverGotItsWords => self.the_ask_never_got_its_words(session).await,
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
            // Forwarded, and it claims NOTHING. A Live talker streams audio
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
            // Held as well as forwarded, exactly as the talker's deltas are.
            // A partial is not a sentence, so it gets no row of its own. It is
            // still the only account of the caller a turn-less protocol gives
            // before it closes. See [`Call::heard_so_far`].
            //
            // Concatenated raw, never space-joined: a delta carries its own
            // spacing, and one can split a word in two.
            VoiceEvent::UserTranscript { text } => {
                self.heard_so_far.push_str(&text);
                self.caller_is_owed_an_answer();
                // Words while the line is closing say they were not done, and
                // the client sends no barge-in for somebody who never stopped.
                // So this is the only signal that reaches a goodbye the talker
                // began over the top of them.
                if !text.trim().is_empty() {
                    self.the_caller_was_not_done();
                    self.open_the_floor();
                }
                ServerFrame::UserTranscript { text }
            }
            // The floor and nothing else. It says the caller opened their
            // mouth, which no transcriber can withhold, and it is what a
            // partial-less one leaves us with (ADR 0211).
            //
            // No frame: the client draws the caller off its own microphone
            // (ADR 0184), so this would tell it what it already knows.
            VoiceEvent::CallerStartedSpeaking => {
                self.open_the_floor();
                return None;
            }
            VoiceEvent::UserTurnEnded { transcript } => {
                // Recorded at once, so the row's `created` is when the caller
                // stopped speaking. Nothing finished waits in memory for a move
                // of the conversation, and nothing is stamped a move later.
                //
                // Always a `SpokenMessageReceived`, whatever the talker does
                // next. A caller utterance starts no turn: the talker's own
                // delegation does, and that is a separate fact (ADR 0201).
                //
                // A transcript with no words is not a row and not held. Held,
                // it would pair with a waiting ask and spend it on nothing.
                if !transcript.trim().is_empty() {
                    // Said again here as well as on the partial, because a
                    // provider can report a finished turn having streamed no
                    // partial for it.
                    self.open_the_floor();
                    // The talker answered whatever came before, so those
                    // words are its business rather than the doer's. Left on
                    // the pile, a later ask would run on a question already
                    // answered plus the new one.
                    let answered_before = std::mem::take(&mut self.talker_answered_them);
                    if answered_before {
                        self.undelivered_words = None;
                    }
                    // **The reply is over only if the caller TOOK THE FLOOR.**
                    // Finishing a sentence the talker jumped in on is not
                    // that: it is still speaking, and its own turn end writes
                    // the row. Written here, one breath cut a reply in two
                    // (ADR 0200).
                    //
                    // Cut, the words were said BEFORE these, so the row goes
                    // first. The other order puts an answer above its
                    // question.
                    if self.reply_was_cut_off.is_cut() {
                        self.write_down_the_reply().await;
                    }
                    self.write_a_spoken_row(transcript.clone()).await;
                    // **This is the moment a call earns a name.** The caller
                    // answered something the talker said, so the thread holds
                    // both voices and a subject. An opening "hey" reaches
                    // nobody's ear but the talker's, and names nothing.
                    //
                    // After the row, because the namer reads the exchange back
                    // out of the thread and this utterance is half of it.
                    if answered_before && !self.asked_for_a_name {
                        self.asked_for_a_name = true;
                        self.namer.name_this_call(self.subject.thread_id).await;
                    }
                    self.caller_said_more(&transcript);
                    self.caller_is_owed_an_answer();
                }
                // The finished text covers the stretch the partials described,
                // so they are spent rather than written down twice.
                self.heard_so_far.clear();
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
                self.settle_the_delegation(session).await;
                // The row and this frame say the same thing, so the bubble the
                // client draws matches the row that replaces it. A turn is the
                // unit of both, and the transcript glues neighbours together.
                ServerFrame::UserTurnEnded { transcript }
            }
            VoiceEvent::TalkerTranscript { text } => {
                // **Both of these run for a turn nobody hears**, which is what
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
                // Dropped once the caller has cut this reply off. They never
                // heard these words, so a row carrying them would claim they
                // did. See [`CutReply`].
                //
                // Dropped again for a turn nobody is listening to, which is
                // the same claim made a beat earlier: these words reached no
                // ear either. See [`Audience`].
                if self.reply_was_cut_off.is_cut() || !self.the_caller_hears_this() {
                    return None;
                }
                // The talker is answering, so nobody else owes the caller one.
                if !text.trim().is_empty() {
                    self.answer_is_no_longer_owed();
                }
                // Held as well as forwarded. This is the only account of the
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
                // The line is closing, so nothing is owed an answer any more.
                self.answer_is_no_longer_owed();
                // Held, NOT acted on. A tool call lands while the talker is
                // still speaking, which is what makes delegation free and what
                // would cut the goodbye off mid-word here. The turn's end is
                // where the line closes.
                self.hanging_up = true;
                return None;
            }
            // **The talker's turn ended.** On a provider that states its own
            // turn ends this really is one, and on a Live call it is a hole in
            // the words. Either way the words so far are final, so they are
            // written down here and the row is dated by this moment.
            VoiceEvent::TalkerTurnEnded { transcript, usage } => {
                // Recorded HERE, where the number arrives, and never with the
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
                // Who this turn was for. A turn nobody heard reaches no row,
                // no running round and no flag saying the caller was answered
                // (ADR 0211).
                //
                // Read here, and made undecided again only once the row is
                // past: `write_down_the_reply` asks the same question.
                let heard = self.the_caller_hears_this();
                // An unheard sentence is not done, so the mute outlives this
                // turn. Bounded, so a talker that never punctuates cannot mute
                // the rest of the call. See [`Call::mute_held_for`].
                //
                // **Only over an OPEN floor**, which is the case the hold is
                // for: the caller spoke mid-sentence, and the rest of it is not
                // theirs to hear. Over a shut one the mute is already total, so
                // holding it would only delay the answer their next word buys
                // (ADR 0213).
                let holding = !heard
                    && self.floor.is_open()
                    && self.mute_held_for < TURNS_ONE_OPENER_BUYS
                    && the_sentence_is_unfinished(&transcript);
                self.mute_held_for = if holding { self.mute_held_for + 1 } else { 0 };
                if !heard {
                    // Loud, because a caller who hears nothing has no other
                    // trace to be found by. Their transcript failing is one way
                    // here: the floor opens on their words.
                    //
                    // It names the FLOOR rather than a cause, because there are
                    // now two. A call that never opened one, and a monologue
                    // that spent its budget, which `the_talker_spent_a_turn`
                    // announces once when it happens.
                    //
                    // A held run says so, so the turn that resumes a sentence
                    // nobody heard is greppable beside the one that began it.
                    log!(
                        "[Voice] The floor was shut, so nobody heard the talker{}: {}",
                        if holding {
                            ", and its sentence is held"
                        } else {
                            ""
                        },
                        super::clip(transcript.trim(), super::READ_ALOUD_CHARS)
                    );
                }
                // The reply is over, whatever the caller did to it. The next
                // one is heard in full unless they cut that one off too.
                let cut = std::mem::replace(&mut self.reply_was_cut_off, CutReply::Running);
                // A reply already written down reports NOTHING here. See
                // [`CutReply::CutAndWritten`].
                let owed_a_row = heard && cut != CutReply::CutAndWritten;
                // A wordless turn reports nothing, so it must not wipe what an
                // earlier one did report before its row was written.
                if owed_a_row && !transcript.trim().is_empty() {
                    self.last_turn_transcript = transcript.clone();
                    // The other half of `talker_said`, for a provider that
                    // reports its words only here.
                    self.talker_answered_them = true;
                }
                // Offered before the row, because a running round is what it is
                // for: it learns what the caller was told in its name while it
                // can still act on it (ADR 0164).
                //
                // A relay is skipped, and the flag is spent on the turn that
                // ends it. The round wrote that answer itself, so offering it
                // back is the round reading its own words as news.
                //
                // A CUT reply is offered to nobody. This transcript covers
                // the tail the caller was never played. A round told they
                // heard it makes the same wrong claim the cut refuses.
                //
                // A turn nobody heard is offered to nobody either, for that
                // same reason: it was told in no one's name.
                let relaying = std::mem::take(&mut self.relaying);
                if heard && !relaying && !cut.is_cut() && !transcript.trim().is_empty() {
                    self.doer
                        .overheard(self.subject.thread_id, &transcript)
                        .await;
                }
                // The words the caller just heard, on the thread at once. A
                // turn is the unit of the row, so `created` is when the talker
                // stopped (ADR 0201). Rejoining the pieces is the reader's job:
                // see `core::store::messages::spoken_merge`.
                self.write_down_the_reply().await;
                // One opener buys one answer, and this turn was part of one
                // (ADR 0213). After the row, so the turn that spends the last
                // of the budget is still heard whole.
                //
                // A turn nobody heard spends nothing: it was never on the
                // budget. Nor is a wordless one, which is the provider's
                // silence rather than an answer.
                if heard && !transcript.trim().is_empty() {
                    self.the_talker_spent_a_turn();
                }
                // The row is past, so the next word decides afresh. Unless the
                // sentence is held: a mute covers the whole of one, never a
                // 700 ms turn of it.
                self.audience = if holding {
                    Audience::Nobody
                } else {
                    Audience::Undecided
                };
                // The reply is over, so its cut is spent with it. Cleared here
                // rather than with the row, because a cut that wrote no row
                // would otherwise mark the next reply.
                self.interrupted = false;
                // **The caller's held words are NOT spent here.** A pause is
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
                    // Loud, because it costs the caller a reply and nothing
                    // else records it. Twenty-one of these in one call is what
                    // a caller hears as the agent never answering.
                    log!("[Voice] A talker turn ended with nothing said");
                } else if heard {
                    // The caller was answered, so nothing is owed. Said again
                    // here as well as on the first delta, because a transcript
                    // that lands late re-arms the bound after the reply.
                    //
                    // Not for a turn nobody heard. They are still owed, and
                    // the bound is what sends them to the doer instead.
                    self.answer_is_no_longer_owed();
                }
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
                // The provider saw the cut itself, so the rest of this reply is
                // over here too. Realtime reports one, and a Live talker never
                // does: on that protocol the barge-in control is the only word
                // we get.
                //
                // It never walks a cut BACK, because the row it names may
                // already be down. Realtime reports the cut beside the turn
                // end, and the caller's own words can land between the two.
                self.reply_was_cut_off = self.reply_was_cut_off.and_cut();
                // The caller talked over the goodbye, so they were not done.
                // Their intent is the only thing that ends a call (ADR 0170),
                // and taking the floor back is them saying otherwise.
                self.the_caller_was_not_done();
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

    /// The caller took the floor back, so the talker's reply is over.
    ///
    /// **Once per reply.** The gate opens on a run of loud frames, so a caller
    /// talking through a reply raises several edges. The second cancel says
    /// nothing the first did not, and on a provider that refuses one for a
    /// reply already cancelled it costs the call.
    ///
    /// Nothing at all while the talker is quiet. There is no reply to cut, and
    /// the caller speaking on their own floor is an utterance rather than an
    /// interruption.
    async fn the_caller_cut_in(&mut self, session: &mut dyn VoiceSession) {
        if !self.talker_has_the_floor || self.reply_was_cut_off.is_cut() {
            return;
        }
        self.reply_was_cut_off = self.reply_was_cut_off.and_cut();
        self.interrupted = true;
        self.the_caller_was_not_done();
        if let Err(e) = session.cancel().await {
            log!("[Voice] Could not interrupt the talker: {}", e);
        }
    }

    /// The caller is still talking, so a goodbye in flight rings off nobody.
    ///
    /// Their intent is the only thing that ends a call (ADR 0170). A Live
    /// talker reports no interruption of its own, so without this the line
    /// closed over somebody mid-sentence on every Live call.
    fn the_caller_was_not_done(&mut self) {
        if self.hanging_up {
            log!("[Voice] The caller cut in over the goodbye, so the call stays up");
            self.hanging_up = false;
        }
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
        // Disarmed only where the ask was TAKEN, matching [`Call::answer`]. An
        // ignored duplicate starts nothing, so nobody is answering the caller
        // and the bound is still theirs. Disarming above this line left their
        // second question with no ask and no net under it.
        self.answer_is_no_longer_owed();
        // A Live ask composes no reason at all, so the line says which it was
        // rather than printing a blank. See `live.rs::delegation`.
        log!(
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

    /// What an ask means when the doer is already parked on a card.
    ///
    /// **A talker holding the answering tool is refused, exactly as before.** It
    /// can put the card back and hand a choice id over, which picks the option
    /// the caller actually named. That is better than anything here, so it is
    /// the path wherever it exists (ADR 0170).
    ///
    /// **A talker holding none settles a QUESTION card with the caller's own
    /// words.** The ask is that talker's whole signal, and it means the caller
    /// said something worth acting on. A question card is issued a choice for
    /// exactly that, and picking it sends the transcript verbatim. So nothing
    /// here compares a spoken word against a label, and the doer reads what was
    /// said. Typing does the same: a typed reply on a thread with an open
    /// question is rerouted to its answer and never becomes a message.
    ///
    /// **Anything else is refused, and the note names the screen.** A permission
    /// card takes a decision rather than words, so there is nothing honest to
    /// send it.
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
            log!(
                "[Voice] Not delegating {:?}: this thread's doer is parked on a card",
                reason
            );
            self.acknowledge(session, tool_call_id, note).await;
            return;
        };
        log!(
            "[Voice] The ask settles what is waiting, in the caller's own words: {}",
            choice
        );
        // Everything an answer needs comes with this call. A transcript still
        // in flight is held and settled on the utterance it was made for. The
        // words are spent, so no later ask runs on them.
        //
        // A held one acknowledges NOTHING yet, deliberately: see the
        // `NeedsTheirWords` arm. The tool call is answered when it settles, or
        // dropped with a note on the retry that still had no words.
        self.answer(session, tool_call_id.to_string(), choice.to_string(), false)
            .await;
    }

    /// Hold an ask with nothing to run on yet, and start its clock.
    ///
    /// The one writer of [`Call::pending_delegation`], so an ask can never be
    /// held with no bound under it. See [`Call::ask_owed_by`].
    fn hold_the_ask(&mut self, reason: String) {
        self.pending_delegation = Some(reason);
        self.ask_owed_by = Some(tokio::time::Instant::now() + CALLER_WAITED_LONG_ENOUGH);
    }

    /// Take the held ask and stop its clock. `None` when there was none.
    fn take_the_ask(&mut self) -> Option<String> {
        self.ask_owed_by = None;
        self.pending_delegation.take()
    }

    /// Give up on an ask whose words never came, and say so out loud.
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
        log!(
            "[Voice] An ask never got the caller's words, so nothing ran: {}",
            stale
        );
        self.say(session, ASK_NEVER_GOT_ITS_WORDS.to_string()).await;
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
        // Disarmed only where something SETTLED. A refusal and a held retry
        // both leave the caller with nothing, so the bound is still theirs.
        if matches!(
            outcome,
            Resolution::Settled | Resolution::SettledWithTheirWords
        ) {
            self.answer_is_no_longer_owed();
        }
        match outcome {
            Resolution::Settled => self.acknowledge(session, &tool_call_id, ANSWER_TAKEN).await,
            Resolution::SettledWithTheirWords => {
                // Spent, so no later ask runs the doer on a question the
                // caller already settled with a card. Their own row went down
                // when they said it, and stays.
                self.undelivered_words = None;
                // An ask still waiting for those same words is now waiting for
                // nothing. Left sticky it pairs with a LATER utterance. That
                // wakes the doer on words asking for something else, under a
                // reason taken from the sentence just spent.
                //
                // Its "Taken." went out when it was made, and a tool call is
                // answered once. So the correction is appended instead, which
                // is what lets the talker tell the caller.
                if let Some(stale) = self.take_the_ask() {
                    log!("[Voice] The answer spent the words an ask was waiting for");
                    // The reason is appended only when there IS one. A Live ask
                    // carries none, and a heading with nothing under it reads as
                    // a request whose text went missing.
                    let note = match stale.trim() {
                        "" => ASK_LOST_ITS_WORDS.to_string(),
                        why => format!("{}\n\n{}", ASK_LOST_ITS_WORDS, why),
                    };
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
        let framing = decision_to_ask(&decision, self.provider.holds_the_answer_tool());
        self.say(session, framing).await;
    }

    /// Run the doer on words the caller has said, under a held ask.
    ///
    /// Called from BOTH sides, so the order the two frames arrive in decides
    /// nothing. The transcript comes from a different model than the tool
    /// call, and on a short fast reply the call really does land first.
    ///
    /// Does nothing until it has both. The words already have their own row,
    /// written when the caller stopped speaking. So nothing here is their only
    /// record, and an ask that never pairs loses none of them.
    ///
    /// **`WorkDelegated` is what starts the turn** (ADR 0201). The caller's
    /// words start none: the talker decides whether the doer is wanted, and a
    /// row written before that decision could not know. So the wake anchors on
    /// the delegation, and the doer reads the words from the thread.
    ///
    /// **A doer that refuses is handled here, not ignored.** The words go back
    /// on the undelivered pile, so a later ask can still run on them.
    async fn settle_the_delegation(&mut self, session: &mut dyn VoiceSession) {
        if self.undelivered_words.is_none() || self.pending_delegation.is_none() {
            return;
        }
        let transcript = self.undelivered_words.take().unwrap_or_default();
        let reason = self.take_the_ask().unwrap_or_default();
        // The words are spent on the wake, so the partials behind them are too.
        self.heard_so_far.clear();
        // A turn is starting on them, which is the answer the caller waited on.
        self.answer_is_no_longer_owed();

        // The wake first, so the doer's history already carries the reason by
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
        // No row, no anchor. A turn anchored on nothing is one the transcript
        // cannot place, so the caller is told instead of left waiting.
        let Some(delegation) = delegation else {
            log!("[Voice] The delegation was not recorded, so no turn was started");
            self.undelivered_words = Some(transcript);
            self.say(session, NOT_TAKEN.to_string()).await;
            return;
        };
        let taken = self
            .doer
            .wake(
                self.subject.thread_id,
                delegation,
                &transcript,
                self.subject.actor.clone(),
            )
            .await;
        if !taken {
            self.undelivered_words = Some(transcript);
            self.say(session, NOT_TAKEN.to_string()).await;
        }
    }

    /// Close the caller's unfinished partial, since no better text is coming.
    ///
    /// A partial normally gets no row: the provider's own final text for it is
    /// still on its way, and writing both would draw the sentence twice. When
    /// the call is over, nothing is coming, so the partial IS what they said.
    /// A provider with no turn-end frame reports the caller this way alone.
    async fn close_the_unfinished_partial(&mut self) {
        let partial = std::mem::take(&mut self.heard_so_far);
        if partial.trim().is_empty() {
            return;
        }
        self.write_a_spoken_row(partial.clone()).await;
        self.caller_said_more(&partial);
    }

    /// Put one caller row on the thread, unless there is nothing to put.
    ///
    /// `SpokenMessageReceived`, which is `Metadata` and starts nothing. What
    /// starts a doer turn is the talker's own `WorkDelegated` (ADR 0201), so
    /// this row never has to know what the talker will do next.
    ///
    /// The caller's own actor, not the talker's. Whoever handles it, the
    /// caller said it.
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

    /// Everything the caller has said that the doer has not been given.
    ///
    /// Every word of it is on the thread already, so taking it here spends a
    /// copy rather than a record. The unclosed partial is written first, being
    /// the one part that had no row yet.
    async fn everything_the_caller_said(&mut self) -> String {
        self.close_the_unfinished_partial().await;
        self.undelivered_words.take().unwrap_or_default()
    }

    /// Add finished words to what the doer has not been given yet.
    ///
    /// Joined by the rule the doer's own history uses. So the sentence it is
    /// RUN on and the sentence it reads back are the same one. A plain space
    /// between every piece made `Status` and `, please` into `Status , please`
    /// on one side and `Status, please` on the other.
    fn caller_said_more(&mut self, text: &str) {
        let said = self.undelivered_words.take().unwrap_or_default();
        let joined = join_spoken(&said, text);
        if !joined.is_empty() {
            self.undelivered_words = Some(joined);
        }
    }

    /// Take a delta of the talker's speech.
    ///
    /// **A worded delta is already an answer**, before the turn ends. A caller
    /// who cuts into one has been answered as far as it got. Their earlier
    /// words are the talker's business, not the doer's.
    fn talker_said(&mut self, delta: &str) {
        self.spoken_so_far.push(delta);
        if !delta.trim().is_empty() {
            self.talker_answered_them = true;
        }
    }

    /// This call is a conversation now, so the talker may be heard (ADR 0211).
    ///
    /// **Three openers, and each hands back a whole budget.** The caller's own
    /// words, worded only, because a blank delta is the provider's silence
    /// rather than theirs. The provider hearing them start, which is the one
    /// opener no transcriber can withhold. And the engine asking the talker to
    /// speak, which is how a card parked on a silent caller is put to them.
    ///
    /// The reset is the point. A caller who keeps talking keeps buying answers,
    /// so a working call never meets [`TURNS_ONE_OPENER_BUYS`] at all.
    fn open_the_floor(&mut self) {
        self.floor = Floor::Open {
            turns_left: TURNS_ONE_OPENER_BUYS,
        };
    }

    /// The talker has been heard for one turn, so that turn is off the budget.
    ///
    /// Spending the last one shuts the floor, and only an opener reopens it. So
    /// the caller hears an answer of any length and never a monologue: the
    /// talker keeps speaking, and nothing it says past the bound is played or
    /// written down (ADR 0213).
    ///
    /// Called at a turn END, after the row, so the turn that spends the last
    /// one is still heard in full. What the shut floor decides is the NEXT turn.
    fn the_talker_spent_a_turn(&mut self) {
        let Floor::Open { turns_left } = self.floor else {
            return;
        };
        // Saturating, so a budget of nought can never wrap into a floor held
        // for 255 more turns. `Open` carries at least one today, and the arm
        // below is what keeps that true rather than an assumption about it.
        self.floor = match turns_left.saturating_sub(1) {
            0 => {
                // Loud, because the caller hears the talker carry on with its
                // mouth shut and has nothing else to point at.
                log!(
                    "[Voice] The talker has had {} turns on one opener, so it is \
                     not heard again until the caller speaks",
                    TURNS_ONE_OPENER_BUYS
                );
                Floor::Shut
            }
            left => Floor::Open { turns_left: left },
        };
    }

    /// Whether what the talker is producing right now reaches the caller.
    fn the_caller_hears_this(&self) -> bool {
        match self.audience {
            Audience::Nobody => false,
            Audience::TheCaller => true,
            Audience::Undecided => self.floor.is_open(),
        }
    }

    /// Settle who this turn is for, on the first word of it.
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

    /// The caller said something, so somebody owes them an answer from now.
    fn caller_is_owed_an_answer(&mut self) {
        self.answer_owed_by = Some(tokio::time::Instant::now() + CALLER_WAITED_LONG_ENOUGH);
    }

    /// Something is answering the caller, so the wait is over.
    fn answer_is_no_longer_owed(&mut self) {
        self.answer_owed_by = None;
    }

    /// Nobody answered the caller, so the doer is woken with what they said.
    ///
    /// **The talker is the fast path to an answer, never the only one.** It
    /// decides whether to answer itself or ask for the doer. A talker that
    /// decides neither leaves the caller in silence until they hang up. That is
    /// the one outcome a call may not have, so the doer takes the question
    /// here instead.
    ///
    /// Loud in three places, because a silent recovery hides a broken talker.
    /// The log says it happened, the `WorkDelegated` row says why, and the
    /// caller is told on screen.
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
        // Taken above the seam, so the provider must stop holding its own copy.
        // A turn-less one accumulates until something asks, and would hand the
        // same sentence over again at its next boundary or at close. That is
        // one breath and two rows, which is the whole defect.
        session.caller_words_were_taken().await;
        // Refused for the same reason `delegated` refuses: the doer is blocked
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
            log!(
                "[Voice] Nothing answered {:?}, and the doer is parked on a card",
                said
            );
            self.undelivered_words = Some(said);
            self.tell_the_caller(transport).await;
            return;
        }
        log!(
            "[Voice] Nothing answered {:?} in {}s, so the doer takes it",
            said,
            CALLER_WAITED_LONG_ENOUGH.as_secs()
        );
        self.undelivered_words = Some(said);
        // Never over a sticky ask. The talker's own reason outlives the turn
        // that made it, exactly so a transcript still in flight can pair with
        // it. Clobbering it here would spend the caller's words under the wrong
        // reason and leave that ask to claim a later, unrelated sentence.
        if self.pending_delegation.is_none() {
            self.hold_the_ask(NOBODY_ANSWERED.to_string());
        }
        self.settle_the_delegation(session).await;
        self.tell_the_caller(transport).await;
    }

    /// Say on screen that the voice is not answering, once per call.
    ///
    /// Once, because a second card says nothing the first did not. Dropped on
    /// failure: a caller who has already gone cannot read it, and the row is on
    /// the thread either way.
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

    /// Fold what the session was still holding into what is about to be
    /// written.
    ///
    /// Only the two accounts a row is built from are read. The loop has already
    /// stopped, so a tool call or a floor change arriving here has nothing left
    /// to act on it.
    ///
    /// A caller turn held here never reached the loop, so it has no row yet and
    /// gets one now. Its own words are final, which is what the row says.
    async fn take_what_was_still_held(&mut self, held: Vec<VoiceEvent>) {
        for event in held {
            match event {
                VoiceEvent::UserTurnEnded { transcript } => {
                    self.write_a_spoken_row(transcript.clone()).await;
                    self.caller_said_more(&transcript);
                    self.heard_so_far.clear();
                }
                VoiceEvent::UserTranscript { text } => self.heard_so_far.push_str(&text),
                // Dropped when the caller cut this reply off, exactly as the
                // live ones were. Nothing is coming after the socket, but
                // these words still reached nobody's ear.
                VoiceEvent::TalkerTranscript { text } if !self.reply_was_cut_off.is_cut() => {
                    self.talker_said(&text)
                }
                // Kept as the FALLBACK, never folded into the deltas. Those
                // already carry this turn, so replacing them here would drop
                // what the caller actually heard. A reply already written is
                // read for nothing: the drained end covers the whole of it,
                // tail included, and the caller stopped listening part-way.
                VoiceEvent::TalkerTurnEnded { transcript, usage } => {
                    // The spend is recorded wherever the turn end arrives, and
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

    /// Write down the reply the caller was hearing when the call stopped.
    ///
    /// Nothing else writes it: the turn's own end never came, because the
    /// caller rang off in the middle of it.
    ///
    /// **Interrupted only if the talker still held the floor.** That is the
    /// state where the caller left mid-reply. A reply whose pause had already
    /// landed finished, so marking it cut off would be a claim about the
    /// caller that is usually false.
    async fn write_down_whatever_was_said(&mut self) {
        self.interrupted |= self.talker_has_the_floor;
        self.write_down_the_reply().await;
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
        // Nothing to write down here. Both sides record at their own turn
        // ends, and the floor check above means no talker turn is in flight.
        //
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
            return;
        }
        // Only once it was taken, and never rolled back: a floor the CALLER
        // opened is not the engine's to shut again (ADR 0211). It is a whole
        // fresh budget either way, so an answer handed over after a monologue
        // spent the last one is still heard (ADR 0213).
        self.open_the_floor();
        // A run the engine handed over is a new one, whatever the talker was
        // half way through saying to nobody. Left held, the mute would eat the
        // one answer we asked for.
        self.audience = Audience::Undecided;
        self.mute_held_for = 0;
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

    /// Write down one thing the talker said, and it is the only writer.
    ///
    /// **A turn, not a whole reply.** Everything it said before the provider
    /// ended the turn, written the moment that end lands. So `created` is when
    /// the words stopped, which is what lets the transcript read by the clock
    /// alone (ADR 0201).
    ///
    /// The row also says how long the talker had been speaking, because those
    /// words began before every step they were said over. The transcript reads
    /// the row there (ADR 0206).
    ///
    /// The DELTAS are the row, because they carry their own spacing. The
    /// turn's own transcript stands in when none arrived. See
    /// [`Call::spoken_so_far`].
    ///
    /// Attributed to the talker, so `history.rs` gives it its own speaker label
    /// and the doer never reads it as its own prior turn (ADR 0150).
    ///
    /// A reply with no words is not written. A cancelled response can end
    /// before the talker said anything, and an empty row would claim the caller
    /// heard something they did not.
    async fn write_down_the_reply(&mut self) {
        // The turn is accounted for now, however it ended. Forgotten here,
        // so nothing downstream writes the same words a second time.
        let (streamed, age) = self.spoken_so_far.take();
        let reported = std::mem::take(&mut self.last_turn_transcript);
        // A turn nobody heard leaves no row (ADR 0211). Taken first, so a
        // babble the hangup caught mid-sentence is forgotten rather than
        // written by the flush that follows the socket.
        if !self.the_caller_hears_this() {
            return;
        }
        // The age belongs to the DELTAS, so the fallback transcript carries
        // none. Nothing timed those words, and a row with no age reads at its
        // own `created`, which is where every reply read before ADR 0206.
        let (transcript, spoken_secs_before) = if streamed.trim().is_empty() {
            (reported, None)
        } else {
            (streamed.trim().to_string(), age)
        };
        if transcript.trim().is_empty() {
            return;
        }
        // READ, never taken. The flag belongs to the REPLY, and a cut one is
        // written mid-turn by the caller's own turn end: taking it there left
        // the row right and the turn that follows saying nobody cut in.
        // Taking it here instead leaked it onto the next reply, which a cut
        // that wrote nothing never cleared. The turn end owns it, below.
        let interrupted = self.interrupted;
        // A cut reply now HAS its row, so that turn end reports no second one.
        // A cut that wrote nothing is still owed whatever it reports.
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

/// Put one row on the thread, and answer with the id it was given.
///
/// The id is what a turn anchors on: `WorkDelegated` is the starter of a
/// delegated call (ADR 0201), so its caller needs the row back. `None` means
/// the write failed and has been logged, and no turn may anchor on nothing.
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
            // No channel on any of these. Voice is a mode of a chat thread
            // (ADR 0148), and stamping one here is how a fourth `EventChannel`
            // starts.
            meta,
        })
        .await
    {
        Ok(result) => result.map(|r| r.event_id),
        Err(e) => {
            log!("[EventBus] {} emit failed: {}", ctx, e);
            None
        }
    }
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
