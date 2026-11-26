use std::collections::{BTreeMap, BTreeSet};
use std::ops::RangeInclusive;
use std::sync::{Arc, Mutex};

use chrono::{DateTime, Utc};
use sqlx::PgPool;
use tokio::sync::broadcast::{self, error::RecvError};
use uuid::Uuid;

use super::append_ticket::{AppendTickets, InFlightAppend, ReplayStart};
use super::{committed_event_horizon, BusEvent, EmittedEvent, EventBus, SystemEvent};
use crate::engine::thread_events::{EventMeta, ThreadEvent};

#[derive(Clone, Default)]
pub(super) struct UnbroadcastRows(Arc<Mutex<BTreeSet<i64>>>);

impl UnbroadcastRows {
    pub(super) fn record(&self, seq: i64) {
        if let Ok(mut rows) = self.0.lock() {
            rows.insert(seq);
        }
    }

    fn within(&self, after: i64, through: i64) -> Vec<i64> {
        // INVARIANT: A poisoned lock yields none, so those rows replay: an extra offer,
        // never a lost one.
        self.0
            .lock()
            .map(|rows| rows.range(after + 1..=through).copied().collect())
            .unwrap_or_default()
    }
}

#[derive(Clone, Copy, Debug)]
pub enum ReplayScope {
    Workspace,
    Thread(Uuid),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplayWindow {
    pub after: i64,
    pub through: i64,
    skip: Vec<RangeInclusive<i64>>,
    floor: i64,
    owed: Vec<i64>,
}

impl ReplayWindow {
    pub fn includes(&self, seq: i64) -> bool {
        seq > self.after && seq <= self.through && !self.skip.iter().any(|r| r.contains(&seq))
    }

    pub fn up_to(&self, through: i64) -> Self {
        Self {
            through: through.min(self.through),
            ..self.clone()
        }
    }
}

/// INVARIANT: One subscriber's replay position, opened with [`LagReplay::subscribe`].
///
/// **[`Self::read`] hands on only the event types it is given.** `None` means
/// every type. The position itself records every live frame, of any type.
pub struct LagReplay {
    scope: ReplayScope,
    restore_floor: i64,
    event_types: Option<Vec<&'static str>>,
    unbroadcast: UnbroadcastRows,
    tickets: AppendTickets,
    floor: Option<i64>,
    opened_at: Option<i64>,
    in_flight: Vec<InFlightAppend>,
    seen_below: BTreeSet<i64>,
    received: BTreeMap<i64, i64>,
    replayed: Option<RangeInclusive<i64>>,
}

impl LagReplay {
    pub fn subscribe(
        bus: &EventBus,
        scope: ReplayScope,
        event_types: Option<Vec<&'static str>>,
    ) -> (broadcast::Receiver<EmittedEvent>, Self) {
        let (receiver, start) = bus.append_tickets.subscribe(&bus.event_tx);
        let replay = Self::opened(
            scope,
            event_types,
            bus.unbroadcast.clone(),
            bus.append_tickets.clone(),
            start,
        );
        (receiver, replay)
    }

    fn opened(
        scope: ReplayScope,
        event_types: Option<Vec<&'static str>>,
        unbroadcast: UnbroadcastRows,
        tickets: AppendTickets,
        start: ReplayStart,
    ) -> Self {
        Self {
            scope,
            restore_floor: 0,
            event_types,
            unbroadcast,
            tickets,
            floor: start.floor,
            opened_at: start.floor,
            in_flight: start.in_flight,
            seen_below: BTreeSet::new(),
            received: BTreeMap::new(),
            replayed: None,
        }
    }

    pub async fn read(
        &mut self,
        received: Result<EmittedEvent, RecvError>,
        pool: &PgPool,
    ) -> Option<Vec<EmittedEvent>> {
        match received {
            Ok(frame) if self.live(&frame) => Some(vec![frame]),
            Ok(_) => Some(Vec::new()),
            Err(RecvError::Lagged(n)) => {
                log_legacy!(
                    "[LagReplay] Lagged by {} frames, replaying from the events table",
                    n
                );
                Some(self.missed(pool).await)
            }
            Err(RecvError::Closed) => None,
        }
    }

    pub fn live(&mut self, frame: &EmittedEvent) -> bool {
        let first_sight = frame.seq.is_none_or(|seq| self.first_sight(seq));
        first_sight && self.hands_on(frame)
    }

    fn hands_on(&self, frame: &EmittedEvent) -> bool {
        let in_scope = match (self.scope, &frame.typed) {
            (ReplayScope::Workspace, _) => true,
            (ReplayScope::Thread(mine), BusEvent::Thread { thread_id, .. }) => mine == *thread_id,
            (ReplayScope::Thread(_), BusEvent::System(_)) => false,
        };
        let type_name = match &frame.typed {
            BusEvent::Thread { event, .. } => event.event_type(),
            BusEvent::System(event) => event.stored_event_type(),
        };
        in_scope
            && self
                .event_types
                .as_ref()
                .is_none_or(|types| types.contains(&type_name))
    }

