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
    deferral: AtomicBool,
    needed: AtomicBool,
    pool: Option<sqlx::PgPool>,
    head: AtomicUsize,
    snapshots: Mutex<Vec<usize>>,
    retried_failures: Mutex<Vec<Option<String>>>,
}

impl StubHost {
    fn new() -> Arc<Self> {
        Self::with_pool(None)
    }

    fn with_pool(pool: Option<sqlx::PgPool>) -> Arc<Self> {
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
            deferral: AtomicBool::new(false),
            needed: AtomicBool::new(true),
            pool,
            head: AtomicUsize::new(0),
            snapshots: Mutex::new(Vec::new()),
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
    async fn deferred_build_needed(&self) -> bool {
        self.needed.load(Ordering::SeqCst)
    }
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

    async fn run_build(&self, previous_failure: Option<&str>, _: u64) -> FinishedBuild {
        self.retried_failures
            .lock()
            .unwrap()
            .push(previous_failure.map(str::to_string));
        let n = self.started.fetch_add(1, Ordering::SeqCst) + 1;
        self.snapshots
            .lock()
            .unwrap()
            .push(self.head.load(Ordering::SeqCst));
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

    async fn automatic_deferral(&self) -> super::BuildDeferral {
        if let Some(pool) = &self.pool {
            return crate::engine::build_deferral::automatic_build_deferral(pool).await;
        }
        if self.deferral.load(Ordering::SeqCst) {
            super::BuildDeferral::Defer(crate::engine::build_deferral::ActiveThreads {
                count: 1,
                threads: vec![],
            })
        } else {
            super::BuildDeferral::Admit
        }
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

#[tokio::test]
async fn a_superseded_build_cannot_publish_progress() {
    let host = StubHost::new();
    join_or_start(&host);
    until("first build", || host.started() == 1).await;
    let old = host.build.slot.lock().unwrap().generation;
    restart(&host);
    assert!(super::with_current_build(host.as_ref(), old, || host
        .set_build_state(BuildState::Verifying))
    .is_none());
    assert!(matches!(host.build_state(), BuildState::Building { .. }));
    until("replacement build", || host.started() == 2).await;
    host.release_one();
    until_settled(&host).await;
}

#[tokio::test]
async fn automatic_requests_coalesce_until_admitted_and_manual_overrides() {
    let host = StubHost::new();
    host.deferral.store(true, Ordering::SeqCst);
    for _ in 0..3 {
        super::automatic(&host).await;
    }
    assert_eq!(host.started(), 0);
    let mut state = host.build_state();
    if let BuildState::Deferred(deferred) = &mut state {
        deferred.since = std::time::Instant::now() - Duration::from_secs(365 * 24 * 3600);
    }
    host.set_build_state(state);
    let since = host.build_state().deferred().unwrap().since;
    super::automatic(&host).await;
    super::start_deferred_build_if_admitted(&host).await;
    assert_eq!(host.build_state().deferred().unwrap().since, since);
    assert_eq!(host.started(), 0);
    join_or_start(&host);
    until("manual override", || host.started() == 1).await;
    host.release_one();
    until_settled(&host).await;
    assert_eq!(host.started(), 1);
    assert_eq!(host.killed.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn a_joined_follow_up_waits_without_hiding_ready() {
    let host = StubHost::new();
    host.covers_head.store(false, Ordering::SeqCst);
    super::automatic(&host).await;
    until("first build", || host.started() == 1).await;
    host.deferral.store(true, Ordering::SeqCst);
    super::automatic(&host).await;
    host.release_one();
    until_settled(&host).await;
    assert!(matches!(
        host.build_state(),
        BuildState::Ready {
            deferred: Some(_),
            ..
        }
    ));
    assert_eq!(host.started(), 1);
    super::start_deferred_build_if_admitted(&host).await;
    assert_eq!(host.started(), 1);
    host.deferral.store(false, Ordering::SeqCst);
    super::start_deferred_build_if_admitted(&host).await;
    until("released follow-up", || host.started() == 2).await;
    host.release_one();
    until_settled(&host).await;
    assert_eq!(host.killed.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn restart_overrides_a_hold_and_failed_follow_up_keeps_diagnostics() {
    let host = StubHost::new();
    host.fail.store(true, Ordering::SeqCst);
    host.covers_head.store(false, Ordering::SeqCst);
    super::automatic(&host).await;
    until("first build", || host.started() == 1).await;
    host.deferral.store(true, Ordering::SeqCst);
    super::automatic(&host).await;
    host.release_one();
    until_settled(&host).await;
    assert_eq!(host.build_state().as_wire(), "deferred");
    assert_eq!(host.build_state().failure().unwrap().summary, "broke at 1");
    restart(&host);
    until("explicit retry", || host.started() == 2).await;
    assert_eq!(
        host.retried_failures.lock().unwrap()[1].as_deref(),
        Some("broke at 1")
    );
    host.release_one();
    until_settled(&host).await;
}

/// Production preference/query + scheduler, with only the compiler substituted.
#[tokio::test]
async fn workspace_activity_holds_coalesces_and_releases_on_one_tick() {
    use crate::core::prefs;
    use crate::test_support::{seed_preference, setup_test_db, teardown_test_db};
    let (pool, db) = setup_test_db().await;
    let thread = uuid::Uuid::new_v4();
    sqlx::query("INSERT INTO thread_summaries(thread_id, title, status, is_coding_agent) VALUES ($1, 'Worker', 'waiting_for_user_answer', true)")
        .bind(thread).execute(&pool).await.unwrap();

    // Default workspaces still build with a blocked coding thread.
    let default_host = StubHost::with_pool(Some(pool.clone()));
    assert_eq!(
        super::automatic(&default_host).await,
        super::BuildDeferral::Admit
    );
    until("default build", || default_host.started() == 1).await;
    default_host.release_one();
    until_settled(&default_host).await;

    seed_preference(&pool, prefs::DEFER_VERSION_BUILDS.key(), "while-coding")
        .await
        .unwrap();
    let host = StubHost::with_pool(Some(pool.clone()));
    for head in 1..=3 {
        host.head.store(head, Ordering::SeqCst);
        assert!(matches!(
            super::automatic(&host).await,
            super::BuildDeferral::Defer(_)
        ));
    }
    assert_eq!(host.started(), 0);
    super::start_deferred_build_if_admitted(&host).await;
    assert_eq!(host.started(), 0);

    // A fresh host derives the same hold from durable workspace state.
    let fresh = StubHost::with_pool(Some(pool.clone()));
    super::automatic(&fresh).await;
    assert_eq!(fresh.build_state().as_wire(), "deferred");
    assert_eq!(fresh.started(), 0);

    sqlx::query("UPDATE thread_summaries SET status='idle' WHERE thread_id=$1")
        .bind(thread)
        .execute(&pool)
        .await
        .unwrap();
    super::start_deferred_build_if_admitted(&host).await;
    until("one tick starts the latest HEAD", || host.started() == 1).await;
    assert_eq!(*host.snapshots.lock().unwrap(), vec![3]);
    assert!(host.build_state().deferred().is_none());

    // Activity that begins during a build must leave that build alone.
    sqlx::query("UPDATE thread_summaries SET status='running' WHERE thread_id=$1")
        .bind(thread)
        .execute(&pool)
        .await
        .unwrap();
    super::automatic(&host).await;
    host.release_one();
    until_settled(&host).await;
    assert_eq!(host.started(), 1);
    assert_eq!(host.killed.load(Ordering::SeqCst), 0);
    assert_eq!(host.build_state().as_wire(), "ready");
    drop((host, fresh, default_host));
    teardown_test_db(&db).await;
}

#[test]
fn automatic_paths_use_admission_and_explicit_api_bypasses_it() {
    let follow = include_str!("follow_main.rs");
    let apply = include_str!("change_ops_emitters.rs");
    let api = include_str!("../../../lucidos-agents-api/src/api/history.rs");
    let tick = include_str!("frontend_refresh.rs");
    assert!(follow.contains("self.automatic_background_rebuild().await"));
    assert!(apply.contains("engine.automatic_background_rebuild().await.into()"));
    assert!(api.contains("RebuildMode::Join => state.engine.join_or_start_background_rebuild()"));
    assert!(api.contains("RebuildMode::Restart => state.engine.restart_background_rebuild()"));
    assert!(tick.contains("engine.start_deferred_build_if_admitted().await"));
}

#[tokio::test]
async fn a_peer_covering_head_clears_the_hold_and_keeps_ready() {
    for ready in [false, true] {
        let host = StubHost::new();
        let state = if ready {
            BuildState::ready_from(Some("older".into()))
        } else {
            BuildState::Idle
        };
        host.set_build_state(state.with_deferral());
        host.needed.store(false, Ordering::SeqCst);
        super::start_deferred_build_if_admitted(&host).await;
        assert_eq!(host.started(), 0);
        assert!(host.build_state().deferred().is_none());
        assert_eq!(
            host.build_state().as_wire(),
            if ready { "ready" } else { "idle" }
        );
    }
}
