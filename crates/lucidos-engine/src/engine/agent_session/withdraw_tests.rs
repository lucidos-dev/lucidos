use super::*;
use crate::engine::agent_session::input_ledger::{ReadInput, Settled};
use crate::runtime::{AgentInput, CodingAgent};
use crate::test_support::{setup_test_db, teardown_test_db};
use futures::{FutureExt, StreamExt};

fn sent(text: &str) -> AgentInput {
    AgentInput {
        text: text.into(),
        images: Vec::new(),
        uuid: Uuid::new_v4(),
    }
}

/// A driver the withdraw never reaches: the store answers first.
fn unused_driver() -> mpsc::UnboundedSender<WithdrawRequest> {
    mpsc::unbounded_channel().0
}

fn request(input_event_id: Uuid) -> (WithdrawInputRequest, oneshot::Receiver<InputWithdrawal>) {
    let (reply, answer) = oneshot::channel();
    let request = WithdrawInputRequest {
        input_event_id,
        actor: None,
        purpose: WithdrawPurpose::Remove,
        reply,
    };
    (request, answer)
}

/// A pool for a test the store never answers, so it never connects.
fn unused_pool() -> sqlx::PgPool {
    sqlx::postgres::PgPoolOptions::new()
        .connect_lazy("postgres://unused@127.0.0.1:1/unused")
        .unwrap()
}

async fn reads(pool: &sqlx::PgPool, thread_id: Uuid) -> i64 {
    sqlx::query_scalar(
        "SELECT COUNT(*) FROM events \
         WHERE thread_id = $1 AND event_type = 'CodingAgentInputRead'",
    )
    .bind(thread_id)
    .fetch_one(pool)
    .await
    .unwrap()
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

/// A message this session's agent already read is not taken back, and the
/// store is never asked.
#[tokio::test]
async fn an_input_this_session_read_is_already_read() {
    let id = Uuid::new_v4();
    let queued = sent("queued");
    let mut inputs = InputLedger::new();
    inputs.forwarded(vec![id], &queued, None);
    inputs.observe(&crate::runtime::AgentEvent::InputRead { input: queued.uuid });
    let (driver, mut driver_rx) = mpsc::unbounded_channel();
    let mut pending = PendingWithdraws::new();
    let (withdraw, answer) = request(id);
    begin_withdraw(
        &inputs,
        &driver,
        &unused_pool(),
        Uuid::new_v4(),
        withdraw,
        &mut pending,
    );
    assert_eq!(answer.await.unwrap(), InputWithdrawal::AlreadyRead);
    assert!(driver_rx.try_recv().is_err(), "Claude Code is never asked");
    assert!(pending.is_empty());
}

/// The reported bug: the previous process took the follow-up into its queue
/// and died. This session never forwarded it, so no live process holds it,
/// and the remove records the tombstone. It records no read.
#[tokio::test]
async fn an_unread_input_this_session_never_forwarded_is_withdrawn() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    let id = Uuid::new_v4();
    forwarded_in_the_store(&bus, thread_id, id).await;
    let mut inputs = InputLedger::new();
    let (driver, mut driver_rx) = mpsc::unbounded_channel();
    let mut pending = PendingWithdraws::new();
    let (withdraw, answer) = request(id);
    begin_withdraw(&inputs, &driver, &pool, thread_id, withdraw, &mut pending);
    let back = pending.next().await.expect("the store answers");
    finish_withdraw(&bus, thread_id, &mut inputs, back).await;

    assert_eq!(answer.await.unwrap(), InputWithdrawal::Withdrawn);
    assert!(driver_rx.try_recv().is_err(), "Claude Code is never asked");
    assert_eq!(tombstones(&pool, thread_id).await, [id.to_string()]);
    assert_eq!(reads(&pool, thread_id).await, 0);

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// A message an earlier session's agent read stays read.
#[tokio::test]
async fn an_input_an_earlier_session_read_is_already_read() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    let id = Uuid::new_v4();
    read_in_the_store(&bus, thread_id, id).await;
    let mut inputs = InputLedger::new();
    let mut pending = PendingWithdraws::new();
    let (withdraw, answer) = request(id);
    begin_withdraw(
        &inputs,
        &unused_driver(),
        &pool,
        thread_id,
        withdraw,
        &mut pending,
    );
    let back = pending.next().await.expect("the store answers");
    finish_withdraw(&bus, thread_id, &mut inputs, back).await;

    assert_eq!(answer.await.unwrap(), InputWithdrawal::AlreadyRead);
    assert!(tombstones(&pool, thread_id).await.is_empty());

    pool.close().await;
    teardown_test_db(&db_name).await;
}

