//! *Lag replay*: how a broadcast subscriber that does work per event gets
//! back, from the events table, exactly the persisted events a
//! `RecvError::Lagged` dropped. A subscriber that only mirrors state re-reads
//! that state instead and needs none of this.
//!
//! A lag says frames were dropped, not which. So the subscriber records which
//! sequences reached it live, and on a lag replays only the rows that did not.
//!
//! The position opens with the subscription, from the bus's *append tickets*.
//! It owes exactly the rows the bus sent after that, even one that committed
//! before. Each window's top is a committed horizon, so a row that committed
//! out of sequence order is never skipped (ADR 0364, ADR 0453).
//!
//! Two read shapes share that position. [`LagReplay::read`] hands a subscriber
//! the frames to act on. The scheduler pages through a [`ReplayWindow`] itself.

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

/// Sequences the bus wrote without broadcasting them: a bulk backfill that no
/// subscriber was meant to see arrive. Shared by every clone of the bus.
///
/// Kept for the process's life. Only a one-shot backfill writes here, so the
/// set is as large as that backfill. A restart starts every floor above it.
#[derive(Clone, Default)]
pub(super) struct UnbroadcastRows(Arc<Mutex<BTreeSet<i64>>>);

impl UnbroadcastRows {
    pub(super) fn record(&self, seq: i64) {
        if let Ok(mut rows) = self.0.lock() {
            rows.insert(seq);
        }
    }

    fn within(&self, after: i64, through: i64) -> Vec<i64> {
        // A poisoned lock yields none, so those rows replay: an extra offer,
        // never a lost one.
        self.0
            .lock()
            .map(|rows| rows.range(after + 1..=through).copied().collect())
            .unwrap_or_default()
    }
}

/// Which rows a replay reads.
#[derive(Clone, Copy, Debug)]
pub(crate) enum ReplayScope {
    /// Every row, bounded by the workspace's committed horizon.
    Workspace,
    /// One thread's rows. A thread's events commit in sequence order
    /// (ADR 0360), so its `MAX(sequence)` is already a horizon and never waits.
    Thread(Uuid),
}

/// The rows a replay offers: sequences in `(after, through]`, minus `skip`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReplayWindow {
    pub(crate) after: i64,
    pub(crate) through: i64,
    /// Rows already handled live, or never broadcast at all.
    skip: Vec<RangeInclusive<i64>>,
    /// The floor this window was read at. `after` sits lower only for `owed`.
    floor: i64,
    /// Rows at or below the floor that an append in flight at the subscription
    /// sent afterwards, and that the subscriber never saw.
    owed: Vec<i64>,
}

impl ReplayWindow {
    /// Whether the row at `seq` is one this replay must offer.
    pub(crate) fn includes(&self, seq: i64) -> bool {
        seq > self.after && seq <= self.through && !self.skip.iter().any(|r| r.contains(&seq))
    }

    /// The part of this window up to `through`, for a replay that stopped
    /// after offering every row up to it.
    pub(crate) fn up_to(&self, through: i64) -> Self {
        Self {
            through: through.min(self.through),
            ..self.clone()
        }
    }
}

/// One subscriber's replay position, opened with [`LagReplay::subscribe`].
///
/// **[`Self::read`] hands on only the event types it is given.** `None` means
/// every type. The position itself records every live frame, of any type.
pub(crate) struct LagReplay {
    scope: ReplayScope,
    event_types: Option<Vec<&'static str>>,
    unbroadcast: UnbroadcastRows,
    tickets: AppendTickets,
    /// Every row at or below this was handled, live or by a replay, or the bus
    /// sent it before the subscription. `in_flight` names the exceptions.
    /// `None` while the bus has learned no sequence: it had sent nothing, so
    /// every row is owed, and the first lag sets the floor.
    floor: Option<i64>,
    /// The floor the subscription opened at. A live frame at or below it can
    /// only come from `in_flight`.
    opened_at: Option<i64>,
    /// The appends in flight at the subscription, which may still send a row
    /// at or below `opened_at`. Dropped once ended and fully seen.
    in_flight: Vec<InFlightAppend>,
    /// Rows at or below `opened_at` the subscriber already handled, live or by
    /// a replay. Only `in_flight` rows land here, so it stays a handful.
    seen_below: BTreeSet<i64>,
    /// Sequences above the floor that arrived live, as inclusive ranges keyed
    /// by their start. Frames arrive nearly in sequence order, so this stays a
    /// handful of ranges.
    received: BTreeMap<i64, i64>,
    /// The sequences replays have covered, from the first window's floor to
    /// the last window's top. A live frame in it arrived late from the buffer,
    /// and the subscriber skips it.
    replayed: Option<RangeInclusive<i64>>,
}

