//! The call loop, driven end to end over a scripted transport and a mock
//! talker. No socket and no credential, and every event it writes lands in a
//! real database.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::{DateTime, Utc};
use sqlx::PgPool;
use tokio::sync::Notify;

use super::{
    answer_to_say, decision_to_ask, ASK_NEVER_GOT_ITS_WORDS, DELEGATION_PARKED,
    DELEGATION_PARKED_ON_SCREEN, OFFER_THE_DETAIL_ABOVE_CHARS,
};
use crate::engine::event_bus::EventBus;
use crate::engine::thread_events::{
    ActorMode, AnswerKind, CancelCause, MessageOrigin, QuestionOption, ThreadEvent,
    VoiceSessionEndReason,
};
use crate::engine::ApiUsage;
use crate::test_support::{seed_thread_event, setup_test_db, teardown_test_db};
use crate::voice::call::{run_call, CallSubject, CallTransport, CallerFrame};
use crate::voice::decision::{DecisionResolver, OpenDecision, Resolution};
use crate::voice::doer::TurnStarter;
use crate::voice::mock::MockVoiceProvider;
use crate::voice::provider::{AudioFormat, SessionOpening, VoiceEvent};
use crate::voice::wire::{ClientControl, ServerFrame};

type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// A thread with nothing waiting on the caller, and a resolver that records
/// what it was asked to settle.
///
/// The default is what most cases need: a free doer, so a delegation goes
/// through. `parked` and `answers` are set by the cases that are about the
/// other side.
#[derive(Default)]
struct NoDecisions {
    /// What `parked_on` answers.
    parked: Option<OpenDecision>,
    /// What `resolve` answers, in order. Exhausted, it settles.
    answers: Mutex<std::collections::VecDeque<Resolution>>,
    /// Every `(choice_id, spoken)` it was asked to settle, oldest first.
    asked: Arc<Mutex<Vec<(String, String)>>>,
}

/// The `tool_use_id` of the question card [`NoDecisions::parked`] stands on.
const PARKED_QUESTION: &str = "toolu_parked";

impl NoDecisions {
    /// A thread whose doer is parked on a question card.
    ///
    /// The commonest card, and the one a talker with no answering tool can
    /// settle out loud. Its free-text choice is `question:toolu_parked#said`.
    fn parked() -> Self {
        Self {
            parked: Some(OpenDecision::question(
                PARKED_QUESTION,
                "Run the tail now?",
                &[QuestionOption {
                    id: "opt-0".to_string(),
                    label: "Run it".to_string(),
                    description: None,
                    preview: None,
                }],
                false,
            )),
            ..Self::default()
        }
    }

    /// A thread whose doer is parked on a card that takes a DECISION rather
    /// than words. Nothing can settle one with a sentence.
    fn parked_on_a_permission() -> Self {
        Self {
            parked: Some(OpenDecision::command_permission(
                "req-cmd",
                "run_bash",
                "rm -rf build",
                "Deletes files.",
            )),
            ..Self::default()
        }
    }

    /// A resolver answering each `resolve` from this script, in order.
    fn answering(script: Vec<Resolution>) -> Self {
        Self {
            answers: Mutex::new(script.into_iter().collect()),
            ..Self::default()
        }
    }

    /// The same script, on a resolver already built. For a case that is about
    /// a parked card AND about what settling it answers.
    fn answering_with(mut self, script: Vec<Resolution>) -> Self {
        self.answers = Mutex::new(script.into_iter().collect());
        self
    }

    fn asked(&self) -> Arc<Mutex<Vec<(String, String)>>> {
        Arc::clone(&self.asked)
    }
}

/// The default resolver, shared: nothing waiting, and every answer settles.
///
/// Most cases are about something else entirely and only need a free doer, so
/// they pass this inline. A case asserting what was ASKED builds its own.
fn free_doer() -> &'static NoDecisions {
    static FREE: std::sync::OnceLock<NoDecisions> = std::sync::OnceLock::new();
    FREE.get_or_init(NoDecisions::default)
}

/// A namer that records every thread it was asked to name, and names nothing.
///
/// Naming reaches a model, so the real one is not something a call test can
/// run. What a test can assert is WHEN the call asked, which is the whole of
/// this file's half of the rule.
#[derive(Default)]
struct RecordingNames {
    asked: Mutex<Vec<uuid::Uuid>>,
}

impl RecordingNames {
    fn asked(&self) -> Vec<uuid::Uuid> {
        self.asked.lock().unwrap().clone()
    }
}

#[async_trait::async_trait]
impl crate::voice::naming::ThreadNamer for RecordingNames {
    async fn name_this_call(&self, thread_id: uuid::Uuid) {
        self.asked.lock().unwrap().push(thread_id);
    }
}

/// The default namer, shared: every case that is about something else.
///
/// The same shape as [`free_doer`]. A case asserting what was NAMED builds its
/// own, so nothing it reads was recorded by another test.
fn nobody_names() -> &'static RecordingNames {
    static NOBODY: std::sync::OnceLock<RecordingNames> = std::sync::OnceLock::new();
    NOBODY.get_or_init(RecordingNames::default)
}

#[async_trait::async_trait]
impl DecisionResolver for NoDecisions {
    async fn resolve(
        &self,
        _thread_id: uuid::Uuid,
        choice_id: &str,
        spoken: &str,
        _actor: Option<MessageOrigin>,
    ) -> Resolution {
        self.asked
            .lock()
            .unwrap()
            .push((choice_id.to_string(), spoken.to_string()));
        self.answers
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(Resolution::Settled)
    }

    async fn parked_on(&self, _thread_id: uuid::Uuid) -> Option<OpenDecision> {
        self.parked.clone()
    }
}

/// A doer that records what it was asked to start, and starts nothing.
///
/// The seam is what keeps a whole call runnable with no engine behind it. What
/// `ThreadTurn` does with an utterance is the chat path's own business, and it
/// is covered where that path is.
#[derive(Default)]
struct RecordingTurns {
    woken: Arc<Mutex<Vec<String>>>,
    /// The `WorkDelegated` row each turn anchored on. It is the turn's
    /// starter, so a call that dropped it would leave the transcript unable to
    /// place the turn's own events (ADR 0201).
    anchors: Arc<Mutex<Vec<uuid::Uuid>>>,
    /// Spoken replies offered to a running round. Whether one was running is
    /// the engine's business, so this records what was OFFERED.
    overheard: Arc<Mutex<Vec<String>>>,
    /// What `wake` answers. The shipping doer refuses a thread a call cannot
    /// reach (ADR 0165). The loop then owes the caller a row and a sentence.
    refuses: bool,
}

impl RecordingTurns {
    /// A doer that will not take an utterance, whatever it is.
    fn refusing() -> Self {
        Self {
            refuses: true,
            ..Self::default()
        }
    }

    fn woken(&self) -> Arc<Mutex<Vec<String>>> {
        Arc::clone(&self.woken)
    }

    fn anchors(&self) -> Arc<Mutex<Vec<uuid::Uuid>>> {
        Arc::clone(&self.anchors)
    }

    fn overheard(&self) -> Arc<Mutex<Vec<String>>> {
        Arc::clone(&self.overheard)
    }
}

#[async_trait::async_trait]
impl TurnStarter for RecordingTurns {
    async fn wake(
        &self,
        _thread_id: uuid::Uuid,
        delegation: uuid::Uuid,
        transcript: &str,
        _actor: Option<MessageOrigin>,
    ) -> bool {
        if self.refuses {
            return false;
        }
        self.woken.lock().unwrap().push(transcript.to_string());
        self.anchors.lock().unwrap().push(delegation);
        true
    }

    async fn overheard(&self, _thread_id: uuid::Uuid, spoken: &str) {
        self.overheard.lock().unwrap().push(spoken.to_string());
    }
}

/// The talker asking for the doer, with a reason.
fn asks_for_the_doer(reason: &str) -> VoiceEvent {
    VoiceEvent::DelegationRequested {
        tool_call_id: "call_1".to_string(),
        reason: reason.to_string(),
    }
}

/// What [`the_caller_opens_the_floor`] says.
const FIRST_WORD: &str = "hello";

/// The caller's first word, which is the whole of what opens the floor.
///
/// A call opens silent, so nothing the talker says before this reaches
/// anybody (ADR 0211). Most cases below are about the talker's own mechanics
/// rather than about that gate, so they open the floor and get on with it.
///
/// It costs one `SpokenMessageReceived` row and one delivery, both of which a
/// case counting either has to allow for.
fn the_caller_opens_the_floor() -> VoiceEvent {
    the_caller_says(FIRST_WORD)
}

/// The caller finishing a thought.
fn the_caller_says(text: &str) -> VoiceEvent {
    VoiceEvent::UserTurnEnded {
        transcript: text.to_string(),
    }
}

/// A piece of what the caller is saying, while they are still saying it.
fn the_caller_is_saying(text: &str) -> VoiceEvent {
    VoiceEvent::UserTranscript {
        text: text.to_string(),
    }
}

/// The talker finishing a reply.
fn the_talker_says(text: &str) -> VoiceEvent {
    VoiceEvent::TalkerTurnEnded {
        transcript: text.to_string(),
        usage: usage(),
    }
}

/// A piece of the reply the talker is speaking now.
///
/// Before the turn ends, these deltas are the whole account of a reply. That
/// is what makes a call stopping first worth scripting.
fn the_talker_is_saying(text: &str) -> VoiceEvent {
    VoiceEvent::TalkerTranscript {
        text: text.to_string(),
    }
}

fn subject(thread_id: uuid::Uuid, session_id: uuid::Uuid) -> CallSubject {
    CallSubject {
        thread_id,
        session_id,
        actor: None,
    }
}

/// How long a case may take past the bound it is waiting out.
///
/// It pays for a Postgres write, a build sharing the host, and the other cases
/// in the suite. It is not a budget any case is meant to spend.
const WAITING_SLACK: Duration = Duration::from_secs(20);

/// Wait until `ready` holds, letting the call loop make progress meanwhile.
///
/// The test runtime is single-threaded, so yielding is what hands it the
/// processor. Bounded, so a broken expectation fails rather than hangs.
///
/// **The bound is a DEADLINE, never an iteration count.** `yield_now` consumes
/// no wall clock. A spin of ten thousand yields can therefore finish in
/// milliseconds, while the Postgres write it waits on is still in flight. On a
/// loaded machine that is how a correct test fails, and it failed two of them.
///
/// Yields first, so a condition another task on this runtime can settle costs
/// no sleep at all. Past that it waits on the clock, which is the only thing an
/// answer coming over a socket can be waited on with.
///
/// **It has to clear the longest bound any case waits out, with room to
/// spare.** Two cases wait out [`super::CALLER_WAITED_LONG_ENOUGH`] for real,
/// and ten seconds left them four to lose. A loaded host loses four: one of
/// them timed out under the suite and passed on its own moments later.
async fn until(what: &str, mut ready: impl FnMut() -> bool) {
    let deadline = std::time::Instant::now() + super::CALLER_WAITED_LONG_ENOUGH + WAITING_SLACK;
    let mut spins = 0;
    loop {
        if ready() {
            return;
        }
        if std::time::Instant::now() >= deadline {
            panic!("timed out waiting for {}", what);
        }
        if spins < 1_000 {
            spins += 1;
            tokio::task::yield_now().await;
        } else {
            tokio::time::sleep(std::time::Duration::from_millis(2)).await;
        }
    }
}

/// A turn the engine ended without an answer, for the given reason.
///
/// Built directly rather than through `emit_response_canceled`, which needs a
/// live turn to anchor to. Same shape the fan-out tests seed.
fn canceled(cause: CancelCause) -> ThreadEvent {
    ThreadEvent::ResponseCanceled {
        text: String::new(),
        images: vec![],
        model: None,
        reasoning_effort: None,
        cause,
    }
}

/// A turn on this thread, seeded so a `ResponseGenerated` is legal to emit.
async fn a_turn_starts(bus: &EventBus, thread_id: uuid::Uuid) {
    seed_thread_event(
        bus,
        thread_id,
        ThreadEvent::MessageReceived {
            provider: None,
            voice_session_id: None,
            text: "what have I got running".to_string(),
            user_image_hashes: vec![],
            device_id: None,
            device: None,
            image_description: None,
            parent_thread_id: None,
            spawning_event_id: None,
            mode: ActorMode::Human,
            model: None,
            reasoning_effort: None,
            origin: None,
        },
    )
    .await;
}

/// A caller that reads from a script and records what it was sent.
///
/// **The end of a call is sequenced, never raced.** The loop selects over the
/// caller and the talker. So a hangup ready from the first poll can beat a
/// reply the talker already produced. `hanging_up_after` waits for N
/// deliveries first, which is what a person does: they hear the answer, then
/// ring off. `dropping_after` is the same wait with a dead socket instead.
struct ScriptedCaller {
    incoming: std::collections::VecDeque<CallerFrame>,
    sent: Arc<Mutex<Vec<ServerFrame>>>,
    audio_out_bytes: Arc<Mutex<usize>>,
    /// Frames plus audio chunks delivered to this caller so far.
    delivered: Arc<Mutex<usize>>,
    delivery: Arc<tokio::sync::Notify>,
    /// How this caller leaves, and after how many deliveries.
    ends_after: Option<(usize, CallerFrame)>,
    /// Rings off when the test says so, rather than on a delivery count.
    ///
    /// For a test whose sequence ends with something the caller never sees: a
    /// silent append, or an answer handed to the talker. `notify_one` stores a
    /// permit, so signalling before the caller listens is safe.
    hang_up_on: Option<Arc<Notify>>,
    /// A caller the test drives frame by frame. See [`ScriptedCaller::driven`].
    live: Option<tokio::sync::mpsc::Receiver<CallerFrame>>,
    gone: bool,
}

impl ScriptedCaller {
    fn new(incoming: Vec<CallerFrame>) -> Self {
        Self {
            incoming: incoming.into_iter().collect(),
            sent: Arc::new(Mutex::new(Vec::new())),
            audio_out_bytes: Arc::new(Mutex::new(0)),
            delivered: Arc::new(Mutex::new(0)),
            delivery: Arc::new(tokio::sync::Notify::new()),
            ends_after: None,
            hang_up_on: None,
            live: None,
            gone: false,
        }
    }

    /// A caller the test hands one frame at a time.
    ///
    /// A queued frame is read on the FIRST poll, before the talker has said
    /// anything. So a script cannot express "the caller cuts in here", and a
    /// barge-in means nothing except in relation to what the talker is doing.
    ///
    /// The hangup goes down the same channel, so it is sequenced behind
    /// whatever the test sent before it.
    fn driven() -> (Self, tokio::sync::mpsc::Sender<CallerFrame>) {
        let (tx, rx) = tokio::sync::mpsc::channel(8);
        let mut caller = Self::new(vec![]);
        caller.live = Some(rx);
        (caller, tx)
    }

    /// Ring off once `deliveries` frames or audio chunks have arrived.
    fn hanging_up_after(mut self, deliveries: usize) -> Self {
        self.ends_after = Some((deliveries, CallerFrame::Control(ClientControl::HangUp)));
        self
    }

    /// Lose the socket once `deliveries` have arrived.
    ///
    /// The same wait as a hangup, because a dropped phone is only sequenced
    /// differently by accident. A `Closed` frame handed over on the first poll
    /// ends the call before the talker has said anything.
    fn dropping_after(mut self, deliveries: usize) -> Self {
        self.ends_after = Some((deliveries, CallerFrame::Closed));
        self
    }

    /// Ring off when this is signalled.
    fn hanging_up_on(mut self, signal: Arc<Notify>) -> Self {
        self.hang_up_on = Some(signal);
        self
    }

    fn record_delivery(&self) {
        *self.delivered.lock().unwrap() += 1;
        self.delivery.notify_waiters();
    }
}

#[async_trait::async_trait]
impl CallTransport for ScriptedCaller {
    async fn recv(&mut self) -> CallerFrame {
        if let Some(frame) = self.incoming.pop_front() {
            return frame;
        }
        if let Some(live) = &mut self.live {
            return match live.recv().await {
                Some(frame) => frame,
                // Quiet, not gone. A test that drops its sender has finished
                // driving the caller, and the talker's own script ends the call.
                None => std::future::pending().await,
            };
        }
        if let Some(signal) = self.hang_up_on.clone().filter(|_| !self.gone) {
            signal.notified().await;
            self.gone = true;
            return CallerFrame::Control(ClientControl::HangUp);
        }
        let Some((target, ending)) = self.ends_after.clone().filter(|_| !self.gone) else {
            // A caller with nothing left to say has not hung up. Park, so the
            // talker's own script decides when the call ends.
            return std::future::pending().await;
        };
        // **Registered before the count is read, and that order is the whole
        // of it.** `notify_waiters` wakes only waiters already registered, and
        // a `Notified` registers when it is first polled. Checking first left a
        // window where the LAST delivery landed unheard, and the caller then
        // waited for a delivery nobody was going to make. `enable` registers
        // without awaiting, so no delivery can fall between the two.
        //
        // Rare, and this suite makes it likelier: a caller with a queued frame
        // reaches this wait one `recv` late, by which point the talker's script
        // is already pouring deliveries in.
        loop {
            let waiting = self.delivery.notified();
            tokio::pin!(waiting);
            waiting.as_mut().enable();
            if *self.delivered.lock().unwrap() >= target {
                break;
            }
            waiting.await;
        }
        self.gone = true;
        ending
    }

    async fn send_audio(&mut self, pcm: Vec<u8>) -> Result<(), BoxError> {
        *self.audio_out_bytes.lock().unwrap() += pcm.len();
        self.record_delivery();
        Ok(())
    }

    async fn send_frame(&mut self, frame: ServerFrame) -> Result<(), BoxError> {
        self.sent.lock().unwrap().push(frame);
        self.record_delivery();
        Ok(())
    }
}

fn opening() -> SessionOpening {
    SessionOpening {
        instructions: "You are Lucidos.".to_string(),
        resident_block: "[WHAT YOU ALREADY KNOW]".to_string(),
        voice: "marin".to_string(),
        transcriber: "gpt-4o-mini-transcribe".to_string(),
        audio: AudioFormat::default(),
        language: None,
    }
}

fn usage() -> ApiUsage {
    ApiUsage {
        input_tokens: 1200,
        output_tokens: 64,
        cache_read_tokens: 1024,
        cache_creation_tokens: 0,
        modality: Some(crate::engine::ModalityUsage {
            input_text_tokens: 176,
            input_audio_tokens: 1024,
            input_image_tokens: 0,
            cache_read_text_tokens: 100,
            cache_read_audio_tokens: 924,
            cache_read_image_tokens: 0,
            output_text_tokens: 20,
            output_audio_tokens: 44,
        }),
    }
}

/// Rows of `(event_type, payload, created)` for one thread, oldest first.
///
/// The one query, so the ordering contract is written once. Most callers want
/// [`thread_events`], which drops the timestamp.
async fn dated_thread_events(
    pool: &PgPool,
    thread_id: uuid::Uuid,
) -> Vec<(String, serde_json::Value, DateTime<Utc>)> {
    sqlx::query_as(
        "SELECT event_type, payload, created FROM events \
         WHERE thread_id = $1 ORDER BY created, sequence",
    )
    .bind(thread_id)
    .fetch_all(pool)
    .await
    .expect("read the thread's events")
}

/// Rows of `(event_type, payload)` for one thread, oldest first.
async fn thread_events(pool: &PgPool, thread_id: uuid::Uuid) -> Vec<(String, serde_json::Value)> {
    dated_thread_events(pool, thread_id)
        .await
        .into_iter()
        .map(|(kind, payload, _)| (kind, payload))
        .collect()
}

async fn a_chat_thread(pool: &PgPool) -> uuid::Uuid {
    let thread_id = uuid::Uuid::new_v4();
    sqlx::query("INSERT INTO thread_summaries (thread_id, source) VALUES ($1, 'chat')")
        .bind(thread_id)
        .execute(pool)
        .await
        .expect("create the thread");
    thread_id
}

/// A coding-agent thread, the only kind its permission lane fires on.
async fn a_coding_agent_thread(bus: &EventBus) -> uuid::Uuid {
    let thread_id = uuid::Uuid::new_v4();
    bus.emit(crate::engine::event_bus::BusEvent::Thread {
        thread_id,
        event: ThreadEvent::SessionStarted {
            coding_agent: crate::runtime::CodingAgent::ClaudeCode,
            session_id: "sid-test".to_string(),
            branch: "claude-code/test".to_string(),
            repo_id: None,
            coding_agent_kind: Default::default(),
            coding_agent_folder: String::new(),
            app_id: None,
        },
        meta: crate::engine::thread_events::EventMeta {
            channel: Some(crate::engine::thread_events::EventChannel::ClaudeCode),
            ..crate::engine::thread_events::EventMeta::NONE
        },
    })
    .await
    .expect("SessionStarted emit")
    .expect("SessionStarted persisted");
    thread_id
}

/// The ordinary shape of a call: one start, one end, and a hangup reason.
#[tokio::test]
async fn a_hangup_pairs_the_start_with_one_end() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = a_chat_thread(&pool).await;

    let provider = MockVoiceProvider::new(vec![]);
    let mut caller = ScriptedCaller::new(vec![CallerFrame::Control(ClientControl::HangUp)]);
    let session_id = uuid::Uuid::new_v4();

    let reason = run_call(
        &bus,
        &provider,
        &mut caller,
        &RecordingTurns::default(),
        free_doer(),
        nobody_names(),
        opening(),
        subject(thread_id, session_id),
    )
    .await;
    assert_eq!(reason, Some(VoiceSessionEndReason::Hangup));

    let events = thread_events(&pool, thread_id).await;
    let voice: Vec<&(String, serde_json::Value)> = events
        .iter()
        .filter(|(kind, _)| kind.starts_with("VoiceSession"))
        .collect();
    assert_eq!(voice.len(), 2, "expected exactly one pair: {:?}", events);
    assert_eq!(voice[0].0, "VoiceSessionStarted");
    assert_eq!(voice[1].0, "VoiceSessionEnded");
    assert_eq!(voice[0].1["session_id"], session_id.to_string());
    assert_eq!(voice[1].1["session_id"], session_id.to_string());
    assert_eq!(voice[1].1["reason"], "hangup");

    teardown_test_db(&db_name).await;
}

/// A dropped socket ends the call and still closes the pair. Nothing is left
/// running, because neither half touches thread status.
#[tokio::test]
async fn a_dropped_socket_still_closes_the_pair() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = a_chat_thread(&pool).await;

    let provider = MockVoiceProvider::new(vec![]);
    let mut caller = ScriptedCaller::new(vec![CallerFrame::Closed]);

    let reason = run_call(
        &bus,
        &provider,
        &mut caller,
        &RecordingTurns::default(),
        free_doer(),
        nobody_names(),
        opening(),
        subject(thread_id, uuid::Uuid::new_v4()),
    )
    .await;
    assert_eq!(reason, Some(VoiceSessionEndReason::Disconnected));

    let events = thread_events(&pool, thread_id).await;
    assert!(events.iter().any(|(k, _)| k == "VoiceSessionEnded"));

    let status: Option<String> =
        sqlx::query_scalar("SELECT status FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_one(&pool)
            .await
            .expect("read the thread status");
    assert_ne!(status.as_deref(), Some("running"));

    teardown_test_db(&db_name).await;
}

/// Voice is a mode of a thread (ADR 0148). A whole call therefore leaves
/// `source` as it found it, and opens no channel of its own.
#[tokio::test]
async fn a_whole_call_leaves_the_thread_a_chat_thread() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = a_chat_thread(&pool).await;

    let provider = MockVoiceProvider::new(vec![
        VoiceEvent::UserTurnEnded {
            transcript: "what have I got running".to_string(),
        },
        VoiceEvent::TalkerTranscript {
            text: "Checking".to_string(),
        },
        VoiceEvent::TalkerTurnEnded {
            transcript: "Checking.".to_string(),
            usage: usage(),
        },
    ]);
    // SessionStarted plus the talker's three events.
    let mut caller =
        ScriptedCaller::new(vec![CallerFrame::Audio(vec![0; 480])]).hanging_up_after(4);

    run_call(
        &bus,
        &provider,
        &mut caller,
        &RecordingTurns::default(),
        free_doer(),
        nobody_names(),
        opening(),
        subject(thread_id, uuid::Uuid::new_v4()),
    )
    .await;

    let source: Option<String> =
        sqlx::query_scalar("SELECT source FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_one(&pool)
            .await
            .expect("read the thread source");
    assert_eq!(source.as_deref(), Some("chat"));

    for (kind, payload) in thread_events(&pool, thread_id).await {
        assert_ne!(
            payload["channel"].as_str(),
            Some("voice"),
            "{} opened a voice channel",
            kind
        );
    }

    teardown_test_db(&db_name).await;
}

/// Decision 13: a session records what it spent, with the cached and fresh
/// split. One row per spoken reply, not one per call.
#[tokio::test]
async fn a_spoken_reply_records_what_it_spent() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = a_chat_thread(&pool).await;

    let provider = MockVoiceProvider::new(vec![VoiceEvent::TalkerTurnEnded {
        transcript: "I am checking.".to_string(),
        usage: usage(),
    }]);
    // SessionStarted, then TalkerTurnEnded. Ring off only once both landed.
    let mut caller = ScriptedCaller::new(vec![]).hanging_up_after(2);

    run_call(
        &bus,
        &provider,
        &mut caller,
        &RecordingTurns::default(),
        free_doer(),
        nobody_names(),
        opening(),
        subject(thread_id, uuid::Uuid::new_v4()),
    )
    .await;

    let captures = crate::test_support::aux_captures(&pool, thread_id, "voice").await;
    assert_eq!(captures.len(), 1, "one row per spoken reply");
    assert_eq!(captures[0]["producer"], "auxiliary");
    assert_eq!(captures[0]["usage"]["input_tokens"], 1200);
    assert_eq!(captures[0]["usage"]["cache_read_tokens"], 1024);
    assert_eq!(captures[0]["usage"]["output_tokens"], 64);
    // The modality split has to survive into the stored payload: it is what
    // prices the turn, and nothing recomputes it downstream.
    let modality = &captures[0]["usage"]["modality"];
    assert_eq!(modality["input_audio_tokens"], 1024);
    assert_eq!(modality["input_text_tokens"], 176);
    assert_eq!(modality["output_audio_tokens"], 44);
    assert_eq!(modality["cache_read_audio_tokens"], 924);

    teardown_test_db(&db_name).await;
}

/// A talker that will not open writes NO events. A start with no call behind
/// it would make the pair count sessions that never happened.
#[tokio::test]
async fn a_talker_that_never_answers_leaves_no_trace() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = a_chat_thread(&pool).await;

    let provider = MockVoiceProvider::refusing("the provider is down");
    let mut caller = ScriptedCaller::new(vec![]);
    let sent = Arc::clone(&caller.sent);

    let reason = run_call(
        &bus,
        &provider,
        &mut caller,
        &RecordingTurns::default(),
        free_doer(),
        nobody_names(),
        opening(),
        subject(thread_id, uuid::Uuid::new_v4()),
    )
    .await;
    assert_eq!(reason, None);

    let events = thread_events(&pool, thread_id).await;
    assert!(
        !events.iter().any(|(k, _)| k.starts_with("VoiceSession")),
        "a call that never opened wrote {:?}",
        events
    );

    // The caller is told, rather than left looking at a dead socket, and the
    // sentence names no provider.
    let first = sent.lock().unwrap().first().cloned();
    match first {
        Some(ServerFrame::Error { message }) => {
            assert!(!message.to_lowercase().contains("openai"), "{}", message)
        }
        other => panic!("expected an error frame, got {:?}", other),
    }

    teardown_test_db(&db_name).await;
}

/// Talker audio reaches the caller and is written down nowhere.
#[tokio::test]
async fn talker_audio_reaches_the_caller_and_no_event() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = a_chat_thread(&pool).await;

    let provider = MockVoiceProvider::new(vec![
        the_caller_opens_the_floor(),
        VoiceEvent::Audio(vec![7; 960]),
    ]);
    // SessionStarted, the caller's own turn, then the audio chunk.
    let mut caller = ScriptedCaller::new(vec![]).hanging_up_after(3);
    let audio_out = Arc::clone(&caller.audio_out_bytes);

    run_call(
        &bus,
        &provider,
        &mut caller,
        &RecordingTurns::default(),
        free_doer(),
        nobody_names(),
        opening(),
        subject(thread_id, uuid::Uuid::new_v4()),
    )
    .await;

    assert_eq!(*audio_out.lock().unwrap(), 960);
    for (kind, payload) in thread_events(&pool, thread_id).await {
        assert!(
            !payload.to_string().contains("\"audio\""),
            "{} carried audio",
            kind
        );
    }

    teardown_test_db(&db_name).await;
}

