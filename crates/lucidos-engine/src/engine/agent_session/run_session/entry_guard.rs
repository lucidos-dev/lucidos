//! RAII backstop for a session's `agent_sessions` entry.
//!
//! `run_session` inserts an [`AgentSession`] into `agent_sessions` and removes
//! it again on every *completion* path. Cancellation is the gap: if the run
//! future is **dropped** rather than polled to completion, none of those
//! removals run and the entry survives as a **phantom session** — present,
//! `process_exited == false`, but with no loop behind it.
//!
//! That is not hypothetical. On 2026-07-28 `POST /api/v1/claude-code/apply-now`
//! awaited a whole merge session inline in the axum handler; iOS Safari dropped
//! the connection 72 s in, hyper dropped the handler future, and the leftover
//! entry made worktree cleanup skip the thread ("live agent session active")
//! and made every follow-up bounce off the resume guard with "A coding agent is
//! already running for this thread". The thread stayed wedged until the engine
//! restarted.
//!
//! [`AgentSession::is_live`] makes such an entry *harmless* the instant it
//! appears (it consults `msg_tx.is_closed()`, which needs no cleanup to have
//! run). This guard makes it *transient* as well: the entry is reaped, and the
//! thread — which would otherwise sit mid-turn forever with no terminal event —
//! is settled.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use tokio::sync::mpsc::UnboundedSender;
use tokio::sync::Mutex;
use uuid::Uuid;

use crate::engine::event_bus::{BusEvent, EventBus};
use crate::engine::thread_events::{AbortCause, EventMeta, ThreadEvent};
use crate::engine::types::{AgentSession, AgentUserInput};

/// Drop-guard over `agent_sessions[thread_id]` for one `run_session` call.
///
/// Identity is the message channel, not the thread id: a recovery hand-off
/// deliberately leaves the outgoing session in the map until the incoming one
/// replaces it (see the `recovery_worktree` skip in `run_session`), so a blind
/// `remove(&thread_id)` on drop would delete the *replacement*. Comparing
/// `msg_tx` with `same_channel` reaps only the entry this run inserted.
pub(super) struct SessionEntryGuard {
    sessions: Arc<Mutex<HashMap<Uuid, AgentSession>>>,
    thread_id: Uuid,
    /// Sender half of *this run's* channel. Held purely as an identity token —
    /// keeping a sender alive does not keep the session live, because
    /// `is_live` is decided by the receiver (owned by the run future).
    msg_tx: UnboundedSender<AgentUserInput>,
    /// Set by the engine shutdown sweep, which emits its own terminal for every
    /// running thread. Suppresses ours so a switch doesn't double-report.
    shutting_down: Arc<AtomicBool>,
    event_bus: EventBus,
    pool: sqlx::PgPool,
}

impl SessionEntryGuard {
    pub(super) fn new(
        sessions: Arc<Mutex<HashMap<Uuid, AgentSession>>>,
        thread_id: Uuid,
        msg_tx: UnboundedSender<AgentUserInput>,
        shutting_down: Arc<AtomicBool>,
        event_bus: EventBus,
        pool: sqlx::PgPool,
    ) -> Self {
        Self {
            sessions,
            thread_id,
            msg_tx,
            shutting_down,
            event_bus,
            pool,
        }
    }
}

