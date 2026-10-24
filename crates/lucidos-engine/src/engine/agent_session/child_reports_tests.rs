use super::*;
use crate::engine::agent_session::withdraw::finish_withdraw;
use crate::test_support::{setup_test_db, start_cc_session, teardown_test_db};
use futures::StreamExt;

fn sent(text: &str) -> AgentInput {
    AgentInput {
        text: text.into(),
        images: Vec::new(),
        uuid: Uuid::new_v4(),
    }
}

async fn count(pool: &sqlx::PgPool, thread_id: Uuid, event_type: &str) -> Vec<String> {
    sqlx::query_scalar("SELECT payload::text FROM events WHERE thread_id = $1 AND event_type = $2")
        .bind(thread_id)
        .bind(event_type)
        .fetch_all(pool)
        .await
        .unwrap()
}

/// A pool for a test the store never answers, so it never connects.
fn unused_pool() -> sqlx::PgPool {
    sqlx::postgres::PgPoolOptions::new()
        .connect_lazy("postgres://unused@127.0.0.1:1/unused")
        .unwrap()
}

/// One session's run loop, as far as a take reaches.
struct Session {
    bus: EventBus,
    pool: sqlx::PgPool,
    db_name: String,
    thread_id: Uuid,
    inputs: InputLedger,
    driver: mpsc::UnboundedSender<WithdrawRequest>,
    driver_rx: mpsc::UnboundedReceiver<WithdrawRequest>,
    agent_tx: mpsc::UnboundedSender<AgentInput>,
    agent_rx: mpsc::UnboundedReceiver<AgentInput>,
    loop_end: ChildReportsLoopEnd,
    pending: PendingWithdraws,
}

impl Session {
    async fn new() -> Self {
        let (pool, db_name) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let thread_id = Uuid::new_v4();
        start_cc_session(&bus, thread_id, "work", None).await;
        let (driver, driver_rx) = mpsc::unbounded_channel();
        let (agent_tx, agent_rx) = mpsc::unbounded_channel();
        Self {
            bus,
            pool,
            db_name,
            thread_id,
            inputs: InputLedger::new(),
            driver,
            driver_rx,
            agent_tx,
            agent_rx,
            loop_end: child_reports_channel().1,
            pending: PendingWithdraws::new(),
        }
    }

    /// Start a take, as the run loop's take arm does.
    fn take(&mut self) -> oneshot::Receiver<Vec<ChildReport>> {
        let (reply, taken) = oneshot::channel();
        begin_take(
            &self.inputs,
            &self.driver,
            &self.loop_end,
            &self.pool,
            self.thread_id,
            TakeChildReportsRequest { actor: None, reply },
            &mut self.pending,
        );
        taken
    }

    /// The driver answers the next withdraw, and the run loop settles it.
    async fn driver_answers(&mut self, expected: Uuid, outcome: InputWithdrawal) {
        let withdraw = self.driver_rx.recv().await.unwrap();
        assert_eq!(withdraw.input_uuid, expected);
        withdraw.reply.send(outcome).unwrap();
        let answered = self.pending.next().await.unwrap();
        finish_withdraw(&self.bus, self.thread_id, &mut self.inputs, answered).await;
    }

    /// The take's settle arm, once all its withdraws answered.
    async fn settle(&mut self) -> Vec<(Uuid, Uuid)> {
        let withdrawn =
            tokio::time::timeout(Duration::from_secs(10), self.loop_end.settled_rx.recv())
                .await
                .expect("settled within 10s")
                .expect("loop open");
        settle_take(
            &self.bus,
            self.thread_id,
            &mut self.inputs,
            &self.agent_tx,
            withdrawn,
        )
        .await
    }

    async fn close(self) {
        self.pool.close().await;
        teardown_test_db(&self.db_name).await;
    }
}

/// A take asks the driver for each report alone. It hands back only what the
/// driver dropped: a report the agent already read stays where it is.
#[tokio::test]
async fn a_take_hands_back_only_the_reports_the_driver_dropped() {
    let mut s = Session::new().await;
    let (message, card_a, card_b) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
    let (follow_up, report_a, report_b) = (sent("also"), sent("[a]"), sent("[b]"));
    s.inputs.forwarded(vec![message], &follow_up, None);
    s.inputs
        .forwarded_child_report(card_a, &report_a, None, "report a".into());
    s.inputs
        .forwarded_child_report(card_b, &report_b, None, "report b".into());

    let taken = s.take();
    s.driver_answers(report_a.uuid, InputWithdrawal::Withdrawn)
        .await;
    s.driver_answers(report_b.uuid, InputWithdrawal::AlreadyRead)
        .await;
    assert!(
        s.driver_rx.try_recv().is_err(),
        "only reports are taken, never the follow-up"
    );
    assert!(
        s.inputs.owes(report_a.uuid),
        "a withdrawn report stays owed until the take settles"
    );
    assert!(
        s.settle().await.is_empty(),
        "a received take resends nothing"
    );

    assert_eq!(
        taken.await.unwrap(),
        vec![ChildReport {
            card_id: card_a,
            text: "report a".into(),
            sent: "[a]".into(),
        }]
    );
    assert!(s.inputs.owes(follow_up.uuid));
    assert!(
        s.inputs.owes(report_b.uuid),
        "a read report stays owed until read"
    );
    assert!(!s.inputs.owes(report_a.uuid));
    assert!(
        s.agent_rx.try_recv().is_err(),
        "nothing goes back to the agent"
    );
    let recorded = count(&s.pool, s.thread_id, "ChildReportTaken").await;
    assert_eq!(recorded.len(), 1);
    assert!(recorded[0].contains(&card_a.to_string()));
    assert!(count(&s.pool, s.thread_id, "QueuedMessageRemoved")
        .await
        .is_empty());
    s.close().await;
}