    fn first_sight(&mut self, seq: i64) -> bool {
        if self.is_covered(Some(seq)) {
            return false;
        }
        self.received(Some(seq));
        true
    }

    fn received(&mut self, seq: Option<i64>) {
        let Some(seq) = seq else {
            return;
        };
        if self.opened_at.is_some_and(|opened_at| seq <= opened_at) {
            self.seen_below.insert(seq);
            return;
        }
        if self.floor.is_some_and(|floor| seq <= floor) {
            return;
        }
        let mut start = seq;
        let mut end = seq;
        if let Some((&prev_start, &prev_end)) = self.received.range(..=seq).next_back() {
            if prev_end >= seq {
                return;
            }
            if prev_end + 1 == seq {
                start = prev_start;
            }
        }
        if let Some(next_end) = self.received.remove(&(seq + 1)) {
            end = next_end;
        }
        self.received.insert(start, end);
        self.advance_floor();
    }

    fn advance_floor(&mut self) {
        let Some(mut floor) = self.floor else {
            return;
        };
        while let Some(entry) = self.received.first_entry() {
            if *entry.key() > floor + 1 {
                break;
            }
            floor = floor.max(entry.remove());
        }
        self.floor = Some(floor);
    }

    pub async fn window(&mut self, pool: &PgPool) -> Option<ReplayWindow> {
        // INVARIANT: Refuse the entire window on an unknown restore boundary. No position
        // advances, so a later successful read can recover new work.
        self.restore_floor = match crate::core::snapshot::boundary::read(pool).await {
            Ok(boundary) => boundary.map_or(0, |b| b.event_sequence),
            Err(error) => {
                log_legacy!(
                    "[LagReplay] Restore boundary unavailable; refusing replay: {}",
                    error
                );
                return None;
            }
        };
        let top = match horizon(pool, self.scope).await {
            Ok(top) => top,
            Err(e) => {
                log_legacy!(
                    "[LagReplay] Could not read the horizon, so this lag is replayed \
                     with the next one: {}",
                    e
                );
                return None;
            }
        };
        let floor = match self.floor {
            Some(floor) => floor,
            None => {
                let committed = match self.scope {
                    ReplayScope::Workspace => top,
                    ReplayScope::Thread(_) => match committed_event_horizon(pool).await {
                        Ok(committed) => committed,
                        Err(e) => {
                            log_legacy!(
                                "[LagReplay] Could not read the committed horizon, so this \
                                 lag is replayed with the next one: {}",
                                e
                            );
                            return None;
                        }
                    },
                };
                self.first_floor(committed)?
            }
        };
        let floor = floor.max(self.restore_floor);
        let owed: Vec<i64> = self
            .owed()
            .into_iter()
            .filter(|seq| *seq > self.restore_floor)
            .collect();
        if top <= floor && owed.is_empty() {
            return None;
        }
        let through = top.max(floor);
        let after = owed.first().map_or(floor, |lowest| lowest - 1);
        let mut skip: Vec<RangeInclusive<i64>> = self
            .received
            .iter()
            .map(|(&start, &end)| start..=end)
            .collect();
        skip.extend(
            self.unbroadcast
                .within(floor, through)
                .into_iter()
                .map(|seq| seq..=seq),
        );
        skip.extend(gaps_between(after, &owed, floor));
        Some(ReplayWindow {
            after,
            through,
            skip,
            floor,
            owed,
        })
    }

    /// INVARIANT: Set the floor of a subscription that opened before the bus learned
    /// any sequence. `committed` must be a committed horizon: every row the
    /// bus appended at or below it has committed, so it was learned. A row
    /// above it may still be unlearned and lower than any learned one, so the
    /// floor never rises above `committed`. `None` while nothing is learned,
    /// since then nothing was sent.
    fn first_floor(&mut self, committed: i64) -> Option<i64> {
        let lowest = self.tickets.lowest_learned()?;
        self.floor = Some(committed.min(lowest - 1));
        self.advance_floor();
        self.floor
    }

    fn owed(&self) -> Vec<i64> {
        let Some(opened_at) = self.opened_at else {
            return Vec::new();
        };
        let owed: BTreeSet<i64> = self
            .in_flight
            .iter()
            .flat_map(InFlightAppend::sent_since)
            .filter(|seq| *seq <= opened_at && !self.seen_below.contains(seq))
            .collect();
        owed.into_iter().collect()
    }

    pub fn covered(&mut self, window: &ReplayWindow) {
        let through = window.through;
        self.seen_below
            .extend(window.owed.iter().filter(|seq| **seq <= through));
        self.drop_settled_appends();
        if through <= window.floor {
            return;
        }
        self.floor = Some(self.floor.map_or(through, |floor| floor.max(through)));
        let first = self
            .replayed
            .as_ref()
            .map_or(window.floor + 1, |replayed| *replayed.start());
        self.replayed = Some(first..=through);
        let straddling = self
            .received
            .range(..=through)
            .next_back()
            .filter(|(_, &end)| end > through)
            .map(|(_, &end)| end);
        self.received.retain(|start, _| *start > through);
        if let Some(end) = straddling {
            self.received.insert(through + 1, end);
        }
        self.advance_floor();
    }

