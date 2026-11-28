use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::{DateTime, Utc};
use sqlx::PgPool;
use tokio::sync::Notify;

use super::{
    answer_to_say, decision_to_ask, ASK_NEVER_GOT_ITS_WORDS, DELEGATION_PARKED,
    DELEGATION_PARKED_ON_SCREEN, EARLIER_TAG, OFFER_THE_DETAIL_ABOVE_CHARS, UNFINISHED_EARLIER,
    WORKING_NOTE_TAG,
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

#[derive(Default)]
struct NoDecisions {
    parked: Option<OpenDecision>,
    answers: Mutex<std::collections::VecDeque<Resolution>>,
    asked: Arc<Mutex<Vec<(String, String)>>>,
}

const PARKED_QUESTION: &str = "toolu_parked";

impl NoDecisions {
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
                    widget: None,
                    quiet: false,
                    quiet_reply: None,
                }],
                false,
            )),
            ..Self::default()
        }
    }

    fn parked_on_a_permission() -> Self {
        Self {
            parked: Some(OpenDecision::tool_permission(
                &crate::engine::permission::legacy::LegacyCard::command(
                    "req-cmd",
                    "run_bash",
                    "rm -rf build",
                    "Deletes files.",
                ),
            )),
            ..Self::default()
        }
    }

    fn answering(script: Vec<Resolution>) -> Self {
        Self {
            answers: Mutex::new(script.into_iter().collect()),
            ..Self::default()
        }
    }

    fn answering_with(mut self, script: Vec<Resolution>) -> Self {
        self.answers = Mutex::new(script.into_iter().collect());
        self
    }

    fn asked(&self) -> Arc<Mutex<Vec<(String, String)>>> {
        Arc::clone(&self.asked)
    }
}

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

    async fn parked_on(&self, _thread_id: uuid::Uuid) -> Option<OpenDecision> {
        self.parked.clone()
    }
}

#[derive(Default)]
struct RecordingTurns {
    woken: Arc<Mutex<Vec<String>>>,
    anchors: Arc<Mutex<Vec<uuid::Uuid>>>,
    overheard: Arc<Mutex<Vec<String>>>,
    continued: Arc<Mutex<Vec<(uuid::Uuid, String)>>>,
    refusals: AtomicUsize,
}

impl RecordingTurns {
    fn refusing() -> Self {
        Self {
            refusals: AtomicUsize::new(usize::MAX),
            ..Self::default()
        }
    }

    fn refusing_once() -> Self {
        Self {
            refusals: AtomicUsize::new(1),
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

    fn continued(&self) -> Arc<Mutex<Vec<(uuid::Uuid, String)>>> {
        Arc::clone(&self.continued)
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
        let refused = self
            .refusals
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |left| {
                left.checked_sub(1)
            })
            .is_ok();
        if refused {
            return false;
        }
        self.woken.lock().unwrap().push(transcript.to_string());
        self.anchors.lock().unwrap().push(delegation);
        true
    }

    async fn overheard(&self, _thread_id: uuid::Uuid, spoken: &str) {
        self.overheard.lock().unwrap().push(spoken.to_string());
    }

    async fn continued(&self, _thread_id: uuid::Uuid, asked_turn: uuid::Uuid, words: &str) {
        self.continued
            .lock()
            .unwrap()
            .push((asked_turn, words.to_string()));
    }
}

fn asks_for_the_doer(reason: &str) -> VoiceEvent {
    VoiceEvent::DelegationRequested {
        tool_call_id: "call_1".to_string(),
        reason: reason.to_string(),
    }
}

const FIRST_WORD: &str = "hello";

fn the_caller_opens_the_floor() -> VoiceEvent {
    the_caller_says(FIRST_WORD)
}

fn the_caller_says(text: &str) -> VoiceEvent {
    VoiceEvent::UserTurnEnded {
        transcript: text.to_string(),
    }
}

fn the_caller_goes_on(text: &str) -> VoiceEvent {
    VoiceEvent::UserTurnContinued {
        transcript: text.to_string(),
    }
}

fn the_caller_is_saying(text: &str) -> VoiceEvent {
    VoiceEvent::UserTranscript {
        text: text.to_string(),
    }
}

fn the_talker_says(text: &str) -> VoiceEvent {
    VoiceEvent::TalkerTurnEnded {
        transcript: text.to_string(),
        usage: usage(),
    }
}

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

const WAITING_SLACK: Duration = Duration::from_secs(20);

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

fn canceled(cause: CancelCause) -> ThreadEvent {
    ThreadEvent::ResponseCanceled {
        text: String::new(),
        images: vec![],
        model: None,
        reasoning_effort: None,
        cause,
    }
}

async fn a_turn_starts(bus: &EventBus, thread_id: uuid::Uuid) -> uuid::Uuid {
    seed_thread_event(
        bus,
        thread_id,
        ThreadEvent::MessageReceived {
            provider: None,
            voice_session_id: None,
            wake_mode: None,
            text: "what have I got running".to_string(),
            user_image_hashes: vec![],
            device_id: None,
            image_description: None,
            parent_thread_id: None,
            spawning_event_id: None,
            mode: ActorMode::Human,
            model: None,
            reasoning_effort: None,
            origin: None,
        },
    )
    .await
}

struct ScriptedCaller {
    incoming: std::collections::VecDeque<CallerFrame>,
    sent: Arc<Mutex<Vec<ServerFrame>>>,
    audio_out_bytes: Arc<Mutex<usize>>,
    delivered: Arc<Mutex<usize>>,
    delivery: Arc<tokio::sync::Notify>,
    ends_after: Option<(usize, CallerFrame)>,
    hang_up_on: Option<Arc<Notify>>,
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

    fn driven() -> (Self, tokio::sync::mpsc::Sender<CallerFrame>) {
        let (tx, rx) = tokio::sync::mpsc::channel(8);
        let mut caller = Self::new(vec![]);
        caller.live = Some(rx);
        (caller, tx)
    }

    fn hanging_up_after(mut self, deliveries: usize) -> Self {
        self.ends_after = Some((deliveries, CallerFrame::Control(ClientControl::HangUp)));
        self
    }

    fn dropping_after(mut self, deliveries: usize) -> Self {
        self.ends_after = Some((deliveries, CallerFrame::Closed));
        self
    }

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
                None => std::future::pending().await,
            };
        }
        if let Some(signal) = self.hang_up_on.clone().filter(|_| !self.gone) {
            signal.notified().await;
            self.gone = true;
            return CallerFrame::Control(ClientControl::HangUp);
        }
        let Some((target, ending)) = self.ends_after.clone().filter(|_| !self.gone) else {
            return std::future::pending().await;
        };
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
        transcriber: "gpt-transcribe".to_string(),
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
        service_tier: None,
        cache_creation_1h_tokens: None,
        cache_creation_30m_tokens: None,
        cache_creation_12h_tokens: None,
        cache_creation_requested_ttl: None,
    }
}

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

