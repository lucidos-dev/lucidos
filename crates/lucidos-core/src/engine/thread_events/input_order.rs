use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};

use tokio::sync::oneshot;
use uuid::Uuid;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InputPosition {
    pub(crate) chain: Uuid,
    pub(crate) seq: i64,
}

impl InputPosition {
    pub fn from_wire(
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ChainTail {
    pub(crate) seq: i64,
    pub(crate) event_id: Uuid,
}

pub(crate) fn admits(tail: Option<ChainTail>, seq: i64, event_id: Uuid) -> bool {
    match tail {
        None => true,
        Some(tail) => seq > tail.seq || (seq == tail.seq && event_id == tail.event_id),
    }
}

pub const OVERTAKEN_REASON: &str = "overtaken";

pub fn overtaken_message(thread_id: Uuid) -> String {
    format!(
        "A later input from this page reached thread {thread_id} first, so this one was not \
         recorded. Send it again to add it after."
    )
}

pub enum InputAdmission {
    Admitted(InputTurn),
    /// INVARIANT: A later input from the same chain was admitted first. This one must
    /// never be recorded.
    Overtaken,
}

#[derive(Clone, Default)]
pub struct InputOrder {
    threads: Arc<Mutex<Threads>>,
    pub successor_released: Arc<tokio::sync::Notify>,
}

#[derive(Default)]
struct Threads {
    next_turn: u64,
    tails: HashMap<Uuid, ThreadTail>,
}

struct ThreadTail {
    turn: u64,
    input: u64,
    done: oneshot::Receiver<()>,
}

impl InputOrder {
    pub async fn admit(
        &self,
        pool: &sqlx::PgPool,
        thread_id: Uuid,
        position: Option<InputPosition>,
        event_id: Option<Uuid>,
    ) -> Result<InputAdmission, sqlx::Error> {
        if position.is_none() || event_id.is_none() {
            return Ok(InputAdmission::Admitted(self.join(thread_id)));
        }
        let mut tx = pool.begin().await?;
        let admission = self
            .admit_in_transaction(&mut tx, thread_id, position, event_id)
            .await?;
        tx.commit().await?;
        Ok(admission)
    }

    /// INVARIANT: Join admission to the caller's durable acceptance transaction. The
    /// caller retains the returned turn through commit, or drops it on error.
    /// Acquire any append-horizon lock before calling this method: it takes
    /// the thread append lock even for an input without a page position.
    pub(crate) async fn admit_in_transaction(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        thread_id: Uuid,
        position: Option<InputPosition>,
        event_id: Option<Uuid>,
    ) -> Result<InputAdmission, sqlx::Error> {
        crate::engine::event_bus::EventBus::lock_thread_append(tx, thread_id).await?;
        let (Some(position), Some(event_id)) = (position, event_id) else {
            return Ok(InputAdmission::Admitted(self.join(thread_id)));
        };
        let tail: Option<(i64, Uuid)> = sqlx::query_as(
            "SELECT last_seq, last_event_id FROM thread_input_chains \
             WHERE thread_id = $1 AND input_chain = $2",
        )
        .bind(thread_id)
        .bind(position.chain)
        .fetch_optional(&mut **tx)
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
        .execute(&mut **tx)
        .await?;
        Ok(InputAdmission::Admitted(self.join(thread_id)))
    }

    /// INVARIANT: Has any page admitted an input to `thread_id`? A thread whose first
    /// input is admitted but not yet recorded has no row of its own yet.
    pub async fn thread_has_admitted_input(
        pool: &sqlx::PgPool,
        thread_id: Uuid,
    ) -> Result<bool, sqlx::Error> {
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM thread_input_chains WHERE thread_id = $1)")
            .bind(thread_id)
            .fetch_one(pool)
            .await
    }

    pub fn join(&self, thread_id: Uuid) -> InputTurn {
        self.join_input(thread_id, None)
    }

    pub(crate) fn join_recording_barrier(&self, routing: &InputTurn) -> InputTurn {
        self.join_input(routing.thread_id, Some(routing.input))
    }

    fn join_input(&self, thread_id: Uuid, input: Option<u64>) -> InputTurn {
        let (done_tx, done_rx) = oneshot::channel();
        let mut threads = lock(&self.threads);
        let turn = threads.next_turn;
        threads.next_turn += 1;
        let input = input.unwrap_or(turn);
        let predecessor = threads
            .tails
            .insert(
                thread_id,
                ThreadTail {
                    turn,
                    input,
                    done: done_rx,
                },
            )
            .map(|tail| tail.done);
        InputTurn {
            threads: self.threads.clone(),
            successor_released: self.successor_released.clone(),
            thread_id,
            turn,
            input,
            predecessor,
            done: Some(done_tx),
        }
    }
}

pub struct InputTurn {
    threads: Arc<Mutex<Threads>>,
    successor_released: Arc<tokio::sync::Notify>,
    thread_id: Uuid,
    turn: u64,
    input: u64,
    predecessor: Option<oneshot::Receiver<()>>,
    done: Option<oneshot::Sender<()>>,
}

impl InputTurn {
    /// INVARIANT: Wait until every input admitted before this one is released.
    /// Cancellation-safe: a cancelled wait keeps the predecessor to wait on.
    pub async fn wait_for_predecessor(&mut self) {
        if let Some(predecessor) = self.predecessor.as_mut() {
            let _ = predecessor.await;
            self.predecessor = None;
        }
    }

    pub fn has_input_ahead(&mut self) -> bool {
        let Some(predecessor) = self.predecessor.as_mut() else {
            return false;
        };
        match predecessor.try_recv() {
            Err(oneshot::error::TryRecvError::Empty) => true,
            _ => {
                self.predecessor = None;
                false
            }
        }
    }
}

impl Drop for InputTurn {
    fn drop(&mut self) {
        self.has_input_ahead();
        let release = Release {
            threads: self.threads.clone(),
            successor_released: self.successor_released.clone(),
            thread_id: self.thread_id,
            turn: self.turn,
            input: self.input,
            done: self.done.take(),
        };
        // INVARIANT: A turn dropped before its predecessor finished must not let its
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
    successor_released: Arc<tokio::sync::Notify>,
    thread_id: Uuid,
    turn: u64,
    input: u64,
    done: Option<oneshot::Sender<()>>,
}

impl Release {
    fn run(self) {
        let mut threads = lock(&self.threads);
        let tail = threads.tails.get(&self.thread_id);
        let has_successor_input = tail.is_some_and(|tail| tail.input != self.input);
        if tail.is_some_and(|tail| tail.turn == self.turn) {
            threads.tails.remove(&self.thread_id);
        }
        drop(threads);
        drop(self.done);
        // INVARIANT: Close the predecessor channel before the queue checks it. With no
        // successor there is no newly admissible input to drain.
        if has_successor_input {
            self.successor_released.notify_one();
        }
    }
}

fn lock(threads: &Mutex<Threads>) -> MutexGuard<'_, Threads> {
    threads
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
#[path = "input_order_tests.rs"]
mod tests;
