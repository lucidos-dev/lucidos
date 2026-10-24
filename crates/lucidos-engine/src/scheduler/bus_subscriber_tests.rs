//! The scheduler subscriber under a real lag.
//!
//! Every test subscribes, emits the real event, floods past `BUS_CAPACITY`,
//! and hands the first `recv` (a `Lagged`) to the step. No sleep and no timing:
//! the flood alone decides that the event is gone from the buffer.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

use serde_json::json;

use super::*;
use crate::core::PreferenceStore;
use crate::engine::event_bus::{EventBus, ReplayScope};
use crate::engine::thread_queue::{ThreadQueueKind, ThreadQueueRequest};
use crate::scheduler::user_tasks::EVENT_TRIGGER_DEPTH;
use crate::test_support::{flood_past_bus_capacity, setup_test_db, teardown_test_db};
use crate::triggers::{find_matching_event_triggers, TriggerRun};

/// One offer the step made to the trigger matcher.
#[derive(Debug, Clone)]
struct Offer {
    dispatch: TriggerDispatch,
    source_event_id: Uuid,
    excluded: HashSet<String>,
}

/// The effects the step asked for. With `queue` set it also stands in for the
/// Thread Queue: each trigger the matcher would wake gets the `ThreadQueued`
/// row a real submit writes before it returns.
struct Recorder {
    configs: TriggerConfigs,
    queue: Option<EventBus>,
    /// Registered on the first offer, as a trigger created mid-replay is.
    created_on_first_offer: Mutex<Option<TriggerConfig>>,
    offers: Mutex<Vec<Offer>>,
    backup_reloads: AtomicUsize,
}

impl Recorder {
    /// The triggers one offer wakes: the live matcher, minus the exclusions.
    fn woken(&self, offer: &Offer) -> Vec<String> {
        let configs = self.configs.read().unwrap();
        find_matching_event_triggers(
            &configs,
            &offer.dispatch.event_type,
            &offer.dispatch.payload,
            offer.dispatch.emitting_trigger_id.as_deref(),
        )
        .into_iter()
        .map(|t| t.id)
        .filter(|id| !offer.excluded.contains(id))
        .collect()
    }

    fn offers_for(&self, source_event_id: Uuid) -> Vec<Offer> {
        self.offers
            .lock()
            .unwrap()
            .iter()
            .filter(|o| o.source_event_id == source_event_id)
            .cloned()
            .collect()
    }

    /// Every trigger woken for one source event, across all offers.
    fn woken_for(&self, source_event_id: Uuid) -> Vec<String> {
        self.offers_for(source_event_id)
            .iter()
            .flat_map(|o| self.woken(o))
            .collect()
    }
}

#[async_trait]
impl SchedulerEffects for Arc<Recorder> {
    async fn apply_trigger_lifecycle(&self, _: &str, _: &str, _: &serde_json::Value) {}

    async fn rearm_cron_after_lag(&self) {}

    async fn reload_backup_schedule(&self) {
        self.backup_reloads.fetch_add(1, Ordering::SeqCst);
    }

    async fn dispatch_trigger(
        &self,
        dispatch: &TriggerDispatch,
        source_event_id: Uuid,
        excluded: &HashSet<String>,
    ) {
        let offer = Offer {
            dispatch: dispatch.clone(),
            source_event_id,
            excluded: excluded.clone(),
        };
        if let Some(bus) = &self.queue {
            for trigger_id in self.woken(&offer) {
                record_fire(bus, &trigger_id, source_event_id).await;
            }
        }
        if let Some(created) = self.created_on_first_offer.lock().unwrap().take() {
            self.configs
                .write()
                .unwrap()
                .insert(created.id.clone(), created);
        }
        self.offers.lock().unwrap().push(offer);
    }
}

