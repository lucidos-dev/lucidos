//! `Live`: the talker as OpenAI's GPT-Live model, on its own socket.
//!
//! The second implementation behind the seam, and the first that proves the
//! seam was worth having. Nothing above `provider.rs` changes for it. The two
//! decisions below are ADR 0181, and the plan is
//! `docs/plans/2026-09-10-a-live-talker-has-no-turns-and-no-tools.md`.
//!
//! **It is not a Realtime model.** Its own model page supports one endpoint,
//! `v1/live/sessions`, and marks `v1/realtime` unsupported. Every frame
//! differs: the opening one, both audio ones, both transcript ones, and the
//! way it asks for help.
//!
//! **It holds no tools.** Under client delegation the API declares none, so
//! `delegate` arrives as `session.delegation.created` and the other two have no
//! expression at all. On a Live call the caller settles a card by tapping it and
//! rings off on the button. ADR 0170's three tools scope to a Realtime talker.
//!
//! **It has no turns either.** There is no user-turn-end frame and no
//! output-done frame, which the provider's own guide says outright. The seam
//! needs both, so this module synthesizes them and nothing above it can tell.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use base64::Engine as _;
use futures::stream::{SplitSink, SplitStream};
use futures::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio::time::Instant;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::{Error as WsError, Message};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

use super::provider::{
    drain_held, wait_until, SessionOpening, VoiceEvent, VoiceProvider, VoiceSession,
};
use crate::engine::ApiUsage;

type BoxError = Box<dyn std::error::Error + Send + Sync>;
type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;
type Reader = SplitStream<Socket>;
type Writer = SplitSink<Socket, Message>;

/// Where a Live talker lives. No query string: the model rides the opening
/// frame instead, which is the first thing this protocol does differently.
const LIVE_URL: &str = "wss://api.openai.com/v1/live/sessions";

/// How many talker events may queue before the reader waits. Same bound and
/// same reason as the Realtime provider's.
const EVENT_QUEUE: usize = 64;

/// How long the opening handshake may take before the call gives up.
///
/// The provider requires `session.started` before anything else is sent. So a
/// socket that connects and then says nothing would hang the caller forever.
const OPENING_TIMEOUT: Duration = Duration::from_secs(10);

/// How long the talker's own WORDS may go quiet before its turn is over.
///
/// **Words, never the output stream carrying them.** The stream runs when
/// nothing is being said: audio arrives with silence in it, and a transcript
/// delta arrives blank. Read off the stream, this bound never expired at all,
/// so one call's two replies landed as a single row at hangup. See
/// `docs/plans/2026-09-15-a-talker-turn-ends-when-its-words-do.md`.
///
/// **This is not the forbidden silence timer.** That rule is about the CALLER:
/// a timer cannot tell a person's pause from their full stop, so it must never
/// decide one. This measures our own received words instead, which is a
/// mechanical fact rather than a judgment about anybody.
const TALKER_IDLE: Duration = Duration::from_millis(700);

/// The most characters one append may carry.
///
/// The provider caps `content` at 500 tokens per append. Characters are what we
/// can count, so this is deliberately conservative at roughly three per token.
/// Going over is refused, and a refused answer is one the caller never hears.
const APPEND_CHARS: usize = 1_200;

/// Result to speak. The talker paraphrases it rather than reading it out, which
/// is exactly what the seam's `speak` promises.
const COMMENTARY: &str = "session.commentary.append";

/// Quiet state. It reaches the talker and is not spoken on arrival, which is
/// what the seam's `append_context` promises.
const THINKING: &str = "session.thinking.append";

/// Session-level steering, the one append the provider documents with no
/// delegation behind it.
const INSTRUCTIONS: &str = "session.instructions.append";

/// The provider's terminal frame. It carries the session's final usage, so it
/// arrives while the socket is still open and the reader has to act on it.
const SESSION_CLOSED: &str = "session.closed";

/// A talker reached over OpenAI's Live API.
pub struct LiveProvider {
    api_key: String,
    model: String,
}