async fn a_coding_agent_thread(bus: &EventBus) -> uuid::Uuid {
    let thread_id = uuid::Uuid::new_v4();
    bus.emit(crate::engine::event_bus::BusEvent::Thread {
        thread_id,
        event: ThreadEvent::SessionStarted {
            sandbox: None,
            coding_agent: crate::runtime::CodingAgent::ClaudeCode,
            session_id: "sid-test".to_string(),
            branch: "claude-code/test".to_string(),
            repo_id: None,
            coding_agent_kind: Default::default(),
            coding_agent_folder: String::new(),
            app_id: None,
            integrated: false,
            auto_compact_window: None,
            auto_compact_arm: None,
            continuity: None,
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

#[tokio::test]
async fn a_spoken_reply_records_what_it_spent() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = a_chat_thread(&pool).await;

    let provider = MockVoiceProvider::new(vec![VoiceEvent::TalkerTurnEnded {
        transcript: "I am checking.".to_string(),
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

    let captures = crate::test_support::aux_captures(&pool, thread_id, "voice").await;
    assert_eq!(captures.len(), 1, "one row per spoken reply");
    assert_eq!(captures[0]["producer"], "auxiliary");
    assert_eq!(captures[0]["usage"]["input_tokens"], 1200);
    assert_eq!(captures[0]["usage"]["cache_read_tokens"], 1024);
    assert_eq!(captures[0]["usage"]["output_tokens"], 64);
    let modality = &captures[0]["usage"]["modality"];
    assert_eq!(modality["input_audio_tokens"], 1024);
    assert_eq!(modality["input_text_tokens"], 176);
    assert_eq!(modality["output_audio_tokens"], 44);
    assert_eq!(modality["cache_read_audio_tokens"], 924);

    teardown_test_db(&db_name).await;
}

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

    let first = sent.lock().unwrap().first().cloned();
    match first {
        Some(ServerFrame::Error { message }) => {
            assert!(!message.to_lowercase().contains("openai"), "{}", message)
        }
        other => panic!("expected an error frame, got {:?}", other),
    }

    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn talker_audio_reaches_the_caller_and_no_event() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = a_chat_thread(&pool).await;

    let provider = MockVoiceProvider::new(vec![
        the_caller_opens_the_floor(),
        VoiceEvent::Audio(vec![7; 960]),
    ]);
    let mut caller = ScriptedCaller::new(vec![]).hanging_up_after(3);
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

    crate::voice::recovery::settle_orphan_voice_sessions(&pool, &bus).await;
    let ends = thread_events(&pool, thread_id)
        .await
        .into_iter()
        .filter(|(kind, _)| kind == "VoiceSessionEnded")
        .count();
    assert_eq!(ends, 1);

    teardown_test_db(&db_name).await;
}

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
        if self.sends > 2 {
            return Err("the caller is gone".into());
        }
        Ok(())
    }
}

fn reaches_the_caller(event: &VoiceEvent) -> bool {
    !matches!(
        event,
        VoiceEvent::DelegationRequested { .. } | VoiceEvent::CallerStartedSpeaking
    )
}

fn deliveries_of(script: &[VoiceEvent]) -> usize {
    let mut turns_left = 0u8;
    let mut audience: Option<bool> = None;
    let mut held = 0u8;
    let mut deliveries = 0;
    for event in script {
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

fn opens_the_floor(event: &VoiceEvent) -> bool {
    match event {
        VoiceEvent::UserTranscript { text } => !text.trim().is_empty(),
        VoiceEvent::UserTurnEnded { transcript } | VoiceEvent::UserTurnContinued { transcript } => {
            !transcript.trim().is_empty()
        }
        VoiceEvent::CallerStartedSpeaking => true,
        _ => false,
    }
}

fn latches_the_turn(event: &VoiceEvent) -> bool {
    matches!(event, VoiceEvent::TalkerTranscript { text } if !text.trim().is_empty())
}

fn the_talker_draws_breath() -> VoiceEvent {
    the_talker_is_saying("")
}

struct WhatTheCallDid {
    events: Vec<(String, serde_json::Value)>,
    woken: Vec<String>,
    anchors: Vec<uuid::Uuid>,
    overheard: Vec<String>,
    continued: Vec<(uuid::Uuid, String)>,
}

async fn a_call_that_hears(
    pool: &PgPool,
    bus: &EventBus,
    thread_id: uuid::Uuid,
    session_id: uuid::Uuid,
    script: Vec<VoiceEvent>,
) -> WhatTheCallDid {
    let turns = RecordingTurns::default();
    a_call_with_this_doer_that_hears(pool, bus, thread_id, session_id, turns, script).await
}

async fn a_call_with_this_doer_that_hears(
    pool: &PgPool,
    bus: &EventBus,
    thread_id: uuid::Uuid,
    session_id: uuid::Uuid,
    turns: RecordingTurns,
    script: Vec<VoiceEvent>,
) -> WhatTheCallDid {
    let mut script = script;
    script.push(the_talker_draws_breath());
    let deliveries = 1 + deliveries_of(&script);
    let provider = MockVoiceProvider::new(script);
    let woken = turns.woken();
    let anchors = turns.anchors();
    let overheard = turns.overheard();
    let continued = turns.continued();
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
    let anchors = anchors.lock().unwrap().clone();
    let overheard = overheard.lock().unwrap().clone();
    let continued = continued.lock().unwrap().clone();
    WhatTheCallDid {
        events: thread_events(pool, thread_id).await,
        woken,
        anchors,
        overheard,
        continued,
    }
}

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

fn voice_kinds(events: &[(String, serde_json::Value)]) -> Vec<String> {
    voice_rows(events)
        .into_iter()
        .map(|(kind, _)| kind)
        .collect()
}

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
    assert_eq!(kinds, vec!["SpokenMessageReceived".to_string()]);

    teardown_test_db(&db_name).await;
}

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

fn replies(events: &[(String, serde_json::Value)]) -> Vec<serde_json::Value> {
    events
        .iter()
        .filter(|(kind, _)| kind == "SpokenReplyGenerated")
        .map(|(_, payload)| payload.clone())
        .collect()
}

fn voice_captures(events: &[(String, serde_json::Value)]) -> usize {
    events
        .iter()
        .filter(|(kind, payload)| kind == "ContextCaptured" && payload["purpose"] == "voice")
        .count()
}

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
    assert!(said.iter().all(|row| row["interrupted"] == false));
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
    assert_eq!(voice_captures(&did.events), 3);

    teardown_test_db(&db_name).await;
}

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

const A_BEAT: Duration = Duration::from_millis(60);

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

            tokio::time::sleep(A_BEAT).await;
            say(asks_for_the_doer("check what is waiting")).await;
            until("the doer to be woken", || !woken.lock().unwrap().is_empty()).await;

            say(the_talker_is_saying(" give me a sec.")).await;
            say(the_talker_says("give me a sec.")).await;
            until("the second stretch to reach the caller", || {
                transcripts() == 2
            })
            .await;
            hang_up.notify_one();
        }
    );

    let replies = rows_of(&pool, thread_id, "SpokenReplyGenerated").await;
    assert_eq!(replies.len(), 2, "{:?}", replies);
    assert_eq!(replies[0].0["text"], "I'm on it,");
    assert_eq!(replies[1].0["text"], "give me a sec.");

    let delegated = rows_of(&pool, thread_id, "WorkDelegated").await;
    assert_eq!(delegated.len(), 1, "{:?}", delegated);
    let started_working = delegated[0].1;

    assert!(
        replies[0].1 < started_working,
        "the stall was written at {} and the work started {}",
        replies[0].1,
        started_working
    );
    assert!(
        started_working < replies[1].1,
        "the tail was written before the work it followed"
    );

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

fn began_speaking(reply: &(serde_json::Value, DateTime<Utc>)) -> DateTime<Utc> {
    let secs = reply.0["spoken_secs_before"]
        .as_f64()
        .unwrap_or_else(|| panic!("the row says how long it had been speaking: {:?}", reply.0));
    reply.1 - chrono::Duration::microseconds((secs * 1_000_000.0) as i64)
}

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

            tokio::time::sleep(A_BEAT).await;
            say(asks_for_the_doer("improve the thread title")).await;
            until("the doer to be woken", || !woken.lock().unwrap().is_empty()).await;

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

    assert!(
        started_working < replies[0].1,
        "the reply was written at {} and the work started {}",
        replies[0].1,
        started_working
    );
    assert!(
        began_speaking(&replies[0]) < started_working,
        "the reply began at {} and the work started {}",
        began_speaking(&replies[0]),
        started_working
    );
    let age = replies[0].0["spoken_secs_before"].as_f64().expect("an age");
    assert!(
        age >= A_BEAT.as_secs_f64(),
        "the reply was said over {}s, which is shorter than the gap inside it",
        age
    );

    teardown_test_db(&db_name).await;
}

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
    let rows = voice_rows(&thread_events(&pool, thread_id).await);
    assert_eq!(rows.len(), 2, "{:?}", rows);
    assert_eq!(rows[0].1["text"], "Let's try this for a");
    assert_eq!(rows[1].1["text"], "bit");

    teardown_test_db(&db_name).await;
}

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
            VoiceEvent::Interrupted,
            the_talker_says(""),
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
            VoiceEvent::Interrupted,
            the_caller_says("wait"),
            the_talker_says("Two things are running."),
        ],
    )
    .await;

    let said = replies(&did.events);
    assert_eq!(said.len(), 1, "{:?}", said);
    assert_eq!(said[0]["interrupted"], true, "{:?}", said[0]);
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
    assert_eq!(said[0]["interrupted"], false);

    teardown_test_db(&db_name).await;
}