/// The `ThreadQueued` row `ThreadQueue::submit` writes for an event-trigger fire.
async fn record_fire(bus: &EventBus, trigger_id: &str, source_event_id: Uuid) {
    let request = ThreadQueueRequest::EventTrigger {
        trigger_id: trigger_id.to_string(),
        event_type: "BuildObserved".into(),
        event_payload: json!({}),
        depth: 1,
        origin_thread_id: None,
        source_event_id: Some(source_event_id),
    };
    bus.emit(BusEvent::System(SystemEvent::ThreadQueued {
        entry_id: Uuid::new_v4(),
        kind: ThreadQueueKind::EventTrigger,
        trigger_id: Some(trigger_id.to_string()),
        trigger_name: None,
        thread_id: None,
        summary: "fire".into(),
        request: serde_json::to_value(request).unwrap(),
        requeued: false,
        actor: None,
    }))
    .await
    .unwrap();
}

/// An enabled trigger listening for `event_type`.
fn event_trigger(id: &str, event_type: &str) -> TriggerConfig {
    TriggerConfig {
        provider: None,
        id: id.to_string(),
        name: format!("Trigger {id}"),
        slug: format!("trigger-{id}"),
        schedule: vec![],
        timezone: "UTC".to_string(),
        run: TriggerRun::Script {
            path: "scripts/run.py".to_string(),
        },
        on: vec![crate::core::event_subscription::EventSubscription {
            event_type: event_type.to_string(),
            condition: None,
        }],
        paused: false,
        last_run: None,
        last_run_status: None,
        app_id: None,
        go_to_review: false,
        group_id: None,
        side_effect_grant: vec![],
        mcp_servers: vec![],
        plugin_id: None,
        model: None,
        reasoning_effort: None,
    }
}

/// A subscriber with a recorder, its bus receiver, and the group registry.
struct Harness {
    bus: EventBus,
    pool: PgPool,
    rx: tokio::sync::broadcast::Receiver<EmittedEvent>,
    recorder: Arc<Recorder>,
    groups: TriggerGroups,
    subscriber: BusSubscriber<Arc<Recorder>>,
}

impl Harness {
    /// Subscribe first, then build the subscriber, as the engine does.
    async fn new(pool: &PgPool, triggers: Vec<TriggerConfig>) -> Self {
        Self::build(pool, triggers, false).await
    }

    /// [`Self::new`] with the recorder recording fires as the queue does.
    async fn with_queue(pool: &PgPool, triggers: Vec<TriggerConfig>) -> Self {
        Self::build(pool, triggers, true).await
    }

    async fn build(pool: &PgPool, triggers: Vec<TriggerConfig>, queue: bool) -> Self {
        let (bus, _callback_rx) = EventBus::new(pool.clone());
        let (rx, lag_replay) = LagReplay::subscribe(&bus, ReplayScope::Workspace, None);
        let configs: TriggerConfigs = Arc::new(std::sync::RwLock::new(
            triggers.into_iter().map(|t| (t.id.clone(), t)).collect(),
        ));
        let recorder = Arc::new(Recorder {
            configs: configs.clone(),
            queue: queue.then(|| bus.clone()),
            created_on_first_offer: Mutex::default(),
            offers: Mutex::default(),
            backup_reloads: AtomicUsize::default(),
        });
        let groups: TriggerGroups = Arc::default();
        let subscriber = BusSubscriber::new(
            recorder.clone(),
            bus.clone(),
            configs,
            groups.clone(),
            lag_replay,
        );
        Self {
            bus,
            pool: pool.clone(),
            rx,
            recorder,
            groups,
            subscriber,
        }
    }

    /// Flood, then step on the first `recv`, which must be the lag.
    async fn lag(&mut self) {
        flood_past_bus_capacity(&self.bus);
        let received = self.rx.recv().await;
        assert!(
            matches!(received, Err(RecvError::Lagged(_))),
            "the flood must overrun the buffer"
        );
        assert!(self.subscriber.step(received).await.is_continue());
    }

    /// Step through the buffer until the frame for `event_id`, inclusive.
    async fn step_through(&mut self, event_id: Uuid) {
        loop {
            let received = self.rx.recv().await;
            let found = matches!(&received, Ok(frame) if frame.event_id == event_id);
            assert!(self.subscriber.step(received).await.is_continue());
            if found {
                return;
            }
        }
    }
}

