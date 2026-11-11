use super::*;
use crate::engine::thread_events::{
    ActorMode, AnswerKind, EngineReason, EventChannel, MessageOrigin, ThreadEvent,
};
use crate::gateway_operators::Operator;
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

fn person(id: &str, emails: &[&str], devices: &[&str]) -> Operator {
    Operator {
        id: id.into(),
        name: id.to_uppercase(),
        emails: emails.iter().map(|e| e.to_string()).collect(),
        device_ids: devices.iter().map(|d| d.to_string()).collect(),
    }
}

/// Sam holds two devices and an email; Kim holds one device.
fn team() -> OperatorSnapshot {
    OperatorSnapshot {
        operators: vec![
            person("sam", &["sam@example.com"], &["sam-phone", "sam-laptop"]),
            person("kim", &["kim@example.com"], &["kim-phone"]),
        ],
        relayed: vec![],
    }
}

fn event(event_type: &str, actor: MessageOrigin) -> RoleEvent {
    RoleEvent {
        event_type: event_type.into(),
        actor: Some(actor),
        operator_id: None,
    }
}

/// A person mention recorded by `actor`, naming `operator_id`.
fn mentioned(operator_id: &str, actor: MessageOrigin) -> RoleEvent {
    RoleEvent {
        operator_id: Some(operator_id.into()),
        ..event("WorkItemWatched", actor)
    }
}

fn driver_id(roles: &ThreadPeople) -> Option<&str> {
    roles.driver.as_ref().map(|d| d.id.as_str())
}

#[test]
fn a_person_started_root_thread_starts_with_its_requester_and_a_fire_with_none() {
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
fn a_watch_is_per_person_so_any_of_their_devices_can_end_it() {
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

/// A mention names the person it made a watcher, not its sender. That person
/// leaves from any device, and a removed operator watches nothing.
#[test]
fn a_mention_makes_the_named_person_a_watcher() {
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

/// A people-only message reaches the mentioned people, the driver and the
/// watchers. A woken one reaches only the mentioned. Never the sender.
#[test]
fn a_people_message_reaches_who_it_is_for_never_its_sender() {
    let roles = ThreadPeople {
        driver: Some(named("sam")),
        watchers: vec![named("kim"), named("lee")],
    };
    let mentions = vec!["kim".to_string(), "ana".to_string()];
    assert_eq!(
        people_message_recipients(&roles, &mentions, true, Some(&named("kim"))),
        ["ana", "sam", "lee"]
    );
    assert_eq!(
        people_message_recipients(&roles, &mentions, false, Some(&named("sam"))),
        ["kim", "ana"]
    );
    assert_eq!(
        people_message_recipients(&roles, &[], true, None),
        ["sam", "kim", "lee"]
    );
}

/// Events store device ids, so moving a device re-attributes the role it took.
#[test]
fn moving_a_device_to_another_person_moves_the_role_it_took() {
    let taken = [event("WorkItemDriverTaken", device("kim-phone"))];
    assert_eq!(driver_id(&fold(None, &taken, &team())), Some("kim"));

    let mut moved = team();
    moved.operators[1].device_ids.clear();
    moved.operators[0].device_ids.push("kim-phone".into());
    assert_eq!(driver_id(&fold(None, &taken, &moved)), Some("sam"));
}

#[test]
fn a_gate_always_reaches_someone() {
    let snapshot = team();
    let no_driver = ThreadPeople::default();
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
    deviceless.operators[0].device_ids.clear();
    let driven_alone = fold(
        None,
        &[event("WorkItemDriverTaken", fire_by("sam@example.com"))],
        &deviceless,
    );
    assert_eq!(audience(&driven_alone, false, &deviceless), Audience::Team);
}

#[test]
fn a_gateway_with_one_person_pushes_everyone() {
    let solo = OperatorSnapshot {
        operators: vec![person("sam", &[], &["sam-phone"])],
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
/// an operator, and a driver answering again records nothing.
#[tokio::test]
async fn answering_a_gate_takes_the_driver_role_for_a_known_person_only() {
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

/// A thread started with a person has its sender as requester, so it has a
/// driver like any person-started root thread.
#[tokio::test]
async fn a_thread_started_with_a_person_is_driven_by_its_sender() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let snapshot = team();
    let root = Uuid::new_v4();
    bus.emit(BusEvent::Thread {
        thread_id: root,
        event: ThreadEvent::PeopleMessagePosted {
            text: "Kim, can you look?".into(),
            user_image_hashes: vec![],
            device_id: Some("sam-phone".into()),
            mentioned_operator_ids: vec!["kim".into()],
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

/// A mention records one watch per new person. The driver, a watcher and an
/// unknown id record nothing.
#[tokio::test]
async fn a_mention_records_a_watch_only_for_a_person_without_a_role() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let snapshot = team();
    let root = Uuid::new_v4();
    start_thread(&bus, root, device("sam-phone")).await;
    let mentions = vec!["sam".to_string(), "kim".to_string(), "lee".to_string()];
    let sender = device("sam-phone");

    watch_mentioned(&pool, &bus, root, &mentions, Some(&sender), &snapshot)
        .await
        .unwrap();
    watch_mentioned(&pool, &bus, root, &mentions, Some(&sender), &snapshot)
        .await
        .unwrap();
    assert_eq!(count(&pool, root, "WorkItemWatched").await, 1);
    let roles = roles(&pool, root, &snapshot).await.unwrap();
    assert_eq!(driver_id(&roles), Some("sam"));
    let ids: Vec<&str> = roles.watchers.iter().map(|w| w.id.as_str()).collect();
    assert_eq!(ids, vec!["kim"]);

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
        ThreadEvent::WorkItemWatched { operator_id: None },
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
        ThreadEvent::WorkItemWatched { operator_id: None },
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

    let kim = snapshot.operator_of_device("kim-phone");
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

fn named(id: &str) -> OperatorRef {
    OperatorRef {
        id: id.into(),
        name: id.to_uppercase(),
    }
}

/// A takeover tells the person displaced. A finished root thread tells its
/// driver and watchers once each, never the person who finished it.
#[test]
fn a_notice_reaches_the_people_it_concerns() {
    let takeover = Takeover {
        from: named("sam"),
        to: named("kim"),
    };
    let roles = ThreadPeople {
        driver: Some(named("sam")),
        watchers: vec![named("kim"), named("sam"), named("lee")],
    };
    let ids = |people: Vec<OperatorRef>| people.into_iter().map(|p| p.id).collect::<Vec<_>>();

    assert_eq!(
        ids(PeopleNotice::TakenOver(takeover.clone()).recipients(&roles)),
        ["sam"]
    );
    assert_eq!(
        ids(PeopleNotice::GateAnswered(takeover).recipients(&roles)),
        ["sam"]
    );
    assert_eq!(
        ids(PeopleNotice::Completed {
            by: Some(named("kim"))
        }
        .recipients(&roles)),
        ["sam", "lee"]
    );
    assert_eq!(
        ids(PeopleNotice::Completed { by: None }.recipients(&roles)),
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
    let alone = OperatorSnapshot {
        operators: vec![person("sam", &[], &["sam-phone"])],
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
