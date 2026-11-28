use super::*;
use crate::engine::thread_events::permission::projection;
use crate::engine::thread_events::EventChannel;
use crate::test_support::{seed_idle_coding_thread, setup_test_db, teardown_test_db};

fn device() -> MessageOrigin {
    MessageOrigin::Device {
        device_id: "phone".into(),
    }
}

#[tokio::test]
async fn host_log_transfer_consent_cannot_be_forged_retargeted_or_replayed() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread = seed_idle_coding_thread(&bus, None).await;
    let other = seed_idle_coding_thread(&bus, None).await;
    let action: Action = serde_json::from_str("\"transfer-host-logs\"").unwrap();
    let scope = "host-log-transfer:immutable-manifest-and-destination";
    let card = RequestCard {
        question: "Send the approved host logs to the named server?".into(),
        header: "Permission",
        action,
        breadth: Some(scope.into()),
        options: menu::options(action, thread, "The host logs were not sent."),
        batch: None,
    };
    let card_id = raise_request(&bus, thread, card, EventChannel::ClaudeCode)
        .await
        .unwrap();
    let request = card_request(&pool, thread, &card_id)
        .await
        .unwrap()
        .unwrap();
    let once = PermissionChoice::Grant {
        lifetime: Lifetime::Once,
        holder: Holder::Thread { thread_id: thread },
        breadth: GrantBreadth::Asked,
    };
    let answer = pick(option_id(&request, &once));
    let agent = MessageOrigin::Agent {
        agent: Default::default(),
    };
    assert_eq!(
        tap(&bus, thread, &card_id, &request, &answer, Some(&agent))
            .await
            .unwrap(),
        Resolution::Ignored
    );
    assert_eq!(granted_count(&pool, thread).await, 0);
    assert!(live_permission(&pool, &user(thread), action, Some(scope))
        .await
        .unwrap()
        .is_none());
    assert!(matches!(
        tap(&bus, thread, &card_id, &request, &answer, Some(&device()))
            .await
            .unwrap(),
        Resolution::Granted(_)
    ));
    let (reopened, _reopened_rx) = EventBus::new(pool.clone());
    assert!(live_permission(&pool, &user(other), action, Some(scope))
        .await
        .unwrap()
        .is_none());
    for changed in ["another-source-manifest", "another-destination"] {
        assert!(live_permission(&pool, &user(thread), action, Some(changed))
            .await
            .unwrap()
            .is_none());
    }
    let permission = live_permission(&pool, &user(thread), action, Some(scope))
        .await
        .unwrap()
        .expect("the owner's exact permission survives reopening");
    assert!(spend(&reopened, &permission).await);
    assert!(!spend(&reopened, &permission).await);
    assert!(live_permission(&pool, &user(thread), action, Some(scope))
        .await
        .unwrap()
        .is_none());
    teardown_test_db(&db).await;
}

fn option(label: &str, choice: PermissionChoice) -> RequestOption {
    RequestOption {
        label: label.into(),
        description: format!("{label}."),
        choice,
    }
}

fn grant(lifetime: Lifetime, holder: Holder) -> PermissionChoice {
    PermissionChoice::grant(lifetime, holder)
}

async fn raise(
    bus: &EventBus,
    thread: Uuid,
    action: Action,
    options: Vec<RequestOption>,
) -> (String, PermissionRequest) {
    let card = RequestCard {
        question: "Allow it?".into(),
        header: "Permission",
        action,
        breadth: None,
        options,
        batch: None,
    };
    let id = raise_request(bus, thread, card, EventChannel::ClaudeCode)
        .await
        .unwrap();
    let request = card_request(bus.pool(), thread, &id)
        .await
        .unwrap()
        .expect("the card carries its request");
    (id, request)
}

fn option_id(request: &PermissionRequest, wanted: &PermissionChoice) -> String {
    request
        .choices
        .iter()
        .find(|(_, choice)| *choice == wanted)
        .map(|(id, _)| id.clone())
        .expect("the request offers that choice")
}

fn pick(id: String) -> AnswerKind {
    AnswerKind::Selected { option_id: id }
}