/// The boot sweep is the floor under a killed engine: an unpaired start is a
/// call that is over, and it gets its end.
#[tokio::test]
async fn the_boot_sweep_settles_a_session_its_engine_died_holding() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = a_chat_thread(&pool).await;
    let session_id = uuid::Uuid::new_v4();

    crate::test_support::seed_thread_event(
        &bus,
        thread_id,
        crate::engine::thread_events::ThreadEvent::VoiceSessionStarted { session_id },
    )
    .await;

    crate::voice::recovery::settle_orphan_voice_sessions(&pool, &bus).await;

    let ends: Vec<serde_json::Value> = thread_events(&pool, thread_id)
        .await
        .into_iter()
        .filter(|(kind, _)| kind == "VoiceSessionEnded")
        .map(|(_, payload)| payload)
        .collect();
    assert_eq!(ends.len(), 1);
    assert_eq!(ends[0]["session_id"], session_id.to_string());
    assert_eq!(ends[0]["reason"], "engine_shutdown");
    assert_eq!(ends[0]["duration_secs"], 0);

    // Idempotent: the pair is now closed, so a second sweep adds nothing.
    crate::voice::recovery::settle_orphan_voice_sessions(&pool, &bus).await;
    let ends = thread_events(&pool, thread_id)
        .await
        .into_iter()
        .filter(|(kind, _)| kind == "VoiceSessionEnded")
        .count();
    assert_eq!(ends, 1);

    teardown_test_db(&db_name).await;
}

/// The talker dropping the call is not the caller hanging up. It ends as a
/// provider failure, and the caller is told rather than left on a dead socket.
#[tokio::test]
async fn a_talker_that_drops_the_call_ends_it_as_a_failure() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = a_chat_thread(&pool).await;

    let provider = MockVoiceProvider::ending_after(vec![]);
    let mut caller = ScriptedCaller::new(vec![]);
    let sent = Arc::clone(&caller.sent);

    let reason = run_call(
        &bus,
        &provider,
        &mut caller,
        &RecordingTurns::default(),
        free_doer(),
        nobody_names(),
        opening(),
        subject(thread_id, uuid::Uuid::new_v4()),
    )
    .await;
    assert_eq!(reason, Some(VoiceSessionEndReason::ProviderFailed));

    let ends: Vec<serde_json::Value> = thread_events(&pool, thread_id)
        .await
        .into_iter()
        .filter(|(kind, _)| kind == "VoiceSessionEnded")
        .map(|(_, payload)| payload)
        .collect();
    assert_eq!(ends.len(), 1);
    assert_eq!(ends[0]["reason"], "provider_failed");

    let sent = sent.lock().unwrap().clone();
    assert!(
        sent.iter().any(|f| matches!(f, ServerFrame::Error { .. })),
        "the caller was not told: {:?}",
        sent
    );

    teardown_test_db(&db_name).await;
}

/// A caller whose socket dies mid-reply is DISCONNECTED, not a provider
/// failure. The reason lands in a persisted event a trigger can match on, so
/// blaming the talker for somebody's tunnel is a lie the log keeps.
#[tokio::test]
async fn a_caller_who_stops_receiving_is_not_a_provider_failure() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = a_chat_thread(&pool).await;

    let provider = MockVoiceProvider::new(vec![
        the_caller_opens_the_floor(),
        VoiceEvent::Audio(vec![1; 320]),
    ]);
    let mut caller = DeafCaller { sends: 0 };

    let reason = run_call(
        &bus,
        &provider,
        &mut caller,
        &RecordingTurns::default(),
        free_doer(),
        nobody_names(),
        opening(),
        subject(thread_id, uuid::Uuid::new_v4()),
    )
    .await;
    assert_eq!(reason, Some(VoiceSessionEndReason::Disconnected));

    let ends: Vec<serde_json::Value> = thread_events(&pool, thread_id)
        .await
        .into_iter()
        .filter(|(kind, _)| kind == "VoiceSessionEnded")
        .map(|(_, payload)| payload)
        .collect();
    assert_eq!(ends.len(), 1);
    assert_eq!(ends[0]["reason"], "disconnected");

    teardown_test_db(&db_name).await;
}

/// A caller that takes the opening frame and then stops receiving anything.
struct DeafCaller {
    sends: usize,
}

#[async_trait::async_trait]
impl CallTransport for DeafCaller {
    async fn recv(&mut self) -> CallerFrame {
        std::future::pending().await
    }

    async fn send_audio(&mut self, _pcm: Vec<u8>) -> Result<(), BoxError> {
        Err("the caller is gone".into())
    }

    async fn send_frame(&mut self, _frame: ServerFrame) -> Result<(), BoxError> {
        self.sends += 1;
        // The opening frame lands, and so does the caller's own first turn:
        // the floor is shut until they speak, so nothing else is sent before
        // it (ADR 0211). Everything after that finds a dead socket.
        if self.sends > 2 {
            return Err("the caller is gone".into());
        }
        Ok(())
    }
}

// ── The talker decides ────────────────────────────────────────────────────

/// Whether this talker event puts a frame on the caller's socket.
///
/// Two do not. A delegation is the whole non-goal: the wire vocabulary gains
/// nothing, because the caller only hears one answer. The caller starting to
/// speak is the other, and the client already draws that off its own
/// microphone (ADR 0184).
fn reaches_the_caller(event: &VoiceEvent) -> bool {
    !matches!(
        event,
        VoiceEvent::DelegationRequested { .. } | VoiceEvent::CallerStartedSpeaking
    )
}

/// How many deliveries a whole script makes, read in order.
///
/// **Floor-aware, because the floor is what decides** (ADR 0211, ADR 0213). The
/// talker reaches nobody before the caller's first word, and nobody again once
/// one opener's turns are spent. Its audio and its deltas are no delivery at
/// all then. Counted as one, the scripted caller waits on something nobody
/// sends and the case hangs.
///
/// A turn END is still a frame either way, and so is an interruption. Neither
/// carries words the caller was or was not played.
fn deliveries_of(script: &[VoiceEvent]) -> usize {
    let mut turns_left = 0u8;
    // `Audience`, as the loop keeps it. `None` is undecided, and reads the
    // floor. The latch has to be modelled now that the floor can shut
    // MID-CALL. A caller opening it inside a turn the talker began unheard
    // changes nothing about that turn. A helper that missed this would count
    // frames nobody sends, which hangs the scripted caller.
    let mut audience: Option<bool> = None;
    // `Call::mute_held_for`. An unheard sentence keeps the latch across its own
    // turn end, so the helper has to carry it too or it counts frames nobody
    // sends.
    let mut held = 0u8;
    let mut deliveries = 0;
    for event in script {
        // The openers first, because one of them reaches the caller as
        // nothing and would be skipped below before it opened anything.
        if opens_the_floor(event) {
            turns_left = super::TURNS_ONE_OPENER_BUYS;
        }
        if latches_the_turn(event) && audience.is_none() {
            audience = Some(turns_left > 0);
        }
        let heard = audience.unwrap_or(turns_left > 0);
        if let VoiceEvent::TalkerTurnEnded { transcript, .. } = event {
            if heard && !transcript.trim().is_empty() {
                turns_left = turns_left.saturating_sub(1);
            }
            let holding = !heard
                && turns_left > 0
                && held < super::TURNS_ONE_OPENER_BUYS
                && super::the_sentence_is_unfinished(transcript);
            held = if holding { held + 1 } else { 0 };
            audience = if holding { Some(false) } else { None };
        }
        if !reaches_the_caller(event) {
            continue;
        }
        let is_words = matches!(
            event,
            VoiceEvent::Audio(_) | VoiceEvent::TalkerTranscript { .. }
        );
        if is_words && !heard {
            continue;
        }
        deliveries += 1;
    }
    deliveries
}

/// Whether this event hands the talker a fresh budget of turns.
fn opens_the_floor(event: &VoiceEvent) -> bool {
    match event {
        VoiceEvent::UserTranscript { text } => !text.trim().is_empty(),
        VoiceEvent::UserTurnEnded { transcript } => !transcript.trim().is_empty(),
        VoiceEvent::CallerStartedSpeaking => true,
        _ => false,
    }
}

/// Whether this event settles who the turn it belongs to is for.
///
/// The talker's first WORD, exactly as `Call::the_talker_said_a_word` reads it.
/// A blank delta settles nothing, and nor does audio.
fn latches_the_turn(event: &VoiceEvent) -> bool {
    matches!(event, VoiceEvent::TalkerTranscript { text } if !text.trim().is_empty())
}

/// A frame that lands on the caller and writes nothing down.
///
/// The last event of every scripted call, so the hangup is sequenced behind
/// whatever the script did. An empty transcript delta is the cheapest event
/// with that shape: it sends a frame and touches only the floor.
///
/// **It lands only while the floor is open**, being a delta like any other. A
/// script that ends with the floor shut therefore rings off on its own last
/// frame instead, which is safe for every such case here: a turn end writes
/// its row before it sends that frame. A case whose last event settles
/// something AFTER its frame has to say so with a frame of its own.
fn the_talker_draws_breath() -> VoiceEvent {
    the_talker_is_saying("")
}

/// What one scripted call left behind.
struct WhatTheCallDid {
    /// Every event on the thread, oldest first.
    events: Vec<(String, serde_json::Value)>,
    /// The utterances that reached the doer, in order.
    woken: Vec<String>,
    /// The `WorkDelegated` row each of those turns anchored on. It is the
    /// turn's starter, so a call that dropped it would leave the transcript
    /// unable to place the turn's own events (ADR 0201).
    anchors: Vec<uuid::Uuid>,
    /// What a running round was told the caller had been told, in order.
    ///
    /// A turn nobody heard is offered to nobody, so this is where the floor
    /// shows up on the doer's side (ADR 0211, ADR 0213).
    overheard: Vec<String>,
}

/// One call, driven over a script, returning what it wrote and what it woke.
///
/// Every branch below asks the same two questions, so they ask them the same
/// way: which rows the thread holds, and which utterances reached the doer.
async fn a_call_that_hears(
    pool: &PgPool,
    bus: &EventBus,
    thread_id: uuid::Uuid,
    session_id: uuid::Uuid,
    script: Vec<VoiceEvent>,
) -> WhatTheCallDid {
    a_call_named_by(pool, bus, thread_id, session_id, script, nobody_names()).await
}

/// The same call, with a namer the case can read afterwards.
///
/// Separate because most cases are about what a call WROTE, and only the
/// naming cases care who was asked for a name.
async fn a_call_named_by(
    pool: &PgPool,
    bus: &EventBus,
    thread_id: uuid::Uuid,
    session_id: uuid::Uuid,
    script: Vec<VoiceEvent>,
    namer: &RecordingNames,
) -> WhatTheCallDid {
    // Counted, not guessed, and with a trailing frame of its own.
    //
    // A delegation reaches the caller as NOTHING, by design: they hear one
    // answer and never learn which model produced it. Counting the script
    // would leave the caller waiting on a frame nobody sends. Gating on the
    // last VISIBLE frame would let the hangup beat a silent event after it.
    // The sentinel gives every script one frame that lands last.
    let mut script = script;
    script.push(the_talker_draws_breath());
    // The opening frame, plus whatever the script itself delivers. The
    // sentinel is counted with the rest, being droppable for the same reason
    // any other talker frame is.
    let deliveries = 1 + deliveries_of(&script);
    let provider = MockVoiceProvider::new(script);
    let turns = RecordingTurns::default();
    let woken = turns.woken();
    let anchors = turns.anchors();
    let overheard = turns.overheard();
    let mut caller = ScriptedCaller::new(vec![]).hanging_up_after(deliveries);

    run_call(
        bus,
        &provider,
        &mut caller,
        &turns,
        free_doer(),
        namer,
        opening(),
        subject(thread_id, session_id),
    )
    .await;

    let woken = woken.lock().unwrap().clone();
    let anchors = anchors.lock().unwrap().clone();
    let overheard = overheard.lock().unwrap().clone();
    WhatTheCallDid {
        events: thread_events(pool, thread_id).await,
        woken,
        anchors,
        overheard,
    }
}

/// Just the rows one utterance can produce, oldest first.
///
/// `MessageReceived` is in the list deliberately. A delegated utterance takes
/// the typed message's path, so recording it twice shows up here as an extra
/// row rather than as a passing test.
fn voice_rows(events: &[(String, serde_json::Value)]) -> Vec<(String, serde_json::Value)> {
    events
        .iter()
        .filter(|(kind, _)| {
            matches!(
                kind.as_str(),
                "SpokenMessageReceived" | "WorkDelegated" | "MessageReceived"
            )
        })
        .cloned()
        .collect()
}

/// Both halves of a talker-only exchange, as type names, oldest first.
///
/// Wider than `voice_kinds`, which leaves the reply out because it answers a
/// different question: how many rows one utterance produced. This one is about
/// the order a reader meets the two in.
fn spoken_kinds(events: &[(String, serde_json::Value)]) -> Vec<String> {
    events
        .iter()
        .filter(|(kind, _)| {
            matches!(
                kind.as_str(),
                "SpokenMessageReceived" | "SpokenReplyGenerated"
            )
        })
        .map(|(kind, _)| kind.clone())
        .collect()
}

/// The same rows, as their type names only.
fn voice_kinds(events: &[(String, serde_json::Value)]) -> Vec<String> {
    voice_rows(events)
        .into_iter()
        .map(|(kind, _)| kind)
        .collect()
}

/// The bug this whole change fixes. The talker answered from what it already
/// knew, so the doer never ran and the caller heard ONE answer.
///
/// The utterance is still in the thread, as the row that starts nothing.
#[tokio::test]
async fn an_utterance_the_talker_handles_alone_wakes_nobody() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = a_chat_thread(&pool).await;
    let session_id = uuid::Uuid::new_v4();

    let did = a_call_that_hears(
        &pool,
        &bus,
        thread_id,
        session_id,
        vec![the_caller_says("hei"), the_talker_says("Hei!")],
    )
    .await;

    assert!(
        did.woken.is_empty(),
        "the doer ran for a turn nobody asked for"
    );
    let rows = voice_rows(&did.events);
    assert_eq!(rows.len(), 1, "{:?}", rows);
    assert_eq!(rows[0].0, "SpokenMessageReceived");
    assert_eq!(rows[0].1["text"], "hei");
    assert_eq!(rows[0].1["session_id"], session_id.to_string());

    teardown_test_db(&db_name).await;
}

/// A partial is drawn and never written down.
///
/// It reaches the caller, which the sentinel accounting proves: every visible
/// frame is counted and the hangup waits for all of them, so a partial the
/// loop swallowed would leave this call hanging. What it must NOT do is settle
/// anything WHILE THE CALL RUNS. A partial pairing with a waiting ask would
/// spend it on half a sentence.
///
/// A call that ends holding only partials is the other case, and it is covered
/// below: those words are the caller's, and losing them is the defect the
/// hold exists to prevent.
#[tokio::test]
async fn a_caller_partial_settles_nothing_while_the_call_runs() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = a_chat_thread(&pool).await;
    let session_id = uuid::Uuid::new_v4();

    let did = a_call_that_hears(
        &pool,
        &bus,
        thread_id,
        session_id,
        vec![
            the_caller_is_saying("what have "),
            the_caller_is_saying("I got running"),
            // The ask is what a partial must not pair with. Half a sentence
            // would run as a turn, and the rest would arrive with no ask left.
            asks_for_the_doer("they asked what is running"),
        ],
    )
    .await;

    assert!(did.woken.is_empty(), "a partial paired with an ask");
    let kinds: Vec<_> = voice_rows(&did.events)
        .into_iter()
        .map(|(kind, _)| kind)
        .collect();
    assert!(
        !kinds.iter().any(|kind| kind == "WorkDelegated"),
        "a partial started a turn: {:?}",
        kinds
    );
    // Still owed a row, because the words are the caller's. It is written when
    // the call ends and nothing better is coming.
    assert_eq!(kinds, vec!["SpokenMessageReceived".to_string()]);

    teardown_test_db(&db_name).await;
}

/// **Defect 3 of the seven-bubble call: the caller's last words were lost.**
///
/// A provider with no turn-end frame reports the caller through partials alone.
/// The call ends, nothing ever says the turn finished, and the words reached no
/// row. They are the caller's either way, so they are written down.
#[tokio::test]
async fn a_call_that_ends_on_a_partial_still_writes_the_words_down() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = a_chat_thread(&pool).await;
    let session_id = uuid::Uuid::new_v4();

    let did = a_call_that_hears(
        &pool,
        &bus,
        thread_id,
        session_id,
        vec![
            the_caller_is_saying("restart the "),
            the_caller_is_saying("computer when"),
        ],
    )
    .await;

    let rows = voice_rows(&did.events);
    assert_eq!(rows.len(), 1, "{:?}", rows);
    assert_eq!(rows[0].0, "SpokenMessageReceived");
    assert_eq!(rows[0].1["text"], "restart the computer when");

    teardown_test_db(&db_name).await;
}

/// The row carries the transcript as the frame carried it.
///
/// The transcript client-side must be matchable against the row, because that
/// is how a *live utterance* row is retired (`claimUtteranceRows`). Only ONE
/// leg of the chain normalizes, `doer.rs::wake`, which trims before it writes
/// a delegated row. So trimming both sides is the whole of the match, and a
/// second normalization added here would break it silently.
#[tokio::test]
async fn a_spoken_row_carries_the_transcript_unchanged() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = a_chat_thread(&pool).await;
    let session_id = uuid::Uuid::new_v4();
    let spoken = "  what have I got running  ";

    let did = a_call_that_hears(
        &pool,
        &bus,
        thread_id,
        session_id,
        vec![the_caller_says(spoken), the_talker_says("Two things.")],
    )
    .await;

    let rows = voice_rows(&did.events);
    assert_eq!(rows.len(), 1, "{:?}", rows);
    assert_eq!(rows[0].0, "SpokenMessageReceived");
    assert_eq!(rows[0].1["text"], spoken);

    teardown_test_db(&db_name).await;
}

/// The partials give way to the finished words, which are the only ones any
/// row is written from.
#[tokio::test]
async fn the_finished_words_are_what_the_row_carries() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = a_chat_thread(&pool).await;
    let session_id = uuid::Uuid::new_v4();

    let did = a_call_that_hears(
        &pool,
        &bus,
        thread_id,
        session_id,
        vec![
            the_caller_is_saying("what have "),
            the_caller_says("what have I got running"),
            the_talker_says("Two things."),
        ],
    )
    .await;

    let rows = voice_rows(&did.events);
    assert_eq!(rows.len(), 1, "{:?}", rows);
    assert_eq!(rows[0].0, "SpokenMessageReceived");
    assert_eq!(rows[0].1["text"], "what have I got running");

    teardown_test_db(&db_name).await;
}

/// The transcript reads in the order the call happened.
///
/// Both rows of a talker-only exchange leave one handler, so the order they
/// are emitted in IS the order a reader meets them. Recording the reply first
/// put every answer above the question it answered.
#[tokio::test]
async fn a_spoken_answer_never_lands_above_its_question() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = a_chat_thread(&pool).await;

    let did = a_call_that_hears(
        &pool,
        &bus,
        thread_id,
        uuid::Uuid::new_v4(),
        vec![
            the_caller_says("what happened"),
            the_talker_says("The codebase is clean."),
            the_caller_says("anything for me"),
            the_talker_says("Nothing urgent."),
        ],
    )
    .await;

    assert_eq!(
        spoken_kinds(&did.events),
        vec![
            "SpokenMessageReceived",
            "SpokenReplyGenerated",
            "SpokenMessageReceived",
            "SpokenReplyGenerated",
        ],
        "{:?}",
        spoken_kinds(&did.events)
    );

    teardown_test_db(&db_name).await;
}

/// **A call earns a name when the caller answers something.**
///
/// The opening utterance names nothing: a person starts a call with "hey" or
/// "what's going on", and that is what the thread was called for as long as it
/// existed. What the caller says AFTER a reply is about something, and it
/// arrives with the reply behind it.
#[tokio::test]
async fn a_call_is_named_once_the_caller_answers_a_reply() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = a_chat_thread(&pool).await;
    let namer = RecordingNames::default();

    a_call_named_by(
        &pool,
        &bus,
        thread_id,
        uuid::Uuid::new_v4(),
        vec![
            the_caller_says("what's going on"),
            the_talker_says("Watching the tab-icon fix."),
            the_caller_says("yeah, please check"),
        ],
        &namer,
    )
    .await;

    assert_eq!(
        namer.asked(),
        vec![thread_id],
        "the second utterance answers a reply, so the call has a subject"
    );

    teardown_test_db(&db_name).await;
}

/// A call nobody answered is not a conversation, and names nothing.
///
/// The caller says one thing into the void and rings off. Named anyway, the
/// model describes the fragment: " So, yeah, I think" became "Incomplete
/// Conversation Opener", and a name is permanent. The fallback shows their own
/// words instead, and the thread stays nameable.
#[tokio::test]
async fn a_call_nothing_answered_is_never_named() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = a_chat_thread(&pool).await;
    let namer = RecordingNames::default();

    a_call_named_by(
        &pool,
        &bus,
        thread_id,
        uuid::Uuid::new_v4(),
        vec![the_caller_says(" So, yeah, I think")],
        &namer,
    )
    .await;

    assert!(
        namer.asked().is_empty(),
        "nothing answered them, so the loop asked for no name: {:?}",
        namer.asked()
    );

    teardown_test_db(&db_name).await;
}

/// One ask per call, however long the call runs.
///
/// Every later utterance also answers a reply, and asking on each would put a
/// model call behind every sentence a caller says. The name is settled by the
/// first one.
#[tokio::test]
async fn a_long_call_asks_for_one_name() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = a_chat_thread(&pool).await;
    let namer = RecordingNames::default();

    a_call_named_by(
        &pool,
        &bus,
        thread_id,
        uuid::Uuid::new_v4(),
        vec![
            the_caller_says("what's going on"),
            the_talker_says("Watching the tab-icon fix."),
            the_caller_says("yeah, please check"),
            the_talker_says("Still running."),
            the_caller_says("and the release"),
            the_talker_says("Tagged this morning."),
            the_caller_says("good"),
        ],
        &namer,
    )
    .await;

    assert_eq!(namer.asked(), vec![thread_id], "one name, one call");

    teardown_test_db(&db_name).await;
}

/// The reply rows of a call, oldest first.
fn replies(events: &[(String, serde_json::Value)]) -> Vec<serde_json::Value> {
    events
        .iter()
        .filter(|(kind, _)| kind == "SpokenReplyGenerated")
        .map(|(_, payload)| payload.clone())
        .collect()
}

/// How many usage rows this call's talker produced.
fn voice_captures(events: &[(String, serde_json::Value)]) -> usize {
    events
        .iter()
        .filter(|(kind, payload)| kind == "ContextCaptured" && payload["purpose"] == "voice")
        .count()
}

/// **One turn is one row, written as that turn ends** (ADR 0201).
///
/// A Live talker paces its transcript with its audio, so one sentence has
/// holes past `TALKER_IDLE` inside it. The three turns below are the real
/// frames of one reported reply, and its own full stop was a turn of its own.
///
/// Each gets a row, dated when its words stopped. That is what lets the
/// transcript read by the clock alone. Putting the sentence back together is a
/// reading of those rows, and `spoken_merge` is where it happens.
#[tokio::test]
async fn each_talker_turn_is_its_own_row() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = a_chat_thread(&pool).await;

    let did = a_call_that_hears(
        &pool,
        &bus,
        thread_id,
        uuid::Uuid::new_v4(),
        vec![
            the_caller_says("What's the status"),
            the_talker_is_saying("Nothing is waiting on you, and I have no unread notifications"),
            the_talker_says("Nothing is waiting on you, and I have no unread notifications"),
            the_talker_is_saying("."),
            the_talker_says("."),
            the_talker_is_saying(" I'm getting a current snapshot."),
            the_talker_says("I'm getting a current snapshot."),
        ],
    )
    .await;

    let said = replies(&did.events);
    assert_eq!(said.len(), 3, "{:?}", said);
    assert_eq!(
        said[0]["text"],
        "Nothing is waiting on you, and I have no unread notifications"
    );
    assert_eq!(said[1]["text"], ".");
    assert_eq!(said[2]["text"], "I'm getting a current snapshot.");
    // Its turns all ended, so the caller never cut into any of them.
    assert!(said.iter().all(|row| row["interrupted"] == false));
    // And the question reads above every answer to it, because it was said
    // first and every row is now dated when its words stopped.
    assert_eq!(
        spoken_kinds(&did.events),
        vec![
            "SpokenMessageReceived",
            "SpokenReplyGenerated",
            "SpokenReplyGenerated",
            "SpokenReplyGenerated",
        ],
        "{:?}",
        spoken_kinds(&did.events)
    );
    // The usage row goes with the words it paid for, so one per turn.
    assert_eq!(voice_captures(&did.events), 3);

    teardown_test_db(&db_name).await;
}

/// The three pieces above read back to the doer as the one sentence they are.
///
/// The engine writes turns and the history joins them, which is the whole of
/// the split: `created` stays honest and the model still reads prose.
#[tokio::test]
async fn the_doer_reads_those_three_rows_as_one_sentence() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = a_chat_thread(&pool).await;

    a_call_that_hears(
        &pool,
        &bus,
        thread_id,
        uuid::Uuid::new_v4(),
        vec![
            the_caller_says("What's the status"),
            the_talker_is_saying("Nothing is waiting on you, and I have no unread notifications"),
            the_talker_says("Nothing is waiting on you, and I have no unread notifications"),
            the_talker_is_saying("."),
            the_talker_says("."),
            the_talker_is_saying(" I'm getting a current snapshot."),
            the_talker_says("I'm getting a current snapshot."),
        ],
    )
    .await;

    let store = crate::core::EventStore::new(pool.clone());
    let messages = store
        .get_thread_messages(&thread_id.to_string())
        .await
        .expect("the thread's messages read back");
    let spoken: Vec<&str> = messages
        .iter()
        .filter(|m| m.event_id.is_some())
        .map(|m| m.content.as_str())
        .collect();
    assert_eq!(
        spoken,
        vec![
            "What's the status",
            "Nothing is waiting on you, and I have no unread notifications. \
             I'm getting a current snapshot.",
        ],
        "{:?}",
        spoken
    );

    teardown_test_db(&db_name).await;
}

/// Each row of one type as `(payload, created)`, oldest first.
///
/// `created` is a column rather than a payload field, so a test comparing the
/// two needs the dated read.
async fn rows_of(
    pool: &PgPool,
    thread_id: uuid::Uuid,
    kind: &str,
) -> Vec<(serde_json::Value, DateTime<Utc>)> {
    dated_thread_events(pool, thread_id)
        .await
        .into_iter()
        .filter(|(row_kind, _, _)| row_kind == kind)
        .map(|(_, payload, created)| (payload, created))
        .collect()
}

/// How long the stall runs before the doer is asked for.
///
/// A real one runs for a second or more. A scripted call has no wall clock, so
/// its two rows land inside one millisecond. The ordering under test cannot be
/// observed there. This is the smallest gap well clear of the noise.
const A_BEAT: Duration = Duration::from_millis(60);

/// **The misplaced-stall regression.** The reported call drew `I'm on it, give
/// me a sec.` under fifteen seconds of the work it promised.
///
/// The turn's own end writes the row, so `created` IS when the words stopped
/// and the stall reads above the work it promised (ADR 0201).
#[tokio::test]
async fn a_stall_is_written_before_the_work_it_promised() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = a_chat_thread(&pool).await;

    let (provider, talker) = MockVoiceProvider::driven();
    let log = provider.log();
    let turns = RecordingTurns::default();
    let woken = turns.woken();
    let hang_up = Arc::new(Notify::new());
    let mut caller = ScriptedCaller::new(vec![]).hanging_up_on(Arc::clone(&hang_up));
    let sent = Arc::clone(&caller.sent);

    tokio::join!(
        run_call(
            &bus,
            &provider,
            &mut caller,
            &turns,
            free_doer(),
            nobody_names(),
            opening(),
            subject(thread_id, uuid::Uuid::new_v4()),
        ),
        async {
            until("the session to open", || {
                log.lock().unwrap().openings.len() == 1
            })
            .await;
            let say = |event| async {
                talker.send(event).await.expect("the talker is listening");
            };
            let transcripts = || {
                sent.lock()
                    .unwrap()
                    .iter()
                    .filter(|f| matches!(f, ServerFrame::TalkerTranscript { .. }))
                    .count()
            };

            say(the_caller_says("What is it waiting an answer for")).await;
            say(the_talker_is_saying("I'm on it,")).await;
            until("the stall to reach the caller", || transcripts() == 1).await;
            say(the_talker_says("I'm on it,")).await;

            // The talker draws breath and THEN asks, which is the ordinary
            // shape of a Live call (ADR 0191).
            tokio::time::sleep(A_BEAT).await;
            say(asks_for_the_doer("check what is waiting")).await;
            until("the doer to be woken", || !woken.lock().unwrap().is_empty()).await;

            // The rest of the sentence lands after the work started.
            say(the_talker_is_saying(" give me a sec.")).await;
            say(the_talker_says("give me a sec.")).await;
            until("the second stretch to reach the caller", || {
                transcripts() == 2
            })
            .await;
            hang_up.notify_one();
        }
    );

    // Both turns are their own row, because each one's words were final when
    // the provider ended it.
    let replies = rows_of(&pool, thread_id, "SpokenReplyGenerated").await;
    assert_eq!(replies.len(), 2, "{:?}", replies);
    assert_eq!(replies[0].0["text"], "I'm on it,");
    assert_eq!(replies[1].0["text"], "give me a sec.");

    let delegated = rows_of(&pool, thread_id, "WorkDelegated").await;
    assert_eq!(delegated.len(), 1, "{:?}", delegated);
    let started_working = delegated[0].1;

    // So the stall reads ABOVE the turn it promised, which is where a reader
    // meets it. By `created` alone, with no second clock to consult.
    assert!(
        replies[0].1 < started_working,
        "the stall was written at {} and the work started {}",
        replies[0].1,
        started_working
    );
    // And the tail that really did land later reads later.
    assert!(
        started_working < replies[1].1,
        "the tail was written before the work it followed"
    );

    // Neither row claims to have begun before the one it followed, and
    // neither begins after its own words stopped (ADR 0206).
    for reply in &replies {
        assert!(
            began_speaking(reply) <= reply.1,
            "a reply begun at {} was written at {}",
            began_speaking(reply),
            reply.1
        );
    }
    assert!(
        replies[0].1 <= began_speaking(&replies[1]),
        "the tail claims to have begun at {}, before the stall ended at {}",
        began_speaking(&replies[1]),
        replies[0].1
    );

    teardown_test_db(&db_name).await;
}

