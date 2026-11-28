use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use tokio::sync::broadcast;

use super::EmittedEvent;

#[derive(Clone, Default)]
pub(super) struct AppendTickets(Arc<Mutex<Register>>);

#[derive(Default)]
struct Register {
    next_id: u64,
    in_flight: HashMap<u64, Arc<Mutex<TicketRows>>>,
    learned: Option<(i64, i64)>,
}

#[derive(Default)]
struct TicketRows {
    sent: Vec<i64>,
    ended: bool,
}

impl AppendTickets {
    fn register(&self) -> MutexGuard<'_, Register> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

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

    pub(super) fn lowest_learned(&self) -> Option<i64> {
        self.register().learned.map(|(lowest, _)| lowest)
    }

    /// INVARIANT: Subscribe, and say what the new receiver owes, in one step. Sends
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

pub(super) struct AppendTicket {
    tickets: AppendTickets,
    id: u64,
    rows: Arc<Mutex<TicketRows>>,
}

impl AppendTicket {
    /// INVARIANT: Record a sequence this append drew. Call it before the commit, so a
    /// committed row is always learned.
    pub(super) fn learned(&self, seq: i64) {
        let mut register = self.tickets.register();
        register.learned = Some(match register.learned {
            Some((lowest, highest)) => (lowest.min(seq), highest.max(seq)),
            None => (seq, seq),
        });
    }

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

pub(super) struct ReplayStart {
    pub(super) floor: Option<i64>,
    pub(super) in_flight: Vec<InFlightAppend>,
}

pub(super) struct InFlightAppend {
    rows: Arc<Mutex<TicketRows>>,
    sent_before: usize,
}

impl InFlightAppend {
    pub(super) fn sent_since(&self) -> Vec<i64> {
        lock_rows(&self.rows).sent[self.sent_before..].to_vec()
    }

    pub(super) fn ended(&self) -> bool {
        lock_rows(&self.rows).ended
    }
}

#[cfg(test)]
#[path = "append_ticket_tests.rs"]
mod tests;
