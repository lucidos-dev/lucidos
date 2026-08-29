//! The call loop, driven end to end over a scripted transport and a mock
//! talker. No socket and no credential, and every event it writes lands in a
//! real database.

use std::sync::{Arc, Mutex};

use sqlx::PgPool;
use tokio::sync::Notify;

use crate::engine::event_bus::EventBus;
use crate::engine::thread_events::{
    ActorMode, CancelCause, MessageOrigin, ThreadEvent, VoiceSessionEndReason,
};
use crate::engine::ApiUsage;
use crate::test_support::{seed_thread_event, setup_test_db, teardown_test_db};
use crate::voice::call::{run_call, CallSubject, CallTransport, CallerFrame};
use crate::voice::mock::MockVoiceProvider;
use crate::voice::provider::{AudioFormat, SessionOpening, VoiceEvent};
use crate::voice::reasoner::TurnStarter;
use crate::voice::wire::{ClientControl, ServerFrame};

type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// A reasoner that records what it was asked to start, and starts nothing.
///
/// The seam is what keeps a whole call runnable with no engine behind it. What
/// `ThreadTurn` does with an utterance is the chat path's own business, and it
/// is covered where that path is.
#[derive(Default)]
struct RecordingTurns {
    heard: Arc<Mutex<Vec<String>>>,
}

impl RecordingTurns {
    fn heard(&self) -> Arc<Mutex<Vec<String>>> {
        Arc::clone(&self.heard)
    }
}

#[async_trait::async_trait]
impl TurnStarter for RecordingTurns {
    async fn heard(&self, _thread_id: uuid::Uuid, transcript: &str, _actor: Option<MessageOrigin>) {
        self.heard.lock().unwrap().push(transcript.to_string());
    }
}

fn subject(thread_id: uuid::Uuid, session_id: uuid::Uuid) -> CallSubject {
    CallSubject {
        thread_id,
        session_id,
        actor: None,
    }
}

/// Spin until `ready` holds, yielding so the call loop can make progress.
///
/// The test runtime is single-threaded, so yielding is what hands it the
/// processor. Capped, so a broken expectation fails rather than hangs.
async fn until(what: &str, mut ready: impl FnMut() -> bool) {
    for _ in 0..10_000 {
        if ready() {
            return;
        }
        tokio::task::yield_now().await;
    }
    panic!("timed out waiting for {}", what);
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
/// **A hangup is sequenced, never raced.** The loop selects over the caller and
/// the talker. So a hangup ready from the first poll can beat a reply the
/// talker already produced. `hanging_up_after` waits for N deliveries first,
/// which is what a person does: they hear the answer, then ring off.
struct ScriptedCaller {
    incoming: std::collections::VecDeque<CallerFrame>,
    sent: Arc<Mutex<Vec<ServerFrame>>>,
    audio_out_bytes: Arc<Mutex<usize>>,
    /// Frames plus audio chunks delivered to this caller so far.
    delivered: Arc<Mutex<usize>>,
    delivery: Arc<tokio::sync::Notify>,
    hang_up_after: Option<usize>,
    /// Rings off when the test says so, rather than on a delivery count.
    ///
    /// For a test whose sequence ends with something the caller never sees: a
    /// silent append, or an answer handed to the talker. `notify_one` stores a
    /// permit, so signalling before the caller listens is safe.
    hang_up_on: Option<Arc<Notify>>,
    hung_up: bool,
}

impl ScriptedCaller {
    fn new(incoming: Vec<CallerFrame>) -> Self {
        Self {
            incoming: incoming.into_iter().collect(),
            sent: Arc::new(Mutex::new(Vec::new())),
            audio_out_bytes: Arc::new(Mutex::new(0)),
            delivered: Arc::new(Mutex::new(0)),
            delivery: Arc::new(tokio::sync::Notify::new()),
            hang_up_after: None,
            hang_up_on: None,
            hung_up: false,
        }
    }

    /// Ring off once `deliveries` frames or audio chunks have arrived.
    fn hanging_up_after(mut self, deliveries: usize) -> Self {
        self.hang_up_after = Some(deliveries);
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
        if let Some(signal) = self.hang_up_on.clone().filter(|_| !self.hung_up) {
            signal.notified().await;
            self.hung_up = true;
            return CallerFrame::Control(ClientControl::HangUp);
        }
        let Some(target) = self.hang_up_after.filter(|_| !self.hung_up) else {
            // A caller with nothing left to say has not hung up. Park, so the
            // talker's own script decides when the call ends.
            return std::future::pending().await;
        };
        while *self.delivered.lock().unwrap() < target {
            self.delivery.notified().await;
        }
        self.hung_up = true;
        CallerFrame::Control(ClientControl::HangUp)
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
        audio: AudioFormat::default(),
    }
}

fn usage() -> ApiUsage {
    ApiUsage {
        input_tokens: 1200,
        output_tokens: 64,
        cache_read_tokens: 1024,
        cache_creation_tokens: 0,
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

// ── The thread wakes ──────────────────────────────────────────────────────

/// The whole point of the phase: a finished utterance reaches the thread.
#[tokio::test]
async fn a_finished_utterance_wakes_the_thread() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = a_chat_thread(&pool).await;

    let provider = MockVoiceProvider::new(vec![VoiceEvent::UserTurnEnded {
        transcript: "what have I got running".to_string(),
    }]);
    let turns = RecordingTurns::default();
    let heard = turns.heard();
    // SessionStarted, then the user_turn_ended frame.
    let mut caller = ScriptedCaller::new(vec![]).hanging_up_after(2);

    run_call(
        &bus,
        &provider,
        &mut caller,
        &turns,
        opening(),
        subject(thread_id, uuid::Uuid::new_v4()),
    )
    .await;

    assert_eq!(
        *heard.lock().unwrap(),
        vec!["what have I got running".to_string()]
    );

    teardown_test_db(&db_name).await;
}

/// Every utterance, not the first one only. There is no gate, by decision:
/// the engine decides and the talker cannot ask.
#[tokio::test]
async fn every_utterance_wakes_the_thread_again() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = a_chat_thread(&pool).await;

    let provider = MockVoiceProvider::new(vec![
        VoiceEvent::UserTurnEnded {
            transcript: "what have I got running".to_string(),
        },
        VoiceEvent::UserTurnEnded {
            transcript: "and tomorrow".to_string(),
        },
    ]);
    let turns = RecordingTurns::default();
    let heard = turns.heard();
    let mut caller = ScriptedCaller::new(vec![]).hanging_up_after(3);

    run_call(
        &bus,
        &provider,
        &mut caller,
        &turns,
        opening(),
        subject(thread_id, uuid::Uuid::new_v4()),
    )
    .await;

    assert_eq!(
        *heard.lock().unwrap(),
        vec![
            "what have I got running".to_string(),
            "and tomorrow".to_string()
        ]
    );

    teardown_test_db(&db_name).await;
}

/// Progress is appended silently and an answer is spoken. Appending alone
/// reaches the caller's ear never, which is what `speak` exists for.
#[tokio::test]
async fn the_reasoners_answer_is_spoken_and_progress_is_not() {
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
/// reasoner reads what was already said in its name (ADR 0150).
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
        "a call must not end the reasoner's turn: {:?}",
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