/// When the talker began the words a row carries, on the row's own clock.
///
/// `created` is when they stopped, and the age says how long before that they
/// started. The two are subtracted here exactly as the transcript subtracts
/// them (ADR 0206).
fn began_speaking(reply: &(serde_json::Value, DateTime<Utc>)) -> DateTime<Utc> {
    let secs = reply.0["spoken_secs_before"]
        .as_f64()
        .unwrap_or_else(|| panic!("the row says how long it had been speaking: {:?}", reply.0));
    reply.1 - chrono::Duration::microseconds((secs * 1_000_000.0) as i64)
}

/// **The reported re-arrangement.** The talker began `Good idea. I'm on it.`
/// before the doer's first step and stopped after it. Placed by `created`
/// alone, the row therefore filed under the work, and the bubble jumped down
/// two rows as it landed.
///
/// A spoken row covers a stretch of time, while every step beside it is an
/// instant. So the row says how long the talker had been speaking (ADR 0206).
#[tokio::test]
async fn a_reply_says_how_long_it_had_been_speaking() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = a_chat_thread(&pool).await;

    let (provider, talker) = MockVoiceProvider::driven();
    let log = provider.log();
    let turns = RecordingTurns::default();
    let woken = turns.woken();
    let hang_up = Arc::new(Notify::new());
    let mut caller = ScriptedCaller::new(vec![]).hanging_up_on(Arc::clone(&hang_up));
    let sent = Arc::clone(&caller.sent);

    tokio::join!(
        run_call(
            &bus,
            &provider,
            &mut caller,
            &turns,
            free_doer(),
            nobody_names(),
            opening(),
            subject(thread_id, uuid::Uuid::new_v4()),
        ),
        async {
            until("the session to open", || {
                log.lock().unwrap().openings.len() == 1
            })
            .await;
            let say = |event| async {
                talker.send(event).await.expect("the talker is listening");
            };
            let transcripts = || {
                sent.lock()
                    .unwrap()
                    .iter()
                    .filter(|f| matches!(f, ServerFrame::TalkerTranscript { .. }))
                    .count()
            };

            say(the_caller_says("Make the thread title better")).await;
            say(the_talker_is_saying("Good idea.")).await;
            until("the first words to reach the caller", || transcripts() == 1).await;

            // The work starts while the talker is still speaking, and its
            // first step is the row the bubble jumped under.
            tokio::time::sleep(A_BEAT).await;
            say(asks_for_the_doer("improve the thread title")).await;
            until("the doer to be woken", || !woken.lock().unwrap().is_empty()).await;

            // The rest of one sentence, said after the work began.
            tokio::time::sleep(A_BEAT).await;
            say(the_talker_is_saying(" I'm on it.")).await;
            until("the rest to reach the caller", || transcripts() == 2).await;
            say(the_talker_says("Good idea. I'm on it.")).await;
            hang_up.notify_one();
        }
    );

    let replies = rows_of(&pool, thread_id, "SpokenReplyGenerated").await;
    assert_eq!(replies.len(), 1, "{:?}", replies);
    assert_eq!(replies[0].0["text"], "Good idea. I'm on it.");
    let delegated = rows_of(&pool, thread_id, "WorkDelegated").await;
    assert_eq!(delegated.len(), 1, "{:?}", delegated);
    let started_working = delegated[0].1;

    // The words stopped after the work started, which is why `created` alone
    // cannot place this row.
    assert!(
        started_working < replies[0].1,
        "the reply was written at {} and the work started {}",
        replies[0].1,
        started_working
    );
    // And they began before it, which is where the reader heard them.
    assert!(
        began_speaking(&replies[0]) < started_working,
        "the reply began at {} and the work started {}",
        began_speaking(&replies[0]),
        started_working
    );
    // The stamp is the FIRST delta's, so the gap inside this one turn is
    // inside the age too. Taken at the last delta it would be near zero.
    let age = replies[0].0["spoken_secs_before"].as_f64().expect("an age");
    assert!(
        age >= A_BEAT.as_secs_f64(),
        "the reply was said over {}s, which is shorter than the gap inside it",
        age
    );

    teardown_test_db(&db_name).await;
}

/// A provider that streams no deltas still writes its reply.
///
/// Its turn's own transcript is the whole account of what was said, so it is
/// what the row carries. Dropped, such a reply would leave no record at all of
/// what the caller heard.
///
/// **And it carries no age.** Nothing timed those words, so the row reads at
/// its own `created`, which is where every reply read before ADR 0206.
#[tokio::test]
async fn a_reply_with_no_deltas_is_still_written() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = a_chat_thread(&pool).await;

    a_call_that_hears(
        &pool,
        &bus,
        thread_id,
        uuid::Uuid::new_v4(),
        vec![
            the_caller_says("What's the status"),
            the_talker_says("Nothing is waiting on you."),
            the_caller_says("Thanks"),
        ],
    )
    .await;

    let replies = rows_of(&pool, thread_id, "SpokenReplyGenerated").await;
    assert_eq!(replies.len(), 1, "{:?}", replies);
    assert_eq!(replies[0].0["text"], "Nothing is waiting on you.");
    assert!(
        replies[0].0.get("spoken_secs_before").is_none(),
        "{:?}",
        replies[0].0
    );

    teardown_test_db(&db_name).await;
}

/// The caller's frame says exactly what the row beside it says.
///
/// Both are one provider turn, so the bubble the client draws is replaced by a
/// row carrying the same words. Sent a stretch the row did not match, the
/// reader was left with a stray bubble and a "Requesting" header under it.
#[tokio::test]
async fn the_callers_frame_matches_the_row_it_becomes() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = a_chat_thread(&pool).await;

    let provider = MockVoiceProvider::new(vec![
        the_caller_says("Let's try this for a"),
        the_caller_says("bit"),
    ]);
    let mut caller = ScriptedCaller::new(vec![]).hanging_up_after(3);
    let sent = Arc::clone(&caller.sent);

    run_call(
        &bus,
        &provider,
        &mut caller,
        &RecordingTurns::default(),
        free_doer(),
        nobody_names(),
        opening(),
        subject(thread_id, uuid::Uuid::new_v4()),
    )
    .await;

    let captions: Vec<String> = sent
        .lock()
        .unwrap()
        .iter()
        .filter_map(|frame| match frame {
            ServerFrame::UserTurnEnded { transcript } => Some(transcript.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(
        captions,
        vec!["Let's try this for a".to_string(), "bit".to_string()]
    );
    // One row per frame, saying the same thing. The transcript joins the two.
    let rows = voice_rows(&thread_events(&pool, thread_id).await);
    assert_eq!(rows.len(), 2, "{:?}", rows);
    assert_eq!(rows[0].1["text"], "Let's try this for a");
    assert_eq!(rows[1].1["text"], "bit");

    teardown_test_db(&db_name).await;
}

/// A caller who cuts in mid-reply still reads above the answer they cut into.
///
/// Their own next words are the move, and it writes both rows. Writing the
/// reply's first would put every answer above the question it answered.
///
/// The `barge_in` control is what makes those words a move at all. Without one
/// they are the caller finishing a sentence the talker talked over, and the
/// case below says what happens then.
#[tokio::test]
async fn a_barge_in_writes_the_question_before_the_answer() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = a_chat_thread(&pool).await;

    let (provider, talker) = MockVoiceProvider::driven();
    let log = provider.log();
    let (mut caller, from_caller) = ScriptedCaller::driven();
    let sent = Arc::clone(&caller.sent);
    let turns = RecordingTurns::default();

    tokio::join!(
        run_call(
            &bus,
            &provider,
            &mut caller,
            &turns,
            free_doer(),
            nobody_names(),
            opening(),
            subject(thread_id, uuid::Uuid::new_v4()),
        ),
        async {
            until("the session to open", || {
                log.lock().unwrap().openings.len() == 1
            })
            .await;
            say(&talker, the_caller_says("what is on today")).await;
            await_frames(&sent, 1, "the caller's words to land").await;
            say(&talker, the_talker_is_saying("Two meetings and")).await;
            await_frames(&sent, 2, "the talker to take the floor").await;

            cut_in(&from_caller).await;
            until("the talker to be cancelled", || {
                log.lock().unwrap().cancels == 1
            })
            .await;

            say(&talker, the_caller_says("and tomorrow")).await;
            await_frames(&sent, 3, "their next words to land").await;
            hang_up(&from_caller).await;
        }
    );

    let events = thread_events(&pool, thread_id).await;
    assert_eq!(
        spoken_kinds(&events),
        vec![
            "SpokenMessageReceived",
            "SpokenReplyGenerated",
            "SpokenMessageReceived",
        ],
        "{:?}",
        spoken_kinds(&events)
    );

    teardown_test_db(&db_name).await;
}

/// A cut that wrote no row does not mark the NEXT reply as cut.
///
/// The flag belongs to the reply the caller cut into. That reply is over at
/// its own turn end, whether or not it had words to write. A provider
/// cancelling a response before it produced any reports exactly that shape.
#[tokio::test]
async fn a_cut_that_wrote_nothing_does_not_cut_the_next_reply() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = a_chat_thread(&pool).await;

    let did = a_call_that_hears(
        &pool,
        &bus,
        thread_id,
        uuid::Uuid::new_v4(),
        vec![
            the_caller_opens_the_floor(),
            // A response cancelled before it said anything, so its turn end
            // reports no words and no row is owed.
            VoiceEvent::Interrupted,
            the_talker_says(""),
            // The next reply is heard in full.
            the_talker_is_saying("Two things are running."),
            the_talker_says("Two things are running."),
        ],
    )
    .await;

    let said = replies(&did.events);
    assert_eq!(said.len(), 1, "{:?}", said);
    assert_eq!(
        said[0]["interrupted"], false,
        "a reply nobody cut into reads as cut: {:?}",
        said[0]
    );

    teardown_test_db(&db_name).await;
}

/// Every talker turn's spend is recorded, including one whose row is already
/// down.
///
/// A cut reply writes its row at the caller's own turn end, before the
/// provider has reported what the turn cost. That report arrives at the turn
/// end, which owes no second row: read there for the row alone, the number
/// went nowhere and the call under-reported its spend.
#[tokio::test]
async fn a_cut_reply_still_reports_what_its_turn_cost() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = a_chat_thread(&pool).await;

    let did = a_call_that_hears(
        &pool,
        &bus,
        thread_id,
        uuid::Uuid::new_v4(),
        vec![
            the_caller_opens_the_floor(),
            the_talker_is_saying("Two things are"),
            // They take the floor, so the row goes down now.
            VoiceEvent::Interrupted,
            the_caller_says("wait"),
            // And the provider reports the turn afterwards.
            the_talker_says("Two things are running."),
        ],
    )
    .await;

    let said = replies(&did.events);
    assert_eq!(said.len(), 1, "{:?}", said);
    assert_eq!(said[0]["interrupted"], true, "{:?}", said[0]);
    // One turn, one usage row, carrying what that turn reported.
    assert_eq!(
        voice_captures(&did.events),
        1,
        "{:?}",
        did.events
            .iter()
            .filter(|(kind, _)| kind == "ContextCaptured")
            .collect::<Vec<_>>()
    );
    let spend: i64 = did
        .events
        .iter()
        .filter(|(kind, payload)| kind == "ContextCaptured" && payload["purpose"] == "voice")
        .filter_map(|(_, payload)| payload["usage"]["output_tokens"].as_i64())
        .sum();
    assert!(spend > 0, "the turn's spend reached no usage row");

    teardown_test_db(&db_name).await;
}

/// The other half of the reported defect: those two rows are one sentence.
///
/// The engine writes what happened and the readers join it back. A breath is
/// not a turn, and it is not a message either.
#[tokio::test]
async fn the_doer_reads_that_breath_as_one_sentence() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = a_chat_thread(&pool).await;

    a_call_that_hears(
        &pool,
        &bus,
        thread_id,
        uuid::Uuid::new_v4(),
        a_sentence_the_talker_talked_over(),
    )
    .await;

    let store = crate::core::EventStore::new(pool.clone());
    let messages = store
        .get_thread_messages(&thread_id.to_string())
        .await
        .expect("the thread's messages read back");
    let said: Vec<&str> = messages.iter().map(|m| m.content.as_str()).collect();
    assert!(
        said.contains(&"Status, please"),
        "the breath reached the doer as {:?}",
        said
    );

    teardown_test_db(&db_name).await;
}

/// **The reported defect, on the engine's side.**
///
/// The caller said "Status, please" with a breath before the last word. The
/// talker answered into that breath, so the rest of their sentence arrived
/// while it was speaking. The exchange drew four rows.
///
/// Each piece is its own row, written when its own words stopped (ADR 0201).
/// The breath no longer cuts the reply in two, because only the caller TAKING
/// THE FLOOR ends it. Rejoining the two halves is a reading, and
/// [`the_doer_reads_that_breath_as_one_sentence`] is the other half of this.
#[tokio::test]
async fn a_breath_inside_one_sentence_is_two_rows_and_one_reply() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = a_chat_thread(&pool).await;

    let did = a_call_that_hears(
        &pool,
        &bus,
        thread_id,
        uuid::Uuid::new_v4(),
        a_sentence_the_talker_talked_over(),
    )
    .await;

    // Their words read above the answer to them, and the answer is ONE row:
    // the breath inside their sentence never closed it.
    assert_eq!(
        spoken_kinds(&did.events),
        vec![
            "SpokenMessageReceived",
            "SpokenMessageReceived",
            "SpokenReplyGenerated",
        ],
        "{:?}",
        spoken_kinds(&did.events)
    );
    let rows = voice_rows(&did.events);
    assert_eq!(rows[0].1["text"], "Status");
    assert_eq!(rows[1].1["text"], ", please");

    teardown_test_db(&db_name).await;
}

/// The other half of the same defect: the reply that breath landed inside.
///
/// It used to be cut at the word the caller's turn arrived on. The rest of the
/// same sentence was then emitted as a second row, after their words. To the
/// caller that is one reply with a hole punched in the middle.
#[tokio::test]
async fn a_reply_the_caller_talked_over_is_one_row() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = a_chat_thread(&pool).await;

    let did = a_call_that_hears(
        &pool,
        &bus,
        thread_id,
        uuid::Uuid::new_v4(),
        a_sentence_the_talker_talked_over(),
    )
    .await;

    let said = replies(&did.events);
    assert_eq!(said.len(), 1, "{:?}", said);
    assert_eq!(
        said[0]["text"],
        "Still in it. I'm pulling the threads together."
    );
    // Nobody cut it off. It ran to its own full stop.
    assert_eq!(said[0]["interrupted"], false);

    teardown_test_db(&db_name).await;
}

/// The script both cases above read, which is the recorded call.
///
/// The talker answers into a mid-sentence pause, the caller finishes their
/// sentence over the reply, and the reply carries on to its full stop.
fn a_sentence_the_talker_talked_over() -> Vec<VoiceEvent> {
    vec![
        the_caller_says("Status"),
        the_talker_is_saying("Still "),
        the_caller_says(", please"),
        the_talker_is_saying("in it. I'm pulling the threads together."),
        the_talker_says("Still in it. I'm pulling the threads together."),
    ]
}

/// **A barge-in cuts the talker off, and the cut is the engine's to make.**
///
/// A Live talker cannot be cancelled: its API has no such frame, so it keeps
/// speaking. The client throws its own queue away, so without this the caller
/// hears a hole and then the rest of the reply.
#[tokio::test]
async fn a_barge_in_still_cuts_the_talker_off() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = a_chat_thread(&pool).await;

    let cut = a_reply_the_caller_cuts_into(&bus, thread_id, 1).await;

    assert_eq!(cut.cancels, 1);
    let events = thread_events(&pool, thread_id).await;
    let said = replies(&events);
    assert_eq!(said.len(), 1, "{:?}", said);
    assert_eq!(said[0]["text"], "A traveler finds a tiny key");
    assert_eq!(said[0]["interrupted"], true);

    teardown_test_db(&db_name).await;
}

/// Nothing of a reply the caller cut off reaches them, or its row.
///
/// The talker goes on composing for a beat after it hears them. Forwarded,
/// that beat is the fragment they hear land after their own words. Written
/// down, it is a row claiming they heard it.
#[tokio::test]
async fn nothing_of_a_cut_reply_reaches_the_caller() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = a_chat_thread(&pool).await;

    let cut = a_reply_the_caller_cuts_into(&bus, thread_id, 1).await;

    assert_eq!(cut.audio_out_bytes, 0, "the caller heard the rest of it");
    let events = thread_events(&pool, thread_id).await;
    let said = replies(&events);
    let text = said[0]["text"].as_str().unwrap_or_default();
    assert!(
        !text.contains("opens"),
        "the row carries words nobody heard"
    );

    teardown_test_db(&db_name).await;
}

/// A cut reply gets ONE row, and never a second one carrying its tail.
///
/// The turn end reports the whole reply, tail included, and it lands after the
/// caller's own words have already closed the row. Taken as the next stretch's
/// fallback, it wrote the reply again, in full, saying nobody cut it off.
#[tokio::test]
async fn a_cut_reply_is_not_written_again_in_full() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = a_chat_thread(&pool).await;

    let (provider, talker) = MockVoiceProvider::driven();
    let log = provider.log();
    let (mut caller, from_caller) = ScriptedCaller::driven();
    let sent = Arc::clone(&caller.sent);
    let turns = RecordingTurns::default();

    tokio::join!(
        run_call(
            &bus,
            &provider,
            &mut caller,
            &turns,
            free_doer(),
            nobody_names(),
            opening(),
            subject(thread_id, uuid::Uuid::new_v4()),
        ),
        async {
            until("the session to open", || {
                log.lock().unwrap().openings.len() == 1
            })
            .await;
            // A call opens silent, so the floor opens before the talker
            // says anything anybody hears (ADR 0211).
            say(&talker, the_caller_opens_the_floor()).await;
            await_frames(&sent, 1, "the caller's first word").await;
            say(&talker, the_talker_is_saying("Two meetings and")).await;
            await_frames(&sent, 2, "the talker to take the floor").await;

            cut_in(&from_caller).await;
            until("the talker to be cancelled", || {
                log.lock().unwrap().cancels == 1
            })
            .await;

            // Their words close the cut reply's row.
            say(&talker, the_caller_says("and tomorrow")).await;
            await_frames(&sent, 3, "their words to land").await;
            // The talker was still composing, and the turn end reports the lot.
            say(&talker, the_talker_is_saying(" three emails.")).await;
            say(&talker, the_talker_says("Two meetings and three emails.")).await;
            await_frames(&sent, 4, "the reply to end").await;
            hang_up(&from_caller).await;
        }
    );

    let said = replies(&thread_events(&pool, thread_id).await);
    assert_eq!(said.len(), 1, "the cut reply was written twice: {:?}", said);
    assert_eq!(said[0]["text"], "Two meetings and");
    assert_eq!(said[0]["interrupted"], true);

    teardown_test_db(&db_name).await;
}

/// A caller heard only as a PARTIAL still reads below the reply they spoke
/// over.
///
/// The end-of-call row is built from a finished stretch plus whatever partial
/// never closed, so both have to date it. Dated from the finished half alone,
/// a partial-only tail is ordered by a stretch already spent.
#[tokio::test]
async fn a_partial_the_caller_got_in_over_a_reply_reads_below_it() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = a_chat_thread(&pool).await;

    let (provider, talker) = MockVoiceProvider::driven();
    let log = provider.log();
    let (mut caller, from_caller) = ScriptedCaller::driven();
    let sent = Arc::clone(&caller.sent);
    let turns = RecordingTurns::default();
    let woken = turns.woken();

    tokio::join!(
        run_call(
            &bus,
            &provider,
            &mut caller,
            &turns,
            free_doer(),
            nobody_names(),
            opening(),
            subject(thread_id, uuid::Uuid::new_v4()),
        ),
        async {
            until("the session to open", || {
                log.lock().unwrap().openings.len() == 1
            })
            .await;
            say(&talker, the_caller_says("do the thing")).await;
            await_frames(&sent, 1, "their words to land").await;
            // The ask spends those words, so nothing of theirs is held.
            say(&talker, asks_for_the_doer("do the thing")).await;
            until("the doer to be woken", || !woken.lock().unwrap().is_empty()).await;

            say(&talker, the_talker_is_saying("On it")).await;
            await_frames(&sent, 2, "the stall to open").await;
            // A partial and nothing else, said over the stall.
            say(&talker, the_caller_is_saying("and quickly")).await;
            await_frames(&sent, 3, "their partial to land").await;
            hang_up(&from_caller).await;
        }
    );

    let events = thread_events(&pool, thread_id).await;
    // Their delegated words first, which is when they said them. Then the
    // stall and the partial that landed over it, both written at the close:
    // the talker still held the floor, so its reply began first.
    assert_eq!(
        spoken_kinds(&events),
        vec![
            "SpokenMessageReceived",
            "SpokenReplyGenerated",
            "SpokenMessageReceived",
        ],
        "{:?}",
        spoken_kinds(&events)
    );

    teardown_test_db(&db_name).await;
}

/// A cut the PROVIDER reports after the row went down writes no second one.
///
/// Realtime reports the cut beside the turn end, and the caller's own words
/// can land between the two. Read as a fresh cut, it walks the reply back to
/// owing a row it has already had.
#[tokio::test]
async fn a_cut_the_provider_reports_late_writes_no_second_row() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = a_chat_thread(&pool).await;

    let (provider, talker) = MockVoiceProvider::driven();
    let log = provider.log();
    let (mut caller, from_caller) = ScriptedCaller::driven();
    let sent = Arc::clone(&caller.sent);
    let turns = RecordingTurns::default();

    tokio::join!(
        run_call(
            &bus,
            &provider,
            &mut caller,
            &turns,
            free_doer(),
            nobody_names(),
            opening(),
            subject(thread_id, uuid::Uuid::new_v4()),
        ),
        async {
            until("the session to open", || {
                log.lock().unwrap().openings.len() == 1
            })
            .await;
            // A call opens silent, so the floor opens before the talker
            // says anything anybody hears (ADR 0211).
            say(&talker, the_caller_opens_the_floor()).await;
            await_frames(&sent, 1, "the caller's first word").await;
            say(&talker, the_talker_is_saying("Two meetings and")).await;
            await_frames(&sent, 2, "the talker to take the floor").await;
            cut_in(&from_caller).await;
            until("the talker to be cancelled", || {
                log.lock().unwrap().cancels == 1
            })
            .await;
            // Their words close the cut reply's row.
            say(&talker, the_caller_says("and tomorrow")).await;
            await_frames(&sent, 3, "their words to land").await;
            // Only now does the provider say what the caller already did.
            say(&talker, VoiceEvent::Interrupted).await;
            say(&talker, the_talker_says("Two meetings and three emails.")).await;
            await_frames(&sent, 5, "the reply to end").await;
            hang_up(&from_caller).await;
        }
    );

    let said = replies(&thread_events(&pool, thread_id).await);
    assert_eq!(said.len(), 1, "the cut reply was written twice: {:?}", said);
    assert_eq!(said[0]["text"], "Two meetings and");

    teardown_test_db(&db_name).await;
}

/// Nor does the teardown, for the turn end the socket was still holding.
///
/// The drain takes everything, because nothing is coming after the socket. A
/// cut reply is the one thing it is owed nothing for: the row it would open
/// carries the tail the caller was never played.
#[tokio::test]
async fn the_teardown_writes_no_second_row_for_a_cut_reply() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = a_chat_thread(&pool).await;

    let (provider, talker) = MockVoiceProvider::driven();
    let provider = provider.still_holding(vec![the_talker_says("Two meetings and three emails.")]);
    let log = provider.log();
    let (mut caller, from_caller) = ScriptedCaller::driven();
    let sent = Arc::clone(&caller.sent);
    let turns = RecordingTurns::default();

    tokio::join!(
        run_call(
            &bus,
            &provider,
            &mut caller,
            &turns,
            free_doer(),
            nobody_names(),
            opening(),
            subject(thread_id, uuid::Uuid::new_v4()),
        ),
        async {
            until("the session to open", || {
                log.lock().unwrap().openings.len() == 1
            })
            .await;
            // A call opens silent, so the floor opens before the talker
            // says anything anybody hears (ADR 0211).
            say(&talker, the_caller_opens_the_floor()).await;
            await_frames(&sent, 1, "the caller's first word").await;
            say(&talker, the_talker_is_saying("Two meetings and")).await;
            await_frames(&sent, 2, "the talker to take the floor").await;
            cut_in(&from_caller).await;
            until("the talker to be cancelled", || {
                log.lock().unwrap().cancels == 1
            })
            .await;
            say(&talker, the_caller_says("and tomorrow")).await;
            await_frames(&sent, 3, "their words to land").await;
            hang_up(&from_caller).await;
        }
    );

    let said = replies(&thread_events(&pool, thread_id).await);
    assert_eq!(said.len(), 1, "the teardown wrote it again: {:?}", said);
    assert_eq!(said[0]["text"], "Two meetings and");

    teardown_test_db(&db_name).await;
}

/// A caller still making words over the goodbye keeps the call, with no
/// barge-in needed.
///
/// A Live talker reports no interruption of its own, and the client sends no
/// cut for somebody who never stopped. So their partial is the only thing that
/// reaches a goodbye the talker began over the top of them.
#[tokio::test]
async fn a_caller_still_talking_over_the_goodbye_keeps_the_call() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = a_chat_thread(&pool).await;

    let provider = MockVoiceProvider::new(vec![
        VoiceEvent::HangupRequested {
            tool_call_id: "call_h".to_string(),
        },
        the_talker_is_saying("Speak soon"),
        // They never stopped, so no cut is sent and none is reported.
        the_caller_is_saying("no wait"),
        the_talker_says("Speak soon."),
    ]);
    // The opening frame, the goodbye's delta, their partial and the turn end.
    let mut caller = ScriptedCaller::new(vec![]).hanging_up_after(4);

    let reason = run_call(
        &bus,
        &provider,
        &mut caller,
        &RecordingTurns::default(),
        free_doer(),
        nobody_names(),
        opening(),
        subject(thread_id, uuid::Uuid::new_v4()),
    )
    .await;

    assert_eq!(
        reason,
        Some(VoiceSessionEndReason::Hangup),
        "the call rang off over somebody mid-sentence"
    );

    teardown_test_db(&db_name).await;
}

/// One barge-in, one cut. The gate opens on a run of loud frames, so a caller
/// talking through a reply raises several edges for the one interruption.
#[tokio::test]
async fn one_barge_in_cancels_once() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = a_chat_thread(&pool).await;

    let cut = a_reply_the_caller_cuts_into(&bus, thread_id, 3).await;

    assert_eq!(cut.cancels, 1);

    teardown_test_db(&db_name).await;
}

/// A barge-in with nobody speaking cuts nothing, and asks for nothing.
///
/// There is no reply to stop, and a cancel for a response that does not exist
/// is refused. On the Realtime path that refusal reads as the session dying.
#[tokio::test]
async fn a_barge_in_with_nobody_speaking_cuts_nothing() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = a_chat_thread(&pool).await;

    let cut = a_reply_the_caller_cuts_into(&bus, thread_id, 0).await;

    assert_eq!(cut.cancels, 0);

    teardown_test_db(&db_name).await;
}

/// What a call the caller cut into left behind, on the seam's own side.
struct WhatTheCutDid {
    cancels: usize,
    /// Talker audio bytes that reached the caller after the cut.
    audio_out_bytes: usize,
}

