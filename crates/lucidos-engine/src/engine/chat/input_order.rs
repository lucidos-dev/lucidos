//! A thread's human inputs record in the order the engine admitted them.
//!
//! `POST /api/v1/chat/stream` and `POST /api/v1/threads/:id/answer-question`
//! admit an input before they ack. *Admission* checks the page's *input
//! sequence* under the thread append lock and joins the thread's *input order*
//! in the same step. A message then waits for the input admitted before it.
//! It releases the next once it is recorded, and a coding-agent message once
//! it is routed too. A card answer records at once, since the turn ahead may be
//! blocked on that very question. ADR 0449 widened ADR 0281's coding-agent
//! follow-up chain to every admitted input.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};

use tokio::sync::oneshot;
use uuid::Uuid;

/// Where a page puts one input in its run of inputs to a thread: the page's
/// *input chain* and the input's *input sequence*.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct InputPosition {
    pub(crate) chain: Uuid,
    pub(crate) seq: i64,
}

impl InputPosition {
    /// Read the two wire fields. They come together or not at all, and a
    /// position names the input it places, so it needs the event id too.
    pub(crate) fn from_wire(
        chain: Option<&str>,
        seq: Option<i64>,
        event_id: Option<Uuid>,
    ) -> Result<Option<Self>, String> {
        match (chain, seq) {
            (None, None) => Ok(None),
            (Some(chain), Some(seq)) => {
                let chain = Uuid::parse_str(chain)
                    .map_err(|_| format!("input_chain '{chain}' is not a uuid"))?;
                if seq < 1 {
                    return Err(format!("input_seq must be at least 1, got {seq}"));
                }
                if event_id.is_none() {
                    return Err("an input_seq needs the event_id of the input it places".into());
                }
                Ok(Some(Self { chain, seq }))
            }
            _ => Err("input_chain and input_seq come together, or not at all".into()),
        }
    }
}

/// The last input a chain admitted to a thread.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ChainTail {
    pub(crate) seq: i64,
    pub(crate) event_id: Uuid,
}

/// May an input at `seq`, named `event_id`, follow `tail`? Past it, yes. At
/// it, only as a repeat of the tail itself: a quiet retry, or a retry after a
/// restart lost the first attempt.
pub(crate) fn admits(tail: Option<ChainTail>, seq: i64, event_id: Uuid) -> bool {
    match tail {
        None => true,
        Some(tail) => seq > tail.seq || (seq == tail.seq && event_id == tail.event_id),
    }
}

/// The refusal slug an overtaken input answers with.
pub(crate) const OVERTAKEN_REASON: &str = "overtaken";

/// Why an overtaken input was refused. The client already shows it as not
/// sent, so this only has to say why it stays that way.
pub(crate) fn overtaken_message(thread_id: Uuid) -> String {
    format!(
        "A later input from this page reached thread {thread_id} first, so this one was not \
         recorded. Send it again to add it after."
    )
}

/// What admission decided.
pub(crate) enum InputAdmission {
    /// The input holds its place. Dropping the turn releases the next input.
    Admitted(InputTurn),
    /// A later input from the same chain was admitted first. This one must
    /// never be recorded.
    Overtaken,
}

/// Every thread's order of admitted inputs still in flight. A thread with
/// none holds no entry.
#[derive(Clone, Default)]
pub(crate) struct InputOrder {
    threads: Arc<Mutex<Threads>>,
}

#[derive(Default)]
struct Threads {
    next_turn: u64,
    /// The newest turn of each thread. A new turn waits on its `done`.
    tails: HashMap<Uuid, ThreadTail>,
}

struct ThreadTail {
    turn: u64,
    done: oneshot::Receiver<()>,
}