fn a_sentence_the_talker_talked_over() -> Vec<VoiceEvent> {
    vec![
        the_caller_says("Status"),
        the_talker_is_saying("Still "),
        the_caller_says(", please"),
        the_talker_is_saying("in it. I'm pulling the threads together."),
        the_talker_says("Still in it. I'm pulling the threads together."),
    ]
}

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
            opening(),
            subject(thread_id, uuid::Uuid::new_v4()),
        ),
        async {
            until("the session to open", || {
                log.lock().unwrap().openings.len() == 1
            })
            .await;
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
            say(&talker, asks_for_the_doer("do the thing")).await;
            until("the doer to be woken", || !woken.lock().unwrap().is_empty()).await;

            say(&talker, the_talker_is_saying("On it")).await;
            await_frames(&sent, 2, "the stall to open").await;
            say(&talker, the_caller_is_saying("and quickly")).await;
            await_frames(&sent, 3, "their partial to land").await;
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
            opening(),
            subject(thread_id, uuid::Uuid::new_v4()),
        ),
        async {
            until("the session to open", || {
                log.lock().unwrap().openings.len() == 1
            })
            .await;
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
            opening(),
            subject(thread_id, uuid::Uuid::new_v4()),
        ),
        async {
            until("the session to open", || {
                log.lock().unwrap().openings.len() == 1
            })
            .await;
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
        the_caller_is_saying("no wait"),
        the_talker_says("Speak soon."),
    ]);
    let mut caller = ScriptedCaller::new(vec![]).hanging_up_after(4);

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
        "the call rang off over somebody mid-sentence"
    );

    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn one_barge_in_cancels_once() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = a_chat_thread(&pool).await;

    let cut = a_reply_the_caller_cuts_into(&bus, thread_id, 3).await;

    assert_eq!(cut.cancels, 1);

    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn a_barge_in_with_nobody_speaking_cuts_nothing() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = a_chat_thread(&pool).await;

    let cut = a_reply_the_caller_cuts_into(&bus, thread_id, 0).await;

    assert_eq!(cut.cancels, 0);

    teardown_test_db(&db_name).await;
}

struct WhatTheCutDid {
    cancels: usize,
    audio_out_bytes: usize,
}

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
            opening(),
            subject(thread_id, uuid::Uuid::new_v4()),
        ),
        async {
            until("the session to open", || {
                log.lock().unwrap().openings.len() == 1
            })
            .await;
            say(&talker, the_caller_opens_the_floor()).await;
            await_frames(&sent, 1, "the caller's first word").await;
            if !quiet {
                say(&talker, the_talker_is_saying("A traveler finds a tiny key")).await;
                await_frames(&sent, 2, "the talker to take the floor").await;
            }

            for _ in 0..barge_ins.max(1) {
                cut_in(&from_caller).await;
            }
            from_caller
                .send(CallerFrame::Audio(vec![7]))
                .await
                .expect("the call is listening");
            until("the caller's audio to go up", || {
                log.lock().unwrap().audio_in_bytes == 1
            })
            .await;
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

async fn say(talker: &tokio::sync::mpsc::Sender<VoiceEvent>, event: VoiceEvent) {
    talker.send(event).await.expect("the talker is listening");
}

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

async fn await_frames(sent: &Arc<Mutex<Vec<ServerFrame>>>, count: usize, what: &str) {
    until(what, || sent.lock().unwrap().len() > count).await;
}

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
    assert_eq!(rows[1].1["actor"]["kind"], "agent");
    assert_eq!(rows[1].1["actor"]["agent"]["kind"], "guest");
    assert_eq!(did.anchors.len(), 1, "{:?}", did.anchors);

    teardown_test_db(&db_name).await;
}

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

#[tokio::test]
async fn the_rest_of_an_asked_sentence_reaches_the_turn_it_started() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = a_chat_thread(&pool).await;

    let did = a_call_that_hears(
        &pool,
        &bus,
        thread_id,
        uuid::Uuid::new_v4(),
        vec![
            the_caller_says("when do you estimate that it's"),
            asks_for_the_doer(""),
            the_caller_goes_on("finished"),
            the_talker_says("Hmm. Checking."),
        ],
    )
    .await;

    assert_eq!(
        did.woken,
        vec!["when do you estimate that it's".to_string()]
    );
    let rows = voice_rows(&did.events);
    assert_eq!(
        voice_kinds(&rows),
        vec![
            "SpokenMessageReceived",
            "WorkDelegated",
            "SpokenMessageReceived"
        ],
        "{:?}",
        rows
    );
    assert_eq!(rows[0].1["text"], "when do you estimate that it's");
    assert_eq!(rows[2].1["text"], "finished");
    assert_eq!(did.anchors.len(), 1, "{:?}", did.anchors);
    assert_eq!(
        did.continued,
        vec![(did.anchors[0], "finished".to_string())],
        "the tail did not reach the turn that took its ask"
    );

    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn a_delivered_tail_is_never_run_again_by_a_later_ask() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = a_chat_thread(&pool).await;

    let did = a_call_that_hears(
        &pool,
        &bus,
        thread_id,
        uuid::Uuid::new_v4(),
        vec![
            the_caller_says("when do you estimate that it's"),
            asks_for_the_doer(""),
            the_caller_goes_on("finished"),
            the_talker_says("Hmm. Checking."),
            asks_for_the_doer(""),
            the_talker_says("Still on it."),
        ],
    )
    .await;

    assert_eq!(
        did.woken,
        vec!["when do you estimate that it's".to_string()]
    );
    assert_eq!(did.continued.len(), 1, "{:?}", did.continued);

    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn a_tail_after_a_reply_the_talker_gave_itself_starts_nothing() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = a_chat_thread(&pool).await;

    let did = a_call_that_hears(
        &pool,
        &bus,
        thread_id,
        uuid::Uuid::new_v4(),
        vec![
            the_caller_says("is this in the latest"),
            the_talker_says("Yes, it is."),
            the_caller_goes_on("release"),
        ],
    )
    .await;

    assert!(did.woken.is_empty(), "{:?}", did.woken);
    assert!(did.continued.is_empty(), "{:?}", did.continued);
    let rows = voice_rows(&did.events);
    assert_eq!(
        voice_kinds(&rows),
        vec!["SpokenMessageReceived", "SpokenMessageReceived"],
        "{:?}",
        rows
    );

    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn an_ask_with_no_words_yet_runs_on_the_rest_of_its_sentence() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = a_chat_thread(&pool).await;

    let did = a_call_that_hears(
        &pool,
        &bus,
        thread_id,
        uuid::Uuid::new_v4(),
        vec![
            asks_for_the_doer(""),
            the_caller_goes_on("move my three o'clock"),
            the_talker_says("On it."),
        ],
    )
    .await;

    assert_eq!(did.woken, vec!["move my three o'clock".to_string()]);
    assert!(did.continued.is_empty(), "{:?}", did.continued);
    assert_eq!(
        voice_kinds(&did.events),
        vec!["SpokenMessageReceived", "WorkDelegated"]
    );

    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn a_refused_ask_keeps_the_rest_of_its_sentence() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = a_chat_thread(&pool).await;

    let did = a_call_with_this_doer_that_hears(
        &pool,
        &bus,
        thread_id,
        uuid::Uuid::new_v4(),
        RecordingTurns::refusing_once(),
        vec![
            the_caller_says("when do you estimate that it's"),
            asks_for_the_doer(""),
            the_talker_says("I could not start that."),
            the_caller_goes_on("finished"),
            asks_for_the_doer(""),
            the_talker_says("On it."),
        ],
    )
    .await;

    assert_eq!(
        did.woken,
        vec!["when do you estimate that it's finished".to_string()]
    );
    assert!(did.continued.is_empty(), "{:?}", did.continued);

    teardown_test_db(&db_name).await;
}

async fn the_turn_answers(bus: &EventBus, thread_id: uuid::Uuid, request_event_id: uuid::Uuid) {
    bus.emit(crate::engine::event_bus::BusEvent::Thread {
        thread_id,
        event: the_doer_answers("Tomorrow."),
        meta: crate::engine::thread_events::EventMeta {
            request_event_id: Some(request_event_id),
            ..crate::engine::thread_events::EventMeta::NONE
        },
    })
    .await
    .expect("the answer is recorded");
}

#[test]
fn only_the_terminal_of_the_turn_that_took_the_ask_ends_it() {
    let delegation = uuid::Uuid::new_v4();
    let asked = super::AskedTurn::new(delegation);
    assert!(asked.ended_by(Some(delegation)));
    assert!(!asked.ended_by(Some(uuid::Uuid::new_v4())), "another turn");
    assert!(!asked.ended_by(None), "an unanchored terminal");
}

