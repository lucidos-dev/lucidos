//! A thread's admitted inputs keep the order admission gave them.

use super::*;
use std::time::Duration;

/// Long enough for a released waiter to run, short enough to keep the suite fast.
const SETTLE: Duration = Duration::from_millis(50);

fn tails(order: &InputOrder) -> usize {
    lock(&order.threads).tails.len()
}

async fn is_released(turn: &mut InputTurn) -> bool {
    tokio::time::timeout(SETTLE, turn.wait_for_predecessor())
        .await
        .is_ok()
}

#[tokio::test]
async fn turns_run_in_the_order_they_joined() {
    let order = InputOrder::default();
    let thread = Uuid::new_v4();
    let turns: Vec<InputTurn> = (0..3).map(|_| order.join(thread)).collect();
    let log = Arc::new(Mutex::new(Vec::new()));

    // Started newest first, so any turn that did not wait would log early.
    let mut tasks = Vec::new();
    for (sent, mut turn) in turns.into_iter().enumerate().rev() {
        let log = log.clone();
        tasks.push(tokio::spawn(async move {
            turn.wait_for_predecessor().await;
            tokio::time::sleep(Duration::from_millis(5)).await;
            log.lock().unwrap().push(sent);
        }));
    }
    for task in tasks {
        task.await.unwrap();
    }
    assert_eq!(*log.lock().unwrap(), vec![0, 1, 2]);
}

#[tokio::test]
async fn a_dropped_turn_releases_its_successor() {
    let order = InputOrder::default();
    let thread = Uuid::new_v4();
    let first = order.join(thread);
    let mut second = order.join(thread);
    assert!(!is_released(&mut second).await);

    drop(first);
    assert!(is_released(&mut second).await);
}

/// The second input's task died while it waited. The third must still
/// wait for the first, or it would overtake it.
#[tokio::test]
async fn a_turn_dropped_while_waiting_keeps_the_order() {
    let order = InputOrder::default();
    let thread = Uuid::new_v4();
    let first = order.join(thread);
    let second = order.join(thread);
    let mut third = order.join(thread);

    drop(second);
    assert!(
        !is_released(&mut third).await,
        "the third input overtook the first"
    );

    drop(first);
    assert!(is_released(&mut third).await);
}

#[tokio::test]
async fn threads_do_not_wait_on_each_other() {
    let order = InputOrder::default();
    let _busy = order.join(Uuid::new_v4());
    let mut other = order.join(Uuid::new_v4());
    assert!(is_released(&mut other).await);
}

#[tokio::test]
async fn an_idle_order_holds_nothing() {
    let order = InputOrder::default();
    let thread = Uuid::new_v4();
    drop(order.join(thread));
    assert_eq!(tails(&order), 0);

    let first = order.join(thread);
    let second = order.join(thread);
    let third = order.join(thread);
    drop(second);
    drop(first);
    drop(third);
    tokio::time::sleep(SETTLE).await;
    assert_eq!(tails(&order), 0);
}

#[tokio::test]
async fn an_input_with_nothing_ahead_says_so() {
    let order = InputOrder::default();
    let thread = Uuid::new_v4();
    let mut first = order.join(thread);
    assert!(!first.has_input_ahead());

    let mut second = order.join(thread);
    assert!(
        second.has_input_ahead(),
        "the first input is still in flight"
    );
    drop(first);
    tokio::time::sleep(SETTLE).await;
    assert!(!second.has_input_ahead(), "the first input was released");
}

fn tail(seq: i64, event_id: Uuid) -> Option<ChainTail> {
    Some(ChainTail { seq, event_id })
}

#[test]
fn a_chain_admits_its_first_input_and_anything_past_its_tail() {
    let id = Uuid::new_v4();
    assert!(admits(None, 1, id));
    assert!(admits(tail(1, Uuid::new_v4()), 2, id));
    assert!(
        admits(tail(1, Uuid::new_v4()), 7, id),
        "a gap is a send that never landed"
    );
}

#[test]
fn a_chain_refuses_an_input_its_tail_overtook() {
    let id = Uuid::new_v4();
    assert!(!admits(tail(3, Uuid::new_v4()), 2, id));
    assert!(
        !admits(tail(3, Uuid::new_v4()), 3, id),
        "same place, another input"
    );
}

#[test]
fn a_repeat_of_the_tail_itself_is_admitted() {
    let id = Uuid::new_v4();
    assert!(admits(tail(3, id), 3, id));
}

#[test]
fn a_position_comes_whole_or_not_at_all() {
    let chain = Uuid::new_v4().to_string();
    let id = Some(Uuid::new_v4());
    assert_eq!(InputPosition::from_wire(None, None, id), Ok(None));
    assert!(InputPosition::from_wire(Some(&chain), None, id).is_err());
    assert!(InputPosition::from_wire(None, Some(1), id).is_err());
    assert!(InputPosition::from_wire(Some("not-a-uuid"), Some(1), id).is_err());
    assert!(InputPosition::from_wire(Some(&chain), Some(0), id).is_err());
    assert!(
        InputPosition::from_wire(Some(&chain), Some(1), None).is_err(),
        "a position names the input it places",
    );
    assert_eq!(
        InputPosition::from_wire(Some(&chain), Some(4), id),
        Ok(Some(InputPosition {
            chain: Uuid::parse_str(&chain).unwrap(),
            seq: 4
        })),
    );
}

mod admission {
    use super::*;
    use crate::test_support::{setup_test_db, teardown_test_db};

    async fn admit(
        order: &InputOrder,
        pool: &sqlx::PgPool,
        thread: Uuid,
        chain: Uuid,
        seq: i64,
        event_id: Uuid,
    ) -> InputAdmission {
        order
            .admit(
                pool,
                thread,
                Some(InputPosition { chain, seq }),
                Some(event_id),
            )
            .await
            .expect("admission ran")
    }