/// A caller that leaves while the withdraw is in flight loses nothing: the
/// report goes back to the agent and stays owed, and no take is recorded.
#[tokio::test]
async fn a_report_taken_after_its_caller_left_goes_back_to_the_agent() {
    let mut s = Session::new().await;
    let card = Uuid::new_v4();
    let report = sent("[a]");
    s.inputs
        .forwarded_child_report(card, &report, None, "report a".into());

    drop(s.take());
    s.driver_answers(report.uuid, InputWithdrawal::Withdrawn)
        .await;
    let moved = s.settle().await;

    let resent = s.agent_rx.try_recv().expect("the report goes back");
    assert_eq!(
        moved,
        vec![(report.uuid, resent.uuid)],
        "state keyed by the write follows it"
    );
    assert_eq!(
        resent.text, "[a]",
        "it goes back as first sent, notes included"
    );
    assert_ne!(
        resent.uuid, report.uuid,
        "a fresh write, never a reused uuid"
    );
    assert!(s.inputs.owes(resent.uuid));
    assert_eq!(
        s.inputs.child_reports(),
        vec![ChildReport {
            card_id: card,
            text: "report a".into(),
            sent: "[a]".into(),
        }],
        "still a report the agent may take"
    );
    assert!(count(&s.pool, s.thread_id, "ChildReportTaken")
        .await
        .is_empty());
    s.close().await;
}

/// A take with nothing waiting answers at once, and asks the driver nothing.
#[tokio::test]
async fn a_take_with_nothing_waiting_answers_empty() {
    let inputs = InputLedger::new();
    let (driver, mut driver_rx) = mpsc::unbounded_channel::<WithdrawRequest>();
    let mut loop_end = child_reports_channel().1;
    let mut pending = PendingWithdraws::new();
    let (reply, taken) = oneshot::channel();
    begin_take(
        &inputs,
        &driver,
        &loop_end,
        &unused_pool(),
        Uuid::new_v4(),
        TakeChildReportsRequest { actor: None, reply },
        &mut pending,
    );
    let withdrawn = loop_end.settled_rx.recv().await.unwrap();
    assert!(withdrawn.reports.is_empty());
    withdrawn.request.reply.send(withdrawn.reports).unwrap();
    assert!(taken.await.unwrap().is_empty());
    assert!(driver_rx.try_recv().is_err());
    assert!(pending.is_empty());
}

/// A take whose caller already gave up withdraws nothing, so no report is
/// taken with nobody left to read it.
#[tokio::test]
async fn a_take_nobody_waits_for_withdraws_nothing() {
    let mut inputs = InputLedger::new();
    let card = Uuid::new_v4();
    inputs.forwarded_child_report(card, &sent("[a]"), None, "report".into());
    let (driver, mut driver_rx) = mpsc::unbounded_channel::<WithdrawRequest>();
    let loop_end = child_reports_channel().1;
    let mut pending = PendingWithdraws::new();
    let (reply, taken) = oneshot::channel();
    drop(taken);
    begin_take(
        &inputs,
        &driver,
        &loop_end,
        &unused_pool(),
        Uuid::new_v4(),
        TakeChildReportsRequest { actor: None, reply },
        &mut pending,
    );
    assert!(driver_rx.try_recv().is_err());
    assert!(pending.is_empty());
    assert_eq!(inputs.owed_child_reports(), 1, "the report stays queued");
}

/// The watch moves with the ledger, and only when the count changes.
#[test]
fn publish_tracks_the_owed_reports() {
    let (handle, loop_end) = child_reports_channel();
    let mut inputs = InputLedger::new();
    let card = Uuid::new_v4();
    let report = sent("[a]");
    inputs.forwarded_child_report(card, &report, None, "report".into());
    loop_end.publish(&inputs);
    assert_eq!(*handle.waiting_rx.borrow(), 1);
    inputs.child_report_taken(card);
    loop_end.publish(&inputs);
    assert_eq!(*handle.waiting_rx.borrow(), 0);
}
