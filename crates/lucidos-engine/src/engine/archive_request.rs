//! The *archive request*: a thread's own agent asked to be archived once its
//! turn ends (ADR 0310).
//!
//! The agent is mid-turn when it asks, so archiving on the spot cannot work.
//! The cascade gate refuses a running thread, and the turn's own end would move
//! it back to the inbox. So the call records `ThreadArchiveRequested`, and this
//! resolver runs the Archive button's cascade once the thread has settled.
//!
//! **The request lives in the event log.** A request is open while its thread
//! holds no newer `ThreadArchived` or `MessageReceived`. So a restart keeps it,
//! and a follow-up message closes it with nothing else to update.
//!
//! **It only ever waits or archives.** A thread that has not settled, or that
//! the cascade gate would refuse, keeps its request, and the next event on it
//! re-takes the verdict. Nothing here ends a request except the two events
//! above.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use uuid::Uuid;

use crate::engine::chat::agent_archive::agent_thread_actor;
use crate::engine::event_bus::BusEvent;
use crate::engine::standing_apply::{
    read_turn_settle, thread_to_resolve, AgentSessions, TurnSettle,
};
use crate::engine::thread_events::ThreadEvent;
use crate::engine::thread_lifecycle::ThreadStatus;
use crate::engine::LucidosEngine;

type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// The events that close an open request: the archive itself, and a message
/// asking the thread for more.
const CLOSING_EVENTS_SQL: &str = "('ThreadArchived', 'MessageReceived')";

/// How long a request waits before retrying past a held change claim. An
/// Apply or Discard releases its claim with no event behind it, so nothing
/// else would wake the request.
const CLAIM_RETRY: Duration = Duration::from_secs(2);

/// What the verdict reads. One struct, so the decision is testable without a
/// database.
#[derive(Debug, Clone)]
pub(crate) struct RequestFacts {
    /// The newest `ThreadArchiveRequested` is newer than every closing event.
    /// Read only for an idle thread: any other one waits whatever it says.
    pub open: bool,
    pub archived: bool,
    /// `thread_summaries.status`.
    pub status: ThreadStatus,
    /// A row can read `idle` with its question card still open (ADR 0293).
    pub parked_on_question: bool,
    pub live_event_waits: bool,
    pub turn_settle: TurnSettle,
    /// A pending change the Archive button's gate refuses. An external-repo
    /// change does not count: the cascade clears it, as the gate lets it.
    pub pending_change: bool,
    pub blocking_descendants: bool,
    /// The user pinned the thread after its agent asked. An agent never
    /// archives a pinned thread (ADR 0312), so the request waits for an unpin.
    pub pinned: bool,
}

/// What the resolver does with a request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RequestVerdict {
    /// No open request, or the thread is already archived.
    Closed,
    /// Not settled yet, or the cascade gate would refuse. The next event
    /// re-takes the verdict.
    Wait,
    Archive,
}

/// Take the verdict. Pure.
///
/// It waits on everything the Archive button's gate refuses. It also waits on
/// the standing apply's settle probes, so the cascade never runs between a
/// turn's terminal and its idle.
pub(crate) fn request_verdict(facts: &RequestFacts) -> RequestVerdict {
    if !facts.open || facts.archived {
        return RequestVerdict::Closed;
    }
    let settled = facts.status == ThreadStatus::Idle
        && !facts.parked_on_question
        && !facts.live_event_waits
        && facts.turn_settle == TurnSettle::Settled;
    if settled && !facts.pending_change && !facts.blocking_descendants && !facts.pinned {
        RequestVerdict::Archive
    } else {
        RequestVerdict::Wait
    }
}

/// Is the thread's newest request still open?
async fn request_is_open(pool: &sqlx::PgPool, thread_id: Uuid) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar(&format!(
        "SELECT EXISTS ( \
           SELECT 1 FROM events r \
            WHERE r.thread_id = $1 AND r.event_type = 'ThreadArchiveRequested' \
              AND r.sequence > COALESCE(( \
                SELECT MAX(c.sequence) FROM events c \
                 WHERE c.thread_id = $1 AND c.event_type IN {CLOSING_EVENTS_SQL}), 0))"
    ))
    .bind(thread_id)
    .fetch_one(pool)
    .await
}

