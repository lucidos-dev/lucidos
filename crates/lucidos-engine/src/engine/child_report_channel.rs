use std::time::Duration;

use tokio::sync::{mpsc, oneshot, watch};
use uuid::Uuid;

use super::thread_events::MessageOrigin;
use super::LucidosEngine;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChildReport {
    pub card_id: Uuid,
    pub text: String,
    pub sent: String,
}

#[derive(Debug)]
pub struct TakeChildReportsRequest {
    pub actor: Option<MessageOrigin>,
    pub reply: oneshot::Sender<Vec<ChildReport>>,
}

#[derive(Debug, Clone)]
pub struct ChildReportsHandle {
    pub(crate) take_tx: mpsc::UnboundedSender<TakeChildReportsRequest>,
    pub waiting_rx: watch::Receiver<usize>,
}

#[derive(Debug)]
pub struct ChildReportsLoopEnd {
    pub take_rx: mpsc::UnboundedReceiver<TakeChildReportsRequest>,
    pub settled_rx: mpsc::UnboundedReceiver<WithdrawnReports>,
    pub settled_tx: mpsc::UnboundedSender<WithdrawnReports>,
    waiting_tx: watch::Sender<usize>,
}

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
    /// INVARIANT: Publish how many reports `inputs` holds unread. Call after every
    /// ledger change.
    pub fn publish(&self, count: usize) {
        self.waiting_tx
            .send_if_modified(|waiting| std::mem::replace(waiting, count) != count);
    }
}

const TAKE_TIMEOUT: Duration = Duration::from_secs(15);

impl LucidosEngine {
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
