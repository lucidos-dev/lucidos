//! A watermark is a committed horizon: no events row at or below it commits
//! after it was read (ADR 0364).
//!
//! Each test parks one append after it drew its sequence, then commits a later
//! event past it. The parked row is a late commit. Below a watermark, no
//! catch-up scan could ever see it.

use super::*;
use tokio::task::JoinHandle;

/// How the outside transaction lets a parked append go.
enum Release {
    /// The append was waiting on a row lock, and proceeds.
    Commit,
    /// The append was waiting on a conflicting insert, which vanishes.
    Rollback,
}

/// An events row the test can look for, as a `WHERE` clause over `$1`.
struct ParkedRow {
    predicate: &'static str,
    key: String,
}

/// Hold `thread_id`'s summary row, so an emit on that thread parks in its
/// projection, after its INSERT drew a sequence.
async fn hold_summary_row(
    pool: &PgPool,
    thread_id: Uuid,
) -> sqlx::Transaction<'static, sqlx::Postgres> {
    let mut holder = pool.begin().await.unwrap();
    sqlx::query("SELECT 1 FROM thread_summaries WHERE thread_id = $1 FOR UPDATE")
        .bind(thread_id)
        .execute(&mut *holder)
        .await
        .unwrap();
    holder
}

/// Spawn a message on `thread_id` with a known event id. Its projection parks
/// it behind [`hold_summary_row`].
fn spawn_parked_thread_emit(bus: &EventBus, thread_id: Uuid) -> (Uuid, JoinHandle<i64>) {
    let event_id = Uuid::new_v4();
    let BusEvent::Thread { event, meta, .. } = thread_message(thread_id, None, "parked") else {
        unreachable!("thread_message builds a thread event");
    };
    let bus = bus.clone();
    let parked = tokio::spawn(async move {
        bus.emit(BusEvent::Thread {
            thread_id,
            event,
            meta: EventMeta {
                event_id: Some(event_id),
                ..meta
            },
        })
        .await
        .unwrap()
        .expect("the parked emit is persisted")
        .seq
    });
    (event_id, parked)
}

/// A thread with a summary row, ready to be held.
async fn seeded_thread(bus: &EventBus) -> Uuid {
    let thread_id = Uuid::new_v4();
    emit_thread_message(bus, thread_id, None, "start").await;
    thread_id
}

/// Read the horizon in the background, and with it whether `row` was already
/// visible when the read returned.
fn spawn_horizon_read(pool: &PgPool, row: ParkedRow) -> JoinHandle<(i64, bool)> {
    let pool = pool.clone();
    tokio::spawn(async move {
        let horizon = committed_event_horizon(&pool).await.unwrap();
        let sql = format!(
            "SELECT EXISTS(SELECT 1 FROM events WHERE {})",
            row.predicate
        );
        let visible: bool = sqlx::query_scalar(&sql)
            .bind(&row.key)
            .fetch_one(&pool)
            .await
            .unwrap();
        (horizon, visible)
    })
}

/// The invariant, around an append the caller has already parked.
///
/// A later event commits first. The horizon read then runs, and only after
/// that does the parked append go. The read must not return a horizon at or
/// above the parked sequence while that row is still uncommitted.
async fn assert_horizon_covers_parked_append(
    pool: &PgPool,
    bus: &EventBus,
    holder: sqlx::Transaction<'static, sqlx::Postgres>,
    release: Release,
    parked: JoinHandle<i64>,
    row: ParkedRow,
) {
    let later = bus
        .emit(thread_message(Uuid::new_v4(), None, "later"))
        .await
        .unwrap()
        .expect("the later event is persisted")
        .seq;

    let mut read = spawn_horizon_read(pool, row);
    // The parked append is one lock waiter. A read waiting it out is the second.
    let early = tokio::select! {
        r = &mut read => Some(r.unwrap()),
        _ = wait_until_blocked_on_locks(pool, 2) => None,
    };
    match release {
        Release::Commit => holder.commit().await.unwrap(),
        Release::Rollback => holder.rollback().await.unwrap(),
    }
    let parked_seq = parked.await.unwrap();
    let (horizon, visible) = match early {
        Some(r) => r,
        None => read.await.unwrap(),
    };

    assert!(
        horizon >= later,
        "horizon {horizon} is below the committed sequence {later}"
    );
    assert!(
        visible || parked_seq > horizon,
        "watermark {horizon} passed uncommitted sequence {parked_seq}: that row committed \
         after the read, so a catch-up scan from this watermark can never see it"
    );
}

