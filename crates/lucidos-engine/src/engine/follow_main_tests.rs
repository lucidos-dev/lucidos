//! How the follow task reacts to each answer from `origin`, over a stub host.

use super::{follow_round, wait_for_next_round, FollowHost, Follower};
use crate::engine::git_ops::{FollowOutcome, LandingOrder};
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::Notify;

/// Far shorter than the stub's hour-long interval.
const WOKEN_WITHIN: Duration = Duration::from_secs(5);

/// A paired gateway landed: the task asks `origin` now, not in an hour.
#[tokio::test]
async fn a_nudge_ends_the_wait_before_the_interval() {
    let host = Arc::new(StubHost::default());
    let waiting = tokio::spawn({
        let host = host.clone();
        async move { wait_for_next_round(host.as_ref()).await }
    });
    tokio::task::yield_now().await;
    host.nudge.notify_one();
    tokio::time::timeout(WOKEN_WITHIN, waiting)
        .await
        .expect("the nudge woke the follow task")
        .unwrap();
}

/// A nudge that lands while a round runs is kept for the next wait.
#[tokio::test]
async fn a_nudge_during_a_round_buys_one_more_round() {
    let host = StubHost::default();
    host.nudge.notify_one();
    tokio::time::timeout(WOKEN_WITHIN, wait_for_next_round(&host))
        .await
        .expect("the kept nudge ended the wait");
}

#[derive(Default)]
struct StubHost {
    /// `None` reads as origin-first.
    order: Mutex<Option<LandingOrder>>,
    follows: AtomicUsize,
    answered: AtomicUsize,
    builds: AtomicUsize,
    refusals: Mutex<Vec<String>>,
    nudge: Notify,
}

#[async_trait::async_trait]
impl FollowHost for StubHost {
    fn follow_repo(&self) -> &Path {
        self.follows.fetch_add(1, Ordering::SeqCst);
        Path::new("/nowhere")
    }

    async fn follow_interval(&self) -> Duration {
        Duration::from_secs(3600)
    }

    async fn landing_order(&self) -> LandingOrder {
        let order = self.order.lock().unwrap().clone();
        order.unwrap_or(LandingOrder::OriginFirst)
    }

    fn follow_nudge(&self) -> &Notify {
        &self.nudge
    }

    fn origin_answered(&self) {
        self.answered.fetch_add(1, Ordering::SeqCst);
    }

    fn start_build(self: &Arc<Self>) {
        self.builds.fetch_add(1, Ordering::SeqCst);
    }

    async fn follow_refused(&self, reason: String) {
        self.refusals.lock().unwrap().push(reason);
    }
}

fn followed() -> FollowOutcome {
    FollowOutcome::Followed {
        from: "a".into(),
        to: "b".into(),
    }
}

/// A pulled version builds and waits for a tap. The lane hears that
/// `origin` answered.
#[tokio::test]
async fn a_followed_main_starts_a_build_and_wakes_the_lane() {
    let host = Arc::new(StubHost::default());
    let mut follower = Follower::default();
    follower.react(&host, followed()).await;
    assert_eq!(host.builds.load(Ordering::SeqCst), 1);
    assert_eq!(host.answered.load(Ordering::SeqCst), 1);
    assert!(host.refusals.lock().unwrap().is_empty());
}

#[tokio::test]
async fn only_an_answer_from_origin_wakes_the_lane() {
    let host = Arc::new(StubHost::default());
    let mut follower = Follower::default();
    follower.react(&host, FollowOutcome::NoOrigin).await;
    follower
        .react(&host, FollowOutcome::Unreachable("offline".into()))
        .await;
    assert_eq!(host.answered.load(Ordering::SeqCst), 0);
    follower.react(&host, FollowOutcome::UpToDate).await;
    assert_eq!(host.answered.load(Ordering::SeqCst), 1);
    assert_eq!(host.builds.load(Ordering::SeqCst), 0);
}

/// Paired, a round asks `origin`. Unpaired, it never does, and a landing that
/// waited may land locally. Without an answer from the gateway, nothing moves.
#[tokio::test]
async fn only_a_paired_engine_follows_origin() {
    let host = Arc::new(StubHost::default());
    let mut follower = Follower::default();
    follow_round(&host, &mut follower).await;
    assert_eq!(host.follows.load(Ordering::SeqCst), 1);

    *host.order.lock().unwrap() = Some(LandingOrder::LocalFirst);
    follow_round(&host, &mut follower).await;
    assert_eq!(host.follows.load(Ordering::SeqCst), 1);
    assert_eq!(host.answered.load(Ordering::SeqCst), 1);

    *host.order.lock().unwrap() = Some(LandingOrder::Unknown("down".into()));
    follow_round(&host, &mut follower).await;
    assert_eq!(host.follows.load(Ordering::SeqCst), 1);
    assert_eq!(host.answered.load(Ordering::SeqCst), 1);
    assert!(host.refusals.lock().unwrap().is_empty());
}

/// A lasting refusal reaches the user once. Once cleared, a new one is
/// reported again.
#[tokio::test]
async fn a_refusal_is_reported_once_until_it_clears() {
    let host = Arc::new(StubHost::default());
    let mut follower = Follower::default();
    let refused = || FollowOutcome::Refused("diverged".into());
    follower.react(&host, refused()).await;
    follower.react(&host, refused()).await;
    assert_eq!(*host.refusals.lock().unwrap(), vec!["diverged".to_string()]);
    assert_eq!(host.answered.load(Ordering::SeqCst), 2);
    follower.react(&host, FollowOutcome::UpToDate).await;
    follower.react(&host, refused()).await;
    assert_eq!(host.refusals.lock().unwrap().len(), 2);
    assert_eq!(host.builds.load(Ordering::SeqCst), 0);
}

/// Following never switches by itself: the module asks for a build only,
/// never a restart or a rebuild that aborts one.
#[test]
fn following_never_restarts_the_engine() {
    let source = include_str!("follow_main.rs");
    let code = source.split("#[cfg(test)]").next().unwrap();
    for forbidden in ["restart", "request_switch", "exit("] {
        let calls = code
            .lines()
            .filter(|l| !l.trim_start().starts_with("//"))
            .any(|l| l.contains(forbidden));
        assert!(!calls, "follow_main.rs must not call {forbidden}");
    }
    assert!(code.contains("join_or_start_background_rebuild()"));
}
