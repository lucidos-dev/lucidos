//! Coding-agent follow-ups keep the order they were sent in.
//!
//! `POST /api/v1/chat/stream` acks a coding-agent follow-up before recording
//! it, and each spawned turn then waits for the session on its own timer. Two
//! follow-ups sent while a session starts could therefore be recorded and
//! delivered in either order. So the handler joins a per-thread chain before
//! its ack, in arrival order. Each turn waits for the one before it and
//! releases the next once its message is recorded and routed.
//! See ADR 0281.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};

use tokio::sync::oneshot;
use uuid::Uuid;

/// Every thread's chain of follow-ups still in flight. A thread with none
/// holds no entry.
#[derive(Clone, Default)]
pub(crate) struct FollowUpOrder {
    chains: Arc<Mutex<Chains>>,
}

#[derive(Default)]
struct Chains {
    next_turn: u64,
    /// The newest turn of each thread. A new turn waits on its `done`.
    tails: HashMap<Uuid, Tail>,
}

struct Tail {
    turn: u64,
    done: oneshot::Receiver<()>,
}

impl FollowUpOrder {
    /// Take the next place in `thread_id`'s chain. Never waits, so the handler
    /// can call it before its ack.
    pub(crate) fn join(&self, thread_id: Uuid) -> FollowUpTurn {
        let (done_tx, done_rx) = oneshot::channel();
        let mut chains = lock(&self.chains);
        let turn = chains.next_turn;
        chains.next_turn += 1;
        let predecessor = chains
            .tails
            .insert(
                thread_id,
                Tail {
                    turn,
                    done: done_rx,
                },
            )
            .map(|tail| tail.done);
        FollowUpTurn {
            chains: self.chains.clone(),
            thread_id,
            turn,
            predecessor,
            done: Some(done_tx),
        }
    }
}

/// One follow-up's place in its thread's chain. Dropping it releases the next
/// follow-up, but never before this one's own predecessor is done.
pub(crate) struct FollowUpTurn {
    chains: Arc<Mutex<Chains>>,
    thread_id: Uuid,
    turn: u64,
    predecessor: Option<oneshot::Receiver<()>>,
    /// Dropped to release the successor. Nothing is ever sent on it.
    done: Option<oneshot::Sender<()>>,
}

impl FollowUpTurn {
    /// Wait until every follow-up sent before this one is released.
    /// Cancellation-safe: a cancelled wait keeps the predecessor to wait on.
    pub(crate) async fn wait_for_predecessor(&mut self) {
        if let Some(predecessor) = self.predecessor.as_mut() {
            // A closed channel is the release: the sender is only ever dropped.
            let _ = predecessor.await;
            self.predecessor = None;
        }
    }
}

impl Drop for FollowUpTurn {
    fn drop(&mut self) {
        let release = Release {
            chains: self.chains.clone(),
            thread_id: self.thread_id,
            turn: self.turn,
            done: self.done.take(),
        };
        // A turn dropped before its predecessor finished must not let its
        // successor overtake that predecessor, so it hands the wait on.
        match (
            self.predecessor.take(),
            tokio::runtime::Handle::try_current(),
        ) {
            (Some(predecessor), Ok(runtime)) => {
                runtime.spawn(async move {
                    let _ = predecessor.await;
                    release.run();
                });
            }
            _ => release.run(),
        }
    }
}

struct Release {
    chains: Arc<Mutex<Chains>>,
    thread_id: Uuid,
    turn: u64,
    done: Option<oneshot::Sender<()>>,
}

impl Release {
    fn run(self) {
        let mut chains = lock(&self.chains);
        if chains
            .tails
            .get(&self.thread_id)
            .is_some_and(|tail| tail.turn == self.turn)
        {
            chains.tails.remove(&self.thread_id);
        }
        drop(chains);
        drop(self.done);
    }
}

/// The map holds no invariant a panicking holder could break halfway.
fn lock(chains: &Mutex<Chains>) -> MutexGuard<'_, Chains> {
    chains
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
#[path = "follow_up_order_tests.rs"]
mod tests;
