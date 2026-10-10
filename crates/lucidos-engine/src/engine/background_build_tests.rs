//! The join and restart rules over a stub build that runs until released.

use super::{join_or_start, restart, BackgroundBuild, BuildHost, FinishedBuild};
use crate::engine::engine_version::{BuildFailure, BuildState};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// A host whose every build blocks until the test hands it a permit.
struct StubHost {
    build: BackgroundBuild,
    state: Mutex<BuildState>,
    emitted: Mutex<Vec<&'static str>>,
    release: tokio::sync::Semaphore,
    started: AtomicUsize,
    finished: AtomicUsize,
    killed: AtomicUsize,
    covers_head_calls: AtomicUsize,
    covers_head: AtomicBool,
    fail: AtomicBool,
    retried_failures: Mutex<Vec<Option<String>>>,
}

impl StubHost {
    fn new() -> Arc<Self> {
        Arc::new(StubHost {
            build: BackgroundBuild::default(),
            state: Mutex::new(BuildState::Idle),
            emitted: Mutex::new(Vec::new()),
            release: tokio::sync::Semaphore::new(0),
            started: AtomicUsize::new(0),
            finished: AtomicUsize::new(0),
            killed: AtomicUsize::new(0),
            covers_head_calls: AtomicUsize::new(0),
            covers_head: AtomicBool::new(true),
            fail: AtomicBool::new(false),
            retried_failures: Mutex::new(Vec::new()),
        })
    }

    fn started(&self) -> usize {
        self.started.load(Ordering::SeqCst)
    }

    fn emitted(&self) -> Vec<&'static str> {
        self.emitted.lock().unwrap().clone()
    }

    fn release_one(&self) {
        self.release.add_permits(1);
    }
}

/// Counts a build whose future was dropped before it finished: a kill.
struct KillProbe<'a>(Option<&'a AtomicUsize>);

impl Drop for KillProbe<'_> {
    fn drop(&mut self) {
        if let Some(killed) = self.0 {
            killed.fetch_add(1, Ordering::SeqCst);
        }
    }
}

#[async_trait::async_trait]
impl BuildHost for StubHost {
    fn background_build(&self) -> &BackgroundBuild {
        &self.build
    }

    fn build_state(&self) -> BuildState {
        self.state.lock().unwrap().clone()
    }

    fn set_build_state(&self, state: BuildState) {
        *self.state.lock().unwrap() = state;
    }

    async fn emit_build_state_changed(&self, state: &BuildState) {
        self.emitted.lock().unwrap().push(state.as_wire());
    }

    async fn run_build(&self, previous_failure: Option<&str>) -> FinishedBuild {
        self.retried_failures
            .lock()
            .unwrap()
            .push(previous_failure.map(str::to_string));
        let n = self.started.fetch_add(1, Ordering::SeqCst) + 1;
        let mut probe = KillProbe(Some(&self.killed));
        self.release.acquire().await.unwrap().forget();
        probe.0 = None;
        self.finished.fetch_add(1, Ordering::SeqCst);
        let built_head = Some(format!("head-{n}"));
        let state = if self.fail.load(Ordering::SeqCst) {
            BuildState::failed_with(BuildFailure::plain(format!("broke at {n}")))
        } else {
            BuildState::ready_from(built_head.clone())
        };
        FinishedBuild { state, built_head }
    }

    async fn covers_head(&self, _finished: &FinishedBuild) -> bool {
        self.covers_head_calls.fetch_add(1, Ordering::SeqCst);
        self.covers_head.load(Ordering::SeqCst)
    }
}

/// Wait, yielding to the build task, until `done` holds. Panics after 5s.
async fn until(what: &str, done: impl Fn() -> bool) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while !done() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for {what}"
        );
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
}

async fn until_settled(host: &StubHost) {
    until("the build to settle", || {
        !matches!(host.build_state(), BuildState::Building { .. })
            && host.emitted().last() != Some(&"building")
    })
    .await;
}

#[tokio::test]
async fn a_join_leaves_the_build_in_flight_running_to_completion() {
    let host = StubHost::new();
    join_or_start(&host);
    until("the first build to start", || host.started() == 1).await;

    join_or_start(&host);
    // Give a wrongly restarted build every chance to show up.
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert_eq!(host.started(), 1, "a join must not start a second build");
    assert_eq!(host.killed.load(Ordering::SeqCst), 0);

    host.release_one();
    until_settled(&host).await;
    assert_eq!(host.finished.load(Ordering::SeqCst), 1);
    assert_eq!(host.killed.load(Ordering::SeqCst), 0, "no build was killed");
    assert_eq!(host.started(), 1, "HEAD is covered, so no follow-up");
    assert!(matches!(host.build_state(), BuildState::Ready { .. }));
}