impl LiveProvider {
    pub fn new(api_key: String, model: String) -> Self {
        Self { api_key, model }
    }
}

#[async_trait]
impl VoiceProvider for LiveProvider {
    fn name(&self) -> &'static str {
        "live"
    }

    fn model(&self) -> &str {
        &self.model
    }

    async fn open(&self, opening: SessionOpening) -> Result<Box<dyn VoiceSession>, BoxError> {
        let mut request = LIVE_URL.into_client_request()?;
        request
            .headers_mut()
            .insert("Authorization", format!("Bearer {}", self.api_key).parse()?);

        let (stream, _) = tokio_tungstenite::connect_async(request).await?;
        let (mut writer, mut reader) = stream.split();

        writer
            .send(Message::Text(
                session_start(&self.model, &opening).to_string(),
            ))
            .await?;
        await_session_started(&mut reader).await?;

        // The resident block is the dynamic half, and it lands beside the
        // persona rather than inside it (the parent plan's decision 16).
        //
        // A workspace can turn every section off, and then there is nothing to
        // append. An empty one would be a frame the provider has to refuse.
        for (index, chunk) in appendable(&opening.resident_block).iter().enumerate() {
            let id = format!("resident-{}", index);
            writer
                .send(Message::Text(
                    append_frame(INSTRUCTIONS, &id, None, chunk).to_string(),
                ))
                .await?;
        }

        let (tx, rx) = mpsc::channel(EVENT_QUEUE);
        let caller_words = Arc::new(Mutex::new(String::new()));
        let held = Arc::clone(&caller_words);
        tokio::spawn(async move {
            let mut turn = TurnState {
                caller_words: held,
                ..TurnState::default()
            };
            loop {
                let quiet_at = turn.last_words.map(|at| at + TALKER_IDLE);
                let (events, over) = tokio::select! {
                    message = reader.next() => read_frame(message, &mut turn),
                    _ = wait_until(quiet_at) => (talker_went_quiet(&mut turn), false),
                };
                for event in events {
                    if tx.send(event).await.is_err() {
                        return;
                    }
                }
                if over {
                    break;
                }
            }
        });

        Ok(Box::new(LiveSession {
            writer,
            rx,
            sent: 0,
            caller_words,
        }))
    }
}

struct LiveSession {
    writer: Writer,
    rx: mpsc::Receiver<VoiceEvent>,
    /// The reader's own copy of what the caller has said, so the engine can say
    /// it took those words. See [`TurnState::caller_words`].
    caller_words: Arc<Mutex<String>>,
    /// How many frames this session has sent, which is what numbers the next
    /// one. See [`LiveSession::next_event_id`].
    sent: u64,
}

impl LiveSession {
    /// A name for the next frame, so a refusal can say which one it refused.
    ///
    /// The provider echoes it back as `client_event_id`, and [`error_events`]
    /// reads that to tell a refused command from a dead session. Without one,
    /// every refusal looks like the session ending.
    fn next_event_id(&mut self) -> String {
        self.sent += 1;
        format!("lucidos-{}", self.sent)
    }

    /// Send one frame as JSON text.
    async fn send(&mut self, frame: Value) -> Result<(), BoxError> {
        self.writer.send(Message::Text(frame.to_string())).await?;
        Ok(())
    }

    /// Send `content` as one or more appends of the same kind.
    ///
    /// Chunked rather than cut. Repeated appends extend the same delegation, so
    /// a long answer arrives whole. Cutting it would drop the half the caller
    /// asked for, silently.
    async fn append(
        &mut self,
        kind: &str,
        delegation_id: Option<&str>,
        content: &str,
    ) -> Result<(), BoxError> {
        for chunk in appendable(content) {
            let id = self.next_event_id();
            self.send(append_frame(kind, &id, delegation_id, &chunk))
                .await?;
        }
        Ok(())
    }
}

