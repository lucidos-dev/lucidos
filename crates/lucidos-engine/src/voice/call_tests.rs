//! The call loop, driven end to end over a scripted transport and a mock
//! talker. No socket and no credential, and every event it writes lands in a
//! real database.

use std::sync::{Arc, Mutex};

use sqlx::PgPool;
use tokio::sync::Notify;

use super::{answer_to_say, decision_to_ask, OFFER_THE_DETAIL_ABOVE_CHARS};
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
    /// What `doer_is_parked` answers.
    parked: bool,
    /// What `resolve` answers, in order. Exhausted, it settles.
    answers: Mutex<std::collections::VecDeque<Resolution>>,
    /// Every `(choice_id, spoken)` it was asked to settle, oldest first.
    asked: Arc<Mutex<Vec<(String, String)>>>,
}

impl NoDecisions {
    /// A thread whose doer is parked on something waiting on the caller.
    fn parked() -> Self {
        Self {
            parked: true,
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

    async fn doer_is_parked(&self, _thread_id: uuid::Uuid) -> bool {
        self.parked
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
    /// The session each utterance was attributed to. It is what marks the
    /// message as spoken, so a call that dropped it would leave the transcript
    /// unable to tell speech from typing.
    sessions: Arc<Mutex<Vec<uuid::Uuid>>>,
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

    fn sessions(&self) -> Arc<Mutex<Vec<uuid::Uuid>>> {
        Arc::clone(&self.sessions)
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
        session_id: uuid::Uuid,
        transcript: &str,
        _actor: Option<MessageOrigin>,
    ) -> bool {
        if self.refuses {
            return false;
        }
        self.woken.lock().unwrap().push(transcript.to_string());
        self.sessions.lock().unwrap().push(session_id);
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

/// The caller finishing a thought.
fn the_caller_says(text: &str) -> VoiceEvent {
    VoiceEvent::UserTurnEnded {
        transcript: text.to_string(),
    }
}

/// The talker finishing a reply.
fn the_talker_says(text: &str) -> VoiceEvent {
    VoiceEvent::TalkerTurnEnded {
        transcript: text.to_string(),
        usage: usage(),
    }
}

fn subject(thread_id: uuid::Uuid, session_id: uuid::Uuid) -> CallSubject {
    CallSubject {
        thread_id,
        session_id,
        actor: None,
    }
}

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
async fn until(what: &str, mut ready: impl FnMut() -> bool) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
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
            gone: false,
        }
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
        while *self.delivered.lock().unwrap() < target {
            self.delivery.notified().await;
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

/// Rows of `(event_type, payload)` for one thread, oldest first.
async fn thread_events(pool: &PgPool, thread_id: uuid::Uuid) -> Vec<(String, serde_json::Value)> {
    sqlx::query_as(
        "SELECT event_type, payload FROM events \
         WHERE thread_id = $1 ORDER BY created, sequence",
    )
    .bind(thread_id)
    .fetch_all(pool)
    .await
    .expect("read the thread's events")
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

    let provider = MockVoiceProvider::new(vec![VoiceEvent::Audio(vec![7; 960])]);
    // SessionStarted, then the audio chunk.
    let mut caller = ScriptedCaller::new(vec![]).hanging_up_after(2);
    let audio_out = Arc::clone(&caller.audio_out_bytes);

    run_call(
        &bus,
        &provider,
        &mut caller,
        &RecordingTurns::default(),
        free_doer(),
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

    let provider = MockVoiceProvider::new(vec![VoiceEvent::Audio(vec![1; 320])]);
    let mut caller = DeafCaller { sends: 0 };

    let reason = run_call(
        &bus,
        &provider,
        &mut caller,
        &RecordingTurns::default(),
        free_doer(),
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
        // The opening frame lands; everything after it finds a dead socket.
        if self.sends > 1 {
            return Err("the caller is gone".into());
        }
        Ok(())
    }
}

// ── The talker decides ────────────────────────────────────────────────────

/// Whether this talker event puts a frame on the caller's socket.
///
/// The delegation is the one that does not, which is the whole non-goal: the
/// wire vocabulary gains nothing, because the caller only hears one answer.
fn reaches_the_caller(event: &VoiceEvent) -> bool {
    !matches!(event, VoiceEvent::DelegationRequested { .. })
}

/// A frame that lands on the caller and writes nothing down.
///
/// The last event of every scripted call, so the hangup is sequenced behind
/// whatever the script did. An empty transcript delta is the cheapest event
/// with that shape: it sends a frame and touches only the floor.
fn the_talker_draws_breath() -> VoiceEvent {
    VoiceEvent::TalkerTranscript {
        text: String::new(),
    }
}

/// What one scripted call left behind.
struct WhatTheCallDid {
    /// Every event on the thread, oldest first.
    events: Vec<(String, serde_json::Value)>,
    /// The utterances that reached the doer, in order.
    woken: Vec<String>,
    /// The session each of those was attributed to. It is what marks the
    /// message as spoken, so a call that dropped it would leave the transcript
    /// unable to tell speech from typing.
    sessions: Vec<uuid::Uuid>,
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
    // Counted, not guessed, and with a trailing frame of its own.
    //
    // A delegation reaches the caller as NOTHING, by design: they hear one
    // answer and never learn which model produced it. Counting the script
    // would leave the caller waiting on a frame nobody sends. Gating on the
    // last VISIBLE frame would let the hangup beat a silent event after it.
    // The sentinel gives every script one frame that lands last.
    let mut script = script;
    let visible = 1 + script.iter().filter(|e| reaches_the_caller(e)).count();
    script.push(the_talker_draws_breath());
    let deliveries = visible + 1;
    let provider = MockVoiceProvider::new(script);
    let turns = RecordingTurns::default();
    let woken = turns.woken();
    let sessions = turns.sessions();
    let mut caller = ScriptedCaller::new(vec![]).hanging_up_after(deliveries);

    run_call(
        bus,
        &provider,
        &mut caller,
        &turns,
        free_doer(),
        opening(),
        subject(thread_id, session_id),
    )
    .await;

    let woken = woken.lock().unwrap().clone();
    let sessions = sessions.lock().unwrap().clone();
    WhatTheCallDid {
        events: thread_events(pool, thread_id).await,
        woken,
        sessions,
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

/// The talker asked, so the utterance takes the path a typed message takes.
/// One `MessageReceived`, carrying the session id that marks it spoken, plus
/// the row saying who asked for the turn and why.
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
    // It reaches the doer as SPOKEN. The composer stays live during a call
    // (ADR 0148). So this id is the only thing telling the transcript the
    // message was said rather than typed.
    assert_eq!(did.sessions, vec![session_id]);

    let rows = voice_rows(&did.events);
    assert_eq!(
        rows.len(),
        1,
        "the utterance was recorded twice: {:?}",
        rows
    );
    assert_eq!(rows[0].0, "WorkDelegated");
    assert_eq!(rows[0].1["reason"], "they want today's threads");
    assert_eq!(rows[0].1["session_id"], session_id.to_string());
    // Authored by the talker, so the thread names all three participants.
    assert_eq!(rows[0].1["actor"]["kind"], "agent");
    assert_eq!(rows[0].1["actor"]["agent"]["kind"], "guest");

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
    // The delegation still happened: the talker asked, and the engine is what
    // refused. What must NOT be here is a `MessageReceived`, which would claim
    // a turn nothing is running.
    assert_eq!(
        voice_kinds(&rows),
        vec![
            "WorkDelegated".to_string(),
            "SpokenMessageReceived".to_string()
        ],
        "{:?}",
        rows
    );
    assert_eq!(rows[1].1["text"], "book it for tuesday");
    assert_eq!(rows[1].1["session_id"], session_id.to_string());

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
        assert_eq!(kinds, vec!["WorkDelegated"], "{:?}", kinds);
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
    assert_eq!(kinds, vec!["WorkDelegated"], "{:?}", kinds);

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
    assert_eq!(kinds, vec!["WorkDelegated", "WorkDelegated"], "{:?}", kinds);

    teardown_test_db(&db_name).await;
}

/// Two utterances, one delegated and one not. Each is recorded once, and only
/// the delegated one runs a turn.
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
        vec!["SpokenMessageReceived", "WorkDelegated"],
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
        vec!["WorkDelegated", "SpokenMessageReceived"],
        "{:?}",
        kinds
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
    assert_eq!(kinds, vec!["WorkDelegated"], "{:?}", kinds);

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
            },
            QuestionOption {
                id: "opt-1".to_string(),
                label: "Leave it for tonight".to_string(),
                description: None,
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
    }];
    let single = decision_to_ask(&OpenDecision::question("toolu_q0", "Ready?", &one, false));
    assert!(
        single.contains("- Ship it [question:toolu_q0#opt0]"),
        "{}",
        single
    );

    let free = decision_to_ask(&OpenDecision::question(
        "toolu_q1",
        "What should I call it?",
        &[],
        false,
    ));
    assert!(free.contains("What should I call it?"), "{}", free);
    assert!(free.contains("[question:toolu_q1#said]"), "{}", free);
}

/// Both surfaces stopped sending the caller to the screen. This is the note
/// handed over mid-call; `sections` covers the resident block.
#[test]
fn the_note_says_the_caller_answers_out_loud_and_never_on_screen() {
    let decision = OpenDecision::question("toolu_q0", "Ready?", &[], false);
    let note = decision_to_ask(&decision);
    assert!(!note.to_lowercase().contains("on screen"), "{}", note);
    assert!(note.contains("hand its id back"), "{}", note);
    assert!(note.contains("Never say an id out loud"), "{}", note);
}

/// A permission card reads as a request for permission, not as a question the
/// agent asked.
#[test]
fn a_permission_note_asks_for_a_say_so() {
    let note = decision_to_ask(&OpenDecision::mcp_permission(
        "req-1",
        "example-server",
        "Example Server",
        "post_message",
        "{\"channel\":\"general\"}",
    ));
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
            opening(),
            subject(thread_id, uuid::Uuid::new_v4()),
        ),
        async {
            until("the session to open", || {
                log.lock().unwrap().openings.len() == 1
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

// ── What the caller heard is written down ─────────────────────────────────

/// The talker's turn lands in the thread under the talker's own name, so the
/// doer reads what was already said in its name (ADR 0150).
#[tokio::test]
async fn a_spoken_reply_is_written_down_under_the_talkers_name() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = a_chat_thread(&pool).await;
    let session_id = uuid::Uuid::new_v4();

    let provider = MockVoiceProvider::new(vec![VoiceEvent::TalkerTurnEnded {
        transcript: "Two things are running.".to_string(),
        usage: usage(),
    }]);
    let mut caller = ScriptedCaller::new(vec![]).hanging_up_after(2);

    run_call(
        &bus,
        &provider,
        &mut caller,
        &RecordingTurns::default(),
        free_doer(),
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
        VoiceEvent::Interrupted,
        VoiceEvent::TalkerTurnEnded {
            transcript: "Two things are".to_string(),
            usage: usage(),
        },
    ]);
    // SessionStarted, the interrupted frame, then the turn end.
    let mut caller = ScriptedCaller::new(vec![]).hanging_up_after(3);

    run_call(
        &bus,
        &provider,
        &mut caller,
        &RecordingTurns::default(),
        free_doer(),
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

    let provider = MockVoiceProvider::new(vec![the_talker_says("Still on it.")]);
    let turns = RecordingTurns::default();
    let overheard = turns.overheard();
    let mut caller = ScriptedCaller::new(vec![]).hanging_up_after(2);

    run_call(
        &bus,
        &provider,
        &mut caller,
        &turns,
        free_doer(),
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
    let thread_id = a_chat_thread(pool).await;
    // Counted, not guessed. A tool call reaches the caller as no frame at all.
    // So the deliveries are the utterance, the reply, and the opening frame
    // every call sends. Ringing off too early ends the call before the tool
    // call is even read.
    let deliveries = 1 + script
        .iter()
        .filter(|event| {
            matches!(
                event,
                VoiceEvent::UserTurnEnded { .. } | VoiceEvent::TalkerTurnEnded { .. }
            )
        })
        .count();
    let provider = MockVoiceProvider::new(script);
    let log = provider.log();
    let turns = RecordingTurns::default();
    let mut caller = ScriptedCaller::new(vec![]).hanging_up_after(deliveries);

    run_call(
        bus,
        &provider,
        &mut caller,
        &turns,
        decisions,
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

/// Words spent on an answer are not written down a second time. Typing one
/// writes the answer's row and nothing else, and speaking one matches.
#[tokio::test]
async fn words_spent_on_an_answer_are_not_also_written_down_as_speech() {
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
        opening(),
        subject(thread_id, uuid::Uuid::new_v4()),
    )
    .await;

    let events = thread_events(&pool, thread_id).await;
    assert!(
        !voice_kinds(&events).contains(&"SpokenMessageReceived".to_string()),
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