fn build_observed(depth: u32) -> BusEvent {
    BusEvent::System(SystemEvent::DomainEvent {
        event_type: "BuildObserved".to_string(),
        payload: json!({"summary": "green"}),
        depth,
        transient: false,
        actor: None,
    })
}

async fn emit_build_observed(bus: &EventBus) -> Uuid {
    bus.emit(build_observed(0))
        .await
        .unwrap()
        .expect("a domain event is persisted")
        .event_id
}

/// I1. The event the trigger listens for is dropped by the lag, and the step
/// still offers it to the matcher, which wakes the trigger.
#[tokio::test]
async fn a_lag_still_fires_a_trigger_for_the_dropped_event() {
    let (pool, db) = setup_test_db().await;
    let mut h = Harness::new(&pool, vec![event_trigger("t-build", "BuildObserved")]).await;

    let event_id = emit_build_observed(&h.bus).await;
    h.lag().await;

    let offered = h.recorder.offers_for(event_id);
    assert_eq!(
        offered.len(),
        1,
        "the dropped event reaches the matcher once"
    );
    assert_eq!(offered[0].dispatch.event_type, "BuildObserved");
    assert_eq!(h.recorder.woken_for(event_id), vec!["t-build".to_string()]);

    drop(h);
    teardown_test_db(&db).await;
}

/// I2, backup half. A missed `PreferencesChanged` for the backup schedule
/// still re-arms the backup loop.
#[tokio::test]
async fn a_lag_still_reloads_the_backup_schedule() {
    let (pool, db) = setup_test_db().await;
    let mut h = Harness::new(&pool, vec![]).await;

    PreferenceStore::set(
        &h.pool,
        &h.bus,
        prefs::BACKUP_SCHEDULE.key(),
        "0 0 3 * * *",
        None,
    )
    .await
    .unwrap();
    h.lag().await;

    assert_eq!(h.recorder.backup_reloads.load(Ordering::SeqCst), 1);

    drop(h);
    teardown_test_db(&db).await;
}

/// I2, group half. The registry's only live writer is this subscriber, so a
/// missed `TriggerGroupCreated` would leave the panel without the group.
#[tokio::test]
async fn a_lag_still_applies_a_dropped_group_change() {
    let (pool, db) = setup_test_db().await;
    let mut h = Harness::new(&pool, vec![]).await;

    h.bus
        .emit(BusEvent::System(SystemEvent::TriggerGroupCreated {
            group_id: "g-morning".into(),
            payload: json!({"group_id": "g-morning", "name": "Morning"}),
            actor: None,
        }))
        .await
        .unwrap();
    h.lag().await;

    let name = h
        .groups
        .read()
        .unwrap()
        .get("g-morning")
        .map(|g| g.name.clone());
    assert_eq!(name.as_deref(), Some("Morning"));

    drop(h);
    teardown_test_db(&db).await;
}

/// I3. A frame handled live, then swept into a lag window, fires once.
#[tokio::test]
async fn a_frame_fired_live_is_not_fired_again_by_the_replay() {
    let (pool, db) = setup_test_db().await;
    let mut h = Harness::with_queue(&pool, vec![event_trigger("t-build", "BuildObserved")]).await;

    let event_id = emit_build_observed(&h.bus).await;
    h.step_through(event_id).await;
    h.lag().await;

    assert_eq!(
        h.recorder.woken_for(event_id),
        vec!["t-build".to_string()],
        "one fire in total, live"
    );
    drop(h);
    teardown_test_db(&db).await;
}

/// I3. A recorded fire excludes its trigger from the replay.
#[tokio::test]
async fn a_recorded_fire_excludes_its_trigger_from_the_replay() {
    let (pool, db) = setup_test_db().await;
    let mut h = Harness::new(&pool, vec![event_trigger("t-build", "BuildObserved")]).await;

    let event_id = emit_build_observed(&h.bus).await;
    record_fire(&h.bus, "t-build", event_id).await;
    h.lag().await;

    assert!(h.recorder.woken_for(event_id).is_empty());
    drop(h);
    teardown_test_db(&db).await;
}

