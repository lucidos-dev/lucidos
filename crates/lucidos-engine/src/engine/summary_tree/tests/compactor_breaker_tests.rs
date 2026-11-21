//! A model that is down gets a few calls, not one per owed scope. The waits
//! here are tens of milliseconds, so the tests run on real time: paused time
//! would jump the database pool's own timeouts.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::json;
use uuid::Uuid;

use super::compactor_tests::{
    assert_every_scope_complete, backfill_frames, compactor_with_waits, drain_to_rest, progress_of,
    run_to_rest, seed_workspace,
};
use super::{age_events, line_response, seed, EchoProvider, COMPACTOR_MODEL, NO_ESTIMATE};
use crate::engine::event_bus::EventBus;
use crate::engine::summary_tree::breaker::Waits;
use crate::engine::summary_tree::compactor::{CompactionModel, Compactor, CompactorDeps, Drained};
use crate::engine::summary_tree::spend::StartEstimate;
use crate::engine::summary_tree::store;
use crate::engine::thread_events::EventChannel;
use crate::llm::provider::{LlmProvider, LlmResponse, Message};
use crate::test_support::{setup_test_db, teardown_test_db};

/// A model that fails every call while `down`, and answers like
/// [`EchoProvider`] after.
struct FlakyProvider {
    down: AtomicBool,
    /// What a call fails with while `down`.
    error: &'static str,
    calls: AtomicUsize,
    echo: EchoProvider,
}

impl FlakyProvider {
    fn new(down: bool) -> Arc<Self> {
        Self::failing_with(down, "connection refused")
    }

    fn failing_with(down: bool, error: &'static str) -> Arc<Self> {
        Arc::new(Self {
            down: AtomicBool::new(down),
            error,
            calls: AtomicUsize::new(0),
            echo: EchoProvider::new(),
        })
    }

    fn down() -> Arc<Self> {
        Self::new(true)
    }

    fn up() -> Arc<Self> {
        Self::new(false)
    }

    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }

    fn set_down(&self, down: bool) {
        self.down.store(down, Ordering::SeqCst);
    }
}

#[async_trait::async_trait]
impl LlmProvider for FlakyProvider {
    async fn chat(
        &self,
        messages: Vec<Message>,
        tools: Vec<crate::llm::provider::ToolDefinition>,
        selection: crate::llm::ModelSelection<'_>,
        system_prompt: Option<&str>,
        on_token: Option<crate::llm::provider::TokenCallback>,
        call: crate::llm::metered::CallToken,
    ) -> Result<LlmResponse, Box<dyn std::error::Error + Send + Sync>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if self.down.load(Ordering::SeqCst) {
            return Err(self.error.into());
        }
        self.echo
            .chat(messages, tools, selection, system_prompt, on_token, call)
            .await
    }

    fn default_model(&self) -> &str {
        "flaky-model"
    }
}

/// A model that always answers with nothing, so every node fails while the
/// provider is fine.
struct SilentProvider;

#[async_trait::async_trait]
impl LlmProvider for SilentProvider {
    async fn chat(
        &self,
        _messages: Vec<Message>,
        _tools: Vec<crate::llm::provider::ToolDefinition>,
        _selection: crate::llm::ModelSelection<'_>,
        _system_prompt: Option<&str>,
        _on_token: Option<crate::llm::provider::TokenCallback>,
        _call: crate::llm::metered::CallToken,
    ) -> Result<LlmResponse, Box<dyn std::error::Error + Send + Sync>> {
        Ok(line_response(String::new()))
    }

    fn default_model(&self) -> &str {
        "silent-model"
    }
}

/// A compactor model the test can switch, and the notices it was asked to
/// send.
struct SwitchDeps {
    route: Mutex<(String, Arc<dyn LlmProvider>)>,
    flex: bool,
    notices: AtomicUsize,
}

impl SwitchDeps {
    fn on(model: &str, provider: Arc<dyn LlmProvider>) -> Arc<Self> {
        Self::with_flex(model, provider, false)
    }