#[test]
fn an_ask_follows_the_turn_it_was_coalesced_into() {
    let delegation = uuid::Uuid::new_v4();
    let running = uuid::Uuid::new_v4();
    let injected = |id| ThreadEvent::PromptInjected {
        text: "what is on my calendar".to_string(),
        mode: ActorMode::Human,
        origin: None,
        injected_message_id: Some(id),
        delivered_event_id: None,
    };

    let mut asked = super::AskedTurn::new(delegation);
    asked.follow(&injected(uuid::Uuid::new_v4()), Some(running));
    assert!(!asked.ended_by(Some(running)), "another prompt joined it");

    asked.follow(&injected(delegation), Some(running));
    assert!(asked.ended_by(Some(running)));
    assert!(!asked.ended_by(Some(delegation)));
}

#[tokio::test]
async fn a_tail_after_another_turn_ended_still_reaches_the_asked_turn() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = a_chat_thread(&pool).await;

    let (provider, talker) = MockVoiceProvider::driven();
    let log = provider.log();
    let turns = RecordingTurns::default();
    let anchors = turns.anchors();
    let continued = turns.continued();
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
            let say = |event| async {
                talker.send(event).await.expect("the talker is listening");
            };

            let other = a_turn_starts(&bus, thread_id).await;
            say(the_caller_says("when do you estimate that it's")).await;
            say(asks_for_the_doer("")).await;
            until("the doer to be woken", || {
                !anchors.lock().unwrap().is_empty()
            })
            .await;
            the_turn_answers(&bus, thread_id, other).await;
            until("the other answer to be handed to the talker", || {
                log.lock().unwrap().asked_to_speak.len() == 1
            })
            .await;

            say(the_caller_goes_on("finished")).await;
            until("the rest to be offered", || {
                !continued.lock().unwrap().is_empty()
            })
            .await;
            hang_up.notify_one();
        }
    );

    let anchor = anchors.lock().unwrap()[0];
    assert_eq!(
        continued.lock().unwrap().clone(),
        vec![(anchor, "finished".to_string())]
    );

    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn a_tail_after_its_turn_ended_is_offered_to_nobody() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = a_chat_thread(&pool).await;

    let (provider, talker) = MockVoiceProvider::driven();
    let log = provider.log();
    let turns = RecordingTurns::default();
    let anchors = turns.anchors();
    let continued = turns.continued();
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
            let say = |event| async {
                talker.send(event).await.expect("the talker is listening");
            };
            let sentences = || {
                sent.lock()
                    .unwrap()
                    .iter()
                    .filter(|f| matches!(f, ServerFrame::UserTurnEnded { .. }))
                    .count()
            };

            say(the_caller_says("when do you estimate that it's")).await;
            say(asks_for_the_doer("")).await;
            until("the doer to be woken", || {
                !anchors.lock().unwrap().is_empty()
            })
            .await;
            a_turn_starts(&bus, thread_id).await;
            let anchor = anchors.lock().unwrap()[0];
            the_turn_answers(&bus, thread_id, anchor).await;
            until("the answer to be handed to the talker", || {
                log.lock().unwrap().asked_to_speak.len() == 1
            })
            .await;

            say(the_caller_goes_on("finished")).await;
            until("the late words to be taken", || sentences() == 2).await;
            hang_up.notify_one();
        }
    );

    let continued = continued.lock().unwrap().clone();
    assert!(continued.is_empty(), "{:?}", continued);

    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn a_tail_follows_an_orphaned_ask_into_another_turn() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = a_chat_thread(&pool).await;

    let (provider, talker) = MockVoiceProvider::driven();
    let log = provider.log();
    let turns = RecordingTurns::default();
    let anchors = turns.anchors();
    let continued = turns.continued();
    let hang_up = Arc::new(Notify::new());
    let mut caller = ScriptedCaller::new(vec![]).hanging_up_on(Arc::clone(&hang_up));
    let sent = Arc::clone(&caller.sent);
    let running = uuid::Uuid::new_v4();

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
            let say = |event| async {
                talker.send(event).await.expect("the talker is listening");
            };
            let sentences = || {
                sent.lock()
                    .unwrap()
                    .iter()
                    .filter(|f| matches!(f, ServerFrame::UserTurnEnded { .. }))
                    .count()
            };

            say(the_caller_says("when do you estimate that it's")).await;
            say(asks_for_the_doer("")).await;
            until("the doer to be woken", || {
                !anchors.lock().unwrap().is_empty()
            })
            .await;
            let delegation = anchors.lock().unwrap()[0];
            bus.emit(crate::engine::event_bus::BusEvent::Thread {
                thread_id,
                event: ThreadEvent::PromptInjected {
                    text: "when do you estimate that it's".to_string(),
                    mode: ActorMode::Human,
                    origin: None,
                    injected_message_id: Some(delegation),
                    delivered_event_id: None,
                },
                meta: crate::engine::thread_events::EventMeta {
                    request_event_id: Some(running),
                    ..crate::engine::thread_events::EventMeta::NONE
                },
            })
            .await
            .expect("the pickup is recorded");
            bus.emit(crate::engine::event_bus::BusEvent::Thread {
                thread_id,
                event: the_doer_uses("list_files"),
                meta: crate::engine::thread_events::EventMeta {
                    request_event_id: Some(running),
                    ..crate::engine::thread_events::EventMeta::NONE
                },
            })
            .await
            .expect("the tool call is recorded");
            until("the call to read the pickup", || {
                log.lock()
                    .unwrap()
                    .history
                    .iter()
                    .any(|h| h.starts_with(WORKING_NOTE_TAG))
            })
            .await;

            say(the_caller_goes_on("finished")).await;
            until("the rest to be offered", || {
                !continued.lock().unwrap().is_empty()
            })
            .await;

            the_turn_answers(&bus, thread_id, running).await;
            until("the answer to be handed to the talker", || {
                log.lock().unwrap().asked_to_speak.len() == 1
            })
            .await;
            let before = sentences();
            say(the_caller_goes_on("and thanks")).await;
            until("the late words to be taken", || sentences() > before).await;
            hang_up.notify_one();
        }
    );

    assert_eq!(
        continued.lock().unwrap().clone(),
        vec![(running, "finished".to_string())],
        "only the words before that turn ended are offered, and to that turn"
    );

    teardown_test_db(&db_name).await;
}

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
            the_talker_says("Okay."),
            asks_for_the_doer("they want it recorded"),
        ],
    )
    .await;

    assert_eq!(did.woken, vec!["yeah".to_string()]);
    let kinds = voice_kinds(&did.events);
    assert_eq!(
        kinds,
        vec!["SpokenMessageReceived", "WorkDelegated"],
        "{:?}",
        kinds
    );

    teardown_test_db(&db_name).await;
}

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

#[tokio::test]
async fn every_ask_is_acknowledged() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = a_chat_thread(&pool).await;

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
        let (provider, mut caller) = match ending {
            VoiceSessionEndReason::Hangup => (
                MockVoiceProvider::new(script),
                ScriptedCaller::new(vec![]).hanging_up_after(2),
            ),
            VoiceSessionEndReason::Disconnected => (
                MockVoiceProvider::new(script),
                ScriptedCaller::new(vec![]).dropping_after(2),
            ),
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
        let script = vec![
            the_caller_says("what have I got running"),
            the_talker_is_saying("Two things are "),
            the_talker_is_saying("running."),
        ];
        let (provider, mut caller) = match ending {
            VoiceSessionEndReason::Hangup => (
                MockVoiceProvider::new(script),
                ScriptedCaller::new(vec![]).hanging_up_after(4),
            ),
            VoiceSessionEndReason::Disconnected => (
                MockVoiceProvider::new(script),
                ScriptedCaller::new(vec![]).dropping_after(4),
            ),
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
        assert_eq!(reply["interrupted"], true, "{:?}", ending);
        assert_eq!(reply["actor"]["agent"]["kind"], "guest", "{:?}", ending);
    }

    teardown_test_db(&db_name).await;
}

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

