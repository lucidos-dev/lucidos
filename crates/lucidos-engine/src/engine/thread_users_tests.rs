use super::*;
use crate::engine::thread_events::{
    ActorMode, AnswerKind, EngineReason, EventChannel, MessageOrigin, ThreadEvent,
};
use crate::gateway_users::User;
use crate::test_support::{setup_test_db, teardown_test_db};

fn device(id: &str) -> MessageOrigin {
    MessageOrigin::Device {
        device_id: id.into(),
    }
}

fn fire_by(email: &str) -> MessageOrigin {
    MessageOrigin::Engine {
        reason: EngineReason::Scheduler {
            trigger_id: "intake".into(),
            trigger_name: Some("Intake".into()),
            actor_email: Some(email.into()),
        },
    }
}

fn user(id: &str, emails: &[&str], devices: &[&str]) -> User {
    User {
        id: id.into(),
        name: id.to_uppercase(),
        emails: emails.iter().map(|e| e.to_string()).collect(),
        device_ids: devices.iter().map(|d| d.to_string()).collect(),
    }
}

/// Sam holds two devices and an email; Kim holds one device.
fn team() -> UserSnapshot {
    UserSnapshot {
        users: vec![
            user("sam", &["sam@example.com"], &["sam-phone", "sam-laptop"]),
            user("kim", &["kim@example.com"], &["kim-phone"]),
        ],
        relayed: vec![],
    }
}

fn event(event_type: &str, actor: MessageOrigin) -> RoleEvent {
    RoleEvent {
        event_type: event_type.into(),
        actor: Some(actor),
        user_id: None,
    }
}

/// A user mention recorded by `actor`, naming `user_id`.
fn mentioned(user_id: &str, actor: MessageOrigin) -> RoleEvent {
    RoleEvent {
        user_id: Some(user_id.into()),
        ..event("WorkItemWatched", actor)
    }
}

fn driver_id(roles: &ThreadUsers) -> Option<&str> {
    roles.driver.as_ref().map(|d| d.id.as_str())
}

#[test]
fn a_user_started_root_thread_starts_with_its_requester_and_a_fire_with_none() {
    let snapshot = team();
    let started = fold(Some(&device("sam-phone")), &[], &snapshot);
    assert_eq!(driver_id(&started), Some("sam"));

    // A ticket fire names its author by email, and still starts with nobody.
    let fired = fold(Some(&fire_by("sam@example.com")), &[], &snapshot);
    assert_eq!(fired.driver, None);
    assert_eq!(fold(None, &[], &snapshot).driver, None);
}

#[test]
fn take_over_moves_the_role_and_a_release_gives_up_only_what_it_holds() {
    let snapshot = team();
    let requester = device("sam-phone");
    let taken = [event("WorkItemDriverTaken", device("kim-phone"))];
    assert_eq!(
        driver_id(&fold(Some(&requester), &taken, &snapshot)),
        Some("kim")
    );

    let stale_release = [
        event("WorkItemDriverTaken", device("kim-phone")),
        event("WorkItemDriverReleased", device("sam-laptop")),
    ];
    assert_eq!(
        driver_id(&fold(Some(&requester), &stale_release, &snapshot)),
        Some("kim")
    );

    let released = [
        event("WorkItemDriverTaken", device("kim-phone")),
        event("WorkItemDriverReleased", device("kim-phone")),
    ];
    assert_eq!(fold(Some(&requester), &released, &snapshot).driver, None);
}

#[test]
fn an_unmatched_actor_sets_no_driver_and_a_matched_email_does() {
    let snapshot = team();
    let stranger = [event("WorkItemDriverTaken", fire_by("someone@example.org"))];
    assert_eq!(fold(None, &stranger, &snapshot).driver, None);

    let matched = [event("WorkItemDriverTaken", fire_by("KIM@example.com"))];
    assert_eq!(driver_id(&fold(None, &matched, &snapshot)), Some("kim"));
}

#[test]
fn a_watch_is_per_user_so_any_of_their_devices_can_end_it() {
    let snapshot = team();
    let watched = [
        event("WorkItemWatched", device("sam-phone")),
        event("WorkItemWatched", device("sam-laptop")),
        event("WorkItemWatched", device("kim-phone")),
    ];
    let roles = fold(None, &watched, &snapshot);
    let ids: Vec<&str> = roles.watchers.iter().map(|w| w.id.as_str()).collect();
    assert_eq!(ids, vec!["sam", "kim"]);

    let mut unwatched = watched.to_vec();
    unwatched.push(event("WorkItemUnwatched", device("sam-laptop")));
    let roles = fold(None, &unwatched, &snapshot);
    let ids: Vec<&str> = roles.watchers.iter().map(|w| w.id.as_str()).collect();
    assert_eq!(ids, vec!["kim"]);
}

