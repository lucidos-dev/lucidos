use std::collections::HashMap;
use std::sync::RwLock;

use uuid::Uuid;

use super::{enforce_spawn_gate, spawn_provenance, RefusedOrigin, SpawnRefusal, SpawnRoot};
use crate::engine::event_bus::{BusEvent, EventBus};
use crate::engine::thread_events::{
    ActorMode, EngineReason, EventChannel, EventMeta, MessageOrigin, ThreadDirection, ThreadEvent,
};
use crate::test_support::{message_received, setup_test_db, teardown_test_db};
use crate::triggers::TriggerConfig;

const TRIGGER: &str = "trig-gate";

async fn spawn_root(pool: &sqlx::PgPool, thread_id: Uuid) -> SpawnRoot {
    spawn_provenance(pool, thread_id).await.root
}

/// A raw event row carrying `origin`, and `parent_thread_id` when given.
async fn insert_origin(
    pool: &sqlx::PgPool,
    thread_id: Uuid,
    event_type: &str,
    origin: &MessageOrigin,
    parent: Option<Uuid>,
) {
    let mut payload = serde_json::json!({ "origin": origin });
    if let Some(parent) = parent {
        payload["parent_thread_id"] = serde_json::json!(parent);
    }
    sqlx::query(
        "INSERT INTO events (id, aggregate, aggregate_id, event_type, payload, created, thread_id) \
         VALUES ($1, 'thread', $2, $3, $4, NOW(), $2::uuid)",
    )
    .bind(Uuid::new_v4())
    .bind(thread_id.to_string())
    .bind(event_type)
    .bind(payload)
    .execute(pool)
    .await
    .expect("insert origin event");
}

fn scheduler(trigger_id: &str) -> MessageOrigin {
    MessageOrigin::engine(EngineReason::Scheduler {
        trigger_id: trigger_id.to_string(),
        trigger_name: Some("Nightly fix".into()),
        actor_email: None,
    })
}

fn link(parent: Uuid) -> MessageOrigin {
    MessageOrigin::ThreadLink {
        thread_id: parent,
        title: None,
        spawning_event_id: None,
        mode: ActorMode::Agent,
        direction: ThreadDirection::Parent,
    }
}

fn device() -> MessageOrigin {
    MessageOrigin::Device {
        device_id: "d1".into(),
    }
}

fn cli_caller(thread_id: Option<Uuid>) -> MessageOrigin {
    MessageOrigin::Workspace {
        workspace: "myws".into(),
        thread_id,
        event_id: None,
        user_agent: None,
        mode: ActorMode::Agent,
    }
}

fn trigger_root() -> SpawnRoot {
    SpawnRoot::Trigger {
        trigger_id: TRIGGER.into(),
        trigger_name: Some("Nightly fix".into()),
    }
}

/// A trigger thread fired by the scheduler.
async fn seed_trigger_run(pool: &sqlx::PgPool) -> Uuid {
    let root = Uuid::new_v4();
    insert_origin(pool, root, "TriggerStarted", &scheduler(TRIGGER), None).await;
    root
}

