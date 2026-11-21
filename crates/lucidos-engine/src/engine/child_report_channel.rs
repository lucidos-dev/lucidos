//! Session channels for taking and observing unread child reports.

use std::time::Duration;

use tokio::sync::{mpsc, oneshot, watch};
use uuid::Uuid;

use super::thread_events::MessageOrigin;
use super::LucidosEngine;

/// A child report the agent has not read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChildReport {
    /// The `ChildThreadCompleted` card that carried it.
    pub card_id: Uuid,
    /// The report, as a take hands it over.
    pub text: String,
    /// What the agent was sent for it, notes included. A report that goes
    /// back to the agent goes as this.
    pub sent: String,
}

/// A request for the run loop to take every waiting child report.
#[derive(Debug)]
pub struct TakeChildReportsRequest {
    /// The agent that asked, for each `ChildReportTaken`.
    pub actor: Option<MessageOrigin>,
    /// The reports taken, oldest first.
    pub reply: oneshot::Sender<Vec<ChildReport>>,
}

/// The session's end: asks the run loop for a take, and watches how many
/// reports wait unread.
#[derive(Debug, Clone)]
pub struct ChildReportsHandle {
    pub(crate) take_tx: mpsc::UnboundedSender<TakeChildReportsRequest>,
    pub waiting_rx: watch::Receiver<usize>,
}

/// The run loop's end.
#[derive(Debug)]
pub struct ChildReportsLoopEnd {
    pub take_rx: mpsc::UnboundedReceiver<TakeChildReportsRequest>,
    /// Takes whose withdraws have all answered, for the run loop to settle.
    pub settled_rx: mpsc::UnboundedReceiver<WithdrawnReports>,
    pub settled_tx: mpsc::UnboundedSender<WithdrawnReports>,
    waiting_tx: watch::Sender<usize>,
}

/// The reports one take withdrew from the driver. They stay owed in the ledger
/// until `settle_take` hands them to the caller or sends them again.
#[derive(Debug)]
pub struct WithdrawnReports {
    pub request: TakeChildReportsRequest,
    pub reports: Vec<ChildReport>,
}

pub fn child_reports_channel() -> (ChildReportsHandle, ChildReportsLoopEnd) {
    let (take_tx, take_rx) = mpsc::unbounded_channel();
    let (settled_tx, settled_rx) = mpsc::unbounded_channel();
    let (waiting_tx, waiting_rx) = watch::channel(0);
    (
        ChildReportsHandle {
            take_tx,
            waiting_rx,
        },
        ChildReportsLoopEnd {
            take_rx,
            settled_rx,
            settled_tx,
            waiting_tx,
        },
    )
}

impl ChildReportsLoopEnd {
    /// Publish how many reports `inputs` holds unread. Call after every
    /// ledger change.
    pub fn publish(&self, count: usize) {
        self.waiting_tx
            .send_if_modified(|waiting| std::mem::replace(waiting, count) != count);
    }
}

/// How long a take waits for the run loop and the driver. Both answer at once;
/// the slack covers a run loop busy with another event.
const TAKE_TIMEOUT: Duration = Duration::from_secs(15);

impl LucidosEngine {
    /// Take every child report the thread's agent has not read, oldest first.
    /// Empty when no live session holds one.
    pub async fn take_child_reports(
        &self,
        thread_id: Uuid,
        actor: Option<MessageOrigin>,
    ) -> Vec<ChildReport> {
        let Some(handle) = self.child_reports_handle(thread_id).await else {
            return Vec::new();
        };
        let (reply, answer) = oneshot::channel();
        if handle
            .take_tx
            .send(TakeChildReportsRequest { actor, reply })
            .is_err()
        {
            return Vec::new();
        }
        match tokio::time::timeout(TAKE_TIMEOUT, answer).await {
            Ok(Ok(taken)) => taken,
            Ok(Err(_loop_ended)) => Vec::new(),
            Err(_) => {
                crate::log_legacy!(
                    "[ChildReports] take for thread {} not answered within {}s",
                    thread_id,
                    TAKE_TIMEOUT.as_secs()
                );
                Vec::new()
            }
        }
    }

    /// How many child reports wait unread on the thread's live session, as a
    /// watch. `None` when no live session runs.
    pub async fn child_reports_waiting(&self, thread_id: Uuid) -> Option<watch::Receiver<usize>> {
        self.child_reports_handle(thread_id)
            .await
            .map(|handle| handle.waiting_rx)
    }

    async fn child_reports_handle(&self, thread_id: Uuid) -> Option<ChildReportsHandle> {
        self.agent_sessions
            .lock()
            .await
            .get(&thread_id)
            .filter(|session| session.is_live())
            .map(|session| session.child_reports.clone())
    }
}