/// Drive one reply and have the caller cut into it `barge_ins` times.
///
/// `0` sends one control with the talker still quiet, which is the case that
/// must cancel nothing.
///
/// The talker keeps composing through the cut, which is what a Live one does.
/// So the events after the barge-in are what this is really about.
async fn a_reply_the_caller_cuts_into(
    bus: &EventBus,
    thread_id: uuid::Uuid,
    barge_ins: usize,
) -> WhatTheCutDid {
    let (provider, talker) = MockVoiceProvider::driven();
    let log = provider.log();
    let (mut caller, from_caller) = ScriptedCaller::driven();
    let sent = Arc::clone(&caller.sent);
    let audio_out = Arc::clone(&caller.audio_out_bytes);
    let quiet = barge_ins == 0;
    let turns = RecordingTurns::default();

    tokio::join!(
        run_call(
            bus,
            &provider,
            &mut caller,
            &turns,
            free_doer(),
            nobody_names(),
            opening(),
            subject(thread_id, uuid::Uuid::new_v4()),
        ),
        async {
            until("the session to open", || {
                log.lock().unwrap().openings.len() == 1
            })
            .await;
            // A reply nobody can hear cannot be cut into, so the floor opens
            // first (ADR 0211).
            say(&talker, the_caller_opens_the_floor()).await;
            await_frames(&sent, 1, "the caller's first word").await;
            if !quiet {
                say(&talker, the_talker_is_saying("A traveler finds a tiny key")).await;
                await_frames(&sent, 2, "the talker to take the floor").await;
            }

            for _ in 0..barge_ins.max(1) {
                cut_in(&from_caller).await;
            }
            // The loop reads the caller in order, so audio it acknowledges
            // proves every control before it was handled.
            from_caller
                .send(CallerFrame::Audio(vec![7]))
                .await
                .expect("the call is listening");
            until("the caller's audio to go up", || {
                log.lock().unwrap().audio_in_bytes == 1
            })
            .await;
            // Everything from here is the tail the caller never heard.
            say(&talker, VoiceEvent::Audio(vec![1, 2, 3])).await;
            say(
                &talker,
                the_talker_is_saying(" and doesn't know what it opens"),
            )
            .await;
            say(
                &talker,
                the_talker_says("A traveler finds a tiny key and doesn't know what it opens"),
            )
            .await;
            await_frames(&sent, 3, "the reply to end").await;
            hang_up(&from_caller).await;
        }
    );

    let cancels = log.lock().unwrap().cancels;
    let audio_out_bytes = *audio_out.lock().unwrap();
    WhatTheCutDid {
        cancels,
        audio_out_bytes,
    }
}

/// Hand the talker one event, and wait for nothing.
async fn say(talker: &tokio::sync::mpsc::Sender<VoiceEvent>, event: VoiceEvent) {
    talker.send(event).await.expect("the talker is listening");
}

/// The caller taking the floor back.
async fn cut_in(from_caller: &tokio::sync::mpsc::Sender<CallerFrame>) {
    from_caller
        .send(CallerFrame::Control(ClientControl::BargeIn))
        .await
        .expect("the call is listening");
}

async fn hang_up(from_caller: &tokio::sync::mpsc::Sender<CallerFrame>) {
    from_caller
        .send(CallerFrame::Control(ClientControl::HangUp))
        .await
        .expect("the call is listening");
}

/// Wait until `count` text frames past the opening one have reached the caller.
async fn await_frames(sent: &Arc<Mutex<Vec<ServerFrame>>>, count: usize, what: &str) {
    until(what, || sent.lock().unwrap().len() > count).await;
}

/// A reply the caller rang off in the middle of is still marked cut off.
///
/// The talker holds the floor until a pause lands, so holding it at the close
/// IS the caller leaving mid-reply. Marking every last reply would be a claim
/// about the caller that is usually false.
#[tokio::test]
async fn a_reply_the_caller_rang_off_inside_is_marked_cut_off() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = a_chat_thread(&pool).await;

    let did = a_call_that_hears(
        &pool,
        &bus,
        thread_id,
        uuid::Uuid::new_v4(),
        // Words, and no pause after them.
        vec![
            the_caller_opens_the_floor(),
            the_talker_is_saying("Two things are ru"),
        ],
    )
    .await;

    let said = replies(&did.events);
    assert_eq!(said.len(), 1, "{:?}", said);
    assert_eq!(said[0]["text"], "Two things are ru");
    assert_eq!(said[0]["interrupted"], true);

    teardown_test_db(&db_name).await;
}

/// The doer is RUN on the same sentence it reads back.
///
/// Two fragments of one breath reach the wake joined, and the history joins
/// them the same way. A plain space between every piece put `Status , please`
/// in the prompt and `Status, please` in the history.
#[tokio::test]
async fn the_doer_runs_on_the_sentence_its_history_shows() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = a_chat_thread(&pool).await;

    let did = a_call_that_hears(
        &pool,
        &bus,
        thread_id,
        uuid::Uuid::new_v4(),
        vec![
            the_caller_says("Status"),
            the_caller_says(", please"),
            asks_for_the_doer("they want the status"),
            the_talker_says("Checking."),
        ],
    )
    .await;

    assert_eq!(did.woken, vec!["Status, please".to_string()]);

    let store = crate::core::EventStore::new(pool.clone());
    let messages = store
        .get_thread_messages(&thread_id.to_string())
        .await
        .expect("the thread's messages read back");
    assert!(
        messages.iter().any(|m| m.content == "Status, please"),
        "{:?}",
        messages.iter().map(|m| &m.content).collect::<Vec<_>>()
    );

    teardown_test_db(&db_name).await;
}

/// The talker asked, so the doer runs on what the caller said.
///
/// Two rows and no more: what the caller said, then what the talker asked for.
/// No `MessageReceived`, because the caller's words are already recorded and a
/// second row would put the same sentence in the store twice (ADR 0201).
#[tokio::test]
async fn a_delegated_utterance_wakes_the_doer_exactly_once() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = a_chat_thread(&pool).await;
    let session_id = uuid::Uuid::new_v4();

    let did = a_call_that_hears(
        &pool,
        &bus,
        thread_id,
        session_id,
        vec![
            the_caller_says("what have I got running"),
            asks_for_the_doer("they want today's threads"),
            the_talker_says("Let me check."),
        ],
    )
    .await;

    assert_eq!(did.woken, vec!["what have I got running".to_string()]);

    let rows = voice_rows(&did.events);
    assert_eq!(
        rows.len(),
        2,
        "the utterance was recorded twice: {:?}",
        rows
    );
    assert_eq!(rows[0].0, "SpokenMessageReceived");
    assert_eq!(rows[0].1["text"], "what have I got running");
    assert_eq!(rows[1].0, "WorkDelegated");
    assert_eq!(rows[1].1["reason"], "they want today's threads");
    assert_eq!(rows[1].1["session_id"], session_id.to_string());
    // Authored by the talker, so the thread names all three participants.
    assert_eq!(rows[1].1["actor"]["kind"], "agent");
    assert_eq!(rows[1].1["actor"]["agent"]["kind"], "guest");
    // And the turn anchors on that delegation, which is its starter.
    assert_eq!(did.anchors.len(), 1, "{:?}", did.anchors);

    teardown_test_db(&db_name).await;
}

/// A doer that will not take the utterance owes the caller two things.
///
/// The words go in the thread, so a `WorkDelegated` is never left beside no
/// record of what was said. And the talker is told, because a caller waiting
/// in silence for an answer that is never coming cannot recover on their own.
///
/// The shipping refusal is a thread a call cannot reach (ADR 0165). What the
/// doer refuses for is its own business, so this drives the seam instead.
#[tokio::test]
async fn a_refused_utterance_is_written_down_and_said_out_loud() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = a_chat_thread(&pool).await;
    let session_id = uuid::Uuid::new_v4();

    let provider = MockVoiceProvider::new(vec![
        the_caller_says("book it for tuesday"),
        asks_for_the_doer("they want a booking"),
    ]);
    let log = provider.log();
    // Signalled rather than counted. The delegation reaches the caller as no
    // frame at all. A count would therefore ring off on the transcript before
    // it, and the refusal under test would never run.
    let hang_up = Arc::new(Notify::new());
    let mut caller = ScriptedCaller::new(vec![]).hanging_up_on(hang_up.clone());
    let turns = RecordingTurns::refusing();

    tokio::join!(
        run_call(
            &bus,
            &provider,
            &mut caller,
            &turns,
            free_doer(),
            nobody_names(),
            opening(),
            subject(thread_id, session_id),
        ),
        async {
            until("the refusal to be handed to the talker", || {
                log.lock().unwrap().asked_to_speak.len() == 1
            })
            .await;
            hang_up.notify_one();
        }
    );

    let rows = voice_rows(&thread_events(&pool, thread_id).await);
    // The words were written when they were said, and the delegation still
    // happened: the talker asked, and the engine is what refused. What must
    // NOT be here is a second copy of the sentence.
    assert_eq!(
        voice_kinds(&rows),
        vec![
            "SpokenMessageReceived".to_string(),
            "WorkDelegated".to_string()
        ],
        "{:?}",
        rows
    );
    assert_eq!(rows[0].1["text"], "book it for tuesday");
    assert_eq!(rows[0].1["session_id"], session_id.to_string());

    // Scoped, so the guard is gone before the teardown's await.
    {
        let log = log.lock().unwrap();
        assert_eq!(log.asked_to_speak.len(), 1, "{:?}", log.asked_to_speak);
        assert!(
            log.asked_to_speak[0].contains("could not be started"),
            "{}",
            log.asked_to_speak[0]
        );
    }

    teardown_test_db(&db_name).await;
}

/// The ordering hazard, both ways round. The transcript and the tool call come
/// from two models on one socket, and a short fast reply produces the call
/// first. Either order wakes the doer once, with the same rows.
#[tokio::test]
async fn the_wake_does_not_care_which_frame_lands_first() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let transcript_first = vec![
        the_caller_says("book it for tuesday"),
        asks_for_the_doer("they want a booking"),
    ];
    let call_first = vec![
        asks_for_the_doer("they want a booking"),
        the_caller_says("book it for tuesday"),
    ];

    for script in [transcript_first, call_first] {
        let thread_id = a_chat_thread(&pool).await;
        let did = a_call_that_hears(&pool, &bus, thread_id, uuid::Uuid::new_v4(), script).await;

        assert_eq!(did.woken, vec!["book it for tuesday".to_string()]);
        let kinds = voice_kinds(&did.events);
        assert_eq!(
            kinds,
            vec!["SpokenMessageReceived", "WorkDelegated"],
            "{:?}",
            kinds
        );
    }

    teardown_test_db(&db_name).await;
}

/// A tool call whose transcript is still in flight when the talker stops
/// speaking. The ask OUTLIVES that turn, so the question still runs.
///
/// Clearing it at the turn's end would write the caller's real question down
/// as a row that starts nothing, which is silence on the phone.
#[tokio::test]
async fn an_ask_outlives_the_turn_that_made_it() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = a_chat_thread(&pool).await;

    let did = a_call_that_hears(
        &pool,
        &bus,
        thread_id,
        uuid::Uuid::new_v4(),
        vec![
            asks_for_the_doer("they want a booking"),
            the_talker_says("On it."),
            the_caller_says("book it for tuesday"),
        ],
    )
    .await;

    assert_eq!(did.woken, vec!["book it for tuesday".to_string()]);
    let kinds = voice_kinds(&did.events);
    assert_eq!(
        kinds,
        vec!["SpokenMessageReceived", "WorkDelegated"],
        "{:?}",
        kinds
    );

    teardown_test_db(&db_name).await;
}

/// **The lost-ask regression, and the other ordering of the race above.** The
/// talker says a word, draws breath, and THEN asks for the doer.
///
/// The words the ask needs are the ones the caller just said, and the pause
/// used to write them down and take them. The ask then had nothing to pair
/// with, so it waited in `pending_delegation` until the hangup: no
/// `WorkDelegated`, no turn, and a talker that had been told `Taken.` telling
/// the caller it was on it. A pause spends nothing. See
/// `docs/plans/2026-09-16-a-pause-spends-nothing.md`.
#[tokio::test]
async fn an_ask_after_the_pause_still_wakes_the_doer() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = a_chat_thread(&pool).await;

    let did = a_call_that_hears(
        &pool,
        &bus,
        thread_id,
        uuid::Uuid::new_v4(),
        vec![
            the_caller_says("yeah"),
            // The whole of the reported shape: one word, then the hole.
            the_talker_says("Okay."),
            asks_for_the_doer("they want it recorded"),
        ],
    )
    .await;

    assert_eq!(did.woken, vec!["yeah".to_string()]);
    let kinds = voice_kinds(&did.events);
    // And it is the delegated shape, not the answered-alone one. A
    // `SpokenMessageReceived` here would be the words spent on a row that
    // starts nothing, which is exactly how they were lost.
    assert_eq!(
        kinds,
        vec!["SpokenMessageReceived", "WorkDelegated"],
        "{:?}",
        kinds
    );

    teardown_test_db(&db_name).await;
}

/// A caller who speaks again while work runs. The talker asks a second time,
/// and that utterance reaches the doer too.
///
/// Whether it starts a turn or joins the running one is single-flight
/// admission's business, and the talker is never told which it got.
///
/// Each ask sits in its own talker turn, which is the only shape the provider
/// can produce: one response holds one reply, and a second reply needs the
/// first to have ended.
#[tokio::test]
async fn a_second_utterance_mid_turn_reaches_the_doer_too() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = a_chat_thread(&pool).await;

    let did = a_call_that_hears(
        &pool,
        &bus,
        thread_id,
        uuid::Uuid::new_v4(),
        vec![
            the_caller_says("what have I got running"),
            asks_for_the_doer("they want today's threads"),
            the_talker_says("Let me check."),
            the_caller_says("and tomorrow"),
            asks_for_the_doer("and tomorrow's"),
            the_talker_says("One moment."),
        ],
    )
    .await;

    assert_eq!(
        did.woken,
        vec![
            "what have I got running".to_string(),
            "and tomorrow".to_string()
        ]
    );
    let kinds = voice_kinds(&did.events);
    assert_eq!(
        kinds,
        vec![
            "SpokenMessageReceived",
            "WorkDelegated",
            "SpokenMessageReceived",
            "WorkDelegated"
        ],
        "{:?}",
        kinds
    );

    teardown_test_db(&db_name).await;
}

/// Two utterances, one delegated and one not. Each is recorded once, whoever
/// handles it, and only the delegated one runs a turn.
#[tokio::test]
async fn a_mixed_call_records_each_utterance_once() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = a_chat_thread(&pool).await;

    let did = a_call_that_hears(
        &pool,
        &bus,
        thread_id,
        uuid::Uuid::new_v4(),
        vec![
            the_caller_says("hei"),
            the_talker_says("Hei!"),
            the_caller_says("what have I got running"),
            asks_for_the_doer("they want today's threads"),
            the_talker_says("Let me check."),
        ],
    )
    .await;

    assert_eq!(did.woken, vec!["what have I got running".to_string()]);
    let kinds = voice_kinds(&did.events);
    assert_eq!(
        kinds,
        vec![
            "SpokenMessageReceived",
            "SpokenMessageReceived",
            "WorkDelegated"
        ],
        "{:?}",
        kinds
    );

    teardown_test_db(&db_name).await;
}

/// The talker is acknowledged whatever happens next. An unresolved call leaves
/// a dangling item in its history, which it reads as work it never heard about.
#[tokio::test]
async fn every_ask_is_acknowledged() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = a_chat_thread(&pool).await;

    // The ask is silent on the wire, so the hangup waits on the reply after
    // it. Otherwise it could ring off before the ask was ever read.
    let provider = MockVoiceProvider::new(vec![
        asks_for_the_doer("they want a booking"),
        the_talker_says("On it."),
    ]);
    let log = provider.log();
    let mut caller = ScriptedCaller::new(vec![]).hanging_up_after(2);

    run_call(
        &bus,
        &provider,
        &mut caller,
        &RecordingTurns::default(),
        free_doer(),
        nobody_names(),
        opening(),
        subject(thread_id, uuid::Uuid::new_v4()),
    )
    .await;

    let resolved = log.lock().unwrap().resolved_tool_calls.clone();
    assert_eq!(resolved.len(), 1, "{:?}", resolved);
    assert_eq!(resolved[0].0, "call_1");

    teardown_test_db(&db_name).await;
}

/// Buffering costs nothing when a call ends holding something. Every end
/// reason flushes, so a caller whose phone died still has their last words in
/// the thread.
///
/// Three reasons, one property. The hangup and the dropped socket both come
/// from the caller's end; the provider failure comes from the talker's.
#[tokio::test]
async fn a_call_that_drops_mid_utterance_loses_nothing() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    for ending in [
        VoiceSessionEndReason::Hangup,
        VoiceSessionEndReason::Disconnected,
        VoiceSessionEndReason::ProviderFailed,
    ] {
        let thread_id = a_chat_thread(&pool).await;
        let script = vec![the_caller_says("book it for tuesday")];
        // Two deliveries: SessionStarted, then the utterance's own frame. So
        // the end always lands with the utterance already held.
        let (provider, mut caller) = match ending {
            VoiceSessionEndReason::Hangup => (
                MockVoiceProvider::new(script),
                ScriptedCaller::new(vec![]).hanging_up_after(2),
            ),
            VoiceSessionEndReason::Disconnected => (
                MockVoiceProvider::new(script),
                ScriptedCaller::new(vec![]).dropping_after(2),
            ),
            // The talker goes quiet instead, and the caller waits.
            _ => (
                MockVoiceProvider::ending_after(script),
                ScriptedCaller::new(vec![]),
            ),
        };

        run_call(
            &bus,
            &provider,
            &mut caller,
            &RecordingTurns::default(),
            free_doer(),
            nobody_names(),
            opening(),
            subject(thread_id, uuid::Uuid::new_v4()),
        )
        .await;

        let events = thread_events(&pool, thread_id).await;
        let rows = voice_rows(&events);
        assert_eq!(
            rows.len(),
            1,
            "{:?} lost the utterance: {:?}",
            ending,
            events
        );
        assert_eq!(rows[0].0, "SpokenMessageReceived", "{:?}", ending);
        assert_eq!(rows[0].1["text"], "book it for tuesday", "{:?}", ending);
    }

    teardown_test_db(&db_name).await;
}

/// The same property for the talker, which is the half that lacked it. A call
/// ending mid-reply keeps the caller's words AND the answer they heard.
///
/// The same three reasons, because a teardown flush that covers only the
/// hangup is the hole again under another name. Both rows come out of teardown
/// here, so the order they land in is asserted too.
#[tokio::test]
async fn a_call_that_drops_mid_reply_keeps_what_the_caller_heard() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    for ending in [
        VoiceSessionEndReason::Hangup,
        VoiceSessionEndReason::Disconnected,
        VoiceSessionEndReason::ProviderFailed,
    ] {
        let thread_id = a_chat_thread(&pool).await;
        // No turn end anywhere: the talker is still speaking when it stops.
        let script = vec![
            the_caller_says("what have I got running"),
            the_talker_is_saying("Two things are "),
            the_talker_is_saying("running."),
        ];
        // Four deliveries: SessionStarted, then one frame per scripted event.
        // So the end always lands with the reply half said.
        let (provider, mut caller) = match ending {
            VoiceSessionEndReason::Hangup => (
                MockVoiceProvider::new(script),
                ScriptedCaller::new(vec![]).hanging_up_after(4),
            ),
            VoiceSessionEndReason::Disconnected => (
                MockVoiceProvider::new(script),
                ScriptedCaller::new(vec![]).dropping_after(4),
            ),
            // The talker drops the call instead, and the caller waits.
            _ => (
                MockVoiceProvider::ending_after(script),
                ScriptedCaller::new(vec![]),
            ),
        };

        run_call(
            &bus,
            &provider,
            &mut caller,
            &RecordingTurns::default(),
            free_doer(),
            nobody_names(),
            opening(),
            subject(thread_id, uuid::Uuid::new_v4()),
        )
        .await;

        let events = thread_events(&pool, thread_id).await;
        assert_eq!(
            spoken_kinds(&events),
            vec!["SpokenMessageReceived", "SpokenReplyGenerated"],
            "{:?} lost the reply or misordered it: {:?}",
            ending,
            events
        );
        let reply = events
            .iter()
            .find(|(kind, _)| kind == "SpokenReplyGenerated")
            .map(|(_, payload)| payload.clone())
            .expect("the reply row");
        assert_eq!(reply["text"], "Two things are running.", "{:?}", ending);
        // The caller left before the reply finished, so the row says so.
        assert_eq!(reply["interrupted"], true, "{:?}", ending);
        // The talker's own name, exactly as a finished reply carries.
        assert_eq!(reply["actor"]["agent"]["kind"], "guest", "{:?}", ending);
    }

    teardown_test_db(&db_name).await;
}

/// A reply the talker never started is not invented at teardown. Nothing was
/// heard, so nothing is written.
#[tokio::test]
async fn a_call_that_drops_before_a_word_writes_no_reply() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = a_chat_thread(&pool).await;

    let provider = MockVoiceProvider::new(vec![the_caller_says("what have I got running")]);
    let mut caller = ScriptedCaller::new(vec![]).hanging_up_after(2);

    run_call(
        &bus,
        &provider,
        &mut caller,
        &RecordingTurns::default(),
        free_doer(),
        nobody_names(),
        opening(),
        subject(thread_id, uuid::Uuid::new_v4()),
    )
    .await;

    let events = thread_events(&pool, thread_id).await;
    assert_eq!(
        spoken_kinds(&events),
        vec!["SpokenMessageReceived"],
        "a reply nobody heard was written down: {:?}",
        events
    );

    teardown_test_db(&db_name).await;
}

/// A reply the turn end already recorded is not recorded again at teardown.
/// One writer, so the held deltas go with it.
#[tokio::test]
async fn a_finished_reply_is_not_written_twice_by_the_teardown() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = a_chat_thread(&pool).await;

    let provider = MockVoiceProvider::new(vec![
        the_caller_says("what have I got running"),
        the_talker_is_saying("Two things are running."),
        the_talker_says("Two things are running."),
    ]);
    let mut caller = ScriptedCaller::new(vec![]).hanging_up_after(4);

    run_call(
        &bus,
        &provider,
        &mut caller,
        &RecordingTurns::default(),
        free_doer(),
        nobody_names(),
        opening(),
        subject(thread_id, uuid::Uuid::new_v4()),
    )
    .await;

    let events = thread_events(&pool, thread_id).await;
    assert_eq!(
        spoken_kinds(&events),
        vec!["SpokenMessageReceived", "SpokenReplyGenerated"],
        "{:?}",
        events
    );
    let reply = events
        .iter()
        .find(|(kind, _)| kind == "SpokenReplyGenerated")
        .map(|(_, payload)| payload.clone())
        .expect("the reply row");
    assert_eq!(reply["interrupted"], false);

    teardown_test_db(&db_name).await;
}

/// An empty transcript is not written down. A provider that reports a finished
/// utterance with no words would otherwise put a blank row in the thread.
#[tokio::test]
async fn an_utterance_with_no_words_is_not_written_down() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = a_chat_thread(&pool).await;

    let did = a_call_that_hears(
        &pool,
        &bus,
        thread_id,
        uuid::Uuid::new_v4(),
        vec![the_caller_says("   "), the_talker_says("Sorry?")],
    )
    .await;

    assert!(did.woken.is_empty());
    assert!(voice_rows(&did.events).is_empty(), "{:?}", did.events);

    teardown_test_db(&db_name).await;
}

/// Two `delegate` calls in one talker turn wake the doer once, and the second
/// does not survive into the next utterance.
///
/// A model that calls a tool twice is asking about one utterance. Kept, the
/// spare ask would delegate the NEXT thing the caller says, which is the
/// double answer this whole change removes.
#[tokio::test]
async fn asking_twice_in_one_turn_delegates_once() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = a_chat_thread(&pool).await;

    let did = a_call_that_hears(
        &pool,
        &bus,
        thread_id,
        uuid::Uuid::new_v4(),
        vec![
            the_caller_says("what have I got running"),
            asks_for_the_doer("they want today's threads"),
            asks_for_the_doer("they want today's threads"),
            the_talker_says("Let me check."),
            // The talker handles this one alone. A leftover ask would
            // delegate it anyway.
            the_caller_says("hei"),
            the_talker_says("Hei!"),
        ],
    )
    .await;

    assert_eq!(did.woken, vec!["what have I got running".to_string()]);
    let kinds = voice_kinds(&did.events);
    assert_eq!(
        kinds,
        vec![
            "SpokenMessageReceived",
            "WorkDelegated",
            "SpokenMessageReceived"
        ],
        "{:?}",
        kinds
    );

    teardown_test_db(&db_name).await;
}

/// An IGNORED duplicate started nothing, so the caller is still owed.
///
/// A talker that never says a word holds one turn for the whole call, and its
/// second ask is dropped. Disarming the bound on that ask left the caller's
/// second question with no ask and no net under it.
///
/// It waits out the real bound, for the reason
/// `a_caller_nobody_answers_reaches_the_doer_and_is_told_so` gives.
#[tokio::test]
async fn a_duplicate_ask_leaves_the_caller_owed_an_answer() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = a_chat_thread(&pool).await;

    let provider = MockVoiceProvider::new(vec![
        the_caller_says("what have I got running"),
        asks_for_the_doer("they want today's threads"),
        // A second question, and a second ask inside the same wordless turn.
        the_caller_says("and tomorrow"),
        asks_for_the_doer("and tomorrow's"),
    ]);
    let turns = RecordingTurns::default();
    let woken = turns.woken();
    let rang_off = Arc::new(Notify::new());
    let mut caller = ScriptedCaller::new(vec![]).hanging_up_on(Arc::clone(&rang_off));

    let watcher = {
        let woken = Arc::clone(&woken);
        let rang_off = Arc::clone(&rang_off);
        async move {
            until("the second question to reach the doer", || {
                woken.lock().unwrap().len() == 2
            })
            .await;
            rang_off.notify_one();
        }
    };
    tokio::join!(
        run_call(
            &bus,
            &provider,
            &mut caller,
            &turns,
            free_doer(),
            nobody_names(),
            opening(),
            subject(thread_id, uuid::Uuid::new_v4()),
        ),
        watcher
    );

    let woken = woken.lock().unwrap().clone();
    assert_eq!(
        woken,
        vec![
            "what have I got running".to_string(),
            "and tomorrow".to_string()
        ]
    );

    teardown_test_db(&db_name).await;
}

/// A wordless transcript does not spend a waiting ask.
///
/// The transcriber can return nothing for a cough. Pairing that with the ask
/// would write a `WorkDelegated` with no turn behind it. The caller's real
/// words would then arrive with no ask left to claim them.
#[tokio::test]
async fn a_wordless_transcript_does_not_spend_a_waiting_ask() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = a_chat_thread(&pool).await;

    let did = a_call_that_hears(
        &pool,
        &bus,
        thread_id,
        uuid::Uuid::new_v4(),
        vec![
            asks_for_the_doer("they want a booking"),
            the_caller_says("   "),
            the_caller_says("book it for tuesday"),
        ],
    )
    .await;

    assert_eq!(did.woken, vec!["book it for tuesday".to_string()]);
    let kinds = voice_kinds(&did.events);
    assert_eq!(
        kinds,
        vec!["SpokenMessageReceived", "WorkDelegated"],
        "{:?}",
        kinds
    );

    teardown_test_db(&db_name).await;
}

/// Progress is appended silently and an answer is spoken. Appending alone
/// reaches the caller's ear never, which is what `speak` exists for.
#[tokio::test]
async fn the_doers_answer_is_spoken_and_progress_is_not() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = a_chat_thread(&pool).await;

    let provider = MockVoiceProvider::new(vec![]);
    let log = provider.log();
    let turns = RecordingTurns::default();
    let hang_up = Arc::new(Notify::new());
    let mut caller = ScriptedCaller::new(vec![]).hanging_up_on(Arc::clone(&hang_up));

    tokio::join!(
        run_call(
            &bus,
            &provider,
            &mut caller,
            &turns,
            free_doer(),
            nobody_names(),
            opening(),
            subject(thread_id, uuid::Uuid::new_v4()),
        ),
        async {
            until("the session to open", || {
                log.lock().unwrap().openings.len() == 1
            })
            .await;
            a_turn_starts(&bus, thread_id).await;
            seed_thread_event(
                &bus,
                thread_id,
                ThreadEvent::ToolCalled {
                    name: "list_files".to_string(),
                    args: serde_json::json!({}),
                    description: String::new(),
                },
            )
            .await;
            seed_thread_event(
                &bus,
                thread_id,
                ThreadEvent::ResponseGenerated {
                    text: "Two things are running.".to_string(),
                    images: vec![],
                    model: None,
                    reasoning_effort: None,
                },
            )
            .await;
            until("the answer to be handed to the talker", || {
                log.lock().unwrap().asked_to_speak.len() == 1
            })
            .await;
            hang_up.notify_one();
        }
    );

    // Scoped, so the guard is gone before the teardown's await.
    {
        let log = log.lock().unwrap();
        assert!(
            log.history
                .iter()
                .any(|h| h == "[WORKING] Using list_files."),
            "the tool call was not appended: {:?}",
            log.history
        );
        assert_eq!(log.asked_to_speak.len(), 1);
        assert!(
            log.asked_to_speak[0].contains("Two things are running."),
            "{}",
            log.asked_to_speak[0]
        );
        assert!(
            !log.asked_to_speak[0].contains("list_files"),
            "progress was spoken: {}",
            log.asked_to_speak[0]
        );
    }

    teardown_test_db(&db_name).await;
}

