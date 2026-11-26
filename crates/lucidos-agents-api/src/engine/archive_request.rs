use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use uuid::Uuid;

use crate::engine::event_bus::BusEvent;
use crate::engine::standing_apply::{
    read_turn_settle, thread_to_resolve, AgentSessions, TurnSettle,
};
use crate::engine::thread_events::agent_thread_actor;
use crate::engine::thread_events::ThreadEvent;
use crate::engine::thread_lifecycle::{ChangeStateKind, ThreadStatus};
use crate::engine::LucidosEngine;

type BoxError = Box<dyn std::error::Error + Send + Sync>;

const CLOSING_EVENTS_SQL: &str = "('ThreadArchived', 'MessageReceived')";

const CLAIM_RETRY: Duration = Duration::from_secs(2);

#[derive(Debug, Clone)]
pub(crate) struct RequestFacts {
    pub open: bool,
    pub archived: bool,
    pub status: ThreadStatus,
    pub parked_on_question: bool,
    pub live_event_waits: bool,
    pub turn_settle: TurnSettle,
    pub pending_change: bool,
    pub unproposed_work: bool,
    pub blocking_descendants: bool,
    pub pinned: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RequestVerdict {
    Closed,
    Wait,
    Archive,
}

pub(crate) fn request_verdict(facts: &RequestFacts) -> RequestVerdict {
    if !facts.open || facts.archived {
        return RequestVerdict::Closed;
    }
    let settled = facts.status == ThreadStatus::Idle
        && !facts.parked_on_question
        && !facts.live_event_waits
        && facts.turn_settle == TurnSettle::Settled;
    if settled
        && !facts.pending_change
        && !facts.unproposed_work
        && !facts.blocking_descendants
        && !facts.pinned
    {
        RequestVerdict::Archive
    } else {
        RequestVerdict::Wait
    }
}

async fn request_is_open(pool: &sqlx::PgPool, thread_id: Uuid) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar(&format!(
        concat!(
            "SELECT EXISTS ( \
           SELECT 1 FROM events r \
            WHERE r.thread_id = $1 AND r.event_type = 'ThreadArchiveRequested' \
              AND r.sequence > ",
            crate::core::snapshot::boundary::restore_floor_sql!(),
            " \
              AND r.sequence > COALESCE(( \
                SELECT MAX(c.sequence) FROM events c \
                 WHERE c.thread_id = $1 AND c.event_type IN {CLOSING_EVENTS_SQL}), 0))"
        ),
        CLOSING_EVENTS_SQL = CLOSING_EVENTS_SQL
    ))
    .bind(thread_id)
    .fetch_one(pool)
    .await
}

async fn threads_with_open_requests(pool: &sqlx::PgPool) -> Result<HashSet<Uuid>, sqlx::Error> {
    let rows: Vec<(Uuid,)> = sqlx::query_as(&format!(
        concat!(
            "SELECT DISTINCT r.thread_id FROM events r \
          WHERE r.event_type = 'ThreadArchiveRequested' \
            AND r.sequence > ",
            crate::core::snapshot::boundary::restore_floor_sql!(),
            " \
            AND r.sequence > COALESCE(( \
              SELECT MAX(c.sequence) FROM events c \
               WHERE c.thread_id = r.thread_id AND c.event_type IN {CLOSING_EVENTS_SQL}), 0)"
        ),
        CLOSING_EVENTS_SQL = CLOSING_EVENTS_SQL
    ))
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(|(id,)| id).collect())
}

type SummaryRow = (
    String,
    String,
    i32,
    ChangeStateKind,
    Option<String>,
    bool,
    i32,
    bool,
    bool,
);

pub(crate) async fn read_request_facts(
    pool: &sqlx::PgPool,
    sessions: &AgentSessions,
    thread_id: Uuid,
) -> Result<Option<RequestFacts>, BoxError> {
    let row: Option<SummaryRow> = sqlx::query_as(
        "SELECT status, archive_state, live_event_wait_count, coding_agent_change_state, \
                coding_agent_unproposed_reason, coding_agent_is_external_repo, \
                blocking_descendant_count, \
                is_coding_agent, is_saved \
           FROM thread_summaries WHERE thread_id = $1",
    )
    .bind(thread_id)
    .fetch_optional(pool)
    .await?;
    let Some((
        status,
        archive_state,
        live_waits,
        change_state,
        withheld_reason,
        external,
        blocking,
        is_coding_agent,
        is_saved,
    )) = row
    else {
        return Ok(None);
    };
    let status = ThreadStatus::parse(&status);
    let (open, parked_on_question, turn_settle) = if status == ThreadStatus::Idle {
        let open = request_is_open(pool, thread_id).await?;
        let parked =
            crate::engine::question_park_sql::thread_parked_on_question(pool, thread_id).await?;
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
        pending_change: change_state == ChangeStateKind::Proposed && !external,
        unproposed_work: change_state == ChangeStateKind::Unproposed
            && withheld_reason.is_none()
            && !external,
        blocking_descendants: blocking > 0,
        pinned: is_saved,
    }))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RequestState {
    Closed,
    Open,
    ClaimHeld,
}

pub fn start_archive_request_resolver(engine: &std::sync::Arc<LucidosEngine>) {
    engine.start_archive_request_resolver();
}

#[easy_ext::ext(ArchiveRequestExt)]
impl LucidosEngine {
    async fn resolve_archive_request(self: &Arc<Self>, thread_id: Uuid) -> RequestState {
        let facts = match read_request_facts(self.pool(), &self.agent_sessions, thread_id).await {
            Ok(Some(facts)) => facts,
            Ok(None) => return RequestState::Closed,
            Err(e) => {
                log_legacy!(
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
                match crate::engine::archive_family::archive_family(
                    self,
                    thread_id,
                    actor,
                    crate::engine::archive_family::PinnedMembers::ByActor,
                )
                .await
                {
                    Ok(outcome) if outcome.archived.contains(&thread_id) => {
                        log_legacy!("[ArchiveRequest] {} settled and was archived", thread_id);
                        RequestState::Closed
                    }
                    Ok(_) => RequestState::Open,
                    Err(rejection) => {
                        log_legacy!(
                            "[ArchiveRequest] {} not archived yet: {}",
                            thread_id,
                            crate::engine::archive_family::rejection_text(&rejection)
                        );
                        RequestState::Open
                    }
                }
            }
        }
    }

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

    async fn recover_archive_requests(
        self: &Arc<Self>,
        watched: &mut HashSet<Uuid>,
        retry: &tokio::sync::mpsc::UnboundedSender<Uuid>,
    ) {
        match threads_with_open_requests(self.pool()).await {
            Ok(open) => *watched = open,
            Err(e) => log_legacy!("[ArchiveRequest] open request scan failed: {}", e),
        }
        for thread_id in watched.clone() {
            self.track_archive_request(thread_id, watched, retry).await;
        }
    }

    fn start_archive_request_resolver(self: &Arc<Self>) {
        let mut rx = self.event_bus.subscribe();
        let engine = self.clone();
        let (retry_tx, mut retry_rx) = tokio::sync::mpsc::unbounded_channel::<Uuid>();
        tokio::spawn(async move {
            log_legacy!("[ArchiveRequest] resolver started");
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
                            // INVARIANT: The skipped batch may hold a request or the settle
                            // it waits for, so rebuild from the log.
                            log_legacy!(
                                "[ArchiveRequest] subscriber lagged by {} events, re-resolving",
                                n
                            );
                            engine.recover_archive_requests(&mut watched, &retry_tx).await;
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                            log_legacy!("[ArchiveRequest] EventBus closed, resolver stopping");
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