#[tokio::test]
async fn a_duplicate_ask_leaves_the_caller_owed_an_answer() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = a_chat_thread(&pool).await;

    let provider = MockVoiceProvider::new(vec![
        the_caller_says("what have I got running"),
        asks_for_the_doer("they want today's threads"),
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

fn the_doer_answers(text: &str) -> ThreadEvent {
    ThreadEvent::ResponseGenerated {
        text: text.to_string(),
        images: vec![],
        model: None,
        reasoning_effort: None,
    }
}

fn the_doer_uses(tool: &str) -> ThreadEvent {
    ThreadEvent::ToolCalled {
        name: tool.to_string(),
        args: serde_json::json!({}),
        description: String::new(),
    }
}

#[tokio::test]
async fn a_doer_turn_sends_the_talker_one_working_note() {
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
            for tool in ["list_files", "grep_files", "read_file"] {
                seed_thread_event(&bus, thread_id, the_doer_uses(tool)).await;
            }
            seed_thread_event(&bus, thread_id, the_doer_answers("Three files.")).await;
            a_turn_starts(&bus, thread_id).await;
            seed_thread_event(&bus, thread_id, the_doer_uses("list_threads")).await;
            until("the second turn's note", || {
                log.lock()
                    .unwrap()
                    .history
                    .iter()
                    .any(|h| h.contains("list_threads"))
            })
            .await;
            hang_up.notify_one();
        }
    );

    {
        let log = log.lock().unwrap();
        let notes: Vec<&String> = log
            .history
            .iter()
            .filter(|h| h.starts_with(WORKING_NOTE_TAG))
            .collect();
        let expected = ["list_files", "list_threads"]
            .map(|tool| format!("{} Using {}.", WORKING_NOTE_TAG, tool));
        assert_eq!(
            notes,
            expected.iter().collect::<Vec<_>>(),
            "{:?}",
            log.history
        );
    }

    teardown_test_db(&db_name).await;
}

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
                widget: None,
                quiet: false,
                quiet_reply: None,
            },
            QuestionOption {
                id: "opt-1".to_string(),
                label: "Leave it for tonight".to_string(),
                description: None,
                preview: None,
                widget: None,
                quiet: false,
                quiet_reply: None,
            },
        ],
        worktree_path: None,
        multi_select: false,
        owner_approval: None,
        permission_request: None,
    }
}

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
        assert!(
            !log.history.iter().any(|h| h.contains("ask_user_question")),
            "the tool name reached the talker: {:?}",
            log.history
        );
    }

    teardown_test_db(&db_name).await;
}

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
                    compose: None,
                    tool_use_id: "toolu_q0".to_string(),
                    quiet: false,
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

#[tokio::test]
async fn a_permission_card_is_put_to_the_caller_out_loud_in_every_lane() {
    let lanes = [
        (
            "command",
            false,
            ThreadEvent::CommandPermissionRequested {
                decision_trace: None,
                request_id: "req-cmd".to_string(),
                tool_use_id: "toolu_b0".to_string(),
                tool_name: "run_bash".to_string(),
                command: "gh release delete v1".to_string(),
                summary: "Deletes a published release.".to_string(),
                grant_head: None,
                grant_offer: Some(crate::engine::thread_events::GrantOffer {
                    deny: None,
                    broad: Some("Bash".to_string()),
                    narrow: Some("Bash(gh:*)".to_string()),
                    session: Some("Bash(gh:*)".to_string()),
                    withheld: None,
                }),
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
                grant_offer: None,
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
        assert_eq!(
            spoken.contains("Always allow"),
            !on_a_coding_agent_thread,
            "{}: {}",
            lane,
            spoken
        );

        teardown_test_db(&db_name).await;
    }
}

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
                    decision_trace: None,
                    request_id: "req-cmd".to_string(),
                    tool_use_id: "toolu_b0".to_string(),
                    tool_name: "run_bash".to_string(),
                    command: "gh release delete v1".to_string(),
                    summary: "Deletes a published release.".to_string(),
                    grant_head: None,
                    grant_offer: None,
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
                    standing_deny: None,
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

#[test]
fn the_choices_are_read_out_with_the_ids_that_settle_them() {
    let one = [QuestionOption {
        id: "opt-0".to_string(),
        label: "Ship it".to_string(),
        description: None,
        preview: None,
        widget: None,
        quiet: false,
        quiet_reply: None,
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

#[test]
fn the_note_says_the_caller_answers_out_loud_and_never_on_screen() {
    let decision = OpenDecision::question("toolu_q0", "Ready?", &[], false);
    let note = decision_to_ask(&decision, true);
    assert!(!note.to_lowercase().contains("on screen"), "{}", note);
    assert!(note.contains("hand its id back"), "{}", note);
    assert!(note.contains("Never say an id out loud"), "{}", note);
}

#[test]
fn a_talker_with_no_answering_tool_is_told_to_hand_the_words_over() {
    let decision = OpenDecision::question("toolu_q0", "Ready?", &[], false);
    let note = decision_to_ask(&decision, false);
    assert!(!note.contains("hand its id back"), "{}", note);
    assert!(note.contains("in their own words"), "{}", note);
    assert!(note.contains("what settles this"), "{}", note);
    assert!(note.contains("Never say an id out loud"), "{}", note);
}

#[test]
fn a_permission_put_to_a_tool_less_talker_names_the_screen() {
    let note = decision_to_ask(
        &OpenDecision::tool_permission(&crate::engine::permission::legacy::LegacyCard::command(
            "req-1",
            "run_bash",
            "rm -rf build",
            "Deletes files.",
        )),
        false,
    );
    assert!(note.starts_with("[PERMISSION]"), "{}", note);
    assert!(note.contains("settle on their screen"), "{}", note);
    assert!(!note.contains("hand its id back"), "{}", note);
}

#[test]
fn a_permission_note_asks_for_a_say_so() {
    let note = decision_to_ask(
        &OpenDecision::tool_permission(&crate::engine::permission::legacy::LegacyCard::mcp(
            "req-1",
            "example-server",
            "Example Server",
            "post_message",
            "{\"channel\":\"general\"}",
        )),
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

#[test]
fn a_card_that_argues_for_itself_stays_inside_the_fence() {
    for card in crate::voice::cards_that_argue_for_themselves() {
        for holds_the_answer_tool in [true, false] {
            let note = decision_to_ask(&card, holds_the_answer_tool);
            crate::voice::assert_the_card_text_is_fenced(&note, &card);
        }
    }
}

#[test]
fn a_permission_note_allows_only_on_the_callers_spoken_yes() {
    let rule = crate::voice::PERMISSION_NEEDS_THEIR_WORDS;
    let card =
        OpenDecision::tool_permission(&crate::engine::permission::legacy::LegacyCard::command(
            "req-1",
            "run_bash",
            "make build",
            "Runs a build.",
        ));
    assert!(decision_to_ask(&card, true).contains(rule));

    let question = OpenDecision::question("toolu_q0", "Ready?", &[], false);
    assert!(!decision_to_ask(&question, true).contains(rule));
}

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
            say(&talker, the_caller_opens_the_floor()).await;
            until("the caller's first word", || {
                sent.lock().unwrap().len() >= 2
            })
            .await;

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
            opening(),
            subject(thread_id, uuid::Uuid::new_v4()),
        ),
        async {
            until("the session to open", || {
                log.lock().unwrap().openings.len() == 1
            })
            .await;

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

#[tokio::test]
async fn the_talkers_audio_alone_does_not_hold_the_floor() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = a_chat_thread(&pool).await;

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
    let mut caller = ScriptedCaller::new(vec![]).hanging_up_after(3);

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
    let mut caller = ScriptedCaller::new(vec![]).hanging_up_after(4);

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

#[tokio::test]
async fn an_answer_to_a_turn_begun_before_the_call_is_kept_not_said() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = a_chat_thread(&pool).await;
    let earlier = a_turn_starts(&bus, thread_id).await;

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
            opening(),
            subject(thread_id, uuid::Uuid::new_v4()),
        ),
        async {
            until("the session to open", || {
                log.lock().unwrap().openings.len() == 1
            })
            .await;
            the_turn_answers(&bus, thread_id, earlier).await;
            until("the answer to be kept", || {
                log.lock()
                    .unwrap()
                    .history
                    .iter()
                    .any(|h| h.starts_with(EARLIER_TAG))
            })
            .await;
            say(&talker, the_talker_is_saying("Tomorrow.")).await;
            say(&talker, VoiceEvent::Audio(vec![4; 64])).await;
            say(&talker, the_talker_says("Tomorrow.")).await;
            the_call_has_taken(&talker).await;
            hang_up(&from_caller).await;
        }
    );

    {
        let log = log.lock().unwrap();
        assert!(log.asked_to_speak.is_empty(), "{:?}", log.asked_to_speak);
        let kept: Vec<&String> = log
            .history
            .iter()
            .filter(|h| h.starts_with(EARLIER_TAG))
            .collect();
        assert_eq!(kept.len(), 1, "{:?}", log.history);
        assert!(kept[0].ends_with("Tomorrow."), "{}", kept[0]);
    }
    assert_eq!(*audio_out.lock().unwrap(), 0);
    let frames = sent.lock().unwrap().clone();
    assert!(
        !frames
            .iter()
            .any(|frame| matches!(frame, ServerFrame::TalkerTranscript { .. })),
        "{:?}",
        frames
    );

    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn an_injection_from_outside_the_call_keeps_an_earlier_answer_silent() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = a_chat_thread(&pool).await;
    let earlier = a_turn_starts(&bus, thread_id).await;

    let (provider, _talker) = MockVoiceProvider::driven();
    let log = provider.log();
    let turns = RecordingTurns::default();
    let (mut caller, from_caller) = ScriptedCaller::driven();

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
            for injected_message_id in [None, Some(uuid::Uuid::new_v4())] {
                bus.emit(crate::engine::event_bus::BusEvent::Thread {
                    thread_id,
                    event: ThreadEvent::PromptInjected {
                        text: "A background task finished.".to_string(),
                        mode: ActorMode::Engine,
                        origin: None,
                        injected_message_id,
                        delivered_event_id: None,
                    },
                    meta: crate::engine::thread_events::EventMeta {
                        request_event_id: Some(earlier),
                        ..crate::engine::thread_events::EventMeta::NONE
                    },
                })
                .await
                .expect("the injection is recorded");
            }
            the_turn_answers(&bus, thread_id, earlier).await;
            until("the answer to be kept", || {
                log.lock()
                    .unwrap()
                    .history
                    .iter()
                    .any(|h| h.starts_with(EARLIER_TAG))
            })
            .await;
            hang_up(&from_caller).await;
        }
    );

    let asked = log.lock().unwrap().asked_to_speak.clone();
    assert!(asked.is_empty(), "{:?}", asked);

    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn a_turn_begun_before_the_call_that_stops_is_kept_not_said() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = a_chat_thread(&pool).await;
    let earlier = a_turn_starts(&bus, thread_id).await;

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
            bus.emit(crate::engine::event_bus::BusEvent::Thread {
                thread_id,
                event: canceled(CancelCause::UserStop),
                meta: crate::engine::thread_events::EventMeta {
                    request_event_id: Some(earlier),
                    ..crate::engine::thread_events::EventMeta::NONE
                },
            })
            .await
            .expect("the stop is recorded");
            until("the stop to be kept", || {
                log.lock()
                    .unwrap()
                    .history
                    .iter()
                    .any(|h| h == UNFINISHED_EARLIER)
            })
            .await;
            hang_up.notify_one();
        }
    );

    assert!(log.lock().unwrap().asked_to_speak.is_empty());

    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn an_answer_to_a_turn_typed_during_the_call_is_said() {
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
            let typed = a_turn_starts(&bus, thread_id).await;
            the_turn_answers(&bus, thread_id, typed).await;
            until("the answer to be said", || {
                log.lock().unwrap().asked_to_speak.len() == 1
            })
            .await;
            hang_up.notify_one();
        }
    );

    {
        let log = log.lock().unwrap();
        assert!(
            log.asked_to_speak[0].ends_with("Tomorrow."),
            "{}",
            log.asked_to_speak[0]
        );
        assert!(!log.history.iter().any(|h| h.starts_with(EARLIER_TAG)));
    }

    teardown_test_db(&db_name).await;
}

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