/// The question the doer parked on, and the two options it offered.
fn asks_the_caller() -> ThreadEvent {
    ThreadEvent::UserQuestionAsked {
        tool_use_id: "toolu_q0".to_string(),
        cc_session_id: String::new(),
        question: "The mobile-webkit tail has no verdict. Do something now?".to_string(),
        options: vec![
            QuestionOption {
                id: "opt-0".to_string(),
                label: "Run the tail now".to_string(),
                description: Some("Chunks 25-33, on the current main".to_string()),
                preview: None,
            },
            QuestionOption {
                id: "opt-1".to_string(),
                label: "Leave it for tonight".to_string(),
                description: None,
                preview: None,
            },
        ],
        worktree_path: None,
        multi_select: false,
    }
}

/// The call reported in
/// `docs/plans/2026-08-30-the-talker-sees-the-open-question.md`. The doer parks
/// on a question, so no `ResponseGenerated` ever follows.
///
/// It is SPOKEN rather than appended for that reason: the turn is waiting on a
/// person, and a talker that stays quiet leaves them waiting for an answer
/// that is not coming.
#[tokio::test]
async fn a_question_is_put_to_the_caller_out_loud() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = a_chat_thread(&pool).await;

    let provider = MockVoiceProvider::new(vec![]);
    let log = provider.log();
    let turns = RecordingTurns::default();
    let hang_up = Arc::new(Notify::new());
    let mut caller = ScriptedCaller::new(vec![]).hanging_up_on(Arc::clone(&hang_up));

    tokio::join!(
        run_call(
            &bus,
            &provider,
            &mut caller,
            &turns,
            free_doer(),
            nobody_names(),
            opening(),
            subject(thread_id, uuid::Uuid::new_v4()),
        ),
        async {
            until("the session to open", || {
                log.lock().unwrap().openings.len() == 1
            })
            .await;
            a_turn_starts(&bus, thread_id).await;
            // The pair the doer really emits, in the order it emits them.
            seed_thread_event(
                &bus,
                thread_id,
                ThreadEvent::ToolCalled {
                    name: "ask_user_question".to_string(),
                    args: serde_json::json!({}),
                    description: String::new(),
                },
            )
            .await;
            seed_thread_event(&bus, thread_id, asks_the_caller()).await;
            until("the question to be handed to the talker", || {
                log.lock().unwrap().asked_to_speak.len() == 1
            })
            .await;
            hang_up.notify_one();
        }
    );

    {
        let log = log.lock().unwrap();
        let spoken = &log.asked_to_speak[0];
        assert!(spoken.contains("no verdict"), "{}", spoken);
        assert!(spoken.contains("Run the tail now"), "{}", spoken);
        assert!(spoken.contains("Chunks 25-33"), "{}", spoken);
        assert!(spoken.contains("Leave it for tonight"), "{}", spoken);
        assert!(
            !spoken.to_lowercase().contains("on screen"),
            "the talker was still sending the caller to the screen: {}",
            spoken
        );
        assert!(
            spoken.contains("question:toolu_q0#opt0"),
            "the talker was given no id to hand back: {}",
            spoken
        );
        // The only tool whose progress note is suppressed. It is the tool the
        // talker is told never to name, and the question says the real thing.
        assert!(
            !log.history.iter().any(|h| h.contains("ask_user_question")),
            "the tool name reached the talker: {:?}",
            log.history
        );
    }

    teardown_test_db(&db_name).await;
}

/// Whoever settled it already knows, so the talker is told rather than asked
/// to say it. What that prevents is the card being offered a second time.
#[tokio::test]
async fn an_answered_question_is_appended_and_never_asked_again() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = a_chat_thread(&pool).await;

    let provider = MockVoiceProvider::new(vec![]);
    let log = provider.log();
    let turns = RecordingTurns::default();
    let hang_up = Arc::new(Notify::new());
    let mut caller = ScriptedCaller::new(vec![]).hanging_up_on(Arc::clone(&hang_up));

    tokio::join!(
        run_call(
            &bus,
            &provider,
            &mut caller,
            &turns,
            free_doer(),
            nobody_names(),
            opening(),
            subject(thread_id, uuid::Uuid::new_v4()),
        ),
        async {
            until("the session to open", || {
                log.lock().unwrap().openings.len() == 1
            })
            .await;
            a_turn_starts(&bus, thread_id).await;
            seed_thread_event(&bus, thread_id, asks_the_caller()).await;
            until("the question to be handed to the talker", || {
                log.lock().unwrap().asked_to_speak.len() == 1
            })
            .await;
            seed_thread_event(
                &bus,
                thread_id,
                ThreadEvent::UserQuestionAnswered {
                    tool_use_id: "toolu_q0".to_string(),
                    answer: AnswerKind::Canceled,
                },
            )
            .await;
            until("the answer to reach the talker", || {
                log.lock()
                    .unwrap()
                    .history
                    .iter()
                    .any(|h| h.starts_with("[SETTLED]"))
            })
            .await;
            hang_up.notify_one();
        }
    );

    {
        let log = log.lock().unwrap();
        assert_eq!(
            log.asked_to_speak.len(),
            1,
            "the resolution was said out loud: {:?}",
            log.asked_to_speak
        );
    }

    teardown_test_db(&db_name).await;
}

/// A permission card in each of the three lanes, put to the caller out loud.
///
/// Spoken for the same reason a question is: the agent is blocked inside the
/// card, so no answer follows it. Before this the caller heard nothing at all,
/// and a delegated utterance silently resolved the card as denied.
#[tokio::test]
async fn a_permission_card_is_put_to_the_caller_out_loud_in_every_lane() {
    // The coding-agent lane fires only on a coding-agent thread: the lifecycle
    // validator refuses its event anywhere else. A call is refused on one at
    // admission (ADR 0165), so voice meets it after a destination flip mid-call.
    let lanes = [
        (
            "command",
            false,
            ThreadEvent::CommandPermissionRequested {
                request_id: "req-cmd".to_string(),
                tool_use_id: "toolu_b0".to_string(),
                tool_name: "run_bash".to_string(),
                command: "gh release delete v1".to_string(),
                summary: "Deletes a published release.".to_string(),
            },
            "Deletes a published release.",
            "command:req-cmd#allow-once",
        ),
        (
            "mcp",
            false,
            ThreadEvent::McpPermissionRequested {
                request_id: "req-mcp".to_string(),
                tool_use_id: "toolu_m0".to_string(),
                server_id: "example-server".to_string(),
                server_name: "Example Server".to_string(),
                tool_name: "post_message".to_string(),
                arguments_summary: "{\"channel\":\"general\"}".to_string(),
            },
            "Example Server",
            "mcp:req-mcp#allow-once",
        ),
        (
            "coding agent",
            true,
            ThreadEvent::CodingAgentPermissionRequest {
                request_id: "req-agent".to_string(),
                tool_use_id: "toolu_c0".to_string(),
                tool_name: "Bash".to_string(),
                input: serde_json::json!({ "command": "git push" }),
                summary: "Bash git push".to_string(),
            },
            "Bash git push",
            "agent:req-agent#allow-once",
        ),
    ];

    for (lane, on_a_coding_agent_thread, card, expected, first_choice) in lanes {
        let (pool, db_name) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let thread_id = if on_a_coding_agent_thread {
            a_coding_agent_thread(&bus).await
        } else {
            a_chat_thread(&pool).await
        };

        let provider = MockVoiceProvider::new(vec![]);
        let log = provider.log();
        let turns = RecordingTurns::default();
        let hang_up = Arc::new(Notify::new());
        let mut caller = ScriptedCaller::new(vec![]).hanging_up_on(Arc::clone(&hang_up));

        tokio::join!(
            run_call(
                &bus,
                &provider,
                &mut caller,
                &turns,
                free_doer(),
                nobody_names(),
                opening(),
                subject(thread_id, uuid::Uuid::new_v4()),
            ),
            async {
                until("the session to open", || {
                    log.lock().unwrap().openings.len() == 1
                })
                .await;
                if !on_a_coding_agent_thread {
                    a_turn_starts(&bus, thread_id).await;
                }
                seed_thread_event(&bus, thread_id, card).await;
                until("the card to be handed to the talker", || {
                    log.lock()
                        .unwrap()
                        .asked_to_speak
                        .iter()
                        .any(|s| s.starts_with("[PERMISSION]"))
                })
                .await;
                hang_up.notify_one();
            }
        );

        let spoken = log
            .lock()
            .unwrap()
            .asked_to_speak
            .iter()
            .find(|s| s.starts_with("[PERMISSION]"))
            .cloned()
            .unwrap_or_default();
        assert!(spoken.contains(expected), "{}: {}", lane, spoken);
        assert!(spoken.contains(first_choice), "{}: {}", lane, spoken);
        // Decision 7: both Always-allow scopes stay on screen.
        assert!(
            !spoken.to_lowercase().contains("always allow"),
            "{}: {}",
            lane,
            spoken
        );

        teardown_test_db(&db_name).await;
    }
}

/// A card resolved on screen mid-call tells the talker it is settled, so it
/// stops offering a spent card. Appended, never spoken.
#[tokio::test]
async fn a_permission_settled_mid_call_is_appended_and_never_asked_again() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = a_chat_thread(&pool).await;

    let provider = MockVoiceProvider::new(vec![]);
    let log = provider.log();
    let turns = RecordingTurns::default();
    let hang_up = Arc::new(Notify::new());
    let mut caller = ScriptedCaller::new(vec![]).hanging_up_on(Arc::clone(&hang_up));

    tokio::join!(
        run_call(
            &bus,
            &provider,
            &mut caller,
            &turns,
            free_doer(),
            nobody_names(),
            opening(),
            subject(thread_id, uuid::Uuid::new_v4()),
        ),
        async {
            until("the session to open", || {
                log.lock().unwrap().openings.len() == 1
            })
            .await;
            a_turn_starts(&bus, thread_id).await;
            seed_thread_event(
                &bus,
                thread_id,
                ThreadEvent::CommandPermissionRequested {
                    request_id: "req-cmd".to_string(),
                    tool_use_id: "toolu_b0".to_string(),
                    tool_name: "run_bash".to_string(),
                    command: "gh release delete v1".to_string(),
                    summary: "Deletes a published release.".to_string(),
                },
            )
            .await;
            until("the card to be handed to the talker", || {
                log.lock().unwrap().asked_to_speak.len() == 1
            })
            .await;
            seed_thread_event(
                &bus,
                thread_id,
                ThreadEvent::CommandPermissionResolved {
                    request_id: "req-cmd".to_string(),
                    allowed: true,
                    reason: None,
                    persist_scope: None,
                },
            )
            .await;
            until("the resolution to reach the talker", || {
                log.lock()
                    .unwrap()
                    .history
                    .iter()
                    .any(|h| h.starts_with("[SETTLED]"))
            })
            .await;
            hang_up.notify_one();
        }
    );

    assert_eq!(
        log.lock().unwrap().asked_to_speak.len(),
        1,
        "the resolution was said out loud"
    );

    teardown_test_db(&db_name).await;
}

/// A question with one option reads it out with the id that settles it, and a
/// free-text one still offers the caller's own words. Pure, so neither needs a
/// call behind it.
#[test]
fn the_choices_are_read_out_with_the_ids_that_settle_them() {
    let one = [QuestionOption {
        id: "opt-0".to_string(),
        label: "Ship it".to_string(),
        description: None,
        preview: None,
    }];
    let single = decision_to_ask(
        &OpenDecision::question("toolu_q0", "Ready?", &one, false),
        true,
    );
    assert!(
        single.contains("- Ship it [question:toolu_q0#opt0]"),
        "{}",
        single
    );

    let free = decision_to_ask(
        &OpenDecision::question("toolu_q1", "What should I call it?", &[], false),
        true,
    );
    assert!(free.contains("What should I call it?"), "{}", free);
    assert!(free.contains("[question:toolu_q1#said]"), "{}", free);
}

/// Both surfaces stopped sending the caller to the screen. This is the note
/// handed over mid-call; `sections` covers the resident block.
#[test]
fn the_note_says_the_caller_answers_out_loud_and_never_on_screen() {
    let decision = OpenDecision::question("toolu_q0", "Ready?", &[], false);
    let note = decision_to_ask(&decision, true);
    assert!(!note.to_lowercase().contains("on screen"), "{}", note);
    assert!(note.contains("hand its id back"), "{}", note);
    assert!(note.contains("Never say an id out loud"), "{}", note);
}

/// The framing fits the talker reading it, exactly as the refusal does.
///
/// A talker with no answering tool cannot hand an id back, so telling it to is
/// the same defect one surface over. It is told what it CAN do: hand the
/// caller's words over, which is what settles a question card (ADR 0205).
#[test]
fn a_talker_with_no_answering_tool_is_told_to_hand_the_words_over() {
    let decision = OpenDecision::question("toolu_q0", "Ready?", &[], false);
    let note = decision_to_ask(&decision, false);
    assert!(!note.contains("hand its id back"), "{}", note);
    assert!(note.contains("in their own words"), "{}", note);
    assert!(note.contains("what settles this"), "{}", note);
    assert!(note.contains("Never say an id out loud"), "{}", note);
}

/// The one card such a talker cannot settle says so, rather than offering a
/// choice nothing can press.
#[test]
fn a_permission_put_to_a_tool_less_talker_names_the_screen() {
    let note = decision_to_ask(
        &OpenDecision::command_permission("req-1", "run_bash", "rm -rf build", "Deletes files."),
        false,
    );
    assert!(note.starts_with("[PERMISSION]"), "{}", note);
    assert!(note.contains("settle on their screen"), "{}", note);
    assert!(!note.contains("hand its id back"), "{}", note);
}

/// A permission card reads as a request for permission, not as a question the
/// agent asked.
#[test]
fn a_permission_note_asks_for_a_say_so() {
    let note = decision_to_ask(
        &OpenDecision::mcp_permission(
            "req-1",
            "example-server",
            "Example Server",
            "post_message",
            "{\"channel\":\"general\"}",
        ),
        true,
    );
    assert!(note.starts_with("[PERMISSION]"), "{}", note);
    assert!(note.contains("Example Server"), "{}", note);
    assert!(
        note.contains("- Allow once [mcp:req-1#allow-once]"),
        "{}",
        note
    );
    assert!(note.contains("- Deny [mcp:req-1#deny]"), "{}", note);
}

/// Two replies at once is the failure a listener cannot recover from. An
/// answer landing mid-sentence waits for the floor, and is said after.
#[tokio::test]
async fn the_talker_is_not_asked_to_speak_over_itself() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = a_chat_thread(&pool).await;

    let (provider, talker) = MockVoiceProvider::driven();
    let log = provider.log();
    let turns = RecordingTurns::default();
    let hang_up = Arc::new(Notify::new());
    let mut caller = ScriptedCaller::new(vec![]).hanging_up_on(Arc::clone(&hang_up));
    let sent = Arc::clone(&caller.sent);

    tokio::join!(
        run_call(
            &bus,
            &provider,
            &mut caller,
            &turns,
            free_doer(),
            nobody_names(),
            opening(),
            subject(thread_id, uuid::Uuid::new_v4()),
        ),
        async {
            until("the session to open", || {
                log.lock().unwrap().openings.len() == 1
            })
            .await;
            // A call opens silent, so the floor opens before anything the
            // talker says reaches a soul (ADR 0211).
            say(&talker, the_caller_opens_the_floor()).await;
            until("the caller's first word", || {
                sent.lock().unwrap().len() >= 2
            })
            .await;

            // The talker takes the floor with a stall of its own.
            talker
                .send(VoiceEvent::TalkerTranscript {
                    text: "Let me check".to_string(),
                })
                .await
                .expect("the talker is listening");
            until("the talker to hold the floor", || {
                sent.lock()
                    .unwrap()
                    .iter()
                    .any(|f| matches!(f, ServerFrame::TalkerTranscript { .. }))
            })
            .await;

            a_turn_starts(&bus, thread_id).await;
            seed_thread_event(
                &bus,
                thread_id,
                ThreadEvent::ResponseGenerated {
                    text: "Two things are running.".to_string(),
                    images: vec![],
                    model: None,
                    reasoning_effort: None,
                },
            )
            .await;
            // The bus delivers in order, so a marker emitted after the answer
            // proves the answer was already handled. Without it the assertion
            // below would race the loop rather than test it.
            seed_thread_event(
                &bus,
                thread_id,
                ThreadEvent::ToolCalled {
                    name: "marker".to_string(),
                    args: serde_json::json!({}),
                    description: String::new(),
                },
            )
            .await;
            until("the marker to be appended", || {
                log.lock()
                    .unwrap()
                    .history
                    .iter()
                    .any(|h| h == "[WORKING] Using marker.")
            })
            .await;
            assert!(
                log.lock().unwrap().asked_to_speak.is_empty(),
                "the answer was spoken over the talker"
            );

            // The talker stops, and the queued answer goes out.
            talker
                .send(VoiceEvent::TalkerTurnEnded {
                    transcript: "Let me check.".to_string(),
                    usage: usage(),
                })
                .await
                .expect("the talker is listening");
            until("the queued answer to be said", || {
                log.lock().unwrap().asked_to_speak.len() == 1
            })
            .await;
            hang_up.notify_one();
        }
    );

    {
        let log = log.lock().unwrap();
        assert_eq!(log.asked_to_speak.len(), 1);
        assert!(log.asked_to_speak[0].contains("Two things are running."));
    }

    teardown_test_db(&db_name).await;
}

/// Is a doer answer spoken, after the talker sent `output` and nothing else?
///
/// `output` is the whole of what the talker produced, so the question is
/// whether it took the floor. An answer that reaches [`MockLog::asked_to_speak`]
/// says it did not, and one that never does says it did.
///
/// Waits on the DELIVERY count rather than on a frame shape, so one wait covers
/// audio and a transcript alike. Two deliveries: the session's own frame, then
/// whatever the talker sent.
async fn an_answer_spoken_after(
    bus: &EventBus,
    thread_id: uuid::Uuid,
    output: VoiceEvent,
    answer: &str,
) -> Vec<String> {
    let (provider, talker) = MockVoiceProvider::driven();
    let log = provider.log();
    let turns = RecordingTurns::default();
    let hang_up = Arc::new(Notify::new());
    let mut caller = ScriptedCaller::new(vec![]).hanging_up_on(Arc::clone(&hang_up));
    let delivered = Arc::clone(&caller.delivered);

    tokio::join!(
        run_call(
            bus,
            &provider,
            &mut caller,
            &turns,
            free_doer(),
            nobody_names(),
            opening(),
            subject(thread_id, uuid::Uuid::new_v4()),
        ),
        async {
            until("the session to open", || {
                log.lock().unwrap().openings.len() == 1
            })
            .await;

            // A call opens silent, and this is about the TALKER's floor
            // rather than that one (ADR 0211). So the caller speaks first,
            // which costs one delivery.
            say(&talker, the_caller_opens_the_floor()).await;
            until("the caller's first word", || {
                *delivered.lock().unwrap() >= 2
            })
            .await;

            talker.send(output).await.expect("the talker is listening");
            until("the talker's output to reach the caller", || {
                *delivered.lock().unwrap() >= 3
            })
            .await;

            a_turn_starts(bus, thread_id).await;
            seed_thread_event(
                bus,
                thread_id,
                ThreadEvent::ResponseGenerated {
                    text: answer.to_string(),
                    images: vec![],
                    model: None,
                    reasoning_effort: None,
                },
            )
            .await;
            until("the answer to be said out loud", || {
                log.lock().unwrap().asked_to_speak.len() == 1
            })
            .await;
            hang_up.notify_one();
        }
    );

    let said = log.lock().unwrap().asked_to_speak.clone();
    said
}

/// **The unheard-answer regression.** A Live talker streams audio between its
/// turns, so audio that claimed the floor claimed it for the whole call.
///
/// Every doer answer then queued behind a turn end that was never coming, and
/// the caller heard none of them. The floor reads the talker's WORDS.
#[tokio::test]
async fn the_talkers_audio_alone_does_not_hold_the_floor() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = a_chat_thread(&pool).await;

    // Audio, and not one word with it.
    let said = an_answer_spoken_after(
        &bus,
        thread_id,
        VoiceEvent::Audio(vec![0; 64]),
        "Two things are running.",
    )
    .await;

    assert_eq!(said.len(), 1);
    assert!(said[0].contains("Two things are running."), "{:?}", said);

    teardown_test_db(&db_name).await;
}

/// A blank delta is not the talker speaking either, so it takes no floor.
///
/// Both providers forward one exactly as it arrives. Read as speech, a stream
/// of them wedges the queue for the whole call, which is the case above again.
#[tokio::test]
async fn a_blank_talker_delta_does_not_hold_the_floor() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = a_chat_thread(&pool).await;

    let said = an_answer_spoken_after(
        &bus,
        thread_id,
        the_talker_is_saying("  "),
        "Nothing urgent.",
    )
    .await;

    assert_eq!(said.len(), 1);
    assert!(said[0].contains("Nothing urgent."), "{:?}", said);

    teardown_test_db(&db_name).await;
}

// ── What the caller heard is written down ─────────────────────────────────

/// The talker's turn lands in the thread under the talker's own name, so the
/// doer reads what was already said in its name (ADR 0150).
#[tokio::test]
async fn a_spoken_reply_is_written_down_under_the_talkers_name() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = a_chat_thread(&pool).await;
    let session_id = uuid::Uuid::new_v4();

    let provider = MockVoiceProvider::new(vec![
        the_caller_opens_the_floor(),
        VoiceEvent::TalkerTurnEnded {
            transcript: "Two things are running.".to_string(),
            usage: usage(),
        },
    ]);
    // SessionStarted, the caller's own turn, then the talker's turn end.
    let mut caller = ScriptedCaller::new(vec![]).hanging_up_after(3);

    run_call(
        &bus,
        &provider,
        &mut caller,
        &RecordingTurns::default(),
        free_doer(),
        nobody_names(),
        opening(),
        subject(thread_id, session_id),
    )
    .await;

    let spoken: Vec<serde_json::Value> = thread_events(&pool, thread_id)
        .await
        .into_iter()
        .filter(|(kind, _)| kind == "SpokenReplyGenerated")
        .map(|(_, payload)| payload)
        .collect();
    assert_eq!(spoken.len(), 1);
    assert_eq!(spoken[0]["text"], "Two things are running.");
    assert_eq!(spoken[0]["session_id"], session_id.to_string());
    assert_eq!(spoken[0]["interrupted"], false);
    assert_eq!(spoken[0]["actor"]["kind"], "agent");
    assert_eq!(spoken[0]["actor"]["agent"]["kind"], "guest");

    teardown_test_db(&db_name).await;
}

/// A reply the caller cut off says so, so the log does not claim they heard
/// the whole thing.
#[tokio::test]
async fn an_interrupted_reply_says_so() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = a_chat_thread(&pool).await;

    let provider = MockVoiceProvider::new(vec![
        the_caller_opens_the_floor(),
        VoiceEvent::Interrupted,
        VoiceEvent::TalkerTurnEnded {
            transcript: "Two things are".to_string(),
            usage: usage(),
        },
    ]);
    // SessionStarted, the caller's own turn, the interrupted frame, then the
    // turn end.
    let mut caller = ScriptedCaller::new(vec![]).hanging_up_after(4);

    run_call(
        &bus,
        &provider,
        &mut caller,
        &RecordingTurns::default(),
        free_doer(),
        nobody_names(),
        opening(),
        subject(thread_id, uuid::Uuid::new_v4()),
    )
    .await;

    let spoken: Vec<serde_json::Value> = thread_events(&pool, thread_id)
        .await
        .into_iter()
        .filter(|(kind, _)| kind == "SpokenReplyGenerated")
        .map(|(_, payload)| payload)
        .collect();
    assert_eq!(spoken.len(), 1);
    assert_eq!(spoken[0]["interrupted"], true);
    assert_eq!(spoken[0]["text"], "Two things are");

    teardown_test_db(&db_name).await;
}

/// A cancelled reply can end before a word was said. An empty row would claim
/// the caller heard something they did not.
#[tokio::test]
async fn a_reply_with_no_words_is_not_written_down() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = a_chat_thread(&pool).await;

    let provider = MockVoiceProvider::new(vec![VoiceEvent::TalkerTurnEnded {
        transcript: String::new(),
        usage: usage(),
    }]);
    let mut caller = ScriptedCaller::new(vec![]).hanging_up_after(2);

    run_call(
        &bus,
        &provider,
        &mut caller,
        &RecordingTurns::default(),
        free_doer(),
        nobody_names(),
        opening(),
        subject(thread_id, uuid::Uuid::new_v4()),
    )
    .await;

    assert!(
        !thread_events(&pool, thread_id)
            .await
            .iter()
            .any(|(kind, _)| kind == "SpokenReplyGenerated"),
        "an empty reply was written down"
    );

    teardown_test_db(&db_name).await;
}

/// Ringing off ends the CALL, never the turn a spoken utterance started. The
/// answer lands in the thread and the user reads it there.
///
/// A source scan, because the property is structural: a module that cannot
/// name a way to end a turn cannot end one. It fails at the first `use`, where
/// a behavioural test would need a whole engine and a turn to race.
///
/// **The list is the VERBS, not the event names.** `call.rs` reads
/// `ResponseCanceled` and `ResponseAborted` to decide what to tell the caller,
/// which is the opposite of causing one. Scanning for the variant names
/// therefore fails on the reader, and it did. Every way to actually end a turn
/// goes through one of these, per `.claude/rules/rust.md`, which is what makes
/// the narrower list total.
#[test]
fn ending_a_call_never_terminates_the_turn_it_started() {
    let terminators = [
        "emit_response_canceled",
        "emit_response_aborted",
        "make_terminal_event",
        "cancel_thread",
    ];
    let mut offenders = Vec::new();
    let mut scanned = 0;
    for (rel, text) in crate::test_support::source_scan::production_sources() {
        if !rel.starts_with("voice/") {
            continue;
        }
        scanned += 1;
        for name in terminators {
            if text.contains(name) {
                offenders.push(format!("{}: {}", rel, name));
            }
        }
    }
    // A renamed directory would otherwise make this pass by reading nothing.
    assert!(scanned > 5, "the scan found no voice sources to read");
    assert!(
        offenders.is_empty(),
        "a call must not end the doer's turn: {:?}",
        offenders
    );
}

/// A turn the user stopped is going nowhere, and the caller is owed that.
/// Silence there is a person holding a phone waiting for an answer.
#[tokio::test]
async fn a_stopped_turn_tells_the_caller_it_is_not_coming() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = a_chat_thread(&pool).await;

    let provider = MockVoiceProvider::new(vec![]);
    let log = provider.log();
    let turns = RecordingTurns::default();
    let hang_up = Arc::new(Notify::new());
    let mut caller = ScriptedCaller::new(vec![]).hanging_up_on(Arc::clone(&hang_up));

    tokio::join!(
        run_call(
            &bus,
            &provider,
            &mut caller,
            &turns,
            free_doer(),
            nobody_names(),
            opening(),
            subject(thread_id, uuid::Uuid::new_v4()),
        ),
        async {
            until("the session to open", || {
                log.lock().unwrap().openings.len() == 1
            })
            .await;
            a_turn_starts(&bus, thread_id).await;
            seed_thread_event(&bus, thread_id, canceled(CancelCause::UserStop)).await;
            until("the caller to be told", || {
                log.lock().unwrap().asked_to_speak.len() == 1
            })
            .await;
            hang_up.notify_one();
        }
    );

    {
        let log = log.lock().unwrap();
        assert_eq!(log.asked_to_speak.len(), 1);
        assert!(
            log.asked_to_speak[0].contains("did not finish"),
            "{}",
            log.asked_to_speak[0]
        );
    }

    teardown_test_db(&db_name).await;
}