impl LagReplay {
    /// Subscribe to the bus and open the replay position, in one step. No
    /// read: the bus knows which rows it sent before (ADR 0453).
    pub(crate) fn subscribe(
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

    /// The frames one read of the bus gives the subscriber to act on: a live
    /// frame of its types, unless a replay already returned it, or every row a
    /// lag dropped. `None` once the bus closes.
    pub(crate) async fn read(
        &mut self,
        received: Result<EmittedEvent, RecvError>,
        pool: &PgPool,
    ) -> Option<Vec<EmittedEvent>> {
        match received {
            Ok(frame) if self.live(&frame) => Some(vec![frame]),
            Ok(_) => Some(Vec::new()),
            Err(RecvError::Lagged(n)) => {
                log!(
                    "[LagReplay] Lagged by {} frames, replaying from the events table",
                    n
                );
                Some(self.missed(pool).await)
            }
            Err(RecvError::Closed) => None,
        }
    }

    /// Whether to act on this live frame: one of the subscriber's types that
    /// no replay returned. Records it either way, so a later replay skips it.
    pub(crate) fn live(&mut self, frame: &EmittedEvent) -> bool {
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

    /// Whether to act on a live frame with this sequence. `false` means a
    /// replay already returned it. Records it, so a later replay skips it.
    fn first_sight(&mut self, seq: i64) -> bool {
        if self.is_covered(Some(seq)) {
            return false;
        }
        self.received(Some(seq));
        true
    }

    /// Record a frame that reached the subscriber live. Once every sequence
    /// right above the floor has arrived, the floor moves up over them.
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

    /// The window a lag must replay, read against a fresh horizon.
    ///
    /// `None` when there is nothing to replay. A failed horizon read moves
    /// nothing, so the next lag's window covers this one too.
    pub(crate) async fn window(&mut self, pool: &PgPool) -> Option<ReplayWindow> {
        let top = match horizon(pool, self.scope).await {
            Ok(top) => top,
            Err(e) => {
                log!(
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
                            log!(
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
        let owed = self.owed();
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

    /// Set the floor of a subscription that opened before the bus learned
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

    /// The `in_flight` rows sent at or below `opened_at` that the subscriber
    /// has not seen, lowest first. Only sent rows: an unsent one arrives live.
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

    /// Record that every row of `window` was offered. A replay that stopped
    /// part way passes [`ReplayWindow::up_to`] instead, and the next lag reads
    /// the rest.
    pub(crate) fn covered(&mut self, window: &ReplayWindow) {
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

    /// Forget the in-flight appends that ended with every row the subscriber
    /// owed from them seen.
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

    /// Whether a frame needs no action: a replay offered it, or, below the
    /// opening floor, it already arrived live. A frame below the first window
    /// that no replay owed was never replayed, so it is not covered.
    fn is_covered(&self, seq: Option<i64>) -> bool {
        seq.is_some_and(|seq| {
            self.seen_below.contains(&seq)
                || self
                    .replayed
                    .as_ref()
                    .is_some_and(|replayed| replayed.contains(&seq))
        })
    }

    /// The rows a lag dropped that this subscriber has not seen, oldest first.
    /// A failed read returns none and keeps the floor, so the next replay
    /// covers this window too.
    pub(crate) async fn missed(&mut self, pool: &PgPool) -> Vec<EmittedEvent> {
        let Some(window) = self.window(pool).await else {
            return Vec::new();
        };
        let rows = match self.rows_in(pool, &window).await {
            Ok(rows) => rows,
            Err(e) => {
                log!(
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

/// The ranges in `(after, floor]` that are not `owed`, which is sorted.
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
    /// The row as the frame its live broadcast carried, less the aggregate
    /// snapshot, which is never stored. The stored dispatch depth stands in for
    /// the frame's depth, so [`EmittedEvent::trigger_chain_depth`] reads as it
    /// did live. A row that does not read back is logged and skipped.
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
                log!(
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