#[test]
fn the_answer_is_handed_over_to_be_said_not_read() {
    let framing = answer_to_say("Both endpoints answered live.");
    assert!(framing.contains("Do not read it out"), "{}", framing);
    assert!(
        !framing.contains("Say this to the caller"),
        "the talker was handed a script: {}",
        framing
    );
    assert!(
        framing.contains("Both endpoints answered live."),
        "{}",
        framing
    );
}

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

#[test]
fn a_short_answer_is_not_padded_with_an_offer() {
    let framing = answer_to_say("Yes, both are green.");
    assert!(
        !framing.contains("want the detail"),
        "a one-line answer offered more: {}",
        framing
    );
}

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

    assert_eq!(*overheard.lock().unwrap(), vec!["Still on it.".to_string()]);

    teardown_test_db(&db_name).await;
}

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

            talker
                .send(the_talker_says("Two things are running."))
                .await
                .expect("the talker is listening");
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

#[tokio::test]
async fn a_relayed_answer_writes_the_question_before_the_stall() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = a_chat_thread(&pool).await;

    let (provider, talker) = MockVoiceProvider::driven();
    let log = provider.log();
    let turns = RecordingTurns::default();
    let mut caller = ScriptedCaller::new(vec![]).hanging_up_after(4);

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
            talker
                .send(the_caller_says("what have I got running"))
                .await
                .expect("the talker is listening");
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
            opening(),
            subject(thread_id, uuid::Uuid::new_v4()),
        ),
        async {
            until("the session to open", || {
                log.lock().unwrap().openings.len() == 1
            })
            .await;
            say(&talker, the_caller_opens_the_floor()).await;
            await_frames(&sent, 1, "the caller's first word").await;
            say(&talker, the_talker_is_saying("Let me check")).await;
            await_frames(&sent, 2, "the talker to take the floor").await;
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

fn answers_with(choice_id: &str) -> VoiceEvent {
    VoiceEvent::AnswerRequested {
        tool_call_id: "call_a".to_string(),
        choice_id: choice_id.to_string(),
    }
}

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
        opening(),
        subject(thread_id, uuid::Uuid::new_v4()),
    )
    .await;

    let woken = turns.woken().lock().unwrap().clone();
    (log, woken)
}

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
    assert!(woken.is_empty(), "{:?}", woken);

    teardown_test_db(&db_name).await;
}

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
    assert!(
        !voice_kinds(&events).contains(&"WorkDelegated".to_string()),
        "{:?}",
        voice_kinds(&events)
    );

    teardown_test_db(&db_name).await;
}

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

    let resolved = log.lock().unwrap().resolved_tool_calls.clone();
    assert_eq!(resolved.len(), 1, "{:?}", resolved);
    assert!(resolved[0].1.contains("Answered"), "{:?}", resolved);

    teardown_test_db(&db_name).await;
}

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

    assert!(turns.woken().lock().unwrap().is_empty());
    let events = thread_events(&pool, thread_id).await;
    let kinds = voice_kinds(&events);
    assert!(!kinds.contains(&"WorkDelegated".to_string()), "{:?}", kinds);

    assert_eq!(
        kinds
            .iter()
            .filter(|k| *k == "SpokenMessageReceived")
            .count(),
        1,
        "{:?}",
        kinds
    );

    let resolved = log.lock().unwrap().resolved_tool_calls.clone();
    assert_eq!(resolved.len(), 1, "{:?}", resolved);
    assert!(resolved[0].1.contains("Not started"), "{:?}", resolved);

    teardown_test_db(&db_name).await;
}

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
        opening(),
        subject(thread_id, uuid::Uuid::new_v4()),
    )
    .await;

    assert_eq!(
        asked.lock().unwrap().clone(),
        vec![(
            format!("question:{}#said", PARKED_QUESTION),
            "he can just set up the MCP server, discard any follow up".to_string()
        )]
    );

    assert!(turns.woken().lock().unwrap().is_empty());
    let kinds = voice_kinds(&thread_events(&pool, thread_id).await);
    assert!(!kinds.contains(&"WorkDelegated".to_string()), "{:?}", kinds);

    let resolved = log.lock().unwrap().resolved_tool_calls.clone();
    assert_eq!(resolved.len(), 1, "{:?}", resolved);
    assert!(resolved[0].1.contains("Answered"), "{:?}", resolved);

    teardown_test_db(&db_name).await;
}

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

    let resolved = log.lock().unwrap().resolved_tool_calls.clone();
    assert_eq!(resolved.len(), 1, "{:?}", resolved);
    assert_eq!(resolved[0].0, "call_h");

    let events = thread_events(&pool, thread_id).await;
    let kinds: Vec<&str> = events.iter().map(|(kind, _)| kind.as_str()).collect();
    assert!(
        kinds.contains(&"SpokenReplyGenerated"),
        "the goodbye was never recorded: {:?}",
        kinds
    );
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
        answers_with("question:toolu_q0#said"),
        asks_for_the_doer("they want the second half"),
        the_caller_says("neither, do the second half only"),
        the_talker_says("Right."),
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
        the_caller_says("   "),
        the_talker_says("Sorry, I missed that."),
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

    let asked = asked.lock().unwrap().clone();
    assert_eq!(asked.len(), 2, "{:?}", asked);
    assert!(
        asked.iter().all(|(_, spoken)| spoken.is_empty()),
        "{:?}",
        asked
    );

    let resolved = log.lock().unwrap().resolved_tool_calls.clone();
    assert_eq!(resolved.len(), 1, "{:?}", resolved);
    assert!(resolved[0].1.contains("dropped"), "{:?}", resolved);

    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn seven_transcription_items_are_seven_rows_and_one_sentence() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = a_chat_thread(&pool).await;
    let session_id = uuid::Uuid::new_v4();

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
        for _ in 0..3 {
            script.push(the_talker_says(""));
        }
    }

    let did = a_call_that_hears(&pool, &bus, thread_id, session_id, script).await;

    let rows = voice_rows(&did.events);
    assert_eq!(rows.len(), fragments.len(), "{:?}", rows);
    assert!(rows.iter().all(|(kind, _)| kind == "SpokenMessageReceived"));

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