    /// A route that serves the model on the Flex tier.
    fn on_flex(model: &str, provider: Arc<dyn LlmProvider>) -> Arc<Self> {
        Self::with_flex(model, provider, true)
    }

    fn with_flex(model: &str, provider: Arc<dyn LlmProvider>, flex: bool) -> Arc<Self> {
        Arc::new(Self {
            route: Mutex::new((model.to_string(), provider)),
            flex,
            notices: AtomicUsize::new(0),
        })
    }

    fn switch_to(&self, model: &str, provider: Arc<dyn LlmProvider>) {
        *self.route.lock().unwrap() = (model.to_string(), provider);
    }

    fn notices(&self) -> usize {
        self.notices.load(Ordering::SeqCst)
    }
}

#[async_trait::async_trait]
impl CompactorDeps for SwitchDeps {
    async fn model(&self) -> Result<CompactionModel, String> {
        let (model, provider) = self.route.lock().unwrap().clone();
        Ok(CompactionModel {
            provider,
            lane: format!("test/{model}"),
            model,
            effort: None,
            deadline: Duration::from_secs(5),
            flex: self.flex,
        })
    }

    fn read_artifact(&self, _path: &str, _commit: &str) -> Option<String> {
        None
    }

    async fn backfill_estimate(&self) -> Result<StartEstimate, String> {
        Err(NO_ESTIMATE.to_string())
    }

    async fn notify_model_down(&self, _model: &str) {
        self.notices.fetch_add(1, Ordering::SeqCst);
    }
}

const SLOW: Duration = Duration::from_secs(60);

fn waits(first_ms: u64, longest_ms: u64) -> Waits {
    Waits::new(
        Duration::from_millis(first_ms),
        Duration::from_millis(longest_ms),
    )
}