/// Talking over the answer is how people talk. The turn that replaced this one
/// is already running, so saying "that did not finish" would talk over it.
#[tokio::test]
async fn a_turn_superseded_by_the_next_utterance_says_nothing() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = a_chat_thread(&pool).await;

    let provider = MockVoiceProvider::new(vec![]);
    let log = provider.log();
    let turns = RecordingTurns::default();
    let hang_up = Arc::new(Notify::new());
    let mut caller = ScriptedCaller::new(vec![]).hanging_up_on(Arc::clone(&hang_up));

    tokio::join!(
        run_call(
            &bus,
            &provider,
            &mut caller,
            &turns,
            free_doer(),
            nobody_names(),
            opening(),
            subject(thread_id, uuid::Uuid::new_v4()),
        ),
        async {
            until("the session to open", || {
                log.lock().unwrap().openings.len() == 1
            })
            .await;
            a_turn_starts(&bus, thread_id).await;
            seed_thread_event(&bus, thread_id, canceled(CancelCause::SupersededByFollowup)).await;
            // The bus delivers in order, so a marker after the cancel proves
            // the cancel was already handled.
            seed_thread_event(
                &bus,
                thread_id,
                ThreadEvent::ToolCalled {
                    name: "marker".to_string(),
                    args: serde_json::json!({}),
                    description: String::new(),
                },
            )
            .await;
            until("the marker to be appended", || {
                log.lock()
                    .unwrap()
                    .history
                    .iter()
                    .any(|h| h == "[WORKING] Using marker.")
            })
            .await;
            hang_up.notify_one();
        }
    );

    assert!(
        log.lock().unwrap().asked_to_speak.is_empty(),
        "a superseded turn talked over its own replacement"
    );

    teardown_test_db(&db_name).await;
}

// ── An answer is said, never read ─────────────────────────────────────────

/// A written answer is a source, not a script. The doer writes for a
/// reader, and the caller is on a phone.
#[test]
fn the_answer_is_handed_over_to_be_said_not_read() {
    let framing = answer_to_say("Both endpoints answered live.");
    assert!(framing.contains("Do not read it out"), "{}", framing);
    assert!(
        !framing.contains("Say this to the caller"),
        "the talker was handed a script: {}",
        framing
    );
    // In full, always. The talker holds no tools, so what is trimmed here is
    // what it has to invent later.
    assert!(
        framing.contains("Both endpoints answered live."),
        "{}",
        framing
    );
}

/// Past the threshold the caller gets the headline and the offer, because
/// there is more meaning in the answer than a listener can hold.
#[test]
fn a_long_answer_is_offered_as_a_summary() {
    let long = "The account is fine. ".repeat(30);
    assert!(long.chars().count() > OFFER_THE_DETAIL_ABOVE_CHARS);

    let framing = answer_to_say(&long);
    assert!(
        framing.contains("ask whether they want the detail"),
        "{}",
        framing
    );
    assert!(framing.contains(&long), "the long answer was trimmed");
}

/// A one-line answer is not padded with an offer of detail it does not have.
#[test]
fn a_short_answer_is_not_padded_with_an_offer() {
    let framing = answer_to_say("Yes, both are green.");
    assert!(
        !framing.contains("want the detail"),
        "a one-line answer offered more: {}",
        framing
    );
}

// ── The talker's turns reach a running round ──────────────────────────────

/// The talker's own words are offered to a turn already running, so the doer
/// learns what the caller was told in its name.
///
/// Offered, not forced: with no turn running there is no loop to inject into,
/// and the engine drops it. The row is in the thread either way.
#[tokio::test]
async fn the_talkers_own_words_are_offered_to_a_running_round() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = a_chat_thread(&pool).await;

    let provider = MockVoiceProvider::new(vec![
        the_caller_opens_the_floor(),
        the_talker_says("Still on it."),
    ]);
    let turns = RecordingTurns::default();
    let overheard = turns.overheard();
    // SessionStarted, the caller's own turn, then the talker's turn end.
    let mut caller = ScriptedCaller::new(vec![]).hanging_up_after(3);

    run_call(
        &bus,
        &provider,
        &mut caller,
        &turns,
        free_doer(),
        nobody_names(),
        opening(),
        subject(thread_id, uuid::Uuid::new_v4()),
    )
    .await;

    assert_eq!(*overheard.lock().unwrap(), vec!["Still on it.".to_string()]);

    teardown_test_db(&db_name).await;
}

/// A reply the talker was HANDED is not offered back. The round wrote that
/// answer itself, so echoing it in would be the round reading its own words
/// as something new.
#[tokio::test]
async fn an_answer_the_talker_relayed_is_not_offered_back() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = a_chat_thread(&pool).await;

    let (provider, talker) = MockVoiceProvider::driven();
    let log = provider.log();
    let turns = RecordingTurns::default();
    let overheard = turns.overheard();
    let hang_up = Arc::new(Notify::new());
    let mut caller = ScriptedCaller::new(vec![]).hanging_up_on(Arc::clone(&hang_up));

    tokio::join!(
        run_call(
            &bus,
            &provider,
            &mut caller,
            &turns,
            free_doer(),
            nobody_names(),
            opening(),
            subject(thread_id, uuid::Uuid::new_v4()),
        ),
        async {
            until("the session to open", || {
                log.lock().unwrap().openings.len() == 1
            })
            .await;
            // A call opens silent, so a turn of the talker's OWN reaches
            // nobody until the caller has spoken (ADR 0211).
            say(&talker, the_caller_opens_the_floor()).await;
            a_turn_starts(&bus, thread_id).await;
            seed_thread_event(
                &bus,
                thread_id,
                ThreadEvent::ResponseGenerated {
                    text: "Two things are running.".to_string(),
                    images: vec![],
                    model: None,
                    reasoning_effort: None,
                },
            )
            .await;
            until("the answer to be handed over", || {
                log.lock().unwrap().asked_to_speak.len() == 1
            })
            .await;

            // The talker says it, which is the relay.
            talker
                .send(the_talker_says("Two things are running."))
                .await
                .expect("the talker is listening");
            // Then a turn of its own. The channel is FIFO, so this one being
            // offered proves the relay ahead of it was already handled.
            talker
                .send(the_talker_says("Anything else?"))
                .await
                .expect("the talker is listening");
            until("the talker's own turn to be offered", || {
                !overheard.lock().unwrap().is_empty()
            })
            .await;
            hang_up.notify_one();
        }
    );

    assert_eq!(
        *overheard.lock().unwrap(),
        vec!["Anything else?".to_string()],
        "the relayed answer was offered back to the round that wrote it"
    );

    teardown_test_db(&db_name).await;
}

/// Handing the talker something new is a MOVE, so it writes both held rows,
/// and the caller's goes first.
///
/// This is the move the caller cannot make for themselves: they have said
/// their piece and are waiting. Nothing between their words and the relay
/// writes a row now that a pause does not. So a reply flushed alone here would
/// put the talker's stall above the question it was stalling on.
#[tokio::test]
async fn a_relayed_answer_writes_the_question_before_the_stall() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = a_chat_thread(&pool).await;

    let (provider, talker) = MockVoiceProvider::driven();
    let log = provider.log();
    let turns = RecordingTurns::default();
    // Sequenced on the last frame the caller sees, which is the relay's own
    // turn end. Ringing off on a notify races the teardown against it, and the
    // relay's row is the one that goes missing.
    let mut caller = ScriptedCaller::new(vec![]).hanging_up_after(4);

    tokio::join!(
        run_call(
            &bus,
            &provider,
            &mut caller,
            &turns,
            free_doer(),
            nobody_names(),
            opening(),
            subject(thread_id, uuid::Uuid::new_v4()),
        ),
        async {
            until("the session to open", || {
                log.lock().unwrap().openings.len() == 1
            })
            .await;
            talker
                .send(the_caller_says("what have I got running"))
                .await
                .expect("the talker is listening");
            // A stall of its own, and the pause after it. Both rows are held.
            talker
                .send(the_talker_says("Let me check."))
                .await
                .expect("the talker is listening");

            a_turn_starts(&bus, thread_id).await;
            seed_thread_event(
                &bus,
                thread_id,
                ThreadEvent::ResponseGenerated {
                    text: "Two things are running.".to_string(),
                    images: vec![],
                    model: None,
                    reasoning_effort: None,
                },
            )
            .await;
            until("the answer to be handed over", || {
                log.lock().unwrap().asked_to_speak.len() == 1
            })
            .await;
            talker
                .send(the_talker_says("Two things are running."))
                .await
                .expect("the talker is listening");
        }
    );

    let events = thread_events(&pool, thread_id).await;
    assert_eq!(
        spoken_kinds(&events),
        vec![
            "SpokenMessageReceived",
            "SpokenReplyGenerated",
            "SpokenReplyGenerated",
        ],
        "{:?}",
        events
    );
    let said = replies(&events);
    assert_eq!(said[0]["text"], "Let me check.");
    assert_eq!(said[1]["text"], "Two things are running.");

    teardown_test_db(&db_name).await;
}

/// Words the caller FINISHED over a stall read above the stall's own row.
///
/// Both rows go down at their own turn end, and the caller's sentence ended
/// first: the stall was still being spoken. Nothing is held to be reordered,
/// which is the point (ADR 0201).
///
/// The sibling above is the other case, and the difference is WHO finished
/// first rather than who started. Two rows written at one moment, at the
/// close, are the only place the order is chosen.
#[tokio::test]
async fn words_the_caller_finished_over_a_stall_read_above_it() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = a_chat_thread(&pool).await;

    let (provider, talker) = MockVoiceProvider::driven();
    let log = provider.log();
    let turns = RecordingTurns::default();
    let hang_up = Arc::new(Notify::new());
    let mut caller = ScriptedCaller::new(vec![]).hanging_up_on(Arc::clone(&hang_up));
    let sent = Arc::clone(&caller.sent);

    tokio::join!(
        run_call(
            &bus,
            &provider,
            &mut caller,
            &turns,
            free_doer(),
            nobody_names(),
            opening(),
            subject(thread_id, uuid::Uuid::new_v4()),
        ),
        async {
            until("the session to open", || {
                log.lock().unwrap().openings.len() == 1
            })
            .await;
            // A call opens silent, so the floor opens before anything the
            // talker says reaches a soul (ADR 0211).
            say(&talker, the_caller_opens_the_floor()).await;
            await_frames(&sent, 1, "the caller's first word").await;
            // A stall with nothing of the caller's held behind it.
            say(&talker, the_talker_is_saying("Let me check")).await;
            await_frames(&sent, 2, "the talker to take the floor").await;
            // They talk over it, and never cut it off.
            say(&talker, the_caller_says("and quickly")).await;
            await_frames(&sent, 3, "their words to land").await;
            say(&talker, the_talker_says("Let me check.")).await;
            await_frames(&sent, 4, "the stall to pause").await;

            a_turn_starts(&bus, thread_id).await;
            seed_thread_event(
                &bus,
                thread_id,
                ThreadEvent::ResponseGenerated {
                    text: "Two things are running.".to_string(),
                    images: vec![],
                    model: None,
                    reasoning_effort: None,
                },
            )
            .await;
            until("the answer to be handed over", || {
                log.lock().unwrap().asked_to_speak.len() == 1
            })
            .await;
            hang_up.notify_one();
        }
    );

    let events = thread_events(&pool, thread_id).await;
    assert_eq!(
        spoken_kinds(&events),
        vec![
            "SpokenMessageReceived",
            "SpokenMessageReceived",
            "SpokenReplyGenerated"
        ],
        "{:?}",
        spoken_kinds(&events)
    );

    teardown_test_db(&db_name).await;
}

// ---------------------------------------------------------------------------
// Answering, and the refusal
// ---------------------------------------------------------------------------

/// The talker answering a choice id, as the seam sees it.
fn answers_with(choice_id: &str) -> VoiceEvent {
    VoiceEvent::AnswerRequested {
        tool_call_id: "call_a".to_string(),
        choice_id: choice_id.to_string(),
    }
}

/// One call, driven over a script with a resolver of the test's choosing.
///
/// The sibling of `a_call_that_hears`, for the cases that are about what the
/// call does with a decision rather than about what it writes down.
async fn a_call_deciding(
    pool: &PgPool,
    bus: &EventBus,
    decisions: &NoDecisions,
    script: Vec<VoiceEvent>,
) -> (Arc<Mutex<crate::voice::mock::MockLog>>, Vec<String>) {
    let deliveries = deliveries_for(&script);
    a_call_driven_by(
        pool,
        bus,
        decisions,
        MockVoiceProvider::new(script),
        deliveries,
    )
    .await
}

/// The same, on a talker that holds no answering tool, as a Live one.
///
/// Its only signal is the ask, so these cases script a delegation where the
/// sibling above scripts an `answer`.
async fn a_tool_less_call_deciding(
    pool: &PgPool,
    bus: &EventBus,
    decisions: &NoDecisions,
    script: Vec<VoiceEvent>,
) -> (Arc<Mutex<crate::voice::mock::MockLog>>, Vec<String>) {
    let deliveries = deliveries_for(&script);
    let provider = MockVoiceProvider::new(script).holding_no_answer_tool();
    a_call_driven_by(pool, bus, decisions, provider, deliveries).await
}

/// How many frames reach the caller before the script is spent.
///
/// Counted, not guessed. A tool call reaches the caller as no frame at all. So
/// the deliveries are the utterances, the replies, and the opening frame every
/// call sends. Ringing off too early ends the call before the tool call is even
/// read.
fn deliveries_for(script: &[VoiceEvent]) -> usize {
    1 + script
        .iter()
        .filter(|event| {
            matches!(
                event,
                VoiceEvent::UserTurnEnded { .. } | VoiceEvent::TalkerTurnEnded { .. }
            )
        })
        .count()
}

async fn a_call_driven_by(
    pool: &PgPool,
    bus: &EventBus,
    decisions: &NoDecisions,
    provider: MockVoiceProvider,
    deliveries: usize,
) -> (Arc<Mutex<crate::voice::mock::MockLog>>, Vec<String>) {
    let thread_id = a_chat_thread(pool).await;
    let log = provider.log();
    let turns = RecordingTurns::default();
    let mut caller = ScriptedCaller::new(vec![]).hanging_up_after(deliveries);

    run_call(
        bus,
        &provider,
        &mut caller,
        &turns,
        decisions,
        nobody_names(),
        opening(),
        subject(thread_id, uuid::Uuid::new_v4()),
    )
    .await;

    let woken = turns.woken().lock().unwrap().clone();
    (log, woken)
}

/// The whole route, end to end with no socket: the caller says which one, the
/// talker hands back the id, and the engine settles it.
#[tokio::test]
async fn a_spoken_answer_settles_the_card_and_the_talker_is_told() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let decisions = NoDecisions::default();
    let asked = decisions.asked();
    let (log, _) = a_call_deciding(
        &pool,
        &bus,
        &decisions,
        vec![
            the_caller_says("the first one"),
            answers_with("question:toolu_q0#opt0"),
            the_talker_says("Done."),
        ],
    )
    .await;

    assert_eq!(
        asked.lock().unwrap().clone(),
        vec![(
            "question:toolu_q0#opt0".to_string(),
            "the first one".to_string()
        )]
    );
    let resolved = log.lock().unwrap().resolved_tool_calls.clone();
    assert_eq!(resolved.len(), 1, "{:?}", resolved);
    assert_eq!(resolved[0].0, "call_a");
    assert!(resolved[0].1.contains("Answered"), "{:?}", resolved);

    teardown_test_db(&db_name).await;
}

/// An id the engine did not issue is refused with a note saying so, never
/// guessed at. The refusal still resolves the tool call, so nothing dangles.
#[tokio::test]
async fn an_id_the_engine_did_not_issue_is_refused_out_loud() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let refusal = crate::voice::decision::NOT_WAITING.to_string();
    let decisions = NoDecisions::answering(vec![Resolution::Refused(refusal.clone())]);
    let (log, _) = a_call_deciding(
        &pool,
        &bus,
        &decisions,
        vec![
            the_caller_says("allow it"),
            answers_with("something the talker made up"),
            the_talker_says("Let me check that."),
        ],
    )
    .await;

    let resolved = log.lock().unwrap().resolved_tool_calls.clone();
    assert_eq!(resolved.len(), 1, "{:?}", resolved);
    assert_eq!(resolved[0].1, refusal);

    teardown_test_db(&db_name).await;
}

/// The "something else" choice sends the caller's transcript, word for word.
/// A paraphrase would be a different answer (ADR 0149).
#[tokio::test]
async fn their_own_words_reach_the_card_exactly_as_they_said_them() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let decisions = NoDecisions::answering(vec![Resolution::SettledWithTheirWords]);
    let asked = decisions.asked();
    let (_, woken) = a_call_deciding(
        &pool,
        &bus,
        &decisions,
        vec![
            the_caller_says("neither, do the second half only"),
            answers_with("question:toolu_q0#said"),
            the_talker_says("Right."),
        ],
    )
    .await;

    assert_eq!(
        asked.lock().unwrap().clone(),
        vec![(
            "question:toolu_q0#said".to_string(),
            "neither, do the second half only".to_string()
        )]
    );
    // No turn: an answer is not a request, and the words were spent on it.
    assert!(woken.is_empty(), "{:?}", woken);

    teardown_test_db(&db_name).await;
}

/// Words spent on an answer are still what the caller said, so they have their
/// own row. What they must not do is run a turn as well.
///
/// The row and the answer are two facts: the caller spoke, and a card settled
/// on what they spoke. The transcript reads both in the order they happened.
#[tokio::test]
async fn words_spent_on_an_answer_are_still_written_down_as_speech() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = a_chat_thread(&pool).await;

    let decisions = NoDecisions::answering(vec![Resolution::SettledWithTheirWords]);
    let provider = MockVoiceProvider::new(vec![
        the_caller_says("neither, do the second half only"),
        answers_with("question:toolu_q0#said"),
        the_talker_says("Right."),
    ]);
    let mut caller = ScriptedCaller::new(vec![]).hanging_up_after(3);

    run_call(
        &bus,
        &provider,
        &mut caller,
        &RecordingTurns::default(),
        &decisions,
        nobody_names(),
        opening(),
        subject(thread_id, uuid::Uuid::new_v4()),
    )
    .await;

    let events = thread_events(&pool, thread_id).await;
    let spoken: Vec<_> = events
        .iter()
        .filter(|(kind, _)| kind == "SpokenMessageReceived")
        .collect();
    assert_eq!(spoken.len(), 1, "{:?}", voice_kinds(&events));
    assert_eq!(spoken[0].1["text"], "neither, do the second half only");
    // And no turn ran on them: the answer settled a card instead.
    assert!(
        !voice_kinds(&events).contains(&"WorkDelegated".to_string()),
        "{:?}",
        voice_kinds(&events)
    );

    teardown_test_db(&db_name).await;
}

/// An answer that needs the caller's words, made before the transcript landed.
///
/// The same race the ask has: the tool call and the transcript come from two
/// models on one socket. Held, and settled by the `UserTurnEnded` that follows.
#[tokio::test]
async fn an_answer_waiting_on_their_words_settles_when_the_words_arrive() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let decisions = NoDecisions::answering(vec![
        Resolution::NeedsTheirWords,
        Resolution::SettledWithTheirWords,
    ]);
    let asked = decisions.asked();
    let (log, _) = a_call_deciding(
        &pool,
        &bus,
        &decisions,
        vec![
            // The call first, then the words. That order is the bug this
            // handles, and a fixed script really does deliver it.
            answers_with("question:toolu_q0#said"),
            the_caller_says("do the second half only"),
            the_talker_says("Right."),
        ],
    )
    .await;

    let asked = asked.lock().unwrap().clone();
    assert_eq!(asked.len(), 2, "{:?}", asked);
    assert_eq!(asked[0].1, "", "the first try had no words yet");
    assert_eq!(asked[1].1, "do the second half only");

    // One acknowledgement, and only once it actually settled. The held call
    // must not be answered twice.
    let resolved = log.lock().unwrap().resolved_tool_calls.clone();
    assert_eq!(resolved.len(), 1, "{:?}", resolved);
    assert!(resolved[0].1.contains("Answered"), "{:?}", resolved);

    teardown_test_db(&db_name).await;
}

/// A delegation is refused exactly while this thread's doer is parked. The
/// refusal states a fact: the doer is blocked inside the card that is waiting.
#[tokio::test]
async fn a_delegation_is_refused_while_the_doer_is_parked() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = a_chat_thread(&pool).await;

    let provider = MockVoiceProvider::new(vec![
        the_caller_says("book me a table for eight"),
        asks_for_the_doer("they want a booking"),
        the_talker_says("I need the other answer first."),
    ]);
    let log = provider.log();
    let turns = RecordingTurns::default();
    let mut caller = ScriptedCaller::new(vec![]).hanging_up_after(3);

    run_call(
        &bus,
        &provider,
        &mut caller,
        &turns,
        &NoDecisions::parked(),
        nobody_names(),
        opening(),
        subject(thread_id, uuid::Uuid::new_v4()),
    )
    .await;

    // Nothing was started, and no ask was recorded beside a turn that never ran.
    assert!(turns.woken().lock().unwrap().is_empty());
    let events = thread_events(&pool, thread_id).await;
    let kinds = voice_kinds(&events);
    assert!(!kinds.contains(&"WorkDelegated".to_string()), "{:?}", kinds);

    // The utterance is still written down, exactly once.
    assert_eq!(
        kinds
            .iter()
            .filter(|k| *k == "SpokenMessageReceived")
            .count(),
        1,
        "{:?}",
        kinds
    );

    // And the talker was told why, rather than left holding a dangling call.
    let resolved = log.lock().unwrap().resolved_tool_calls.clone();
    assert_eq!(resolved.len(), 1, "{:?}", resolved);
    assert!(resolved[0].1.contains("Not started"), "{:?}", resolved);

    teardown_test_db(&db_name).await;
}

/// The other side of the refusal: a free doer takes the delegation as before.
#[tokio::test]
async fn a_delegation_goes_through_when_nothing_is_waiting() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = a_chat_thread(&pool).await;

    let provider = MockVoiceProvider::new(vec![
        the_caller_says("book me a table for eight"),
        asks_for_the_doer("they want a booking"),
        the_talker_says("On it."),
    ]);
    let log = provider.log();
    let turns = RecordingTurns::default();
    let mut caller = ScriptedCaller::new(vec![]).hanging_up_after(3);

    run_call(
        &bus,
        &provider,
        &mut caller,
        &turns,
        free_doer(),
        nobody_names(),
        opening(),
        subject(thread_id, uuid::Uuid::new_v4()),
    )
    .await;

    assert_eq!(
        *turns.woken().lock().unwrap(),
        vec!["book me a table for eight".to_string()]
    );
    let resolved = log.lock().unwrap().resolved_tool_calls.clone();
    assert!(resolved[0].1.contains("Taken"), "{:?}", resolved);

    teardown_test_db(&db_name).await;
}

// ---------------------------------------------------------------------------
// A talker with no answering tool, settling what is waiting anyway
// ---------------------------------------------------------------------------

/// The reported defect, end to end. A Live caller settles the card by speaking.
///
/// Its talker holds no answering tool, so the ask is the whole of what it can
/// send. That ask means the caller said something worth acting on, and the
/// question card's free-text choice is what carries it.
///
/// Before this, the same script refused every ask and told the talker to use a
/// tool it does not have. One reported call ran that loop for three minutes.
#[tokio::test]
async fn a_talker_with_no_answering_tool_settles_a_question_with_the_callers_words() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = a_chat_thread(&pool).await;

    let decisions = NoDecisions::parked().answering_with(vec![Resolution::SettledWithTheirWords]);
    let asked = decisions.asked();
    let provider = MockVoiceProvider::new(vec![
        the_caller_says("he can just set up the MCP server, discard any follow up"),
        asks_for_the_doer(""),
        the_talker_says("Understood."),
    ])
    .holding_no_answer_tool();
    let log = provider.log();
    let turns = RecordingTurns::default();
    let mut caller = ScriptedCaller::new(vec![]).hanging_up_after(3);

    run_call(
        &bus,
        &provider,
        &mut caller,
        &turns,
        &decisions,
        nobody_names(),
        opening(),
        subject(thread_id, uuid::Uuid::new_v4()),
    )
    .await;

    // The card was settled with what they actually said, word for word.
    assert_eq!(
        asked.lock().unwrap().clone(),
        vec![(
            format!("question:{}#said", PARKED_QUESTION),
            "he can just set up the MCP server, discard any follow up".to_string()
        )]
    );

    // No turn ran on those words: the answer spent them.
    assert!(turns.woken().lock().unwrap().is_empty());
    let kinds = voice_kinds(&thread_events(&pool, thread_id).await);
    assert!(!kinds.contains(&"WorkDelegated".to_string()), "{:?}", kinds);

    // And the talker heard that it landed, rather than being told to answer.
    let resolved = log.lock().unwrap().resolved_tool_calls.clone();
    assert_eq!(resolved.len(), 1, "{:?}", resolved);
    assert!(resolved[0].1.contains("Answered"), "{:?}", resolved);

    teardown_test_db(&db_name).await;
}

/// Nothing matches a spoken word against a label, ever.
///
/// The only choice this route can reach is the one that sends the transcript.
/// A card's labelled options are the talker's to pick, and this talker cannot,
/// so the engine must not pick one for it (ADR 0170).
#[tokio::test]
async fn the_settling_choice_is_their_own_words_and_never_a_labelled_option() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let decisions = NoDecisions::parked().answering_with(vec![Resolution::SettledWithTheirWords]);
    let asked = decisions.asked();
    a_tool_less_call_deciding(
        &pool,
        &bus,
        &decisions,
        vec![
            // Words that name an option. Still their words, never `#opt0`.
            the_caller_says("run it"),
            asks_for_the_doer(""),
            the_talker_says("Right."),
        ],
    )
    .await;

    let asked = asked.lock().unwrap().clone();
    assert_eq!(asked.len(), 1, "{:?}", asked);
    assert!(asked[0].0.ends_with("#said"), "{:?}", asked);

    teardown_test_db(&db_name).await;
}

/// A permission card takes a decision rather than words, so nothing settles it
/// out loud here. The talker is sent to the screen, which is the only true
/// thing left.
#[tokio::test]
async fn a_permission_card_is_not_settled_by_an_ask_and_the_note_names_the_screen() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = a_chat_thread(&pool).await;

    let decisions = NoDecisions::parked_on_a_permission();
    let asked = decisions.asked();
    let provider = MockVoiceProvider::new(vec![
        the_caller_says("yeah go ahead"),
        asks_for_the_doer(""),
        the_talker_says("Hmm."),
    ])
    .holding_no_answer_tool();
    let log = provider.log();
    let turns = RecordingTurns::default();
    let mut caller = ScriptedCaller::new(vec![]).hanging_up_after(3);

    run_call(
        &bus,
        &provider,
        &mut caller,
        &turns,
        &decisions,
        nobody_names(),
        opening(),
        subject(thread_id, uuid::Uuid::new_v4()),
    )
    .await;

    assert!(asked.lock().unwrap().is_empty(), "a permission was settled");
    assert!(turns.woken().lock().unwrap().is_empty());
    let kinds = voice_kinds(&thread_events(&pool, thread_id).await);
    assert!(!kinds.contains(&"WorkDelegated".to_string()), "{:?}", kinds);

    let resolved = log.lock().unwrap().resolved_tool_calls.clone();
    assert_eq!(resolved.len(), 1, "{:?}", resolved);
    assert!(resolved[0].1.contains("screen"), "{:?}", resolved);

    teardown_test_db(&db_name).await;
}

/// The ask and the transcript come from two models on one socket, so the ask
/// can land first. Held, and settled by the utterance it was made for.
#[tokio::test]
async fn an_ask_that_settles_a_card_waits_for_the_words_it_was_made_for() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let decisions = NoDecisions::parked().answering_with(vec![
        Resolution::NeedsTheirWords,
        Resolution::SettledWithTheirWords,
    ]);
    let asked = decisions.asked();
    let (log, _) = a_tool_less_call_deciding(
        &pool,
        &bus,
        &decisions,
        vec![
            asks_for_the_doer(""),
            the_caller_says("leave it for now"),
            the_talker_says("Right."),
        ],
    )
    .await;

    let asked = asked.lock().unwrap().clone();
    assert_eq!(asked.len(), 2, "{:?}", asked);
    assert_eq!(asked[0].1, "", "the ask arrived before the words");
    assert_eq!(asked[1].1, "leave it for now");

    let resolved = log.lock().unwrap().resolved_tool_calls.clone();
    assert_eq!(resolved.len(), 1, "{:?}", resolved);
    assert!(resolved[0].1.contains("Answered"), "{:?}", resolved);

    teardown_test_db(&db_name).await;
}

/// Neither refusal asks the talker for something it cannot do.
///
/// The reported call is this test's reason. A talker holding no answering tool
/// read "answer it with what they say" on every ask, promised the caller it
/// would, and could not. Each note now fits the talker that reads it.
#[test]
fn each_refusal_fits_the_talker_that_reads_it() {
    assert!(DELEGATION_PARKED.contains("answer it with what they say"));
    assert!(!DELEGATION_PARKED.contains("screen"));

    assert!(DELEGATION_PARKED_ON_SCREEN.contains("screen"));
    for promised in ["answer it", "hand back", "pick one"] {
        assert!(
            !DELEGATION_PARKED_ON_SCREEN.contains(promised),
            "a talker with no answering tool was asked to: {:?}",
            promised
        );
    }
}