#[tokio::test]
async fn a_caller_nobody_answers_reaches_the_doer_and_is_told_so() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = a_chat_thread(&pool).await;
    let session_id = uuid::Uuid::new_v4();

    let provider = MockVoiceProvider::new(vec![
        the_caller_says("why didn't you tell me to restart the computer"),
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

#[tokio::test]
async fn words_the_session_was_still_holding_reach_the_row() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = a_chat_thread(&pool).await;
    let session_id = uuid::Uuid::new_v4();

    let provider = MockVoiceProvider::new(vec![the_caller_says("restart the computer")])
        .still_holding(vec![the_caller_says("when it is idle")]);
    let turns = RecordingTurns::default();
    let mut caller = ScriptedCaller::new(vec![]).hanging_up_after(2);

    run_call(
        &bus,
        &provider,
        &mut caller,
        &turns,
        free_doer(),
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

    let rows = voice_rows(&did.events);
    assert_eq!(rows.len(), 2, "{:?}", rows);
    assert_eq!(rows[0].1["text"], "why didn't you tell me");
    assert_eq!(rows[1].1["text"], "to restart the computer");
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

#[tokio::test]
async fn words_the_bound_spends_are_not_written_twice() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = a_chat_thread(&pool).await;
    let session_id = uuid::Uuid::new_v4();

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
    let mut caller = ScriptedCaller::new(vec![]).hanging_up_after(2);
    let sent = Arc::clone(&caller.sent);
    let audio_out = Arc::clone(&caller.audio_out_bytes);

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
        opening(),
        subject(thread_id, uuid::Uuid::new_v4()),
    )
    .await;

    let replies = rows_of(&pool, thread_id, "SpokenReplyGenerated").await;
    assert_eq!(replies.len(), 1, "{:?}", replies);

    teardown_test_db(&db_name).await;
}

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
        opening(),
        subject(thread_id, uuid::Uuid::new_v4()),
    )
    .await;

    assert_eq!(*audio_out.lock().unwrap(), 320);
    let replies = rows_of(&pool, thread_id, "SpokenReplyGenerated").await;
    assert_eq!(replies.len(), 1, "{:?}", replies);

    teardown_test_db(&db_name).await;
}

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

#[tokio::test]
async fn a_muted_run_is_not_heard_from_its_middle() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = a_chat_thread(&pool).await;

    let provider = MockVoiceProvider::new(vec![
        the_talker_is_saying("Preflight flagged the deletion of that tap-shape migration"),
        the_caller_says("What about"),
        the_talker_says("Preflight flagged the deletion of that tap-shape migration"),
        the_talker_is_saying("doc. It says it's intentional."),
        VoiceEvent::Audio(vec![3; 320]),
        the_talker_says("doc. It says it's intentional."),
    ]);
    let turns = RecordingTurns::default();
    let overheard = turns.overheard();
    let mut caller = ScriptedCaller::new(vec![]).hanging_up_after(4);
    let sent = Arc::clone(&caller.sent);
    let audio_out = Arc::clone(&caller.audio_out_bytes);

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
    let mut caller = ScriptedCaller::new(vec![]).hanging_up_after(6);
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

    assert_eq!(*audio_out.lock().unwrap(), 480);
    let replies = rows_of(&pool, thread_id, "SpokenReplyGenerated").await;
    assert_eq!(replies.len(), 1, "{:?}", replies);
    assert_eq!(replies[0].0["text"], "Two threads are running.");

    teardown_test_db(&db_name).await;
}

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

            say(&talker, the_talker_is_saying("There's one waiting")).await;
            say(&talker, the_talker_says("There's one waiting")).await;
            await_frames(&sent, 2, "the first breath to end").await;

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

async fn the_device_hears_a_sound(from_caller: &tokio::sync::mpsc::Sender<CallerFrame>) {
    let idle = from_caller.capacity();
    from_caller
        .send(CallerFrame::Control(ClientControl::CallerStartedSpeaking))
        .await
        .expect("the call to take the caller's signal");
    until("the call to take that signal", || {
        from_caller.capacity() == idle
    })
    .await;
}

async fn the_call_has_taken(talker: &tokio::sync::mpsc::Sender<VoiceEvent>) {
    let idle = talker.max_capacity();
    until("the call to take the talker's frames", || {
        talker.capacity() == idle
    })
    .await;
}