impl Drop for SessionEntryGuard {
    fn drop(&mut self) {
        // `Drop` can't await, and the map is behind an async mutex, so the
        // cleanup runs as a detached task. It is deliberately idempotent and
        // no-ops in the overwhelmingly common case (a completed run already
        // removed its own entry), so nothing depends on when it lands.
        let sessions = self.sessions.clone();
        let thread_id = self.thread_id;
        let msg_tx = self.msg_tx.clone();
        let shutting_down = self.shutting_down.clone();
        let event_bus = self.event_bus.clone();
        let pool = self.pool.clone();

        // Outside a runtime (a unit test dropping the guard synchronously,
        // or the runtime already gone during shutdown) there is nothing to
        // spawn onto — and nothing to clean up either, since the map dies with
        // the process. Reaping inline is impossible (async mutex), so skip.
        let Ok(handle) = tokio::runtime::Handle::try_current() else {
            return;
        };

        handle.spawn(async move {
            let Some(reaped) = reap_entry(&sessions, thread_id, &msg_tx).await else {
                return;
            };
            log!(
                "[AgentSession] Session future for thread {} was dropped before completing — \
                 reaped its agent_sessions entry (process_exited={})",
                thread_id,
                !reaped.was_live
            );
            if !reaped.was_live {
                // `finalize_direct_agent` sets `process_exited` as its very
                // first act, so an entry already flagged means the loop DID
                // run and reach teardown — it just didn't get as far as the
                // `remove`. Whatever ended that turn owns its terminal (or
                // deliberately left none); claiming `SessionDropped` on top
                // would mislabel the cause. Reaping the leak is still right.
                return;
            }
            if shutting_down.load(Ordering::Acquire) {
                // The shutdown sweep owns the terminal for this thread.
                return;
            }
            settle_dropped_session(&pool, &event_bus, thread_id).await;
        });
    }
}

/// Outcome of a reap: present only when the entry belonged to this run.
struct Reaped {
    /// Whether the entry was still live when reaped — i.e. the loop never got
    /// to set `process_exited`, which is what makes this a genuinely *dropped*
    /// session rather than a teardown that stopped short of its `remove`.
    was_live: bool,
}

/// Remove `thread_id`'s entry iff it is still the one owning `msg_tx`'s
/// channel. `None` when the entry is absent (the common case — a completed run
/// removed it) or belongs to a different run.
async fn reap_entry(
    sessions: &Arc<Mutex<HashMap<Uuid, AgentSession>>>,
    thread_id: Uuid,
    msg_tx: &UnboundedSender<AgentUserInput>,
) -> Option<Reaped> {
    let mut guard = sessions.lock().await;
    // `is_live()` would be false either way here — this run's `msg_rx` is
    // already gone — so ask `process_exited` directly: it is the flag that
    // distinguishes "the loop reached teardown" from "the loop never ran it".
    let ours = guard
        .get(&thread_id)
        .filter(|s| s.msg_tx.same_channel(msg_tx))
        .map(|s| Reaped {
            was_live: !s.process_exited,
        })?;
    guard.remove(&thread_id);
    Some(ours)
}