/// One frame off the socket, as events and whether the session is over.
///
/// Three ways it is over, and each owes whatever the turn still holds. The
/// provider says so with its terminal frame, the socket says so by failing,
/// and it says so again by going. A session ending with words held would lose
/// them from the thread for good.
fn read_frame(
    message: Option<Result<Message, WsError>>,
    turn: &mut TurnState,
) -> (Vec<VoiceEvent>, bool) {
    match message {
        Some(Ok(Message::Text(text))) => {
            let Ok(value) = serde_json::from_str::<Value>(&text) else {
                log!("[Voice] The talker sent something that is not JSON");
                return (vec![], false);
            };
            if value.get("type").and_then(Value::as_str) == Some(SESSION_CLOSED) {
                return (closing_events(turn), true);
            }
            (map_event(&value, turn, Instant::now()), false)
        }
        Some(Ok(_)) => (vec![], false),
        Some(Err(e)) => {
            log!("[Voice] The Live socket failed: {}", e);
            (closing_events(turn), true)
        }
        None => (closing_events(turn), true),
    }
}

#[async_trait]
impl VoiceSession for LiveSession {
    async fn push_audio(&mut self, pcm: &[u8]) -> Result<(), BoxError> {
        let audio = base64::engine::general_purpose::STANDARD.encode(pcm);
        self.send(json!({ "type": "session.input_audio.append", "audio": audio }))
            .await
    }

    async fn append_context(&mut self, note: &str) -> Result<(), BoxError> {
        self.append(THINKING, None, note).await
    }

    async fn speak(&mut self, note: &str) -> Result<(), BoxError> {
        // No second frame, unlike the Realtime provider. Commentary IS the ask
        // to say something, and the talker picks the moment.
        self.append(COMMENTARY, None, note).await
    }

    async fn resolve_tool_call(&mut self, tool_call_id: &str, note: &str) -> Result<(), BoxError> {
        // The one append that names its delegation, and the only place the id
        // matters: this is the answer to that particular ask.
        self.append(THINKING, Some(tool_call_id), note).await
    }

    async fn next(&mut self) -> Option<VoiceEvent> {
        self.rx.recv().await
    }

    /// Nothing to send, and that is the correct behaviour rather than a gap.
    ///
    /// A Live talker listens while it speaks, so it hears the caller cut in and
    /// stops on its own. The API documents no cancel, and inventing one would
    /// mean muting the caller or closing the line.
    async fn cancel(&mut self) -> Result<(), BoxError> {
        Ok(())
    }

    /// Drop the reader's copy, because the engine wrote those words down.
    async fn caller_words_were_taken(&mut self) {
        self.caller_words.lock().expect("caller words").clear();
    }

    /// Close the line, then drain what the reader was still holding.
    ///
    /// The drain is the whole reason this returns anything. `caller_words`
    /// lives in the reader task and is handed over by `closing_events`, which
    /// runs only once the socket goes. Nothing was reading the channel by then,
    /// so the caller's last sentence died with the session.
    ///
    /// It terminates on the reader task's own exit: that task breaks on a
    /// closed socket, which drops the sender. The bound is a backstop for a
    /// server that accepts the close and then says nothing.
    async fn close(&mut self) -> Vec<VoiceEvent> {
        let _ = self.send(json!({ "type": "session.close" })).await;
        let _ = self.writer.close().await;
        drain_held(&mut self.rx).await
    }
}

/// The opening payload: which model, who it is, what audio it speaks.
///
/// `delegation.type` is `client`, never `responses`. The managed mode would
/// rent a second brain that holds tools and acts, and ADR 0149 puts every
/// action behind our own doer.
pub fn session_start(model: &str, opening: &SessionOpening) -> Value {
    json!({
        "type": "session.start",
        "event_id": "session_start",
        "session": {
            "model": model,
            "instructions": opening.instructions,
            "audio": {
                "format": {
                    "type": "audio/pcm",
                    "rate": opening.audio.sample_rate_hz,
                },
                "output": { "voice": opening.voice },
            },
            "delegation": { "type": "client" },
        }
    })
}

