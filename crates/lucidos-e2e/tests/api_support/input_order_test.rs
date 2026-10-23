//! A page's inputs to a thread are ordered by the engine (ADR 0449).
//!
//! Each send carries the page's `input_chain` and an `input_seq`. The engine
//! admits it under the thread append lock before the ack. It refuses one that
//! a later send from the same chain overtook, and records every input in the
//! order it admitted them.
//!
//! The two race tests use the thread append lock itself as their sync point.
//! The test takes it from its own connection, then waits until `pg_locks` shows
//! the engine waiting on it. No test here sleeps to make a race happen.

use crate::support::{base_url, db_url, user_client};
use lucidos_engine::engine::event_bus::THREAD_APPEND_LOCK_CLASS;
use uuid::Uuid;

fn chat_url() -> String {
    format!("{}/api/v1/chat/stream", base_url())
}

async fn pool() -> sqlx::PgPool {
    sqlx::PgPool::connect(&db_url())
        .await
        .expect("Failed to connect to E2E workspace database")
}

/// One input on `chain`, as the frontend sends it.
fn input(
    thread_id: Uuid,
    chain: Uuid,
    seq: i64,
    event_id: Uuid,
    text: &str,
    new_thread: bool,
) -> serde_json::Value {
    let mut body = serde_json::json!({
        "message": text,
        "mode": "human",
        "thread_id": thread_id.to_string(),
        "event_id": event_id.to_string(),
        "input_chain": chain.to_string(),
        "input_seq": seq,
    });
    if new_thread {
        body["new_thread"] = serde_json::json!(true);
    }
    body
}

/// The event sequence of the `MessageReceived` this event id became, if any.
async fn recorded_sequence(pool: &sqlx::PgPool, event_id: Uuid) -> Option<i64> {
    sqlx::query_scalar(
        "SELECT sequence FROM events WHERE id = $1 AND event_type = 'MessageReceived'",
    )
    .bind(event_id)
    .fetch_optional(pool)
    .await
    .expect("events query failed")
}

/// Wait for the engine's own turn to record `event_id`. The input was already
/// acked, so this waits on a detached task, not on a race.
async fn wait_until_recorded(pool: &sqlx::PgPool, event_id: Uuid) -> i64 {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        if let Some(seq) = recorded_sequence(pool, event_id).await {
            return seq;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "message {event_id} was acked but never recorded",
        );
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
}

/// Wait for the turn that `request` started to end. Its closing events take the
/// thread append lock, so a test that holds the lock must wait for this first.
/// Otherwise the turn is the waiter the test's sync point sees, not the request
/// under test.
async fn wait_until_turn_ended(pool: &sqlx::PgPool, thread_id: Uuid, request: Uuid) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    loop {
        let ended: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM events WHERE thread_id = $1 \
             AND payload->>'request_event_id' = $2 \
             AND event_type IN ('ResponseGenerated', 'ResponseCompleted', \
                                'ResponseFailed', 'ResponseCanceled', 'ResponseAborted'))",
        )
        .bind(thread_id)
        .bind(request.to_string())
        .fetch_one(pool)
        .await
        .expect("events query failed");
        if ended {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the turn for {request} never ended",
        );
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
}

/// Open a thread with one input on `chain` at sequence 1, and wait for it and
/// for its turn to end.
async fn open_thread(client: &reqwest::Client, pool: &sqlx::PgPool, chain: Uuid) -> Uuid {
    let thread_id = Uuid::new_v4();
    let opening = Uuid::new_v4();
    let status = client
        .post(chat_url())
        .json(&input(
            thread_id,
            chain,
            1,
            opening,
            "Reply with one word.",
            true,
        ))
        .send()
        .await
        .expect("opening request failed")
        .status();
    assert_eq!(status, 200, "the opening message should be accepted");
    wait_until_recorded(pool, opening).await;
    wait_until_turn_ended(pool, thread_id, opening).await;
    thread_id
}