#[tokio::test]
async fn a_join_emits_no_state_change_of_its_own() {
    let host = StubHost::new();
    join_or_start(&host);
    until("the first build to start", || host.started() == 1).await;
    join_or_start(&host);
    join_or_start(&host);
    assert!(matches!(host.build_state(), BuildState::Building { .. }));
    assert_eq!(host.emitted(), ["building"]);

    host.release_one();
    until_settled(&host).await;
    assert_eq!(host.emitted(), ["building", "ready"]);
}

#[tokio::test]
async fn a_burst_of_joins_behind_head_costs_one_follow_up_build() {
    let host = StubHost::new();
    host.covers_head.store(false, Ordering::SeqCst);
    join_or_start(&host);
    until("the first build to start", || host.started() == 1).await;
    for _ in 0..3 {
        join_or_start(&host);
    }

    host.release_one();
    until("the follow-up build to start", || host.started() == 2).await;
    // The follow-up runs inside the same build: still Building, no new poke.
    assert!(matches!(host.build_state(), BuildState::Building { .. }));
    assert_eq!(host.emitted(), ["building"]);

    host.release_one();
    until_settled(&host).await;
    assert_eq!(host.started(), 2, "three joins cost exactly one follow-up");
    assert_eq!(host.finished.load(Ordering::SeqCst), 2);
    assert_eq!(host.killed.load(Ordering::SeqCst), 0);
    assert_eq!(host.emitted(), ["building", "ready"]);
}

#[tokio::test]
async fn a_build_nobody_joined_settles_without_asking_where_head_is() {
    let host = StubHost::new();
    join_or_start(&host);
    host.release_one();
    until_settled(&host).await;
    assert_eq!(host.covers_head_calls.load(Ordering::SeqCst), 0);
    assert_eq!(host.started(), 1);
}

#[tokio::test]
async fn a_follow_up_after_a_failure_retries_with_that_failure() {
    let host = StubHost::new();
    host.covers_head.store(false, Ordering::SeqCst);
    host.fail.store(true, Ordering::SeqCst);
    join_or_start(&host);
    until("the first build to start", || host.started() == 1).await;
    join_or_start(&host);

    host.release_one();
    until("the follow-up build to start", || host.started() == 2).await;
    host.release_one();
    until_settled(&host).await;
    assert_eq!(
        *host.retried_failures.lock().unwrap(),
        [None, Some("broke at 1".to_string())],
        "the follow-up must know which failure it retries, so a repeat reads as one"
    );
    assert!(matches!(host.build_state(), BuildState::Failed { .. }));
}

#[tokio::test]
async fn a_request_after_the_build_settled_starts_a_fresh_one() {
    let host = StubHost::new();
    join_or_start(&host);
    host.release_one();
    until_settled(&host).await;

    join_or_start(&host);
    until("a fresh build to start", || host.started() == 2).await;
    assert!(matches!(host.build_state(), BuildState::Building { .. }));
    host.release_one();
    until_settled(&host).await;
    assert_eq!(host.emitted(), ["building", "ready", "building", "ready"]);
}

#[tokio::test]
async fn an_explicit_restart_kills_the_build_in_flight_and_starts_over() {
    let host = StubHost::new();
    join_or_start(&host);
    until("the first build to start", || host.started() == 1).await;

    restart(&host);
    until("the replacement build to start", || host.started() == 2).await;
    assert_eq!(
        host.killed.load(Ordering::SeqCst),
        1,
        "the restart kills the old build"
    );

    host.release_one();
    until_settled(&host).await;
    assert_eq!(host.finished.load(Ordering::SeqCst), 1);
    // The killed build never reports a result of its own.
    assert_eq!(host.emitted(), ["building", "building", "ready"]);
}

#[tokio::test]
async fn an_explicit_restart_with_nothing_running_starts_a_build() {
    let host = StubHost::new();
    restart(&host);
    until("a build to start", || host.started() == 1).await;
    host.release_one();
    until_settled(&host).await;
    assert_eq!(host.killed.load(Ordering::SeqCst), 0);
    assert!(matches!(host.build_state(), BuildState::Ready { .. }));
}
