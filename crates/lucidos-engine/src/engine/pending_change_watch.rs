//! EventBus consumer that keeps the Changes panel's Apply gate current.
//!
//! A pending change's row reads `thread_unsettled` and `thread_settling` from
//! the `ChangesUpdated` frame, and the engine computes both when it builds the
//! frame. The frame goes out on change events and at turn end, but a thread can
//! leave its settled state without either: a follow-up starts a turn, a
//! question opens, an event wait is armed. With no new frame, the row would
//! keep offering an Apply the engine refuses.
//!
//! So this consumer watches each thread event and its projection snapshot. It
//! sends a fresh frame when a pending change's thread changes status or wait
//! state, or its question opens or ends. It decides only *when* to recompute;
//! the frame's own queries decide what the row shows.

use std::collections::HashMap;
use std::future::Future;
use std::sync::Arc;

use tokio::sync::broadcast;
use tokio_stream::wrappers::errors::BroadcastStreamRecvError;
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::StreamExt;
use uuid::Uuid;

use super::event_bus::{BusEvent, EmittedEvent};
use super::thread_events::ThreadEvent;
use super::thread_lifecycle::ThreadStatus;
use super::LucidosEngine;

/// The projection fields the Apply gate reads from `thread_summaries`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Snapshot {
    pub status: ThreadStatus,
    pub watching: bool,
}

/// What one event does to the thread's question park.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum QuestionMove {
    Asked,
    Ended,
    Untouched,
}

impl QuestionMove {
    /// The same reading as the gate's unanswered-question predicate.
    fn of(event: &ThreadEvent) -> Self {
        if matches!(event, ThreadEvent::UserQuestionAsked { .. }) {
            Self::Asked
        } else if super::agent_recovery::ends_question_park(event.event_type()) {
            Self::Ended
        } else {
            Self::Untouched
        }
    }
}

/// Everything the Apply gate reads. The open question is event-derived: a
/// thread can read `idle` with its question still open (ADR 0293).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct GateInputs {
    snapshot: Snapshot,
    /// `None` until a question event is seen: after a restart or a lag, the
    /// park is unknown, and its first known value counts as a change.
    question_open: Option<bool>,
}

/// The last gate inputs seen per thread with a proposed change. A cache: a
/// restart starts it empty, and reconnecting clients refetch the list anyway.
#[derive(Debug, Default)]
pub(crate) struct PendingChangeWatch {
    last: HashMap<Uuid, GateInputs>,
    /// Each thread's question park, where known. An open one is kept for any
    /// thread, since a question can open before the proposal. A closed one is
    /// kept only while the thread holds a pending change.
    questions: HashMap<Uuid, bool>,
}

impl PendingChangeWatch {
    /// Record one event, and say whether the frame must be rebuilt.
    ///
    /// A thread seen for the first time rebuilds it too: after a restart, the
    /// state the panel last drew is unknown.
    ///
    /// A thread with no proposed change has no pending row, so it is dropped.
    /// Proposing, applying and discarding each send their own frame.
    pub(crate) fn observe(&mut self, thread_id: Uuid, proposed: bool, snapshot: Snapshot) -> bool {
        if !proposed {
            self.last.remove(&thread_id);
            if self.questions.get(&thread_id) == Some(&false) {
                self.questions.remove(&thread_id);
            }
            return false;
        }
        let inputs = GateInputs {
            snapshot,
            question_open: self.questions.get(&thread_id).copied(),
        };
        self.last.insert(thread_id, inputs) != Some(inputs)
    }

    /// Follow one persisted event's effect on the thread's question park. The
    /// next [`Self::observe`] reads the result.
    pub(crate) fn note_question(&mut self, thread_id: Uuid, question: QuestionMove) {
        let open = match question {
            QuestionMove::Asked => true,
            QuestionMove::Ended => false,
            QuestionMove::Untouched => return,
        };
        self.questions.insert(thread_id, open);
    }

    /// Drop every cached state, so each thread's next event rebuilds the frame
    /// and every question park reads as unknown.
    pub(crate) fn forget_all(&mut self) {
        self.last.clear();
        self.questions.clear();
    }
}

/// Spawn the consumer. Returns the `JoinHandle` so a caller can observe panics.
pub fn spawn(engine: Arc<LucidosEngine>) -> tokio::task::JoinHandle<()> {
    let rx = engine.event_bus.subscribe();
    tokio::spawn(watch(rx, move || {
        let engine = engine.clone();
        async move { engine.broadcast_changes_updated().await }
    }))
}