/// Until `ready` holds, or fail after `limit`.
async fn until(limit: Duration, what: &str, mut ready: impl FnMut() -> bool) {
    let deadline = std::time::Instant::now() + limit;
    while !ready() {
        assert!(std::time::Instant::now() < deadline, "never saw {what}");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

fn failing_model(c: &Compactor) -> Option<String> {
    c.backfill_progress().and_then(|p| p.failing_model)
}

/// The compactor, owed `threads` scopes of one long message each, on `deps`.
async fn compactor_owing(
    pool: &sqlx::PgPool,
    bus: &EventBus,
    threads: usize,
    deps: Arc<dyn CompactorDeps>,
    waits: Waits,
) -> Arc<Compactor> {
    for i in 0..threads {
        seed(
            bus,
            Uuid::new_v4(),
            EventChannel::Chat,
            vec![
                json!({"type": "MessageReceived", "mode": "human",
                       "text": format!("request {i}: {}", "a long paste. ".repeat(50))}),
                json!({"type": "ResponseGenerated", "text": "ok"}),
            ],
        )
        .await;
    }
    age_events(pool).await;
    compactor_with_waits(pool, bus, deps, waits)
}

/// Far more scopes are owed than workers take at once, and the model fails
/// every call. Once the breaker opens, workers park on their nodes. The calls
/// stop at about one wave, then one probe per wait.
#[tokio::test]
async fn a_dead_model_gets_a_wave_of_calls_and_then_one_probe_per_wait() {
    let provider = FlakyProvider::down();
    let wave = calls_after_the_trip_are_one_probe_per_wait(
        provider.clone(),
        SwitchDeps::on(COMPACTOR_MODEL, provider),
    )
    .await;
    assert!(
        wave < SCOPES,
        "{wave} calls went out before the breaker held {SCOPES} scopes back"
    );
}

/// A Flex route that refuses for capacity while down. Each node spends its
/// Flex tries before the trip, so no wave bound holds here. The probe asks
/// for the standard tier, so a refusal still backs off rather than handing
/// the probe to the next node at once.
#[tokio::test]
async fn a_flex_route_that_is_down_still_gets_one_probe_per_wait() {
    let provider = FlakyProvider::failing_with(true, "503 Service Unavailable: overloaded");
    calls_after_the_trip_are_one_probe_per_wait(
        provider.clone(),
        SwitchDeps::on_flex(COMPACTOR_MODEL, provider),
    )
    .await;
}

const SCOPES: usize = 300;

/// Run [`SCOPES`] owed scopes on a dead model, and return the calls that went
/// out before the breaker held them back.
async fn calls_after_the_trip_are_one_probe_per_wait(
    provider: Arc<FlakyProvider>,
    deps: Arc<SwitchDeps>,
) -> usize {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let c = compactor_owing(&pool, &bus, SCOPES, deps.clone(), waits(300, 300)).await;

    c.start();
    until(Duration::from_secs(120), "the breaker opening", || {
        failing_model(&c).is_some()
    })
    .await;
    assert_eq!(failing_model(&c).as_deref(), Some(COMPACTOR_MODEL));
    tokio::time::sleep(Duration::from_millis(250)).await;
    let wave = provider.calls();

    tokio::time::sleep(Duration::from_millis(1_200)).await;
    let probes = provider.calls() - wave;
    assert!(
        probes <= 6,
        "{probes} calls in 1.2 s, with one probe per 0.3 s allowed"
    );
    c.pause();
    teardown_test_db(&db_name).await;
    wave
}

/// The first answer closes the breaker, and the build finishes.
#[tokio::test]
async fn a_recovered_model_closes_the_breaker_and_the_build_finishes() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let fixture = seed_workspace(&pool, &bus).await;
    let mut rx = bus.subscribe();
    let provider = FlakyProvider::down();
    let deps = SwitchDeps::on(COMPACTOR_MODEL, provider.clone());
    let c = compactor_with_waits(&pool, &bus, deps, waits(100, 100));

    let recover = {
        let (c, provider) = (c.clone(), provider.clone());
        tokio::spawn(async move {
            until(Duration::from_secs(60), "the breaker opening", || {
                failing_model(&c).is_some()
            })
            .await;
            provider.set_down(false);
        })
    };
    c.seed().await.unwrap();
    tokio::time::timeout(Duration::from_secs(60), drain_to_rest(&c))
        .await
        .expect("the drain finishes once the model answers");
    recover.await.unwrap();
    assert_eq!(failing_model(&c), None, "a close clears the failing model");

    run_to_rest(&c).await;
    assert_every_scope_complete(&pool, &fixture).await;
    assert!(store::is_ready(&pool).await.unwrap());
    let progress = progress_of(&backfill_frames(&mut rx));
    let named = progress
        .iter()
        .position(|p| p.failing_model.as_deref() == Some(COMPACTOR_MODEL))
        .expect("a frame names the failing model");
    assert!(
        progress[named..].iter().any(|p| p.failing_model.is_none()),
        "a frame announces the close"
    );
    teardown_test_db(&db_name).await;
}

/// A switch to Classic pauses the compactor. Nodes parked on the breaker stop
/// then, rather than probing the dead model for as long as it stays down.
#[tokio::test]
async fn a_pause_stops_the_nodes_parked_on_the_breaker() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let provider = FlakyProvider::down();
    let deps = SwitchDeps::on(COMPACTOR_MODEL, provider.clone());
    let c = compactor_owing(&pool, &bus, 100, deps, waits(50, 50)).await;

    c.start();
    until(Duration::from_secs(120), "the breaker opening", || {
        failing_model(&c).is_some()
    })
    .await;
    c.pause();
    // Calls already out may still finish.
    tokio::time::sleep(Duration::from_millis(300)).await;
    let at_pause = provider.calls();
    tokio::time::sleep(Duration::from_millis(1_000)).await;
    assert_eq!(
        provider.calls(),
        at_pause,
        "no probe goes out after the pause"
    );
    teardown_test_db(&db_name).await;
}