#[tokio::test]
async fn the_first_answer_is_held_until_the_callers_words_then_heard_whole() {
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
            opening(),
            subject(thread_id, uuid::Uuid::new_v4()),
        ),
        async {
            until("the session to open", || {
                log.lock().unwrap().openings.len() == 1
            })
            .await;
            the_device_hears_a_sound(&from_caller).await;
            say(&talker, the_talker_is_saying("Sure, I can")).await;
            say(&talker, VoiceEvent::Audio(vec![4; 256])).await;
            say(&talker, the_talker_says("Sure, I can")).await;
            the_call_has_taken(&talker).await;
            assert_eq!(sent.lock().unwrap().len(), 1, "held, so nothing sent yet");
            assert_eq!(*audio_out.lock().unwrap(), 0, "held, so nothing played yet");
            say(&talker, the_caller_is_saying("Please")).await;
            say(&talker, the_talker_is_saying(" do that.")).await;
            say(&talker, VoiceEvent::Audio(vec![5; 128])).await;
            say(&talker, the_talker_says(" do that.")).await;
            await_frames(&sent, 5, "the whole answer to land").await;
            hang_up(&from_caller).await;
        }
    );

    assert_eq!(*audio_out.lock().unwrap(), 384, "every held frame, once");
    let frames: Vec<String> = sent
        .lock()
        .unwrap()
        .iter()
        .skip(1)
        .take(5)
        .map(|frame| format!("{:?}", frame))
        .collect();
    assert_eq!(
        frames,
        vec![
            format!(
                "{:?}",
                ServerFrame::UserTranscript {
                    text: "Please".into()
                }
            ),
            format!(
                "{:?}",
                ServerFrame::TalkerTranscript {
                    text: "Sure, I can".into()
                }
            ),
            format!("{:?}", ServerFrame::TalkerTurnEnded),
            format!(
                "{:?}",
                ServerFrame::TalkerTranscript {
                    text: " do that.".into()
                }
            ),
            format!("{:?}", ServerFrame::TalkerTurnEnded),
        ]
    );
    let replies = rows_of(&pool, thread_id, "SpokenReplyGenerated").await;
    let said: Vec<&str> = replies
        .iter()
        .map(|(payload, _)| payload["text"].as_str().unwrap_or_default())
        .collect();
    assert_eq!(said, vec!["Sure, I can", "do that."]);
    assert_eq!(replies[0].0["interrupted"], false);

    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn a_sound_before_any_words_lets_no_talker_sentence_through() {
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
            opening(),
            subject(thread_id, uuid::Uuid::new_v4()),
        ),
        async {
            until("the session to open", || {
                log.lock().unwrap().openings.len() == 1
            })
            .await;
            the_device_hears_a_sound(&from_caller).await;
            say(&talker, the_talker_is_saying("Yeah. Same thing.")).await;
            say(&talker, VoiceEvent::Audio(vec![4; 256])).await;
            say(&talker, the_talker_says("Yeah. Same thing.")).await;
            await_frames(&sent, 1, "the unheard turn to end").await;
            hang_up(&from_caller).await;
        }
    );

    assert_eq!(*audio_out.lock().unwrap(), 0);
    let frames = sent.lock().unwrap().clone();
    assert!(
        !frames
            .iter()
            .any(|frame| matches!(frame, ServerFrame::TalkerTranscript { .. })),
        "{:?}",
        frames
    );
    let replies = rows_of(&pool, thread_id, "SpokenReplyGenerated").await;
    assert!(replies.is_empty(), "{:?}", replies);

    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn a_recitation_over_a_sound_at_call_open_is_never_heard() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = a_chat_thread(&pool).await;

    let (provider, talker) = MockVoiceProvider::driven();
    let log = provider.log();
    let turns = RecordingTurns::default();
    let (mut caller, from_caller) = ScriptedCaller::driven();
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
            the_device_hears_a_sound(&from_caller).await;
            for line in [
                "Yeah.",
                "Yes...",
                "Right. Same thing. That batch lost its landing ownership.",
            ] {
                say(&talker, the_talker_is_saying(line)).await;
                say(&talker, VoiceEvent::Audio(vec![4; 64])).await;
                say(&talker, the_talker_says(line)).await;
            }
            await_frames(&sent, 3, "the three unheard turns to end").await;
            say(&talker, the_caller_says("So what's going on now")).await;
            say(&talker, the_talker_is_saying("Two threads are running.")).await;
            say(&talker, the_talker_says("Two threads are running.")).await;
            await_frames(&sent, 6, "the real answer to end").await;
            hang_up(&from_caller).await;
        }
    );

    let events = thread_events(&pool, thread_id).await;
    assert_eq!(
        spoken_kinds(&events),
        vec!["SpokenMessageReceived", "SpokenReplyGenerated"]
    );
    let replies = rows_of(&pool, thread_id, "SpokenReplyGenerated").await;
    assert_eq!(replies[0].0["text"], "Two threads are running.");

    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn a_held_unfinished_sentence_keeps_no_answer_waiting() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = a_chat_thread(&pool).await;

    let (provider, talker) = MockVoiceProvider::driven();
    let log = provider.log();
    let turns = RecordingTurns::default();
    let (mut caller, from_caller) = ScriptedCaller::driven();

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
            the_device_hears_a_sound(&from_caller).await;
            say(&talker, the_talker_is_saying("So earlier we")).await;
            say(&talker, the_talker_says("So earlier we")).await;
            the_call_has_taken(&talker).await;
            a_turn_starts(&bus, thread_id).await;
            seed_thread_event(&bus, thread_id, asks_the_caller()).await;
            until("the question to be handed to the talker", || {
                log.lock().unwrap().asked_to_speak.len() == 1
            })
            .await;
            hang_up(&from_caller).await;
        }
    );

    let replies = rows_of(&pool, thread_id, "SpokenReplyGenerated").await;
    assert!(
        replies.is_empty(),
        "the held sentence went unheard: {:?}",
        replies
    );

    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn an_answer_queued_behind_a_hold_waits_for_the_released_turn() {
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
            opening(),
            subject(thread_id, uuid::Uuid::new_v4()),
        ),
        async {
            until("the session to open", || {
                log.lock().unwrap().openings.len() == 1
            })
            .await;
            the_device_hears_a_sound(&from_caller).await;
            say(&talker, the_talker_is_saying("Sure, I can")).await;
            say(&talker, the_talker_says("Sure, I can")).await;
            say(&talker, the_talker_is_saying(" do")).await;
            say(&talker, VoiceEvent::Audio(vec![5; 128])).await;
            the_call_has_taken(&talker).await;
            a_turn_starts(&bus, thread_id).await;
            seed_thread_event(&bus, thread_id, asks_the_caller()).await;
            seed_thread_event(&bus, thread_id, the_doer_uses("list_files")).await;
            until("the call to read the question", || {
                log.lock()
                    .unwrap()
                    .history
                    .iter()
                    .any(|h| h.starts_with(WORKING_NOTE_TAG))
            })
            .await;
            say(&talker, the_caller_is_saying("Please")).await;
            the_call_has_taken(&talker).await;
            assert!(
                log.lock().unwrap().asked_to_speak.is_empty(),
                "the released turn is still in flight"
            );
            say(&talker, the_talker_says(" do that.")).await;
            until("the question to be handed to the talker", || {
                log.lock().unwrap().asked_to_speak.len() == 1
            })
            .await;
            hang_up(&from_caller).await;
        }
    );

    assert_eq!(*audio_out.lock().unwrap(), 128, "the released audio, once");
    let frames = sent.lock().unwrap().clone();
    assert!(
        frames.iter().any(|frame| matches!(
            frame,
            ServerFrame::TalkerTranscript { text } if text == " do"
        )),
        "{:?}",
        frames
    );

    teardown_test_db(&db_name).await;
}

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
    let mut caller = ScriptedCaller::new(vec![]).hanging_up_after(4);
    let audio_out = Arc::clone(&caller.audio_out_bytes);
    let sent = Arc::clone(&caller.sent);

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

    assert_eq!(*audio_out.lock().unwrap(), 256);
    let replies = rows_of(&pool, thread_id, "SpokenReplyGenerated").await;
    assert_eq!(replies.len(), 1, "{:?}", replies);
    assert_eq!(replies[0].0["text"], "Two threads");
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

#[tokio::test]
async fn the_caller_speaking_again_buys_another_answer() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = a_chat_thread(&pool).await;

    let bound = super::TURNS_ONE_OPENER_BUYS as usize;
    let mut script = vec![the_caller_says("okay, how are we")];
    script.extend(RECITED.iter().take(bound).map(|line| the_talker_says(line)));
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
            opening(),
            subject(thread_id, uuid::Uuid::new_v4()),
        ),
        async {
            until("the session to open", || {
                log.lock().unwrap().openings.len() == 1
            })
            .await;

            say(&talker, the_caller_says("okay, how are we")).await;
            for line in RECITED.iter().take(bound) {
                say(&talker, the_talker_says(line)).await;
            }
            say(&talker, the_talker_says("nobody hears this one")).await;
            await_frames(&sent, bound + 2, "the budget to be spent").await;

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

struct AnswerLostToALag<'a> {
    inner: MockVoiceProvider,
    bus: &'a EventBus,
    thread_id: uuid::Uuid,
    answer: &'static str,
}

#[async_trait::async_trait]
impl crate::voice::provider::VoiceProvider for AnswerLostToALag<'_> {
    fn name(&self) -> &'static str {
        crate::voice::provider::VoiceProvider::name(&self.inner)
    }

    fn model(&self) -> &str {
        crate::voice::provider::VoiceProvider::model(&self.inner)
    }

    async fn open(
        &self,
        opening: SessionOpening,
    ) -> Result<Box<dyn crate::voice::provider::VoiceSession>, BoxError> {
        a_turn_starts(self.bus, self.thread_id).await;
        seed_thread_event(
            self.bus,
            self.thread_id,
            ThreadEvent::ResponseGenerated {
                text: self.answer.to_string(),
                images: vec![],
                model: None,
                reasoning_effort: None,
            },
        )
        .await;
        crate::test_support::flood_past_bus_capacity(self.bus);
        crate::voice::provider::VoiceProvider::open(&self.inner, opening).await
    }
}

#[tokio::test]
async fn an_answer_lost_to_a_lag_is_still_said_once() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = a_chat_thread(&pool).await;
    let (inner, talker) = MockVoiceProvider::driven();
    let log = inner.log();
    let provider = AnswerLostToALag {
        inner,
        bus: &bus,
        thread_id,
        answer: "Two things are running.",
    };
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
            say(&talker, the_caller_opens_the_floor()).await;
            until("the answer to be said out loud", || {
                log.lock().unwrap().asked_to_speak.len() == 1
            })
            .await;
            hang_up.notify_one();
        }
    );

    let said = log.lock().unwrap().asked_to_speak.clone();
    assert_eq!(said.len(), 1, "said once, never twice: {:?}", said);
    assert!(said[0].contains("Two things are running."), "{:?}", said);

    teardown_test_db(&db_name).await;
}

mod guard_tests {
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
        assert!(scanned > 5, "the scan found no voice sources to read");
        assert!(
            offenders.is_empty(),
            "a call must not end the doer's turn: {:?}",
            offenders
        );
    }
}
