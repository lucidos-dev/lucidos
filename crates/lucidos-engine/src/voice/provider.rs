//! The voice-session seam: what a talker must do, and nothing about who it is.
//!
//! A `VoiceProvider` opens a [`VoiceSession`]. The session hears audio, speaks
//! audio, and yields a stream of [`VoiceEvent`]. Everything above this file
//! talks to those two traits. So swapping `Realtime` for `Cascaded` changes no
//! socket payload and no event shape (ADR 0149).
//!
//! **There is still no tool field here, on purpose.** The talker's three tools
//! are named by `voice/mod.rs` rather than passed in. A list nobody can append
//! to beats a list every caller must remember not to grow. ADR 0149 made the
//! talker tool-less, and ADR 0170 keeps its guarantee while widening the set:
//! none of the three mutates anything.

use async_trait::async_trait;

use super::language::SpokenLanguage;
use crate::engine::ApiUsage;

type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// The audio both directions carry.
///
/// One format, because the client is dumb (parent plan, decision 3) and a
/// negotiation it cannot influence is a negotiation worth not having. 24 kHz
/// mono PCM16 is what the realtime APIs speak natively, so nothing resamples.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AudioFormat {
    pub sample_rate_hz: u32,
    pub channels: u8,
}

impl Default for AudioFormat {
    fn default() -> Self {
        Self {
            sample_rate_hz: 24_000,
            channels: 1,
        }
    }
}

/// Everything a session is opened with.
///
/// `instructions` is the stable half: the persona and the honesty constraint
/// from ADR 0149. It is the cached prefix, so nothing per-session belongs in it.
///
/// `resident_block` is the dynamic half: what this session can answer with no
/// wait. It enters as the FIRST history item rather than as instructions. That
/// way a refresh appends beside it instead of rewriting it (parent plan,
/// decision 16).
///
/// `language` is what the caller is expected to speak, and `instructions`
/// already carries its name. The field is here for the half a sentence cannot
/// express: a transcriber is configured with a code, not asked in prose.
#[derive(Debug, Clone)]
pub struct SessionOpening {
    pub instructions: String,
    pub resident_block: String,
    /// The provider's name for a voice. Opaque to everything above the seam.
    pub voice: String,
    /// The provider's name for the model turning caller audio into text. Opaque
    /// in the same way, and for the same reason: a cascaded talker would name
    /// its transcriber differently, or hold none at all.
    pub transcriber: String,
    pub audio: AudioFormat,
    /// `None` leaves the transcriber to guess, which is what it did before
    /// anything here named a language.
    pub language: Option<SpokenLanguage>,
}

/// What a talker produced.
///
/// Named for what happened, not for the frame that carried it, so an
/// implementation can coalesce or split provider frames freely.
#[derive(Debug, Clone, PartialEq)]
pub enum VoiceEvent {
    /// Talker audio to play. Forwarded to the socket and never written down
    /// (parent plan, decision 12).
    Audio(Vec<u8>),
    /// The provider decided the caller stopped talking. Semantic or
    /// audio-native, never a silence timer we wrote (parent plan, decision 11).
    UserTurnEnded { transcript: String },
    /// A piece of what the talker is saying, as it says it.
    TalkerTranscript { text: String },
    /// The talker called `delegate`: this needs the doer.
    ///
    /// It arrives DURING the talker's turn, before [`Self::TalkerTurnEnded`].
    /// That is what makes delegation free: waiting for the turn to end would
    /// cost a full spoken reply of latency on every real question.
    ///
    /// The talker is never told whether a turn is already running, so this
    /// means "the caller needs the doer" and never "wake it". Deciding between
    /// starting a turn and joining one is the engine's business.
    DelegationRequested {
        /// The provider's handle for this tool call, opaque above the seam. It
        /// goes back on [`VoiceSession::resolve_tool_call`] and nowhere else.
        tool_call_id: String,
        /// The talker's own few words on what the caller wants.
        reason: String,
    },
    /// The talker called `answer`: the caller picked one of the choices.
    ///
    /// Arrives during the turn, like a delegation, and for the same reason.
    AnswerRequested {
        tool_call_id: String,
        /// A choice id the ENGINE issued, handed back exactly as it was given.
        /// Nothing here checks it: the engine looks it up against what is
        /// still open, and refuses one it did not issue.
        choice_id: String,
    },
    /// The talker called `hang_up`: the caller said the conversation is over.
    ///
    /// It ends the CALL and never the work. A doer turn in flight keeps
    /// running and the thread carries on, exactly as when the caller rings off
    /// on the button.
    HangupRequested { tool_call_id: String },
    /// The talker finished a reply, and reported what it spent.
    TalkerTurnEnded { transcript: String, usage: ApiUsage },
    /// The talker stopped mid-utterance because the caller spoke over it.
    Interrupted,
    /// The session cannot continue. The runtime ends the call and says why.
    Failed { message: String },
}

/// Opens sessions. One implementation per way of holding a conversation.
#[async_trait]
pub trait VoiceProvider: Send + Sync {
    /// Which implementation this is, for the log line and nothing else. Never
    /// reaches a client (parent plan, decision 3).
    fn name(&self) -> &'static str;

    /// The model id this provider talks to, for the usage row. Same rule: it is
    /// recorded, never served to a socket.
    fn model(&self) -> &str;

    async fn open(&self, opening: SessionOpening) -> Result<Box<dyn VoiceSession>, BoxError>;
}

/// One live conversation.
#[async_trait]
pub trait VoiceSession: Send {
    /// Send a chunk of caller audio upstream.
    async fn push_audio(&mut self, pcm: &[u8]) -> Result<(), BoxError>;

    /// Append one item to the session's history.
    ///
    /// **Append-only.** There is no edit and no delete, because deleting one
    /// item invalidates the cached prefix behind it. The measurement is in
    /// `docs/notes/2026-06-01-voice-control.md`: one deletion tripled
    /// full-price input for that turn.
    async fn append_context(&mut self, note: &str) -> Result<(), BoxError>;

    /// Append one item AND ask the talker to answer it out loud.
    ///
    /// The other half of [`Self::append_context`], and the difference is the
    /// whole point: appending is silent, so an answer appended alone reaches
    /// the caller's ear never. This is what the doer's answer travels on.
    ///
    /// Append-only too. It adds an item exactly as the silent one does, and
    /// asks for a reply on top.
    ///
    /// **The caller decides when.** A talker mid-sentence must not be asked for
    /// a second reply, and this makes no such check: `call.rs` owns the floor.
    async fn speak(&mut self, note: &str) -> Result<(), BoxError>;

    /// Tell the talker one of its tool calls landed.
    ///
    /// One member for every tool, rather than a near-identical one each. An
    /// unresolved call leaves a dangling item in the session's history, and
    /// the talker reads that as work it never heard back about. So this is
    /// owed even when the call went nowhere, and `note` is what says so.
    ///
    /// It asks for no reply. The talker already spoke in the turn that made
    /// the call, and anything that comes back arrives later on [`Self::speak`].
    async fn resolve_tool_call(&mut self, tool_call_id: &str, note: &str) -> Result<(), BoxError>;

    /// The next thing the talker produced, or `None` once the session is over.
    async fn next(&mut self) -> Option<VoiceEvent>;

    /// Stop the talker mid-utterance, because the caller spoke over it.
    async fn cancel(&mut self) -> Result<(), BoxError>;

    /// Close the session down. Cleanup only: usage was already reported per
    /// reply on [`VoiceEvent::TalkerTurnEnded`], so nothing is owed here.
    async fn close(&mut self);
}