    fn drop_settled_appends(&mut self) {
        let Some(opened_at) = self.opened_at else {
            return;
        };
        let seen_below = &self.seen_below;
        self.in_flight.retain(|append| {
            !append.ended()
                || append
                    .sent_since()
                    .iter()
                    .any(|seq| *seq <= opened_at && !seen_below.contains(seq))
        });
    }

    fn is_covered(&self, seq: Option<i64>) -> bool {
        seq.is_some_and(|seq| {
            seq <= self.restore_floor
                || self.seen_below.contains(&seq)
                || self
                    .replayed
                    .as_ref()
                    .is_some_and(|replayed| replayed.contains(&seq))
        })
    }

    /// INVARIANT: The rows a lag dropped that this subscriber has not seen, oldest first.
    /// A failed read returns none and keeps the floor, so the next replay
    /// covers this window too.
    pub async fn missed(&mut self, pool: &PgPool) -> Vec<EmittedEvent> {
        let Some(window) = self.window(pool).await else {
            return Vec::new();
        };
        let rows = match self.rows_in(pool, &window).await {
            Ok(rows) => rows,
            Err(e) => {
                log_legacy!(
                    "[LagReplay] Could not read rows {}..={}: {}",
                    window.after + 1,
                    window.through,
                    e
                );
                return Vec::new();
            }
        };
        self.covered(&window);
        rows.into_iter()
            .filter(|row| window.includes(row.sequence))
            .filter_map(ReplayRow::into_frame)
            .collect()
    }

    async fn rows_in(
        &self,
        pool: &PgPool,
        window: &ReplayWindow,
    ) -> Result<Vec<ReplayRow>, sqlx::Error> {
        let thread_id = match self.scope {
            ReplayScope::Workspace => None,
            ReplayScope::Thread(thread_id) => Some(thread_id),
        };
        sqlx::query_as(
            "SELECT id, sequence, event_type, payload, created, thread_id, \
                    trigger_chain_depth, emitting_trigger_id \
             FROM events \
             WHERE sequence > $1 AND sequence <= $2 \
               AND ($3::text[] IS NULL OR event_type = ANY($3)) \
               AND ($4::uuid IS NULL OR thread_id = $4) \
             ORDER BY sequence",
        )
        .bind(window.after)
        .bind(window.through)
        .bind(self.event_types.as_deref())
        .bind(thread_id)
        .fetch_all(pool)
        .await
    }
}

fn gaps_between(after: i64, owed: &[i64], floor: i64) -> Vec<RangeInclusive<i64>> {
    let mut gaps = Vec::new();
    let mut previous = after;
    for &seq in owed.iter().chain(std::iter::once(&(floor + 1))) {
        if seq > previous + 1 {
            gaps.push(previous + 1..=seq - 1);
        }
        previous = seq;
    }
    gaps
}

async fn horizon(pool: &PgPool, scope: ReplayScope) -> Result<i64, sqlx::Error> {
    match scope {
        ReplayScope::Workspace => committed_event_horizon(pool).await,
        ReplayScope::Thread(thread_id) => {
            sqlx::query_scalar("SELECT COALESCE(MAX(sequence), 0) FROM events WHERE thread_id = $1")
                .bind(thread_id)
                .fetch_one(pool)
                .await
        }
    }
}

#[derive(sqlx::FromRow)]
struct ReplayRow {
    id: Uuid,
    sequence: i64,
    event_type: String,
    payload: serde_json::Value,
    created: DateTime<Utc>,
    thread_id: Option<Uuid>,
    trigger_chain_depth: Option<i32>,
    emitting_trigger_id: Option<String>,
}

impl ReplayRow {
    fn into_frame(self) -> Option<EmittedEvent> {
        let typed = match self.thread_id {
            Some(thread_id) => ThreadEvent::from_stored(&self.event_type, self.payload.clone())
                .map(|event| BusEvent::Thread {
                    thread_id,
                    event,
                    meta: EventMeta::from_payload(&self.payload),
                }),
            None => SystemEvent::from_stored(self.payload).map(BusEvent::System),
        };
        match typed {
            Ok(typed) => Some(EmittedEvent {
                event_id: self.id,
                seq: Some(self.sequence),
                created: self.created,
                typed,
                aggregate: None,
                depth: self
                    .trigger_chain_depth
                    .and_then(|depth| u32::try_from(depth).ok())
                    .unwrap_or(0),
                emitting_trigger_id: self.emitting_trigger_id,
            }),
            Err(e) => {
                log_legacy!(
                    "[LagReplay] Skipping row {} ({}), it does not read back: {}",
                    self.sequence,
                    self.event_type,
                    e
                );
                None
            }
        }
    }
}

#[cfg(test)]
#[path = "lag_replay_tests.rs"]
mod tests;
