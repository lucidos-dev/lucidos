use super::*;
use crate::runtime::AgentInput;
use crate::test_support::{setup_test_db, teardown_test_db};
use futures::{FutureExt, StreamExt};

fn sent(text: &str) -> AgentInput {
    AgentInput {
        text: text.into(),
        images: Vec::new(),
        uuid: Uuid::new_v4(),
    }
}

fn request(input_event_id: Uuid) -> (WithdrawInputRequest, oneshot::Receiver<InputWithdrawal>) {
    let (reply, answer) = oneshot::channel();
    let request = WithdrawInputRequest {
        input_event_id,
        actor: None,
        reply,
    };
    (request, answer)
}

async fn tombstones(pool: &sqlx::PgPool, thread_id: Uuid) -> Vec<String> {
    sqlx::query_scalar(
        "SELECT payload->>'removed_message_id' FROM events \
         WHERE thread_id = $1 AND event_type = 'QueuedMessageRemoved'",
    )
    .bind(thread_id)
    .fetch_all(pool)
    .await
    .unwrap()
}

/// A message the agent already read, or never got, is not taken back.
#[tokio::test]
async fn an_input_that_is_not_owed_is_already_read() {
    let (driver, mut driver_rx) = mpsc::unbounded_channel();
    let mut pending = PendingWithdraws::new();
    let (withdraw, answer) = request(Uuid::new_v4());
    begin_withdraw(&InputLedger::new(), Some(&driver), withdraw, &mut pending);
    assert_eq!(answer.await.unwrap(), InputWithdrawal::AlreadyRead);
    assert!(driver_rx.try_recv().is_err(), "Claude Code is never asked");
    assert!(pending.is_empty());
}

#[tokio::test]
async fn a_backend_without_withdraws_refuses() {
    let id = Uuid::new_v4();
    let mut inputs = InputLedger::new();
    inputs.forwarded(vec![id], &sent("queued"));
    let mut pending = PendingWithdraws::new();
    let (withdraw, answer) = request(id);
    begin_withdraw(&inputs, None, withdraw, &mut pending);
    assert_eq!(
        answer.await.unwrap(),
        InputWithdrawal::Refused(CODEX_UNSUPPORTED.to_string())
    );
}

/// A driver that is gone before it answers leaves the message where it was.
#[tokio::test]
async fn a_driver_that_ended_refuses() {
    let id = Uuid::new_v4();
    let mut inputs = InputLedger::new();
    inputs.forwarded(vec![id], &sent("queued"));
    let (driver, driver_rx) = mpsc::unbounded_channel();
    drop(driver_rx);
    let mut pending = PendingWithdraws::new();
    let (withdraw, _answer) = request(id);
    begin_withdraw(&inputs, Some(&driver), withdraw, &mut pending);
    let back = pending.next().await.expect("the answer lands");
    assert_eq!(
        back.outcome,
        InputWithdrawal::Refused(NO_LIVE_SESSION.to_string())
    );
}

/// The whole path: Claude Code is asked by the input's own uuid, and its
/// `cancelled: true` takes the input off the ledger and records the tombstone.
#[tokio::test]
async fn a_withdrawn_input_leaves_the_ledger_and_records_its_tombstone() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    let id = Uuid::new_v4();
    let queued = sent("never mind");
    let mut inputs = InputLedger::new();
    inputs.forwarded(vec![id], &queued);

    let (driver, mut driver_rx) = mpsc::unbounded_channel();
    let mut pending = PendingWithdraws::new();
    let (withdraw, answer) = request(id);
    begin_withdraw(&inputs, Some(&driver), withdraw, &mut pending);
    let asked: WithdrawRequest = driver_rx.recv().await.expect("Claude Code is asked");
    assert_eq!(asked.input_uuid, queued.uuid);
    asked.reply.send(InputWithdrawal::Withdrawn).unwrap();

    settle_answered_withdraws(&bus, thread_id, &mut inputs, &mut pending).await;
    assert_eq!(answer.await.unwrap(), InputWithdrawal::Withdrawn);
    assert_eq!(inputs.owed(), 0);
    assert_eq!(tombstones(&pool, thread_id).await, [id.to_string()]);

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Any answer but `cancelled: true` leaves the message owed and unhidden.
#[tokio::test]
async fn a_refused_withdraw_records_nothing() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    let id = Uuid::new_v4();
    let mut inputs = InputLedger::new();
    inputs.forwarded(vec![id], &sent("queued"));

    for outcome in [
        InputWithdrawal::AlreadyRead,
        InputWithdrawal::Refused("no".into()),
    ] {
        let (withdraw, answer) = request(id);
        let back = WithdrawAnswered {
            request: withdraw,
            outcome: outcome.clone(),
        };
        finish_withdraw(&bus, thread_id, &mut inputs, back).await;
        assert_eq!(answer.await.unwrap(), outcome);
    }
    assert_eq!(inputs.owed(), 1);
    assert!(tombstones(&pool, thread_id).await.is_empty());

    pool.close().await;
    teardown_test_db(&db_name).await;
}

fn sessions_with(session: AgentSession, thread_id: Uuid) -> Mutex<HashMap<Uuid, AgentSession>> {
    Mutex::new(HashMap::from([(thread_id, session)]))
}

const QUICK: Duration = Duration::from_secs(5);