/// Every thread holding an open request.
async fn threads_with_open_requests(pool: &sqlx::PgPool) -> Result<HashSet<Uuid>, sqlx::Error> {
    let rows: Vec<(Uuid,)> = sqlx::query_as(&format!(
        "SELECT DISTINCT r.thread_id FROM events r \
          WHERE r.event_type = 'ThreadArchiveRequested' \
            AND r.sequence > COALESCE(( \
              SELECT MAX(c.sequence) FROM events c \
               WHERE c.thread_id = r.thread_id AND c.event_type IN {CLOSING_EVENTS_SQL}), 0)"
    ))
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(|(id,)| id).collect())
}

/// The summary columns the verdict reads.
type SummaryRow = (String, String, i32, bool, bool, i32, bool, bool);

/// Read the facts. `Ok(None)` = the thread is gone. A probe that could not run
/// is an `Err`, and the request keeps its place.
///
/// Takes the pool and the session map rather than the engine, so the whole
/// read is testable without standing one up.
pub(crate) async fn read_request_facts(
    pool: &sqlx::PgPool,
    sessions: &AgentSessions,
    thread_id: Uuid,
) -> Result<Option<RequestFacts>, BoxError> {
    let row: Option<SummaryRow> = sqlx::query_as(
        "SELECT status, archive_state, live_event_wait_count, coding_agent_proposed, \
                coding_agent_is_external_repo, blocking_descendant_count, is_coding_agent, \
                is_saved \
           FROM thread_summaries WHERE thread_id = $1",
    )
    .bind(thread_id)
    .fetch_optional(pool)
    .await?;
    let Some((
        status,
        archive_state,
        live_waits,
        proposed,
        external,
        blocking,
        is_coding_agent,
        is_saved,
    )) = row
    else {
        return Ok(None);
    };
    let status = ThreadStatus::parse(&status);
    // The caller streams its whole turn after asking, so a running thread
    // skips every event scan. It waits whatever they would say.
    let (open, parked_on_question, turn_settle) = if status == ThreadStatus::Idle {
        let open = request_is_open(pool, thread_id).await?;
        let parked =
            crate::engine::agent_recovery::thread_parked_on_question(pool, thread_id).await?;
        let settle = if is_coding_agent {
            read_turn_settle(pool, thread_id, sessions)
                .await
                .ok_or("the turn closer lookup failed")?
        } else {
            TurnSettle::Settled
        };
        (open, parked, settle)
    } else {
        (true, false, TurnSettle::Settled)
    };
    Ok(Some(RequestFacts {
        open,
        archived: archive_state == "archived",
        status,
        parked_on_question,
        live_event_waits: live_waits > 0,
        turn_settle,
        pending_change: proposed && !external,
        blocking_descendants: blocking > 0,
        pinned: is_saved,
    }))
}

/// Where one resolution left a request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RequestState {
    Closed,
    Open,
    /// Open, and an Apply or Discard holds the thread's change claim. Its
    /// release emits nothing, so the resolver retries on its own.
    ClaimHeld,
}

impl LucidosEngine {
    /// Take one request's verdict and act on it.
    async fn resolve_archive_request(self: &Arc<Self>, thread_id: Uuid) -> RequestState {
        let facts = match read_request_facts(self.pool(), &self.agent_sessions, thread_id).await {
            Ok(Some(facts)) => facts,
            Ok(None) => return RequestState::Closed,
            Err(e) => {
                log!(
                    "[ArchiveRequest] facts for {} could not be read: {}",
                    thread_id,
                    e
                );
                return RequestState::Open;
            }
        };
        match request_verdict(&facts) {
            RequestVerdict::Closed => RequestState::Closed,
            RequestVerdict::Wait => RequestState::Open,
            RequestVerdict::Archive if self.change_claim_holder(thread_id).await.is_some() => {
                RequestState::ClaimHeld
            }
            RequestVerdict::Archive => {
                let actor = Some(agent_thread_actor(thread_id));
                match crate::api::threads::archive::archive_family(
                    self,
                    thread_id,
                    actor,
                    crate::api::threads::archive::PinnedMembers::ByActor,
                )
                .await
                {
                    Ok(outcome) if outcome.archived.contains(&thread_id) => {
                        log!("[ArchiveRequest] {} settled and was archived", thread_id);
                        RequestState::Closed
                    }
                    Ok(_) => RequestState::Open,
                    Err(rejection) => {
                        log!(
                            "[ArchiveRequest] {} not archived yet: {}",
                            thread_id,
                            crate::api::threads::archive::rejection_text(&rejection)
                        );
                        RequestState::Open
                    }
                }
            }
        }
    }