/// A mention names the user it made a watcher, not its sender. That user
/// leaves from any device, and a removed user watches nothing.
#[test]
fn a_mention_makes_the_named_user_a_watcher() {
    let snapshot = team();
    let watched = [
        mentioned("kim", device("sam-phone")),
        mentioned("op-removed", device("sam-phone")),
    ];
    let roles = fold(None, &watched, &snapshot);
    let ids: Vec<&str> = roles.watchers.iter().map(|w| w.id.as_str()).collect();
    assert_eq!(ids, vec!["kim"]);

    let mut left = watched.to_vec();
    left.push(event("WorkItemUnwatched", device("kim-phone")));
    assert!(fold(None, &left, &snapshot).watchers.is_empty());
}

/// A user-only message reaches the mentioned users, the driver and the
/// watchers. A woken one reaches only the mentioned. Never the sender.
#[test]
fn a_user_only_message_reaches_who_it_is_for_never_its_sender() {
    let roles = ThreadUsers {
        driver: Some(named("sam")),
        watchers: vec![named("kim"), named("lee")],
    };
    let mentions = vec!["kim".to_string(), "ana".to_string()];
    assert_eq!(
        user_only_message_recipients(&roles, &mentions, true, Some(&named("kim"))),
        ["ana", "sam", "lee"]
    );
    assert_eq!(
        user_only_message_recipients(&roles, &mentions, false, Some(&named("sam"))),
        ["kim", "ana"]
    );
    assert_eq!(
        user_only_message_recipients(&roles, &[], true, None),
        ["sam", "kim", "lee"]
    );
}

/// Events store device ids, so moving a device re-attributes the role it took.
#[test]
fn moving_a_device_to_another_user_moves_the_role_it_took() {
    let taken = [event("WorkItemDriverTaken", device("kim-phone"))];
    assert_eq!(driver_id(&fold(None, &taken, &team())), Some("kim"));

    let mut moved = team();
    moved.users[1].device_ids.clear();
    moved.users[0].device_ids.push("kim-phone".into());
    assert_eq!(driver_id(&fold(None, &taken, &moved)), Some("sam"));
}

#[test]
fn a_gate_always_reaches_someone() {
    let snapshot = team();
    let no_driver = ThreadUsers::default();
    assert_eq!(audience(&no_driver, false, &snapshot), Audience::Team);

    let driven = fold(
        None,
        &[
            event("WorkItemDriverTaken", device("sam-phone")),
            event("WorkItemWatched", device("kim-phone")),
        ],
        &snapshot,
    );
    let expected: BTreeSet<String> = ["sam-phone", "sam-laptop", "kim-phone"]
        .iter()
        .map(|d| d.to_string())
        .collect();
    assert_eq!(
        audience(&driven, false, &snapshot),
        Audience::Devices(expected)
    );
    assert_eq!(audience(&driven, true, &snapshot), Audience::Team);

    // A driver who holds no device yet cannot be pushed, so the team is.
    let mut deviceless = team();
    deviceless.users[0].device_ids.clear();
    let driven_alone = fold(
        None,
        &[event("WorkItemDriverTaken", fire_by("sam@example.com"))],
        &deviceless,
    );
    assert_eq!(audience(&driven_alone, false, &deviceless), Audience::Team);
}

#[test]
fn a_gateway_with_one_user_pushes_everyone() {
    let solo = UserSnapshot {
        users: vec![user("sam", &[], &["sam-phone"])],
        relayed: vec![],
    };
    let driven = fold(Some(&device("sam-phone")), &[], &solo);
    assert_eq!(driver_id(&driven), Some("sam"));
    assert_eq!(audience(&driven, false, &solo), Audience::Team);
    assert!(Audience::Team.reaches(None));
    assert!(!Audience::Devices(BTreeSet::from(["a".to_string()])).reaches(None));
}

// ---- Against the database ----