#[tokio::test]
async fn a_thread_without_a_live_session_refuses() {
    let outcome = withdraw_from_session(
        &Mutex::new(HashMap::new()),
        Uuid::new_v4(),
        Uuid::new_v4(),
        None,
        QUICK,
    )
    .await;
    assert_eq!(
        outcome,
        InputWithdrawal::Refused(NO_LIVE_SESSION.to_string())
    );
}

#[tokio::test]
async fn a_codex_session_refuses() {
    let thread_id = Uuid::new_v4();
    let (mut session, _msg_rx) = AgentSession::for_test();
    session.coding_agent = CodingAgent::Codex;
    let outcome = withdraw_from_session(
        &sessions_with(session, thread_id),
        thread_id,
        Uuid::new_v4(),
        None,
        QUICK,
    )
    .await;
    assert_eq!(
        outcome,
        InputWithdrawal::Refused(CODEX_UNSUPPORTED.to_string())
    );
}

/// The request reaches the session's run loop, and the loop's answer is the
/// caller's answer.
#[tokio::test]
async fn a_claude_code_session_answers_through_its_run_loop() {
    let thread_id = Uuid::new_v4();
    let message = Uuid::new_v4();
    let (mut session, _msg_rx) = AgentSession::for_test();
    let (withdraw_tx, mut withdraw_rx) = mpsc::unbounded_channel();
    session.withdraw_tx = withdraw_tx;
    let run_loop = tokio::spawn(async move {
        let request: WithdrawInputRequest = withdraw_rx.recv().await.unwrap();
        assert_eq!(request.input_event_id, message);
        request.reply.send(InputWithdrawal::Withdrawn).unwrap();
    });
    let outcome = withdraw_from_session(
        &sessions_with(session, thread_id),
        thread_id,
        message,
        None,
        QUICK,
    )
    .await;
    assert_eq!(outcome, InputWithdrawal::Withdrawn);
    run_loop.await.unwrap();
}

/// A run loop that ends with the request unanswered refuses rather than hangs.
#[tokio::test]
async fn a_run_loop_that_ends_first_refuses() {
    let thread_id = Uuid::new_v4();
    let (mut session, _msg_rx) = AgentSession::for_test();
    let (withdraw_tx, mut withdraw_rx) = mpsc::unbounded_channel::<WithdrawInputRequest>();
    session.withdraw_tx = withdraw_tx;
    let run_loop = tokio::spawn(async move { drop(withdraw_rx.recv().await) });
    let outcome = withdraw_from_session(
        &sessions_with(session, thread_id),
        thread_id,
        Uuid::new_v4(),
        None,
        QUICK,
    )
    .await;
    assert_eq!(
        outcome,
        InputWithdrawal::Refused(NO_LIVE_SESSION.to_string())
    );
    run_loop.await.unwrap();
}

/// A withdrawal whose tombstone failed to record is retried from the ledger,
/// without asking Claude Code again: it already dropped the message.
#[tokio::test]
async fn a_withdrawal_whose_tombstone_failed_records_it_on_retry() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    let id = Uuid::new_v4();
    let mut inputs = InputLedger::new();
    inputs.forwarded(vec![id], &sent("queued"));
    inputs.withdrawn(id);

    let (driver, mut driver_rx) = mpsc::unbounded_channel();
    let mut pending = PendingWithdraws::new();
    let (withdraw, answer) = request(id);
    begin_withdraw(&inputs, Some(&driver), withdraw, &mut pending);
    assert!(
        driver_rx.try_recv().is_err(),
        "Claude Code is not asked again"
    );
    settle_answered_withdraws(&bus, thread_id, &mut inputs, &mut pending).await;
    assert_eq!(answer.await.unwrap(), InputWithdrawal::Withdrawn);
    assert!(!inputs.awaits_tombstone(id));
    assert_eq!(tombstones(&pool, thread_id).await, [id.to_string()]);

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// The read-ordering race: Claude Code drops A, then reads B. A's answer lands
/// before B's replay reaches the run loop. So settling answered withdraws first
/// leaves B's read for B, never for A.
#[tokio::test]
async fn a_read_after_a_drop_settles_the_input_it_names() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    let [a, b] = [Uuid::new_v4(), Uuid::new_v4()];
    let mut inputs = InputLedger::new();
    inputs.forwarded(vec![a], &sent("never mind"));
    inputs.forwarded(vec![b], &sent("check the totals"));

    let (driver, mut driver_rx) = mpsc::unbounded_channel();
    let mut pending = PendingWithdraws::new();
    let (withdraw, _answer) = request(a);
    begin_withdraw(&inputs, Some(&driver), withdraw, &mut pending);
    // Polled once, as the select arm does, so it waits on the answer.
    assert!(pending.next().now_or_never().is_none());
    let asked: WithdrawRequest = driver_rx.recv().await.unwrap();
    asked.reply.send(InputWithdrawal::Withdrawn).unwrap();

    // B's replay arrives next. The run loop settles answers before it.
    settle_answered_withdraws(&bus, thread_id, &mut inputs, &mut pending).await;
    let replay = crate::runtime::AgentEvent::InputRead(Some(crate::runtime::ReplayedInput {
        texts: vec!["check the totals".into()],
        images: 0,
    }));
    assert_eq!(inputs.observe(&replay), vec![b]);
    assert_eq!(tombstones(&pool, thread_id).await, [a.to_string()]);

    pool.close().await;
    teardown_test_db(&db_name).await;
}