/// One append, of whichever kind the caller named.
///
/// `delegation_id` is written even when it is absent, because the provider
/// requires the key on all three. `null` is what says this append belongs to
/// the session rather than to one ask.
///
/// `event_id` is what a refusal names, and every append carries one. See
/// [`error_events`] for what reading it back prevents.
pub fn append_frame(
    kind: &str,
    event_id: &str,
    delegation_id: Option<&str>,
    content: &str,
) -> Value {
    json!({
        "type": kind,
        "event_id": event_id,
        "delegation_id": delegation_id,
        "content": content,
    })
}

/// The appends `content` is worth sending as, which is none when it is blank.
///
/// The one place a note is allowed to reach nowhere, and only because there was
/// nothing in it. Every caller above holds real text, so this is the guard for
/// a resident block whose sections are all switched off.
pub fn appendable(content: &str) -> Vec<String> {
    if content.trim().is_empty() {
        return vec![];
    }
    chunks(content)
}

/// Split `content` into pieces the provider will accept.
///
/// Always at least one piece, so a caller never loses a note to an empty
/// return. Breaks on the last space in range where there is one, and on a char
/// boundary otherwise, so nothing is split mid-character.
///
/// A piece that trims to nothing is dropped rather than sent. A long run of
/// whitespace produces one, and an empty `content` is a frame the provider has
/// to refuse.
pub fn chunks(content: &str) -> Vec<String> {
    if content.chars().count() <= APPEND_CHARS {
        return vec![content.to_string()];
    }
    let mut out = Vec::new();
    let mut rest = content;
    while !rest.is_empty() {
        if rest.chars().count() <= APPEND_CHARS {
            out.push(rest.to_string());
            break;
        }
        let ceiling = byte_len_of_chars(rest, APPEND_CHARS);
        let cut = match rest[..ceiling].rfind(char::is_whitespace) {
            Some(space) if space > 0 => space,
            _ => ceiling,
        };
        let piece = rest[..cut].trim_end();
        if !piece.is_empty() {
            out.push(piece.to_string());
        }
        rest = rest[cut..].trim_start();
    }
    out
}

/// How many bytes the first `count` chars of `text` occupy.
fn byte_len_of_chars(text: &str, count: usize) -> usize {
    text.char_indices()
        .nth(count)
        .map(|(at, _)| at)
        .unwrap_or(text.len())
}

/// What the reader knows between frames.
///
/// Two accumulators and one clock reading. Together they are the whole of the
/// turn synthesis, and they live below the seam because they are one provider's
/// problem.
#[derive(Default)]
pub struct TurnState {
    /// What the caller has said since their words were last handed over.
    ///
    /// Shared with the session, which is the only way the ENGINE can say it
    /// took these words. It does that when nothing answered the caller and the
    /// doer was woken with them. Left here, the next boundary would hand the
    /// same sentence over again.
    caller_words: Arc<Mutex<String>>,
    /// What the talker has said in the turn it is speaking now.
    talker_words: String,
    /// When the talker last said a WORD. `None` between turns.
    ///
    /// Armed by [`map_event`]'s transcript arm alone, and only for a delta that
    /// carries words. Audio arms nothing: this provider streams it whether or
    /// not anybody is speaking, so a clock reading it never runs out. Nor does
    /// a blank transcript delta, for the same reason.
    ///
    /// So it is `Some` exactly while the talker owes the end of something it
    /// said, which is what makes [`talker_went_quiet`] one-shot and never
    /// empty.
    last_words: Option<Instant>,
}