/// The caller said they were done, so the talker rang off for them.
///
/// **The tool call comes BEFORE the goodbye's turn end**, which is the real
/// wire order: a tool call lands while the talker is still speaking. So the
/// call must survive it and close on the turn's end instead. Scripted the other
/// way round, this test would pass over a hangup that cuts the goodbye off.
#[tokio::test]
async fn the_talker_can_ring_off_when_the_caller_says_they_are_done() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = a_chat_thread(&pool).await;
    let session_id = uuid::Uuid::new_v4();

    let provider = MockVoiceProvider::new(vec![
        the_caller_says("that's all, thanks"),
        VoiceEvent::HangupRequested {
            tool_call_id: "call_h".to_string(),
        },
        the_talker_says("Speak soon."),
    ]);
    let log = provider.log();
    // Never rings off itself: the talker's own call is what ends this one.
    let mut caller = ScriptedCaller::new(vec![]);

    let reason = run_call(
        &bus,
        &provider,
        &mut caller,
        &RecordingTurns::default(),
        free_doer(),
        nobody_names(),
        opening(),
        subject(thread_id, session_id),
    )
    .await;

    assert_eq!(reason, Some(VoiceSessionEndReason::AgentHangup));

    // Acknowledged, so nothing dangles in the talker's history.
    let resolved = log.lock().unwrap().resolved_tool_calls.clone();
    assert_eq!(resolved.len(), 1, "{:?}", resolved);
    assert_eq!(resolved[0].0, "call_h");

    let events = thread_events(&pool, thread_id).await;
    let kinds: Vec<&str> = events.iter().map(|(kind, _)| kind.as_str()).collect();
    // The goodbye was said in full and written down. Ending on the tool call
    // would have cut it off mid-word and lost the row.
    assert!(
        kinds.contains(&"SpokenReplyGenerated"),
        "the goodbye was never recorded: {:?}",
        kinds
    );
    // The pair is one start and one end, exactly as a caller hangup writes.
    let session_rows: Vec<&&str> = kinds
        .iter()
        .filter(|kind| kind.starts_with("VoiceSession"))
        .collect();
    assert_eq!(
        session_rows,
        vec![&"VoiceSessionStarted", &"VoiceSessionEnded"],
        "{:?}",
        kinds
    );

    teardown_test_db(&db_name).await;
}

/// A caller who talks over the goodbye was not done, so the call stays up.
///
/// Their intent is the only thing that ends a call (ADR 0170), and taking the
/// floor back says otherwise.
#[tokio::test]
async fn a_caller_who_cuts_in_over_the_goodbye_keeps_the_call() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = a_chat_thread(&pool).await;

    let provider = MockVoiceProvider::new(vec![
        the_caller_says("that's all, thanks"),
        VoiceEvent::HangupRequested {
            tool_call_id: "call_h".to_string(),
        },
        VoiceEvent::Interrupted,
        the_talker_says("Speak s"),
        the_caller_says("actually, one more thing"),
        the_talker_says("Go on."),
    ]);
    // Rings off itself, since the talker's own call was withdrawn: the opening
    // frame, two utterances and two replies.
    let mut caller = ScriptedCaller::new(vec![]).hanging_up_after(6);

    let reason = run_call(
        &bus,
        &provider,
        &mut caller,
        &RecordingTurns::default(),
        free_doer(),
        nobody_names(),
        opening(),
        subject(thread_id, uuid::Uuid::new_v4()),
    )
    .await;

    assert_eq!(
        reason,
        Some(VoiceSessionEndReason::Hangup),
        "the withdrawn hangup ended the call anyway"
    );

    teardown_test_db(&db_name).await;
}

/// Words spent on an answer cannot later pair with a waiting ask.
///
/// Both tool calls can land before the transcript, and the answer takes the
/// words. An ask left sticky would then grab the NEXT utterance and wake the
/// doer on words asking for something else, under a stale reason.
#[tokio::test]
async fn an_ask_waiting_on_words_an_answer_spent_never_pairs_with_a_later_one() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = a_chat_thread(&pool).await;

    let decisions = NoDecisions::answering(vec![
        Resolution::NeedsTheirWords,
        Resolution::SettledWithTheirWords,
    ]);
    let provider = MockVoiceProvider::new(vec![
        // Both calls first, then the words they were both waiting for.
        answers_with("question:toolu_q0#said"),
        asks_for_the_doer("they want the second half"),
        the_caller_says("neither, do the second half only"),
        the_talker_says("Right."),
        // A later, unrelated utterance. Nothing may pair with it.
        the_caller_says("what time is it"),
        the_talker_says("Just gone eleven."),
    ]);
    let turns = RecordingTurns::default();
    let mut caller = ScriptedCaller::new(vec![]).hanging_up_after(5);

    run_call(
        &bus,
        &provider,
        &mut caller,
        &turns,
        &decisions,
        nobody_names(),
        opening(),
        subject(thread_id, uuid::Uuid::new_v4()),
    )
    .await;

    assert!(
        turns.woken().lock().unwrap().is_empty(),
        "a stale ask woke the doer: {:?}",
        turns.woken().lock().unwrap()
    );
    let kinds = voice_kinds(&thread_events(&pool, thread_id).await);
    assert!(!kinds.contains(&"WorkDelegated".to_string()), "{:?}", kinds);

    teardown_test_db(&db_name).await;
}

/// A held answer is bounded to the utterance it was made for.
///
/// The talker answers with the choice that sends the caller's words, and the
/// transcript that follows carries none. Held on, it would settle the card with
/// some later sentence about something else, and a card cannot be unsettled.
#[tokio::test]
async fn a_held_answer_gives_up_rather_than_claiming_a_later_sentence() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = a_chat_thread(&pool).await;

    let decisions = NoDecisions::answering(vec![
        Resolution::NeedsTheirWords,
        Resolution::NeedsTheirWords,
    ]);
    let asked = decisions.asked();
    let provider = MockVoiceProvider::new(vec![
        answers_with("question:toolu_q0#said"),
        // Nothing came through for it: a wordless turn.
        the_caller_says("   "),
        the_talker_says("Sorry, I missed that."),
        // A later, unrelated sentence. It must not settle the card.
        the_caller_says("what time is it"),
        the_talker_says("Just gone eleven."),
    ]);
    let log = provider.log();
    let mut caller = ScriptedCaller::new(vec![]).hanging_up_after(5);

    run_call(
        &bus,
        &provider,
        &mut caller,
        &RecordingTurns::default(),
        &decisions,
        nobody_names(),
        opening(),
        subject(thread_id, uuid::Uuid::new_v4()),
    )
    .await;

    // Two tries and no more: the call, then the utterance it was made for.
    let asked = asked.lock().unwrap().clone();
    assert_eq!(asked.len(), 2, "{:?}", asked);
    assert!(
        asked.iter().all(|(_, spoken)| spoken.is_empty()),
        "{:?}",
        asked
    );

    // And it was answered rather than left dangling in the talker's history.
    let resolved = log.lock().unwrap().resolved_tool_calls.clone();
    assert_eq!(resolved.len(), 1, "{:?}", resolved);
    assert!(resolved[0].1.contains("dropped"), "{:?}", resolved);

    teardown_test_db(&db_name).await;
}

// The call that cost seven bubbles and one answer.
//
// `docs/plans/2026-09-14-one-thing-the-caller-said-is-one-row.md` has the event
// log these three replay.

/// **The headline regression, and where it is answered now.** One sentence,
/// seven finished transcription items, twenty-one wordless talker turns.
///
/// The row's unit is the item, because that is what the engine can date
/// honestly: each one is written when the provider closed it. Putting the
/// sentence back together is a reading, and the doer's history is one of the
/// two places it happens (ADR 0201).
#[tokio::test]
async fn seven_transcription_items_are_seven_rows_and_one_sentence() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = a_chat_thread(&pool).await;
    let session_id = uuid::Uuid::new_v4();

    // The provider's own item boundaries, exactly as they landed.
    let fragments = [
        "Why",
        "didn't you",
        "tell me",
        "to",
        "restart the",
        "computer",
        "when",
    ];
    let mut script = Vec::new();
    for fragment in fragments {
        script.push(the_caller_says(fragment));
        // A talker turn that ended having said nothing, three per item, which
        // is the rate the reported call ran at.
        for _ in 0..3 {
            script.push(the_talker_says(""));
        }
    }

    let did = a_call_that_hears(&pool, &bus, thread_id, session_id, script).await;

    let rows = voice_rows(&did.events);
    assert_eq!(rows.len(), fragments.len(), "{:?}", rows);
    assert!(rows.iter().all(|(kind, _)| kind == "SpokenMessageReceived"));

    // And the doer is offered the one sentence they said.
    let store = crate::core::EventStore::new(pool.clone());
    let messages = store
        .get_thread_messages(&thread_id.to_string())
        .await
        .expect("the thread's messages read back");
    let said: Vec<&str> = messages.iter().map(|m| m.content.as_str()).collect();
    assert_eq!(
        said,
        vec!["Why didn't you tell me to restart the computer when"],
        "{:?}",
        said
    );

    teardown_test_db(&db_name).await;
}

/// The other half of the rule: a talker that DOES answer closes the row, so the
/// next thing the caller says is a row of its own.
#[tokio::test]
async fn a_talker_that_answers_closes_the_row_and_the_next_sentence_opens_one() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = a_chat_thread(&pool).await;
    let session_id = uuid::Uuid::new_v4();

    let did = a_call_that_hears(
        &pool,
        &bus,
        thread_id,
        session_id,
        vec![
            the_caller_says("what is on today"),
            the_talker_says("Two meetings."),
            the_caller_says("and tomorrow"),
            the_talker_says("One."),
        ],
    )
    .await;

    let spoken: Vec<_> = voice_rows(&did.events)
        .into_iter()
        .filter(|(kind, _)| kind == "SpokenMessageReceived")
        .map(|(_, payload)| payload["text"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(spoken, vec!["what is on today", "and tomorrow"]);

    teardown_test_db(&db_name).await;
}

/// **Defect 2: the turn never ran.** A caller asks a question and the talker
/// does nothing at all with it, which is what twenty-one wordless turns are.
///
/// The doer takes the question rather than leaving the caller in silence. The
/// caller is told on screen too, because the one route to their ear is the
/// thing that failed.
///
/// It waits out the real bound. A test-only one would leave the shipped number
/// unexercised, and the number is the whole of the behaviour.
#[tokio::test]
async fn a_caller_nobody_answers_reaches_the_doer_and_is_told_so() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = a_chat_thread(&pool).await;
    let session_id = uuid::Uuid::new_v4();

    let provider = MockVoiceProvider::new(vec![
        the_caller_says("why didn't you tell me to restart the computer"),
        // Not one word, however many turns it takes.
        the_talker_says(""),
        the_talker_says(""),
    ]);
    let turns = RecordingTurns::default();
    let woken = turns.woken();
    let rang_off = Arc::new(Notify::new());
    let mut caller = ScriptedCaller::new(vec![]).hanging_up_on(Arc::clone(&rang_off));
    let sent = caller.sent.clone();

    let call = run_call(
        &bus,
        &provider,
        &mut caller,
        &turns,
        free_doer(),
        nobody_names(),
        opening(),
        subject(thread_id, session_id),
    );
    let watcher = {
        let woken = Arc::clone(&woken);
        let rang_off = Arc::clone(&rang_off);
        async move {
            until("the doer to be woken with nobody having answered", || {
                !woken.lock().unwrap().is_empty()
            })
            .await;
            rang_off.notify_one();
        }
    };
    tokio::join!(call, watcher);

    let woken = woken.lock().unwrap().clone();
    assert_eq!(
        woken,
        vec!["why didn't you tell me to restart the computer".to_string()]
    );

    // Said out loud in the log, recorded as a row, and shown to the caller.
    let rows = voice_rows(&thread_events(&pool, thread_id).await);
    assert!(
        rows.iter().any(|(kind, payload)| kind == "WorkDelegated"
            && payload["reason"]
                .as_str()
                .unwrap_or_default()
                .contains("the talker said nothing")),
        "{:?}",
        rows
    );
    let told: Vec<_> = sent
        .lock()
        .unwrap()
        .iter()
        .filter_map(|frame| match frame {
            ServerFrame::Error { message } => Some(message.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(told.len(), 1, "{:?}", told);
    assert!(told[0].contains("not answering"), "{:?}", told[0]);

    teardown_test_db(&db_name).await;
}

/// A provider still holding the caller's words when the line closes hands them
/// back, and they reach the row.
///
/// A turn-less protocol accumulates the caller and gives them up only as the
/// socket goes. That is after the loop stopped reading, so the sentence used to
/// die with the session.
#[tokio::test]
async fn words_the_session_was_still_holding_reach_the_row() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = a_chat_thread(&pool).await;
    let session_id = uuid::Uuid::new_v4();

    let provider = MockVoiceProvider::new(vec![the_caller_says("restart the computer")])
        .still_holding(vec![the_caller_says("when it is idle")]);
    let turns = RecordingTurns::default();
    // Two deliveries: the session opening, then the caller's own turn end.
    let mut caller = ScriptedCaller::new(vec![]).hanging_up_after(2);

    run_call(
        &bus,
        &provider,
        &mut caller,
        &turns,
        free_doer(),
        nobody_names(),
        opening(),
        subject(thread_id, session_id),
    )
    .await;

    let rows = voice_rows(&thread_events(&pool, thread_id).await);
    assert_eq!(rows.len(), 2, "{:?}", rows);
    assert_eq!(rows[0].1["text"], "restart the computer");
    assert_eq!(rows[1].1["text"], "when it is idle");

    teardown_test_db(&db_name).await;
}

/// **An empty output cycle settles nothing.** A talker turn that said no words
/// delivered nothing to the caller, so it leaves no reply behind claiming they
/// heard one.
///
/// The words also stay on the undelivered pile, which is what keeps them
/// reachable by the two things that can still run on them: a delegation, or
/// the bounded wait that hands them to the doer.
#[tokio::test]
async fn an_empty_talker_turn_neither_answers_the_caller_nor_closes_their_row() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = a_chat_thread(&pool).await;
    let session_id = uuid::Uuid::new_v4();

    let did = a_call_that_hears(
        &pool,
        &bus,
        thread_id,
        session_id,
        vec![
            the_caller_says("why didn't you tell me"),
            the_talker_says(""),
            the_talker_says(""),
            the_caller_says("to restart the computer"),
            the_talker_says(""),
        ],
    )
    .await;

    // A row per turn, and nothing in between spent the words: a silent turn
    // answered nobody, so both halves still reach the doer as one sentence.
    let rows = voice_rows(&did.events);
    assert_eq!(rows.len(), 2, "{:?}", rows);
    assert_eq!(rows[0].1["text"], "why didn't you tell me");
    assert_eq!(rows[1].1["text"], "to restart the computer");
    // And nothing claims the caller heard a reply.
    let replies: Vec<_> = did
        .events
        .iter()
        .filter(|(kind, _)| kind == "SpokenReplyGenerated")
        .collect();
    assert!(
        replies.is_empty(),
        "a silent turn wrote a reply: {:?}",
        replies
    );

    teardown_test_db(&db_name).await;
}

/// **A blank talker delta holds nothing off.** Both providers forward one
/// exactly as it arrives, so a mute talker can stream them for the whole call.
/// Reading one as the talker answering is how the bound would never fire on
/// precisely the failure it exists for.
#[tokio::test]
async fn blank_talker_deltas_do_not_hold_the_bound_off() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = a_chat_thread(&pool).await;
    let session_id = uuid::Uuid::new_v4();

    let provider = MockVoiceProvider::new(vec![
        the_caller_says("why didn't you tell me to restart the computer"),
        the_talker_is_saying(""),
        the_talker_is_saying(""),
        the_talker_is_saying(""),
    ]);
    let turns = RecordingTurns::default();
    let woken = turns.woken();
    let rang_off = Arc::new(Notify::new());
    let mut caller = ScriptedCaller::new(vec![]).hanging_up_on(Arc::clone(&rang_off));

    let call = run_call(
        &bus,
        &provider,
        &mut caller,
        &turns,
        free_doer(),
        nobody_names(),
        opening(),
        subject(thread_id, session_id),
    );
    let watcher = {
        let woken = Arc::clone(&woken);
        let rang_off = Arc::clone(&rang_off);
        async move {
            until("the doer to be woken past a stream of blank deltas", || {
                !woken.lock().unwrap().is_empty()
            })
            .await;
            rang_off.notify_one();
        }
    };
    tokio::join!(call, watcher);

    assert_eq!(
        woken.lock().unwrap().clone(),
        vec!["why didn't you tell me to restart the computer".to_string()]
    );

    teardown_test_db(&db_name).await;
}

/// **The words the bound spends are spent everywhere.** A provider with no
/// turn-end frame keeps its own copy, so the engine tells it they are gone.
/// Left holding them, its next boundary draws the same breath a second time.
#[tokio::test]
async fn words_the_bound_spends_are_not_written_twice() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = a_chat_thread(&pool).await;
    let session_id = uuid::Uuid::new_v4();

    // What a turn-less provider does: partials only, and the same sentence
    // handed over again when the socket closes.
    let provider = MockVoiceProvider::new(vec![
        the_caller_is_saying("why didn't you tell me "),
        the_caller_is_saying("to restart the computer"),
    ])
    .still_holding(vec![the_caller_says(
        "why didn't you tell me to restart the computer",
    )]);
    let turns = RecordingTurns::default();
    let woken = turns.woken();
    let rang_off = Arc::new(Notify::new());
    let mut caller = ScriptedCaller::new(vec![]).hanging_up_on(Arc::clone(&rang_off));

    let call = run_call(
        &bus,
        &provider,
        &mut caller,
        &turns,
        free_doer(),
        nobody_names(),
        opening(),
        subject(thread_id, session_id),
    );
    let watcher = {
        let woken = Arc::clone(&woken);
        let rang_off = Arc::clone(&rang_off);
        async move {
            until("the doer to take the unanswered words", || {
                !woken.lock().unwrap().is_empty()
            })
            .await;
            rang_off.notify_one();
        }
    };
    tokio::join!(call, watcher);

    // One wake, and ONE row for the breath. The session's own copy came back
    // after the words were spent, and must not draw it a second time.
    let rows = voice_rows(&thread_events(&pool, thread_id).await);
    let spoken: Vec<_> = rows
        .iter()
        .filter(|(kind, _)| kind == "SpokenMessageReceived")
        .collect();
    assert_eq!(spoken.len(), 1, "the breath was drawn twice: {:?}", rows);
    assert_eq!(
        spoken[0].1["text"],
        "why didn't you tell me to restart the computer"
    );
    assert_eq!(
        woken.lock().unwrap().clone(),
        vec!["why didn't you tell me to restart the computer".to_string()]
    );

    teardown_test_db(&db_name).await;
}

/// A doer parked inside a card has no turn to start, so the bound does not
/// pretend otherwise. The caller is still told, and their words still land.
#[tokio::test]
async fn a_parked_doer_is_not_woken_by_the_bound_and_the_caller_is_told() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = a_chat_thread(&pool).await;
    let session_id = uuid::Uuid::new_v4();

    let provider = MockVoiceProvider::new(vec![the_caller_says("the second one")]);
    let turns = RecordingTurns::default();
    let woken = turns.woken();
    let parked = NoDecisions::parked();
    let rang_off = Arc::new(Notify::new());
    let mut caller = ScriptedCaller::new(vec![]).hanging_up_on(Arc::clone(&rang_off));
    let sent = caller.sent.clone();

    let call = run_call(
        &bus,
        &provider,
        &mut caller,
        &turns,
        &parked,
        nobody_names(),
        opening(),
        subject(thread_id, session_id),
    );
    let watcher = {
        let sent = Arc::clone(&sent);
        let rang_off = Arc::clone(&rang_off);
        async move {
            until("the caller to be told the voice is not answering", || {
                sent.lock()
                    .unwrap()
                    .iter()
                    .any(|f| matches!(f, ServerFrame::Error { .. }))
            })
            .await;
            rang_off.notify_one();
        }
    };
    tokio::join!(call, watcher);

    assert!(woken.lock().unwrap().is_empty(), "a parked doer was woken");
    let rows = voice_rows(&thread_events(&pool, thread_id).await);
    assert_eq!(rows.len(), 1, "{:?}", rows);
    assert_eq!(rows[0].0, "SpokenMessageReceived");
    assert_eq!(rows[0].1["text"], "the second one");

    teardown_test_db(&db_name).await;
}

/// **An ask with no words behind it does not wait for ever.**
///
/// A Live delegation frame carries an id and no text, so the ask is held until
/// a transcript pairs with it. One that never arrives used to leave the caller
/// with a promise, silence, and nothing running. The plan is
/// `docs/plans/2026-09-16-a-live-call-delegates-what-it-promised.md`.
#[tokio::test]
async fn an_ask_whose_words_never_came_tells_the_caller() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = a_chat_thread(&pool).await;
    let session_id = uuid::Uuid::new_v4();

    let provider = MockVoiceProvider::new(vec![asks_for_the_doer("what is going on")]);
    let log = provider.log();
    let turns = RecordingTurns::default();
    let woken = turns.woken();
    let rang_off = Arc::new(Notify::new());
    let mut caller = ScriptedCaller::new(vec![]).hanging_up_on(Arc::clone(&rang_off));

    let call = run_call(
        &bus,
        &provider,
        &mut caller,
        &turns,
        free_doer(),
        nobody_names(),
        opening(),
        subject(thread_id, session_id),
    );
    let watcher = {
        let log = Arc::clone(&log);
        let rang_off = Arc::clone(&rang_off);
        async move {
            until("the caller to be told nothing came through", || {
                log.lock()
                    .unwrap()
                    .asked_to_speak
                    .iter()
                    .any(|note| note == ASK_NEVER_GOT_ITS_WORDS)
            })
            .await;
            rang_off.notify_one();
        }
    };
    tokio::join!(call, watcher);

    assert!(
        woken.lock().unwrap().is_empty(),
        "the doer was woken on words nobody had"
    );
    let rows = voice_rows(&thread_events(&pool, thread_id).await);
    assert!(
        rows.is_empty(),
        "a turn was claimed with nothing behind it: {:?}",
        rows
    );

    teardown_test_db(&db_name).await;
}

// A call opens silent (ADR 0211).

/// The defect, scripted: a talker that answers its own opening block.
///
/// The caller says nothing, and the talker says five things at them. Nothing
/// it says may be played, framed, written down or offered to a running round.
#[tokio::test]
async fn a_talker_that_speaks_before_the_caller_reaches_nobody() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = a_chat_thread(&pool).await;

    let provider = MockVoiceProvider::new(vec![
        VoiceEvent::Audio(vec![7; 960]),
        the_talker_is_saying("Yeah, on it"),
        VoiceEvent::Audio(vec![7; 960]),
        the_talker_says("Yeah, on it"),
    ]);
    let turns = RecordingTurns::default();
    let overheard = turns.overheard();
    // SessionStarted, then the turn end, which is a frame whatever the
    // floor did with the words. Ringing off on the opening frame alone would
    // pass this without the script ever running.
    let mut caller = ScriptedCaller::new(vec![]).hanging_up_after(2);
    let sent = Arc::clone(&caller.sent);
    let audio_out = Arc::clone(&caller.audio_out_bytes);

    run_call(
        &bus,
        &provider,
        &mut caller,
        &turns,
        free_doer(),
        nobody_names(),
        opening(),
        subject(thread_id, uuid::Uuid::new_v4()),
    )
    .await;

    assert_eq!(*audio_out.lock().unwrap(), 0, "the caller heard it");
    let frames = sent.lock().unwrap().clone();
    assert!(
        !frames
            .iter()
            .any(|frame| matches!(frame, ServerFrame::TalkerTranscript { .. })),
        "{:?}",
        frames
    );
    let events = thread_events(&pool, thread_id).await;
    assert!(
        !events
            .iter()
            .any(|(kind, _)| kind == "SpokenReplyGenerated"),
        "{:?}",
        events
    );
    assert!(overheard.lock().unwrap().is_empty());

    teardown_test_db(&db_name).await;
}

/// The other side of it: once they have spoken, the reply lands as it always
/// did.
#[tokio::test]
async fn the_callers_first_word_opens_the_floor() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = a_chat_thread(&pool).await;

    let provider = MockVoiceProvider::new(vec![
        the_caller_says("what have I got running"),
        the_talker_is_saying("Two threads"),
        VoiceEvent::Audio(vec![7; 480]),
        the_talker_says("Two threads"),
    ]);
    let mut caller = ScriptedCaller::new(vec![]).hanging_up_after(5);
    let audio_out = Arc::clone(&caller.audio_out_bytes);

    run_call(
        &bus,
        &provider,
        &mut caller,
        &RecordingTurns::default(),
        free_doer(),
        nobody_names(),
        opening(),
        subject(thread_id, uuid::Uuid::new_v4()),
    )
    .await;

    assert_eq!(*audio_out.lock().unwrap(), 480);
    let replies = rows_of(&pool, thread_id, "SpokenReplyGenerated").await;
    assert_eq!(replies.len(), 1, "{:?}", replies);
    assert_eq!(replies[0].0["text"], "Two threads");

    teardown_test_db(&db_name).await;
}

/// Their first PARTIAL opens it, not their full stop.
///
/// A caller mid-sentence is already somebody to answer, and a reply held until
/// they finish is one the shut floor would have eaten.
#[tokio::test]
async fn a_partial_is_enough_to_open_the_floor() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = a_chat_thread(&pool).await;

    let provider = MockVoiceProvider::new(vec![
        the_caller_is_saying("what have"),
        the_talker_is_saying("Checking"),
        the_talker_says("Checking"),
    ]);
    let mut caller = ScriptedCaller::new(vec![]).hanging_up_after(4);

    run_call(
        &bus,
        &provider,
        &mut caller,
        &RecordingTurns::default(),
        free_doer(),
        nobody_names(),
        opening(),
        subject(thread_id, uuid::Uuid::new_v4()),
    )
    .await;

    let replies = rows_of(&pool, thread_id, "SpokenReplyGenerated").await;
    assert_eq!(replies.len(), 1, "{:?}", replies);

    teardown_test_db(&db_name).await;
}

/// A blank partial is the provider's silence, not the caller's word.
#[tokio::test]
async fn a_blank_partial_leaves_the_floor_shut() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = a_chat_thread(&pool).await;

    let provider = MockVoiceProvider::new(vec![
        the_caller_is_saying("   "),
        the_talker_is_saying("Yeah, on it"),
        the_talker_says("Yeah, on it"),
    ]);
    let mut caller = ScriptedCaller::new(vec![]).hanging_up_after(3);

    run_call(
        &bus,
        &provider,
        &mut caller,
        &RecordingTurns::default(),
        free_doer(),
        nobody_names(),
        opening(),
        subject(thread_id, uuid::Uuid::new_v4()),
    )
    .await;

    let events = thread_events(&pool, thread_id).await;
    assert!(
        !events
            .iter()
            .any(|(kind, _)| kind == "SpokenReplyGenerated"),
        "{:?}",
        events
    );

    teardown_test_db(&db_name).await;
}

/// Wordless audio decides no turn, so it cannot latch a call silent.
///
/// A Live talker streams audio between turns. Read as a turn, the stream would
/// be decided at call open and never decided again (ADR 0187).
#[tokio::test]
async fn audio_before_the_caller_speaks_latches_nothing() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = a_chat_thread(&pool).await;

    let provider = MockVoiceProvider::new(vec![
        VoiceEvent::Audio(vec![7; 960]),
        VoiceEvent::Audio(vec![7; 960]),
        the_caller_says("what have I got running"),
        the_talker_is_saying("Two threads"),
        VoiceEvent::Audio(vec![1; 320]),
        the_talker_says("Two threads"),
    ]);
    let mut caller = ScriptedCaller::new(vec![]).hanging_up_after(5);
    let audio_out = Arc::clone(&caller.audio_out_bytes);

    run_call(
        &bus,
        &provider,
        &mut caller,
        &RecordingTurns::default(),
        free_doer(),
        nobody_names(),
        opening(),
        subject(thread_id, uuid::Uuid::new_v4()),
    )
    .await;

    // The two chunks before their word are dropped, and the one after is not.
    assert_eq!(*audio_out.lock().unwrap(), 320);
    let replies = rows_of(&pool, thread_id, "SpokenReplyGenerated").await;
    assert_eq!(replies.len(), 1, "{:?}", replies);

    teardown_test_db(&db_name).await;
}