impl InputOrder {
    /// Admit one input to `thread_id`, and take its place in the order.
    ///
    /// A positioned input is checked against its chain's tail and advances it,
    /// all under the thread append lock (ADR 0360). The join happens before
    /// the commit, so two admissions join in the order the lock let them check.
    /// An input with no position (agent mode, the CLI, another workspace, a
    /// stale client) is never refused and writes no row.
    pub(crate) async fn admit(
        &self,
        pool: &sqlx::PgPool,
        thread_id: Uuid,
        position: Option<InputPosition>,
        event_id: Option<Uuid>,
    ) -> Result<InputAdmission, sqlx::Error> {
        let (Some(position), Some(event_id)) = (position, event_id) else {
            return Ok(InputAdmission::Admitted(self.join(thread_id)));
        };
        let mut tx = pool.begin().await?;
        crate::engine::event_bus::EventBus::lock_thread_append(&mut tx, thread_id).await?;
        let tail: Option<(i64, Uuid)> = sqlx::query_as(
            "SELECT last_seq, last_event_id FROM thread_input_chains \
             WHERE thread_id = $1 AND input_chain = $2",
        )
        .bind(thread_id)
        .bind(position.chain)
        .fetch_optional(&mut *tx)
        .await?;
        let tail = tail.map(|(seq, event_id)| ChainTail { seq, event_id });
        if !admits(tail, position.seq, event_id) {
            return Ok(InputAdmission::Overtaken);
        }
        sqlx::query(
            "INSERT INTO thread_input_chains (thread_id, input_chain, last_seq, last_event_id) \
             VALUES ($1, $2, $3, $4) \
             ON CONFLICT (thread_id, input_chain) \
             DO UPDATE SET last_seq = EXCLUDED.last_seq, last_event_id = EXCLUDED.last_event_id",
        )
        .bind(thread_id)
        .bind(position.chain)
        .bind(position.seq)
        .bind(event_id)
        .execute(&mut *tx)
        .await?;
        // Dropped on a failed commit, which releases the place it took.
        let turn = self.join(thread_id);
        tx.commit().await?;
        Ok(InputAdmission::Admitted(turn))
    }

    /// Has any page admitted an input to `thread_id`? A thread whose first
    /// input is admitted but not yet recorded has no row of its own yet.
    pub(crate) async fn thread_has_admitted_input(
        pool: &sqlx::PgPool,
        thread_id: Uuid,
    ) -> Result<bool, sqlx::Error> {
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM thread_input_chains WHERE thread_id = $1)")
            .bind(thread_id)
            .fetch_one(pool)
            .await
    }

    /// Take the next place in `thread_id`'s order. Never waits.
    fn join(&self, thread_id: Uuid) -> InputTurn {
        let (done_tx, done_rx) = oneshot::channel();
        let mut threads = lock(&self.threads);
        let turn = threads.next_turn;
        threads.next_turn += 1;
        let predecessor = threads
            .tails
            .insert(
                thread_id,
                ThreadTail {
                    turn,
                    done: done_rx,
                },
            )
            .map(|tail| tail.done);
        InputTurn {
            threads: self.threads.clone(),
            thread_id,
            turn,
            predecessor,
            done: Some(done_tx),
        }
    }
}

/// One input's place in its thread's order. Dropping it releases the next
/// input, but never before this one's own predecessor is done.
pub(crate) struct InputTurn {
    threads: Arc<Mutex<Threads>>,
    thread_id: Uuid,
    turn: u64,
    predecessor: Option<oneshot::Receiver<()>>,
    /// Dropped to release the successor. Nothing is ever sent on it.
    done: Option<oneshot::Sender<()>>,
}

impl InputTurn {
    /// Wait until every input admitted before this one is released.
    /// Cancellation-safe: a cancelled wait keeps the predecessor to wait on.
    pub(crate) async fn wait_for_predecessor(&mut self) {
        if let Some(predecessor) = self.predecessor.as_mut() {
            // A closed channel is the release: the sender is only ever dropped.
            let _ = predecessor.await;
            self.predecessor = None;
        }
    }

    /// Is an input admitted before this one still unreleased? An input that
    /// records before its ack may do so only when none is.
    pub(crate) fn has_input_ahead(&mut self) -> bool {
        let Some(predecessor) = self.predecessor.as_mut() else {
            return false;
        };
        match predecessor.try_recv() {
            Err(oneshot::error::TryRecvError::Empty) => true,
            // Closed is the release; nothing is ever sent.
            _ => {
                self.predecessor = None;
                false
            }
        }
    }
}

impl Drop for InputTurn {
    fn drop(&mut self) {
        let release = Release {
            threads: self.threads.clone(),
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
    threads: Arc<Mutex<Threads>>,
    thread_id: Uuid,
    turn: u64,
    done: Option<oneshot::Sender<()>>,
}

impl Release {
    fn run(self) {
        let mut threads = lock(&self.threads);
        if threads
            .tails
            .get(&self.thread_id)
            .is_some_and(|tail| tail.turn == self.turn)
        {
            threads.tails.remove(&self.thread_id);
        }
        drop(threads);
        drop(self.done);
    }
}

/// The map holds no invariant a panicking holder could break halfway.
fn lock(threads: &Mutex<Threads>) -> MutexGuard<'_, Threads> {
    threads
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
#[path = "input_order_tests.rs"]
mod tests;
