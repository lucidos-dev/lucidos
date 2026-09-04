//! A scripted [`VoiceProvider`] the tests drive.
//!
//! It answers the questions a real provider cannot answer cheaply: what the
//! session was opened with, in what order history grew, and what the runtime
//! does when a talker fails mid-call. No socket, no credential, no latency.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use tokio::sync::mpsc;

use super::provider::{SessionOpening, VoiceEvent, VoiceProvider, VoiceSession};

type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// Everything a mock session recorded, readable while the session is still
/// alive. Shared so a test can assert without owning the session.
#[derive(Debug, Default)]
pub struct MockLog {
    /// What each `open` was given. One entry per session.
    pub openings: Vec<SessionOpening>,
    /// The session's history, oldest first.
    ///
    /// Item 0 is the resident block. Every later item is an append. Nothing
    /// here can rewrite an earlier entry, which is the append-only invariant
    /// expressed as a data structure rather than as a rule.
    pub history: Vec<String>,
    /// Caller audio bytes pushed upstream. Counted, never kept: no audio is
    /// written down anywhere (parent plan, decision 12).
    pub audio_in_bytes: usize,
    /// Items the talker was asked to answer out loud, oldest first. Every one
    /// is in `history` too: a speak appends and then asks.
    pub asked_to_speak: Vec<String>,
    /// Tool calls answered, as `(tool_call_id, note)`, oldest first. An
    /// unanswered one leaves the talker holding a call it never heard back on.
    pub resolved_tool_calls: Vec<(String, String)>,
    pub cancels: usize,
    pub closed: bool,
}

/// A provider that hands out sessions replaying a fixed script.
pub struct MockVoiceProvider {
    script: Mutex<Vec<VoiceEvent>>,
    /// A talker the test drives event by event, instead of by a fixed script.
    live: Mutex<Option<mpsc::Receiver<VoiceEvent>>>,
    log: Arc<Mutex<MockLog>>,
    open_error: Option<String>,
    ends_after_script: bool,
}

impl MockVoiceProvider {
    /// A provider whose session replays `script` and then goes quiet.
    ///
    /// Quiet, NOT closed. An exhausted script is a talker with nothing more to
    /// say, and the loop reads a closed session as the provider dropping the
    /// call. Conflating the two made a hangup test report a provider failure.
    /// Use [`Self::ending_after`] for the case where the talker really drops.
    pub fn new(script: Vec<VoiceEvent>) -> Self {
        Self {
            script: Mutex::new(script),
            live: Mutex::new(None),
            log: Arc::new(Mutex::new(MockLog::default())),
            open_error: None,
            ends_after_script: false,
        }
    }

    /// A talker the test drives one event at a time.
    ///
    /// A fixed script drains as fast as the loop polls it, so it cannot express
    /// "the talker is still speaking WHILE this happens". The floor rule is
    /// exactly that shape, and this is what lets a test state it.
    ///
    /// Closing the sender leaves the talker quiet, not gone, matching
    /// [`Self::new`].
    pub fn driven() -> (Self, mpsc::Sender<VoiceEvent>) {
        let (tx, rx) = mpsc::channel(16);
        let mut provider = Self::new(vec![]);
        provider.live = Mutex::new(Some(rx));
        (provider, tx)
    }

    /// A provider that replays `script` and then drops the call.
    pub fn ending_after(script: Vec<VoiceEvent>) -> Self {
        Self {
            ends_after_script: true,
            ..Self::new(script)
        }
    }

    /// A provider that refuses to open, so the runtime's failure path is
    /// reachable without a live credential.
    pub fn refusing(message: &str) -> Self {
        Self {
            script: Mutex::new(Vec::new()),
            live: Mutex::new(None),
            log: Arc::new(Mutex::new(MockLog::default())),
            open_error: Some(message.to_string()),
            ends_after_script: false,
        }
    }

    /// The shared record. Clone it before opening a session.
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
            log: Arc::clone(&self.log),
            ends_after_script: self.ends_after_script,
        }))
    }
}

struct MockVoiceSession {
    script: std::collections::VecDeque<VoiceEvent>,
    live: Option<mpsc::Receiver<VoiceEvent>>,
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
            // Quiet, not gone. See `MockVoiceProvider::new`.
            None => std::future::pending().await,
        }
    }

    async fn cancel(&mut self) -> Result<(), BoxError> {
        self.log.lock().expect("mock log lock").cancels += 1;
        Ok(())
    }

    async fn close(&mut self) {
        self.log.lock().expect("mock log lock").closed = true;
    }
}