#[tokio::test]
async fn the_root_is_the_cause_of_the_work() {
    let (pool, db_name) = setup_test_db().await;
    let trigger_run = seed_trigger_run(&pool).await;

    let child = Uuid::new_v4();
    insert_origin(
        &pool,
        child,
        "MessageReceived",
        &link(trigger_run),
        Some(trigger_run),
    )
    .await;
    let grandchild = Uuid::new_v4();
    insert_origin(
        &pool,
        grandchild,
        "MessageReceived",
        &link(child),
        Some(child),
    )
    .await;
    // A top spawn names its cause without the callback linkage.
    let top_spawn = Uuid::new_v4();
    insert_origin(
        &pool,
        top_spawn,
        "MessageReceived",
        &link(trigger_run),
        None,
    )
    .await;
    // `lucidos spawn-thread --relation top` from the trigger's own thread.
    let cli_spawn = Uuid::new_v4();
    insert_origin(
        &pool,
        cli_spawn,
        "MessageReceived",
        &cli_caller(Some(trigger_run)),
        None,
    )
    .await;
    // A subprocess of the trigger's run, calling the API with its token.
    let api_spawn = Uuid::new_v4();
    let api_agent = MessageOrigin::Api {
        user_agent: None,
        mode: ActorMode::Agent,
        source_thread_id: Some(trigger_run),
        emitting_trigger_id: None,
    };
    insert_origin(&pool, api_spawn, "MessageReceived", &api_agent, None).await;
    for thread in [
        trigger_run,
        child,
        grandchild,
        top_spawn,
        cli_spawn,
        api_spawn,
    ] {
        assert_eq!(spawn_root(&pool, thread).await, trigger_root(), "{thread}");
    }
    // A scheduled script has no thread. Its verified token names the trigger.
    let script_spawn = Uuid::new_v4();
    let script = MessageOrigin::Api {
        user_agent: None,
        mode: ActorMode::Agent,
        source_thread_id: None,
        emitting_trigger_id: Some(TRIGGER.into()),
    };
    insert_origin(&pool, script_spawn, "MessageReceived", &script, None).await;
    assert_eq!(
        spawn_root(&pool, script_spawn).await,
        SpawnRoot::Trigger {
            trigger_id: TRIGGER.into(),
            trigger_name: None,
        }
    );

    let owned = Uuid::new_v4();
    insert_origin(&pool, owned, "MessageReceived", &device(), None).await;
    let owned_child = Uuid::new_v4();
    insert_origin(
        &pool,
        owned_child,
        "MessageReceived",
        &link(owned),
        Some(owned),
    )
    .await;
    let main_check = Uuid::new_v4();
    let main_check_red = MessageOrigin::engine(EngineReason::MainCheckRed);
    insert_origin(&pool, main_check, "MessageReceived", &main_check_red, None).await;
    // Another workspace's thread has no origin here.
    let from_elsewhere = Uuid::new_v4();
    let elsewhere = cli_caller(Some(Uuid::new_v4()));
    insert_origin(&pool, from_elsewhere, "MessageReceived", &elsewhere, None).await;
    let legacy = Uuid::new_v4();
    for thread in [owned, owned_child, main_check, from_elsewhere, legacy] {
        assert_eq!(
            spawn_root(&pool, thread).await,
            SpawnRoot::Owner,
            "{thread}"
        );
    }

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn webhook_system_and_untraced_agent_roots_are_refused() {
    let (pool, db_name) = setup_test_db().await;
    let cases = [
        (
            MessageOrigin::Webhook {
                webhook_id: "w1".into(),
                name: "Inbound".into(),
            },
            RefusedOrigin::Webhook,
        ),
        (MessageOrigin::System, RefusedOrigin::System),
        (
            MessageOrigin::Agent {
                agent: Default::default(),
            },
            RefusedOrigin::Agent,
        ),
        (
            MessageOrigin::Api {
                user_agent: None,
                mode: ActorMode::Agent,
                source_thread_id: None,
                emitting_trigger_id: None,
            },
            RefusedOrigin::Agent,
        ),
    ];
    for (origin, refused) in cases {
        let root = Uuid::new_v4();
        insert_origin(&pool, root, "MessageReceived", &origin, None).await;
        let child = Uuid::new_v4();
        insert_origin(&pool, child, "MessageReceived", &link(root), Some(root)).await;
        for thread in [root, child] {
            assert_eq!(
                spawn_root(&pool, thread).await,
                SpawnRoot::Refused(refused),
                "{origin:?}"
            );
        }
    }

    let (a, b) = (Uuid::new_v4(), Uuid::new_v4());
    insert_origin(&pool, a, "MessageReceived", &link(b), Some(b)).await;
    insert_origin(&pool, b, "MessageReceived", &link(a), Some(a)).await;
    assert_eq!(
        spawn_root(&pool, a).await,
        SpawnRoot::Refused(RefusedOrigin::Untraceable)
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// A resume, a person's follow-up and an answered card all add events after
/// the spawn. Only the earliest origin counts, so none of them moves the root.
#[tokio::test]
async fn later_human_acts_never_change_the_root() {
    let (pool, db_name) = setup_test_db().await;
    let trigger_run = seed_trigger_run(&pool).await;
    insert_origin(&pool, trigger_run, "MessageReceived", &device(), None).await;
    let child = Uuid::new_v4();
    insert_origin(
        &pool,
        child,
        "MessageReceived",
        &link(trigger_run),
        Some(trigger_run),
    )
    .await;
    insert_origin(&pool, child, "MessageReceived", &device(), None).await;
    let continued = MessageOrigin::engine(EngineReason::ContinuationStarted);
    insert_origin(&pool, child, "MessageReceived", &continued, None).await;

    assert_eq!(spawn_root(&pool, child).await, trigger_root());

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// A read that fails, or an origin that does not parse, proves nothing
/// about the cause, so it never reads as the owner's.
#[tokio::test]
async fn an_unreadable_provenance_is_refused() {
    let (pool, db_name) = setup_test_db().await;
    let garbled = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO events (id, aggregate, aggregate_id, event_type, payload, created, thread_id) \
         VALUES ($1, 'thread', $2, 'MessageReceived', '{\"origin\": {\"kind\": \"bogus\"}}', NOW(), $2::uuid)",
    )
    .bind(Uuid::new_v4())
    .bind(garbled.to_string())
    .execute(&pool)
    .await
    .expect("insert a garbled origin");
    assert_eq!(
        spawn_root(&pool, garbled).await,
        SpawnRoot::Refused(RefusedOrigin::Untraceable)
    );

    let trigger_run = seed_trigger_run(&pool).await;
    pool.close().await;

    assert_eq!(
        spawn_root(&pool, trigger_run).await,
        SpawnRoot::Refused(RefusedOrigin::Untraceable)
    );

    teardown_test_db(&db_name).await;
}

fn registry(grant: Option<bool>) -> RwLock<HashMap<String, TriggerConfig>> {
    let Some(grant) = grant else {
        return RwLock::new(HashMap::new());
    };
    let config = TriggerConfig::from_created_payload(&serde_json::json!({
        "trigger_id": TRIGGER,
        "name": "Nightly fix",
        "schedule": ["0 0 3 * * *"],
        "run": { "type": "intent", "intent": "fix what broke" },
        "coding_agent_grant": grant,
    }))
    .expect("build TriggerConfig");
    RwLock::new(HashMap::from([(TRIGGER.to_string(), config)]))
}

/// A coding-agent thread opened by `origin`, through the bus, so its turn is
/// open for a terminal. Returns the thread and its origin event.
async fn seed_coding_turn(bus: &EventBus, origin: MessageOrigin) -> (Uuid, Uuid) {
    let thread_id = Uuid::new_v4();
    let mut event = message_received("work");
    if let ThreadEvent::MessageReceived {
        origin: slot, mode, ..
    } = &mut event
    {
        *mode = origin.mode();
        *slot = Some(origin);
    }
    let origin_id = bus
        .emit(BusEvent::Thread {
            thread_id,
            event,
            meta: EventMeta {
                channel: Some(EventChannel::ClaudeCode),
                ..EventMeta::NONE
            },
        })
        .await
        .expect("emit the spawn message")
        .expect("persisted")
        .event_id;
    (thread_id, origin_id)
}

async fn failures(pool: &sqlx::PgPool, thread_id: Uuid) -> Vec<serde_json::Value> {
    sqlx::query_scalar(
        "SELECT payload FROM events WHERE thread_id = $1 AND event_type = 'ResponseFailed'",
    )
    .bind(thread_id)
    .fetch_all(pool)
    .await
    .unwrap()
}

async fn notifications(pool: &sqlx::PgPool, thread_id: Uuid) -> Vec<serde_json::Value> {
    sqlx::query_scalar(
        "SELECT payload->'data' FROM events WHERE event_type = 'NotificationCreated' \
         AND payload->'data'->>'thread_id' = $1",
    )
    .bind(thread_id.to_string())
    .fetch_all(pool)
    .await
    .unwrap()
}

/// Invariants 1 and 7: an ungranted trigger's run is refused, and the refusal
/// ends the turn and notifies, naming the trigger and the missing grant.
#[tokio::test]
async fn an_ungranted_trigger_run_fails_loudly() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let trigger_run = seed_trigger_run(&pool).await;

    let revoked = SpawnRefusal::UngrantedTrigger {
        trigger_id: TRIGGER.into(),
        trigger_name: "Nightly fix".into(),
    };
    let deleted = SpawnRefusal::DeletedTrigger {
        trigger_id: TRIGGER.into(),
        trigger_name: "Nightly fix".into(),
    };
    for (configs, expected, remedy) in [
        (registry(Some(false)), revoked, "Allow coding agents"),
        (registry(None), deleted, "Archive this thread"),
    ] {
        let (thread_id, origin_id) = seed_coding_turn(&bus, cli_caller(Some(trigger_run))).await;
        let refused = enforce_spawn_gate(&bus, &configs, thread_id, origin_id)
            .await
            .expect_err("an ungranted trigger never starts a coding agent");
        assert_eq!(refused.refusal, expected);
        assert!(refused.notice.is_some(), "the caller pushes the notice");

        let failures = failures(&pool, thread_id).await;
        assert_eq!(failures.len(), 1);
        let error = failures[0]["error"].as_str().unwrap();
        assert!(error.contains("Nightly fix"), "{error}");
        assert!(error.contains(remedy), "{error}");
        assert_eq!(failures[0]["request_event_id"], origin_id.to_string());
        let notifications = notifications(&pool, thread_id).await;
        assert_eq!(notifications.len(), 1);
        assert_eq!(notifications[0]["task_id"], TRIGGER);
        assert_eq!(notifications[0]["title"], "Nightly fix failed");
    }

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn a_granted_trigger_and_an_owner_run_start() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let trigger_run = seed_trigger_run(&pool).await;
    let granted = registry(Some(true));
    for origin in [
        cli_caller(Some(trigger_run)),
        device(),
        MessageOrigin::engine(EngineReason::MainCheckRed),
    ] {
        let (thread_id, origin_id) = seed_coding_turn(&bus, origin.clone()).await;
        assert!(
            enforce_spawn_gate(&bus, &granted, thread_id, origin_id)
                .await
                .is_ok(),
            "{origin:?}"
        );
        assert!(failures(&pool, thread_id).await.is_empty());
        assert!(notifications(&pool, thread_id).await.is_empty());
    }

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// A continue key can let another trigger's fire into a granted trigger's
/// thread. That input steers the thread's agents, so it needs its own grant.
#[tokio::test]
async fn a_trigger_that_fed_the_thread_needs_its_grant_too() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let trigger_run = seed_trigger_run(&pool).await;
    let fed_by = "trig-other";
    insert_origin(
        &pool,
        trigger_run,
        "MessageReceived",
        &scheduler(fed_by),
        None,
    )
    .await;

    let (thread_id, origin_id) = seed_coding_turn(&bus, link(trigger_run)).await;
    let refused = enforce_spawn_gate(&bus, &registry(Some(true)), thread_id, origin_id)
        .await
        .expect_err("an ungranted trigger's input never steers a coding agent");
    assert_eq!(
        refused.refusal,
        SpawnRefusal::DeletedTrigger {
            trigger_id: fed_by.into(),
            trigger_name: "Nightly fix".into(),
        }
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn a_webhook_rooted_run_fails_without_a_trigger_notice() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let webhook = MessageOrigin::Webhook {
        webhook_id: "w1".into(),
        name: "Inbound".into(),
    };
    let (thread_id, origin_id) = seed_coding_turn(&bus, webhook).await;

    let refused = enforce_spawn_gate(&bus, &registry(Some(true)), thread_id, origin_id)
        .await
        .expect_err("a webhook never starts a coding agent");
    assert_eq!(
        refused.refusal,
        SpawnRefusal::Origin(RefusedOrigin::Webhook)
    );
    assert!(refused.notice.is_none());
    assert_eq!(failures(&pool, thread_id).await.len(), 1);
    assert!(notifications(&pool, thread_id).await.is_empty());

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Every run, a resume included, passes `run_agent_session`. The gate is its
/// first await, so nothing starts for a refused run.
#[test]
fn every_run_passes_the_gate_before_it_starts() {
    let source = include_str!("run_session/run.rs");
    let session = source
        .find("async fn run_agent_session(")
        .expect("run_agent_session is defined in run_session/run.rs");
    let body = &source[session..];
    let gate = body
        .find("self.gate_coding_agent_spawn(thread_id, origin_id).await?;")
        .expect("run_agent_session calls the spawn gate");
    let first_other_await = body[..gate].matches(".await").count();
    assert_eq!(
        first_other_await, 0,
        "the gate is the session's first await"
    );
    let direct = source
        .find("pub(crate) async fn run_direct_agent(")
        .expect("run_direct_agent is defined");
    assert!(
        source[direct..session].contains(".run_agent_session("),
        "run_direct_agent runs every session through run_agent_session"
    );
}