fn user(thread: Uuid) -> PermissionUser {
    PermissionUser {
        thread_id: Some(thread),
        trigger_id: None,
    }
}

async fn tap(
    bus: &EventBus,
    thread: Uuid,
    card: &str,
    request: &PermissionRequest,
    answer: &AnswerKind,
    actor: Option<&MessageOrigin>,
) -> Result<Resolution, BoxError> {
    resolve_answer(bus, thread, card, request, answer, actor, PickedBy::Tap).await
}

async fn answer_and_tap(
    bus: &EventBus,
    thread: Uuid,
    card: &str,
    request: &PermissionRequest,
    answer: AnswerKind,
) {
    bus.emit(BusEvent::Thread {
        thread_id: thread,
        event: ThreadEvent::UserQuestionAnswered {
            compose: None,
            tool_use_id: card.into(),
            answer: answer.clone(),
            quiet: true,
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();
    tap(bus, thread, card, request, &answer, Some(&device()))
        .await
        .unwrap();
}

async fn granted_count(pool: &sqlx::PgPool, thread: Uuid) -> i64 {
    sqlx::query_scalar(
        "SELECT COUNT(*) FROM events WHERE thread_id = $1 AND event_type = 'PermissionGranted'",
    )
    .bind(thread)
    .fetch_one(pool)
    .await
    .unwrap()
}

#[tokio::test]
async fn a_once_grant_covers_one_action() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread = seed_idle_coding_thread(&bus, None).await;
    let once = grant(Lifetime::Once, Holder::Thread { thread_id: thread });
    let (card, request) = raise(
        &bus,
        thread,
        Action::Apply,
        vec![
            option("Allow once", once.clone()),
            option("Deny", PermissionChoice::Refuse),
        ],
    )
    .await;

    let resolution = tap(
        &bus,
        thread,
        &card,
        &request,
        &pick(option_id(&request, &once)),
        Some(&device()),
    )
    .await
    .unwrap();
    assert!(matches!(resolution, Resolution::Granted(_)));

    let permission = live_permission(&pool, &user(thread), Action::Apply, None)
        .await
        .unwrap()
        .expect("the grant is live");
    assert!(spend(&bus, &permission).await);
    assert!(!spend(&bus, &permission).await, "a once grant spends once");
    assert_eq!(
        live_permission(&pool, &user(thread), Action::Apply, None)
            .await
            .unwrap(),
        None
    );
    let ended: Option<String> =
        sqlx::query_scalar("SELECT end_reason FROM permissions WHERE permission_id = $1")
            .bind(permission.permission_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(ended.as_deref(), Some("spent"));
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn a_revoked_grant_never_spends() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread = seed_idle_coding_thread(&bus, None).await;
    let once = grant(Lifetime::Once, Holder::Thread { thread_id: thread });
    let (card, request) = raise(
        &bus,
        thread,
        Action::Apply,
        vec![
            option("Allow once", once.clone()),
            option("Deny", PermissionChoice::Refuse),
        ],
    )
    .await;
    let picked = pick(option_id(&request, &once));
    tap(&bus, thread, &card, &request, &picked, Some(&device()))
        .await
        .unwrap();
    let permission = live_permission(&pool, &user(thread), Action::Apply, None)
        .await
        .unwrap()
        .expect("the grant is live");

    let revoked = revoke(&bus, permission.permission_id, Some(device()))
        .await
        .unwrap();
    assert_eq!(revoked, RevokeOutcome::Revoked);
    assert!(
        !spend(&bus, &permission).await,
        "a revoked grant runs nothing"
    );
    let (ended, spends): (Option<String>, i64) = sqlx::query_as(
        "SELECT end_reason, (SELECT count(*) FROM events WHERE event_type = 'PermissionSpent') \
         FROM permissions WHERE permission_id = $1",
    )
    .bind(permission.permission_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!((ended.as_deref(), spends), (Some("revoked"), 0));
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn only_an_owner_pick_of_a_grant_grants() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread = seed_idle_coding_thread(&bus, None).await;
    let standing = grant(Lifetime::Standing, Holder::Workspace);
    let (card, request) = raise(
        &bus,
        thread,
        Action::Archive,
        vec![
            option("Always allow", standing.clone()),
            option("Deny", PermissionChoice::Refuse),
        ],
    )
    .await;
    let agent = MessageOrigin::Agent {
        agent: Default::default(),
    };
    let granting = pick(option_id(&request, &standing));

    let by_agent = tap(&bus, thread, &card, &request, &granting, Some(&agent))
        .await
        .unwrap();
    let by_nobody = tap(&bus, thread, &card, &request, &granting, None)
        .await
        .unwrap();
    let refused = tap(
        &bus,
        thread,
        &card,
        &request,
        &pick(option_id(&request, &PermissionChoice::Refuse)),
        Some(&device()),
    )
    .await
    .unwrap();
    let typed = tap(
        &bus,
        thread,
        &card,
        &request,
        &AnswerKind::FreeText {
            text: "yes, always".into(),
            image_hashes: Vec::new(),
        },
        Some(&device()),
    )
    .await
    .unwrap();
    let unknown = tap(
        &bus,
        thread,
        &card,
        &request,
        &pick("opt-99".into()),
        Some(&device()),
    )
    .await
    .unwrap();

    assert_eq!(by_agent, Resolution::Ignored);
    assert_eq!(by_nobody, Resolution::Ignored);
    assert_eq!(refused, Resolution::Refused);
    assert_eq!(typed, Resolution::Ignored);
    assert_eq!(unknown, Resolution::Ignored);
    assert_eq!(granted_count(&pool, thread).await, 0);
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn a_permission_covers_exactly_its_scope() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread = seed_idle_coding_thread(&bus, None).await;
    let other = seed_idle_coding_thread(&bus, None).await;

    for lifetime in [Lifetime::Once, Lifetime::ThisThread] {
        let choice = grant(lifetime, Holder::Thread { thread_id: thread });
        let (card, request) = raise(
            &bus,
            thread,
            Action::Apply,
            vec![option("Allow", choice.clone())],
        )
        .await;
        answer_and_tap(
            &bus,
            thread,
            &card,
            &request,
            pick(option_id(&request, &choice)),
        )
        .await;
    }

    let used = live_permission(&pool, &user(thread), Action::Apply, None)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(used.lifetime, "this_thread");
    assert!(spend(&bus, &used).await, "a lasting grant needs no spend");
    assert!(
        live_permission(&pool, &user(thread), Action::Apply, None)
            .await
            .unwrap()
            .is_some(),
        "both grants are still live"
    );
    assert_eq!(
        live_permission(&pool, &user(other), Action::Apply, None)
            .await
            .unwrap(),
        None,
        "another thread holds nothing"
    );
    assert_eq!(
        live_permission(&pool, &user(thread), Action::Discard, None)
            .await
            .unwrap(),
        None,
        "another action is not covered"
    );
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn breadth_and_holders_narrow_a_grant() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread = seed_idle_coding_thread(&bus, None).await;
    let other = seed_idle_coding_thread(&bus, None).await;

    let card = RequestCard {
        question: "Allow git commands?".into(),
        header: "Permission",
        action: Action::Continue,
        breadth: Some("git".into()),
        options: vec![option(
            "Always allow",
            grant(Lifetime::Standing, Holder::Workspace),
        )],
        batch: None,
    };
    let id = raise_request(&bus, thread, card, EventChannel::ClaudeCode)
        .await
        .unwrap();
    let request = card_request(&pool, thread, &id).await.unwrap().unwrap();
    let opt = request.choices.keys().next().unwrap().clone();
    answer_and_tap(&bus, thread, &id, &request, pick(opt)).await;

    let trigger_choice = grant(
        Lifetime::Standing,
        Holder::Trigger {
            trigger_id: "nightly".into(),
        },
    );
    let (card, request) = raise(
        &bus,
        thread,
        Action::Cancel,
        vec![option("Allow", trigger_choice.clone())],
    )
    .await;
    answer_and_tap(
        &bus,
        thread,
        &card,
        &request,
        pick(option_id(&request, &trigger_choice)),
    )
    .await;

    let git = live_permission(&pool, &user(other), Action::Continue, Some("git"))
        .await
        .unwrap();
    assert!(git.is_some(), "a workspace grant reaches every thread");
    assert_eq!(
        live_permission(&pool, &user(other), Action::Continue, Some("rm"))
            .await
            .unwrap(),
        None,
        "a breadth covers only itself"
    );
    assert_eq!(
        live_permission(&pool, &user(thread), Action::Cancel, None)
            .await
            .unwrap(),
        None,
        "a trigger grant is not the thread's"
    );
    let nightly = PermissionUser {
        thread_id: Some(other),
        trigger_id: Some("nightly".into()),
    };
    assert!(live_permission(&pool, &nightly, Action::Cancel, None)
        .await
        .unwrap()
        .is_some());
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn a_revoke_ends_a_standing_grant() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread = seed_idle_coding_thread(&bus, None).await;
    let standing = grant(Lifetime::Standing, Holder::Workspace);
    let (card, request) = raise(
        &bus,
        thread,
        Action::Archive,
        vec![option("Always allow", standing.clone())],
    )
    .await;
    let Resolution::Granted(id) = tap(
        &bus,
        thread,
        &card,
        &request,
        &pick(option_id(&request, &standing)),
        Some(&device()),
    )
    .await
    .unwrap() else {
        panic!("the pick grants");
    };

    assert_eq!(
        revoke(&bus, id, Some(device())).await.unwrap(),
        RevokeOutcome::Revoked
    );
    assert_eq!(
        revoke(&bus, id, Some(device())).await.unwrap(),
        RevokeOutcome::AlreadyEnded
    );
    assert_eq!(
        revoke(&bus, Uuid::new_v4(), None).await.unwrap(),
        RevokeOutcome::NotFound
    );
    assert_eq!(
        live_permission(&pool, &user(thread), Action::Archive, None)
            .await
            .unwrap(),
        None
    );
    let live = list(&pool, true).await.unwrap();
    let all = list(&pool, false).await.unwrap();
    assert!(live.is_empty());
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].end_reason.as_deref(), Some("revoked"));
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn a_landed_plan_ends_its_grants() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread = seed_idle_coding_thread(&bus, None).await;
    let until = grant(
        Lifetime::UntilPlanLands,
        Holder::Thread { thread_id: thread },
    );
    let (card, request) = raise(
        &bus,
        thread,
        Action::Apply,
        vec![option("Allow", until.clone())],
    )
    .await;
    tap(
        &bus,
        thread,
        &card,
        &request,
        &pick(option_id(&request, &until)),
        Some(&device()),
    )
    .await
    .unwrap();

    let mut tx = pool.begin().await.unwrap();
    projection::end_until_plan_lands(&mut tx, thread)
        .await
        .unwrap();
    tx.commit().await.unwrap();

    let row = &list(&pool, false).await.unwrap()[0];
    assert_eq!(row.end_reason.as_deref(), Some("plan_landed"));
    teardown_test_db(&db).await;
}

#[test]
fn a_spoken_pick_names_its_transcript_row_only_when_it_has_one() {
    let row = Uuid::new_v4();
    let judged = PickedBy::Spoken {
        message_event_id: Some(row),
    };
    let talker = PickedBy::Spoken {
        message_event_id: None,
    };
    assert_eq!(
        serde_json::to_value(&judged).unwrap(),
        serde_json::json!({ "kind": "spoken", "message_event_id": row })
    );
    assert_eq!(
        serde_json::to_value(&talker).unwrap(),
        serde_json::json!({ "kind": "spoken" })
    );
    let stored: PickedBy =
        serde_json::from_value(serde_json::json!({ "kind": "spoken", "message_event_id": row }))
            .unwrap();
    assert_eq!(stored, judged);
}

#[test]
fn a_target_title_stays_on_one_line_and_inert() {
    assert_eq!(one_line("a\n\n**b**"), "a \\*\\*b\\*\\*");
    let long = "x".repeat(TITLE_MAX_CHARS + 10);
    assert!(one_line(&long).ends_with('\u{2026}'));
}