/// Run `on_shift` each time a thread holding a pending change moves to new
/// gate inputs. Ends when the bus closes.
pub(crate) async fn watch<F, Fut>(rx: broadcast::Receiver<EmittedEvent>, mut on_shift: F)
where
    F: FnMut() -> Fut,
    Fut: Future<Output = ()>,
{
    let mut state = PendingChangeWatch::default();
    let stream = BroadcastStream::new(rx);
    tokio::pin!(stream);
    while let Some(result) = stream.next().await {
        let emitted = match result {
            Ok(e) => e,
            // A lag may have hidden a shift, so rebuild rather than guess. The
            // cache is stale too, and a thread back in its cached state would
            // read as unchanged.
            Err(BroadcastStreamRecvError::Lagged(n)) => {
                log!(
                    "[PendingChangeWatch] Broadcast lagged by {} events, rebuilding the changes frame",
                    n
                );
                state.forget_all();
                on_shift().await;
                continue;
            }
        };
        let BusEvent::Thread {
            thread_id, event, ..
        } = &emitted.typed
        else {
            continue;
        };
        // The gate's question predicate reads stored events only.
        if emitted.seq.is_some() {
            state.note_question(*thread_id, QuestionMove::of(event));
        }
        let Some(aggregate) = &emitted.aggregate else {
            continue;
        };
        let snapshot = Snapshot {
            status: aggregate.status,
            watching: aggregate.live_event_wait_count > 0,
        };
        let proposed = aggregate.coding_agent_change_state.is_proposed();
        if state.observe(*thread_id, proposed, snapshot) {
            on_shift().await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inputs(status: ThreadStatus, watching: bool) -> Snapshot {
        Snapshot { status, watching }
    }

    #[test]
    fn an_idle_thread_whose_question_ends_rebuilds_the_frame() {
        // ADR 0293: the status reads idle while the question is still open.
        let mut watch = PendingChangeWatch::default();
        let thread = Uuid::new_v4();
        watch.note_question(thread, QuestionMove::Asked);
        watch.observe(thread, true, inputs(ThreadStatus::Idle, false));
        watch.note_question(thread, QuestionMove::Untouched);
        assert!(!watch.observe(thread, true, inputs(ThreadStatus::Idle, false)));
        watch.note_question(thread, QuestionMove::Ended);
        assert!(watch.observe(thread, true, inputs(ThreadStatus::Idle, false)));
    }

    #[test]
    fn a_question_end_after_a_lag_rebuilds_the_frame_once() {
        // The lag hid the ask, so the park is unknown until the end arrives.
        let mut watch = PendingChangeWatch::default();
        let thread = Uuid::new_v4();
        watch.observe(thread, true, inputs(ThreadStatus::Idle, false));
        watch.forget_all();
        watch.observe(thread, true, inputs(ThreadStatus::Idle, false));
        watch.note_question(thread, QuestionMove::Ended);
        assert!(watch.observe(thread, true, inputs(ThreadStatus::Idle, false)));
        watch.note_question(thread, QuestionMove::Ended);
        assert!(!watch.observe(thread, true, inputs(ThreadStatus::Idle, false)));
    }

    #[test]
    fn a_question_reads_like_the_gate_predicate() {
        assert_eq!(QuestionMove::of(&agent_idled()), QuestionMove::Ended);
    }

    fn agent_idled() -> ThreadEvent {
        ThreadEvent::CodingAgentIdled {
            has_changes: true,
            is_external_repo: false,
            requires_restart: false,
            cc_session_id: None,
            coding_agent: crate::runtime::CodingAgent::ClaudeCode,
            reason: None,
            worktree_path: None,
            worktree_head_sha: None,
            bg_bash_pending: false,
        }
    }

    #[test]
    fn a_follow_up_on_a_thread_with_a_pending_change_rebuilds_the_frame() {
        let mut watch = PendingChangeWatch::default();
        let thread = Uuid::new_v4();
        watch.observe(thread, true, inputs(ThreadStatus::Idle, false));
        assert!(watch.observe(thread, true, inputs(ThreadStatus::Running, false)));
    }

    #[test]
    fn a_thread_seen_for_the_first_time_rebuilds_the_frame() {
        // After a restart the panel may show a wait that has since ended.
        let mut watch = PendingChangeWatch::default();
        let thread = Uuid::new_v4();
        assert!(watch.observe(thread, true, inputs(ThreadStatus::Idle, false)));
        assert!(!watch.observe(thread, true, inputs(ThreadStatus::Idle, false)));
    }

    #[test]
    fn a_thread_back_in_its_cached_state_after_a_lag_rebuilds_the_frame() {
        // The lag hid Running -> Idle, and the thread is Running again.
        let mut watch = PendingChangeWatch::default();
        let thread = Uuid::new_v4();
        watch.observe(thread, true, inputs(ThreadStatus::Running, false));
        watch.forget_all();
        assert!(watch.observe(thread, true, inputs(ThreadStatus::Running, false)));
    }

    #[test]
    fn events_inside_one_state_send_nothing_more() {
        let mut watch = PendingChangeWatch::default();
        let thread = Uuid::new_v4();
        assert!(watch.observe(thread, true, inputs(ThreadStatus::Running, false)));
        assert!(!watch.observe(thread, true, inputs(ThreadStatus::Running, false)));
    }

    #[test]
    fn a_question_a_wait_and_the_settle_each_rebuild_the_frame() {
        let mut watch = PendingChangeWatch::default();
        let thread = Uuid::new_v4();
        watch.observe(thread, true, inputs(ThreadStatus::Running, false));
        assert!(watch.observe(
            thread,
            true,
            inputs(ThreadStatus::WaitingForUserAnswer, false)
        ));
        assert!(watch.observe(thread, true, inputs(ThreadStatus::Idle, true)));
        assert!(watch.observe(thread, true, inputs(ThreadStatus::Idle, false)));
    }

    #[test]
    fn a_thread_without_a_pending_change_never_rebuilds_the_frame() {
        let mut watch = PendingChangeWatch::default();
        let thread = Uuid::new_v4();
        assert!(!watch.observe(thread, false, inputs(ThreadStatus::Running, false)));
        assert!(!watch.observe(thread, false, inputs(ThreadStatus::Idle, false)));
    }

    #[test]
    fn a_resolved_change_forgets_the_thread() {
        let mut watch = PendingChangeWatch::default();
        let thread = Uuid::new_v4();
        watch.observe(thread, true, inputs(ThreadStatus::Running, false));
        watch.observe(thread, false, inputs(ThreadStatus::Running, false));
        assert!(watch.last.is_empty());
    }
}