/// With no model preference, the default follows the configured providers,
/// and a change there announces no preference. The probe looks at the model
/// again, so the parked nodes go on to the new default.
#[tokio::test]
async fn a_moved_default_model_releases_the_parked_nodes_at_the_probe() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let fixture = seed_workspace(&pool, &bus).await;
    let bad = FlakyProvider::down();
    let good = FlakyProvider::up();
    let deps = SwitchDeps::on("model-a", bad.clone());
    let c = compactor_with_waits(&pool, &bus, deps.clone(), waits(100, 100));

    c.seed().await.unwrap();
    let drained = {
        let c = c.clone();
        tokio::spawn(async move { drain_to_rest(&c).await })
    };
    until(Duration::from_secs(60), "the breaker opening", || {
        failing_model(&c).as_deref() == Some("model-a")
    })
    .await;
    assert!(
        !drained.is_finished(),
        "the drain waits on the open breaker"
    );

    deps.switch_to("model-b", good.clone());
    tokio::time::timeout(Duration::from_secs(60), drained)
        .await
        .expect("the nodes go on to the new default")
        .unwrap();

    run_to_rest(&c).await;
    assert_every_scope_complete(&pool, &fixture).await;
    assert!(good.calls() > 0);
    assert_eq!(failing_model(&c), None, "the old model is no longer named");
    teardown_test_db(&db_name).await;
}

/// Picking another model releases the parked nodes at once, with no wait for
/// the old model's probe. The wait here would outlast the test.
#[tokio::test]
async fn a_model_change_releases_the_parked_nodes_onto_the_new_model() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let fixture = seed_workspace(&pool, &bus).await;
    let bad = FlakyProvider::down();
    let good = FlakyProvider::up();
    let deps = SwitchDeps::on("model-a", bad.clone());
    let c = compactor_with_waits(&pool, &bus, deps.clone(), Waits::new(SLOW, SLOW));

    c.seed().await.unwrap();
    let drained = {
        let c = c.clone();
        tokio::spawn(async move { drain_to_rest(&c).await })
    };
    until(Duration::from_secs(60), "the breaker opening", || {
        failing_model(&c).as_deref() == Some("model-a")
    })
    .await;
    assert!(
        !drained.is_finished(),
        "the drain waits on the open breaker"
    );

    deps.switch_to("model-b", good.clone());
    c.model_changed().await;
    assert_eq!(failing_model(&c), None, "the old model is no longer named");
    tokio::time::timeout(Duration::from_secs(60), drained)
        .await
        .expect("the nodes go on to the new model")
        .unwrap();

    run_to_rest(&c).await;
    assert_every_scope_complete(&pool, &fixture).await;
    assert!(good.calls() > 0);
    teardown_test_db(&db_name).await;
}

/// One notice per outage, sent when the wait reaches its cap, and a close
/// re-arms it.
#[tokio::test]
async fn an_outage_sends_one_notice_and_a_recovery_re_arms_it() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let provider = FlakyProvider::down();
    let deps = SwitchDeps::on(COMPACTOR_MODEL, provider.clone());
    // Enough nodes to outlast the trip and the probes up to the cap.
    let c = compactor_owing(&pool, &bus, 12, deps.clone(), waits(20, 80)).await;

    for outage in 1..=2 {
        provider.set_down(true);
        c.seed().await.unwrap();
        let drain = {
            let c = c.clone();
            tokio::spawn(async move { drain_to_rest(&c).await })
        };
        until(Duration::from_secs(60), "the notice", || {
            deps.notices() == outage
        })
        .await;
        tokio::time::sleep(Duration::from_millis(500)).await;
        assert_eq!(deps.notices(), outage, "one notice per outage");

        provider.set_down(false);
        tokio::time::timeout(Duration::from_secs(60), drain)
            .await
            .expect("the drain finishes once the model answers")
            .unwrap();
        // Nodes that failed before the trip left their scopes incomplete.
        // Seeding again builds them, and the next outage starts on a clean
        // tree.
        run_to_rest(&c).await;
        sqlx::query("DELETE FROM summary_tree_nodes")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("DELETE FROM summary_tree_scopes")
            .execute(&pool)
            .await
            .unwrap();
    }
    teardown_test_db(&db_name).await;
}