    /// Resolve one thread and record where its request stands: watched while
    /// open, and retried after [`CLAIM_RETRY`] while a claim holds it.
    async fn track_archive_request(
        self: &Arc<Self>,
        thread_id: Uuid,
        watched: &mut HashSet<Uuid>,
        retry: &tokio::sync::mpsc::UnboundedSender<Uuid>,
    ) {
        match self.resolve_archive_request(thread_id).await {
            RequestState::Closed => {
                watched.remove(&thread_id);
            }
            RequestState::Open => {
                watched.insert(thread_id);
            }
            RequestState::ClaimHeld => {
                watched.insert(thread_id);
                let retry = retry.clone();
                tokio::spawn(async move {
                    tokio::time::sleep(CLAIM_RETRY).await;
                    let _ = retry.send(thread_id);
                });
            }
        }
    }

    /// Rebuild the watched set from the event log, then re-take every verdict.
    /// On boot this catches a thread that settled while the engine was down.
    async fn recover_archive_requests(
        self: &Arc<Self>,
        watched: &mut HashSet<Uuid>,
        retry: &tokio::sync::mpsc::UnboundedSender<Uuid>,
    ) {
        match threads_with_open_requests(self.pool()).await {
            Ok(open) => *watched = open,
            Err(e) => log!("[ArchiveRequest] open request scan failed: {}", e),
        }
        for thread_id in watched.clone() {
            self.track_archive_request(thread_id, watched, retry).await;
        }
    }

    /// Start the bus subscriber that archives a thread once its request can
    /// land.
    ///
    /// The watched set is the cheap filter: an engine with no open request
    /// runs no query per event. The event log is the authority, so the set is
    /// rebuilt from it at start and whenever the subscriber lags.
    pub fn start_archive_request_resolver(self: &Arc<Self>) {
        let mut rx = self.event_bus.subscribe();
        let engine = self.clone();
        let (retry_tx, mut retry_rx) = tokio::sync::mpsc::unbounded_channel::<Uuid>();
        tokio::spawn(async move {
            log!("[ArchiveRequest] resolver started");
            let mut watched = HashSet::new();
            engine
                .recover_archive_requests(&mut watched, &retry_tx)
                .await;
            loop {
                tokio::select! {
                    Some(thread_id) = retry_rx.recv() => {
                        if watched.contains(&thread_id) {
                            engine.track_archive_request(thread_id, &mut watched, &retry_tx).await;
                        }
                    }
                    received = rx.recv() => match received {
                        Ok(emitted) => {
                            let Some(thread_id) = thread_to_resolve(&emitted.typed) else {
                                continue;
                            };
                            let requested = matches!(
                                emitted.typed,
                                BusEvent::Thread {
                                    event: ThreadEvent::ThreadArchiveRequested,
                                    ..
                                }
                            );
                            if requested || watched.contains(&thread_id) {
                                engine.track_archive_request(thread_id, &mut watched, &retry_tx).await;
                            }
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                            // The skipped batch may hold a request or the settle
                            // it waits for, so rebuild from the log.
                            log!(
                                "[ArchiveRequest] subscriber lagged by {} events, re-resolving",
                                n
                            );
                            engine.recover_archive_requests(&mut watched, &retry_tx).await;
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                            log!("[ArchiveRequest] EventBus closed, resolver stopping");
                            break;
                        }
                    },
                }
            }
        });
    }
}

#[cfg(test)]
#[path = "archive_request_tests.rs"]
mod tests;