/// Map one Live frame onto zero or more [`VoiceEvent`].
///
/// Zero for most of them. This protocol narrates a session, and the seam models
/// a conversation.
pub fn map_event(value: &Value, turn: &mut TurnState, now: Instant) -> Vec<VoiceEvent> {
    let Some(kind) = value.get("type").and_then(Value::as_str) else {
        return vec![];
    };
    match kind {
        // Forwarded and nothing else. Audio says nothing about whether the
        // talker is speaking, because this provider streams it either way. See
        // [`TurnState::last_words`].
        "session.output_audio.delta" => match decoded_audio(value) {
            Some(pcm) => vec![VoiceEvent::Audio(pcm)],
            None => vec![],
        },
        "session.output_transcript.delta" => {
            let Some(text) = value.get("delta").and_then(Value::as_str) else {
                return vec![];
            };
            // The talker composing words IS its judgment that the caller
            // finished, which is ADR 0181's rule. A blank delta composes none,
            // so it neither ends the caller's turn nor arms the talker's own.
            let mut events = if text.trim().is_empty() {
                vec![]
            } else {
                turn.last_words = Some(now);
                caller_finished(turn)
            };
            turn.talker_words.push_str(text);
            events.push(VoiceEvent::TalkerTranscript {
                text: text.to_string(),
            });
            events
        }
        // Forwarded AND held. The partial is what draws the caller's bubble as
        // they speak, and the accumulator is what a delegation hands over.
        "session.input_transcript.delta" => match value.get("delta").and_then(Value::as_str) {
            Some(text) if !text.is_empty() => {
                turn.caller_words
                    .lock()
                    .expect("caller words")
                    .push_str(text);
                vec![VoiceEvent::UserTranscript {
                    text: text.to_string(),
                }]
            }
            _ => vec![],
        },
        "session.delegation.created" => delegation(value, turn),
        "error" => error_events(value),
        _ => vec![],
    }
}

/// An `error` frame ends the call only when it is about the SESSION.
///
/// One frame carries two things here, and `client_event_id` tells them apart:
/// it names the client frame that was refused. Every APPEND carries an
/// `event_id`, so a refused note can always say which.
///
/// A refused append costs one note. Ending a working call over it is the worse
/// failure, and a session that really did die closes the socket, which the
/// reader already reports.
///
/// **Audio and the closing frame carry no id, so a refusal of either is read
/// as the session dying.** That is the right reading. A rejected audio frame
/// means the format is wrong, which no later frame recovers from, and a
/// refused close is the line going anyway.
fn error_events(value: &Value) -> Vec<VoiceEvent> {
    let message = error_message(value);
    let refused = value
        .pointer("/error/client_event_id")
        .or_else(|| value.pointer("/client_event_id"))
        .and_then(Value::as_str);
    match refused {
        Some(id) => {
            log!("[Voice] The talker refused {}: {}", id, message);
            vec![]
        }
        None => vec![VoiceEvent::Failed { message }],
    }
}

/// The talker asked for help, which is this protocol's whole tool surface.
///
/// The frame carries an id and no words, so the caller's own words are the
/// reason. That is honest: the talker composed nothing to explain itself with.
fn delegation(value: &Value, turn: &mut TurnState) -> Vec<VoiceEvent> {
    let Some(id) = value.pointer("/delegation/id").and_then(Value::as_str) else {
        log!("[Voice] A delegation arrived with no id, so it cannot be answered");
        return vec![];
    };
    // Asking for help is the talker's own judgment that the caller finished.
    let spoken = std::mem::take(&mut *turn.caller_words.lock().expect("caller words"))
        .trim()
        .to_string();
    let mut events = Vec::new();
    if !spoken.is_empty() {
        events.push(VoiceEvent::UserTurnEnded {
            transcript: spoken.clone(),
        });
    }
    events.push(VoiceEvent::DelegationRequested {
        tool_call_id: id.to_string(),
        reason: reason_for(&spoken),
    });
    events
}

/// A few words on what the caller wants, for the row the wake writes.
fn reason_for(spoken: &str) -> String {
    if spoken.is_empty() {
        return "the talker asked for the doer without saying why".to_string();
    }
    super::clip(spoken, super::READ_ALOUD_CHARS)
}

