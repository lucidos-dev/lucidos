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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Snapshot {
    pub status: ThreadStatus,
    pub watching: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum QuestionMove {
    Asked(String),
    Ended,
    ResultOf(String),
    Untouched,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum QuestionPark {
    Open { tool_use_id: String },
    Closed,
}

impl QuestionMove {
    /// INVARIANT: The same reading as the gate's unanswered-question predicate.
    fn of(event: &ThreadEvent) -> Self {
        if let ThreadEvent::UserQuestionAsked { tool_use_id, .. } = event {
            Self::Asked(tool_use_id.clone())
        } else if let ThreadEvent::CodingAgentToolResult { tool_use_id, .. } = event {
            Self::ResultOf(tool_use_id.clone())
        } else if crate::engine::question_park_sql::ends_question_park(event.event_type()) {
            Self::Ended
        } else {
            Self::Untouched
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct GateInputs {
    snapshot: Snapshot,
    question_open: Option<bool>,
}

#[derive(Debug, Default)]
pub(crate) struct PendingChangeWatch {
    last: HashMap<Uuid, GateInputs>,
    /// INVARIANT: Each thread's question park, where known. An open one is kept for any
    /// thread, since a question can open before the proposal. A closed one is
    /// kept only while the thread holds a pending change.
    questions: HashMap<Uuid, QuestionPark>,
}

impl PendingChangeWatch {
    pub(crate) fn observe(&mut self, thread_id: Uuid, proposed: bool, snapshot: Snapshot) -> bool {
        if !proposed {
            self.last.remove(&thread_id);
            if self.questions.get(&thread_id) == Some(&QuestionPark::Closed) {
                self.questions.remove(&thread_id);
            }
            return false;
        }
        let inputs = GateInputs {
            snapshot,
            question_open: self
                .questions
                .get(&thread_id)
                .map(|park| matches!(park, QuestionPark::Open { .. })),
        };
        self.last.insert(thread_id, inputs) != Some(inputs)
    }

    pub(crate) fn note_question(&mut self, thread_id: Uuid, question: QuestionMove) {
        let park = match question {
            QuestionMove::Asked(tool_use_id) => QuestionPark::Open { tool_use_id },
            QuestionMove::Ended => QuestionPark::Closed,
            QuestionMove::ResultOf(result_id) => match self.questions.get(&thread_id) {
                Some(QuestionPark::Open { tool_use_id }) if *tool_use_id == result_id => {
                    QuestionPark::Closed
                }
                _ => return,
            },
            QuestionMove::Untouched => return,
        };
        self.questions.insert(thread_id, park);
    }

    pub(crate) fn forget_all(&mut self) {
        self.last.clear();
        self.questions.clear();
    }
}

/// INVARIANT: Spawn the consumer. Returns the `JoinHandle` so a caller can observe panics.
pub fn spawn(engine: Arc<LucidosEngine>) -> tokio::task::JoinHandle<()> {
    let rx = engine.event_bus.subscribe();
    tokio::spawn(watch(rx, move || {
        let engine = engine.clone();
        async move { engine.broadcast_changes_updated().await }
    }))
}

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
            // INVARIANT: A lag may have hidden a shift, so rebuild rather than guess. The
            // cache is stale too, and a thread back in its cached state would
            // read as unchanged.
            Err(BroadcastStreamRecvError::Lagged(n)) => {
                log_legacy!(
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
        let mut watch = PendingChangeWatch::default();
        let thread = Uuid::new_v4();
        watch.note_question(thread, QuestionMove::Asked("toolu_q".into()));
        watch.observe(thread, true, inputs(ThreadStatus::Idle, false));
        watch.note_question(thread, QuestionMove::Untouched);
        assert!(!watch.observe(thread, true, inputs(ThreadStatus::Idle, false)));
        watch.note_question(thread, QuestionMove::Ended);
        assert!(watch.observe(thread, true, inputs(ThreadStatus::Idle, false)));
    }

    #[test]
    fn a_question_end_after_a_lag_rebuilds_the_frame_once() {
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
    fn only_the_open_questions_own_result_ends_the_park() {
        let mut watch = PendingChangeWatch::default();
        let thread = Uuid::new_v4();
        watch.note_question(thread, QuestionMove::Asked("toolu_q".into()));
        watch.observe(thread, true, inputs(ThreadStatus::Idle, false));
        watch.note_question(thread, QuestionMove::ResultOf("toolu_earlier".into()));
        assert!(!watch.observe(thread, true, inputs(ThreadStatus::Idle, false)));
        watch.note_question(thread, QuestionMove::ResultOf("toolu_q".into()));
        assert!(watch.observe(thread, true, inputs(ThreadStatus::Idle, false)));
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
            turn_end: None,
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
        let mut watch = PendingChangeWatch::default();
        let thread = Uuid::new_v4();
        assert!(watch.observe(thread, true, inputs(ThreadStatus::Idle, false)));
        assert!(!watch.observe(thread, true, inputs(ThreadStatus::Idle, false)));
    }

    #[test]
    fn a_thread_back_in_its_cached_state_after_a_lag_rebuilds_the_frame() {
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