/// A node that fails among working ones holds up its own scope only: the
/// breaker stays closed, and nothing waits for it.
#[tokio::test]
async fn a_failing_node_among_working_ones_never_opens_the_breaker() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    seed_workspace(&pool, &bus).await;
    let poisoned = Uuid::new_v4();
    seed(
        &bus,
        poisoned,
        EventChannel::Chat,
        vec![
            json!({"type": "MessageReceived", "mode": "human",
                   "text": format!("POISON {}", "pill ".repeat(200))}),
            json!({"type": "ResponseGenerated", "text": "ok"}),
        ],
    )
    .await;
    age_events(&pool).await;
    // The real waits: a breaker that opened would hold the drain for 10 s.
    let deps = SwitchDeps::on(COMPACTOR_MODEL, Arc::new(EchoProvider::new()));
    let c = compactor_with_waits(&pool, &bus, deps, Waits::default());

    c.seed().await.unwrap();
    let outcomes = tokio::time::timeout(Duration::from_secs(8), drain_to_rest(&c))
        .await
        .expect("no node waits on a breaker");
    assert!(outcomes.iter().any(|(_, o)| *o == Drained::Failed));
    assert_eq!(failing_model(&c), None);
    teardown_test_db(&db_name).await;
}

/// An empty answer means the provider is up, so a build of empty answers
/// fails node by node and never opens the breaker.
#[tokio::test]
async fn empty_answers_never_open_the_breaker() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    seed_workspace(&pool, &bus).await;
    let deps = SwitchDeps::on(COMPACTOR_MODEL, Arc::new(SilentProvider));
    let c = compactor_with_waits(&pool, &bus, deps, Waits::default());

    c.seed().await.unwrap();
    let outcomes = tokio::time::timeout(Duration::from_secs(8), drain_to_rest(&c))
        .await
        .expect("no node waits on a breaker");
    assert!(outcomes.iter().any(|(_, o)| *o == Drained::Failed));
    assert_eq!(failing_model(&c), None);
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn spend_caps_park_without_lane_breaker_or_flex_pressure() {
    struct Capped {
        calls: AtomicUsize,
        refuse: AtomicBool,
        echo: EchoProvider,
    }
    #[async_trait::async_trait]
    impl LlmProvider for Capped {
        fn default_model(&self) -> &str {
            COMPACTOR_MODEL
        }
        fn resolved_provider_kind(
            &self,
            _: &crate::llm::ModelSelection<'_>,
        ) -> Result<Option<crate::llm::ProviderKind>, Box<dyn std::error::Error + Send + Sync>>
        {
            Ok(Some(crate::llm::ProviderKind::OpenAi))
        }
        async fn chat(
            &self,
            messages: Vec<Message>,
            tools: Vec<crate::llm::provider::ToolDefinition>,
            selection: crate::llm::ModelSelection<'_>,
            system: Option<&str>,
            tokens: Option<crate::llm::TokenCallback>,
            call: crate::llm::metered::CallToken,
        ) -> Result<LlmResponse, Box<dyn std::error::Error + Send + Sync>> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if self.refuse.load(Ordering::SeqCst) {
                return Err(crate::llm::spend_cap::SpendCapReached {
                    provider: crate::llm::ProviderKind::OpenAi,
                    resets_at: None,
                    message: "spent".into(),
                }
                .into());
            }
            self.echo
                .chat(messages, tools, selection, system, tokens, call)
                .await
        }
    }
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let provider = Arc::new(Capped {
        calls: AtomicUsize::new(0),
        refuse: AtomicBool::new(true),
        echo: EchoProvider::new(),
    });
    let deps = SwitchDeps::on_flex(COMPACTOR_MODEL, provider.clone());
    let c = compactor_owing(&pool, &bus, 1, deps.clone(), waits(15, 30)).await;
    let lane = c.lane(&format!("test/{COMPACTOR_MODEL}"));
    let initial = lane.limit();
    c.seed().await.unwrap();
    let outcomes = drain_to_rest(&c).await;
    assert!(outcomes
        .iter()
        .any(|(_, outcome)| matches!(outcome, Drained::SpendCapped(..))));
    assert_eq!(lane.limit(), initial);
    assert!(failing_model(&c).is_none());
    assert_eq!(deps.notices(), 0);
    assert!(c
        .backfill_progress()
        .unwrap()
        .spend_cap_held_until
        .is_some());
    let calls = provider.calls.load(Ordering::SeqCst);
    assert!(
        c.drain_next().await.is_none(),
        "a capped scope is parked, not queued for retry"
    );
    assert_eq!(provider.calls.load(Ordering::SeqCst), calls);
    provider.refuse.store(false, Ordering::SeqCst);
    crate::engine::model_call::spend_cap::clear(
        &bus,
        crate::llm::ProviderKind::OpenAi,
        crate::engine::thread_events::MessageOrigin::Device {
            device_id: "test-device".into(),
        },
    )
    .await;
    c.spend_cap_hold_ended().await;
    run_to_rest(&c).await;
    assert!(failing_model(&c).is_none());
    assert!(c
        .backfill_progress()
        .is_none_or(|progress| progress.spend_cap_held_until.is_none()));
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn a_model_change_during_a_call_never_parks_on_its_late_spend_cap() {
    struct DelayedCap {
        started: tokio::sync::Notify,
        release: tokio::sync::Semaphore,
    }
    #[async_trait::async_trait]
    impl LlmProvider for DelayedCap {
        fn default_model(&self) -> &str {
            "model-a"
        }
        fn resolved_provider_kind(
            &self,
            _: &crate::llm::ModelSelection<'_>,
        ) -> Result<Option<crate::llm::ProviderKind>, Box<dyn std::error::Error + Send + Sync>>
        {
            Ok(Some(crate::llm::ProviderKind::OpenAi))
        }
        async fn chat(
            &self,
            _: Vec<Message>,
            _: Vec<crate::llm::provider::ToolDefinition>,
            _: crate::llm::ModelSelection<'_>,
            _: Option<&str>,
            _: Option<crate::llm::TokenCallback>,
            _: crate::llm::metered::CallToken,
        ) -> Result<LlmResponse, Box<dyn std::error::Error + Send + Sync>> {
            self.started.notify_one();
            assert!(self.release.acquire().await.is_err());
            Err(crate::llm::spend_cap::SpendCapReached {
                provider: crate::llm::ProviderKind::OpenAi,
                resets_at: None,
                message: "spent".into(),
            }
            .into())
        }
    }
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let fixture = seed_workspace(&pool, &bus).await;
    let old = Arc::new(DelayedCap {
        started: tokio::sync::Notify::new(),
        release: tokio::sync::Semaphore::new(0),
    });
    let good = FlakyProvider::up();
    let deps = SwitchDeps::on("model-a", old.clone());
    let c = compactor_with_waits(&pool, &bus, deps.clone(), Waits::new(SLOW, SLOW));
    c.seed().await.unwrap();
    let draining = {
        let c = c.clone();
        tokio::spawn(async move { drain_to_rest(&c).await })
    };
    tokio::time::timeout(Duration::from_secs(10), old.started.notified())
        .await
        .expect("the old provider's call started");
    deps.switch_to("model-b", good.clone());
    c.model_changed().await;
    old.release.close();
    tokio::time::timeout(Duration::from_secs(10), draining)
        .await
        .expect("the old call returns")
        .unwrap();
    tokio::time::timeout(Duration::from_secs(10), run_to_rest(&c))
        .await
        .expect("the healthy model drains without the old hold expiring");
    assert_every_scope_complete(&pool, &fixture).await;
    assert!(good.calls() > 0);
    assert!(bus
        .spend_cap_holds()
        .lock()
        .await
        .refusal(crate::llm::ProviderKind::OpenAi)
        .is_some());
    assert!(c
        .backfill_progress()
        .is_none_or(|progress| progress.spend_cap_held_until.is_none()));
    teardown_test_db(&db).await;
}