/// Hold `thread_id`'s append lock until the returned transaction ends.
async fn hold_thread_append_lock(
    pool: &sqlx::PgPool,
    thread_id: Uuid,
) -> sqlx::Transaction<'static, sqlx::Postgres> {
    let mut tx = pool.begin().await.expect("begin failed");
    sqlx::query("SELECT pg_advisory_xact_lock($1, hashtext($2::text))")
        .bind(THREAD_APPEND_LOCK_CLASS)
        .bind(thread_id)
        .execute(&mut *tx)
        .await
        .expect("could not take the thread append lock");
    tx
}

/// Is any session waiting on `thread_id`'s append lock?
async fn lock_has_a_waiter(pool: &sqlx::PgPool, thread_id: Uuid) -> bool {
    sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM pg_locks \
         WHERE locktype = 'advisory' AND NOT granted AND objsubid = 2 \
           AND classid::bigint = $1 \
           AND objid::bigint = (hashtext($2::text)::bigint & 4294967295))",
    )
    .bind(i64::from(THREAD_APPEND_LOCK_CLASS))
    .bind(thread_id)
    .fetch_one(pool)
    .await
    .expect("pg_locks query failed")
}

/// The sync point: wait until the engine is blocked on the held lock.
async fn wait_for_a_lock_waiter(pool: &sqlx::PgPool, thread_id: Uuid) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    while !lock_has_a_waiter(pool, thread_id).await {
        assert!(
            std::time::Instant::now() < deadline,
            "the engine never waited on thread {thread_id}'s append lock",
        );
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
}

/// A send the client gave up on can still reach the engine after a later one.
/// It must be refused then, and never recorded after the later one.
#[tokio::test]
async fn an_overtaken_input_is_refused_and_never_recorded() {
    let client = user_client().await;
    let pool = pool().await;
    let chain = Uuid::new_v4();
    let thread_id = open_thread(&client, &pool, chain).await;

    let later = Uuid::new_v4();
    let status = client
        .post(chat_url())
        .json(&input(
            thread_id,
            chain,
            3,
            later,
            "the later message",
            false,
        ))
        .send()
        .await
        .expect("later request failed")
        .status();
    assert_eq!(status, 200, "the later message should be accepted");

    let straggler = Uuid::new_v4();
    let resp = client
        .post(chat_url())
        .json(&input(
            thread_id,
            chain,
            2,
            straggler,
            "the straggler",
            false,
        ))
        .send()
        .await
        .expect("straggler request failed");
    let status = resp.status();
    let body: serde_json::Value = resp.json().await.unwrap_or_default();
    assert_eq!(
        status, 409,
        "an overtaken input must be refused, got {body}"
    );
    assert_eq!(
        body["reason"], "overtaken",
        "the refusal names its cause: {body}"
    );
    assert_eq!(
        recorded_sequence(&pool, straggler).await,
        None,
        "an overtaken input must never be recorded",
    );
}

/// A repeat of the same input, same sequence and same event id, is the quiet
/// retry of a send that lost its reply. It is acked, and runs once.
#[tokio::test]
async fn a_repeat_of_the_tail_is_acked_not_refused() {
    let client = user_client().await;
    let pool = pool().await;
    let chain = Uuid::new_v4();
    let thread_id = open_thread(&client, &pool, chain).await;

    let follow_up = Uuid::new_v4();
    for attempt in 1..=2 {
        let status = client
            .post(chat_url())
            .json(&input(
                thread_id,
                chain,
                2,
                follow_up,
                "the follow-up",
                false,
            ))
            .send()
            .await
            .expect("follow-up request failed")
            .status();
        assert_eq!(
            status, 200,
            "attempt {attempt} of the same input should be acked"
        );
    }
    wait_until_recorded(&pool, follow_up).await;
}