async fn read_in_the_store(bus: &EventBus, thread_id: Uuid, id: Uuid) {
    forwarded_in_the_store(bus, thread_id, id).await;
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::CodingAgentInputRead {
            input_event_id: id,
            started_turn: false,
            verbatim: false,
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();
}

/// A session forwarded `id`'s message to its agent.
async fn forwarded_in_the_store(bus: &EventBus, thread_id: Uuid, id: Uuid) {
    crate::test_support::start_cc_session(bus, thread_id, "work", None).await;
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::CodingAgentPromptSent {
            text: "forwarded".into(),
            coding_agent: CodingAgent::ClaudeCode,
            origin: None,
            input_event_id: Some(id),
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();
}

/// A driver that is gone before it answers leaves the message where it was.
#[tokio::test]
async fn a_driver_that_ended_refuses() {
    let id = Uuid::new_v4();
    let mut inputs = InputLedger::new();
    inputs.forwarded(vec![id], &sent("queued"), None);
    let (driver, driver_rx) = mpsc::unbounded_channel();
    drop(driver_rx);
    let mut pending = PendingWithdraws::new();
    let (withdraw, _answer) = request(id);
    begin_withdraw(
        &inputs,
        &driver,
        &unused_pool(),
        Uuid::new_v4(),
        withdraw,
        &mut pending,
    );
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
    inputs.forwarded(vec![id], &queued, None);

    let (driver, mut driver_rx) = mpsc::unbounded_channel();
    let mut pending = PendingWithdraws::new();
    let (withdraw, answer) = request(id);
    begin_withdraw(&inputs, &driver, &pool, thread_id, withdraw, &mut pending);
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
    inputs.forwarded(vec![id], &sent("queued"), None);

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

/// No run loop takes the request, so the caller falls back to the claim.
#[tokio::test]
async fn a_thread_without_a_live_session_has_no_run_loop_to_ask() {
    let outcome = withdraw_from_session(
        &Mutex::new(HashMap::new()),
        Uuid::new_v4(),
        Uuid::new_v4(),
        None,
        QUICK,
    )
    .await;
    assert_eq!(outcome, None);
}

/// The request reaches the session's run loop, and the loop's answer is the
/// caller's answer, for either coding agent.
#[tokio::test]
async fn a_session_answers_through_its_run_loop() {
    for coding_agent in [CodingAgent::ClaudeCode, CodingAgent::Codex] {
        let thread_id = Uuid::new_v4();
        let message = Uuid::new_v4();
        let (mut session, _msg_rx) = AgentSession::for_test();
        session.coding_agent = coding_agent;
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
        assert_eq!(
            outcome,
            Some(InputWithdrawal::Withdrawn),
            "{coding_agent:?}"
        );
        run_loop.await.unwrap();
    }
}

/// A run loop that ends with the request unanswered does not hang. Its process
/// is gone, so the caller falls back to the claim.
#[tokio::test]
async fn a_run_loop_that_ends_first_leaves_it_to_the_claim() {
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
    assert_eq!(outcome, None);
    run_loop.await.unwrap();
}

/// Claim `thread_id` the way the engine does, from `sessions`.
fn claim(
    sessions: &mut HashMap<Uuid, AgentSession>,
    thread_id: Uuid,
) -> std::future::Ready<
    Result<Option<crate::engine::agent_session::spawn_claim::ClaimedEntry>, String>,
> {
    let claimed = crate::engine::agent_session::spawn_claim::claim_thread_for(
        sessions,
        thread_id,
        CodingAgent::ClaudeCode,
        crate::engine::agent_session::spawn_claim::ClaimFor::Run,
    );
    std::future::ready(Ok(claimed.ok()))
}

/// An idle thread has no process to ask. The remove claims the thread, finds no
/// read, and records the tombstone. It records no read.
#[tokio::test]
async fn an_idle_thread_records_the_tombstone_under_the_claim() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    let id = Uuid::new_v4();
    forwarded_in_the_store(&bus, thread_id, id).await;
    let mut sessions = HashMap::new();
    let outcome =
        withdraw_while_claimed(claim(&mut sessions, thread_id), &bus, thread_id, id, None).await;
    assert_eq!(outcome, Some(InputWithdrawal::Withdrawn));
    assert_eq!(tombstones(&pool, thread_id).await, [id.to_string()]);
    assert_eq!(reads(&pool, thread_id).await, 0);

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn an_idle_thread_keeps_a_message_its_agent_read() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    let id = Uuid::new_v4();
    read_in_the_store(&bus, thread_id, id).await;
    let mut sessions = HashMap::new();
    let outcome =
        withdraw_while_claimed(claim(&mut sessions, thread_id), &bus, thread_id, id, None).await;
    assert_eq!(outcome, Some(InputWithdrawal::AlreadyRead));
    assert!(tombstones(&pool, thread_id).await.is_empty());

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// A message no session ever forwarded may be waiting on a spawn's claim. The
/// remove writes nothing, or that spawn would deliver it over its tombstone.
#[tokio::test]
async fn a_message_no_session_forwarded_is_not_taken_back() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    let id = Uuid::new_v4();
    let mut sessions = HashMap::new();
    let outcome =
        withdraw_while_claimed(claim(&mut sessions, thread_id), &bus, thread_id, id, None).await;
    assert_eq!(
        outcome,
        Some(InputWithdrawal::Refused(NOT_YET_FORWARDED.to_string()))
    );
    assert!(tombstones(&pool, thread_id).await.is_empty());

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// A run that holds the thread could forward the message at any moment. So
/// the remove writes nothing and leaves the answer to that run's loop.
#[tokio::test]
async fn a_held_claim_leaves_the_remove_to_the_run_loop() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    let id = Uuid::new_v4();
    let mut sessions = HashMap::new();
    let _run = claim(&mut sessions, thread_id)
        .into_inner()
        .unwrap()
        .expect("the run claims");
    let outcome =
        withdraw_while_claimed(claim(&mut sessions, thread_id), &bus, thread_id, id, None).await;
    assert_eq!(outcome, None);
    assert!(tombstones(&pool, thread_id).await.is_empty());

    pool.close().await;
    teardown_test_db(&db_name).await;
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
    inputs.forwarded(vec![id], &sent("queued"), None);
    inputs.withdrawn(id);

    let (driver, mut driver_rx) = mpsc::unbounded_channel();
    let mut pending = PendingWithdraws::new();
    let (withdraw, answer) = request(id);
    begin_withdraw(&inputs, &driver, &pool, thread_id, withdraw, &mut pending);
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
    inputs.forwarded(vec![a], &sent("never mind"), None);
    let b_input = sent("check the totals");
    let b_uuid = b_input.uuid;
    inputs.forwarded(vec![b], &b_input, None);

    let (driver, mut driver_rx) = mpsc::unbounded_channel();
    let mut pending = PendingWithdraws::new();
    let (withdraw, _answer) = request(a);
    begin_withdraw(&inputs, &driver, &pool, thread_id, withdraw, &mut pending);
    // Polled once, as the select arm does, so it waits on the answer.
    assert!(pending.next().now_or_never().is_none());
    let asked: WithdrawRequest = driver_rx.recv().await.unwrap();
    asked.reply.send(InputWithdrawal::Withdrawn).unwrap();

    // B's replay arrives next. The run loop settles answers before it.
    settle_answered_withdraws(&bus, thread_id, &mut inputs, &mut pending).await;
    let replay = crate::runtime::AgentEvent::InputRead { input: b_uuid };
    assert_eq!(
        inputs.observe(&replay),
        Settled::Read(vec![ReadInput {
            event_id: b,
            verbatim: false
        }])
    );
    assert_eq!(tombstones(&pool, thread_id).await, [a.to_string()]);

    pool.close().await;
    teardown_test_db(&db_name).await;
}
