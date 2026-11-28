use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use uuid::Uuid;

/// INVARIANT: Orders a turn's start with a rung pick, per thread. A turn holds its
/// thread's gate from resolving its pair until its starter event is recorded.
/// A pick waits for the short start section, then records next-turn settings.
#[derive(Default)]
pub struct TurnStartGate {
    gates: Mutex<HashMap<Uuid, Arc<tokio::sync::Mutex<()>>>>,
    /// INVARIANT: Picks queue here before they try the gate, so a busy gate always
    /// means a turn is starting, never another pick.
    pub picks: tokio::sync::Mutex<()>,
}

pub struct TurnStartHold<'a> {
    gate: &'a TurnStartGate,
    thread_id: Uuid,
    guard: Option<tokio::sync::OwnedMutexGuard<()>>,
}

impl TurnStartGate {
    fn entry(&self, thread_id: Uuid) -> Arc<tokio::sync::Mutex<()>> {
        self.gates
            .lock()
            .unwrap()
            .entry(thread_id)
            .or_default()
            .clone()
    }

    pub async fn hold(&self, thread_id: Uuid) -> TurnStartHold<'_> {
        let guard = self.entry(thread_id).lock_owned().await;
        TurnStartHold {
            gate: self,
            thread_id,
            guard: Some(guard),
        }
    }

    pub fn try_hold(&self, thread_id: Uuid) -> Option<TurnStartHold<'_>> {
        let hold = TurnStartHold {
            gate: self,
            thread_id,
            guard: self.entry(thread_id).try_lock_owned().ok(),
        };
        hold.guard.is_some().then_some(hold)
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn tracked(&self) -> usize {
        self.gates.lock().unwrap().len()
    }
}

impl Drop for TurnStartHold<'_> {
    fn drop(&mut self) {
        let mut gates = self.gate.gates.lock().unwrap();
        drop(self.guard.take());
        // INVARIANT: Only the map's own reference is left, so nobody holds or waits.
        // Waiters clone the entry under this map lock, so none appears now.
        if gates
            .get(&self.thread_id)
            .is_some_and(|gate| Arc::strong_count(gate) == 1)
        {
            gates.remove(&self.thread_id);
        }
    }
}