/// A thread emit holds a lower sequence uncommitted while a later event
/// commits. `MAX(sequence)` alone reads past it.
#[tokio::test]
async fn a_watermark_never_passes_an_uncommitted_thread_event() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = seeded_thread(&bus).await;

    let holder = hold_summary_row(&pool, thread_id).await;
    let (event_id, parked) = spawn_parked_thread_emit(&bus, thread_id);
    wait_until_blocked_on_locks(&pool, 1).await;

    assert_horizon_covers_parked_append(
        &pool,
        &bus,
        holder,
        Release::Commit,
        parked,
        ParkedRow {
            predicate: "id = $1::uuid",
            key: event_id.to_string(),
        },
    )
    .await;

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// A persisted system event is an append like any other. It parks in its
/// projection on a `notifications` row an outside transaction is inserting.
#[tokio::test]
async fn a_watermark_never_passes_an_uncommitted_system_event() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let notification_id = Uuid::new_v4();

    let mut holder = pool.begin().await.unwrap();
    sqlx::query("INSERT INTO notifications (id, title, message) VALUES ($1, 'held', 'held')")
        .bind(notification_id)
        .execute(&mut *holder)
        .await
        .unwrap();
    let parked = tokio::spawn({
        let bus = bus.clone();
        async move {
            bus.emit(BusEvent::System(SystemEvent::NotificationCreated {
                id: notification_id.to_string(),
                title: "parked".into(),
                message: "parked".into(),
                task_id: None,
                app_id: None,
                thread_id: None,
                event_id: None,
                tap: crate::scheduler::notifications::Tap::Modal,
                actor: None,
            }))
            .await
            .unwrap()
            .expect("the parked system event is persisted")
            .seq
        }
    });
    wait_until_blocked_on_locks(&pool, 1).await;

    assert_horizon_covers_parked_append(
        &pool,
        &bus,
        holder,
        Release::Rollback,
        parked,
        ParkedRow {
            predicate: "aggregate = 'notification' AND aggregate_id = $1",
            key: notification_id.to_string(),
        },
    )
    .await;

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// A historical replay draws a sequence too. It parks on an outside
/// transaction inserting the same event id, which then rolls back.
#[tokio::test]
async fn a_watermark_never_passes_an_uncommitted_replay() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let event_id = Uuid::new_v4();

    let mut holder = pool.begin().await.unwrap();
    sqlx::query(
        "INSERT INTO events (id, aggregate, aggregate_id, event_type, payload) \
         VALUES ($1, 'test', 'held', 'HistoryReplayed', '{}')",
    )
    .bind(event_id)
    .execute(&mut *holder)
    .await
    .unwrap();
    let parked = tokio::spawn({
        let bus = bus.clone();
        async move {
            let payload = serde_json::json!({ "summary": "replayed" });
            bus.replay_historical_event(HistoricalReplay {
                event_id,
                aggregate: "test",
                aggregate_id: "replayed",
                event_type: "HistoryReplayed",
                payload: &payload,
                thread_id: None,
                created: None,
                broadcast: false,
            })
            .await
            .unwrap()
            .expect("the replay inserts once the conflicting row is gone")
        }
    });
    wait_until_blocked_on_locks(&pool, 1).await;

    assert_horizon_covers_parked_append(
        &pool,
        &bus,
        holder,
        Release::Rollback,
        parked,
        ParkedRow {
            predicate: "id = $1::uuid",
            key: event_id.to_string(),
        },
    )
    .await;

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// A horizon read waiting out one slow append stalls nobody else. Every other
/// emit in the workspace still commits.
#[tokio::test]
async fn a_waiting_horizon_read_does_not_stall_other_emits() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = seeded_thread(&bus).await;

    let holder = hold_summary_row(&pool, thread_id).await;
    let (_, parked) = spawn_parked_thread_emit(&bus, thread_id);
    wait_until_blocked_on_locks(&pool, 1).await;
    let read = tokio::spawn({
        let pool = pool.clone();
        async move { committed_event_horizon(&pool).await.unwrap() }
    });
    wait_until_blocked_on_locks(&pool, 2).await;

    let other = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        bus.emit(thread_message(Uuid::new_v4(), None, "elsewhere")),
    )
    .await;
    assert!(
        other.is_ok(),
        "an emit on another thread waited behind a horizon read"
    );
    assert!(
        !read.is_finished(),
        "the read stopped waiting for the parked append"
    );

    holder.commit().await.unwrap();
    parked.await.unwrap();
    read.await.unwrap();

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// The backend pid of the connection holding `tx`.
async fn backend_pid(tx: &mut sqlx::Transaction<'static, sqlx::Postgres>) -> i32 {
    sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut **tx)
        .await
        .unwrap()
}