/// I3. A frame still in the buffer after the lag was covered by the replay,
/// so it reaches the matcher once, not twice.
#[tokio::test]
async fn a_late_buffered_frame_the_replay_covered_is_skipped() {
    let (pool, db) = setup_test_db().await;
    let mut h = Harness::new(&pool, vec![event_trigger("t-build", "BuildObserved")]).await;

    flood_past_bus_capacity(&h.bus);
    let late = emit_build_observed(&h.bus).await;
    let received = h.rx.recv().await;
    assert!(matches!(received, Err(RecvError::Lagged(_))));
    assert!(h.subscriber.step(received).await.is_continue());
    h.step_through(late).await;

    assert_eq!(h.recorder.offers_for(late).len(), 1);
    drop(h);
    teardown_test_db(&db).await;
}

/// I4. A trigger's own fire emitted the event. The replay names it, so the
/// matcher still leaves it asleep.
#[tokio::test]
async fn a_replay_never_wakes_a_trigger_with_its_own_fire() {
    let (pool, db) = setup_test_db().await;
    let mut h = Harness::new(&pool, vec![event_trigger("t-build", "BuildObserved")]).await;

    let event_id = h
        .bus
        .emit_as_trigger(build_observed(1), Some("t-build".into()))
        .await
        .unwrap()
        .expect("persisted")
        .event_id;
    h.lag().await;

    let offered = h.recorder.offers_for(event_id);
    assert_eq!(offered.len(), 1);
    assert_eq!(
        offered[0].dispatch.emitting_trigger_id.as_deref(),
        Some("t-build")
    );
    assert!(h.recorder.woken_for(event_id).is_empty());
    drop(h);
    teardown_test_db(&db).await;
}

/// I4. A replayed event keeps its chain depth, so the depth cap still
/// engages for a chain that passed through a lag.
#[tokio::test]
async fn a_replay_dispatches_at_the_live_depth() {
    let (pool, db) = setup_test_db().await;
    let mut h = Harness::new(&pool, vec![event_trigger("t-build", "BuildObserved")]).await;

    let event_id = h
        .bus
        .emit(build_observed(3))
        .await
        .unwrap()
        .expect("persisted")
        .event_id;
    h.lag().await;

    let offered = h.recorder.offers_for(event_id);
    assert_eq!(offered.len(), 1);
    assert_eq!(offered[0].dispatch.depth, 3);
    drop(h);
    teardown_test_db(&db).await;
}

/// I6. A trigger created after the event committed did not exist when the
/// event happened, so the replay does not wake it.
#[tokio::test]
async fn a_replay_never_wakes_a_trigger_created_after_the_event() {
    let (pool, db) = setup_test_db().await;
    let mut h = Harness::new(&pool, vec![event_trigger("t-new", "BuildObserved")]).await;

    let event_id = emit_build_observed(&h.bus).await;
    h.bus
        .emit(BusEvent::System(SystemEvent::TriggerCreated {
            trigger_id: "t-new".into(),
            payload: json!({"id": "t-new"}),
            actor: None,
        }))
        .await
        .unwrap();
    h.lag().await;

    let offered = h.recorder.offers_for(event_id);
    assert_eq!(offered.len(), 1);
    assert!(offered[0].excluded.contains("t-new"));
    assert!(h.recorder.woken_for(event_id).is_empty());
    drop(h);
    teardown_test_db(&db).await;
}

/// The stored row of `event_id`, as the replay reads it.
async fn stored_row(pool: &PgPool, event_id: Uuid) -> StoredTriggerCarrier {
    sqlx::query_as(
        "SELECT id, sequence, event_type, payload, thread_id, trigger_chain_depth, \
                emitting_trigger_id \
         FROM events WHERE id = $1",
    )
    .bind(event_id)
    .fetch_one(pool)
    .await
    .unwrap()
}

