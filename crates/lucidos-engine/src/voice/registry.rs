//! Which threads have a live voice session right now.
//!
//! One session per thread. A second upgrade against a busy thread is refused,
//! which is what keeps every `VoiceSessionStarted` paired with exactly one
//! `VoiceSessionEnded`. Without the rule, two overlapping calls on one thread
//! write two starts and the pair stops meaning anything.
//!
//! Transient by design, and reset to zero on restart. A live call cannot
//! survive the process holding its socket, so a slot that outlived one would be
//! a lie. The boot sweep settles the events those dead sessions left behind.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use uuid::Uuid;

/// Shared, cloneable handle to the live-session map. Held on `LucidosEngine`,
/// the same way `SseConnectionCounter` is.
#[derive(Default, Clone)]
pub struct LiveVoiceSessions {
    live: Arc<Mutex<HashMap<Uuid, Uuid>>>,
}

impl LiveVoiceSessions {
    pub fn new() -> Self {
        Self::default()
    }

    /// Take this thread's one slot, or `None` when a session already holds it.
    ///
    /// The returned guard frees the slot on drop, so a panicking handler
    /// cannot strand a thread as permanently busy.
    #[must_use]
    pub fn claim(&self, thread_id: Uuid, session_id: Uuid) -> Option<VoiceSessionSlot> {
        let mut live = self.live.lock().expect("voice session registry lock");
        if live.contains_key(&thread_id) {
            return None;
        }
        live.insert(thread_id, session_id);
        Some(VoiceSessionSlot {
            live: Arc::clone(&self.live),
            thread_id,
        })
    }

    /// Whether a voice session is live on this thread.
    pub fn is_live(&self, thread_id: Uuid) -> bool {
        self.live
            .lock()
            .expect("voice session registry lock")
            .contains_key(&thread_id)
    }

    /// How many calls are up across the workspace.
    pub fn count(&self) -> usize {
        self.live.lock().expect("voice session registry lock").len()
    }
}

/// A held slot. Dropping it frees the thread for the next call.
pub struct VoiceSessionSlot {
    live: Arc<Mutex<HashMap<Uuid, Uuid>>>,
    thread_id: Uuid,
}

impl Drop for VoiceSessionSlot {
    fn drop(&mut self) {
        if let Ok(mut live) = self.live.lock() {
            live.remove(&self.thread_id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_thread_holds_one_session_at_a_time() {
        let sessions = LiveVoiceSessions::new();
        let thread = Uuid::new_v4();
        let first = sessions.claim(thread, Uuid::new_v4());
        assert!(first.is_some());
        assert!(sessions.claim(thread, Uuid::new_v4()).is_none());
        assert!(sessions.is_live(thread));
    }

    #[test]
    fn dropping_the_slot_frees_the_thread() {
        let sessions = LiveVoiceSessions::new();
        let thread = Uuid::new_v4();
        drop(sessions.claim(thread, Uuid::new_v4()));
        assert!(!sessions.is_live(thread));
        assert!(sessions.claim(thread, Uuid::new_v4()).is_some());
    }

    #[test]
    fn two_threads_can_each_hold_a_call() {
        let sessions = LiveVoiceSessions::new();
        let _a = sessions.claim(Uuid::new_v4(), Uuid::new_v4()).expect("a");
        let _b = sessions.claim(Uuid::new_v4(), Uuid::new_v4()).expect("b");
        assert_eq!(sessions.count(), 2);
    }
}