/// The holder pid behind the append the horizon read is waiting on now. The
/// read is the one backend waiting on an advisory lock. The append it waits
/// on is blocked, in turn, by exactly one holder.
async fn holder_behind_the_read(pool: &PgPool) -> i32 {
    sqlx::query_scalar(
        "SELECT unnest(pg_blocking_pids(a.pid)) FROM pg_stat_activity a \
         WHERE a.pid = ( \
             SELECT unnest(pg_blocking_pids(r.pid)) FROM pg_stat_activity r \
             WHERE r.datname = current_database() AND r.wait_event = 'advisory')",
    )
    .fetch_one(pool)
    .await
    .unwrap()
}

/// A read that cannot outlast stuck appends fails loudly rather than guess.
/// Both arming callers turn the error into a refusal they already report.
///
/// Two stuck appends share ONE deadline. Halfway through, the append the read
/// waits on goes, so the read moves to the other one with half its time left.
/// A fresh timeout per append would run half again as long.
#[tokio::test]
async fn a_horizon_read_gives_up_on_stuck_appends_within_one_deadline() {
    const TIMEOUT_MS: u64 = 4_000;
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let mut holders = Vec::new();
    let mut parked = Vec::new();
    for _ in 0..2 {
        let thread_id = seeded_thread(&bus).await;
        let mut holder = hold_summary_row(&pool, thread_id).await;
        holders.push((backend_pid(&mut holder).await, holder));
        parked.push(spawn_parked_thread_emit(&bus, thread_id).1);
    }
    wait_until_blocked_on_locks(&pool, 2).await;

    // Its own pool: the holders and parked appends take four of the five
    // shared connections, and the read must not queue for one.
    let reader_pool = PgPool::connect_with((*pool.connect_options()).clone())
        .await
        .unwrap();
    let started = std::time::Instant::now();
    let read = tokio::spawn({
        let pool = reader_pool.clone();
        async move {
            super::super::append_horizon::committed_event_horizon_within(&pool, TIMEOUT_MS).await
        }
    });
    wait_until_blocked_on_locks(&pool, 3).await;
    // Looked up before the sleep: the read waits on this append until it is
    // released, and a starved test task can oversleep the whole deadline.
    let behind = holder_behind_the_read(&pool).await;
    tokio::time::sleep(std::time::Duration::from_millis(TIMEOUT_MS / 2)).await;
    let at = holders.iter().position(|(pid, _)| *pid == behind).unwrap();
    holders.remove(at).1.commit().await.unwrap();

    let read = read.await.unwrap();
    let took = started.elapsed();
    assert!(
        read.is_err(),
        "the read returned {read:?} while an append was still in flight"
    );
    assert!(
        took < std::time::Duration::from_millis(TIMEOUT_MS * 5 / 4),
        "the read took {took:?}, so the second append got a fresh {TIMEOUT_MS} ms"
    );

    for (_, holder) in holders {
        holder.commit().await.unwrap();
    }
    for p in parked {
        p.await.unwrap();
    }

    reader_pool.close().await;
    pool.close().await;
    teardown_test_db(&db_name).await;
}