/// A trigger-born root: its `TriggerStarted` is its first event, and it has
/// no `MessageReceived` of its own.
async fn fire_thread(bus: &EventBus, thread_id: Uuid, email: &str) {
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::TriggerStarted {
            retry_of: None,
            trigger_id: "intake".into(),
            trigger_name: Some("Intake".into()),
            prompt: Some("fix the login bug".into()),
            invocation: None,
            origin: Some(fire_by(email)),
            go_to_review: false,
            model: None,
            reasoning_effort: None,
            provider: None,
            continue_key: None,
        },
        meta: EventMeta {
            channel: Some(EventChannel::Trigger),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();
}

async fn start_thread(bus: &EventBus, thread_id: Uuid, origin: MessageOrigin) {
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::MessageReceived {
            provider: None,
            voice_session_id: None,
            wake_mode: None,
            text: "fix the login bug".into(),
            user_image_hashes: vec![],
            device_id: None,
            image_description: None,
            parent_thread_id: None,
            spawning_event_id: None,
            mode: ActorMode::Human,
            model: None,
            reasoning_effort: None,
            origin: Some(origin),
        },
        meta: EventMeta {
            channel: Some(EventChannel::Chat),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();
}

async fn ask(bus: &EventBus, thread_id: Uuid, tool_use_id: &str) {
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::UserQuestionAsked {
            tool_use_id: tool_use_id.into(),
            cc_session_id: String::new(),
            question: "Ship it?".into(),
            options: vec![],
            worktree_path: None,
            multi_select: false,
            owner_approval: None,
            unattested_act: None,
            permission_request: None,
        },
        meta: EventMeta {
            channel: Some(EventChannel::Chat),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();
}

async fn answer(bus: &EventBus, thread_id: Uuid, tool_use_id: &str, by: MessageOrigin) {
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::UserQuestionAnswered {
            compose: None,
            tool_use_id: tool_use_id.into(),
            answer: AnswerKind::FreeText {
                text: "yes".into(),
                image_hashes: vec![],
            },
            quiet: false,
        },
        meta: EventMeta {
            channel: Some(EventChannel::Chat),
            actor: Some(by),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();
}

async fn count(pool: &PgPool, thread_id: Uuid, event_type: &str) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM events WHERE thread_id = $1 AND event_type = $2")
        .bind(thread_id)
        .bind(event_type)
        .fetch_one(pool)
        .await
        .unwrap()
}

/// The answer from a source sets a driver only when its actor email matches
/// a user, and a driver answering again records nothing.
#[tokio::test]
async fn answering_a_gate_takes_the_driver_role_for_a_known_user_only() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let snapshot = team();
    let root = Uuid::new_v4();
    start_thread(&bus, root, device("sam-phone")).await;

    take_on_answer(
        &pool,
        &bus,
        root,
        &fire_by("stranger@example.org"),
        &snapshot,
    )
    .await
    .unwrap();
    assert_eq!(count(&pool, root, "WorkItemDriverTaken").await, 0);

    let again = take_on_answer(&pool, &bus, root, &device("sam-laptop"), &snapshot)
        .await
        .unwrap();
    assert_eq!(again, None, "the driver answering again takes nothing over");
    assert_eq!(count(&pool, root, "WorkItemDriverTaken").await, 0);

    let takeover = take_on_answer(&pool, &bus, root, &fire_by("kim@example.com"), &snapshot)
        .await
        .unwrap()
        .expect("Kim took over from Sam");
    assert_eq!(
        (takeover.from.id.as_str(), takeover.to.id.as_str()),
        ("sam", "kim")
    );
    assert_eq!(count(&pool, root, "WorkItemDriverTaken").await, 1);
    let roles = roles(&pool, root, &snapshot).await.unwrap();
    assert_eq!(driver_id(&roles), Some("kim"));

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// An overdue gate escalates once, reaches the team while open, and the
/// driver keeps the root thread. The answer narrows the audience again.
#[tokio::test]
async fn an_overdue_gate_escalates_once_and_the_driver_keeps_the_root_thread() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let snapshot = team();
    let root = Uuid::new_v4();
    start_thread(&bus, root, device("sam-phone")).await;
    ask(&bus, root, "gate-1").await;

    assert!(overdue_gates(&pool, 24.0, &snapshot)
        .await
        .unwrap()
        .is_empty());
    sqlx::query(
        "UPDATE events SET created = now() - interval '2 days' \
         WHERE thread_id = $1 AND event_type = 'UserQuestionAsked'",
    )
    .bind(root)
    .execute(&pool)
    .await
    .unwrap();

    let overdue = overdue_gates(&pool, 24.0, &snapshot).await.unwrap();
    assert_eq!(overdue.len(), 1);
    assert_eq!(overdue[0].root, root);
    assert_eq!(overdue[0].driver.id, "sam");
    assert!(matches!(
        audience_of_thread(&pool, root, &snapshot).await.unwrap(),
        Audience::Devices(_)
    ));

    record(
        &bus,
        root,
        ThreadEvent::WorkItemGateEscalated {
            gate_thread_id: root,
            tool_use_id: "gate-1".into(),
        },
        None,
    )
    .await
    .unwrap();
    assert!(overdue_gates(&pool, 24.0, &snapshot)
        .await
        .unwrap()
        .is_empty());
    assert_eq!(
        audience_of_thread(&pool, root, &snapshot).await.unwrap(),
        Audience::Team
    );
    let roles_after = roles(&pool, root, &snapshot).await.unwrap();
    assert_eq!(driver_id(&roles_after), Some("sam"));

    answer(&bus, root, "gate-1", device("kim-phone")).await;
    assert!(matches!(
        audience_of_thread(&pool, root, &snapshot).await.unwrap(),
        Audience::Devices(_)
    ));

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// A thread started with a user has its sender as requester, so it has a
/// driver like any user-started root thread.
#[tokio::test]
async fn a_thread_started_with_a_user_is_driven_by_its_sender() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let snapshot = team();
    let root = Uuid::new_v4();
    bus.emit(BusEvent::Thread {
        thread_id: root,
        event: ThreadEvent::UserOnlyMessagePosted {
            text: "Kim, can you look?".into(),
            user_image_hashes: vec![],
            device_id: Some("sam-phone".into()),
            mentioned_user_ids: vec!["kim".into()],
        },
        meta: EventMeta::with_actor(Some(device("sam-phone"))),
    })
    .await
    .unwrap();
    let roles = roles(&pool, root, &snapshot).await.unwrap();
    assert_eq!(driver_id(&roles), Some("sam"));

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Replaying a watch after an explicit Unwatch preserves the user choice.
#[tokio::test]
async fn a_mention_records_a_watch_only_for_a_user_without_a_role() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let snapshot = team();
    let root = Uuid::new_v4();
    start_thread(&bus, root, device("sam-phone")).await;
    let notice = UserOnlyMessageNotice {
        thread_id: root,
        event_id: Some(Uuid::new_v4()),
        text: "Hello".into(),
        mentions: vec!["kim".into()],
        user_only: true,
        sender: Some(device("sam-phone")),
    };
    apply_message_watches(&pool, &bus, &notice, root, &notice.mentions)
        .await
        .unwrap();
    // Explicit Unwatch between delivery and recovery must survive the replay.
    record(
        &bus,
        root,
        ThreadEvent::WorkItemUnwatched,
        Some(device("kim-phone")),
    )
    .await
    .unwrap();
    apply_message_watches(&pool, &bus, &notice, root, &notice.mentions)
        .await
        .unwrap();
    assert_eq!(count(&pool, root, "WorkItemWatched").await, 1);
    let roles = roles(&pool, root, &snapshot).await.unwrap();
    assert_eq!(driver_id(&roles), Some("sam"));
    let ids: Vec<&str> = roles.watchers.iter().map(|w| w.id.as_str()).collect();
    assert!(ids.is_empty());

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Roles live only in events, so a fresh connection after a restart reads
/// the same roles.
#[tokio::test]
async fn roles_survive_a_restart() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let snapshot = team();
    let root = Uuid::new_v4();
    start_thread(&bus, root, device("sam-phone")).await;
    record(
        &bus,
        root,
        ThreadEvent::WorkItemWatched { user_id: None },
        Some(device("kim-phone")),
    )
    .await
    .unwrap();
    let before = roles(&pool, root, &snapshot).await.unwrap();
    drop(bus);

    let options = (*pool.connect_options()).clone();
    pool.close().await;
    let reopened = PgPool::connect_with(options).await.unwrap();
    assert_eq!(roles(&reopened, root, &snapshot).await.unwrap(), before);
    assert_eq!(before.watchers.len(), 1);

    reopened.close().await;
    teardown_test_db(&db_name).await;
}

/// The factory listing. Kim watches a driven item. A ticket gate with no
/// driver needs the team, and so does an escalated gate. An archived root is
/// done.
#[tokio::test]
async fn the_listing_shows_open_root_threads_with_their_views() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let snapshot = team();
    let (driven, ticket, escalated, done) = (
        Uuid::new_v4(),
        Uuid::new_v4(),
        Uuid::new_v4(),
        Uuid::new_v4(),
    );
    start_thread(&bus, driven, device("sam-phone")).await;
    record(
        &bus,
        driven,
        ThreadEvent::WorkItemWatched { user_id: None },
        Some(device("kim-phone")),
    )
    .await
    .unwrap();
    fire_thread(&bus, ticket, "sam@example.com").await;
    // A later message is a follow-up, never the requester.
    start_thread(&bus, ticket, device("kim-phone")).await;
    ask(&bus, ticket, "gate-t").await;
    start_thread(&bus, escalated, device("sam-laptop")).await;
    ask(&bus, escalated, "gate-e").await;
    record(
        &bus,
        escalated,
        ThreadEvent::WorkItemGateEscalated {
            gate_thread_id: escalated,
            tool_use_id: "gate-e".into(),
        },
        None,
    )
    .await
    .unwrap();
    start_thread(&bus, done, device("kim-phone")).await;
    record(&bus, done, ThreadEvent::ThreadArchived, None)
        .await
        .unwrap();

    let draft = Uuid::new_v4();
    sqlx::query("INSERT INTO thread_summaries (thread_id, state) VALUES ($1, 'composing')")
        .bind(draft)
        .execute(&pool)
        .await
        .unwrap();

    let kim = snapshot.user_of_device("kim-phone");
    let items = list(&pool, &snapshot, kim.as_ref()).await.unwrap();
    let of = |root: Uuid| items.iter().find(|i| i.root_thread_id == root);
    assert!(of(done).is_none(), "an archived root thread is done");
    assert!(of(draft).is_none(), "an unsent draft is no root thread");

    let driven = of(driven).expect("listed");
    assert_eq!(driver_id(&driven.roles), Some("sam"));
    assert!(driven.yours && !driven.gate_open && !driven.needs_team);

    let ticket = of(ticket).expect("listed");
    assert!(ticket.gate_open && ticket.needs_team && !ticket.yours);
    assert_eq!(ticket.roles.driver, None);

    let escalated = of(escalated).expect("listed");
    assert_eq!(driver_id(&escalated.roles), Some("sam"));
    assert!(escalated.escalated && escalated.needs_team);

    pool.close().await;
    teardown_test_db(&db_name).await;
}

fn named(id: &str) -> UserRef {
    UserRef {
        id: id.into(),
        name: id.to_uppercase(),
    }
}

/// A takeover tells the user displaced. A finished root thread tells its
/// driver and watchers once each, never the user who finished it.
#[test]
fn a_notice_reaches_the_users_it_concerns() {
    let takeover = Takeover {
        from: named("sam"),
        to: named("kim"),
    };
    let roles = ThreadUsers {
        driver: Some(named("sam")),
        watchers: vec![named("kim"), named("sam"), named("lee")],
    };
    let ids = |users: Vec<UserRef>| users.into_iter().map(|p| p.id).collect::<Vec<_>>();

    assert_eq!(
        ids(UserNotice::TakenOver(takeover.clone()).recipients(&roles)),
        ["sam"]
    );
    assert_eq!(
        ids(UserNotice::GateAnswered(takeover).recipients(&roles)),
        ["sam"]
    );
    assert_eq!(
        ids(UserNotice::Completed {
            by: Some(named("kim"))
        }
        .recipients(&roles)),
        ["sam", "lee"]
    );
    assert_eq!(
        ids(UserNotice::Completed { by: None }.recipients(&roles)),
        ["sam", "kim", "lee"]
    );
}

/// A notification that names its audience keeps it, and one that opens a
/// thread gets its root thread's audience on a team gateway.
#[tokio::test]
async fn a_notification_records_who_it_is_for() {
    use crate::scheduler::notifications::{default_tap, Tap};
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let snapshot = team();
    let root = Uuid::new_v4();
    start_thread(&bus, root, device("sam-phone")).await;

    let thread_tap = Tap::Navigate {
        to: Box::new(crate::scheduler::notifications::NavigateUi {
            target: crate::scheduler::notifications::NavigateTarget::Thread,
            id: Some(root.to_string()),
            ..Default::default()
        }),
    };
    let audience = audience_of_notification(&pool, &thread_tap, None, &snapshot)
        .await
        .unwrap();
    assert_eq!(
        audience.device_ids(),
        Some(vec!["sam-laptop".to_string(), "sam-phone".to_string()])
    );
    assert_eq!(
        audience_of_notification(&pool, &default_tap(None, None), None, &snapshot)
            .await
            .unwrap(),
        Audience::Team
    );
    let alone = UserSnapshot {
        users: vec![user("sam", &[], &["sam-phone"])],
        relayed: vec![],
    };
    assert_eq!(
        audience_of_notification(&pool, &thread_tap, None, &alone)
            .await
            .unwrap(),
        Audience::Team
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Real delivery wiring at each crash boundary, isolated from host credentials.
#[tokio::test]
async fn durable_delivery_recovers_frozen_audiences_without_rewatching() {
    use crate::{llm::ProviderBuildContext, test_support::*};
    use std::sync::Arc;
    const CHILD: &str = "LUCIDOS_TEST_USER_ONLY_DELIVERY_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let home = tempfile::tempdir().unwrap();
        let output = tokio::time::timeout(std::time::Duration::from_secs(120),
            tokio::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "engine::thread_users::tests::durable_delivery_recovers_frozen_audiences_without_rewatching", "--nocapture"])
                .env(CHILD, "1").env("HOME", home.path())
                .env("RUST_MIN_STACK", "16777216")
                .env("GIT_AUTHOR_NAME", "Test User").env("GIT_AUTHOR_EMAIL", "test@example.com")
                .env("GIT_COMMITTER_NAME", "Test User").env("GIT_COMMITTER_EMAIL", "test@example.com")
                .env_remove("LUCIDOS_GATEWAY_DATA").kill_on_drop(true).output()
        ).await.unwrap().unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }
    let (pool, db) = setup_test_db().await;
    let workspace = tempfile::tempdir().unwrap();
    let engine = Arc::new(
        crate::engine::LucidosEngine::new(
            workspace.path().to_path_buf(),
            &test_db_url(&db),
            ProviderBuildContext {
                default_model: "mock".into(),
                model_is_mock: true,
                vertex_project_id: String::new(),
                vertex_location: crate::llm::vertex::location_handle(String::new()),
                vertex_token_cache: None,
                model_registry: Default::default(),
                not_served: Default::default(),
                claude_gateway_access: None,
                chatgpt_plan_access: None,
            },
        )
        .await
        .unwrap(),
    );
    engine.set_self_arc(&engine);
    let bus = &engine.event_bus;
    let mut snapshot = team();
    snapshot.users.push(user("lee", &[], &["lee-phone"]));
    snapshot.users.push(user("pat", &[], &["pat-phone"]));
    seed_device(&pool, "kim-phone", Some("Lucidos-Desktop"), None).await;
    for user_only in [false, true] {
        let thread = Uuid::new_v4();
        start_thread(bus, thread, device("sam-phone")).await;
        record(
            bus,
            thread,
            ThreadEvent::WorkItemDriverTaken,
            Some(device("kim-phone")),
        )
        .await
        .unwrap();
        record(
            bus,
            thread,
            ThreadEvent::WorkItemWatched { user_id: None },
            Some(device("lee-phone")),
        )
        .await
        .unwrap();
        let id = Uuid::new_v4();
        bus.emit(BusEvent::Thread {
            thread_id: thread,
            event: ThreadEvent::UserOnlyMessagePosted {
                text: "Hello".into(),
                user_image_hashes: vec![],
                device_id: None,
                mentioned_user_ids: vec!["kim".into(), "pat".into()],
            },
            meta: EventMeta {
                event_id: Some(id),
                ..EventMeta::NONE
            },
        })
        .await
        .unwrap();
        let notice = UserOnlyMessageNotice {
            thread_id: thread,
            event_id: Some(id),
            text: "Hello".into(),
            mentions: vec!["kim".into(), "pat".into()],
            user_only,
            sender: Some(device("sam-phone")),
        };
        sqlx::query("INSERT INTO user_only_message_notices (event_id, notice) VALUES ($1, $2)")
            .bind(id)
            .bind(sqlx::types::Json(&notice))
            .execute(&pool)
            .await
            .unwrap();
        assert!(
            on_user_only_message(&engine, notice.clone(), &snapshot)
                .await
                .is_err(),
            "native client has not acknowledged"
        );
        // Exercise cached presence while the native dispatch is still incomplete.
        crate::core::DevicePresenceStore::record_visible(&pool, "kim-phone")
            .await
            .unwrap();
        let mut retry_events = bus.subscribe();
        assert!(on_user_only_message(&engine, notice.clone(), &snapshot)
            .await
            .is_err());
        while let Ok(event) = retry_events.try_recv() {
            assert!(
                !matches!(
                    event.typed,
                    BusEvent::System(crate::engine::event_bus::SystemEvent::PresenceCheck { .. })
                ),
                "pending retries must reuse their first presence decision"
            );
        }
        let notification_count = if user_only { 3 } else { 2 };
        let notifications: Vec<(Uuid, Option<Vec<String>>)> =
            sqlx::query_as("SELECT id, audience FROM notifications WHERE thread_id = $1")
                .bind(thread)
                .fetch_all(&pool)
                .await
                .unwrap();
        assert_eq!(
            notifications.len(),
            notification_count,
            "pending driver delivery cannot starve other recipients"
        );
        let recipients: BTreeSet<String> = notifications
            .iter()
            .flat_map(|(_, audience)| audience.clone().unwrap())
            .collect();
        let mut expected = BTreeSet::from(["kim-phone".to_string(), "pat-phone".to_string()]);
        if user_only {
            expected.insert("lee-phone".into());
        }
        assert_eq!(recipients, expected);
        assert_eq!(count(&pool, thread, "WorkItemWatched").await, 2);
        record(
            bus,
            thread,
            ThreadEvent::WorkItemUnwatched,
            Some(device("pat-phone")),
        )
        .await
        .unwrap();
        for (notification, audience) in &notifications {
            if !audience
                .as_ref()
                .is_some_and(|devices| devices.iter().any(|id| id == "kim-phone"))
            {
                continue;
            }
            crate::scheduler::push::record_push_receipt(&pool, *notification, "native:kim-phone")
                .await
                .unwrap();
        }
        // Recovery keeps the original devices even if current topology shrinks.
        let collapsed = UserSnapshot {
            users: vec![user("sam", &[], &["new-device"])],
            relayed: vec![],
        };
        on_user_only_message(&engine, notice.clone(), &collapsed)
            .await
            .unwrap();
        let frozen: sqlx::types::Json<MessagePlan> = sqlx::query_scalar(
            "SELECT deliveries FROM user_only_message_notices WHERE event_id = $1",
        )
        .bind(id)
        .fetch_one(&pool)
        .await
        .unwrap();
        let frozen_devices: BTreeSet<String> = frozen
            .0
            .deliveries
            .into_iter()
            .flat_map(|delivery| delivery.devices)
            .collect();
        assert_eq!(frozen_devices, expected);
        assert_eq!(count(&pool, thread, "WorkItemWatched").await, 2);
        assert!(!roles(&pool, thread, &snapshot)
            .await
            .unwrap()
            .watchers
            .iter()
            .any(|user| user.id == "pat"));
        let incomplete: i64 = sqlx::query_scalar("SELECT count(*) FROM user_only_message_dispatches d JOIN notifications n ON n.id = d.notification_id WHERE n.thread_id = $1 AND NOT d.completed")
            .bind(thread).fetch_one(&pool).await.unwrap();
        assert_eq!(incomplete, 0);
        let mut receiver = bus.subscribe();
        on_user_only_message(&engine, notice, &collapsed)
            .await
            .unwrap();
        assert!(
            receiver.try_recv().is_err(),
            "completed dispatch must emit nothing on replay"
        );
    }
    // A connected browser with no OS transport still receives its first check.
    let browser_thread = Uuid::new_v4();
    start_thread(bus, browser_thread, device("sam-phone")).await;
    let browser_event: Uuid = sqlx::query_scalar(
        "SELECT id FROM events WHERE thread_id = $1 AND event_type = 'MessageReceived'",
    )
    .bind(browser_thread)
    .fetch_one(&pool)
    .await
    .unwrap();
    let browser_notice = UserOnlyMessageNotice {
        thread_id: browser_thread,
        event_id: Some(browser_event),
        text: "Hello browser".into(),
        mentions: vec!["pat".into()],
        user_only: false,
        sender: Some(device("sam-phone")),
    };
    sqlx::query("INSERT INTO user_only_message_notices (event_id, notice) VALUES ($1, $2)")
        .bind(browser_event)
        .bind(sqlx::types::Json(&browser_notice))
        .execute(&pool)
        .await
        .unwrap();
    crate::core::DevicePresenceStore::record_visible(&pool, "pat-phone")
        .await
        .unwrap();
    let mut receiver = bus.subscribe();
    on_user_only_message(&engine, browser_notice.clone(), &snapshot)
        .await
        .unwrap();
    let mut checked_browser = false;
    while let Ok(event) = receiver.try_recv() {
        if let BusEvent::System(crate::engine::event_bus::SystemEvent::PresenceCheck {
            audience,
            ..
        }) = event.typed
        {
            assert_eq!(audience, Some(vec!["pat-phone".into()]));
            checked_browser = true;
        }
    }
    assert!(
        checked_browser,
        "browser-only recipients need the in-app surface"
    );
    on_user_only_message(&engine, browser_notice, &snapshot)
        .await
        .unwrap();
    assert!(
        receiver.try_recv().is_err(),
        "completed browser delivery must not repeat its toast check"
    );
    // Membership may shrink before the first attempt has frozen any plan.
    let shrinking_thread = Uuid::new_v4();
    start_thread(bus, shrinking_thread, device("sam-phone")).await;
    let shrinking_event: Uuid = sqlx::query_scalar(
        "SELECT id FROM events WHERE thread_id = $1 AND event_type = 'MessageReceived'",
    )
    .bind(shrinking_thread)
    .fetch_one(&pool)
    .await
    .unwrap();
    let shrinking_notice = UserOnlyMessageNotice {
        thread_id: shrinking_thread,
        event_id: Some(shrinking_event),
        text: "Hello Kim".into(),
        mentions: vec!["kim".into()],
        user_only: false,
        sender: Some(device("sam-phone")),
    };
    sqlx::query("INSERT INTO user_only_message_notices (event_id, notice) VALUES ($1, $2)")
        .bind(shrinking_event)
        .bind(sqlx::types::Json(&shrinking_notice))
        .execute(&pool)
        .await
        .unwrap();
    let one_user = UserSnapshot {
        users: vec![user("kim", &[], &["kim-phone"])],
        relayed: vec![],
    };
    assert!(
        on_user_only_message(&engine, shrinking_notice.clone(), &one_user)
            .await
            .is_err()
    );
    let notification: Uuid =
        sqlx::query_scalar("SELECT id FROM notifications WHERE thread_id = $1")
            .bind(shrinking_thread)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(crate::scheduler::NotificationStore::mark_read(
        &pool,
        bus,
        &crate::gateway_users::Reader::Team,
        notification,
        Some(device("kim-phone")),
    )
    .await
    .unwrap());
    let mut after_read = bus.subscribe();
    on_user_only_message(&engine, shrinking_notice, &one_user)
        .await
        .unwrap();
    while let Ok(event) = after_read.try_recv() {
        assert!(
            !matches!(
                event.typed,
                BusEvent::System(crate::engine::event_bus::SystemEvent::NativePushRequested { .. })
            ),
            "reading a pending native notification cancels its retry"
        );
    }
    // One user's positive pong must not consume another user's delivery.
    let shared_thread = Uuid::new_v4();
    start_thread(bus, shared_thread, device("sam-phone")).await;
    let shared_event: Uuid = sqlx::query_scalar(
        "SELECT id FROM events WHERE thread_id = $1 AND event_type = 'MessageReceived'",
    )
    .bind(shared_thread)
    .fetch_one(&pool)
    .await
    .unwrap();
    let shared_notice = UserOnlyMessageNotice {
        thread_id: shared_thread,
        event_id: Some(shared_event),
        text: "Read independently".into(),
        mentions: vec!["kim".into(), "lee".into()],
        user_only: false,
        sender: Some(device("sam-phone")),
    };
    sqlx::query("INSERT INTO user_only_message_notices (event_id, notice) VALUES ($1, $2)")
        .bind(shared_event)
        .bind(sqlx::types::Json(&shared_notice))
        .execute(&pool)
        .await
        .unwrap();
    seed_device(&pool, "lee-phone", Some("Lucidos-Desktop"), None).await;
    let lee_push = Arc::new(tokio::sync::Notify::new());
    let kim_push = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let mut events = bus.subscribe();
    let responder = tokio::spawn({
        let engine = engine.clone();
        let lee_push = lee_push.clone();
        let kim_push = kim_push.clone();
        async move {
            while let Ok(event) = events.recv().await {
                match event.typed {
                    BusEvent::System(crate::engine::event_bus::SystemEvent::PresenceCheck {
                        notification_id,
                        audience: Some(devices),
                        ..
                    }) if devices.iter().any(|id| id == "kim-phone") => {
                        engine.presence_tracker.record(
                            crate::api::presence_pong::PresencePongRequest {
                                notification_id,
                                device_id: "kim-phone".into(),
                                outcome: crate::api::presence_pong::PresenceOutcome::Shown,
                            },
                        );
                    }
                    BusEvent::System(
                        crate::engine::event_bus::SystemEvent::NativePushRequested {
                            audience: Some(devices),
                            ..
                        },
                    ) => {
                        if devices.iter().any(|id| id == "lee-phone") {
                            lee_push.notify_one();
                        }
                        if devices.iter().any(|id| id == "kim-phone") {
                            kim_push.store(true, std::sync::atomic::Ordering::SeqCst);
                        }
                    }
                    _ => {}
                }
            }
        }
    });
    let delivered = on_user_only_message(&engine, shared_notice, &snapshot).await;
    assert!(
        delivered.is_err(),
        "Lee still needs native delivery after Kim handles the toast"
    );
    tokio::time::timeout(std::time::Duration::from_secs(2), lee_push.notified())
        .await
        .expect("Lee must receive an OS surface request");
    assert!(
        !kim_push.load(std::sync::atomic::Ordering::SeqCst),
        "Kim already handled the in-app surface"
    );
    responder.abort();
    assert!(responder.await.unwrap_err().is_cancelled());
    drop(engine);
    pool.close().await;
    teardown_test_db(&db).await;
}