/// I4 per carrier. A replay builds the same dispatch from the row as the live
/// frame gave. Each carrier is emitted inside a fire, so every field is set.
#[tokio::test]
async fn a_stored_row_dispatches_exactly_like_its_live_frame() {
    use crate::engine::thread_events::{
        ActorMode, EventChannel, EventMeta, MessageOrigin, ThreadEvent,
    };

    let (pool, db) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());
    let mut rx = bus.subscribe();

    let carriers = [
        BusEvent::Thread {
            thread_id: Uuid::new_v4(),
            event: ThreadEvent::MessageReceived {
                provider: None,
                voice_session_id: None,
                wake_mode: None,
                text: "fired".into(),
                user_image_hashes: vec![],
                device_id: None,
                image_description: None,
                parent_thread_id: None,
                spawning_event_id: None,
                mode: ActorMode::Human,
                model: None,
                reasoning_effort: None,
                origin: None,
            },
            meta: EventMeta {
                channel: Some(EventChannel::Chat),
                ..EventMeta::NONE
            },
        },
        BusEvent::System(SystemEvent::TriggerExecuted {
            trigger_id: "t-1".into(),
            payload: json!({"last_run": "now"}),
        }),
        BusEvent::System(SystemEvent::DomainEvent {
            event_type: "BuildObserved".into(),
            payload: json!({"summary": "green"}),
            depth: 2,
            transient: false,
            actor: Some(MessageOrigin::Device {
                device_id: "device-1".into(),
            }),
        }),
    ];

    for carrier in carriers {
        let event_id = EVENT_TRIGGER_DEPTH
            .scope(2, bus.emit_as_trigger(carrier, Some("t-1".into())))
            .await
            .unwrap()
            .expect("persisted")
            .event_id;
        let live = loop {
            let frame = rx.recv().await.unwrap();
            if frame.event_id == event_id {
                break frame;
            }
        };
        let live = trigger_dispatch(&live).expect("a trigger carrier");
        let stored = stored_trigger_dispatch(&stored_row(&pool, event_id).await)
            .expect("the stored row is a trigger carrier too");
        assert_eq!(stored, live, "{} differs on replay", live.event_type);
        assert_eq!(stored.depth, 2);
        assert_eq!(stored.emitting_trigger_id.as_deref(), Some("t-1"));
    }

    teardown_test_db(&db).await;
}

/// A frame that reached the subscriber live is decided once, live. A trigger
/// paused then, and resumed before a later lag, does not fire for it.
#[tokio::test]
async fn a_replay_never_redecides_a_frame_seen_live() {
    let (pool, db) = setup_test_db().await;
    let mut paused = event_trigger("t-build", "BuildObserved");
    paused.paused = true;
    let mut h = Harness::new(&pool, vec![paused]).await;

    let event_id = emit_build_observed(&h.bus).await;
    h.step_through(event_id).await;
    assert_eq!(
        h.recorder.offers_for(event_id).len(),
        1,
        "the live offer, decided while the trigger was paused"
    );
    h.recorder
        .configs
        .write()
        .unwrap()
        .get_mut("t-build")
        .unwrap()
        .paused = false;
    h.lag().await;

    // `woken_for` would re-decide the live offer against today's registry, so
    // count offers instead: the replay must not have made a second one.
    assert_eq!(
        h.recorder.offers_for(event_id).len(),
        1,
        "the replay offered the frame seen live a second time"
    );
    drop(h);
    teardown_test_db(&db).await;
}

/// A trigger registered while a replay runs did not exist when the replayed
/// events happened, so the replay does not wake it.
#[tokio::test]
async fn a_replay_never_wakes_a_trigger_created_while_it_runs() {
    let (pool, db) = setup_test_db().await;
    let mut h = Harness::new(&pool, vec![event_trigger("t-build", "BuildObserved")]).await;
    *h.recorder.created_on_first_offer.lock().unwrap() =
        Some(event_trigger("t-new", "BuildObserved"));

    emit_build_observed(&h.bus).await;
    let second = emit_build_observed(&h.bus).await;
    h.lag().await;

    assert_eq!(h.recorder.woken_for(second), vec!["t-build".to_string()]);
    drop(h);
    teardown_test_db(&db).await;
}