/// A card parked on the caller is put to them, and that reply IS heard.
///
/// The carve-out the shut floor turns on. Nothing else is coming for them,
/// because the turn behind the card is parked on a person (ADR 0185). A gate
/// that swallowed this would leave a silent caller waiting forever.
#[tokio::test]
async fn a_turn_the_engine_asked_for_is_heard_on_a_silent_call() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = a_chat_thread(&pool).await;

    let turns = RecordingTurns::default();
    let (provider, talker) = MockVoiceProvider::driven();
    let log = provider.log();
    let hang_up = Arc::new(Notify::new());
    let mut caller = ScriptedCaller::new(vec![]).hanging_up_on(Arc::clone(&hang_up));
    let sent = Arc::clone(&caller.sent);
    let audio_out = Arc::clone(&caller.audio_out_bytes);

    tokio::join!(
        run_call(
            &bus,
            &provider,
            &mut caller,
            &turns,
            free_doer(),
            nobody_names(),
            opening(),
            subject(thread_id, uuid::Uuid::new_v4()),
        ),
        async {
            until("the session to open", || {
                log.lock().unwrap().openings.len() == 1
            })
            .await;
            a_turn_starts(&bus, thread_id).await;
            seed_thread_event(&bus, thread_id, asks_the_caller()).await;
            until("the question to be handed to the talker", || {
                log.lock().unwrap().asked_to_speak.len() == 1
            })
            .await;
            say(&talker, the_talker_is_saying("There's one waiting for you")).await;
            say(&talker, VoiceEvent::Audio(vec![3; 240])).await;
            say(&talker, the_talker_says("There's one waiting for you")).await;
            await_frames(&sent, 2, "the relayed reply to end").await;
            hang_up.notify_one();
        }
    );

    assert_eq!(*audio_out.lock().unwrap(), 240);
    let replies = rows_of(&pool, thread_id, "SpokenReplyGenerated").await;
    assert_eq!(replies.len(), 1, "{:?}", replies);
    assert_eq!(replies[0].0["text"], "There's one waiting for you");

    teardown_test_db(&db_name).await;
}

/// A turn that began unheard stays unheard for the whole of its length.
///
/// Decided once, by its first word. Re-asked per delta, the caller would hear
/// the back half of a sentence whose front half was dropped.
#[tokio::test]
async fn a_turn_decided_unheard_does_not_resume_when_the_caller_speaks() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = a_chat_thread(&pool).await;

    let turns = RecordingTurns::default();
    let (provider, talker) = MockVoiceProvider::driven();
    let log = provider.log();
    let (mut caller, from_caller) = ScriptedCaller::driven();
    let sent = Arc::clone(&caller.sent);
    let audio_out = Arc::clone(&caller.audio_out_bytes);

    tokio::join!(
        run_call(
            &bus,
            &provider,
            &mut caller,
            &turns,
            free_doer(),
            nobody_names(),
            opening(),
            subject(thread_id, uuid::Uuid::new_v4()),
        ),
        async {
            until("the session to open", || {
                log.lock().unwrap().openings.len() == 1
            })
            .await;
            say(&talker, the_talker_is_saying("You're on that thread")).await;
            say(&talker, the_caller_says("hold on")).await;
            await_frames(&sent, 1, "the caller's own turn to land").await;
            // Said after their word, and still part of the turn before it.
            say(&talker, the_talker_is_saying(" now.")).await;
            say(&talker, VoiceEvent::Audio(vec![5; 160])).await;
            say(&talker, the_talker_says("You're on that thread now.")).await;
            await_frames(&sent, 2, "the unheard turn to end").await;
            hang_up(&from_caller).await;
        }
    );

    assert_eq!(*audio_out.lock().unwrap(), 0, "the caller heard the tail");
    let frames = sent.lock().unwrap().clone();
    assert!(
        !frames
            .iter()
            .any(|frame| matches!(frame, ServerFrame::TalkerTranscript { .. })),
        "{:?}",
        frames
    );
    let events = thread_events(&pool, thread_id).await;
    assert!(
        !events
            .iter()
            .any(|(kind, _)| kind == "SpokenReplyGenerated"),
        "{:?}",
        events
    );

    teardown_test_db(&db_name).await;
}

/// **The reported defect.** The caller heard the back half of a muted sentence.
///
/// A Live turn ends at every 700 ms hole in the words (ADR 0187), so the latch
/// above expires inside a sentence. The talker opened the call mid-thought, the
/// caller spoke over it, and the next fragment of that same sentence reached
/// their ear and the thread. The row began "doc. It says it's intentional",
/// which answers nothing anybody asked. The plan is
/// `docs/plans/2026-09-18-a-muted-answer-is-muted-whole.md`.
#[tokio::test]
async fn a_muted_run_is_not_heard_from_its_middle() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = a_chat_thread(&pool).await;

    let provider = MockVoiceProvider::new(vec![
        // Before the caller's first word, so nobody hears it. It stops
        // mid-sentence, and the turn after it carries the rest.
        the_talker_is_saying("Preflight flagged the deletion of that tap-shape migration"),
        the_caller_says("What about"),
        the_talker_says("Preflight flagged the deletion of that tap-shape migration"),
        the_talker_is_saying("doc. It says it's intentional."),
        VoiceEvent::Audio(vec![3; 320]),
        the_talker_says("doc. It says it's intentional."),
    ]);
    let turns = RecordingTurns::default();
    let overheard = turns.overheard();
    // SessionStarted, the caller's own turn, and one frame per talker turn end.
    // The muted words and the muted audio are no frame at all.
    let mut caller = ScriptedCaller::new(vec![]).hanging_up_after(4);
    let sent = Arc::clone(&caller.sent);
    let audio_out = Arc::clone(&caller.audio_out_bytes);

    run_call(
        &bus,
        &provider,
        &mut caller,
        &turns,
        free_doer(),
        nobody_names(),
        opening(),
        subject(thread_id, uuid::Uuid::new_v4()),
    )
    .await;

    assert_eq!(*audio_out.lock().unwrap(), 0, "the caller heard the tail");
    let frames = sent.lock().unwrap().clone();
    assert!(
        !frames
            .iter()
            .any(|frame| matches!(frame, ServerFrame::TalkerTranscript { .. })),
        "{:?}",
        frames
    );
    let events = thread_events(&pool, thread_id).await;
    assert!(
        !events
            .iter()
            .any(|(kind, _)| kind == "SpokenReplyGenerated"),
        "{:?}",
        events
    );
    assert!(overheard.lock().unwrap().is_empty());

    teardown_test_db(&db_name).await;
}

/// The other side of it: a muted run that FINISHED frees the next turn.
///
/// The hold is about a sentence, not about the call. A talker muted through a
/// whole thought is heard again on its next one, which is the ordinary way a
/// silent call starts working.
#[tokio::test]
async fn a_finished_muted_run_lets_the_next_turn_be_heard() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = a_chat_thread(&pool).await;

    let provider = MockVoiceProvider::new(vec![
        the_talker_is_saying("Something else on your mind?"),
        the_caller_says("Please"),
        the_talker_says("Something else on your mind?"),
        the_talker_is_saying("Two threads are running."),
        VoiceEvent::Audio(vec![7; 480]),
        the_talker_says("Two threads are running."),
    ]);
    // SessionStarted, the caller's turn, the muted turn's end, then the heard
    // turn's delta, audio and end. The muted delta is no delivery. Stopping
    // short of the last one leaves the hangup racing it.
    let mut caller = ScriptedCaller::new(vec![]).hanging_up_after(6);
    let audio_out = Arc::clone(&caller.audio_out_bytes);

    run_call(
        &bus,
        &provider,
        &mut caller,
        &RecordingTurns::default(),
        free_doer(),
        nobody_names(),
        opening(),
        subject(thread_id, uuid::Uuid::new_v4()),
    )
    .await;

    assert_eq!(*audio_out.lock().unwrap(), 480);
    let replies = rows_of(&pool, thread_id, "SpokenReplyGenerated").await;
    assert_eq!(replies.len(), 1, "{:?}", replies);
    assert_eq!(replies[0].0["text"], "Two threads are running.");

    teardown_test_db(&db_name).await;
}

/// **The reported defect, on the caller's side.** Half their question was eaten.
///
/// The transcriber split "What about now" one second apart. A heard turn
/// between the halves says the talker answered the first, so the pile is
/// dropped and the doer runs on the rest. The reported wake carried the single
/// word "now", and the answer was about something else entirely.
///
/// A muted run answers nobody, so it spends nothing. This needs no code of its
/// own: the flag is already gated on the turn being heard.
#[tokio::test]
async fn a_muted_run_spends_none_of_the_callers_words() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = a_chat_thread(&pool).await;

    let did = a_call_that_hears(
        &pool,
        &bus,
        thread_id,
        uuid::Uuid::new_v4(),
        vec![
            the_talker_is_saying("Preflight flagged the deletion of that tap-shape migration"),
            the_caller_says("What about"),
            the_talker_says("Preflight flagged the deletion of that tap-shape migration"),
            the_talker_is_saying("doc. It says it's intentional."),
            the_talker_says("doc. It says it's intentional."),
            the_caller_says("now"),
            asks_for_the_doer(""),
        ],
    )
    .await;

    assert_eq!(did.woken, vec!["What about now".to_string()]);

    teardown_test_db(&db_name).await;
}

/// An unheard turn answered nobody, so the caller is still owed one.
///
/// The bound is what sends them to the doer instead. Read as an answer, a
/// babble the caller never heard would disarm it and leave them in silence
/// until they hang up (ADR 0185).
///
/// It waits out the real bound, like every other case about that number.
#[tokio::test]
async fn an_unheard_turn_leaves_the_caller_still_owed_an_answer() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = a_chat_thread(&pool).await;

    let (provider, talker) = MockVoiceProvider::driven();
    let log = provider.log();
    let turns = RecordingTurns::default();
    let woken = turns.woken();
    let (mut caller, from_caller) = ScriptedCaller::driven();
    let sent = Arc::clone(&caller.sent);

    tokio::join!(
        run_call(
            &bus,
            &provider,
            &mut caller,
            &turns,
            free_doer(),
            nobody_names(),
            opening(),
            subject(thread_id, uuid::Uuid::new_v4()),
        ),
        async {
            until("the session to open", || {
                log.lock().unwrap().openings.len() == 1
            })
            .await;
            say(&talker, the_talker_is_saying("Yeah, on it")).await;
            say(&talker, the_caller_says("why is the build red")).await;
            await_frames(&sent, 1, "the caller's own turn to land").await;
            say(&talker, the_talker_says("Yeah, on it")).await;
            until("the doer to be woken with nobody having answered", || {
                !woken.lock().unwrap().is_empty()
            })
            .await;
            hang_up(&from_caller).await;
        }
    );

    assert_eq!(
        woken.lock().unwrap().clone(),
        vec!["why is the build red".to_string()]
    );

    teardown_test_db(&db_name).await;
}

/// A relayed answer gets its row even when the provider streamed no delta.
///
/// The regression the floor nearly introduced. `Call::say` spends its relay
/// flag at the turn end, and a turn with no delta never latched an audience of
/// its own. Asked again at the row, such a turn read as unheard, so the caller
/// heard the answer and the thread kept no record of it.
#[tokio::test]
async fn a_relayed_turn_with_no_delta_still_gets_its_row() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = a_chat_thread(&pool).await;

    let (provider, talker) = MockVoiceProvider::driven();
    let log = provider.log();
    let turns = RecordingTurns::default();
    let hang_up = Arc::new(Notify::new());
    let mut caller = ScriptedCaller::new(vec![]).hanging_up_on(Arc::clone(&hang_up));
    let sent = Arc::clone(&caller.sent);

    tokio::join!(
        run_call(
            &bus,
            &provider,
            &mut caller,
            &turns,
            free_doer(),
            nobody_names(),
            opening(),
            subject(thread_id, uuid::Uuid::new_v4()),
        ),
        async {
            until("the session to open", || {
                log.lock().unwrap().openings.len() == 1
            })
            .await;
            // Nobody has spoken, so the relay is the only thing opening the
            // floor for this turn.
            a_turn_starts(&bus, thread_id).await;
            seed_thread_event(&bus, thread_id, asks_the_caller()).await;
            until("the question to be handed to the talker", || {
                log.lock().unwrap().asked_to_speak.len() == 1
            })
            .await;
            // The provider reports the whole turn at its end, having streamed
            // nothing. That is what `last_turn_transcript` stands in for.
            say(&talker, the_talker_says("There's one waiting for you")).await;
            await_frames(&sent, 1, "the relayed reply to end").await;
            hang_up.notify_one();
        }
    );

    let replies = rows_of(&pool, thread_id, "SpokenReplyGenerated").await;
    assert_eq!(replies.len(), 1, "the relayed answer reached no row");
    assert_eq!(replies[0].0["text"], "There's one waiting for you");

    teardown_test_db(&db_name).await;
}

/// A relayed answer survives the pause in the middle of it.
///
/// The floor is STICKY, and this is why. A Live turn ends at every 700 ms hole
/// in the talker's words, so one answer spans several. Spent per turn, the
/// engine's own answer went audible for one sentence and then cut out on a
/// caller who had said nothing.
#[tokio::test]
async fn a_relayed_answer_stays_audible_across_the_pause_in_it() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = a_chat_thread(&pool).await;

    let (provider, talker) = MockVoiceProvider::driven();
    let log = provider.log();
    let turns = RecordingTurns::default();
    let hang_up = Arc::new(Notify::new());
    let mut caller = ScriptedCaller::new(vec![]).hanging_up_on(Arc::clone(&hang_up));
    let sent = Arc::clone(&caller.sent);
    let audio_out = Arc::clone(&caller.audio_out_bytes);

    tokio::join!(
        run_call(
            &bus,
            &provider,
            &mut caller,
            &turns,
            free_doer(),
            nobody_names(),
            opening(),
            subject(thread_id, uuid::Uuid::new_v4()),
        ),
        async {
            until("the session to open", || {
                log.lock().unwrap().openings.len() == 1
            })
            .await;
            // Nobody has spoken. The card is what opens the floor.
            a_turn_starts(&bus, thread_id).await;
            seed_thread_event(&bus, thread_id, asks_the_caller()).await;
            until("the question to be handed to the talker", || {
                log.lock().unwrap().asked_to_speak.len() == 1
            })
            .await;

            // The first breath of the answer.
            say(&talker, the_talker_is_saying("There's one waiting")).await;
            say(&talker, the_talker_says("There's one waiting")).await;
            await_frames(&sent, 2, "the first breath to end").await;

            // The hole in the words ended that turn, and the same answer
            // carries on into the next one.
            say(&talker, the_talker_is_saying("Run the tail, or leave it?")).await;
            say(&talker, VoiceEvent::Audio(vec![9; 128])).await;
            say(&talker, the_talker_says("Run the tail, or leave it?")).await;
            await_frames(&sent, 4, "the rest of the answer to end").await;
            hang_up.notify_one();
        }
    );

    assert_eq!(*audio_out.lock().unwrap(), 128, "the rest went unheard");
    let replies = rows_of(&pool, thread_id, "SpokenReplyGenerated").await;
    let said: Vec<&str> = replies
        .iter()
        .map(|(payload, _)| payload["text"].as_str().unwrap_or_default())
        .collect();
    assert_eq!(
        said,
        vec!["There's one waiting", "Run the tail, or leave it?"]
    );

    teardown_test_db(&db_name).await;
}

/// **The caller's own DEVICE hearing them start is enough too.**
///
/// The provider signal below reaches one provider. A Live session states no
/// such frame, and its talker answers the caller's audio before its own
/// transcriber reports a word. So the answer to their first sentence was
/// played to nobody. The client measures the same edge for its own bubble
/// (`voice/speechGate.ts`), and sends it.
#[tokio::test]
async fn the_callers_own_device_opens_the_floor() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = a_chat_thread(&pool).await;

    let (provider, talker) = MockVoiceProvider::driven();
    let log = provider.log();
    let turns = RecordingTurns::default();
    let (mut caller, from_caller) = ScriptedCaller::driven();
    let sent = Arc::clone(&caller.sent);
    let audio_out = Arc::clone(&caller.audio_out_bytes);

    tokio::join!(
        run_call(
            &bus,
            &provider,
            &mut caller,
            &turns,
            free_doer(),
            nobody_names(),
            opening(),
            subject(thread_id, uuid::Uuid::new_v4()),
        ),
        async {
            until("the session to open", || {
                log.lock().unwrap().openings.len() == 1
            })
            .await;
            // **Taken before the talker speaks, by construction.** The signal
            // and the talker's words come from two senders, so nothing orders
            // them but this: the channel emptying says the call has it. Sent
            // and then raced, the turn is muted and the case reads as a
            // failure of the floor rather than of its own sequencing.
            let idle = from_caller.capacity();
            from_caller
                .send(CallerFrame::Control(ClientControl::CallerStartedSpeaking))
                .await
                .expect("the call to take the caller's signal");
            until("the call to take that signal", || {
                from_caller.capacity() == idle
            })
            .await;
            say(&talker, the_talker_is_saying("Two threads")).await;
            say(&talker, VoiceEvent::Audio(vec![4; 256])).await;
            say(&talker, the_talker_says("Two threads")).await;
            await_frames(&sent, 2, "the reply to end").await;
            hang_up(&from_caller).await;
        }
    );

    assert_eq!(*audio_out.lock().unwrap(), 256);
    let replies = rows_of(&pool, thread_id, "SpokenReplyGenerated").await;
    assert_eq!(replies.len(), 1, "{:?}", replies);
    assert_eq!(replies[0].0["text"], "Two threads");
    // It says they opened their mouth, and nothing more. Read as a cut, it
    // would report one on every turn, this one included (ADR 0211).
    assert_eq!(replies[0].0["interrupted"], false);

    teardown_test_db(&db_name).await;
}

/// It is not an interruption, even landing mid-reply.
///
/// It fires on the first word of a call with nothing playing, so reading it as
/// a cut would report one on every turn. The client detects its own barge-in
/// and sends that separately, under its own quiet-time condition.
#[tokio::test]
async fn the_callers_first_sound_cuts_nothing_off() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = a_chat_thread(&pool).await;

    let (provider, talker) = MockVoiceProvider::driven();
    let log = provider.log();
    let turns = RecordingTurns::default();
    let (mut caller, from_caller) = ScriptedCaller::driven();
    let sent = Arc::clone(&caller.sent);
    let audio_out = Arc::clone(&caller.audio_out_bytes);

    tokio::join!(
        run_call(
            &bus,
            &provider,
            &mut caller,
            &turns,
            free_doer(),
            nobody_names(),
            opening(),
            subject(thread_id, uuid::Uuid::new_v4()),
        ),
        async {
            until("the session to open", || {
                log.lock().unwrap().openings.len() == 1
            })
            .await;
            say(&talker, the_caller_says("what have I got running")).await;
            await_frames(&sent, 1, "the caller's own turn to land").await;
            say(&talker, the_talker_is_saying("Two threads")).await;
            await_frames(&sent, 2, "the reply to start").await;
            // Mid-reply, which is where a cut would show.
            from_caller
                .send(CallerFrame::Control(ClientControl::CallerStartedSpeaking))
                .await
                .expect("the caller to say they started");
            say(&talker, VoiceEvent::Audio(vec![9; 192])).await;
            say(&talker, the_talker_says("Two threads")).await;
            await_frames(&sent, 3, "the reply to end").await;
            hang_up(&from_caller).await;
        }
    );

    assert_eq!(*audio_out.lock().unwrap(), 192, "the rest was cut off");
    let replies = rows_of(&pool, thread_id, "SpokenReplyGenerated").await;
    assert_eq!(replies.len(), 1, "{:?}", replies);
    assert_eq!(replies[0].0["interrupted"], false);
    let frames = sent.lock().unwrap().clone();
    assert!(
        !frames
            .iter()
            .any(|frame| matches!(frame, ServerFrame::Interrupted)),
        "{:?}",
        frames
    );

    teardown_test_db(&db_name).await;
}

/// The provider hearing the caller start is enough, with no transcript at all.
///
/// The opener a transcriber cannot withhold. `whisper-1` streams no partials,
/// and its completed frame is asynchronous, so it can land after the reply it
/// prompted. Waiting for one drops the answer to the caller's first sentence.
#[tokio::test]
async fn the_caller_starting_to_speak_opens_the_floor_with_no_transcript() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = a_chat_thread(&pool).await;

    let provider = MockVoiceProvider::new(vec![
        VoiceEvent::CallerStartedSpeaking,
        the_talker_is_saying("Two threads"),
        VoiceEvent::Audio(vec![4; 256]),
        the_talker_says("Two threads"),
    ]);
    // SessionStarted, the delta, then the turn end. The speech signal draws
    // the caller nothing, so it is no delivery.
    let mut caller = ScriptedCaller::new(vec![]).hanging_up_after(4);
    let audio_out = Arc::clone(&caller.audio_out_bytes);
    let sent = Arc::clone(&caller.sent);

    run_call(
        &bus,
        &provider,
        &mut caller,
        &RecordingTurns::default(),
        free_doer(),
        nobody_names(),
        opening(),
        subject(thread_id, uuid::Uuid::new_v4()),
    )
    .await;

    assert_eq!(*audio_out.lock().unwrap(), 256);
    let replies = rows_of(&pool, thread_id, "SpokenReplyGenerated").await;
    assert_eq!(replies.len(), 1, "{:?}", replies);
    assert_eq!(replies[0].0["text"], "Two threads");
    // It says the caller started, and the client already draws that off its
    // own microphone. A frame here would be telling it what it knows.
    let frames = sent.lock().unwrap().clone();
    assert!(
        !frames
            .iter()
            .any(|frame| matches!(frame, ServerFrame::UserTranscript { .. })),
        "{:?}",
        frames
    );

    teardown_test_db(&db_name).await;
}

// One opener buys one answer (ADR 0213).

/// The talker's own words for the recitation test, one per turn.
///
/// Distinct, so an assertion says WHICH turn fell off the end rather than how
/// many did. They are the reported call's lines, shortened.
const RECITED: &[&str] = &[
    "I'm good, but I'm waiting on you.",
    "I'm still waiting for your answer.",
    "I do hand things over when you ask.",
    "On it! I'm opening it up again.",
    "Checking.",
    "Ah. It's because it's waiting for your answer.",
    "Sure, checking.",
    "Right, it was in the trigger's own thread.",
    "And now I see why.",
    "Okay, checking that.",
    "Right, that one's to do with the MCP server.",
];

/// The defect, scripted: one hello and then a monologue.
///
/// The reported call ran like this. The caller said one thing, the floor
/// opened, and the talker recited the thread's own history at them for
/// forty-nine seconds. A held floor licensed every word of it.
///
/// The bound is what ends it. The turns it bought are heard in full, and the
/// rest reach no ear and no row.
#[tokio::test]
async fn a_run_of_turns_on_one_opener_is_cut_off_at_the_bound() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = a_chat_thread(&pool).await;

    let bound = super::TURNS_ONE_OPENER_BUYS as usize;
    assert!(RECITED.len() > bound, "the script must outrun the bound");
    let mut script = vec![the_caller_says("okay, how are we")];
    script.extend(RECITED.iter().map(|line| the_talker_says(line)));

    let did = a_call_that_hears(&pool, &bus, thread_id, uuid::Uuid::new_v4(), script).await;

    let said: Vec<String> = replies(&did.events)
        .iter()
        .map(|reply| reply["text"].as_str().unwrap_or_default().to_string())
        .collect();
    let heard_in_full: Vec<String> = RECITED[..bound].iter().map(|s| s.to_string()).collect();
    assert_eq!(said, heard_in_full, "{:?}", said);

    teardown_test_db(&db_name).await;
}

/// A caller who speaks again buys the next answer.
///
/// The bound silences a monologue, never the conversation. One word from them
/// hands the talker a whole budget back, which is why a working call never
/// meets the bound at all.
#[tokio::test]
async fn the_caller_speaking_again_buys_another_answer() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = a_chat_thread(&pool).await;

    let bound = super::TURNS_ONE_OPENER_BUYS as usize;
    let mut script = vec![the_caller_says("okay, how are we")];
    script.extend(RECITED.iter().take(bound).map(|line| the_talker_says(line)));
    // Spent. Nothing the talker says now is heard, until this lands.
    script.push(the_talker_says("nobody hears this one"));
    script.push(the_caller_says("stop, listen to me"));
    script.push(the_talker_says("Sorry. Go on."));

    let did = a_call_that_hears(&pool, &bus, thread_id, uuid::Uuid::new_v4(), script).await;

    let said: Vec<String> = replies(&did.events)
        .iter()
        .map(|reply| reply["text"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(said.len(), bound + 1, "{:?}", said);
    assert_eq!(said.last().map(String::as_str), Some("Sorry. Go on."));
    assert!(
        !said.iter().any(|text| text == "nobody hears this one"),
        "{:?}",
        said
    );

    teardown_test_db(&db_name).await;
}

/// A turn the bound muted is still billed, and still owes the caller.
///
/// We were billed for it whoever heard it, so its usage is recorded exactly as
/// an opening babble's is (ADR 0211). And it answered nobody, so the doer is
/// never told the caller was told anything.
#[tokio::test]
async fn a_turn_the_bound_muted_is_still_billed_and_reaches_no_round() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = a_chat_thread(&pool).await;

    let bound = super::TURNS_ONE_OPENER_BUYS as usize;
    let mut script = vec![the_caller_says("okay, how are we")];
    script.extend(RECITED.iter().map(|line| the_talker_says(line)));

    let did = a_call_that_hears(&pool, &bus, thread_id, uuid::Uuid::new_v4(), script).await;

    assert_eq!(
        voice_captures(&did.events),
        RECITED.len(),
        "a muted turn went unbilled"
    );
    let overheard = did.overheard.len();
    assert_eq!(overheard, bound, "a muted turn was offered to a round");

    teardown_test_db(&db_name).await;
}

/// A turn with no words in it takes nothing off the budget.
///
/// A wordless turn is the provider's silence, not an answer. Counted, a mute
/// talker would exhaust the budget and silence the real answer behind it.
#[tokio::test]
async fn a_wordless_turn_spends_nothing_off_the_budget() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = a_chat_thread(&pool).await;

    let bound = super::TURNS_ONE_OPENER_BUYS as usize;
    let mut script = vec![the_caller_says("what have I got running")];
    script.extend((0..bound + 2).map(|_| the_talker_says("")));
    script.push(the_talker_says("Two threads."));

    let did = a_call_that_hears(&pool, &bus, thread_id, uuid::Uuid::new_v4(), script).await;

    let said: Vec<String> = replies(&did.events)
        .iter()
        .map(|reply| reply["text"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(said, vec!["Two threads.".to_string()], "{:?}", said);

    teardown_test_db(&db_name).await;
}

/// The engine's own answer reopens a floor the talker spent.
///
/// The third opener, and the one that is not the caller. A card parked on a
/// silent caller is still put to them. So an answer landing after a monologue
/// is heard, whatever the talker did with the budget before it.
#[tokio::test]
async fn an_engine_relay_reopens_a_spent_floor() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = a_chat_thread(&pool).await;

    let bound = super::TURNS_ONE_OPENER_BUYS as usize;
    let (provider, talker) = MockVoiceProvider::driven();
    let log = provider.log();
    let turns = RecordingTurns::default();
    let hang_up = Arc::new(Notify::new());
    let mut caller = ScriptedCaller::new(vec![]).hanging_up_on(Arc::clone(&hang_up));
    let sent = Arc::clone(&caller.sent);

    tokio::join!(
        run_call(
            &bus,
            &provider,
            &mut caller,
            &turns,
            free_doer(),
            nobody_names(),
            opening(),
            subject(thread_id, uuid::Uuid::new_v4()),
        ),
        async {
            until("the session to open", || {
                log.lock().unwrap().openings.len() == 1
            })
            .await;

            // One word, then the whole budget, then one turn past it.
            say(&talker, the_caller_says("okay, how are we")).await;
            for line in RECITED.iter().take(bound) {
                say(&talker, the_talker_says(line)).await;
            }
            say(&talker, the_talker_says("nobody hears this one")).await;
            // SessionStarted, the caller's turn end, and one frame per talker
            // turn end, which lands whatever the floor did with the words.
            await_frames(&sent, bound + 2, "the budget to be spent").await;

            // The caller has still said nothing more. The card is what opens
            // the floor again.
            a_turn_starts(&bus, thread_id).await;
            seed_thread_event(&bus, thread_id, asks_the_caller()).await;
            until("the question to be handed to the talker", || {
                log.lock().unwrap().asked_to_speak.len() == 1
            })
            .await;
            say(&talker, the_talker_says("There's one waiting.")).await;
            await_frames(&sent, bound + 3, "the relayed answer to end").await;
            hang_up.notify_one();
        }
    );

    let said: Vec<String> = rows_of(&pool, thread_id, "SpokenReplyGenerated")
        .await
        .iter()
        .map(|(payload, _)| payload["text"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(said.len(), bound + 1, "{:?}", said);
    assert_eq!(
        said.last().map(String::as_str),
        Some("There's one waiting.")
    );
    assert!(
        !said.iter().any(|text| text == "nobody hears this one"),
        "{:?}",
        said
    );

    teardown_test_db(&db_name).await;
}
