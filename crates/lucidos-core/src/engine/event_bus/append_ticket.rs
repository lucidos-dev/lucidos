//! *Append tickets*: the bus's in-memory record of each persisted append, from
//! [`super::EventBus::begin_append`] until the append ends. A lag replay opens its
//! position from them, so it owes exactly the frames sent after it subscribed
//! (ADR 0453).
//!
//! Two facts make that position exact. Every row the bus sent before a
//! subscription is at or below the highest sequence an append had learned by
//! then. And an append that begins later draws a higher sequence, because
//! draws are monotonic. So only the appends in flight at that moment can still
//! send a row at or below that mark.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use tokio::sync::broadcast;

use super::EmittedEvent;

/// Every append in flight, and the sequences appends have learned. Shared by
/// every clone of the bus.
#[derive(Clone, Default)]
pub(super) struct AppendTickets(Arc<Mutex<Register>>);

#[derive(Default)]
struct Register {
    next_id: u64,
    in_flight: HashMap<u64, Arc<Mutex<TicketRows>>>,
    learned: Option<(i64, i64)>,
}

/// What one append has sent, and whether it has ended.
#[derive(Default)]
struct TicketRows {
    sent: Vec<i64>,
    ended: bool,
}

impl AppendTickets {
    /// The register's lock. It guards no invariant a panic could half-write,
    /// so a poisoned lock is still read.
    fn register(&self) -> MutexGuard<'_, Register> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Open a ticket for an append about to draw its sequences.
    pub(super) fn open(&self) -> AppendTicket {
        let rows = Arc::new(Mutex::new(TicketRows::default()));
        let mut register = self.register();
        let id = register.next_id;
        register.next_id += 1;
        register.in_flight.insert(id, rows.clone());
        AppendTicket {
            tickets: self.clone(),
            id,
            rows,
        }
    }

    /// The lowest sequence any append has learned. `None` before the first.
    pub(super) fn lowest_learned(&self) -> Option<i64> {
        self.register().learned.map(|(lowest, _)| lowest)
    }

    /// Subscribe, and say what the new receiver owes, in one step. Sends
    /// take the same lock, so no frame falls between the two.
    pub(super) fn subscribe(
        &self,
        event_tx: &broadcast::Sender<EmittedEvent>,
    ) -> (broadcast::Receiver<EmittedEvent>, ReplayStart) {
        let register = self.register();
        let receiver = event_tx.subscribe();
        let start = match register.learned {
            Some((_, highest)) => ReplayStart {
                floor: Some(highest),
                in_flight: register
                    .in_flight
                    .values()
                    .map(|rows| InFlightAppend {
                        sent_before: lock_rows(rows).sent.len(),
                        rows: rows.clone(),
                    })
                    .collect(),
            },
            // Nothing learned, so nothing sent: every row is owed.
            None => ReplayStart {
                floor: None,
                in_flight: Vec::new(),
            },
        };
        (receiver, start)
    }
}

fn lock_rows(rows: &Mutex<TicketRows>) -> MutexGuard<'_, TicketRows> {
    rows.lock().unwrap_or_else(PoisonError::into_inner)
}

/// One append's ticket. Dropping it ends the append, sent or not.
pub(super) struct AppendTicket {
    tickets: AppendTickets,
    id: u64,
    rows: Arc<Mutex<TicketRows>>,
}

impl AppendTicket {
    /// Record a sequence this append drew. Call it before the commit, so a
    /// committed row is always learned.
    pub(super) fn learned(&self, seq: i64) {
        let mut register = self.tickets.register();
        register.learned = Some(match register.learned {
            Some((lowest, highest)) => (lowest.min(seq), highest.max(seq)),
            None => (seq, seq),
        });
    }

    /// Broadcast one of this append's rows, recording the send under the
    /// register's lock.
    pub(super) fn send(&self, event_tx: &broadcast::Sender<EmittedEvent>, frame: EmittedEvent) {
        let seq = frame.seq;
        let _register = self.tickets.register();
        let _ = event_tx.send(frame);
        if let Some(seq) = seq {
            lock_rows(&self.rows).sent.push(seq);
        }
    }
}

impl Drop for AppendTicket {
    fn drop(&mut self) {
        let mut register = self.tickets.register();
        register.in_flight.remove(&self.id);
        lock_rows(&self.rows).ended = true;
    }
}

/// Where a new subscriber's replay position starts.
pub(super) struct ReplayStart {
    /// Every row the bus sent before the subscription is at or below this.
    /// `None` when the bus had learned no sequence, so it had sent nothing.
    pub(super) floor: Option<i64>,
    /// The appends in flight at the subscription. Only these can still send a
    /// row at or below the floor.
    pub(super) in_flight: Vec<InFlightAppend>,
}

/// An append that was in flight when a subscription opened.
pub(super) struct InFlightAppend {
    rows: Arc<Mutex<TicketRows>>,
    sent_before: usize,
}

impl InFlightAppend {
    /// The rows this append has sent since the subscription opened.
    pub(super) fn sent_since(&self) -> Vec<i64> {
        lock_rows(&self.rows).sent[self.sent_before..].to_vec()
    }

    /// Whether the append has ended, so it sends nothing more.
    pub(super) fn ended(&self) -> bool {
        lock_rows(&self.rows).ended
    }
}

#[cfg(test)]
#[path = "append_ticket_tests.rs"]
mod tests;