/// The talker stopped saying words, so its turn is over.
///
/// **A turn the talker never spoke in has no end.** The clock is armed by words
/// alone, so this answers with nothing after audio that carried none. That is
/// the honest reading: with no words there is no reply to write down, and
/// `call.rs` hands no floor back for one either.
///
/// Usage is all zeros, and that is what this provider reports. It bills by the
/// second, not by the token. Its duration figures are cumulative snapshots, and
/// summing them per turn would overstate every call. What prices a Live call is
/// `VoiceSessionEnded.duration_secs`, which the call already writes.
fn talker_went_quiet(turn: &mut TurnState) -> Vec<VoiceEvent> {
    if turn.last_words.take().is_none() {
        return vec![];
    }
    vec![VoiceEvent::TalkerTurnEnded {
        transcript: std::mem::take(&mut turn.talker_words).trim().to_string(),
        usage: ApiUsage::default(),
    }]
}

/// Everything still held when the socket goes.
///
/// The caller's words are owed whatever ended the call. A turn the talker was
/// mid-way through is owed too, or its words reach no transcript.
fn closing_events(turn: &mut TurnState) -> Vec<VoiceEvent> {
    let mut events = caller_finished(turn);
    events.extend(talker_went_quiet(turn));
    events
}

/// Hand over the caller's held words, if there are any.
///
/// **Asked on EVERY transcript delta, gated on nothing else.** The caller has
/// said nothing new between two deltas of one answer, so this answers with
/// nothing and the turn is not cut. That emptiness IS the gate.
///
/// Two richer gates were tried and both are wrong, because both couple the
/// caller's boundary to [`TALKER_IDLE`]. Reading the output stream RESUMING cut
/// one spoken sentence into seven rows. Reading `talker_words` being empty is
/// the same cut at a higher threshold: a hole over 700 ms in the middle of one
/// answer empties it, and the next delta re-opens a turn. The plan is
/// `docs/plans/2026-09-14-one-thing-the-caller-said-is-one-row.md`.
fn caller_finished(turn: &mut TurnState) -> Vec<VoiceEvent> {
    let spoken = std::mem::take(&mut *turn.caller_words.lock().expect("caller words"))
        .trim()
        .to_string();
    if spoken.is_empty() {
        return vec![];
    }
    vec![VoiceEvent::UserTurnEnded { transcript: spoken }]
}

fn decoded_audio(value: &Value) -> Option<Vec<u8>> {
    let delta = value.get("delta").and_then(Value::as_str)?;
    base64::engine::general_purpose::STANDARD.decode(delta).ok()
}

/// What an `error` frame said, or a sentence saying it said nothing.
pub fn error_message(value: &Value) -> String {
    value
        .pointer("/error/message")
        .or_else(|| value.pointer("/message"))
        .and_then(Value::as_str)
        .unwrap_or("the talker reported an error with no message")
        .to_string()
}

/// Read frames until the provider confirms the session, and refuse otherwise.
///
/// Nothing may be sent before this lands, which the provider states. It is also
/// where a wrong model id surfaces, as an `error` frame. Without it such a call
/// opens and then never speaks.
async fn await_session_started(reader: &mut Reader) -> Result<(), BoxError> {
    let deadline = Instant::now() + OPENING_TIMEOUT;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        let Ok(next) = tokio::time::timeout(left, reader.next()).await else {
            return Err("the talker never confirmed the session".into());
        };
        match next {
            Some(Ok(Message::Text(text))) => {
                let Ok(value) = serde_json::from_str::<Value>(&text) else {
                    continue;
                };
                match value.get("type").and_then(Value::as_str) {
                    Some("session.started") => return Ok(()),
                    Some("error") => return Err(error_message(&value).into()),
                    _ => {}
                }
            }
            Some(Ok(_)) => {}
            Some(Err(e)) => return Err(Box::new(e)),
            None => return Err("the talker closed the socket before starting".into()),
        }
    }
}

#[cfg(test)]
#[path = "live_tests.rs"]
mod tests;
