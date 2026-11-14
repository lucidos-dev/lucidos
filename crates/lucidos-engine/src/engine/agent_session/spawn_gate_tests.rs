use std::collections::HashMap;
use std::sync::RwLock;

use uuid::Uuid;

use super::{
    enforce_spawn_gate, spawn_provenance, spawn_verdict, RefusedOrigin, SpawnRefusal, SpawnRoot,
};
use crate::engine::event_bus::{BusEvent, EventBus};
use crate::engine::permission::menu::{ALLOW_ONCE_LABEL, DONT_ALLOW_LABEL};
use crate::engine::thread_events::{
    ActorMode, EngineReason, EventChannel, EventMeta, MessageOrigin, SpawnVerdict, ThreadDirection,
    ThreadEvent,
};
use crate::engine::thread_lifecycle::ThreadStatus;
use crate::engine::unattested_act;
use crate::test_support::{message_received, setup_test_db, teardown_test_db};
use crate::triggers::TriggerConfig;

const TRIGGER: &str = "trig-gate";

#[tokio::test]
async fn session_runs_stay_local_through_children_and_saved_submissions() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let session = Uuid::new_v4();
    let delegated = MessageOrigin::SessionDevice {
        device_id: "rig-page".into(),
    };
    insert_origin(&pool, session, "MessageReceived", &delegated, None).await;
    let child = Uuid::new_v4();
    insert_origin(
        &pool,
        child,
        "MessageReceived",
        &MessageOrigin::ThreadLink {
            thread_id: session,
            title: None,
            spawning_event_id: None,
            mode: ActorMode::Agent,
            direction: ThreadDirection::Parent,
        },
        Some(session),
    )
    .await;
    let saved = fed_by_submitter(&pool, &device(), &delegated).await;
    let existing = Uuid::new_v4();
    insert_origin(&pool, existing, "MessageReceived", &device(), None).await;
    insert_origin(&pool, existing, "MessageReceived", &delegated, None).await;
    let injected = Uuid::new_v4();
    insert_origin(&pool, injected, "MessageReceived", &device(), None).await;
    insert_origin(&pool, injected, "PromptInjected", &delegated, None).await;
    let configs = RwLock::new(HashMap::new());
    for thread in [session, child, saved, existing, injected] {
        assert_eq!(
            spawn_provenance(&pool, thread).await.root,
            SpawnRoot::SessionDevice
        );
        assert!(enforce_spawn_gate(&bus, &configs, thread, Uuid::new_v4())
            .await
            .is_ok());
        let caller = MessageOrigin::Api {
            user_agent: None,
            mode: ActorMode::Agent,
            source_thread_id: Some(thread),
            emitting_trigger_id: None,
        };
        assert_eq!(
            spawn_verdict(&pool, &configs, caller).await,
            SpawnVerdict::Unattested
        );
        assert_eq!(
            super::thread_spawn_verdict(&pool, &configs, thread).await,
            SpawnVerdict::Unattested
        );
    }
    pool.close().await;
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn saved_draft_first_send_keeps_its_submitting_trigger_provenance() {
    let (pool, db) = setup_test_db().await;
    let caller = Uuid::new_v4();
    insert_origin(&pool, caller, "TriggerStarted", &scheduler(TRIGGER), None).await;
    let target = Uuid::new_v4();
    insert_origin(
        &pool,
        target,
        "MessageReceived",
        &MessageOrigin::SavedDraft {
            submission_event_id: Uuid::new_v4(),
            submitted_by_thread_id: caller,
        },
        None,
    )
    .await;
    assert!(
        matches!(spawn_root(&pool,target).await, SpawnRoot::Trigger {trigger_id,..} if trigger_id==TRIGGER)
    );
    pool.close().await;
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn saved_draft_feeding_an_owner_thread_keeps_the_submitting_trigger_grant_gate() {
    let (pool, db) = setup_test_db().await;
    let caller = Uuid::new_v4();
    insert_origin(&pool, caller, "TriggerStarted", &scheduler(TRIGGER), None).await;
    let target = Uuid::new_v4();
    insert_origin(
        &pool,
        target,
        "MessageReceived",
        &MessageOrigin::Device {
            device_id: "test-owner".into(),
        },
        None,
    )
    .await;
    insert_origin(
        &pool,
        target,
        "MessageReceived",
        &MessageOrigin::SavedDraft {
            submission_event_id: Uuid::new_v4(),
            submitted_by_thread_id: caller,
        },
        None,
    )
    .await;
    let provenance = spawn_provenance(&pool, target).await;
    assert_eq!(provenance.root, SpawnRoot::Owner);
    assert!(provenance
        .fired_triggers
        .iter()
        .any(|(id, _)| id == TRIGGER));
    assert!(matches!(
        super::spawn_refusal(provenance, &RwLock::new(HashMap::new())),
        Some(SpawnRefusal::DeletedTrigger { .. })
    ));
    pool.close().await;
    teardown_test_db(&db).await;
}

/// A thread started by `first`, then fed a saved draft its submitter wrote.
async fn fed_by_submitter(
    pool: &sqlx::PgPool,
    first: &MessageOrigin,
    submitter: &MessageOrigin,
) -> Uuid {
    let caller = Uuid::new_v4();
    insert_origin(pool, caller, "MessageReceived", submitter, None).await;
    let target = Uuid::new_v4();
    insert_origin(pool, target, "MessageReceived", first, None).await;
    insert_origin(
        pool,
        target,
        "MessageReceived",
        &MessageOrigin::SavedDraft {
            submission_event_id: Uuid::new_v4(),
            submitted_by_thread_id: caller,
        },
        None,
    )
    .await;
    target
}

#[tokio::test]
async fn saved_draft_from_an_unattested_submitter_waits_for_the_owner() {
    let (pool, db) = setup_test_db().await;
    let owner = MessageOrigin::Device {
        device_id: "test-owner".into(),
    };
    let target = fed_by_submitter(&pool, &owner, &typed_device_caller()).await;
    assert_eq!(
        spawn_root(&pool, target).await,
        SpawnRoot::Unattested(typed_device_caller())
    );
    pool.close().await;
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn saved_draft_from_a_refused_workspace_call_stays_refused() {
    let (pool, db) = setup_test_db().await;
    let owner = MessageOrigin::Device {
        device_id: "test-owner".into(),
    };
    let refused = stamped_caller(SpawnVerdict::Refused {
        reason: "no grant".into(),
    });
    let target = fed_by_submitter(&pool, &owner, &refused).await;
    assert!(matches!(
        spawn_root(&pool, target).await,
        SpawnRoot::RefusedByCaller { reason, .. } if reason == "no grant"
    ));
    pool.close().await;
    teardown_test_db(&db).await;
}

/// The root holds one cause, so the trigger's grant check moves to the fired
/// list rather than being lost.
#[tokio::test]
async fn saved_draft_from_an_unattested_submitter_keeps_the_trigger_root_gated() {
    let (pool, db) = setup_test_db().await;
    let target = fed_by_submitter(&pool, &scheduler(TRIGGER), &typed_device_caller()).await;
    let provenance = spawn_provenance(&pool, target).await;
    assert_eq!(
        provenance.root,
        SpawnRoot::Unattested(typed_device_caller())
    );
    assert!(provenance
        .fired_triggers
        .iter()
        .any(|(id, _)| id == TRIGGER));
    assert!(matches!(
        super::spawn_refusal(provenance, &RwLock::new(HashMap::new())),
        Some(SpawnRefusal::DeletedTrigger { .. })
    ));
    pool.close().await;
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn a_missing_saved_draft_submitter_is_untraceable_instead_of_owner() {
    let (pool, db) = setup_test_db().await;
    let target = Uuid::new_v4();
    insert_origin(
        &pool,
        target,
        "MessageReceived",
        &MessageOrigin::SavedDraft {
            submission_event_id: Uuid::new_v4(),
            submitted_by_thread_id: Uuid::new_v4(),
        },
        None,
    )
    .await;
    assert_eq!(
        spawn_root(&pool, target).await,
        SpawnRoot::Refused(RefusedOrigin::Untraceable)
    );
    pool.close().await;
    teardown_test_db(&db).await;
}

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

/// A `Workspace` row from before gateway stamps, which keeps its old reading.
fn cli_caller(thread_id: Option<Uuid>) -> MessageOrigin {
    MessageOrigin::Workspace {
        workspace: "myws".into(),
        thread_id,
        event_id: None,
        user_agent: None,
        mode: ActorMode::Agent,
        verdict: None,
    }
}

/// A stamped call from another workspace's thread, carrying `verdict`.
fn stamped_caller(verdict: SpawnVerdict) -> MessageOrigin {
    MessageOrigin::Workspace {
        workspace: "myws".into(),
        thread_id: Some(Uuid::new_v4()),
        event_id: None,
        user_agent: None,
        mode: ActorMode::Agent,
        verdict: Some(verdict),
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

fn typed_device_caller() -> MessageOrigin {
    MessageOrigin::UnattestedCaller {
        mode: ActorMode::Human,
        user_agent: None,
        claimed_device_id: Some("typed-device".into()),
        claimed_local_token: false,
        claimed_workspace: None,
    }
}

/// Invariant 3: an unattested caller's root, and its children, wait for the
/// owner rather than refusing outright.
#[tokio::test]
async fn unattested_roots_wait_for_the_owner() {
    let (pool, db_name) = setup_test_db().await;
    for origin in [
        typed_device_caller(),
        crate::api::local_auth::local_process_origin(None),
    ] {
        let root = Uuid::new_v4();
        insert_origin(&pool, root, "MessageReceived", &origin, None).await;
        let child = Uuid::new_v4();
        insert_origin(&pool, child, "MessageReceived", &link(root), Some(root)).await;
        for thread in [root, child] {
            assert_eq!(
                spawn_root(&pool, thread).await,
                SpawnRoot::Unattested(origin.clone()),
                "{origin:?}"
            );
        }
    }
    pool.close().await;
    teardown_test_db(&db_name).await;
}

async fn start_cards(pool: &sqlx::PgPool, thread_id: Uuid) -> Vec<String> {
    sqlx::query_scalar(
        "SELECT payload->>'tool_use_id' FROM events WHERE aggregate_id = $1 \
         AND event_type = 'UserQuestionAsked' \
         AND payload->'permission_request'->'unattested_act'->'request'->>'kind' \
             = 'start_coding_agent' \
         ORDER BY sequence",
    )
    .bind(thread_id.to_string())
    .fetch_all(pool)
    .await
    .unwrap()
}

/// The `ContinuationRequested` the owner's Allow of a start emits
/// (`owner_door::act_on_card_answer`), as the origin of the run it starts.
async fn owner_allowed_continuation(pool: &sqlx::PgPool, thread_id: Uuid) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO events (id, aggregate, aggregate_id, event_type, payload, created, thread_id) \
         VALUES ($1, 'thread', $2, 'ContinuationRequested', $3, NOW(), $2::uuid)",
    )
    .bind(id)
    .bind(thread_id.to_string())
    .bind(serde_json::json!({
        "reason": crate::engine::agent_recovery::OWNER_ALLOWED_START_REASON,
    }))
    .execute(pool)
    .await
    .unwrap();
    id
}

async fn allow_once(bus: &EventBus, thread_id: Uuid, card: &str, actor: MessageOrigin) {
    unattested_act::tests::answer(bus, thread_id, card, ALLOW_ONCE_LABEL, actor).await;
}

/// The thread's projected status.
async fn status(pool: &sqlx::PgPool, thread_id: Uuid) -> String {
    sqlx::query_scalar("SELECT status FROM thread_summaries WHERE thread_id = $1")
        .bind(thread_id)
        .fetch_one(pool)
        .await
        .unwrap()
}

/// Every terminator on the thread, as (type, cause, request_event_id).
async fn terminators(
    pool: &sqlx::PgPool,
    thread_id: Uuid,
) -> Vec<(String, Option<String>, Option<String>)> {
    sqlx::query_as(
        "SELECT event_type, payload->>'cause', payload->>'request_event_id' FROM events \
         WHERE thread_id = $1 AND event_type = ANY($2) ORDER BY sequence",
    )
    .bind(thread_id)
    .bind(ThreadEvent::TERMINATOR_EVENT_TYPES)
    .fetch_all(pool)
    .await
    .unwrap()
}

/// Invariants 3, 4 and I19: the run parks on one card as waiting, never
/// failed, and only a device's Allow once starts it, for one run.
#[tokio::test]
async fn an_unattested_run_parks_until_a_device_allows_it_once() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let configs = registry(Some(true));
    let (thread_id, origin_id) = seed_coding_turn(&bus, typed_device_caller()).await;

    let refused = enforce_spawn_gate(&bus, &configs, thread_id, origin_id)
        .await
        .expect_err("an unattested caller's run waits for the owner");
    assert_eq!(
        refused.refusal,
        SpawnRefusal::Unattested(typed_device_caller())
    );
    assert!(refused.notice.is_none());
    assert!(
        terminators(&pool, thread_id).await.is_empty(),
        "a parked run is waiting: the card holds the turn"
    );
    assert_eq!(
        status(&pool, thread_id).await,
        ThreadStatus::WaitingForUserAnswer.as_str()
    );
    let cards = start_cards(&pool, thread_id).await;
    assert_eq!(cards.len(), 1);

    assert!(enforce_spawn_gate(&bus, &configs, thread_id, origin_id)
        .await
        .is_err());
    assert_eq!(
        start_cards(&pool, thread_id).await.len(),
        1,
        "one card waits"
    );

    allow_once(&bus, thread_id, &cards[0], typed_device_caller()).await;
    assert!(
        enforce_spawn_gate(&bus, &configs, thread_id, origin_id)
            .await
            .is_err(),
        "an unattested Allow starts nothing"
    );
    let cards = start_cards(&pool, thread_id).await;
    assert_eq!(cards.len(), 2, "the answered card leaves a new one");

    allow_once(&bus, thread_id, &cards[1], device()).await;
    assert!(
        enforce_spawn_gate(&bus, &configs, thread_id, origin_id)
            .await
            .is_err(),
        "a later message cannot spend the Allow its own run was not started by"
    );
    let allowed = owner_allowed_continuation(&pool, thread_id).await;
    assert!(enforce_spawn_gate(&bus, &configs, thread_id, allowed)
        .await
        .is_ok());
    assert!(
        enforce_spawn_gate(&bus, &configs, thread_id, allowed)
            .await
            .is_err(),
        "one Allow covers one run"
    );
    assert!(
        failures(&pool, thread_id).await.is_empty(),
        "no step of the wait wrote a failure"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// I19: Don't allow closes the parked turn as refused, which is no failure,
/// and only a start card's Don't allow does.
#[tokio::test]
async fn dont_allow_closes_the_parked_start_as_refused() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let configs = registry(Some(true));
    let (thread_id, origin_id) = seed_coding_turn(&bus, typed_device_caller()).await;
    let _ = enforce_spawn_gate(&bus, &configs, thread_id, origin_id).await;
    let card = start_cards(&pool, thread_id).await.remove(0);
    let option = |label: &'static str| {
        let pool = pool.clone();
        let card = card.clone();
        async move {
            sqlx::query_scalar::<_, String>(
                "SELECT o->>'id' FROM events, jsonb_array_elements(payload->'options') o \
                 WHERE aggregate_id = $1 AND payload->>'tool_use_id' = $2 AND o->>'label' = $3",
            )
            .bind(thread_id.to_string())
            .bind(&card)
            .bind(label)
            .fetch_one(&pool)
            .await
            .unwrap()
        }
    };

    let allow = option(ALLOW_ONCE_LABEL).await;
    unattested_act::close_refused_start(&bus, thread_id, &card, &allow, Some(device()))
        .await
        .unwrap();
    assert!(
        terminators(&pool, thread_id).await.is_empty(),
        "Allow once closes nothing"
    );

    let dont = option(DONT_ALLOW_LABEL).await;
    unattested_act::close_refused_start(&bus, thread_id, &card, &dont, Some(device()))
        .await
        .unwrap();
    assert_eq!(
        terminators(&pool, thread_id).await,
        vec![(
            "ResponseCanceled".to_string(),
            Some("refused".to_string()),
            Some(origin_id.to_string())
        )]
    );
    assert!(failures(&pool, thread_id).await.is_empty());
    assert_eq!(status(&pool, thread_id).await, ThreadStatus::Idle.as_str());

    unattested_act::close_refused_start(&bus, thread_id, &card, &dont, Some(device()))
        .await
        .unwrap();
    assert_eq!(
        terminators(&pool, thread_id).await.len(),
        1,
        "a repeated refusal closes nothing new"
    );

    let later = bus
        .emit(BusEvent::Thread {
            thread_id,
            event: message_received("run the suite"),
            meta: EventMeta {
                channel: Some(EventChannel::ClaudeCode),
                ..EventMeta::NONE
            },
        })
        .await
        .unwrap()
        .unwrap()
        .event_id;
    let owed = crate::engine::agent_recovery::resume_input(
        &pool,
        thread_id,
        Some(crate::engine::agent_recovery::OWNER_ALLOWED_START_REASON),
    )
    .await
    .expect("the later message is owed");
    assert_eq!(
        owed.event_ids,
        vec![later],
        "a later Allow carries no refused message"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// A run from a verified caller passes the gate with no card and no failure.
#[tokio::test]
async fn a_verified_start_passes_with_no_card() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    for origin in [device(), stamped_caller(SpawnVerdict::Allowed)] {
        let (thread_id, origin_id) = seed_coding_turn(&bus, origin.clone()).await;
        assert!(
            enforce_spawn_gate(&bus, &registry(Some(true)), thread_id, origin_id)
                .await
                .is_ok(),
            "{origin:?}"
        );
        assert!(start_cards(&pool, thread_id).await.is_empty(), "{origin:?}");
        assert!(terminators(&pool, thread_id).await.is_empty(), "{origin:?}");
    }
    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// I19: a run's caller writes no failure for a parked run, and still writes
/// one for a real refusal.
#[tokio::test]
async fn a_parked_run_gets_no_failure_from_its_caller() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let (thread_id, origin_id) = seed_coding_turn(&bus, typed_device_caller()).await;

    let parked = super::refusal_error(&SpawnRefusal::Unattested(typed_device_caller()));
    assert!(super::awaits_owner(parked.as_ref()));
    crate::engine::agentic_loop::ensure_run_failure_terminator_emitted(
        &bus,
        &pool,
        thread_id,
        origin_id,
        Some(EventChannel::ClaudeCode),
        parked.as_ref(),
    )
    .await;
    assert!(failures(&pool, thread_id).await.is_empty());

    let refused = super::refusal_error(&SpawnRefusal::Origin(RefusedOrigin::Webhook));
    assert!(!super::awaits_owner(refused.as_ref()));
    crate::engine::agentic_loop::ensure_run_failure_terminator_emitted(
        &bus,
        &pool,
        thread_id,
        origin_id,
        Some(EventChannel::ClaudeCode),
        refused.as_ref(),
    )
    .await;
    assert_eq!(failures(&pool, thread_id).await.len(), 1);

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// A trigger that fed an unattested thread still needs its own grant, and its
/// refusal wins over the owner's card.
#[tokio::test]
async fn an_ungranted_trigger_refuses_before_an_unattested_root_asks() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let (thread_id, origin_id) = seed_coding_turn(&bus, typed_device_caller()).await;
    insert_origin(
        &pool,
        thread_id,
        "PromptInjected",
        &scheduler(TRIGGER),
        None,
    )
    .await;

    let refused = enforce_spawn_gate(&bus, &registry(Some(false)), thread_id, origin_id)
        .await
        .expect_err("the fed trigger has no grant");
    assert!(matches!(
        refused.refusal,
        SpawnRefusal::UngrantedTrigger { .. }
    ));
    assert!(start_cards(&pool, thread_id).await.is_empty());

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

/// Invariant 6: the receiving workspace honors the caller's verdict and never
/// upgrades it. Only `allowed` roots at the owner.
#[tokio::test]
async fn a_stamped_call_roots_at_the_callers_verdict() {
    let (pool, db_name) = setup_test_db().await;
    let refused = SpawnVerdict::Refused {
        reason: "The trigger has no grant.".into(),
    };
    let unattested = stamped_caller(SpawnVerdict::Unattested);
    let cases = [
        (stamped_caller(SpawnVerdict::Allowed), SpawnRoot::Owner),
        (
            stamped_caller(refused),
            SpawnRoot::RefusedByCaller {
                workspace: "myws".into(),
                reason: "The trigger has no grant.".into(),
            },
        ),
        (unattested.clone(), SpawnRoot::Unattested(unattested)),
    ];
    for (origin, expected) in cases {
        let thread = Uuid::new_v4();
        insert_origin(&pool, thread, "MessageReceived", &origin, None).await;
        assert_eq!(spawn_root(&pool, thread).await, expected, "{origin:?}");
    }
    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// A refused call ends the turn naming the calling workspace and its reason,
/// and raises no owner card: no Allow here can override the caller's gate.
#[tokio::test]
async fn a_refused_call_fails_naming_the_caller_and_its_reason() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let reason = "The trigger \"Nightly fix\" has no coding-agent grant.";
    let (thread, origin_id) = seed_coding_turn(
        &bus,
        stamped_caller(SpawnVerdict::Refused {
            reason: reason.into(),
        }),
    )
    .await;
    let refused = enforce_spawn_gate(&bus, &registry(Some(true)), thread, origin_id)
        .await
        .expect_err("a refused caller never starts a run");
    assert_eq!(
        refused.refusal,
        SpawnRefusal::CallerWorkspace {
            workspace: "myws".into(),
            reason: reason.into(),
        }
    );
    let error = failures(&pool, thread).await[0]["error"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(
        error.contains("\"myws\"") && error.contains(reason),
        "{error}"
    );
    assert!(start_cards(&pool, thread).await.is_empty());
    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// The calling side: the verdict a thread's call carries is exactly what its
/// own gate would say about a local run.
#[tokio::test]
async fn a_threads_verdict_is_what_its_own_gate_says() {
    let (pool, db_name) = setup_test_db().await;
    let subprocess_of = |thread: Uuid| MessageOrigin::Api {
        user_agent: None,
        mode: ActorMode::Agent,
        source_thread_id: Some(thread),
        emitting_trigger_id: None,
    };
    let owned = Uuid::new_v4();
    insert_origin(&pool, owned, "MessageReceived", &device(), None).await;
    let trigger_run = seed_trigger_run(&pool).await;
    let unattested = Uuid::new_v4();
    insert_origin(
        &pool,
        unattested,
        "MessageReceived",
        &typed_device_caller(),
        None,
    )
    .await;

    let granted = registry(Some(true));
    let ungranted = registry(Some(false));
    assert_eq!(
        spawn_verdict(&pool, &ungranted, subprocess_of(owned)).await,
        SpawnVerdict::Allowed
    );
    assert_eq!(
        spawn_verdict(&pool, &granted, subprocess_of(trigger_run)).await,
        SpawnVerdict::Allowed
    );
    let SpawnVerdict::Refused { reason } =
        spawn_verdict(&pool, &ungranted, subprocess_of(trigger_run)).await
    else {
        panic!("an ungranted trigger's thread is refused");
    };
    assert!(reason.contains("Nightly fix"), "{reason}");
    assert_eq!(
        spawn_verdict(&pool, &granted, subprocess_of(unattested)).await,
        SpawnVerdict::Unattested
    );
    // A scheduled script names its trigger and no thread.
    let script = MessageOrigin::Api {
        user_agent: None,
        mode: ActorMode::Agent,
        source_thread_id: None,
        emitting_trigger_id: Some(TRIGGER.into()),
    };
    assert!(matches!(
        spawn_verdict(&pool, &ungranted, script).await,
        SpawnVerdict::Refused { .. }
    ));
    pool.close().await;
    teardown_test_db(&db_name).await;
}
