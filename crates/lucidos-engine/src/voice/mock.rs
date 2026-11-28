use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use tokio::sync::mpsc;

use super::provider::{SessionOpening, VoiceEvent, VoiceProvider, VoiceSession};

type BoxError = Box<dyn std::error::Error + Send + Sync>;

#[derive(Debug, Default)]
pub struct MockLog {
    pub openings: Vec<SessionOpening>,
    pub history: Vec<String>,
    pub audio_in_bytes: usize,
    pub asked_to_speak: Vec<String>,
    pub resolved_tool_calls: Vec<(String, String)>,
    pub cancels: usize,
    pub closed: bool,
}

pub struct MockVoiceProvider {
    script: Mutex<Vec<VoiceEvent>>,
    live: Mutex<Option<mpsc::Receiver<VoiceEvent>>>,
    holding: Mutex<Vec<VoiceEvent>>,
    log: Arc<Mutex<MockLog>>,
    open_error: Option<String>,
    ends_after_script: bool,
    holds_the_answer_tool: bool,
}

impl MockVoiceProvider {
    /// INVARIANT: A provider whose session replays `script` and then goes quiet.
    ///
    /// Quiet, NOT closed. An exhausted script is a talker with nothing more to
    /// say, and the loop reads a closed session as the provider dropping the
    /// call. Conflating the two made a hangup test report a provider failure.
    /// Use [`Self::ending_after`] for the case where the talker really drops.
    pub fn new(script: Vec<VoiceEvent>) -> Self {
        Self {
            script: Mutex::new(script),
            live: Mutex::new(None),
            holding: Mutex::new(Vec::new()),
            log: Arc::new(Mutex::new(MockLog::default())),
            open_error: None,
            ends_after_script: false,
            holds_the_answer_tool: true,
        }
    }

    pub fn holding_no_answer_tool(mut self) -> Self {
        self.holds_the_answer_tool = false;
        self
    }

    pub fn driven() -> (Self, mpsc::Sender<VoiceEvent>) {
        let (tx, rx) = mpsc::channel(16);
        let mut provider = Self::new(vec![]);
        provider.live = Mutex::new(Some(rx));
        (provider, tx)
    }

    pub fn still_holding(mut self, held: Vec<VoiceEvent>) -> Self {
        self.holding = Mutex::new(held);
        self
    }

    pub fn ending_after(script: Vec<VoiceEvent>) -> Self {
        Self {
            ends_after_script: true,
            ..Self::new(script)
        }
    }

    pub fn refusing(message: &str) -> Self {
        Self {
            script: Mutex::new(Vec::new()),
            live: Mutex::new(None),
            holding: Mutex::new(Vec::new()),
            log: Arc::new(Mutex::new(MockLog::default())),
            open_error: Some(message.to_string()),
            ends_after_script: false,
            holds_the_answer_tool: true,
        }
    }

    pub fn log(&self) -> Arc<Mutex<MockLog>> {
        Arc::clone(&self.log)
    }
}

#[async_trait]
impl VoiceProvider for MockVoiceProvider {
    fn name(&self) -> &'static str {
        "mock"
    }

    fn model(&self) -> &str {
        "mock-talker"
    }

    fn holds_the_answer_tool(&self) -> bool {
        self.holds_the_answer_tool
    }

    async fn open(&self, opening: SessionOpening) -> Result<Box<dyn VoiceSession>, BoxError> {
        if let Some(message) = &self.open_error {
            return Err(message.clone().into());
        }
        let script = std::mem::take(&mut *self.script.lock().expect("mock script lock"));
        {
            let mut log = self.log.lock().expect("mock log lock");
            log.history.push(opening.resident_block.clone());
            log.openings.push(opening);
        }
        Ok(Box::new(MockVoiceSession {
            script: script.into_iter().collect(),
            live: self.live.lock().expect("mock live lock").take(),
            holding: std::mem::take(&mut *self.holding.lock().expect("mock holding lock")),
            log: Arc::clone(&self.log),
            ends_after_script: self.ends_after_script,
        }))
    }
}

struct MockVoiceSession {
    script: std::collections::VecDeque<VoiceEvent>,
    live: Option<mpsc::Receiver<VoiceEvent>>,
    holding: Vec<VoiceEvent>,
    log: Arc<Mutex<MockLog>>,
    ends_after_script: bool,
}

#[async_trait]
impl VoiceSession for MockVoiceSession {
    async fn push_audio(&mut self, pcm: &[u8]) -> Result<(), BoxError> {
        self.log.lock().expect("mock log lock").audio_in_bytes += pcm.len();
        Ok(())
    }

    async fn append_context(&mut self, note: &str) -> Result<(), BoxError> {
        self.log
            .lock()
            .expect("mock log lock")
            .history
            .push(note.to_string());
        Ok(())
    }

    async fn speak(&mut self, note: &str) -> Result<(), BoxError> {
        self.append_context(note).await?;
        self.log
            .lock()
            .expect("mock log lock")
            .asked_to_speak
            .push(note.to_string());
        Ok(())
    }

    async fn resolve_tool_call(&mut self, tool_call_id: &str, note: &str) -> Result<(), BoxError> {
        self.log
            .lock()
            .expect("mock log lock")
            .resolved_tool_calls
            .push((tool_call_id.to_string(), note.to_string()));
        Ok(())
    }

    async fn next(&mut self) -> Option<VoiceEvent> {
        if let Some(live) = &mut self.live {
            return match live.recv().await {
                Some(event) => Some(event),
                None => std::future::pending().await,
            };
        }
        match self.script.pop_front() {
            Some(event) => Some(event),
            None if self.ends_after_script => None,
            None => std::future::pending().await,
        }
    }

    async fn cancel(&mut self) -> Result<(), BoxError> {
        self.log.lock().expect("mock log lock").cancels += 1;
        Ok(())
    }

    async fn caller_words_were_taken(&mut self) {
        self.holding.retain(|event| {
            !matches!(
                event,
                VoiceEvent::UserTurnEnded { .. } | VoiceEvent::UserTranscript { .. }
            )
        });
    }

    async fn close(&mut self) -> Vec<VoiceEvent> {
        self.log.lock().expect("mock log lock").closed = true;
        std::mem::take(&mut self.holding)
    }
}
