//! The per-thread chain that keeps coding-agent follow-ups in sent order.

use super::*;
use std::time::Duration;

/// Long enough for a released waiter to run, short enough to keep the suite fast.
const SETTLE: Duration = Duration::from_millis(50);

fn tails(order: &FollowUpOrder) -> usize {
    lock(&order.chains).tails.len()
}

async fn is_released(turn: &mut FollowUpTurn) -> bool {
    tokio::time::timeout(SETTLE, turn.wait_for_predecessor())
        .await
        .is_ok()
}

#[tokio::test]
async fn turns_run_in_the_order_they_joined() {
    let order = FollowUpOrder::default();
    let thread = Uuid::new_v4();
    let turns: Vec<FollowUpTurn> = (0..3).map(|_| order.join(thread)).collect();
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
    let order = FollowUpOrder::default();
    let thread = Uuid::new_v4();
    let first = order.join(thread);
    let mut second = order.join(thread);
    assert!(!is_released(&mut second).await);

    drop(first);
    assert!(is_released(&mut second).await);
}

/// The second follow-up's task died while it waited. The third must still
/// wait for the first, or it would overtake it.
#[tokio::test]
async fn a_turn_dropped_while_waiting_keeps_the_order() {
    let order = FollowUpOrder::default();
    let thread = Uuid::new_v4();
    let first = order.join(thread);
    let second = order.join(thread);
    let mut third = order.join(thread);

    drop(second);
    assert!(
        !is_released(&mut third).await,
        "the third follow-up overtook the first"
    );

    drop(first);
    assert!(is_released(&mut third).await);
}

#[tokio::test]
async fn threads_do_not_wait_on_each_other() {
    let order = FollowUpOrder::default();
    let _busy = order.join(Uuid::new_v4());
    let mut other = order.join(Uuid::new_v4());
    assert!(is_released(&mut other).await);
}

#[tokio::test]
async fn an_idle_chain_holds_nothing() {
    let order = FollowUpOrder::default();
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