/// Emit the terminal the dropped loop never got to emit, so the thread stops
/// reading as mid-turn. Gated on the projection still showing `running` —
/// which also covers the case where some other path (shutdown sweep, recovery,
/// an already-emitted `Result`) settled the thread first.
async fn settle_dropped_session(pool: &sqlx::PgPool, event_bus: &EventBus, thread_id: Uuid) {
    match crate::engine::claude_code::thread_is_running(pool, thread_id).await {
        Ok(false) => return,
        Ok(true) => {}
        Err(e) => {
            log!(
                "[AgentSession] dropped-session settle probe failed for {}: {} — \
                 skipping the terminal emit rather than risk a duplicate",
                thread_id,
                e
            );
            return;
        }
    }
    event_bus
        .emit_or_log(
            BusEvent::Thread {
                thread_id,
                event: ThreadEvent::ResponseAborted {
                    text: String::new(),
                    images: vec![],
                    model: None,
                    reasoning_effort: None,
                    cause: AbortCause::SessionDropped,
                },
                // No actor: nobody chose this. The caller's future was
                // cancelled out from under the session — a transport event,
                // not a decision. `stamp_system_actor_if_aborted` supplies
                // `MessageOrigin::System` downstream.
                meta: EventMeta::NONE,
            },
            "[AgentSession] ResponseAborted (session future dropped)",
        )
        .await;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sessions_map() -> Arc<Mutex<HashMap<Uuid, AgentSession>>> {
        Arc::new(Mutex::new(HashMap::new()))
    }

    /// The whole point: a run that never completed still gives its entry back.
    #[tokio::test]
    async fn reaps_the_entry_it_inserted() {
        let sessions = sessions_map();
        let thread_id = Uuid::new_v4();
        let (session, _msg_rx) = AgentSession::for_test();
        let msg_tx = session.msg_tx.clone();
        sessions.lock().await.insert(thread_id, session);

        let reaped = reap_entry(&sessions, thread_id, &msg_tx)
            .await
            .expect("the entry belongs to this run");
        assert!(
            reaped.was_live,
            "the loop never set process_exited — this is a genuine dropped session"
        );
        assert!(
            !sessions.lock().await.contains_key(&thread_id),
            "a dropped run must not leave its entry behind"
        );
    }

    /// A teardown that set `process_exited` but stopped short of its own
    /// `remove` still gets its entry reaped — but must NOT be reported as a
    /// dropped session, because the loop demonstrably ran. Emitting
    /// `SessionDropped` there would mislabel whatever actually ended the turn.
    #[tokio::test]
    async fn reaps_but_does_not_claim_a_session_that_reached_teardown() {
        let sessions = sessions_map();
        let thread_id = Uuid::new_v4();
        let (mut session, _msg_rx) = AgentSession::for_test();
        session.process_exited = true;
        let msg_tx = session.msg_tx.clone();
        sessions.lock().await.insert(thread_id, session);

        let reaped = reap_entry(&sessions, thread_id, &msg_tx)
            .await
            .expect("the entry belongs to this run");
        assert!(
            !reaped.was_live,
            "process_exited means finalize ran — not a dropped session"
        );
        assert!(!sessions.lock().await.contains_key(&thread_id));
    }

    /// Recovery hand-off: the incoming session replaces the outgoing one in the
    /// map *before* the outgoing future finishes dropping. The old guard must
    /// not reap the replacement — that would delete a live session and strand
    /// the resumed turn.
    #[tokio::test]
    async fn leaves_a_replacement_session_alone() {
        let sessions = sessions_map();
        let thread_id = Uuid::new_v4();

        let (outgoing, _outgoing_rx) = AgentSession::for_test();
        let outgoing_tx = outgoing.msg_tx.clone();

        let (replacement, _replacement_rx) = AgentSession::for_test();
        let replacement_tx = replacement.msg_tx.clone();
        sessions.lock().await.insert(thread_id, replacement);

        assert!(
            reap_entry(&sessions, thread_id, &outgoing_tx).await.is_none(),
            "the outgoing run must not claim an entry it does not own"
        );
        let guard = sessions.lock().await;
        let still_there = guard.get(&thread_id).expect("replacement must survive");
        assert!(
            still_there.msg_tx.same_channel(&replacement_tx),
            "the surviving entry must be the replacement, untouched"
        );
    }

    /// A completed run removes its own entry first; the guard then finds
    /// nothing and must report that it reaped nothing (so no terminal is
    /// emitted on top of the one the loop already produced).
    #[tokio::test]
    async fn no_op_when_the_entry_is_already_gone() {
        let sessions = sessions_map();
        let thread_id = Uuid::new_v4();
        let (session, _msg_rx) = AgentSession::for_test();
        let msg_tx = session.msg_tx.clone();
        // Never inserted — models the completion path having removed it.
        drop(session);

        assert!(reap_entry(&sessions, thread_id, &msg_tx).await.is_none());
    }

    /// Dropping the guard outside a Tokio runtime must not panic. `Drop` can't
    /// await, so the cleanup needs a runtime to spawn onto; without one there
    /// is also nothing worth cleaning (the process is going away). Build the
    /// guard inside a runtime — `PgPool` construction requires one — then shut
    /// the runtime down and drop the guard with no context current.
    #[test]
    fn drop_outside_a_runtime_is_inert() {
        let rt = tokio::runtime::Runtime::new().expect("runtime");
        let guard = rt.block_on(async {
            let (session, _msg_rx) = AgentSession::for_test();
            let pool = sqlx::PgPool::connect_lazy("postgres://invalid/none").expect("lazy pool");
            let (bus, _rx) = EventBus::new(pool.clone());
            SessionEntryGuard::new(
                sessions_map(),
                Uuid::new_v4(),
                session.msg_tx.clone(),
                Arc::new(AtomicBool::new(false)),
                bus,
                pool,
            )
        });
        drop(rt);
        drop(guard); // must not panic
    }
}