/// An input that sends no chain (agent mode, the CLI, a stale client) is
/// never refused, wherever the chains on that thread stand.
#[tokio::test]
async fn an_input_without_a_chain_is_never_refused() {
    let client = user_client().await;
    let pool = pool().await;
    let chain = Uuid::new_v4();
    let thread_id = open_thread(&client, &pool, chain).await;

    let chainless = Uuid::new_v4();
    let status = client
        .post(chat_url())
        .json(&serde_json::json!({
            "message": "a message with no chain",
            "mode": "human",
            "thread_id": thread_id.to_string(),
            "event_id": chainless.to_string(),
        }))
        .send()
        .await
        .expect("chainless request failed")
        .status();
    assert_eq!(status, 200, "an input with no chain is never refused");
    wait_until_recorded(&pool, chainless).await;
}

/// A dropped request must not split an admitted input from the work that
/// records it. Otherwise the message is lost, and every retry is acked as a
/// duplicate of a message that does not exist.
#[tokio::test]
async fn an_admitted_input_is_recorded_even_when_its_request_is_dropped() {
    let client = user_client().await;
    let pool = pool().await;
    let chain = Uuid::new_v4();
    let thread_id = open_thread(&client, &pool, chain).await;

    let lock = hold_thread_append_lock(&pool, thread_id).await;
    let follow_up = Uuid::new_v4();
    let body = input(
        thread_id,
        chain,
        2,
        follow_up,
        "the dropped follow-up",
        false,
    );
    let request = {
        let client = client.clone();
        tokio::spawn(async move { client.post(chat_url()).json(&body).send().await })
    };
    // The engine has the request and is blocked on the lock.
    wait_for_a_lock_waiter(&pool, thread_id).await;
    // The client gives up: its connection, and its request, are dropped.
    request.abort();
    let _ = request.await;
    lock.rollback().await.expect("releasing the lock failed");

    wait_until_recorded(&pool, follow_up).await;
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM events WHERE id = $1")
        .bind(follow_up)
        .fetch_one(&pool)
        .await
        .expect("events query failed");
    assert_eq!(count, 1, "the dropped follow-up is recorded exactly once");
}

/// The client sends a thread's second message once the first is acked. If the
/// first is not yet recorded, the second must wait behind it rather than be
/// refused as addressing an unknown thread.
#[tokio::test]
async fn a_second_send_to_a_new_thread_orders_behind_the_first() {
    let client = user_client().await;
    let pool = pool().await;
    let chain = Uuid::new_v4();
    let thread_id = Uuid::new_v4();

    // Held from before the first send, so the engine must admit it under the
    // lock before it can ack.
    let lock = hold_thread_append_lock(&pool, thread_id).await;
    let first = Uuid::new_v4();
    let first_request = {
        let client = client.clone();
        let body = input(thread_id, chain, 1, first, "Reply with one word.", true);
        tokio::spawn(async move { client.post(chat_url()).json(&body).send().await })
    };
    wait_for_a_lock_waiter(&pool, thread_id).await;
    lock.rollback().await.expect("releasing the lock failed");
    let first_status = first_request
        .await
        .expect("first request task panicked")
        .expect("first request failed")
        .status();
    assert_eq!(first_status, 200, "the first message should be accepted");

    // Only now, as the client does. A send before the ack races the admission's
    // commit, which the engine does not promise to order.
    let second = Uuid::new_v4();
    let second_status = client
        .post(chat_url())
        .json(&input(
            thread_id,
            chain,
            2,
            second,
            "the second message",
            false,
        ))
        .send()
        .await
        .expect("second request failed")
        .status();
    assert_eq!(
        second_status, 200,
        "a send to a thread whose first message is admitted must not be refused",
    );

    let first_seq = wait_until_recorded(&pool, first).await;
    let second_seq = wait_until_recorded(&pool, second).await;
    assert!(
        first_seq < second_seq,
        "the messages must record in send order, got {first_seq} and {second_seq}",
    );
}