    fn is_admitted(admission: &InputAdmission) -> bool {
        matches!(admission, InputAdmission::Admitted(_))
    }

    async fn chain_rows(pool: &sqlx::PgPool, thread: Uuid) -> i64 {
        sqlx::query_scalar("SELECT count(*) FROM thread_input_chains WHERE thread_id = $1")
            .bind(thread)
            .fetch_one(pool)
            .await
            .unwrap()
    }

    /// The straggler of a send the client gave up on reaches the engine after
    /// the next one. It is refused, and the tail stays where the later one put it.
    #[tokio::test]
    async fn a_straggler_is_refused_after_its_successor() {
        let (pool, db) = setup_test_db().await;
        let order = InputOrder::default();
        let (thread, chain) = (Uuid::new_v4(), Uuid::new_v4());

        assert!(is_admitted(
            &admit(&order, &pool, thread, chain, 2, Uuid::new_v4()).await
        ));
        assert!(matches!(
            admit(&order, &pool, thread, chain, 1, Uuid::new_v4()).await,
            InputAdmission::Overtaken
        ));
        assert!(is_admitted(
            &admit(&order, &pool, thread, chain, 3, Uuid::new_v4()).await
        ));

        teardown_test_db(&db).await;
    }

    /// A quiet retry, or a retry after a restart lost the first attempt, is the
    /// same input at the same place.
    #[tokio::test]
    async fn the_same_input_is_admitted_again() {
        let (pool, db) = setup_test_db().await;
        let order = InputOrder::default();
        let (thread, chain, id) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());

        assert!(is_admitted(
            &admit(&order, &pool, thread, chain, 1, id).await
        ));
        assert!(is_admitted(
            &admit(&order, &pool, thread, chain, 1, id).await
        ));

        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn chains_never_refuse_each_other() {
        let (pool, db) = setup_test_db().await;
        let order = InputOrder::default();
        let thread = Uuid::new_v4();
        let (one, other) = (Uuid::new_v4(), Uuid::new_v4());

        assert!(is_admitted(
            &admit(&order, &pool, thread, one, 5, Uuid::new_v4()).await
        ));
        assert!(is_admitted(
            &admit(&order, &pool, thread, other, 1, Uuid::new_v4()).await
        ));
        assert_eq!(chain_rows(&pool, thread).await, 2);

        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn an_input_with_no_position_is_admitted_and_writes_nothing() {
        let (pool, db) = setup_test_db().await;
        let order = InputOrder::default();
        let thread = Uuid::new_v4();

        let admission = order
            .admit(&pool, thread, None, Some(Uuid::new_v4()))
            .await
            .unwrap();
        assert!(is_admitted(&admission));
        assert_eq!(chain_rows(&pool, thread).await, 0);
        assert!(!InputOrder::thread_has_admitted_input(&pool, thread)
            .await
            .unwrap());

        teardown_test_db(&db).await;
    }

    /// An admitted input holds its place: the next one admitted to the thread
    /// sees it ahead, until it is released.
    #[tokio::test]
    async fn an_admitted_input_holds_its_place_in_the_order() {
        let (pool, db) = setup_test_db().await;
        let order = InputOrder::default();
        let (thread, chain) = (Uuid::new_v4(), Uuid::new_v4());

        let InputAdmission::Admitted(first) =
            admit(&order, &pool, thread, chain, 1, Uuid::new_v4()).await
        else {
            panic!("the first input is admitted");
        };
        assert!(InputOrder::thread_has_admitted_input(&pool, thread)
            .await
            .unwrap());
        let InputAdmission::Admitted(mut second) =
            admit(&order, &pool, thread, chain, 2, Uuid::new_v4()).await
        else {
            panic!("the second input is admitted");
        };
        assert!(second.has_input_ahead());
        drop(first);
        second.wait_for_predecessor().await;
        assert!(!second.has_input_ahead());

        teardown_test_db(&db).await;
    }

    /// Admission takes the thread append lock, so it waits for an append in
    /// progress on the thread. The test holds the lock as the sync point.
    #[tokio::test]
    async fn admission_waits_for_the_thread_append_lock() {
        let (pool, db) = setup_test_db().await;
        let order = InputOrder::default();
        let (thread, chain) = (Uuid::new_v4(), Uuid::new_v4());

        let mut held = pool.begin().await.unwrap();
        crate::engine::event_bus::EventBus::lock_thread_append(&mut held, thread)
            .await
            .unwrap();
        let admission = {
            let (order, pool) = (order.clone(), pool.clone());
            tokio::spawn(async move {
                order
                    .admit(
                        &pool,
                        thread,
                        Some(InputPosition { chain, seq: 1 }),
                        Some(Uuid::new_v4()),
                    )
                    .await
                    .map(|admission| is_admitted(&admission))
            })
        };
        // The sync point: admission is blocked on the held lock.
        loop {
            let waiting: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM pg_locks WHERE locktype = 'advisory' AND NOT granted)",
            )
            .fetch_one(&pool)
            .await
            .unwrap();
            if waiting {
                break;
            }
            assert!(!admission.is_finished(), "admission ran without the lock");
            tokio::task::yield_now().await;
        }
        assert_eq!(
            chain_rows(&pool, thread).await,
            0,
            "nothing is admitted under a held lock"
        );
        held.rollback().await.unwrap();
        assert!(admission.await.unwrap().unwrap());
        assert_eq!(chain_rows(&pool, thread).await, 1);

        teardown_test_db(&db).await;
    }
}
